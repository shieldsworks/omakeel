//! `omakeel probe`: listen to a serial port or a TCP stream and say what is
//! on the other end, so a sailor never has to know a baud rate.

use crate::{
    nmea,
    source::{ProbePort, Spec, serial_parts},
};
use std::{
    fmt,
    io::{self, Read},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

/// The order a sweep tries: NMEA 0183's 4800 first, then AIS receivers'
/// 38400, then the rest by how common they are.
const SWEEP: [u32; 6] = [4800, 38400, 9600, 115_200, 19_200, 57_600];
/// Sentences a rate must produce, checksums good, to be the right one.
const ENOUGH: u32 = 3;
/// A line longer than this isn't NMEA.
const MAX_LINE: usize = 1024;
/// How long to wait for a TCP host to answer.
const CONNECT: Duration = Duration::from_secs(3);

/// How long to listen at each rate.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// At each baud rate, and on a TCP stream.
    pub rate: Duration,
    /// Where AIS framing was heard but fewer than `ENOUGH` sentences:
    /// a receiver in a quiet anchorage talks rarely.
    pub extended: Duration,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            rate: Duration::from_millis(2500),
            extended: Duration::from_secs(8),
        }
    }
}

/// What to listen to.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A serial port; with no `baud`, every rate in the sweep.
    Serial {
        path: PathBuf,
        baud: Option<u32>,
    },
    Tcp {
        address: String,
    },
}

impl FromStr for Target {
    type Err = String;

    fn from_str(target: &str) -> Result<Target, String> {
        if target.starts_with("tcp:") {
            return match target.parse::<Spec>()? {
                Spec::Tcp { address } => Ok(Target::Tcp { address }),
                _ => Err(format!("'{target}' is not a TCP target")),
            };
        }
        let (path, baud) = match target.strip_prefix("serial:") {
            Some(rest) => serial_parts(rest)?,
            None if target.starts_with(['/', '.']) => (target, None),
            None => {
                return Err(format!(
                    "'{target}' is not something to probe; use PATH, serial:PATH[:BAUD] or tcp:HOST:PORT"
                ));
            }
        };
        if path.is_empty() {
            return Err(format!("'{target}' has no device path"));
        }
        Ok(Target::Serial {
            path: path.into(),
            baud,
        })
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Target::Serial { path, baud: None } => write!(f, "serial:{}", path.display()),
            Target::Serial {
                path,
                baud: Some(baud),
            } => write!(f, "serial:{}:{baud}", path.display()),
            Target::Tcp { address } => write!(f, "tcp:{address}"),
        }
    }
}

/// What a stretch of listening heard. It reads bytes as they arrive and
/// tells GPS from AIS by the sentences, never by who made the device.
#[derive(Debug, Default, Clone)]
pub struct Heard {
    /// Sentences whose checksums matched.
    pub valid: u32,
    /// Lines that looked like sentences and weren't, or were too long. The
    /// line a stream is joined in the middle of doesn't count.
    pub rejected: u32,
    /// RMC and GGA sentences carrying a real fix.
    pub fix: u32,
    /// `VDM` and `VDO` sentences.
    pub ais: u32,
    /// The last real fix, latitude then longitude.
    pub at: Option<(f64, f64)>,
    /// A line that starts like an AIS sentence, checksum or not.
    pub ais_framing: bool,
    pending: Vec<u8>,
    skipping: bool,
    lines: u32,
}

