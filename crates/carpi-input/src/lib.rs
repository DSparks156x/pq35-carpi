//! Car keys in, routed actions out.
//!
//! ```text
//! frames ─▶ Decoder ─▶ KeyEvent ─▶ Recogniser ─▶ Gesture ─▶ route() ─▶ Route ─▶ sinks
//! ```
//!
//! Everything up to `route()` is sans-IO: feed it times, call `tick` at
//! `next_deadline`. The only I/O is `LiviKeyboard` (Linux uinput).

mod decode;
mod gesture;
mod key;
mod livi;
mod route;
#[cfg(target_os = "linux")]
mod sink;

pub use decode::{Decoder, DecoderConfig};
pub use gesture::{Gesture, GestureConfig, GestureKind, Recogniser, Timing};
pub use key::{Edge, Key, KeyEvent};
pub use livi::LiviKey;
pub use route::{
    Call, CarAction, Context, DisKey, Focus, Page, Route, RouteConfig, SystemAction, Trigger, route,
};
#[cfg(target_os = "linux")]
pub use sink::LiviKeyboard;

#[cfg(test)]
mod tests {
    use super::*;
    use carpi_signals::ids;
    use std::time::{Duration, Instant};

    /// Frames through all three stages, with ticks at each deadline.
    fn pipeline(ctx: Context, frames: &[(u64, u32, &[u8])], end: u64) -> Vec<Route> {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut d = Decoder::new(DecoderConfig::default());
        let mut g = Recogniser::new(GestureConfig::default());
        let cfg = RouteConfig::default();
        let mut gestures = Vec::new();
        let feed = |evs: Vec<KeyEvent>, g: &mut Recogniser, gestures: &mut Vec<Gesture>| {
            for e in evs {
                gestures.extend(g.on_event(e));
            }
        };
        for &(t, id, data) in frames {
            feed(d.on_frame(ms(t), id, data), &mut g, &mut gestures);
        }
        feed(d.tick(ms(end)), &mut g, &mut gestures);
        gestures.extend(g.tick(ms(end)));
        gestures.iter().map(|x| route(&cfg, &ctx, x)).collect()
    }

    const TV: Context = Context {
        rnse_on_tv: true,
        livi_connected: true,
        focus: Focus::Livi,
        call: Call::None,
    };

    #[test]
    fn captured_mode_double_is_two_presses() {
        let w = ids::MFL_CMD;
        let got = pipeline(
            TV,
            &[
                (0, w, &[0x3A, 0x1C]),
                (160, w, &[0x3A, 0x00]),
                (240, w, &[0x39, 0x01]),
                (400, w, &[0x39, 0x00]),
            ],
            3000,
        );
        // MODE short is unbound (Drew, 2026-11-07); no key waits for doubles.
        assert_eq!(got, vec![Route::Drop, Route::Drop]);
    }

    #[test]
    fn next_held_to_extended_shuts_down_once() {
        let p = ids::RNSE_KEYS;
        let on: &[u8] = &[0x37, 0x30, 1, 0x02, 0x00, 0];
        let off: &[u8] = &[0x37, 0x30, 4, 0x02, 0x00, 0];
        // 55 frames = 6 s, past the 5.5 s extended threshold.
        let mut frames: Vec<(u64, u32, &[u8])> = (0..55).map(|i| (i * 110, p, on)).collect();
        for i in 0..5 {
            frames.push((55 * 110 + i * 110, p, off));
        }
        let got = pipeline(TV, &frames, 8000);
        assert_eq!(got, vec![Route::Action(SystemAction::Shutdown)]);

        // Released at ~3.9 s: a long press, which is D-pad right.
        let mut frames: Vec<(u64, u32, &[u8])> = (0..35).map(|i| (i * 110, p, on)).collect();
        for i in 0..5 {
            frames.push((35 * 110 + i * 110, p, off));
        }
        let got = pipeline(TV, &frames, 6000);
        assert_eq!(got, vec![Route::Livi(LiviKey::Right)]);
    }

    #[test]
    fn missing_wheel_release_still_ends_press() {
        let w = ids::MFL_CMD;
        // PTT tapped, release lost: the 1.5 s timeout ends it, but the 1 s long
        // threshold fires first (PTT long is unbound). Known cost of an
        // unknown repeat rate.
        let got = pipeline(TV, &[(0, w, &[0x39, 0x2A])], 5000);
        assert_eq!(got, vec![Route::Drop]);
    }
}
