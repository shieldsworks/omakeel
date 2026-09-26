//! What happened to the links and the fix, as it happened: the lines that
//! make a recording say why the GPS dropped, not just that it did.
//!
//! A recording on its own shows sentences stopping. It can't tell a receiver
//! that lost the sky from a cable that came out, or from a network bridge
//! that went away, because in all three the sentences simply stop. The hub
//! knows which it was, so it writes it down: a comment line in the recording
//! and a line on stderr each time a source's status or the fix's status
//! changes.
//!
//! Only changes are written. A TCP or serial source that can't be reached
//! retries every 2 seconds and fails the same way each time; that is one
//! line, not one every 2 seconds for as long as the link is down.

use crate::protocol::{FixState, FixStatus, SourceStatus};

/// Different error messages remembered per outage. A link that alternates
/// between two failures says each once; past this many, it goes quiet until
/// it recovers rather than filling the recording.
const MESSAGES_PER_OUTAGE: usize = 8;

#[derive(Default)]
pub struct Journal {
    sources: Vec<Seen>,
    fix: Option<FixStatus>,
}

#[derive(Default)]
struct Seen {
    status: Option<SourceStatus>,
    /// The error messages already written since the source last worked.
    errors: Vec<String>,
}

impl Journal {
    /// A source's status as the hub now has it. Returns the line to write
    /// when it is news.
    pub fn source(
        &mut self,
        index: usize,
        name: &str,
        status: SourceStatus,
        message: Option<&str>,
    ) -> Option<String> {
        if self.sources.len() <= index {
            self.sources.resize_with(index + 1, Seen::default);
        }
        let seen = &mut self.sources[index];
        match status {
            SourceStatus::Error => {
                // One line: a message broken across two would leave half of
                // it in the recording as if it were a received line.
                let message = message.unwrap_or("").replace(|c: char| c.is_control(), " ");
                if seen.errors.contains(&message) || seen.errors.len() >= MESSAGES_PER_OUTAGE {
                    return None;
                }
                seen.errors.push(message.clone());
                seen.status = Some(status);
                Some(if message.is_empty() {
                    format!("source {name} error")
                } else {
                    format!("source {name} error: {message}")
                })
            }
            // Retrying after an error is still the same outage.
            SourceStatus::Connecting if seen.status == Some(SourceStatus::Error) => None,
            _ if seen.status == Some(status) => None,
            _ => {
                if matches!(status, SourceStatus::Ok | SourceStatus::Quiet) {
                    seen.errors.clear();
                }
                seen.status = Some(status);
                Some(format!("source {name} {}", word(status)))
            }
        }
    }

    /// The fix as the hub now judges it. Returns the line to write when its
    /// status has changed. The satellites and HDOP go with it, because a
    /// receiver losing the sky loses satellites first: a stale fix that
    /// last had 4 satellites is a different story from one that had 11.
    pub fn fix(&mut self, fix: &FixState) -> Option<String> {
        if self.fix == Some(fix.status) {
            return None;
        }
        self.fix = Some(fix.status);
        let mut line = format!("fix {}", fix_word(fix.status));
        let last = if fix.status == FixStatus::Ok {
            ""
        } else {
            "last "
        };
        match (fix.satellites, fix.hdop) {
            (Some(n), Some(h)) => line.push_str(&format!(" · {last}{n} satellites, hdop {h}")),
            (Some(n), None) => line.push_str(&format!(" · {last}{n} satellites")),
            (None, Some(h)) => line.push_str(&format!(" · {last}hdop {h}")),
            (None, None) => {}
        }
        Some(line)
    }
}

fn word(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::Connecting => "connecting",
        SourceStatus::Ok => "ok",
        SourceStatus::Quiet => "quiet",
        SourceStatus::Error => "error",
        SourceStatus::Ended => "ended",
    }
}

fn fix_word(status: FixStatus) -> &'static str {
    match status {
        FixStatus::None => "none",
        FixStatus::Nofix => "nofix",
        FixStatus::Ok => "ok",
        FixStatus::Stale => "stale",
    }
}

