//! CAN frames and ports.
//!
//! `TxPort` sends, but only the daemon holds one, and only gate-approved
//! frames (carpi-gate) reach it.

pub mod candump;
mod frame;
mod port;
pub mod recorder;
mod tx;

pub use frame::{Frame, RxFrame};
pub use port::{Port, ReplayPort};
pub use tx::TxPort;
