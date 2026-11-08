//! Shutdown policy and power backends.
//!
//! CAN is the shutdown trigger: once the ICAN bus has been `Silent` for
//! `silent_for` with Kl15 not on, the policy asks for shutdown. Any bus
//! activity before the backend acts cancels it. The amp-wake GPIO is a
//! secondary input the backend may report; it doesn't drive the policy.
//!
//! [`ShutdownPolicy`] is sans-IO. Backends do the I/O.

use carpi_nm::BusState;
use std::io;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// How long the bus must stay silent before shutdown is requested.
    pub silent_for: Duration,
}

impl Default for Config {
    /// 60 s matches the old amp-wake delay. Tune from an ignition-off capture.
    fn default() -> Self {
        Config {
            silent_for: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerEvent {
    ShutdownRequested,
    /// The bus woke after a request. Only a backend that can abort (the hat
    /// MCU) can act on this.
    ShutdownCancelled,
}

pub struct ShutdownPolicy {
    cfg: Config,
    silent_since: Option<Instant>,
    requested: bool,
}

impl ShutdownPolicy {
    pub fn new(cfg: Config) -> Self {
        ShutdownPolicy {
            cfg,
            silent_since: None,
            requested: false,
        }
    }

    /// Call on every bus-state change and tick. `kl15` is `None` when
    /// unknown (stale); with the bus silent it always will be.
    pub fn update(
        &mut self,
        now: Instant,
        bus: BusState,
        kl15: Option<bool>,
    ) -> Option<PowerEvent> {
        if bus != BusState::Silent || kl15 == Some(true) {
            self.silent_since = None;
            if self.requested {
                self.requested = false;
                return Some(PowerEvent::ShutdownCancelled);
            }
            return None;
        }
        let since = *self.silent_since.get_or_insert(now);
        if !self.requested && now.saturating_duration_since(since) >= self.cfg.silent_for {
            self.requested = true;
            return Some(PowerEvent::ShutdownRequested);
        }
        None
    }

    pub fn requested(&self) -> bool {
        self.requested
    }

    /// When shutdown will be requested if nothing changes.
    pub fn next_deadline(&self) -> Option<Instant> {
        match (self.silent_since, self.requested) {
            (Some(t), false) => Some(t + self.cfg.silent_for),
            _ => None,
        }
    }
}

/// Why the Pi is on. Only the hat MCU can really tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeReason {
    Unknown,
    /// ICAN activity.
    Can,
    /// RNS-E amplifier remote line (unlock, door).
    AmpWake,
    Kl15,
    /// Power returned while halting; the MCU re-powered the Pi.
    Rewake,
}

pub trait PowerBackend {
    /// Power the Pi off. On the CarPiHat this can't be undone.
    fn request_poweroff(&mut self) -> io::Result<()>;

    /// Abort a pending poweroff. `Ok(false)` if the backend can't.
    fn cancel_poweroff(&mut self) -> io::Result<bool> {
        Ok(false)
    }

    fn wake_reason(&self) -> WakeReason;

    /// Call about once a second. A no-op unless something watches the Pi.
    fn heartbeat(&mut self, now: Instant) -> io::Result<()>;
}

/// CarPiHat Pro 5. The 5 V latch is GPIO 25 via the `gpio-poweroff` overlay
/// in config.txt: the kernel drops it after halt and the hat cuts power. So
/// all this does is `systemctl poweroff`, once.
///
/// GPIO 12 (amp wake) isn't read here yet, so the wake reason is unknown.
pub struct CarPiHat {
    command: Vec<String>,
    requested: bool,
}

impl Default for CarPiHat {
    fn default() -> Self {
        Self::with_command(["systemctl", "poweroff"])
    }
}

impl CarPiHat {
    pub fn new() -> Self {
        Self::default()
    }

    /// A different poweroff command (tests, dry runs).
    pub fn with_command<S: Into<String>>(cmd: impl IntoIterator<Item = S>) -> Self {
        CarPiHat {
            command: cmd.into_iter().map(Into::into).collect(),
            requested: false,
        }
    }
}

impl PowerBackend for CarPiHat {
    fn request_poweroff(&mut self) -> io::Result<()> {
        if self.requested {
            return Ok(());
        }
        run(&self.command)?;
        self.requested = true;
        Ok(())
    }

    fn wake_reason(&self) -> WakeReason {
        WakeReason::Unknown
    }

    fn heartbeat(&mut self, _now: Instant) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn run(cmd: &[String]) -> io::Result<()> {
    let (prog, args) = cmd
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty poweroff command"))?;
    tracing::warn!(command = ?cmd, "powering off");
    let status = std::process::Command::new(prog).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{prog} failed: {status}")))
    }
}

#[cfg(not(target_os = "linux"))]
fn run(_cmd: &[String]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "poweroff only on Linux",
    ))
}

