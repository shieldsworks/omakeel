//! Where sentences come from: a serial port, a TCP stream, or a recording.

use crate::protocol::SourceStatus;
use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Read},
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    str::FromStr,
    thread,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader as AsyncBufReader},
    net::TcpStream,
    sync::mpsc,
    time::{Duration, Instant, sleep, sleep_until},
};

/// A line longer than this isn't NMEA, so it's read in pieces that fail
/// their checksums. A whole sentence at the end of one still counts, the way
/// NMEA readers resynchronize on `$`.
const MAX_LINE: u64 = 1024;
/// A recording's line: a timestamp, a space, then a line up to `MAX_LINE`.
const MAX_RECORD: u64 = 2 * MAX_LINE;
/// Wait between attempts to reopen a device or reconnect a stream.
const RETRY: Duration = Duration::from_secs(2);
/// NMEA 0183's standard rate, and the GlobalSat BU-353-S4's.
const DEFAULT_BAUD: u32 = 4800;

pub enum Event {
    Line {
        source: usize,
        line: String,
    },
    Status {
        source: usize,
        status: SourceStatus,
        message: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Spec {
    Serial { path: PathBuf, baud: u32 },
    Tcp { address: String },
    Replay { path: PathBuf },
}

impl FromStr for Spec {
    type Err = String;

    fn from_str(spec: &str) -> Result<Spec, String> {
        let bad = || {
            format!(
                "'{spec}' is not a source; use serial:PATH[:BAUD], tcp:HOST:PORT or replay:FILE"
            )
        };
        let (kind, rest) = spec.split_once(':').ok_or_else(bad)?;
        if rest.is_empty() {
            return Err(bad());
        }
        match kind {
            "serial" => {
                let (path, baud) = match rest.rsplit_once(':') {
                    Some((path, baud))
                        if !path.is_empty()
                            && !baud.is_empty()
                            && baud.bytes().all(|b| b.is_ascii_digit()) =>
                    {
                        (path, baud.parse().map_err(|_| bad())?)
                    }
                    _ => (rest, DEFAULT_BAUD),
                };
                if speed(baud).is_none() {
                    return Err(format!(
                        "{baud} baud isn't supported; use 4800, 9600, 19200, 38400, 57600 or 115200"
                    ));
                }
                Ok(Spec::Serial {
                    path: path.into(),
                    baud,
                })
            }
            "tcp" if rest.contains(':') => Ok(Spec::Tcp {
                address: rest.to_string(),
            }),
            "replay" => Ok(Spec::Replay { path: rest.into() }),
            _ => Err(bad()),
        }
    }
}

impl fmt::Display for Spec {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Spec::Serial { path, baud } => write!(f, "serial:{}:{baud}", path.display()),
            Spec::Tcp { address } => write!(f, "tcp:{address}"),
            Spec::Replay { path } => write!(f, "replay:{}", path.display()),
        }
    }
}

/// Starts reading `spec`, sending each line and status change as `source`.
/// TCP and replay stop as soon as the hub is gone. A serial port's thread
/// stops at its next line, or when the process exits.
pub fn spawn(source: usize, spec: Spec, events: mpsc::Sender<Event>) {
    match spec {
        Spec::Serial { path, baud } => {
            thread::spawn(move || serial(source, &path, baud, &events));
        }
        Spec::Tcp { address } => {
            tokio::spawn(tcp(source, address, events));
        }
        Spec::Replay { path } => {
            tokio::spawn(replay(source, path, events));
        }
    }
}

/// A received line, trimmed, with invalid UTF-8 replaced; blank lines are none.
fn line(source: usize, bytes: &[u8]) -> Option<Event> {
    let line = String::from_utf8_lossy(bytes).trim().to_string();
    (!line.is_empty()).then_some(Event::Line { source, line })
}

fn failed(source: usize, message: String) -> Event {
    Event::Status {
        source,
        status: SourceStatus::Error,
        message: Some(message),
    }
}

/// Reads a serial device on its own thread, reopening it after an unplug.
fn serial(source: usize, path: &Path, baud: u32, events: &mpsc::Sender<Event>) {
    loop {
        let result = open_serial(path, baud).and_then(|device| {
            let mut reader = BufReader::new(device);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                if (&mut reader).take(MAX_LINE).read_until(b'\n', &mut buf)? == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "device closed",
                    ));
                }
                if let Some(event) = line(source, &buf)
                    && events.blocking_send(event).is_err()
                {
                    return Ok(()); // the hub is gone
                }
            }
        });
        let Err(e) = result else { return };
        if events
            .blocking_send(failed(source, format!("{}: {e}", path.display())))
            .is_err()
        {
            return;
        }
        thread::sleep(RETRY);
    }
}

fn speed(baud: u32) -> Option<libc::speed_t> {
    Some(match baud {
        4800 => libc::B4800,
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        _ => return None,
    })
}

/// Opens a serial device raw at `baud`, as a GPS or AIS receiver expects.
fn open_serial(path: &Path, baud: u32) -> io::Result<File> {
    let speed = speed(baud).ok_or_else(|| io::Error::other("unsupported baud rate"))?;
    // Non-blocking, so a port that would wait for carrier detect can't hang
    // the open. A GPS has no modem lines; CLOCAL below says so, and then
    // reads go back to blocking.
    let device = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(path)?;
    let fd = device.as_raw_fd();
    // SAFETY: `fd` is open for the life of `device`, and `termios` is a
    // plain C struct that `tcgetattr` fills in before anything reads it.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut termios) != 0 {
            return Err(io::Error::last_os_error());
        }
        libc::cfmakeraw(&mut termios);
        libc::cfsetispeed(&mut termios, speed);
        libc::cfsetospeed(&mut termios, speed);
        termios.c_cflag |= libc::CLOCAL | libc::CREAD;
        termios.c_cc[libc::VMIN] = 1;
        termios.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
            return Err(io::Error::last_os_error());
        }
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(device)
}

