//! `omakeel probe` end to end: a pseudo-terminal pair stands in for a GPS
//! on a serial port, and a local listener for a TCP stream.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic is how an integration test fails"
)]

use omakeel::{
    hub::{self, Config},
    probe::{Budget, Report, Target, probe_all},
    source::Spec,
};
use std::{
    fs::File,
    io::Write,
    net::TcpListener,
    os::unix::{
        fs::PermissionsExt,
        io::{AsRawFd, FromRawFd},
    },
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const SAIL: &str = include_str!("fixtures/berkeley-marina.nmea");

const BUDGET: Budget = Budget {
    rate: Duration::from_millis(250),
    extended: Duration::from_millis(600),
};

/// The sail's sentences as lines, a GPS fix and AIS among them.
fn sentences() -> Vec<String> {
    SAIL.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once(' '))
        .map(|(_, sentence)| format!("{sentence}\r\n"))
        .collect()
}

struct Pty {
    master: File,
    slave: File,
    path: PathBuf,
}

fn pty() -> Pty {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: both out-pointers are valid for the call, and null for the
    // name, termios and window size asks `openpty` for the defaults.
    let made = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(made, 0, "openpty");
    // SAFETY: `openpty` just returned these two descriptors, and nothing
    // else owns them.
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    // SAFETY: `master` is open; F_GETFL and F_SETFL take and return ints.
    // A full pty buffer then fails a write instead of blocking the writer.
    unsafe {
        let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
        assert_eq!(
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK),
            0
        );
    }
    let path = std::fs::read_link(format!("/proc/self/fd/{}", slave.as_raw_fd())).unwrap();
    Pty {
        master,
        slave,
        path,
    }
}

