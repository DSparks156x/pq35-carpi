//! Saved Wi-Fi networks: list, add, forget, through the hook carpid sets. No
//! scan or test here: both drop the hotspot this phone is on.
//!
//! Passwords go from the form to the hook and nowhere else. The form structs
//! have no `Debug`, nothing logs them, and a refused form comes back with the
//! name kept and the password empty.

use crate::page::{self, Access, Tab, esc};
use crate::views::Viewer;
use crate::{Shared, WifiNet, WifiOp};
use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::fmt::Write;
use std::sync::Arc;

/// A name and password as typed into the form; `Ok(None)`: open network.
pub fn check(ssid: &str, password: &str) -> Result<Option<String>, String> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("A network name is 1-32 characters.".into());
    }
    if ssid.chars().any(char::is_control) {
        return Err("The network name has a control character in it.".into());
    }
    if password.is_empty() {
        return Ok(None);
    }
    if !(8..=63).contains(&password.chars().count()) {
        return Err("A Wi-Fi password is 8-63 characters, or empty for an open network.".into());
    }
    Ok(Some(password.to_owned()))
}

/// `ssid` for a query string.
fn pct(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            let _ = write!(o, "%{b:02X}");
        }
    }
    o
}

fn pin_input(o: &mut String, access: Access) {
    if !access.can_write() {
        o.push_str("<input class=pin name=pin type=password inputmode=numeric autocomplete=current-password placeholder=PIN aria-label=PIN required>");
    }
}

/// The page: saved networks, the add form, where scan and test live.
fn render(access: Access, nets: &[WifiNet], error: Option<&str>, ssid: &str) -> String {
    let mut b = String::from(
        "<div class=phead><a class=back href=\"/setup\">Setup</a><h1>Wi-Fi networks</h1>\
         <p class=mut>Networks the car joins for updates, when the hotspot steps aside.</p></div>",
    );
    if let Some(e) = error {
        let _ = write!(b, "<p class=errline>{}</p>", esc(e));
    }
    b.push_str("<h2>Saved</h2><section class=\"card list\">");
    if nets.is_empty() {
        b.push_str("<div class=row><span class=\"main mut\">No saved networks.</span></div>");
    }
    for n in nets {
        let mut what = vec![if n.secured { "Password" } else { "Open" }];
        if n.hidden {
            what.push("hidden");
        }
        let _ = write!(
            b,
            "<div class=row><span class=main><span class=t>{name}{on}</span><span class=s>{what}</span></span>\
             <a class=btn href=\"/wifi/forget?ssid={q}\">Forget</a></div>",
            name = esc(&n.ssid),
            on = if n.connected {
                " <span class=\"badge on\">connected</span>"
            } else {
                ""
            },
            what = what.join(" · "),
            q = pct(&n.ssid),
        );
    }
    b.push_str("</section>");

    let _ = write!(
        b,
        "<h2>Add a network</h2><form class=\"card pad wf\" method=post action=\"/wifi/add\" data-busy=\"Saving…\">\
         <label class=f>Name<input name=ssid type=text maxlength=32 required autocomplete=off \
         autocapitalize=off autocorrect=off spellcheck=false value=\"{}\"></label>\
         <label class=f>Password<input name=password type=password maxlength=63 autocomplete=new-password \
         placeholder=\"Empty for an open network\"></label>\
         <label class=check><input type=checkbox name=hidden value=yes><span>Hidden (not broadcast)</span></label>",
        esc(ssid)
    );
    pin_input(&mut b, access);
    b.push_str(
        "<button class=\"btn pri wide\"><span>Save network</span></button>\
         <p class=hint>Saved only; the car joins it during an update. Same name replaces the saved one.</p></form>\
         <p class=hint>Scan and Test drop the hotspot this phone is on, so they're on the head unit: \
         Car Integration &gt; Updates &gt; Wi-Fi networks.</p>",
    );
    page::layout("Wi-Fi networks", Tab::None, access, &b)
}

fn confirm_page(access: Access, ssid: &str) -> String {
    let n = esc(ssid);
    let mut b = format!(
        "<div class=phead><a class=back href=\"/wifi\">Wi-Fi networks</a></div>\
         <form class=\"card pad danger\" method=post action=\"/wifi/forget\">\
         <h1>Forget {n}?</h1>\
         <p class=mut>The car stops joining it for updates. Add it again to undo.</p>\
         <input type=hidden name=ssid value=\"{n}\"><input type=hidden name=confirm value=yes>"
    );
    pin_input(&mut b, access);
    let _ = write!(
        b,
        "<button class=\"btn bad wide\">Forget {n}</button></form>"
    );
    page::layout("Forget network", Tab::None, access, &b)
}

/// The list, or the error line to show instead.
async fn list(s: &Shared) -> (Vec<WifiNet>, Option<String>) {
    let hook = s.wifi.clone().expect("checked by the caller");
    match hook(WifiOp::List).await {
        Ok(n) => (n, None),
        Err(e) => (
            Vec::new(),
            Some(format!("Couldn't read the saved networks: {e}")),
        ),
    }
}

