//! A second client, written from `plans/weft/spec/api.md` alone.
//!
//! This is Phase 3.9's exit criterion. It uses no part of Weft except the
//! socket path and the line types — no `App`, no board rules, no ledger, no
//! terminal. If it needs something the spec does not name, the spec was wrong.
//!
//!   cargo run --example wire -- <project> board
//!   cargo run --example wire -- <project> agents
//!   cargo run --example wire -- <project> waiting
//!   cargo run --example wire -- <project> answer <pending-id> yes|no
//!   cargo run --example wire -- <project> watch
//!
//! It talks to whatever daemon is already listening. Nothing here starts one.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()))
        .canonicalize()?;
    let what = args.next().unwrap_or_else(|| "board".into());

    let mut wire = Wire::open(&weft::protocol::socket_path())?;
    wire.call("hello", json!({"client": "wire/0.1", "protocol": 1}))?;
    // The board and the panes come back in the answer, so there is nothing to
    // wait for and nothing to miss.
    let opened = wire.call(
        "project.open",
        json!({"path": project.to_string_lossy(), "rows": 24, "cols": 80}),
    )?;

    match what.as_str() {
        "board" => {
            for u in list(&opened, "units") {
                println!(
                    "{:<38} {:<12} {}",
                    string(&u, "title"),
                    string(&u, "harness"),
                    // Nothing here works out what a unit means: the record says.
                    if u.get("cancelled") == Some(&json!(true)) { "cancelled" } else { "open" }
                );
            }
        }
        "agents" => {
            for p in list(&opened, "panes") {
                println!(
                    "{} {:<14} {:<30} {}",
                    p.get("pane").and_then(Value::as_u64).unwrap_or(0),
                    string(&p, "harness"),
                    string(&p, "spec"),
                    if p.get("running") == Some(&json!(true)) { "running" } else { "gone" }
                );
            }
        }
        "waiting" => {
            let mut none = true;
            for w in wire.events("pending.added", Duration::from_secs(2)) {
                none = false;
                println!("{}  {}  {}", string(&w, "id"), string(&w, "what"), string(&w, "why"));
            }
            if none {
                println!("nothing is waiting");
            }
        }
        "answer" => {
            let id = args.next().unwrap_or_default();
            let yes = args.next().as_deref() != Some("no");
            match wire.call("pending.resolve", json!({"pending": id, "yes": yes})) {
                Ok(_) => println!("answered {}", if yes { "yes" } else { "no" }),
                Err(e) => println!("{e}"),
            }
        }
        "watch" => {
            println!("watching {}; ^C to stop", project.display());
            loop {
                if let Some((method, params)) = wire.next_event(Duration::from_secs(30)) {
                    println!("{method} {}", truncate(&params.to_string(), 100));
                }
            }
        }
        other => anyhow::bail!("unknown: {other}"),
    }
    Ok(())
}

fn list(v: &Value, key: &str) -> Vec<Value> {
    v.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn string(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect::<String>() + "…" }
}

/// NDJSON over a Unix socket: one object per line, `id`/`method`/`params` out,
/// `id`/`result`, `id`/`error`, or `method`/`params` back.
struct Wire {
    out: UnixStream,
    lines: BufReader<UnixStream>,
    next: u64,
}

impl Wire {
    fn open(socket: &std::path::Path) -> anyhow::Result<Self> {
        let out = UnixStream::connect(socket)
            .map_err(|e| anyhow::anyhow!("no daemon at {}: {e}", socket.display()))?;
        let lines = BufReader::new(out.try_clone()?);
        Ok(Wire { out, lines, next: 1 })
    }

    fn call(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next;
        self.next += 1;
        let line = json!({"id": id, "method": method, "params": params}).to_string();
        writeln!(self.out, "{line}")?;
        self.out.flush()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let Some(v) = self.read_line()? else { break };
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(e) = v.get("error") {
                anyhow::bail!("{}: {}", string(e, "code"), string(e, "message"));
            }
            return Ok(v.get("result").cloned().unwrap_or_else(|| json!({})));
        }
        anyhow::bail!("no answer to {method}")
    }

    fn events(&mut self, method: &str, within: Duration) -> Vec<Value> {
        let deadline = Instant::now() + within;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            match self.next_event(Duration::from_millis(200)) {
                Some((m, params)) if m == method => out.push(params),
                _ => continue,
            }
        }
        out
    }

    fn next_event(&mut self, within: Duration) -> Option<(String, Value)> {
        self.out.set_read_timeout(Some(within)).ok()?;
        let v = self.read_line().ok()??;
        let method = v.get("method")?.as_str()?.to_string();
        Some((method, v.get("params").cloned().unwrap_or_else(|| json!({}))))
    }

    fn read_line(&mut self) -> anyhow::Result<Option<Value>> {
        let mut buf = String::new();
        if self.lines.read_line(&mut buf)? == 0 {
            return Ok(None);
        }
        Ok(serde_json::from_str(&buf).ok())
    }
}
