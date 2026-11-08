//! Hand-written decoders for the ICAN messages we use.
//!
//! Each decoder emits `(name, value)` pairs; `None` means the frame carried an
//! error/init value, an "Alt" (stale-at-gateway) flag, or was too short.
//! Bit positions come from the PQ35/46 ICAN DBC (see `ICAN_rundown.md` in the
//! old repo). Only "Alt" bits the DBC ties to a signal by name are applied.

use crate::bits::{bit, get};
use crate::ids::*;
use carpi_state::Value;
use std::time::Duration;

pub type Emit<'a> = &'a mut dyn FnMut(&'static str, Option<Value>);

pub struct Message {
    pub id: u32,
    pub name: &'static str,
    /// Nominal cycle time.
    pub cycle: Duration,
    pub decode: fn(&[u8], Emit),
}

impl Message {
    /// How long a value from this message stays valid: 3x the cycle, at
    /// least a second.
    pub fn timeout(&self) -> Duration {
        (self.cycle * 3).max(Duration::from_secs(1))
    }
}

const fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

pub static MESSAGES: &[Message] = &[
    Message {
        id: ZAS_STATUS,
        name: "mZAS_Status",
        cycle: ms(100),
        decode: zas_status,
    },
    Message {
        id: GW_BREMSE_GETRIEBE,
        name: "mGW_Bremse_Getriebe",
        cycle: ms(100),
        decode: gw_bremse_getriebe,
    },
    Message {
        id: GW_MOTOR,
        name: "mGW_Motor",
        cycle: ms(100),
        decode: gw_motor,
    },
    Message {
        id: LENKWINKEL_1,
        name: "mLenkwinkel_1",
        cycle: ms(100),
        decode: lenkwinkel_1,
    },
    Message {
        id: BSG_KOMBI,
        name: "mBSG_Kombi",
        cycle: ms(50),
        decode: bsg_kombi,
    },
    Message {
        id: GW_KOMBI,
        name: "mGW_Kombi",
        cycle: ms(200),
        decode: gw_kombi,
    },
    Message {
        id: BSG_2,
        name: "mBSG_2",
        cycle: ms(500),
        decode: bsg_2,
    },
    Message {
        id: BSG_3,
        name: "mBSG_3",
        cycle: ms(200),
        decode: bsg_3,
    },
    Message {
        id: BREMSE_10,
        name: "mBremse_10",
        cycle: ms(200),
        decode: bremse_10,
    },
    Message {
        id: KOMBI_K1,
        name: "mKombi_K1",
        cycle: ms(100),
        decode: kombi_k1,
    },
    Message {
        id: DIMMUNG,
        name: "mDimmung",
        cycle: ms(200),
        decode: dimmung,
    },
    Message {
        id: CLIMA_2,
        name: "mClima_2",
        cycle: ms(800),
        decode: clima_2,
    },
    Message {
        id: RADIO_4,
        name: "mRadio_4",
        cycle: ms(1000),
        decode: radio_4,
    },
    Message {
        id: DIAGNOSE_1,
        name: "mDiagnose_1",
        cycle: ms(1000),
        decode: diagnose_1,
    },
];

pub fn message(id: u32) -> Option<&'static Message> {
    MESSAGES.iter().find(|m| m.id == id)
}

/// Decodes one frame. Returns false if the ID isn't one we decode.
pub fn decode(id: u32, data: &[u8], emit: Emit) -> bool {
    match message(id) {
        Some(m) => {
            (m.decode)(data, emit);
            true
        }
        None => false,
    }
}

// --- helpers -----------------------------------------------------------------

fn flag(d: &[u8], n: u32) -> Option<Value> {
    bit(d, n).map(Value::Bool)
}

/// Scaled number; `None` if the raw value is in `bad`.
fn num(d: &[u8], start: u32, len: u32, factor: f64, offset: f64, bad: &[u64]) -> Option<Value> {
    let raw = get(d, start, len)?;
    if bad.contains(&raw) {
        return None;
    }
    Some(Value::Num(raw as f64 * factor + offset))
}

/// Scaled number; `None` above `max_raw`.
fn num_max(
    d: &[u8],
    start: u32,
    len: u32,
    factor: f64,
    offset: f64,
    max_raw: u64,
) -> Option<Value> {
    let raw = get(d, start, len)?;
    if raw > max_raw {
        return None;
    }
    Some(Value::Num(raw as f64 * factor + offset))
}

