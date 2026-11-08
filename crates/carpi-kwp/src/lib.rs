//! KWP2000 client over a carpi-tp2 channel. Sans-IO.
//!
//! - [`Request`]s carry a [`Class`]; anything above `ReadOnly` is built with an
//!   [`ExclusiveToken`].
//! - [`KwpClient`] wraps one [`carpi_tp2::Channel`], one request at a time,
//!   handles `7F xx 78`, and refuses modules a [`Profile`] denies (ABS 0x03 by
//!   default).
//! - Typed helpers: session `10 89`, measuring groups `21 gg` with the formula
//!   table, `1A 9B` ident, `1A 9F` installation list, DTC reads.

mod client;
pub mod group;
mod request;
pub mod services;

pub use client::{ABS, KwpClient, KwpError, KwpEvent, Profile, Profiles, ReqId};
pub use request::{Class, ExclusiveToken, Request, RequestError, classify};

#[cfg(test)]
mod tests;
