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
//! Only news is written. A TCP or serial source that can't be reached
//! retries every 2 seconds and fails the same way each time; that is one
//! line, not one every 2 seconds for as long as the link is down. A source
//! that is only briefly silent, like AIS on an empty bay, isn't news either.

use crate::fix::STALE_AFTER;
use crate::protocol::{FixState, FixStatus, SourceStatus};
use std::{
    io::{self, Write},
    sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    thread,
};
use tokio::time::{Duration, Instant};

/// Different error messages remembered per outage. A link that alternates
/// between two failures says each once; past this many, it goes quiet until
/// it recovers rather than filling the recording.
const MESSAGES_PER_OUTAGE: usize = 8;

/// How long a source must have been quiet before that is written down. An
/// AIS receiver on a quiet bay goes minutes between vessels and is fine; a
/// GPS quiet this long is not.
pub const QUIET_HOLD: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Journal {
    sources: Vec<Seen>,
    fix: Option<FixStatus>,
}

#[derive(Default)]
struct Seen {
    /// The status last written.
    status: Option<SourceStatus>,
    /// The error messages already written since the source last sent a
    /// line.
    errors: Vec<String>,
    /// When the link was last reached, until it sends a line, fails, or
    /// goes quiet.
    connected: Option<Instant>,
    /// `connected · nothing heard` has been written since the source last
    /// sent a line. A peer that keeps accepting, sitting silent and hanging
    /// up says so once, not once a cycle.
    told_connected: bool,
}

impl Journal {
    fn seen(&mut self, index: usize) -> &mut Seen {
        if self.sources.len() <= index {
            self.sources.resize_with(index + 1, Seen::default);
        }
        &mut self.sources[index]
    }

    /// A TCP connection made or a device opened. Nothing is written yet:
    /// lines arriving say it better, and if none do the hub turns the
    /// source `quiet`, which is when it is written.
    pub fn connected(&mut self, index: usize, now: Instant) {
        self.seen(index).connected = Some(now);
    }

