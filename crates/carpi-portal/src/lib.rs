//! Phone portal: a small HTTP server on the car's hotspot. Download captures,
//! the carpid journal, sdlink logs and a bug-report bundle; upload a new
//! `carpid.toml` or a Teensy `.hex`. Nothing is flashed or restarted from here.
//!
//! # Threat model
//!
//! The listener is reachable by anything on the hotspot (WPA2 with a
//! passphrase, no client isolation) and on any other interface the Pi is on.
//! Anyone who has the passphrase can read logs, which hold no secrets, so
//! downloads are open unless `protect_downloads` is set. Writes change what the
//! car runs next, so they need the PIN when one is set. It's plain HTTP: a
//! client on the same Wi-Fi can sniff the PIN. The PIN stops passengers and
//! fat fingers, not a determined attacker who already has the passphrase.
//!
//! Also enforced:
//! - Files are served by bare name from fixed directories only. Names are
//!   `[A-Za-z0-9._-]`, no leading dot; symlinks and anything resolving outside
//!   the directory are refused.
//! - Uploads have size caps, and firmware only lands as a new, valid Intel HEX
//!   file (never overwrites). Config is validated by the caller before it
//!   replaces anything, and the old file is backed up first.
//! - POSTs with a foreign `Origin` are refused (CSRF from a page the phone has
//!   open). `Host` must be an IP literal, `localhost` or listed in
//!   `allowed_hosts` (DNS rebinding).
//! - Five wrong PINs lock PIN checks for 30 s.

mod bundle;
mod files;
mod page;

use axum::extract::{DefaultBodyLimit, Multipart, Path as UrlPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

pub use files::check_ihex;

/// Portal settings. Deserializes from a `[portal]` table; every key is optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PortalConfig {
    /// Listen address.
    pub bind: SocketAddr,
    /// CAN captures (`Car::save_capture` output).
    pub captures: PathBuf,
    /// sdlink logs.
    pub sdlink_logs: PathBuf,
    /// The carpid config the upload replaces. Backups go next to it.
    pub config_file: PathBuf,
    /// Where uploaded firmware lands.
    pub firmware: PathBuf,
    /// Command that prints the carpid journal. Empty disables the download.
    pub journal: Vec<String>,
    /// Upload PIN. Unset or empty: uploads are open.
    pub pin: Option<String>,
    /// Downloads need the PIN too.
    pub protect_downloads: bool,
    /// Allowed firmware extensions, lower case with the dot.
    pub firmware_ext: Vec<String>,
    /// Firmware upload cap, bytes.
    pub firmware_max: u64,
    /// Config upload cap, bytes.
    pub config_max: u64,
    /// Journal output cap, bytes; the rest is cut.
    pub journal_max: u64,
    /// Bug-report cap on uncompressed file bytes; files past it are listed, not included.
    pub bundle_max: u64,
    /// Host names accepted besides IP literals and `localhost`.
    pub allowed_hosts: Vec<String>,
}

impl Default for PortalConfig {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        PortalConfig {
            bind: SocketAddr::from(([0, 0, 0, 0], 8080)),
            captures: home.join("captures"),
            sdlink_logs: home.join("avsd/logs"),
            config_file: home.join(".config/carpi/carpid.toml"),
            firmware: home.join("firmware"),
            journal: [
                "journalctl",
                "--user",
                "-u",
                "carpid",
                "-o",
                "short-iso",
                "--no-pager",
            ]
            .map(String::from)
            .to_vec(),
            pin: None,
            protect_downloads: false,
            firmware_ext: vec![".hex".into()],
            firmware_max: 16 << 20,
            config_max: 256 << 10,
            journal_max: 32 << 20,
            bundle_max: 256 << 20,
            allowed_hosts: Vec::new(),
        }
    }
}

/// What the status block shows. carpid fills it in.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub version: String,
    pub uptime_s: u64,
    /// Bus state, as carpid names it ("awake", "asleep", ...).
    pub bus: String,
    /// Anything else worth a line: (label, value).
    pub extra: Vec<(String, String)>,
}

type StatusFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Status> + Send>> + Send + Sync>;
type ValidateFn = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Builder. `Portal::new(cfg).status(..).validate_config(..).serve(..)`.
pub struct Portal {
    cfg: PortalConfig,
    status: Option<StatusFn>,
    validate: Option<ValidateFn>,
}

impl Portal {
    pub fn new(cfg: PortalConfig) -> Self {
        Portal {
            cfg,
            status: None,
            validate: None,
        }
    }

    /// Called on every page load, status request and bug report.
    pub fn status<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Status> + Send + 'static,
    {
        self.status = Some(Arc::new(move || Box::pin(f())));
        self
    }

    /// Checks an uploaded `carpid.toml` with the daemon's own rules. `Err`
    /// text goes back to the phone. Without a validator, config uploads are
    /// refused.
    pub fn validate_config<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) -> Result<(), String> + Send + Sync + 'static,
    {
        self.validate = Some(Arc::new(f));
        self
    }

    pub fn router(self) -> Router {
        let fw_limit = self.cfg.firmware_max as usize + (64 << 10);
        let cfg_limit = self.cfg.config_max as usize + (64 << 10);
        let shared = Arc::new(Shared {
            cfg: self.cfg,
            status: self.status,
            validate: self.validate,
            fails: Mutex::new(Fails::default()),
            bundle: Semaphore::new(1),
        });
        Router::new()
            .route("/", get(|| async { Redirect::to("/files") }))
            .route("/files", get(page_handler))
            .route("/api/status", get(status_handler))
            .route("/dl/config", get(dl_config))
            .route("/dl/journal", get(dl_journal))
            .route("/dl/bugreport", get(dl_bundle))
            .route("/dl/{kind}/{name}", get(dl_file))
            .route("/pin", post(pin_handler))
            .route(
                "/up/config",
                post(up_config).layer(DefaultBodyLimit::max(cfg_limit)),
            )
            .route(
                "/up/firmware",
                post(up_firmware).layer(DefaultBodyLimit::max(fw_limit)),
            )
            .layer(middleware::from_fn_with_state(shared.clone(), guard))
            .with_state(shared)
    }

    /// Binds `cfg.bind` and serves until `shutdown` resolves.
    pub async fn serve(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let listener = tokio::net::TcpListener::bind(self.cfg.bind).await?;
        tracing::info!("portal on http://{}/files", listener.local_addr()?);
        axum::serve(listener, self.router())
            .with_graceful_shutdown(shutdown)
            .await
    }
}

struct Shared {
    cfg: PortalConfig,
    status: Option<StatusFn>,
    validate: Option<ValidateFn>,
    fails: Mutex<Fails>,
    bundle: Semaphore,
}

#[derive(Default)]
struct Fails {
    count: u32,
    locked_until: Option<Instant>,
}

const MAX_FAILS: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(30);
const PIN_COOKIE: &str = "carpi_pin";
const PIN_HEADER: &str = "x-carpi-pin";

/// Error reply: status plus a plain-text line.
struct Reject(StatusCode, String);

impl Reject {
    fn new(code: StatusCode, msg: impl Into<String>) -> Self {
        Reject(code, msg.into())
    }
    fn not_found() -> Self {
        Reject::new(StatusCode::NOT_FOUND, "Not found.")
    }
}

impl IntoResponse for Reject {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

type Reply = Result<Response, Reject>;

impl Shared {
    fn pin(&self) -> Option<&str> {
        self.cfg.pin.as_deref().filter(|p| !p.is_empty())
    }

    fn check_pin(&self, headers: &HeaderMap) -> Result<(), Reject> {
        let Some(pin) = self.pin() else {
            return Ok(());
        };
        let Some(given) = supplied_pin(headers) else {
            return Err(Reject::new(StatusCode::UNAUTHORIZED, "PIN required."));
        };
        self.try_pin(pin, &given)
    }

