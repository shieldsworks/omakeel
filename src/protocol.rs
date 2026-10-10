//! Wire messages for the hub. The implementation is the `omakeel-protocol`
//! crate, re-exported here so the daemon and a client share one type.

pub use omakeel_protocol::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

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

    fn contains_null(value: &Value) -> bool {
        match value {
            Value::Null => true,
            Value::Array(items) => items.iter().any(contains_null),
            Value::Object(fields) => fields.values().any(contains_null),
            Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
        }
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
