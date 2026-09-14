//! AIS: what ships radio about themselves, as `!AIVDM` sentences from a
//! receiver like the dAISy. Decodes position reports (types 1, 2, 3, 18 and
//! 19) and particulars (types 5, 19 and 24), per ITU-R M.1371. Own-ship
//! `!AIVDO` is left out: a target list shouldn't include us.

use crate::nmea::Sentence;

/// Multi-sentence messages waiting for their other sentences. A few at
/// once, since messages on channels A and B interleave.
const PENDING: usize = 4;

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

/// Joins the sentences of multi-sentence messages. One per source.
#[derive(Default)]
pub struct Assembler {
    pending: Vec<Pending>,
}

struct Pending {
    count: u8,
    next: u8,
    id: String,
    channel: String,
    payload: String,
}

impl Assembler {
    /// Takes any sentence; returns what a completed `VDM` message reports.
    pub fn feed(&mut self, s: &Sentence) -> Vec<Report> {
        if s.kind != "VDM" {
            return Vec::new();
        }
        let f = |i: usize| s.fields.get(i).copied().unwrap_or("");
        let (Ok(count), Ok(number), Ok(fill)) = (
            f(0).parse::<u8>(),
            f(1).parse::<u8>(),
            f(5).parse::<usize>(),
        ) else {
            return Vec::new();
        };
        let (id, channel, payload) = (f(2), f(3), f(4));
        if count == 0 || number == 0 || number > count || count > 9 {
            return Vec::new();
        }
        if count == 1 {
            return decode(payload, fill);
        }
        // A message's sentences share a sequence id, a channel and a count.
        let same = |p: &Pending| p.id == id && p.channel == channel && p.count == count;
        if number == 1 {
            self.pending.retain(|p| !same(p));
            if self.pending.len() == PENDING {
                self.pending.remove(0);
            }
            self.pending.push(Pending {
                count,
                next: 2,
                id: id.to_string(),
                channel: channel.to_string(),
                payload: payload.to_string(),
            });
            return Vec::new();
        }
        let Some(i) = self
            .pending
            .iter()
            .position(|p| same(p) && p.next == number)
        else {
            // Out of order: the message it belonged to is lost.
            self.pending.retain(|p| !same(p));
            return Vec::new();
        };
        self.pending[i].payload.push_str(payload);
        if number == count {
            let whole = self.pending.remove(i);
            decode(&whole.payload, fill)
        } else {
            self.pending[i].next += 1;
            Vec::new()
        }
    }
}

