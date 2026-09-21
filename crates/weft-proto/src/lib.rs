//! The socket API.
//!
//! Spec: `plans/weft/spec/api.md`, decided in ADR-0007. One NDJSON line per
//! message. The TUI is a client of this and so is everything else; if the TUI
//! needs something this cannot express, that is a gap here rather than a
//! reason for a private path.
//!
//! **It speaks in decisions, never in keystrokes.** `pane.input` is the one
//! exception: a client showing a pane forwards the person's own typing into
//! it. Everything that would type on the person's behalf goes through a
//! pending, which is shown before anything is written.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
pub use weft_core::board::PaneInfo;
use weft_core::inject::Handoff;
use weft_core::ledger::Unit;

/// What this daemon speaks. A client that asks for another is refused with a
/// number rather than left to guess.
pub const PROTOCOL: u64 = 1;

/// Refuse anything absurd rather than allocating on a corrupt length.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// A call, from a client. Every one gets a `result` or an `error`.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// Version handshake, first.
    Hello { client: String, protocol: u64 },
    /// Watch a project. The daemon holds many; a client sees one.
    Open { path: String, rows: u16, cols: u16 },
    /// Start a harness, fresh or resuming a recorded session.
    StartAgent { harness: String, spec: String },
    /// Stop an agent and take its pane away.
    CloseAgent { pane: u32 },
    /// A person's own typing, into a pane they are showing. Not an act.
    Input { pane: u32, bytes: Vec<u8> },
    Resize { pane: u32, rows: u16, cols: u16 },
    /// Put a prompt in front of the person. Answers with its pending id;
    /// nothing is typed until that is resolved.
    Stage { pane: u32, bytes: Vec<u8>, how: Handoff, what: String, why: String },
    /// One of RingFrame's three acts, by name. The daemon works out what to
    /// type, where it goes, and how it has to be typed; a client that
    /// assembled bytes would be re-deriving all of that. Answers with a
    /// pending.
    Act { act: String, unit: Option<String>, pane: Option<u32>, text: Option<String> },
    /// Record the person's yes for an Ask whose chooser never got one.
    ConfirmAsk { unit: String },
    /// The exact wording, or the judgements, as RingFrame recorded them.
    Read { what: String, unit: String },
    /// Run the install commands for a harness, then ask it again.
    SetUp { harness: String },
    /// The agents that could be started here: fresh, or picking up a session
    /// RingFrame has a receipt for.
    Available,
    /// Yes or no, by id. Whoever answers first answers for everyone.
    Resolve { pending: String, yes: bool },
    /// Leave, without stopping anything.
    Detach,
    /// Stop every agent and end the session.
    Shutdown,
}

/// Something that happened. Unsolicited, to every client of that project.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The panes of the project just opened, in order.
    Panes { panes: Vec<PaneInfo> },
    Output { pane: u32, bytes: Vec<u8> },
    Added { pane: u32, harness: String, spec: String },
    Exited { pane: u32 },
    /// The board, and the Eval record behind each check. Sent on opening a
    /// project and whenever the ledger moves, so nothing reads a file to draw
    /// a frame.
    Units { units: Vec<Unit>, records: Value },
    /// Something is waiting on a person. Every attached client is told.
    Pending { id: String, pane: u32, what: String, why: String, payload: Vec<u8> },
    /// It was answered. Once.
    Resolved { id: String, yes: bool },
    /// A prompt was typed, or was not, and why not.
    Injected { pane: u32, refusal: Option<String> },
    /// What each harness is short of here, by the name RingFrame records.
    Readiness { states: Value },
}

/// One line on the wire: a call, its answer, or an event.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Call { id: u64, call: Call },
    Result { id: u64, result: Value },
    Error { id: u64, code: String, message: String },
    Event(Event),
}

// --- bytes in a text protocol ------------------------------------------------

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Pane output is arbitrary bytes and JSON holds text, so what crosses is
/// base64. A third larger, and paid only by a client that asked for output.
fn b64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

