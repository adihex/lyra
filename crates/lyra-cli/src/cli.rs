//! `lyra` CLI: thin socket client. Human output by default, `--json` for agents.
//! Exit codes (§6 rule 7): 0 ok, 2 not-running, 3 conflict, 4 invalid.

use clap::{Parser, Subcommand};
use lyra_ipc::{Client, ClientError};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "lyra",
    about = "Drive the Lyra player over the local control socket"
)]
pub struct Cli {
    /// Explicit socket path (overrides LYRA_SOCKET + well-known paths).
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,
    /// Machine-readable JSON output (for agents).
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Play,
    Pause,
    Toggle,
    Stop,
    Next,
    Prev,
    /// Seek: `+30s`, `-10`, `83` (seconds) or `01:23` (mm:ss).
    Seek {
        spec: String,
    },
    /// Volume 0-100 (percent) or 0.0-1.0.
    Volume {
        level: String,
    },
    /// Full snapshot (same object agents poll).
    State,
    /// One-line human status (alias of state, condensed).
    Status,
    /// Current track line.
    #[command(name = "now-playing")]
    NowPlaying,
    /// Queue management.
    Queue {
        #[command(subcommand)]
        op: QueueOp,
    },
    /// FTS library search.
    Search {
        q: String,
        #[arg(long)]
        r#type: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Library rescan (async job).
    Scan {
        #[arg(long)]
        path: Option<String>,
    },
    /// Library stats.
    Stats,
    /// Audio devices (list) / switch output.
    Devices {
        #[arg(long)]
        use_device: Option<String>,
    },
    /// EQ: `eq get`, `eq set --band N --gain DB`, `eq set --bands …`.
    Eq {
        #[command(subcommand)]
        op: EqOp,
    },
    /// Stream events as NDJSON until interrupted.
    Subscribe {
        #[arg(default_values_t = [String::from("runtime.state"), String::from("runtime.job"), String::from("queue")])]
        topics: Vec<String>,
    },
    /// Machine-readable op list (agents self-discover).
    Capabilities,
    /// Liveness probe: socket, sidecar, perms, ping.
    Doctor,
}

#[derive(Debug, Subcommand)]
pub enum QueueOp {
    List,
    Add {
        #[arg(long)]
        track: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        position: Option<i64>,
    },
    Remove {
        index: i64,
    },
    Move {
        from: i64,
        to: i64,
    },
    Clear,
}

#[derive(Debug, Subcommand)]
pub enum EqOp {
    Get,
    Set {
        #[arg(long)]
        band: Option<i64>,
        #[arg(long, allow_hyphen_values = true)]
        gain: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        bands: Option<String>,
        #[arg(long)]
        preamp: Option<f64>,
    },
}

/// Parsed seek target.
#[derive(Debug, PartialEq)]
pub enum SeekArg {
    Absolute(f64),
    Relative(f64),
}

/// `+30s`/`-10` → relative; `83`/`01:23` → absolute. Pure, unit-tested.
pub fn parse_seek(spec: &str) -> Result<SeekArg, String> {
    let s = spec.trim();
    let (relative, body) = match s.strip_prefix('+') {
        Some(b) => (true, b),
        None => match s.strip_prefix('-') {
            Some(_) => (true, s),
            None => (false, s),
        },
    };
    let body = body.strip_suffix('s').unwrap_or(body);
    let secs = if let Some((m, sec)) = body.split_once(':') {
        let m: f64 = m.parse().map_err(|_| format!("bad seek spec: {spec}"))?;
        let sec: f64 = sec.parse().map_err(|_| format!("bad seek spec: {spec}"))?;
        m * 60.0 + sec
    } else {
        body.parse::<f64>()
            .map_err(|_| format!("bad seek spec: {spec}"))?
    };
    Ok(if relative {
        SeekArg::Relative(secs)
    } else {
        SeekArg::Absolute(secs)
    })
}

/// `0-100` or `0.0-1.0` → `0.0-1.0`. Pure, unit-tested.
pub fn parse_volume(level: &str) -> Result<f64, String> {
    let v: f64 = level
        .trim()
        .strip_suffix('%')
        .unwrap_or(level.trim())
        .parse()
        .map_err(|_| format!("bad volume: {level}"))?;
    let v = if v > 1.0 { v / 100.0 } else { v };
    if !(0.0..=1.0).contains(&v) {
        return Err(format!("volume out of range: {level}"));
    }
    Ok(v)
}

struct Out {
    json: bool,
}

impl Out {
    fn value(&self, v: &Value) {
        if self.json {
            println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
        }
    }

