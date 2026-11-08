//! Acting on what the car model decided: send frames, press keys, power off.
//!
//! Sending and powering off are off unless enabled in the config; when off,
//! they're logged so a first run on the car shows what would have happened.

use carpi_can::{Frame, TxPort};
use carpi_core::{Car, Output};
use carpi_input::SystemAction;
use carpi_power::{CarPiHat, PowerBackend, PowerEvent};
use carpi_proto::{Command, InputAction};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub struct Actuators {
    /// One sender per interface; `None` when TX is off or in replay.
    tx: Vec<Option<mpsc::Sender<Frame>>>,
    iface_names: Vec<String>,
    replay: bool,
    shutdown_enabled: bool,
    power: CarPiHat,
    #[cfg(target_os = "linux")]
    keyboard: Option<carpi_input::LiviKeyboard>,
    /// Bridge commands to LIVI, for key actions while AA/CarPlay has focus.
    hub: Option<Arc<crate::commands::CommandHub>>,
    /// The capture key gesture.
    captures: Option<crate::capture::Captures>,
    /// The logging key, when bound.
    datalog: Option<crate::datalog::Datalog>,
}

impl Actuators {
    pub fn new(cfg: &crate::config::Config, iface_names: Vec<String>, replay: bool) -> Self {
        let tx = iface_names
            .iter()
            .map(|name| (cfg.tx.enabled && !replay).then(|| open_tx(name)).flatten())
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
            replay,
            shutdown_enabled: cfg.power.shutdown,
            power: CarPiHat::new(),
            #[cfg(target_os = "linux")]
            keyboard,
            hub: None,
            captures: None,
            datalog: None,
        }
    }

    pub fn set_hub(&mut self, hub: Arc<crate::commands::CommandHub>) {
        self.hub = Some(hub);
    }

    pub fn set_captures(&mut self, captures: crate::capture::Captures) {
        self.captures = Some(captures);
    }

    pub fn set_datalog(&mut self, datalog: crate::datalog::Datalog) {
        self.datalog = Some(datalog);
    }

    /// Sends input actions to LIVI in order. False if no LIVI is attached.
    fn send_input(&self, actions: Vec<InputAction>) -> bool {
        let Some(hub) = self.hub.clone() else {
            return false;
        };
        tokio::spawn(async move {
            for action in actions {
                let label = format!("{action:?}");
                if let Err(e) = hub
                    .send(Command::Input { action }, Duration::from_secs(2))
                    .await
                {
                    debug!("input {label}: {e}");
                    break;
                }
            }
        });
        true
    }

    /// `[tx] enabled`, live. Never sends in replay.
    pub fn set_tx_enabled(&mut self, on: bool) {
        for (slot, name) in self.tx.iter_mut().zip(&self.iface_names) {
            match (on && !self.replay, slot.is_some()) {
                (true, false) => *slot = open_tx(name),
                (false, true) => *slot = None,
                _ => {}
            }
        }
        info!("TX {}", if on { "enabled" } else { "disabled" });
    }

    /// `[power] shutdown`, live.
    pub fn set_shutdown_enabled(&mut self, on: bool) {
        self.shutdown_enabled = on;
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
            Output::Key(k) => {
                // AA/CarPlay in front: semantic actions over the bridge.
                // LIVI's own menus and our pages: uinput (PLAN 2.4).
                let projection = matches!(
                    car.store().get("ui.focus", std::time::Instant::now()),
                    Some(carpi_state::Value::Text(f)) if f == "projection"
                );
                let sent = projection
                    .then(|| key_action(k))
                    .flatten()
                    .is_some_and(|actions| self.send_input(actions));
                if !sent {
                    self.key(k);
                }
            }
            Output::Car(a) => {
                if !self.send_input(car_action(a)) {
                    info!("car action {a:?}: LIVI isn't connected");
                }
            }
            Output::Dis(k) => debug!("DIS key {k:?}"),
            Output::Action(SystemAction::SaveCapture) => match &self.captures {
                Some(c) => {
                    let c = c.clone();
                    tokio::spawn(async move { c.capture("key").await });
                }
                None => info!("capture: {}", car.save_default_capture()),
            },
            Output::Action(SystemAction::ToggleLogging) => match &self.datalog {
                Some(d) => d.toggle(),
                None => info!("logging key: no logger"),
            },
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
            // The daemon persists these before outputs get here.
            Output::LightLearned(_) => {}
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

/// A LIVI key as actions for the projection. `None`: no projection
/// equivalent, uinput handles it.
fn key_action(k: carpi_input::LiviKey) -> Option<Vec<InputAction>> {
    use InputAction as A;
    use carpi_input::LiviKey as L;
    Some(match k {
        L::Up => vec![A::Up],
        L::Down => vec![A::Down],
        // Knob: AA/CarPlay rotary. Left/Right: the D-pad (Prev/Next long),
        // a real D-pad press as Hudiy sent it.
        L::KnobLeft => vec![A::Rotate { steps: -1 }],
        L::KnobRight => vec![A::Rotate { steps: 1 }],
        L::Left => vec![A::Left],
        L::Right => vec![A::Right],
        L::Select => vec![A::Select { down: true }, A::Select { down: false }],
        L::Back => vec![A::Back],
        L::Home => vec![A::Home],
        L::PlayPause => vec![A::PlayPause],
        L::Next => vec![A::Next],
        L::Prev => vec![A::Prev],
        L::AcceptPhone => vec![A::CallAccept],
        L::RejectPhone => vec![A::CallEnd],
        L::Voice => vec![A::Voice { down: true }, A::Voice { down: false }],
        _ => return None,
    })
}

fn car_action(a: carpi_input::CarAction) -> Vec<InputAction> {
    use InputAction as A;
    use carpi_input::{CarAction as C, Page};
    vec![match a {
        C::OpenNav => A::OpenNav,
        C::OpenPhone => A::OpenPhone,
        C::OpenMedia => A::OpenMedia,
        C::OpenPage(p) => A::OpenPage {
            page: match p {
                Page::Data => "data",
                Page::Car => "car",
            }
            .into(),
        },
        C::CallAccept => A::CallAccept,
        C::CallEnd => A::CallEnd,
    }]
}

/// A sending socket and its writer task. Dropping the sender ends the task.
fn open_tx(name: &str) -> Option<mpsc::Sender<Frame>> {
    match TxPort::open(name) {
        Ok(port) => {
            let (s, mut r) = mpsc::channel::<Frame>(256);
            let name = name.to_string();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knob_is_rotary_and_select_is_a_press() {
        use carpi_input::LiviKey as L;
        assert_eq!(
            key_action(L::KnobRight),
            Some(vec![InputAction::Rotate { steps: 1 }])
        );
        // The D-pad stays a D-pad in AA/CarPlay.
        assert_eq!(key_action(L::Left), Some(vec![InputAction::Left]));
        assert_eq!(
            key_action(L::Select).unwrap(),
            vec![
                InputAction::Select { down: true },
                InputAction::Select { down: false }
            ]
        );
        assert_eq!(key_action(L::CycleSession), None);
    }

    #[test]
    fn pages_map_to_names() {
        use carpi_input::{CarAction, Page};
        assert_eq!(
            car_action(CarAction::OpenPage(Page::Car)),
            vec![InputAction::OpenPage { page: "car".into() }]
        );
    }
}
