//! Local API server: one Unix socket, length-prefixed JSON (see carpi-proto).
//! Same-user only, by socket file permissions (0600).
//!
//! Each connection gets a reader loop, a writer task, and one task per
//! subscription. Requests go to the core loop; subscriptions poll it for
//! readings at their period and send only what changed.

use crate::Msg;
use carpi_proto::{Reading, Value};
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::mpsc;

/// Readings that differ from what this subscriber last saw. Age alone
/// doesn't count as a change.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn diff(seen: &mut HashMap<String, Option<Value>>, items: Vec<Reading>) -> Vec<Reading> {
    items
        .into_iter()
        .filter(|r| {
            let changed = seen.get(&r.name) != Some(&r.value);
            if changed {
                seen.insert(r.name.clone(), r.value.clone());
            }
            changed
        })
        .collect()
}

#[cfg(unix)]
pub fn spawn(path: &Path, tx: mpsc::Sender<Msg>) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("removing stale {}", path.display()))?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!("API on {}", path.display());

    tokio::spawn(async move {
        let mut next_conn = 1u64;
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let conn = next_conn;
                    next_conn += 1;
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        serve(conn, stream, tx.clone()).await;
                        let _ = tx.send(Msg::ConnClosed(conn)).await;
                    });
                }
                Err(e) => {
                    tracing::warn!("API accept failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
    });
    Ok(())
}

#[cfg(unix)]
async fn serve(conn: u64, stream: tokio::net::UnixStream, tx: mpsc::Sender<Msg>) {
    use carpi_proto::{Request, Response};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;

    let (mut rd, mut wr) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::channel::<Response>(64);
    let writer = tokio::spawn(async move {
        while let Some(resp) = out_rx.recv().await {
            if wr.write_all(&carpi_proto::encode(&resp)).await.is_err() {
                return;
            }
        }
    });

    let mut subs: HashMap<u32, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut greeted = false;
    loop {
        let mut prefix = [0u8; 4];
        if rd.read_exact(&mut prefix).await.is_err() {
            break;
        }
        let Ok(n) = carpi_proto::body_len(prefix) else {
            break;
        };
        let mut body = vec![0u8; n];
        if rd.read_exact(&mut body).await.is_err() {
            break;
        }
        let resp = match carpi_proto::decode::<Request>(&body) {
            Err(e) => Response::Error {
                message: format!("bad request: {e}"),
            },
            Ok(req) if !greeted && !matches!(req, Request::Hello { .. }) => Response::Error {
                message: "send hello first".into(),
            },
            Ok(Request::Subscribe {
                sub,
                prefixes,
                period_ms,
            }) => {
                if let Some(old) = subs.remove(&sub) {
                    old.abort();
                }
                let period = std::time::Duration::from_millis(period_ms.max(20) as u64);
                subs.insert(
                    sub,
                    tokio::spawn(run_sub(sub, prefixes, period, tx.clone(), out_tx.clone())),
                );
                Response::Ok
            }
            Ok(Request::Unsubscribe { sub }) => {
                if let Some(old) = subs.remove(&sub) {
                    old.abort();
                }
                Response::Ok
            }
            Ok(req) => {
                let is_hello = matches!(req, Request::Hello { .. });
                let (rtx, rrx) = oneshot::channel();
                if tx.send(Msg::Api(conn, req, rtx)).await.is_err() {
                    break;
                }
                let Ok(resp) = rrx.await else { break };
                if is_hello {
                    greeted = matches!(resp, Response::Hello { .. });
                }
                resp
            }
        };
        if out_tx.send(resp).await.is_err() {
            break;
        }
    }
    for (_, h) in subs {
        h.abort();
    }
    drop(out_tx);
    let _ = writer.await;
}

#[cfg(unix)]
async fn run_sub(
    sub: u32,
    prefixes: Vec<String>,
    period: std::time::Duration,
    tx: mpsc::Sender<Msg>,
    out: mpsc::Sender<carpi_proto::Response>,
) {
    let mut seen = HashMap::new();
    let mut tick = tokio::time::interval(period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut first = true;
    loop {
        tick.tick().await;
        let (rtx, rrx) = tokio::sync::oneshot::channel();
        if tx.send(Msg::Readings(prefixes.clone(), rtx)).await.is_err() {
            return;
        }
        let Ok(items) = rrx.await else { return };
        let items = diff(&mut seen, items);
        if items.is_empty() && !first {
            continue;
        }
        first = false;
        if out
            .send(carpi_proto::Response::Update { sub, items })
            .await
            .is_err()
        {
            return;
        }
    }
}

#[cfg(unix)]
pub fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(not(unix))]
pub fn spawn(path: &Path, _tx: mpsc::Sender<Msg>) -> anyhow::Result<()> {
    tracing::warn!(
        "no Unix sockets on this platform; API at {} disabled",
        path.display()
    );
    Ok(())
}

#[cfg(not(unix))]
pub fn cleanup(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, v: Option<f64>, age: u64) -> Reading {
        Reading {
            name: name.into(),
            value: v.map(Value::Num),
            age_ms: age,
        }
    }

    #[test]
    fn diff_sends_changes_only() {
        let mut seen = HashMap::new();
        assert_eq!(
            diff(&mut seen, vec![r("a", Some(1.0), 0), r("b", None, 0)]).len(),
            2
        );
        // Same values, older: nothing.
        assert!(diff(&mut seen, vec![r("a", Some(1.0), 50), r("b", None, 50)]).is_empty());
        // a changes, b becomes valid.
        let d = diff(&mut seen, vec![r("a", Some(2.0), 0), r("b", Some(5.0), 0)]);
        assert_eq!(d.len(), 2);
        // a times out.
        let d = diff(&mut seen, vec![r("a", None, 3000), r("b", Some(5.0), 10)]);
        assert_eq!(d, vec![r("a", None, 3000)]);
    }
}
