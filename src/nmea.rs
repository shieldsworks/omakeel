//! NMEA 0183: sentence framing and checksums, and the two position
//! sentences omakeel reads, RMC and GGA. Everything else, AIS included, is
//! checked and counted but not decoded yet.

/// A sentence whose checksum matched, split into its talker (`GP`), kind
/// (`RMC`) and the comma-separated fields after the address.
#[derive(Debug, PartialEq)]
pub struct Sentence<'a> {
    pub talker: &'a str,
    pub kind: &'a str,
    pub fields: Vec<&'a str>,
}

/// Why a line was rejected.
#[derive(Debug, PartialEq)]
pub enum Reject {
    /// Not shaped like a sentence: no leading `$` or `!`, no `*hh`, or no address.
    Framing,
    /// Shaped like one, but the data doesn't match its checksum.
    Checksum { computed: u8, sent: u8 },
}

pub fn parse(line: &str) -> Result<Sentence<'_>, Reject> {
    let body = line
        .trim_end()
        .strip_prefix(['$', '!'])
        .ok_or(Reject::Framing)?;
    let (data, sum) = body.rsplit_once('*').ok_or(Reject::Framing)?;
    if sum.len() != 2 || !sum.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Reject::Framing);
    }
    let sent = u8::from_str_radix(sum, 16).map_err(|_| Reject::Framing)?;
    let computed = data.bytes().fold(0, |acc, b| acc ^ b);
    if computed != sent {
        return Err(Reject::Checksum { computed, sent });
    }
    let mut fields = data.split(',');
    let address = fields.next().unwrap_or_default();
    if address.len() < 3 || !address.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(Reject::Framing);
    }
    let (talker, kind) = address.split_at(2);
    Ok(Sentence {
        talker,
        kind,
        fields: fields.collect(),
    })
}

/// Which sentence a position came from; each carries different fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Rmc,
    Gga,
}

/// A position sentence. `None` where the receiver left a field empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub kind: Kind,
    /// A real, usable fix: RMC status `A` in a real mode, or GGA quality 1-5.
    pub valid: bool,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Speed and course over ground, from RMC.
    pub sog_kn: Option<f64>,
    pub cog_deg: Option<f64>,
    /// From RMC, the only one with the date: `1994-03-23T12:35:19Z`.
    pub utc: Option<String>,
    /// From GGA.
    pub satellites: Option<u8>,
    pub hdop: Option<f64>,
}

/// The position in an RMC or GGA sentence; `None` for any other kind.
pub fn position(s: &Sentence) -> Option<Position> {
    let f = |i: usize| s.fields.get(i).copied().unwrap_or("");
    match s.kind {
        "RMC" => Some(Position {
            kind: Kind::Rmc,
            // NMEA 2.3 added a mode: estimated (dead reckoning), manual,
            // simulated and not valid are no fix, whatever the status says.
            valid: f(1) == "A" && !matches!(f(11), "E" | "M" | "S" | "N"),
            lat: coord(f(2), f(3), 2, ("N", "S"), 90.0),
            lon: coord(f(4), f(5), 3, ("E", "W"), 180.0),
            sog_kn: number(f(6)),
            cog_deg: number(f(7)),
            utc: utc(f(8), f(0)),
            satellites: None,
            hdop: None,
        }),
        "GGA" => Some(Position {
            kind: Kind::Gga,
            // 1 GPS, 2 DGPS, 3 PPS, 4 RTK, 5 float RTK. 6 estimated, 7 manual
            // and 8 simulation are no fix.
            valid: matches!(f(5), "1" | "2" | "3" | "4" | "5"),
            lat: coord(f(1), f(2), 2, ("N", "S"), 90.0),
            lon: coord(f(3), f(4), 3, ("E", "W"), 180.0),
            sog_kn: None,
            cog_deg: None,
            utc: None,
            satellites: f(6).parse().ok(),
            hdop: number(f(7)),
        }),
        _ => None,
    }
}

