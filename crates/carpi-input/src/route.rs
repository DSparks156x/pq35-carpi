//! Gesture + context to a destination. Stateless: focus lives in the store,
//! and toggling it is an action the daemon carries out.
//!
//! Order:
//! 1. Not on TV: drop.
//! 2. `dis_toggle` trigger: `Action(ToggleDisFocus)`.
//! 3. `system` table: `Action`.
//! 4. DIS focus and in the `dis` table: `Dis`.
//! 5. `livi` table: `Livi`, or drop if LIVI isn't connected.
//! 6. Drop.

use crate::{Gesture, GestureKind, Key, LiviKey};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Livi,
    Dis,
    /// One of our pages inside LIVI. Routes like `Livi`: LIVI does focus.
    Page,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Context {
    pub rnse_on_tv: bool,
    pub livi_connected: bool,
    pub focus: Focus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DisKey {
    Up,
    Down,
    Select,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemAction {
    ToggleDisFocus,
    Shutdown,
    Reboot,
    SaveCapture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Livi(LiviKey),
    Dis(DisKey),
    Action(SystemAction),
    Drop,
}

/// Key and gesture. Knob detents are `(Knob(±1), Detent)`.
pub type Trigger = (Key, GestureKind);

#[derive(Clone, Debug)]
pub struct RouteConfig {
    pub dis_toggle: Trigger,
    pub system: HashMap<Trigger, SystemAction>,
    pub dis: HashMap<Trigger, DisKey>,
    pub livi: HashMap<Trigger, LiviKey>,
}

impl Default for RouteConfig {
    fn default() -> Self {
        use GestureKind::*;
        use Key as K;
        use LiviKey as L;

        let system = HashMap::from([
            ((K::Prev, Extended), SystemAction::SaveCapture),
            ((K::Next, Extended), SystemAction::Shutdown),
            ((K::Enter, Extended), SystemAction::Reboot),
        ]);
        let dis = HashMap::from([
            ((K::WheelUp, Short), DisKey::Up),
            ((K::WheelDown, Short), DisKey::Down),
            ((K::WheelClick, Short), DisKey::Select),
            ((K::WheelClick, Double), DisKey::Back),
            ((K::WheelClick, Long), DisKey::Back),
        ]);
        let livi = HashMap::from([
            // Wheel.
            ((K::WheelUp, Short), L::Prev),
            ((K::WheelDown, Short), L::Next),
            ((K::WheelClick, Short), L::PlayPause),
            ((K::WheelClick, Long), L::Home),
            ((K::Mode, Short), L::Select),
            ((K::Mode, Double), L::Back),
            ((K::Ptt, Short), L::Voice),
            ((K::Ptt, Long), L::Home),
            // Faceplate.
            ((K::Prev, Short), L::Prev),
            ((K::Next, Short), L::Next),
            ((K::Up, Short), L::Up),
            ((K::Down, Short), L::Down),
            ((K::Down, Long), L::Voice),
            ((K::Enter, Short), L::Select),
            ((K::Return, Short), L::Back),
            ((K::Return, Long), L::Home),
            ((K::Setup, Short), L::Home),
            ((K::Tel, Short), L::AcceptPhone),
            ((K::Tel, Long), L::RejectPhone),
            ((K::Knob(1), Detent), L::Right),
            ((K::Knob(-1), Detent), L::Left),
        ]);
        RouteConfig {
            dis_toggle: (K::Mode, Long),
            system,
            dis,
            livi,
        }
    }
}

pub fn route(cfg: &RouteConfig, ctx: &Context, g: &Gesture) -> Route {
    if !ctx.rnse_on_tv {
        return Route::Drop;
    }
    let t = (g.key, g.kind);
    if t == cfg.dis_toggle {
        return Route::Action(SystemAction::ToggleDisFocus);
    }
    if let Some(&a) = cfg.system.get(&t) {
        return Route::Action(a);
    }
    if ctx.focus == Focus::Dis
        && let Some(&k) = cfg.dis.get(&t)
    {
        return Route::Dis(k);
    }
    match cfg.livi.get(&t) {
        Some(&k) if ctx.livi_connected => Route::Livi(k),
        _ => Route::Drop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use GestureKind::*;
    use std::time::Instant;

    fn r(ctx: Context, key: Key, kind: GestureKind) -> Route {
        let g = Gesture {
            key,
            kind,
            at: Instant::now(),
        };
        route(&RouteConfig::default(), &ctx, &g)
    }

    const TV: Context = Context {
        rnse_on_tv: true,
        livi_connected: true,
        focus: Focus::Livi,
    };
    const DIS: Context = Context {
        focus: Focus::Dis,
        ..TV
    };

    #[test]
    fn nothing_off_tv() {
        let off = Context {
            rnse_on_tv: false,
            ..TV
        };
        for (k, g) in [
            (Key::WheelUp, Short),
            (Key::Mode, Long),
            (Key::Next, Extended),
        ] {
            assert_eq!(r(off, k, g), Route::Drop);
        }
    }

    #[test]
    fn table() {
        use Route::*;
        let cases = [
            // ctx, key, gesture, expected
            (TV, Key::Mode, Long, Action(SystemAction::ToggleDisFocus)),
            (DIS, Key::Mode, Long, Action(SystemAction::ToggleDisFocus)),
            (TV, Key::WheelUp, Short, Livi(LiviKey::Prev)),
            (DIS, Key::WheelUp, Short, Dis(DisKey::Up)),
            (DIS, Key::WheelDown, Short, Dis(DisKey::Down)),
            (DIS, Key::WheelClick, Short, Dis(DisKey::Select)),
            (DIS, Key::WheelClick, Double, Dis(DisKey::Back)),
            // Not in the DIS table: falls through to LIVI.
            (DIS, Key::Ptt, Short, Livi(LiviKey::Voice)),
            (DIS, Key::Knob(1), Detent, Livi(LiviKey::Right)),
            (TV, Key::Knob(-1), Detent, Livi(LiviKey::Left)),
            (TV, Key::Next, Short, Livi(LiviKey::Next)),
            (TV, Key::Next, Long, Drop),
            (TV, Key::Next, Extended, Action(SystemAction::Shutdown)),
            (DIS, Key::Enter, Extended, Action(SystemAction::Reboot)),
            (TV, Key::Prev, Extended, Action(SystemAction::SaveCapture)),
            (TV, Key::VolUp, Short, Drop),
            (TV, Key::Nav, Short, Drop),
            (
                Context {
                    focus: Focus::Page,
                    ..TV
                },
                Key::WheelUp,
                Short,
                Livi(LiviKey::Prev),
            ),
        ];
        for (ctx, k, g, want) in cases {
            assert_eq!(r(ctx, k, g), want, "{k:?} {g:?} {:?}", ctx.focus);
        }
    }

    #[test]
    fn livi_down_drops_livi_keys_only() {
        let ctx = Context {
            livi_connected: false,
            ..DIS
        };
        assert_eq!(r(ctx, Key::WheelUp, Short), Route::Dis(DisKey::Up));
        assert_eq!(r(ctx, Key::Ptt, Short), Route::Drop);
        assert_eq!(
            r(ctx, Key::Next, Extended),
            Route::Action(SystemAction::Shutdown)
        );
    }

    #[test]
    fn toggle_is_configurable() {
        let cfg = RouteConfig {
            dis_toggle: (Key::Mode, Double),
            ..Default::default()
        };
        let g = |kind| Gesture {
            key: Key::Mode,
            kind,
            at: Instant::now(),
        };
        assert_eq!(
            route(&cfg, &TV, &g(Double)),
            Route::Action(SystemAction::ToggleDisFocus)
        );
        assert_eq!(route(&cfg, &TV, &g(Long)), Route::Drop);
    }
}