pub async fn page(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    let v = Viewer::new(&s, &headers);
    if s.wifi.is_none() {
        return v.not_found();
    }
    if !v.can_read {
        return v.locked("/wifi");
    }
    let (nets, err) = list(&s).await;
    v.html(StatusCode::OK, render(v.access, &nets, err.as_deref(), ""))
}

#[derive(Deserialize)]
pub struct AddForm {
    ssid: Option<String>,
    password: Option<String>,
    hidden: Option<String>,
    pin: Option<String>,
}

pub async fn add(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    Form(f): Form<AddForm>,
) -> Response {
    let Some(hook) = s.wifi.clone() else {
        return Viewer::quiet(&s).not_found();
    };
    if let Err(r) = s.check_pin_or_form(&headers, f.pin.as_deref()) {
        return Viewer::quiet(&s).refused(r, "/wifi");
    }
    let v = Viewer::checked(&s, &headers);
    let ssid = f.ssid.unwrap_or_default();
    let hidden = f.hidden.as_deref() == Some("yes");
    let password = match check(&ssid, f.password.as_deref().unwrap_or("")) {
        Ok(p) => p,
        Err(e) => {
            let (nets, _) = list(&s).await;
            return v.html(
                StatusCode::BAD_REQUEST,
                render(v.access, &nets, Some(&e), &ssid),
            );
        }
    };
    let secured = password.is_some();
    let op = WifiOp::Add {
        ssid: ssid.clone(),
        password,
        hidden,
    };
    match hook(op).await {
        Ok(_) => {
            tracing::info!(%ssid, secured, hidden, "portal: wifi network saved");
            Redirect::to("/wifi").into_response()
        }
        Err(e) => {
            let (nets, _) = list(&s).await;
            let e = format!("Not saved: {e}");
            v.html(
                StatusCode::INTERNAL_SERVER_ERROR,
                render(v.access, &nets, Some(&e), &ssid),
            )
        }
    }
}

#[derive(Deserialize)]
pub struct ForgetForm {
    ssid: Option<String>,
    confirm: Option<String>,
    pin: Option<String>,
}

/// GET: the confirm step, for a saved network only.
pub async fn confirm(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    Form(f): Form<ForgetForm>,
) -> Response {
    let v = Viewer::new(&s, &headers);
    if s.wifi.is_none() {
        return v.not_found();
    }
    if !v.can_read {
        return v.locked("/wifi");
    }
    let ssid = f.ssid.unwrap_or_default();
    let (nets, err) = list(&s).await;
    if let Some(e) = err {
        return v.message(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Couldn't forget",
            &e,
            "/wifi",
        );
    }
    if !nets.iter().any(|n| n.ssid == ssid) {
        return v.message(
            StatusCode::NOT_FOUND,
            "Not saved",
            "That network isn't saved.",
            "/wifi",
        );
    }
    v.html(StatusCode::OK, confirm_page(v.access, &ssid))
}

pub async fn forget(
    State(s): State<Arc<Shared>>,
    headers: HeaderMap,
    Form(f): Form<ForgetForm>,
) -> Response {
    let v = Viewer::quiet(&s);
    let Some(hook) = s.wifi.clone() else {
        return v.not_found();
    };
    if let Err(r) = s.check_pin_or_form(&headers, f.pin.as_deref()) {
        return v.refused(r, "/wifi");
    }
    let ssid = f.ssid.unwrap_or_default();
    if f.confirm.as_deref() != Some("yes") {
        return v.message(
            StatusCode::BAD_REQUEST,
            "Not forgotten",
            "Confirm on the Forget page first.",
            "/wifi",
        );
    }
    if let Err(e) = check(&ssid, "") {
        return v.message(StatusCode::BAD_REQUEST, "Not forgotten", &e, "/wifi");
    }
    match hook(WifiOp::Forget { ssid: ssid.clone() }).await {
        Ok(_) => {
            tracing::info!(%ssid, "portal: wifi network forgotten");
            Redirect::to("/wifi").into_response()
        }
        Err(e) => v.message(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Couldn't forget",
            &e,
            "/wifi",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks() {
        assert_eq!(check("Home", ""), Ok(None));
        assert_eq!(check("Home", "12345678"), Ok(Some("12345678".into())));
        assert!(check("", "").is_err());
        assert!(check(&"x".repeat(33), "").is_err());
        assert!(check(&"x".repeat(32), "").is_ok());
        assert!(check("a\nb", "").is_err());
        assert!(check("Home", "1234567").is_err());
        assert!(check("Home", &"p".repeat(64)).is_err());
        assert!(check("Home", &"p".repeat(63)).is_ok());
    }

    #[test]
    fn query_escape() {
        assert_eq!(pct("Home 5G&x"), "Home%205G%26x");
        assert_eq!(pct("é"), "%C3%A9");
    }

    #[test]
    fn debug_hides_the_password() {
        let op = WifiOp::Add {
            ssid: "Home".into(),
            password: Some("hunter2hunter2".into()),
            hidden: false,
        };
        let d = format!("{op:?}");
        assert!(d.contains("Home") && !d.contains("hunter2"), "{d}");
    }
}