impl Heard {
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match byte {
                b'\n' => {
                    let pending = std::mem::take(&mut self.pending);
                    if std::mem::take(&mut self.skipping) {
                        continue;
                    }
                    self.line(&String::from_utf8_lossy(&pending));
                }
                _ if self.skipping => {}
                _ if self.pending.len() >= MAX_LINE => {
                    self.pending.clear();
                    self.skipping = true;
                    self.rejected += 1;
                }
                _ => self.pending.push(byte),
            }
        }
    }

    pub fn enough(&self) -> bool {
        self.valid >= ENOUGH
    }

    fn line(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let first = self.lines == 0;
        self.lines += 1;
        if text.starts_with('!') && matches!(text.get(3..6), Some("VDM" | "VDO")) {
            self.ais_framing = true;
        }
        let Ok(sentence) = nmea::parse(text) else {
            if !first {
                self.rejected += 1;
            }
            return;
        };
        self.valid += 1;
        if matches!(sentence.kind, "VDM" | "VDO") {
            self.ais += 1;
        } else if let Some(position) = nmea::position(&sentence)
            && position.valid
            && let (Some(lat), Some(lon)) = (position.lat, position.lon)
        {
            self.fix += 1;
            self.at = Some((lat, lon));
        }
    }
}

#[derive(Debug, Clone)]
pub enum Verdict {
    /// At least three good sentences. A serial port also says its baud rate.
    Ok { baud: Option<u32>, heard: Heard },
    /// The port can't be opened without more rights.
    Denied,
    /// Something else has the port.
    Busy,
    /// Opened, but no rate or stream produced three good sentences.
    Silent,
    /// The path is not there.
    Missing,
    /// A TCP host that can't be reached, or a port that failed in some other
    /// way, and why.
    Unreachable(String),
}

#[derive(Debug, Clone)]
pub struct Report {
    pub target: Target,
    pub verdict: Verdict,
}

/// A line whose first field is a valid `--source` for an `ok` verdict.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let verdict = match &self.verdict {
            Verdict::Ok { baud, heard } => {
                match (&self.target, baud) {
                    (Target::Serial { path, .. }, Some(baud)) => {
                        write!(f, "serial:{}:{baud}", path.display())?;
                    }
                    (target, _) => write!(f, "{target}")?,
                }
                write!(
                    f,
                    "  ok  fix={} ais={} rejected={}",
                    heard.fix, heard.ais, heard.rejected
                )?;
                if let Some((lat, lon)) = heard.at {
                    write!(f, " at={lat:.7},{lon:.7}")?;
                }
                return Ok(());
            }
            Verdict::Denied => "denied".to_string(),
            Verdict::Busy => "busy".to_string(),
            Verdict::Silent => "silent".to_string(),
            Verdict::Missing => "missing".to_string(),
            Verdict::Unreachable(why) => format!("unreachable  {why}"),
        };
        write!(f, "{}  {verdict}", self.target)
    }
}

/// The serial devices to probe when none are named: everything under
/// `DEV/serial/by-id`, whose names survive a replug onto another `ttyACMn`,
/// or `ttyUSB*` and `ttyACM*` where that directory doesn't exist.
pub fn scan(dev: &Path) -> Vec<Target> {
    let list = |dir: &Path, keep: &dyn Fn(&str) -> bool| -> Option<Vec<PathBuf>> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| keep(&entry.file_name().to_string_lossy()))
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        Some(paths)
    };
    let paths = list(&dev.join("serial/by-id"), &|_| true)
        .or_else(|| {
            list(dev, &|name| {
                name.starts_with("ttyUSB") || name.starts_with("ttyACM")
            })
        })
        .unwrap_or_default();
    paths
        .into_iter()
        .map(|path| Target::Serial { path, baud: None })
        .collect()
}

/// Probes every target at once and hands each report to `each` in the order
/// of `targets`, as soon as it and those before it are done.
pub fn probe_all(targets: &[Target], budget: Budget, mut each: impl FnMut(Report)) {
    thread::scope(|scope| {
        let running: Vec<_> = targets
            .iter()
            .map(|target| scope.spawn(move || probe(target, budget)))
            .collect();
        for (target, handle) in targets.iter().zip(running) {
            each(Report {
                target: target.clone(),
                verdict: handle
                    .join()
                    .unwrap_or_else(|_| Verdict::Unreachable("the probe failed".to_string())),
            });
        }
    });
}

fn probe(target: &Target, budget: Budget) -> Verdict {
    match target {
        Target::Serial { path, baud } => probe_serial(path, *baud, budget),
        Target::Tcp { address } => probe_tcp(address, budget),
    }
}

