//! The driver: session + arbiter + screens + frame pacer. One frame in flight;
//! the next is built from current inputs only when the last one is done, so
//! the latest values always win and nothing queues.

use crate::arbiter::{Arbiter, ArbiterConfig, Class, Surface};
use crate::fb::Fb;
use crate::helpers::Ctx;
use crate::pack::PackError;
use crate::screens::nav::{NavConfig, NavInput, NavScreen};
use crate::screens::readings::{ReadingSpec, ReadingsInput, ReadingsScreen};
use crate::screens::{Screen, build_frame};
use crate::session::{Session, SessionConfig, SessionEvent};
use carpi_can::Frame;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenId {
    Nav,
    Readings,
}

#[derive(Clone, Debug, Default)]
pub struct DisConfig {
    pub session: SessionConfig,
    pub arbiter: ArbiterConfig,
    pub nav: NavConfig,
    pub readings: Vec<ReadingSpec>,
}

pub struct Dis {
    session: Session,
    pub arbiter: Arbiter,
    nav: NavScreen,
    readings: ReadingsScreen,
    user_page: Option<ScreenId>,
    /// Screen entered, and the generation it was entered in.
    shown: Option<(ScreenId, u32)>,
    belief: Option<Fb>,
    /// Last packing failure (a screen/helper bug); the frame was skipped.
    pub last_error: Option<PackError>,
}

impl Dis {
    pub fn new(cfg: DisConfig) -> Self {
        Dis {
            session: Session::new(cfg.session),
            arbiter: Arbiter::new(cfg.arbiter),
            nav: NavScreen::new(cfg.nav),
            readings: ReadingsScreen::new(cfg.readings),
            user_page: None,
            shown: None,
            belief: None,
            last_error: None,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn shown(&self) -> Option<ScreenId> {
        self.shown.map(|(s, _)| s)
    }
    /// What the driver believes is on the centre (all frames it handed over).
    pub fn belief(&self) -> Option<&Fb> {
        self.belief.as_ref()
    }
    pub fn nav(&self) -> &NavScreen {
        &self.nav
    }

    pub fn start(&mut self, now: Instant) {
        self.session.start(now);
    }

    pub fn stop(&mut self, now: Instant) {
        self.session.stop(now);
    }

    pub fn set_nav(&mut self, now: Instant, n: NavInput) {
        self.arbiter.nav(now, n.maneuver.as_deref(), n.distance_m);
        self.nav.set(now, n);
        self.step(now);
    }

    pub fn set_readings(&mut self, now: Instant, r: ReadingsInput) {
        self.readings.set(r);
        self.step(now);
    }

    /// The page the driver picked (stalk / DIS menu). `None` = nothing.
    pub fn set_user_page(&mut self, now: Instant, page: Option<ScreenId>) {
        if page.is_some() && page != self.user_page {
            self.arbiter.manual_select();
        }
        self.user_page = page;
        self.arbiter.set(Class::UserPage, page.is_some());
        self.step(now);
    }

    pub fn on_frame(&mut self, now: Instant, f: &Frame) {
        self.session.on_frame(now, f);
        self.step(now);
    }

    pub fn tick(&mut self, now: Instant) {
        self.session.tick(now);
        self.step(now);
    }

    pub fn poll_transmit(&mut self) -> Option<Frame> {
        self.session.poll_transmit()
    }

    pub fn poll_event(&mut self) -> Option<SessionEvent> {
        self.session.poll_event()
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let screen = match (self.session.profile(), self.shown) {
            (Some(p), Some((id, _))) if self.session.owned() => {
                let cx = Ctx::new(p);
                match id {
                    ScreenId::Nav => self.nav.next_deadline(&cx),
                    ScreenId::Readings => self.readings.next_deadline(&cx),
                }
            }
            _ => None,
        };
        [
            self.session.next_deadline(),
            self.arbiter.next_deadline(now),
            screen,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn screen_for(&self, c: Class) -> Option<ScreenId> {
        match c {
            Class::NavManeuver => Some(ScreenId::Nav),
            Class::UserPage => self.user_page,
            // No screens yet for alerts, calls, media or the stock radio.
            _ => None,
        }
    }

    fn step(&mut self, now: Instant) {
        let class = self.arbiter.winner(now, Surface::Centre);
        let want = self.screen_for(class);
        self.session.set_want(now, want.is_some());
        let (Some(id), Some(profile)) = (want, self.session.profile()) else {
            return;
        };
        if !self.session.owned() || !self.session.frame_idle() {
            return;
        }
        let cx = Ctx::new(profile);
        let generation = self.session.generation();
        let fresh = self.shown.is_none_or(|(_, g)| g != generation);
        let enter = self.shown != Some((id, generation));
        let screen: &mut dyn Screen = match id {
            ScreenId::Nav => &mut self.nav,
            ScreenId::Readings => &mut self.readings,
        };
        match build_frame(screen, now, &cx, self.session.registers(), enter) {
            Ok(packed) => {
                self.shown = Some((id, generation));
                if fresh || self.belief.is_none() {
                    // The grant's claim record cleared the centre.
                    self.belief = Some(Fb::new(cx.metrics));
                }
                if let Some(fb) = &mut self.belief {
                    packed.messages.iter().for_each(|m| fb.apply(&m.bytes));
                }
                let _ = self.session.submit(now, packed);
            }
            Err(e) => self.last_error = Some(e),
        }
    }
}
