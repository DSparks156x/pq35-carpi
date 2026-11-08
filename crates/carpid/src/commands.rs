//! carpid -> LIVI commands over the bridge connection.
//!
//! LIVI's connection sends `Commands` once to become the target. A command
//! goes out as an unsolicited `Response::Command { id }`; LIVI answers
//! `Request::CommandDone { id, error }`. When LIVI's connection closes, its
//! pending commands fail at once.

#![cfg_attr(not(unix), allow(dead_code))]

use carpi_proto::{Command, Response};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

type Reply = oneshot::Sender<Result<(), String>>;

#[derive(Default)]
pub struct CommandHub {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    target: Option<(u64, mpsc::Sender<Response>)>,
    next: u32,
    pending: HashMap<u32, (u64, Reply)>,
}

impl CommandHub {
    /// Connection `conn` takes commands from now on.
    pub fn attach(&self, conn: u64, out: mpsc::Sender<Response>) {
        self.inner.lock().unwrap().target = Some((conn, out));
    }

    /// Connection `conn` closed.
    pub fn detach(&self, conn: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.target.as_ref().is_some_and(|(c, _)| *c == conn) {
            inner.target = None;
        }
        // Dropping the reply senders fails the waiters.
        inner.pending.retain(|_, (c, _)| *c != conn);
    }

    /// `CommandDone` from LIVI.
    pub fn done(&self, id: u32, error: Option<String>) {
        if let Some((_, reply)) = self.inner.lock().unwrap().pending.remove(&id) {
            let _ = reply.send(error.map_or(Ok(()), Err));
        }
    }

    /// Sends `command` and waits up to `timeout` for LIVI's answer.
    pub async fn send(&self, command: Command, timeout: Duration) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        let (id, out) = {
            let mut inner = self.inner.lock().unwrap();
            let Some((conn, out)) = inner.target.clone() else {
                return Err("LIVI isn't connected".into());
            };
            inner.next = inner.next.wrapping_add(1);
            let id = inner.next;
            inner.pending.insert(id, (conn, tx));
            (id, out)
        };
        if out.send(Response::Command { id, command }).await.is_err() {
            self.inner.lock().unwrap().pending.remove(&id);
            return Err("LIVI's connection closed".into());
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("LIVI's connection closed".into()),
            Err(_) => {
                self.inner.lock().unwrap().pending.remove(&id);
                Err(format!("LIVI didn't answer in {} s", timeout.as_secs()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carpi_proto::ApAction;
    use std::sync::Arc;

    const SUSPEND: Command = Command::Ap {
        ap: ApAction::Suspend,
    };

    #[tokio::test]
    async fn command_round_trip_and_failures() {
        let hub = Arc::new(CommandHub::default());
        let t = Duration::from_secs(5);
        assert!(
            hub.send(SUSPEND, t)
                .await
                .unwrap_err()
                .contains("isn't connected")
        );

        let (out, mut livi) = mpsc::channel(4);
        hub.attach(7, out);
        let h = hub.clone();
        let sent = tokio::spawn(async move { h.send(SUSPEND, t).await });
        let Some(Response::Command { id, command }) = livi.recv().await else {
            panic!("no command");
        };
        assert_eq!(command, SUSPEND);
        hub.done(id, None);
        assert_eq!(sent.await.unwrap(), Ok(()));

        let h = hub.clone();
        let sent = tokio::spawn(async move { h.send(SUSPEND, t).await });
        let Some(Response::Command { id, .. }) = livi.recv().await else {
            panic!("no command");
        };
        hub.done(id, Some("sudo refused".into()));
        assert_eq!(sent.await.unwrap(), Err("sudo refused".into()));

        // LIVI goes away mid-command.
        let h = hub.clone();
        let sent = tokio::spawn(async move { h.send(SUSPEND, t).await });
        livi.recv().await.unwrap();
        hub.detach(7);
        assert!(sent.await.unwrap().unwrap_err().contains("closed"));
        assert!(hub.send(SUSPEND, t).await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn silent_livi_times_out() {
        let hub = CommandHub::default();
        let (out, _livi) = mpsc::channel(4);
        hub.attach(1, out);
        let r = hub.send(SUSPEND, Duration::from_secs(20)).await;
        assert!(r.unwrap_err().contains("didn't answer"));
    }
}
