//! carpi-diag against the carpi-sim gateway, on virtual time.
//!
//! Sim gaps filled here, without touching carpi-sim: [`Vcds`] is a scripted
//! foreign tester, and ABS / a slow engine are built from `ModuleSpec` and
//! `Latency` fields in the tests.

use carpi_can::Frame;
use carpi_diag::*;
use carpi_kwp::Class;
use carpi_kwp::services::Dtc;
use carpi_sim::bus::{Bus, Node};
use carpi_sim::tp2::{Gateway, ModuleSpec};
use carpi_state::Store;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

struct Daemon {
    d: Diag,
    store: Store,
    events: Vec<Event>,
}

impl Daemon {
    fn new(catalog: Catalog) -> Self {
        Daemon {
            d: Diag::new(Config::default(), catalog),
            store: Store::new(),
            events: Vec::new(),
        }
    }
    fn settle(&mut self) {
        self.d.write_samples(&mut self.store);
        while let Some(e) = self.d.poll_event() {
            self.events.push(e);
        }
    }
    fn job_done(&self, id: JobId) -> Option<Result<JobOutput, JobError>> {
        self.events.iter().find_map(|e| match e {
            Event::Job(JobEvent::Done { id: i, result }) if *i == id => Some(result.clone()),
            _ => None,
        })
    }
}

impl Node for Daemon {
    fn on_frame(&mut self, now: Instant, f: &Frame) {
        self.d.on_frame(now, f);
        self.settle();
    }
    fn tick(&mut self, now: Instant) {
        self.d.tick(now);
        self.settle();
    }
    fn poll_transmit(&mut self, _now: Instant) -> Option<Frame> {
        self.d.poll_transmit()
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.d.next_deadline()
    }
}

/// A foreign tester: sends scripted frames at set times.
struct Vcds {
    script: Vec<(Instant, Frame)>,
}

impl Node for Vcds {
    fn on_frame(&mut self, _: Instant, _: &Frame) {}
    fn tick(&mut self, _: Instant) {}
    fn poll_transmit(&mut self, now: Instant) -> Option<Frame> {
        let i = self.script.iter().position(|(t, _)| *t <= now)?;
        Some(self.script.remove(i).1)
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.script.iter().map(|(t, _)| *t).min()
    }
}

struct World {
    bus: Bus,
    gw: Gateway,
    dm: Daemon,
    vcds: Vcds,
}

impl World {
    fn new(gw: Gateway, catalog: Catalog) -> Self {
        World {
            bus: Bus::new(Instant::now()),
            gw,
            dm: Daemon::new(catalog),
            vcds: Vcds { script: Vec::new() },
        }
    }
    fn tt() -> Self {
        World::new(Gateway::tt(), Catalog::seed())
    }
    fn now(&self) -> Instant {
        self.bus.now
    }
    /// Node 0 is the daemon.
    fn run(&mut self, d: Duration) {
        self.bus
            .run_for(&mut [&mut self.dm, &mut self.gw, &mut self.vcds], d);
    }
    fn sub(&mut self, value: &str, period: Duration, priority: u8, lease: Duration) -> SubId {
        let now = self.now();
        let id = self
            .dm
            .d
            .subscribe(
                now,
                Subscription {
                    value: value.into(),
                    period,
                    priority,
                    lease_expires: now + lease,
                },
            )
            .unwrap();
        self.dm.settle();
        id
    }
    fn status(&self, id: SubId) -> SubStatus {
        self.dm.d.status(self.now(), id).expect("live sub")
    }
    fn num(&self, name: &str) -> Option<f64> {
        self.dm.store.get_num(name, self.now())
    }
    fn job(&mut self, job: Job, token: Option<&carpi_kwp::ExclusiveToken>) -> JobId {
        let now = self.now();
        let id = self.dm.d.start_job(now, job, token).unwrap();
        self.dm.settle();
        for _ in 0..500 {
            if self.dm.job_done(id).is_some() {
                break;
            }
            self.run(ms(10));
        }
        id
    }
    /// Frames the daemon sent in `[from, to)`.
    fn sent(&self, from: Instant, to: Instant) -> Vec<(u32, Vec<u8>)> {
        self.bus
            .sent_by(0)
            .filter(|(t, ..)| *t >= from && *t < to)
            .map(|(_, id, d)| (id, d.to_vec()))
            .collect()
    }
    fn setups(&self, module: u8) -> usize {
        self.gw.setups.iter().filter(|(m, _)| *m == module).count()
    }
}

