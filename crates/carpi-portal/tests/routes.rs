//! Route tests against the router in-process.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use carpi_logs::{Kind, Logs};
use carpi_portal::{Portal, PortalConfig, Status, WifiNet, WifiOp};
use http_body_util::BodyExt;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tower::ServiceExt;

const HOST: &str = "10.10.0.1:8080";
const HEX: &[u8] = b":0400000001020304F2\n:00000001FF\n";
const GOOD_TOML: &str = "[bus.ican]\niface = \"can0\"\n";
const JOURNAL: &[u8] = b"Nov 08 10:04:00 carpid[1]: hello\n";
const ZST: &[u8] = &[0x28, 0xb5, 0x2f, 0xfd, 1, 2, 3];
const CSV: &str = "time_s,engine.rpm,engine.boost.actual,nav.road\n\
                   0.000,900,1010,\"A9, south\"\n\
                   0.100,910,,\n\
                   0.200,,1240,A9\n\
                   0.300,950,1300,A9\n";

fn setup() -> (TempDir, PortalConfig) {
    let t = tempfile::tempdir().unwrap();
    let p = t.path();
    for d in ["captures", "sdlink", "fw", "cfg"] {
        fs::create_dir(p.join(d)).unwrap();
    }
    fs::write(p.join("captures/capture-1.log"), "(0.0) can0 123#00\n").unwrap();
    fs::write(p.join("sdlink/sd.log"), "sdlink ok\n").unwrap();
    fs::write(p.join("cfg/carpid.toml"), GOOD_TOML).unwrap();
    fs::write(p.join("secret.txt"), "secret").unwrap();

    // logs/2026-11-08/01 capture, /02 datalog; logs/2026-11-07/01 capture.
    let logs = Logs::new(p.join("logs"));
    let (mut m, dir) = logs
        .create(Kind::Capture, "2026-11-08", "key", "2026-11-08T10:04:33")
        .unwrap();
    fs::write(dir.join("journal.txt"), JOURNAL).unwrap();
    fs::write(dir.join("can.log.zst"), ZST).unwrap();
    fs::create_dir(dir.join("config")).unwrap();
    fs::write(dir.join("config/carpid.toml"), GOOD_TOML).unwrap();
    m.summary = "4 buses, 1 frame".into();
    m.ended = Some("2026-11-08T10:04:35".into());
    logs.save_meta(&m).unwrap();
    let (_, dir) = logs
        .create(Kind::Datalog, "2026-11-08", "boost", "2026-11-08T11:00:00")
        .unwrap();
    fs::write(dir.join("data.csv"), CSV).unwrap();
    logs.create(Kind::Capture, "2026-11-07", "page", "2026-11-07T09:00:00")
        .unwrap();

    let journal = if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), "echo journal-line".into()]
    } else {
        vec!["echo".into(), "journal-line".into()]
    };
    let cfg = PortalConfig {
        logs: p.join("logs"),
        captures: p.join("captures"),
        sdlink_logs: p.join("sdlink"),
        firmware: p.join("fw"),
        config_file: p.join("cfg/carpid.toml"),
        journal,
        ..PortalConfig::default()
    };
    (t, cfg)
}

fn app(cfg: PortalConfig) -> Router {
    Portal::new(cfg)
        .status(|| async {
            Status {
                version: "0.1.0".into(),
                uptime_s: 3725,
                bus: "awake".into(),
                extra: vec![],
            }
        })
        .validate_config(|text| {
            if text.contains("[bus.ican]") {
                Ok(())
            } else {
                Err("[bus.ican] is required".into())
            }
        })
        .router()
}

fn get(uri: &str) -> axum::http::request::Builder {
    Request::get(uri).header(header::HOST, HOST)
}

fn form(uri: &str) -> axum::http::request::Builder {
    Request::post(uri)
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
}

fn upload(uri: &str) -> axum::http::request::Builder {
    Request::post(uri)
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=XyZ")
}

fn form_body(filename: &str, data: &[u8]) -> Body {
    let mut b = Vec::new();
    b.extend_from_slice(
        format!(
            "--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    b.extend_from_slice(data);
    b.extend_from_slice(b"\r\n--XyZ--\r\n");
    Body::from(b)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let r = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = r.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes().to_vec();
    (parts.status, parts.headers, bytes)
}

/// GET a page that must be 200 and HTML.
async fn page(app: &Router, uri: &str) -> String {
    let (code, h, body) = send(app, get(uri).body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK, "{uri}");
    assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
    String::from_utf8(body).unwrap()
}

async fn up(app: &Router, uri: &str, name: &str, data: &[u8], pin: Option<&str>) -> StatusCode {
    let mut req = upload(uri);
    if let Some(pin) = pin {
        req = req.header("x-carpi-pin", pin);
    }
    send(app, req.body(form_body(name, data)).unwrap()).await.0
}

fn backups(cfg: &PortalConfig) -> Vec<String> {
    fs::read_dir(cfg.config_file.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".bak"))
        .collect()
}

fn starred(cfg: &PortalConfig, n: u32) -> bool {
    let id = carpi_logs::EntryId {
        date: "2026-11-08".into(),
        number: n,
    };
    Logs::new(&cfg.logs).meta(&id).unwrap().starred
}

fn unzip(body: &[u8]) -> BTreeMap<String, (Vec<u8>, zip::CompressionMethod)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(body.to_vec())).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        let name = f.name().unwrap().into_owned();
        out.insert(name, (data, f.compression()));
    }
    out
}

