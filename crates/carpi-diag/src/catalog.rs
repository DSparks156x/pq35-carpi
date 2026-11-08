//! Value catalog: one name per value, and the places it can come from.
//!
//! A Rust table, not a TOML file: it's ~40 rows, the compiler checks every
//! row, there's no file to ship out of step with the binary, and a sans-IO
//! crate shouldn't read files. Per-ECU group tables (keyed by `1A 9B` part
//! number) can become data later if they grow.
//!
//! Refs:
//! - `[pdf]` owner's "Measuring Groups - Engine - 01.pdf", via RNS-E-Hudiy
//!   `vehicle_data/catalog.py` and `engine_definitions.py`. A generic layout,
//!   **not** keyed to Drew's ECU part number. Groups differ by ECU software, so
//!   every `[pdf]` location is a guess on Drew's MED17.5.1 until a capture says
//!   otherwise.
//! - `[trace]` RNS-E-Hudiy `references/logs/2026-06-17/tp2_worker_2.log`: Drew's
//!   engine answered groups 3 and 113 with formulas whose units match the
//!   `[pdf]` meaning (rpm, g/s, %, deg / rpm, %, %, mbar). Meaning is still from
//!   the PDF; the trace only shows the unit lines up.
//! - `[awd]` "Measuring Groups - AWD - 22.pdf" via `vehicle_data/consumer_definitions.py`.
//!   The PDF is headed 22 (VCDS address); the old code polled TP2 0x0A. The
//!   install list should settle it; until then 0x0A is a guess.

use carpi_kwp::group::{self, Record};
use carpi_state::Value;
use std::collections::BTreeMap;

/// Where a diag location's meaning comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// `[pdf]`. Guessed for Drew's ECU.
    EnginePdf,
    /// `[pdf]` and the unit matches `[trace]`.
    EnginePdfTrace,
    /// `[awd]`. Guessed, address unreconciled.
    AwdPdf,
    /// Tests and hand-made tables.
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// A value carpi-signals already puts in the store under this name. A value
    /// with an ICAN provider is never polled (ours: ICAN is always there when
    /// the car is awake, and diag would fight it for the same store name).
    Ican(&'static str),
    /// Measuring block `21 group`, one-based `field` (as the old catalog).
    Diag {
        module: u8,
        group: u8,
        field: u8,
        basis: Basis,
    },
}

