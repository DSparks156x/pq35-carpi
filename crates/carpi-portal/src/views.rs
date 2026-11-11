//! The pages and the logs-folder routes: days, entries, files, zips, star,
//! delete, capture. Every entry or file named in a URL goes through
//! `carpi_logs` ([`EntryId::parse`], [`Logs::dir`], [`Logs::file`]).

use crate::page::{self, Access};
use crate::{Reject, Shared, files};
use axum::Form;
use axum::extract::{Path as UrlPath, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use carpi_logs::EntryId;
use serde::Deserialize;
use std::sync::Arc;
use tokio::io::AsyncReadExt;

/// Cap on the inline text view; download the file for the rest.
const VIEW_MAX: u64 = 1 << 20;

const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; \
                   img-src data:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

/// Who is looking, worked out once per page.
pub struct Viewer {
    pub access: Access,
    pub can_read: bool,
    /// The PIN cookie is wrong: drop it so it stops counting as a failure.
    clear_cookie: bool,
}

impl Viewer {
    pub fn new(s: &Shared, headers: &HeaderMap) -> Self {
        let (access, clear_cookie) = s.access(headers);
        Viewer {
            can_read: !s.cfg.protect_downloads || access.can_write(),
            access,
            clear_cookie,
        }
    }

    /// For error pages after a failed write: no second PIN check.
    pub fn quiet(s: &Shared) -> Self {
        let access = Access {
            pin_set: s.pin().is_some(),
            unlocked: false,
        };
        Viewer {
            can_read: !s.cfg.protect_downloads || !access.pin_set,
            access,
            clear_cookie: false,
        }
    }

    /// After a write's PIN check passed: a header or cookie PIN was right.
    pub fn checked(s: &Shared, headers: &HeaderMap) -> Self {
        let pin_set = s.pin().is_some();
        let access = Access {
            pin_set,
            unlocked: pin_set && crate::supplied_pin(headers).is_some(),
        };
        Viewer {
            can_read: !s.cfg.protect_downloads || access.can_write(),
            access,
            clear_cookie: false,
        }
    }

    pub fn html(&self, code: StatusCode, body: String) -> Response {
        let mut r = html(code, body);
        if self.clear_cookie {
            r.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_static(
                    "carpi_pin=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
                ),
            );
        }
        r
    }

    pub(crate) fn locked(&self, back: &str) -> Response {
        self.html(StatusCode::UNAUTHORIZED, page::locked(self.access, back))
    }

    pub(crate) fn message(
        &self,
        code: StatusCode,
        title: &str,
        text: &str,
        back: &str,
    ) -> Response {
        let ask_pin = matches!(code, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN);
        self.html(code, page::message(self.access, title, text, back, ask_pin))
    }

    pub(crate) fn not_found(&self) -> Response {
        self.message(StatusCode::NOT_FOUND, "Not found", "Nothing here.", "/")
    }

    /// A refused write as a page, with a PIN form when that was the problem.
    pub(crate) fn refused(&self, r: Reject, back: &str) -> Response {
        let title = match r.0 {
            StatusCode::UNAUTHORIZED => "PIN needed",
            StatusCode::FORBIDDEN => "Wrong PIN",
            StatusCode::TOO_MANY_REQUESTS => "Locked for now",
            _ => "Couldn't do that",
        };
        self.message(r.0, title, &r.1, back)
    }
}

pub fn html(code: StatusCode, body: String) -> Response {
    (
        code,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_SECURITY_POLICY, CSP),
        ],
        body,
    )
        .into_response()
}

/// `2026-11-08` + `03`, exactly those shapes.
pub fn entry_id(date: &str, nn: &str) -> Option<EntryId> {
    // EntryId::parse slices at byte 10, so keep it to ASCII.
    if date.len() != 10 || !date.is_ascii() || !nn.is_ascii() {
        return None;
    }
    let id = EntryId::parse(&format!("{date}/{nn}"))?;
    (id.date == date && id.folder() == nn).then_some(id)
}

fn valid_date(date: &str) -> bool {
    entry_id(date, "01").is_some()
}

fn tail(id: &EntryId) -> String {
    format!("{}/{}", id.date, id.folder())
}

pub async fn home(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    let v = Viewer::new(&s, &headers);
    if !v.can_read {
        return v.locked("/");
    }
    let status = s.status().await;
    let days = s.logs.days();
    let body = page::home(v.access, status.as_ref(), s.capture.is_some(), &days);
    v.html(StatusCode::OK, body)
}

pub async fn setup(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    let v = Viewer::new(&s, &headers);
    let lists = page::SetupLists {
        firmware: files::list(&s.cfg.firmware),
        sdlink: files::list(&s.cfg.sdlink_logs),
        captures: files::list(&s.cfg.captures),
    };
    let body = page::setup(
        v.access,
        &s.cfg,
        s.validate.is_some(),
        s.wifi.is_some(),
        &lists,
        v.can_read,
    );
    v.html(StatusCode::OK, body)
}