fn un_b64(text: &str) -> Option<Vec<u8>> {
    let (mut acc, mut bits) = (0u32, 0u8);
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for ch in text.bytes() {
        let six = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        acc = acc << 6 | six as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn handoff_json(how: &Handoff) -> Value {
    match how {
        Handoff::Whole => json!({"kind": "whole"}),
        Handoff::TypeCommand { command, at } => {
            json!({"kind": "type", "command": command, "at": at})
        }
        Handoff::EnterMode { command, at, active } => {
            json!({"kind": "mode", "command": command, "at": at, "active": active})
        }
    }
}

fn handoff_of(v: &Value) -> Option<Handoff> {
    let command = v.get("command").and_then(Value::as_str).map(str::to_string);
    let at = v.get("at").and_then(Value::as_u64).map(|n| n as usize);
    match v.get("kind")?.as_str()? {
        "whole" => Some(Handoff::Whole),
        "type" => Some(Handoff::TypeCommand { command: command?, at: at? }),
        "mode" => Some(Handoff::EnterMode {
            command: command?,
            at: at?,
            active: v.get("active")?.as_str()?.to_string(),
        }),
        _ => None,
    }
}

// --- the method names --------------------------------------------------------

impl Call {
    fn parts(&self) -> (&'static str, Value) {
        match self {
            Call::Hello { client, protocol } => {
                ("hello", json!({"client": client, "protocol": protocol}))
            }
            Call::Open { path, rows, cols } => {
                ("project.open", json!({"path": path, "rows": rows, "cols": cols}))
            }
            Call::StartAgent { harness, spec } => {
                ("agent.start", json!({"harness": harness, "spec": spec}))
            }
            Call::CloseAgent { pane } => ("agent.close", json!({"pane": pane})),
            Call::Input { pane, bytes } => ("pane.input", json!({"pane": pane, "bytes": b64(bytes)})),
            Call::Resize { pane, rows, cols } => {
                ("pane.resize", json!({"pane": pane, "rows": rows, "cols": cols}))
            }
            Call::Stage { pane, bytes, how, what, why } => (
                "stage",
                json!({"pane": pane, "bytes": b64(bytes), "how": handoff_json(how),
                       "what": what, "why": why}),
            ),
            Call::Act { act, unit, pane, text } => {
                ("act", json!({"act": act, "unit": unit, "pane": pane, "text": text}))
            }
            Call::ConfirmAsk { unit } => ("unit.confirm", json!({"unit": unit})),
            Call::Read { what, unit } => ("read", json!({"what": what, "unit": unit})),
            Call::SetUp { harness } => ("readiness.setup", json!({"harness": harness})),
            Call::Available => ("agents.available", json!({})),
            Call::Resolve { pending, yes } => {
                ("pending.resolve", json!({"pending": pending, "yes": yes}))
            }
            Call::Detach => ("detach", json!({})),
            Call::Shutdown => ("shutdown", json!({})),
        }
    }

    fn of(method: &str, p: &Value) -> Option<Self> {
        let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| p.get(k).and_then(Value::as_u64);
        let bytes = |k: &str| un_b64(p.get(k)?.as_str()?);
        Some(match method {
            "hello" => Call::Hello { client: s("client")?, protocol: n("protocol")? },
            "project.open" => {
                Call::Open { path: s("path")?, rows: n("rows")? as u16, cols: n("cols")? as u16 }
            }
            "agent.start" => Call::StartAgent { harness: s("harness")?, spec: s("spec")? },
            "agent.close" => Call::CloseAgent { pane: n("pane")? as u32 },
            "pane.input" => Call::Input { pane: n("pane")? as u32, bytes: bytes("bytes")? },
            "pane.resize" => Call::Resize {
                pane: n("pane")? as u32,
                rows: n("rows")? as u16,
                cols: n("cols")? as u16,
            },
            "stage" => Call::Stage {
                pane: n("pane")? as u32,
                bytes: bytes("bytes")?,
                how: handoff_of(p.get("how")?)?,
                what: s("what")?,
                why: s("why")?,
            },
            "act" => Call::Act {
                act: s("act")?,
                unit: s("unit"),
                pane: n("pane").map(|n| n as u32),
                text: s("text"),
            },
            "unit.confirm" => Call::ConfirmAsk { unit: s("unit")? },
            "read" => Call::Read { what: s("what")?, unit: s("unit")? },
            "readiness.setup" => Call::SetUp { harness: s("harness")? },
            "agents.available" => Call::Available,
            "pending.resolve" => {
                Call::Resolve { pending: s("pending")?, yes: p.get("yes")?.as_bool()? }
            }
            "detach" => Call::Detach,
            "shutdown" => Call::Shutdown,
            _ => return None,
        })
    }
}

impl Event {
    fn parts(&self) -> (&'static str, Value) {
        match self {
            Event::Panes { panes } => ("panes", json!({"panes": panes_json(panes)})),
            Event::Output { pane, bytes } => {
                ("pane.output", json!({"pane": pane, "bytes": b64(bytes)}))
            }
            Event::Added { pane, harness, spec } => {
                ("pane.added", json!({"pane": pane, "harness": harness, "spec": spec}))
            }
            Event::Exited { pane } => ("pane.exited", json!({"pane": pane})),
            Event::Units { units, records } => {
                ("units", json!({"units": units, "records": records}))
            }
            Event::Pending { id, pane, what, why, payload } => (
                "pending.added",
                json!({"id": id, "pane": pane, "what": what, "why": why,
                       "payload": b64(payload)}),
            ),
            Event::Resolved { id, yes } => ("pending.resolved", json!({"id": id, "yes": yes})),
            Event::Injected { pane, refusal } => {
                ("injected", json!({"pane": pane, "refusal": refusal}))
            }
            Event::Readiness { states } => ("readiness", json!({"states": states})),
        }
    }

    fn of(method: &str, p: &Value) -> Option<Self> {
        let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| p.get(k).and_then(Value::as_u64);
        Some(match method {
            "panes" => Event::Panes { panes: panes_of(p.get("panes")?)? },
            "pane.output" => {
                Event::Output { pane: n("pane")? as u32, bytes: un_b64(p.get("bytes")?.as_str()?)? }
            }
            "pane.added" => {
                Event::Added { pane: n("pane")? as u32, harness: s("harness")?, spec: s("spec")? }
            }
            "pane.exited" => Event::Exited { pane: n("pane")? as u32 },
            "units" => Event::Units {
                units: serde_json::from_value(p.get("units")?.clone()).ok()?,
                records: p.get("records").cloned().unwrap_or_else(|| json!({})),
            },
            "pending.added" => Event::Pending {
                id: s("id")?,
                pane: n("pane")? as u32,
                what: s("what")?,
                why: s("why")?,
                payload: un_b64(p.get("payload")?.as_str()?)?,
            },
            "pending.resolved" => Event::Resolved { id: s("id")?, yes: p.get("yes")?.as_bool()? },
            "readiness" => Event::Readiness { states: p.get("states")?.clone() },
            "injected" => Event::Injected {
                pane: n("pane")? as u32,
                refusal: p.get("refusal")?.as_str().map(str::to_string),
            },
            _ => return None,
        })
    }
}

/// A pane's bytes, in the shape `pane.output` uses.
pub fn output_json(pane: u32, bytes: &[u8]) -> Value {
    json!({"pane": pane, "bytes": b64(bytes)})
}

/// Reads one back.
pub fn output_of(v: &Value) -> Option<(u32, Vec<u8>)> {
    Some((v.get("pane")?.as_u64()? as u32, un_b64(v.get("bytes")?.as_str()?)?))
}

/// Panes as they go on the wire. One shape, whether in a result or an event.
pub fn panes_json(panes: &[PaneInfo]) -> Value {
    Value::Array(
        panes
            .iter()
            .map(|p| {
                json!({"pane": p.pane, "harness": p.harness, "spec": p.spec, "running": p.running})
            })
            .collect(),
    )
}

pub fn panes_of(v: &Value) -> Option<Vec<PaneInfo>> {
    v.as_array()?
        .iter()
        .map(|p| {
            Some(PaneInfo {
                pane: p.get("pane")?.as_u64()? as u32,
                harness: p.get("harness")?.as_str()?.to_string(),
                spec: p.get("spec")?.as_str()?.to_string(),
                running: p.get("running")?.as_bool()?,
            })
        })
        .collect()
}

impl Line {
    /// One line, newline included.
    pub fn encode(&self) -> Vec<u8> {
        let v = match self {
            Line::Call { id, call } => {
                let (method, params) = call.parts();
                json!({"id": id, "method": method, "params": params})
            }
            Line::Result { id, result } => json!({"id": id, "result": result}),
            Line::Error { id, code, message } => {
                json!({"id": id, "error": {"code": code, "message": message}})
            }
            Line::Event(e) => {
                let (method, params) = e.parts();
                json!({"method": method, "params": params})
            }
        };
        let mut out = serde_json::to_vec(&v).unwrap_or_default();
        out.push(b'\n');
        out
    }

    pub fn decode(line: &[u8]) -> Option<Self> {
        let v: Value = serde_json::from_slice(line).ok()?;
        let id = v.get("id").and_then(Value::as_u64);
        if let Some(err) = v.get("error") {
            return Some(Line::Error {
                id: id?,
                code: err.get("code")?.as_str()?.to_string(),
                message: err.get("message")?.as_str()?.to_string(),
            });
        }
        if let Some(result) = v.get("result") {
            return Some(Line::Result { id: id?, result: result.clone() });
        }
        let method = v.get("method")?.as_str()?;
        let params = v.get("params").cloned().unwrap_or_else(|| json!({}));
        match id {
            Some(id) => Some(Line::Call { id, call: Call::of(method, &params)? }),
            None => Some(Line::Event(Event::of(method, &params)?)),
        }
    }
}

// --- where it listens --------------------------------------------------------
/// Where a project's session listens. One session per project directory.
/// A Unix socket path is length-limited by `sun_path` — 104 bytes on macOS,
/// 108 on Linux — and the limit is on the path, not on the name. A deep
/// `TMPDIR`, which an isolated host environment routinely has, overruns it.
const SUN_LEN: usize = 100;

/// Where the daemon listens. One per machine (ADR-0007), so one name.
pub fn socket_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    socket_path_in(&base, "weft.sock".to_string())
}

fn socket_path_in(base: &Path, name: String) -> PathBuf {
    let chosen = base.join("weft").join(&name);
    if chosen.as_os_str().len() <= SUN_LEN {
        return chosen;
    }
    // Too deep to bind. `/tmp` is the one directory that is always short.
    PathBuf::from("/tmp").join("weft").join(name)
}

/// A socket of one's own, for a test or a probe that starts its own daemon.
///
/// The real one is per machine, which is the point of it — so anything that
/// wants a daemon to itself has to say so rather than quietly taking the
/// machine's and fighting whatever else is running.
pub fn private_socket(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    socket_path_in(
        &std::env::temp_dir(),
        format!("{name}-{}-{n}.sock", std::process::id()),
    )
}

/// Is a session already listening there?
pub fn is_live(socket: &Path) -> bool {
    socket.exists() && UnixStream::connect(socket).is_ok()
}

/// Drop a stale socket so a new server can bind.
pub fn clear_dead(socket: &Path) {
    if socket.exists() && UnixStream::connect(socket).is_err() {
        let _ = std::fs::remove_file(socket);
    }
}

pub fn send(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    w.write_all(bytes)?;
    w.flush()
}

/// Lines off a socket, one JSON object each.
pub struct Lines<R> {
    inner: BufReader<R>,
}

impl<R: Read> Lines<R> {
    pub fn new(inner: R) -> Self {
        Self { inner: BufReader::new(inner) }
    }

    /// The next message, or `None` at the end. A line that does not parse is
    /// skipped: one client's bad message is not the session's problem.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> io::Result<Option<Line>> {
        loop {
            let mut buf = Vec::new();
            let n = self.inner.by_ref().take(MAX_LINE as u64).read_until(b'\n', &mut buf)?;
            if n == 0 {
                return Ok(None);
            }
            if let Some(line) = Line::decode(&buf) {
                return Ok(Some(line));
            }
        }
    }
}

