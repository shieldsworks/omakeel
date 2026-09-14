//! `omakeel watch`: the hub's state as one line per change, for a terminal.

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
        match message["type"].as_str() {
            Some("hello") => println!(
                "omakeel {} · protocol v{}",
                message["keel"].as_str().unwrap_or("?"),
                message["v"]
            ),
            Some("state") => println!("{}", describe(&message)),
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
    fn describes_no_fix_without_inventing_one() {
        let state = serde_json::json!({"fix": {"status": "none"}, "sources": []});
        assert_eq!(describe(&state), "none  ");
    }
}
