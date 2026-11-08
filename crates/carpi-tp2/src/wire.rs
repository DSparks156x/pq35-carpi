//! TP2.0 byte layouts. Pure functions, no state.
//!
//! Refs used below:
//! - `[trace]` RNS-E-Hudiy `references/logs/2026-06-17/tp2_worker_2.log`, engine 0x01
//!   on Drew's TT. Proven on the car.
//! - `[jazdw]` `references/tp2diag references/VW-TP-2.0.txt` (jazdw.net/tp20). Docs,
//!   reverse engineered, not official.
//! - `[oldtp2]` RNS-E-Hudiy `flasher/vag_protocols/tp2.py`. Code that ran on the car.
//! - `[dis]` pq35-carpi `docs/research/dis.md` section 1. A3 bench.

use std::time::Duration;

/// Channel setup requests go out on this ID; replies come back on `SETUP_ID + module`.
/// Proven `[trace]`.
pub const SETUP_ID: u32 = 0x200;
/// Setup request opcode. Proven `[trace]`.
pub const OP_SETUP: u8 = 0xC0;
/// Positive setup reply. Proven `[trace]`.
pub const OP_SETUP_OK: u8 = 0xD0;
/// Application type byte in setup. Always 0x01 for KWP. Proven `[trace]`; meaning from `[jazdw]`.
pub const APP_KWP: u8 = 0x01;

pub const OP_PARAMS_REQ: u8 = 0xA0;
pub const OP_PARAMS_RESP: u8 = 0xA1;
/// Channel test (keepalive); the reply is an A1. Proven `[trace]`.
pub const OP_TEST: u8 = 0xA3;
/// Break: receiver drops data since the last ACK. `[jazdw]` only; never seen on the car.
pub const OP_BREAK: u8 = 0xA4;
/// Disconnect; the receiver answers A8. Proven that modules accept it `[trace]`;
/// the reply etiquette is `[jazdw]`.
pub const OP_DISCONNECT: u8 = 0xA8;

/// Highest sequence-numbered block the protocol allows (4-bit sequence).
pub const MAX_BLOCK: u8 = 15;

/// Decode a timing byte: bits 7-6 unit (0.1, 1, 10, 100 ms), bits 5-0 count.
/// `[jazdw]`, confirmed by the engine's A1 `0F 8A FF 4A FF` = T1 100 ms, T3 10 ms `[trace]`.
/// (The old worker logged "T1=1380ms, T3=7.4ms" for the same bytes; that was a decode bug.)
pub fn decode_timing(b: u8) -> Duration {
    const UNIT_US: [u64; 4] = [100, 1_000, 10_000, 100_000];
    Duration::from_micros(UNIT_US[(b >> 6) as usize] * u64::from(b & 0x3F))
}

/// Encode a duration as a timing byte, using the finest unit that holds it.
/// Rounds up. Saturates at 6.3 s (`0xFF`).
pub fn encode_timing(d: Duration) -> u8 {
    const UNIT_US: [u64; 4] = [100, 1_000, 10_000, 100_000];
    let us = d.as_micros().min(u128::from(u64::MAX)) as u64;
    for (i, unit) in UNIT_US.iter().enumerate() {
        let count = us.div_ceil(*unit);
        if count <= 0x3F {
            return ((i as u8) << 6) | count as u8;
        }
    }
    0xFF
}

/// Channel parameters carried in A0/A1 (`op BS T1 T2 T3 T4`; T2 and T4 are always 0xFF).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// Frames the sender may send before it must ask for an ACK.
    pub block_size: u8,
    /// ACK wait time.
    pub t1: Duration,
    /// Gap between frames.
    pub t3: Duration,
}

impl Params {
    /// What we ask for on diagnostic channels: `A0 0F 8A FF 32 FF` = BS 15, T1 100 ms,
    /// T3 5 ms. Proven `[trace]`.
    pub const DIAG: Params = Params {
        block_size: 15,
        t1: Duration::from_millis(100),
        t3: Duration::from_millis(5),
    };
    /// What the white cluster and the old DDP driver use: `0F 8A FF 4A FF` = BS 15,
    /// T1 100 ms, T3 10 ms. Bench `[dis]`. The engine answers the same bytes `[trace]`.
    pub const DDP: Params = Params {
        block_size: 15,
        t1: Duration::from_millis(100),
        t3: Duration::from_millis(10),
    };

    /// `op BS T1 FF T3 FF`.
    pub fn encode(&self, op: u8) -> [u8; 6] {
        [
            op,
            self.block_size,
            encode_timing(self.t1),
            0xFF,
            encode_timing(self.t3),
            0xFF,
        ]
    }

