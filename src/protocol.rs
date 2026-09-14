//! Hub-to-app messages: newline-delimited JSON, version 1.
//! `docs/protocol.md` is the contract; change both together.

use serde::Serialize;

pub const VERSION: u32 = 1;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message<'a> {
    Hello {
        v: u32,
        keel: &'a str,
    },
    State {
        v: u32,
        fix: &'a FixState,
        sources: &'a [SourceState],
    },
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FixState {
    pub status: FixStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sog_kn: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cog_deg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub satellites: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hdop: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<u64>,
}

impl FixState {
    pub fn without_position(status: FixStatus) -> FixState {
        FixState {
            status,
            lat: None,
            lon: None,
            sog_kn: None,
            cog_deg: None,
            utc: None,
            satellites: None,
            hdop: None,
            age_seconds: None,
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
    /// Can't be opened or reached; retried every two seconds.
    Error,
    /// A replay that has reached the end of its recording.
    Ended,
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