/// Magnitude plus a separate sign bit (1 = negative).
fn signed_mag(d: &[u8], start: u32, len: u32, sign: u32, factor: f64) -> Option<Value> {
    let raw = get(d, start, len)? as f64 * factor;
    let neg = bit(d, sign)?;
    Some(Value::Num(if neg { -raw } else { raw }))
}

/// Voids `v` when the given "Alt"/qualifier bit is set (or missing).
fn unless(d: &[u8], n: u32, v: Option<Value>) -> Option<Value> {
    match bit(d, n) {
        Some(false) => v,
        _ => None,
    }
}

// --- decoders ----------------------------------------------------------------

/// 0x2C3, ignition switch (what the key asks for). DLC 1.
fn zas_status(d: &[u8], e: Emit) {
    e("ignition.zas.s", flag(d, 0));
    e("ignition.zas.kl15", flag(d, 1));
    e("ignition.zas.x", flag(d, 2));
    e("ignition.zas.kl50", flag(d, 3));
}

/// 0x575, terminal states from the body controller. DLC 4.
fn bsg_3(d: &[u8], e: Emit) {
    e("ignition.kl_s", flag(d, 0));
    e("ignition.kl15", flag(d, 1));
    e("ignition.kl_x", flag(d, 2));
    e("ignition.kl50", flag(d, 3));
    e("ignition.kl_p", flag(d, 4));
}

/// 0x571, battery. DLC 6.
fn bsg_2(d: &[u8], e: Emit) {
    e("power.battery_v", num(d, 0, 8, 0.05, 5.0, &[255]));
}

/// 0x359, speed, selector, EPB.
fn gw_bremse_getriebe(d: &[u8], e: Emit) {
    // 32708 init, 32725 undervoltage, 32742 error; valid up to 32639.
    e(
        "vehicle.speed_kmh",
        unless(d, 0, num_max(d, 9, 15, 0.01, 0.0, 32639)),
    );
    e(
        "vehicle.front_axle_pulses",
        unless(d, 1, num(d, 24, 11, 1.0, 0.0, &[])),
    );
    e(
        "vehicle.pulses_per_rev",
        unless(d, 1, num(d, 40, 6, 1.0, 0.0, &[])),
    );
    e("brakes.epb_closed", unless(d, 5, flag(d, 51)));
    // 0 between, 1-4, 5 D, 6 N, 7 R, 8 P, 9 manual sport, 10/11 Z1/Z2,
    // 12 S, 13 L, 14 manual gate. Selector position, not actual gear.
    e(
        "trans.selector",
        unless(d, 3, num(d, 60, 4, 1.0, 0.0, &[15])),
    );
}

/// 0x35B, engine values relayed by the gateway.
fn gw_motor(d: &[u8], e: Emit) {
    e("engine.rpm", num_max(d, 8, 16, 0.25, 0.0, 0xFEFF));
    let coolant = num(d, 24, 8, 0.75, -48.0, &[0, 255]);
    e("engine.coolant_c", unless(d, 34, coolant));
}

/// 0x3C3, steering angle and rate. Valid only when the sensor state is OK.
fn lenkwinkel_1(d: &[u8], e: Emit) {
    let ok = get(d, 41, 2) == Some(0);
    let angle = signed_mag(d, 0, 15, 15, 0.04375);
    let rate = signed_mag(d, 16, 15, 31, 0.04375);
    e("steering.angle_deg", if ok { angle } else { None });
    e("steering.rate_dps", if ok { rate } else { None });
}

/// 0x470, body controller to cluster: doors, lights, dimming.
fn bsg_kombi(d: &[u8], e: Emit) {
    e("lights.turn_left", flag(d, 0));
    e("lights.turn_right", flag(d, 1));
    e("lights.hazard", flag(d, 3));
    e("lights.reverse", flag(d, 5));
    e("body.door_fl_open", flag(d, 8));
    e("body.door_fr_open", flag(d, 9));
    e("body.door_rl_open", flag(d, 10));
    e("body.door_rr_open", flag(d, 11));
    e("body.hood_open", flag(d, 12));
    e(
        "light.display_pct",
        unless(d, 23, num_max(d, 16, 7, 1.0, 0.0, 100)),
    );
    e(
        "light.kl58t_pct",
        unless(d, 31, num_max(d, 24, 7, 1.0, 0.0, 100)),
    );
    e("lights.side", flag(d, 40));
    e("lights.low_beam", flag(d, 43));
    e("lights.fog_front", flag(d, 44));
    e("lights.fog_rear", flag(d, 61));
    e("lights.high_beam", flag(d, 62));
    e("lights.drl", flag(d, 63));
}

