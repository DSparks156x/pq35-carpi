//! candump text formats.
//!
//! Reads both the default screen format (`  can0  470   [8]  80 00 ...`,
//! optionally with a leading `(timestamp)`) and the log format
//! (`(1699999999.123456) can0 470#8000...`). Writes the log format.

use crate::Frame;
use std::io::{self, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Seconds, as written in the file (absolute or relative).
    pub ts: Option<f64>,
    pub iface: String,
    pub frame: Frame,
}

/// Parses one line. Returns `None` for blank lines, comments, remote/FD/error
/// frames and anything else that isn't a classic data frame.
pub fn parse_line(line: &str) -> Option<Line> {
    let mut rest = line.trim();
    if rest.is_empty() || rest.starts_with('#') {
        return None;
    }
    let mut ts = None;
    if let Some(stripped) = rest.strip_prefix('(') {
        let end = stripped.find(')')?;
        ts = stripped[..end].trim().parse::<f64>().ok();
        rest = stripped[end + 1..].trim_start();
    }
    let mut parts = rest.split_whitespace();
    let iface = parts.next()?.to_string();
    let second = parts.next()?;

    let frame = if let Some((id, payload)) = second.split_once('#') {
        // Log format. `##` is CAN FD, `R` is remote: skip both.
        if payload.starts_with('#') || payload.starts_with('R') {
            return None;
        }
        let (id, extended) = parse_id(id)?;
        let data = parse_hex_run(payload)?;
        make(id, extended, &data)?
    } else {
        let (id, extended) = parse_id(second)?;
        let len_tok = parts.next()?;
        let len: usize = len_tok.strip_prefix('[')?.strip_suffix(']')?.parse().ok()?;
        let mut data = Vec::with_capacity(len);
        for tok in parts.by_ref().take(len) {
            data.push(u8::from_str_radix(tok, 16).ok()?);
        }
        if data.len() != len {
            return None;
        }
        make(id, extended, &data)?
    };
    Some(Line { ts, iface, frame })
}

fn parse_id(s: &str) -> Option<(u32, bool)> {
    let id = u32::from_str_radix(s, 16).ok()?;
    // candump prints extended IDs as 8 hex digits.
    Some((id, s.len() > 3))
}

fn parse_hex_run(s: &str) -> Option<Vec<u8>> {
    let s = s.replace('.', "");
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn make(id: u32, extended: bool, data: &[u8]) -> Option<Frame> {
    if extended {
        Frame::new_extended(id, data)
    } else {
        Frame::new(id, data)
    }
}

/// Writes one frame in candump log format.
pub fn write_line(
    w: &mut impl Write,
    wall: SystemTime,
    iface: &str,
    frame: &Frame,
) -> io::Result<()> {
    let t = wall.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    write!(w, "({}.{:06}) {} ", t.as_secs(), t.subsec_micros(), iface)?;
    if frame.extended {
        write!(w, "{:08X}#", frame.id)?;
    } else {
        write!(w, "{:03X}#", frame.id)?;
    }
    for b in frame.data() {
        write!(w, "{b:02X}")?;
    }
    writeln!(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_format() {
        let l = parse_line("  can0  470   [8]  80 00 01 3A 20 00 00 00").unwrap();
        assert_eq!(l.ts, None);
        assert_eq!(l.iface, "can0");
        assert_eq!(l.frame.id, 0x470);
        assert_eq!(l.frame.data(), &[0x80, 0, 1, 0x3A, 0x20, 0, 0, 0]);
    }

    #[test]
    fn screen_format_with_timestamp() {
        let l = parse_line(" (1699999999.500000)  ican  5C3   [2]  39 0B").unwrap();
        assert_eq!(l.ts, Some(1699999999.5));
        assert_eq!(l.iface, "ican");
        assert_eq!(l.frame.data(), &[0x39, 0x0B]);
    }

    #[test]
    fn log_format_roundtrip() {
        let f = Frame::new(0x42B, &[0x16, 0x01, 0, 0, 0, 0]).unwrap();
        let wall = UNIX_EPOCH + Duration::from_micros(1_700_000_000_123_456);
        let mut out = Vec::new();
        write_line(&mut out, wall, "ican", &f).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "(1700000000.123456) ican 42B#160100000000\n");
        let l = parse_line(&text).unwrap();
        assert_eq!(l.frame, f);
        assert_eq!(l.iface, "ican");
    }

    #[test]
    fn extended_and_empty() {
        let l = parse_line("(1.0) obd 18DAF110#").unwrap();
        assert!(l.frame.extended);
        assert_eq!(l.frame.id, 0x18DAF110);
        assert!(l.frame.is_empty());
    }

    #[test]
    fn skips_junk() {
        assert!(parse_line("").is_none());
        assert!(parse_line("# comment").is_none());
        assert!(parse_line("(1.0) can0 123#R").is_none());
        assert!(parse_line("(1.0) can0 123##1AA").is_none());
        assert!(parse_line("  can0  470   [8]  80 00").is_none());
    }
}
