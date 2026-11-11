//! Value catalog: one name per value, and the places it can come from.
//!
//! A Rust table, not a TOML file: the compiler checks every row, there's no
//! file to ship out of step with the binary, and a sans-IO crate shouldn't
//! read files. Per-ECU group tables (keyed by `1A 9B` part number) can become
//! data later. The table itself is in [`seed`](self::seed()), ~830 rows: every
//! value carpi-signals emits, and the old project's diag values.
//!
//! Each value has a label and a place (category, sub-group) for the picker
//! (PAGES 2). The table's order is the picker's curated order.
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
//! - `[awd]` "Measuring Groups - AWD - 22" (owner's PDF, then the full sheet in
//!   owner's "Measuring Groups.xlsx"). VCDS 22 is TP2 0x0A on the install list.
//! - `[trans]` "Measuring Groups - Transmission - 02" (same sources): DSG on
//!   TP2 0x02. No trace from Drew's DSG yet.
//! - `[bcm]` "Measuring Groups - Central Electronic - 09" sheet of
//!   "Measuring Groups.xlsx". VCDS 09 is the BCM, TP2 0x20. No trace yet.
//!
//! Sheet rows with no label are left out. Where a sheet repeats a meaning
//! (selector lever position in groups 2-7, say), it is one value with every
//! location.

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
    /// `[trans]`. Guessed for Drew's DSG.
    TransmissionPdf,
    /// `[bcm]`. Guessed for Drew's BCM.
    CentralPdf,
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

/// Picker category (PAGES 2). Sub-groups are free text in the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Engine,
    Driving,
    Transmission,
    Haldex,
    FuelTrip,
    Electrical,
    Body,
    Lights,
    Rnse,
    /// Bridge values (nav, media, calls). None in this table: they come from
    /// the LIVI bridge, not a bus.
    PhoneNav,
    Other,
}

impl Category {
    /// In picker order.
    pub const ALL: [Category; 11] = [
        Category::Engine,
        Category::Driving,
        Category::Transmission,
        Category::Haldex,
        Category::FuelTrip,
        Category::Electrical,
        Category::Body,
        Category::Lights,
        Category::Rnse,
        Category::PhoneNav,
        Category::Other,
    ];

    /// What the picker shows.
    pub fn label(self) -> &'static str {
        match self {
            Category::Engine => "Engine",
            Category::Driving => "Driving",
            Category::Transmission => "Transmission",
            Category::Haldex => "Haldex",
            Category::FuelTrip => "Fuel & trip",
            Category::Electrical => "Electrical",
            Category::Body => "Body",
            Category::Lights => "Lights",
            Category::Rnse => "RNS-E",
            Category::PhoneNav => "Phone & nav",
            Category::Other => "Other",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValueDef {
    /// Dotted store name.
    pub name: &'static str,
    /// What the picker shows. The name itself when the table gives none.
    pub label: &'static str,
    pub category: Category,
    /// Sub-group header inside the category; empty for none.
    pub sub: &'static str,
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
            label: name,
            category: Category::Other,
            sub: "",
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
    pub fn label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }
    pub fn place(mut self, category: Category, sub: &'static str) -> Self {
        self.category = category;
        self.sub = sub;
        self
    }
    /// Engine `[pdf]` locations; groups 3 and 113 are `[trace]`.
    pub fn engine(mut self, locs: &[(u8, u8)]) -> Self {
        for &(group, field) in locs {
            let basis = if TRACE_GROUPS.contains(&group) {
                Basis::EnginePdfTrace
            } else {
                Basis::EnginePdf
            };
            self = self.diag(ENGINE, group, field, basis);
        }
        self
    }
    /// Transmission `[trans]` locations.
    pub fn trans(mut self, locs: &[(u8, u8)]) -> Self {
        for &(group, field) in locs {
            self = self.diag(TRANSMISSION, group, field, Basis::TransmissionPdf);
        }
        self
    }
    /// Central electronics `[bcm]` locations.
    pub fn bcm(mut self, locs: &[(u8, u8)]) -> Self {
        for &(group, field) in locs {
            self = self.diag(CENTRAL_ELECTRICS, group, field, Basis::CentralPdf);
        }
        self
    }
    /// Haldex `[awd]` locations.
    pub fn awd(mut self, locs: &[(u8, u8)]) -> Self {
        for &(group, field) in locs {
            self = self.diag(HALDEX, group, field, Basis::AwdPdf);
        }
        self
    }

    /// The unit to show: the expected one, or `""` when the ECU decides.
    pub fn unit_label(&self) -> &'static str {
        match self.unit {
            Unit::Expect(u) => u,
            Unit::Reported => "",
        }
    }

