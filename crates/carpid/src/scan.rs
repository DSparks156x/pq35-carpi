//! Fault scan reports (docs/PAGES.md 5c, Faults): each `DiagAction::Scan`
//! ends with a plain-text report in the logs folder, VCDS auto-scan style.
//!
//! ```text
//! entry.json    carpi_logs::Meta, kind scan, trigger "faults"
//! scan.txt      date, odometer, then per module: address, name, part
//!               number, component, software, coding, fault codes
//! ```
//!
//! Fault status bytes stay raw hex: VAG KWP2000 status bits aren't the UDS
//! ones and their meaning isn't established here.

use crate::capture;
use crate::clock;
use carpi_core::diagnostics::{ScanModule, ScanReport};
use carpi_logs::{Kind, Logs};
use std::path::Path;

/// The report's file in the entry.
pub const FILE: &str = "scan.txt";

/// `6 modules, 2 faults`.
pub fn summary(r: &ScanReport) -> String {
    format!(
        "{} module{}, {} fault{}",
        r.modules.len(),
        if r.modules.len() == 1 { "" } else { "s" },
        r.faults(),
        if r.faults() == 1 { "" } else { "s" }
    )
}

/// The report. `when`: local time; `odometer`: km, if the car said.
pub fn text(r: &ScanReport, when: &str, odometer: Option<f64>) -> String {
    let mut t = String::new();
    let mut line = |k: &str, v: String| t.push_str(&format!("{k:<11}{v}\n"));
    line("Fault scan", when.to_string());
    line(
        "Odometer",
        odometer.map_or("-".into(), |km| {
            format!("{} km", capture::group(km.max(0.0).round() as u64))
        }),
    );
    line("carpid", env!("CARGO_PKG_VERSION").into());
    line(
        "Modules",
        format!(
            "{} coded, {} answered, {} fault{}",
            r.modules.len(),
            r.answered(),
            r.faults(),
            if r.faults() == 1 { "" } else { "s" }
        ),
    );
    if let Some(e) = &r.error {
        line("Stopped", e.clone());
    }
    for m in &r.modules {
        t.push('\n');
        module(&mut t, m);
    }
    t
}

fn module(t: &mut String, m: &ScanModule) {
    t.push_str(&format!(
        "Address {:02X}: {} (TP2 {:02X}, status {:02X})\n",
        m.address, m.name, m.tp2, m.status
    ));
    let field = |t: &mut String, k: &str, v: &Option<String>| {
        if let Some(v) = v {
            t.push_str(&format!("   {k:<11}{v}\n"));
        }
    };
    if let Some(i) = &m.ident {
        field(t, "Part No", &i.part_no);
        field(t, "Component", &i.component);
        field(t, "SW", &i.sw_version);
        field(t, "Coding", &i.coding);
        if i.metadata.iter().any(|&b| b != 0) {
            let hex: Vec<String> = i.metadata.iter().map(|b| format!("{b:02X}")).collect();
            t.push_str(&format!("   {:<11}{}\n", "9B extra", hex.join(" ")));
        }
    }
    match (&m.dtcs, &m.error) {
        (Some(d), _) if d.is_empty() => t.push_str("   No faults\n"),
        (Some(d), _) => {
            t.push_str(&format!(
                "   {} Fault{}\n",
                d.len(),
                if d.len() == 1 { "" } else { "s" }
            ));
            for f in d {
                t.push_str(&format!("   {:05}  status {:02X}\n", f.code, f.status));
            }
        }
        (None, Some(e)) => t.push_str(&format!("   {e}\n")),
        (None, None) => t.push_str("   Not read\n"),
    }
}

/// Writes the report as the next entry of today; its label (`2026-11-11 #04`).
pub fn save(
    root: &Path,
    max_pct: u8,
    r: &ScanReport,
    odometer: Option<f64>,
) -> Result<String, String> {
    let logs = Logs::new(root);
    let now = clock::local_now();
    let (mut meta, dir) = logs
        .create(Kind::Scan, &now.date, "faults", &now.datetime)
        .map_err(|e| format!("creating the entry in {}: {e}", root.display()))?;
    capture::prune(&logs, max_pct, Some(&meta.id));
    std::fs::write(dir.join(FILE), text(r, &now.datetime, odometer))
        .map_err(|e| format!("{FILE}: {e}"))?;
    meta.ended = Some(clock::local_now().datetime);
    meta.summary = summary(r);
    logs.save_meta(&meta)
        .map_err(|e| format!("entry.json: {e}"))?;
    Ok(meta.id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use carpi_core::diagnostics::{Dtc, Ident};

    fn report() -> ScanReport {
        ScanReport {
            modules: vec![
                ScanModule {
                    address: 0x01,
                    tp2: 0x01,
                    name: "Engine",
                    status: 0x01,
                    dtcs: Some(vec![Dtc {
                        code: 17965,
                        status: 0x23,
                    }]),
                    ident: Some(Ident {
                        part_no: Some("8J0907115B".into()),
                        sw_version: Some("0040".into()),
                        component: Some("MED17.5.1".into()),
                        coding: None,
                        metadata: vec![0; 10],
                    }),
                    error: None,
                },
                ScanModule {
                    address: 0x46,
                    tp2: 0x21,
                    name: "Central convenience",
                    status: 0x09,
                    dtcs: None,
                    ident: None,
                    error: Some("No answer".into()),
                },
            ],
            error: None,
        }
    }

    #[test]
    fn report_reads_like_an_auto_scan() {
        let r = report();
        let t = text(&r, "2026-11-11T10:00:00", Some(123456.4));
        assert!(
            t.starts_with("Fault scan 2026-11-11T10:00:00\nOdometer   123 456 km\n"),
            "{t}"
        );
        assert!(
            t.contains("Modules    2 coded, 1 answered, 1 fault\n"),
            "{t}"
        );
        assert!(t.contains(
            "Address 01: Engine (TP2 01, status 01)\n   Part No    8J0907115B\n   Component  MED17.5.1\n   SW         0040\n   1 Fault\n   17965  status 23\n"
        ), "{t}");
        assert!(!t.contains("9B extra"));
        assert!(
            t.contains("Address 46: Central convenience (TP2 21, status 09)\n   No answer\n"),
            "{t}"
        );
        assert!(text(&r, "x", None).contains("Odometer   -\n"));
        assert_eq!(summary(&r), "2 modules, 1 fault");
    }

    #[test]
    fn save_makes_a_scan_entry() {
        let t = tempfile::tempdir().unwrap();
        let label = save(t.path(), 25, &report(), None).unwrap();
        let logs = Logs::new(t.path());
        let id = carpi_logs::EntryId::parse(&label).unwrap();
        let meta = logs.meta(&id).unwrap();
        assert_eq!(meta.kind, Kind::Scan);
        assert_eq!(meta.summary, "2 modules, 1 fault");
        assert!(meta.ended.is_some());
        let names: Vec<_> = logs.files(&id).into_iter().map(|f| f.path).collect();
        assert_eq!(names, ["entry.json", FILE]);
    }
}
