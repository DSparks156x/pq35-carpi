//! Everything the car model decides to do: RNS-E commands, keys for LIVI,
//! power events. Sans-IO; [`crate::Car`] collects [`Output`]s and the daemon
//! acts on them.
//!
//! Every frame goes through the interface's [`Gate`] before it becomes an
//! `Output::Send`.

use carpi_can::Frame;
use carpi_gate::{BusView, Gate};
use carpi_input::{
    Call, CarAction, Context, Decoder, DecoderConfig, DisKey, Focus, GestureConfig, LiviKey,
    Recogniser, Route, RouteConfig, SystemAction,
};
use carpi_light::{KEY_STEP, LightModel};
use carpi_nm::BusMonitor;
use carpi_power::{PowerEvent, ShutdownPolicy};
use carpi_rnse::{Brightness, SourceLabel};
use carpi_signals::ids;
use carpi_state::{Store, Value};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    /// A frame the gate passed, for interface `iface`.
    Send {
        iface: usize,
        frame: Frame,
    },
    /// A key for LIVI (uinput).
    Key(LiviKey),
    /// A projection-neutral action for LIVI over the bridge.
    Car(CarAction),
    /// A key for the DIS. The car model already acted on it; this is for logs.
    Dis(DisKey),
    /// Shutdown, reboot, save capture.
    Action(SystemAction),
    Power(PowerEvent),
    /// The light model's learned points changed (a nudge or a reset): every
    /// source's, for the daemon to persist.
    LightLearned(carpi_light::Learned),
}

/// Light settings (PLAN 2.6); see `carpi_light`.
pub use carpi_light::Setup as LightSetup;
pub use carpi_light::{AmbientSource, CurveMode, DarkSetup, DarkSource, Location};

/// Light values are recomputed every tick; let them lapse if ticks stop.
const DERIVED: Duration = Duration::from_secs(1);