#[test]
fn haldex_queries_before_setting_and_waits_for_debounce_confirmation() {
    let mut spec = ModuleSpec::haldex();
    spec.replies.insert(
        vec![0x3B, 0xF0],
        [
            vec![0x7B, 0xF0, 0, 0, 0xFF, 3],
            vec![0x7B, 0xF0, 2, 2, 0xFF, 3],
        ]
        .into(),
    );
    spec.replies.insert(
        vec![0x3B, 0xF0, 0x3C, 0xC3],
        [vec![0x7B, 0xF0, 0, 0, 2, 1]].into(),
    );
    let mut w = World::new(Gateway::new([spec]), Catalog::seed());
    let token = w.dm.d.begin_exclusive(w.now(), Class::Persistent).unwrap();
    let id = w.job(Job::HaldexMode { mode: 2 }, Some(&token));
    assert!(matches!(w.dm.job_done(id), Some(Ok(JobOutput::HaldexMode(s))) if s.matches(2)));
    let requests = &w.gw.module(0x0A).requests;
    assert_eq!(
        requests
            .iter()
            .filter(|r| r[0] == 0x3B)
            .cloned()
            .collect::<Vec<_>>(),
        [
            vec![0x3B, 0xF0],
            vec![0x3B, 0xF0, 0x3C, 0xC3],
            vec![0x3B, 0xF0]
        ]
    );
    let sent = w
        .bus
        .sent_by(0)
        .filter(|(_, _, d)| d.get(3..5) == Some(&[0x3B, 0xF0]))
        .collect::<Vec<_>>();
    assert_eq!(sent.len(), 3);
    assert!(sent[2].0.duration_since(sent[1].0) >= Duration::from_millis(300));
}

#[test]
fn haldex_never_sets_an_unsupported_or_malformed_handler() {
    for reply in [
        vec![0x7F, 0x3B, 0x12],
        vec![0x7B, 0xF0, 0, 0],
        vec![0x7B, 0xF0, 9, 0, 0xFF, 3],
    ] {
        let mut spec = ModuleSpec::haldex();
        spec.replies.insert(vec![0x3B, 0xF0], [reply].into());
        let mut w = World::new(Gateway::new([spec]), Catalog::seed());
        let token = w.dm.d.begin_exclusive(w.now(), Class::Persistent).unwrap();
        let id = w.job(Job::HaldexMode { mode: 4 }, Some(&token));
        assert_eq!(w.dm.job_done(id), Some(Err(JobError::HaldexUnsupported)));
        assert_eq!(
            w.gw.module(0x0A)
                .requests
                .iter()
                .filter(|r| r[0] == 0x3B)
                .cloned()
                .collect::<Vec<_>>(),
            [vec![0x3B, 0xF0]]
        );
    }
}

#[test]
fn haldex_set_acknowledgement_does_not_count_as_confirmation() {
    let mut spec = ModuleSpec::haldex();
    spec.replies.insert(
        vec![0x3B, 0xF0],
        [
            vec![0x7B, 0xF0, 0, 0, 0xFF, 3],
            vec![0x7B, 0xF0, 0, 0, 4, 2],
        ]
        .into(),
    );
    spec.replies.insert(
        vec![0x3B, 0xF0, 0x96, 0x69],
        [vec![0x7B, 0xF0, 4, 4, 0xFF, 3]].into(),
    );
    let mut w = World::new(Gateway::new([spec]), Catalog::seed());
    let token = w.dm.d.begin_exclusive(w.now(), Class::Persistent).unwrap();
    let id = w.job(Job::HaldexMode { mode: 4 }, Some(&token));
    assert_eq!(w.dm.job_done(id), Some(Err(JobError::HaldexUnconfirmed)));
}

