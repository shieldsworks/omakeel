//! Hub-to-app messages: newline-delimited JSON, version 1.
//! `docs/protocol.md` is the contract; change both together.

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::fmt;

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        v: u32,
        keel: String,
    },
    State {
        v: u32,
        fix: Fix,
        sources: Vec<SourceState>,
    },
    Targets {
        v: u32,
        targets: Vec<Target>,
    },
}

impl Message {
    /// One JSON object, with or without the trailing newline the hub writes.
    /// `Ok(None)` is a blank line or a `type` this version does not know.
    /// Any `v` other than [`VERSION`], a missing `v`, a `null`, or a known
    /// type with an illegal fix or target is `Err`.
    pub fn from_line(line: &str) -> Result<Option<Message>, ReadError> {
        let line = line.strip_suffix('\n').unwrap_or(line);
        if line.is_empty() {
            return Ok(None);
        }
        let value: Value = serde_json::from_str(line).map_err(|_| ReadError::NotJson)?;
        if contains_null(&value) {
            return Err(ReadError::Null);
        }
        let Some(obj) = value.as_object() else {
            return Err(ReadError::NotJson);
        };
        require_version(obj)?;
        match obj.get("type").and_then(Value::as_str) {
            Some("hello") => hello(obj).map(Some),
            Some("state") => state(obj).map(Some),
            Some("targets") => targets(obj).map(Some),
            _ => Ok(None),
        }
    }

    /// The bytes the hub writes, including the trailing newline.
    #[expect(
        clippy::expect_used,
        reason = "to_value maps a non-finite f64 to null and errors only for a non-finite map key, and a message has no map; to_string of that value fails only for a non-finite number, which null-stripping has removed"
    )]
    pub fn to_line(&self) -> String {
        let value = without_nulls(
            serde_json::to_value(self).expect("a message is strings, bools, and numbers"),
        );
        let mut line =
            serde_json::to_string(&value).expect("nulls are gone, so every number is finite");
        line.push('\n');
        line
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    /// Not JSON, or a known message whose fields are not the shape version 1 reads.
    NotJson,
    /// Missing `v`, or a `v` other than 1. `found` is `None` when `v` is absent
    /// or is not a `u32`.
    Version { found: Option<u32> },
    /// A `null` anywhere in the line.
    Null,
    /// A fix or source status outside the closed set.
    UnknownStatus,
    /// `ok` or `stale` without `lat`, `lon`, and `ageSeconds`, or a carried
    /// `nofix` position missing `ageSeconds`.
    PositionMissing,
    /// `none` carrying `lat` or `lon`.
    PositionForbidden,
    /// `lat` without `lon`, or `lon` without `lat`, on a fix or a target.
    PositionPartial,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::NotJson => f.write_str("not a version 1 message"),
            ReadError::Version { found: None } => f.write_str("missing protocol version"),
            ReadError::Version { found: Some(found) } => {
                write!(f, "protocol version {found}")
            }
            ReadError::Null => f.write_str("null is not a value on this socket"),
            ReadError::UnknownStatus => f.write_str("unknown status"),
            ReadError::PositionMissing => f.write_str("position is missing"),
            ReadError::PositionForbidden => f.write_str("position is not allowed"),
            ReadError::PositionPartial => f.write_str("position is only one coordinate"),
        }
    }
}

impl std::error::Error for ReadError {}

#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    inner: FixInner,
}

#[derive(Clone, Debug, PartialEq)]
enum FixInner {
    None,
    Nofix(Option<Place>),
    Ok(Place),
    Stale(Place),
}

#[derive(Serialize)]
struct EncodedFix<'a> {
    status: FixStatus,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    place: Option<&'a Place>,
}

impl Serialize for Fix {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        EncodedFix {
            status: self.status(),
            place: self.place(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Fix {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if contains_null(&value) {
            return Err(de::Error::custom(ReadError::Null));
        }
        fix_from_value(&value).map_err(de::Error::custom)
    }
}

impl Fix {
    pub(crate) fn none() -> Fix {
        Fix {
            inner: FixInner::None,
        }
    }

    pub(crate) fn nofix(last: Option<Place>) -> Fix {
        Fix {
            inner: FixInner::Nofix(last),
        }
    }