/// Plays the sail into the master side over and over until dropped.
struct Talker {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Talker {
    fn start(master: &File) -> Talker {
        let mut master = master.try_clone().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::spawn(move || {
            let lines = sentences();
            // Starts at the first fix, so a short budget still hears one.
            let start = lines.iter().position(|l| l.contains(",A,")).unwrap();
            for line in lines.iter().skip(start).cycle() {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let _ = master.write_all(line.as_bytes());
                thread::sleep(Duration::from_millis(10));
            }
        });
        Talker {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Talker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn lines(targets: &[&str], budget: Budget) -> Vec<String> {
    let targets: Vec<Target> = targets.iter().map(|t| t.parse().unwrap()).collect();
    let mut out = Vec::new();
    probe_all(&targets, budget, |report: Report| {
        out.push(report.to_string());
    });
    out
}

fn exclusive(file: &File, on: bool) {
    let request = if on { libc::TIOCEXCL } else { libc::TIOCNXCL };
    // SAFETY: `file` is open, and both requests take no argument.
    assert_eq!(unsafe { libc::ioctl(file.as_raw_fd(), request) }, 0);
}

fn termios_of(file: &File) -> libc::termios {
    // SAFETY: `file` is open, and `termios` is a plain C struct that
    // `tcgetattr` fills in before anything reads it.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(file.as_raw_fd(), &mut termios), 0);
        termios
    }
}

#[test]
fn a_gps_on_a_serial_port_is_found_by_sweeping() {
    let pty = pty();
    let _talker = Talker::start(&pty.master);
    let path = pty.path.display().to_string();
    let out = lines(&[&path], BUDGET);
    // A pty has no wire speed, so the first rate in the sweep hears it.
    let prefix = format!("serial:{path}:4800  ok  fix=");
    assert!(out[0].starts_with(&prefix), "{out:?}");
    assert!(out[0].contains(" at=37.8"), "{out:?}");
}

#[test]
fn an_explicit_baud_is_the_only_one_tried_and_is_reported() {
    let pty = pty();
    let _talker = Talker::start(&pty.master);
    let path = pty.path.display().to_string();
    let spec = format!("serial:{path}:38400");
    let out = lines(&[&spec], BUDGET);
    assert!(out[0].starts_with(&format!("{spec}  ok  fix=")), "{out:?}");
}

#[test]
fn a_port_that_says_nothing_is_silent() {
    let pty = pty();
    let path = pty.path.display().to_string();
    assert_eq!(
        lines(&[&format!("serial:{path}:9600")], BUDGET),
        [format!("serial:{path}:9600  silent")]
    );
}

#[test]
fn the_port_is_put_back_the_way_it_was_found() {
    let pty = pty();
    let before = termios_of(&pty.slave);
    let _talker = Talker::start(&pty.master);
    let path = pty.path.display().to_string();
    assert!(lines(&[&path], BUDGET)[0].contains("  ok  "));
    let after = termios_of(&pty.slave);
    assert_eq!(
        (after.c_iflag, after.c_oflag, after.c_cflag, after.c_lflag),
        (
            before.c_iflag,
            before.c_oflag,
            before.c_cflag,
            before.c_lflag
        )
    );
    assert_eq!(after.c_cc, before.c_cc);
    assert_eq!(
        // SAFETY: `after` is a termios that `tcgetattr` filled in.
        unsafe { libc::cfgetispeed(&after) },
        // SAFETY: `before` is a termios that `tcgetattr` filled in.
        unsafe { libc::cfgetispeed(&before) }
    );
}

#[test]
fn a_port_held_exclusively_is_busy_and_free_again_after_a_probe() {
    let pty = pty();
    let path = pty.path.display().to_string();
    exclusive(&pty.slave, true);
    assert_eq!(lines(&[&path], BUDGET), [format!("serial:{path}  busy")]);
    exclusive(&pty.slave, false);
    let _talker = Talker::start(&pty.master);
    assert!(lines(&[&path], BUDGET)[0].contains("  ok  "));
    // The probe's own exclusive hold is gone, so a second probe is not busy.
    assert!(lines(&[&path], BUDGET)[0].contains("  ok  "));
}

#[test]
fn a_missing_path_and_an_unreadable_one_are_told_apart() {
    let dir = std::env::temp_dir().join(format!("omakeel-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let locked = dir.join("locked");
    std::fs::write(&locked, "").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let missing = dir.join("gone");
    let (locked, missing) = (locked.display().to_string(), missing.display().to_string());
    assert_eq!(
        lines(&[&locked, &missing], BUDGET),
        [
            format!("serial:{locked}  denied"),
            format!("serial:{missing}  missing")
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_tcp_stream_is_listened_to_and_a_closed_port_is_unreachable() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        for line in sentences().iter().cycle().take(30) {
            if stream.write_all(line.as_bytes()).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    let live = format!("tcp:127.0.0.1:{port}");
    let out = lines(&[&live], BUDGET);
    assert!(out[0].starts_with(&format!("{live}  ok  fix=")), "{out:?}");
    server.join().unwrap();

    // The listener is gone, so nothing answers on that port now.
    assert_eq!(
        lines(&[&live], BUDGET),
        [format!("{live}  unreachable  connection refused")]
    );
}

fn serial_hub(name: &str, pty: &Pty) -> (PathBuf, tokio::task::JoinHandle<std::io::Result<()>>) {
    let dir = std::env::temp_dir().join(format!("omakeel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("keel.sock");
    let config = Config {
        sources: vec![Spec::Serial {
            path: pty.path.clone(),
            baud: 4800,
        }],
        socket: socket.clone(),
        record: None,
    };
    (socket, tokio::spawn(hub::run(config)))
}

#[tokio::test]
async fn a_port_the_running_hub_reads_is_busy_to_a_probe_and_left_alone() {
    let pty = pty();
    let _talker = Talker::start(&pty.master);
    let path = pty.path.display().to_string();
    let (_socket, hub) = serial_hub("probe-hub", &pty);
    let mut out = Vec::new();
    for _ in 0..100 {
        out = lines(&[&path], BUDGET);
        if out[0].ends_with("  busy") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(out, [format!("serial:{path}  busy")]);
    // A probe at another baud must not retune what the hub is reading.
    assert_eq!(
        lines(&[&format!("serial:{path}:9600")], BUDGET),
        [format!("serial:{path}:9600  busy")]
    );
    let after = termios_of(&pty.slave);
    assert_eq!(
        // SAFETY: `after` is a termios that `tcgetattr` filled in.
        unsafe { libc::cfgetispeed(&after) },
        libc::B4800
    );
    assert!(!hub.is_finished());
    hub.abort();
}

#[tokio::test]
async fn a_hub_whose_port_is_held_says_so() {
    let pty = pty();
    exclusive(&pty.slave, true);
    let (socket, hub) = serial_hub("hub-busy", &pty);
    let mut state = String::new();
    for _ in 0..100 {
        if let Ok(text) = connect_and_read(&socket).await
            && text.contains("busy: another program has the port open")
        {
            state = text;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(state.contains("\"status\":\"error\""), "{state}");
    hub.abort();
}

async fn connect_and_read(socket: &std::path::Path) -> std::io::Result<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let stream = tokio::net::UnixStream::connect(socket).await?;
    let mut lines = BufReader::new(stream).lines();
    let mut text = String::new();
    while let Ok(Some(line)) = tokio::time::timeout(Duration::from_millis(100), lines.next_line())
        .await
        .unwrap_or(Ok(None))
    {
        text.push_str(&line);
    }
    Ok(text)
}
