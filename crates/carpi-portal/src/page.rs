//! The HTML. Server-rendered, inline CSS, a few lines of JS for niceties
//! (upload replies in place, busy buttons, delete confirm). Everything works
//! without JS.

use crate::files;
use crate::{PortalConfig, Status};
use carpi_logs::{Day, Entry, EntryId, FileInfo, Kind, Meta};
use std::fmt::Write;

/// What the viewer may do, for the top bar and forms.
#[derive(Clone, Copy)]
pub struct Access {
    pub pin_set: bool,
    /// The PIN cookie is right.
    pub unlocked: bool,
}

impl Access {
    /// Writes need no PIN entry right now.
    pub fn can_write(self) -> bool {
        !self.pin_set || self.unlocked
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Home,
    Setup,
    None,
}

pub fn size(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.2} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 10 << 20 => format!("{:.0} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{} KB", n.div_ceil(1 << 10)),
        n => format!("{n} B"),
    }
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

fn uptime(s: u64) -> String {
    match s {
        s if s >= 86_400 => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
        s if s >= 3600 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        s => format!("{}m {}s", s / 60, s % 60),
    }
}

fn status_rows(st: &Status) -> Vec<(String, String)> {
    let mut rows = vec![
        ("carpid".to_string(), st.version.clone()),
        ("Uptime".to_string(), uptime(st.uptime_s)),
        ("Bus".to_string(), st.bus.clone()),
    ];
    rows.extend(st.extra.iter().cloned());
    rows
}