#[cfg(test)]
mod where_it_listens {
    use super::*;

    #[test]
    fn one_machine_has_one_socket() {
        // It used to be one per project, with the project hashed into the
        // name. One daemon holds every project now (ADR-0007), and a client
        // says which it wants when it attaches.
        assert_eq!(socket_path(), socket_path(), "stable");
        assert!(socket_path().to_string_lossy().ends_with("weft.sock"));
    }

    #[test]
    fn a_deep_tmpdir_still_yields_a_path_a_socket_can_bind() {
        // Found by the W3 probe: an isolated host environment puts TMPDIR deep
        // enough that the socket path overran sun_path and no session started.
        let deep = std::path::Path::new(
            "/Users/someone/Documents/works/fab7/sandbox/hostlab/hosts/claude/g30/runtime/tmp",
        );
        let path = socket_path_in(deep, "7e1ede271e5eceef.sock".into());
        assert!(
            path.as_os_str().len() <= SUN_LEN,
            "{} is {} bytes",
            path.display(),
            path.as_os_str().len()
        );
    }

    #[test]
    fn a_socket_nothing_is_listening_on_is_not_a_live_session() {
        let path = std::env::temp_dir().join(format!("weft-dead-{}.sock", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        assert!(!is_live(&path), "a leftover file is not a server");
        clear_dead(&path);
        assert!(!path.exists(), "and it is cleared out of the way");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(line: Line) {
        let wire = line.encode();
        assert_eq!(*wire.last().expect("a line"), b'\n', "one line, newline ended");
        assert_eq!(Line::decode(&wire), Some(line));
    }

    #[test]
    fn every_call_survives_the_wire() {
        for call in [
            Call::Hello { client: "weft-tui/0.1".into(), protocol: PROTOCOL },
            Call::Open { path: "/home/me/work/a thing".into(), rows: 40, cols: 120 },
            Call::StartAgent { harness: "codex".into(), spec: "codex resume 01a0".into() },
            Call::CloseAgent { pane: 3 },
            // Arbitrary bytes, including what JSON cannot hold as text.
            Call::Input { pane: 2, bytes: vec![0x00, 0x1b, 0xff, b'a'] },
            Call::Resize { pane: 1, rows: 24, cols: 80 },
            Call::Resolve { pending: "pnd_1".into(), yes: false },
            Call::Act {
                act: "send".into(),
                unit: Some("ask_1".into()),
                pane: None,
                text: None,
            },
            Call::Act { act: "ask".into(), unit: None, pane: Some(1), text: Some("do it".into()) },
            Call::ConfirmAsk { unit: "ask_1".into() },
            Call::Read { what: "judges".into(), unit: "ask_1".into() },
            Call::SetUp { harness: "codex".into() },
            Call::Available,
            Call::Detach,
            Call::Shutdown,
        ] {
            roundtrip(Line::Call { id: 7, call });
        }
    }

    #[test]
    fn a_staged_prompt_carries_its_handoff() {
        for how in [
            Handoff::Whole,
            Handoff::TypeCommand { command: "/goal".into(), at: 6 },
            Handoff::EnterMode { command: "/plan".into(), at: 6, active: "Plan mode".into() },
        ] {
            roundtrip(Line::Call {
                id: 1,
                call: Call::Stage {
                    pane: 0,
                    bytes: b"/plan ship it".to_vec(),
                    how,
                    what: "Ready to send to codex".into(),
                    why: "This is the exact wording.".into(),
                },
            });
        }
    }

    #[test]
    fn every_event_survives_the_wire() {
        for event in [
            Event::Panes {
                panes: vec![PaneInfo {
                    pane: 0,
                    harness: "codex".into(),
                    spec: "codex resume 01a0".into(),
                    running: true,
                }],
            },
            Event::Output { pane: 0, bytes: vec![0x1b, b'[', b'2', b'J', 0xfe] },
            Event::Added { pane: 1, harness: "claude-code".into(), spec: "claude".into() },
            Event::Exited { pane: 1 },
            Event::Units { units: Vec::new(), records: json!({}) },
            Event::Pending {
                id: "pnd_1".into(),
                pane: 2,
                what: "Ready to send".into(),
                why: "the exact wording".into(),
                payload: b"/plan ship it".to_vec(),
            },
            Event::Resolved { id: "pnd_1".into(), yes: true },
            Event::Injected { pane: 0, refusal: Some("PaneBlocked".into()) },
            Event::Injected { pane: 0, refusal: None },
            Event::Readiness { states: json!({"codex": "ready"}) },
        ] {
            roundtrip(Line::Event(event));
        }
    }

    #[test]
    fn an_answer_is_a_result_or_an_error_and_carries_its_id() {
        roundtrip(Line::Result { id: 9, result: json!({"pending": "pnd_2"}) });
        roundtrip(Line::Error {
            id: 9,
            code: "no_pane".into(),
            message: "No codex pane is open here.".into(),
        });
    }

    #[test]
    fn base64_carries_any_byte_at_any_length() {
        for len in 0..=32usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 % 256) as u8).collect();
            assert_eq!(un_b64(&b64(&bytes)).as_deref(), Some(&bytes[..]), "{len} bytes");
        }
        assert_eq!(b64(b"Man"), "TWFu");
        assert_eq!(b64(b"Ma"), "TWE=");
        assert_eq!(b64(b"M"), "TQ==");
    }

    #[test]
    fn a_line_that_does_not_parse_is_skipped_rather_than_fatal() {
        let mut wire = b"not json\n".to_vec();
        wire.extend(Line::Event(Event::Exited { pane: 4 }).encode());
        let mut lines = Lines::new(&wire[..]);
        assert_eq!(lines.next().unwrap(), Some(Line::Event(Event::Exited { pane: 4 })));
        assert_eq!(lines.next().unwrap(), None);
    }

    #[test]
    fn a_method_this_daemon_does_not_know_is_not_a_guess() {
        assert_eq!(Line::decode(br#"{"id":1,"method":"unit.teleport","params":{}}"#), None);
        assert_eq!(Line::decode(br#"{"method":"nothing.happened","params":{}}"#), None);
    }
}