pub(crate) struct Controls {
    pub gates: Vec<Gate>,
    rnse: carpi_rnse::Client,
    light: LightModel,
    /// Auto off: `BB 00` went out; send nothing more until auto comes on.
    handed_back: bool,
    power: ShutdownPolicy,
    decoder: Decoder,
    /// Unmapped key frames already logged.
    unknown_keys: std::collections::HashSet<(u32, Vec<u8>)>,
    gestures: Recogniser,
    pub routes: RouteConfig,
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
            handed_back: false,
            power: ShutdownPolicy::new(Default::default()),
            decoder: Decoder::new(DecoderConfig::default()),
            unknown_keys: Default::default(),
            gestures: Recogniser::new(GestureConfig::default()),
            routes: RouteConfig::default(),
            focus: Focus::Livi,
        }
    }

    pub fn set_light(&mut self, s: LightSetup) {
        if s.auto_brightness != self.light.setup().auto_brightness {
            self.handed_back = false;
        }
        self.light.set_setup(s);
    }

    /// A `Request::Light`. Learned changes go out as `Output::LightLearned`.
    pub fn light_action(
        &mut self,
        action: carpi_proto::LightAction,
        out: &mut Vec<Output>,
    ) -> Result<(), String> {
        match action {
            carpi_proto::LightAction::Nudge { delta_pct } => {
                self.light.nudge(delta_pct.into())?;
            }
            carpi_proto::LightAction::ResetLearned { source } => {
                self.light.reset_learned(source.map(|s| match s {
                    carpi_proto::AmbientSource::Cluster => AmbientSource::Cluster,
                    carpi_proto::AmbientSource::Climatronic => AmbientSource::Climatronic,
                    carpi_proto::AmbientSource::Blend => AmbientSource::Blend,
                }));
            }
        }
        out.push(Output::LightLearned(self.light.learned().clone()));
        Ok(())
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
        if !carpi_input::known_key_frame(id, data)
            && self.unknown_keys.len() < 256
            && self.unknown_keys.insert((id, data.to_vec()))
        {
            tracing::info!("key: unmapped {id:03X} {}", hex(data));
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
        let auto = self.light.setup().auto_brightness;
        let num = |v: Option<f64>| v.map(Value::Num);
        store.set("light.night", Some(Value::Bool(light.night)), now, DERIVED);
        store.set(
            "light.night_source",
            light.night_source.map(|s| Value::Text(s.as_str().into())),
            now,
            DERIVED,
        );
        store.set(
            "light.lights_on",
            light.lights_on.map(Value::Bool),
            now,
            DERIVED,
        );
        store.set("light.ambient", num(light.ambient), now, DERIVED);
        store.set("light.target_pct", num(light.target_pct), now, DERIVED);
        store.set(
            "light.trim_pct",
            Some(Value::Num(light.trim_pct)),
            now,
            DERIVED,
        );
        store.set(
            "light.curve",
            Some(Value::Text(self.light.curve_json().into())),
            now,
            DERIVED,
        );
        // What the panel shows: our level, else the car's display dimming
        // (the RNS-E follows it).
        let ours = light.panel_pct.filter(|_| auto);
        let pct = ours
            .map(f64::from)
            .or_else(|| store.get_num("light.dim_display_pct", now));
        store.set("light.panel_pct", num(pct), now, DERIVED);
        // `BB` argument: 0 = follows the car.
        store.set(
            "light.panel_level",
            Some(Value::Num(ours.map_or(0.0, f64::from))),
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
        // Auto on: the curve's level once there is one (until then the
        // RNS-E is left as it is).
        self.rnse.set_brightness(match (auto, self.handed_back) {
            (true, _) => light.panel_pct.map(Brightness::Level),
            (false, false) => Some(Brightness::FollowCar),
            (false, true) => None,
        });
        for cmd in self.rnse.update(now, view.rnse_awake) {
            match self.gates[ican_idx].check(cmd.class, &cmd.frame, now, &view) {
                Ok(()) => {
                    if !auto && cmd.frame.data()[0] == 0xBB {
                        self.handed_back = true;
                    }
                    out.push(Output::Send {
                        iface: ican_idx,
                        frame: cmd.frame,
                    })
                }
                Err(_) => self.rnse.refused(&cmd, now),
            }
        }
        for g in &mut self.gates {
            g.tick(now);
        }

        if let Some(on) = store.get_bool("rnse.radio_on", now) {
            self.power.set_radio_on(on);
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
                call: match store.get("call.state", now) {
                    Some(Value::Text(s)) if s == "ringing" => Call::Ringing,
                    Some(Value::Text(_)) => Call::Active,
                    _ => Call::None,
                },
            };
            let r = carpi_input::route(&self.routes, &ctx, &g);
            // Every key the car sends and where it went, so a drive log says
            // what each press did. Knob detents only at debug.
            if matches!(g.key, carpi_input::Key::Knob(_)) {
                tracing::debug!("key: {:?} {:?} -> {r:?}", g.key, g.kind);
            } else {
                tracing::info!(
                    "key: {:?} {:?} -> {r:?} (tv {}, focus {:?}, livi {})",
                    g.key,
                    g.kind,
                    ctx.rnse_on_tv,
                    ctx.focus,
                    ctx.livi_connected
                );
            }
            match r {
                Route::Livi(k) => out.push(Output::Key(k)),
                Route::Car(a) => out.push(Output::Car(a)),
                Route::Dis(k) => out.push(Output::Dis(k)),
                Route::Action(SystemAction::ToggleDisFocus) => {
                    self.focus = match self.focus {
                        Focus::Dis => Focus::Livi,
                        _ => Focus::Dis,
                    };
                }
                Route::Action(a @ (SystemAction::BrightnessUp | SystemAction::BrightnessDown)) => {
                    let step = if a == SystemAction::BrightnessUp {
                        KEY_STEP
                    } else {
                        -KEY_STEP
                    };
                    match self.light.nudge(step) {
                        Ok(_) => out.push(Output::LightLearned(self.light.learned().clone())),
                        Err(e) => tracing::debug!("{a:?}: {e}"),
                    }
                }
                Route::Action(a) => out.push(Output::Action(a)),
                Route::Drop => {}
            }
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
