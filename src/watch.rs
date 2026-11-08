//! `omakeel watch`: the hub's state as one line per change, for a terminal.

use crate::protocol::{Fix, Message, ReadError, SourceState, Target, VERSION};
use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

/// How long `--once` waits for the hub's first state.
const FIRST_STATE: Duration = Duration::from_secs(5);

fn connect(socket: &Path) -> io::Result<UnixStream> {
    UnixStream::connect(socket).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("{}: {e}; is omakeel running?", socket.display()),
        )
    })
}

/// `omakeel watch --once`: the hub's first full state, written to `out` as a
/// `fix` line and one `source` line each, for a script to read.
pub fn once(socket: &Path, out: &mut impl Write) -> io::Result<()> {
    let stream = connect(socket)?;
    stream.set_read_timeout(Some(FIRST_STATE))?;
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|e| match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                io::Error::other("the hub sent no state")
            }
            _ => e,
        })?;
        match Message::from_line(&line) {
            Ok(Some(Message::State { fix, sources, .. })) => {
                return write!(out, "{}", describe_once(&fix, &sources));
            }
            Ok(_) => {}
            Err(ReadError::Version { found }) => {
                let spoken = found.map_or("none".to_string(), |v| v.to_string());
                return Err(io::Error::other(format!(
                    "the hub speaks protocol v{spoken}; this watch speaks v{VERSION}"
                )));
            }
            Err(err) => return Err(io::Error::other(err)),
        }
    }
    Err(io::Error::other("the hub closed before sending a state"))
}

/// `fix STATUS [LAT,LON] [AGEs]`, then `source NAME STATUS [MESSAGE]` for
/// each source, every line ending in a newline.
fn describe_once(fix: &Fix, sources: &[SourceState]) -> String {
    let mut out = format!("fix {}", fix.status().as_str());
    if let Some(pos) = fix.position() {
        let place = &pos.place;
        out += &format!(" {:.7},{:.7} {}s", place.lat, place.lon, place.age_seconds);
    }
    out.push('\n');
    for source in sources {
        out += &format!("source {} {}", source.name, source.status.as_str());
        if let Some(message) = &source.message {
            out += &format!(" {message}");
        }
        out.push('\n');
    }
    out
}

pub fn run(socket: &Path) -> io::Result<()> {
    let stream = connect(socket)?;
    for line in BufReader::new(stream).lines() {
        match Message::from_line(&line?) {
            Ok(None) => {}
            Ok(Some(Message::Hello { keel, v })) => {
                println!("omakeel {keel} · protocol v{v}");
            }
            Ok(Some(Message::State { fix, sources, .. })) => {
                println!("{}", describe(&fix, &sources));
            }
            Ok(Some(Message::Targets { targets, .. })) => {
                println!("{}", describe_targets(&targets));
            }
            Err(ReadError::Version { found }) => {
                let spoken = match found {
                    Some(v) => v.to_string(),
                    None => "none".to_string(),
                };
                return Err(io::Error::other(format!(
                    "the hub speaks protocol v{spoken}; this watch speaks v{VERSION}"
                )));
            }
            Err(err) => return Err(io::Error::other(err)),
        }
    }
    Ok(())
}

/// One `state` message as a line: fix, position, speed, course, age, sources.
fn describe(fix: &Fix, sources: &[SourceState]) -> String {
    let mut out = format!("{:<6}", fix.status().as_str());
    if let Some(pos) = fix.position() {
        let place = &pos.place;
        out += &format!(
            "  {}  {}",
            dm(place.lat, 'N', 'S', 2),
            dm(place.lon, 'E', 'W', 3)
        );
        if let Some(sog) = place.sog_kn {
            out += &format!("  {sog:4.1} kn");
        }
        if let Some(cog) = place.cog_deg {
            out += &format!("  {cog:03.0}°T");
        }
        out += &format!("  {}s ago", place.age_seconds);
    }
    for source in sources {
        out += &format!(
            "  | {} {} ({} ok, {} bad)",
            source.name,
            source.status.as_str(),
            source.sentences,
            source.rejected
        );
    }
    out
}

/// One `targets` message as a line: how many vessels, the nearest, and any
/// danger by name.
fn describe_targets(targets: &[Target]) -> String {
    let called = |t: &Target| t.name.clone().unwrap_or_else(|| format!("MMSI {}", t.mmsi));
    let mut out = match targets.len() {
        1 => "AIS    1 vessel".to_string(),
        n => format!("AIS    {n} vessels"),
    };
    if let Some((near, range)) = targets
        .iter()
        .find_map(|t| t.range_nm.map(|range| (t, range)))
    {
        out += &format!(
            "  · nearest {} {range:.2} nm {:03.0}°T",
            called(near),
            near.bearing_deg.unwrap_or(0.0)
        );
        if let (Some(cpa), Some(tcpa)) = (near.cpa_nm, near.tcpa_minutes) {
            out += &format!(", CPA {cpa:.2} nm in {tcpa:.1} min");
        }
    }
    let dangers: Vec<String> = targets.iter().filter(|t| t.danger).map(called).collect();
    if !dangers.is_empty() {
        out += &format!("  DANGER {}", dangers.join(", "));
    }
    out
}

