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
    /// A projection-neutral action for LIVI over the bridge.
    Car(CarAction),
    /// A key for the DIS. The car model already acted on it; this is for logs.
    Dis(DisKey),
    /// Shutdown, reboot, save capture.
    Action(SystemAction),
    Power(PowerEvent),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CurveMode {
    Manual,
    #[default]
    Learned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AmbientSource {
    /// 0x621 `KO1_Lichtsensor` (else 0x635 `DI1_Sensor`). Drew's default.
    #[default]
    Cluster,
    /// 0x3E3 sun load.
    Climatronic,
    /// Cluster, sun load filling in the bright end.
    Blend,
}

/// Light settings (PLAN 2.6). Only `auto_brightness` acts yet.
///
/// TODO(light model): ambient fusion (sun sensor by day, cluster sensor in
/// the dark), the curve, scroll trim, learning and the ambient night
/// threshold replace carpi-light's lights-on rule and fixed panel levels.
/// Until then the rest is stored and shown, and night still follows the
/// car's lights.
#[derive(Clone, Debug, PartialEq)]
pub struct LightSetup {
    /// Off: `BB 00` once, then the RNS-E is left alone.
    pub auto_brightness: bool,
    pub scroll_trim: bool,
    pub ambient_source: AmbientSource,
    pub curve_mode: CurveMode,
    /// (ambient, panel %), ascending ambient.
    pub manual_curve: Vec<(f64, u8)>,
    /// Learned points, keyed by the source they were learned on.
    pub learned: Vec<(AmbientSource, f64, u8)>,
    pub night_threshold: f64,
    pub night_hysteresis: f64,
}

impl Default for LightSetup {
    /// Drew's defaults (PLAN 2.6). Curve and threshold are guesses until a
    /// daylight-to-dark capture seeds them (PLAN 2.6 open item).
    fn default() -> Self {
        LightSetup {
            auto_brightness: true,
            scroll_trim: true,
            ambient_source: AmbientSource::Cluster,
            curve_mode: CurveMode::Learned,
            manual_curve: vec![(0.0, 5), (1.0, 30), (2.0, 70), (3.0, 100)],
            learned: Vec::new(),
            night_threshold: 1.0,
            night_hysteresis: 0.2,
        }
    }
}

/// Interim `light.ambient`, log10 of the raw source (floor 1): cluster
/// sensor counts or mean sun load in W/m2. Blend takes the cluster and
/// falls back to the sun load.
/// TODO(light model): scaled, smoothed, deadbanded; Blend filling in only
/// where the cluster sensor saturates (scale and saturation not known yet).
pub fn interim_ambient(src: AmbientSource, store: &Store, now: Instant) -> Option<f64> {
    let cluster = || {
        store
            .get_num("light.cluster_sensor", now)
            .or_else(|| store.get_num("light.dim_sensor", now))
    };
    let sun = || {
        let l = store.get_num("light.sun_left_wm2", now);
        let r = store.get_num("light.sun_right_wm2", now);
        match (l, r) {
            (Some(a), Some(b)) => Some((a + b) / 2.0),
            (a, b) => a.or(b),
        }
    };
    let raw = match src {
        AmbientSource::Cluster => cluster(),
        AmbientSource::Climatronic => sun(),
        AmbientSource::Blend => cluster().or_else(sun),
    }?;
    Some(raw.max(1.0).log10())
}

/// Light values are recomputed every tick; let them lapse if ticks stop.
const DERIVED: Duration = Duration::from_secs(1);

pub(crate) struct Controls {
    pub gates: Vec<Gate>,
    rnse: carpi_rnse::Client,
    light: LightModel,
    light_setup: LightSetup,
    /// Auto off: `BB 00` went out; send nothing more until auto comes on.
    handed_back: bool,
    power: ShutdownPolicy,
    decoder: Decoder,
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
            light_setup: LightSetup::default(),
            handed_back: false,
            power: ShutdownPolicy::new(Default::default()),
            decoder: Decoder::new(DecoderConfig::default()),
            gestures: Recogniser::new(GestureConfig::default()),
            routes: RouteConfig::default(),
            focus: Focus::Livi,
        }
    }

    pub fn set_light(&mut self, s: LightSetup) {
        if s.auto_brightness != self.light_setup.auto_brightness {
            self.handed_back = false;
        }
        self.light_setup = s;
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
        // For the light page's plot.
        let auto = self.light_setup.auto_brightness;
        store.set(
            "light.ambient",
            interim_ambient(self.light_setup.ambient_source, store, now).map(Value::Num),
            now,
            DERIVED,
        );
        let pct = match panel {
            Brightness::Level(n) if auto => Some(n as f64),
            // The RNS-E follows the car's display dimming.
            _ => store.get_num("light.dim_display_pct", now),
        };
        store.set("light.panel_pct", pct.map(Value::Num), now, DERIVED);

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
        // TODO(light model): with auto on, the curve's level instead.
        self.rnse.set_brightness(match (auto, self.handed_back) {
            (true, _) => Some(panel),
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
            match carpi_input::route(&self.routes, &ctx, &g) {
                Route::Livi(k) => out.push(Output::Key(k)),
                Route::Car(a) => out.push(Output::Car(a)),
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