/// `2026-09-21 13:50:12`, this machine's local time, for stderr. The
/// recording carries Unix milliseconds instead, as its sentences do.
pub fn local_time(unix_secs: i64) -> String {
    // SAFETY: `localtime_r` fills the zeroed `tm` it is given, or fails and
    // leaves it zeroed; neither reads anything else.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t = unix_secs as libc::time_t;
        if libc::localtime_r(&t, &mut tm).is_null() {
            return format!("@{unix_secs}");
        }
        tm
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPS: &str = "tcp:10.0.2.2:10110";

    #[test]
    fn a_link_that_keeps_failing_the_same_way_is_one_line() {
        let mut j = Journal::default();
        assert_eq!(
            j.source(0, GPS, SourceStatus::Connecting, None).as_deref(),
            Some("source tcp:10.0.2.2:10110 connecting")
        );
        let refused = Some("10.0.2.2:10110: Connection refused (os error 111)");
        assert_eq!(
            j.source(0, GPS, SourceStatus::Error, refused).as_deref(),
            Some(
                "source tcp:10.0.2.2:10110 error: 10.0.2.2:10110: Connection refused (os error 111)"
            )
        );
        for _ in 0..30 {
            assert_eq!(j.source(0, GPS, SourceStatus::Error, refused), None);
            assert_eq!(j.source(0, GPS, SourceStatus::Connecting, None), None);
        }
    }

    #[test]
    fn a_new_failure_is_news_and_an_old_one_coming_back_is_not() {
        let mut j = Journal::default();
        let closed = Some("10.0.2.2:10110: connection closed");
        let refused = Some("10.0.2.2:10110: Connection refused");
        assert!(j.source(0, GPS, SourceStatus::Error, closed).is_some());
        assert!(j.source(0, GPS, SourceStatus::Error, refused).is_some());
        // Alternating between the two says nothing more.
        for _ in 0..10 {
            assert_eq!(j.source(0, GPS, SourceStatus::Error, closed), None);
            assert_eq!(j.source(0, GPS, SourceStatus::Error, refused), None);
        }
        // Working again ends the outage, so the next failure is written.
        assert_eq!(
            j.source(0, GPS, SourceStatus::Ok, None).as_deref(),
            Some("source tcp:10.0.2.2:10110 ok")
        );
        assert!(j.source(0, GPS, SourceStatus::Error, closed).is_some());
    }

    #[test]
    fn a_link_with_ever_new_messages_runs_out_of_lines() {
        let mut j = Journal::default();
        let written = (0..100)
            .filter(|n| {
                j.source(0, GPS, SourceStatus::Error, Some(&format!("try {n}")))
                    .is_some()
            })
            .count();
        assert_eq!(written, MESSAGES_PER_OUTAGE);
    }

    #[test]
    fn a_message_stays_on_one_line() {
        let mut j = Journal::default();
        let line = j
            .source(0, GPS, SourceStatus::Error, Some("bad\n1790 $GPRMC\r"))
            .unwrap();
        assert_eq!(line, "source tcp:10.0.2.2:10110 error: bad 1790 $GPRMC ");
    }

    #[test]
    fn quiet_and_back_is_written_both_ways() {
        let mut j = Journal::default();
        assert!(j.source(0, GPS, SourceStatus::Ok, None).is_some());
        assert_eq!(j.source(0, GPS, SourceStatus::Ok, None), None);
        assert_eq!(
            j.source(0, GPS, SourceStatus::Quiet, None).as_deref(),
            Some("source tcp:10.0.2.2:10110 quiet")
        );
        assert!(j.source(0, GPS, SourceStatus::Ok, None).is_some());
    }

    #[test]
    fn sources_are_judged_apart() {
        let mut j = Journal::default();
        assert!(j.source(0, GPS, SourceStatus::Ok, None).is_some());
        assert!(
            j.source(1, "serial:/dev/ttyACM0:38400", SourceStatus::Ok, None)
                .is_some()
        );
        assert_eq!(j.source(0, GPS, SourceStatus::Ok, None), None);
    }

    fn state(status: FixStatus, satellites: Option<u8>, hdop: Option<f64>) -> FixState {
        FixState {
            satellites,
            hdop,
            ..FixState::without_position(status)
        }
    }

    #[test]
    fn the_fix_is_written_when_its_status_changes_with_the_sky_it_had() {
        let mut j = Journal::default();
        assert_eq!(
            j.fix(&state(FixStatus::None, None, None)).as_deref(),
            Some("fix none")
        );
        assert_eq!(
            j.fix(&state(FixStatus::Ok, Some(9), Some(0.9))).as_deref(),
            Some("fix ok · 9 satellites, hdop 0.9")
        );
        // Satellites coming and going aren't news on their own.
        assert_eq!(j.fix(&state(FixStatus::Ok, Some(7), Some(1.4))), None);
        assert_eq!(
            j.fix(&state(FixStatus::Stale, Some(4), Some(3.1)))
                .as_deref(),
            Some("fix stale · last 4 satellites, hdop 3.1")
        );
        assert_eq!(
            j.fix(&state(FixStatus::Nofix, None, None)).as_deref(),
            Some("fix nofix")
        );
    }

    #[test]
    fn local_time_is_a_readable_stamp() {
        let stamp = local_time(1_790_012_127);
        assert_eq!(stamp.len(), 19, "{stamp}");
        assert!(stamp.starts_with("2026-09-2"), "{stamp}");
    }
}