#[tokio::test]
async fn home_and_setup_pages() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let (code, h, body) = send(&app, get("/").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    assert!(
        h[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("default-src 'none'")
    );
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains("awake"));
    assert!(html.contains("1h 2m"));
    // Days newest first, with count.
    let a = html.find("/day/2026-11-08").unwrap();
    let b = html.find("/day/2026-11-07").unwrap();
    assert!(a < b);
    assert!(html.contains("2 entries") && html.contains("1 entry"));

    let html = page(&app, "/setup").await;
    assert!(html.contains("/dl/captures/capture-1.log"));
    assert!(html.contains("Older captures"));
    assert!(html.contains("/dl/sdlink/sd.log"));
    assert!(html.contains("/dl/bugreport"));
    assert!(html.contains("/up/config") && html.contains("/up/firmware"));

    let (code, _, body) = send(&app, get("/api/status").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["bus"], "awake");

    let (code, h, _) = send(&app, get("/files").body(Body::empty()).unwrap()).await;
    assert!(code.is_redirection());
    assert_eq!(h[header::LOCATION], "/");
}

#[tokio::test]
async fn no_older_captures_when_empty() {
    let (t, cfg) = setup();
    fs::remove_file(t.path().join("captures/capture-1.log")).unwrap();
    let app = app(cfg);
    assert!(!page(&app, "/setup").await.contains("Older captures"));
}

#[tokio::test]
async fn day_lists_entries_newest_first() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let html = page(&app, "/day/2026-11-08").await;
    let a = html.find("#02").unwrap();
    let b = html.find("#01").unwrap();
    assert!(a < b, "newest first");
    for want in [
        "Sunday 8 Nov 2026",
        "/zip/day/2026-11-08",
        "/zip/e/2026-11-08/01",
        "/zip/e/2026-11-08/02",
        "Download #01 (.zip)",
        "/e/2026-11-08/01",
        "capture",
        "datalog",
        "boost",
        "4 buses, 1 frame",
        "10:04",
        "/star/2026-11-08/01",
    ] {
        assert!(html.contains(want), "{want}");
    }
    for uri in [
        "/day/2026-11-09",
        "/day/2026-11-0",
        "/day/..",
        "/day/2026-11-08x",
    ] {
        let (code, _, _) = send(&app, get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn entry_lists_files() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let html = page(&app, "/e/2026-11-08/01").await;
    for want in [
        "/f/2026-11-08/01/journal.txt",
        "/v/2026-11-08/01/journal.txt",
        "/f/2026-11-08/01/config/carpid.toml",
        "/f/2026-11-08/01/can.log.zst",
        "/delete/2026-11-08/01",
        "/zip/e/2026-11-08/01",
        "10:04:35",
    ] {
        assert!(html.contains(want), "{want}");
    }
    // Compressed files get no inline view.
    assert!(!html.contains("/v/2026-11-08/01/can.log.zst"));
    // No PIN set: the delete form has no PIN field.
    assert!(!html.contains("name=pin"));
    let (code, _, _) = send(&app, get("/e/2026-11-08/09").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn entry_file_download_and_view() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let (code, h, body) = send(
        &app,
        get("/f/2026-11-08/01/journal.txt")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, JOURNAL);
    let cd = h[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        cd.starts_with("attachment") && cd.contains("2026-11-08-01-journal.txt"),
        "{cd}"
    );

    let (code, _, body) = send(
        &app,
        get("/f/2026-11-08/01/config/carpid.toml")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, GOOD_TOML.as_bytes());

    let (code, h, body) = send(
        &app,
        get("/v/2026-11-08/01/journal.txt")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "text/plain; charset=utf-8");
    assert_eq!(h[header::CONTENT_DISPOSITION], "inline");
    assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(body, JOURNAL);
}

#[tokio::test]
async fn text_view_is_capped() {
    let (t, cfg) = setup();
    let big = t.path().join("logs/2026-11-08/01/big.log");
    fs::write(&big, vec![b'x'; (1 << 20) + 500]).unwrap();
    let app = app(cfg);
    let (code, _, body) = send(
        &app,
        get("/v/2026-11-08/01/big.log").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert!(body.len() < (1 << 20) + 200);
    assert!(String::from_utf8_lossy(&body).contains("cut at 1.0 MB"));
}

#[tokio::test]
async fn logs_path_traversal_rejected() {
    let (t, cfg) = setup();
    fs::write(t.path().join("logs/2026-11-08/01/.hidden"), "secret").unwrap();
    fs::write(t.path().join("logs/2026-11-08/secret.txt"), "secret").unwrap();
    let app = app(cfg);
    for uri in [
        "/e/2026-11-08/../..",
        "/e/2026-11-08/..",
        "/e/../01",
        "/e/2026-11-08/1",
        "/e/2026-11-08/-01",
        "/e/2026-11-08/%2301",
        "/e/2026-11-08/001",
        "/e/2026-11-0%C3%A9/01",
        "/e/2026-11-08/0%C3%A9",
        "/zip/e/x/01",
        "/zip/e/2026-11-08/..",
        "/zip/e/2026-11-08/%2E%2E",
        "/zip/e/2026-11-08/09",
        "/zip/day/..",
        "/zip/day/2026-11-08%2F..",
        "/zip/day/2026-11-09",
        "/f/2026-11-08/01/../secret.txt",
        "/f/2026-11-08/01/..%2Fsecret.txt",
        "/f/2026-11-08/01/..%2F..%2F..%2Fsecret.txt",
        "/f/2026-11-08/01/%2E%2E/secret.txt",
        "/f/2026-11-08/01/..%5Csecret.txt",
        "/f/2026-11-08/01/.hidden",
        "/f/2026-11-08/01/config/../journal.txt",
        "/f/2026-11-08/01//etc/passwd",
        "/f/2026-11-08/01/C:%5Csecret.txt",
        "/f/2026-11-08/01/missing.txt",
        "/f/2026-11-08/01",
        "/f/2026-11-08/../secret.txt",
        "/v/2026-11-08/01/..%2Fsecret.txt",
        "/v/2026-11-08/01/.hidden",
    ] {
        let (code, _, body) = send(&app, get(uri).body(Body::empty()).unwrap()).await;
        assert!(code.is_client_error(), "{uri}: {code}");
        assert!(!String::from_utf8_lossy(&body).contains("secret"), "{uri}");
    }
}

#[tokio::test]
async fn entry_zip_contents() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let (code, h, body) = send(
        &app,
        get("/zip/e/2026-11-08/01").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "application/zip");
    assert!(
        h[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("carpi-2026-11-08-01.zip")
    );
    let z = unzip(&body);
    let names: Vec<_> = z.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "can.log.zst",
            "config/carpid.toml",
            "entry.json",
            "journal.txt"
        ]
    );
    assert_eq!(z["journal.txt"].0, JOURNAL);
    assert_eq!(z["journal.txt"].1, zip::CompressionMethod::Deflated);
    assert_eq!(z["can.log.zst"].0, ZST);
    assert_eq!(z["can.log.zst"].1, zip::CompressionMethod::Stored);
    assert_eq!(z["config/carpid.toml"].0, GOOD_TOML.as_bytes());
    let meta: serde_json::Value = serde_json::from_slice(&z["entry.json"].0).unwrap();
    assert_eq!(meta["trigger"], "key");
}

#[tokio::test]
async fn day_zip_contents() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let (code, h, body) = send(
        &app,
        get("/zip/day/2026-11-08").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert!(
        h[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("carpi-2026-11-08.zip")
    );
    let z = unzip(&body);
    let names: Vec<_> = z.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "01/can.log.zst",
            "01/config/carpid.toml",
            "01/entry.json",
            "01/journal.txt",
            "02/data.csv",
            "02/entry.json",
        ]
    );
    assert_eq!(z["02/data.csv"].0, CSV.as_bytes());
    assert_eq!(z["01/journal.txt"].0, JOURNAL);
}

