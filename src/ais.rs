//! AIS: what ships radio about themselves, as `!AIVDM` sentences from a
//! receiver like the dAISy. Decodes position reports (types 1, 2, 3, 18 and
//! 19) and particulars (types 5, 19 and 24), per ITU-R M.1371. Own-ship
//! `!AIVDO` is left out: a target list shouldn't include us.

use crate::nmea::Sentence;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Ships: the full transponder, reporting every 2 to 10 seconds under way.
    A,
    /// Small craft: every 30 seconds to 3 minutes.
    B,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub mmsi: u32,
    pub class: Class,
    /// Navigational status 0 to 15, which only class A sends.
    pub status: Option<u8>,
    pub lat: f64,
    pub lon: f64,
    pub sog_kn: Option<f64>,
    pub cog_deg: Option<f64>,
    pub heading_deg: Option<u16>,
}

/// A vessel's particulars. Each message carries only some of them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Particulars {
    pub mmsi: u32,
    pub name: Option<String>,
    pub callsign: Option<String>,
    pub ship_type: Option<u8>,
    pub length_m: Option<u16>,
    pub beam_m: Option<u16>,
    pub destination: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    Position(Position),
    Particulars(Particulars),
}

/// Joins the sentences of a multi-sentence message. One per source, since
/// a receiver sends a message's sentences back to back.
#[derive(Default)]
pub struct Assembler {
    pending: Option<Pending>,
}

struct Pending {
    count: u8,
    next: u8,
    id: String,
    payload: String,
}

impl Assembler {
    /// Takes any sentence; returns what a completed `VDM` message reports.
    pub fn feed(&mut self, s: &Sentence) -> Vec<Report> {
        if s.kind != "VDM" {
            return Vec::new();
        }
        let f = |i: usize| s.fields.get(i).copied().unwrap_or("");
        let (Ok(count), Ok(number)) = (f(0).parse::<u8>(), f(1).parse::<u8>()) else {
            self.pending = None;
            return Vec::new();
        };
        let (id, payload) = (f(2), f(4));
        let fill = f(5).parse::<usize>().unwrap_or(0);
        if count == 0 || number == 0 || number > count || count > 9 {
            self.pending = None;
            return Vec::new();
        }
        if count == 1 {
            self.pending = None;
            return decode(payload, fill);
        }
        if number == 1 {
            self.pending = Some(Pending {
                count,
                next: 2,
                id: id.to_string(),
                payload: payload.to_string(),
            });
            return Vec::new();
        }
        match self.pending.take() {
            Some(mut p) if p.count == count && p.next == number && p.id == id => {
                p.payload.push_str(payload);
                if number == count {
                    decode(&p.payload, fill)
                } else {
                    p.next += 1;
                    self.pending = Some(p);
                    Vec::new()
                }
            }
            // A sentence out of order: the message it belonged to is lost.
            _ => Vec::new(),
        }
    }
}

