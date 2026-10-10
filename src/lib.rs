//! omakeel, the Omahoy data hub: sentences in from the boat's instruments,
//! one live state out to every app. `docs/protocol.md` is the contract.

pub(crate) mod ais;
pub(crate) mod fix;
pub mod hub;
pub(crate) mod journal;
pub(crate) mod nmea;
pub mod protocol;
pub mod source;
pub(crate) mod targets;
pub mod watch;
