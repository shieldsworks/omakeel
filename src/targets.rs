//! Every vessel heard on AIS, and how close each will come to the boat.

use crate::ais::{Class, Report};
use crate::protocol::Target;
use std::{cmp::Ordering, collections::HashMap};
use tokio::time::{Duration, Instant};

/// A vessel not heard from in this long is gone. An anchored class A ship
/// reports every 3 minutes, so this is three missed reports.
pub const LOST_AFTER: Duration = Duration::from_secs(10 * 60);
/// Bounds memory on the busiest day any bay will see.
const MAX_VESSELS: usize = 1000;
/// A vessel that will pass within this many miles...
pub const DANGER_CPA_NM: f64 = 0.5;
/// ...within this many minutes is a danger.
pub const DANGER_TCPA_MIN: f64 = 12.0;

/// Where the boat is and how she's moving, from a current fix.
#[derive(Debug, Clone, Copy)]
pub struct Own {
    pub lat: f64,
    pub lon: f64,
    pub sog_kn: Option<f64>,
    pub cog_deg: Option<f64>,
}

#[derive(Default)]
pub struct Traffic {
    vessels: HashMap<u32, Vessel>,
}

struct Vessel {
    heard: Instant,
    class: Option<Class>,
    position: Option<(crate::ais::Position, Instant)>,
    name: Option<String>,
    callsign: Option<String>,
    ship_type: Option<u8>,
    length_m: Option<u16>,
    beam_m: Option<u16>,
    destination: Option<String>,
}

impl Traffic {
    pub fn update(&mut self, report: Report, at: Instant) {
        let mmsi = match &report {
            Report::Position(p) => p.mmsi,
            Report::Particulars(p) => p.mmsi,
        };
        if !self.vessels.contains_key(&mmsi) {
            self.expire(at);
            // Full: a newcomer with a position might be the one on a collision
            // course, so it replaces the vessel least worth keeping, first one
            // with no position, then the one heard longest ago. A newcomer
            // with only particulars never displaces a vessel with a position.
            if self.vessels.len() >= MAX_VESSELS
                && let Some((&victim, positioned)) = self
                    .vessels
                    .iter()
                    .min_by_key(|(_, v)| (v.position.is_some(), v.heard))
                    .map(|(mmsi, v)| (mmsi, v.position.is_some()))
            {
                if positioned && !matches!(report, Report::Position(_)) {
                    return;
                }
                self.vessels.remove(&victim);
            }
        }
        let vessel = self.vessels.entry(mmsi).or_insert_with(|| Vessel {
            heard: at,
            class: None,
            position: None,
            name: None,
            callsign: None,
            ship_type: None,
            length_m: None,
            beam_m: None,
            destination: None,
        });
        vessel.heard = at;
        match report {
            Report::Position(p) => {
                vessel.class = Some(p.class);
                vessel.position = Some((p, at));
            }
            // Each message carries some particulars (a type 24 part A is
            // only the name), so take what came and keep the rest.
            Report::Particulars(p) => {
                vessel.name = p.name.or(vessel.name.take());
                vessel.callsign = p.callsign.or(vessel.callsign.take());
                vessel.ship_type = p.ship_type.or(vessel.ship_type);
                vessel.length_m = p.length_m.or(vessel.length_m);
                vessel.beam_m = p.beam_m.or(vessel.beam_m);
                vessel.destination = p.destination.or(vessel.destination.take());
            }
        }
    }

    /// Drops vessels not heard for `LOST_AFTER`, and positions that old even
    /// from vessels still sending their particulars.
    pub fn expire(&mut self, now: Instant) {
        let lost = |t: Instant| now.saturating_duration_since(t) >= LOST_AFTER;
        self.vessels.retain(|_, v| !lost(v.heard));
        for vessel in self.vessels.values_mut() {
            if vessel.position.as_ref().is_some_and(|(_, at)| lost(*at)) {
                vessel.position = None;
            }
        }
    }

    /// Every vessel, nearest first; any the boat can't range come last.
    pub fn targets(&self, now: Instant, own: Option<Own>) -> Vec<Target> {
        let mut targets: Vec<Target> = self
            .vessels
            .iter()
            .map(|(&mmsi, vessel)| vessel.target(mmsi, now, own))
            .collect();
        targets.sort_by(|a, b| {
            match (a.range_nm, b.range_nm) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
            .then(a.mmsi.cmp(&b.mmsi))
        });
        targets
    }
}