/// A whole message's payload, less its fill bits, as the reports it holds.
pub fn decode(payload: &str, fill: usize) -> Vec<Report> {
    let Some(bits) = Bits::new(payload, fill) else {
        return Vec::new();
    };
    let Some(mmsi) = bits.uint(8, 30).filter(|&m| m != 0) else {
        return Vec::new();
    };
    let ship_type = |at| bits.uint(at, 8).map(|t| t as u8).filter(|&t| t != 0);
    match bits.uint(0, 6) {
        Some(1..=3) => {
            let status = bits.uint(38, 4).map(|s| s as u8);
            position(&bits, mmsi, Class::A, status, 50)
                .map(Report::Position)
                .into_iter()
                .collect()
        }
        Some(18) => position(&bits, mmsi, Class::B, None, 46)
            .map(Report::Position)
            .into_iter()
            .collect(),
        Some(19) => {
            let (length_m, beam_m) = dimensions(&bits, 271);
            let particulars = Particulars {
                mmsi,
                name: bits.text(143, 20),
                ship_type: ship_type(263),
                length_m,
                beam_m,
                ..Particulars::default()
            };
            position(&bits, mmsi, Class::B, None, 46)
                .map(Report::Position)
                .into_iter()
                .chain([Report::Particulars(particulars)])
                .collect()
        }
        Some(5) => {
            let (length_m, beam_m) = dimensions(&bits, 240);
            vec![Report::Particulars(Particulars {
                mmsi,
                callsign: bits.text(70, 7),
                name: bits.text(112, 20),
                ship_type: ship_type(232),
                length_m,
                beam_m,
                destination: bits.text(302, 20),
            })]
        }
        Some(24) => match bits.uint(38, 2) {
            Some(0) => vec![Report::Particulars(Particulars {
                mmsi,
                name: bits.text(40, 20),
                ..Particulars::default()
            })],
            Some(1) => {
                let (length_m, beam_m) = dimensions(&bits, 132);
                vec![Report::Particulars(Particulars {
                    mmsi,
                    ship_type: ship_type(40),
                    callsign: bits.text(90, 7),
                    length_m,
                    beam_m,
                    ..Particulars::default()
                })]
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Every position report has the same run from speed on: SOG (10 bits),
/// accuracy (1), longitude (28), latitude (27), COG (12), heading (9).
fn position(
    bits: &Bits,
    mmsi: u32,
    class: Class,
    status: Option<u8>,
    sog_at: usize,
) -> Option<Position> {
    let lon = f64::from(bits.int(sog_at + 11, 28)?) / 600_000.0;
    let lat = f64::from(bits.int(sog_at + 39, 27)?) / 600_000.0;
    // 181 and 91 degrees mean not available.
    if lon.abs() > 180.0 || lat.abs() > 90.0 {
        return None;
    }
    Some(Position {
        mmsi,
        class,
        status,
        lat,
        lon,
        sog_kn: bits
            .uint(sog_at, 10)
            .filter(|&v| v < 1023)
            .map(|v| f64::from(v) / 10.0),
        cog_deg: bits
            .uint(sog_at + 66, 12)
            .filter(|&v| v < 3600)
            .map(|v| f64::from(v) / 10.0),
        heading_deg: bits
            .uint(sog_at + 78, 9)
            .filter(|&v| v < 360)
            .map(|v| v as u16),
    })
}

/// Length and beam from the GPS antenna's distances to bow, stern, port and
/// starboard. Zeros mean not available.
fn dimensions(bits: &Bits, at: usize) -> (Option<u16>, Option<u16>) {
    let n = |offset, len| bits.uint(at + offset, len).map(|v| v as u16);
    let sum = |a: Option<u16>, b: Option<u16>| Some(a? + b?).filter(|&v| v > 0);
    (sum(n(0, 9), n(9, 9)), sum(n(18, 6), n(24, 6)))
}

/// A payload's bits, six to a character.
struct Bits(Vec<bool>);

impl Bits {
    fn new(payload: &str, fill: usize) -> Option<Bits> {
        let mut bits = Vec::with_capacity(payload.len() * 6);
        for c in payload.bytes() {
            let v = match c {
                b'0'..=b'W' => c - b'0',
                b'`'..=b'w' => c - b'`' + 40,
                _ => return None,
            };
            bits.extend((0..6).rev().map(|i| (v >> i) & 1 == 1));
        }
        let keep = bits.len().checked_sub(fill.min(5))?;
        bits.truncate(keep);
        Some(Bits(bits))
    }

    fn uint(&self, start: usize, len: usize) -> Option<u32> {
        let bits = self.0.get(start..start + len)?;
        Some(bits.iter().fold(0, |acc, &b| acc << 1 | u32::from(b)))
    }

    fn int(&self, start: usize, len: usize) -> Option<i32> {
        let value = i64::from(self.uint(start, len)?);
        let signed = if value >> (len - 1) & 1 == 1 {
            value - (1 << len)
        } else {
            value
        };
        Some(signed as i32)
    }

    /// Six-bit text: `@` pads the end, and trailing spaces go too.
    fn text(&self, start: usize, chars: usize) -> Option<String> {
        let mut s = String::with_capacity(chars);
        for i in 0..chars {
            let v = self.uint(start + i * 6, 6)? as u8;
            if v == 0 {
                break;
            }
            s.push(char::from(if v < 32 { v + 64 } else { v }));
        }
        let s = s.trim_end();
        (!s.is_empty()).then(|| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nmea::parse;

    /// Feeds sentences through one assembler; returns everything reported.
    fn heard(lines: &[&str]) -> Vec<Report> {
        let mut assembler = Assembler::default();
        lines
            .iter()
            .flat_map(|l| assembler.feed(&parse(l).unwrap()))
            .collect()
    }

    // The two test vectors from gpsd's AIVDM guide, checked against pyais.
    const MOORED: &str = "!AIVDM,1,1,,B,177KQJ5000G?tO`K>RA1wUbN0TKH,0*5C";
    const STATIC_1: &str =
        "!AIVDM,2,1,1,A,55?MbV02;H;s<HtKR20EHE:0@T4@Dn2222222216L961O5Gf0NSQEp6ClRp8,0*1C";
    const STATIC_2: &str = "!AIVDM,2,2,1,A,88888888880,2*25";

    #[test]
    fn decodes_a_class_a_position() {
        let [Report::Position(p)] = &heard(&[MOORED])[..] else {
            panic!("expected one position");
        };
        assert_eq!((p.mmsi, p.class, p.status), (477553000, Class::A, Some(5)));
        assert!((p.lat - 47.582833).abs() < 1e-6, "{}", p.lat);
        assert!((p.lon + 122.345833).abs() < 1e-6, "{}", p.lon);
        assert_eq!(
            (p.sog_kn, p.cog_deg, p.heading_deg),
            (Some(0.0), Some(51.0), Some(181))
        );
    }

    #[test]
    fn joins_a_two_sentence_static_message() {
        let [Report::Particulars(p)] = &heard(&[STATIC_1, STATIC_2])[..] else {
            panic!("expected particulars");
        };
        assert_eq!(p.mmsi, 351759000);
        assert_eq!(p.name.as_deref(), Some("EVER DIADEM"));
        assert_eq!(p.callsign.as_deref(), Some("3FOF8"));
        assert_eq!(p.ship_type, Some(70));
        assert_eq!((p.length_m, p.beam_m), (Some(295), Some(32)));
        assert_eq!(p.destination.as_deref(), Some("NEW YORK"));
    }

    #[test]
    fn a_lost_sentence_loses_its_message_but_not_the_next() {
        assert!(heard(&[STATIC_2]).is_empty(), "a second part alone");
        assert!(
            heard(&[STATIC_1, MOORED, STATIC_2]).len() == 1,
            "only the position"
        );
        assert_eq!(heard(&[STATIC_1, STATIC_1, STATIC_2]).len(), 1, "a restart");
    }

    #[test]
    fn own_ship_and_other_sentences_are_ignored() {
        let own = MOORED.replace("AIVDM", "AIVDO").replace("*5C", "*5E");
        assert!(heard(&[&own]).is_empty());
        let rmc = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        assert!(heard(&[rmc]).is_empty());
    }

    #[test]
    fn garbage_payloads_report_nothing() {
        assert!(decode("!!!!", 0).is_empty(), "not six-bit characters");
        assert!(decode("177KQJ5", 0).is_empty(), "too short for a position");
        assert!(decode("", 0).is_empty());
    }

    #[test]
    fn text_stops_at_padding() {
        // "AB" then an @ pad: 000001 000010 000000.
        let bits = Bits::new("120", 0).unwrap();
        assert_eq!(bits.text(0, 3).as_deref(), Some("AB"));
    }
}