    pub(crate) fn ok(place: Place) -> Fix {
        Fix {
            inner: FixInner::Ok(place),
        }
    }

    pub(crate) fn stale(place: Place) -> Fix {
        Fix {
            inner: FixInner::Stale(place),
        }
    }

    pub fn status(&self) -> FixStatus {
        match &self.inner {
            FixInner::None => FixStatus::None,
            FixInner::Nofix(_) => FixStatus::Nofix,
            FixInner::Ok(_) => FixStatus::Ok,
            FixInner::Stale(_) => FixStatus::Stale,
        }
    }

    /// `None` for `none` and for `nofix` with nothing stored.
    /// `current` is true only for `ok`.
    pub fn position(&self) -> Option<Position> {
        Some(Position {
            place: self.place()?.clone(),
            current: matches!(self.inner, FixInner::Ok(_)),
        })
    }

    pub(crate) fn satellites(&self) -> Option<u8> {
        self.place().and_then(|place| place.satellites)
    }

    pub(crate) fn hdop(&self) -> Option<f64> {
        self.place().and_then(|place| place.hdop)
    }

    fn place(&self) -> Option<&Place> {
        match &self.inner {
            FixInner::None | FixInner::Nofix(None) => None,
            FixInner::Nofix(Some(place)) | FixInner::Ok(place) | FixInner::Stale(place) => {
                Some(place)
            }
        }
    }
}

/// A place the hub still has. `current` is true only for an `ok` fix.
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    pub place: Place,
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub lat: f64,
    pub lon: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sog_kn: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cog_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub satellites: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdop: Option<f64>,
    pub age_seconds: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FixStatus {
    /// No position sentence heard.
    None,
    /// The receiver is talking but has no fix.
    Nofix,
    Ok,
    /// The last fix is at least five seconds old.
    Stale,
}

impl FixStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FixStatus::None => "none",
            FixStatus::Nofix => "nofix",
            FixStatus::Ok => "ok",
            FixStatus::Stale => "stale",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// Opening or connecting, with nothing received yet.
    Connecting,
    Ok,
    /// Connected, but nothing received for five seconds.
    Quiet,
    /// Can't be opened or reached.
    Error,
    /// A replay that has reached the end of its recording.
    Ended,
}

impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceStatus::Connecting => "connecting",
            SourceStatus::Ok => "ok",
            SourceStatus::Quiet => "quiet",
            SourceStatus::Error => "error",
            SourceStatus::Ended => "ended",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceState {
    /// The source as given on the command line, like `serial:/dev/ttyUSB0:4800`.
    pub name: String,
    pub status: SourceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Lines that checked out as NMEA sentences.
    pub sentences: u64,
    /// Lines that didn't: bad checksums or garbage.
    pub rejected: u64,
}

impl SourceState {
    pub fn new(name: String) -> SourceState {
        SourceState {
            name,
            status: SourceStatus::Connecting,
            message: None,
            sentences: 0,
            rejected: 0,
        }
    }
}

/// One vessel heard on AIS. A position older than ten minutes is absent, not stale.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub mmsi: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship_type: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sog_kn: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cog_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_m: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beam_m: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_nm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearing_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpa_nm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcpa_minutes: Option<f64>,
    pub danger: bool,
}

impl Target {
    pub fn sighting(&self) -> Option<(f64, f64)> {
        Some((self.lat?, self.lon?))
    }
}

fn require_version(obj: &Map<String, Value>) -> Result<(), ReadError> {
    let Some(value) = obj.get("v") else {
        return Err(ReadError::Version { found: None });
    };
    let found = value.as_u64().and_then(|n| u32::try_from(n).ok());
    if found == Some(VERSION) {
        Ok(())
    } else {
        Err(ReadError::Version { found })
    }
}

fn hello(obj: &Map<String, Value>) -> Result<Message, ReadError> {
    let keel = obj
        .get("keel")
        .and_then(Value::as_str)
        .ok_or(ReadError::NotJson)?;
    Ok(Message::Hello {
        v: VERSION,
        keel: keel.to_owned(),
    })
}