fn number(field: &str) -> Option<f64> {
    field.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Nothing, or a decimal point and digits: the `.038` in `4807.038`.
fn fraction(s: &str) -> bool {
    s.is_empty() || s.strip_prefix('.').is_some_and(digits)
}

/// `4807.038`,`N` to 48.1173: whole degrees, then two whole-minute digits
/// and their decimals. The checksum can't catch a moved decimal point (XOR
/// ignores order), so the shape is checked here.
fn coord(
    value: &str,
    hemisphere: &str,
    degree_digits: usize,
    (positive, negative): (&str, &str),
    max: f64,
) -> Option<f64> {
    let (degrees, minutes) = value.split_at_checked(degree_digits)?;
    let (whole, decimals) = minutes.split_at_checked(2)?;
    if !digits(degrees) || !digits(whole) || !fraction(decimals) {
        return None;
    }
    let minutes: f64 = minutes.parse().ok()?;
    if minutes >= 60.0 {
        return None;
    }
    let degrees = degrees.parse::<f64>().ok()? + minutes / 60.0;
    if degrees > max {
        return None;
    }
    if hemisphere == positive {
        Some(degrees)
    } else if hemisphere == negative {
        Some(-degrees)
    } else {
        None
    }
}

/// RMC's `ddmmyy` and `hhmmss.ss` as an ISO 8601 UTC time, whole seconds.
fn utc(date: &str, time: &str) -> Option<String> {
    let (clock, decimals) = time.split_at_checked(6)?;
    if date.len() != 6 || !digits(date) || !digits(clock) || !fraction(decimals) {
        return None;
    }
    let n = |s: &str, at: usize| s[at..at + 2].parse::<u32>().ok();
    let (day, month, yy) = (n(date, 0)?, n(date, 2)?, n(date, 4)?);
    let (hour, minute, second) = (n(clock, 0)?, n(clock, 2)?, n(clock, 4)?);
    // NMEA sends two-digit years; receivers date from 1980 onward.
    let year = if yy >= 80 { 1900 + yy } else { 2000 + yy };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > days || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RMC: &str = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
    const GGA: &str = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
    const NO_FIX: &str = "$GPRMC,,V,,,,,,,,,,N*53";
    const AIS: &str = "!AIVDM,1,1,,B,177KQJ5000G?tO`K>RA1wUbN0TKH,0*5C";

    /// A sentence with its checksum computed, for data the tests invent.
    fn sentence(data: &str) -> String {
        format!("${data}*{:02X}", data.bytes().fold(0, |acc, b| acc ^ b))
    }

    fn of(data: &str) -> Position {
        position(&parse(&sentence(data)).unwrap()).unwrap()
    }

    #[test]
    fn reads_a_fix_from_rmc() {
        let p = position(&parse(RMC).unwrap()).unwrap();
        assert_eq!(p.kind, Kind::Rmc);
        assert!(p.valid);
        assert!((p.lat.unwrap() - 48.1173).abs() < 1e-9);
        assert!((p.lon.unwrap() - (11.0 + 31.0 / 60.0)).abs() < 1e-9);
        assert_eq!(p.sog_kn, Some(22.4));
        assert_eq!(p.cog_deg, Some(84.4));
        assert_eq!(p.utc.as_deref(), Some("1994-03-23T12:35:19Z"));
    }

    #[test]
    fn reads_quality_from_gga() {
        let p = position(&parse(GGA).unwrap()).unwrap();
        assert_eq!(p.kind, Kind::Gga);
        assert!(p.valid);
        assert_eq!(p.satellites, Some(8));
        assert_eq!(p.hdop, Some(0.9));
        assert_eq!(p.utc, None);
    }

    #[test]
    fn a_receiver_without_a_fix_says_so() {
        let p = position(&parse(NO_FIX).unwrap()).unwrap();
        assert!(!p.valid);
        assert_eq!((p.lat, p.lon, p.utc), (None, None, None));
    }

    #[test]
    fn simulated_estimated_and_manual_fixes_are_not_fixes() {
        for quality in ["0", "6", "7", "8"] {
            let p = of(&format!(
                "GPGGA,210000,3751.900,N,12219.200,W,{quality},09,0.9,,M,,M,,"
            ));
            assert!(!p.valid, "GGA quality {quality}");
        }
        for mode in ["E", "M", "S", "N"] {
            let p = of(&format!(
                "GPRMC,210000,A,3751.900,N,12219.200,W,5.0,255.0,130926,,,{mode}"
            ));
            assert!(!p.valid, "RMC mode {mode}");
        }
        assert!(of("GPRMC,210000,A,3751.900,N,12219.200,W,5.0,255.0,130926,,,D").valid);
        assert!(of("GPRMC,210000,A,3751.900,N,12219.200,W,5.0,255.0,130926,,").valid);
    }

    #[test]
    fn west_and_south_are_negative() {
        let p = of("GPRMC,210000,A,3751.900,N,12219.200,W,5.0,255.0,130926,,,A");
        assert!((p.lat.unwrap() - 37.865).abs() < 1e-9);
        assert!((p.lon.unwrap() + 122.32).abs() < 1e-9);
        assert_eq!(p.utc.as_deref(), Some("2026-09-13T21:00:00Z"));
        let p = of("GPGGA,000000,3352.000,S,15112.000,E,1,07,1.1,,M,,M,,");
        assert!(p.lat.unwrap() < 0.0);
    }

    #[test]
    fn impossible_coordinates_are_dropped() {
        for data in [
            "GPRMC,210000,A,9130.000,N,12219.200,W,,,130926,,,A",
            "GPRMC,210000,A,3760.000,N,12219.200,W,,,130926,,,A",
            "GPRMC,210000,A,3751.900,E,12219.200,W,,,130926,,,A",
            // The same characters as 3751.900 with the point moved: the
            // checksum still matches, and it would put the boat 47 nm south.
            "GPRMC,210000,A,375.1900,N,12219.200,W,,,130926,,,A",
            "GPRMC,210000,A,3751.9x0,N,12219.200,W,,,130926,,,A",
        ] {
            assert_eq!(of(data).lat, None, "{data}");
        }
    }

    #[test]
    fn impossible_dates_and_times_are_dropped() {
        for (date, time) in [
            ("310226", "120000"),
            ("290225", "120000"),
            ("130926", "120000garbage"),
            ("130926", "250000"),
        ] {
            let p = of(&format!(
                "GPRMC,{time},A,3751.900,N,12219.200,W,,,{date},,,A"
            ));
            assert_eq!(p.utc, None, "{date} {time}");
        }
        let leap_day = of("GPRMC,120000.50,A,3751.900,N,12219.200,W,,,290224,,,A");
        assert_eq!(leap_day.utc.as_deref(), Some("2024-02-29T12:00:00Z"));
    }

    #[test]
    fn rejects_a_bad_checksum() {
        let line = RMC.replace("*6A", "*6B");
        assert_eq!(
            parse(&line),
            Err(Reject::Checksum {
                computed: 0x6A,
                sent: 0x6B
            })
        );
    }

    #[test]
    fn rejects_what_is_not_a_sentence() {
        for line in ["", "hello", "$GPRMC,1,2", "$GPRMC*6", "$GPRMC*6G", "$*00"] {
            assert_eq!(parse(line), Err(Reject::Framing), "{line:?}");
        }
    }

    #[test]
    fn ais_is_checked_but_not_a_position() {
        let s = parse(AIS).unwrap();
        assert_eq!((s.talker, s.kind), ("AI", "VDM"));
        assert_eq!(position(&s), None);
    }

    #[test]
    fn tolerates_a_trailing_carriage_return() {
        assert!(parse(&format!("{RMC}\r\n")).is_ok());
    }
}
