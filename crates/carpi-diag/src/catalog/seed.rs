//! The built-in table, in picker order: category, sub-group, then curated
//! (actual before specified, FL FR RL RR, cylinders 1-4).
//!
//! Generated once from RNS-E-Hudiy's exported catalog (`tools/export_value_catalog.py
//! --format json`) plus the names carpi-signals emits, then kept by hand.
//! Diag rows: every location the old catalog had except its tentative
//! group 11 fields 5-7. Labels are the old catalog's.
//!
//! Every diag location is `[pdf]` / `[trans]` / `[awd]` / `[bcm]`: from VCDS-style
//! PDFs, not keyed to Drew's ECU part number. Groups 3 and 113 on the
//! engine are `[trace]` (the unit lines up). Unit checks catch most wrong
//! guesses: a field that decodes to another unit decodes to nothing.
//!
//! Engine rows list `[trace]` groups first: earlier providers win ties in
//! the scheduler.
//!
//! Naming: no diag-polled name may start with another catalog name, or a
//! watch on the shorter one would poll it too (tested).

use super::Category::*;
use super::Unit::{Expect, Reported};
use super::{Category, Unit, ValueDef};

fn v(
    name: &'static str,
    label: &'static str,
    unit: Unit,
    category: Category,
    sub: &'static str,
) -> ValueDef {
    ValueDef::new(name, unit).label(label).place(category, sub)
}