    fn try_pin(&self, pin: &str, given: &str) -> Result<(), Reject> {
        let mut f = self.fails.lock().unwrap();
        let now = Instant::now();
        if f.locked_until.is_some_and(|t| now < t) {
            return Err(Reject::new(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many wrong PINs. Wait 30 s.",
            ));
        }
        if ct_eq(pin.as_bytes(), given.as_bytes()) {
            *f = Fails::default();
            return Ok(());
        }
        f.count += 1;
        if f.count >= MAX_FAILS {
            f.count = 0;
            f.locked_until = Some(now + LOCKOUT);
        }
        Err(Reject::new(StatusCode::FORBIDDEN, "Wrong PIN."))
    }

    fn check_read(&self, headers: &HeaderMap) -> Result<(), Reject> {
        if self.cfg.protect_downloads {
            self.check_pin(headers)
        } else {
            Ok(())
        }
    }

    async fn status(&self) -> Option<Status> {
        match &self.status {
            Some(f) => Some(f().await),
            None => None,
        }
    }

    /// Runs the journal command; `None` when disabled.
    async fn journal(&self) -> Option<Result<Vec<u8>, String>> {
        let (cmd, args) = self.cfg.journal.split_first()?;
        let run = async {
            let mut child = tokio::process::Command::new(cmd)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("{cmd}: {e}"))?;
            let mut out = Vec::new();
            let stdout = child.stdout.take().expect("piped");
            stdout
                .take(self.cfg.journal_max)
                .read_to_end(&mut out)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>(out)
        };
        Some(
            tokio::time::timeout(Duration::from_secs(30), run)
                .await
                .unwrap_or_else(|_| Err("journal command timed out".into())),
        )
    }
}

/// PIN from the `X-Carpi-Pin` header or the hex `carpi_pin` cookie.
fn supplied_pin(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get(PIN_HEADER) {
        return v.to_str().ok().map(str::to_owned);
    }
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(PIN_COOKIE)?.strip_prefix('='))
        .and_then(files::unhex)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Host and Origin checks on every request.
async fn guard(State(s): State<Arc<Shared>>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok());
    if let Some(host) = host
        && !host_ok(host, &s.cfg.allowed_hosts)
    {
        return Reject::new(StatusCode::FORBIDDEN, "Unknown host name.").into_response();
    }
    if req.method() == Method::POST
        && let Some(origin) = req.headers().get(header::ORIGIN)
    {
        let same = origin
            .to_str()
            .ok()
            .and_then(|o| o.split_once("://"))
            .is_some_and(|(_, auth)| Some(auth) == host);
        if !same {
            return Reject::new(StatusCode::FORBIDDEN, "Cross-site request refused.")
                .into_response();
        }
    }
    next.run(req).await
}

fn host_ok(host: &str, allowed: &[String]) -> bool {
    if let Some(rest) = host.strip_prefix('[') {
        // [v6]:port
        return rest
            .split_once(']')
            .is_some_and(|(ip, _)| ip.parse::<std::net::Ipv6Addr>().is_ok());
    }
    let name = host.rsplit_once(':').map_or(host, |(h, _)| h);
    name.parse::<IpAddr>().is_ok()
        || name.eq_ignore_ascii_case("localhost")
        || allowed.iter().any(|a| a.eq_ignore_ascii_case(name))
}

async fn page_handler(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    let status = s.status().await;
    let can_read = s.check_read(&headers).is_ok();
    let html = page::render(&s.cfg, status.as_ref(), can_read, s.validate.is_some());
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        [(header::CACHE_CONTROL, "no-store")],
        html,
    )
        .into_response()
}

async fn status_handler(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Reply {
    s.check_read(&headers)?;
    Ok(Json(s.status().await.unwrap_or_default()).into_response())
}

#[derive(Deserialize)]
struct PinForm {
    pin: String,
}

/// Checks the PIN and stores it in a cookie so links and forms carry it.
async fn pin_handler(State(s): State<Arc<Shared>>, Form(f): Form<PinForm>) -> Reply {
    let Some(pin) = s.pin() else {
        return Ok(Redirect::to("/files").into_response());
    };
    s.try_pin(pin, &f.pin)?;
    let cookie = format!(
        "{PIN_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000",
        files::hex(f.pin.as_bytes())
    );
    let mut r = Redirect::to("/files").into_response();
    r.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("ascii"),
    );
    Ok(r)
}