    fn line(&self, s: &str) {
        if !self.json {
            println!("{s}");
        }
    }
}

fn connect(socket: Option<PathBuf>) -> Result<Client, ClientError> {
    if let Some(p) = socket {
        Client::connect(&p)
    } else {
        Client::discover_and_connect()
    }
}

/// `operation.submit` wrapper returning (job, snapshot).
fn submit(
    c: &mut Client,
    operation: &str,
    params: Value,
) -> Result<(Value, Option<Value>), ClientError> {
    let r = c.call(
        "operation.submit",
        serde_json::json!({"operation": operation, "params": params}),
    )?;
    let job = r.job.clone().unwrap_or(Value::Null);
    Ok((job, r.snapshot.clone()))
}

fn job_result(job: &Value) -> Value {
    job.get("result").cloned().unwrap_or(Value::Null)
}

fn snapshot_line(snap: &Value) -> String {
    let state = snap.get("state").and_then(|v| v.as_str()).unwrap_or("?");
    let track = snap.get("track");
    let title = track
        .and_then(|t| t.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("—");
    let artist = track
        .and_then(|t| t.get("artist"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let pos = snap.get("position").and_then(Value::as_f64).unwrap_or(0.0);
    let dur = snap.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
    format!("{state} {artist} — {title} [{pos:.0}/{dur:.0}s]")
}

pub fn run(args: Vec<String>) -> i32 {
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            // clap prints help/version itself; usage errors → exit 4.
            let _ = e.print();
            return 4;
        }
    };
    match execute(cli) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lyra: {e}");
            e.exit_code()
        }
    }
}

fn exec_queue(c: &mut Client, out: &Out, json: bool, op: QueueOp) -> Result<(), ClientError> {
    match op {
        QueueOp::List => {
            let r = c.call(
                "operation.submit",
                serde_json::json!({"operation": "queue.list", "params": {}}),
            )?;
            let payload = job_result(r.job.as_ref().unwrap_or(&Value::Null));
            out.value(&payload);
            if !json {
                let tracks = payload.get("tracks").and_then(|t| t.as_array());
                match tracks {
                    Some(ts) if !ts.is_empty() => {
                        for (i, t) in ts.iter().enumerate() {
                            println!(
                                "{i}\t{}\t{}\t{}",
                                t.get("id").and_then(|v| v.as_str()).unwrap_or("?"),
                                t.get("artist").and_then(|v| v.as_str()).unwrap_or(""),
                                t.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                            );
                        }
                    }
                    _ => println!("(empty)"),
                }
            }
        }
        QueueOp::Add {
            track,
            query,
            position,
        } => {
            let mut p = serde_json::json!({});
            if let Some(t) = track {
                p["track_id"] = t.into();
            }
            if let Some(q) = query {
                p["query"] = q.into();
            }
            if let Some(pos) = position {
                p["position"] = pos.into();
            }
            let (job, _) = submit(c, "queue.enqueue", p)?;
            out.value(&job);
            out.line(&format!(
                "queued ({})",
                job.get("length").map(|v| v.to_string()).unwrap_or_default()
            ));
        }
        QueueOp::Remove { index } => {
            let (job, _) = submit(c, "queue.remove", serde_json::json!({"index": index}))?;
            out.value(&job);
            out.line("removed");
        }
        QueueOp::Move { from, to } => {
            let (job, _) = submit(c, "queue.move", serde_json::json!({"from": from, "to": to}))?;
            out.value(&job);
            out.line("moved");
        }
        QueueOp::Clear => {
            let (job, _) = submit(c, "queue.clear", serde_json::json!({}))?;
            out.value(&job);
            out.line("cleared");
        }
    }
    Ok(())
}

fn exec_eq(c: &mut Client, out: &Out, op: EqOp) -> Result<(), ClientError> {
    match op {
        EqOp::Get => {
            let (job, _) = submit(c, "eq.get", serde_json::json!({}))?;
            let payload = job_result(&job);
            out.value(&payload);
            out.line(&payload.to_string());
        }
        EqOp::Set {
            band,
            gain,
            bands,
            preamp,
        } => {
            let params = if let (Some(b), Some(g)) = (band, gain) {
                serde_json::json!({"band": b, "gain_db": g})
            } else {
                let mut p = serde_json::json!({});
                if let Some(list) = bands {
                    let arr: Result<Vec<f64>, _> =
                        list.split(',').map(|s| s.trim().parse()).collect();
                    let arr =
                        arr.map_err(|_| ClientError::Protocol(format!("bad --bands: {list}")))?;
                    p["bands"] = arr.into();
                }
                if let Some(pa) = preamp {
                    p["preamp"] = pa.into();
                }
                p
            };
            let op = if band.is_some() {
                "eq.band.set"
            } else {
                "eq.set"
            };
            let (job, _) = submit(c, op, params)?;
            out.value(&job);
            out.line("eq updated");
        }
    }
    Ok(())
}