    /// Decode the bytes after the A0/A1 opcode. Lenient: missing bytes take the
    /// `DIAG` values. The red cluster sends a short `A0 07 00` `[dis]` (unproven).
    pub fn decode(body: &[u8]) -> Params {
        let d = Params::DIAG;
        Params {
            block_size: body.first().copied().unwrap_or(d.block_size),
            t1: body.get(1).map_or(d.t1, |b| decode_timing(*b)),
            t3: body.get(3).map_or(d.t3, |b| decode_timing(*b)),
        }
    }
}

/// One parsed frame on an open channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pdu<'a> {
    /// High nibble 0..3: bit 0 set = last frame of the message, bit 1 set = no ACK
    /// wanted. So `0n` more+ACK, `1n` last+ACK, `2n` more, `3n` last. `[jazdw]`,
    /// proven for 1n/2n `[trace]` and 0n/1n/2n `[dis]`.
    Data {
        seq: u8,
        last: bool,
        wants_ack: bool,
        payload: &'a [u8],
    },
    /// `Bn` ready, `9n` not ready; `n` is the next sequence the receiver expects.
    /// Bn proven `[trace]`; 9n proven on the bench `[dis]`. `Dn`/`Fn` decode like
    /// 9n/Bn because the stock masks do (`[dis]`, unproven).
    Ack {
        next: u8,
        ready: bool,
    },
    ParamsRequest(Params),
    ParamsResponse(Params),
    Test,
    Break,
    Disconnect,
    Unknown(u8),
}

pub fn parse(data: &[u8]) -> Option<Pdu<'_>> {
    let b0 = *data.first()?;
    let (op, low) = (b0 >> 4, b0 & 0x0F);
    Some(match op {
        0..=3 => Pdu::Data {
            seq: low,
            last: op & 1 != 0,
            wants_ack: op & 2 == 0,
            payload: &data[1..],
        },
        0x9 | 0xB | 0xD | 0xF => Pdu::Ack {
            next: low,
            ready: op & 0x2 != 0,
        },
        _ => match b0 {
            OP_PARAMS_REQ => Pdu::ParamsRequest(Params::decode(&data[1..])),
            OP_PARAMS_RESP => Pdu::ParamsResponse(Params::decode(&data[1..])),
            OP_TEST => Pdu::Test,
            OP_BREAK => Pdu::Break,
            OP_DISCONNECT => Pdu::Disconnect,
            other => Pdu::Unknown(other),
        },
    })
}

/// Data frame header byte.
pub fn data_header(seq: u8, last: bool, wants_ack: bool) -> u8 {
    let mut op = 0u8;
    if last {
        op |= 1;
    }
    if !wants_ack {
        op |= 2;
    }
    (op << 4) | (seq & 0x0F)
}

/// `Bn`: ACK, ready, next expected sequence `n`. Proven `[trace]`.
pub fn ack(next: u8) -> u8 {
    0xB0 | (next & 0x0F)
}

/// 11-bit ID from a little-endian ID field whose high byte carries a validity flag
/// in its high nibble (1 = invalid). `[jazdw]`; `00 03` and `40 07` proven `[trace]`.
fn id_field(lo: u8, hi: u8) -> (u16, bool) {
    (u16::from(lo) | (u16::from(hi & 0x07) << 8), hi & 0x10 == 0)
}

