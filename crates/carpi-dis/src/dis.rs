//! The driver: session + arbiter + screens + frame pacer. One frame in flight;
//! the next is built from current inputs only when the last one is done, so
//! the latest values always win and nothing queues.

use crate::arbiter::{Arbiter, ArbiterConfig, Class, Surface};
use crate::fb::Fb;
use crate::helpers::Ctx;
use crate::pack::{PackError, Packed, Registers};
use crate::screens::accel::{AccelConfig, AccelScreen};
use crate::screens::call::{CallInput, CallScreen};
use crate::screens::cover::CoverScreen;
use crate::screens::egg::{Egg, EggScreen};
use crate::screens::media::{MediaInput, MediaLayout, MediaScreen};
use crate::screens::nav::{NavConfig, NavInput, NavScreen};
use crate::screens::readings::{ReadingSpec, ReadingsInput, ReadingsPage, ReadingsScreen};
use crate::screens::{Screen, build_frame};
use crate::session::{Session, SessionConfig, SessionEvent};
use carpi_can::Frame;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenId {
    Nav,
    Readings,
    Media,
    Call,
    /// Cover art over the whole centre (flash on a new cover, or a page).
    Cover,
    /// Acceleration test (a user page).
    Accel,
    /// An easter egg animation, over media.
    Egg,
}

#[derive(Clone, Debug)]
pub struct DisConfig {
    pub session: SessionConfig,
    pub arbiter: ArbiterConfig,
    pub nav: NavConfig,
    /// One unnamed page from bare specs, used when `readings_pages` is empty.
    pub readings: Vec<ReadingSpec>,
    /// Subpages the wheel steps through.
    pub readings_pages: Vec<ReadingsPage>,
    pub media_layout: MediaLayout,
    /// When the cover changes while media has the centre, show the full cover
    /// (native refine included) and hold it this long once it's fully drawn
    /// (the old page's option; Drew: 4 s from the start was too short).
    /// `None` = off.
    pub cover_flash: Option<Duration>,
    pub accel: AccelConfig,
    /// `true`: our default page holds the centre whenever nothing else wants
    /// it. `false`: we idle as a tab in the cluster's menu until the driver
    /// picks it there (or with the wheel keys).
    pub claim_always: bool,
    /// The page "always" shows, and the cluster menu tab opens to.
    pub default_page: ScreenId,
    /// Per source (Drew): take the centre when it gets content, and give it
    /// back when the content goes. Claim without release keeps that screen
    /// up after its content ends, until a page is picked or something else
    /// claims.
    pub claim_nav: bool,
    pub release_nav: bool,
    pub claim_call: bool,
    pub release_call: bool,
    pub claim_media: bool,
    pub release_media: bool,
}

/// A flash never holds the centre longer than this, drawn or not.
const FLASH_CAP: Duration = Duration::from_secs(20);

/// A running cover flash: the hold starts once the cover is fully drawn.
#[derive(Clone, Copy, Debug)]
struct Flash {
    cap_at: Instant,
    hold_until: Option<Instant>,
}

impl Default for DisConfig {
    fn default() -> Self {
        DisConfig {
            session: SessionConfig::default(),
            arbiter: ArbiterConfig::default(),
            nav: NavConfig::default(),
            readings: Vec::new(),
            readings_pages: Vec::new(),
            media_layout: MediaLayout::default(),
            cover_flash: Some(Duration::from_secs(4)),
            accel: AccelConfig::default(),
            claim_always: true,
            default_page: ScreenId::Readings,
            claim_nav: true,
            release_nav: true,
            claim_call: true,
            release_call: true,
            claim_media: true,
            release_media: true,
        }
    }
}