impl Vessel {
    fn target(&self, mmsi: u32, now: Instant, own: Option<Own>) -> Target {
        let mut target = Target {
            mmsi,
            name: self.name.clone(),
            callsign: self.callsign.clone(),
            ship_type: self.ship_type,
            kind: self.ship_type.and_then(kind),
            class: self.class.map(|c| match c {
                Class::A => "A",
                Class::B => "B",
            }),
            status: None,
            lat: None,
            lon: None,
            sog_kn: None,
            cog_deg: None,
            heading_deg: None,
            length_m: self.length_m,
            beam_m: self.beam_m,
            destination: self.destination.clone(),
            age_seconds: None,
            range_nm: None,
            bearing_deg: None,
            cpa_nm: None,
            tcpa_minutes: None,
            danger: false,
        };
        let Some((p, at)) = self
            .position
            .as_ref()
            .filter(|(_, at)| now.saturating_duration_since(*at) < LOST_AFTER)
        else {
            return target;
        };
        let age = now.saturating_duration_since(*at);
        target.status = p.status.and_then(status);
        target.lat = Some(round(p.lat, 7));
        target.lon = Some(round(p.lon, 7));
        target.sog_kn = p.sog_kn;
        target.cog_deg = p.cog_deg;
        target.heading_deg = p.heading_deg;
        target.age_seconds = Some(age.as_secs());
        let Some(own) = own else {
            return target;
        };
        // Carried forward along its reported course and speed to now, the
        // way a chartplotter shows a target between reports.
        let moving = velocity(p.sog_kn, p.cog_deg);
        let hours = age.as_secs_f64() / 3600.0;
        let (east, north) = offset(own.lat, own.lon, p.lat, p.lon);
        let (east, north) = match moving {
            Some((ve, vn)) => (east + ve * hours, north + vn * hours),
            None => (east, north),
        };
        target.range_nm = Some(round(east.hypot(north), 2));
        let bearing = round(east.atan2(north).to_degrees().rem_euclid(360.0), 1);
        target.bearing_deg = Some(if bearing >= 360.0 { 0.0 } else { bearing });
        if let (Some(theirs), Some(ours)) = (moving, velocity(own.sog_kn, own.cog_deg)) {
            let (cpa, tcpa) = closest((east, north), (theirs.0 - ours.0, theirs.1 - ours.1));
            target.cpa_nm = Some(round(cpa, 2));
            target.tcpa_minutes = Some(round(tcpa, 1));
            target.danger = cpa <= DANGER_CPA_NM && tcpa <= DANGER_TCPA_MIN;
        }
        target
    }
}

/// Knots east and north. A vessel barely moving may send no course; it's
/// taken as stopped. Otherwise no course means no velocity.
fn velocity(sog_kn: Option<f64>, cog_deg: Option<f64>) -> Option<(f64, f64)> {
    match (sog_kn?, cog_deg) {
        (sog, Some(cog)) => {
            let cog = cog.to_radians();
            Some((sog * cog.sin(), sog * cog.cos()))
        }
        (sog, None) if sog < 0.5 => Some((0.0, 0.0)),
        _ => None,
    }
}

/// Miles east and north from one position to another. Flat-earth, which
/// is off by a small fraction of a mile at the edge of AIS range.
fn offset(from_lat: f64, from_lon: f64, to_lat: f64, to_lon: f64) -> (f64, f64) {
    let dlon = (to_lon - from_lon + 540.0).rem_euclid(360.0) - 180.0;
    let mid = ((from_lat + to_lat) / 2.0).to_radians();
    (dlon * 60.0 * mid.cos(), (to_lat - from_lat) * 60.0)
}

/// Closest point of approach in miles, and minutes until it, for a vessel
/// at `at` moving at `relative` knots against the boat. A vessel already
/// past its closest is closest now.
fn closest(at: (f64, f64), relative: (f64, f64)) -> (f64, f64) {
    let speed2 = relative.0 * relative.0 + relative.1 * relative.1;
    let hours = if speed2 < 1e-9 {
        0.0
    } else {
        (-(at.0 * relative.0 + at.1 * relative.1) / speed2).max(0.0)
    };
    let cpa = (at.0 + relative.0 * hours).hypot(at.1 + relative.1 * hours);
    (cpa, hours * 60.0)
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// The ITU ship type codes, in words.
fn kind(code: u8) -> Option<&'static str> {
    Some(match code {
        20..=29 => "wing in ground",
        30 => "fishing",
        31 | 32 => "towing",
        33 => "dredging",
        34 => "diving",
        35 => "military",
        36 => "sailing",
        37 => "pleasure craft",
        40..=49 => "high speed craft",
        50 => "pilot",
        51 => "search and rescue",
        52 => "tug",
        53 => "port tender",
        55 => "law enforcement",
        58 => "medical transport",
        60..=69 => "passenger",
        70..=79 => "cargo",
        80..=89 => "tanker",
        90..=99 => "other",
        _ => return None,
    })
}

