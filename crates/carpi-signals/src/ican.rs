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
    Message {
        id: GATEWAY_1,
        name: "mGateway_1",
        cycle: ms(100),
        decode: gateway_1,
    },
    Message {
        id: MOTOR_7,
        name: "mMotor7",
        cycle: ms(100),
        decode: motor_7,
    },
    Message {
        id: MOTOR_10,
        name: "mMotor_10",
        cycle: ms(100),
        decode: motor_10,
    },
    Message {
        id: BEM_02,
        name: "mBEM_02",
        cycle: ms(100),
        decode: bem_02,
    },
    Message {
        id: CLIMA_1,
        name: "mClima_1",
        cycle: ms(100),
        decode: clima_1,
    },
    Message {
        id: CLIMA_3,
        name: "mClima_3",
        cycle: ms(200),
        decode: clima_3,
    },
    Message {
        id: EINHEITEN,
        name: "mEinheiten",
        cycle: ms(1000),
        decode: einheiten,
    },
    Message {
        id: MFA_1,
        name: "mMFA_1",
        cycle: ms(1000),
        decode: mfa_1,
    },
    Message {
        id: MFA_2,
        name: "mMFA_2",
        cycle: ms(1000),
        decode: mfa_2,
    },
    Message {
        id: MFA_3,
        name: "mMFA_3",
        cycle: ms(1000),
        decode: mfa_3,
    },
    Message {
        id: MFA_4,
        name: "mMFA_4",
        cycle: ms(1000),
        decode: mfa_4,
    },
    Message {
        id: MFA_5,
        name: "mMFA_5",
        cycle: ms(1000),
        decode: mfa_5,
    },
    Message {
        id: PARKHILFE_3,
        name: "mParkhilfe_3",
        cycle: ms(500),
        decode: parkhilfe_3,
    },
    // No cycle time in the DBC; 200 ms is ours.
    Message {
        id: PARKHILFE_5,
        name: "mParkhilfe_5",
        cycle: ms(200),
        decode: parkhilfe_5,
    },
    Message {
        id: NAVIG_1,
        name: "mNavig_1",
        cycle: ms(250),
        decode: navig_1,
    },
    // Custom firmware frames, not in the DBC. 0x679 is 50 Hz; a 0x6DA page
    // repeats at 6.25 Hz, so 160 ms covers the slowest one.
    Message {
        id: HALDEX_YAW,
        name: "Haldex 0x679",
        cycle: ms(20),
        decode: haldex_yaw,
    },
    Message {
        id: HALDEX_TELEMETRY,
        name: "Haldex 0x6DA",
        cycle: ms(160),
        decode: haldex_telemetry,
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

/// Two's-complement field, scaled.
fn signed(d: &[u8], start: u32, len: u32, factor: f64) -> Option<Value> {
    let raw = get(d, start, len)?;
    let v = if (raw >> (len - 1)) & 1 == 1 {
        raw as i64 - (1i64 << len)
    } else {
        raw as i64
    };
    Some(Value::Num(v as f64 * factor))
}

/// One reading shown in one of several units, picked by one-hot flag bits
/// (the MFA frames). Emits the value under the name whose flag is set and
/// `None` under the others; no flag set, or more than one, voids them all.
fn by_unit(d: &[u8], e: Emit, v: Option<Value>, units: &[(u32, &'static str)]) {
    let set: Vec<bool> = units
        .iter()
        .map(|(b, _)| bit(d, *b) == Some(true))
        .collect();
    let one = set.iter().filter(|s| **s).count() == 1;
    for ((_, name), on) in units.iter().zip(set) {
        e(name, if one && on { v.clone() } else { None });
    }
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
    e("brakes.abs_active", unless(d, 2, flag(d, 50)));
    e("brakes.esp_active", unless(d, 2, flag(d, 58)));
    e("trans.shifting", unless(d, 3, flag(d, 56)));
    e("trans.shift_lock", unless(d, 4, flag(d, 59)));
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
    e("engine.brake_switch", unless(d, 3, flag(d, 32)));
    e("engine.clutch_switch", unless(d, 3, flag(d, 35)));
    // Radiator fan command. Parked log: 0x18 = 9.6 %.
    e(
        "engine.fan_pct",
        unless(d, 2, num_max(d, 40, 8, 0.4, 0.0, 250)),
    );
    // 0 off/not fitted, 1 active, 2 driver override, 3 not allowed/fault.
    e(
        "engine.cruise_state",
        unless(d, 2, num(d, 54, 2, 1.0, 0.0, &[])),
    );
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
    // Parked log: filtered 0x8A = 19.0, unfiltered 0x8B = 19.5.
    e(
        "ambient.unfiltered_temp_c",
        unless(d, 2, num(d, 48, 8, 0.5, -50.0, &[0, 1, 255])),
    );
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
    // Parked log: 0x95 = 24.5 °C with 19 °C outside.
    e("climate.cabin_temp_c", num(d, 16, 8, 0.5, -50.0, &[255]));
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

/// 0x351, the gateway's other speed channel. The parked log is all zeros,
/// which only shows nothing is set at standstill.
// unverified: no moving frame
fn gateway_1(d: &[u8], e: Emit) {
    e(
        "vehicle.speed_gw_kmh",
        unless(d, 0, num_max(d, 9, 15, 0.01, 0.0, 32639)),
    );
    e("lights.reverse_gw", flag(d, 1));
}

/// 0x555, engine info. Parked log `E0 00 6A 00 2A 00 00 48`, engine off and
/// cold: oil 72 - 60 = 12 °C (coolant on 0x35B is 12 °C too), boost 42 x 20
/// = 840 mbar (absolute: ambient with the engine off), altitude 106/128.
fn motor_7(d: &[u8], e: Emit) {
    e("engine.fault_stored", flag(d, 3));
    // 0 "not fitted", 1 init, 255 error; bit 4 is the oil sensor's fault flag.
    e(
        "engine.oil_temp_c",
        unless(d, 4, num(d, 56, 8, 1.0, -60.0, &[0, 1, 255])),
    );
    e("engine.generator_pct", num(d, 8, 8, 0.4, 0.0, &[255]));
    // 1.0 = 1013 mbar outside (DBC). Not altitude in metres.
    e(
        "engine.altitude_factor",
        num(d, 16, 8, 1.0 / 128.0, 0.0, &[255]),
    );
    // The cluster's boost gauge signal: absolute, 20 mbar steps. Only the
    // engine-off point is checked; on boost it's unverified.
    e(
        "engine.boost.cluster_mbar",
        num(d, 32, 8, 20.0, 0.0, &[255]),
    );
    e("engine.extra_cooling", flag(d, 45));
    // 0 none, 1 13.3 V, 2 12.6 V, 3 11.8 V (emergency).
    e("power.load_shed_stage", num(d, 48, 2, 1.0, 0.0, &[]));
}

/// 0x58C, engine 10. Parked log `00 00 2E 00 ...`: 46 x 18 = 828 mbar with
/// the engine off, where 0x555 says ~840 mbar outside. So it reads
/// near-absolute at rest, not "vacuum below ambient" as the old catalog
/// had it. What it reads running is unverified.
fn motor_10(d: &[u8], e: Emit) {
    e("engine.manifold_mbar", num(d, 16, 6, 18.0, 0.0, &[63]));
    e("engine.overrun_cutoff", flag(d, 12));
}

/// 0x658, energy management. Parked log `70 00 00 0E 40 00 00 00`: state of
/// charge raw 14 (init) and the LIN-invalid bit set, so the TT has no
/// battery monitor behind it: all None there.
fn bem_02(d: &[u8], e: Emit) {
    e("power.battery_soc_pct", num_max(d, 24, 4, 10.0, 0.0, 10));
    e(
        "power.bdm_v",
        unless(d, 38, num(d, 40, 8, 0.05, 5.0, &[255])),
    );
    e(
        "power.generator_target_v",
        unless(d, 38, num(d, 48, 6, 0.1, 10.6, &[63])),
    );
    // 0 none, 1 start-stop not allowed, 2 battery disconnected, 3 weak.
    e("power.battery_warning", num(d, 28, 3, 1.0, 0.0, &[]));
    // 0 none, 1 charge lamp, 2 generator fault.
    e("power.generator_warning", num(d, 16, 2, 1.0, 0.0, &[]));
}

/// 0x3E1, climatronic 1. Parked log `20 8B 1D 02 58 00 81 00`: outside
/// 19.5 °C (0x527 says 19.0), refrigerant 5.8 bar at rest, blower 35.2 %,
/// Fahrenheit display on.
fn clima_1(d: &[u8], e: Emit) {
    e("climate.rear_window_heat", flag(d, 2));
    e("climate.compressor_on", flag(d, 4));
    e("climate.outside_temp_c", num(d, 8, 8, 0.5, -50.0, &[255]));
    e(
        "climate.refrigerant_bar",
        unless(d, 7, num(d, 16, 8, 0.2, 0.0, &[255])),
    );
    e(
        "climate.compressor_nm",
        unless(d, 6, num(d, 24, 8, 0.25, 0.0, &[255])),
    );
    e("climate.blower_pct", num_max(d, 32, 8, 0.4, 0.0, 250));
    e("climate.fan_pct", num_max(d, 40, 8, 0.4, 0.0, 250));
    e("climate.fahrenheit", flag(d, 48));
    e("climate.ac_on", flag(d, 49));
    e("climate.fault_stored", flag(d, 55));
}

/// 0x5E1, climatronic switches. DLC 7.
// unverified: not in the parked log
fn clima_3(d: &[u8], e: Emit) {
    e("climate.recirculation", flag(d, 0));
    e("climate.defrost", flag(d, 4));
    e("climate.econ", flag(d, 6));
    e("climate.on", flag(d, 7));
    e("climate.aux_heat", flag(d, 23));
    e("climate.wheel_heat", flag(d, 46));
}

/// 0x60E, the cluster's display units. Parked log `C7 00`: miles, °F,
/// gallons, distance per volume (mpg), 12 h clock, month-first date: a US
/// cluster.
fn einheiten(d: &[u8], e: Emit) {
    e("units.distance_mi", flag(d, 0));
    e("units.temp_f", flag(d, 1));
    e("units.volume_gal", flag(d, 2));
    // false: distance per volume (mpg, km/l); true: volume per distance.
    e("units.consumption_per_distance", flag(d, 3));
    // 0 bar, 1 psi, 3 kPa.
    e("units.pressure", num(d, 4, 2, 1.0, 0.0, &[2]));
    e("units.date_month_first", flag(d, 6));
    e("units.clock_12h", flag(d, 7));
}

/// Consumption (bits 0-11, 0.1) under the unit flagged by bits 12-14:
/// l/100km, km/l, mpg. Which gallon the mpg is, the frame doesn't say.
fn consumption(d: &[u8], e: Emit, names: [&'static str; 3]) {
    let v = num(d, 0, 12, 0.1, 0.0, &[4095]);
    by_unit(d, e, v, &[(12, names[0]), (13, names[1]), (14, names[2])]);
}

/// 0x629, trip computer: instant consumption and range, in the units the
/// cluster shows. DLC 7.
// unverified: not in the parked log
fn mfa_1(d: &[u8], e: Emit) {
    consumption(
        d,
        e,
        [
            "fuel.consumption_l100km",
            "fuel.consumption_kml",
            "fuel.consumption_mpg",
        ],
    );
    let range = num(d, 16, 14, 1.0, 0.0, &[16383]);
    by_unit(d, e, range, &[(30, "fuel.range_km"), (31, "fuel.range_mi")]);
}

/// MFA 2 and 3 share a layout: consumption x3, distance km/mi, time,
/// average speed km/h / mph.
fn mfa_memory(d: &[u8], e: Emit, n: [&'static str; 8]) {
    consumption(d, e, [n[0], n[1], n[2]]);
    let dist = num(d, 16, 14, 1.0, 0.0, &[16383]);
    by_unit(d, e, dist, &[(30, n[3]), (31, n[4])]);
    e(n[5], num(d, 32, 14, 1.0, 0.0, &[16383]));
    let speed = num(d, 48, 14, 1.0, 0.0, &[16383]);
    by_unit(d, e, speed, &[(62, n[6]), (63, n[7])]);
}

/// 0x62B, trip memory 1 (since start).
// unverified: not in the parked log
fn mfa_2(d: &[u8], e: Emit) {
    mfa_memory(
        d,
        e,
        [
            "trip.short.consumption_l100km",
            "trip.short.consumption_kml",
            "trip.short.consumption_mpg",
            "trip.short.distance_km",
            "trip.short.distance_mi",
            "trip.short.time_min",
            "trip.short.speed_kmh",
            "trip.short.speed_mph",
        ],
    );
}

/// 0x62D, trip memory 2 (long term).
// unverified: not in the parked log
fn mfa_3(d: &[u8], e: Emit) {
    mfa_memory(
        d,
        e,
        [
            "trip.long.consumption_l100km",
            "trip.long.consumption_kml",
            "trip.long.consumption_mpg",
            "trip.long.distance_km",
            "trip.long.distance_mi",
            "trip.long.time_min",
            "trip.long.speed_kmh",
            "trip.long.speed_mph",
        ],
    );
}

/// 0x529, tank-cycle memory. Bit 15 picks the cycle, 1 this tank and 0 the
/// previous one; only that cycle's names are emitted. DLC 6.
// unverified: not in the parked log, and the bit-15 sense is the old
// catalog's reading
fn mfa_4(d: &[u8], e: Emit) {
    let n: [&'static str; 7] = match bit(d, 15) {
        Some(true) => [
            "trip.tank.consumption_l100km",
            "trip.tank.consumption_kml",
            "trip.tank.consumption_mpg",
            "trip.tank.distance_km",
            "trip.tank.distance_mi",
            "trip.tank.speed_kmh",
            "trip.tank.speed_mph",
        ],
        Some(false) => [
            "trip.tank_prev.consumption_l100km",
            "trip.tank_prev.consumption_kml",
            "trip.tank_prev.consumption_mpg",
            "trip.tank_prev.distance_km",
            "trip.tank_prev.distance_mi",
            "trip.tank_prev.speed_kmh",
            "trip.tank_prev.speed_mph",
        ],
        None => return,
    };
    consumption(d, e, [n[0], n[1], n[2]]);
    let dist = num(d, 16, 14, 1.0, 0.0, &[16383]);
    by_unit(d, e, dist, &[(30, n[3]), (31, n[4])]);
    let speed = num(d, 32, 14, 1.0, 0.0, &[16383]);
    by_unit(d, e, speed, &[(46, n[5]), (47, n[6])]);
}

/// 0x52B, driving time without a break (resets after 10 min off) and the
/// time left on the tank. DLC 7.
// unverified: not in the parked log
fn mfa_5(d: &[u8], e: Emit) {
    e("trip.drive_time_min", num(d, 0, 14, 1.0, 0.0, &[16383]));
    e("fuel.time_left_min", num(d, 16, 14, 1.0, 0.0, &[16383]));
}

/// 0x545, parking aid state. DLC 4.
// unverified: not in the parked log
fn parkhilfe_3(d: &[u8], e: Emit) {
    e("parking.fault", flag(d, 1));
    e("parking.button_on", flag(d, 2));
    e("parking.reverse_on", flag(d, 3));
    e("parking.ready", flag(d, 24));
}

/// 0x54B, parking distances, cm. 255 is "nothing in range", emitted as None.
// unverified: not in the parked log
fn parkhilfe_5(d: &[u8], e: Emit) {
    const SENSORS: [&str; 8] = [
        "parking.fl_cm",
        "parking.fr_cm",
        "parking.rl_cm",
        "parking.rr_cm",
        "parking.fml_cm",
        "parking.fmr_cm",
        "parking.rml_cm",
        "parking.rmr_cm",
    ];
    for (i, name) in SENSORS.into_iter().enumerate() {
        e(name, num(d, i as u32 * 8, 8, 1.0, 0.0, &[255]));
    }
}

/// 0x2A1, measured yaw rate from the nav unit, which the Haldex firmware
/// also uses. Sign bit 15: the old catalog took 0 as negative (opposite of
/// the steering angle); the DBC doesn't say. DLC 7.
// unverified: not in the parked log, sign convention unchecked
fn navig_1(d: &[u8], e: Emit) {
    let mag = unless(d, 14, num(d, 0, 14, 0.01, 0.0, &[16383]));
    let yaw = match (mag, bit(d, 15)) {
        (Some(Value::Num(v)), Some(pos)) => Some(Value::Num(if pos { v } else { -v })),
        _ => None,
    };
    e("vehicle.yaw_dps", yaw);
}

/// Haldex firmware yaw scale: counts per deg/s.
const HALDEX_YAW_SCALE: f64 = 17.87;

/// 0x679, custom Haldex firmware: model yaw (s16), C06 proactive reference,
/// C22 pre-reference, B08 commanded torque (1/16 Nm). Layout from
/// `HaldexRE/phase2_can_telemetry_mux.py`.
// unverified: no capture of the custom firmware on the car
fn haldex_yaw(d: &[u8], e: Emit) {
    if d.len() != 8 {
        return;
    }
    e(
        "awd.model_yaw_dps",
        signed(d, 0, 16, 1.0 / HALDEX_YAW_SCALE),
    );
    e("awd.c06_raw", num(d, 16, 16, 1.0, 0.0, &[]));
    e("awd.c22_raw", num(d, 32, 16, 1.0, 0.0, &[]));
    e("awd.commanded_nm", num(d, 48, 16, 0.0625, 0.0, &[]));
}

/// 0x6DA, custom Haldex firmware, paged: byte 0 `0xD0 | page`, byte 1 status
/// (mode, B1CC, A78, token, ABS), then three little-endian words. Frames
/// without the header are dropped. Layout from
/// `HaldexRE/phase2_can_telemetry_mux.py`.
// unverified: no capture of the custom firmware on the car
fn haldex_telemetry(d: &[u8], e: Emit) {
    if d.len() != 8 || d[0] & 0xF8 != 0xD0 {
        return;
    }
    // The mode the Haldex applied, not the one asked for.
    let mode = match d[1] & 3 {
        0 => "Stock",
        1 => "Performance",
        2 => "Competition",
        _ => "Unknown",
    };
    e("awd.mode", Some(Value::Text(mode.into())));
    e("awd.selector_b1cc", num(d, 10, 3, 1.0, 0.0, &[]));
    e("awd.force_zero", flag(d, 13));
    e("awd.token_ok", flag(d, 14));
    e("awd.abs_braking", flag(d, 15));
    let u = |w: u32, f: f64| num(d, 16 + 16 * w, 16, f, 0.0, &[]);
    let s = |w: u32, f: f64| signed(d, 16 + 16 * w, 16, f);
    match d[0] & 7 {
        0 => {
            e("awd.ceiling_nm", u(0, 0.0625));
            e("awd.reference_nm", u(1, 0.0625));
            e("awd.slip_nm", u(2, 0.0625));
        }
        1 => {
            e("awd.accel_demand_raw", s(0, 1.0));
            e("awd.accel_actual_raw", s(1, 1.0));
            e("awd.measured_yaw_dps", s(2, 1.0 / HALDEX_YAW_SCALE));
        }
        2 => {
            e("awd.lateral_ff_raw", s(0, 1.0));
            e("awd.curvature_raw", u(1, 1.0));
            e("awd.axle_slip_raw", s(2, 1.0));
        }
        3 => {
            e("wheels.fl_kmh", u(0, 0.005));
            e("wheels.fr_kmh", u(1, 0.005));
            e("wheels.rl_kmh", u(2, 0.005));
        }
        4 => {
            e("wheels.rr_kmh", u(0, 0.005));
            e("awd.lateral_accel_raw", s(1, 1.0));
            e("awd.throttle_raw", num(d, 48, 8, 1.0, 0.0, &[]));
            e("awd.brake_light_raw", num(d, 56, 8, 1.0, 0.0, &[]));
        }
        5 => {
            e("awd.hold_timer", u(0, 1.0));
            e("awd.high_gear_factor_raw", u(1, 1.0));
            // Target/input gear, not proof of the engaged gear.
            e("awd.target_gear", num(d, 48, 8, 1.0, 0.0, &[]));
        }
        6 => {
            e("awd.slip_energy_raw", u(0, 1.0));
            e("awd.energy_ceiling_raw", u(1, 1.0));
            e("awd.derate_ceiling_raw", u(2, 1.0));
        }
        _ => {}
    }
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

    /// `Some(Num)` within 1e-9 of `want`.
    fn near(v: &Option<Value>, want: f64) -> bool {
        matches!(v, Some(Value::Num(x)) if (x - want).abs() < 1e-9)
    }

    // Frames from the parked, ignition-on, engine-off log
    // (testdata/canlog-parked-kl15.txt).

    #[test]
    fn motor_7_parked() {
        let v = run(MOTOR_7, &[0xE0, 0x00, 0x6A, 0x00, 0x2A, 0x00, 0x00, 0x48]);
        assert!(near(&v["engine.oil_temp_c"], 12.0));
        assert!(near(&v["engine.boost.cluster_mbar"], 840.0));
        assert!(near(&v["engine.generator_pct"], 0.0));
        assert!(near(&v["engine.altitude_factor"], 106.0 / 128.0));
        assert_eq!(v["engine.fault_stored"], Some(Value::Bool(false)));
        // Oil sensor fault flag voids the temperature.
        let v = run(MOTOR_7, &[0xF0, 0x00, 0x6A, 0x00, 0x2A, 0x00, 0x00, 0x48]);
        assert_eq!(v["engine.oil_temp_c"], None);
    }

    #[test]
    fn motor_10_parked() {
        let v = run(MOTOR_10, &[0x00, 0x00, 0x2E, 0, 0, 0, 0, 0]);
        assert!(near(&v["engine.manifold_mbar"], 828.0));
        assert_eq!(v["engine.overrun_cutoff"], Some(Value::Bool(false)));
    }

    #[test]
    fn bem_02_parked_has_no_battery_monitor() {
        let v = run(BEM_02, &[0x70, 0x00, 0x00, 0x0E, 0x40, 0x00, 0x00, 0x00]);
        assert_eq!(v["power.battery_soc_pct"], None);
        assert_eq!(v["power.bdm_v"], None);
        assert_eq!(v["power.generator_target_v"], None);
        assert!(near(&v["power.battery_warning"], 0.0));
        // A real state of charge: raw 7 = 70 %.
        let v = run(BEM_02, &[0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00]);
        assert!(near(&v["power.battery_soc_pct"], 70.0));
    }

    #[test]
    fn clima_1_parked() {
        let v = run(CLIMA_1, &[0x20, 0x8B, 0x1D, 0x02, 0x58, 0x00, 0x81, 0x00]);
        assert!(near(&v["climate.outside_temp_c"], 19.5));
        assert!(near(&v["climate.refrigerant_bar"], 5.8));
        assert!(near(&v["climate.compressor_nm"], 0.5));
        assert!(near(&v["climate.blower_pct"], 35.2));
        assert!(near(&v["climate.fan_pct"], 0.0));
        assert_eq!(v["climate.fahrenheit"], Some(Value::Bool(true)));
        assert_eq!(v["climate.ac_on"], Some(Value::Bool(false)));
        assert_eq!(v["climate.compressor_on"], Some(Value::Bool(false)));
    }

    #[test]
    fn clima_2_cabin_parked() {
        let v = run(CLIMA_2, &[0x19, 0x11, 0x95, 0, 0, 0x79, 0, 0]);
        assert!(near(&v["climate.cabin_temp_c"], 24.5));
    }

    #[test]
    fn units_parked_us_cluster() {
        let v = run(EINHEITEN, &[0xC7, 0x00]);
        for k in [
            "units.distance_mi",
            "units.temp_f",
            "units.volume_gal",
            "units.date_month_first",
            "units.clock_12h",
        ] {
            assert_eq!(v[k], Some(Value::Bool(true)), "{k}");
        }
        assert_eq!(
            v["units.consumption_per_distance"],
            Some(Value::Bool(false))
        );
        assert!(near(&v["units.pressure"], 0.0));
    }

    #[test]
    fn gateway_1_parked() {
        let v = run(GATEWAY_1, &[0; 8]);
        assert!(near(&v["vehicle.speed_gw_kmh"], 0.0));
        assert_eq!(v["lights.reverse_gw"], Some(Value::Bool(false)));
    }

    #[test]
    fn gw_kombi_unfiltered_parked() {
        let v = run(GW_KOMBI, &[0x10, 0x01, 0x00, 0x40, 0x7D, 0x8A, 0x8B, 0x00]);
        assert!(near(&v["ambient.outside_temp_c"], 19.0));
        assert!(near(&v["ambient.unfiltered_temp_c"], 19.5));
    }

    #[test]
    fn bremse_getriebe_flags_parked() {
        let v = run(
            GW_BREMSE_GETRIEBE,
            &[0x20, 0x01, 0x00, 0x00, 0x00, 0x6B, 0x40, 0x88],
        );
        assert!(near(&v["trans.selector"], 8.0)); // P
        assert_eq!(v["trans.shift_lock"], Some(Value::Bool(true)));
        assert_eq!(v["trans.shifting"], Some(Value::Bool(false)));
        assert_eq!(v["brakes.abs_active"], Some(Value::Bool(false)));
        assert_eq!(v["brakes.esp_active"], Some(Value::Bool(false)));
        // EPB "Alt" bit 5 is set in this frame.
        assert_eq!(v["brakes.epb_closed"], None);
    }

    #[test]
    fn gw_motor_extras_parked() {
        let v = run(GW_MOTOR, &[0x08, 0x00, 0x00, 0x50, 0x20, 0x18, 0x00, 0x00]);
        assert!(near(&v["engine.coolant_c"], 12.0));
        assert!(near(&v["engine.fan_pct"], 9.6));
        assert!(near(&v["engine.cruise_state"], 0.0));
        // Switch "Alt" bit 3 is set in this frame.
        assert_eq!(v["engine.brake_switch"], None);
    }

    // Synthetic frames, built from the DBC: these decoders have no real frame.

    #[test]
    fn mfa_units_pick_one_name() {
        let mut d = [0u8; 7];
        put(&mut d, 0, 12, 284); // 28.4
        put(&mut d, 14, 1, 1); // mpg
        put(&mut d, 16, 14, 312);
        put(&mut d, 31, 1, 1); // miles
        let v = run(MFA_1, &d);
        assert!(near(&v["fuel.consumption_mpg"], 28.4));
        assert_eq!(v["fuel.consumption_l100km"], None);
        assert!(near(&v["fuel.range_mi"], 312.0));
        assert_eq!(v["fuel.range_km"], None);
        // Two unit flags at once: nothing.
        put(&mut d, 30, 1, 1);
        assert_eq!(run(MFA_1, &d)["fuel.range_mi"], None);
    }

    #[test]
    fn mfa_memory_and_tank_cycle() {
        let mut d = [0u8; 8];
        put(&mut d, 12, 1, 1);
        put(&mut d, 0, 12, 95);
        put(&mut d, 16, 14, 42);
        put(&mut d, 30, 1, 1);
        put(&mut d, 32, 14, 37);
        put(&mut d, 48, 14, 68);
        put(&mut d, 62, 1, 1);
        let v = run(MFA_2, &d);
        assert!(near(&v["trip.short.consumption_l100km"], 9.5));
        assert!(near(&v["trip.short.distance_km"], 42.0));
        assert!(near(&v["trip.short.time_min"], 37.0));
        assert!(near(&v["trip.short.speed_kmh"], 68.0));

        let mut t = [0u8; 6];
        put(&mut t, 16, 14, 410);
        put(&mut t, 30, 1, 1);
        put(&mut t, 15, 1, 0);
        let v = run(MFA_4, &t);
        assert!(near(&v["trip.tank_prev.distance_km"], 410.0));
        assert!(!v.contains_key("trip.tank.distance_km"));
        put(&mut t, 15, 1, 1);
        assert!(near(&run(MFA_4, &t)["trip.tank.distance_km"], 410.0));
    }

    #[test]
    fn parking_255_is_nothing() {
        let v = run(PARKHILFE_5, &[30, 255, 0, 0, 0, 0, 0, 0]);
        assert!(near(&v["parking.fl_cm"], 30.0));
        assert_eq!(v["parking.fr_cm"], None);
    }

    #[test]
    fn yaw_sign_and_fault() {
        let mut d = [0u8; 7];
        put(&mut d, 0, 14, 1234);
        let v = run(NAVIG_1, &d);
        assert!(near(&v["vehicle.yaw_dps"], -12.34));
        put(&mut d, 15, 1, 1);
        assert!(near(&run(NAVIG_1, &d)["vehicle.yaw_dps"], 12.34));
        put(&mut d, 14, 1, 1);
        assert_eq!(run(NAVIG_1, &d)["vehicle.yaw_dps"], None);
    }

    #[test]
    fn haldex_pages() {
        // Page 1, Performance, token ok; measured yaw -1787 counts = -100 deg/s.
        let yaw = (-1787i16) as u16;
        let d = [
            0xD1,
            0x41,
            0x10,
            0x00,
            0xF0,
            0xFF,
            yaw as u8,
            (yaw >> 8) as u8,
        ];
        let v = run(HALDEX_TELEMETRY, &d);
        assert_eq!(v["awd.mode"], Some(Value::Text("Performance".into())));
        assert_eq!(v["awd.token_ok"], Some(Value::Bool(true)));
        assert!(near(&v["awd.accel_demand_raw"], 16.0));
        assert!(near(&v["awd.accel_actual_raw"], -16.0));
        assert!(near(&v["awd.measured_yaw_dps"], -100.0));
        // Page 3: wheel speeds, 0.005 km/h.
        let v = run(HALDEX_TELEMETRY, &[0xD3, 0, 0x10, 0x27, 0, 0, 0, 0]);
        assert!(near(&v["wheels.fl_kmh"], 50.0));
        // Old untagged layout: dropped.
        assert!(run(HALDEX_TELEMETRY, &[0x12, 0, 0, 0, 0, 0, 0, 0]).is_empty());
        let v = run(HALDEX_YAW, &[0, 0, 0, 0, 0, 0, 0x40, 0x06]);
        assert!(near(&v["awd.commanded_nm"], 100.0));
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
