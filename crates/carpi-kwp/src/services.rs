//! Typed helpers for the other read-only services, plus DTC clear.
//! Refs: RNS-E-Hudiy `docs/research/diagnostics.md` (rnse-carpi) section 1, KWP table.

use crate::request::{ExclusiveToken, Request, RequestError};

/// `10 89`: the session every old connect opened. Proven on engine and Haldex.
pub fn start_session() -> Request {
    Request::known_read(vec![0x10, 0x89])
}

/// `3E`: tester present. Coded in the old worker; never shown to be needed next to A3.
pub fn tester_present() -> Request {
    Request::known_read(vec![0x3E])
}

/// `1A 9B`: identification (part number, SW, coding). Proven via the old survey.
pub fn read_ident() -> Request {
    Request::known_read(vec![0x1A, 0x9B])
}

/// Printable text of a `5A 9B` reply. Field layout varies by module; this is for
/// display and for keying group tables, not for parsing fields.
pub fn ident_text(resp: &[u8]) -> Option<String> {
    if resp.len() < 2 || resp[0] != 0x5A || resp[1] != 0x9B {
        return None;
    }
    Some(
        resp[2..]
            .iter()
            .map(|&c| {
                if (0x20..0x7F).contains(&c) {
                    c as char
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// `1A 9B` fields at fixed offsets after `5A 9B`: part number (12 ASCII),
/// software version (4 ASCII), 10 bytes of binary metadata, then the
/// component text. The part, version and component offsets are the ones the
/// Haldex and EPS flashers parse (PQ35-Flasher `engine.py`,
/// `parse_vag_identification`), checked on Drew's Haldex replies. The 10
/// metadata bytes hold coding / workshop / programming data in a layout not
/// yet known (Drew's Haldex: `03 00 00 05 20 26 01 01 00 01`, the middle
/// looking like a BCD flash date), so they're kept raw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ident {
    pub part_no: Option<String>,
    pub sw_version: Option<String>,
    pub component: Option<String>,
    /// Coding as upper-case hex (`22 06 00`, VW's coding DID).
    pub coding: Option<String>,
    /// Bytes 16..26 of the `5A 9B` payload.
    pub metadata: Vec<u8>,
}

impl Ident {
    /// Nothing at all came back.
    pub fn is_empty(&self) -> bool {
        self.part_no.is_none()
            && self.sw_version.is_none()
            && self.component.is_none()
            && self.coding.is_none()
    }
}

/// Printable ASCII, trimmed; `None` when nothing is left.
fn ascii_field(b: &[u8]) -> Option<String> {
    let s: String = b
        .iter()
        .map(|&c| {
            if (0x20..0x7F).contains(&c) {
                c as char
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!s.is_empty()).then_some(s)
}

/// Parse a `5A 9B` reply. `None` if it isn't one or is shorter than the part
/// number and version.
pub fn parse_ident(resp: &[u8]) -> Option<Ident> {
    if resp.len() < 2 + 16 || resp[0] != 0x5A || resp[1] != 0x9B {
        return None;
    }
    let p = &resp[2..];
    Some(Ident {
        part_no: ascii_field(&p[..12]),
        sw_version: ascii_field(&p[12..16]),
        component: p.get(26..).and_then(ascii_field),
        coding: None,
        metadata: p[16..p.len().min(26)].to_vec(),
    })
}

/// `22 hi lo`: read a common identifier. The PQ BCM answers `NRC 11` to every
/// `1A` and these instead (RNS-E-Hudiy `module_survey.py`, proven on the BCM).
pub fn read_did(did: u16) -> Request {
    let [hi, lo] = did.to_be_bytes();
    Request::known_read(vec![0x22, hi, lo])
}

/// VW spare part number.
pub const DID_PART: u16 = 0xF187;
/// VW software version.
pub const DID_SW: u16 = 0xF189;
/// System name / component.
pub const DID_SYSTEM: u16 = 0xF197;
/// VW coding value.
pub const DID_CODING: u16 = 0x0600;

/// The data of a `62 hi lo data` reply to `did`.
pub fn did_data(resp: &[u8], did: u16) -> Option<&[u8]> {
    let [hi, lo] = did.to_be_bytes();
    match resp {
        [0x62, h, l, data @ ..] if *h == hi && *l == lo => Some(data),
        _ => None,
    }
}

/// A `22` reply's data as trimmed ASCII.
pub fn did_text(resp: &[u8], did: u16) -> Option<String> {
    did_data(resp, did).and_then(ascii_field)
}

/// Bytes as upper-case hex, no spaces; `None` when empty.
pub fn hex(b: &[u8]) -> Option<String> {
    (!b.is_empty()).then(|| b.iter().map(|x| format!("{x:02X}")).collect())
}

/// TP2 logical ID of the gateway, which serves the installation list.
pub const GATEWAY: u8 = 0x1F;

/// `1A 9F` to the gateway (TP2 0x1F). Proven.
pub fn read_install_list() -> Request {
    Request::known_read(vec![0x1A, 0x9F])
}

/// One installation-list record: `[VCDS addr, TP2 id, bus, status]`. Proven layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Installed {
    /// Workshop (VCDS) address, e.g. 0x01 engine, 0x22 AWD.
    pub vcds: u8,
    /// Logical ID for TP2 setup.
    pub tp2: u8,
    pub bus: u8,
    pub status: u8,
}

impl Installed {
    /// Bit 0. Proven.
    pub fn coded(&self) -> bool {
        self.status & 0x01 != 0
    }
    /// Bit 3. Guess.
    pub fn unreachable(&self) -> bool {
        self.status & 0x08 != 0
    }
    /// Bit 2. Guess.
    pub fn fault(&self) -> bool {
        self.status & 0x04 != 0
    }
}

/// Parse `5A 9F <header> [rec]... FF`. Stops at a record starting 0xFF or at the end.
pub fn parse_install_list(resp: &[u8]) -> Option<Vec<Installed>> {
    if resp.len() < 3 || resp[0] != 0x5A || resp[1] != 0x9F {
        return None;
    }
    let mut out = Vec::new();
    for c in resp[3..].as_chunks::<4>().0 {
        if c[0] == 0xFF {
            break;
        }
        out.push(Installed {
            vcds: c[0],
            tp2: c[1],
            bus: c[2],
            status: c[3],
        });
    }
    Some(out)
}

/// DTC read variants in the order the old worker tried them (which modules need
/// which: unknown).
pub fn read_dtcs() -> Request {
    Request::known_read(vec![0x18, 0x02, 0xFF, 0x00])
}
pub fn read_dtcs_all() -> Request {
    Request::known_read(vec![0x18, 0x00, 0xFF, 0x00])
}
pub fn read_dtcs_legacy() -> Request {
    Request::known_read(vec![0x13])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dtc {
    /// VAG code; shown as 5-digit decimal (e.g. 17965).
    pub code: u16,
    /// Raw status byte. VAG KWP status bits are not the UDS ones the old code
    /// used; meaning unknown, so it stays raw.
    pub status: u8,
}

/// Parse `58 n [hi lo status]...` or `53 n [hi lo]...`. `None` if the count and
/// length disagree.
pub fn parse_dtcs(resp: &[u8]) -> Option<Vec<Dtc>> {
    let (&sid, rest) = resp.split_first()?;
    let (&n, body) = rest.split_first()?;
    let width = match sid {
        0x58 => 3,
        0x53 => 2,
        _ => return None,
    };
    if body.len() < usize::from(n) * width {
        return None;
    }
    Some(
        body.chunks_exact(width)
            .take(usize::from(n))
            .map(|c| Dtc {
                code: u16::from_be_bytes([c[0], c[1]]),
                status: if width == 3 { c[2] } else { 0 },
            })
            .collect(),
    )
}

/// `14 FF 00`: clear all DTCs. Persistent, needs a token.
pub fn clear_dtcs(token: &ExclusiveToken) -> Result<Request, RequestError> {
    Request::exclusive(vec![0x14, 0xFF, 0x00], token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Class;

    #[test]
    fn install_list() {
        let resp = [
            0x5A, 0x9F, 0x00, 0x01, 0x01, 0x00, 0x01, 0x09, 0x20, 0x01, 0x01, 0x46, 0x21, 0x01,
            0x08, 0xFF, 0x00, 0x00, 0x00,
        ];
        let l = parse_install_list(&resp).unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(
            l[1],
            Installed {
                vcds: 0x09,
                tp2: 0x20,
                bus: 1,
                status: 1
            }
        );
        assert!(l[1].coded());
        assert!(!l[2].coded() && l[2].unreachable());
        assert!(parse_install_list(&[0x7F, 0x1A, 0x11]).is_none());
    }

    #[test]
    fn dtcs() {
        let d = parse_dtcs(&[0x58, 0x02, 0x46, 0x2D, 0x23, 0x46, 0x2C, 0x60]).unwrap();
        assert_eq!(
            d[0],
            Dtc {
                code: 17965,
                status: 0x23
            }
        );
        assert_eq!(d[1].code, 17964);
        assert_eq!(parse_dtcs(&[0x58, 0x00]), Some(vec![]));
        assert_eq!(parse_dtcs(&[0x58, 0x02, 0x46]), None);
        assert_eq!(parse_dtcs(&[0x53, 0x01, 0x46, 0x2D]).unwrap()[0].status, 0);
        let t = ExclusiveToken::mint(1, Class::Volatile);
        assert!(clear_dtcs(&t).is_err());
    }

    #[test]
    fn ident_fields() {
        // Drew's Haldex (HaldexRE flasher logs).
        let r = [
            &[0x5A, 0x9B][..],
            b"0BR907554A  3016",
            &[0x03, 0x00, 0x00, 0x05, 0x20, 0x26, 0x01, 0x01, 0x00, 0x01],
            b"Haldex 4Motion      ",
        ]
        .concat();
        let i = parse_ident(&r).unwrap();
        assert_eq!(i.part_no.as_deref(), Some("0BR907554A"));
        assert_eq!(i.sw_version.as_deref(), Some("3016"));
        assert_eq!(i.component.as_deref(), Some("Haldex 4Motion"));
        assert_eq!(i.metadata[4..8], [0x20, 0x26, 0x01, 0x01]);
        assert!(i.coding.is_none() && !i.is_empty());
        // Short: part and version only.
        let i = parse_ident(b"\x5A\x9B8J0907115B  0040").unwrap();
        assert_eq!(i.component, None);
        assert!(i.metadata.is_empty());
        assert!(parse_ident(b"\x5A\x9B8J09").is_none());
        assert!(parse_ident(&[0x7F, 0x1A, 0x11]).is_none());

        assert_eq!(read_did(DID_CODING).bytes(), [0x22, 0x06, 0x00]);
        let r = [0x62, 0xF1, 0x87, b'8', b'P', b'0', b' '];
        assert_eq!(did_text(&r, DID_PART).as_deref(), Some("8P0"));
        assert_eq!(did_text(&r, DID_SW), None);
        let c = [0x62, 0x06, 0x00, 0x0A, 0x1B, 0x00];
        assert_eq!(
            did_data(&c, DID_CODING).and_then(hex).as_deref(),
            Some("0A1B00")
        );
        assert_eq!(hex(&[]), None);
    }

    #[test]
    fn ident() {
        let mut r = vec![0x5A, 0x9B];
        r.extend_from_slice(b"8J0907115B  0040\x00\x01MED17.5.1");
        assert_eq!(ident_text(&r).unwrap(), "8J0907115B 0040 MED17.5.1");
    }
}
