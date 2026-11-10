//! `carpi`: talk to carpid, or run a candump file through the car model
//! offline.

mod bench;
mod nav_trace;

use anyhow::{Context, bail};
use carpi_can::ReplayPort;
use carpi_can::recorder::Recorder;
use carpi_core::{Car, IfaceSpec};
use carpi_proto::{Request, Response};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(version, about = "rnse-carpi debug tool")]
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
    /// The value catalog: name, unit, category / sub-group, source.
    Catalog,
    /// Bus and NM ring state from carpid.
    Bus,
    /// Save a capture into carpid's logs folder: the last few minutes of
    /// CAN, journal, values and config, as the next `<date> #NN`.
    Capture {
        /// What made it: `cli` by hand; `crash:<unit>` from carpi-capture@.service.
        #[arg(long, default_value = "cli")]
        trigger: String,
    },
    /// Data logging: start a profile, stop the run, list the profiles.
    Log {
        #[command(subcommand)]
        cmd: LogCmd,
    },
    /// Stream changing values from carpid until Ctrl-C.
    Watch {
        #[arg(default_value = "")]
        prefix: Vec<String>,
        #[arg(long, default_value_t = 200)]
        period_ms: u32,
    },
    /// Record nav as carpid holds it until Ctrl-C: one JSON line per change
    /// with the whole `nav.*` / `projection.*` picture.
    NavTrace {
        /// Also append the lines to this file.
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 100)]
        period_ms: u32,
    },
    /// Drive the bench cluster's DIS with no daemon: wake + Kl15 frames,
    /// scripted nav and readings, a session log.
    DisBench {
        #[arg(long, value_enum, default_value = "both")]
        screen: bench::Screens,
        #[arg(long, default_value_t = 60.0)]
        secs: f64,
        /// Write the driver's believed centre as ASCII here at the end.
        #[arg(long)]
        save_fb: Option<PathBuf>,
        /// SocketCAN interface (Linux).
        #[arg(long)]
        iface: Option<String>,
        /// J2534 DLL; default: from the registry.
        #[arg(long)]
        j2534_dll: Option<PathBuf>,
        /// Registry entry name to pick (substring), e.g. "SM2".
        #[arg(long)]
        j2534_device: Option<String>,
        #[arg(long, default_value_t = 500_000)]
        bitrate: u32,
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

#[derive(Subcommand)]
enum LogCmd {
    /// Start logging a profile into the logs folder.
    Start { profile: String },
    /// Stop the run and save it.
    Stop,
    /// The profiles, what each costs, and the run.
    Profiles,
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
        Cmd::Catalog => print(call(args.socket, Request::Catalog)?),
        Cmd::Bus => print(call(args.socket, Request::Bus)?),
        Cmd::Capture { trigger } => print(call(args.socket, Request::Capture { trigger })?),
        Cmd::Log { cmd } => log(args.socket, cmd),
        Cmd::Watch { prefix, period_ms } => watch(args.socket, prefix, period_ms),
        Cmd::NavTrace { out, period_ms } => nav_trace(args.socket, out, period_ms),
        Cmd::Update { action, apply } => update(args.socket, action.into(), apply),
        Cmd::DisBench {
            screen,
            secs,
            save_fb,
            iface,
            j2534_dll,
            j2534_device,
            bitrate,
        } => {
            let ports = open_ports(iface, j2534_dll, j2534_device, bitrate)?;
            dis_bench(ports, screen, secs, save_fb)
        }
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
        Response::Wifi {
            networks,
            connected,
        } => {
            for n in networks {
                let on = connected.as_deref() == Some(n.ssid.as_str());
                println!(
                    "{}  {}{}",
                    n.ssid,
                    if n.secured { "password" } else { "open" },
                    if on { "  (connected)" } else { "" }
                );
            }
        }
        Response::Captured {
            label,
            dir,
            summary,
            ..
        } => println!(
            "{label}: {summary}
{dir}"
        ),
        Response::Catalog { values, .. } => {
            let w = values.iter().map(|v| v.name.len()).max().unwrap_or(0);
            for v in values {
                let src = match v.source {
                    carpi_proto::ValueSource::Ican => "ican".to_string(),
                    carpi_proto::ValueSource::Diag { modules } => format!("diag {modules:02X?}"),
                };
                println!(
                    "{:w$}  {:>7}  {} / {}  {}  ({})",
                    v.name, v.unit, v.category, v.sub, v.label, src
                );
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
        Response::Datalog { status } => print_datalog(&status),
        Response::Storage { storage } => println!("{}", serde_json::to_string_pretty(&storage)?),
        Response::Firmware { status } => println!("{}", serde_json::to_string_pretty(&status)?),
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

fn log(socket: Option<PathBuf>, cmd: LogCmd) -> anyhow::Result<()> {
    use carpi_proto::DatalogAction as A;
    let action = match &cmd {
        LogCmd::Start { profile } => A::Start {
            profile: profile.clone(),
        },
        LogCmd::Stop => A::Stop,
        LogCmd::Profiles => A::Status,
    };
    let status = match call(socket, Request::Datalog { action })? {
        Response::Datalog { status } => status,
        Response::Error { message } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    };
    let run = match cmd {
        LogCmd::Profiles => {
            print_datalog(&status);
            return Ok(());
        }
        LogCmd::Start { .. } => status.running.as_ref(),
        LogCmd::Stop => status.last.as_ref(),
    };
    if let Some(r) = run {
        match &r.error {
            _ if r.summary.is_empty() => println!(
                "Logging {} ({}, {} values, {} Hz)",
                r.label, r.profile, r.values, r.rate_hz
            ),
            None => println!("{}: {}", r.label, r.summary),
            Some(e) => println!("{}: {} (stopped: {e})", r.label, r.summary),
        }
        println!("{}", r.dir);
    }
    Ok(())
}

fn print_datalog(s: &carpi_proto::DatalogStatus) {
    let w = s
        .profiles
        .iter()
        .map(|p| p.profile.name.chars().count())
        .max()
        .unwrap_or(0);
    for p in &s.profiles {
        let l = &p.load;
        let mut cost = format!("{} bus, {} diag", l.bus_values, l.diag_values);
        if l.diag_groups > 0 {
            cost.push_str(&format!(
                " ({} groups, {} modules, ~{} Hz)",
                l.diag_groups,
                l.diag_modules,
                l.diag_hz.unwrap_or(0.0)
            ));
        }
        let key = if s.selected.as_ref() == Some(&p.profile.name) {
            "  [key]"
        } else {
            ""
        };
        println!(
            "{:w$}  {:>2} Hz  {:>3} values  {cost}{key}",
            p.profile.name,
            p.profile.rate_hz,
            p.profile.values.len()
        );
    }
    if s.profiles.is_empty() {
        println!("no profiles");
    }
    if let Some(r) = &s.running {
        println!(
            "running: {} {}, {} s, {} rows",
            r.label,
            r.profile,
            r.elapsed_ms / 1000,
            r.rows
        );
    }
    if let Some(r) = &s.last {
        println!("last: {} {}, {}", r.label, r.profile, r.summary);
    }
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

#[cfg(unix)]
fn nav_trace(socket: Option<PathBuf>, out: Option<PathBuf>, period_ms: u32) -> anyhow::Result<()> {
    use std::io::Write;
    let mut file = match &out {
        Some(p) => Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .with_context(|| format!("opening {}", p.display()))?,
        ),
        None => None,
    };
    let mut c = Client::connect(socket)?;
    print(c.request(&Request::Subscribe {
        sub: 1,
        prefixes: nav_trace::PREFIXES.map(String::from).to_vec(),
        period_ms,
    })?)?;
    let start = Instant::now();
    let mut trace = nav_trace::Trace::default();
    loop {
        let Response::Update { items, .. } = c.recv()? else {
            continue;
        };
        let unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let Some(line) = trace.line(&items, unix_ms, start.elapsed().as_millis()) else {
            continue;
        };
        println!("{line}");
        if let Some(f) = &mut file {
            writeln!(f, "{line}")?;
            f.flush()?;
        }
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
fn nav_trace(
    _socket: Option<PathBuf>,
    _out: Option<PathBuf>,
    _period_ms: u32,
) -> anyhow::Result<()> {
    bail!("talking to carpid needs a Unix socket")
}

#[cfg(not(unix))]
fn watch(_socket: Option<PathBuf>, _prefixes: Vec<String>, _period_ms: u32) -> anyhow::Result<()> {
    bail!("talking to carpid needs a Unix socket; use `carpi replay` here")
}

#[cfg(windows)]
fn open_ports(
    iface: Option<String>,
    dll: Option<PathBuf>,
    device: Option<String>,
    bitrate: u32,
) -> anyhow::Result<(carpi_can::Port, carpi_can::TxPort)> {
    if iface.is_some() {
        bail!("--iface is SocketCAN (Linux); use --j2534-dll or --j2534-device here");
    }
    let (rx, tx) = carpi_can::j2534::open(&carpi_can::j2534::Options {
        dll,
        device,
        bitrate,
        filter: carpi_can::j2534::Filter::PassAll,
    })
    .context("opening J2534")?;
    if let carpi_can::Port::J2534(p) = &rx {
        println!("J2534 {} at {bitrate} bit/s", p.device().dll.display());
    }
    Ok((rx, tx))
}

#[cfg(not(windows))]
fn open_ports(
    iface: Option<String>,
    _dll: Option<PathBuf>,
    _device: Option<String>,
    _bitrate: u32,
) -> anyhow::Result<(carpi_can::Port, carpi_can::TxPort)> {
    // The interface's bitrate is set with `ip link`.
    let iface = iface.context("--iface needed (e.g. can0 at 500k)")?;
    Ok((
        carpi_can::Port::open(&iface).with_context(|| format!("opening {iface}"))?,
        carpi_can::TxPort::open(&iface).with_context(|| format!("opening {iface}"))?,
    ))
}

/// Runs the bench until `secs` or Ctrl-C, then releases and closes the link.
fn dis_bench(
    (mut rx, tx): (carpi_can::Port, carpi_can::TxPort),
    screen: bench::Screens,
    secs: f64,
    save_fb: Option<PathBuf>,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let mut b = bench::Bench::new(Instant::now(), screen, secs);
        let mut tx_errors = 0u32;
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        loop {
            while let Some(f) = b.poll_transmit() {
                if let Err(e) = tx.send(&f).await {
                    tx_errors += 1;
                    if tx_errors <= 5 || tx_errors.is_power_of_two() {
                        println!("tx error #{tx_errors}: {e}");
                    }
                }
            }
            while let Some(l) = b.poll_log() {
                println!("{l}");
            }
            let now = Instant::now();
            if b.done(now) {
                break;
            }
            let at = b.next_deadline(now);
            tokio::select! {
                r = rx.recv() => match r? {
                    Some(f) => b.on_frame(f.at, &f.frame),
                    None => break,
                },
                _ = tokio::time::sleep_until(at.into()) => {}
                _ = &mut ctrl_c => break,
            }
            b.tick(Instant::now());
        }
        // Release (33) and A8, give them time to go out, then close the port.
        b.stop(Instant::now());
        let until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < until {
            while let Some(f) = b.poll_transmit() {
                let _ = tx.send(&f).await;
            }
            tokio::select! {
                r = rx.recv() => if let Ok(Some(f)) = r { b.on_frame(f.at, &f.frame) },
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
            b.tick(Instant::now());
        }
        while let Some(l) = b.poll_log() {
            println!("{l}");
        }
        if let Some(path) = save_fb {
            let text = b
                .belief_ascii()
                .unwrap_or_else(|| "(nothing drawn)\n".into());
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
            println!("belief -> {}", path.display());
        }
        Ok(())
    })
}