fn near(v: f64, want: f64, tol: f64) -> bool {
    (v - want).abs() <= tol
}

#[test]
fn engine_values_reach_the_store_at_the_asked_rate() {
    let mut w = World::tt();
    let maf = w.sub("engine.maf_gs", ms(200), 1, Duration::from_secs(60));
    let atm = w.sub(
        "engine.atmospheric_mbar",
        ms(200),
        1,
        Duration::from_secs(60),
    );
    let thr = w.sub("engine.throttle_pct", ms(200), 1, Duration::from_secs(60));
    w.run(Duration::from_secs(10));

    // Packed into groups 3 (maf, throttle) and 113 (atmospheric).
    assert_eq!(
        w.status(maf).source,
        Source::Diag {
            module: 0x01,
            group: 3,
            field: 2
        }
    );
    assert_eq!(
        w.status(thr).source,
        Source::Diag {
            module: 0x01,
            group: 3,
            field: 3
        }
    );
    for id in [maf, atm] {
        let s = w.status(id);
        assert_eq!(s.granted, Some(ms(200)));
        assert_eq!(s.degraded, None, "{s:?}");
        let hz = s.achieved_hz.unwrap();
        assert!(near(hz, 5.0, 0.5), "{hz} Hz");
    }
    // Trace values: 4.44 g/s, 5.1 %, 795 mbar.
    assert!(near(w.num("engine.maf_gs").unwrap(), 4.44, 0.01));
    assert!(near(w.num("engine.throttle_pct").unwrap(), 5.1, 0.01));
    assert_eq!(w.num("engine.atmospheric_mbar"), Some(795.0));
    // One channel, our tester ID, never 0x300.
    assert_eq!(w.setups(0x01), 1);
    assert!(w.gw.setups.iter().all(|(_, t)| (0x308..=0x30F).contains(t)));
    assert!(w.dm.store.age("engine.maf_gs", w.now()).unwrap() < ms(300));
    // Store timeout is 3 x 200 ms, floored at 1 s: once reads stop, the value
    // reads as nothing after a second.
    let now = w.now();
    assert!(w.dm.d.unsubscribe(now, maf) && w.dm.d.unsubscribe(now, thr));
    w.run(ms(1100));
    assert_eq!(w.num("engine.maf_gs"), None);
    assert_eq!(w.num("engine.atmospheric_mbar"), Some(795.0));
}

#[test]
fn over_budget_is_granted_less_and_says_so() {
    let mut w = World::tt();
    let a = w.sub("engine.maf_gs", ms(50), 1, Duration::from_secs(60));
    let b = w.sub("engine.load_pct", ms(50), 1, Duration::from_secs(60));
    w.run(Duration::from_secs(5));
    for id in [a, b] {
        let s = w.status(id);
        assert!(s.granted.unwrap() > ms(50), "{s:?}");
        assert_eq!(s.degraded, Some(Degraded::Budget));
        assert!(s.achieved_hz.unwrap() > 3.0, "{s:?}");
    }
}

#[test]
fn two_modules_interleave_and_a_slow_one_stalls_nothing() {
    let mut w = World::tt();
    // Engine answers in 300 ms; the Haldex at the measured engine pace.
    w.gw.module(0x01).lat.first_frame = (ms(300), ms(300));
    let eng = w.sub("engine.maf_gs", ms(100), 1, Duration::from_secs(60));
    let hal = w.sub("awd.plate_temp_c", ms(100), 1, Duration::from_secs(60));
    w.run(Duration::from_secs(10));

    let e = w.status(eng).achieved_hz.unwrap();
    let h = w.status(hal).achieved_hz.unwrap();
    assert!(e < 3.5, "engine {e} Hz");
    assert!(h > 8.0, "haldex {h} Hz");
    // Sim Haldex group 1 field 2: 05 0A 82 = 30 °C (reported unit).
    assert!(near(w.num("awd.plate_temp_c").unwrap(), 30.0, 1e-9));
    assert!(w.num("engine.maf_gs").is_some());
    // Two channels, two different tester IDs.
    let chans = w.dm.d.channels();
    let open: Vec<_> = chans
        .iter()
        .filter(|c| c.state == ChannelState::Open)
        .collect();
    assert_eq!(open.len(), 2);
    assert_ne!(open[0].tester_rx, open[1].tester_rx);
}