    /// A source's status as the hub now has it, and how long since it last
    /// sent a line or was reached. Returns the line to write when it is
    /// news.
    pub fn source(
        &mut self,
        index: usize,
        name: &str,
        status: SourceStatus,
        message: Option<&str>,
        silent: Option<Duration>,
        now: Instant,
    ) -> Option<String> {
        let name = one_line(name);
        let seen = self.seen(index);
        match status {
            SourceStatus::Error => {
                seen.connected = None;
                // One line: a message broken across two would leave half of
                // it in the recording as if it were a received line.
                let message = one_line(message.unwrap_or(""));
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
            SourceStatus::Connecting => {
                // Reached and waiting for a first line, or retrying after an
                // error: either way, the same outage.
                if seen.connected.is_some()
                    || seen.status.is_some_and(|s| {
                        matches!(s, SourceStatus::Error | SourceStatus::Connecting)
                    })
                {
                    return None;
                }
                seen.status = Some(status);
                Some(format!("source {name} connecting"))
            }
            SourceStatus::Quiet => {
                if let Some(at) = seen.connected.take() {
                    // Reached, and silent ever since: the link is fine and
                    // whatever is behind it isn't talking. A different story
                    // from the error before it, so it isn't held back.
                    seen.status = Some(status);
                    if std::mem::replace(&mut seen.told_connected, true) {
                        return None;
                    }
                    return Some(format!(
                        "source {name} connected · nothing heard for {} s",
                        now.saturating_duration_since(at).as_secs()
                    ));
                }
                let silent = silent.unwrap_or_default();
                if silent < STALE_AFTER + QUIET_HOLD || seen.status == Some(status) {
                    return None;
                }
                seen.status = Some(status);
                Some(format!(
                    "source {name} quiet · nothing for {} s",
                    silent.as_secs()
                ))
            }
            SourceStatus::Ok | SourceStatus::Ended => {
                seen.connected = None;
                // A line arrived: whatever went wrong before is over.
                seen.errors.clear();
                seen.told_connected = false;
                // Quiet that never lasted long enough to be written ends
                // without a word, so ok is only news after something else.
                if seen.status == Some(status) {
                    return None;
                }
                seen.status = Some(status);
                Some(format!("source {name} {}", word(status)))
            }
        }
    }

    /// The fix as the hub now judges it, and whether any source has sent a
    /// line in the last `STALE_AFTER`. Returns the line to write when its
    /// status has changed.
    ///
    /// The satellites and HDOP go with it, because a receiver losing the sky
    /// loses satellites first: a stale fix that last had 4 satellites is a
    /// different story from one that had 11. A stale fix also says whether
    /// sentences were still arriving, since a quiet link is only written
    /// after `QUIET_HOLD` and a short dropout would otherwise say nothing
    /// about which it was.
    pub fn fix(&mut self, fix: &FixState, arriving: bool) -> Option<String> {
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
        if fix.status == FixStatus::Stale {
            line.push_str(if arriving {
                " · sentences still arriving"
            } else {
                " · no sentences"
            });
        }
        Some(line)
    }

    /// Forget what was written, so every source and the fix are written
    /// again as they stand: for when lines were lost on the way to the disk.
    /// A link still waiting on its first line stays waiting.
    pub fn forget(&mut self) {
        self.fix = None;
        for seen in &mut self.sources {
            seen.status = None;
            seen.errors.clear();
            seen.told_connected = false;
        }
    }
}

fn one_line(text: &str) -> String {
    text.replace(|c: char| c.is_control(), " ")
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

/// Lines for stderr that haven't been written yet. Past this, they're
/// dropped and counted.
const STDERR_QUEUE: usize = 256;

/// stderr, written on a thread of its own. `eprintln!` panics when stderr
/// is a closed pipe or a full disk, and blocks while a pipe is full; on the
/// hub's single-threaded runtime either would stop navigation for the sake
/// of a log line. Here a line that can't be queued is dropped, and one that
/// can't be written is lost, and the hub never waits.
#[derive(Clone)]
pub struct Stderr {
    lines: SyncSender<String>,
}

impl Stderr {
    pub fn spawn() -> Stderr {
        Stderr::to(io::stderr())
    }

    pub fn to<W: Write + Send + 'static>(mut out: W) -> Stderr {
        let (lines, rx): (SyncSender<String>, Receiver<String>) = sync_channel(STDERR_QUEUE);
        thread::spawn(move || {
            for line in rx {
                let _ = writeln!(out, "{line}");
                let _ = out.flush();
            }
        });
        Stderr { lines }
    }

    /// Queues a line, never waiting. False when it was dropped.
    pub fn say(&self, line: String) -> bool {
        match self.lines.try_send(line) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPS: &str = "tcp:10.0.2.2:10110";

    fn t0() -> Instant {
        Instant::now()
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// A status with no time to it: not quiet, not newly connected.
    fn at(j: &mut Journal, status: SourceStatus, message: Option<&str>) -> Option<String> {
        j.source(0, GPS, status, message, None, Instant::now())
    }

    #[test]
    fn a_link_that_keeps_failing_the_same_way_is_one_line() {
        let mut j = Journal::default();
        assert_eq!(
            at(&mut j, SourceStatus::Connecting, None).as_deref(),
            Some("source tcp:10.0.2.2:10110 connecting")
        );
        let refused = Some("10.0.2.2:10110: Connection refused (os error 111)");
        assert_eq!(
            at(&mut j, SourceStatus::Error, refused).as_deref(),
            Some(
                "source tcp:10.0.2.2:10110 error: 10.0.2.2:10110: Connection refused (os error 111)"
            )
        );
        for _ in 0..30 {
            assert_eq!(at(&mut j, SourceStatus::Error, refused), None);
            assert_eq!(at(&mut j, SourceStatus::Connecting, None), None);
        }
    }

    #[test]
    fn a_new_failure_is_news_and_an_old_one_coming_back_is_not() {
        let mut j = Journal::default();
        let closed = Some("10.0.2.2:10110: connection closed");
        let refused = Some("10.0.2.2:10110: Connection refused");
        assert!(at(&mut j, SourceStatus::Error, closed).is_some());
        assert!(at(&mut j, SourceStatus::Error, refused).is_some());
        // Alternating between the two says nothing more.
        for _ in 0..10 {
            assert_eq!(at(&mut j, SourceStatus::Error, closed), None);
            assert_eq!(at(&mut j, SourceStatus::Error, refused), None);
        }
        // Working again ends the outage, so the next failure is written.
        assert_eq!(
            at(&mut j, SourceStatus::Ok, None).as_deref(),
            Some("source tcp:10.0.2.2:10110 ok")
        );
        assert!(at(&mut j, SourceStatus::Error, closed).is_some());
    }

    #[test]
    fn a_link_with_ever_new_messages_runs_out_of_lines() {
        let mut j = Journal::default();
        let written = (0..100)
            .filter(|n| at(&mut j, SourceStatus::Error, Some(&format!("try {n}"))).is_some())
            .count();
        assert_eq!(written, MESSAGES_PER_OUTAGE);
    }

    #[test]
    fn a_message_and_a_name_stay_on_one_line() {
        let mut j = Journal::default();
        let line = j
            .source(
                0,
                "tcp:a\nb:1",
                SourceStatus::Error,
                Some("bad\n1790 $GPRMC\r"),
                None,
                t0(),
            )
            .unwrap();
        assert_eq!(line, "source tcp:a b:1 error: bad 1790 $GPRMC ");
    }

    #[test]
    fn a_short_quiet_is_not_news_and_neither_is_its_end() {
        // AIS on a quiet bay: a line every 20 s, quiet in between.
        let mut j = Journal::default();
        let now = t0();
        assert!(at(&mut j, SourceStatus::Ok, None).is_some());
        for _ in 0..50 {
            let quiet = j.source(0, GPS, SourceStatus::Quiet, None, Some(secs(20)), now);
            assert_eq!(quiet, None);
            assert_eq!(at(&mut j, SourceStatus::Ok, None), None);
        }
    }

    #[test]
    fn a_long_quiet_is_written_and_so_is_its_end() {
        let mut j = Journal::default();
        let now = t0();
        assert!(at(&mut j, SourceStatus::Ok, None).is_some());
        let quiet =
            |j: &mut Journal, s| j.source(0, GPS, SourceStatus::Quiet, None, Some(secs(s)), now);
        assert_eq!(quiet(&mut j, 34), None);
        assert_eq!(
            quiet(&mut j, 35).as_deref(),
            Some("source tcp:10.0.2.2:10110 quiet · nothing for 35 s")
        );
        assert_eq!(quiet(&mut j, 60), None, "once");
        assert_eq!(
            at(&mut j, SourceStatus::Ok, None).as_deref(),
            Some("source tcp:10.0.2.2:10110 ok")
        );
    }

    #[test]
    fn a_link_back_but_silent_says_so_when_it_goes_quiet() {
        let mut j = Journal::default();
        let start = t0();
        let closed = Some("10.0.2.2:10110: connection closed");
        let src =
            |j: &mut Journal, status, message, now| j.source(0, GPS, status, message, None, now);
        assert!(src(&mut j, SourceStatus::Error, closed, start).is_some());
        // The bridge is back: the hub marks the source connecting...
        j.connected(0, start + secs(2));
        assert_eq!(
            src(&mut j, SourceStatus::Connecting, None, start + secs(3)),
            None
        );
        // ...then quiet once it has sent nothing for 5 s, and that is
        // written at once, not after the quiet hold.
        assert_eq!(
            src(&mut j, SourceStatus::Quiet, None, start + secs(7)).as_deref(),
            Some("source tcp:10.0.2.2:10110 connected · nothing heard for 5 s")
        );
        assert_eq!(
            src(&mut j, SourceStatus::Quiet, None, start + secs(9)),
            None
        );
        // A line at last.
        assert_eq!(
            src(&mut j, SourceStatus::Ok, None, start + secs(20)).as_deref(),
            Some("source tcp:10.0.2.2:10110 ok")
        );
    }

    #[test]
    fn a_peer_that_sits_silent_and_hangs_up_says_so_once() {
        // Connect, nothing for more than 5 s, close; over and over.
        let mut j = Journal::default();
        let start = t0();
        let closed = Some("10.0.2.2:10110: connection closed");
        let mut written = Vec::new();
        let mut now = start;
        for _ in 0..20 {
            written.extend(j.source(0, GPS, SourceStatus::Error, closed, None, now));
            now += secs(2);
            j.connected(0, now);
            written.extend(j.source(0, GPS, SourceStatus::Connecting, None, None, now));
            now += secs(6);
            written.extend(j.source(0, GPS, SourceStatus::Quiet, None, Some(secs(6)), now));
            now += secs(1);
        }
        assert_eq!(
            written,
            [
                "source tcp:10.0.2.2:10110 error: 10.0.2.2:10110: connection closed",
                "source tcp:10.0.2.2:10110 connected · nothing heard for 6 s",
            ]
        );
    }

    #[test]
    fn a_bridge_that_hangs_up_at_once_is_not_back() {
        // socat with no device behind it: accept, close, every 2 s.
        let mut j = Journal::default();
        let start = t0();
        let closed = Some("10.0.2.2:10110: connection closed");
        assert!(
            j.source(0, GPS, SourceStatus::Error, closed, None, start)
                .is_some()
        );
        for n in 1..50 {
            let now = start + secs(2 * n);
            j.connected(0, now);
            assert_eq!(
                j.source(0, GPS, SourceStatus::Connecting, None, None, now),
                None
            );
            let now = now + Duration::from_millis(10);
            assert_eq!(
                j.source(0, GPS, SourceStatus::Error, closed, None, now),
                None
            );
        }
    }

    #[test]
    fn sources_are_judged_apart() {
        let mut j = Journal::default();
        let now = t0();
        assert!(at(&mut j, SourceStatus::Ok, None).is_some());
        let ais = j.source(
            1,
            "serial:/dev/ttyACM0:38400",
            SourceStatus::Ok,
            None,
            None,
            now,
        );
        assert!(ais.is_some());
        assert_eq!(at(&mut j, SourceStatus::Ok, None), None);
    }

    #[test]
    fn forgetting_writes_everything_again() {
        let mut j = Journal::default();
        let refused = Some("refused");
        assert!(at(&mut j, SourceStatus::Error, refused).is_some());
        assert!(j.fix(&state(FixStatus::Stale, None, None), false).is_some());
        j.forget();
        assert!(at(&mut j, SourceStatus::Error, refused).is_some());
        assert!(j.fix(&state(FixStatus::Stale, None, None), false).is_some());
    }

    #[test]
    fn forgetting_keeps_a_link_waiting_on_its_first_line() {
        let mut j = Journal::default();
        let start = t0();
        j.connected(0, start);
        j.forget();
        assert_eq!(
            j.source(
                0,
                GPS,
                SourceStatus::Quiet,
                None,
                Some(secs(5)),
                start + secs(5)
            )
            .as_deref(),
            Some("source tcp:10.0.2.2:10110 connected · nothing heard for 5 s")
        );
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
            j.fix(&state(FixStatus::None, None, None), false).as_deref(),
            Some("fix none")
        );
        assert_eq!(
            j.fix(&state(FixStatus::Ok, Some(9), Some(0.9)), true)
                .as_deref(),
            Some("fix ok · 9 satellites, hdop 0.9")
        );
        // Satellites coming and going aren't news on their own.
        assert_eq!(j.fix(&state(FixStatus::Ok, Some(7), Some(1.4)), true), None);
        assert_eq!(
            j.fix(&state(FixStatus::Stale, Some(4), Some(3.1)), false)
                .as_deref(),
            Some("fix stale · last 4 satellites, hdop 3.1 · no sentences")
        );
        assert_eq!(
            j.fix(&state(FixStatus::Nofix, None, None), true).as_deref(),
            Some("fix nofix")
        );
        assert_eq!(
            j.fix(&state(FixStatus::Stale, None, None), true).as_deref(),
            Some("fix stale · sentences still arriving")
        );
    }

    #[test]
    fn local_time_is_a_readable_stamp() {
        let stamp = local_time(1_790_012_127);
        assert_eq!(stamp.len(), 19, "{stamp}");
        assert!(stamp.starts_with("2026-09-2"), "{stamp}");
    }

    /// stderr as a pipe nobody reads: every write waits forever.
    struct Stuck;
    impl Write for Stuck {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            thread::sleep(Duration::from_secs(3600));
            Ok(0)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// stderr as a pipe whose reader has gone.
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }

    #[test]
    fn a_stuck_stderr_never_holds_up_the_hub() {
        let err = Stderr::to(Stuck);
        let start = Instant::now();
        let kept = (0..10_000).filter(|n| err.say(format!("line {n}"))).count();
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(kept <= STDERR_QUEUE + 1, "{kept}");
    }

    #[test]
    fn a_broken_stderr_is_shrugged_off() {
        let err = Stderr::to(Broken);
        for n in 0..1000 {
            err.say(format!("line {n}"));
        }
        thread::sleep(Duration::from_millis(50));
        assert!(err.say("still here".into()), "the writer thread is alive");
    }
}