/// Reads an NMEA stream over TCP, reconnecting when it drops.
async fn tcp(source: usize, address: String, events: mpsc::Sender<Event>) {
    loop {
        let connected = tokio::select! {
            connected = TcpStream::connect(&address) => connected,
            () = events.closed() => return,
        };
        let error = match connected {
            Ok(stream) => {
                let mut reader = AsyncBufReader::new(stream);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    let mut limited = (&mut reader).take(MAX_LINE);
                    let read = tokio::select! {
                        read = limited.read_until(b'\n', &mut buf) => read,
                        () = events.closed() => return,
                    };
                    match read {
                        Ok(0) => break "connection closed".to_string(),
                        Ok(_) => {
                            if let Some(event) = line(source, &buf)
                                && events.send(event).await.is_err()
                            {
                                return;
                            }
                        }
                        Err(e) => break e.to_string(),
                    }
                }
            }
            Err(e) => e.to_string(),
        };
        if events
            .send(failed(source, format!("{address}: {error}")))
            .await
            .is_err()
        {
            return;
        }
        tokio::select! {
            () = sleep(RETRY) => {}
            () = events.closed() => return,
        }
    }
}

/// Plays a recording (`docs/protocol.md`, recordings) line by line with its
/// original timing, then reports the source ended.
async fn replay(source: usize, path: PathBuf, events: mpsc::Sender<Event>) {
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(e) => {
            let _ = events
                .send(failed(source, format!("{}: {e}", path.display())))
                .await;
            return;
        }
    };
    let mut reader = AsyncBufReader::new(file);
    let mut buf = Vec::new();
    let start = Instant::now();
    let mut first = None;
    loop {
        buf.clear();
        match (&mut reader)
            .take(MAX_RECORD)
            .read_until(b'\n', &mut buf)
            .await
        {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                let _ = events
                    .send(failed(source, format!("{}: {e}", path.display())))
                    .await;
                return;
            }
        }
        let record = String::from_utf8_lossy(&buf);
        let record = record.trim();
        if record.is_empty() || record.starts_with('#') {
            continue;
        }
        let Some((ms, sentence)) = record.split_once(' ') else {
            continue;
        };
        let Ok(ms) = ms.parse::<u64>() else { continue };
        let offset = ms.saturating_sub(*first.get_or_insert(ms));
        tokio::select! {
            () = sleep_until(start + Duration::from_millis(offset)) => {}
            () = events.closed() => return,
        }
        if let Some(event) = line(source, sentence.as_bytes())
            && events.send(event).await.is_err()
        {
            return;
        }
    }
    let _ = events
        .send(Event::Status {
            source,
            status: SourceStatus::Ended,
            message: None,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_names_each_kind_of_source() {
        for (spec, name) in [
            ("serial:/dev/ttyUSB0", "serial:/dev/ttyUSB0:4800"),
            ("serial:/dev/ttyACM0:38400", "serial:/dev/ttyACM0:38400"),
            ("tcp:192.168.1.20:10110", "tcp:192.168.1.20:10110"),
            ("replay:sails/first.nmea", "replay:sails/first.nmea"),
        ] {
            assert_eq!(spec.parse::<Spec>().unwrap().to_string(), name);
        }
    }

    #[test]
    fn refuses_what_it_cannot_open() {
        for spec in [
            "",
            "gps",
            "serial:",
            "serial:/dev/ttyUSB0:1200",
            "tcp:nohost",
            "usb:/dev/x",
        ] {
            assert!(spec.parse::<Spec>().is_err(), "{spec}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn replay_keeps_the_recorded_timing() {
        let path = std::env::temp_dir().join(format!("omakeel-replay-{}.nmea", std::process::id()));
        std::fs::write(
            &path,
            "# omakeel recording v1\n1000 $A\n\n2000 $B\nnot a record\n3500 $C\n",
        )
        .unwrap();
        let (tx, mut rx) = mpsc::channel(8);
        let start = Instant::now();
        spawn(0, Spec::Replay { path: path.clone() }, tx);
        let mut heard = Vec::new();
        for _ in 0..3 {
            let Some(Event::Line { line, .. }) = rx.recv().await else {
                panic!("expected a line");
            };
            heard.push((line, start.elapsed().as_millis()));
        }
        let expected = [("$A", 0), ("$B", 1000), ("$C", 2500)].map(|(l, t)| (l.to_string(), t));
        assert_eq!(heard, expected);
        assert!(matches!(
            rx.recv().await,
            Some(Event::Status {
                status: SourceStatus::Ended,
                ..
            })
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn replay_stops_when_the_hub_is_gone() {
        let path = std::env::temp_dir().join(format!("omakeel-gap-{}.nmea", std::process::id()));
        // An hour-long gap between two lines.
        std::fs::write(&path, "0 $A\n3600000 $B\n").unwrap();
        let (tx, mut rx) = mpsc::channel(8);
        let task = tokio::spawn(replay(0, path.clone(), tx));
        assert!(matches!(rx.recv().await, Some(Event::Line { .. })));
        drop(rx);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("replay should stop without waiting out the gap")
            .unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
