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
    /// The receiver says the fix is usable: RMC status `A`, GGA quality above 0.
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
            valid: f(1) == "A",
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
            valid: f(5).parse::<u8>().is_ok_and(|quality| quality > 0),
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

/// `4807.038`,`N` to 48.1173: whole degrees, then decimal minutes.
fn coord(
    value: &str,
    hemisphere: &str,
    degree_digits: usize,
    (positive, negative): (&str, &str),
    max: f64,
) -> Option<f64> {
    let degrees = value.get(..degree_digits)?;
    let minutes: f64 = value.get(degree_digits..)?.parse().ok()?;
    if !degrees.bytes().all(|b| b.is_ascii_digit()) || !(0.0..60.0).contains(&minutes) {
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
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let time = time.get(..6)?;
    if date.len() != 6 || !digits(date) || !digits(time) {
        return None;
    }
    let n = |s: &str, at: usize| s[at..at + 2].parse::<u32>().ok();
    let (day, month, yy) = (n(date, 0)?, n(date, 2)?, n(date, 4)?);
    let (hour, minute, second) = (n(time, 0)?, n(time, 2)?, n(time, 4)?);
    if !(1..=31).contains(&day) || !(1..=12).contains(&month) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 {
        return None;
    }
    // NMEA sends two-digit years; receivers date from 1980 onward.
    let year = if yy >= 80 { 1900 + yy } else { 2000 + yy };
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
    fn west_and_south_are_negative() {
        let line = sentence("GPRMC,210000,A,3751.900,N,12219.200,W,5.0,255.0,130926,,,A");
        let p = position(&parse(&line).unwrap()).unwrap();
        assert!((p.lat.unwrap() - 37.865).abs() < 1e-9);
        assert!((p.lon.unwrap() + 122.32).abs() < 1e-9);
        assert_eq!(p.utc.as_deref(), Some("2026-09-13T21:00:00Z"));
        let line = sentence("GPGGA,000000,3352.000,S,15112.000,E,1,07,1.1,,M,,M,,");
        assert!(position(&parse(&line).unwrap()).unwrap().lat.unwrap() < 0.0);
    }

    #[test]
    fn impossible_coordinates_are_dropped() {
        for data in [
            "GPRMC,210000,A,9130.000,N,12219.200,W,,,130926,,,A",
            "GPRMC,210000,A,3760.000,N,12219.200,W,,,130926,,,A",
            "GPRMC,210000,A,3751.900,E,12219.200,W,,,130926,,,A",
        ] {
            let p = position(&parse(&sentence(data)).unwrap()).unwrap();
            assert_eq!(p.lat, None, "{data}");
        }
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
