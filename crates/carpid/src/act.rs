//! Acting on what the car model decided: send frames, press keys, power off.
//!
//! Sending and powering off are off unless enabled in the config; when off,
//! they're logged so a first run on the car shows what would have happened.

use carpi_can::{Frame, TxPort};
use carpi_core::{Car, Output};
use carpi_input::SystemAction;
use carpi_power::{CarPiHat, PowerBackend, PowerEvent};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub struct Actuators {
    /// One sender per interface; `None` when TX is off or in replay.
    tx: Vec<Option<mpsc::Sender<Frame>>>,
    iface_names: Vec<String>,
    shutdown_enabled: bool,
    power: CarPiHat,
    #[cfg(target_os = "linux")]
    keyboard: Option<carpi_input::LiviKeyboard>,
}

impl Actuators {
    pub fn new(cfg: &crate::config::Config, iface_names: Vec<String>, replay: bool) -> Self {
        let tx = iface_names
            .iter()
            .map(|name| {
                if !cfg.tx.enabled || replay {
                    return None;
                }
                match TxPort::open(name) {
                    Ok(port) => {
                        let (s, mut r) = mpsc::channel::<Frame>(256);
                        let name = name.clone();
                        tokio::spawn(async move {
                            while let Some(f) = r.recv().await {
                                if let Err(e) = port.send(&f).await {
                                    warn!("{name}: send {f:?} failed: {e}");
                                }
                            }
                        });
                        Some(s)
                    }
                    Err(e) => {
                        warn!("{name}: can't open for sending: {e}");
                        None
                    }
                }
            })
            .collect();
        if !cfg.tx.enabled {
            info!("TX disabled ([tx] enabled = false): logging frames instead of sending");
        }

        #[cfg(target_os = "linux")]
        let keyboard = if cfg.input.uinput {
            match carpi_input::LiviKeyboard::open([]) {
                Ok(k) => Some(k),
                Err(e) => {
                    warn!("uinput keyboard unavailable ({e}); keys won't reach LIVI");
                    None
                }
            }
        } else {
            None
        };

        #[cfg(not(target_os = "linux"))]
        if cfg.input.uinput {
            info!("uinput needs Linux; keys are only logged here");
        }

        Actuators {
            tx,
            iface_names,
            shutdown_enabled: cfg.power.shutdown,
            power: CarPiHat::new(),
            #[cfg(target_os = "linux")]
            keyboard,
        }
    }

    pub fn act(&mut self, out: Output, car: &Car) {
        match out {
            Output::Send { iface, frame } => match self.tx.get(iface).and_then(Option::as_ref) {
                Some(s) => {
                    if s.try_send(frame).is_err() {
                        warn!(
                            "{}: TX queue full, dropped {frame:?}",
                            self.iface_names[iface]
                        );
                    }
                }
                None => info!("{}: would send {frame:?}", self.iface_names[iface]),
            },
            Output::Key(k) => self.key(k),
            Output::Dis(k) => debug!("DIS key {k:?} (no DIS controller yet)"),
            Output::Action(SystemAction::SaveCapture) => {
                let resp = car.save_default_capture();
                info!("capture: {resp}");
            }
            Output::Action(a @ (SystemAction::Shutdown | SystemAction::Reboot)) => {
                if !self.shutdown_enabled {
                    info!("would run {a:?} ([power] shutdown = false)");
                    return;
                }
                let cmd = if a == SystemAction::Reboot {
                    "reboot"
                } else {
                    "poweroff"
                };
                warn!("{a:?} requested from the faceplate");
                if let Err(e) = std::process::Command::new("systemctl").arg(cmd).status() {
                    warn!("systemctl {cmd}: {e}");
                }
            }
            Output::Action(a) => debug!("action {a:?}"),
            Output::Power(PowerEvent::ShutdownRequested) => {
                if self.shutdown_enabled {
                    if let Err(e) = self.power.request_poweroff() {
                        warn!("poweroff failed: {e}");
                    }
                } else {
                    info!("bus silent: would power off ([power] shutdown = false)");
                }
            }
            Output::Power(PowerEvent::ShutdownCancelled) => info!("bus woke; shutdown cancelled"),
        }
    }

    #[cfg(target_os = "linux")]
    fn key(&mut self, k: carpi_input::LiviKey) {
        match self.keyboard.as_mut() {
            Some(kb) => {
                if let Err(e) = kb.press(k) {
                    warn!("uinput {k:?}: {e}");
                }
            }
            None => debug!("key {k:?} (no uinput)"),
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn key(&mut self, k: carpi_input::LiviKey) {
        debug!("key {k:?} (no uinput off Linux)");
    }
}
