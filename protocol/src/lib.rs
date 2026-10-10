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
    pub fn none() -> Fix {
        Fix {
            inner: FixInner::None,
        }
    }

    pub fn nofix(last: Option<Place>) -> Fix {
        Fix {
            inner: FixInner::Nofix(last),
        }
    }

    pub fn ok(place: Place) -> Fix {
        Fix {
            inner: FixInner::Ok(place),
        }
    }

    pub fn stale(place: Place) -> Fix {
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

    pub fn satellites(&self) -> Option<u8> {
        self.place().and_then(|place| place.satellites)
    }

    pub fn hdop(&self) -> Option<f64> {
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
