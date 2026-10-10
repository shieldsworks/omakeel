//! `omakeel watch`: the hub's state as one line per change, for a terminal.

use crate::protocol::{Fix, Message, ReadError, SourceState, Target, VERSION};
use std::{
    io::{self, BufRead, BufReader},
    os::unix::net::UnixStream,
    path::Path,
};

pub fn run(socket: &Path) -> io::Result<()> {
    let stream = UnixStream::connect(socket).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("{}: {e}; is omakeel running?", socket.display()),
        )
    })?;
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