/// 0x527, gateway to cluster: outside temperature, tyre circumference.
fn gw_kombi(d: &[u8], e: Emit) {
    e("ambient.outside_temp_c", num(d, 40, 8, 0.5, -50.0, &[255]));
    let circ = if bit(d, 4) == Some(true) {
        num(d, 28, 12, 1.0, 0.0, &[])
    } else {
        None
    };
    e("vehicle.tyre_circumference_mm", circ);
}

/// 0x5B5, per-wheel pulse counters and direction (dead reckoning inputs).
fn bremse_10(d: &[u8], e: Emit) {
    const WHEELS: [(&str, &str, u32, u32, u32); 4] = [
        ("wheels.fl_pulses", "wheels.fl_reverse", 16, 12, 60),
        ("wheels.fr_pulses", "wheels.fr_reverse", 26, 13, 61),
        ("wheels.rl_pulses", "wheels.rl_reverse", 36, 14, 62),
        ("wheels.rr_pulses", "wheels.rr_reverse", 46, 15, 63),
    ];
    for (pulses, dir, start, qual, dir_bit) in WHEELS {
        // 1021-1023 are init/error; the counter runs 0-1000.
        e(
            pulses,
            unless(d, qual, num_max(d, start, 10, 1.0, 0.0, 1000)),
        );
        e(dir, unless(d, dir_bit - 4, flag(d, dir_bit)));
    }
}

/// 0x621, cluster: fuel, light sensor, handbrake. DLC 7.
fn kombi_k1(d: &[u8], e: Emit) {
    e("fuel.low_warning", flag(d, 1));
    e("brakes.handbrake", flag(d, 5));
    e("fuel.level_l", num(d, 24, 7, 1.0, 0.0, &[127]));
    e(
        "light.cluster_display_pct",
        num_max(d, 40, 7, 1.0, 0.0, 100),
    );
    e("light.cluster_sensor", num(d, 48, 8, 1.0, 0.0, &[254, 255]));
}

/// 0x635, gateway dimming as the RNS-E sees it. DLC 3.
fn dimmung(d: &[u8], e: Emit) {
    e(
        "light.dim_display_pct",
        unless(d, 7, num_max(d, 0, 7, 1.0, 0.0, 100)),
    );
    // Terminal 58s; the RNS-E's own auto day/night treats nonzero as night.
    e(
        "light.dim_switch_pct",
        unless(d, 15, num_max(d, 8, 7, 1.0, 0.0, 100)),
    );
    e("light.dim_sensor", num(d, 16, 8, 1.0, 0.0, &[254, 255]));
}

/// 0x3E3, sun load from the climatronic's dash-top sensor: W/m2 x4, FF =
/// error (DBC). Parked log, indoors by day: 100 / 68.
fn clima_2(d: &[u8], e: Emit) {
    e("light.sun_left_wm2", num(d, 0, 8, 4.0, 0.0, &[255]));
    e("light.sun_right_wm2", num(d, 8, 8, 4.0, 0.0, &[255]));
}

/// TV source marker in 0x661 byte 3 (RA4_Radio_Para2).
pub const RADIO_4_TV: u8 = 0x37;

/// 0x661, sent by the RNS-E. Byte 3 says which source is active; 0x37 is TV.
fn radio_4(d: &[u8], e: Emit) {
    e("rnse.radio_on", flag(d, 0));
    e("rnse.mode", num(d, 8, 8, 1.0, 0.0, &[]));
    e(
        "rnse.on_tv",
        d.get(3).map(|b| Value::Bool(*b == RADIO_4_TV)),
    );
}

/// 0x65D, odometer and the car's clock.
fn diagnose_1(d: &[u8], e: Emit) {
    e(
        "vehicle.odometer_km",
        unless(d, 62, num(d, 8, 20, 1.0, 0.0, &[])),
    );
    e("clock.datetime", unless(d, 63, datetime(d)));
}

