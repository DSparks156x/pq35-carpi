//! Call, "name hero" (Drew picked it over a header band and two columns;
//! v2 with name + number + full-width buttons looked badly proportioned, and
//! v3 with everything pushed to the edges looked too spread out).
//!
//! One compact block, centred vertically (logical):
//! - status row y=9: signal bars x 3..10, status centred in x 12..52
//!   (`Incoming`, `Calling`, `On hold`, or the call timer), battery x 53..61;
//! - the name alone, centred at y=20, marquee when wider; the number shows
//!   there only when there's no name;
//! - action labels y=32, side by side around the centre, inverted only
//!   while the DIS has focus:
//!   ringing `Yes` (wheel click / TEL) and `No` (TEL held; `Answer`/`Decline`
//!   filled the whole row); in a call `End` (wheel click / TEL held). Keys
//!   from carpi-input `route.rs`.
//!
//! Bars, battery and labels are `52` fills/clears plus black text: no
//! bitmaps, no `63` latch. Each redraws in one group, only when it changes.
//! The timer is ours from when the call went active (the phone sends no
//! duration) and re-sends one run a second.

use super::Screen;
use crate::cmd::{Ink, Op, text_flags};
use crate::font::{Font, TEXT_H, audscii};
use crate::helpers::{Ctx, ScrollText, TextSlot};
use crate::pack::Group;
use crate::proto::Rect;
use crate::screens::media::clock;
use std::time::{Duration, Instant};

/// As carpi-core has it (`call.state`, `call.name`, `call.number`,
/// `phone.signal_pct`, `phone.battery_pct`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallInput {
    /// `ringing`, `dialing`, `active`, `held`; `None` = no call.
    pub state: Option<String>,
    pub name: Option<String>,
    pub number: Option<String>,
    pub signal_pct: Option<u8>,
    pub battery_pct: Option<u8>,
}

pub const STATUS_Y: u8 = 9;
pub const NAME_Y: u8 = 20;
pub const ACTION_Y: u8 = 32;
/// Gap between pills.
const PILL_GAP: u8 = 4;

/// Signal bars 0..=4 from a percentage.
pub fn bars(pct: Option<u8>) -> Option<u8> {
    pct.map(|p| (u16::from(p.min(100)) * 4).div_ceil(100) as u8)
}

/// Battery fill 0..=4 (inner width) from a percentage.
pub fn battery_fill(pct: Option<u8>) -> Option<u8> {
    pct.map(|p| (u16::from(p.min(100)) * 4 / 100) as u8)
}

pub struct CallScreen {
    input: CallInput,
    active_since: Option<Instant>,
    /// Whole seconds the timer last showed (deadlines without a clock read).
    shown_secs: u64,
    status: TextSlot,
    name: ScrollText,
    /// What's drawn: bars, battery, action labels.
    shown_bars: Option<Option<u8>>,
    shown_battery: Option<Option<u8>>,
    shown_actions: Option<(Vec<&'static str>, bool)>,
    focus: bool,
}

impl Default for CallScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl CallScreen {
    pub fn new() -> Self {
        let line = |y| {
            let mut s = ScrollText::new(y, 1, 63, Font::Prop);
            s.centred = true;
            s
        };
        CallScreen {
            input: CallInput::default(),
            active_since: None,
            shown_secs: 0,
            status: TextSlot::new(STATUS_Y, 12, 52, Font::Prop),
            name: line(NAME_Y),
            shown_bars: None,
            shown_battery: None,
            shown_actions: None,
            focus: false,
        }
    }

    /// DIS focus: the action labels invert.
    pub fn set_focus(&mut self, on: bool) {
        self.focus = on;
    }

    pub fn active(&self) -> bool {
        self.input.state.is_some()
    }

    pub fn set(&mut self, now: Instant, input: CallInput) {
        let was = self.input.state.as_deref() == Some("active");
        let is = input.state.as_deref() == Some("active");
        if is && !was {
            self.active_since = Some(now);
        } else if input.state.is_none() {
            self.active_since = None;
        }
        let who = input
            .name
            .as_deref()
            .or(input.number.as_deref())
            .unwrap_or("Unknown");
        self.name.set_text(now, who);
        self.input = input;
    }

    fn status_text(&self, now: Instant) -> String {
        match self.input.state.as_deref() {
            Some("ringing") => "Incoming".into(),
            Some("dialing") => "Calling".into(),
            Some("held") => "On hold".into(),
            Some("active") => {
                let t = self.active_since.map_or(Duration::ZERO, |s| now - s);
                clock(t.as_millis() as u64)
            }
            Some(other) => other.into(),
            None => String::new(),
        }
    }

    fn actions(&self) -> Vec<&'static str> {
        match self.input.state.as_deref() {
            Some("ringing") => vec!["Yes", "No"],
            Some("active" | "held" | "dialing") => vec!["End"],
            _ => Vec::new(),
        }
    }

    fn status_groups(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        if let Some(s) = self.active_since {
            self.shown_secs = (now - s).as_secs();
        }
        let m = &cx.metrics;
        let b = audscii(&self.status_text(now));
        let x = self.status.centre_x(m, &b);
        self.status.draw(m, "status", x, b).into_iter().collect()
    }