fn probe_serial(path: &Path, baud: Option<u32>, budget: Budget) -> Verdict {
    let bauds = baud.map_or(SWEEP.to_vec(), |baud| vec![baud]);
    let mut port = match ProbePort::open(path, bauds[0]) {
        Ok(port) => port,
        Err(e) => return refused(&e),
    };
    let found = sweep(&bauds, |baud| {
        port.set_baud(baud)?;
        listen(|buf| port.read(buf), budget)
    });
    match found {
        Ok(Some((baud, heard))) => Verdict::Ok {
            baud: Some(baud),
            heard,
        },
        Ok(None) => Verdict::Silent,
        Err(e) => Verdict::Unreachable(plain(&e)),
    }
}

fn probe_tcp(address: &str, budget: Budget) -> Verdict {
    let connect = || -> io::Result<TcpStream> {
        let mut last = io::Error::other("no address");
        for addr in address.to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, CONNECT) {
                Ok(stream) => return Ok(stream),
                Err(e) => last = e,
            }
        }
        Err(last)
    };
    let mut stream = match connect() {
        Ok(stream) => stream,
        Err(e) => return Verdict::Unreachable(plain(&e)),
    };
    let heard = stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .and_then(|()| {
            listen(
                |buf| match stream.read(buf) {
                    Ok(n) => Ok(Some(n)),
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        Ok(None)
                    }
                    Err(e) => Err(e),
                },
                budget,
            )
        });
    match heard {
        Ok(heard) if heard.enough() => Verdict::Ok { baud: None, heard },
        Ok(_) => Verdict::Silent,
        Err(e) => Verdict::Unreachable(plain(&e)),
    }
}

/// Why a port wouldn't open, as a verdict.
fn refused(e: &io::Error) -> Verdict {
    match (e.kind(), e.raw_os_error()) {
        (io::ErrorKind::PermissionDenied, _) => Verdict::Denied,
        (io::ErrorKind::NotFound, _) => Verdict::Missing,
        (_, Some(libc::EBUSY)) => Verdict::Busy,
        (_, Some(libc::ENODEV | libc::ENXIO)) => Verdict::Missing,
        _ => Verdict::Unreachable(plain(e)),
    }
}

/// An I/O error as a short lowercase phrase, without the errno.
fn plain(e: &io::Error) -> String {
    let text = e.to_string();
    let text = text.split(" (os error").next().unwrap_or(&text);
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => text.to_string(),
    }
}

/// Tries each rate in order and stops at the first that `listen` says is
/// right.
fn sweep(
    bauds: &[u32],
    mut listen: impl FnMut(u32) -> io::Result<Heard>,
) -> io::Result<Option<(u32, Heard)>> {
    for &baud in bauds {
        let heard = listen(baud)?;
        if heard.enough() {
            return Ok(Some((baud, heard)));
        }
    }
    Ok(None)
}

/// Feeds what `read` returns to a `Heard` until the rate's budget is spent,
/// or the budget for a quiet AIS receiver once framing has been heard
/// without enough sentences. `read` gives `None` when nothing came just now
/// and `Some(0)` when the stream is over.
fn listen(
    mut read: impl FnMut(&mut [u8]) -> io::Result<Option<usize>>,
    budget: Budget,
) -> io::Result<Heard> {
    let start = Instant::now();
    let mut deadline = start + budget.rate;
    let mut extended = false;
    let mut heard = Heard::default();
    let mut buf = [0u8; 512];
    loop {
        if Instant::now() >= deadline {
            if extended || heard.enough() || !heard.ais_framing {
                return Ok(heard);
            }
            extended = true;
            deadline = start + budget.extended;
            continue;
        }
        match read(&mut buf)? {
            None => {}
            Some(0) => return Ok(heard),
            Some(n) => heard.feed(&buf[..n]),
        }
    }
}