pub struct Dis {
    session: Session,
    pub arbiter: Arbiter,
    nav: NavScreen,
    readings: ReadingsScreen,
    media: MediaScreen,
    call: CallScreen,
    cover: CoverScreen,
    accel: AccelScreen,
    egg: Option<EggScreen>,
    cover_flash: Option<Duration>,
    flash: Option<Flash>,
    user_page: Option<ScreenId>,
    /// Screen entered, and the generation it was entered in.
    shown: Option<(ScreenId, u32)>,
    /// Re-enter the shown screen on the next frame (subpage change).
    reenter: bool,
    /// What the arbiter last picked for the centre.
    want: Option<ScreenId>,
    claim_always: bool,
    default_page: ScreenId,
    /// (claim, release) for nav, call, media.
    claims: [(bool, bool); 3],
    /// Content per source as last fed: nav, call, media.
    content: [bool; 3],
    /// A source ended without release: its screen stays up.
    sticky: Option<ScreenId>,
    /// The cluster menu picked our tab; applied on the next tick.
    tab_picked: bool,
    /// Bench probes: hold the centre, draw nothing; the probe sends raw frames.
    probe: bool,
    belief: Option<Fb>,
    /// Last packing failure (a screen/helper bug); the frame was skipped.
    pub last_error: Option<PackError>,
}