#[test]
fn expired_lease_closes_the_channel_after_grace() {
    let mut w = World::tt();
    let id = w.sub("engine.maf_gs", ms(200), 1, Duration::from_secs(2));
    w.run(Duration::from_secs(1));
    assert!(w.gw.module(0x01).is_open());
    let t_exp = w.now() + Duration::from_secs(1);

    w.run(Duration::from_millis(1100));
    assert!(w.dm.d.status(w.now(), id).is_none());
    assert!(w.dm.events.contains(&Event::SubExpired(id)));
    // Grace: still open, no more reads.
    let ch = &w.dm.d.channels()[0];
    assert!(matches!(ch.state, ChannelState::Grace { .. }), "{ch:?}");
    w.run(Duration::from_secs(3));
    assert!(w.gw.module(0x01).is_open());
    let reads_after = w
        .sent(t_exp + ms(100), w.now())
        .iter()
        .filter(|(_, d)| d.len() >= 4 && d[3] == 0x21)
        .count();
    assert_eq!(reads_after, 0);

    // Grace (5 s) ends: A8, sim drops its session, ID back in the pool.
    w.run(Duration::from_secs(2));
    assert!(!w.gw.module(0x01).is_open());
    assert!(w.sent(t_exp, w.now()).contains(&(0x740, vec![0xA8])));
    assert_eq!(w.dm.d.channels()[0].state, ChannelState::Closed);
}

#[test]
fn ican_value_never_opens_a_channel() {
    let mut w = World::tt();
    let id = w.sub("engine.rpm", ms(100), 1, Duration::from_secs(60));
    w.run(Duration::from_secs(3));
    let s = w.status(id);
    assert_eq!(s.source, Source::Ican("engine.rpm"));
    assert_eq!(s.degraded, None);
    assert!(w.gw.setups.is_empty());
    assert_eq!(w.bus.sent_by(0).count(), 0);
}

#[test]
fn foreign_tester_pauses_us_until_quiet() {
    let mut w = World::tt();
    let maf = w.sub("engine.maf_gs", ms(100), 1, Duration::from_secs(120));
    let hal = w.sub("awd.oil_temp_c", ms(100), 1, Duration::from_secs(120));
    w.run(Duration::from_secs(2));
    assert!(w.num("engine.maf_gs").is_some());

    // VCDS opens the engine on 0x300, then pokes it for 3 s.
    let t0 = w.now() + ms(5);
    w.vcds.script.push((
        t0,
        Frame::new(0x200, &[0x01, 0xC0, 0x00, 0x10, 0x00, 0x03, 0x01]).unwrap(),
    ));
    for i in 1..=3u64 {
        w.vcds.script.push((
            t0 + Duration::from_secs(i),
            Frame::new(0x740, &[0xA3]).unwrap(),
        ));
    }
    w.run(ms(100));
    assert_eq!(w.dm.d.mode(), Mode::VcdsPause { manual: false });
    assert!(
        w.dm.events
            .contains(&Event::Mode(Mode::VcdsPause { manual: false }))
    );
    assert_eq!(
        w.status(maf).degraded,
        Some(Degraded::Paused(Mode::VcdsPause { manual: false }))
    );
    // Polite to the Haldex, nothing at all to the engine VCDS is using.
    // (A frame of ours already on the wire when the setup arrived may finish
    // inside the first ~1 ms, hence the 2 ms start.)
    let burst = w.sent(t0, t0 + ms(10));
    assert!(burst.contains(&(0x764, vec![0xA8])), "{burst:?}");
    assert!(
        w.sent(t0 + ms(2), t0 + ms(10))
            .iter()
            .all(|(id, _)| *id != 0x740)
    );

    // Silent while VCDS is active (its A3s make the engine answer on 0x300)
    // and for the quiet period after.
    w.run(Duration::from_secs(12));
    let last_vcds = t0 + Duration::from_secs(3);
    assert!(w.sent(t0 + ms(10), last_vcds + ms(9_900)).is_empty());

    // Resumed: channels reopen, values flow again.
    w.run(Duration::from_secs(3));
    assert_eq!(w.dm.d.mode(), Mode::Normal);
    assert!(w.dm.store.age("engine.maf_gs", w.now()).unwrap() < ms(300));
    assert!(w.dm.store.age("awd.oil_temp_c", w.now()).unwrap() < ms(300));
    assert!(w.status(hal).degraded.is_none());
}

