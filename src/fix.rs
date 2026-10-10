//! The boat's fix as omakeel knows it, and how fresh it is.

use crate::nmea::{Kind, Position};
use crate::protocol::{Fix, Place};
use tokio::time::{Duration, Instant};

/// A fix older than this is stale: still shown, never passed off as current.
/// A sentence's own fields go missing once they're this much older than the
/// newest position.
pub const STALE_AFTER: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Navigation {
    stored: Option<Stored>,
    /// When a position sentence of any kind last arrived.
    heard: Option<Instant>,
    /// The receiver is reporting no fix; cleared by the next good one.
    lost: bool,
}

struct Stored {
    lat: f64,
    lon: f64,
    /// When the newest position arrived, from either sentence.
    at: Instant,
    rmc: Option<Rmc>,
    gga: Option<Gga>,
}

struct Rmc {
    at: Instant,
    sog_kn: Option<f64>,
    cog_deg: Option<f64>,
    utc: Option<String>,
}

struct Gga {
    at: Instant,
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
        let fix = self.stored.get_or_insert(Stored {
            lat,
            lon,
            at,
            rmc: None,
            gga: None,
        });
        fix.lat = lat;
        fix.lon = lon;
        fix.at = at;
        match p.kind {
            Kind::Rmc => {
                fix.rmc = Some(Rmc {
                    at,
                    sog_kn: p.sog_kn,
                    cog_deg: p.cog_deg,
                    utc: p.utc,
                })
            }
            Kind::Gga => {
                fix.gga = Some(Gga {
                    at,
                    satellites: p.satellites,
                    hdop: p.hdop,
                })
            }
        }
    }

    pub fn state(&self, now: Instant) -> Fix {
        let recent = |t: Instant| now.saturating_duration_since(t) < STALE_AFTER;
        let talking = self.heard.is_some_and(recent);
        let Some(fix) = &self.stored else {
            return if talking {
                Fix::nofix(None)
            } else {
                Fix::none()
            };
        };
        let age = now.saturating_duration_since(fix.at);
        // A GPS sending only GGA mustn't carry an old RMC's speed along with
        // every fresh position, and the other way round.
        let fresh = |t: Instant| fix.at.saturating_duration_since(t) < STALE_AFTER;
        let rmc = fix.rmc.as_ref().filter(|r| fresh(r.at));
        let gga = fix.gga.as_ref().filter(|g| fresh(g.at));
        let place = Place {
            lat: round7(fix.lat),
            lon: round7(fix.lon),
            sog_kn: rmc.and_then(|r| r.sog_kn),
            cog_deg: rmc.and_then(|r| r.cog_deg),
            utc: rmc.and_then(|r| r.utc.clone()),
            satellites: gga.and_then(|g| g.satellites),
            hdop: gga.and_then(|g| g.hdop),
            age_seconds: age.as_secs(),
        };
        if self.lost && talking {
            Fix::nofix(Some(place))
        } else if age >= STALE_AFTER {
            Fix::stale(place)
        } else {
            Fix::ok(place)
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
    use crate::protocol::FixStatus;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

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
        assert_eq!(state.status(), FixStatus::None);
        assert_eq!(state.position(), None);
    }

    #[test]
    fn a_fresh_fix_is_ok_then_stale() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(true, Some(5.0)), t);
        let state = nav.state(t + secs(4));
        assert_eq!(state.status(), FixStatus::Ok);
        let pos = state.position().expect("a fresh fix has a place");
        assert!(pos.current);
        assert_eq!((pos.place.lat, pos.place.lon), (37.865, -122.32));
        assert_eq!(pos.place.age_seconds, 4);
        let state = nav.state(t + STALE_AFTER);
        assert_eq!(state.status(), FixStatus::Stale);
        let pos = state
            .position()
            .expect("a stale fix still shows where it was");
        assert!(!pos.current);
        assert_eq!(
            pos.place.lat, 37.865,
            "a stale fix still shows where it was"
        );
        assert_eq!(
            pos.place.sog_kn,
            Some(5.0),
            "and its speed, marked stale with it"
        );
    }

    #[test]
    fn acquiring_is_nofix_until_the_receiver_goes_quiet() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(false, None), t);
        assert_eq!(nav.state(t).status(), FixStatus::Nofix);
        assert_eq!(nav.state(t).position(), None);
        assert_eq!(nav.state(t + STALE_AFTER).status(), FixStatus::None);
    }

    #[test]
    fn losing_the_fix_keeps_the_last_position_marked_nofix() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(true, Some(5.0)), t);
        nav.update(rmc(false, None), t + secs(1));
        let state = nav.state(t + secs(2));
        assert_eq!(state.status(), FixStatus::Nofix);
        let pos = state
            .position()
            .expect("lost and talking keeps the last place");
        assert!(!pos.current);
        assert_eq!(pos.place.lat, 37.865);
        assert_eq!(pos.place.age_seconds, 2);
        nav.update(rmc(true, Some(5.0)), t + secs(3));
        assert_eq!(nav.state(t + secs(3)).status(), FixStatus::Ok);
    }

    #[test]
    fn nofix_becomes_stale_once_the_receiver_goes_silent() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(true, Some(5.0)), t);
        nav.update(rmc(false, None), t + secs(1));
        let state = nav.state(t + secs(1) + STALE_AFTER);
        assert_eq!(state.status(), FixStatus::Stale);
        assert_eq!(
            state.position().expect("stale still has a place").place.lat,
            37.865
        );
    }

    #[test]
    fn rmc_and_gga_fill_in_their_own_fields() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(true, Some(5.0)), t);
        nav.update(gga(), t);
        let state = nav.state(t);
        let place = &state.position().expect("a fresh fix has a place").place;
        assert_eq!((place.sog_kn, place.satellites), (Some(5.0), Some(9)));
        // An RMC that stops sending speed clears it rather than keeping 5.0.
        nav.update(rmc(true, None), t);
        let state = nav.state(t);
        let place = &state.position().expect("a fresh fix has a place").place;
        assert_eq!((place.sog_kn, place.satellites), (None, Some(9)));
    }

    #[test]
    fn a_gps_sending_only_gga_drops_the_old_speed() {
        let t = Instant::now();
        let mut nav = Navigation::default();
        nav.update(rmc(true, Some(5.0)), t);
        for s in 1..=6 {
            nav.update(gga(), t + secs(s));
        }
        let state = nav.state(t + secs(6));
        assert_eq!(state.status(), FixStatus::Ok);
        let place = &state.position().expect("a fresh fix has a place").place;
        assert_eq!(place.sog_kn, None, "5 kn from a 6 s old RMC isn't current");
        assert_eq!(place.utc, None);
        assert_eq!(place.satellites, Some(9));
    }
}
