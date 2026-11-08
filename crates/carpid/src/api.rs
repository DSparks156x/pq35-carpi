//! Local API server: one Unix socket, length-prefixed JSON (see carpi-proto).
//! Same-user only, by socket file permissions (0600).

use crate::Msg;
use std::path::Path;
use tokio::sync::mpsc;

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
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(serve(stream, tx.clone()));
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
async fn serve(mut stream: tokio::net::UnixStream, tx: mpsc::Sender<Msg>) {
    use carpi_proto::{Request, Response};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;

    let mut greeted = false;
    loop {
        let mut prefix = [0u8; 4];
        if stream.read_exact(&mut prefix).await.is_err() {
            return;
        }
        let Ok(n) = carpi_proto::body_len(prefix) else {
            return;
        };
        let mut body = vec![0u8; n];
        if stream.read_exact(&mut body).await.is_err() {
            return;
        }
        let resp = match carpi_proto::decode::<Request>(&body) {
            Err(e) => Response::Error {
                message: format!("bad request: {e}"),
            },
            Ok(req) => {
                let is_hello = matches!(req, Request::Hello { .. });
                if !greeted && !is_hello {
                    Response::Error {
                        message: "send hello first".into(),
                    }
                } else {
                    let (rtx, rrx) = oneshot::channel();
                    if tx.send(Msg::Api(req, rtx)).await.is_err() {
                        return;
                    }
                    let Ok(resp) = rrx.await else { return };
                    if is_hello {
                        greeted = matches!(resp, Response::Hello { .. });
                    }
                    resp
                }
            }
        };
        if stream.write_all(&carpi_proto::encode(&resp)).await.is_err() {
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
