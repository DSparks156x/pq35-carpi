//! Everything the car model decides to do: RNS-E commands, keys for LIVI,
//! power events. Sans-IO; [`crate::Car`] collects [`Output`]s and the daemon
//! acts on them.
//!
//! Every frame goes through the interface's [`Gate`] before it becomes an
//! `Output::Send`.

use carpi_can::Frame;
use carpi_gate::{BusView, Gate};
use carpi_input::{
    Context, Decoder, DecoderConfig, DisKey, Focus, GestureConfig, LiviKey, Recogniser, Route,
    RouteConfig, SystemAction,
};
use carpi_light::{LightModel, PanelLevel};
use carpi_nm::BusMonitor;
use carpi_power::{PowerEvent, ShutdownPolicy};
use carpi_rnse::{Brightness, SourceLabel};
use carpi_signals::ids;
use carpi_state::{Store, Value};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// A frame the gate passed, for interface `iface`.
    Send {
        iface: usize,
        frame: Frame,
    },
    /// A key for LIVI (uinput).
    Key(LiviKey),
    /// A key for the DIS controller (not built yet; logged).
    Dis(DisKey),
    /// Shutdown, reboot, save capture.
    Action(SystemAction),
    Power(PowerEvent),
}

/// Light values are recomputed every tick; let them lapse if ticks stop.
const DERIVED: Duration = Duration::from_secs(1);

pub(crate) struct Controls {
    pub gates: Vec<Gate>,
    rnse: carpi_rnse::Client,
    light: LightModel,
    power: ShutdownPolicy,
    decoder: Decoder,
    gestures: Recogniser,
    routes: RouteConfig,
    pub focus: Focus,
}

impl Controls {
    pub fn new(iface_names: &[String]) -> Self {
        Controls {
            gates: iface_names
                .iter()
                .map(|n| Gate::new(n.clone(), Default::default()))
                .collect(),
            rnse: carpi_rnse::Client::new(Default::default()),
            light: LightModel::new(Default::default()),
            power: ShutdownPolicy::new(Default::default()),
            decoder: Decoder::new(DecoderConfig::default()),
            gestures: Recogniser::new(GestureConfig::default()),
            routes: RouteConfig::default(),
            focus: Focus::Livi,
        }
    }

    /// A frame from the ICAN interface: keys go through the input pipeline.
    pub fn on_ican_frame(
        &mut self,
        at: Instant,
        id: u32,
        data: &[u8],
        store: &Store,
        livi_up: bool,
        out: &mut Vec<Output>,
    ) {
        if id != ids::MFL_CMD && id != ids::RNSE_KEYS {
            return;
        }
        let events = self.decoder.on_frame(at, id, data);
        for ev in events {
            let gestures = self.gestures.on_event(ev);
            self.route(gestures, store, at, livi_up, out);
        }
    }

    /// Every ~100 ms.
    pub fn tick(
        &mut self,
        now: Instant,
        ican: Option<(usize, &BusMonitor)>,
        store: &mut Store,
        livi_up: bool,
        out: &mut Vec<Output>,
    ) {
        // Input timeouts (lost releases, long-press thresholds).
        let events = self.decoder.tick(now);
        let mut gestures = Vec::new();
        for ev in events {
            gestures.extend(self.gestures.on_event(ev));
        }
        gestures.extend(self.gestures.tick(now));
        self.route(gestures, store, now, livi_up, out);

        // Light model into the store.
        let light = self
            .light
            .update(now, &carpi_light::Inputs::from_store(store, now));
        store.set("light.night", Some(Value::Bool(light.night)), now, DERIVED);
        store.set(
            "light.lights_on",
            light.lights_on.map(Value::Bool),
            now,
            DERIVED,
        );
        let panel = match light.panel_level {
            PanelLevel::FollowCar => Brightness::FollowCar,
            PanelLevel::Level(n) => Brightness::Level(n),
        };
        store.set(
            "light.panel_level",
            Some(Value::Num(match panel {
                Brightness::FollowCar => 0.0,
                Brightness::Level(n) => n as f64,
            })),
            now,
            DERIVED,
        );

        let Some((ican_idx, monitor)) = ican else {
            return;
        };
        let view = BusView::new(monitor, store, now);

        // RNS-E desired state: label from what LIVI is projecting.
        let label = match store.get("projection.kind", now) {
            Some(Value::Text(k)) if k == "android_auto" => SourceLabel::AndroidAuto,
            Some(Value::Text(k)) if k == "carplay" => SourceLabel::CarPlay,
            _ => SourceLabel::Livi,
        };
        self.rnse.set_label(Some(label));
        self.rnse.set_brightness(Some(panel));
        for cmd in self.rnse.update(now, view.rnse_awake) {
            match self.gates[ican_idx].check(cmd.class, &cmd.frame, now, &view) {
                Ok(()) => out.push(Output::Send {
                    iface: ican_idx,
                    frame: cmd.frame,
                }),
                Err(_) => self.rnse.refused(&cmd, now),
            }
        }
        for g in &mut self.gates {
            g.tick(now);
        }

        if let Some(ev) = self.power.update(now, monitor.state(), view.kl15) {
            out.push(Output::Power(ev));
        }
    }

    fn route(
        &mut self,
        gestures: Vec<carpi_input::Gesture>,
        store: &Store,
        now: Instant,
        livi_up: bool,
        out: &mut Vec<Output>,
    ) {
        for g in gestures {
            let ctx = Context {
                rnse_on_tv: store.get_bool("rnse.on_tv", now) == Some(true),
                livi_connected: livi_up,
                focus: self.focus,
            };
            match carpi_input::route(&self.routes, &ctx, &g) {
                Route::Livi(k) => out.push(Output::Key(k)),
                Route::Dis(k) => out.push(Output::Dis(k)),
                Route::Action(SystemAction::ToggleDisFocus) => {
                    self.focus = match self.focus {
                        Focus::Dis => Focus::Livi,
                        _ => Focus::Dis,
                    };
                }
                Route::Action(a) => out.push(Output::Action(a)),
                Route::Drop => {}
            }
        }
    }
}
