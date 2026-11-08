//! Wire-level simulators on virtual time.
//!
//! - [`bus`]: a virtual CAN bus that runs any set of [`bus::Node`]s.
//! - [`tp2`]: a gateway answering TP2 setups and KWP modules behind it, with
//!   car-measured latencies and scripted faults (`9n`, lost ACKs, `7F xx 78`,
//!   drop-outs, refused setup).
//! - [`ddp`]: the instrument cluster's DDP peer on 0x6C0/0x6C1: handshake,
//!   centre ownership with scripted busy/free/reinit, `9n`, drop-out, and
//!   checks on every drawing message.
//!
//! Other simulators (RNS-E, NM ring) go in their own modules.

pub mod bus;
pub mod ddp;
pub mod tp2;
