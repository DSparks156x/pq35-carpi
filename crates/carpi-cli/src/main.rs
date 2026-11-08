//! `carpi`: talk to carpid, or run a candump file through the car model
//! offline.

use anyhow::{Context, bail};
use carpi_can::ReplayPort;
use carpi_can::recorder::Recorder;
use carpi_core::{Car, IfaceSpec};
use carpi_proto::{Request, Response};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(version, about = "pq35-carpi debug tool")]
struct Args {
    /// carpid socket (default: $XDG_RUNTIME_DIR/carpid.sock).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Decode a candump file offline and print bus events and final values.
    Replay {
        file: PathBuf,
        /// Only print values starting with this.
        #[arg(long, default_value = "")]
        prefix: String,
        /// Gap between frames for files without timestamps.
        #[arg(long, default_value_t = 3.5)]
        gap_ms: f64,
    },
    /// Live values from carpid.
    Values {
        #[arg(default_value = "")]
        prefix: String,
    },
    /// Bus and NM ring state from carpid.
    Bus,
    /// Save carpid's rolling capture (candump log format).
    Capture { path: Option<String> },
    /// Stream changing values from carpid until Ctrl-C.
    Watch {
        #[arg(default_value = "")]
        prefix: Vec<String>,
        #[arg(long, default_value_t = 200)]
        period_ms: u32,
    },
    /// Run an update job in carpid and follow it: check, fetch, apply, status.
    Update {
        #[arg(value_enum)]
        action: Action,
        /// With fetch: apply once verified.
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Action {
    Check,
    Fetch,
    Apply,
    Status,
}

impl From<Action> for carpi_proto::UpdateAction {
    fn from(a: Action) -> Self {
        match a {
            Action::Check => Self::Check,
            Action::Fetch => Self::Fetch,
            Action::Apply => Self::Apply,
            Action::Status => Self::Status,
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.cmd {
        Cmd::Replay {
            file,
            prefix,
            gap_ms,
        } => replay(file, &prefix, gap_ms),
        Cmd::Values { prefix } => print(call(args.socket, Request::Values { prefix })?),
        Cmd::Bus => print(call(args.socket, Request::Bus)?),
        Cmd::Capture { path } => print(call(args.socket, Request::SaveCapture { path })?),
        Cmd::Watch { prefix, period_ms } => watch(args.socket, prefix, period_ms),
        Cmd::Update { action, apply } => update(args.socket, action.into(), apply),
    }
}

fn replay(file: PathBuf, prefix: &str, gap_ms: f64) -> anyhow::Result<()> {
    let port = ReplayPort::load(&file, None, Duration::from_secs_f64(gap_ms / 1000.0), false)
        .with_context(|| format!("reading {}", file.display()))?;
    let mut car = Car::new(
        vec![IfaceSpec {
            name: "replay".into(),
            roles: vec!["ican".into()],
        }],
        Recorder::new(Duration::ZERO, 1),
        std::env::temp_dir(),
    );
    let base = Instant::now();
    let mut end = base;
    for rx in port.iter_from(base) {
        if let Some(ev) = car.ingest(0, &rx) {
            println!(
                "{:>9.3}s  bus {} -> {}",
                (rx.at - base).as_secs_f64(),
                ev.transition.from,
                ev.transition.to
            );
        }
        end = rx.at;
    }
    println!("{} frames, {:.1}s", port.len(), (end - base).as_secs_f64());
    print(car.handle(0, Request::Bus, end))?;
    print(car.handle(
        0,
        Request::Values {
            prefix: prefix.to_string(),
        },
        end,
    ))
}

fn print(resp: Response) -> anyhow::Result<()> {
    match resp {
        Response::Values { items } | Response::Update { items, .. } => {
            let w = items.iter().map(|r| r.name.len()).max().unwrap_or(0);
            for r in items {
                let v = match r.value {
                    Some(carpi_proto::Value::Bool(b)) => b.to_string(),
                    Some(carpi_proto::Value::Num(n)) => {
                        format!("{}", (n * 1000.0).round() / 1000.0)
                    }
                    Some(carpi_proto::Value::Text(s)) => s,
                    None => "-".into(),
                };
                println!("{:w$}  {:>12}  {:>6} ms", r.name, v, r.age_ms);
            }
        }
        Response::Bus { buses } => {
            for b in buses {
                println!(
                    "{} ({}): {} frames, {}",
                    b.iface,
                    b.role,
                    b.frames,
                    b.state.as_deref().unwrap_or("no ring")
                );
                for m in b.ring {
                    println!(
                        "  node {:02X} -> {:02X}{}{}  {} ms",
                        m.node,
                        m.successor,
                        if m.sleep_ind { " sleep-ind" } else { "" },
                        if m.sleep_ack { " sleep-ack" } else { "" },
                        m.age_ms
                    );
                }
            }
        }
        Response::CaptureSaved { path, frames } => println!("{frames} frames -> {path}"),
        Response::Hello { daemon, .. } => println!("{daemon}"),
        Response::Ok => {}
        Response::Error { message } => bail!(message),
        Response::Command { id, command } => println!("command {id}: {command:?}"),
        Response::JobStarted { job } => println!("job {job} started"),
        Response::Job { event, .. } => println!("{}", serde_json::to_string(&event)?),
        Response::Settings {
            settings,
            restart_needed,
        } => {
            println!("{}", serde_json::to_string_pretty(&settings)?);
            if !restart_needed.is_empty() {
                println!("restart needed for: {}", restart_needed.join(", "));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
struct Client(std::os::unix::net::UnixStream);

#[cfg(unix)]
impl Client {
    fn connect(socket: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = socket.unwrap_or_else(|| {
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("carpid.sock")
        });
        let s = std::os::unix::net::UnixStream::connect(&path)
            .with_context(|| format!("connecting to {}", path.display()))?;
        let mut c = Client(s);
        match c.request(&Request::Hello {
            version: carpi_proto::PROTOCOL_VERSION,
            client: "carpi".into(),
        })? {
            Response::Hello { .. } => Ok(c),
            Response::Error { message } => bail!(message),
            other => bail!("unexpected reply to hello: {other:?}"),
        }
    }

    fn request(&mut self, req: &Request) -> anyhow::Result<Response> {
        use std::io::Write;
        self.0.write_all(&carpi_proto::encode(req))?;
        self.recv()
    }

    fn recv(&mut self) -> anyhow::Result<Response> {
        use std::io::Read;
        let mut prefix = [0u8; 4];
        self.0.read_exact(&mut prefix)?;
        let mut body = vec![0u8; carpi_proto::body_len(prefix).map_err(anyhow::Error::msg)?];
        self.0.read_exact(&mut body)?;
        Ok(carpi_proto::decode(&body)?)
    }
}

#[cfg(unix)]
fn call(socket: Option<PathBuf>, req: Request) -> anyhow::Result<Response> {
    Client::connect(socket)?.request(&req)
}

#[cfg(unix)]
fn watch(socket: Option<PathBuf>, prefixes: Vec<String>, period_ms: u32) -> anyhow::Result<()> {
    let mut c = Client::connect(socket)?;
    print(c.request(&Request::Subscribe {
        sub: 1,
        prefixes,
        period_ms,
    })?)?;
    loop {
        print(c.recv()?)?;
    }
}

/// Starts an update job and prints its events until it finishes.
#[cfg(unix)]
fn update(
    socket: Option<PathBuf>,
    action: carpi_proto::UpdateAction,
    and_apply: bool,
) -> anyhow::Result<()> {
    let mut c = Client::connect(socket)?;
    print(c.request(&Request::Update { action, and_apply })?)?;
    loop {
        let resp = c.recv()?;
        let done = matches!(
            &resp,
            Response::Job {
                event: carpi_proto::JobEvent::Finished { .. },
                ..
            }
        );
        print(resp)?;
        if done {
            return Ok(());
        }
    }
}

#[cfg(not(unix))]
fn update(
    _socket: Option<PathBuf>,
    _action: carpi_proto::UpdateAction,
    _and_apply: bool,
) -> anyhow::Result<()> {
    bail!("talking to carpid needs a Unix socket")
}

#[cfg(not(unix))]
fn call(_socket: Option<PathBuf>, _req: Request) -> anyhow::Result<Response> {
    bail!("talking to carpid needs a Unix socket; use `carpi replay` here")
}

#[cfg(not(unix))]
fn watch(_socket: Option<PathBuf>, _prefixes: Vec<String>, _period_ms: u32) -> anyhow::Result<()> {
    bail!("talking to carpid needs a Unix socket; use `carpi replay` here")
}
