//! Phone portal: a small HTTP server on the car's hotspot. Browse the logs
//! folder (`carpi_logs`: one entry per capture or data-log run, by day),
//! download files or zips of an entry or a whole day, star, delete, start a
//! capture; download the carpid journal, sdlink logs and a bug report; upload
//! a new `carpid.toml` or a Teensy `.hex`. Nothing is flashed or restarted
//! from here.
//!
//! # Routes
//!
//! ```text
//! GET  /                       home: status, Capture now, days
//! GET  /setup                  config, firmware, PIN, bug report, sdlink logs, older captures
//! GET  /day/<date>             entries of a day
//! GET  /e/<date>/<NN>          one entry: files, star, delete
//! GET  /f/<date>/<NN>/<path>   one file of an entry (download)
//! GET  /v/<date>/<NN>/<path>   the same as text/plain, first 1 MiB
//! GET  /zip/day/<date>         carpi-<date>.zip, a folder per entry
//! GET  /zip/e/<date>/<NN>      carpi-<date>-<NN>.zip
//! POST /star/<date>/<NN>       toggle the star            (PIN)
//! POST /delete/<date>/<NN>     confirm=yes                 (PIN)
//! POST /capture                runs the capture hook       (PIN)
//! POST /pin, /lock             set / drop the PIN cookie
//! GET  /api/status, /dl/config, /dl/journal, /dl/bugreport, /dl/<kind>/<name>
//! POST /up/config, /up/firmware                            (PIN)
//! ```
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
//! - Logs entries and their files only resolve through `carpi_logs`
//!   (`EntryId::parse`, `Logs::dir`, `Logs::file`): fixed date/number shapes,
//!   and only paths the entry listing itself produced. Symlinks are skipped.
//! - Star, delete and capture are writes: PIN when one is set. Delete also
//!   needs `confirm=yes`.
//! - At most two zips build at once, one capture at a time.
//! - Uploads have size caps, and firmware only lands as a new, valid Intel HEX
//!   file (never overwrites). Config is validated by the caller before it
//!   replaces anything, and the old file is backed up first.
//! - POSTs with a foreign `Origin` are refused (CSRF from a page the phone has
//!   open). `Host` must be an IP literal, `localhost` or listed in
//!   `allowed_hosts` (DNS rebinding).
//! - Five wrong PINs lock PIN checks for 30 s.

mod archive;
mod files;
mod page;
mod views;

