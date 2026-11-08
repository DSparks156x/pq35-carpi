//! carpid: the car daemon.
//!
//! Step 1 of the plan: read-only. It opens every configured interface,
//! decodes ICAN into the value store, watches the NM ring, keeps a rolling
//! capture, and answers the local API. It sends nothing on any bus.

mod api;
mod config;

use anyhow::Context;
use carpi_can::recorder::Recorder;
use carpi_can::{Port, ReplayPort, RxFrame};
use carpi_core::Car;
use clap::Parser;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

#[derive(Parser)]
#[command(version, about = "pq35-carpi car daemon (read-only for now)")]
struct Args {
    /// Config file. Without one, ICAN and diag are both on can0.
    #[arg(long, default_value = "/etc/carpi/carpid.toml")]
    config: PathBuf,
    /// Play a candump file into the ICAN interface instead of opening sockets.
    #[arg(long)]
    replay: Option<PathBuf>,
    /// With --replay: play at recorded speed rather than as fast as possible.
    #[arg(long)]
    realtime: bool,
}

pub enum Msg {
    Frame(usize, RxFrame),
    PortEnded(usize, Option<std::io::Error>),
    /// A request from API connection `conn`.
    Api(
        u64,
        carpi_proto::Request,
        oneshot::Sender<carpi_proto::Response>,
    ),
    /// Current readings under these prefixes, for a subscription.
    Readings(Vec<String>, oneshot::Sender<Vec<carpi_proto::Reading>>),
    ConnClosed(u64),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();

    let cfg = if args.config.exists() {
        config::Config::load(&args.config)
            .with_context(|| format!("loading {}", args.config.display()))?
    } else {
        warn!(
            "{} not found; using can0 for ican and diag",
            args.config.display()
        );
        config::Config::fallback()
    };
    let ifaces = cfg.ifaces();
    let recorder = Recorder::new(Duration::from_secs(cfg.recorder.minutes * 60), 2_000_000);
    let mut car = Car::new(ifaces.clone(), recorder, cfg.recorder.dir.clone());

    let (tx, mut rx) = mpsc::channel::<Msg>(4096);

    for (idx, spec) in ifaces.iter().enumerate() {
        let port = match &args.replay {
            Some(path) if spec.has_role(carpi_core::ROLE_ICAN) => {
                let r = ReplayPort::load(path, None, Duration::from_micros(3500), args.realtime)
                    .with_context(|| format!("loading {}", path.display()))?;
                info!(
                    "replaying {} frames from {} as {}",
                    r.len(),
                    path.display(),
                    spec.name
                );
                Port::Replay(r)
            }
            Some(_) => continue,
            None => Port::open(&spec.name).with_context(|| format!("opening {}", spec.name))?,
        };
        info!("listening on {} ({})", spec.name, spec.roles.join(","));
        tokio::spawn(read_port(idx, spec.name.clone(), port, tx.clone()));
    }

    api::spawn(&cfg.api.socket, tx.clone())?;

    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some(msg) = msg else { break };
                match msg {
                    Msg::Frame(idx, frame) => {
                        if let Some(ev) = car.ingest(idx, &frame) {
                            info!("{}: bus {} -> {}", ev.iface, ev.transition.from, ev.transition.to);
                        }
                    }
                    Msg::PortEnded(idx, err) => {
                        let name = &ifaces[idx].name;
                        match err {
                            Some(e) => warn!("{name}: read failed: {e}"),
                            None => info!("{name}: replay finished"),
                        }
                    }
                    Msg::Api(conn, req, reply) => {
                        let _ = reply.send(car.handle(conn, req, Instant::now()));
                    }
                    Msg::Readings(prefixes, reply) => {
                        let _ = reply.send(car.readings(&prefixes, Instant::now()));
                    }
                    Msg::ConnClosed(conn) => car.conn_closed(conn, Instant::now()),
                }
            }
            _ = tick.tick() => {
                for ev in car.tick(Instant::now()) {
                    info!("{}: bus {} -> {}", ev.iface, ev.transition.from, ev.transition.to);
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("stopping");
                break;
            }
        }
    }
    api::cleanup(&cfg.api.socket);
    Ok(())
}

/// Reads one interface forever. On a read error it waits and reopens the
/// interface by name, so a bus-off or a link bounce doesn't kill the daemon.
/// (Replay ports never error.)
async fn read_port(idx: usize, name: String, mut port: Port, tx: mpsc::Sender<Msg>) {
    loop {
        match port.recv().await {
            Ok(Some(frame)) => {
                if tx.send(Msg::Frame(idx, frame)).await.is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = tx.send(Msg::PortEnded(idx, None)).await;
                return;
            }
            Err(e) => {
                let _ = tx.send(Msg::PortEnded(idx, Some(e))).await;
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if let Ok(p) = Port::open(&name) {
                        port = p;
                        break;
                    }
                }
            }
        }
    }
}