impl Dis {
    pub fn new(cfg: DisConfig) -> Self {
        Dis {
            session: Session::new(cfg.session),
            arbiter: Arbiter::new(ArbiterConfig {
                nav_whole_route: cfg.arbiter.nav_whole_route || cfg.claim_nav,
                ..cfg.arbiter
            }),
            nav: NavScreen::new(cfg.nav),
            readings: if cfg.readings_pages.is_empty() {
                ReadingsScreen::new(cfg.readings)
            } else {
                ReadingsScreen::with_pages(cfg.readings_pages)
            },
            media: MediaScreen::new(cfg.media_layout),
            call: CallScreen::new(),
            cover: CoverScreen::new(),
            accel: AccelScreen::new(cfg.accel),
            egg: None,
            cover_flash: cfg.cover_flash,
            flash: None,
            user_page: None,
            shown: None,
            reenter: false,
            want: None,
            claim_always: cfg.claim_always,
            default_page: cfg.default_page,
            claims: [
                (cfg.claim_nav, cfg.release_nav),
                (cfg.claim_call, cfg.release_call),
                (cfg.claim_media, cfg.release_media),
            ],
            content: [false; 3],
            sticky: None,
            tab_picked: false,
            probe: false,
            belief: None,
            last_error: None,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
    /// The screen the arbiter wants on the centre right now (`None`: nothing
    /// to show, the centre is left to the cluster).
    pub fn wanted(&self) -> Option<ScreenId> {
        self.want
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
    pub fn readings(&self) -> &ReadingsScreen {
        &self.readings
    }

    /// Wheel up/down on the readings page: step subpages (wraps). The page
    /// re-enters with a clear; a blank on page switch is fine (Drew).
    pub fn step_readings(&mut self, now: Instant, delta: i32) {
        if self.readings.step(delta) && self.shown.is_some_and(|(id, _)| id == ScreenId::Readings) {
            self.reenter = true;
        }
        self.step(now);
    }

    /// Bench probes only: keep the centre claimed and stop drawing screens.
    pub fn set_probe(&mut self, now: Instant, on: bool) {
        self.probe = on;
        self.shown = None;
        self.step(now);
    }

    /// Bench probes only: send `messages` as one frame, as given (no packer,
    /// no commit added). Registers are forgotten. `false` if not owned or a
    /// frame is in flight.
    pub fn submit_raw(&mut self, now: Instant, messages: Vec<crate::pack::Message>) -> bool {
        self.submit_packed(
            now,
            Packed {
                messages,
                regs: Registers::default(),
                spans: Vec::new(),
            },
        )
    }

    /// Bench only: send a frame the caller packed (a screen not wired into
    /// the arbiter yet). Same rules as [`Dis::submit_raw`].
    pub fn submit_packed(&mut self, now: Instant, p: Packed) -> bool {
        self.probe && self.session.frame_idle() && self.session.submit(now, p).is_ok()
    }

    /// DIS focus (the wheel drives the DIS): the readings page name and the
/// call actions invert. Nothing else is ever inverted.
    pub fn set_focus(&mut self, now: Instant, on: bool) {
        self.readings.set_focus(on);
        self.call.set_focus(on);
        self.step(now);
    }

    pub fn start(&mut self, now: Instant) {
        self.session.start(now);
    }

    pub fn stop(&mut self, now: Instant) {
        self.session.stop(now);
    }

    pub fn set_nav(&mut self, now: Instant, n: NavInput) {
        if self.claims[0].0 {
            self.arbiter.nav(now, n.maneuver.as_deref(), n.distance_m);
        }
        self.content_changed(0, n.maneuver.is_some(), ScreenId::Nav);
        self.nav.set(now, n);
        self.step(now);
    }

    /// Media state. Something with a title or a duration makes media a
    /// candidate for the centre (below nav, calls and user pages).
    pub fn set_media(&mut self, now: Instant, m: MediaInput) {
        let on = m.title.is_some() || m.duration_ms.is_some_and(|d| d > 0);
        self.arbiter.set(Class::Media, on && self.claims[2].0);
        self.content_changed(2, on, ScreenId::Media);
        let new_cover = m.cover.as_ref().map(|(id, _)| *id);
        if new_cover.is_some() && new_cover != self.cover.id() {
            // Flash only if media has the centre right now (not on a cold start
            // under another screen, which would steal the centre later).
            if let (Some(_), Some(ScreenId::Media)) = (self.cover_flash, self.shown()) {
                self.flash = Some(Flash {
                    cap_at: now + FLASH_CAP,
                    hold_until: None,
                });
            }
        }
        self.cover.set(m.cover.clone());
        self.media.set(now, m);
        self.step(now);
    }

    /// Vehicle speed in km/h, every sample (the accel test times from these;
    /// feed it at the bus rate, not throttled). Converted to mph if the accel
    /// test is imperial.
    pub fn set_speed(&mut self, now: Instant, kmh: f64) {
        let v = if self.accel.timer.cfg.imperial { kmh / 1.609_344 } else { kmh };
        self.accel.sample(now, v);
        if self.shown.is_some_and(|(id, _)| id == ScreenId::Accel) {
            self.step(now);
        }
    }

    /// Play an easter egg over media, `loops` times, then hand back. The
    /// caller decides which track gets which egg. Replaces one that's playing.
    pub fn play_egg(&mut self, now: Instant, egg: Egg, loops: u32) {
        self.egg = Some(EggScreen::new(egg, loops));
        if self.shown.is_some_and(|(id, _)| id == ScreenId::Egg) {
            self.reenter = true;
        }
        self.step(now);
    }

    pub fn stop_egg(&mut self, now: Instant) {
        self.egg = None;
        self.step(now);
    }

    pub fn egg_playing(&self) -> bool {
        self.egg.is_some()
    }

    pub fn accel(&self) -> &AccelScreen {
        &self.accel
    }

    /// Call state. Any call outranks media and user pages (not nav maneuvers).
    pub fn set_call(&mut self, now: Instant, c: CallInput) {
        self.arbiter.set(Class::Call, c.state.is_some() && self.claims[1].0);
        self.content_changed(1, c.state.is_some(), ScreenId::Call);
        self.call.set(now, c);
        self.step(now);
    }

    pub fn set_readings(&mut self, now: Instant, r: ReadingsInput) {
        self.readings.set(r);
        self.step(now);
    }

    /// Source `i` (nav, call, media) gained or lost content. Losing it while
    /// its screen is up, claimed and not set to release, keeps the screen.
    fn content_changed(&mut self, i: usize, has: bool, screen: ScreenId) {
        let was = std::mem::replace(&mut self.content[i], has);
        let (claim, release) = self.claims[i];
        if has && !was && claim {
            // Something new claims: an old held screen gives way.
            self.sticky = None;
        }
        if was && !has && claim && !release && self.want == Some(screen) {
            self.sticky = Some(screen);
        }
        if has && self.sticky == Some(screen) {
            self.sticky = None;
        }
    }

    /// The page the driver picked (stalk / DIS menu). `None` = nothing.
    pub fn set_user_page(&mut self, now: Instant, page: Option<ScreenId>) {
        if page.is_some() {
            self.sticky = None;
        }
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
        if std::mem::take(&mut self.tab_picked) && self.user_page.is_none() {
            let page = self.default_page;
            self.set_user_page(now, Some(page));
            return;
        }
        self.step(now);
    }

    /// The page shown when nothing is picked and we claim always.
    pub fn default_page(&self) -> ScreenId {
        self.default_page
    }

    pub fn user_page(&self) -> Option<ScreenId> {
        self.user_page
    }

    pub fn poll_transmit(&mut self) -> Option<Frame> {
        self.session.poll_transmit()
    }

    pub fn poll_event(&mut self) -> Option<SessionEvent> {
        let e = self.session.poll_event();
        if e == Some(SessionEvent::ClusterSelected) {
            // The driver picked our tab in the cluster menu: open our page.
            self.tab_picked = true;
        }
        e
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let screen = match (self.session.profile(), self.shown) {
            (Some(p), Some((id, _))) if self.session.owned() => {
                let cx = Ctx::new(p);
                match id {
                    ScreenId::Nav => self.nav.next_deadline(&cx),
                    ScreenId::Readings => self.readings.next_deadline(&cx),
                    ScreenId::Media => self.media.next_deadline(&cx),
                    ScreenId::Call => self.call.next_deadline(&cx),
                    ScreenId::Cover => self.cover.next_deadline(&cx),
                    ScreenId::Accel => self.accel.next_deadline(&cx),
                    ScreenId::Egg => self.egg.as_ref().and_then(|e| e.next_deadline(&cx)),
                }
            }
            _ => None,
        };
        [
            self.session.next_deadline(),
            self.arbiter.next_deadline(now),
            screen,
            self.flash.as_ref().map(|f| f.hold_until.unwrap_or(f.cap_at)),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn screen_for(&mut self, now: Instant, c: Class) -> Option<ScreenId> {
        if let Some(f) = &mut self.flash {
            let drawn = self.shown.is_some_and(|(id, _)| id == ScreenId::Cover)
                && !self.cover.refining()
                && self.session.frame_idle();
            if f.hold_until.is_none() && drawn {
                f.hold_until = Some(now + self.cover_flash.unwrap_or_default());
            }
            if now >= f.cap_at || f.hold_until.is_some_and(|t| now >= t) {
                self.flash = None;
            }
        }
        if self.egg.as_ref().is_some_and(|e| e.done(now)) {
            self.egg = None;
        }
        match c {
            Class::NavManeuver => Some(ScreenId::Nav),
            Class::UserPage => self.user_page,
            // Nothing else wants the centre: our default page, if we claim always.
            Class::Idle => self
                .sticky
                .or(self.claim_always.then_some(self.default_page)),
            Class::Media if self.egg.is_some() => Some(ScreenId::Egg),
            Class::Media if self.flash.is_some() => Some(ScreenId::Cover),
            Class::Media => Some(ScreenId::Media),
            Class::Call => Some(ScreenId::Call),
            // No screens yet for alerts or the stock radio.
            _ => None,
        }
    }

    fn step(&mut self, now: Instant) {
        if self.probe {
            self.session.set_want(now, true);
            return;
        }
        let class = self.arbiter.winner(now, Surface::Centre);
        let want = self.screen_for(now, class);
        self.want = want;
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
        let enter = self.shown != Some((id, generation)) || self.reenter;
        let screen: &mut dyn Screen = match id {
            ScreenId::Nav => &mut self.nav,
            ScreenId::Readings => &mut self.readings,
            ScreenId::Media => &mut self.media,
            ScreenId::Call => &mut self.call,
            ScreenId::Cover => &mut self.cover,
            ScreenId::Accel => &mut self.accel,
            ScreenId::Egg => match &mut self.egg {
                Some(e) => e,
                None => return,
            },
        };
        match build_frame(screen, now, &cx, self.session.registers(), enter) {
            Ok(packed) => {
                self.shown = Some((id, generation));
                if enter {
                    self.reenter = false;
                }
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
