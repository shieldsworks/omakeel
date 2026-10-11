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
        state_of(line).0
    }

    fn state_of(line: &str) -> (Fix, Vec<SourceState>) {
        let Some(Message::State { fix, sources, .. }) = Message::from_line(line).unwrap() else {
            panic!("a state line");
        };
        (fix, sources)
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
    fn a_blank_line_is_absent_and_bad_json_is_rejected() {
        assert_eq!(Message::from_line("").unwrap(), None);
        assert_eq!(Message::from_line("\n").unwrap(), None);
        for line in [
            "not json",
            "[]",
            "null",
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":NaN,"lon":1}}"#,
            r#"{"type":"state","v":Infinity}"#,
            r#"{"type":"state","v":1e9999}"#,
            r#"{"type":"hello","v":1,"keel":null}"#,
        ] {
            assert_eq!(
                Message::from_line(line).unwrap_err(),
                ReadError::NotJson,
                "{line}"
            );
        }
    }

    #[test]
    fn version_is_an_integer_read_before_the_other_fields() {
        let missing = Message::from_line(r#"{"type":"future"}"#).unwrap_err();
        assert_eq!(missing, ReadError::Version { found: None });
        assert_eq!(missing.to_string(), "missing protocol version");
        let two = Message::from_line(r#"{"type":"future","v":2}"#).unwrap_err();
        assert_eq!(two, ReadError::Version { found: Some(2) });
        assert_eq!(two.to_string(), "protocol version 2");
        assert_eq!(
            Message::from_line(r#"{"type":"future","v":1}"#).unwrap(),
            None
        );
        assert_eq!(
            Message::from_line(r#"{"type":"nope","v":null}"#).unwrap_err(),
            ReadError::Version { found: None }
        );
        assert_eq!(
            Message::from_line(r#"{"type":"hello","v":2,"keel":null}"#).unwrap_err(),
            ReadError::Version { found: Some(2) }
        );
        assert_eq!(
            Message::from_line(r#"{"type":"hello","v":0,"keel":"x"}"#).unwrap_err(),
            ReadError::Version { found: Some(0) }
        );
        let huge =
            Message::from_line(r#"{"type":"state","v":4294967296,"fix":null,"sources":null}"#)
                .unwrap_err();
        assert_eq!(
            huge,
            ReadError::Version {
                found: Some(4_294_967_296)
            }
        );
        assert_eq!(huge.to_string(), "protocol version 4294967296");
        for line in [
            r#"{"type":"state","v":1.0,"fix":{"status":"ok"}}"#,
            r#"{"type":"state","v":1.5}"#,
            r#"{"type":"state","v":"1"}"#,
            r#"{"type":"state","v":-1}"#,
            r#"{"type":"state","v":true}"#,
        ] {
            assert_eq!(
                Message::from_line(line).unwrap_err(),
                ReadError::Version { found: None },
                "{line}"
            );
        }
    }

    #[test]
    fn a_state_without_a_fix_key_is_absent() {
        assert_eq!(
            Message::from_line(
                r#"{"type":"state","v":1,"sources":[{"name":"a","status":"ok","sentences":4,"rejected":1}]}"#
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn a_fix_that_is_not_an_object_is_none_and_keeps_sources() {
        let (fix, sources) = state_of(
            r#"{"type":"state","v":1,"fix":null,"sources":[{"name":"down","status":"error","message":"no carrier","sentences":1,"rejected":2}]}"#,
        );
        assert_eq!(fix.status(), FixStatus::None);
        assert_eq!(fix.position(), None);
        assert_eq!(
            sources,
            vec![SourceState {
                name: "down".into(),
                status: SourceStatus::Error,
                message: Some("no carrier".into()),
                sentences: 1,
                rejected: 2,
            }]
        );
        let (fix, sources) = state_of(r#"{"type":"state","v":1,"fix":"ok","sources":[]}"#);
        assert_eq!(fix.status(), FixStatus::None);
        assert_eq!(fix.position(), None);
        assert_eq!(sources, vec![]);
    }

    #[test]
    fn an_ok_place_is_current_with_its_age() {
        let fix = state_fix(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":4},"sources":[]}"#,
        );
        assert_eq!(fix.status(), FixStatus::Ok);
        let pos = fix.position().expect("ok");
        assert!(pos.current);
        assert_eq!(pos.place.lat, 37.865);
        assert_eq!(pos.place.lon, -122.32);
        assert_eq!(pos.place.age_seconds, 4);
    }

    #[test]
    fn a_missing_or_fractional_age_is_zero_and_the_fix_stays_current() {
        for line in [
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":1.0,"lon":2.0},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":1.0,"lon":2.0,"ageSeconds":null},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":1.0,"lon":2.0,"ageSeconds":0.0},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":1.0,"lon":2.0,"ageSeconds":1.5},"sources":[]}"#,
        ] {
            let fix = state_fix(line);
            assert_eq!(fix.status(), FixStatus::Ok, "{line}");
            let pos = fix.position().expect("ok");
            assert!(pos.current, "{line}");
            assert_eq!(pos.place.lat, 1.0, "{line}");
            assert_eq!(pos.place.lon, 2.0, "{line}");
            assert_eq!(pos.place.age_seconds, 0, "{line}");
        }
    }

    #[test]
    fn a_whole_number_float_is_the_integer_and_a_fraction_is_not() {
        for (age, expected) in [("5.0", 5), ("1e1", 10), ("12.0", 12), ("0.0", 0)] {
            let line = format!(
                r#"{{"type":"state","v":1,"fix":{{"status":"ok","lat":1.0,"lon":2.0,"ageSeconds":{age}}},"sources":[{{"name":"gps","status":"ok","sentences":{age},"rejected":{age}}}]}}"#
            );
            let (fix, sources) = state_of(&line);
            assert_eq!(
                fix.position().expect("ok").place.age_seconds,
                expected,
                "{line}"
            );
            assert_eq!(sources[0].sentences, expected, "{line}");
            assert_eq!(sources[0].rejected, expected, "{line}");
        }
        for age in ["5.5", "-1", "-5.0", "1e20"] {
            let line = format!(
                r#"{{"type":"state","v":1,"fix":{{"status":"ok","lat":1.0,"lon":2.0,"ageSeconds":{age}}},"sources":[{{"name":"gps","status":"ok","sentences":{age},"rejected":{age}}}]}}"#
            );
            let (fix, sources) = state_of(&line);
            assert_eq!(fix.status(), FixStatus::Ok, "{line}");
            assert!(fix.position().expect("ok").current, "{line}");
            assert_eq!(fix.position().expect("ok").place.age_seconds, 0, "{line}");
            assert_eq!(sources[0].sentences, 0, "{line}");
            assert_eq!(sources[0].rejected, 0, "{line}");
        }
        assert_eq!(
            Message::from_line(r#"{"type":"state","v":1.0,"fix":{"status":"ok","lat":1.0,"lon":2.0,"ageSeconds":5.0}}"#)
                .unwrap_err(),
            ReadError::Version { found: None }
        );
    }

    #[test]
    fn a_null_note_on_an_ok_state_keeps_the_place() {
        let fix = state_fix(
            r#"{"type":"state","v":1,"note":null,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":0},"sources":[]}"#,
        );
        assert_eq!(fix.status(), FixStatus::Ok);
        let pos = fix.position().expect("ok");
        assert!(pos.current);
        assert_eq!(pos.place.lat, 37.865);
        assert_eq!(pos.place.lon, -122.32);
        assert_eq!(pos.place.age_seconds, 0);
    }

    #[test]
    fn a_mistyped_optional_field_is_absent_and_the_place_stays() {
        let fix = state_fix(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":2,"sogKn":"fast"},"sources":[]}"#,
        );
        assert_eq!(fix.status(), FixStatus::Ok);
        let pos = fix.position().expect("ok");
        assert!(pos.current);
        assert_eq!(pos.place.lat, 37.865);
        assert_eq!(pos.place.lon, -122.32);
        assert_eq!(pos.place.age_seconds, 2);
        assert_eq!(pos.place.sog_kn, None);
    }

    #[test]
    fn satellites_truncate_inside_a_byte_and_are_absent_outside_it() {
        let over = state_fix(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":0,"satellites":300},"sources":[]}"#,
        );
        let pos = over.position().expect("ok");
        assert!(pos.current);
        assert_eq!(pos.place.satellites, None);
        assert_eq!(over.satellites(), None);
        let fraction = state_fix(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":0,"satellites":9.7},"sources":[]}"#,
        );
        let pos = fraction.position().expect("ok");
        assert!(pos.current);
        assert_eq!(pos.place.satellites, Some(9));
        assert_eq!(fraction.satellites(), Some(9));
        let full = state_fix(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":0,"satellites":255.0},"sources":[]}"#,
        );
        assert_eq!(full.position().expect("ok").place.satellites, Some(255));
    }

    #[test]
    fn a_degree_outside_range_or_one_coordinate_is_not_a_position() {
        for line in [
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":91,"lon":-122.32,"ageSeconds":1},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":181,"ageSeconds":1},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"ageSeconds":1},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lon":-122.32,"ageSeconds":1},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":"37.865","lon":"-122.32","ageSeconds":1},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":null,"lon":null,"ageSeconds":1},"sources":[]}"#,
        ] {
            let fix = state_fix(line);
            assert_eq!(fix.status(), FixStatus::Ok, "{line}");
            assert_eq!(fix.position(), None, "{line}");
        }
    }

    #[test]
    fn the_degree_limits_are_a_current_position() {
        for (line, lat, lon) in [
            (
                r#"{"type":"state","v":1,"fix":{"status":"ok","lat":90,"lon":180,"ageSeconds":0},"sources":[]}"#,
                90.0,
                180.0,
            ),
            (
                r#"{"type":"state","v":1,"fix":{"status":"ok","lat":-90,"lon":-180,"ageSeconds":0},"sources":[]}"#,
                -90.0,
                -180.0,
            ),
        ] {
            let fix = state_fix(line);
            assert_eq!(fix.status(), FixStatus::Ok);
            let pos = fix.position().expect("limit");
            assert!(pos.current);
            assert_eq!(pos.place.lat, lat);
            assert_eq!(pos.place.lon, lon);
            assert_eq!(pos.place.age_seconds, 0);
        }
    }

    #[test]
    fn none_and_a_missing_status_drop_a_place() {
        for line in [
            r#"{"type":"state","v":1,"fix":{"status":"none","lat":37.865,"lon":-122.32,"ageSeconds":3},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"lat":37.865,"lon":-122.32,"ageSeconds":3},"sources":[]}"#,
        ] {
            let fix = state_fix(line);
            assert_eq!(fix.status(), FixStatus::None, "{line}");
            assert_eq!(fix.position(), None, "{line}");
        }
    }

    #[test]
    fn an_unrecognized_status_with_a_place_is_stale() {
        for (line, lat, lon) in [
            (
                r#"{"type":"state","v":1,"fix":{"status":"survey","lat":37.865,"lon":-122.32,"ageSeconds":8},"sources":[]}"#,
                37.865,
                -122.32,
            ),
            (
                r#"{"type":"state","v":1,"fix":{"status":"NONE","lat":10,"lon":20,"ageSeconds":1},"sources":[]}"#,
                10.0,
                20.0,
            ),
        ] {
            let fix = state_fix(line);
            assert_eq!(fix.status(), FixStatus::Stale, "{line}");
            let pos = fix.position().expect("place");
            assert!(!pos.current, "{line}");
            assert_eq!(pos.place.lat, lat, "{line}");
            assert_eq!(pos.place.lon, lon, "{line}");
        }
    }

    #[test]
    fn an_unrecognized_status_without_a_place_is_none() {
        for line in [
            r#"{"type":"state","v":1,"fix":{"status":"survey"},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"survey","lat":91,"lon":0},"sources":[]}"#,
        ] {
            let fix = state_fix(line);
            assert_eq!(fix.status(), FixStatus::None, "{line}");
            assert_eq!(fix.position(), None, "{line}");
        }
    }

    #[test]
    fn nofix_keeps_a_place_that_is_not_current() {
        let fix = state_fix(
            r#"{"type":"state","v":1,"fix":{"status":"nofix","lat":37.865,"lon":-122.32},"sources":[]}"#,
        );
        assert_eq!(fix.status(), FixStatus::Nofix);
        let pos = fix.position().expect("nofix");
        assert!(!pos.current);
        assert_eq!(pos.place.lat, 37.865);
        assert_eq!(pos.place.lon, -122.32);
        assert_eq!(pos.place.age_seconds, 0);
        for line in [
            r#"{"type":"state","v":1,"fix":{"status":"nofix"},"sources":[]}"#,
            r#"{"type":"state","v":1,"fix":{"status":"nofix","lat":37.865},"sources":[]}"#,
        ] {
            let fix = state_fix(line);
            assert_eq!(fix.status(), FixStatus::Nofix, "{line}");
            assert_eq!(fix.position(), None, "{line}");
        }
    }

    #[test]
    fn stale_without_a_place_keeps_an_error_source() {
        let (fix, sources) = state_of(
            r#"{"type":"state","v":1,"fix":{"status":"stale"},"sources":[{"name":"tcp:10.0.2.2:10110","status":"error","message":"connection closed"}]}"#,
        );
        assert_eq!(fix.status(), FixStatus::Stale);
        assert_eq!(fix.position(), None);
        assert_eq!(
            sources,
            vec![SourceState {
                name: "tcp:10.0.2.2:10110".into(),
                status: SourceStatus::Error,
                message: Some("connection closed".into()),
                sentences: 0,
                rejected: 0,
            }]
        );
    }

    #[test]
    fn an_unknown_source_status_is_skipped() {
        let (fix, sources) = state_of(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":1},"sources":[{"name":"a","status":"asleep","sentences":3,"rejected":1}]}"#,
        );
        assert_eq!(fix.status(), FixStatus::Ok);
        let pos = fix.position().expect("ok");
        assert!(pos.current);
        assert_eq!(pos.place.lat, 37.865);
        assert_eq!(pos.place.lon, -122.32);
        assert_eq!(pos.place.age_seconds, 1);
        assert_eq!(sources, vec![]);

        let (fix, sources) = state_of(
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":1},"sources":[{"name":"a","status":"asleep"},{"name":"b","status":"error","message":"down","sentences":2,"rejected":3}]}"#,
        );
        assert_eq!(fix.status(), FixStatus::Ok);
        assert!(fix.position().expect("ok").current);
        assert_eq!(
            sources,
            vec![SourceState {
                name: "b".into(),
                status: SourceStatus::Error,
                message: Some("down".into()),
                sentences: 2,
                rejected: 3,
            }]
        );
    }

    #[test]
    fn a_source_without_a_name_is_skipped() {
        let (fix, sources) = state_of(
            r#"{"type":"state","v":1,"fix":{"status":"none"},"sources":[{"status":"ok","sentences":1,"rejected":0},{"name":"kept","status":"connecting"}]}"#,
        );
        assert_eq!(fix.status(), FixStatus::None);
        assert_eq!(fix.position(), None);
        assert_eq!(
            sources,
            vec![SourceState {
                name: "kept".into(),
                status: SourceStatus::Connecting,
                message: None,
                sentences: 0,
                rejected: 0,
            }]
        );
    }

    #[test]
    fn a_quiet_source_without_counts_is_kept() {
        let (_, sources) = state_of(
            r#"{"type":"state","v":1,"fix":{"status":"none"},"sources":[{"name":"gps","status":"quiet","message":null}]}"#,
        );
        assert_eq!(
            sources,
            vec![SourceState {
                name: "gps".into(),
                status: SourceStatus::Quiet,
                message: None,
                sentences: 0,
                rejected: 0,
            }]
        );
    }

    #[test]
    fn a_source_message_keeps_a_newline() {
        let (_, sources) = state_of(
            r#"{"type":"state","v":1,"fix":{"status":"none"},"sources":[{"name":"gps","status":"error","message":"line\nbreak","sentences":0,"rejected":0}]}"#,
        );
        assert_eq!(
            sources,
            vec![SourceState {
                name: "gps".into(),
                status: SourceStatus::Error,
                message: Some("line\nbreak".into()),
                sentences: 0,
                rejected: 0,
            }]
        );
    }

    #[test]
    fn a_missing_or_unshaped_sources_field_leaves_the_fix() {
        for line in [
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":0}}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":0},"sources":{}}"#,
            r#"{"type":"state","v":1,"fix":{"status":"ok","lat":37.865,"lon":-122.32,"ageSeconds":0},"sources":null}"#,
        ] {
            let (fix, sources) = state_of(line);
            assert_eq!(fix.status(), FixStatus::Ok, "{line}");
            let pos = fix.position().expect("ok");
            assert!(pos.current, "{line}");
            assert_eq!(pos.place.lat, 37.865, "{line}");
            assert_eq!(pos.place.lon, -122.32, "{line}");
            assert_eq!(pos.place.age_seconds, 0, "{line}");
            assert_eq!(sources, vec![], "{line}");
        }
    }

    #[test]
    fn a_target_with_one_coordinate_is_not_json() {
        assert_eq!(
            Message::from_line(
                r#"{"type":"targets","v":1,"targets":[{"mmsi":1,"lon":2.0,"danger":false}]}"#
            )
            .unwrap_err(),
            ReadError::NotJson
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