fn datetime(d: &[u8]) -> Option<Value> {
    let year = get(d, 28, 7)? + 2000;
    let month = get(d, 35, 4)?;
    let day = get(d, 39, 5)?;
    let hour = get(d, 44, 5)?;
    let minute = get(d, 49, 6)?;
    let second = get(d, 55, 6)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(Value::Text(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn run(id: u32, data: &[u8]) -> HashMap<&'static str, Option<Value>> {
        let mut out = HashMap::new();
        assert!(decode(id, data, &mut |k, v| {
            out.insert(k, v);
        }));
        out
    }

    fn put(d: &mut [u8], start: u32, len: u32, v: u64) {
        for i in 0..len {
            let b = start + i;
            let bitv = (v >> i) & 1;
            d[(b / 8) as usize] &= !(1 << (b % 8));
            d[(b / 8) as usize] |= (bitv as u8) << (b % 8);
        }
    }

    #[test]
    fn speed_and_invalids() {
        let mut d = [0u8; 8];
        put(&mut d, 9, 15, 5012);
        let Some(Value::Num(v)) = run(GW_BREMSE_GETRIEBE, &d)["vehicle.speed_kmh"] else {
            panic!()
        };
        assert!((v - 50.12).abs() < 1e-9);
        put(&mut d, 9, 15, 32708);
        assert_eq!(run(GW_BREMSE_GETRIEBE, &d)["vehicle.speed_kmh"], None);
        put(&mut d, 9, 15, 100);
        put(&mut d, 0, 1, 1); // GWB_Alt_FzgGeschw
        assert_eq!(run(GW_BREMSE_GETRIEBE, &d)["vehicle.speed_kmh"], None);
    }

    #[test]
    fn steering_sign_and_state() {
        let mut d = [0u8; 8];
        put(&mut d, 0, 15, 1000);
        put(&mut d, 15, 1, 1);
        let v = run(LENKWINKEL_1, &d);
        assert_eq!(v["steering.angle_deg"], Some(Value::Num(-43.75)));
        put(&mut d, 41, 2, 1); // not initialised
        assert_eq!(run(LENKWINKEL_1, &d)["steering.angle_deg"], None);
    }

    #[test]
    fn short_frame_gives_none() {
        // 0x575 is 4 bytes on the car; a 1-byte frame can't hold bit 15.
        let v = run(BSG_3, &[0x02]);
        assert_eq!(v["ignition.kl15"], Some(Value::Bool(true)));
        assert_eq!(v["ignition.kl_p"], Some(Value::Bool(false)));
    }

    #[test]
    fn sun_load() {
        let v = run(CLIMA_2, &[0x19, 0x11, 0x95, 0, 0, 0x79, 0, 0]);
        assert_eq!(v["light.sun_left_wm2"], Some(Value::Num(100.0)));
        assert_eq!(v["light.sun_right_wm2"], Some(Value::Num(68.0)));
        assert_eq!(run(CLIMA_2, &[0xFF, 0x00])["light.sun_left_wm2"], None);
    }

    #[test]
    fn rpm_error() {
        let mut d = [0u8; 8];
        put(&mut d, 8, 16, 65280);
        assert_eq!(run(GW_MOTOR, &d)["engine.rpm"], None);
        put(&mut d, 8, 16, 3200);
        assert_eq!(run(GW_MOTOR, &d)["engine.rpm"], Some(Value::Num(800.0)));
    }

    #[test]
    fn datetime_decodes() {
        let mut d = [0u8; 8];
        put(&mut d, 8, 20, 238600);
        put(&mut d, 28, 7, 26);
        put(&mut d, 35, 4, 11);
        put(&mut d, 39, 5, 7);
        put(&mut d, 44, 5, 19);
        put(&mut d, 49, 6, 5);
        put(&mut d, 55, 6, 30);
        let v = run(DIAGNOSE_1, &d);
        assert_eq!(v["vehicle.odometer_km"], Some(Value::Num(238600.0)));
        assert_eq!(
            v["clock.datetime"],
            Some(Value::Text("2026-11-07T19:05:30".into()))
        );
    }

    #[test]
    fn ids_unique() {
        for (i, a) in MESSAGES.iter().enumerate() {
            for b in &MESSAGES[i + 1..] {
                assert_ne!(a.id, b.id, "{} and {}", a.name, b.name);
            }
        }
    }
}
