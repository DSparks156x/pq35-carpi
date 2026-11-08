//! Wire-level simulators on virtual time.
//!
//! - [`bus`]: a virtual CAN bus that runs any set of [`bus::Node`]s.
//! - [`tp2`]: a gateway answering TP2 setups and KWP modules behind it, with
//!   car-measured latencies and scripted faults (`9n`, lost ACKs, `7F xx 78`,
//!   drop-outs, refused setup).
//!
//! Other simulators (cluster DDP peer, RNS-E, NM ring) go in their own modules.

pub mod bus;
pub mod tp2;
