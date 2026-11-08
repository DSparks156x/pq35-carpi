//! Gesture + context to a destination. Stateless: focus and call state live
//! in the store, and toggling focus is an action the daemon carries out.
//!
//! Order:
//! 1. Not on TV: drop.
//! 2. `dis_toggle` trigger: `Action(ToggleDisFocus)`.
//! 3. `system` table: `Action`.
//! 4. A call ringing or active, and in the `call` table: `Car`.
//! 5. DIS focus and in the `dis` table: `Dis`.
//! 6. `car` table (projection-neutral actions for the bridge): `Car`.
//! 7. `livi` table (uinput keys): `Livi`.
//!
//! Steps 4, 6 and 7 drop when LIVI isn't connected.
//!
//! Drew's bindings (2026-11-07): wheel down = next and up = previous (going
//! down a list); click = play/pause; Prev/Next held = D-pad left/right;
//! INFO and CAR open our pages; extended presses are 5 s+. MODE short is
//! unbound for now.

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
pub enum Call {
    #[default]
    None,
    Ringing,
    /// Active, dialing or held.
    Active,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Context {
    pub rnse_on_tv: bool,
    pub livi_connected: bool,
    pub focus: Focus,
    pub call: Call,
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
    /// RNS-E panel brighter / darker, learned by the light model. Not bound
    /// by default.
    BrightnessUp,
    BrightnessDown,
}

/// Our pages inside the LIVI fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    /// Live data, dashboards, diagnostics.
    Data,
    /// Car settings and the manager.
    Car,
}

/// Projection-neutral actions, sent over the LIVI bridge. The fork maps each
/// to Android Auto keycodes or CarPlay commands for the active session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CarAction {
    OpenNav,
    OpenPhone,
    OpenMedia,
    OpenPage(Page),
    CallAccept,
    CallEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Livi(LiviKey),
    Car(CarAction),
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
    /// While ringing. Falls back to `call_active` entries.
    pub call_ringing: HashMap<Trigger, CarAction>,
    /// While a call is active (and while ringing, for keys not in
    /// `call_ringing`).
    pub call_active: HashMap<Trigger, CarAction>,
    pub dis: HashMap<Trigger, DisKey>,
    pub car: HashMap<Trigger, CarAction>,
    pub livi: HashMap<Trigger, LiviKey>,
}