#[test]
fn manual_pause_and_bus_lease_stop_all_tx() {
    let mut w = World::tt();
    w.sub("engine.maf_gs", ms(100), 1, Duration::from_secs(120));
    w.run(Duration::from_secs(1));

    let now = w.now();
    w.dm.d.set_vcds_pause(now, true);
    w.dm.settle();
    w.run(Duration::from_secs(2));
    assert!(w.sent(now, w.now()).iter().all(|(_, d)| d == &[0xA8]));
    assert_eq!(w.dm.d.mode(), Mode::VcdsPause { manual: true });
    let now = w.now();
    w.dm.d.set_vcds_pause(now, false);
    w.run(Duration::from_secs(1));
    assert!(w.gw.module(0x01).is_open());

    let now = w.now();
    w.dm.d.set_bus_lease(now, true);
    w.dm.settle();
    assert!(w.dm.d.start_job(now, Job::InstallList, None).is_err());
    w.run(Duration::from_secs(2));
    assert!(w.sent(now, w.now()).iter().all(|(_, d)| d == &[0xA8]));
    assert!(!w.gw.module(0x01).is_open());
    let now = w.now();
    w.dm.d.set_bus_lease(now, false);
    w.run(Duration::from_secs(1));
    assert!(w.gw.module(0x01).is_open());
}

#[test]
fn abs_is_never_polled() {
    let abs = ModuleSpec {
        tp2: 0x03,
        rx_id: 0x743,
        params: [0x0F, 0x8A, 0xFF, 0x4A, 0xFF],
        groups: BTreeMap::from([(1, vec![[0x07, 0x64, 0x10]])]),
        dtcs: vec![(1, 0)],
        ..Default::default()
    };
    let gw = Gateway::new([ModuleSpec::gateway(), ModuleSpec::engine(), abs]);
    let cat = Catalog::new(vec![ValueDef::new("abs.speed", Unit::Reported).diag(
        0x03,
        1,
        1,
        Basis::Custom,
    )])
    .unwrap();
    let mut w = World::new(gw, cat);
    let id = w.sub("abs.speed", ms(100), 9, Duration::from_secs(60));
    w.run(Duration::from_secs(3));
    let s = w.status(id);
    assert_eq!(s.source, Source::Unavailable);
    assert_eq!(s.degraded, Some(Degraded::Denied { module: 0x03 }));
    assert_eq!(w.setups(0x03), 0);
    // Jobs too, without the token.
    let now = w.now();
    assert_eq!(
        w.dm.d.start_job(now, Job::ReadDtcs { module: 0x03 }, None),
        Err(JobError::Denied)
    );
    assert_eq!(w.setups(0x03), 0);
}

