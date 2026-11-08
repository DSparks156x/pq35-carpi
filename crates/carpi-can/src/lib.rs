//! CAN frames and ports.
//!
//! Nothing in this crate transmits. Sending goes through the TX gate, which
//! arrives with the bus-state machine (plan step 3).

pub mod candump;
mod frame;
mod port;
pub mod recorder;

pub use frame::{Frame, RxFrame};
pub use port::{Port, ReplayPort};