impl Default for RouteConfig {
    fn default() -> Self {
        use CarAction as C;
        use GestureKind::*;
        use Key as K;
        use LiviKey as L;

        let system = HashMap::from([
            ((K::Prev, Extended), SystemAction::SaveCapture),
            ((K::Next, Extended), SystemAction::Shutdown),
            ((K::Enter, Extended), SystemAction::Reboot),
        ]);
        let call_ringing = HashMap::from([
            ((K::WheelClick, Short), C::CallAccept),
            ((K::Tel, Short), C::CallAccept),
        ]);
        let call_active = HashMap::from([
            ((K::WheelClick, Short), C::CallEnd),
            ((K::Tel, Long), C::CallEnd),
        ]);
        let dis = HashMap::from([
            ((K::WheelUp, Short), DisKey::Up),
            ((K::WheelDown, Short), DisKey::Down),
            ((K::WheelClick, Short), DisKey::Select),
            ((K::WheelClick, Long), DisKey::Back),
        ]);
        let car = HashMap::from([
            ((K::Nav, Short), C::OpenNav),
            ((K::Tel, Short), C::OpenPhone),
            ((K::Media, Short), C::OpenMedia),
            ((K::Info, Short), C::OpenPage(Page::Data)),
            ((K::Car, Short), C::OpenPage(Page::Car)),
        ]);
        let livi = HashMap::from([
            // Wheel.
            ((K::WheelUp, Short), L::Prev),
            ((K::WheelDown, Short), L::Next),
            ((K::WheelClick, Short), L::PlayPause),
            ((K::WheelClick, Long), L::Home),
            ((K::Ptt, Short), L::Voice),
            // Faceplate.
            ((K::Prev, Short), L::Prev),
            ((K::Next, Short), L::Next),
            ((K::Prev, Long), L::Left),
            ((K::Next, Long), L::Right),
            ((K::Up, Short), L::Up),
            ((K::Down, Short), L::Down),
            ((K::Enter, Short), L::Select),
            ((K::Return, Short), L::Back),
            ((K::Return, Long), L::Home),
            ((K::Setup, Short), L::Home),
            ((K::Name, Short), L::Home),
            ((K::Knob(1), Detent), L::KnobRight),
            ((K::Knob(-1), Detent), L::KnobLeft),
        ]);
        RouteConfig {
            dis_toggle: (K::Mode, Long),
            system,
            call_ringing,
            call_active,
            dis,
            car,
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
    if ctx.livi_connected {
        let call = match ctx.call {
            Call::Ringing => cfg.call_ringing.get(&t).or_else(|| cfg.call_active.get(&t)),
            Call::Active => cfg.call_active.get(&t),
            Call::None => None,
        };
        if let Some(&a) = call {
            return Route::Car(a);
        }
    }
    if ctx.focus == Focus::Dis
        && let Some(&k) = cfg.dis.get(&t)
    {
        return Route::Dis(k);
    }
    if !ctx.livi_connected {
        return Route::Drop;
    }
    if let Some(&a) = cfg.car.get(&t) {
        return Route::Car(a);
    }
    match cfg.livi.get(&t) {
        Some(&k) => Route::Livi(k),
        None => Route::Drop,
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
        call: Call::None,
    };
    const DIS: Context = Context {
        focus: Focus::Dis,
        ..TV
    };
    const RINGING: Context = Context {
        call: Call::Ringing,
        ..TV
    };
    const IN_CALL: Context = Context {
        call: Call::Active,
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
            (TV, Key::Mode, Short, Drop),
            // Down a list = next.
            (TV, Key::WheelUp, Short, Livi(LiviKey::Prev)),
            (TV, Key::WheelDown, Short, Livi(LiviKey::Next)),
            (TV, Key::WheelClick, Short, Livi(LiviKey::PlayPause)),
            (DIS, Key::WheelUp, Short, Dis(DisKey::Up)),
            (DIS, Key::WheelDown, Short, Dis(DisKey::Down)),
            (DIS, Key::WheelClick, Short, Dis(DisKey::Select)),
            (DIS, Key::WheelClick, Long, Dis(DisKey::Back)),
            // Not in the DIS table: falls through.
            (DIS, Key::Ptt, Short, Livi(LiviKey::Voice)),
            (DIS, Key::Knob(1), Detent, Livi(LiviKey::KnobRight)),
            (TV, Key::Knob(-1), Detent, Livi(LiviKey::KnobLeft)),
            (TV, Key::Next, Short, Livi(LiviKey::Next)),
            (TV, Key::Prev, Long, Livi(LiviKey::Left)),
            (TV, Key::Next, Long, Livi(LiviKey::Right)),
            (TV, Key::Next, Extended, Action(SystemAction::Shutdown)),
            (DIS, Key::Enter, Extended, Action(SystemAction::Reboot)),
            (TV, Key::Prev, Extended, Action(SystemAction::SaveCapture)),
            (TV, Key::VolUp, Short, Drop),
            (TV, Key::Nav, Short, Car(CarAction::OpenNav)),
            (TV, Key::Tel, Short, Car(CarAction::OpenPhone)),
            (TV, Key::Media, Short, Car(CarAction::OpenMedia)),
            (TV, Key::Info, Short, Car(CarAction::OpenPage(Page::Data))),
            (TV, Key::Car, Short, Car(CarAction::OpenPage(Page::Car))),
            (TV, Key::Name, Short, Livi(LiviKey::Home)),
            // Calls take the wheel click and TEL.
            (RINGING, Key::WheelClick, Short, Car(CarAction::CallAccept)),
            (RINGING, Key::Tel, Short, Car(CarAction::CallAccept)),
            (RINGING, Key::Tel, Long, Car(CarAction::CallEnd)),
            (IN_CALL, Key::WheelClick, Short, Car(CarAction::CallEnd)),
            (IN_CALL, Key::Tel, Long, Car(CarAction::CallEnd)),
            (IN_CALL, Key::Tel, Short, Car(CarAction::OpenPhone)),
            (IN_CALL, Key::WheelDown, Short, Livi(LiviKey::Next)),
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
            assert_eq!(
                r(ctx, k, g),
                want,
                "{k:?} {g:?} {:?} {:?}",
                ctx.focus,
                ctx.call
            );
        }
    }

    #[test]
    fn livi_down_drops_livi_keys_only() {
        let ctx = Context {
            livi_connected: false,
            call: Call::Ringing,
            ..DIS
        };
        assert_eq!(r(ctx, Key::WheelUp, Short), Route::Dis(DisKey::Up));
        assert_eq!(r(ctx, Key::Ptt, Short), Route::Drop);
        assert_eq!(r(ctx, Key::Nav, Short), Route::Drop);
        assert_eq!(r(ctx, Key::WheelClick, Short), Route::Dis(DisKey::Select));
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