/// `omakeel probe [TARGET...]`: one line per target on stdout.
pub fn run(args: &[String]) -> Result<(), String> {
    let targets = if args.is_empty() {
        scan(Path::new("/dev"))
    } else {
        args.iter()
            .map(|arg| arg.parse())
            .collect::<Result<_, _>>()?
    };
    if targets.is_empty() {
        eprintln!("omakeel: no serial devices found; plug in a receiver");
    }
    probe_all(&targets, Budget::default(), |report| println!("{report}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAIL: &str = include_str!("../tests/fixtures/berkeley-marina.nmea");

    /// The sample sail's sentences, one per line, without timestamps.
    fn sail() -> Vec<&'static str> {
        SAIL.lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split_once(' ').map(|(_, s)| s))
            .collect()
    }

    fn first(kind: &str) -> &'static str {
        sail()
            .into_iter()
            .find(|s| s.contains(kind))
            .expect("the sail has this sentence")
    }

    fn fix_line() -> &'static str {
        sail()
            .into_iter()
            .find(|s| s.starts_with("$GPRMC") && s.contains(",A,"))
            .expect("the sail has a fix")
    }

    fn heard(text: &str) -> Heard {
        let mut heard = Heard::default();
        heard.feed(text.as_bytes());
        heard
    }

    #[test]
    fn a_gps_with_a_fix_counts_fixes_and_remembers_where() {
        let text = format!("{0}\r\n{0}\r\n{0}\r\n", fix_line());
        let h = heard(&text);
        assert_eq!((h.valid, h.fix, h.ais, h.rejected), (3, 3, 0, 0));
        assert!(h.enough());
        let (lat, lon) = h.at.unwrap();
        assert!((lat - 37.865).abs() < 1e-6 && (lon + 122.32).abs() < 1e-6);
    }

    #[test]
    fn a_receiver_without_a_fix_is_heard_but_has_no_place() {
        let no_fix = first("$GPRMC");
        assert!(no_fix.contains(",V,"));
        let h = heard(&format!("{no_fix}\n{no_fix}\n{no_fix}\n"));
        assert_eq!((h.valid, h.fix, h.at), (3, 0, None));
    }

    #[test]
    fn ais_sentences_are_told_from_gps_by_their_kind() {
        let ais = first("!AIVDM");
        let h = heard(&format!("{ais}\n{ais}\n{ais}\n"));
        assert_eq!((h.valid, h.ais, h.fix), (3, 3, 0));
        assert!(h.ais_framing && h.at.is_none());
    }

    #[test]
    fn ais_framing_shows_even_when_the_checksum_does_not() {
        let h = heard("x\n!AIVDM,1,1,,A,B53Bo?h0:=kwrd5Jf`2sSwP4P000,0*00\n");
        assert_eq!((h.valid, h.ais, h.rejected), (0, 0, 1));
        assert!(h.ais_framing);
        assert!(!heard("$GPRMC,1*00\n").ais_framing);
    }

    #[test]
    fn bad_lines_are_rejected_and_never_counted_as_heard() {
        let good = fix_line();
        let bad = good.replace("*", "*0");
        let h = heard(&format!("{good}\nnoise\n{bad}\n\n{good}\n"));
        // The first line is where we joined the stream, so only the two
        // lines after it count against the rate.
        assert_eq!((h.valid, h.rejected), (2, 2));
    }

    #[test]
    fn the_line_the_stream_is_joined_in_the_middle_of_is_not_held_against_it() {
        let good = fix_line();
        let tail = &good[10..];
        let h = heard(&format!("{tail}\n{good}\n"));
        assert_eq!((h.valid, h.rejected), (1, 0));
    }

    #[test]
    fn a_sentence_split_across_reads_is_whole_again() {
        let text = format!("{}\n", fix_line());
        let (a, b) = text.as_bytes().split_at(20);
        let mut h = Heard::default();
        h.feed(a);
        assert_eq!(h.valid, 0);
        h.feed(b);
        assert_eq!(h.valid, 1);
    }

    #[test]
    fn a_line_with_no_end_is_one_rejection_not_a_growing_buffer() {
        let mut h = Heard::default();
        h.feed(&[b'x'; 5000]);
        h.feed(b"\n");
        h.feed(format!("{}\n", fix_line()).as_bytes());
        assert_eq!((h.valid, h.rejected), (1, 1));
    }

    #[test]
    fn the_sweep_goes_in_order_and_stops_at_the_first_rate_that_hears() {
        let mut tried = Vec::new();
        let found = sweep(&SWEEP, |baud| {
            tried.push(baud);
            Ok(if baud == 9600 {
                heard(&format!("{0}\n{0}\n{0}\n{0}\n", fix_line()))
            } else {
                Heard::default()
            })
        })
        .unwrap();
        assert_eq!(tried, [4800, 38400, 9600]);
        assert_eq!(found.unwrap().0, 9600);
    }

    #[test]
    fn a_sweep_that_hears_nothing_tries_every_rate() {
        let mut tried = Vec::new();
        let found = sweep(&SWEEP, |baud| {
            tried.push(baud);
            Ok(heard("noise\n"))
        })
        .unwrap();
        assert_eq!(tried, [4800, 38400, 9600, 115_200, 19_200, 57_600]);
        assert!(found.is_none());
    }

    /// Hands out `data` once, then nothing.
    fn once(data: String) -> impl FnMut(&mut [u8]) -> io::Result<Option<usize>> {
        let mut data = Some(data);
        move |buf| {
            thread::sleep(Duration::from_millis(1));
            Ok(data.take().map(|d| {
                buf[..d.len()].copy_from_slice(d.as_bytes());
                d.len()
            }))
        }
    }

    const SHORT: Budget = Budget {
        rate: Duration::from_millis(40),
        extended: Duration::from_millis(400),
    };

    #[test]
    fn listening_stops_at_the_rate_budget_when_nothing_looks_like_ais() {
        let start = Instant::now();
        let h = listen(once(format!("{}\n", fix_line())), SHORT).unwrap();
        assert_eq!(h.valid, 1);
        assert!(start.elapsed() < Duration::from_millis(300));
    }

    #[test]
    fn listening_runs_on_where_ais_framing_was_heard_without_enough() {
        let start = Instant::now();
        let h = listen(once(format!("{}\n", first("!AIVDM"))), SHORT).unwrap();
        assert_eq!(h.ais, 1);
        assert!(start.elapsed() >= Duration::from_millis(400));
    }

    #[test]
    fn listening_does_not_run_on_once_there_is_enough() {
        let ais = first("!AIVDM");
        let start = Instant::now();
        let h = listen(once(format!("{ais}\n{ais}\n{ais}\n")), SHORT).unwrap();
        assert!(h.enough());
        assert!(start.elapsed() < Duration::from_millis(300));
    }

    #[test]
    fn a_closed_stream_ends_the_listening() {
        let budget = Budget {
            rate: Duration::from_secs(30),
            extended: Duration::from_secs(30),
        };
        let h = listen(|_| Ok(Some(0)), budget).unwrap();
        assert_eq!(h.valid, 0);
    }

    #[test]
    fn targets_parse_and_print() {
        for (arg, shown) in [
            ("/dev/ttyUSB0", "serial:/dev/ttyUSB0"),
            ("./fake", "serial:./fake"),
            ("serial:/dev/ttyUSB0", "serial:/dev/ttyUSB0"),
            ("serial:/dev/ttyACM0:38400", "serial:/dev/ttyACM0:38400"),
            ("tcp:192.168.1.20:10110", "tcp:192.168.1.20:10110"),
        ] {
            assert_eq!(arg.parse::<Target>().unwrap().to_string(), shown, "{arg}");
        }
        for arg in [
            "",
            "demo",
            "replay:x.nmea",
            "ttyUSB0",
            "serial:",
            "serial:/dev/x:1200",
            "tcp:nohost",
        ] {
            assert!(arg.parse::<Target>().is_err(), "{arg}");
        }
    }

    #[test]
    fn a_bare_path_sweeps_and_an_explicit_baud_does_not() {
        assert_eq!(
            "/dev/x".parse::<Target>().unwrap(),
            Target::Serial {
                path: "/dev/x".into(),
                baud: None
            }
        );
        assert_eq!(
            "serial:/dev/x:9600".parse::<Target>().unwrap(),
            Target::Serial {
                path: "/dev/x".into(),
                baud: Some(9600)
            }
        );
    }

    fn report(target: &str, verdict: Verdict) -> String {
        Report {
            target: target.parse().unwrap(),
            verdict,
        }
        .to_string()
    }

    #[test]
    fn an_ok_line_starts_with_a_source_that_includes_the_baud() {
        let text = format!("{0}\n{0}\n{0}\nnoise\n", fix_line());
        let line = report(
            "/dev/ttyUSB0",
            Verdict::Ok {
                baud: Some(38_400),
                heard: heard(&text),
            },
        );
        assert_eq!(
            line,
            "serial:/dev/ttyUSB0:38400  ok  fix=3 ais=0 rejected=1 at=37.8650000,-122.3200000"
        );
        let spec = line.split("  ").next().unwrap();
        assert_eq!(
            spec.parse::<Spec>().unwrap(),
            Spec::Serial {
                path: "/dev/ttyUSB0".into(),
                baud: 38_400
            }
        );
    }

    #[test]
    fn an_ok_line_without_a_fix_has_no_place() {
        let ais = first("!AIVDM");
        let line = report(
            "tcp:10.0.0.2:10110",
            Verdict::Ok {
                baud: None,
                heard: heard(&format!("{ais}\n{ais}\n{ais}\n")),
            },
        );
        assert_eq!(line, "tcp:10.0.0.2:10110  ok  fix=0 ais=3 rejected=0");
    }

    #[test]
    fn the_other_verdicts_name_the_target_as_given() {
        for (verdict, shown) in [
            (Verdict::Denied, "denied"),
            (Verdict::Busy, "busy"),
            (Verdict::Silent, "silent"),
            (Verdict::Missing, "missing"),
        ] {
            assert_eq!(
                report("serial:/dev/x", verdict),
                format!("serial:/dev/x  {shown}")
            );
        }
        assert_eq!(
            report("tcp:h:1", Verdict::Unreachable("connection refused".into())),
            "tcp:h:1  unreachable  connection refused"
        );
    }

    #[test]
    fn an_os_error_reads_as_a_phrase() {
        let e = io::Error::from_raw_os_error(libc::ECONNREFUSED);
        assert_eq!(plain(&e), "connection refused");
    }

    #[test]
    fn a_scan_prefers_by_id_and_falls_back_to_the_tty_names() {
        let dev = std::env::temp_dir().join(format!("omakeel-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        for name in ["ttyUSB1", "ttyACM0", "tty0", "ttyUSB0"] {
            std::fs::write(dev.join(name), "").unwrap();
        }
        let names = |targets: Vec<Target>| -> Vec<String> {
            targets.iter().map(ToString::to_string).collect()
        };
        let d = dev.display();
        assert_eq!(
            names(scan(&dev)),
            [
                format!("serial:{d}/ttyACM0"),
                format!("serial:{d}/ttyUSB0"),
                format!("serial:{d}/ttyUSB1"),
            ]
        );
        std::fs::create_dir_all(dev.join("serial/by-id")).unwrap();
        assert_eq!(names(scan(&dev)), Vec::<String>::new());
        std::fs::write(dev.join("serial/by-id/usb-b"), "").unwrap();
        std::fs::write(dev.join("serial/by-id/usb-a"), "").unwrap();
        assert_eq!(
            names(scan(&dev)),
            [
                format!("serial:{d}/serial/by-id/usb-a"),
                format!("serial:{d}/serial/by-id/usb-b"),
            ]
        );
        std::fs::remove_dir_all(&dev).unwrap();
    }
}
