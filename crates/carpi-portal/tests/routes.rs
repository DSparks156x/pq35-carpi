//! Route tests against the router in-process.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use carpi_portal::{Portal, PortalConfig, Status};
use http_body_util::BodyExt;
use std::fs;
use std::io::Read;
use tempfile::TempDir;
use tower::ServiceExt;

const HOST: &str = "10.10.0.1:8080";
const HEX: &[u8] = b":0400000001020304F2\n:00000001FF\n";
const GOOD_TOML: &str = "[bus.ican]\niface = \"can0\"\n";

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
    let journal = if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), "echo journal-line".into()]
    } else {
        vec!["echo".into(), "journal-line".into()]
    };
    let cfg = PortalConfig {
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

#[tokio::test]
async fn page_lists_files_and_status() {
    let (_t, cfg) = setup();
    let app = app(cfg);
    let (code, _, body) = send(&app, get("/files").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains("/dl/captures/capture-1.log"));
    assert!(html.contains("/dl/sdlink/sd.log"));
    assert!(html.contains("awake"));
    assert!(html.contains("1h 2m"));

    let (code, _, body) = send(&app, get("/api/status").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["bus"], "awake");
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
    let app = app(cfg);
    let (code, _, _) = send(
        &app,
        get("/dl/captures/link.log").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pin_enforced_on_uploads_only() {
    let (_t, mut cfg) = setup();
    cfg.pin = Some("4711".into());
    let app = app(cfg.clone());
    // Downloads stay open.
    let (code, _, _) = send(
        &app,
        get("/dl/captures/capture-1.log")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);

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
    let req = Request::post("/pin")
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("pin=4711"))
        .unwrap();
    let (code, h, _) = send(&app, req).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    let cookie = h[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    let pair = cookie.split(';').next().unwrap().to_owned();

    let req = upload("/up/firmware")
        .header(header::COOKIE, pair)
        .body(form_body("b.hex", HEX))
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::CREATED);
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
    // Even the right PIN waits out the lockout.
    assert_eq!(
        up(&app, "/up/firmware", "a.hex", HEX, Some("4711")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
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
    ] {
        let (code, _, _) = send(&app, get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "{uri}");
    }
    let (_, _, page) = send(&app, get("/files").body(Body::empty()).unwrap()).await;
    assert!(!String::from_utf8(page).unwrap().contains("capture-1.log"));
    let req = get("/dl/captures/capture-1.log")
        .header("x-carpi-pin", "4711")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::OK);
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

    let req = Request::get("/dl/captures/capture-1.log")
        .header(header::HOST, "rebind.evil.example:8080")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
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
async fn bug_report_bundle() {
    let (_t, mut cfg) = setup();
    fs::write(cfg.captures.join("capture-big.log"), vec![b'x'; 4096]).unwrap();
    cfg.bundle_max = 1024;
    let app = app(cfg);
    let (code, h, body) = send(&app, get("/dl/bugreport").body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "application/gzip");
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&body[..]));
    let mut found = std::collections::BTreeMap::new();
    for e in ar.entries().unwrap() {
        let mut e = e.unwrap();
        let path = e.path().unwrap().to_string_lossy().into_owned();
        let name = path.split_once('/').unwrap().1.to_owned();
        let mut s = String::new();
        e.read_to_string(&mut s).unwrap();
        found.insert(name, s);
    }
    let keys: Vec<_> = found.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "captures/capture-1.log",
            "carpid-journal.txt",
            "carpid.toml",
            "sdlink-logs/sd.log",
            "skipped.txt",
            "status.txt",
        ]
    );
    assert!(found["status.txt"].contains("Bus: awake"));
    assert!(found["skipped.txt"].contains("capture-big.log"));
    assert_eq!(found["carpid.toml"], GOOD_TOML);
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