/// A whole message's payload, less its fill bits, as the reports it holds.
/// A message shorter than its type's full length is dropped rather than
/// read with missing fields.
pub fn decode(payload: &str, fill: usize) -> Vec<Report> {
    let Some(bits) = Bits::new(payload, fill) else {
        return Vec::new();
    };
    let Some(mmsi) = bits.uint(8, 30).filter(|&m| m != 0) else {
        return Vec::new();
    };
    let long = |n: usize| bits.len() >= n;
    let ship_type = |at| bits.uint(at, 8).map(|t| t as u8).filter(|&t| t != 0);
    match bits.uint(0, 6) {
        Some(1..=3) if long(168) => {
            let status = bits.uint(38, 4).map(|s| s as u8);
            position(&bits, mmsi, Class::A, status, 50)
                .map(Report::Position)
                .into_iter()
                .collect()
        }
        Some(18) if long(168) => position(&bits, mmsi, Class::B, None, 46)
            .map(Report::Position)
            .into_iter()
            .collect(),
        Some(19) if long(312) => {
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
        // 424 bits in full; some receivers drop the two spare bits at the end.
        Some(5) if long(422) => {
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
            Some(0) if long(160) => vec![Report::Particulars(Particulars {
                mmsi,
                name: bits.text(40, 20),
                ..Particulars::default()
            })],
            Some(1) if long(162) => {
                // An auxiliary craft (MMSI 98...) sends its mother ship's
                // MMSI where others send their size.
                let auxiliary = (980_000_000..990_000_000).contains(&mmsi);
                let (length_m, beam_m) = if auxiliary {
                    (None, None)
                } else {
                    dimensions(&bits, 132)
                };
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
        // Fill pads the last character, so it runs 0 to 5.
        if fill > 5 {
            return None;
        }
        let mut bits = Vec::with_capacity(payload.len() * 6);
        for c in payload.bytes() {
            let v = match c {
                b'0'..=b'W' => c - b'0',
                b'`'..=b'w' => c - b'`' + 40,
                _ => return None,
            };
            bits.extend((0..6).rev().map(|i| (v >> i) & 1 == 1));
        }
        let keep = bits.len().checked_sub(fill)?;
        bits.truncate(keep);
        Some(Bits(bits))
    }

    fn len(&self) -> usize {
        self.0.len()
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

    /// Recomputes a sentence's checksum after a test edits its data.
    fn resum(line: &str) -> String {
        let body = &line[1..line.rfind('*').unwrap()];
        let sum = body.bytes().fold(0, |acc, b| acc ^ b);
        format!("{}{body}*{sum:02X}", &line[..1])
    }

    /// The type 5 below, sent on another channel under another sequence id.
    fn static_on(channel: &str, id: &str) -> [String; 2] {
        [STATIC_1, STATIC_2].map(|l| resum(&l.replace(",1,A,", &format!(",{id},{channel},"))))
    }

    /// Packs (value, bits) fields into a payload and its fill bits.
    fn payload(fields: &[(u64, usize)]) -> (String, usize) {
        let mut bits: Vec<bool> = fields
            .iter()
            .flat_map(|&(v, n)| (0..n).rev().map(move |i| (v >> i) & 1 == 1))
            .collect();
        let fill = (6 - bits.len() % 6) % 6;
        bits.extend(std::iter::repeat_n(false, fill));
        let chars = bits
            .chunks(6)
            .map(|c| {
                let v = c.iter().fold(0u8, |acc, &b| acc << 1 | u8::from(b));
                char::from(if v < 40 { v + 48 } else { v + 56 })
            })
            .collect();
        (chars, fill)
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
        assert_eq!(heard(&[STATIC_1, STATIC_1, STATIC_2]).len(), 1, "a restart");
        assert_eq!(
            heard(&[STATIC_1, MOORED, STATIC_2]).len(),
            2,
            "a position between the parts doesn't break the message"
        );
    }

    #[test]
    fn messages_on_both_channels_interleave() {
        let [a1, a2] = static_on("A", "1");
        let [b1, b2] = static_on("B", "2");
        let reports = heard(&[&a1, &b1, MOORED, &a2, &b2]);
        assert_eq!(reports.len(), 3, "both type 5s and the position between");
    }

    #[test]
    fn sentences_from_different_channels_never_mix() {
        let [a1, _] = static_on("A", "1");
        let [_, b2] = static_on("B", "1");
        assert!(heard(&[&a1, &b2]).is_empty());
    }

    #[test]
    fn a_message_shorter_than_its_type_is_dropped() {
        assert!(
            decode("177KQJ5000G?tO`K>RA1", 0).is_empty(),
            "120 bits of a 168-bit position"
        );
        assert!(
            decode("177KQJ5000G?tO`K>RA1wUbN0TKH", 6).is_empty(),
            "fill runs 0 to 5"
        );
        assert_eq!(decode("177KQJ5000G?tO`K>RA1wUbN0TKH", 0).len(), 1);
    }

    #[test]
    fn an_auxiliary_craft_has_no_size() {
        // A type 24 part B. From a tender (MMSI 98...), the size field holds
        // its mother ship's MMSI.
        let part_b = |mmsi| {
            payload(&[
                (24, 6),
                (0, 2),
                (mmsi, 30),
                (1, 2),
                (36, 8),
                (0, 42),
                (0, 42),
                (366_999_101, 30),
                (0, 6),
            ])
        };
        let (tender, fill) = part_b(983_669_991);
        let [Report::Particulars(p)] = &decode(&tender, fill)[..] else {
            panic!("expected particulars");
        };
        assert_eq!((p.length_m, p.beam_m, p.ship_type), (None, None, Some(36)));
        let (ordinary, fill) = part_b(338_999_103);
        let [Report::Particulars(p)] = &decode(&ordinary, fill)[..] else {
            panic!("expected particulars");
        };
        assert!(p.length_m.is_some(), "the same bits are a size for others");
    }

    #[test]
    fn own_ship_and_other_sentences_are_ignored() {
        let own = resum(&MOORED.replace("AIVDM", "AIVDO"));
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