/// Planned hat MCU backend. Not built yet (plan step 10); every call fails.
///
/// Planned UART status link:
/// - Pi → MCU: heartbeat about once a second; "halting" with a requested
///   off-delay; "cancel" if the bus wakes before halt.
/// - MCU → Pi: bus state from its own CAN listener, wake reason, its own
///   view of Kl15, supply voltage.
/// - The MCU owns 5 V. It waits for halt (or a timeout) before cutting,
///   re-powers the Pi if wake returns during halt, rides through cranking,
///   and cuts power on a missed heartbeat or low battery.
///
/// The bus-state machine and gate are the same with either backend.
#[derive(Default)]
pub struct HatMcu;

impl HatMcu {
    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "hat MCU backend not built")
    }
}

impl PowerBackend for HatMcu {
    fn request_poweroff(&mut self) -> io::Result<()> {
        Err(Self::unsupported())
    }

    fn cancel_poweroff(&mut self) -> io::Result<bool> {
        Err(Self::unsupported())
    }

    fn wake_reason(&self) -> WakeReason {
        WakeReason::Unknown
    }

    fn heartbeat(&mut self, _now: Instant) -> io::Result<()> {
        Err(Self::unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn silent_then_shutdown() {
        let t0 = Instant::now();
        let mut p = ShutdownPolicy::new(Config::default());
        assert_eq!(p.update(t0, BusState::Awake, Some(true)), None);
        assert_eq!(p.next_deadline(), None);
        assert_eq!(p.update(t0 + s(10), BusState::Silent, None), None);
        assert_eq!(p.next_deadline(), Some(t0 + s(70)));
        assert_eq!(p.update(t0 + s(69), BusState::Silent, None), None);
        assert_eq!(
            p.update(t0 + s(70), BusState::Silent, None),
            Some(PowerEvent::ShutdownRequested)
        );
        assert!(p.requested());
        // Asked once only.
        assert_eq!(p.update(t0 + s(80), BusState::Silent, None), None);
        assert_eq!(p.next_deadline(), None);
    }

    #[test]
    fn wake_cancels() {
        let t0 = Instant::now();
        let mut p = ShutdownPolicy::new(Config::default());
        p.update(t0, BusState::Silent, None);
        // Bus wakes before the timer: count restarts.
        assert_eq!(p.update(t0 + s(30), BusState::Waking, None), None);
        p.update(t0 + s(31), BusState::Silent, None);
        assert_eq!(p.update(t0 + s(61), BusState::Silent, None), None);
        assert_eq!(
            p.update(t0 + s(91), BusState::Silent, Some(false)),
            Some(PowerEvent::ShutdownRequested)
        );
        // Bus wakes after the request.
        assert_eq!(
            p.update(t0 + s(92), BusState::Awake, Some(true)),
            Some(PowerEvent::ShutdownCancelled)
        );
        assert!(!p.requested());
    }

    #[test]
    fn kl15_on_holds_off() {
        let t0 = Instant::now();
        let mut p = ShutdownPolicy::new(Config::default());
        for i in 0..200 {
            assert_eq!(p.update(t0 + s(i), BusState::Silent, Some(true)), None);
        }
        // SleepPending isn't silent.
        for i in 200..400 {
            assert_eq!(p.update(t0 + s(i), BusState::SleepPending, None), None);
        }
    }

    #[test]
    fn hat_mcu_is_a_stub() {
        let mut h = HatMcu;
        assert_eq!(
            h.request_poweroff().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(h.wake_reason(), WakeReason::Unknown);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn carpihat_unsupported_off_linux() {
        let mut h = CarPiHat::new();
        assert_eq!(
            h.request_poweroff().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn carpihat_runs_command_once() {
        let mut h = CarPiHat::with_command(["true"]);
        h.request_poweroff().unwrap();
        assert!(h.requested);
        h.request_poweroff().unwrap();
        let mut bad = CarPiHat::with_command(["false"]);
        assert!(bad.request_poweroff().is_err());
        assert!(!bad.requested);
    }
}