/// Setup request, sent on `SETUP_ID`:
/// `[module, C0, 00, 10, rx lo, rx hi, 01]`. Bytes 2-3 are the module's own RX ID,
/// left for the module to choose (validity bit set). Bytes 4-5 are the ID the module
/// must transmit on, i.e. our tester RX ID. Proven with 0x300 `[trace]`.
pub fn setup_request(module: u8, tester_rx: u16) -> [u8; 7] {
    [
        module,
        OP_SETUP,
        0x00,
        0x10,
        (tester_rx & 0xFF) as u8,
        ((tester_rx >> 8) & 0x07) as u8,
        APP_KWP,
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupReply {
    /// `[00, D0, tester lo, tester hi, module rx lo, module rx hi, 01]`: we listen on
    /// `tester_rx`, we send to `module_rx`. Proven `00 D0 00 03 40 07 01` `[trace]`.
    Accepted { tester_rx: u16, module_rx: u16 },
    /// D6..D8 per `[jazdw]`. The old survey only knew "not D0". Layout of the rest
    /// of the frame is unknown.
    Refused(u8),
}

/// Parse a 7-byte frame from `SETUP_ID + module`. `None` for anything else.
pub fn parse_setup_reply(data: &[u8]) -> Option<SetupReply> {
    if data.len() != 7 {
        return None;
    }
    match data[1] {
        OP_SETUP_OK => {
            let (tester_rx, _) = id_field(data[2], data[3]);
            let (module_rx, valid) = id_field(data[4], data[5]);
            if !valid || module_rx == 0 || module_rx == tester_rx {
                return None;
            }
            Some(SetupReply::Accepted {
                tester_rx,
                module_rx,
            })
        }
        op @ 0xD6..=0xD8 => Some(SetupReply::Refused(op)),
        _ => None,
    }
}

/// If `data` is a setup request (`[module, C0, ...]` on `SETUP_ID`), return
/// `(module, tester_rx)`. Lets a channel manager spot a foreign tester (VCDS asks
/// for 0x300).
pub fn parse_setup_request(data: &[u8]) -> Option<(u8, u16)> {
    if data.len() != 7 || data[1] != OP_SETUP {
        return None;
    }
    Some((data[0], id_field(data[4], data[5]).0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_bytes() {
        assert_eq!(decode_timing(0x8A), Duration::from_millis(100));
        assert_eq!(decode_timing(0x32), Duration::from_millis(5));
        assert_eq!(decode_timing(0x4A), Duration::from_millis(10));
        assert_eq!(decode_timing(0xFF), Duration::from_millis(6300));
        assert_eq!(encode_timing(Duration::from_millis(100)), 0x8A);
        assert_eq!(encode_timing(Duration::from_millis(5)), 0x32);
        assert_eq!(encode_timing(Duration::from_millis(10)), 0x4A);
        assert_eq!(encode_timing(Duration::from_secs(60)), 0xFF);
        for b in 0..=255u8 {
            // Round trip is exact for the canonical (finest-unit) encoding.
            let d = decode_timing(b);
            assert_eq!(decode_timing(encode_timing(d)), d);
        }
    }

    #[test]
    fn params_round_trip() {
        assert_eq!(
            Params::DIAG.encode(OP_PARAMS_REQ),
            [0xA0, 0x0F, 0x8A, 0xFF, 0x32, 0xFF]
        );
        assert_eq!(
            Params::DDP.encode(OP_PARAMS_RESP),
            [0xA1, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]
        );
        assert_eq!(
            parse(&[0xA1, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]),
            Some(Pdu::ParamsResponse(Params::DDP))
        );
        // Red cluster's short A0.
        let Some(Pdu::ParamsRequest(p)) = parse(&[0xA0, 0x07, 0x00]) else {
            panic!()
        };
        assert_eq!(p.block_size, 7);
        assert_eq!(p.t1, Duration::ZERO);
    }

    #[test]
    fn setup_frames() {
        assert_eq!(
            setup_request(0x01, 0x300),
            [0x01, 0xC0, 0x00, 0x10, 0x00, 0x03, 0x01]
        );
        assert_eq!(
            parse_setup_reply(&[0x00, 0xD0, 0x00, 0x03, 0x40, 0x07, 0x01]),
            Some(SetupReply::Accepted {
                tester_rx: 0x300,
                module_rx: 0x740
            })
        );
        assert_eq!(
            parse_setup_reply(&[0x00, 0xD6, 0, 0, 0, 0, 0]),
            Some(SetupReply::Refused(0xD6))
        );
        assert_eq!(parse_setup_reply(&[0x00, 0xD0, 0x00, 0x03]), None);
        assert_eq!(
            parse_setup_request(&setup_request(0x1F, 0x308)),
            Some((0x1F, 0x308))
        );
    }

    #[test]
    fn data_and_ack_opcodes() {
        assert_eq!(data_header(0, true, true), 0x10);
        assert_eq!(data_header(1, false, false), 0x21);
        assert_eq!(data_header(5, false, true), 0x05);
        assert_eq!(data_header(5, true, false), 0x35);
        assert_eq!(
            parse(&[0x14, 0x00, 0x25]),
            Some(Pdu::Data {
                seq: 4,
                last: true,
                wants_ack: true,
                payload: &[0x00, 0x25]
            })
        );
        assert_eq!(
            parse(&[0xB5]),
            Some(Pdu::Ack {
                next: 5,
                ready: true
            })
        );
        assert_eq!(
            parse(&[0x93]),
            Some(Pdu::Ack {
                next: 3,
                ready: false
            })
        );
        assert_eq!(parse(&[0xA8]), Some(Pdu::Disconnect));
        assert_eq!(parse(&[]), None);
    }
}