fn execute(cli: Cli) -> Result<(), ClientError> {
    let out = Out { json: cli.json };
    let socket = cli.socket.clone();
    let mut c = connect(socket.clone())?;
    match cli.cmd {
        Command::Play => {
            let (job, snap) = submit(&mut c, "play", serde_json::json!({}))?;
            out.value(&serde_json::json!({"job": job, "snapshot": snap}));
            out.line(&snap.map(|s| snapshot_line(&s)).unwrap_or_default());
        }
        Command::Pause => {
            let (job, snap) = submit(&mut c, "pause", serde_json::json!({}))?;
            out.value(&serde_json::json!({"job": job, "snapshot": snap}));
            out.line(&snap.map(|s| snapshot_line(&s)).unwrap_or_default());
        }
        Command::Toggle => {
            let (job, snap) = submit(&mut c, "toggle", serde_json::json!({}))?;
            out.value(&serde_json::json!({"job": job, "snapshot": snap}));
            out.line(&snap.map(|s| snapshot_line(&s)).unwrap_or_default());
        }
        Command::Stop => {
            let (job, snap) = submit(&mut c, "stop", serde_json::json!({}))?;
            out.value(&serde_json::json!({"job": job, "snapshot": snap}));
            out.line("stopped");
        }
        Command::Next => {
            let (job, snap) = submit(&mut c, "next", serde_json::json!({}))?;
            out.value(&serde_json::json!({"job": job, "snapshot": snap}));
            out.line(&snap.map(|s| snapshot_line(&s)).unwrap_or_default());
        }
        Command::Prev => {
            let (job, snap) = submit(&mut c, "prev", serde_json::json!({}))?;
            out.value(&serde_json::json!({"job": job, "snapshot": snap}));
            out.line(&snap.map(|s| snapshot_line(&s)).unwrap_or_default());
        }
        Command::Seek { spec } => {
            let arg = parse_seek(&spec).map_err(|_| ClientError::Protocol(spec.clone()))?;
            let (op, params) = match arg {
                SeekArg::Absolute(p) => ("seek.absolute", serde_json::json!({"position_s": p})),
                SeekArg::Relative(d) => ("seek.relative", serde_json::json!({"delta_s": d})),
            };
            let (job, snap) = submit(&mut c, op, params)?;
            out.value(&serde_json::json!({"job": job, "snapshot": snap}));
            out.line(&snap.map(|s| snapshot_line(&s)).unwrap_or_default());
        }
        Command::Volume { level } => {
            let v = parse_volume(&level).map_err(ClientError::Protocol)?;
            let (job, snap) = submit(&mut c, "volume.set", serde_json::json!({"volume": v}))?;
            out.value(&serde_json::json!({"job": job, "snapshot": snap}));
            out.line(&format!("volume {}", (v * 100.0).round()));
        }
        Command::State | Command::Status | Command::NowPlaying => {
            let snap = c.state()?;
            out.value(&snap);
            out.line(&snapshot_line(&snap));
        }
        Command::Queue { op } => exec_queue(&mut c, &out, cli.json, op)?,
        Command::Search { q, r#type, limit } => {
            let mut p = serde_json::json!({"q": q, "limit": limit});
            if let Some(t) = r#type {
                p["type"] = t.into();
            }
            let (job, _) = submit(&mut c, "library.search", p)?;
            let payload = job_result(&job);
            out.value(&payload);
            if !cli.json {
                for t in payload
                    .get("results")
                    .and_then(|r| r.as_array())
                    .into_iter()
                    .flatten()
                {
                    println!(
                        "{}\t{}\t{}",
                        t.get("id").and_then(|v| v.as_str()).unwrap_or("?"),
                        t.get("artist").and_then(|v| v.as_str()).unwrap_or(""),
                        t.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                    );
                }
            }
        }
        Command::Scan { path } => {
            let mut p = serde_json::json!({});
            if let Some(path) = path {
                p["path"] = path.into();
            }
            let r = c.call(
                "operation.submit",
                serde_json::json!({"operation": "library.scan", "params": p}),
            )?;
            out.value(r.job.as_ref().unwrap_or(&Value::Null));
            if !cli.json {
                println!(
                    "scan job {} ({})",
                    r.job
                        .as_ref()
                        .and_then(|j| j.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?"),
                    r.job
                        .as_ref()
                        .and_then(|j| j.get("state"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?"),
                );
            }
        }
        Command::Stats => {
            let (job, _) = submit(&mut c, "library.stats", serde_json::json!({}))?;
            let payload = job_result(&job);
            out.value(&payload);
            out.line(&format!(
                "{} tracks",
                payload.get("tracks").and_then(Value::as_u64).unwrap_or(0)
            ));
        }
        Command::Devices { use_device } => {
            if let Some(d) = use_device {
                let (job, _) = submit(&mut c, "device.set", serde_json::json!({"device": d}))?;
                out.value(&job);
                out.line(&format!("device → {d}"));
            } else {
                let (job, _) = submit(&mut c, "device.list", serde_json::json!({}))?;
                let payload = job_result(&job);
                out.value(&payload);
                if !cli.json {
                    for d in payload
                        .get("devices")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                    {
                        println!("{}", d.as_str().unwrap_or("?"));
                    }
                }
            }
        }
        Command::Eq { op } => exec_eq(&mut c, &out, op)?,
        Command::Subscribe { topics } => {
            let refs: Vec<&str> = topics.iter().map(String::as_str).collect();
            let mut sub = c.subscribe(&refs)?;
            loop {
                match sub.next_event() {
                    Ok(lyra_ipc::client::SubItem::Event(ev)) => {
                        println!("{}", serde_json::to_string(&ev).unwrap_or_default());
                    }
                    Ok(lyra_ipc::client::SubItem::Resync { snapshot, .. }) => {
                        eprintln!("lyra: seq gap — resynced");
                        if cli.json {
                            println!("{}", serde_json::to_string(&snapshot).unwrap_or_default());
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        Command::Capabilities => {
            let r = c.call("capabilities", serde_json::json!({}))?;
            let ops = r.operations.clone().unwrap_or(Value::Null);
            out.value(&serde_json::json!({"operations": ops}));
            if !cli.json {
                for op in ops.as_array().into_iter().flatten() {
                    println!("{}", op.get("name").and_then(|v| v.as_str()).unwrap_or("?"));
                }
            }
        }
        Command::Doctor => {
            doctor(socket, &out)?;
        }
    }
    Ok(())
}

fn doctor(socket: Option<PathBuf>, out: &Out) -> Result<(), ClientError> {
    use lyra_ipc::paths;
    let mut report = serde_json::json!({});
    let candidates = match socket.clone() {
        Some(p) => vec![p],
        None => paths::candidates(),
    };
    report["candidates"] = candidates.iter().map(|p| p.display().to_string()).collect();
    let mut live: Option<PathBuf> = None;
    for p in &candidates {
        if std::os::unix::net::UnixStream::connect(p).is_ok() {
            live = Some(p.clone());
            break;
        }
    }
    match &live {
        Some(p) => {
            report["socket"] = p.display().to_string().into();
            if let Some(sc) = paths::read_sidecar(p) {
                report["sidecar"] = serde_json::json!({
                    "pid": sc.pid, "protocol": sc.protocol, "version": sc.version,
                });
            }
            let mut c = Client::connect(p)?;
            let caps = c.call("capabilities", serde_json::json!({}))?;
            report["protocol"] = 2.into();
            report["operations"] = caps
                .operations
                .as_ref()
                .and_then(|o| o.as_array())
                .map(|a| a.len())
                .into();
            report["ok"] = true.into();
        }
        None => {
            report["ok"] = false.into();
            report["hint"] = "is Lyra running? start the app (in-app wiring lands later; use a mock server for now)".into();
        }
    }
    out.value(&report);
    if report["ok"] == true {
        out.line("lyra socket alive");
        Ok(())
    } else {
        Err(ClientError::NotRunning(
            candidates
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_arg_parsing() {
        let cli = Cli::try_parse_from(["lyra", "play"]).unwrap();
        assert!(matches!(cli.cmd, Command::Play));
        let cli = Cli::try_parse_from(["lyra", "--json", "seek", "+30s"]).unwrap();
        assert!(cli.json);
        let cli = Cli::try_parse_from(["lyra", "queue", "add", "--query", "jazz"]).unwrap();
        assert!(matches!(cli.cmd, Command::Queue { .. }));
        let cli =
            Cli::try_parse_from(["lyra", "eq", "set", "--band", "3", "--gain", "-2.5"]).unwrap();
        assert!(matches!(cli.cmd, Command::Eq { .. }));
        assert!(Cli::try_parse_from(["lyra", "bogus"]).is_err());
    }

    #[test]
    fn seek_specs() {
        assert_eq!(parse_seek("+30s").unwrap(), SeekArg::Relative(30.0));
        assert_eq!(parse_seek("-10").unwrap(), SeekArg::Relative(-10.0));
        assert_eq!(parse_seek("83").unwrap(), SeekArg::Absolute(83.0));
        assert_eq!(parse_seek("01:23").unwrap(), SeekArg::Absolute(83.0));
        assert!(parse_seek("soon").is_err());
    }

    #[test]
    fn volume_levels() {
        assert_eq!(parse_volume("80").unwrap(), 0.8);
        assert_eq!(parse_volume("0.5").unwrap(), 0.5);
        assert_eq!(parse_volume("100%").unwrap(), 1.0);
        assert!(parse_volume("loud").is_err());
        assert!(parse_volume("150").is_err());
    }
}