/// Unit check on decoded diag values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Take whatever the ECU's formula byte says.
    Reported,
    /// Anything else decodes to nothing: wrong field for this ECU, most likely.
    /// Strings as `carpi_kwp::group::decode` emits them (`°C`, `mbar`, ...).
    Expect(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValueDef {
    /// Dotted store name.
    pub name: &'static str,
    pub unit: Unit,
    /// Decode diag fields with this formula instead of the ECU's formula byte.
    /// For ECUs that send a wrong one. None in the seed.
    pub formula: Option<u8>,
    /// In preference order. Ties in the scheduler go to the earlier one.
    pub providers: Vec<Provider>,
}

impl ValueDef {
    pub fn new(name: &'static str, unit: Unit) -> Self {
        ValueDef {
            name,
            unit,
            formula: None,
            providers: Vec::new(),
        }
    }
    pub fn ican(mut self, store_name: &'static str) -> Self {
        self.providers.push(Provider::Ican(store_name));
        self
    }
    pub fn diag(mut self, module: u8, group: u8, field: u8, basis: Basis) -> Self {
        self.providers.push(Provider::Diag {
            module,
            group,
            field,
            basis,
        });
        self
    }
    pub fn formula(mut self, f: u8) -> Self {
        self.formula = Some(f);
        self
    }

    /// First ICAN provider, if any.
    pub fn ican_name(&self) -> Option<&'static str> {
        self.providers.iter().find_map(|p| match p {
            Provider::Ican(n) => Some(*n),
            _ => None,
        })
    }

    /// Decode one measuring-block record for this value. `None` for unknown
    /// formulas and unit mismatches.
    pub fn decode(&self, rec: &Record) -> Option<Value> {
        let d = group::decode(self.formula.unwrap_or(rec.formula), rec.a, rec.b)?;
        if let Unit::Expect(u) = self.unit
            && d.unit != u
        {
            return None;
        }
        Some(match d.value {
            group::Value::Number(n) => Value::Num(n),
            group::Value::Text(t) => Value::Text(t),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    Duplicate(&'static str),
    /// Fields are one-based.
    ZeroField(&'static str),
    NoProviders(&'static str),
}

#[derive(Clone, Debug)]
pub struct Catalog {
    defs: Vec<ValueDef>,
    index: BTreeMap<&'static str, usize>,
}

impl Catalog {
    pub fn new(defs: Vec<ValueDef>) -> Result<Self, CatalogError> {
        let mut index = BTreeMap::new();
        for (i, d) in defs.iter().enumerate() {
            if d.providers.is_empty() {
                return Err(CatalogError::NoProviders(d.name));
            }
            if d.providers
                .iter()
                .any(|p| matches!(p, Provider::Diag { field: 0, .. }))
            {
                return Err(CatalogError::ZeroField(d.name));
            }
            if index.insert(d.name, i).is_some() {
                return Err(CatalogError::Duplicate(d.name));
            }
        }
        Ok(Catalog { defs, index })
    }

    /// The built-in table below.
    pub fn seed() -> Self {
        Catalog::new(seed()).expect("seed catalog is valid")
    }

    pub fn get(&self, name: &str) -> Option<&ValueDef> {
        self.index.get(name).map(|&i| &self.defs[i])
    }
    pub(crate) fn index_of(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }
    pub(crate) fn at(&self, i: usize) -> &ValueDef {
        &self.defs[i]
    }
    pub fn iter(&self) -> impl Iterator<Item = &ValueDef> {
        self.defs.iter()
    }
    pub fn len(&self) -> usize {
        self.defs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
}

/// TP2 ID of the engine. Proven `[trace]`.
pub const ENGINE: u8 = 0x01;
/// TP2 ID the old code used for the Haldex. Guess, see `[awd]`.
pub const HALDEX: u8 = 0x0A;

/// The seed table. Modest on purpose: the values the old dashboards used,
/// with ICAN names that carpi-signals already emits.
pub fn seed() -> Vec<ValueDef> {
    use Basis::{AwdPdf, EnginePdf, EnginePdfTrace};
    use Unit::{Expect, Reported};
    const E: u8 = ENGINE;
    const H: u8 = HALDEX;
    let mut v = vec![
        // ICAN first; diag locations kept so the catalog says where else it lives.
        ValueDef::new("engine.rpm", Expect("rpm"))
            .ican("engine.rpm")
            .diag(E, 3, 1, EnginePdfTrace)
            .diag(E, 113, 1, EnginePdfTrace),
        ValueDef::new("engine.coolant_c", Expect("°C"))
            .ican("engine.coolant_c")
            .diag(E, 1, 2, EnginePdf)
            .diag(E, 4, 3, EnginePdf),
        ValueDef::new("vehicle.speed_kmh", Expect("km/h"))
            .ican("vehicle.speed_kmh")
            .diag(E, 5, 3, EnginePdf),
        ValueDef::new("ambient.outside_temp_c", Reported).ican("ambient.outside_temp_c"),
        ValueDef::new("power.battery_v", Reported).ican("power.battery_v"),
        // Engine, diag only.
        ValueDef::new("engine.maf_gs", Expect("g/s"))
            .diag(E, 3, 2, EnginePdfTrace)
            .diag(E, 2, 4, EnginePdf),
        ValueDef::new("engine.throttle_pct", Expect("%"))
            .diag(E, 3, 3, EnginePdfTrace)
            .diag(E, 113, 3, EnginePdfTrace),
        ValueDef::new("engine.ignition_timing_deg", Expect("deg"))
            .diag(E, 3, 4, EnginePdfTrace)
            .diag(E, 10, 4, EnginePdf),
        ValueDef::new("engine.load_pct", Expect("%"))
            .diag(E, 113, 2, EnginePdfTrace)
            .diag(E, 115, 2, EnginePdf)
            .diag(E, 2, 2, EnginePdf),
        ValueDef::new("engine.atmospheric_mbar", Expect("mbar")).diag(E, 113, 4, EnginePdfTrace),
        // Absolute, per the PDF.
        ValueDef::new("engine.boost.actual", Expect("mbar"))
            .diag(E, 115, 4, EnginePdf)
            .diag(E, 118, 4, EnginePdf),
        ValueDef::new("engine.boost.specified", Expect("mbar"))
            .diag(E, 115, 3, EnginePdf)
            .diag(E, 117, 4, EnginePdf),
        ValueDef::new("engine.n75_duty_pct", Expect("%"))
            .diag(E, 114, 4, EnginePdf)
            .diag(E, 118, 3, EnginePdf),
        ValueDef::new("engine.intake_temp_c", Expect("°C"))
            .diag(E, 4, 4, EnginePdf)
            .diag(E, 6, 3, EnginePdf)
            .diag(E, 102, 3, EnginePdf),
        ValueDef::new("engine.oil_temp_c", Expect("°C")).diag(E, 134, 1, EnginePdf),
        // Old code accepted this only with a time unit; same here.
        ValueDef::new("engine.injection_ms", Expect("ms"))
            .diag(E, 2, 3, EnginePdf)
            .diag(E, 102, 4, EnginePdf),
        ValueDef::new("engine.fuel_rail.actual_bar", Expect("bar")).diag(E, 106, 2, EnginePdf),
        ValueDef::new("engine.fuel_rail.specified_bar", Expect("bar")).diag(E, 106, 1, EnginePdf),
        ValueDef::new("engine.pedal_pct", Expect("%"))
            .diag(E, 62, 3, EnginePdf)
            .diag(E, 63, 1, EnginePdf),
        // Unit not in the PDF: take the ECU's.
        ValueDef::new("engine.lambda.actual", Reported).diag(E, 31, 1, EnginePdf),
        ValueDef::new("engine.lambda.specified", Reported).diag(E, 31, 2, EnginePdf),
        // ECU's terminal 30; not the BCM battery reading on ICAN.
        ValueDef::new("engine.terminal30_v", Expect("V"))
            .diag(E, 4, 2, EnginePdf)
            .diag(E, 61, 2, EnginePdf),
        // Haldex. No Haldex trace exists, so units are whatever it reports.
        ValueDef::new("awd.oil_temp_c", Reported).diag(H, 1, 1, AwdPdf),
        ValueDef::new("awd.plate_temp_c", Reported).diag(H, 1, 2, AwdPdf),
        ValueDef::new("awd.supply_v", Reported).diag(H, 1, 3, AwdPdf),
        ValueDef::new("awd.oil_pressure_bar", Reported).diag(H, 3, 1, AwdPdf),
        // The old catalog flagged this one estimated.
        ValueDef::new("awd.estimated_torque_nm", Reported).diag(H, 3, 2, AwdPdf),
        ValueDef::new("awd.valve_opening_pct", Reported).diag(H, 3, 3, AwdPdf),
        ValueDef::new("awd.valve_current", Reported).diag(H, 3, 4, AwdPdf),
    ];
    // Knock retard per cylinder: group 20 fields 1-4 `[pdf]`.
    for (c, name) in [
        "engine.timing_retard.cyl1_deg",
        "engine.timing_retard.cyl2_deg",
        "engine.timing_retard.cyl3_deg",
        "engine.timing_retard.cyl4_deg",
    ]
    .into_iter()
    .enumerate()
    {
        v.push(ValueDef::new(name, Expect("deg")).diag(E, 20, c as u8 + 1, EnginePdf));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_is_valid() {
        let c = Catalog::seed();
        assert!(c.len() >= 30);
        assert_eq!(c.get("engine.rpm").unwrap().ican_name(), Some("engine.rpm"));
        assert!(c.get("engine.maf_gs").unwrap().ican_name().is_none());
    }

    #[test]
    fn rejects_bad_tables() {
        let d = || ValueDef::new("x", Unit::Reported).diag(1, 1, 1, Basis::Custom);
        assert_eq!(
            Catalog::new(vec![d(), d()]).unwrap_err(),
            CatalogError::Duplicate("x")
        );
        assert_eq!(
            Catalog::new(vec![ValueDef::new("y", Unit::Reported).diag(
                1,
                1,
                0,
                Basis::Custom
            )])
            .unwrap_err(),
            CatalogError::ZeroField("y")
        );
    }

    #[test]
    fn decode_checks_unit() {
        let c = Catalog::seed();
        // Group 113 field 4 from the trace: 12 7D 9F = 795 mbar.
        let rec = Record {
            formula: 0x12,
            a: 0x7D,
            b: 0x9F,
        };
        assert_eq!(
            c.get("engine.atmospheric_mbar").unwrap().decode(&rec),
            Some(Value::Num(795.0))
        );
        // Same bytes, wrong value: unit mismatch is nothing.
        assert_eq!(c.get("engine.maf_gs").unwrap().decode(&rec), None);
        // Formula override.
        let forced = ValueDef::new("z", Unit::Expect("rpm"))
            .diag(1, 1, 1, Basis::Custom)
            .formula(0x01);
        assert_eq!(
            forced.decode(&Record {
                formula: 0x7D,
                a: 0xC8,
                b: 0x15
            }),
            Some(Value::Num(840.0))
        );
    }
}