pub(super) fn seed() -> Vec<ValueDef> {
    vec![
        v(
            "engine.rpm",
            "Engine speed",
            Expect("rpm"),
            Engine,
            "Basics",
        )
        .ican("engine.rpm")
        .engine(&[
            (3, 1),
            (113, 1),
            (1, 1),
            (2, 1),
            (4, 1),
            (5, 1),
            (6, 1),
            (7, 1),
            (10, 1),
            (11, 1),
            (13, 1),
            (14, 1),
            (22, 1),
            (23, 1),
            (28, 1),
            (34, 1),
            (43, 1),
            (46, 1),
            (50, 1),
            (51, 1),
            (52, 1),
            (53, 1),
            (54, 1),
            (55, 1),
            (56, 1),
            (57, 1),
            (61, 1),
            (68, 1),
            (91, 1),
            (93, 1),
            (94, 1),
            (99, 1),
            (101, 1),
            (102, 1),
            (107, 1),
            (110, 1),
            (115, 1),
            (116, 1),
            (117, 1),
            (118, 1),
            (119, 1),
            (120, 1),
            (122, 1),
            (143, 1),
        ]),
        v(
            "engine.load_pct",
            "Engine load (actual)",
            Expect("%"),
            Engine,
            "Basics",
        )
        .engine(&[
            (113, 2),
            (2, 2),
            (5, 2),
            (6, 2),
            (7, 2),
            (10, 2),
            (14, 2),
            (22, 2),
            (23, 2),
            (28, 2),
            (68, 2),
            (93, 2),
            (101, 2),
            (115, 2),
            (143, 2),
            (37, 1),
            (114, 3),
        ]),
        v(
            "engine.load_specified_pct",
            "Engine load (specified)",
            Expect("%"),
            Engine,
            "Basics",
        )
        .engine(&[(114, 1)]),
        v(
            "engine.load_corrected_pct",
            "Engine load (specified corrected)",
            Expect("%"),
            Engine,
            "Basics",
        )
        .engine(&[(114, 2)]),
        v(
            "engine.throttle_pct",
            "Throttle valve position",
            Expect("%"),
            Engine,
            "Basics",
        )
        .engine(&[(3, 3), (113, 3), (10, 3), (54, 4), (61, 3), (110, 4)]),
        v(
            "engine.throttle_deg",
            "Throttle valve angle",
            Expect("deg"),
            Engine,
            "Basics",
        )
        .engine(&[(3, 3), (113, 3), (10, 3), (54, 4), (61, 3), (110, 4)]),
        v(
            "engine.pedal_pct",
            "Accelerator pedal position (G79)",
            Expect("%"),
            Engine,
            "Basics",
        )
        .engine(&[(54, 3), (62, 3), (63, 1), (117, 2)]),
        v(
            "engine.coolant_c",
            "Coolant temperature",
            Expect("°C"),
            Engine,
            "Basics",
        )
        .ican("engine.coolant_c")
        .engine(&[
            (1, 2),
            (4, 3),
            (7, 3),
            (11, 2),
            (28, 3),
            (99, 2),
            (100, 2),
            (102, 2),
            (110, 2),
            (131, 1),
        ]),
        v(
            "engine.oil_temp_c",
            "Oil temperature",
            Expect("°C"),
            Engine,
            "Basics",
        )
        .ican("engine.oil_temp_c")
        .engine(&[(134, 1)]),
        v(
            "engine.intake_temp_c",
            "Intake air temperature (G42)",
            Expect("°C"),
            Engine,
            "Basics",
        )
        .engine(&[(4, 4), (6, 3), (11, 3), (102, 3), (118, 2), (134, 3)]),
        v(
            "engine.outlet_temp_c",
            "Engine outlet temperature",
            Expect("°C"),
            Engine,
            "Basics",
        )
        .engine(&[(130, 1), (134, 4)]),
        v(
            "engine.radiator_outlet_temp_c",
            "Radiator outlet temperature",
            Expect("°C"),
            Engine,
            "Basics",
        )
        .engine(&[(130, 2), (131, 3), (135, 1)]),
        v(
            "engine.target_rpm",
            "Engine speed (specified)",
            Expect("rpm"),
            Engine,
            "Basics",
        )
        .engine(&[(50, 2), (51, 2), (52, 2), (53, 2), (56, 2), (57, 2)]),
        v(
            "engine.time_since_start_s",
            "Time since engine start",
            Expect("s"),
            Engine,
            "Basics",
        )
        .engine(&[(100, 3)]),
        v(
            "engine.start_temp_c",
            "Engine start temperature",
            Expect("°C"),
            Engine,
            "Basics",
        )
        .engine(&[(104, 1), (138, 1)]),
        v(
            "engine.torque.actual_nm",
            "Engine torque (actual)",
            Expect("Nm"),
            Engine,
            "Basics",
        )
        .engine(&[(120, 3), (122, 3)]),
        v(
            "engine.boost.actual",
            "Boost pressure (actual absolute)",
            Expect("mbar"),
            Engine,
            "Boost",
        )
        .engine(&[(115, 4), (118, 4), (119, 4)]),
        v(
            "engine.boost.specified",
            "Boost pressure (specified absolute)",
            Expect("mbar"),
            Engine,
            "Boost",
        )
        .engine(&[(115, 3), (117, 4)]),
        v(
            "engine.boost.cluster_mbar",
            "Boost, absolute (cluster signal)",
            Expect("mbar"),
            Engine,
            "Boost",
        )
        .ican("engine.boost.cluster_mbar"),
        v(
            "engine.n75_duty_pct",
            "Boost control valve duty (N75)",
            Expect("%"),
            Engine,
            "Boost",
        )
        .engine(&[(114, 4), (118, 3), (119, 3)]),
        v(
            "engine.atmospheric_mbar",
            "Atmospheric pressure",
            Expect("mbar"),
            Engine,
            "Boost",
        )
        .engine(&[(113, 4)]),
        v(
            "engine.maf_gs",
            "Mass air flow (G70)",
            Expect("g/s"),
            Engine,
            "Boost",
        )
        .engine(&[(3, 2), (2, 4), (101, 4)]),
        v(
            "engine.manifold_mbar",
            "Manifold pressure (ICAN)",
            Expect("mbar"),
            Engine,
            "Boost",
        )
        .ican("engine.manifold_mbar"),
        v(
            "engine.altitude_factor",
            "Altitude factor (1.0 = 1013 mbar)",
            Reported,
            Engine,
            "Boost",
        )
        .ican("engine.altitude_factor"),
        v(
            "engine.altitude_correction",
            "Altitude correction factor",
            Reported,
            Engine,
            "Boost",
        )
        .engine(&[(6, 4)]),
        v(
            "engine.boost.adaptation",
            "Boost pressure control adaptation",
            Reported,
            Engine,
            "Boost",
        )
        .engine(&[(119, 2)]),
        v(
            "engine.fuel_rail.actual_bar",
            "Fuel rail pressure (actual)",
            Expect("bar"),
            Engine,
            "Fuel",
        )
        .engine(&[(106, 2), (140, 3), (141, 4)]),
        v(
            "engine.fuel_rail.specified_bar",
            "Fuel rail pressure (specified)",
            Expect("bar"),
            Engine,
            "Fuel",
        )
        .engine(&[(106, 1)]),
        v(
            "engine.injection_ms",
            "Injection duration",
            Expect("ms"),
            Engine,
            "Fuel",
        )
        .engine(&[(2, 3), (101, 3), (102, 4), (110, 3)]),
        v(
            "engine.lambda.actual",
            "Lambda control bank 1 (actual)",
            Reported,
            Engine,
            "Fuel",
        )
        .engine(&[(31, 1)]),
        v(
            "engine.lambda.specified",
            "Lambda control bank 1 (specified)",
            Reported,
            Engine,
            "Fuel",
        )
        .engine(&[(31, 2)]),
        v(
            "engine.overrun_cutoff",
            "Overrun fuel cutoff",
            Reported,
            Engine,
            "Fuel",
        )
        .ican("engine.overrun_cutoff"),
        v(
            "engine.fuel_pump_pct",
            "Fuel pump duty",
            Expect("%"),
            Engine,
            "Fuel",
        )
        .engine(&[(106, 3)]),
        // Estimated, per the old catalog.
        v(
            "engine.fuel_temp.calculated_c",
            "Fuel temperature (calculated)",
            Expect("°C"),
            Engine,
            "Fuel",
        )
        .engine(&[(106, 4)]),
        v(
            "engine.correction_factor.fuel",
            "Fuel correction factor",
            Reported,
            Engine,
            "Fuel",
        )
        .engine(&[(116, 2)]),
        v(
            "engine.correction_factor.coolant_temp",
            "Coolant temperature correction factor",
            Reported,
            Engine,
            "Fuel",
        )
        .engine(&[(116, 3)]),
        v(
            "engine.correction_factor.intake_temp",
            "Intake air temperature correction factor",
            Reported,
            Engine,
            "Fuel",
        )
        .engine(&[(116, 4)]),
        v(
            "engine.fuel_quantity_valve.closing_angle_deg",
            "Quantity valve closing angle",
            Expect("deg"),
            Engine,
            "Fuel",
        )
        .engine(&[(140, 1)]),
        v(
            "engine.fuel_quantity_valve.opening_angle_deg",
            "Quantity valve opening angle",
            Expect("deg"),
            Engine,
            "Fuel",
        )
        .engine(&[(140, 2)]),
        v(
            "engine.fuel_quantity_valve.status",
            "Quantity control valve status",
            Reported,
            Engine,
            "Fuel",
        )
        .engine(&[(140, 4)]),
        v(
            "engine.fuel_high_pressure.adaptation",
            "High pressure system adaptation",
            Reported,
            Engine,
            "Fuel",
        )
        .engine(&[(141, 1)]),
        v(
            "engine.fuel_high_pressure.controller_portion",
            "High pressure controller portion",
            Reported,
            Engine,
            "Fuel",
        )
        .engine(&[(141, 2)]),
        v(
            "engine.fuel_high_pressure.compression_volume",
            "Total compression volume",
            Reported,
            Engine,
            "Fuel",
        )
        .engine(&[(141, 3)]),
        v(
            "engine.ignition_timing_deg",
            "Ignition timing",
            Expect("deg"),
            Engine,
            "Timing",
        )
        .engine(&[(3, 4), (10, 4), (11, 4)]),
        v(
            "engine.timing_retard.cyl1_deg",
            "Timing retard cylinder 1",
            Expect("deg"),
            Engine,
            "Timing",
        )
        .engine(&[(20, 1), (22, 3)]),
        v(
            "engine.timing_retard.cyl2_deg",
            "Timing retard cylinder 2",
            Expect("deg"),
            Engine,
            "Timing",
        )
        .engine(&[(20, 2), (22, 4)]),
        v(
            "engine.timing_retard.cyl3_deg",
            "Timing retard cylinder 3",
            Expect("deg"),
            Engine,
            "Timing",
        )
        .engine(&[(20, 3), (23, 3)]),
        v(
            "engine.timing_retard.cyl4_deg",
            "Timing retard cylinder 4",
            Expect("deg"),
            Engine,
            "Timing",
        )
        .engine(&[(20, 4), (23, 4)]),
        v(
            "engine.knock.cyl1.sensor_voltage_v",
            "Knock sensor voltage cylinder 1",
            Expect("V"),
            Engine,
            "Timing",
        )
        .engine(&[(26, 1)]),
        v(
            "engine.knock.cyl2.sensor_voltage_v",
            "Knock sensor voltage cylinder 2",
            Expect("V"),
            Engine,
            "Timing",
        )
        .engine(&[(26, 2)]),
        v(
            "engine.knock.cyl3.sensor_voltage_v",
            "Knock sensor voltage cylinder 3",
            Expect("V"),
            Engine,
            "Timing",
        )
        .engine(&[(26, 3)]),
        v(
            "engine.knock.cyl4.sensor_voltage_v",
            "Knock sensor voltage cylinder 4",
            Expect("V"),
            Engine,
            "Timing",
        )
        .engine(&[(26, 4)]),
        v(
            "engine.knock.sensor_test_result",
            "Knock sensor test result",
            Reported,
            Engine,
            "Timing",
        )
        .engine(&[(28, 4)]),
        v(
            "engine.camshaft.intake.bank1.adjustment",
            "Camshaft adjustment intake bank 1",
            Reported,
            Engine,
            "Timing",
        )
        .engine(&[(91, 2)]),
        v(
            "engine.camshaft.intake.bank1.specified_deg",
            "Camshaft intake bank 1 adjustment (specified)",
            Expect("deg"),
            Engine,
            "Timing",
        )
        .engine(&[(91, 3)]),
        v(
            "engine.camshaft.intake.bank1.actual_deg",
            "Camshaft intake bank 1 adjustment (actual)",
            Expect("deg"),
            Engine,
            "Timing",
        )
        .engine(&[(91, 4), (94, 2)]),
        v(
            "engine.camshaft.intake.bank1.phase_position_deg",
            "Camshaft phase position intake bank 1",
            Expect("deg"),
            Engine,
            "Timing",
        )
        .engine(&[(93, 3)]),
        v(
            "engine.camshaft.intake.bank1.test",
            "Camshaft adjustment test intake bank 1",
            Reported,
            Engine,
            "Timing",
        )
        .engine(&[(94, 3)]),
        v(
            "engine.lambda.control.bank1",
            "Lambda control bank 1",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(1, 3), (107, 2)]),
        v(
            "engine.lambda.bank1.sensor1.condition",
            "Lambda sensor condition bank 1 sensor 1",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(30, 1)]),
        v(
            "engine.lambda.bank1.sensor2.condition",
            "Lambda sensor condition bank 1 sensor 2",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(30, 2)]),
        v(
            "engine.lambda.bank1.sensor1.adaptation.idle",
            "Lambda adaptation bank 1 sensor 1 (idle)",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(32, 1)]),
        v(
            "engine.lambda.bank1.sensor1.adaptation.partial",
            "Lambda adaptation bank 1 sensor 1 (partial)",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(32, 2)]),
        v(
            "engine.lambda.bank1.sensor1.control",
            "Lambda control bank 1 sensor 1",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(33, 1)]),
        v(
            "engine.lambda.bank1.sensor1.voltage_v",
            "Lambda sensor voltage bank 1 sensor 1",
            Expect("V"),
            Engine,
            "Lambda",
        )
        .engine(&[(33, 2)]),
        v(
            "engine.lambda.bank1.sensor1.dynamic_factor",
            "Dynamic factor bank 1 sensor 1",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(34, 3)]),
        v(
            "engine.lambda.aging_result",
            "Lambda aging result",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(34, 4)]),
        v(
            "engine.lambda.bank1.sensor2.voltage_v",
            "Lambda sensor voltage bank 1 sensor 2",
            Expect("V"),
            Engine,
            "Lambda",
        )
        .engine(&[(36, 1), (37, 2)]),
        v(
            "engine.lambda.availability_result",
            "Lambda availability result",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(36, 2)]),
        v(
            "engine.lambda.bank1.sensor2.delta",
            "Delta lambda bank 1 sensor 2",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(37, 3)]),
        v(
            "engine.lambda.bank1.delta_result",
            "Delta lambda result bank 1",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(37, 4)]),
        v(
            "engine.lambda.bank1.sensor1.resistance_ohm",
            "Lambda sensor resistance bank 1 sensor 1",
            Expect("ohm"),
            Engine,
            "Lambda",
        )
        .engine(&[(41, 1)]),
        v(
            "engine.lambda.bank1.sensor1.heater_condition",
            "Lambda heater condition bank 1 sensor 1",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(41, 2)]),
        v(
            "engine.lambda.bank1.sensor2.resistance_ohm",
            "Lambda sensor resistance bank 1 sensor 2",
            Expect("ohm"),
            Engine,
            "Lambda",
        )
        .engine(&[(41, 3)]),
        v(
            "engine.lambda.bank1.sensor2.heater_condition",
            "Lambda heater condition bank 1 sensor 2",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(41, 4)]),
        v(
            "engine.lambda.bank1.sensor1.lambda_voltage_v",
            "Lambda voltage bank 1 sensor 1",
            Expect("V"),
            Engine,
            "Lambda",
        )
        .engine(&[(43, 3)]),
        v(
            "engine.lambda.bank1.sensor2.aging_test",
            "Lambda sensor aging test bank 1 sensor 2",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(43, 4)]),
        v(
            "engine.lambda.control.generic",
            "Lambda control (group 99)",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(99, 3)]),
        v(
            "engine.lambda.control_status",
            "Lambda control status",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(99, 4)]),
        v(
            "engine.lambda.control_result",
            "Lambda control result",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(107, 4)]),
        v(
            "engine.lambda.bank1.enrichment_factor",
            "Enrichment factor sensor bank 1",
            Reported,
            Engine,
            "Lambda",
        )
        .engine(&[(112, 2)]),
        v(
            "engine.misfire.count",
            "Misfire counter",
            Reported,
            Engine,
            "Misfire",
        )
        .engine(&[(14, 3)]),
        v(
            "engine.misfire.recognition",
            "Misfire recognition",
            Reported,
            Engine,
            "Misfire",
        )
        .engine(&[(14, 4), (15, 4), (16, 4)]),
        v(
            "engine.misfire.cyl1.count",
            "Misfire counter cylinder 1",
            Reported,
            Engine,
            "Misfire",
        )
        .engine(&[(15, 1)]),
        v(
            "engine.misfire.cyl2.count",
            "Misfire counter cylinder 2",
            Reported,
            Engine,
            "Misfire",
        )
        .engine(&[(15, 2)]),
        v(
            "engine.misfire.cyl3.count",
            "Misfire counter cylinder 3",
            Reported,
            Engine,
            "Misfire",
        )
        .engine(&[(15, 3)]),
        v(
            "engine.misfire.cyl4.count",
            "Misfire counter cylinder 4",
            Reported,
            Engine,
            "Misfire",
        )
        .engine(&[(16, 1)]),
        v(
            "engine.misfire.rpm_barrier.lower_rpm",
            "Misfire RPM barrier (lower)",
            Expect("rpm"),
            Engine,
            "Misfire",
        )
        .engine(&[(18, 1)]),
        v(
            "engine.misfire.rpm_barrier.upper_rpm",
            "Misfire RPM barrier (upper)",
            Expect("rpm"),
            Engine,
            "Misfire",
        )
        .engine(&[(18, 2)]),
        v(
            "engine.misfire.load_barrier.lower",
            "Misfire load barrier (lower)",
            Reported,
            Engine,
            "Misfire",
        )
        .engine(&[(18, 3)]),
        v(
            "engine.misfire.load_barrier.upper",
            "Misfire load barrier (upper)",
            Reported,
            Engine,
            "Misfire",
        )
        .engine(&[(18, 4)]),
        v(
            "engine.fan_pct",
            "Radiator fan command",
            Expect("%"),
            Engine,
            "Cooling",
        )
        .ican("engine.fan_pct"),
        v(
            "engine.extra_cooling",
            "Extra cooling requested",
            Reported,
            Engine,
            "Cooling",
        )
        .ican("engine.extra_cooling"),
        v(
            "engine.cooling.status",
            "Cooling status",
            Reported,
            Engine,
            "Cooling",
        )
        .engine(&[(132, 4)]),
        v(
            "engine.cooling.fan1.duty",
            "Cooling fan 1 activation duty",
            Reported,
            Engine,
            "Cooling",
        )
        .engine(&[(135, 2)]),
        v(
            "engine.cooling.auxiliary_pump",
            "Auxiliary water pump",
            Reported,
            Engine,
            "Cooling",
        )
        .engine(&[(136, 3)]),
        v(
            "engine.cooling.fan_after_run",
            "Fan after run",
            Reported,
            Engine,
            "Cooling",
        )
        .engine(&[(136, 4)]),
        v(
            "engine.thermostat.mean_air_mass",
            "Mean engine air mass (thermostat diagnostic)",
            Reported,
            Engine,
            "Cooling",
        )
        .engine(&[(138, 2)]),
        v(
            "engine.thermostat.median_speed_kmh",
            "Median vehicle speed (thermostat diagnostic)",
            Expect("km/h"),
            Engine,
            "Cooling",
        )
        .engine(&[(138, 3)]),
        v(
            "engine.thermostat.test_result",
            "Thermostat test result",
            Reported,
            Engine,
            "Cooling",
        )
        .engine(&[(138, 4), (139, 4)]),
        v(
            "engine.coolant_diag_c",
            "Engine coolant temperature diagnostic",
            Expect("°C"),
            Engine,
            "Cooling",
        )
        .engine(&[(139, 1)]),
        v(
            "engine.thermostat.integral_air_mass.actual",
            "Integral mass air flow (actual)",
            Reported,
            Engine,
            "Cooling",
        )
        .engine(&[(139, 2)]),
        v(
            "engine.thermostat.integral_air_mass.specified",
            "Integral mass air flow (specified)",
            Reported,
            Engine,
            "Cooling",
        )
        .engine(&[(139, 3)]),
        v(
            "engine.ac.readiness",
            "A/C readiness",
            Reported,
            Engine,
            "A/C",
        )
        .engine(&[(50, 3), (52, 3), (137, 1)]),
        v(
            "engine.ac.compressor_state",
            "A/C compressor",
            Reported,
            Engine,
            "A/C",
        )
        .engine(&[(50, 4), (57, 3), (137, 2)]),
        v(
            "engine.ac.compressor_load",
            "A/C compressor load",
            Reported,
            Engine,
            "A/C",
        )
        .engine(&[(57, 4)]),
        v(
            "engine.ac.refrigerant_pressure_bar",
            "A/C refrigerant pressure",
            Expect("bar"),
            Engine,
            "A/C",
        )
        .engine(&[(137, 3)]),
        v(
            "engine.ac.fan_request",
            "Fan request from A/C system",
            Reported,
            Engine,
            "A/C",
        )
        .engine(&[(137, 4)]),
        v(
            "engine.throttle.sensor1.position",
            "Throttle valve sensor 1 (G187)",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(60, 1), (62, 1), (117, 3)]),
        v(
            "engine.throttle.sensor2.position",
            "Throttle valve sensor 2 (G188)",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(60, 2), (62, 2)]),
        v(
            "engine.throttle.steps_count",
            "Throttle valve steps counter",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(60, 3)]),
        v(
            "engine.throttle.alignment_status",
            "Throttle body alignment status",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(60, 4)]),
        v(
            "engine.pedal.sensor2.position",
            "Accelerator pedal position sensor 2 (G185)",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(62, 4)]),
        v(
            "engine.pedal.kickdown.alignment_value",
            "Kick-down alignment value",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(63, 2)]),
        v(
            "engine.pedal.kickdown.switch",
            "Kick-down switch",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(63, 3)]),
        v(
            "engine.pedal.kickdown.alignment_status",
            "Kick-down alignment status",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(63, 4)]),
        v(
            "engine.throttle.sensor1.lower_adaptation",
            "Throttle sensor 1 lower adaptation",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(64, 1)]),
        v(
            "engine.throttle.sensor2.lower_adaptation",
            "Throttle sensor 2 lower adaptation",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(64, 2)]),
        v(
            "engine.throttle.sensor1.emergency_air_gap",
            "Throttle sensor 1 emergency air gap",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(64, 3)]),
        v(
            "engine.throttle.sensor2.emergency_air_gap",
            "Throttle sensor 2 emergency air gap",
            Reported,
            Engine,
            "Throttle & pedal",
        )
        .engine(&[(64, 4)]),
        v(
            "engine.cruise_state",
            "Cruise control state",
            Reported,
            Engine,
            "Cruise",
        )
        .ican("engine.cruise_state"),
        v(
            "engine.cruise.switch_positions1",
            "Cruise switch positions I",
            Reported,
            Engine,
            "Cruise",
        )
        .engine(&[(66, 2), (67, 2)]),
        v(
            "engine.cruise.speed.specified_kmh",
            "Cruise vehicle speed (specified)",
            Expect("km/h"),
            Engine,
            "Cruise",
        )
        .engine(&[(66, 3)]),
        v(
            "engine.cruise.switch_positions2",
            "Cruise switch positions II",
            Reported,
            Engine,
            "Cruise",
        )
        .engine(&[(66, 4)]),
        v(
            "engine.cruise.shutoff.reversible",
            "Cruise shut-off criteria (reversible)",
            Reported,
            Engine,
            "Cruise",
        )
        .engine(&[(67, 1)]),
        v(
            "engine.cruise.shutoff.group67_field3",
            "Cruise shut-off criteria (group 67 field 3)",
            Reported,
            Engine,
            "Cruise",
        )
        .engine(&[(67, 3)]),
        v(
            "engine.catalyst.bank1.temp_c",
            "Catalytic converter temperature bank 1",
            Expect("°C"),
            Engine,
            "Emissions",
        )
        .engine(&[(34, 2), (43, 2), (46, 2)]),
        v(
            "engine.catalyst.bank1.conversion_rate",
            "Catalytic conversion bank 1",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(46, 3)]),
        v(
            "engine.catalyst.bank1.conversion_test",
            "Catalytic conversion test bank 1",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(46, 4)]),
        v(
            "engine.evap.valve.open",
            "EVAP emissions solenoid valve (open)",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(70, 1)]),
        v(
            "engine.evap.lambda_control",
            "Lambda control during EVAP test",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(70, 2)]),
        v(
            "engine.evap.valve.flow",
            "EVAP emissions solenoid valve (flow)",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(70, 3)]),
        v(
            "engine.evap.valve.test",
            "EVAP valve test",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(70, 4)]),
        v(
            "engine.evap.leak.reed_contact_condition",
            "Leak detection reed contact condition",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(71, 1)]),
        v(
            "engine.evap.leak.malfunction_message",
            "Leak detection malfunction message",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(71, 2)]),
        v(
            "engine.evap.leak.test_status",
            "Leak test status",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(71, 3)]),
        v(
            "engine.evap.leak.diagnostic_test_status",
            "Leak diagnostic test status",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(71, 4)]),
        v(
            "engine.obd.iumpr_record_count",
            "IUMPR record counter",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(85, 2)]),
        v(
            "engine.obd.general_denominator",
            "OBD general denominator",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(85, 3)]),
        v(
            "engine.obd.ignition_cycle_count",
            "OBD ignition cycle counter",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(85, 4)]),
        v(
            "engine.obd.readiness_bits",
            "OBD readiness bits",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(86, 1), (87, 1), (100, 1)]),
        v(
            "engine.obd.cycle_flags1",
            "OBD cycle flags I",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(86, 2), (88, 1)]),
        v(
            "engine.obd.cycle_flags2_part1",
            "OBD cycle flags II (part 1)",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(86, 3)]),
        v(
            "engine.obd.error_flags1",
            "OBD error flags I",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(87, 2)]),
        v(
            "engine.obd.error_flags2_part1",
            "OBD error flags II (part 1)",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(87, 3)]),
        v(
            "engine.obd.cycle_flags2_all",
            "OBD cycle flags II",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(88, 2)]),
        v(
            "engine.obd.cycle_flags3",
            "OBD cycle flags III",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(88, 3)]),
        v(
            "engine.obd.mil_distance_km",
            "Distance driven with MIL on",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(89, 1)]),
        v(
            "engine.obd.tank_condition",
            "OBD tank condition",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(89, 2)]),
        v(
            "engine.obd.status",
            "OBD status",
            Reported,
            Engine,
            "Emissions",
        )
        .engine(&[(100, 4)]),
        // Estimated, per the old catalog.
        v(
            "engine.exhaust_temp.projected_c",
            "Exhaust temperature (projected)",
            Expect("°C"),
            Engine,
            "Emissions",
        )
        .engine(&[(112, 3)]),
        v(
            "engine.idle.regulator",
            "Idle regulator",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(55, 2), (56, 3)]),
        v(
            "engine.idle.stabilization_adaptation",
            "Idle stabilization self-adaptation",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(55, 3)]),
        v(
            "engine.temp.adaptation_factor1",
            "Temperature adaptation factor 1",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(104, 2)]),
        v(
            "engine.temp.adaptation_factor2",
            "Temperature adaptation factor 2",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(104, 3)]),
        v(
            "engine.temp.adaptation_factor3",
            "Temperature adaptation factor 3",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(104, 4)]),
        v(
            "engine.adaptation.rpm_range1",
            "Adaptation RPM range 1",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(111, 1)]),
        v(
            "engine.adaptation.rpm_range2",
            "Adaptation RPM range 2",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(111, 2)]),
        v(
            "engine.adaptation.rpm_range3",
            "Adaptation RPM range 3",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(111, 3)]),
        v(
            "engine.adaptation.rpm_range4",
            "Adaptation RPM range 4",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(111, 4)]),
        v(
            "engine.runner_flap.bank1.position.actual",
            "Runner flap bank 1 position (actual)",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(142, 1)]),
        v(
            "engine.runner_flap.bank1.position.specified",
            "Runner flap bank 1 position (specified)",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(142, 2)]),
        v(
            "engine.runner_flap.bank1.position.offset",
            "Runner flap bank 1 position offset",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(142, 3)]),
        v(
            "engine.runner_flap.bank1.adaptation",
            "Runner flap bank 1 adaptation",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(142, 4)]),
        v(
            "engine.runner_flap.position.actual",
            "Runner flap position (actual, bank unspecified)",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(143, 3)]),
        v(
            "engine.runner_flap.operating_mode",
            "Runner flap operating mode (group 143)",
            Reported,
            Engine,
            "Idle & adaptation",
        )
        .engine(&[(143, 4)]),
        v(
            "engine.fault_stored",
            "Engine fault stored",
            Reported,
            Engine,
            "Status",
        )
        .ican("engine.fault_stored"),
        v(
            "engine.brake_switch",
            "Brake light switch (engine)",
            Reported,
            Engine,
            "Status",
        )
        .ican("engine.brake_switch"),
        v(
            "engine.clutch_switch",
            "Clutch switch",
            Reported,
            Engine,
            "Status",
        )
        .ican("engine.clutch_switch"),
        v(
            "engine.basic_setting.requirements",
            "Basic setting requirements",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(1, 4)]),
        v(
            "engine.load_status",
            "Engine load status",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(5, 4), (54, 2)]),
        v(
            "engine.operating_mode.group7",
            "Engine operating mode (group 7)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(7, 4)]),
        v(
            "engine.operating_mode.group48",
            "Operating mode (group 48)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(48, 1)]),
        v(
            "engine.selected_gear",
            "Selected gear (engine ECU)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(51, 3), (68, 3)]),
        v(
            "engine.window_heater.condition",
            "Front/rear window heater",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(52, 4)]),
        v(
            "engine.operating_condition",
            "Engine operating condition",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(55, 4), (56, 4), (61, 4)]),
        v(
            "engine.torque_converter_clutch.condition",
            "Torque converter/clutch",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(68, 4)]),
        v(
            "engine.identification.advanced1",
            "Advanced identification I",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(80, 1)]),
        v(
            "engine.identification.vin",
            "Vehicle identification number field (VIN)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(81, 1)]),
        v(
            "engine.identification.immobilizer",
            "Immobilizer identification field",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(81, 2)]),
        v(
            "engine.identification.advanced3",
            "Advanced identification III",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(82, 1)]),
        v(
            "engine.odometer_km",
            "Vehicle mileage (engine ECU)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(85, 1)]),
        v(
            "engine.torque.asr_specified_nm",
            "Engine torque specified by ASR",
            Expect("Nm"),
            Engine,
            "Status",
        )
        .engine(&[(120, 2), (122, 2)]),
        v(
            "engine.traction_control.status",
            "Traction control status",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(120, 4)]),
        v(
            "engine.transmission.status",
            "Transmission status (engine ECU)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(122, 4)]),
        v(
            "engine.module_communication.transmission",
            "Transmission electronics (J217) communication",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(125, 1)]),
        v(
            "engine.module_communication.brakes",
            "Brake electronics (J104) communication",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(125, 2)]),
        v(
            "engine.module_communication.instruments",
            "Instrument cluster (J285) communication",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(125, 3), (129, 2)]),
        v(
            "engine.module_communication.hvac",
            "Heating/air conditioning (J255) communication",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(125, 4)]),
        v(
            "engine.module_communication.airbag",
            "Airbag (J234) communication",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(126, 3)]),
        v(
            "engine.module_communication.steering_wheel",
            "Steering wheel electronics (J527) communication",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(127, 3)]),
        v(
            "engine.module_communication.gateway",
            "CAN gateway (J533) communication",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(129, 3)]),
        v(
            "engine.status.group200.counter",
            "Status counter (group 200)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(200, 1)]),
        v(
            "engine.status.group200.field2",
            "Status (group 200 field 2)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(200, 2)]),
        v(
            "engine.status.group200.field3",
            "Status (group 200 field 3)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(200, 3)]),
        v(
            "engine.status.group200.field4",
            "Status (group 200 field 4)",
            Reported,
            Engine,
            "Status",
        )
        .engine(&[(200, 4)]),
        v(
            "vehicle.speed_kmh",
            "Speed",
            Expect("km/h"),
            Driving,
            "Speed",
        )
        .ican("vehicle.speed_kmh")
        .engine(&[(5, 3), (66, 1)]),
        v(
            "vehicle.speed_gw_kmh",
            "Speed (gateway 0x351)",
            Expect("km/h"),
            Driving,
            "Speed",
        )
        .ican("vehicle.speed_gw_kmh"),
        v(
            "vehicle.odometer_km",
            "Odometer",
            Expect("km"),
            Driving,
            "Speed",
        )
        .ican("vehicle.odometer_km"),
        v(
            "vehicle.tyre_circumference_mm",
            "Tyre circumference",
            Expect("mm"),
            Driving,
            "Speed",
        )
        .ican("vehicle.tyre_circumference_mm"),
        v(
            "steering.angle_deg",
            "Steering angle",
            Expect("°"),
            Driving,
            "Steering",
        )
        .ican("steering.angle_deg"),
        v(
            "steering.rate_dps",
            "Steering rate",
            Expect("deg/s"),
            Driving,
            "Steering",
        )
        .ican("steering.rate_dps"),
        v(
            "vehicle.yaw_dps",
            "Yaw rate (nav)",
            Expect("deg/s"),
            Driving,
            "Steering",
        )
        .ican("vehicle.yaw_dps"),
        v("brakes.handbrake", "Handbrake", Reported, Driving, "Brakes").ican("brakes.handbrake"),
        v(
            "brakes.epb_closed",
            "Parking brake closed",
            Reported,
            Driving,
            "Brakes",
        )
        .ican("brakes.epb_closed"),
        v(
            "brakes.abs_active",
            "ABS active",
            Reported,
            Driving,
            "Brakes",
        )
        .ican("brakes.abs_active"),
        v(
            "brakes.esp_active",
            "ESP active",
            Reported,
            Driving,
            "Brakes",
        )
        .ican("brakes.esp_active"),
        v(
            "wheels.fl_kmh",
            "Wheel speed FL (Haldex)",
            Expect("km/h"),
            Driving,
            "Wheels",
        )
        .ican("wheels.fl_kmh"),
        v(
            "wheels.fr_kmh",
            "Wheel speed FR (Haldex)",
            Expect("km/h"),
            Driving,
            "Wheels",
        )
        .ican("wheels.fr_kmh"),
        v(
            "wheels.rl_kmh",
            "Wheel speed RL (Haldex)",
            Expect("km/h"),
            Driving,
            "Wheels",
        )
        .ican("wheels.rl_kmh"),
        v(
            "wheels.rr_kmh",
            "Wheel speed RR (Haldex)",
            Expect("km/h"),
            Driving,
            "Wheels",
        )
        .ican("wheels.rr_kmh"),
        v(
            "wheels.fl_pulses",
            "Wheel pulses FL",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("wheels.fl_pulses"),
        v(
            "wheels.fr_pulses",
            "Wheel pulses FR",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("wheels.fr_pulses"),
        v(
            "wheels.rl_pulses",
            "Wheel pulses RL",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("wheels.rl_pulses"),
        v(
            "wheels.rr_pulses",
            "Wheel pulses RR",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("wheels.rr_pulses"),
        v(
            "wheels.fl_reverse",
            "Wheel reversing FL",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("wheels.fl_reverse"),
        v(
            "wheels.fr_reverse",
            "Wheel reversing FR",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("wheels.fr_reverse"),
        v(
            "wheels.rl_reverse",
            "Wheel reversing RL",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("wheels.rl_reverse"),
        v(
            "wheels.rr_reverse",
            "Wheel reversing RR",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("wheels.rr_reverse"),
        v(
            "vehicle.front_axle_pulses",
            "Front axle pulses",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("vehicle.front_axle_pulses"),
        v(
            "vehicle.pulses_per_rev",
            "Pulses per wheel turn",
            Reported,
            Driving,
            "Wheels",
        )
        .ican("vehicle.pulses_per_rev"),
        v(
            "trans.selector",
            "Selector position",
            Reported,
            Transmission,
            "Selector",
        )
        .ican("trans.selector"),
        v(
            "trans.shift_lock",
            "Shift lock",
            Reported,
            Transmission,
            "Selector",
        )
        .ican("trans.shift_lock"),
        v(
            "trans.shifting",
            "Shift in progress",
            Reported,
            Transmission,
            "Selector",
        )
        .ican("trans.shifting"),
        v(
            "trans.fluid_temp_c",
            "Transmission fluid temperature",
            Reported,
            Transmission,
            "Temps",
        )
        .trans(&[(19, 1)]),
        v(
            "trans.clutch_oil_temp_c",
            "Clutch oil temperature (G509)",
            Reported,
            Transmission,
            "Temps",
        )
        .trans(&[(19, 3)]),
        v(
            "trans.module_temp_c",
            "Transmission control module temperature",
            Reported,
            Transmission,
            "Temps",
        )
        .trans(&[(19, 2)]),
        v(
            "trans.k1.shaft_rpm",
            "Driveshaft 1 speed (G501)",
            Reported,
            Transmission,
            "Clutches",
        )
        .trans(&[(11, 1), (8, 2)]),
        v(
            "trans.k1.torque_specified_nm",
            "Specified clutch torque K1",
            Reported,
            Transmission,
            "Clutches",
        )
        .trans(&[(11, 2)]),
        v(
            "trans.k1.valve_a",
            "Clutch valve 1 current",
            Reported,
            Transmission,
            "Clutches",
        )
        .trans(&[(11, 3), (6, 2), (13, 1)]),
        v(
            "trans.k1.pressure_bar",
            "Actual clutch 1 pressure (G193)",
            Reported,
            Transmission,
            "Clutches",
        )
        .trans(&[(11, 4)]),
        v(
            "trans.k2.shaft_rpm",
            "Driveshaft 2 speed (G502)",
            Reported,
            Transmission,
            "Clutches",
        )
        .trans(&[(12, 1), (8, 3)]),
        v(
            "trans.k2.torque_specified_nm",
            "Specified clutch torque K2",
            Reported,
            Transmission,
            "Clutches",
        )
        .trans(&[(12, 2)]),
        v(
            "trans.k2.valve_a",
            "Clutch valve 2 current",
            Reported,
            Transmission,
            "Clutches",
        )
        .trans(&[(12, 3), (7, 2), (14, 1)]),
        v(
            "trans.k2.pressure_bar",
            "Actual clutch 2 pressure (G194)",
            Reported,
            Transmission,
            "Clutches",
        )
        .trans(&[(12, 4)]),
        v(
            "trans.fork_1_3_mm",
            "Gear selector 1-3 travel distance",
            Reported,
            Transmission,
            "Shift forks",
        )
        .trans(&[(16, 1)]),
        v(
            "trans.fork_2_4_mm",
            "Gear selector 2-4 travel distance",
            Reported,
            Transmission,
            "Shift forks",
        )
        .trans(&[(16, 2)]),
        v(
            "trans.fork_5_n_mm",
            "Gear selector 5-N travel distance",
            Reported,
            Transmission,
            "Shift forks",
        )
        .trans(&[(16, 3)]),
        v(
            "trans.fork_6_r_mm",
            "Gear selector 6-R travel distance",
            Reported,
            Transmission,
            "Shift forks",
        )
        .trans(&[(16, 4)]),
        v(
            "trans.idle_status",
            "Transmission idle status",
            Reported,
            Transmission,
            "Status",
        )
        .trans(&[(19, 4)]),
        v(
            "trans.lever_position",
            "Selector lever position",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(2, 1), (3, 1), (4, 1), (5, 1), (6, 1), (7, 1)]),
        v(
            "trans.lever_plausible",
            "Selector lever position (plausibility)",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(2, 2), (3, 2)]),
        v(
            "trans.lever_error",
            "Selector lever error byte",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(2, 3)]),
        v(
            "trans.gear_mode",
            "Selected gear or driving mode",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(2, 4)]),
        v(
            "trans.tip_up",
            "Tiptronic upshift",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(3, 3)]),
        v(
            "trans.tip_down",
            "Tiptronic downshift",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(3, 4)]),
        v(
            "trans.direction",
            "Driving direction",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(4, 2)]),
        v(
            "trans.start_enabled",
            "Starting enabled",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(5, 2)]),
        v(
            "trans.kickdown",
            "Kick-down switch",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(10, 2)]),
        v(
            "trans.brake_light_switch",
            "Brake light switch (F)",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(1, 1)]),
        v(
            "trans.brake_test_switch",
            "Brake test switch",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(1, 2)]),
        v(
            "trans.shiftlock_status",
            "Shift lock status (DSG)",
            Reported,
            Transmission,
            "Lever",
        )
        .trans(&[(1, 3)]),
        v(
            "trans.vehicle_speed",
            "Vehicle speed (DSG)",
            Reported,
            Transmission,
            "Speeds",
        )
        .trans(&[(1, 4)]),
        v(
            "trans.engine_rpm",
            "Engine speed G28 (DSG)",
            Reported,
            Transmission,
            "Speeds",
        )
        .trans(&[(9, 1)]),
        v(
            "trans.input_rpm",
            "Transmission input speed (G182)",
            Reported,
            Transmission,
            "Speeds",
        )
        .trans(&[(8, 1), (9, 2)]),
        v(
            "trans.output1_rpm",
            "Transmission output speed 1",
            Reported,
            Transmission,
            "Speeds",
        )
        .trans(&[(4, 3), (8, 4), (9, 3)]),
        v(
            "trans.output2_rpm",
            "Transmission output speed 2",
            Reported,
            Transmission,
            "Speeds",
        )
        .trans(&[(4, 4), (9, 4)]),
        v(
            "trans.pedal_pct",
            "Accelerator pedal angle (DSG)",
            Reported,
            Transmission,
            "Torque",
        )
        .trans(&[(10, 1)]),
        v(
            "trans.engine_torque",
            "Engine torque (DSG)",
            Reported,
            Transmission,
            "Torque",
        )
        .trans(&[(10, 3)]),
        v(
            "trans.torque_loss",
            "Engine torque loss (DSG)",
            Reported,
            Transmission,
            "Torque",
        )
        .trans(&[(10, 4)]),
        v(
            "trans.safety_valve_1",
            "Safety valve 1 (N233)",
            Reported,
            Transmission,
            "Valves",
        )
        .trans(&[(6, 3), (13, 2)]),
        v(
            "trans.safety_valve_2",
            "Safety valve 2 (N371)",
            Reported,
            Transmission,
            "Valves",
        )
        .trans(&[(7, 3), (14, 2)]),
        v(
            "trans.main_pressure_valve_a",
            "Main pressure valve current",
            Reported,
            Transmission,
            "Valves",
        )
        .trans(&[(6, 4), (7, 4)]),
        v(
            "trans.n88",
            "Solenoid valve 1 (N88)",
            Reported,
            Transmission,
            "Valves",
        )
        .trans(&[(13, 3)]),
        v(
            "trans.n89",
            "Solenoid valve 2 (N89)",
            Reported,
            Transmission,
            "Valves",
        )
        .trans(&[(13, 4)]),
        v(
            "trans.n90",
            "Solenoid valve 3 (N90)",
            Reported,
            Transmission,
            "Valves",
        )
        .trans(&[(14, 3)]),
        v(
            "trans.n91",
            "Solenoid valve 4 (N91)",
            Reported,
            Transmission,
            "Valves",
        )
        .trans(&[(14, 4)]),
        v(
            "trans.supply1_v",
            "Transmission supply voltage 1",
            Reported,
            Transmission,
            "Supply",
        )
        .trans(&[(5, 3)]),
        v(
            "trans.supply2_v",
            "Transmission supply voltage 2",
            Reported,
            Transmission,
            "Supply",
        )
        .trans(&[(5, 4)]),
        v(
            "trans.gs_1_3",
            "Gear selector 1-3",
            Reported,
            Transmission,
            "Gear selectors",
        )
        .trans(&[(15, 1)]),
        v(
            "trans.gs_2_4",
            "Gear selector 2-4",
            Reported,
            Transmission,
            "Gear selectors",
        )
        .trans(&[(15, 2)]),
        v(
            "trans.gs_5_n",
            "Gear selector 5-N",
            Reported,
            Transmission,
            "Gear selectors",
        )
        .trans(&[(15, 3)]),
        v(
            "trans.gs_6_r",
            "Gear selector 6-R",
            Reported,
            Transmission,
            "Gear selectors",
        )
        .trans(&[(15, 4)]),
        v(
            "trans.sync_1",
            "Synchronous travel path 1st gear",
            Reported,
            Transmission,
            "Synchronizers",
        )
        .trans(&[(18, 1)]),
        v(
            "trans.sync_2",
            "Synchronous travel path 2nd gear",
            Reported,
            Transmission,
            "Synchronizers",
        )
        .trans(&[(17, 2)]),
        v(
            "trans.sync_3",
            "Synchronous travel path 3rd gear",
            Reported,
            Transmission,
            "Synchronizers",
        )
        .trans(&[(17, 1)]),
        v(
            "trans.sync_4",
            "Synchronous travel path 4th gear",
            Reported,
            Transmission,
            "Synchronizers",
        )
        .trans(&[(18, 2)]),
        v(
            "trans.sync_5",
            "Synchronous travel path 5th gear",
            Reported,
            Transmission,
            "Synchronizers",
        )
        .trans(&[(18, 3)]),
        v(
            "trans.sync_6",
            "Synchronous travel path 6th gear",
            Reported,
            Transmission,
            "Synchronizers",
        )
        .trans(&[(18, 4)]),
        v(
            "trans.sync_n",
            "Synchronous travel path N",
            Reported,
            Transmission,
            "Synchronizers",
        )
        .trans(&[(17, 3)]),
        v(
            "trans.sync_r",
            "Synchronous travel path R",
            Reported,
            Transmission,
            "Synchronizers",
        )
        .trans(&[(17, 4)]),
        v(
            "trans.adj30_lower_a",
            "Group 30 lower adjustment A",
            Reported,
            Transmission,
            "Adjustment",
        )
        .trans(&[(30, 1)]),
        v(
            "trans.adj30_lower_b",
            "Group 30 lower adjustment B",
            Reported,
            Transmission,
            "Adjustment",
        )
        .trans(&[(30, 2)]),
        v(
            "trans.adj30_upper_a",
            "Group 30 upper adjustment A",
            Reported,
            Transmission,
            "Adjustment",
        )
        .trans(&[(30, 3)]),
        v(
            "trans.adj30_upper_b",
            "Group 30 upper adjustment B",
            Reported,
            Transmission,
            "Adjustment",
        )
        .trans(&[(30, 4)]),
        v(
            "trans.adj31_lower_a",
            "Group 31 lower adjustment A",
            Reported,
            Transmission,
            "Adjustment",
        )
        .trans(&[(31, 1)]),
        v(
            "trans.adj31_lower_b",
            "Group 31 lower adjustment B",
            Reported,
            Transmission,
            "Adjustment",
        )
        .trans(&[(31, 2)]),
        v(
            "trans.adj31_upper_a",
            "Group 31 upper adjustment A",
            Reported,
            Transmission,
            "Adjustment",
        )
        .trans(&[(31, 3)]),
        v(
            "trans.adj31_upper_b",
            "Group 31 upper adjustment B",
            Reported,
            Transmission,
            "Adjustment",
        )
        .trans(&[(31, 4)]),
        v(
            "trans.bus125_engine",
            "Bus status (125): engine",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(125, 1)]),
        v(
            "trans.bus125_brakes",
            "Bus status (125): brake electronics",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(125, 2)]),
        v(
            "trans.bus125_cluster",
            "Bus status (125): instrument cluster",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(125, 3)]),
        v(
            "trans.bus125_lever",
            "Bus status (125): selector lever",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(125, 4)]),
        v(
            "trans.bus126_gateway",
            "Bus status (126): CAN gateway",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(126, 1)]),
        v(
            "trans.bus126_column",
            "Bus status (126): steering column module",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(126, 2)]),
        v(
            "trans.bus225_engine",
            "Bus status (225): engine",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(225, 1)]),
        v(
            "trans.bus225_brakes",
            "Bus status (225): brake electronics",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(225, 2)]),
        v(
            "trans.bus225_cluster",
            "Bus status (225): instrument cluster",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(225, 3)]),
        v(
            "trans.bus225_lever",
            "Bus status (225): selector lever",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(225, 4)]),
        v(
            "trans.bus226_gateway",
            "Bus status (226): CAN gateway",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(226, 1)]),
        v(
            "trans.bus226_column",
            "Bus status (226): steering column module",
            Reported,
            Transmission,
            "Bus status",
        )
        .trans(&[(226, 2)]),
        v(
            "trans.fault_id_00",
            "Internal malfunction ID #0",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(51, 1)]),
        v(
            "trans.fault_id_01",
            "Internal malfunction ID #1",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(51, 2)]),
        v(
            "trans.fault_id_02",
            "Internal malfunction ID #2",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(51, 3)]),
        v(
            "trans.fault_id_03",
            "Internal malfunction ID #3",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(51, 4)]),
        v(
            "trans.fault_id_04",
            "Internal malfunction ID #4",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(52, 1)]),
        v(
            "trans.fault_id_05",
            "Internal malfunction ID #5",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(52, 2)]),
        v(
            "trans.fault_id_06",
            "Internal malfunction ID #6",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(52, 3)]),
        v(
            "trans.fault_id_07",
            "Internal malfunction ID #7",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(52, 4)]),
        v(
            "trans.fault_id_08",
            "Internal malfunction ID #8",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(53, 1)]),
        v(
            "trans.fault_id_09",
            "Internal malfunction ID #9",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(53, 2)]),
        v(
            "trans.fault_id_10",
            "Internal malfunction ID #10",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(53, 3)]),
        v(
            "trans.fault_id_11",
            "Internal malfunction ID #11",
            Reported,
            Transmission,
            "Faults",
        )
        .trans(&[(53, 4)]),
        v(
            "trans.flash_checksum",
            "Flash checksum",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(40, 1)]),
        v(
            "trans.param_id",
            "Parameter identifier",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(41, 1)]),
        v(
            "trans.sw_a_01",
            "Software parameter 1 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(42, 1)]),
        v(
            "trans.sw_a_02",
            "Software parameter 2 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(42, 2)]),
        v(
            "trans.sw_a_03",
            "Software parameter 3 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(42, 3)]),
        v(
            "trans.sw_a_04",
            "Software parameter 4 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(42, 4)]),
        v(
            "trans.sw_a_05",
            "Software parameter 5 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(43, 1)]),
        v(
            "trans.sw_a_06",
            "Software parameter 6 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(43, 2)]),
        v(
            "trans.sw_a_07",
            "Software parameter 7 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(43, 3)]),
        v(
            "trans.sw_a_08",
            "Software parameter 8 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(43, 4)]),
        v(
            "trans.sw_a_09",
            "Software parameter 9 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(44, 1)]),
        v(
            "trans.sw_a_10",
            "Software parameter 10 (set A)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(44, 2)]),
        v(
            "trans.sw_b_01",
            "Software parameter 1 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(45, 1)]),
        v(
            "trans.sw_b_02",
            "Software parameter 2 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(45, 2)]),
        v(
            "trans.sw_b_03",
            "Software parameter 3 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(45, 3)]),
        v(
            "trans.sw_b_04",
            "Software parameter 4 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(45, 4)]),
        v(
            "trans.sw_b_05",
            "Software parameter 5 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(46, 1)]),
        v(
            "trans.sw_b_06",
            "Software parameter 6 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(46, 2)]),
        v(
            "trans.sw_b_07",
            "Software parameter 7 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(46, 3)]),
        v(
            "trans.sw_b_08",
            "Software parameter 8 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(46, 4)]),
        v(
            "trans.sw_b_09",
            "Software parameter 9 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(47, 1)]),
        v(
            "trans.sw_b_10",
            "Software parameter 10 (set B)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(47, 2)]),
        v(
            "trans.sw_id",
            "Software identifier",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(48, 1)]),
        v(
            "trans.lever_part_no",
            "Selector lever part number (E313)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(50, 1)]),
        v(
            "trans.sv_param",
            "SV parameter",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(54, 1)]),
        v(
            "trans.ident",
            "Identification",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(80, 1)]),
        v(
            "trans.revision",
            "Revision",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(80, 2)]),
        v(
            "trans.production_date",
            "Production date",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(80, 3)]),
        v(
            "trans.test_stand",
            "Manufacturer & test stand number",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(80, 4)]),
        v(
            "trans.serial_no",
            "Serial number",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(81, 1)]),
        v(
            "trans.flash_tool_code",
            "Flash tool code (FTC)",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(82, 1)]),
        v(
            "trans.flash_date",
            "Flash date",
            Reported,
            Transmission,
            "Module info",
        )
        .trans(&[(82, 2)]),
        v(
            "awd.oil_temp_c",
            "Haldex oil temperature",
            Reported,
            Haldex,
            "Temps",
        )
        .awd(&[(1, 1)]),
        v(
            "awd.plate_temp_c",
            "Haldex plate temperature",
            Reported,
            Haldex,
            "Temps",
        )
        .awd(&[(1, 2)]),
        v(
            "awd.oil_pressure_bar",
            "Haldex oil pressure",
            Reported,
            Haldex,
            "Control",
        )
        .awd(&[(3, 1)]),
        // Estimated, per the old catalog.
        v(
            "awd.estimated_torque_nm",
            "Haldex estimated torque",
            Reported,
            Haldex,
            "Control",
        )
        .awd(&[(3, 2)]),
        v(
            "awd.valve_opening_pct",
            "Haldex clutch valve N273 opening",
            Reported,
            Haldex,
            "Control",
        )
        .awd(&[(3, 3)]),
        v(
            "awd.valve_current",
            "Haldex clutch valve N273 current",
            Reported,
            Haldex,
            "Control",
        )
        .awd(&[(3, 4)]),
        v(
            "awd.supply_v",
            "Haldex terminal 30 voltage",
            Reported,
            Haldex,
            "Supply",
        )
        .awd(&[(1, 3)]),
        v(
            "awd.can_outputs",
            "Haldex CAN output signals",
            Reported,
            Haldex,
            "Status",
        )
        .awd(&[(5, 1)]),
        v(
            "awd.vehicle_mode",
            "Haldex vehicle mode",
            Reported,
            Haldex,
            "Status",
        )
        .awd(&[(5, 2)]),
        v(
            "awd.slip_control",
            "Haldex slip control",
            Reported,
            Haldex,
            "Status",
        )
        .awd(&[(5, 3)]),
        v(
            "awd.operating_fault",
            "Haldex operating mode malfunction",
            Reported,
            Haldex,
            "Status",
        )
        .awd(&[(5, 4)]),
        v(
            "awd.timer_temp",
            "Haldex timer temperature",
            Reported,
            Haldex,
            "Timer",
        )
        .awd(&[(2, 1)]),
        v(
            "awd.timer_hours",
            "Haldex timer operating time",
            Reported,
            Haldex,
            "Timer",
        )
        .awd(&[(2, 2)]),
        v(
            "awd.pump_frequency",
            "Haldex pump running frequency",
            Reported,
            Haldex,
            "Pump",
        )
        .awd(&[(2, 3)]),
        v(
            "awd.pump_enabled",
            "Haldex pump V181 enabled",
            Reported,
            Haldex,
            "Pump",
        )
        .awd(&[(4, 1)]),
        v(
            "awd.pump_running",
            "Haldex pump V181 running",
            Reported,
            Haldex,
            "Pump",
        )
        .awd(&[(4, 2)]),
        v(
            "awd.pump_actuation",
            "Haldex pump V181 actuation",
            Reported,
            Haldex,
            "Pump",
        )
        .awd(&[(4, 3)]),
        v(
            "awd.pump_current",
            "Haldex pump V181 current",
            Reported,
            Haldex,
            "Pump",
        )
        .awd(&[(4, 4)]),
        v(
            "awd.engine_torque",
            "Engine torque (Haldex)",
            Reported,
            Haldex,
            "Inputs",
        )
        .awd(&[(10, 1)]),
        v(
            "awd.vehicle_speed",
            "Vehicle speed (Haldex)",
            Reported,
            Haldex,
            "Inputs",
        )
        .awd(&[(10, 2)]),
        v(
            "awd.steering_angle",
            "Steering angle (Haldex)",
            Reported,
            Haldex,
            "Inputs",
        )
        .awd(&[(10, 3)]),
        v(
            "awd.specified_pressure",
            "Haldex specified pressure",
            Reported,
            Haldex,
            "Inputs",
        )
        .awd(&[(10, 4)]),
        v(
            "awd.plausibility",
            "Haldex plausibility detection",
            Reported,
            Haldex,
            "Checks",
        )
        .awd(&[(6, 1)]),
        v(
            "awd.module_error",
            "Haldex control module error",
            Reported,
            Haldex,
            "Checks",
        )
        .awd(&[(6, 4)]),
        v(
            "awd.offroad",
            "Haldex offroad mode",
            Reported,
            Haldex,
            "Checks",
        )
        .awd(&[(7, 1)]),
        v(
            "awd.coding",
            "Haldex control module coding",
            Reported,
            Haldex,
            "Module info",
        )
        .awd(&[(2, 4), (7, 3)]),
        v(
            "awd.manufacturer",
            "Vehicle manufacturer (Haldex)",
            Reported,
            Haldex,
            "Module info",
        )
        .awd(&[(13, 1)]),
        v(
            "awd.platform",
            "Vehicle platform and type (Haldex)",
            Reported,
            Haldex,
            "Module info",
        )
        .awd(&[(14, 1)]),
        v(
            "awd.valve_curve",
            "Haldex valve current characteristic",
            Reported,
            Haldex,
            "Module info",
        )
        .awd(&[(25, 1)]),
        v(
            "awd.flash_ok_count",
            "Haldex successful flash operations",
            Reported,
            Haldex,
            "Module info",
        )
        .awd(&[(75, 1)]),
        v(
            "awd.flash_attempts",
            "Haldex flash attempts",
            Reported,
            Haldex,
            "Module info",
        )
        .awd(&[(75, 2)]),
        v(
            "awd.inconsistency",
            "Haldex inconsistency bit",
            Reported,
            Haldex,
            "Module info",
        )
        .awd(&[(75, 3)]),
        v(
            "awd.mode",
            "Haldex mode (applied)",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.mode"),
        v(
            "awd.commanded_nm",
            "Commanded torque (B08)",
            Expect("Nm"),
            Haldex,
            "Telemetry",
        )
        .ican("awd.commanded_nm"),
        v(
            "awd.reference_nm",
            "Final reference torque (A74)",
            Expect("Nm"),
            Haldex,
            "Telemetry",
        )
        .ican("awd.reference_nm"),
        v(
            "awd.ceiling_nm",
            "Torque ceiling (A72)",
            Expect("Nm"),
            Haldex,
            "Telemetry",
        )
        .ican("awd.ceiling_nm"),
        v(
            "awd.slip_nm",
            "Slip control torque (A7C)",
            Expect("Nm"),
            Haldex,
            "Telemetry",
        )
        .ican("awd.slip_nm"),
        v(
            "awd.model_yaw_dps",
            "Model yaw rate",
            Expect("deg/s"),
            Haldex,
            "Telemetry",
        )
        .ican("awd.model_yaw_dps"),
        v(
            "awd.measured_yaw_dps",
            "Measured yaw rate",
            Expect("deg/s"),
            Haldex,
            "Telemetry",
        )
        .ican("awd.measured_yaw_dps"),
        v(
            "awd.target_gear",
            "Target gear (Haldex input)",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.target_gear"),
        v(
            "awd.abs_braking",
            "ABS braking (Haldex)",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.abs_braking"),
        v(
            "awd.force_zero",
            "Force zero torque (A78)",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.force_zero"),
        v(
            "awd.token_ok",
            "Mode token accepted",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.token_ok"),
        v(
            "awd.selector_b1cc",
            "Selector (B1CC)",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.selector_b1cc"),
        v(
            "awd.c06_raw",
            "Proactive reference (C06)",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.c06_raw"),
        v(
            "awd.c22_raw",
            "Pre-reference (C22)",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.c22_raw"),
        v(
            "awd.accel_demand_raw",
            "Demanded accel (C9E), raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.accel_demand_raw"),
        v(
            "awd.accel_actual_raw",
            "Actual accel (C9C), raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.accel_actual_raw"),
        v(
            "awd.lateral_accel_raw",
            "Lateral accel, raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.lateral_accel_raw"),
        v(
            "awd.lateral_ff_raw",
            "Lateral feed-forward (B26), raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.lateral_ff_raw"),
        v(
            "awd.curvature_raw",
            "Curvature (BC4), raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.curvature_raw"),
        v(
            "awd.axle_slip_raw",
            "Axle slip (BB6), raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.axle_slip_raw"),
        v(
            "awd.throttle_raw",
            "Throttle input, raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.throttle_raw"),
        v(
            "awd.brake_light_raw",
            "Brake light input, raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.brake_light_raw"),
        v(
            "awd.hold_timer",
            "Hold timer (A7E)",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.hold_timer"),
        v(
            "awd.high_gear_factor_raw",
            "High-gear factor (C12), raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.high_gear_factor_raw"),
        v(
            "awd.slip_energy_raw",
            "Slip energy (C3A), raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.slip_energy_raw"),
        v(
            "awd.energy_ceiling_raw",
            "Energy ceiling (C26), raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.energy_ceiling_raw"),
        v(
            "awd.derate_ceiling_raw",
            "Derate ceiling (AFE), raw",
            Reported,
            Haldex,
            "Telemetry",
        )
        .ican("awd.derate_ceiling_raw"),
        v("fuel.level_l", "Fuel level", Expect("L"), FuelTrip, "Fuel").ican("fuel.level_l"),
        v("fuel.low_warning", "Fuel low", Reported, FuelTrip, "Fuel").ican("fuel.low_warning"),
        v("fuel.range_km", "Range", Expect("km"), FuelTrip, "Fuel").ican("fuel.range_km"),
        v("fuel.range_mi", "Range", Expect("mi"), FuelTrip, "Fuel").ican("fuel.range_mi"),
        v(
            "fuel.time_left_min",
            "Time left on the tank",
            Expect("min"),
            FuelTrip,
            "Fuel",
        )
        .ican("fuel.time_left_min"),
        v(
            "fuel.consumption_l100km",
            "Consumption now",
            Expect("l/100km"),
            FuelTrip,
            "Fuel",
        )
        .ican("fuel.consumption_l100km"),
        v(
            "fuel.consumption_kml",
            "Consumption now",
            Expect("km/l"),
            FuelTrip,
            "Fuel",
        )
        .ican("fuel.consumption_kml"),
        v(
            "fuel.consumption_mpg",
            "Consumption now",
            Expect("mpg"),
            FuelTrip,
            "Fuel",
        )
        .ican("fuel.consumption_mpg"),
        v(
            "ambient.outside_temp_c",
            "Outside temperature",
            Reported,
            FuelTrip,
            "Outside",
        )
        .ican("ambient.outside_temp_c"),
        v(
            "ambient.unfiltered_temp_c",
            "Outside temperature (unfiltered)",
            Expect("°C"),
            FuelTrip,
            "Outside",
        )
        .ican("ambient.unfiltered_temp_c"),
        v(
            "ambient.ecu_temp_c",
            "Ambient temperature (engine diagnostic)",
            Expect("°C"),
            FuelTrip,
            "Outside",
        )
        .engine(&[(134, 2)]),
        v(
            "trip.short.consumption_l100km",
            "Consumption (since start)",
            Expect("l/100km"),
            FuelTrip,
            "Since start",
        )
        .ican("trip.short.consumption_l100km"),
        v(
            "trip.short.consumption_kml",
            "Consumption (since start)",
            Expect("km/l"),
            FuelTrip,
            "Since start",
        )
        .ican("trip.short.consumption_kml"),
        v(
            "trip.short.consumption_mpg",
            "Consumption (since start)",
            Expect("mpg"),
            FuelTrip,
            "Since start",
        )
        .ican("trip.short.consumption_mpg"),
        v(
            "trip.short.distance_km",
            "Distance (since start)",
            Expect("km"),
            FuelTrip,
            "Since start",
        )
        .ican("trip.short.distance_km"),
        v(
            "trip.short.distance_mi",
            "Distance (since start)",
            Expect("mi"),
            FuelTrip,
            "Since start",
        )
        .ican("trip.short.distance_mi"),
        v(
            "trip.short.time_min",
            "Driving time (since start)",
            Expect("min"),
            FuelTrip,
            "Since start",
        )
        .ican("trip.short.time_min"),
        v(
            "trip.short.speed_kmh",
            "Average speed (since start)",
            Expect("km/h"),
            FuelTrip,
            "Since start",
        )
        .ican("trip.short.speed_kmh"),
        v(
            "trip.short.speed_mph",
            "Average speed (since start)",
            Expect("mph"),
            FuelTrip,
            "Since start",
        )
        .ican("trip.short.speed_mph"),
        v(
            "trip.long.consumption_l100km",
            "Consumption (long term)",
            Expect("l/100km"),
            FuelTrip,
            "Long term",
        )
        .ican("trip.long.consumption_l100km"),
        v(
            "trip.long.consumption_kml",
            "Consumption (long term)",
            Expect("km/l"),
            FuelTrip,
            "Long term",
        )
        .ican("trip.long.consumption_kml"),
        v(
            "trip.long.consumption_mpg",
            "Consumption (long term)",
            Expect("mpg"),
            FuelTrip,
            "Long term",
        )
        .ican("trip.long.consumption_mpg"),
        v(
            "trip.long.distance_km",
            "Distance (long term)",
            Expect("km"),
            FuelTrip,
            "Long term",
        )
        .ican("trip.long.distance_km"),
        v(
            "trip.long.distance_mi",
            "Distance (long term)",
            Expect("mi"),
            FuelTrip,
            "Long term",
        )
        .ican("trip.long.distance_mi"),
        v(
            "trip.long.time_min",
            "Driving time (long term)",
            Expect("min"),
            FuelTrip,
            "Long term",
        )
        .ican("trip.long.time_min"),
        v(
            "trip.long.speed_kmh",
            "Average speed (long term)",
            Expect("km/h"),
            FuelTrip,
            "Long term",
        )
        .ican("trip.long.speed_kmh"),
        v(
            "trip.long.speed_mph",
            "Average speed (long term)",
            Expect("mph"),
            FuelTrip,
            "Long term",
        )
        .ican("trip.long.speed_mph"),
        v(
            "trip.tank.consumption_l100km",
            "Consumption (this tank)",
            Expect("l/100km"),
            FuelTrip,
            "This tank",
        )
        .ican("trip.tank.consumption_l100km"),
        v(
            "trip.tank.consumption_kml",
            "Consumption (this tank)",
            Expect("km/l"),
            FuelTrip,
            "This tank",
        )
        .ican("trip.tank.consumption_kml"),
        v(
            "trip.tank.consumption_mpg",
            "Consumption (this tank)",
            Expect("mpg"),
            FuelTrip,
            "This tank",
        )
        .ican("trip.tank.consumption_mpg"),
        v(
            "trip.tank.distance_km",
            "Distance (this tank)",
            Expect("km"),
            FuelTrip,
            "This tank",
        )
        .ican("trip.tank.distance_km"),
        v(
            "trip.tank.distance_mi",
            "Distance (this tank)",
            Expect("mi"),
            FuelTrip,
            "This tank",
        )
        .ican("trip.tank.distance_mi"),
        v(
            "trip.tank.speed_kmh",
            "Average speed (this tank)",
            Expect("km/h"),
            FuelTrip,
            "This tank",
        )
        .ican("trip.tank.speed_kmh"),
        v(
            "trip.tank.speed_mph",
            "Average speed (this tank)",
            Expect("mph"),
            FuelTrip,
            "This tank",
        )
        .ican("trip.tank.speed_mph"),
        v(
            "trip.tank_prev.consumption_l100km",
            "Consumption (last tank)",
            Expect("l/100km"),
            FuelTrip,
            "Last tank",
        )
        .ican("trip.tank_prev.consumption_l100km"),
        v(
            "trip.tank_prev.consumption_kml",
            "Consumption (last tank)",
            Expect("km/l"),
            FuelTrip,
            "Last tank",
        )
        .ican("trip.tank_prev.consumption_kml"),
        v(
            "trip.tank_prev.consumption_mpg",
            "Consumption (last tank)",
            Expect("mpg"),
            FuelTrip,
            "Last tank",
        )
        .ican("trip.tank_prev.consumption_mpg"),
        v(
            "trip.tank_prev.distance_km",
            "Distance (last tank)",
            Expect("km"),
            FuelTrip,
            "Last tank",
        )
        .ican("trip.tank_prev.distance_km"),
        v(
            "trip.tank_prev.distance_mi",
            "Distance (last tank)",
            Expect("mi"),
            FuelTrip,
            "Last tank",
        )
        .ican("trip.tank_prev.distance_mi"),
        v(
            "trip.tank_prev.speed_kmh",
            "Average speed (last tank)",
            Expect("km/h"),
            FuelTrip,
            "Last tank",
        )
        .ican("trip.tank_prev.speed_kmh"),
        v(
            "trip.tank_prev.speed_mph",
            "Average speed (last tank)",
            Expect("mph"),
            FuelTrip,
            "Last tank",
        )
        .ican("trip.tank_prev.speed_mph"),
        v(
            "trip.drive_time_min",
            "Driving without a break",
            Expect("min"),
            FuelTrip,
            "Driving time",
        )
        .ican("trip.drive_time_min"),
        v(
            "power.battery_v",
            "Battery",
            Reported,
            Electrical,
            "Battery",
        )
        .ican("power.battery_v"),
        v(
            "engine.terminal30_v",
            "Terminal 30 voltage (engine ECU)",
            Expect("V"),
            Electrical,
            "Battery",
        )
        .engine(&[(4, 2), (51, 4), (53, 3), (61, 2)]),
        v(
            "power.battery_soc_pct",
            "Battery state of charge",
            Expect("%"),
            Electrical,
            "Battery",
        )
        .ican("power.battery_soc_pct"),
        v(
            "power.bdm_v",
            "Battery monitor voltage",
            Expect("V"),
            Electrical,
            "Battery",
        )
        .ican("power.bdm_v"),
        v(
            "power.battery_warning",
            "Battery warning",
            Reported,
            Electrical,
            "Battery",
        )
        .ican("power.battery_warning"),
        v(
            "engine.generator_pct",
            "Generator load (DFM)",
            Expect("%"),
            Electrical,
            "Charging",
        )
        .ican("engine.generator_pct")
        .engine(&[(53, 4)]),
        v(
            "power.generator_target_v",
            "Generator target voltage",
            Expect("V"),
            Electrical,
            "Charging",
        )
        .ican("power.generator_target_v"),
        v(
            "power.generator_warning",
            "Generator warning",
            Reported,
            Electrical,
            "Charging",
        )
        .ican("power.generator_warning"),
        v(
            "power.load_shed_stage",
            "Load shedding stage",
            Reported,
            Electrical,
            "Charging",
        )
        .ican("power.load_shed_stage"),
        v(
            "ignition.kl_s",
            "Key in (S)",
            Reported,
            Electrical,
            "Ignition",
        )
        .ican("ignition.kl_s"),
        v(
            "ignition.kl15",
            "Ignition (15)",
            Reported,
            Electrical,
            "Ignition",
        )
        .ican("ignition.kl15"),
        v(
            "ignition.kl_x",
            "Accessories (X)",
            Reported,
            Electrical,
            "Ignition",
        )
        .ican("ignition.kl_x"),
        v(
            "ignition.kl50",
            "Starter (50)",
            Reported,
            Electrical,
            "Ignition",
        )
        .ican("ignition.kl50"),
        v(
            "ignition.kl_p",
            "Parking lights (P)",
            Reported,
            Electrical,
            "Ignition",
        )
        .ican("ignition.kl_p"),
        v(
            "ignition.zas.s",
            "Key switch S",
            Reported,
            Electrical,
            "ZAS",
        )
        .ican("ignition.zas.s"),
        v(
            "ignition.zas.kl15",
            "Key switch 15",
            Reported,
            Electrical,
            "ZAS",
        )
        .ican("ignition.zas.kl15"),
        v(
            "ignition.zas.x",
            "Key switch X",
            Reported,
            Electrical,
            "ZAS",
        )
        .ican("ignition.zas.x"),
        v(
            "ignition.zas.kl50",
            "Key switch 50",
            Reported,
            Electrical,
            "ZAS",
        )
        .ican("ignition.zas.kl50"),
        v(
            "bcm.load_mgmt",
            "Load management stage",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(31, 1)]),
        v(
            "bcm.kl30_v",
            "Terminal 30 voltage (BCM)",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(31, 2)]),
        v(
            "bcm.generator_load",
            "Generator load (BCM)",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(31, 3)]),
        v(
            "bcm.short_trips",
            "Short-distance operations",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(31, 4)]),
        v(
            "bcm.critical_time",
            "Load management critical time",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(32, 1)]),
        v(
            "bcm.transport_mode_v",
            "Transport mode no-load voltage",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(32, 2)]),
        v(
            "bcm.shutoff_count",
            "Load shut-off counter",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(32, 3)]),
        v(
            "bcm.shutoff_prio_a",
            "Shut-off priority A (engine on/start)",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(33, 1)]),
        v(
            "bcm.shutoff_prio_b",
            "Shut-off priority B (terminal 15 on)",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(33, 2)]),
        v(
            "bcm.shutoff_prio_c",
            "Shut-off priority C (terminal 15 off)",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(33, 3)]),
        v(
            "bcm.shutoff_prio_d",
            "Shut-off priority D (short circuit)",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(33, 4)]),
        v(
            "bcm.shutoff_protect_1",
            "Max. shut-off protection 1",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(34, 1)]),
        v(
            "bcm.shutoff_protect_2",
            "Max. shut-off protection 2",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(35, 1)]),
        v(
            "bcm.quiescent_recent_v",
            "Quiescent voltage, recent phase",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(36, 1)]),
        v(
            "bcm.quiescent_second_v",
            "Quiescent voltage, second phase",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(36, 2)]),
        v(
            "bcm.quiescent_third_v",
            "Quiescent voltage, third phase",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(36, 3)]),
        v(
            "bcm.quiescent_total_v",
            "Quiescent voltage, all phases",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(36, 4)]),
        v(
            "bcm.voltage_diff_mean",
            "Voltage difference mean value",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(37, 1)]),
        v(
            "bcm.battery_disconnects",
            "Battery disconnect counter",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(37, 2)]),
        v(
            "bcm.seat_heat_shutoff",
            "Seat heating shut-off",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(37, 3)]),
        v(
            "bcm.kl30_left_v",
            "Terminal 30 voltage left",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(38, 1)]),
        v(
            "bcm.kl30_right_v",
            "Terminal 30 voltage right",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(38, 2)]),
        v(
            "bcm.kl30_ref_v",
            "Terminal 30 voltage reference",
            Reported,
            Electrical,
            "Load management",
        )
        .bcm(&[(38, 3)]),
        v(
            "bcm.kl_s_hw",
            "S-contact (hardware)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(61, 1)]),
        v(
            "bcm.kl_s_zas",
            "S-contact (CAN in, ZAS)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(61, 3)]),
        v(
            "bcm.kl_s_out",
            "S-contact (CAN out, BCM)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(61, 4)]),
        v(
            "bcm.kl15_hw",
            "Terminal 15 (hardware)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(62, 1)]),
        v(
            "bcm.kl15_zas",
            "Terminal 15 (CAN in, ZAS)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(62, 3)]),
        v(
            "bcm.kl15_out",
            "Terminal 15 (CAN out, BCM)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(62, 4)]),
        v(
            "bcm.kl50_hw",
            "Terminal 50 (hardware)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(63, 1)]),
        v(
            "bcm.kl50_fb",
            "Terminal 50 (feedback)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(63, 2)]),
        v(
            "bcm.kl50_zas",
            "Terminal 50 (CAN in, ZAS)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(63, 3)]),
        v(
            "bcm.kl50_out",
            "Terminal 50 (CAN out, BCM)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(63, 4)]),
        v(
            "bcm.pn_hw",
            "Release P/N (hardware)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(64, 1)]),
        v(
            "bcm.pn_zas",
            "Release P/N (CAN in, ZAS)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(64, 3)]),
        v(
            "bcm.pn_out",
            "Release P/N (CAN out, BCM)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(64, 4)]),
        v(
            "bcm.kl75_zas",
            "Terminal 75 (CAN in, ZAS)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(65, 3)]),
        v(
            "bcm.kl75_out",
            "Terminal 75 (CAN out, BCM)",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(65, 4)]),
        v(
            "bcm.kl30g_relay",
            "Terminal 30G relay",
            Reported,
            Electrical,
            "Terminals (BCM)",
        )
        .bcm(&[(54, 3)]),
        v(
            "bcm.vehicle_speed",
            "Vehicle speed (BCM)",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(71, 1), (74, 3)]),
        v(
            "bcm.reverse",
            "Reverse gear engaged (BCM)",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(55, 1)]),
        v(
            "bcm.parking_brake",
            "Parking brake (BCM)",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(74, 1)]),
        v(
            "bcm.crash_signal",
            "Crash signal",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(71, 2)]),
        v(
            "bcm.engine_design",
            "Engine design / automatic start",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(73, 1)]),
        v(
            "bcm.special_vehicle",
            "Special vehicle activation status",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(73, 2)]),
        v(
            "bcm.fuel_pump_relay",
            "Electric fuel pump relay",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(72, 4)]),
        v(
            "bcm.solenoid",
            "Solenoid (BCM)",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(75, 1)]),
        v(
            "bcm.reset_count",
            "BCM reset counter",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(76, 1)]),
        v(
            "bcm.reset_cause",
            "BCM cause of last reset",
            Reported,
            Electrical,
            "BCM inputs",
        )
        .bcm(&[(76, 2)]),
        v("body.door_fl_open", "Door FL open", Reported, Body, "Doors").ican("body.door_fl_open"),
        v("body.door_fr_open", "Door FR open", Reported, Body, "Doors").ican("body.door_fr_open"),
        v("body.door_rl_open", "Door RL open", Reported, Body, "Doors").ican("body.door_rl_open"),
        v("body.door_rr_open", "Door RR open", Reported, Body, "Doors").ican("body.door_rr_open"),
        v("body.hood_open", "Bonnet open", Reported, Body, "Doors").ican("body.hood_open"),
        v(
            "climate.cabin_temp_c",
            "Cabin temperature",
            Expect("°C"),
            Body,
            "Climate",
        )
        .ican("climate.cabin_temp_c"),
        v(
            "climate.outside_temp_c",
            "Outside temperature (climatronic)",
            Expect("°C"),
            Body,
            "Climate",
        )
        .ican("climate.outside_temp_c"),
        v("climate.blower_pct", "Blower", Expect("%"), Body, "Climate").ican("climate.blower_pct"),
        v(
            "climate.refrigerant_bar",
            "Refrigerant pressure",
            Expect("bar"),
            Body,
            "Climate",
        )
        .ican("climate.refrigerant_bar"),
        v(
            "climate.compressor_nm",
            "Compressor load",
            Expect("Nm"),
            Body,
            "Climate",
        )
        .ican("climate.compressor_nm"),
        v(
            "climate.fan_pct",
            "Radiator fan request (climatronic)",
            Expect("%"),
            Body,
            "Climate",
        )
        .ican("climate.fan_pct"),
        v("climate.on", "Climate on", Reported, Body, "Climate").ican("climate.on"),
        v("climate.ac_on", "A/C on", Reported, Body, "Climate").ican("climate.ac_on"),
        v(
            "climate.compressor_on",
            "Compressor on",
            Reported,
            Body,
            "Climate",
        )
        .ican("climate.compressor_on"),
        v(
            "climate.recirculation",
            "Recirculation",
            Reported,
            Body,
            "Climate",
        )
        .ican("climate.recirculation"),
        v("climate.defrost", "Defrost", Reported, Body, "Climate").ican("climate.defrost"),
        v("climate.econ", "Econ", Reported, Body, "Climate").ican("climate.econ"),
        v(
            "climate.rear_window_heat",
            "Rear window heat",
            Reported,
            Body,
            "Climate",
        )
        .ican("climate.rear_window_heat"),
        v(
            "climate.wheel_heat",
            "Steering wheel heat",
            Reported,
            Body,
            "Climate",
        )
        .ican("climate.wheel_heat"),
        v(
            "climate.aux_heat",
            "Auxiliary heater",
            Reported,
            Body,
            "Climate",
        )
        .ican("climate.aux_heat"),
        v(
            "climate.fahrenheit",
            "Climatronic shows °F",
            Reported,
            Body,
            "Climate",
        )
        .ican("climate.fahrenheit"),
        v(
            "climate.fault_stored",
            "Climatronic fault stored",
            Reported,
            Body,
            "Climate",
        )
        .ican("climate.fault_stored"),
        v(
            "parking.fl_cm",
            "Parking distance FL",
            Expect("cm"),
            Body,
            "Parking",
        )
        .ican("parking.fl_cm"),
        v(
            "parking.fml_cm",
            "Parking distance front mid-left",
            Expect("cm"),
            Body,
            "Parking",
        )
        .ican("parking.fml_cm"),
        v(
            "parking.fmr_cm",
            "Parking distance front mid-right",
            Expect("cm"),
            Body,
            "Parking",
        )
        .ican("parking.fmr_cm"),
        v(
            "parking.fr_cm",
            "Parking distance FR",
            Expect("cm"),
            Body,
            "Parking",
        )
        .ican("parking.fr_cm"),
        v(
            "parking.rl_cm",
            "Parking distance RL",
            Expect("cm"),
            Body,
            "Parking",
        )
        .ican("parking.rl_cm"),
        v(
            "parking.rml_cm",
            "Parking distance rear mid-left",
            Expect("cm"),
            Body,
            "Parking",
        )
        .ican("parking.rml_cm"),
        v(
            "parking.rmr_cm",
            "Parking distance rear mid-right",
            Expect("cm"),
            Body,
            "Parking",
        )
        .ican("parking.rmr_cm"),
        v(
            "parking.rr_cm",
            "Parking distance RR",
            Expect("cm"),
            Body,
            "Parking",
        )
        .ican("parking.rr_cm"),
        v(
            "parking.ready",
            "Parking aid ready",
            Reported,
            Body,
            "Parking",
        )
        .ican("parking.ready"),
        v(
            "parking.reverse_on",
            "Parking aid on (reverse)",
            Reported,
            Body,
            "Parking",
        )
        .ican("parking.reverse_on"),
        v(
            "parking.button_on",
            "Parking aid on (button)",
            Reported,
            Body,
            "Parking",
        )
        .ican("parking.button_on"),
        v(
            "parking.fault",
            "Parking aid fault",
            Reported,
            Body,
            "Parking",
        )
        .ican("parking.fault"),
        v(
            "bcm.driver_key_switch",
            "Driver door key switch",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(1, 1)]),
        v(
            "bcm.driver_lock_switch",
            "Driver door interior lock switch",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(1, 2)]),
        v(
            "bcm.driver_door",
            "Driver door status",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(1, 3)]),
        v(
            "bcm.lock_fb_driver",
            "Central locking feedback, driver (F220)",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(1, 4)]),
        v(
            "bcm.pass_key_switch",
            "Passenger door key switch",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(2, 1)]),
        v(
            "bcm.pass_lock_switch",
            "Passenger door interior lock switch",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(2, 2)]),
        v(
            "bcm.pass_rotary",
            "Passenger door rotary switch (F3)",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(2, 3)]),
        v(
            "bcm.lock_fb_pass",
            "Central locking feedback, passenger (F221)",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(2, 4)]),
        v(
            "bcm.rl_lock_switch",
            "Rear left door interior lock switch",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(3, 2)]),
        v(
            "bcm.rl_rotary",
            "Rear left door rotary switch (F10)",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(3, 3)]),
        v(
            "bcm.lock_fb_rl",
            "Central locking feedback, rear left (F222)",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(3, 4)]),
        v(
            "bcm.rr_lock_switch",
            "Rear right door interior lock switch",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(4, 2)]),
        v(
            "bcm.rr_rotary",
            "Rear right door rotary switch (F11)",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(4, 3)]),
        v(
            "bcm.lock_fb_rr",
            "Central locking feedback, rear right (F223)",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(4, 4)]),
        v(
            "bcm.fuel_door_switch",
            "Interior fuel door unlock switch",
            Reported,
            Body,
            "Locks (BCM)",
        )
        .bcm(&[(6, 4)]),
        v(
            "bcm.tailgate_key_switch",
            "Tailgate key switch",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(5, 1)]),
        v(
            "bcm.tailgate_lock_fb",
            "Central locking feedback, tailgate",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(5, 2)]),
        v(
            "bcm.tailgate_prelatch",
            "Tailgate preliminary latch (F256)",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(5, 3)]),
        v(
            "bcm.tailgate_main_latch",
            "Tailgate main latch (F256)",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(5, 4)]),
        v(
            "bcm.rear_lid_unlock_switch",
            "Rear lid unlock switch",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(6, 3)]),
        v(
            "bcm.tailgate_release",
            "Tailgate release",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(7, 1)]),
        v(
            "bcm.tailgate_ext_switch",
            "Tailgate exterior switch (E234)",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(7, 2)]),
        v(
            "bcm.tailgate_2part_switch",
            "Tailgate 2-part switch (E615)",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(7, 4)]),
        v(
            "bcm.closing_aid_left",
            "Closing aid left motor (V399)",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(11, 1)]),
        v(
            "bcm.closing_aid_right",
            "Closing aid right motor (V398)",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(11, 2)]),
        v(
            "bcm.closing_aid_limits",
            "Closing aid limit switches",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(11, 3)]),
        v(
            "bcm.tailgate_f206",
            "Tailgate switch (F206)",
            Reported,
            Body,
            "Tailgate (BCM)",
        )
        .bcm(&[(11, 4)]),
        v(
            "bcm.heated_ws_switch",
            "Heated windscreen switch",
            Reported,
            Body,
            "Heating (BCM)",
        )
        .bcm(&[(8, 1)]),
        v(
            "bcm.heated_ws_relay",
            "Heated windscreen relay",
            Reported,
            Body,
            "Heating (BCM)",
        )
        .bcm(&[(8, 2)]),
        v(
            "bcm.heated_ws_light",
            "Heated windscreen light",
            Reported,
            Body,
            "Heating (BCM)",
        )
        .bcm(&[(8, 3)]),
        v(
            "bcm.heated_jets",
            "Heated washer jets",
            Reported,
            Body,
            "Heating (BCM)",
        )
        .bcm(&[(8, 4)]),
        v(
            "bcm.rear_defrost_switch",
            "Rear defrost switch",
            Reported,
            Body,
            "Heating (BCM)",
        )
        .bcm(&[(9, 1)]),
        v(
            "bcm.rear_defrost_relay",
            "Rear defrost relay (J9)",
            Reported,
            Body,
            "Heating (BCM)",
        )
        .bcm(&[(9, 2)]),
        v(
            "bcm.hood_switch",
            "Hood switch (F266)",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(10, 1)]),
        v(
            "bcm.sunroof_enable",
            "Sunroof enable",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(14, 1)]),
        v(
            "bcm.sunroof_operation",
            "Sunroof operation",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(14, 2)]),
        v(
            "bcm.spoiler_position",
            "Spoiler position",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(81, 1)]),
        v(
            "bcm.spoiler_thermal",
            "Spoiler thermal protection",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(81, 2)]),
        v(
            "bcm.spoiler_switch_range",
            "Spoiler switch range",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(81, 3)]),
        v(
            "bcm.spoiler_led",
            "Spoiler function LED",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(81, 4)]),
        v(
            "bcm.roof_open_rest",
            "Roof open/rested",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(82, 1)]),
        v(
            "bcm.roof_closed_rest",
            "Roof closed/rested",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(82, 2)]),
        v(
            "bcm.roof_position",
            "Roof position",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(82, 3)]),
        v(
            "bcm.top_status",
            "Convertible top status",
            Reported,
            Body,
            "Hood & roof",
        )
        .bcm(&[(127, 3)]),
        v(
            "bcm.light_sensor_brightness",
            "Light sensor brightness",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(15, 1)]),
        v(
            "bcm.light_sensor_dark",
            "Light sensor light/dark",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(15, 2)]),
        v(
            "bcm.light_sensor_request",
            "Light sensor requirements",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(15, 3)]),
        v(
            "bcm.rain_sensor",
            "Rain sensor",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(15, 4)]),
        v(
            "bcm.wiper_stalk",
            "Wiper signal, steering column",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(16, 1)]),
        v(
            "bcm.front_wiper",
            "Front wiper",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(16, 2)]),
        v(
            "bcm.wiper_end_position",
            "Wiper end position",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(16, 3)]),
        v(
            "bcm.wiper_cycle",
            "Wiper cycle",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(16, 4)]),
        v(
            "bcm.washer_pump_front",
            "Washer pump (front)",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(72, 1)]),
        v(
            "bcm.washer_pump_rear",
            "Washer pump (rear)",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(72, 2)]),
        v(
            "bcm.headlight_washer_relay",
            "Headlight washer relay",
            Reported,
            Body,
            "Wipers & sensors",
        )
        .bcm(&[(72, 3)]),
        v("bcm.horn", "Horn (H1)", Reported, Body, "Misc (BCM)").bcm(&[(71, 3)]),
        v(
            "bcm.garage_door",
            "Garage door opener",
            Reported,
            Body,
            "Misc (BCM)",
        )
        .bcm(&[(71, 4)]),
        v(
            "bcm.tilt_x_ref",
            "Tilt sensor X reference angle",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(17, 1)]),
        v(
            "bcm.tilt_x_delta",
            "Tilt sensor X change in angle",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(17, 2)]),
        v(
            "bcm.tilt_y_ref",
            "Tilt sensor Y reference angle",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(17, 3)]),
        v(
            "bcm.tilt_y_delta",
            "Tilt sensor Y change in angle",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(17, 4)]),
        v(
            "bcm.tilt_angle",
            "Tilt sensor angle",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(125, 1)]),
        v("bcm.tilt_v", "Tilt sensor voltage", Reported, Body, "Alarm").bcm(&[(125, 2)]),
        v(
            "bcm.alarm_horn_src_1",
            "Alarm horn, 1st alarm source",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(18, 1)]),
        v(
            "bcm.alarm_horn_src_2",
            "Alarm horn, 2nd alarm source",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(18, 2)]),
        v(
            "bcm.alarm_horn_src_3",
            "Alarm horn, 3rd alarm source",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(18, 3)]),
        v(
            "bcm.alarm_horn_src_4",
            "Alarm horn, 4th alarm source",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(18, 4)]),
        v(
            "bcm.alarm_interior_src_1",
            "Interior monitoring, 1st alarm source",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(19, 1)]),
        v(
            "bcm.alarm_interior_src_2",
            "Interior monitoring, 2nd alarm source",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(19, 2)]),
        v(
            "bcm.alarm_interior_src_3",
            "Interior monitoring, 3rd alarm source",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(19, 3)]),
        v(
            "bcm.alarm_interior_src_4",
            "Interior monitoring, 4th alarm source",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(19, 4)]),
        v(
            "bcm.alarm_1_trigger",
            "Last alarm source trigger",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(20, 1)]),
        v(
            "bcm.alarm_1_date",
            "Last alarm source date",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(20, 2)]),
        v(
            "bcm.alarm_2_trigger",
            "Second alarm source trigger",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(21, 1)]),
        v(
            "bcm.alarm_2_date",
            "Second alarm source date",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(21, 2)]),
        v(
            "bcm.alarm_2_time",
            "Second alarm source time",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(21, 3)]),
        v(
            "bcm.alarm_2_km",
            "Second alarm source mileage",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(21, 4)]),
        v(
            "bcm.alarm_3_trigger",
            "Third alarm source trigger",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(22, 1)]),
        v(
            "bcm.alarm_3_date",
            "Third alarm source date",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(22, 2)]),
        v(
            "bcm.alarm_3_time",
            "Third alarm source time",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(22, 3)]),
        v(
            "bcm.alarm_3_km",
            "Third alarm source mileage",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(22, 4)]),
        v(
            "bcm.alarm_4_trigger",
            "Fourth alarm source trigger",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(23, 1)]),
        v(
            "bcm.alarm_4_date",
            "Fourth alarm source date",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(23, 2)]),
        v(
            "bcm.alarm_4_time",
            "Fourth alarm source time",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(23, 3)]),
        v(
            "bcm.alarm_4_km",
            "Fourth alarm source mileage",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(23, 4)]),
        v(
            "bcm.alarm_sensor_status",
            "Alarm sensor status",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(25, 1)]),
        v(
            "bcm.alarm_recognition",
            "Alarm recognition",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(25, 2), (26, 2)]),
        v(
            "bcm.alarm_sensor_switch",
            "Alarm sensor switch",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(25, 3)]),
        v(
            "bcm.alarm_led",
            "Alarm LED function",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(25, 4)]),
        v(
            "bcm.siren_status",
            "Alarm siren status",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(26, 1)]),
        v(
            "bcm.alarm_horn_active",
            "Alarm horn active",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(26, 3)]),
        v(
            "bcm.alarm_horn_atws",
            "Alarm horn (ATWS horn)",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(26, 4)]),
        v(
            "bcm.siren_battery",
            "Siren battery condition",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(126, 1)]),
        v(
            "bcm.siren_low_v",
            "Siren low voltage",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(126, 2)]),
        v(
            "bcm.alarm_sensor_state_127",
            "Alarm sensor status (group 127)",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(127, 1)]),
        v(
            "bcm.alarm_sensor_low_v",
            "Alarm sensor low voltage",
            Reported,
            Body,
            "Alarm",
        )
        .bcm(&[(127, 2)]),
        v(
            "bcm.remote_key_number",
            "Remote key number",
            Reported,
            Body,
            "Remotes",
        )
        .bcm(&[(91, 1)]),
        v(
            "bcm.remote_buttons",
            "Remote control buttons",
            Reported,
            Body,
            "Remotes",
        )
        .bcm(&[(91, 2)]),
        v(
            "bcm.immo_key_recognition",
            "Immobilizer key recognition",
            Reported,
            Body,
            "Remotes",
        )
        .bcm(&[(91, 3)]),
        v(
            "bcm.remotes_adapted",
            "Number of adapted remotes",
            Reported,
            Body,
            "Remotes",
        )
        .bcm(&[(91, 4)]),
        v(
            "bcm.remote_fixed_code",
            "Remote fixed code",
            Reported,
            Body,
            "Remotes",
        )
        .bcm(&[(92, 1)]),
        v(
            "bcm.remote_in_range",
            "Remote code in reception range",
            Reported,
            Body,
            "Remotes",
        )
        .bcm(&[(92, 2)]),
        v(
            "bcm.remote_variable_code",
            "Remote variable code algorithm",
            Reported,
            Body,
            "Remotes",
        )
        .bcm(&[(92, 3)]),
        v(
            "bcm.thermal_central_locking",
            "Central locking thermal protection",
            Reported,
            Body,
            "Thermal protection",
        )
        .bcm(&[(83, 1)]),
        v(
            "bcm.thermal_rearlid",
            "Rear lid thermal protection",
            Reported,
            Body,
            "Thermal protection",
        )
        .bcm(&[(83, 2)]),
        v(
            "bcm.thermal_tank_flap",
            "Tank flap thermal protection",
            Reported,
            Body,
            "Thermal protection",
        )
        .bcm(&[(83, 3)]),
        v(
            "bcm.thermal_rear_window",
            "Rear window thermal protection",
            Reported,
            Body,
            "Thermal protection",
        )
        .bcm(&[(83, 4)]),
        v(
            "bcm.thermal_lid_right",
            "Rear lid right thermal protection",
            Reported,
            Body,
            "Thermal protection",
        )
        .bcm(&[(84, 1)]),
        v(
            "bcm.thermal_lid_left",
            "Rear lid left thermal protection",
            Reported,
            Body,
            "Thermal protection",
        )
        .bcm(&[(84, 2)]),
        v(
            "bcm.thermal_closing_aid",
            "Closing aid thermal protection",
            Reported,
            Body,
            "Thermal protection",
        )
        .bcm(&[(84, 3)]),
        v(
            "bcm.central_hist_01_entry",
            "Central lock history I: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(101, 1)]),
        v(
            "bcm.central_hist_01_op_a",
            "Central lock history I: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(101, 2)]),
        v(
            "bcm.central_hist_01_op_b",
            "Central lock history I: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(101, 3)]),
        v(
            "bcm.central_hist_01_log",
            "Central lock history I: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(101, 4)]),
        v(
            "bcm.central_hist_02_entry",
            "Central lock history II: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(102, 1)]),
        v(
            "bcm.central_hist_02_op_a",
            "Central lock history II: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(102, 2)]),
        v(
            "bcm.central_hist_02_op_b",
            "Central lock history II: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(102, 3)]),
        v(
            "bcm.central_hist_02_log",
            "Central lock history II: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(102, 4)]),
        v(
            "bcm.central_hist_03_entry",
            "Central lock history III: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(103, 1)]),
        v(
            "bcm.central_hist_03_op_a",
            "Central lock history III: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(103, 2)]),
        v(
            "bcm.central_hist_03_op_b",
            "Central lock history III: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(103, 3)]),
        v(
            "bcm.central_hist_03_log",
            "Central lock history III: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(103, 4)]),
        v(
            "bcm.central_hist_04_entry",
            "Central lock history IV: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(104, 1)]),
        v(
            "bcm.central_hist_04_op_a",
            "Central lock history IV: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(104, 2)]),
        v(
            "bcm.central_hist_04_op_b",
            "Central lock history IV: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(104, 3)]),
        v(
            "bcm.central_hist_04_log",
            "Central lock history IV: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(104, 4)]),
        v(
            "bcm.central_hist_05_entry",
            "Central lock history V: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(105, 1)]),
        v(
            "bcm.central_hist_05_op_a",
            "Central lock history V: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(105, 2)]),
        v(
            "bcm.central_hist_05_op_b",
            "Central lock history V: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(105, 3)]),
        v(
            "bcm.central_hist_05_log",
            "Central lock history V: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(105, 4)]),
        v(
            "bcm.central_hist_06_entry",
            "Central lock history VI: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(106, 1)]),
        v(
            "bcm.central_hist_06_op_a",
            "Central lock history VI: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(106, 2)]),
        v(
            "bcm.central_hist_06_op_b",
            "Central lock history VI: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(106, 3)]),
        v(
            "bcm.central_hist_06_log",
            "Central lock history VI: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(106, 4)]),
        v(
            "bcm.central_hist_07_entry",
            "Central lock history VII: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(107, 1)]),
        v(
            "bcm.central_hist_07_op_a",
            "Central lock history VII: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(107, 2)]),
        v(
            "bcm.central_hist_07_op_b",
            "Central lock history VII: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(107, 3)]),
        v(
            "bcm.central_hist_07_log",
            "Central lock history VII: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(107, 4)]),
        v(
            "bcm.central_hist_08_entry",
            "Central lock history VIII: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(108, 1)]),
        v(
            "bcm.central_hist_08_op_a",
            "Central lock history VIII: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(108, 2)]),
        v(
            "bcm.central_hist_08_op_b",
            "Central lock history VIII: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(108, 3)]),
        v(
            "bcm.central_hist_08_log",
            "Central lock history VIII: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(108, 4)]),
        v(
            "bcm.central_hist_09_entry",
            "Central lock history IX: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(109, 1)]),
        v(
            "bcm.central_hist_09_op_a",
            "Central lock history IX: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(109, 2)]),
        v(
            "bcm.central_hist_09_op_b",
            "Central lock history IX: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(109, 3)]),
        v(
            "bcm.central_hist_09_log",
            "Central lock history IX: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(109, 4)]),
        v(
            "bcm.central_hist_10_entry",
            "Central lock history X: entry",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(110, 1)]),
        v(
            "bcm.central_hist_10_op_a",
            "Central lock history X: operation A",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(110, 2)]),
        v(
            "bcm.central_hist_10_op_b",
            "Central lock history X: operation B",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(110, 3)]),
        v(
            "bcm.central_hist_10_log",
            "Central lock history X: history",
            Reported,
            Body,
            "Lock history",
        )
        .bcm(&[(110, 4)]),
        v(
            "bcm.comfort_hist_01_entry",
            "Comfort history I: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(111, 1)]),
        v(
            "bcm.comfort_hist_01_op_a",
            "Comfort history I: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(111, 2)]),
        v(
            "bcm.comfort_hist_01_op_b",
            "Comfort history I: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(111, 3)]),
        v(
            "bcm.comfort_hist_01_log",
            "Comfort history I: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(111, 4)]),
        v(
            "bcm.comfort_hist_02_entry",
            "Comfort history II: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(112, 1)]),
        v(
            "bcm.comfort_hist_02_op_a",
            "Comfort history II: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(112, 2)]),
        v(
            "bcm.comfort_hist_02_op_b",
            "Comfort history II: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(112, 3)]),
        v(
            "bcm.comfort_hist_02_log",
            "Comfort history II: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(112, 4)]),
        v(
            "bcm.comfort_hist_03_entry",
            "Comfort history III: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(113, 1)]),
        v(
            "bcm.comfort_hist_03_op_a",
            "Comfort history III: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(113, 2)]),
        v(
            "bcm.comfort_hist_03_op_b",
            "Comfort history III: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(113, 3)]),
        v(
            "bcm.comfort_hist_03_log",
            "Comfort history III: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(113, 4)]),
        v(
            "bcm.comfort_hist_04_entry",
            "Comfort history IV: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(114, 1)]),
        v(
            "bcm.comfort_hist_04_op_a",
            "Comfort history IV: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(114, 2)]),
        v(
            "bcm.comfort_hist_04_op_b",
            "Comfort history IV: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(114, 3)]),
        v(
            "bcm.comfort_hist_04_log",
            "Comfort history IV: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(114, 4)]),
        v(
            "bcm.comfort_hist_05_entry",
            "Comfort history V: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(115, 1)]),
        v(
            "bcm.comfort_hist_05_op_a",
            "Comfort history V: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(115, 2)]),
        v(
            "bcm.comfort_hist_05_op_b",
            "Comfort history V: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(115, 3)]),
        v(
            "bcm.comfort_hist_05_log",
            "Comfort history V: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(115, 4)]),
        v(
            "bcm.comfort_hist_06_entry",
            "Comfort history VI: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(116, 1)]),
        v(
            "bcm.comfort_hist_06_op_a",
            "Comfort history VI: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(116, 2)]),
        v(
            "bcm.comfort_hist_06_op_b",
            "Comfort history VI: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(116, 3)]),
        v(
            "bcm.comfort_hist_06_log",
            "Comfort history VI: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(116, 4)]),
        v(
            "bcm.comfort_hist_07_entry",
            "Comfort history VII: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(117, 1)]),
        v(
            "bcm.comfort_hist_07_op_a",
            "Comfort history VII: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(117, 2)]),
        v(
            "bcm.comfort_hist_07_op_b",
            "Comfort history VII: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(117, 3)]),
        v(
            "bcm.comfort_hist_07_log",
            "Comfort history VII: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(117, 4)]),
        v(
            "bcm.comfort_hist_08_entry",
            "Comfort history VIII: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(118, 1)]),
        v(
            "bcm.comfort_hist_08_op_a",
            "Comfort history VIII: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(118, 2)]),
        v(
            "bcm.comfort_hist_08_op_b",
            "Comfort history VIII: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(118, 3)]),
        v(
            "bcm.comfort_hist_08_log",
            "Comfort history VIII: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(118, 4)]),
        v(
            "bcm.comfort_hist_09_entry",
            "Comfort history IX: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(119, 1)]),
        v(
            "bcm.comfort_hist_09_op_a",
            "Comfort history IX: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(119, 2)]),
        v(
            "bcm.comfort_hist_09_op_b",
            "Comfort history IX: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(119, 3)]),
        v(
            "bcm.comfort_hist_09_log",
            "Comfort history IX: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(119, 4)]),
        v(
            "bcm.comfort_hist_10_entry",
            "Comfort history X: entry",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(120, 1)]),
        v(
            "bcm.comfort_hist_10_op_a",
            "Comfort history X: operation A",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(120, 2)]),
        v(
            "bcm.comfort_hist_10_op_b",
            "Comfort history X: operation B",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(120, 3)]),
        v(
            "bcm.comfort_hist_10_log",
            "Comfort history X: history",
            Reported,
            Body,
            "Comfort history",
        )
        .bcm(&[(120, 4)]),
        v(
            "bcm.info_wiper_motor",
            "Wiper motor component info",
            Reported,
            Body,
            "Module info (BCM)",
        )
        .bcm(&[(93, 1)]),
        v(
            "bcm.info_rls",
            "Rain/light sensor component info",
            Reported,
            Body,
            "Module info (BCM)",
        )
        .bcm(&[(94, 1)]),
        v(
            "bcm.info_siren",
            "Siren component info",
            Reported,
            Body,
            "Module info (BCM)",
        )
        .bcm(&[(95, 1)]),
        v(
            "bcm.info_sensor",
            "Alarm sensor component info",
            Reported,
            Body,
            "Module info (BCM)",
        )
        .bcm(&[(96, 1)]),
        v(
            "bcm.info_interior_monitor",
            "Interior monitoring component info",
            Reported,
            Body,
            "Module info (BCM)",
        )
        .bcm(&[(97, 1)]),
        v("lights.side", "Side lights", Reported, Lights, "Exterior").ican("lights.side"),
        v("lights.low_beam", "Low beam", Reported, Lights, "Exterior").ican("lights.low_beam"),
        v(
            "lights.high_beam",
            "High beam",
            Reported,
            Lights,
            "Exterior",
        )
        .ican("lights.high_beam"),
        v(
            "lights.drl",
            "Daytime running lights",
            Reported,
            Lights,
            "Exterior",
        )
        .ican("lights.drl"),
        v(
            "lights.fog_front",
            "Front fogs",
            Reported,
            Lights,
            "Exterior",
        )
        .ican("lights.fog_front"),
        v("lights.fog_rear", "Rear fog", Reported, Lights, "Exterior").ican("lights.fog_rear"),
        v(
            "lights.turn_left",
            "Turn left",
            Reported,
            Lights,
            "Exterior",
        )
        .ican("lights.turn_left"),
        v(
            "lights.turn_right",
            "Turn right",
            Reported,
            Lights,
            "Exterior",
        )
        .ican("lights.turn_right"),
        v("lights.hazard", "Hazards", Reported, Lights, "Exterior").ican("lights.hazard"),
        v(
            "lights.reverse",
            "Reverse light",
            Reported,
            Lights,
            "Exterior",
        )
        .ican("lights.reverse"),
        v(
            "lights.reverse_gw",
            "Reverse light (gateway)",
            Reported,
            Lights,
            "Exterior",
        )
        .ican("lights.reverse_gw"),
        v(
            "light.display_pct",
            "Display dimming (body)",
            Expect("%"),
            Lights,
            "Dimming",
        )
        .ican("light.display_pct"),
        v(
            "light.kl58t_pct",
            "Terminal 58t",
            Expect("%"),
            Lights,
            "Dimming",
        )
        .ican("light.kl58t_pct"),
        v(
            "light.dim_switch_pct",
            "Terminal 58s (gateway)",
            Expect("%"),
            Lights,
            "Dimming",
        )
        .ican("light.dim_switch_pct"),
        v(
            "light.cluster_display_pct",
            "Cluster display",
            Expect("%"),
            Lights,
            "Dimming",
        )
        .ican("light.cluster_display_pct"),
        v(
            "light.dim_display_pct",
            "Display dimming (gateway)",
            Expect("%"),
            Lights,
            "Dimming",
        )
        .ican("light.dim_display_pct"),
        v(
            "light.cluster_sensor",
            "Cluster light sensor",
            Reported,
            Lights,
            "Dimming",
        )
        .ican("light.cluster_sensor"),
        v(
            "light.dim_sensor",
            "Gateway light sensor",
            Reported,
            Lights,
            "Dimming",
        )
        .ican("light.dim_sensor"),
        v(
            "light.sun_left_wm2",
            "Sun load left",
            Expect("W/m2"),
            Lights,
            "Dimming",
        )
        .ican("light.sun_left_wm2"),
        v(
            "light.sun_right_wm2",
            "Sun load right",
            Expect("W/m2"),
            Lights,
            "Dimming",
        )
        .ican("light.sun_right_wm2"),
        v(
            "bcm.lamp_high_l",
            "Left high beam",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(40, 1)]),
        v(
            "bcm.lamp_high_r",
            "Right high beam",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(40, 2)]),
        v(
            "bcm.lamp_xenon_shutter_l",
            "Left xenon shutter",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(40, 3)]),
        v(
            "bcm.lamp_xenon_shutter_r",
            "Right xenon shutter",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(40, 4)]),
        v(
            "bcm.lamp_low_l",
            "Left low beam",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(42, 1)]),
        v(
            "bcm.lamp_low_r",
            "Right low beam",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(42, 2)]),
        v(
            "bcm.lamp_parking_fl",
            "Left front parking light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(41, 1)]),
        v(
            "bcm.lamp_parking_fr",
            "Right front parking light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(41, 2)]),
        v(
            "bcm.lamp_drl_l",
            "Left DRL",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(41, 3)]),
        v(
            "bcm.lamp_drl_r",
            "Right DRL",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(41, 4)]),
        v(
            "bcm.lamp_fog_l",
            "Left fog lamp",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(42, 3)]),
        v(
            "bcm.lamp_fog_r",
            "Right fog lamp",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(42, 4)]),
        v(
            "bcm.lamp_turn_fl",
            "Left front turn signal",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(43, 1)]),
        v(
            "bcm.lamp_turn_fr",
            "Right front turn signal",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(43, 2)]),
        v(
            "bcm.lamp_turn_rl",
            "Left rear turn signal",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(43, 3)]),
        v(
            "bcm.lamp_turn_rr",
            "Right rear turn signal",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(43, 4)]),
        v(
            "bcm.lamp_turn_side_l",
            "Left side turn signal",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(44, 1)]),
        v(
            "bcm.lamp_turn_side_r",
            "Right side turn signal",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(44, 2)]),
        v(
            "bcm.lamp_brake_l",
            "Left brake light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(45, 1)]),
        v(
            "bcm.lamp_brake_r",
            "Right brake light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(45, 2)]),
        v(
            "bcm.lamp_brake_high",
            "High-mount brake light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(45, 3)]),
        v(
            "bcm.lamp_plate",
            "License plate light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(45, 4)]),
        v(
            "bcm.lamp_reverse_l",
            "Left reverse light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(46, 1)]),
        v(
            "bcm.lamp_reverse_r",
            "Right reverse light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(46, 2)]),
        v(
            "bcm.lamp_tail_l",
            "Left tail light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(46, 3)]),
        v(
            "bcm.lamp_tail_r",
            "Right tail light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(46, 4)]),
        v(
            "bcm.lamp_rear_fog_l",
            "Left rear fog light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(47, 1)]),
        v(
            "bcm.lamp_rear_fog_r",
            "Right rear fog light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(47, 2)]),
        v(
            "bcm.lamp_cornering_l",
            "Left cornering light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(47, 3)]),
        v(
            "bcm.lamp_cornering_r",
            "Right cornering light",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(47, 4)]),
        v(
            "bcm.lamp_footwell",
            "Footwell lighting",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(48, 1)]),
        v(
            "bcm.lamp_interior",
            "Interior lighting",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(48, 2)]),
        v(
            "bcm.lamp_hazards",
            "Hazard lighting",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(48, 3)]),
        v(
            "bcm.hazard_switch_led",
            "Hazard switch LED",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(48, 4)]),
        v(
            "bcm.lamp_trunk",
            "Trunk lighting",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(74, 4)]),
        v(
            "bcm.lamp_engine_bay",
            "Engine bay lighting",
            Reported,
            Lights,
            "Outputs (BCM)",
        )
        .bcm(&[(75, 2)]),
        v(
            "bcm.ls_off",
            "Light switch OFF",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(49, 1)]),
        v(
            "bcm.ls_parking",
            "Light switch parking light",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(49, 2)]),
        v(
            "bcm.ls_low_beam",
            "Light switch low beam",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(49, 3)]),
        v(
            "bcm.ls_assist",
            "Light switch assistance driving lights",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(49, 4)]),
        v(
            "bcm.ls_front_fog",
            "Light switch front fog",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(50, 1)]),
        v(
            "bcm.ls_rear_fog",
            "Light switch rear fog",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(50, 2)]),
        v(
            "bcm.ls_rotary",
            "Rotary light switch position",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(51, 1)]),
        v(
            "bcm.drl_status",
            "Daytime running lights status",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(51, 2)]),
        v(
            "bcm.afl_status",
            "Assistant running light status (AFL)",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(51, 3)]),
        v(
            "bcm.light_config",
            "Light configuration",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(51, 4)]),
        v(
            "bcm.brake_switch_hw",
            "Brake light switch (hardware)",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(50, 3)]),
        v(
            "bcm.brake_switch_can",
            "Brake light switch (CAN)",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(50, 4)]),
        v(
            "bcm.coming_home",
            "Coming home",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(52, 2)]),
        v(
            "bcm.leaving_home",
            "Leaving home",
            Reported,
            Lights,
            "Switches (BCM)",
        )
        .bcm(&[(52, 4)]),
        v(
            "bcm.kl58d_cluster",
            "Instrument cluster illumination terminal 58d",
            Reported,
            Lights,
            "Dimming (BCM)",
        )
        .bcm(&[(53, 1)]),
        v(
            "bcm.kl58d_pwm_range",
            "Terminal 58d PWM range",
            Reported,
            Lights,
            "Dimming (BCM)",
        )
        .bcm(&[(53, 2)]),
        v(
            "bcm.kl58d_request",
            "Illumination request 58d",
            Reported,
            Lights,
            "Dimming (BCM)",
        )
        .bcm(&[(53, 3)]),
        v(
            "bcm.kl58d_tig_request",
            "Illumination request 58d TIG",
            Reported,
            Lights,
            "Dimming (BCM)",
        )
        .bcm(&[(54, 1)]),
        v("rnse.radio_on", "Radio on", Reported, Rnse, "Radio").ican("rnse.radio_on"),
        v("rnse.mode", "Radio mode", Reported, Rnse, "Radio").ican("rnse.mode"),
        v("rnse.on_tv", "On TV", Reported, Rnse, "Radio").ican("rnse.on_tv"),
        v(
            "units.distance_mi",
            "Cluster shows miles",
            Reported,
            Other,
            "Cluster units",
        )
        .ican("units.distance_mi"),
        v(
            "units.temp_f",
            "Cluster shows °F",
            Reported,
            Other,
            "Cluster units",
        )
        .ican("units.temp_f"),
        v(
            "units.volume_gal",
            "Cluster shows gallons",
            Reported,
            Other,
            "Cluster units",
        )
        .ican("units.volume_gal"),
        v(
            "units.consumption_per_distance",
            "Cluster shows volume per distance",
            Reported,
            Other,
            "Cluster units",
        )
        .ican("units.consumption_per_distance"),
        v(
            "units.pressure",
            "Cluster pressure unit",
            Reported,
            Other,
            "Cluster units",
        )
        .ican("units.pressure"),
        v(
            "units.date_month_first",
            "Cluster date month first",
            Reported,
            Other,
            "Cluster units",
        )
        .ican("units.date_month_first"),
        v(
            "units.clock_12h",
            "Cluster 12 h clock",
            Reported,
            Other,
            "Cluster units",
        )
        .ican("units.clock_12h"),
        v("clock.datetime", "Car clock", Reported, Other, "Clock").ican("clock.datetime"),
    ]
}