use axum::extract::{DefaultBodyLimit, Multipart, Path as UrlPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use carpi_logs::Logs;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

pub use files::{check_ihex, replace_with_backup};

/// The upload PIN, shared so the daemon can change it while the portal runs.
/// `None` or empty: no PIN.
pub type PinCell = Arc<RwLock<Option<String>>>;

/// Portal settings. Deserializes from a `[portal]` table; every key is optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PortalConfig {
    /// Listen address. Default `0.0.0.0:5003`, the old portal's port.
    pub bind: SocketAddr,
    /// The logs folder (`carpi_logs`): `<logs>/<YYYY-MM-DD>/<NN>/`.
    pub logs: PathBuf,
    /// Old flat CAN captures; listed on Setup as "Older captures" when not empty.
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
    /// Bug-report cap on the newest entry's file bytes; files past it are listed, not included.
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
            bind: SocketAddr::from(([0, 0, 0, 0], 5003)),
            logs: home.join("logs"),
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
type CaptureFn = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync,
>;

/// Builder. `Portal::new(cfg).status(..).validate_config(..).capture(..).serve(..)`.
pub struct Portal {
    cfg: PortalConfig,
    pin: PinCell,
    status: Option<StatusFn>,
    validate: Option<ValidateFn>,
    capture: Option<CaptureFn>,
}

impl Portal {
    pub fn new(cfg: PortalConfig) -> Self {
        Portal {
            pin: Arc::new(RwLock::new(cfg.pin.clone())),
            cfg,
            status: None,
            validate: None,
            capture: None,
        }
    }

    /// The live PIN. Writes take effect on the next request.
    pub fn pin_cell(&self) -> PinCell {
        self.pin.clone()
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

    /// Starts a capture. Called by POST `/capture` with the trigger
    /// (`"portal"`); returns the new entry's label, `"2026-11-08 #03"`, and
    /// the phone is sent to that entry. `Err` text is shown to the phone.
    /// Without a hook the Capture now button is hidden and the route is 404.
    pub fn capture<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String, String>> + Send + 'static,
    {
        self.capture = Some(Arc::new(move |t| Box::pin(f(t))));
        self
    }

    pub fn router(self) -> Router {
        let fw_limit = self.cfg.firmware_max as usize + (64 << 10);
        let cfg_limit = self.cfg.config_max as usize + (64 << 10);
        let shared = Arc::new(Shared {
            logs: Logs::new(&self.cfg.logs),
            cfg: self.cfg,
            pin: self.pin,
            status: self.status,
            validate: self.validate,
            capture: self.capture,
            fails: Mutex::new(Fails::default()),
            zips: Arc::new(Semaphore::new(ZIPS)),
            capturing: Semaphore::new(1),
        });
        Router::new()
            .route("/", get(views::home))
            .route("/files", get(|| async { Redirect::to("/") }))
            .route("/setup", get(views::setup))
            .route("/day/{date}", get(views::day))
            .route("/e/{date}/{nn}", get(views::entry))
            .route("/f/{date}/{nn}/{*path}", get(views::file))
            .route("/v/{date}/{nn}/{*path}", get(views::view))
            .route("/zip/day/{date}", get(views::zip_day))
            .route("/zip/e/{date}/{nn}", get(views::zip_entry))
            .route("/star/{date}/{nn}", post(views::star))
            .route("/delete/{date}/{nn}", post(views::delete))
            .route("/capture", post(views::capture))
            .route("/lock", post(lock_handler))
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
        tracing::info!("portal on http://{}/", listener.local_addr()?);
        axum::serve(listener, self.router())
            .with_graceful_shutdown(shutdown)
            .await
    }
}

struct Shared {
    cfg: PortalConfig,
    logs: Logs,
    pin: PinCell,
    status: Option<StatusFn>,
    validate: Option<ValidateFn>,
    capture: Option<CaptureFn>,
    fails: Mutex<Fails>,
    /// Zips being built (downloads and bug reports).
    zips: Arc<Semaphore>,
    capturing: Semaphore,
}

const ZIPS: usize = 2;

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
    fn pin(&self) -> Option<String> {
        self.pin
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .filter(|p| !p.is_empty())
    }

    fn check_pin(&self, headers: &HeaderMap) -> Result<(), Reject> {
        let Some(pin) = self.pin() else {
            return Ok(());
        };
        let Some(given) = supplied_pin(headers) else {
            return Err(Reject::new(StatusCode::UNAUTHORIZED, "PIN required."));
        };
        self.try_pin(&pin, &given)
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

    /// Header or cookie PIN if there is one, else the PIN typed into the form.
    fn check_pin_or_form(&self, headers: &HeaderMap, form: Option<&str>) -> Result<(), Reject> {
        match (self.pin(), supplied_pin(headers), form) {
            (Some(pin), None, Some(given)) => self.try_pin(&pin, given),
            _ => self.check_pin(headers),
        }
    }

    /// For rendering: is the PIN set, is this phone's cookie right. A cookie
    /// counts as a try like any other; the bool says it's wrong and should go.
    fn access(&self, headers: &HeaderMap) -> (page::Access, bool) {
        let Some(pin) = self.pin() else {
            return (
                page::Access {
                    pin_set: false,
                    unlocked: false,
                },
                false,
            );
        };
        let (unlocked, wrong) = match supplied_pin(headers) {
            None => (false, false),
            Some(given) => match self.try_pin(&pin, &given) {
                Ok(()) => (true, false),
                Err(r) => (false, r.0 == StatusCode::FORBIDDEN),
            },
        };
        (
            page::Access {
                pin_set: true,
                unlocked,
            },
            wrong,
        )
    }

    fn zip_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Reject> {
        self.zips.clone().try_acquire_owned().map_err(|_| {
            Reject::new(
                StatusCode::CONFLICT,
                "Another zip is being built. Try again when it's done.",
            )
        })
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

async fn status_handler(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Reply {
    s.check_read(&headers)?;
    Ok(Json(s.status().await.unwrap_or_default()).into_response())
}

#[derive(Deserialize)]
struct PinForm {
    pin: String,
    back: Option<String>,
}

/// A path on this site to go back to: `/day/2026-11-08#e03`, never `//host`.
fn local_path(back: Option<&str>) -> &str {
    match back {
        Some(b)
            if b.starts_with('/')
                && !b.starts_with("//")
                && b.len() < 200
                && b.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"/-_.#".contains(&c)) =>
        {
            b
        }
        _ => "/setup",
    }
}

/// Checks the PIN and stores it in a cookie so links and forms carry it.
async fn pin_handler(State(s): State<Arc<Shared>>, Form(f): Form<PinForm>) -> Response {
    let back = local_path(f.back.as_deref()).to_owned();
    let Some(pin) = s.pin() else {
        return Redirect::to(&back).into_response();
    };
    if let Err(r) = s.try_pin(&pin, &f.pin) {
        let v = views::Viewer::quiet(&s);
        let title = if r.0 == StatusCode::FORBIDDEN {
            "Wrong PIN"
        } else {
            "Locked for now"
        };
        let body = page::message(v.access, title, &r.1, &back, true);
        return views::html(r.0, body);
    }
    let cookie = format!(
        "{PIN_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000",
        files::hex(f.pin.as_bytes())
    );
    let mut r = Redirect::to(&back).into_response();
    r.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("ascii"),
    );
    r
}

/// Forgets the PIN on this phone.
async fn lock_handler() -> Response {
    let mut r = Redirect::to("/setup").into_response();
    r.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("carpi_pin=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"),
    );
    r
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
        Some(e) if e == "json" => "application/json",
        Some(e) if e == "zip" => "application/zip",
        Some(e) if e == "zst" => "application/zstd",
        Some(e) if e == "gz" => "application/gzip",
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

/// Zip of the newest entry, `status.txt` and the current config.
async fn dl_bundle(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Reply {
    s.check_read(&headers)?;
    let permit = s.zip_permit()?;
    let stamp = files::stamp();
    let status = page::status_text(s.status().await.as_ref());
    let logs = s.logs.clone();
    let config = s.cfg.config_file.clone();
    let max = s.cfg.bundle_max;
    let items = tokio::task::spawn_blocking(move || bug_items(&logs, status, config, max))
        .await
        .map_err(|e| Reject::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let name = format!("carpi-bugreport-{stamp}.zip");
    Ok((
        attachment(&name, "application/zip"),
        archive::stream(items, permit),
    )
        .into_response())
}

fn bug_items(logs: &Logs, status: String, config: PathBuf, max: u64) -> Vec<archive::Item> {
    use archive::{Item, Src};
    let mut items = vec![Item {
        name: "status.txt".into(),
        src: Src::Bytes(status.into_bytes()),
        time: None,
    }];
    if config.is_file() {
        items.push(Item {
            name: "carpid.toml".into(),
            src: Src::File(config),
            time: None,
        });
    }
    let newest = logs
        .days()
        .first()
        .and_then(|d| logs.entries(&d.date).into_iter().next());
    let Some(e) = newest else {
        items.push(Item {
            name: "no-entries.txt".into(),
            src: Src::Bytes(b"The logs folder has no entries.\n".to_vec()),
            time: None,
        });
        return items;
    };
    let sizes: std::collections::HashMap<String, u64> = logs
        .files(&e.id)
        .into_iter()
        .map(|f| (f.path, f.bytes))
        .collect();
    let mut used = 0u64;
    let mut skipped = String::new();
    for it in archive::entry_items(logs, &e.id, &format!("{}/", e.id.slug())) {
        let rel = it.name.split_once('/').map_or("", |(_, r)| r);
        let n = sizes.get(rel).copied().unwrap_or(0);
        if used + n > max {
            skipped.push_str(&format!("{} ({n} bytes)\n", it.name));
            continue;
        }
        used += n;
        items.push(it);
    }
    if !skipped.is_empty() {
        items.push(Item {
            name: "skipped.txt".into(),
            src: Src::Bytes(
                format!("Over the {max} byte cap, not included:\n{skipped}").into_bytes(),
            ),
            time: None,
        });
    }
    items
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
