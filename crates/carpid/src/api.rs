//! Local API server: one Unix socket, length-prefixed JSON (see carpi-proto).
//! Same-user only, by socket file permissions (0600).
//!
//! Each connection gets a reader loop, a writer task, and one task per
//! subscription. Requests go to the core loop; subscriptions poll it for
//! readings at their period and send only what changed. A subscription
//! whose prefixes cover diag catalog values also holds diag subscriptions:
//! leased for 3x the period, renewed on every poll, dropped with it.
//! Commands to LIVI, update jobs and diag jobs are per connection too
//! (`commands.rs`, `update.rs`). `Capture` goes to `capture.rs`.

use crate::Msg;
use crate::capture::Captures;
use crate::update::Jobs;
use carpi_proto::{Reading, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
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
pub fn spawn(
    path: &Path,
    tx: mpsc::Sender<Msg>,
    jobs: Arc<Jobs>,
    captures: Captures,
) -> anyhow::Result<()> {
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
                    let jobs = jobs.clone();
                    let captures = captures.clone();
                    tokio::spawn(async move {
                        serve(conn, stream, tx.clone(), jobs, captures).await;
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
async fn serve(
    conn: u64,
    stream: tokio::net::UnixStream,
    tx: mpsc::Sender<Msg>,
    jobs: Arc<Jobs>,
    captures: Captures,
) {
    use carpi_proto::{DiagAction, Request, Response};
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
            Ok(Request::Commands) => {
                jobs.hub.attach(conn, out_tx.clone());
                Response::Ok
            }
            Ok(Request::CommandDone { id, error }) => {
                jobs.hub.done(id, error);
                Response::Ok
            }
            Ok(Request::Update { action, and_apply }) => {
                let (events, rx) = mpsc::unbounded_channel();
                match jobs.start(action, and_apply, events) {
                    Err(message) => Response::Error { message },
                    Ok(job) => {
                        // Started goes out before the first event.
                        if out_tx.send(Response::JobStarted { job }).await.is_err() {
                            break;
                        }
                        forward_job(job, rx, &out_tx);
                        continue;
                    }
                }
            }
            Ok(Request::Diag { action }) => {
                let job = jobs.next_job();
                let (events, rx) = mpsc::unbounded_channel();
                let (rtx, rrx) = oneshot::channel();
                let msg = match action {
                    DiagAction::Status => Msg::DiagStatus(rtx, events),
                    action => Msg::DiagJob(action, events, rtx),
                };
                if tx.send(msg).await.is_err() {
                    break;
                }
                match rrx.await {
                    Err(_) => break,
                    Ok(Err(message)) => Response::Error { message },
                    Ok(Ok(())) => {
                        if out_tx.send(Response::JobStarted { job }).await.is_err() {
                            break;
                        }
                        forward_job(job, rx, &out_tx);
                        continue;
                    }
                }
            }
            Ok(Request::Capture { trigger }) => captures.capture(&trigger).await,
            Ok(Request::Settings { action }) => {
                let (rtx, rrx) = oneshot::channel();
                if tx.send(Msg::Settings(action, rtx)).await.is_err() {
                    break;
                }
                let Ok(resp) = rrx.await else { break };
                resp
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
    jobs.hub.detach(conn);
    drop(out_tx);
    let _ = writer.await;
}

/// Relays a job's events to the connection until the job ends. Weak: a
/// closed connection mustn't wait for the job.
#[cfg(unix)]
fn forward_job(
    job: u32,
    mut rx: mpsc::UnboundedReceiver<carpi_proto::JobEvent>,
    out: &mpsc::Sender<carpi_proto::Response>,
) {
    let out = out.downgrade();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Some(out) = out.upgrade() {
                let _ = out.send(carpi_proto::Response::Job { job, event }).await;
            }
        }
    });
}

/// Drops a subscription's diag subscriptions when its task ends or is
/// aborted. If the core queue is full the leases run out by themselves.
#[cfg(unix)]
struct DiagSubs {
    ids: Vec<carpi_core::DiagSubId>,
    tx: mpsc::Sender<Msg>,
}

#[cfg(unix)]
impl Drop for DiagSubs {
    fn drop(&mut self) {
        if !self.ids.is_empty() {
            let _ = self
                .tx
                .try_send(Msg::DiagUnsubscribe(std::mem::take(&mut self.ids)));
        }
    }
}

#[cfg(unix)]
async fn run_sub(
    sub: u32,
    prefixes: Vec<String>,
    period: std::time::Duration,
    tx: mpsc::Sender<Msg>,
    out: mpsc::Sender<carpi_proto::Response>,
) {
    let (rtx, rrx) = tokio::sync::oneshot::channel();
    if tx
        .send(Msg::DiagSubscribe(prefixes.clone(), period, rtx))
        .await
        .is_err()
    {
        return;
    }
    let Ok(ids) = rrx.await else { return };
    let diag = DiagSubs {
        ids,
        tx: tx.clone(),
    };
    let mut seen = HashMap::new();
    let mut tick = tokio::time::interval(period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut first = true;
    loop {
        tick.tick().await;
        let (rtx, rrx) = tokio::sync::oneshot::channel();
        let msg = Msg::Readings {
            prefixes: prefixes.clone(),
            renew: diag.ids.clone(),
            period,
            reply: rtx,
        };
        if tx.send(msg).await.is_err() {
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
pub fn spawn(
    path: &Path,
    _tx: mpsc::Sender<Msg>,
    _jobs: Arc<Jobs>,
    _captures: Captures,
) -> anyhow::Result<()> {
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