/// Plain-text status for the bug report.
pub fn status_text(st: Option<&Status>) -> String {
    let mut o = format!(
        "taken {} UTC\n",
        files::short_time(std::time::SystemTime::now())
    );
    match st {
        Some(st) => {
            for (k, v) in status_rows(st) {
                let _ = writeln!(o, "{k}: {v}");
            }
        }
        None => o.push_str("status not available\n"),
    }
    o
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// `2026-11-08` -> (2026, 11, 8, weekday 0 = Sunday).
fn ymd(date: &str) -> Option<(i64, u32, u32, usize)> {
    let y: i64 = date.get(0..4)?.parse().ok()?;
    let m: u32 = date.get(5..7)?.parse().ok()?;
    let d: u32 = date.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's days_from_civil; 1970-01-01 was a Thursday.
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((y, m, d, (days + 4).rem_euclid(7) as usize))
}

/// "Sunday 8 Nov 2026".
pub fn long_date(date: &str) -> String {
    match ymd(date) {
        Some((y, m, d, w)) => format!("{} {d} {} {y}", WEEKDAYS[w], MONTHS[m as usize - 1]),
        None => date.to_string(),
    }
}

/// `HH:MM` from `YYYY-MM-DDTHH:MM:SS`.
fn hm(t: &str) -> &str {
    t.get(11..16).unwrap_or(t)
}

fn hms(t: &str) -> &str {
    t.get(11..19).unwrap_or(t)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `2026-11-08/03`, the tail of entry URLs.
fn tail(id: &EntryId) -> String {
    format!("{}/{}", id.date, id.folder())
}

fn num(id: &EntryId) -> String {
    format!("#{}", id.folder())
}

// Icons: 24x24 strokes, coloured by currentColor.
const I_STAR: &str = r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3.2l2.7 5.6 6.1.8-4.5 4.2 1.1 6.1L12 17l-5.4 2.9 1.1-6.1-4.5-4.2 6.1-.8z"/></svg>"#;
const I_DL: &str = r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4v11m-5-5 5 5 5-5M5 20h14"/></svg>"#;
const I_CHEV: &str =
    r#"<svg class="chev" viewBox="0 0 24 24" aria-hidden="true"><path d="m9 6 6 6-6 6"/></svg>"#;
const I_LOCK: &str = r#"<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/></svg>"#;
const I_OPEN: &str = r#"<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 7.5-2"/></svg>"#;
const I_REC: &str = r#"<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8"/><circle class="fill" cx="12" cy="12" r="3.5"/></svg>"#;
const I_EYE: &str = r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="2.8"/></svg>"#;
const LOGO: &str = r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 15.5V13l2.2-4.6A2 2 0 0 1 7 7.3h10a2 2 0 0 1 1.8 1.1L21 13v2.5a1 1 0 0 1-1 1h-1.2M5.2 16.5H4a1 1 0 0 1-1-1M9 16.5h6M3.6 12.5h16.8"/><circle cx="7" cy="16.5" r="1.8"/><circle cx="17" cy="16.5" r="1.8"/></svg>"#;

/// The whole page around `body`.
pub fn layout(title: &str, tab: Tab, access: Access, body: &str) -> String {
    let mut o = String::with_capacity(body.len() + HEAD.len() + 2048);
    o.push_str(HEAD);
    let _ = write!(o, "<title>{} · carpi</title></head><body>", esc(title));
    let on = |t: Tab| if t == tab { " on" } else { "" };
    let lock = match (access.pin_set, access.unlocked) {
        (false, _) => String::new(),
        (true, true) => format!(
            "<a class=\"lock open\" href=\"/setup#pin\" aria-label=\"Unlocked\" title=\"Unlocked\">{I_OPEN}</a>"
        ),
        (true, false) => format!(
            "<a class=lock href=\"/setup#pin\" aria-label=\"Locked: enter PIN\" title=\"Enter PIN\">{I_LOCK}</a>"
        ),
    };
    let _ = write!(
        o,
        "<header class=top><nav>\
         <a class=brand href=\"/\">{LOGO}<span>carpi</span></a><span class=grow></span>{lock}\
         <a class=\"tab{}\" href=\"/\">Home</a><a class=\"tab{}\" href=\"/setup\">Setup</a>\
         </nav></header><main>{body}</main>",
        on(Tab::Home),
        on(Tab::Setup)
    );
    o.push_str(TAIL);
    o
}

fn pin_form(o: &mut String, back: &str, label: &str) {
    let _ = write!(
        o,
        "<form class=pinf method=post action=\"/pin\">\
         <input type=hidden name=back value=\"{}\">\
         <input name=pin type=password inputmode=numeric autocomplete=current-password placeholder=PIN aria-label=PIN required>\
         <button class=\"btn pri\">{label}</button></form>",
        esc(back)
    );
}

/// The PIN prompt shown instead of a page when downloads are protected.
pub fn locked(access: Access, back: &str) -> String {
    let mut b = format!("<section class=\"card pad hero\"><div class=big-ico>{I_LOCK}</div>");
    b.push_str("<h1>PIN needed</h1><p class=mut>Downloads on this car are protected. Enter the PIN once; this phone remembers it.</p>");
    pin_form(&mut b, back, "Unlock");
    b.push_str("</section>");
    layout("Locked", Tab::None, access, &b)
}

/// A message page: errors from forms, capture results.
pub fn message(access: Access, title: &str, text: &str, back: &str, ask_pin: bool) -> String {
    let mut b = format!(
        "<section class=\"card pad\"><h1>{}</h1><p>{}</p>",
        esc(title),
        esc(text)
    );
    if ask_pin {
        pin_form(&mut b, back, "Use PIN");
    }
    let _ = write!(
        b,
        "<div class=btns><a class=\"btn\" href=\"{}\">Back</a></div></section>",
        esc(back)
    );
    layout(title, Tab::None, access, &b)
}

fn status_card(o: &mut String, status: Option<&Status>) {
    o.push_str("<section class=\"card status\">");
    match status {
        Some(st) => {
            let cls = match st.bus.to_ascii_lowercase().as_str() {
                "awake" | "on" | "running" => "ok",
                "asleep" | "silent" | "off" => "idle",
                _ => "warn",
            };
            let _ = write!(
                o,
                "<div class=shead><span class=mut>Car</span><span class=\"pill {cls}\"><i></i>{}</span></div><dl>",
                esc(if st.bus.is_empty() { "unknown" } else { &st.bus })
            );
            for (k, v) in status_rows(st) {
                if k == "Bus" {
                    continue;
                }
                let _ = write!(o, "<div><dt>{}</dt><dd>{}</dd></div>", esc(&k), esc(&v));
            }
            o.push_str("</dl>");
        }
        None => o.push_str(
            "<div class=shead><span class=mut>Car</span><span class=\"pill warn\"><i></i>no status</span></div>",
        ),
    }
    o.push_str("</section>");
}

pub fn home(access: Access, status: Option<&Status>, can_capture: bool, days: &[Day]) -> String {
    let mut b = String::new();
    status_card(&mut b, status);
    if can_capture {
        let _ = write!(
            b,
            "<form class=capture method=post action=\"/capture\" data-busy=\"Capturing…\">\
             <button class=\"btn pri wide tall\">{I_REC}<span>Capture now</span></button></form>\
             <p class=hint>Saves the last 5 minutes: CAN, journal, values, config.</p>"
        );
    }
    b.push_str("<h2>Days</h2>");
    if days.is_empty() {
        b.push_str(
            "<section class=\"card pad empty\"><p class=mut>No captures or data logs yet.</p></section>",
        );
    } else {
        b.push_str("<section class=\"card list\">");
        for d in days {
            let (tile_d, tile_m, wd) = match ymd(&d.date) {
                Some((_, m, dd, w)) => (format!("{dd:02}"), MONTHS[m as usize - 1], WEEKDAYS[w]),
                None => (String::new(), "", ""),
            };
            let _ = write!(
                b,
                "<a class=row href=\"/day/{date}\">\
                 <span class=tile><b>{tile_d}</b><small>{tile_m}</small></span>\
                 <span class=main><span class=t>{wd}</span><span class=s>{date} · {n}</span></span>\
                 <span class=r>{sz}</span>{I_CHEV}</a>",
                date = esc(&d.date),
                n = plural(d.entries, "entry", "entries"),
                sz = size(d.bytes),
            );
        }
        b.push_str("</section>");
    }
    layout("Home", Tab::Home, access, &b)
}

fn kind_badge(meta: Option<&Meta>) -> &'static str {
    match meta.map(|m| m.kind) {
        Some(Kind::Capture) => "<span class=\"badge cap\">capture</span>",
        Some(Kind::Datalog) => "<span class=\"badge log\">datalog</span>",
        None => "<span class=badge>unknown</span>",
    }
}

fn star_form(o: &mut String, id: &EntryId, starred: bool, back: &str) {
    let (cls, label) = if starred {
        ("icon star on", "Unstar")
    } else {
        ("icon star", "Star: never pruned")
    };
    let _ = write!(
        o,
        "<form method=post action=\"/star/{}\"><input type=hidden name=back value={back}>\
         <button class=\"{cls}\" aria-pressed={starred} aria-label=\"{label}\" title=\"{label}\">{I_STAR}</button></form>",
        tail(id)
    );
}

pub fn day(access: Access, date: &str, entries: &[Entry]) -> String {
    let total: u64 = entries.iter().map(|e| e.bytes).sum();
    let mut b = format!(
        "<div class=phead><a class=back href=\"/\">Days</a><h1>{}</h1><p class=mut>{} · {}</p></div>\
         <a class=\"btn pri wide\" href=\"/zip/day/{d}\" download>{I_DL}<span>Download day (.zip)</span></a>",
        esc(&long_date(date)),
        plural(entries.len(), "entry", "entries"),
        size(total),
        d = esc(date),
    );
    b.push_str("<div class=stack>");
    for e in entries {
        let m = e.meta.as_ref();
        let _ = write!(
            b,
            "<article class=\"card ent\" id=e{f}>\
             <a class=ehead href=\"/e/{t}\">\
             <span class=num>{n}</span>\
             <span class=main><span class=t>{badge}<span class=trig>{trigger}</span></span>\
             <span class=s>{summary}</span></span>\
             <span class=r><b>{time}</b><small>{sz}</small></span>{I_CHEV}</a><div class=acts>",
            f = e.id.folder(),
            t = tail(&e.id),
            n = num(&e.id),
            badge = kind_badge(m),
            trigger = m.map_or(String::new(), |m| esc(&m.trigger)),
            summary = m.map_or(String::new(), |m| esc(&m.summary)),
            time = esc(m.map_or("", |m| hm(&m.started))),
            sz = size(e.bytes),
        );
        if let Some(m) = m {
            star_form(&mut b, &e.id, m.starred, "day");
        }
        let _ = write!(
            b,
            "<a class=\"btn grow\" href=\"/zip/e/{t}\" download>{I_DL}<span>Download {n} (.zip)</span></a></div></article>",
            t = tail(&e.id),
            n = num(&e.id),
        );
    }
    b.push_str("</div>");
    layout(&long_date(date), Tab::None, access, &b)
}

/// Files the browser can show as plain text.
pub fn is_text(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    matches!(
        ext,
        "txt" | "log" | "csv" | "toml" | "json" | "md" | "conf" | "ini" | "yaml" | "yml" | "hex"
    )
}

pub fn entry(access: Access, id: &EntryId, meta: Option<&Meta>, files: &[FileInfo]) -> String {
    let total: u64 = files.iter().map(|f| f.bytes).sum();
    let mut b = format!(
        "<div class=phead><a class=back href=\"/day/{d}#e{f}\">{ld}</a>\
         <h1><span class=num>{n}</span> {badge}</h1>",
        d = esc(&id.date),
        f = id.folder(),
        ld = esc(&long_date(&id.date)),
        n = num(id),
        badge = kind_badge(meta),
    );
    b.push_str("<dl class=facts>");
    match meta {
        Some(m) => {
            let ended = match &m.ended {
                Some(e) => esc(hms(e)),
                None => "<span class=warn-t>not finished</span>".into(),
            };
            let _ = write!(
                b,
                "<div><dt>Started</dt><dd>{}</dd></div><div><dt>Ended</dt><dd>{ended}</dd></div>\
                 <div><dt>Trigger</dt><dd>{}</dd></div>",
                esc(hms(&m.started)),
                esc(&m.trigger)
            );
        }
        None => b.push_str("<div class=full><dt>Info</dt><dd>No readable entry.json.</dd></div>"),
    }
    let _ = write!(
        b,
        "<div><dt>Size</dt><dd>{} · {}</dd></div>",
        size(total),
        plural(files.len(), "file", "files")
    );
    if let Some(m) = meta.filter(|m| !m.summary.is_empty()) {
        let _ = write!(
            b,
            "<div class=full><dt>Summary</dt><dd>{}</dd></div>",
            esc(&m.summary)
        );
    }
    let _ = write!(
        b,
        "</dl></div><div class=row-btns>\
         <a class=\"btn pri grow\" href=\"/zip/e/{t}\" download>{I_DL}<span>Download {n} (.zip)</span></a>",
        t = tail(id),
        n = num(id)
    );
    if let Some(m) = meta {
        star_form(&mut b, id, m.starred, "entry");
    }
    b.push_str("</div><h2>Files</h2><section class=\"card list\">");
    if files.is_empty() {
        b.push_str("<div class=row><span class=\"main mut\">No files.</span></div>");
    }
    for f in files {
        let p = esc(&f.path);
        let view = if is_text(&f.path) {
            format!(
                "<a class=icon href=\"/v/{t}/{p}\" aria-label=\"View {p}\" title=\"View\">{I_EYE}</a>",
                t = tail(id)
            )
        } else {
            String::new()
        };
        let _ = write!(
            b,
            "<div class=\"row file-row\"><span class=main><span class=\"t mono\">{p}</span></span>\
             <span class=r>{sz}</span>{view}<a class=icon href=\"/f/{t}/{p}\" download aria-label=\"Download {p}\" title=\"Download\">{I_DL}</a></div>",
            sz = size(f.bytes),
            t = tail(id),
        );
    }
    b.push_str("</section>");

    // Delete: PIN (when not unlocked) and a confirm box; JS adds a dialog.
    let _ = write!(
        b,
        "<h2>Delete</h2><form class=\"card pad danger\" method=post action=\"/delete/{t}\" \
         data-confirm=\"Delete {n} and its files? This can't be undone.\">\
         <p class=mut>Removes {n} and its {nf} from the car. There is no undo.{star}</p>",
        t = tail(id),
        n = num(id),
        nf = plural(files.len(), "file", "files"),
        star = if meta.is_some_and(|m| m.starred) {
            " It is starred."
        } else {
            ""
        },
    );
    if !access.can_write() {
        b.push_str("<input name=pin type=password inputmode=numeric autocomplete=current-password placeholder=PIN aria-label=PIN required>");
    }
    let _ = write!(
        b,
        "<label class=check><input type=checkbox name=confirm value=yes required><span>Yes, delete {n}</span></label>\
         <button class=\"btn bad wide\">Delete {n}</button></form>",
        n = num(id)
    );
    layout(&id.to_string(), Tab::None, access, &b)
}

/// One loose file for the setup lists.
pub type Loose = files::Entry;

fn loose_list(o: &mut String, kind: &str, list: &[Loose], empty: &str) {
    o.push_str("<section class=\"card list\">");
    if list.is_empty() {
        let _ = write!(
            o,
            "<div class=row><span class=\"main mut\">{empty}</span></div>"
        );
    }
    for e in list.iter().take(200) {
        let n = esc(&e.name);
        let _ = write!(
            o,
            "<a class=row href=\"/dl/{kind}/{n}\" download><span class=main><span class=\"t mono\">{n}</span>\
             <span class=s>{} UTC</span></span><span class=r>{}</span><span class=icon>{I_DL}</span></a>",
            files::short_time(e.modified),
            size(e.size),
        );
    }
    if list.len() > 200 {
        let _ = write!(
            o,
            "<div class=row><span class=\"main mut\">{} older files not shown.</span></div>",
            list.len() - 200
        );
    }
    o.push_str("</section>");
}

fn upload_form(o: &mut String, action: &str, accept: &str, label: &str, need_pin: bool) {
    let _ = write!(
        o,
        "<form class=up method=post action=\"{action}\" enctype=\"multipart/form-data\">\
         <label class=file><input type=file name=file accept=\"{accept}\" required><span>Choose file</span></label>\
         <button class=\"btn pri\"{dis}>{label}</button><output></output></form>",
        dis = if need_pin {
            " disabled title=\"Enter the PIN first\""
        } else {
            ""
        },
    );
}

pub struct SetupLists {
    pub firmware: Vec<Loose>,
    pub sdlink: Vec<Loose>,
    pub captures: Vec<Loose>,
}

pub fn setup(
    access: Access,
    cfg: &PortalConfig,
    can_config: bool,
    lists: &SetupLists,
    can_read: bool,
) -> String {
    let mut b = String::from("<div class=phead><h1>Setup</h1></div>");
    let need_pin = !access.can_write();

    if access.pin_set {
        b.push_str("<h2 id=pin>PIN</h2><section class=\"card pad\">");
        if access.unlocked {
            let _ = write!(
                b,
                "<p class=okline>{I_OPEN}<span>This phone is unlocked.</span></p>\
                 <form method=post action=\"/lock\"><button class=btn>Lock this phone</button></form>"
            );
        } else {
            let what = if cfg.protect_downloads {
                "everything"
            } else {
                "uploads, stars, deletes and captures"
            };
            let _ = write!(b, "<p class=mut>The PIN is needed for {what}.</p>");
            pin_form(&mut b, "/setup", "Unlock");
        }
        b.push_str("</section>");
    }
    if !can_read {
        return layout("Setup", Tab::Setup, access, &b);
    }

    b.push_str("<h2>Config</h2><section class=\"card pad\">");
    if cfg.config_file.is_file() {
        let _ = write!(
            b,
            "<a class=\"btn wide\" href=\"/dl/config\" download>{I_DL}<span>Download carpid.toml</span></a>"
        );
    }
    if can_config {
        b.push_str("<p class=mut>Upload replaces carpid.toml if it passes carpid's checks. The old one is kept beside it.</p>");
        upload_form(&mut b, "/up/config", ".toml", "Upload", need_pin);
    } else {
        b.push_str("<p class=mut>Upload not available.</p>");
    }
    b.push_str("</section>");

    let _ = write!(
        b,
        "<h2>Firmware</h2><section class=\"card pad\"><p class=mut>{} up to {}. Stored only; nothing is flashed.</p>",
        esc(&cfg.firmware_ext.join(", ")),
        size(cfg.firmware_max)
    );
    upload_form(
        &mut b,
        "/up/firmware",
        &esc(&cfg.firmware_ext.join(",")),
        "Upload",
        need_pin,
    );
    b.push_str("</section><div class=gap></div>");
    loose_list(&mut b, "firmware", &lists.firmware, "No firmware uploaded.");

    let _ = write!(
        b,
        "<h2>Bug report</h2><section class=\"card pad\">\
         <p class=mut>The newest capture, the status and the config in one .zip.</p>\
         <a class=\"btn wide\" href=\"/dl/bugreport\" download>{I_DL}<span>Download bug report</span></a>"
    );
    if !cfg.journal.is_empty() {
        let _ = write!(
            b,
            "<a class=\"btn wide\" href=\"/dl/journal\" download>{I_DL}<span>carpid journal</span></a>"
        );
    }
    b.push_str("</section>");

    b.push_str("<h2>sdlink logs</h2>");
    loose_list(&mut b, "sdlink", &lists.sdlink, "None.");
    if !lists.captures.is_empty() {
        b.push_str("<h2>Older captures</h2>");
        loose_list(&mut b, "captures", &lists.captures, "None.");
    }
    layout("Setup", Tab::Setup, access, &b)
}

const HEAD: &str = r##"<!doctype html><html lang=en><head><meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name=color-scheme content="dark light">
<meta name=theme-color content="#0d0f12" media="(prefers-color-scheme: dark)">
<meta name=theme-color content="#f3f4f6" media="(prefers-color-scheme: light)">
<link rel=icon href="data:,">
<style>
:root{--bg:#0d0f12;--card:#16191e;--card2:#1f232a;--line:#2a2f37;--fg:#eef1f4;--mut:#9aa3ae;
--acc:#ffb224;--acc-fg:#1b1300;--link:#8cb8ff;--ok:#36d399;--idle:#8a94a0;--warn:#ffb224;--bad:#ff6b6b;
--star:#ffcc33;--cap:#8cb8ff;--log:#c99bff;--r:16px;--sh:none}
@media (prefers-color-scheme:light){:root{--bg:#f3f4f6;--card:#fff;--card2:#eff1f4;--line:#dde1e6;--fg:#0c0e11;
--mut:#545d68;--acc:#f59e0b;--acc-fg:#140d00;--link:#0b57d0;--ok:#0b8a4c;--idle:#69727e;--warn:#a85f00;--bad:#c62828;
--star:#d99a00;--cap:#0b57d0;--log:#7b3fd0;--sh:0 1px 2px rgba(16,24,40,.06)}}
*{box-sizing:border-box}
html{-webkit-text-size-adjust:100%}
body{margin:0;background:var(--bg);color:var(--fg);font:17px/1.45 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;-webkit-tap-highlight-color:transparent}
svg{width:22px;height:22px;fill:none;stroke:currentColor;stroke-width:1.9;stroke-linecap:round;stroke-linejoin:round;flex:none}
a{color:var(--link);text-decoration:none}
.top{position:sticky;top:0;z-index:5;background:var(--bg);background:color-mix(in srgb,var(--bg) 88%,transparent);backdrop-filter:blur(12px);-webkit-backdrop-filter:blur(12px);border-bottom:1px solid var(--line);padding-top:env(safe-area-inset-top)}
.top nav{display:flex;align-items:center;gap:4px;max-width:44rem;margin:0 auto;padding:6px 12px;min-height:60px}
.brand{display:flex;align-items:center;gap:8px;color:var(--fg);font-weight:750;letter-spacing:.01em;padding:0 6px;min-height:48px}
.brand svg{width:28px;height:28px;color:var(--acc)}
.grow{flex:1}
.tab{display:flex;align-items:center;min-height:44px;padding:0 16px;border-radius:999px;color:var(--mut);font-weight:650}
.tab.on{background:var(--card2);color:var(--fg)}
.lock{display:grid;place-items:center;width:44px;height:44px;border-radius:999px;color:var(--warn)}
.lock.open{color:var(--ok)}
main{max-width:44rem;margin:0 auto;padding:16px 16px calc(48px + env(safe-area-inset-bottom))}
h1{font-size:1.6rem;line-height:1.2;margin:0 0 4px;letter-spacing:-.01em;display:flex;align-items:center;gap:12px;flex-wrap:wrap}
h2{font-size:.78rem;text-transform:uppercase;letter-spacing:.09em;color:var(--mut);margin:28px 4px 10px;font-weight:750}
p{margin:0 0 12px}
.mut{color:var(--mut)}
.mono{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:.92em;word-break:break-all}
.card{background:var(--card);border:1px solid var(--line);border-radius:var(--r);box-shadow:var(--sh);overflow:hidden}
.pad{padding:18px}
.pad>:last-child{margin-bottom:0}
.hero{text-align:center;margin-top:24px}
.hero .pinf{text-align:left}
.big-ico svg{width:44px;height:44px;color:var(--warn);margin:4px auto 10px}
.phead{margin:4px 4px 16px}
.back{display:inline-flex;align-items:center;min-height:44px;font-weight:650;margin-left:-4px}
.back::before{content:"";width:9px;height:9px;border-left:2.2px solid currentColor;border-bottom:2.2px solid currentColor;transform:rotate(45deg);margin:0 8px 0 6px}
.btn{display:inline-flex;align-items:center;justify-content:center;gap:10px;min-height:48px;padding:0 18px;border-radius:12px;border:1px solid var(--line);background:var(--card2);color:var(--fg);font:inherit;font-weight:650;cursor:pointer;text-align:center;text-decoration:none}
.btn:active{transform:scale(.985)}
.btn.pri{background:var(--acc);border-color:transparent;color:var(--acc-fg)}
.btn.bad{background:transparent;border-color:var(--bad);color:var(--bad)}
.btn.wide{display:flex;width:100%}
.btn.tall{min-height:64px;font-size:1.12rem;border-radius:16px}
.btn[disabled]{opacity:.45;cursor:not-allowed}
.btn.wide+.btn.wide{margin-top:10px}
.pad>.btn.wide:not(:last-child){margin-bottom:14px}
.tall .fill{fill:currentColor;stroke:none}
.tall svg{width:26px;height:26px}
.hint{color:var(--mut);font-size:.86rem;text-align:center;margin:8px 0 0}
.status{padding:16px 18px;margin-bottom:16px}
.shead{display:flex;align-items:center;justify-content:space-between}
.pill{display:inline-flex;align-items:center;gap:8px;font-weight:750;padding:6px 12px;border-radius:999px;background:var(--card2);text-transform:capitalize}
.pill i{width:9px;height:9px;border-radius:50%;background:currentColor;box-shadow:0 0 0 3px color-mix(in srgb,currentColor 25%,transparent)}
.pill.ok{color:var(--ok)}.pill.idle{color:var(--idle)}.pill.warn{color:var(--warn)}
.status dl,.facts{display:grid;grid-template-columns:1fr 1fr;gap:12px 16px;margin:14px 0 0}
dt{color:var(--mut);font-size:.74rem;text-transform:uppercase;letter-spacing:.07em;font-weight:650}
dd{margin:2px 0 0;font-weight:650;font-variant-numeric:tabular-nums;overflow-wrap:anywhere}
.facts{margin:16px 0 4px}.facts .full{grid-column:1/-1}
.warn-t{color:var(--warn)}
.list .row{display:flex;align-items:center;gap:12px;min-height:64px;padding:10px 12px 10px 16px;border-top:1px solid var(--line);color:inherit}
.list .row:first-child{border-top:0}
a.row:active,.ehead:active{background:var(--card2)}
.main{flex:1;min-width:0;display:flex;flex-direction:column;gap:1px}
.t{font-weight:650;display:flex;align-items:center;gap:8px;min-width:0}
.s{color:var(--mut);font-size:.86rem;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;font-variant-numeric:tabular-nums}
.r{color:var(--mut);font-size:.88rem;font-variant-numeric:tabular-nums;text-align:right;white-space:nowrap;display:flex;flex-direction:column;align-items:flex-end;gap:1px}
.r b{color:var(--fg);font-size:1rem;font-weight:650}
.list .file-row{gap:8px}
.file-row .t{font-size:.88em}
.file-row .r{margin-right:4px}
.chev{width:18px;height:18px;color:var(--mut);opacity:.7}
.tile{display:flex;flex-direction:column;align-items:center;justify-content:center;width:48px;height:48px;border-radius:12px;background:var(--card2);flex:none;line-height:1}
.tile b{font-size:1.15rem}
.tile small{font-size:.64rem;text-transform:uppercase;letter-spacing:.08em;color:var(--acc);font-weight:800;margin-top:3px}
.stack{display:flex;flex-direction:column;gap:12px;margin-top:16px}
.ehead{display:flex;align-items:center;gap:14px;padding:14px 12px 10px 16px;color:inherit}
.num{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-weight:750;font-size:1.3rem;color:var(--fg);letter-spacing:-.03em}
h1 .num{font-size:inherit}
.badge{display:inline-block;font-size:.7rem;font-weight:750;text-transform:uppercase;letter-spacing:.07em;padding:3px 8px;border-radius:6px;background:var(--card2);color:var(--mut);flex:none}
h1 .badge{font-size:.78rem}
.badge.cap{color:var(--cap);background:color-mix(in srgb,var(--cap) 15%,transparent)}
.badge.log{color:var(--log);background:color-mix(in srgb,var(--log) 15%,transparent)}
.trig{color:var(--mut);font-weight:500;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.acts,.row-btns{display:flex;gap:10px;align-items:center}
.acts{padding:0 12px 12px 16px}
.btn.grow{flex:1;min-width:0}
.acts .btn{min-height:44px;font-size:.95rem;color:var(--link)}
.icon{display:grid;place-items:center;width:44px;height:44px;border-radius:12px;color:var(--mut);background:none;border:0;padding:0;cursor:pointer;flex:none}
a.icon,.row .icon{color:var(--link);background:var(--card2)}
.star{border:1px solid var(--line);background:var(--card2)}
.row-btns .star{width:48px;height:48px}
.star.on{color:var(--star);border-color:color-mix(in srgb,var(--star) 45%,transparent);background:color-mix(in srgb,var(--star) 14%,transparent)}
.star.on svg{fill:currentColor}
form{margin:0}
input[type=password]{width:100%;min-height:48px;padding:0 14px;border-radius:12px;border:1px solid var(--line);background:var(--bg);color:var(--fg);font:inherit;font-size:1.1rem;letter-spacing:.2em}
input:focus-visible,.btn:focus-visible,a:focus-visible{outline:2px solid var(--acc);outline-offset:2px}
.pinf{display:flex;gap:10px;margin-top:4px}
.pinf input[type=password]{flex:1;min-width:0}
.up{display:flex;flex-wrap:wrap;gap:10px;align-items:center;margin-top:4px}
.file{flex:1;min-width:0;position:relative;display:flex;align-items:center;min-height:48px;padding:0 14px;border:1px dashed var(--mut);border-radius:12px;color:var(--mut);overflow:hidden;white-space:nowrap}
.file span{overflow:hidden;text-overflow:ellipsis}
.file input{position:absolute;inset:0;opacity:0;cursor:pointer}
.file.has{color:var(--fg);border-style:solid;border-color:var(--line)}
output{flex-basis:100%;font-size:.92rem}
output:empty{display:none}
output.err{color:var(--bad)}output.ok{color:var(--ok)}
.danger{border-color:color-mix(in srgb,var(--bad) 40%,var(--line))}
.danger input[type=password]{margin-bottom:12px}
.check{display:flex;align-items:center;gap:12px;min-height:48px;margin-bottom:10px;font-weight:650}
.check input{width:24px;height:24px;accent-color:var(--bad);flex:none}
.okline{display:flex;align-items:center;gap:10px;color:var(--ok);font-weight:650}
.btns{margin-top:16px}
.gap{height:10px}
.empty{text-align:center}
@media (min-width:600px){.status dl{grid-template-columns:repeat(3,1fr)}}
</style>
"##;

const TAIL: &str = r#"<script>
for (const f of document.querySelectorAll("form.up")) {
  const inp = f.querySelector("input[type=file]"), lab = f.querySelector(".file span");
  inp.addEventListener("change", () => { const x = inp.files[0]; lab.textContent = x ? x.name : "Choose file"; lab.parentNode.classList.toggle("has", !!x); });
  f.addEventListener("submit", async e => {
    e.preventDefault();
    const out = f.querySelector("output"), b = f.querySelector("button");
    b.disabled = true; out.className = ""; out.textContent = "Uploading…";
    try {
      const r = await fetch(f.action, {method: "POST", body: new FormData(f)});
      out.textContent = await r.text(); out.className = r.ok ? "ok" : "err";
      if (r.ok) setTimeout(() => location.reload(), 1500);
    } catch (x) { out.textContent = "Upload failed: " + x; out.className = "err"; }
    b.disabled = false;
  });
}
for (const f of document.querySelectorAll("form[data-confirm]"))
  f.addEventListener("submit", e => { if (!confirm(f.dataset.confirm)) e.preventDefault(); });
for (const f of document.querySelectorAll("form[data-busy]"))
  f.addEventListener("submit", () => { const b = f.querySelector("button"); b.querySelector("span").textContent = f.dataset.busy; setTimeout(() => b.disabled = true); });
</script></body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        assert_eq!(
            esc("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn sizes() {
        assert_eq!(size(10), "10 B");
        assert_eq!(size(1500), "2 KB");
        assert_eq!(size(16 << 20), "16 MB");
        assert_eq!(size(3 << 19), "1.5 MB");
        assert_eq!(uptime(3725), "1h 2m");
    }

    #[test]
    fn dates() {
        assert_eq!(long_date("2026-11-08"), "Sunday 8 Nov 2026");
        assert_eq!(long_date("2000-02-29"), "Tuesday 29 Feb 2000");
        assert_eq!(hm("2026-11-08T10:04:33"), "10:04");
    }
}
