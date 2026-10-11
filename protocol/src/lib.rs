//! Hub-to-app messages: newline-delimited JSON, version 1.
//! `docs/protocol.md` is the contract; change both together.

use serde::{Deserialize, Serialize, Serializer};
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
    /// `Ok(None)` is a blank line, a version-1 `type` this parser does not know,
    /// or a version-1 `state` that omits `fix`.
    /// A `v` other than 1 is [`ReadError::Version`]. A hello whose `keel` is
    /// not a string, or a targets line that is not the version-1 shape, is
    /// [`ReadError::NotJson`]. A version-1 `state` that has a `fix` key is
    /// returned even when a field is missing, null, or the wrong JSON type.
    pub fn from_line(line: &str) -> Result<Option<Message>, ReadError> {
        let line = line.strip_suffix('\n').unwrap_or(line);
        if line.is_empty() {
            return Ok(None);
        }
        let value: Value = serde_json::from_str(line).map_err(|_| ReadError::NotJson)?;
        let Some(obj) = value.as_object() else {
            return Err(ReadError::NotJson);
        };
        require_version(obj)?;
        match obj.get("type").and_then(Value::as_str) {
            Some("hello") => hello(obj).map(Some),
            Some("state") => Ok(state(obj)),
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
    /// Not JSON, not an object, or a hello or targets line that is not the version-1 shape.
    NotJson,
    /// `v` is missing or not the integer 1. `found` is that integer when `v` is a `u64` other than 1.
    Version { found: Option<u64> },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::NotJson => f.write_str("not a version 1 message"),
            ReadError::Version { found: None } => f.write_str("missing protocol version"),
            ReadError::Version { found: Some(found) } => {
                write!(f, "protocol version {found}")
            }
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
    Ok(Option<Place>),
    Stale(Option<Place>),
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

impl Fix {
    pub fn none() -> Fix {
        Fix::with_status(FixStatus::None, None)
    }

    pub fn nofix(last: Option<Place>) -> Fix {
        Fix::with_status(FixStatus::Nofix, last)
    }

    pub fn ok(place: Place) -> Fix {
        Fix::with_status(FixStatus::Ok, Some(place))
    }

    pub fn stale(place: Place) -> Fix {
        Fix::with_status(FixStatus::Stale, Some(place))
    }

    fn with_status(status: FixStatus, place: Option<Place>) -> Fix {
        Fix {
            inner: match status {
                FixStatus::None => FixInner::None,
                FixStatus::Nofix => FixInner::Nofix(place),
                FixStatus::Ok => FixInner::Ok(place),
                FixStatus::Stale => FixInner::Stale(place),
            },
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

    /// `None` when the place is absent.
    /// `current` is true only for an `ok` fix that has a place.
    pub fn position(&self) -> Option<Position> {
        Some(Position {
            place: self.place()?.clone(),
            current: matches!(self.inner, FixInner::Ok(Some(_))),
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
            FixInner::Nofix(place) | FixInner::Ok(place) | FixInner::Stale(place) => place.as_ref(),
            FixInner::None => None,
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

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceState {
    /// The source as given on the command line, like `serial:/dev/ttyUSB0:4800`.
    pub name: String,
    pub status: SourceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    let found = obj.get("v").and_then(Value::as_u64);
    if found == Some(u64::from(VERSION)) {
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

fn state(obj: &Map<String, Value>) -> Option<Message> {
    let fix = fix_from_value(obj.get("fix")?);
    Some(Message::State {
        v: VERSION,
        fix,
        sources: sources_from(obj),
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

fn sources_from(obj: &Map<String, Value>) -> Vec<SourceState> {
    let Some(items) = obj.get("sources").and_then(Value::as_array) else {
        return Vec::new();
    };
    items.iter().filter_map(source_from_value).collect()
}

fn source_from_value(value: &Value) -> Option<SourceState> {
    let obj = value.as_object()?;
    let name = obj.get("name").and_then(Value::as_str)?.to_owned();
    let status = source_status(obj.get("status")?)?;
    Some(SourceState {
        name,
        status,
        message: obj
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned),
        sentences: obj.get("sentences").and_then(whole_u64).unwrap_or(0),
        rejected: obj.get("rejected").and_then(whole_u64).unwrap_or(0),
    })
}

fn source_status(value: &Value) -> Option<SourceStatus> {
    match value.as_str()? {
        "connecting" => Some(SourceStatus::Connecting),
        "ok" => Some(SourceStatus::Ok),
        "quiet" => Some(SourceStatus::Quiet),
        "error" => Some(SourceStatus::Error),
        "ended" => Some(SourceStatus::Ended),
        _ => None,
    }
}

fn target_from_value(value: &Value) -> Result<Target, ReadError> {
    let target = Target::deserialize(value).map_err(|_| ReadError::NotJson)?;
    if target.lat.is_some() != target.lon.is_some() {
        return Err(ReadError::NotJson);
    }
    Ok(target)
}

fn fix_from_value(value: &Value) -> Fix {
    let Some(obj) = value.as_object() else {
        return Fix::none();
    };
    let place = place_from(obj);
    match obj.get("status").and_then(Value::as_str) {
        None | Some("none") => Fix::none(),
        Some("nofix") => Fix::nofix(place),
        Some("ok") => Fix::with_status(FixStatus::Ok, place),
        Some("stale") => Fix::with_status(FixStatus::Stale, place),
        Some(_) => match place {
            Some(place) => Fix::stale(place),
            None => Fix::none(),
        },
    }
}

fn place_from(obj: &Map<String, Value>) -> Option<Place> {
    let lat = finite_f64(obj.get("lat")?)?;
    let lon = finite_f64(obj.get("lon")?)?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    Some(Place {
        lat,
        lon,
        sog_kn: obj.get("sogKn").and_then(finite_f64),
        cog_deg: obj.get("cogDeg").and_then(finite_f64),
        utc: obj.get("utc").and_then(Value::as_str).map(str::to_owned),
        satellites: obj.get("satellites").and_then(satellite_count),
        hdop: obj.get("hdop").and_then(finite_f64),
        age_seconds: obj.get("ageSeconds").and_then(whole_u64).unwrap_or(0),
    })
}

fn whole_u64(value: &Value) -> Option<u64> {
    if let Some(count) = value.as_u64() {
        return Some(count);
    }
    let number = value.as_f64().filter(|number| number.is_finite())?;
    if !(0.0..2.0_f64.powi(64)).contains(&number) || number.fract() != 0.0 {
        return None;
    }
    Some(number as u64)
}

fn finite_f64(value: &Value) -> Option<f64> {
    value.as_f64().filter(|number| number.is_finite())
}

fn satellite_count(value: &Value) -> Option<u8> {
    let count = finite_f64(value)?;
    if (0.0..=255.0).contains(&count) {
        Some(count as u8)
    } else {
        None
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