#[test]
fn install_list_and_dtc_read() {
    let mut w = World::tt();
    let id = w.job(Job::InstallList, None);
    let Some(Ok(JobOutput::InstallList(l))) = w.dm.job_done(id) else {
        panic!("{:?}", w.dm.events)
    };
    assert_eq!(l.len(), 6);
    assert!(l.iter().any(|i| i.vcds == 0x22 && i.tp2 == 0x0A));

    let id = w.job(Job::ReadDtcs { module: 0x01 }, None);
    assert_eq!(
        w.dm.job_done(id),
        Some(Ok(JobOutput::Dtcs(vec![Dtc {
            code: 17965,
            status: 0x23
        }])))
    );
    assert!(w.dm.events.iter().any(|e| matches!(
        e,
        Event::Job(JobEvent::Progress {
            stage: JobStage::Request { .. },
            ..
        })
    )));
}

#[test]
fn dtc_clear_needs_the_token() {
    let mut w = World::tt();
    let maf = w.sub("engine.maf_gs", ms(100), 1, Duration::from_secs(120));
    w.run(Duration::from_secs(1));
    let now = w.now();
    assert_eq!(
        w.dm.d.start_job(now, Job::ClearDtcs { module: 0x01 }, None),
        Err(JobError::NeedsToken)
    );
    assert_eq!(w.gw.module(0x01).spec.dtcs.len(), 1);
    assert!(!w.gw.module(0x01).requests.iter().any(|r| r[0] == 0x14));

    // A read-only token doesn't cover it either.
    let tok = w.dm.d.begin_exclusive(now, Class::ReadOnly).unwrap();
    assert_eq!(
        w.dm.d
            .start_job(now, Job::ClearDtcs { module: 0x01 }, Some(&tok)),
        Err(JobError::StaleToken)
    );
    w.dm.d.end_exclusive(now, tok).unwrap();

    let tok = w.dm.d.begin_exclusive(now, Class::Persistent).unwrap();
    assert_eq!(
        w.dm.d.begin_exclusive(now, Class::ReadOnly).err(),
        Some(ModeError::Held)
    );
    // Others are locked out; polling stops.
    assert_eq!(
        w.dm.d.start_job(now, Job::InstallList, None),
        Err(JobError::Exclusive)
    );
    let id = w.job(Job::ClearDtcs { module: 0x01 }, Some(&tok));
    assert_eq!(
        w.dm.job_done(id),
        Some(Ok(JobOutput::Cleared { remaining: vec![] }))
    );
    assert!(w.gw.module(0x01).spec.dtcs.is_empty());
    assert!(matches!(
        w.status(maf).degraded,
        Some(Degraded::Paused(Mode::Exclusive { .. }))
    ));
    let now = w.now();
    w.dm.d.end_exclusive(now, tok).unwrap();
    assert_eq!(w.dm.d.mode(), Mode::Normal);
}

#[test]
fn rejected_group_falls_back_to_another_provider() {
    // Engine sim has no group 2; a value whose first choice is group 2 moves
    // to its next provider.
    let cat = Catalog::new(vec![
        ValueDef::new("x.maf", Unit::Expect("g/s"))
            .diag(0x01, 2, 4, Basis::Custom)
            .diag(0x01, 3, 2, Basis::Custom),
    ])
    .unwrap();
    let mut w = World::new(Gateway::tt(), cat);
    let id = w.sub("x.maf", ms(200), 1, Duration::from_secs(60));
    w.run(Duration::from_secs(2));
    assert_eq!(
        w.status(id).source,
        Source::Diag {
            module: 0x01,
            group: 3,
            field: 2
        }
    );
    assert!(near(w.num("x.maf").unwrap(), 4.44, 0.01));
}

#[test]
fn absent_module_backs_off() {
    let mut w = World::tt();
    let id = w.sub("engine.maf_gs", ms(100), 1, Duration::from_secs(60));
    w.gw.module(0x01).faults.silent = true;
    w.run(Duration::from_millis(3800));
    let s = w.status(id);
    assert!(
        matches!(s.degraded, Some(Degraded::Backoff { module: 1, .. })),
        "{s:?}"
    );
    // 0.5 s setup timeout, then 1 s, 2 s backoff: a handful of tries, not a flood.
    assert!((2..=4).contains(&w.setups(0x01)), "{}", w.setups(0x01));
}