/// Degrees and decimal minutes, the way a chart and a GPS show them.
fn dm(value: f64, positive: char, negative: char, width: usize) -> String {
    let hemisphere = if value < 0.0 { negative } else { positive };
    let value = value.abs();
    let mut degrees = value.trunc();
    let mut minutes = ((value - degrees) * 60_000.0).round() / 1000.0;
    if minutes >= 60.0 {
        degrees += 1.0;
        minutes -= 60.0;
    }
    format!("{degrees:0width$}°{minutes:06.3}′{hemisphere}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(line: &str) -> (Fix, Vec<SourceState>) {
        let Some(Message::State { fix, sources, .. }) = Message::from_line(line).unwrap() else {
            panic!("a state line");
        };
        (fix, sources)
    }

    fn targets(line: &str) -> Vec<Target> {
        let Some(Message::Targets { targets, .. }) = Message::from_line(line).unwrap() else {
            panic!("a targets line");
        };
        targets
    }

    #[test]
    fn once_describes_a_fix_and_each_source() {
        let (fix, sources) = state(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":2},"sources":[{"name":"demo","status":"ok","sentences":12,"rejected":0},{"name":"serial:/dev/ttyUSB0:4800","status":"error","message":"/dev/ttyUSB0: No such file or directory","sentences":0,"rejected":0}]}"#,
        );
        assert_eq!(
            describe_once(&fix, &sources),
            "fix ok 37.8650000,-122.3200000 2s\nsource demo ok\nsource serial:/dev/ttyUSB0:4800 error /dev/ttyUSB0: No such file or directory\n"
        );
    }

    #[test]
    fn once_without_a_fix_has_no_place() {
        let (fix, sources) =
            state(r#"{"type":"state","v":1,"fix":{"status":"none"},"sources":[]}"#);
        assert_eq!(describe_once(&fix, &sources), "fix none\n");
    }

    #[test]
    fn once_reads_the_first_state_from_a_hub_and_stops() {
        let dir = std::env::temp_dir().join(format!("omakeel-once-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("keel.sock");
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let hub = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            for line in [
                r#"{"type":"hello","v":1,"keel":"0.1.0"}"#,
                r#"{"type":"targets","v":1,"targets":[]}"#,
                r#"{"type":"state","v":1,"fix":{"status":"stale","lat":1.5,"lon":-2.5,"ageSeconds":9},"sources":[{"name":"demo","status":"quiet","sentences":3,"rejected":1}]}"#,
                r#"{"type":"state","v":1,"fix":{"status":"none"},"sources":[]}"#,
            ] {
                let _ = writeln!(stream, "{line}");
            }
        });
        let mut out = Vec::new();
        once(&socket, &mut out).unwrap();
        hub.join().unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "fix stale 1.5000000,-2.5000000 9s\nsource demo quiet\n"
        );
    }

    #[test]
    fn once_with_no_hub_says_so() {
        let socket = std::env::temp_dir().join("omakeel-once-nowhere/keel.sock");
        let mut out = Vec::new();
        let err = once(&socket, &mut out).unwrap_err().to_string();
        assert!(err.contains("is omakeel running?"), "{err}");
        assert!(out.is_empty());
    }

    #[test]
    fn describes_a_fix_like_a_gps_would() {
        let (fix, sources) = state(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"sogKn":5.0,"cogDeg":255.0,"ageSeconds":0},"sources":[{"name":"serial:/dev/ttyUSB0:4800","status":"ok","sentences":12,"rejected":1}]}"#,
        );
        assert_eq!(
            describe(&fix, &sources),
            "ok      37°51.900′N  122°19.200′W   5.0 kn  255°T  0s ago  | serial:/dev/ttyUSB0:4800 ok (12 ok, 1 bad)"
        );
    }

    #[test]
    fn a_stale_fix_is_still_printed() {
        let (fix, sources) = state(
            r#"{"type":"state","v":1,"fix":{"status":"stale","lat":37.865,"lon":-122.32,"ageSeconds":5},"sources":[]}"#,
        );
        assert!(!fix.position().expect("stale keeps its place").current);
        assert_eq!(
            describe(&fix, &sources),
            "stale   37°51.900′N  122°19.200′W  5s ago"
        );
    }

    #[test]
    fn describes_the_traffic_and_names_a_danger() {
        let line = r#"{"type":"targets","v":1,"targets":[
            {"mmsi":366999101,"name":"BAY RUNNER","rangeNm":1.52,"bearingDeg":311.2,"cpaNm":0.19,"tcpaMinutes":4.2,"danger":true},
            {"mmsi":366999102,"rangeNm":3.1,"bearingDeg":190.0,"danger":false},
            {"mmsi":338999103,"danger":false}
        ]}"#;
        assert_eq!(
            describe_targets(&targets(line)),
            "AIS    3 vessels  · nearest BAY RUNNER 1.52 nm 311°T, CPA 0.19 nm in 4.2 min  DANGER BAY RUNNER"
        );
        assert_eq!(
            describe_targets(&targets(r#"{"type":"targets","v":1,"targets":[]}"#)),
            "AIS    0 vessels"
        );
    }

    #[test]
    fn describes_no_fix_without_inventing_one() {
        let (fix, sources) =
            state(r#"{"type":"state","v":1,"fix":{"status":"none"},"sources":[]}"#);
        assert_eq!(describe(&fix, &sources), "none  ");
    }
}
