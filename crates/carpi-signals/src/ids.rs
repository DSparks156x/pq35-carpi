//! Every CAN ID in one place. Valid as written for A3 8P / TT 8J / R8 / Q3
//! ICAN; forks for more distant relatives change this file.

// --- Network management (OSEK ring, ICAN) ---------------------------------
/// NM ring IDs are `NM_BASE + node address`.
pub const NM_BASE: u32 = 0x420;
pub const NM_LAST: u32 = 0x43F;
/// Gateway node address (0x42B).
pub const NM_NODE_GATEWAY: u8 = 0x0B;
/// RNS-E node address (0x436).
pub const NM_NODE_RNSE: u8 = 0x16;

// --- Ignition / body -------------------------------------------------------
pub const ZAS_STATUS: u32 = 0x2C3;
pub const BSG_3: u32 = 0x575;
pub const BSG_2: u32 = 0x571;
pub const BSG_KOMBI: u32 = 0x470;

// --- Drivetrain / chassis --------------------------------------------------
pub const GW_BREMSE_GETRIEBE: u32 = 0x359;
pub const GW_MOTOR: u32 = 0x35B;
pub const LENKWINKEL_1: u32 = 0x3C3;
pub const BREMSE_10: u32 = 0x5B5;
/// mGateway_1: second gateway speed, reverse light.
pub const GATEWAY_1: u32 = 0x351;
/// mMotor7: oil temperature, boost, altitude factor, generator.
pub const MOTOR_7: u32 = 0x555;
/// mMotor_10: manifold pressure, overrun cutoff.
pub const MOTOR_10: u32 = 0x58C;
/// mNavig_1: measured yaw rate, sent by the nav unit.
pub const NAVIG_1: u32 = 0x2A1;
/// Custom Haldex firmware (7316+): fixed model-yaw/torque frame, 50 Hz.
pub const HALDEX_YAW: u32 = 0x679;
/// Custom Haldex firmware: paged telemetry, byte 0 = `0xD0 | page`.
pub const HALDEX_TELEMETRY: u32 = 0x6DA;

// --- Trip computer (MFA, gateway to the RNS-E) -------------------------------
/// Instant consumption, range.
pub const MFA_1: u32 = 0x629;
/// Short-term memory (since start).
pub const MFA_2: u32 = 0x62B;
/// Long-term memory.
pub const MFA_3: u32 = 0x62D;
/// Tank-cycle memory, current/previous multiplexed on bit 15.
pub const MFA_4: u32 = 0x529;
/// Driving time, time to refuel.
pub const MFA_5: u32 = 0x52B;
/// mEinheiten: display units the cluster is set to.
pub const EINHEITEN: u32 = 0x60E;

// --- Electrical, climate, parking ---------------------------------------------
/// mBEM_02: energy management, battery state of charge.
pub const BEM_02: u32 = 0x658;
/// mClima_1: climatronic outside temp, refrigerant, blower.
pub const CLIMA_1: u32 = 0x3E1;
/// mClima_3: climatronic switches.
pub const CLIMA_3: u32 = 0x5E1;
/// mParkhilfe_3: parking aid state.
pub const PARKHILFE_3: u32 = 0x545;
/// mParkhilfe_5: parking aid distances.
pub const PARKHILFE_5: u32 = 0x54B;

// --- Cluster / gateway info ------------------------------------------------
pub const GW_KOMBI: u32 = 0x527;
pub const KOMBI_K1: u32 = 0x621;
pub const DIMMUNG: u32 = 0x635;
/// mClima_2: sun load, climatronic dash-top sensor (via the gateway).
pub const CLIMA_2: u32 = 0x3E3;
pub const DIAGNOSE_1: u32 = 0x65D;

// --- Inputs ----------------------------------------------------------------
/// Steering wheel buttons (MFL).
pub const MFL_CMD: u32 = 0x5C3;
/// RNS-E faceplate/TV-panel keys, only sent while the RNS-E is on TV.
pub const RNSE_KEYS: u32 = 0x461;

// --- RNS-E firmware (SD-link firmware, CAN_PROTOCOL.md) ---------------------
pub const RNSE_CMD: u32 = 0x7B0;
pub const RNSE_REPLY: u32 = 0x7B6;
/// RNS-E's own DDP session, redirected away from the cluster.
pub const RNSE_DDP_TX: u32 = 0x7B2;
pub const RNSE_DDP_RX: u32 = 0x7B3;
/// RNS-E's own top-line text, redirected (sent only while on TV).
pub const RNSE_TOPLINE_1: u32 = 0x7B4;
pub const RNSE_TOPLINE_2: u32 = 0x7B5;
pub const RNSE_NAV_STATUS: u32 = 0x604;
/// mRadio_4: radio on/mode. Byte 3 is 0x37 while the TV source is active
/// (stock firmware too).
pub const RADIO_4: u32 = 0x661;

// --- Cluster display ---------------------------------------------------------
pub const DIS_TOPLINE_1: u32 = 0x363;
pub const DIS_TOPLINE_2: u32 = 0x365;
pub const DDP_TX: u32 = 0x6C0;
pub const DDP_RX: u32 = 0x6C1;

// --- Diagnostics (TP2.0) -----------------------------------------------------
pub const TP2_SETUP: u32 = 0x200;
/// Module replies to a setup on `TP2_SETUP_REPLY_BASE + module`.
pub const TP2_SETUP_REPLY_BASE: u32 = 0x200;
/// Gateway's TP2 address (answers `1A 9F`, the installation list).
pub const TP2_GATEWAY: u8 = 0x1F;
/// Our tester RX IDs. Never 0x300, which VCDS uses.
pub const TP2_TESTER_RX: std::ops::RangeInclusive<u32> = 0x308..=0x30F;
/// VCDS uses 0x300 as its tester RX ID. We never do.
pub const TP2_VCDS_TESTER_RX: u32 = 0x300;

pub fn is_nm(id: u32) -> bool {
    (NM_BASE..=NM_LAST).contains(&id)
}
