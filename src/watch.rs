//! `omakeel watch`: the hub's state as one line per change, for a terminal.

use crate::protocol::VERSION;
use serde_json::Value;
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
        let message: Value = serde_json::from_str(&line?).map_err(io::Error::other)?;
        if message["v"].as_u64() != Some(u64::from(VERSION)) {
            return Err(io::Error::other(format!(
                "the hub speaks protocol v{}; this watch speaks v{VERSION}",
                message["v"]
            )));
        }
        match message["type"].as_str() {
            Some("hello") => println!(
                "omakeel {} · protocol v{}",
                message["keel"].as_str().unwrap_or("?"),
                message["v"]
            ),
            Some("state") => println!("{}", describe(&message)),
            Some("targets") => println!("{}", describe_targets(&message)),
            _ => {}
        }
    }
    Ok(())
}

/// One `state` message as a line: fix, position, speed, course, age, sources.
pub fn describe(state: &Value) -> String {
    let fix = &state["fix"];
    let mut out = format!("{:<6}", fix["status"].as_str().unwrap_or("?"));
    if let (Some(lat), Some(lon)) = (fix["lat"].as_f64(), fix["lon"].as_f64()) {
        out += &format!("  {}  {}", dm(lat, 'N', 'S', 2), dm(lon, 'E', 'W', 3));
    }
    if let Some(sog) = fix["sogKn"].as_f64() {
        out += &format!("  {sog:4.1} kn");
    }
    if let Some(cog) = fix["cogDeg"].as_f64() {
        out += &format!("  {cog:03.0}°T");
    }
    if let Some(age) = fix["ageSeconds"].as_u64() {
        out += &format!("  {age}s ago");
    }
    for source in state["sources"].as_array().into_iter().flatten() {
        out += &format!(
            "  | {} {} ({} ok, {} bad)",
            source["name"].as_str().unwrap_or("?"),
            source["status"].as_str().unwrap_or("?"),
            source["sentences"],
            source["rejected"]
        );
    }
    out
}

/// One `targets` message as a line: how many vessels, the nearest, and any
/// danger by name.
pub fn describe_targets(message: &Value) -> String {
    let targets = message["targets"].as_array().map_or(&[][..], Vec::as_slice);
    let called = |t: &Value| {
        t["name"]
            .as_str()
            .map_or_else(|| format!("MMSI {}", t["mmsi"]), str::to_string)
    };
    let mut out = match targets.len() {
        1 => "AIS    1 vessel".to_string(),
        n => format!("AIS    {n} vessels"),
    };
    if let Some(near) = targets.iter().find(|t| t["rangeNm"].is_number()) {
        out += &format!(
            "  · nearest {} {:.2} nm {:03.0}°T",
            called(near),
            near["rangeNm"].as_f64().unwrap_or_default(),
            near["bearingDeg"].as_f64().unwrap_or_default()
        );
        if let (Some(cpa), Some(tcpa)) = (near["cpaNm"].as_f64(), near["tcpaMinutes"].as_f64()) {
            out += &format!(", CPA {cpa:.2} nm in {tcpa:.1} min");
        }
    }
    let dangers: Vec<String> = targets
        .iter()
        .filter(|t| t["danger"] == true)
        .map(called)
        .collect();
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

    #[test]
    fn describes_a_fix_like_a_gps_would() {
        let state = serde_json::json!({
            "type": "state", "v": 1,
            "fix": {"status": "ok", "lat": 37.865, "lon": -122.32, "sogKn": 5.0, "cogDeg": 255.0, "ageSeconds": 0},
            "sources": [{"name": "serial:/dev/ttyUSB0:4800", "status": "ok", "sentences": 12, "rejected": 1}]
        });
        assert_eq!(
            describe(&state),
            "ok      37°51.900′N  122°19.200′W   5.0 kn  255°T  0s ago  | serial:/dev/ttyUSB0:4800 ok (12 ok, 1 bad)"
        );
    }

    #[test]
    fn describes_the_traffic_and_names_a_danger() {
        let message = serde_json::json!({"type": "targets", "v": 1, "targets": [
            {"mmsi": 366999101, "name": "BAY RUNNER", "rangeNm": 1.52, "bearingDeg": 311.2, "cpaNm": 0.19, "tcpaMinutes": 4.2, "danger": true},
            {"mmsi": 366999102, "rangeNm": 3.1, "bearingDeg": 190.0, "danger": false},
            {"mmsi": 338999103, "danger": false}
        ]});
        assert_eq!(
            describe_targets(&message),
            "AIS    3 vessels  · nearest BAY RUNNER 1.52 nm 311°T, CPA 0.19 nm in 4.2 min  DANGER BAY RUNNER"
        );
        let quiet = serde_json::json!({"type": "targets", "v": 1, "targets": []});
        assert_eq!(describe_targets(&quiet), "AIS    0 vessels");
    }

    #[test]
    fn describes_no_fix_without_inventing_one() {
        let state = serde_json::json!({"fix": {"status": "none"}, "sources": []});
        assert_eq!(describe(&state), "none  ");
    }
}
