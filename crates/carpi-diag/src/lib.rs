//! Diagnostics: value catalog, TP2 channel manager, scheduler, exclusive modes
//! and jobs. Sans-IO (PLAN 2.8).
//!
//! One [`Diag`] owns every diagnostic channel on the `diag` bus role. Feed it
//! every received frame with `on_frame(now, f)`, call `tick(now)` at
//! `next_deadline()`, and after each call drain `poll_transmit()` (frames, all
//! `TxClass::Diag`), `write_samples(&mut store)` and `poll_event()`.
//!
//! - [`catalog`]: named values and their providers (ICAN store name or
//!   measuring-block field).
//! - Channel manager: one state machine per module, Closed → Opening → Open →
//!   Grace → Closed, plus Failed with backoff. Tester RX IDs from
//!   [`carpi_tp2::TesterIds`] (0x308..=0x30F, never VCDS's 0x300). A cap on
//!   concurrent channels (default 4) for safety. Each module has its own KWP
//!   client with one request outstanding, so a slow module never stalls the rest.
//! - Scheduler: subscriptions `{value, period, priority, lease_expires}`. ICAN
//!   first; diag values packed into `(module, group)` reads by greedy set
//!   cover; per channel, highest priority due group first, then earliest due.
//!   Granted periods come from a per-group cost estimate against a channel
//!   utilisation cap. Results go into the store under the value names.
//! - Modes: [`Mode::VcdsPause`] (automatic on a foreign tester, or manual),
//!   [`Mode::Exclusive`] (one client holds the [`carpi_kwp::ExclusiveToken`]),
//!   [`Mode::BusLease`] (the daemon's flag; everything stops).
//! - Jobs: install list, DTC read, DTC clear (token only).
//!
//! Defaults marked "ours" are guesses until measured on the car.

pub mod catalog;
mod diag;
mod job;

pub use catalog::{Basis, Catalog, CatalogError, Category, Provider, Unit, ValueDef};
pub use diag::{
    ChannelError, ChannelState, ChannelStatus, Config, Degraded, Diag, Event, Mode, ModeError,
    Sample, Source, SubError, SubId, SubStatus, Subscription, VCDS_TESTER_RX,
};
pub use job::{Job, JobError, JobEvent, JobId, JobOutput, JobStage};
