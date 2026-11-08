//! Measuring blocks: `21 gg` -> `61 gg [formula a b] x N`.
//!
//! Layout proven on the engine (8 records, fields 5-8 often `25 00 00`),
//! RNS-E-Hudiy `references/logs/2026-06-17/tp2_worker_2.log`.
//!
//! The formula table is the OpenHaldex-derived one from RNS-E-Hudiy
//! `tp2/tp2_coding.py`. Only the formulas marked `proven` were checked against
//! values the old worker decoded from that log. The rest are docs.
//! Meaning (which quantity) comes from group and position per ECU part number,
//! not from here; the formula only gives scale and unit.

use crate::request::Request;

pub fn read_group(group: u8) -> Request {
    Request::known_read(vec![0x21, group])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub formula: u8,
    pub a: u8,
    pub b: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub group: u8,
    pub records: Vec<Record>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupError {
    NotGroupReply,
    WrongGroup(u8),
    /// Body isn't a whole number of 3-byte records.
    Ragged(usize),
}

/// Parse a positive `61` reply. Strict: a trailing partial record is an error.
pub fn parse_group(resp: &[u8], group: u8) -> Result<Group, GroupError> {
    if resp.len() < 2 || resp[0] != 0x61 {
        return Err(GroupError::NotGroupReply);
    }
    if resp[1] != group {
        return Err(GroupError::WrongGroup(resp[1]));
    }
    let body = &resp[2..];
    if !body.len().is_multiple_of(3) {
        return Err(GroupError::Ragged(body.len()));
    }
    Ok(Group {
        group,
        records: body
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| Record {
                formula: c[0],
                a: c[1],
                b: c[2],
            })
            .collect(),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub value: Value,
    pub unit: &'static str,
}

impl Record {
    /// `None` for formulas not in the table: show `formula a b` raw.
    pub fn decode(&self) -> Option<Decoded> {
        decode(self.formula, self.a, self.b)
    }
}

fn num(v: f64, unit: &'static str) -> Option<Decoded> {
    Some(Decoded {
        value: Value::Number(v),
        unit,
    })
}

fn text(s: String, unit: &'static str) -> Option<Decoded> {
    Some(Decoded {
        value: Value::Text(s),
        unit,
    })
}

/// The common formula table. `a`/`b` as floats below.
pub fn decode(formula: u8, a: u8, b: u8) -> Option<Decoded> {
    let (fa, fb) = (f64::from(a), f64::from(b));
    let w = f64::from(u16::from(a) << 8 | u16::from(b));
    match formula {
        // proven: 01 C8 15 = 840 rpm
        0x01 => num(0.2 * fa * fb, "rpm"),
        0x02 => num(0.002 * fa * fb, "%"),
        0x03 => num(0.002 * fa * fb, "deg"),
        0x04 if a == 1 => num(0.75 * fb - 48.0, "°C"),
        0x04 => num((fb - 127.0).abs() * 0.01 * fa, "deg"),
        // proven: 05 0A A0 = 60 °C
        0x05 => num(fa * (fb - 100.0) * 0.1, "°C"),
        0x06 => num(0.001 * fa * fb, "V"),
        0x07 => num(0.01 * fa * fb, "km/h"),
        0x08 => num(0.1 * fa * fb, ""),
        0x09 => num((fb - 127.0) * 0.02 * fa, "deg"),
        0x0A => text(if b == 0 { "COLD" } else { "WARM" }.into(), ""),
        0x0B => num(0.0001 * fa * (fb - 128.0) + 1.0, ""),
        0x0C => num(0.001 * fa * fb, "ohm"),
        0x0D => num((fb - 127.0) * 0.001 * fa, "mm"),
        0x0E => num(0.01 * fa * (fb - 100.0), "bar"),
        0x0F => num(0.01 * fa * fb, "ms"),
        0x10 => {
            // a = mask, b = bits.
            let s = (0..8)
                .rev()
                .map(|i| match (a >> i & 1, b >> i & 1) {
                    (0, _) => 'X',
                    (_, 1) => '1',
                    _ => '0',
                })
                .collect();
            text(s, "bits")
        }
        0x11 => text(format!("{}{}", ascii(a), ascii(b)), ""),
        // proven: 12 7D 9F = 795 mbar
        0x12 => num(0.04 * fa * fb, "mbar"),
        0x13 => num(0.01 * fa * fb, "L"),
        0x14 => num(fa * (fb - 128.0) / 128.0, "%"),
        0x15 => num(0.001 * fa * fb, "V"),
        0x16 => num(0.001 * fa * fb, "ms"),
        0x17 => num(fb / 256.0 * fa, "%"),
        0x18 => num(0.001 * fa * fb, "A"),
        // proven: 19 20 03 = 4.44 g/s
        0x19 => num(fa / 182.0 + 1.421 * fb, "g/s"),
        0x1A | 0x1C => num(fb - fa, "°C"),
        // proven: 1B 4B 79 = 5.25 deg
        0x1B => num((fb - 128.0).abs() * 0.01 * fa, "deg"),
        0x1E => num(fb / 12.0 * fa, "deg"),
        0x1F => num(fb / 2560.0 * fa, "°C"),
        // proven: 21 85 22 = 25.56 %, 21 FF 0D = 5.1 %. The current tp2_coding.py
        // says 0.01*a*b, which does not match the log; 100*b/a does.
        0x21 => num(if a == 0 { 100.0 * fb } else { 100.0 * fb / fa }, "%"),
        0x22 | 0x2A => num((fb - 128.0) * 0.01 * fa, "kW"),
        0x23 => num(0.01 * fa * fb, "L/h"),
        0x24 => num(w, "deg"),
        // Table says seconds. Engine fields 5-8 come back `25 00 00` (unused slots).
        0x25 => num(w, "s"),
        0x26 => num((fb - 128.0) * 0.001 * fa, "deg"),
        0x27 => num(fb / 256.0 * fa, "mg/h"),
        0x28 => num(0.001 * fa * fb, "A"),
        0x29 => num(0.1 * fa * fb, "Ah"),
        0x2B => num(fb * 0.1 + 25.5 * fa, "V"),
        0x2C => text(format!("{a}:{b:02}"), "h:m"),
        0x2D => num(fa * fb / 1000.0, ""),
        0x2E => num((fa * fb - 3200.0) * 0.0027, "deg"),
        0x2F => num((fb - 128.0) * fa, "ms"),
        0x30 => num(w, ""),
        0x31 => num(fb / 4.0 * fa * 0.1, "mg/h"),
        0x32 => num(
            if a == 0 {
                (fb - 128.0) / 0.01
            } else {
                (fb - 128.0) / (0.01 * fa)
            },
            "mbar",
        ),
        0x33 => num((fb - 128.0) / 255.0 * fa, "mg/h"),
        0x34 => num(fb * 0.02 * fa - fa, "Nm"),
        0x35 => num((fb - 128.0) * 1.4222 + 0.006 * fa, "g/s"),
        0x36 => num(w, "count"),
        0x37 => num(fa * fb / 200.0, "s"),
        0x38 | 0x39 => num(w, "WSC"),
        0x3B => num(w / 32768.0, ""),
        0x3C => num(w * 0.01, "s"),
        0x3D => num(
            if a == 0 {
                fb - 128.0
            } else {
                (fb - 128.0) / fa
            },
            "",
        ),
        0x3E => num(0.256 * fa * fb, "S"),
        0x3F => text(format!("{}{}?", ascii(a), ascii(b)), ""),
        0x40 => num(fa + fb, "ohm"),
        0x41 => num(0.01 * fa * (fb - 127.0), "mm"),
        0x42 => num(fa * fb / 511.12, "V"),
        0x43 => num(640.0 * fa + fb * 2.5, "deg"),
        0x44 => num(w / 7.365, "deg/s"),
        0x45 => num(w * 0.3254, "bar"),
        0x46 => num(w * 0.192, "m/s2"),
        0x47 => num(0.1 * fa * fb, "%"),
        0x48 => num(0.1 * fa * fb, "L"),
        0x51 => num(f64::from(i16::from_be_bytes([a, b])) * 0.04375, ""),
        0x53 => num(w * 0.01, "bar"),
        0x5E => num(0.1 * fa * (fb - 128.0), "Nm"),
        _ => None,
    }
}

fn ascii(c: u8) -> char {
    if (0x20..0x7F).contains(&c) {
        c as char
    } else {
        '.'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(f: u8, a: u8, b: u8) -> f64 {
        match decode(f, a, b).unwrap().value {
            Value::Number(v) => v,
            Value::Text(t) => panic!("{t}"),
        }
    }

    #[test]
    fn log_values() {
        // Group 3 and 113 from tp2_worker_2.log, as the old worker published them.
        assert!((n(0x01, 0xC8, 0x15) - 840.0).abs() < 1e-9);
        assert!((n(0x19, 0x20, 0x03) - 4.44).abs() < 0.01);
        assert!((n(0x21, 0xFF, 0x0D) - 5.1).abs() < 0.01);
        assert!((n(0x1B, 0x4B, 0x79) - 5.25).abs() < 1e-9);
        assert!((n(0x05, 0x0A, 0xA0) - 60.0).abs() < 1e-9);
        assert!((n(0x05, 0x0A, 0x7D) - 25.0).abs() < 1e-9);
        assert!((n(0x21, 0x85, 0x22) - 25.56).abs() < 0.01);
        assert!((n(0x12, 0x7D, 0x9F) - 795.0).abs() < 1e-9);
        assert_eq!(decode(0x05, 1, 1).unwrap().unit, "°C");
        assert!(decode(0x7D, 0, 0).is_none());
        assert_eq!(
            decode(0x10, 0xF0, 0xA5).unwrap().value,
            Value::Text("1010XXXX".into())
        );
    }

    #[test]
    fn parse_engine_group_3() {
        let resp = [
            0x61, 0x03, 0x01, 0xC8, 0x15, 0x19, 0x20, 0x03, 0x21, 0xFF, 0x0D, 0x1B, 0x4B, 0x79,
            0x05, 0x0A, 0xA0, 0x05, 0x0A, 0x7D, 0x25, 0x00, 0x00, 0x25, 0x00, 0x00,
        ];
        let g = parse_group(&resp, 3).unwrap();
        assert_eq!(g.records.len(), 8);
        assert_eq!(
            g.records[7],
            Record {
                formula: 0x25,
                a: 0,
                b: 0
            }
        );
        assert_eq!(parse_group(&resp, 4), Err(GroupError::WrongGroup(3)));
        assert_eq!(parse_group(&resp[..25], 3), Err(GroupError::Ragged(23)));
    }
}
