//! VW TP2.0 data link, sans-IO.
//!
//! One [`Channel`] per peer. Feed it every received frame with `on_frame(now, f)`,
//! call `tick(now)` at `next_deadline()`, and drain `poll_transmit()` and
//! `poll_event()` after every call. Nothing here sleeps or touches a socket.
//!
//! Two entry modes:
//! - [`Channel::dynamic`]: diagnostics. Setup on 0x200 (`C0`/`D0`), then A0/A1,
//!   then length-prefixed messages. Use [`Config::diag`].
//! - [`Channel::fixed`]: a pre-agreed ID pair, no setup. DDP to the cluster on
//!   0x6C0/0x6C1 uses [`Config::ddp`]: A8 flush, A0/A1, unprefixed messages that
//!   end at the `1n` frame. The DDP session handshake (`15 01 01 02 00 00` ...)
//!   is ordinary message data to this layer.
//!
//! The link handles sequence numbers, ACK blocks, `9n` waits, ACK resends, A3
//! keepalive, A8, and reassembly. It knows nothing about KWP or DDP content.
//!
//! Fact provenance is marked inline: proven on the car, bench, docs, or ours.

mod channel;
pub mod wire;

pub use channel::{
    Channel, CloseReason, Config, Event, Framing, Gap, LinkError, MsgId, SendError, State,
    TesterIds,
};
pub use wire::Params;

#[cfg(test)]
mod tests;
