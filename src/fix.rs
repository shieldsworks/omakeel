//! The boat's fix as omakeel knows it, and how fresh it is.

use crate::nmea::{Kind, Position};
use crate::protocol::{FixState, FixStatus};
use tokio::time::{Duration, Instant};

/// A fix older than this is stale: still shown, never passed off as current.
pub const STALE_AFTER: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Navigation {
    fix: Option<Fix>,
    /// When a position sentence of any kind last arrived.
    heard: Option<Instant>,
    /// The receiver is reporting no fix; cleared by the next good one.
    lost: bool,
}

struct Fix {
    lat: f64,
    lon: f64,
    at: Instant,
    sog_kn: Option<f64>,
    cog_deg: Option<f64>,
    utc: Option<String>,
    satellites: Option<u8>,
    hdop: Option<f64>,
}

impl Navigation {
    pub fn update(&mut self, p: Position, at: Instant) {
        self.heard = Some(at);
        if !p.valid {
            self.lost = true;
            return;
        }
        let (Some(lat), Some(lon)) = (p.lat, p.lon) else {
            return;
        };
        self.lost = false;
        let fix = self.fix.get_or_insert(Fix {
            lat,
            lon,
            at,
            sog_kn: None,
            cog_deg: None,
            utc: None,
            satellites: None,
            hdop: None,
        });
        fix.lat = lat;
        fix.lon = lon;
        fix.at = at;
        // Each sentence replaces its own fields, even with empties, so an
        // old speed never outlives the sentence that stopped sending it.
        match p.kind {
            Kind::Rmc => {
                fix.sog_kn = p.sog_kn;
                fix.cog_deg = p.cog_deg;
                fix.utc = p.utc;
            }
            Kind::Gga => {
                fix.satellites = p.satellites;
                fix.hdop = p.hdop;
            }
        }
    }

    pub fn state(&self, now: Instant) -> FixState {
        let Some(fix) = &self.fix else {
            let talking = self
                .heard
                .is_some_and(|t| now.saturating_duration_since(t) < STALE_AFTER);
            return FixState::without_position(if talking {
                FixStatus::Nofix
            } else {
                FixStatus::None
            });
        };
        let age = now.saturating_duration_since(fix.at);
        let status = if self.lost {
            FixStatus::Nofix
        } else if age >= STALE_AFTER {
            FixStatus::Stale
        } else {
            FixStatus::Ok
        };
        FixState {
            status,
            lat: Some(round7(fix.lat)),
            lon: Some(round7(fix.lon)),
            sog_kn: fix.sog_kn,
            cog_deg: fix.cog_deg,
            utc: fix.utc.clone(),
            satellites: fix.satellites,
            hdop: fix.hdop,
            age_seconds: Some(age.as_secs()),
        }
    }
}

/// Seven decimal places is about a centimetre; more is noise on the wire.
fn round7(degrees: f64) -> f64 {
    (degrees * 1e7).round() / 1e7
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rmc(valid: bool, sog_kn: Option<f64>) -> Position {
        Position {
            kind: Kind::Rmc,
            valid,
            lat: valid.then_some(37.865),
            lon: valid.then_some(-122.32),
            sog_kn,
            cog_deg: sog_kn.map(|_| 255.0),
            utc: Some("2026-09-13T21:00:00Z".into()),
            satellites: None,
            hdop: None,
        }
    }

    fn gga() -> Position {
        Position {
            kind: Kind::Gga,
            valid: true,
            lat: Some(37.865),
            lon: Some(-122.32),
            sog_kn: None,
            cog_deg: None,
            utc: None,
            satellites: Some(9),
            hdop: Some(0.9),
        }
    }

    #[test]
    fn nothing_heard_is_none() {
        let state = Navigation::default().state(Instant::now());
        assert_eq!(state, FixState::without_position(FixStatus::None));
    }

    #[test]
    fn a_fresh_fix_is_ok_then_stale() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(true, Some(5.0)), t);
        let state = nav.state(t + Duration::from_secs(4));
        assert_eq!(state.status, FixStatus::Ok);
        assert_eq!((state.lat, state.lon), (Some(37.865), Some(-122.32)));
        assert_eq!(state.age_seconds, Some(4));
        let state = nav.state(t + STALE_AFTER);
        assert_eq!(state.status, FixStatus::Stale);
        assert_eq!(
            state.lat,
            Some(37.865),
            "a stale fix still shows where it was"
        );
    }

    #[test]
    fn acquiring_is_nofix_until_the_receiver_goes_quiet() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(false, None), t);
        assert_eq!(nav.state(t).status, FixStatus::Nofix);
        assert_eq!(nav.state(t + STALE_AFTER).status, FixStatus::None);
    }

    #[test]
    fn losing_the_fix_keeps_the_last_position_marked_nofix() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(true, Some(5.0)), t);
        nav.update(rmc(false, None), t + Duration::from_secs(1));
        let state = nav.state(t + Duration::from_secs(2));
        assert_eq!(state.status, FixStatus::Nofix);
        assert_eq!(state.lat, Some(37.865));
        assert_eq!(state.age_seconds, Some(2));
        nav.update(rmc(true, Some(5.0)), t + Duration::from_secs(3));
        assert_eq!(nav.state(t + Duration::from_secs(3)).status, FixStatus::Ok);
    }

    #[test]
    fn rmc_and_gga_fill_in_their_own_fields() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(true, Some(5.0)), t);
        nav.update(gga(), t);
        let state = nav.state(t);
        assert_eq!((state.sog_kn, state.satellites), (Some(5.0), Some(9)));
        // An RMC that stops sending speed clears it rather than keeping 5.0.
        nav.update(rmc(true, None), t);
        let state = nav.state(t);
        assert_eq!((state.sog_kn, state.satellites), (None, Some(9)));
    }
}
