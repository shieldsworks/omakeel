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

#[cfg(test)]
mod tests {
    #[test]
    fn the_client_crate_reads_the_line_the_daemon_writes() {
        let line = crate::protocol::Message::Hello {
            v: crate::protocol::VERSION,
            keel: "0.1.0".to_owned(),
        }
        .to_line();
        assert_eq!(
            omakeel_protocol::Message::from_line(&line),
            Ok(Some(omakeel_protocol::Message::Hello {
                v: 1,
                keel: "0.1.0".to_owned(),
            }))
        );
    }

    #[test]
    fn the_daemon_and_the_client_crate_are_one_type() {
        assert_eq!(
            std::any::TypeId::of::<crate::protocol::Message>(),
            std::any::TypeId::of::<omakeel_protocol::Message>()
        );
    }
}