/// Class A navigational status, in words. 15 is "not defined".
fn status(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "under way using engine",
        1 => "at anchor",
        2 => "not under command",
        3 => "restricted maneuverability",
        4 => "constrained by draught",
        5 => "moored",
        6 => "aground",
        7 => "engaged in fishing",
        8 => "under way sailing",
        14 => "AIS-SART active",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::{Particulars, Position};

    const LAT: f64 = 37.865;
    const LON: f64 = -122.32;

    /// A position `north` and `east` miles from the boat.
    fn at(north: f64, east: f64) -> (f64, f64) {
        (
            LAT + north / 60.0,
            LON + east / (60.0 * LAT.to_radians().cos()),
        )
    }

    fn report(mmsi: u32, (lat, lon): (f64, f64), sog: f64, cog: f64) -> Report {
        Report::Position(Position {
            mmsi,
            class: Class::A,
            status: Some(0),
            lat,
            lon,
            sog_kn: Some(sog),
            cog_deg: Some(cog),
            heading_deg: None,
        })
    }

    fn still() -> Option<Own> {
        Some(Own {
            lat: LAT,
            lon: LON,
            sog_kn: Some(0.0),
            cog_deg: None,
        })
    }

    #[test]
    fn a_vessel_heading_straight_for_us_is_a_danger() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(report(1, at(1.0, 0.0), 6.0, 180.0), t);
        let [target] = &traffic.targets(t, still())[..] else {
            panic!()
        };
        assert_eq!(target.range_nm, Some(1.0));
        assert_eq!(target.bearing_deg, Some(0.0));
        assert_eq!(
            (target.cpa_nm, target.tcpa_minutes),
            (Some(0.0), Some(10.0))
        );
        assert!(target.danger);
        assert_eq!(target.status, Some("under way using engine"));
    }

    #[test]
    fn a_vessel_passing_close_abeam_is_a_danger_and_one_far_off_is_not() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(report(1, at(1.0, 0.3), 6.0, 180.0), t);
        traffic.update(report(2, at(1.0, 0.8), 6.0, 180.0), t);
        let targets = traffic.targets(t, still());
        assert_eq!(
            (targets[0].cpa_nm, targets[0].tcpa_minutes),
            (Some(0.3), Some(10.0))
        );
        assert!(targets[0].danger);
        assert_eq!(targets[1].cpa_nm, Some(0.8));
        assert!(!targets[1].danger);
    }

    #[test]
    fn a_vessel_heading_away_is_closest_now() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(report(1, at(1.0, 0.0), 6.0, 0.0), t);
        let target = &traffic.targets(t, still())[0];
        assert_eq!((target.cpa_nm, target.tcpa_minutes), (Some(1.0), Some(0.0)));
        assert!(!target.danger);
    }

    #[test]
    fn a_danger_too_far_ahead_in_time_is_not_one_yet() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        // Dead ahead, but 3 miles away at 6 knots: 30 minutes out.
        traffic.update(report(1, at(3.0, 0.0), 6.0, 180.0), t);
        let target = &traffic.targets(t, still())[0];
        assert_eq!(target.tcpa_minutes, Some(30.0));
        assert!(!target.danger);
    }

    #[test]
    fn an_old_report_is_carried_forward() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(report(1, at(1.0, 0.0), 10.0, 180.0), t);
        // Six minutes at 10 knots is the whole mile.
        let target = &traffic.targets(t + Duration::from_secs(360), still())[0];
        assert_eq!(target.range_nm, Some(0.0));
        assert_eq!(target.age_seconds, Some(360));
        assert_eq!(
            target.lat,
            Some(round(at(1.0, 0.0).0, 7)),
            "reported, not moved"
        );
    }

    #[test]
    fn without_a_fix_there_is_no_range_and_without_our_speed_no_cpa() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(report(1, at(1.0, 0.0), 6.0, 180.0), t);
        let target = &traffic.targets(t, None)[0];
        assert_eq!((target.range_nm, target.cpa_nm), (None, None));
        assert!(target.lat.is_some());
        let moving_unknown = Some(Own {
            sog_kn: Some(5.0),
            cog_deg: None,
            ..still().unwrap()
        });
        let target = &traffic.targets(t, moving_unknown)[0];
        assert_eq!((target.range_nm, target.cpa_nm), (Some(1.0), None));
        assert!(!target.danger);
    }

    #[test]
    fn particulars_fill_in_across_messages() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(
            Report::Particulars(Particulars {
                mmsi: 7,
                name: Some("SEA LARK".into()),
                ..Particulars::default()
            }),
            t,
        );
        traffic.update(
            Report::Particulars(Particulars {
                mmsi: 7,
                callsign: Some("WDY9103".into()),
                ship_type: Some(36),
                ..Particulars::default()
            }),
            t,
        );
        let target = &traffic.targets(t, still())[0];
        assert_eq!(target.name.as_deref(), Some("SEA LARK"));
        assert_eq!(target.callsign.as_deref(), Some("WDY9103"));
        assert_eq!(target.kind, Some("sailing"));
        assert_eq!(target.range_nm, None, "no position yet");
    }

    #[test]
    fn nearest_first_and_the_unranged_last() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(report(3, at(3.0, 0.0), 0.0, 0.0), t);
        traffic.update(
            Report::Particulars(Particulars {
                mmsi: 1,
                ..Particulars::default()
            }),
            t,
        );
        traffic.update(report(2, at(0.0, -2.0), 0.0, 0.0), t);
        let order: Vec<u32> = traffic.targets(t, still()).iter().map(|t| t.mmsi).collect();
        assert_eq!(order, [2, 3, 1]);
    }

    #[test]
    fn a_position_ten_minutes_old_is_dropped_though_its_name_still_comes() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(report(1, at(1.0, 0.0), 10.0, 180.0), t);
        let named = |mmsi| {
            Report::Particulars(Particulars {
                mmsi,
                name: Some("QUIET ONE".into()),
                ..Particulars::default()
            })
        };
        traffic.update(named(1), t + Duration::from_secs(540));
        let now = t + LOST_AFTER + Duration::from_secs(60);
        traffic.update(named(1), now);
        let target = &traffic.targets(now, still())[0];
        assert_eq!(target.name.as_deref(), Some("QUIET ONE"));
        assert_eq!(
            (target.lat, target.range_nm, target.cpa_nm),
            (None, None, None)
        );
        assert!(!target.danger, "not carried forward for eleven minutes");
        traffic.expire(now);
        assert_eq!(traffic.targets(now, still())[0].lat, None);
    }

    #[test]
    fn a_full_table_makes_room_for_a_vessel_with_a_position() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        for mmsi in 1..=MAX_VESSELS as u32 {
            traffic.update(
                Report::Particulars(Particulars {
                    mmsi,
                    ..Particulars::default()
                }),
                t,
            );
        }
        traffic.update(report(5000, at(1.0, 0.0), 6.0, 180.0), t);
        let targets = traffic.targets(t, still());
        assert_eq!(targets.len(), MAX_VESSELS);
        assert_eq!(targets[0].mmsi, 5000, "the newcomer, nearest and first");
        assert!(targets[0].danger);
    }

    #[test]
    fn a_full_table_keeps_its_vessels_with_positions_against_a_name() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        for mmsi in 1..=MAX_VESSELS as u32 {
            traffic.update(report(mmsi, at(1.0, 0.0), 6.0, 180.0), t);
        }
        traffic.update(
            Report::Particulars(Particulars {
                mmsi: 5000,
                name: Some("NEWCOMER".into()),
                ..Particulars::default()
            }),
            t + Duration::from_secs(1),
        );
        let targets = traffic.targets(t, still());
        assert_eq!(targets.len(), MAX_VESSELS);
        assert!(targets.iter().all(|t| t.mmsi != 5000));
        assert!(
            targets.iter().all(|t| t.danger),
            "no danger displaced by a name"
        );
    }

    #[test]
    fn vessels_not_heard_for_ten_minutes_are_gone() {
        let t = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(report(1, at(1.0, 0.0), 0.0, 0.0), t);
        traffic.update(
            report(2, at(2.0, 0.0), 0.0, 0.0),
            t + Duration::from_secs(300),
        );
        traffic.expire(t + LOST_AFTER);
        let left: Vec<u32> = traffic
            .targets(t + LOST_AFTER, None)
            .iter()
            .map(|t| t.mmsi)
            .collect();
        assert_eq!(left, [2]);
    }
}
