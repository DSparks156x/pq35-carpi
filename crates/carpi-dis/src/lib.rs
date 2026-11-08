//! Cluster display (DIS) driver for the white/red PQ35 cluster, sans-IO.
//!
//! Layers, bottom up (design: `docs/DIS.md`, decisions: `docs/PLAN.md` §2.7, §6):
//! - [`session`]: DDP over a [`carpi_tp2::Channel`] on 0x6C0/0x6C1. Handshake,
//!   claim, busy/free/reinit, one generation counter, one frame in flight.
//! - [`cmd`]: typed `52`/`55`/`57`/`63`/`39` commands with the hard limits.
//! - [`pack`]: groups into <=105 B messages, in order, with clip/ROP priming and
//!   the command-buffer rule (a group never straddles an auto-commit).
//! - [`font`]: measured glyph advances and AUDSCII.
//! - [`helpers`]: `NumberField`, `ScrollText`, `QuickThenRefineIcon`, `GlyphCells`.
//! - [`screens`]: nav and readings, written by hand.
//! - [`reading_art`]: the old readings icons and unit legends (data).
//! - [`arbiter`]: per-surface winner from a rule table.
//! - [`top`]: the 0x363/0x365 text lines (frames only; the caller gates).
//! - [`dis`]: ties them together; feed it frames and time, drain frames.
//! - [`fb`]: a 1-bit model of what we believe is on screen, for tests.
//!
//! Outputs are CAN frames: link frames from [`Dis::poll_transmit`]
//! (`TxClass::DisplayDdp`) and top-line frames from [`top::TopLines`]
//! (`TxClass::DisplayTop`). Nothing here sleeps or touches a socket.
//!
//! Fact tags: **bench** (A3 white cluster, 500 kbit/s), **car**, **guess**.
//! `dc/` = RNS-E-Hudiy `dis_client/`.

pub mod arbiter;
pub mod cmd;
pub mod dis;
pub mod fb;
pub mod font;
mod font_data;
pub mod helpers;
pub mod icons;
pub mod image;
mod nav_icon_data;
pub mod reading_art;
pub mod pack;
pub mod proto;
pub mod screens;
pub mod session;
pub mod top;

pub use dis::{Dis, DisConfig, ScreenId};
pub use font_data::STOCK_BAR;
pub use session::{Session, SessionConfig, SessionEvent};
