//! ICAN IDs and decoders.
//!
//! Every CAN ID the project uses is a constant in [`ids`]. Decoders are
//! written by hand, one per message, with the PQ35/46 ICAN DBC and
//! `ICAN_rundown.md` as references. Only what something needs is decoded;
//! add signals as features need them.

pub mod bits;
pub mod ican;
pub mod ids;

pub use ican::{Emit, MESSAGES, Message, decode, message};