pub async fn day(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath(date): UrlPath<String>,
) -> Response {
    let v = Viewer::new(&s, &headers);
    if !valid_date(&date) {
        return v.not_found();
    }
    if !v.can_read {
        return v.locked(&format!("/day/{date}"));
    }
    let entries = s.logs.entries(&date);
    if entries.is_empty() {
        return v.not_found();
    }
    v.html(StatusCode::OK, page::day(v.access, &date, &entries))
}

pub async fn entry(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath((date, nn)): UrlPath<(String, String)>,
) -> Response {
    let v = Viewer::new(&s, &headers);
    let Some(id) = entry_id(&date, &nn).filter(|id| s.logs.dir(id).is_some()) else {
        return v.not_found();
    };
    if !v.can_read {
        return v.locked(&format!("/e/{}", tail(&id)));
    }
    let meta = s.logs.meta(&id);
    let files = s.logs.files(&id);
    // A data log gets the trim form.
    let csv = meta
        .as_ref()
        .filter(|m| m.kind == carpi_logs::Kind::Datalog)
        .and_then(|_| s.logs.file(&id, CSV));
    let outline = match csv {
        Some(path) => tokio::task::spawn_blocking(move || crate::trim::outline(&path).ok())
            .await
            .ok()
            .flatten(),
        None => None,
    };
    v.html(
        StatusCode::OK,
        page::entry(v.access, &id, meta.as_ref(), &files, outline.as_ref()),
    )
}

/// A data log's rows.
const CSV: &str = "data.csv";

