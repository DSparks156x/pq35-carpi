//! The one HTML page. Server-rendered, inline CSS, a few lines of JS so
//! uploads report back without leaving the page. Works without JS too.

use crate::files::{self, Entry};
use crate::{PortalConfig, Status};
use std::fmt::Write;

pub fn size(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{} KB", n.div_ceil(1 << 10)),
        n => format!("{n} B"),
    }
}

fn esc(s: &str) -> String {
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

fn file_list(o: &mut String, kind: &str, entries: &[Entry]) {
    if entries.is_empty() {
        o.push_str("<p class=dim>None.</p>");
        return;
    }
    o.push_str("<ul>");
    for e in entries.iter().take(200) {
        let n = esc(&e.name);
        let _ = write!(
            o,
            "<li><a href=\"/dl/{kind}/{n}\" download>{n}</a> <span class=dim>{} · {}</span></li>",
            size(e.size),
            files::short_time(e.modified)
        );
    }
    o.push_str("</ul>");
    if entries.len() > 200 {
        let _ = write!(
            o,
            "<p class=dim>{} older files not shown.</p>",
            entries.len() - 200
        );
    }
}

fn upload_form(o: &mut String, action: &str, accept: &str, label: &str) {
    let _ = write!(
        o,
        "<form class=up method=post action=\"{action}\" enctype=\"multipart/form-data\">\
         <input type=file name=file accept=\"{accept}\" required>\
         <button>{label}</button><output></output></form>"
    );
}

pub fn render(
    cfg: &PortalConfig,
    status: Option<&Status>,
    can_read: bool,
    can_config: bool,
) -> String {
    let mut o = String::from(HEAD);
    o.push_str("<h1>Car files</h1>");

    if cfg.pin.as_deref().is_some_and(|p| !p.is_empty()) {
        let what = if cfg.protect_downloads {
            "everything"
        } else {
            "uploads"
        };
        let _ = write!(
            o,
            "<form class=pin method=post action=\"/pin\">\
             <input name=pin type=password inputmode=numeric autocomplete=off placeholder=PIN>\
             <button>Use PIN</button></form><p class=dim>PIN needed for {what}.</p>"
        );
    }
    if !can_read {
        o.push_str("<p>Enter the PIN to see files.</p>");
        o.push_str(TAIL);
        return o;
    }

    o.push_str("<h2>Status</h2>");
    match status {
        Some(st) => {
            o.push_str("<table>");
            for (k, v) in status_rows(st) {
                let _ = write!(o, "<tr><th>{}</th><td>{}</td></tr>", esc(&k), esc(&v));
            }
            o.push_str("</table>");
        }
        None => o.push_str("<p class=dim>Not available.</p>"),
    }

    o.push_str(
        "<h2>Bug report</h2><p><a class=btn href=\"/dl/bugreport\" download>Download bug report</a></p>\
         <p class=dim>Status, carpid journal, config, captures and sdlink logs in one .tar.gz.</p>",
    );
    if !cfg.journal.is_empty() {
        o.push_str("<p><a href=\"/dl/journal\" download>carpid journal</a></p>");
    }

    o.push_str("<h2>Captures</h2>");
    file_list(&mut o, "captures", &files::list(&cfg.captures));
    o.push_str("<h2>sdlink logs</h2>");
    file_list(&mut o, "sdlink", &files::list(&cfg.sdlink_logs));

    o.push_str("<h2>Config</h2>");
    if cfg.config_file.is_file() {
        o.push_str("<p><a href=\"/dl/config\" download>carpid.toml</a></p>");
    }
    if can_config {
        o.push_str(
            "<p class=dim>Replaces carpid.toml if it passes carpid's checks. The old one is kept.</p>",
        );
        upload_form(&mut o, "/up/config", ".toml", "Upload carpid.toml");
    } else {
        o.push_str("<p class=dim>Upload not available.</p>");
    }

    o.push_str("<h2>Firmware</h2>");
    let _ = write!(
        o,
        "<p class=dim>{} up to {}. Stored only; nothing is flashed.</p>",
        esc(&cfg.firmware_ext.join(", ")),
        size(cfg.firmware_max)
    );
    upload_form(
        &mut o,
        "/up/firmware",
        &esc(&cfg.firmware_ext.join(",")),
        "Upload firmware",
    );
    file_list(&mut o, "firmware", &files::list(&cfg.firmware));

    o.push_str(TAIL);
    o
}

const HEAD: &str = r#"<!doctype html><html lang=en><head><meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1">
<title>Car files</title><style>
:root{color-scheme:light dark;--bg:#fff;--fg:#111;--dim:#666;--line:#ddd;--acc:#0a58ca}
@media(prefers-color-scheme:dark){:root{--bg:#111;--fg:#eee;--dim:#999;--line:#333;--acc:#6ea8fe}}
body{margin:0 auto;max-width:40rem;padding:0 16px 3rem;background:var(--bg);color:var(--fg);font:16px/1.45 system-ui,sans-serif}
h1{font-size:1.4rem}h2{font-size:1.1rem;margin-top:2rem;border-bottom:1px solid var(--line)}
a{color:var(--acc)}ul{padding-left:1.2rem}li{margin:.4rem 0;word-break:break-all}
.dim{color:var(--dim);font-size:.9rem}table{border-collapse:collapse}
th,td{text-align:left;padding:.2rem 1rem .2rem 0}th{font-weight:500;color:var(--dim)}
form{display:flex;flex-wrap:wrap;gap:.5rem;align-items:center;margin:.5rem 0}
input,button,.btn{font:inherit;padding:.5rem .8rem}button,.btn{border:1px solid var(--acc);border-radius:6px;background:none;color:var(--acc);text-decoration:none}
output{flex-basis:100%}output.err{color:#d33}
</style></head><body>
"#;

const TAIL: &str = r#"<script>
for (const f of document.querySelectorAll("form.up")) f.addEventListener("submit", async e => {
  e.preventDefault();
  const out = f.querySelector("output"), b = f.querySelector("button");
  b.disabled = true; out.className = ""; out.textContent = "Uploading…";
  try {
    const r = await fetch(f.action, {method: "POST", body: new FormData(f)});
    out.textContent = await r.text(); out.className = r.ok ? "" : "err";
    if (r.ok) setTimeout(() => location.reload(), 1500);
  } catch (x) { out.textContent = "Upload failed: " + x; out.className = "err"; }
  b.disabled = false;
});
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
        assert_eq!(size(16 << 20), "16.0 MB");
        assert_eq!(uptime(3725), "1h 2m");
    }
}