    /// Whether reading this value costs polling (no ICAN provider).
    pub fn is_diag(&self) -> bool {
        self.ican_name().is_none()
            && self
                .providers
                .iter()
                .any(|p| matches!(p, Provider::Diag { .. }))
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
    /// Adds a value at the end; an existing name keeps its index.
    pub(crate) fn push(&mut self, def: ValueDef) -> usize {
        if let Some(i) = self.index_of(def.name) {
            return i;
        }
        self.index.insert(def.name, self.defs.len());
        self.defs.push(def);
        self.defs.len() - 1
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
/// TP2 ID of the DSG (VCDS 02), from the install list.
pub const TRANSMISSION: u8 = 0x02;
/// TP2 ID of the Haldex (VCDS 22), from the install list.
pub const HALDEX: u8 = 0x0A;
/// TP2 ID of the central electronics / BCM (VCDS 09), from the install list.
pub const CENTRAL_ELECTRICS: u8 = 0x20;
/// Engine groups seen in `[trace]`.
const TRACE_GROUPS: [u8; 2] = [3, 113];

mod seed;

/// The seed table: every value carpi-signals emits, with its diag
/// locations where it has any, then the diag-only values.
pub fn seed() -> Vec<ValueDef> {
    seed::seed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_is_valid() {
        let c = Catalog::seed();
        assert!(c.len() >= 800);
        assert_eq!(c.get("engine.rpm").unwrap().ican_name(), Some("engine.rpm"));
        assert!(c.get("engine.maf_gs").unwrap().ican_name().is_none());
    }

    #[test]
    fn every_value_is_labelled_and_placed() {
        for d in Catalog::seed().iter() {
            assert!(!d.label.is_empty() && d.label != d.name, "{}", d.name);
            assert!(!d.sub.is_empty(), "{}", d.name);
            assert_ne!(d.category, Category::PhoneNav, "{}", d.name);
        }
    }

    #[test]
    fn table_is_in_picker_order() {
        // Category order, and each sub-group in one run.
        let c = Catalog::seed();
        let defs: Vec<_> = c.iter().collect();
        let rank = |cat| Category::ALL.iter().position(|&x| x == cat).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for w in defs.windows(2) {
            assert!(rank(w[0].category) <= rank(w[1].category), "{}", w[1].name);
            if (w[0].category, w[0].sub) != (w[1].category, w[1].sub) {
                assert!(seen.insert((w[0].category, w[0].sub)), "{} split", w[0].sub);
            }
        }
    }

    /// A watch subscribes names as prefixes, and every catalog value under a
    /// prefix gets a diag subscription. So no polled value may start with
    /// another name, or with a prefix the LIVI bridge subscribes
    /// (`livi-carpi/src/values.rs`).
    #[test]
    fn no_polled_value_hides_under_a_prefix() {
        const BRIDGE: [&str; 14] = [
            "vehicle.speed_kmh",
            "vehicle.odometer_km",
            "engine.rpm",
            "engine.coolant_c",
            "power.battery_v",
            "fuel.",
            "brakes.",
            "trans.selector",
            "lights.",
            "body.",
            "ambient.outside_temp_c",
            "steering.angle_deg",
            "light.night",
            "light.dim_display_pct",
        ];
        let c = Catalog::seed();
        let names: Vec<_> = c.iter().map(|d| d.name).collect();
        for d in c.iter().filter(|d| d.is_diag()) {
            for p in names.iter().copied().chain(BRIDGE) {
                assert!(
                    p == d.name || !d.name.starts_with(p),
                    "{} starts with {p}",
                    d.name
                );
            }
        }
    }

    #[test]
    fn modules_and_basis() {
        let c = Catalog::seed();
        let has = |name: &str, m: u8, b: Basis| {
            c.get(name).unwrap().providers.iter().any(
                |p| matches!(*p, Provider::Diag { module, basis, .. } if module == m && basis == b),
            )
        };
        assert!(has("engine.rpm", ENGINE, Basis::EnginePdfTrace));
        assert!(has("engine.rpm", ENGINE, Basis::EnginePdf));
        assert!(has(
            "trans.fluid_temp_c",
            TRANSMISSION,
            Basis::TransmissionPdf
        ));
        assert!(has(
            "trans.k1.pressure_bar",
            TRANSMISSION,
            Basis::TransmissionPdf
        ));
        assert!(has("awd.oil_temp_c", HALDEX, Basis::AwdPdf));
        assert!(has("awd.pump_current", HALDEX, Basis::AwdPdf));
        assert!(has(
            "trans.lever_position",
            TRANSMISSION,
            Basis::TransmissionPdf
        ));
        assert!(has(
            "bcm.quiescent_total_v",
            CENTRAL_ELECTRICS,
            Basis::CentralPdf
        ));
        assert!(has(
            "bcm.lamp_brake_high",
            CENTRAL_ELECTRICS,
            Basis::CentralPdf
        ));
        assert!(c.get("bcm.comfort_hist_10_log").unwrap().is_diag());
        // Oil temperature now has ICAN 0x555 first, so it's never polled.
        assert_eq!(
            c.get("engine.oil_temp_c").unwrap().ican_name(),
            Some("engine.oil_temp_c")
        );
        assert!(c.get("trans.fluid_temp_c").unwrap().is_diag());
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