fn attachment(name: &str, content_type: &str) -> [(header::HeaderName, String); 3] {
    [
        (header::CONTENT_TYPE, content_type.into()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        ),
        (header::CACHE_CONTROL, "no-store".into()),
    ]
}

async fn send_file(file: tokio::fs::File, name: &str, content_type: &str) -> Response {
    let len = file.metadata().await.map(|m| m.len()).ok();
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut r = (attachment(name, content_type), body).into_response();
    if let Some(len) = len {
        r.headers_mut().insert(header::CONTENT_LENGTH, len.into());
    }
    r
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()) {
        Some(e) if matches!(e.as_str(), "log" | "txt" | "toml" | "hex" | "csv") => {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    }
}

fn collection<'a>(cfg: &'a PortalConfig, kind: &str) -> Option<&'a Path> {
    match kind {
        "captures" => Some(&cfg.captures),
        "sdlink" => Some(&cfg.sdlink_logs),
        "firmware" => Some(&cfg.firmware),
        _ => None,
    }
}

async fn dl_file(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath((kind, name)): UrlPath<(String, String)>,
) -> Reply {
    s.check_read(&headers)?;
    let dir = collection(&s.cfg, &kind).ok_or_else(Reject::not_found)?;
    let path = files::resolve(dir, &name).ok_or_else(Reject::not_found)?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| Reject::not_found())?;
    Ok(send_file(file, &name, content_type(&name)).await)
}

async fn dl_config(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Reply {
    s.check_read(&headers)?;
    let path = &s.cfg.config_file;
    let meta = std::fs::metadata(path).map_err(|_| Reject::not_found())?;
    if !meta.is_file() {
        return Err(Reject::not_found());
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| Reject::not_found())?;
    Ok(send_file(file, "carpid.toml", "text/plain; charset=utf-8").await)
}

async fn dl_journal(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Reply {
    s.check_read(&headers)?;
    let out = s
        .journal()
        .await
        .ok_or_else(Reject::not_found)?
        .map_err(|e| Reject::new(StatusCode::SERVICE_UNAVAILABLE, e))?;
    let name = format!("carpid-journal-{}.txt", files::stamp());
    Ok((attachment(&name, "text/plain; charset=utf-8"), out).into_response())
}

async fn dl_bundle(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Reply {
    s.check_read(&headers)?;
    let _permit = s.bundle.try_acquire().map_err(|_| {
        Reject::new(
            StatusCode::CONFLICT,
            "A bug report is already being built. Try again shortly.",
        )
    })?;
    let stamp = files::stamp();
    let status = page::status_text(s.status().await.as_ref());
    let journal = match s.journal().await {
        None => None,
        Some(Ok(b)) => Some(b),
        Some(Err(e)) => Some(format!("journal unavailable: {e}\n").into_bytes()),
    };
    let input = bundle::Input {
        root: format!("carpi-bugreport-{stamp}"),
        status,
        journal,
        config_file: s.cfg.config_file.clone(),
        dirs: vec![
            ("captures".into(), s.cfg.captures.clone()),
            ("sdlink-logs".into(), s.cfg.sdlink_logs.clone()),
        ],
        max: s.cfg.bundle_max,
    };
    let file = tokio::task::spawn_blocking(move || bundle::build(input))
        .await
        .map_err(|e| Reject::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| {
            Reject::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("bug report failed: {e}"),
            )
        })?;
    let name = format!("carpi-bugreport-{stamp}.tar.gz");
    Ok(send_file(tokio::fs::File::from_std(file), &name, "application/gzip").await)
}

/// Reads the `file` field, at most `max` bytes.
async fn read_upload(mut mp: Multipart, max: u64) -> Result<(String, Vec<u8>), Reject> {
    let too_big = || {
        Reject::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("Too big. Limit is {}.", page::size(max)),
        )
    };
    let mp_err = |e: axum::extract::multipart::MultipartError| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            too_big()
        } else {
            Reject::new(e.status(), e.body_text())
        }
    };
    while let Some(mut field) = mp.next_field().await.map_err(mp_err)? {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().unwrap_or_default().to_owned();
        let mut data = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(mp_err)? {
            if (data.len() + chunk.len()) as u64 > max {
                return Err(too_big());
            }
            data.extend_from_slice(&chunk);
        }
        if data.is_empty() {
            return Err(Reject::new(StatusCode::BAD_REQUEST, "The file is empty."));
        }
        return Ok((name, data));
    }
    Err(Reject::new(StatusCode::BAD_REQUEST, "Choose a file."))
}