/// `data.csv` trimmed to the columns and time range the form picked.
pub async fn trim(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath((date, nn)): UrlPath<(String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Response, Reject> {
    s.check_read(&headers)?;
    let (id, file) = entry_file(&s, &date, &nn, CSV)?;
    let path = file.clone();
    let outline = tokio::task::spawn_blocking(move || crate::trim::outline(&path))
        .await
        .map_err(|e| Reject::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|_| Reject::not_found())?;
    let t = crate::trim::parse_for(query.as_deref().unwrap_or(""), &outline)
        .map_err(|e| Reject::new(StatusCode::BAD_REQUEST, e))?;
    let permit = s.zip_permit()?;
    let body = crate::archive::stream_with(
        move |w| {
            let f = std::io::BufReader::new(std::fs::File::open(&file)?);
            crate::trim::copy(f, w, &t).map(|_| ())
        },
        permit,
    );
    let name = format!("carpi-{}-trim.csv", id.slug());
    Ok((crate::attachment(&name, "text/csv; charset=utf-8"), body).into_response())
}

/// Entry and file from the URL, or 404.
fn entry_file(
    s: &Shared,
    date: &str,
    nn: &str,
    path: &str,
) -> Result<(EntryId, std::path::PathBuf), Reject> {
    let id = entry_id(date, nn).ok_or_else(Reject::not_found)?;
    let file = s.logs.file(&id, path).ok_or_else(Reject::not_found)?;
    Ok((id, file))
}

pub async fn file(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath((date, nn, path)): UrlPath<(String, String, String)>,
) -> Result<Response, Reject> {
    s.check_read(&headers)?;
    let (id, file) = entry_file(&s, &date, &nn, &path)?;
    let f = tokio::fs::File::open(&file)
        .await
        .map_err(|_| Reject::not_found())?;
    let name = format!("{}-{}", id.slug(), path.replace('/', "-"));
    Ok(crate::send_file(f, &name, crate::content_type(&path)).await)
}

pub async fn view(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath((date, nn, path)): UrlPath<(String, String, String)>,
) -> Result<Response, Reject> {
    s.check_read(&headers)?;
    let (_, file) = entry_file(&s, &date, &nn, &path)?;
    let f = tokio::fs::File::open(&file)
        .await
        .map_err(|_| Reject::not_found())?;
    let len = f.metadata().await.map_or(0, |m| m.len());
    let mut out = Vec::new();
    f.take(VIEW_MAX)
        .read_to_end(&mut out)
        .await
        .map_err(|e| Reject::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if len > VIEW_MAX {
        out.extend_from_slice(
            format!(
                "\n\n--- cut at {} of {}; download the file for the rest ---\n",
                page::size(VIEW_MAX),
                page::size(len)
            )
            .as_bytes(),
        );
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CONTENT_DISPOSITION, "inline"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        out,
    )
        .into_response())
}

fn zip_reply(name: &str, body: axum::body::Body) -> Response {
    (crate::attachment(name, "application/zip"), body).into_response()
}

pub async fn zip_day(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath(date): UrlPath<String>,
) -> Result<Response, Reject> {
    s.check_read(&headers)?;
    if !valid_date(&date) {
        return Err(Reject::not_found());
    }
    let permit = s.zip_permit()?;
    let logs = s.logs.clone();
    let d = date.clone();
    let items = tokio::task::spawn_blocking(move || crate::archive::day_items(&logs, &d))
        .await
        .map_err(|e| Reject::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if items.is_empty() {
        return Err(Reject::not_found());
    }
    Ok(zip_reply(
        &format!("carpi-{date}.zip"),
        crate::archive::stream(items, permit),
    ))
}

pub async fn zip_entry(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath((date, nn)): UrlPath<(String, String)>,
) -> Result<Response, Reject> {
    s.check_read(&headers)?;
    let id = entry_id(&date, &nn)
        .filter(|id| s.logs.dir(id).is_some())
        .ok_or_else(Reject::not_found)?;
    let permit = s.zip_permit()?;
    let logs = s.logs.clone();
    let i = id.clone();
    let items = tokio::task::spawn_blocking(move || crate::archive::entry_items(&logs, &i, ""))
        .await
        .map_err(|e| Reject::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(zip_reply(
        &format!("carpi-{}.zip", id.slug()),
        crate::archive::stream(items, permit),
    ))
}

#[derive(Deserialize)]
pub struct StarForm {
    back: Option<String>,
}

pub async fn star(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath((date, nn)): UrlPath<(String, String)>,
    Form(f): Form<StarForm>,
) -> Response {
    let v = Viewer::quiet(&s);
    let Some(id) = entry_id(&date, &nn) else {
        return v.not_found();
    };
    let back = if f.back.as_deref() == Some("entry") {
        format!("/e/{}", tail(&id))
    } else {
        format!("/day/{}#e{}", id.date, id.folder())
    };
    if let Err(r) = s.check_pin(&headers) {
        return v.refused(r, &back);
    }
    let Some(meta) = s.logs.meta(&id) else {
        return v.not_found();
    };
    if let Err(e) = s.logs.set_starred(&id, !meta.starred) {
        return v.message(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Couldn't star",
            &e.to_string(),
            &back,
        );
    }
    tracing::info!(%id, starred = !meta.starred, "portal: star");
    Redirect::to(&back).into_response()
}

#[derive(Deserialize)]
pub struct DeleteForm {
    confirm: Option<String>,
    pin: Option<String>,
}

pub async fn delete(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    UrlPath((date, nn)): UrlPath<(String, String)>,
    Form(f): Form<DeleteForm>,
) -> Response {
    let v = Viewer::quiet(&s);
    let Some(id) = entry_id(&date, &nn).filter(|id| s.logs.dir(id).is_some()) else {
        return v.not_found();
    };
    let back = format!("/e/{}", tail(&id));
    if let Err(r) = s.check_pin_or_form(&headers, f.pin.as_deref()) {
        return v.refused(r, &back);
    }
    if f.confirm.as_deref() != Some("yes") {
        return v.message(
            StatusCode::BAD_REQUEST,
            "Not deleted",
            "Tick the box to confirm.",
            &back,
        );
    }
    if let Err(e) = s.logs.remove(&id) {
        return v.message(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Couldn't delete",
            &e.to_string(),
            &back,
        );
    }
    tracing::info!(%id, "portal: deleted");
    if s.logs.entries(&id.date).is_empty() {
        Redirect::to("/").into_response()
    } else {
        Redirect::to(&format!("/day/{}", id.date)).into_response()
    }
}

pub async fn capture(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    let v = Viewer::quiet(&s);
    let Some(hook) = s.capture.clone() else {
        return v.not_found();
    };
    if let Err(r) = s.check_pin(&headers) {
        return v.refused(r, "/");
    }
    let Ok(_running) = s.capturing.try_acquire() else {
        return v.message(
            StatusCode::CONFLICT,
            "Already collecting",
            "Debug logs are being collected. They show up on Home when done.",
            "/",
        );
    };
    match hook("portal".into()).await {
        Ok(label) => {
            tracing::info!(%label, "portal: capture");
            let id = label
                .is_ascii()
                .then(|| EntryId::parse(&label))
                .flatten()
                .filter(|id| s.logs.dir(id).is_some());
            match id {
                Some(id) => Redirect::to(&format!("/e/{}", tail(&id))).into_response(),
                None => v.message(StatusCode::OK, "Debug logs saved", &format!("Saved {label}."), "/"),
            }
        }
        Err(e) => v.message(StatusCode::INTERNAL_SERVER_ERROR, "Debug logs failed", &e, "/"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_urls() {
        assert!(entry_id("2026-11-08", "03").is_some());
        assert!(entry_id("2026-11-08", "123").is_some());
        for (d, n) in [
            ("2026-11-08", "3"),
            ("2026-11-08", "-03"),
            ("2026-11-08", "#03"),
            ("2026-11-08", ".."),
            ("..", ".."),
            ("x", "01"),
            ("2026-11-0é", "01"),
            ("2026-11-08", "0é"),
            ("2026-11-08", "003"),
        ] {
            assert!(entry_id(d, n).is_none(), "{d} {n}");
        }
    }
}
