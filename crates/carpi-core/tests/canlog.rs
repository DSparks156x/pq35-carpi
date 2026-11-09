//! Replays the 10.7 s parked, ignition-on, engine-off log from Drew's TT
//! through the car model and checks what it decodes.

use carpi_can::ReplayPort;
use carpi_can::recorder::Recorder;
use carpi_core::{Car, IfaceSpec};
use carpi_nm::BusState;
use carpi_signals::ids;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn log_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/canlog-parked-kl15.txt")
}

fn replay() -> (Car, Instant) {
    // The file has no timestamps: ~3041 frames over ~10.7 s.
    let port = ReplayPort::load(&log_path(), None, Duration::from_micros(3500), false).unwrap();
    assert!(port.len() > 3000);
    let mut car = Car::new(
        vec![IfaceSpec {
            name: "can0".into(),
            roles: vec!["ican".into(), "diag".into()],
        }],
        Recorder::new(Duration::from_secs(600), 1_000_000),
        std::env::temp_dir(),
    );
    let base = Instant::now();
    let mut end = base;
    for rx in port.iter_from(base) {
        car.ingest(0, &rx);
        end = rx.at;
    }
    car.tick(end);
    (car, end)
}

#[test]
fn ring_is_awake_with_gateway_and_rnse() {
    let (car, end) = replay();
    let mon = car.ican().unwrap();
    assert_eq!(mon.state(), BusState::Awake);
    let gw = mon.member(ids::NM_NODE_GATEWAY, end).unwrap();
    assert_eq!(gw.last.successor, ids::NM_NODE_RNSE);
    let rnse = mon.member(ids::NM_NODE_RNSE, end).unwrap();
    assert_eq!(rnse.last.successor, ids::NM_NODE_GATEWAY);
    assert!(mon.node_awake(ids::NM_NODE_RNSE, end));
}

#[test]
fn decodes_parked_car() {
    let (car, end) = replay();
    let s = car.store();
    assert_eq!(s.get_bool("ignition.kl15", end), Some(true));
    assert_eq!(s.get_bool("ignition.zas.kl15", end), Some(true));
    // 0x661 `83 01 12 37`: the log was taken with the RNS-E on TV.
    assert_eq!(s.get_bool("rnse.on_tv", end), Some(true));
    assert_eq!(s.get_num("vehicle.speed_kmh", end), Some(0.0));
    // Drew: "sounds plausibly close".
    let odo = s.get_num("vehicle.odometer_km", end).unwrap();
    assert!((230_000.0..250_000.0).contains(&odo), "odometer {odo}");
    // Selector reads 8 (P) while parked.
    assert_eq!(s.get_num("trans.selector", end), Some(8.0));
    // Gateway and cluster agree on display dimming in this log (37 %).
    assert_eq!(s.get_num("light.dim_display_pct", end), Some(37.0));
    let temp = s.get_num("ambient.outside_temp_c", end).unwrap();
    assert!((-40.0..60.0).contains(&temp), "outside temp {temp}");
    // Sun load, indoors by day (PLAN 2.6 baseline).
    assert_eq!(s.get_num("light.sun_left_wm2", end), Some(100.0));
    assert_eq!(s.get_num("light.sun_right_wm2", end), Some(68.0));
    let v = s.get_num("power.battery_v", end).unwrap();
    assert!((10.0..15.5).contains(&v), "battery {v}");
}

#[test]
fn values_time_out_after_the_log() {
    let (car, end) = replay();
    let later = end + Duration::from_secs(5);
    assert_eq!(car.store().get_bool("ignition.kl15", later), None);
}

#[test]
fn bus_goes_silent_after_the_log() {
    let (mut car, end) = replay();
    let ev = car.tick(end + Duration::from_secs(3));
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].transition.to, BusState::Silent);
}

#[test]
fn capture_roundtrips() {
    let (car, _) = replay();
    let path = std::env::temp_dir().join(format!("carpi-test-{}.log", std::process::id()));
    let n = car.save_capture(&path).unwrap();
    assert!(n > 3000);
    let back = ReplayPort::load(&path, Some("can0"), Duration::ZERO, false).unwrap();
    assert_eq!(back.len(), n);
    std::fs::remove_file(path).ok();
}

#[test]
fn sends_only_label_and_brightness_to_the_rnse() {
    let port = ReplayPort::load(&log_path(), None, Duration::from_micros(3500), false).unwrap();
    let mut car = Car::new(
        vec![IfaceSpec {
            name: "can0".into(),
            roles: vec!["ican".into()],
        }],
        Recorder::new(Duration::from_secs(1), 10),
        std::env::temp_dir(),
    );
    let base = Instant::now();
    let mut next_tick = base;
    let mut sent = Vec::new();
    for rx in port.iter_from(base) {
        car.ingest(0, &rx);
        while next_tick <= rx.at {
            car.tick(next_tick);
            next_tick += Duration::from_millis(100);
        }
        for o in car.drain_outputs() {
            if let carpi_core::Output::Send { frame, .. } = o {
                sent.push(frame);
            }
        }
    }
    assert!(!sent.is_empty(), "expected the RNS-E label/brightness");
    for f in &sent {
        assert_eq!(f.id, ids::RNSE_CMD);
        assert!(matches!(f.data()[0], 0xBA | 0xBB), "{f:?}");
    }
    // Daytime in this log, nothing projecting: LIVI label, and auto
    // brightness from the default curve at the parked baseline (cluster 35,
    // display dimming 37 %).
    assert!(sent.iter().any(|f| f.data()[..2] == [0xBA, 0x00]));
    let bb: Vec<u8> = sent
        .iter()
        .filter(|f| f.data()[0] == 0xBB)
        .map(|f| f.data()[1])
        .collect();
    assert!(!bb.is_empty());
    assert!(bb.iter().all(|&l| (45..=62).contains(&l)), "{bb:?}");
    // Settle + one resend: a handful of frames, not a stream.
    assert!(sent.len() <= 6, "{} frames", sent.len());
    let s = car.store();
    let end = next_tick - Duration::from_millis(100);
    assert_eq!(s.get_bool("light.night", end), Some(false));
    assert_eq!(
        s.get("light.night_source", end),
        Some(&carpi_state::Value::Text("headlights".into()))
    );
    let a = s.get_num("light.ambient", end).unwrap();
    assert!((a - 36f64.log10()).abs() < 0.06, "ambient {a}");
    assert_eq!(
        s.get_num("light.panel_pct", end),
        bb.last().map(|&l| l as f64)
    );
    assert!(matches!(
        s.get("light.curve", end),
        Some(carpi_state::Value::Text(t)) if t.starts_with("{\"source\":\"cluster\"")
    ));
}