    /// Four 1-wide bars, heights 2/3/4/5 rows, bottoms on y=7; unlit bars are
    /// a 1-row stub. Nothing when the phone sends no signal.
    fn bars_group(&mut self) -> Option<Group> {
        let want = bars(self.input.signal_pct);
        if self.shown_bars == Some(want) {
            return None;
        }
        self.shown_bars = Some(want);
        let y = STATUS_Y + 1;
        let mut ops = vec![Op::Clear(Rect::new(3, y, 8, 6))];
        if let Some(n) = want {
            for i in 0..4u8 {
                let h = if i < n { 2 + i } else { 1 };
                ops.push(Op::Fill(Rect::new(3 + i * 2, y + 6 - h, 1, h)));
            }
        }
        Some(Group::new("signal", ops))
    }

    /// 6x4 outline, 1-px nub, inner fill 0..=4 wide.
    fn battery_group(&mut self) -> Option<Group> {
        let want = battery_fill(self.input.battery_pct);
        if self.shown_battery == Some(want) {
            return None;
        }
        self.shown_battery = Some(want);
        let y = STATUS_Y + 2;
        let mut ops = vec![Op::Clear(Rect::new(53, y, 9, 5))];
        if let Some(f) = want {
            ops.push(Op::Fill(Rect::new(54, y, 6, 4)));
            ops.push(Op::Clear(Rect::new(55, y + 1, 4, 2)));
            ops.push(Op::Fill(Rect::new(60, y + 1, 1, 2)));
            if f > 0 {
                ops.push(Op::Fill(Rect::new(55, y + 1, f, 2)));
            }
        }
        Some(Group::new("battery", ops))
    }

    /// Action labels side by side around the centre. Inverted (fill + black
    /// text) only while the DIS has focus: Drew's rule, highlight means focus.
    /// The row clear rides in the same group.
    fn actions_group(&mut self, cx: &Ctx) -> Option<Group> {
        let want = (self.actions(), self.focus);
        if self.shown_actions.as_ref() == Some(&want) {
            return None;
        }
        let focus = self.focus;
        let m = &cx.metrics;
        let mut ops = vec![Op::Clear(Rect::new(0, ACTION_Y - 1, 64, TEXT_H + 1))];
        let pills: Vec<(Vec<u8>, u8)> = want
            .0
            .iter()
            .map(|l| {
                let b = audscii(l);
                let w = (m.width(Font::Prop, &b).div_ceil(2) as u8) + 2;
                (b, w)
            })
            .collect();
        let total = pills.iter().map(|(_, w)| *w).sum::<u8>()
            + PILL_GAP * (pills.len().saturating_sub(1) as u8);
        let mut x = 32u8.saturating_sub(total / 2);
        for (b, w) in pills {
            let rect = Rect::new(x, ACTION_Y - 1, w, TEXT_H);
            x += w + PILL_GAP;
            if focus {
                ops.push(Op::Fill(rect));
            }
            ops.push(Op::Text {
                clip: rect,
                x: 1,
                y: 0,
                flags: text_flags(Font::Prop, if focus { Ink::Black } else { Ink::Opaque }),
                bytes: b,
            });
        }
        self.shown_actions = Some(want);
        Some(Group::new("actions", ops))
    }

    fn all(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        let mut out = self.status_groups(now, cx);
        out.extend(self.name.poll(now, cx, "name"));
        out.extend(self.actions_group(cx));
        out.extend(self.bars_group());
        out.extend(self.battery_group());
        out
    }
}

impl Screen for CallScreen {
    fn enter(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        self.status.reset();
        self.name.reset();
        self.shown_bars = None;
        self.shown_battery = None;
        self.shown_actions = None;
        self.all(now, cx)
    }

    fn update(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        self.all(now, cx)
    }

    fn next_deadline(&self, cx: &Ctx) -> Option<Instant> {
        let tick = self
            .active_since
            .map(|s| s + Duration::from_secs(self.shown_secs + 1));
        [tick, self.name.next_deadline(&cx.metrics)]
        .into_iter()
        .flatten()
        .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_words_timer_and_meters() {
        let t0 = Instant::now();
        let mut s = CallScreen::new();
        let call = |st: &str| CallInput {
            state: Some(st.into()),
            name: Some("Mum".into()),
            number: Some("07700 900123".into()),
            ..Default::default()
        };
        s.set(t0, call("ringing"));
        assert_eq!(s.status_text(t0), "Incoming");
        assert_eq!(s.actions(), vec!["Yes", "No"]);
        s.set(t0, call("active"));
        assert_eq!(s.status_text(t0 + Duration::from_secs(83)), "1:23");
        assert_eq!(s.actions(), vec!["End"]);
        s.set(t0, CallInput::default());
        assert!(!s.active());
        assert_eq!(bars(Some(0)), Some(0));
        assert_eq!(bars(Some(30)), Some(2));
        assert_eq!(bars(Some(100)), Some(4));
        assert_eq!(battery_fill(Some(60)), Some(2));
    }
}
