//! Serves the portal on http://127.0.0.1:5099/ with a made-up logs folder,
//! for looking at the pages on a desk. `cargo run -p carpi-portal --example demo [dir]`.
//! The folder is filled once; delete it to start over.

use carpi_logs::{Kind, Logs};
use carpi_portal::{Portal, PortalConfig, Status};
use std::fs;
use std::path::PathBuf;

/// (kind, trigger, time, summary)
type Demo = (Kind, &'static str, &'static str, &'static str);

fn fill(root: &std::path::Path) {
    let logs = Logs::new(root.join("logs"));
    let days: [(&str, &[Demo]); 3] = [
        (
            "2026-11-08",
            &[
                (Kind::Capture, "key", "08:12:40", "4 buses, 31 812 frames"),
                (
                    Kind::Datalog,
                    "boost",
                    "08:20:02",
                    "boost set, 10 Hz, 6 min",
                ),
                (Kind::Scan, "faults", "08:31:09", "14 modules, 2 faults"),
                (
                    Kind::Capture,
                    "portal",
                    "09:41:17",
                    "3 buses, 12 004 frames",
                ),
            ],
        ),
        (
            "2026-11-07",
            &[(
                Kind::Capture,
                "crash:livi",
                "18:03:55",
                "4 buses, 28 113 frames",
            )],
        ),
        (
            "2026-11-05",
            &[
                (Kind::Capture, "page", "07:30:00", "4 buses, 30 001 frames"),
                (Kind::Capture, "key", "07:44:12", ""),
            ],
        ),
    ];
    for (date, entries) in days {
        for (kind, trigger, time, summary) in entries {
            let started = format!("{date}T{time}");
            let (mut m, dir) = logs.create(*kind, date, trigger, &started).unwrap();
            match kind {
                Kind::Capture => {
                    fs::write(dir.join("can-ican.log.zst"), vec![7u8; 1_400_000]).unwrap();
                    fs::write(dir.join("can-comfort.log.zst"), vec![7u8; 620_000]).unwrap();
                    let journal: String = (0..400)
                        .map(|i| format!("{date} {time} carpid[812]: line {i}: bus awake\n"))
                        .collect();
                    fs::write(dir.join("journal.txt"), journal).unwrap();
                    fs::write(dir.join("values.json"), "{\"boost_kpa\": 104.2}\n").unwrap();
                    fs::write(
                        dir.join("info.txt"),
                        format!("trigger: {trigger}\npi: {started}\n"),
                    )
                    .unwrap();
                    fs::create_dir(dir.join("config")).unwrap();
                    fs::write(
                        dir.join("config/carpid.toml"),
                        "[bus.ican]\niface = \"can0\"\n",
                    )
                    .unwrap();
                }
                Kind::Datalog => {
                    let csv: String = (0..3600)
                        .map(|i| format!("{i},{}\n", 100 + i % 50))
                        .collect();
                    fs::write(dir.join("data.csv"), format!("t,boost\n{csv}")).unwrap();
                    fs::write(dir.join("profile.json"), "{}\n").unwrap();
                }
                Kind::Scan => {
                    fs::write(dir.join("scan.txt"), "Address 01: Engine\n").unwrap();
                }
            }
            m.summary = summary.to_string();
            if *trigger != "key" || date != "2026-11-05" {
                m.ended = Some(started.clone());
            }
            m.starred = *trigger == "crash:livi";
            logs.save_meta(&m).unwrap();
        }
    }
    for d in ["captures", "sdlink", "firmware", "cfg"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("sdlink/sdlink-2026-11-08.log"),
        "sdlink ok\n".repeat(900),
    )
    .unwrap();
    fs::write(root.join("firmware/teensy41-r56b.hex"), ":00000001FF\n").unwrap();
    fs::write(
        root.join("cfg/carpid.toml"),
        "[bus.ican]\niface = \"can0\"\n",
    )
    .unwrap();
}

#[tokio::main]
async fn main() {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("carpi-portal-demo"));
    if !root.join("logs").exists() {
        fill(&root);
    }
    let cfg = PortalConfig {
        bind: ([127, 0, 0, 1], 5099).into(),
        logs: root.join("logs"),
        captures: root.join("captures"),
        sdlink_logs: root.join("sdlink"),
        firmware: root.join("firmware"),
        config_file: root.join("cfg/carpid.toml"),
        journal: Vec::new(),
        pin: std::env::var("PIN").ok(),
        ..PortalConfig::default()
    };
    let logs = Logs::new(&cfg.logs);
    println!("http://127.0.0.1:5099/  ({})", root.display());
    Portal::new(cfg)
        .status(|| async {
            Status {
                version: "0.1.0".into(),
                uptime_s: 4_210,
                bus: "awake".into(),
                extra: vec![
                    ("Wi-Fi clients".into(), "1".into()),
                    ("Disk".into(), "41% free".into()),
                ],
            }
        })
        .validate_config(|_| Ok(()))
        .capture(move |trigger| {
            let logs = logs.clone();
            async move {
                let (m, dir) = logs
                    .create(Kind::Capture, "2026-11-08", &trigger, "2026-11-08T10:00:00")
                    .map_err(|e| e.to_string())?;
                fs::write(dir.join("journal.txt"), "demo\n").map_err(|e| e.to_string())?;
                Ok(m.id.to_string())
            }
        })
        .serve(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
}