fn created(msg: String) -> Response {
    (StatusCode::CREATED, msg).into_response()
}

async fn up_config(State(s): State<Arc<Shared>>, headers: HeaderMap, mp: Multipart) -> Reply {
    s.check_pin(&headers)?;
    let validate = s.validate.clone().ok_or_else(|| {
        Reject::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config upload isn't available.",
        )
    })?;
    let (_, data) = read_upload(mp, s.cfg.config_max).await?;
    let text = String::from_utf8(data)
        .map_err(|_| Reject::new(StatusCode::BAD_REQUEST, "Not a text file."))?;
    validate(&text).map_err(|e| {
        Reject::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("Rejected, nothing changed: {e}"),
        )
    })?;
    let target = s.cfg.config_file.clone();
    let backup = tokio::task::spawn_blocking(move || files::replace_with_backup(&target, &text))
        .await
        .map_err(|e| Reject::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| {
            Reject::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Saving failed: {e}"),
            )
        })?;
    tracing::info!(?backup, "portal: carpid.toml replaced");
    Ok(created(match backup {
        Some(b) => format!("Saved carpid.toml. Old one kept as {b}."),
        None => "Saved carpid.toml.".into(),
    }))
}

async fn up_firmware(State(s): State<Arc<Shared>>, headers: HeaderMap, mp: Multipart) -> Reply {
    s.check_pin(&headers)?;
    let (name, data) = read_upload(mp, s.cfg.firmware_max).await?;
    if !files::safe_name(&name) {
        return Err(Reject::new(
            StatusCode::BAD_REQUEST,
            "Bad file name. Use letters, digits, '.', '_' or '-'.",
        ));
    }
    let lower = name.to_ascii_lowercase();
    if !s
        .cfg
        .firmware_ext
        .iter()
        .any(|e| lower.ends_with(e.as_str()))
    {
        return Err(Reject::new(
            StatusCode::BAD_REQUEST,
            format!("Only {} files.", s.cfg.firmware_ext.join(", ")),
        ));
    }
    if lower.ends_with(".hex") {
        check_ihex(&data).map_err(|e| Reject::new(StatusCode::BAD_REQUEST, e))?;
    }
    let dir = s.cfg.firmware.clone();
    let n = name.clone();
    tokio::task::spawn_blocking(move || files::store_new(&dir, &n, &data))
        .await
        .map_err(|e| Reject::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => Reject::new(
                StatusCode::CONFLICT,
                "A file with that name already exists.",
            ),
            _ => Reject::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Saving failed: {e}"),
            ),
        })?;
    tracing::info!(%name, "portal: firmware uploaded");
    Ok(created(format!("Saved {name}. Nothing was flashed.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        let none: &[String] = &[];
        assert!(host_ok("10.10.0.1:8080", none));
        assert!(host_ok("10.10.0.1", none));
        assert!(host_ok("[fe80::1]:8080", none));
        assert!(host_ok("localhost:8080", none));
        assert!(!host_ok("evil.example:8080", none));
        assert!(!host_ok("[evil]:80", none));
        assert!(host_ok("carpi.local", &["carpi.local".into()]));
    }

    #[test]
    fn pin_cookie() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "a=b; carpi_pin=31323334".parse().unwrap());
        assert_eq!(supplied_pin(&h).as_deref(), Some("1234"));
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"1234", b"1234"));
        assert!(!ct_eq(b"1234", b"1235"));
        assert!(!ct_eq(b"1234", b"12345"));
    }
}