fn state(obj: &Map<String, Value>) -> Result<Message, ReadError> {
    let fix = obj
        .get("fix")
        .ok_or(ReadError::NotJson)
        .and_then(fix_from_value)?;
    let sources = obj
        .get("sources")
        .and_then(Value::as_array)
        .ok_or(ReadError::NotJson)?;
    let mut decoded = Vec::with_capacity(sources.len());
    for source in sources {
        decoded.push(source_from_value(source)?);
    }
    Ok(Message::State {
        v: VERSION,
        fix,
        sources: decoded,
    })
}

fn targets(obj: &Map<String, Value>) -> Result<Message, ReadError> {
    let items = obj
        .get("targets")
        .and_then(Value::as_array)
        .ok_or(ReadError::NotJson)?;
    let mut decoded = Vec::with_capacity(items.len());
    for item in items {
        decoded.push(target_from_value(item)?);
    }
    Ok(Message::Targets {
        v: VERSION,
        targets: decoded,
    })
}

fn source_from_value(value: &Value) -> Result<SourceState, ReadError> {
    match value.get("status").and_then(Value::as_str) {
        Some("connecting" | "ok" | "quiet" | "error" | "ended") => {}
        Some(_) => return Err(ReadError::UnknownStatus),
        None => return Err(ReadError::NotJson),
    }
    SourceState::deserialize(value).map_err(|_| ReadError::NotJson)
}

fn target_from_value(value: &Value) -> Result<Target, ReadError> {
    let target = Target::deserialize(value).map_err(|_| ReadError::NotJson)?;
    match (target.lat, target.lon) {
        (Some(_), None) | (None, Some(_)) => Err(ReadError::PositionPartial),
        _ => Ok(target),
    }
}

fn fix_from_value(value: &Value) -> Result<Fix, ReadError> {
    let Some(obj) = value.as_object() else {
        return Err(ReadError::NotJson);
    };
    let status = match obj.get("status").and_then(Value::as_str) {
        Some("none") => FixStatus::None,
        Some("nofix") => FixStatus::Nofix,
        Some("ok") => FixStatus::Ok,
        Some("stale") => FixStatus::Stale,
        Some(_) => return Err(ReadError::UnknownStatus),
        None => return Err(ReadError::NotJson),
    };
    let lat = opt_f64(obj, "lat")?;
    let lon = opt_f64(obj, "lon")?;
    let age = opt_u64(obj, "ageSeconds")?;
    match (status, lat, lon, age) {
        (_, Some(_), None, _) | (_, None, Some(_), _) => Err(ReadError::PositionPartial),
        (FixStatus::None, Some(_), Some(_), _) => Err(ReadError::PositionForbidden),
        (FixStatus::Ok | FixStatus::Stale, None, None, _) => Err(ReadError::PositionMissing),
        (FixStatus::Ok | FixStatus::Stale | FixStatus::Nofix, Some(_), Some(_), None) => {
            Err(ReadError::PositionMissing)
        }
        (FixStatus::None, None, None, _) => Ok(Fix::none()),
        (FixStatus::Nofix, None, None, _) => Ok(Fix::nofix(None)),
        (FixStatus::Ok, Some(lat), Some(lon), Some(age_seconds)) => {
            Ok(Fix::ok(place_from(obj, lat, lon, age_seconds)?))
        }
        (FixStatus::Stale, Some(lat), Some(lon), Some(age_seconds)) => {
            Ok(Fix::stale(place_from(obj, lat, lon, age_seconds)?))
        }
        (FixStatus::Nofix, Some(lat), Some(lon), Some(age_seconds)) => {
            Ok(Fix::nofix(Some(place_from(obj, lat, lon, age_seconds)?)))
        }
    }
}

fn place_from(
    obj: &Map<String, Value>,
    lat: f64,
    lon: f64,
    age_seconds: u64,
) -> Result<Place, ReadError> {
    Ok(Place {
        lat,
        lon,
        sog_kn: opt_f64(obj, "sogKn")?,
        cog_deg: opt_f64(obj, "cogDeg")?,
        utc: opt_string(obj, "utc")?,
        satellites: opt_u8(obj, "satellites")?,
        hdop: opt_f64(obj, "hdop")?,
        age_seconds,
    })
}