#[test]
fn module_group_names_poll_any_field() {
    let mut w = World::tt();
    // Engine group 113 field 4 is not under this name in the catalog: the
    // name alone is enough. Haldex is VCDS 22 -> TP2 0x0A.
    let a = w.sub("diag.01.113.4", ms(200), 1, Duration::from_secs(60));
    let b = w.sub("diag.01.3.1", ms(200), 1, Duration::from_secs(60));
    let h = w.sub("diag.22.1.2", ms(500), 1, Duration::from_secs(60));
    w.run(Duration::from_secs(3));
    assert_eq!(
        w.status(a).source,
        Source::Diag {
            module: 0x01,
            group: 113,
            field: 4
        }
    );
    assert_eq!(
        w.status(h).source,
        Source::Diag {
            module: 0x0A,
            group: 1,
            field: 2
        }
    );
    assert_eq!(w.num("diag.01.113.4"), Some(795.0));
    assert_eq!(w.num("diag.01.3.1"), Some(840.0));
    assert_eq!(w.num("diag.22.1.2"), Some(30.0));
    assert_eq!(w.status(b).value, "diag.01.3.1");
    let def = w.dm.d.catalog().get("diag.01.3.1").unwrap();
    assert_eq!((def.label, def.sub), ("01 · 3 · 1", "Module groups"));

    // ABS, unknown addresses and malformed names are refused.
    let now = w.now();
    let sub = |value: &str| Subscription {
        value: value.into(),
        period: ms(200),
        priority: 1,
        lease_expires: now + Duration::from_secs(5),
    };
    assert_eq!(
        w.dm.d.subscribe(now, sub("diag.03.1.1")),
        Err(SubError::Denied)
    );
    assert_eq!(
        w.dm.d.subscribe(now, sub("diag.7E.1.1")),
        Err(SubError::UnknownValue)
    );
    assert_eq!(
        w.dm.d.subscribe(now, sub("diag.01.3.5")),
        Err(SubError::UnknownValue)
    );
    assert_eq!(w.setups(0x03), 0);
}

#[test]
fn identify_and_one_group() {
    let mut w = World::tt();
    let id = w.job(Job::Identify { module: 0x01 }, None);
    let Some(Ok(JobOutput::Identity(i))) = w.dm.job_done(id) else {
        panic!("{:?}", w.dm.events)
    };
    // The sim's reply isn't laid out like a real one; the part number is.
    assert_eq!(i.part_no.as_deref(), Some("8J0907115"));
    assert_eq!(i.coding, None);

    let id = w.job(
        Job::ReadGroup {
            module: 0x01,
            group: 3,
        },
        None,
    );
    let Some(Ok(JobOutput::Group(g))) = w.dm.job_done(id) else {
        panic!("{:?}", w.dm.events)
    };
    assert_eq!(g.records.len(), 8);
    let id = w.job(
        Job::ReadGroup {
            module: 0x01,
            group: 2,
        },
        None,
    );
    assert!(matches!(
        w.dm.job_done(id),
        Some(Err(JobError::Kwp(carpi_kwp::KwpError::Negative {
            nrc: 0x31
        })))
    ));

    // An install list moves an address to the TP2 ID the gateway gives.
    let id = w.job(Job::InstallList, None);
    assert!(w.dm.job_done(id).is_some_and(|r| r.is_ok()));
    assert_eq!(w.dm.d.tp2_of(0x46), Some(0x21));
    assert_eq!(w.dm.d.address_of(0x0A), Some(0x22));

    // No measuring block from ABS, even under the token.
    let now = w.now();
    let t = w.dm.d.begin_exclusive(now, Class::ReadOnly).unwrap();
    assert_eq!(
        w.dm.d.start_job(
            now,
            Job::ReadGroup {
                module: 0x03,
                group: 1
            },
            Some(&t)
        ),
        Err(JobError::Denied)
    );
    w.dm.d.end_exclusive(now, t).unwrap();
}