#[tokio::test]
async fn datalog_entry_trims_its_csv() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let html = page(&app, "/e/2026-11-08/02").await;
    for want in [
        "Trim CSV",
        "action=\"/trim/2026-11-08/02\"",
        "name=c value=1 checked><span class=mono>engine.rpm",
        "name=c value=3 checked><span class=mono>nav.road",
        "name=from type=number step=any inputmode=decimal value=\"0\"",
        "name=to type=number step=any inputmode=decimal value=\"0.3\"",
    ] {
        assert!(html.contains(want), "{want}");
    }
    assert!(!page(&app, "/e/2026-11-08/01").await.contains("Trim CSV"));

    // The form as sent: two columns, part of the range.
    let (code, h, body) = send(
        &app,
        get("/trim/2026-11-08/02?pick=1&c=3&c=1&from=0.1&to=0.25")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "text/csv; charset=utf-8");
    assert!(
        h[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("carpi-2026-11-08-02-trim.csv")
    );
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "time_s,engine.rpm,nav.road\n0.100,910,\n0.200,,A9\n"
    );
    // Quoted fields stay whole; the defaults are all of it.
    let (_, _, body) = send(
        &app,
        get("/trim/2026-11-08/02?pick=1&c=3&to=0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(body, b"time_s,nav.road\n0.000,\"A9, south\"\n");
    let (_, _, body) = send(
        &app,
        get("/trim/2026-11-08/02").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(body, CSV.as_bytes());

    for (uri, want) in [
        ("/trim/2026-11-08/02?pick=1", StatusCode::BAD_REQUEST),
        ("/trim/2026-11-08/02?pick=1&c=4", StatusCode::BAD_REQUEST),
        ("/trim/2026-11-08/02?from=1&to=0", StatusCode::BAD_REQUEST),
        ("/trim/2026-11-08/02?from=x", StatusCode::BAD_REQUEST),
        ("/trim/2026-11-08/01", StatusCode::NOT_FOUND),
        ("/trim/2026-11-08/09", StatusCode::NOT_FOUND),
        ("/trim/2026-11-08/2", StatusCode::NOT_FOUND),
        ("/trim/2026-11-08/..", StatusCode::NOT_FOUND),
        ("/trim/2026-11-08/%2E%2E", StatusCode::NOT_FOUND),
        ("/trim/..%2F..%2F2026-11-08/02", StatusCode::NOT_FOUND),
        (
            "/trim/2026-11-08/02/../../secret.txt",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (code, _, body) = send(&app, get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(code, want, "{uri}");
        assert!(!String::from_utf8_lossy(&body).contains("secret"), "{uri}");
    }
    // The Host guard covers it like everything else.
    let req = Request::get("/trim/2026-11-08/02")
        .header(header::HOST, "evil.example")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn trim_needs_the_pin_when_downloads_do() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    cfg.protect_downloads = true;
    let app = app(cfg);
    let uri = "/trim/2026-11-08/02?pick=1&c=1";
    let (code, _, body) = send(&app, get(uri).body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert!(!String::from_utf8_lossy(&body).contains("910"));
    let req = get(uri)
        .header("x-carpi-pin", "4711")
        .body(Body::empty())
        .unwrap();
    let (code, _, body) = send(&app, req).await;
    assert_eq!(code, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("0.100,910"));
}

#[cfg(unix)]
#[tokio::test]
async fn trim_refuses_a_symlinked_csv() {
    let (t, cfg) = setup();
    let csv = t.path().join("logs/2026-11-08/02/data.csv");
    fs::remove_file(&csv).unwrap();
    std::os::unix::fs::symlink(t.path().join("secret.txt"), &csv).unwrap();
    let app = app(cfg);
    let (code, _, body) = send(
        &app,
        get("/trim/2026-11-08/02").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert!(!String::from_utf8_lossy(&body).contains("secret"));
    assert!(!page(&app, "/e/2026-11-08/02").await.contains("Trim CSV"));
}

#[tokio::test]
async fn big_zip_streams_whole() {
    let (t, cfg) = setup();
    // Several times what the channel holds, so the body really streams.
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
    fs::write(t.path().join("logs/2026-11-08/02/big.bin"), &data).unwrap();
    let app = app(cfg);
    let (code, _, body) = send(
        &app,
        get("/zip/e/2026-11-08/02").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(unzip(&body)["big.bin"].0, data);
}

#[tokio::test]
async fn star_toggles() {
    let (_t, cfg) = setup();
    let app = app(cfg.clone());
    let req = form("/star/2026-11-08/01")
        .body(Body::from("back=entry"))
        .unwrap();
    let (code, h, _) = send(&app, req).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    assert_eq!(h[header::LOCATION], "/e/2026-11-08/01");
    assert!(starred(&cfg, 1));
    assert!(
        page(&app, "/day/2026-11-08")
            .await
            .contains("aria-pressed=true")
    );
    let req = form("/star/2026-11-08/01")
        .body(Body::from("back=day"))
        .unwrap();
    let (_, h, _) = send(&app, req).await;
    assert_eq!(h[header::LOCATION], "/day/2026-11-08#e01");
    assert!(!starred(&cfg, 1));
    let req = form("/star/2026-11-08/07").body(Body::empty()).unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn star_and_delete_need_pin() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    let app = app(cfg.clone());
    let dir = cfg.logs.join("2026-11-08/01");

    let req = form("/star/2026-11-08/01").body(Body::empty()).unwrap();
    let (code, _, body) = send(&app, req).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert!(String::from_utf8(body).unwrap().contains("name=pin"));
    assert!(!starred(&cfg, 1));
    let req = form("/star/2026-11-08/01")
        .header("x-carpi-pin", "4711")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::SEE_OTHER);
    assert!(starred(&cfg, 1));

    // Delete: no PIN, wrong PIN, no confirm, then for real.
    let req = form("/delete/2026-11-08/01")
        .body(Body::from("confirm=yes"))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::UNAUTHORIZED);
    let req = form("/delete/2026-11-08/01")
        .body(Body::from("confirm=yes&pin=0000"))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    let req = form("/delete/2026-11-08/01")
        .body(Body::from("pin=4711"))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::BAD_REQUEST);
    assert!(dir.is_dir());
    let req = form("/delete/2026-11-08/01")
        .body(Body::from("confirm=yes&pin=4711"))
        .unwrap();
    let (code, h, _) = send(&app, req).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    assert_eq!(h[header::LOCATION], "/day/2026-11-08");
    assert!(!dir.exists());
    assert!(cfg.logs.join("2026-11-08/02").is_dir());

    // Locked phone: the delete form asks for the PIN.
    assert!(page(&app, "/e/2026-11-08/02").await.contains("name=pin"));
}

#[tokio::test]
async fn delete_last_entry_goes_home() {
    let (_t, cfg) = setup();
    let app = app(cfg.clone());
    let req = form("/delete/2026-11-07/01")
        .body(Body::from("confirm=yes"))
        .unwrap();
    let (code, h, _) = send(&app, req).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    assert_eq!(h[header::LOCATION], "/");
    assert!(!cfg.logs.join("2026-11-07").exists());
}

#[tokio::test]
async fn foreign_origin_star_and_delete_refused() {
    let (_t, cfg) = setup();
    let app = app(cfg.clone());
    let req = form("/star/2026-11-08/01")
        .header(header::ORIGIN, "http://evil.example")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    let req = form("/delete/2026-11-08/01")
        .header(header::ORIGIN, "http://evil.example")
        .body(Body::from("confirm=yes"))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    assert!(!starred(&cfg, 1));
    assert!(cfg.logs.join("2026-11-08/01").is_dir());
}

#[tokio::test]
async fn capture_hook() {
    let (_t, cfg) = setup();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let c = calls.clone();
    let logs = Logs::new(&cfg.logs);
    let app = Portal::new(cfg.clone())
        .capture(move |trigger| {
            c.lock().unwrap().push(trigger.clone());
            let logs = logs.clone();
            async move {
                let (m, _) = logs
                    .create(Kind::Capture, "2026-11-08", &trigger, "2026-11-08T12:00:00")
                    .map_err(|e| e.to_string())?;
                Ok(m.id.to_string())
            }
        })
        .router();
    let html = page(&app, "/").await;
    assert!(html.contains("action=\"/capture\"") && html.contains("Capture now"));
    let req = form("/capture").body(Body::empty()).unwrap();
    let (code, h, _) = send(&app, req).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    assert_eq!(h[header::LOCATION], "/e/2026-11-08/03");
    assert_eq!(*calls.lock().unwrap(), ["portal"]);
    assert!(page(&app, "/e/2026-11-08/03").await.contains("portal"));
}

#[tokio::test]
async fn capture_hidden_without_hook() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let html = page(&app, "/").await;
    assert!(!html.contains("/capture") && !html.contains("Capture now"));
    let req = form("/capture").body(Body::empty()).unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn capture_needs_pin_and_reports_errors() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    let app = Portal::new(cfg)
        .capture(|_| async { Err("no CAN ring yet".to_string()) })
        .router();
    let req = form("/capture").body(Body::empty()).unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::UNAUTHORIZED);
    let req = form("/capture")
        .header("x-carpi-pin", "4711")
        .body(Body::empty())
        .unwrap();
    let (code, _, body) = send(&app, req).await;
    assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(String::from_utf8(body).unwrap().contains("no CAN ring yet"));
}

#[tokio::test]
async fn wrong_pin_cookie_is_dropped() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    let app = app(cfg);
    let req = get("/")
        .header(header::COOKIE, "carpi_pin=30303030")
        .body(Body::empty())
        .unwrap();
    let (code, h, _) = send(&app, req).await;
    assert_eq!(code, StatusCode::OK);
    assert!(
        h[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let req = get("/")
        .header(header::COOKIE, "carpi_pin=34373131")
        .body(Body::empty())
        .unwrap();
    let (_, h, body) = send(&app, req).await;
    assert!(h.get(header::SET_COOKIE).is_none());
    assert!(String::from_utf8(body).unwrap().contains("Unlocked"));
}

#[tokio::test]
async fn pin_form_redirects_back_locally() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    let app = app(cfg);
    for (back, want) in [
        ("%2Fday%2F2026-11-08%23e01", "/day/2026-11-08#e01"),
        ("%2F%2Fevil.example", "/setup"),
        ("http%3A%2F%2Fevil.example", "/setup"),
    ] {
        let req = form("/pin")
            .body(Body::from(format!("pin=4711&back={back}")))
            .unwrap();
        let (code, h, _) = send(&app, req).await;
        assert_eq!(code, StatusCode::SEE_OTHER);
        assert_eq!(h[header::LOCATION], want);
    }
    let req = form("/pin").body(Body::from("pin=1")).unwrap();
    let (code, _, body) = send(&app, req).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert!(String::from_utf8(body).unwrap().contains("Wrong PIN"));
}

#[tokio::test]
async fn downloads_a_capture() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let (code, h, body) = send(
        &app,
        get("/dl/captures/capture-1.log")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, b"(0.0) can0 123#00\n");
    assert!(
        h[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("capture-1.log")
    );
    let (code, _, body) = send(&app, get("/dl/config").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, GOOD_TOML.as_bytes());
}

#[tokio::test]
async fn path_traversal_rejected() {
    let (t, cfg) = setup();
    fs::write(t.path().join("captures/.hidden"), "x").unwrap();
    let app = app(cfg);
    for uri in [
        "/dl/captures/..%2Fsecret.txt",
        "/dl/captures/..%2F..%2Fsecret.txt",
        "/dl/captures/..%5Csecret.txt",
        "/dl/captures/..",
        "/dl/captures/.hidden",
        "/dl/captures/%2E%2E",
        "/dl/captures/../secret.txt",
        "/dl/captures/C:%5Csecret.txt",
        "/dl/etc/passwd",
        "/dl/captures/missing.log",
    ] {
        let (code, _, body) = send(&app, get(uri).body(Body::empty()).unwrap()).await;
        assert!(code.is_client_error(), "{uri}: {code}");
        assert_ne!(body, b"secret", "{uri}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_out_of_dir_rejected() {
    let (t, cfg) = setup();
    std::os::unix::fs::symlink(
        t.path().join("secret.txt"),
        t.path().join("captures/link.log"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        t.path().join("secret.txt"),
        t.path().join("logs/2026-11-08/01/link.txt"),
    )
    .unwrap();
    let app = app(cfg);
    for uri in [
        "/dl/captures/link.log",
        "/f/2026-11-08/01/link.txt",
        "/v/2026-11-08/01/link.txt",
    ] {
        let (code, _, _) = send(&app, get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "{uri}");
    }
    let (_, _, body) = send(
        &app,
        get("/zip/e/2026-11-08/01").body(Body::empty()).unwrap(),
    )
    .await;
    assert!(!unzip(&body).contains_key("link.txt"));
}

#[tokio::test]
async fn pin_enforced_on_uploads_only() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    let app = app(cfg.clone());
    // Downloads stay open.
    for uri in [
        "/dl/captures/capture-1.log",
        "/f/2026-11-08/01/journal.txt",
        "/zip/e/2026-11-08/01",
    ] {
        let (code, _, _) = send(&app, get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(code, StatusCode::OK, "{uri}");
    }

    assert_eq!(
        up(&app, "/up/firmware", "a.hex", HEX, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        up(&app, "/up/firmware", "a.hex", HEX, Some("1234")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        up(
            &app,
            "/up/config",
            "carpid.toml",
            GOOD_TOML.as_bytes(),
            None
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert!(!cfg.firmware.join("a.hex").exists());
    assert_eq!(
        up(&app, "/up/firmware", "a.hex", HEX, Some("4711")).await,
        StatusCode::CREATED
    );
    assert_eq!(fs::read(cfg.firmware.join("a.hex")).unwrap(), HEX);
}

#[tokio::test]
async fn pin_cookie_from_form() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    let app = app(cfg.clone());
    let req = form("/pin").body(Body::from("pin=4711")).unwrap();
    let (code, h, _) = send(&app, req).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    let cookie = h[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    let pair = cookie.split(';').next().unwrap().to_owned();

    let req = upload("/up/firmware")
        .header(header::COOKIE, pair.clone())
        .body(form_body("b.hex", HEX))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::CREATED);
    let req = form("/star/2026-11-08/01")
        .header(header::COOKIE, pair)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::SEE_OTHER);
    assert!(starred(&cfg, 1));

    let req = form("/lock").body(Body::empty()).unwrap();
    let (_, h, _) = send(&app, req).await;
    assert!(
        h[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
}

#[tokio::test]
async fn pin_locks_out_after_five_wrong() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    let app = app(cfg);
    for _ in 0..5 {
        assert_eq!(
            up(&app, "/up/firmware", "a.hex", HEX, Some("0000")).await,
            StatusCode::FORBIDDEN
        );
    }
    // Even the right PIN waits out the lockout, for every write.
    assert_eq!(
        up(&app, "/up/firmware", "a.hex", HEX, Some("4711")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    let req = form("/delete/2026-11-08/01")
        .body(Body::from("confirm=yes&pin=4711"))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn protected_downloads_need_pin() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    cfg.protect_downloads = true;
    let app = app(cfg);
    for uri in [
        "/dl/captures/capture-1.log",
        "/dl/config",
        "/dl/bugreport",
        "/api/status",
        "/",
        "/day/2026-11-08",
        "/e/2026-11-08/01",
        "/f/2026-11-08/01/journal.txt",
        "/v/2026-11-08/01/journal.txt",
        "/zip/e/2026-11-08/01",
        "/zip/day/2026-11-08",
    ] {
        let (code, _, body) = send(&app, get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "{uri}");
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("journal.txt") && !text.contains("awake"),
            "{uri}"
        );
    }
    let html = page(&app, "/setup").await;
    assert!(!html.contains("capture-1.log") && html.contains("name=pin"));
    for uri in ["/dl/captures/capture-1.log", "/e/2026-11-08/01"] {
        let req = get(uri)
            .header("x-carpi-pin", "4711")
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&app, req).await.0, StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn bad_config_keeps_old_file() {
    let (_t, cfg) = setup();
    let app = app(cfg.clone());
    let req = upload("/up/config")
        .body(form_body("carpid.toml", b"[bus.obd]\niface = \"x\"\n"))
        .unwrap();
    let (code, _, body) = send(&app, req).await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("[bus.ican] is required")
    );
    assert_eq!(fs::read_to_string(&cfg.config_file).unwrap(), GOOD_TOML);
    assert!(backups(&cfg).is_empty());

    // Not UTF-8: refused before the validator.
    assert_eq!(
        up(&app, "/up/config", "carpid.toml", &[0xff, 0xfe], None).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(fs::read_to_string(&cfg.config_file).unwrap(), GOOD_TOML);
}

#[tokio::test]
async fn good_config_replaces_and_backs_up() {
    let (_t, cfg) = setup();
    let app = app(cfg.clone());
    let new = "[bus.ican]\niface = \"ican\"\n";
    let req = upload("/up/config")
        .body(form_body("x.toml", new.as_bytes()))
        .unwrap();
    let (code, _, body) = send(&app, req).await;
    assert_eq!(
        code,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(fs::read_to_string(&cfg.config_file).unwrap(), new);
    let b = backups(&cfg);
    assert_eq!(b.len(), 1);
    assert!(
        b[0].starts_with("carpid.toml.") && b[0].ends_with("Z.bak"),
        "{b:?}"
    );
    let dir = cfg.config_file.parent().unwrap();
    assert_eq!(fs::read_to_string(dir.join(&b[0])).unwrap(), GOOD_TOML);

    // A second upload in the same second gets its own backup.
    assert_eq!(
        up(&app, "/up/config", "x.toml", GOOD_TOML.as_bytes(), None).await,
        StatusCode::CREATED
    );
    assert_eq!(backups(&cfg).len(), 2);
}

#[tokio::test]
async fn config_without_validator_refused() {
    let (_t, cfg) = setup();
    let app = Portal::new(cfg.clone()).router();
    assert_eq!(
        up(
            &app,
            "/up/config",
            "carpid.toml",
            GOOD_TOML.as_bytes(),
            None
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(fs::read_to_string(&cfg.config_file).unwrap(), GOOD_TOML);
}

#[tokio::test]
async fn size_caps() {
    let (_t, mut cfg) = setup();
    cfg.firmware_max = 1024;
    cfg.config_max = 64;
    let app = app(cfg.clone());
    // Under the body limit, over the cap.
    let mut big = HEX.to_vec();
    big.splice(0..0, std::iter::repeat_n(b'\n', 2000));
    assert_eq!(
        up(&app, "/up/firmware", "big.hex", &big, None).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    // Far over the body limit.
    let huge = vec![b'\n'; 200_000];
    assert_eq!(
        up(&app, "/up/firmware", "huge.hex", &huge, None).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(fs::read_dir(&cfg.firmware).unwrap().next().is_none());

    let long = format!("{GOOD_TOML}# {}\n", "x".repeat(100));
    assert_eq!(
        up(&app, "/up/config", "carpid.toml", long.as_bytes(), None).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(fs::read_to_string(&cfg.config_file).unwrap(), GOOD_TOML);
}

#[tokio::test]
async fn firmware_checks() {
    let (_t, cfg) = setup();
    let app = app(cfg.clone());
    let bad = StatusCode::BAD_REQUEST;
    assert_eq!(up(&app, "/up/firmware", "a.bin", HEX, None).await, bad);
    assert_eq!(up(&app, "/up/firmware", "a.hex.sh", HEX, None).await, bad);
    assert_eq!(
        up(&app, "/up/firmware", "../../a.hex", HEX, None).await,
        bad
    );
    assert_eq!(up(&app, "/up/firmware", ".a.hex", HEX, None).await, bad);
    assert_eq!(
        up(&app, "/up/firmware", "a.hex", b"not hex", None).await,
        bad
    );
    assert_eq!(up(&app, "/up/firmware", "a.hex", b"", None).await, bad);
    assert!(fs::read_dir(&cfg.firmware).unwrap().next().is_none());

    assert_eq!(
        up(&app, "/up/firmware", "Teensy_4.1.HEX", HEX, None).await,
        StatusCode::CREATED
    );
    assert_eq!(
        up(&app, "/up/firmware", "Teensy_4.1.HEX", HEX, None).await,
        StatusCode::CONFLICT
    );
    // Only the stored file, no temp leftovers.
    let names: Vec<_> = fs::read_dir(&cfg.firmware)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["Teensy_4.1.HEX"]);
    assert!(
        page(&app, "/setup")
            .await
            .contains("/dl/firmware/Teensy_4.1.HEX")
    );
}

#[tokio::test]
async fn foreign_origin_and_host_refused() {
    let (_t, cfg) = setup();
    let app = app(cfg.clone());
    let req = upload("/up/firmware")
        .header(header::ORIGIN, "http://evil.example")
        .body(form_body("a.hex", HEX))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    let req = upload("/up/firmware")
        .header(header::ORIGIN, format!("http://{HOST}"))
        .body(form_body("a.hex", HEX))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::CREATED);

    for uri in ["/dl/captures/capture-1.log", "/", "/zip/day/2026-11-08"] {
        let req = Request::get(uri)
            .header(header::HOST, "rebind.evil.example:8080")
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN, "{uri}");
    }
}

#[tokio::test]
async fn journal_download() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let (code, _, body) = send(&app, get("/dl/journal").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("journal-line"));
}

#[tokio::test]
async fn bug_report_zip() {
    let (_t, mut cfg) = setup();
    cfg.bundle_max = 1024;
    // The newest entry is 2026-11-08 #02; give it one file over the cap.
    fs::write(cfg.logs.join("2026-11-08/02/big.csv"), vec![b'x'; 4096]).unwrap();
    let app = app(cfg);
    let (code, h, body) = send(&app, get("/dl/bugreport").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "application/zip");
    let cd = h[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        cd.contains("carpi-bugreport-") && cd.contains(".zip"),
        "{cd}"
    );
    let z = unzip(&body);
    let keys: Vec<_> = z.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "2026-11-08-02/data.csv",
            "2026-11-08-02/entry.json",
            "carpid.toml",
            "skipped.txt",
            "status.txt",
        ]
    );
    let text = |k: &str| String::from_utf8(z[k].0.clone()).unwrap();
    assert!(text("status.txt").contains("Bus: awake"));
    assert!(text("skipped.txt").contains("big.csv"));
    assert_eq!(text("carpid.toml"), GOOD_TOML);
}

#[tokio::test]
async fn pin_changes_live() {
    let (_t, cfg) = setup();
    let portal = Portal::new(cfg.clone()).validate_config(|_| Ok(()));
    let pin = portal.pin_cell();
    let app = portal.router();
    assert_eq!(
        up(&app, "/up/firmware", "a.hex", HEX, None).await,
        StatusCode::CREATED
    );
    *pin.write().unwrap() = Some("4711".into());
    assert_eq!(
        up(&app, "/up/firmware", "b.hex", HEX, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        up(&app, "/up/firmware", "b.hex", HEX, Some("4711")).await,
        StatusCode::CREATED
    );
    *pin.write().unwrap() = None;
    assert_eq!(
        up(&app, "/up/firmware", "c.hex", HEX, None).await,
        StatusCode::CREATED
    );
}

#[test]
fn config_keeps_old_keys() {
    let cfg: PortalConfig = serde_json::from_value(serde_json::json!({
        "bind": "127.0.0.1:5003",
        "captures": "/tmp/c",
        "logs": "/tmp/logs",
        "pin": "1234",
        "protect_downloads": true,
        "bundle_max": 10
    }))
    .unwrap();
    assert_eq!(cfg.logs, std::path::PathBuf::from("/tmp/logs"));
    assert!(serde_json::from_value::<PortalConfig>(serde_json::json!({"nope": 1})).is_err());
    assert!(PortalConfig::default().logs.ends_with("logs"));
}

/// A Wi-Fi hook over a list, recording every op.
fn wifi_app(cfg: PortalConfig) -> (Router, Arc<Mutex<Vec<WifiOp>>>) {
    let ops = Arc::new(Mutex::new(Vec::new()));
    let nets = Arc::new(Mutex::new(vec![WifiNet {
        ssid: "Home & Away".into(),
        secured: true,
        hidden: false,
        connected: true,
    }]));
    let o = ops.clone();
    let app = Portal::new(cfg)
        .wifi(move |op| {
            o.lock().unwrap().push(op.clone());
            let nets = nets.clone();
            async move {
                let mut n = nets.lock().unwrap();
                match op {
                    WifiOp::List => {}
                    WifiOp::Add {
                        ssid,
                        password,
                        hidden,
                    } => {
                        n.retain(|x| x.ssid != ssid);
                        n.push(WifiNet {
                            ssid,
                            secured: password.is_some(),
                            hidden,
                            connected: false,
                        });
                    }
                    WifiOp::Forget { ssid } => {
                        let before = n.len();
                        n.retain(|x| x.ssid != ssid);
                        if n.len() == before {
                            return Err(format!("{ssid:?} is not saved"));
                        }
                    }
                }
                Ok(n.clone())
            }
        })
        .router();
    (app, ops)
}

fn writes(ops: &Mutex<Vec<WifiOp>>) -> Vec<WifiOp> {
    ops.lock()
        .unwrap()
        .iter()
        .filter(|o| **o != WifiOp::List)
        .cloned()
        .collect()
}

#[tokio::test]
async fn wifi_hidden_without_hook() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    assert!(!page(&app, "/setup").await.contains("/wifi"));
    let (code, _, _) = send(&app, get("/wifi").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    let req = form("/wifi/add").body(Body::from("ssid=x")).unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wifi_list_add_forget() {
    let (_t, cfg) = setup();
    let (app, ops) = wifi_app(cfg);
    assert!(page(&app, "/setup").await.contains("href=\"/wifi\""));
    let html = page(&app, "/wifi").await;
    assert!(html.contains("Home &amp; Away") && html.contains("connected"));
    assert!(html.contains("/wifi/forget?ssid=Home%20%26%20Away"));
    assert!(html.contains("Car Integration &gt; Updates &gt; Wi-Fi networks"));
    assert!(!html.contains("name=pin"));

    let req = form("/wifi/add")
        .body(Body::from("ssid=Cafe&password=hunter2hunter2&hidden=yes"))
        .unwrap();
    let (code, h, _) = send(&app, req).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    assert_eq!(h[header::LOCATION], "/wifi");
    let req = form("/wifi/add").body(Body::from("ssid=Open+one")).unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::SEE_OTHER);
    assert_eq!(
        writes(&ops),
        [
            WifiOp::Add {
                ssid: "Cafe".into(),
                password: Some("hunter2hunter2".into()),
                hidden: true
            },
            WifiOp::Add {
                ssid: "Open one".into(),
                password: None,
                hidden: false
            }
        ]
    );
    let html = page(&app, "/wifi").await;
    assert!(html.contains("Password · hidden") && html.contains("Open one"));
    assert!(!html.contains("hunter2"));

    // Forget: the confirm page, then confirm=yes.
    let html = page(&app, "/wifi/forget?ssid=Cafe").await;
    assert!(html.contains("Forget Cafe?") && html.contains("name=confirm value=yes"));
    let (code, _, _) = send(
        &app,
        get("/wifi/forget?ssid=Nope").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    let req = form("/wifi/forget").body(Body::from("ssid=Cafe")).unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::BAD_REQUEST);
    let req = form("/wifi/forget")
        .body(Body::from("ssid=Cafe&confirm=yes"))
        .unwrap();
    let (code, h, _) = send(&app, req).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    assert_eq!(h[header::LOCATION], "/wifi");
    assert!(!page(&app, "/wifi").await.contains(">Cafe<"));
    // The hook's refusal reaches the phone.
    let req = form("/wifi/forget")
        .body(Body::from("ssid=Cafe&confirm=yes"))
        .unwrap();
    let (code, _, body) = send(&app, req).await;
    assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(String::from_utf8(body).unwrap().contains("is not saved"));
}

#[tokio::test]
async fn wifi_bad_input_refused() {
    let (_t, cfg) = setup();
    let (app, ops) = wifi_app(cfg);
    let long = "x".repeat(33);
    let p64 = "p".repeat(64);
    for body in [
        "ssid=&password=hunter2hunter2".to_string(),
        "password=hunter2hunter2".into(),
        format!("ssid={long}"),
        "ssid=a%0Ab".into(),
        "ssid=Cafe&password=s3cr3t!".into(),
        format!("ssid=Cafe&password={p64}"),
    ] {
        let req = form("/wifi/add").body(Body::from(body.clone())).unwrap();
        let (code, _, html) = send(&app, req).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
        let html = String::from_utf8(html).unwrap();
        assert!(html.contains("errline"), "{body}");
        assert!(!html.contains("hunter2") && !html.contains("s3cr3t") && !html.contains(&p64));
    }
    // The name comes back for another try; the password does not.
    let req = form("/wifi/add")
        .body(Body::from("ssid=Cafe&password=s3cr3t!"))
        .unwrap();
    let html = String::from_utf8(send(&app, req).await.2).unwrap();
    assert!(html.contains("value=\"Cafe\"") && html.contains("8-63 characters"));
    let req = form("/wifi/forget")
        .body(Body::from(format!("ssid={long}&confirm=yes")))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::BAD_REQUEST);
    assert!(writes(&ops).is_empty());
}

#[tokio::test]
async fn wifi_needs_pin() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    let (app, ops) = wifi_app(cfg.clone());
    // Locked: the list shows, the forms ask for the PIN.
    let html = page(&app, "/wifi").await;
    assert!(html.contains("Home &amp; Away") && html.contains("name=pin"));

    let add = "ssid=Cafe&password=hunter2hunter2";
    let req = form("/wifi/add").body(Body::from(add)).unwrap();
    let (code, _, body) = send(&app, req).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert!(!String::from_utf8(body).unwrap().contains("hunter2"));
    let req = form("/wifi/add")
        .body(Body::from(format!("{add}&pin=0000")))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    let req = form("/wifi/forget")
        .body(Body::from("ssid=Home+%26+Away&confirm=yes"))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::UNAUTHORIZED);
    let req = form("/wifi/add")
        .header(header::ORIGIN, "http://evil.example")
        .header("x-carpi-pin", "4711")
        .body(Body::from(add))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    assert!(writes(&ops).is_empty());

    // PIN in the form, then the cookie.
    let req = form("/wifi/add")
        .body(Body::from(format!("{add}&pin=4711")))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::SEE_OTHER);
    let req = form("/wifi/forget")
        .header(header::COOKIE, "carpi_pin=34373131")
        .body(Body::from("ssid=Cafe&confirm=yes"))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::SEE_OTHER);
    assert_eq!(writes(&ops).len(), 2);

    // Protected downloads: the list is locked too.
    cfg.protect_downloads = true;
    let (app, _) = wifi_app(cfg);
    let (code, _, body) = send(&app, get("/wifi").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert!(!String::from_utf8(body).unwrap().contains("Away"));
}