fn opt_f64(obj: &Map<String, Value>, key: &str) -> Result<Option<f64>, ReadError> {
    match obj.get(key) {
        None => Ok(None),
        Some(value) => value.as_f64().map(Some).ok_or(ReadError::NotJson),
    }
}

fn opt_u64(obj: &Map<String, Value>, key: &str) -> Result<Option<u64>, ReadError> {
    match obj.get(key) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or(ReadError::NotJson),
    }
}

fn opt_u8(obj: &Map<String, Value>, key: &str) -> Result<Option<u8>, ReadError> {
    match opt_u64(obj, key)? {
        None => Ok(None),
        Some(n) => u8::try_from(n).map(Some).map_err(|_| ReadError::NotJson),
    }
}

fn opt_string(obj: &Map<String, Value>, key: &str) -> Result<Option<String>, ReadError> {
    match obj.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_owned()))
            .ok_or(ReadError::NotJson),
    }
}

/// Drops keys whose value is null. `to_value` writes a non-finite `f64` as null.
fn without_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, item)| !item.is_null())
                .map(|(key, item)| (key, without_nulls(item)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(without_nulls).collect()),
        other => other,
    }
}

fn contains_null(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.iter().any(contains_null),
        Value::Object(fields) => fields.values().any(contains_null),
        Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STALE: &str = r#"{"type":"state","v":1,"fix":{"status":"stale","lat":37.865,"lon":-122.32,"ageSeconds":5},"sources":[]}"#;
    const NOFIX: &str = r#"{"type":"state","v":1,"fix":{"status":"nofix","lat":37.865,"lon":-122.32,"ageSeconds":6},"sources":[]}"#;
    const NONE: &str = r#"{"type":"state","v":1,"fix":{"status":"none"},"sources":[]}"#;
    const OK: &str = r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":0},"sources":[]}"#;

    const PROTOCOL: &str = include_str!("../docs/protocol.md");

    fn state_fix(line: &str) -> Fix {
        let Some(Message::State { fix, .. }) = Message::from_line(line).unwrap() else {
            panic!("a state line");
        };
        fix
    }

    #[test]
    fn a_non_finite_coordinate_is_omitted() {
        let message = Message::State {
            v: VERSION,
            fix: Fix::ok(Place {
                lat: f64::NAN,
                lon: -122.32,
                sog_kn: None,
                cog_deg: None,
                utc: None,
                satellites: None,
                hdop: None,
                age_seconds: 0,
            }),
            sources: vec![],
        };
        let line = message.to_line();
        assert!(!line.contains("null"), "{line}");
        let value: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert!(value["fix"].get("lat").is_none());
        assert_eq!(value["fix"]["lon"], -122.32);
    }

    #[test]
    fn a_stale_fix_is_kept_and_not_current() {
        let pos = state_fix(STALE).position().expect("stale carries a place");
        assert!(!pos.current);
        assert_eq!(pos.place.lat, 37.865);
        assert_eq!(pos.place.lon, -122.32);
        assert_eq!(pos.place.age_seconds, 5);
    }

    #[test]
    fn a_nofix_past_five_seconds_is_kept_and_not_current() {
        let fix = state_fix(NOFIX);
        let pos = fix.position().expect("a stored nofix carries a place");
        assert!(!pos.current);
        assert_eq!(fix.status(), FixStatus::Nofix);
        assert_eq!(pos.place.lat, 37.865);
        assert_eq!(pos.place.lon, -122.32);
        assert_eq!(pos.place.age_seconds, 6);
    }

    #[test]
    fn none_has_no_position() {
        let fix = state_fix(NONE);
        assert_eq!(fix.status(), FixStatus::None);
        assert_eq!(fix.position(), None);
    }

    #[test]
    fn an_ok_fix_is_current_without_reading_age() {
        let fresh = state_fix(OK);
        let pos = fresh.position().expect("ok carries a place");
        assert!(pos.current);
        assert_eq!(pos.place.age_seconds, 0);
        let old = state_fix(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":100},"sources":[]}"#,
        );
        let pos = old.position().expect("ok carries a place");
        assert!(pos.current);
        assert_eq!(pos.place.age_seconds, 100);
    }

    #[test]
    fn a_current_key_on_the_line_does_not_set_currency() {
        let ok = r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":0,"current":false},"sources":[]}"#;
        let stale = r#"{"type":"state","v":1,"fix":{"status":"stale","lat":37.865,"lon":-122.32,"ageSeconds":5,"current":true},"sources":[]}"#;
        assert!(state_fix(ok).position().expect("ok").current);
        assert!(!state_fix(stale).position().expect("stale").current);
        let message = Message::from_line(ok).unwrap().expect("ok state");
        let again: Value = serde_json::from_str(message.to_line().trim_end()).unwrap();
        assert!(again.get("current").is_none());
        assert!(again["fix"].get("current").is_none());
    }

    #[test]
    fn to_line_then_from_line_returns_the_same_message() {
        let place = Place {
            lat: 37.865,
            lon: -122.32,
            sog_kn: Some(5.0),
            cog_deg: Some(255.0),
            utc: Some("2026-09-13T21:00:00Z".into()),
            satellites: Some(9),
            hdop: Some(0.9),
            age_seconds: 4,
        };
        let messages = [
            Message::Hello {
                v: VERSION,
                keel: "0.1.0".into(),
            },
            Message::State {
                v: VERSION,
                fix: Fix::none(),
                sources: vec![],
            },
            Message::State {
                v: VERSION,
                fix: Fix::nofix(None),
                sources: vec![SourceState {
                    name: "tcp:10.0.2.2:10110".into(),
                    status: SourceStatus::Error,
                    message: Some("connection closed".into()),
                    sentences: 0,
                    rejected: 4,
                }],
            },
            Message::State {
                v: VERSION,
                fix: Fix::nofix(Some(place.clone())),
                sources: vec![SourceState::new("replay:sail.nmea".into())],
            },
            Message::State {
                v: VERSION,
                fix: Fix::ok(place.clone()),
                sources: vec![],
            },
            Message::State {
                v: VERSION,
                fix: Fix::stale(place),
                sources: vec![],
            },
            Message::Targets {
                v: VERSION,
                targets: vec![
                    Target {
                        mmsi: 366999101,
                        name: Some("BAY RUNNER".into()),
                        callsign: Some("WDX9101".into()),
                        ship_type: Some(60),
                        kind: Some("passenger".into()),
                        class: Some("A".into()),
                        status: Some("under way using engine".into()),
                        lat: Some(37.8809371),
                        lon: Some(-122.3463369),
                        sog_kn: Some(20.0),
                        cog_deg: Some(150.0),
                        heading_deg: Some(150),
                        length_m: Some(40),
                        beam_m: Some(10),
                        destination: Some("SF FERRY BLDG".into()),
                        age_seconds: Some(1),
                        range_nm: Some(1.52),
                        bearing_deg: Some(311.2),
                        cpa_nm: Some(0.19),
                        tcpa_minutes: Some(4.2),
                        danger: true,
                    },
                    Target {
                        mmsi: 1,
                        name: None,
                        callsign: None,
                        ship_type: None,
                        kind: None,
                        class: None,
                        status: None,
                        lat: None,
                        lon: None,
                        sog_kn: None,
                        cog_deg: None,
                        heading_deg: None,
                        length_m: None,
                        beam_m: None,
                        destination: None,
                        age_seconds: None,
                        range_nm: None,
                        bearing_deg: None,
                        cpa_nm: None,
                        tcpa_minutes: None,
                        danger: false,
                    },
                ],
            },
        ];
        for message in messages {
            let again = Message::from_line(&message.to_line()).unwrap();
            assert_eq!(again, Some(message));
        }
    }

    #[test]
    fn documented_examples_parse_and_keep_their_keys() {
        let examples = fenced_json(PROTOCOL);
        assert_eq!(examples.len(), 3);
        for text in examples {
            let original: Value = serde_json::from_str(text).unwrap();
            let message = Message::from_line(text)
                .unwrap()
                .expect("hello, state, and targets are known");
            let encoded: Value = serde_json::from_str(message.to_line().trim_end()).unwrap();
            assert_same_keys(&original, &encoded);
            assert!(!contains_null(&encoded), "{encoded}");
        }
    }

    #[test]
    fn a_target_sighting_is_both_coordinates_or_nothing() {
        let absent = r#"{"type":"targets","v":1,"targets":[{"mmsi":1,"danger":false}]}"#;
        let present = r#"{"type":"targets","v":1,"targets":[{"mmsi":1,"lat":37.865,"lon":-122.32,"danger":false}]}"#;
        let Message::Targets { targets, .. } = Message::from_line(absent).unwrap().unwrap() else {
            panic!("targets");
        };
        assert_eq!(targets[0].sighting(), None);
        let Message::Targets { targets, .. } = Message::from_line(present).unwrap().unwrap() else {
            panic!("targets");
        };
        assert_eq!(targets[0].sighting(), Some((37.865, -122.32)));
    }

    #[test]
    fn version_null_and_a_broken_position_fail_closed() {
        assert_eq!(Message::from_line("").unwrap(), None);
        assert_eq!(Message::from_line("\n").unwrap(), None);
        assert_eq!(
            Message::from_line("not json").unwrap_err(),
            ReadError::NotJson
        );
        assert_eq!(
            Message::from_line(r#"{"type":"future","v":2}"#).unwrap_err(),
            ReadError::Version { found: Some(2) }
        );
        assert_eq!(
            Message::from_line(r#"{"type":"future"}"#).unwrap_err(),
            ReadError::Version { found: None }
        );
        assert_eq!(
            Message::from_line(r#"{"type":"future","v":1}"#).unwrap(),
            None
        );
        assert_eq!(
            Message::from_line(r#"{"type":"nope","v":null}"#).unwrap_err(),
            ReadError::Null
        );
        assert_eq!(
            Message::from_line(r#"{"type":"state","v":1,"fix":{"status":"ok"},"sources":[]}"#)
                .unwrap_err(),
            ReadError::PositionMissing
        );
        assert_eq!(
            Message::from_line(
                r#"{"type":"state","v":1,"fix":{"status":"stale","lat":1.0},"sources":[]}"#
            )
            .unwrap_err(),
            ReadError::PositionPartial
        );
        assert_eq!(
            Message::from_line(
                r#"{"type":"state","v":1,"fix":{"status":"none","lat":1.0,"lon":2.0},"sources":[]}"#
            )
            .unwrap_err(),
            ReadError::PositionForbidden
        );
        assert_eq!(
            Message::from_line(
                r#"{"type":"state","v":1,"fix":{"status":"nofix","lat":1.0,"lon":2.0},"sources":[]}"#
            )
            .unwrap_err(),
            ReadError::PositionMissing
        );
        assert_eq!(
            Message::from_line(
                r#"{"type":"targets","v":1,"targets":[{"mmsi":1,"lon":2.0,"danger":false}]}"#
            )
            .unwrap_err(),
            ReadError::PositionPartial
        );
        assert_eq!(
            Message::from_line(r#"{"type":"state","v":1,"fix":{"status":"lost"},"sources":[]}"#)
                .unwrap_err(),
            ReadError::UnknownStatus
        );
        assert_eq!(
            Message::from_line(
                r#"{"type":"state","v":1,"fix":{"status":"none"},"sources":[{"name":"a","status":"asleep","sentences":0,"rejected":0}]}"#
            )
            .unwrap_err(),
            ReadError::UnknownStatus
        );
    }

    fn fenced_json(protocol: &str) -> Vec<&str> {
        protocol
            .split("```json\n")
            .skip(1)
            .map(|rest| rest.split_once("```").unwrap().0)
            .collect()
    }

    fn assert_same_keys(left: &Value, right: &Value) {
        match (left, right) {
            (Value::Object(a), Value::Object(b)) => {
                let mut a_keys: Vec<&str> = a.keys().map(String::as_str).collect();
                let mut b_keys: Vec<&str> = b.keys().map(String::as_str).collect();
                a_keys.sort_unstable();
                b_keys.sort_unstable();
                assert_eq!(a_keys, b_keys, "{left} vs {right}");
                for key in a_keys {
                    assert_same_keys(&a[key], &b[key]);
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                assert_eq!(a.len(), b.len());
                for (left, right) in a.iter().zip(b) {
                    assert_same_keys(left, right);
                }
            }
            (Value::Object(_) | Value::Array(_), _) | (_, Value::Object(_) | Value::Array(_)) => {
                panic!("shape changed: {left} vs {right}");
            }
            _ => {}
        }
    }
}
