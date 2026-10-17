//! The socket API.
//!
//! One NDJSON line per message. The desktop app is a client of this and so is
//! everything else; if the app needs something this cannot express, that is a
//! gap here rather than a reason for a private path.
//!
//! **It speaks in decisions, never in keystrokes.** `pane.input` is the one
//! exception: a client showing a pane forwards the person's own typing into
//! it. Everything that would type on the person's behalf goes through a
//! pending, which is shown before anything is written.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
pub use weft_core::board::PaneInfo;
use weft_core::inject::Handoff;
use weft_core::ledger::Unit;
use weft_core::turns::Turn;

/// What this daemon speaks. A client that asks for another is refused with a
/// number rather than left to guess.
pub const PROTOCOL: u64 = 6;

/// Refuse anything absurd rather than allocating on a corrupt length.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// A call, from a client. Every one gets a `result` or an `error`.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// Version handshake, first.
    Hello {
        client: String,
        protocol: u64,
    },
    /// Watch a project. The daemon holds many; a client sees one. `rows` and
    /// `cols` are the size an agent this window asks for starts at, until it
    /// draws one at another.
    Open {
        path: String,
        rows: u16,
        cols: u16,
        /// `output: "raw"` (the default) streams every pane's bytes as
        /// `pane.output`; `"none"` never does, for a client that draws
        /// screens instead (remote.md D4).
        raw: bool,
    },
    /// Start a harness, fresh or resuming a recorded session. A resumed
    /// agent names its session, so the daemon knows it from the start.
    /// Answers with `{"pane": id}`.
    StartAgent {
        harness: String,
        spec: String,
        session: Option<String>,
    },
    /// Stop an agent and take its pane away.
    CloseAgent {
        pane: u32,
    },
    /// A person's own typing, into a pane they are showing. Not an act.
    Input {
        pane: u32,
        bytes: Vec<u8>,
    },
    Resize {
        pane: u32,
        rows: u16,
        cols: u16,
    },
    /// `form: "screen"` sends this client the pane's rendered screen as
    /// `pane.screen` frames, at most ten a second; `"none"` stops them.
    /// Watching resizes nothing (remote.md D2).
    Attach {
        pane: u32,
        form: String,
    },
    /// What to suggest in the pane's message field: the rest of `draft`, or,
    /// when it is empty, the next prompt. Answers `{text, score}` or nothing.
    Suggest {
        pane: u32,
        draft: String,
    },
    /// Put a prompt in front of the person. Answers with its pending id;
    /// nothing is typed until that is resolved.
    Stage {
        pane: u32,
        bytes: Vec<u8>,
        how: Handoff,
        what: String,
        why: String,
    },
    /// One of RingFrame's three acts, by name. The daemon works out what to
    /// type, where it goes, and how it has to be typed; a client that
    /// assembled bytes would be re-deriving all of that. Answers with a
    /// pending.
    Act {
        act: String,
        unit: Option<String>,
        pane: Option<u32>,
        text: Option<String>,
        /// The Eval a Seal seals (`decide`): its round, and only its Asks.
        eval: Option<String>,
        /// An Ask or follow-up into a new agent of this harness, started for
        /// it, rather than into `pane`.
        start: Option<String>,
    },
    /// Record the person's yes for an Ask whose chooser never got one.
    ConfirmAsk {
        unit: String,
    },
    /// The exact wording, or the judgements, as RingFrame recorded them; or
    /// `what: "history"`, a pane's last `lines` of scrollback and screen as
    /// text (remote.md D7), where `unit` is empty.
    Read {
        what: String,
        unit: String,
        pane: Option<u32>,
        lines: Option<u64>,
        /// A conversation's cursors: the page before one, or what was
        /// added after one.
        before: Option<String>,
        after: Option<String>,
    },
    /// The RingFrame view (`[U]`): `look` at RingFrame's configuration and
    /// every harness file, `locate` one whose program is at `path`, `run` one's
    /// setup or update, or `all` that is behind. Answered by `Event::Setup`,
    /// again at every step.
    Setup {
        act: String,
        harness: String,
        path: String,
    },
    /// The agents that could be started here: fresh, or picking up a session
    /// RingFrame has a receipt for.
    Available,
    /// Turn this project's turbo mode on or off. Agents started from now on
    /// follow it; one already running keeps the mode it was started in.
    /// Answers with `{"turbo": bool}`.
    Turbo {
        on: bool,
    },
    /// `weft remote`'s view, from the client whose hello names it
    /// `weft-remote`: kept, and sent to every client as `remote` (remote.md D8).
    RemoteView {
        view: Value,
    },
    /// An act for `weft remote` (`{act, …}`), from any other client: passed
    /// to it unchanged as `remote.act` (remote.md D8).
    Remote {
        act: Value,
    },
    /// Turn Weft's own "Start at login" on or off: the
    /// service runs `program --serve` with `path` as its `PATH`, the window's
    /// when it was turned on. Answers with `{"start_at_login": bool}`.
    StartAtLogin {
        on: bool,
        program: String,
        path: String,
    },
    /// Yes or no, by id. Whoever answers first answers for everyone.
    Resolve {
        pending: String,
        yes: bool,
        /// The person saw what Weft read off the screen and said to type it
        /// anyway. The inference is Weft's; the decision is theirs.
        force: bool,
        /// A send of an Ask typed into this pane instead of the one it was
        /// staged for, or into a new agent of this harness, started for it.
        to: Option<u32>,
        start: Option<String>,
    },
    /// A newer Weft is installed in `versions/<version>`: the
    /// daemon starts again on it once no agent is running. Needs no hello,
    /// as `shutdown`, so the `weft update` of any version can say it.
    Update {
        version: String,
    },
    /// Close a project reopened after an unclean end, from a device or a
    /// window that has nothing to do in it. It closes as any
    /// project does once nobody is in it.
    Dismiss,
    /// Leave, without stopping anything.
    Detach,
    /// Stop every agent and end the session.
    Shutdown,
}

/// Something that happened. Unsolicited, to every client of that project.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The panes of the project just opened, in order.
    Panes {
        panes: Vec<PaneInfo>,
    },
    /// What a harness printed, as chunk `seq` of its pane.
    Output {
        pane: u32,
        seq: u64,
        bytes: Vec<u8>,
    },
    /// A pane's screen, to the one client that attached to it: escape codes
    /// that draw the whole screen (`full`) or turn the last frame into this
    /// one, as of the pane's chunk `seq` (remote.md D3).
    Screen {
        pane: u32,
        seq: u64,
        full: bool,
        rows: u16,
        cols: u16,
        bytes: Vec<u8>,
    },
    Added {
        pane: u32,
        harness: String,
        spec: String,
        /// Started in turbo mode.
        turbo: bool,
    },
    Exited {
        pane: u32,
    },
    /// The board, the Eval record behind each check, and the board grouped
    /// by Eval round. Sent on opening a project and whenever the
    /// ledger moves, so nothing reads a file to draw a frame.
    Units {
        units: Vec<Unit>,
        records: Value,
        rounds: weft_core::rounds::Rounds,
    },
    /// Something is waiting on a person. Every attached client is told.
    Pending {
        id: String,
        pane: u32,
        what: String,
        why: String,
        payload: Vec<u8>,
        /// The Ask whose prompt this sends, when it sends one: it can go to
        /// another agent, or a new one (`pending.resolve`'s `to`, `start`).
        sends: Option<String>,
    },
    /// It was answered. Once.
    Resolved {
        id: String,
        yes: bool,
    },
    /// A prompt was typed, or was not, and why not.
    Injected {
        pane: u32,
        refusal: Option<String>,
        /// It was typed and sent, and RingFrame would not record that it
        /// went: why, in RingFrame's words.
        unrecorded: Option<String>,
    },
    /// What each harness is short of here, by the name RingFrame records.
    Readiness {
        states: Value,
    },
    /// The RingFrame view, as it stands.
    Setup {
        view: Value,
    },
    /// Whether RingFrame is set up in this project, what it still needs if
    /// not, and why an Ask could not finish here, asked again with it.
    Workspace {
        setup: Option<weft_core::workspace::Needs>,
        gap: Option<String>,
    },
    /// `weft remote`'s view, as it last published it.
    Remote {
        view: Value,
    },
    /// An act a client asked of `weft remote`, as it was asked; sent only to
    /// `weft remote`.
    RemoteAct {
        act: Value,
    },
    /// The person typed into Weft on this machine, at `at` (milliseconds):
    /// sent to every client, at most once in ten seconds (remote.md D6).
    /// A project's turbo switch changed: agents started from now on run so.
    Turbo {
        on: bool,
    },
    /// Weft's "Start at login" was turned on or off, to every client.
    StartAtLogin {
        on: bool,
    },
    /// A newer Weft is installed and waits for no agent to be running, or
    /// none does.
    Update {
        version: Option<String>,
    },
    /// The project was reopened after an unclean end at `since`
    /// (milliseconds), or no longer only that: someone acted in it.
    Reopened {
        since: Option<i64>,
    },
    Presence {
        at: i64,
    },
    /// Each agent's state, as its hooks reported it: one per pane, in order;
    /// the project's session index (sessions.md); and the Mac's
    /// distance from UTC, in milliseconds, so every client reads a time and
    /// a date group as the Mac does.
    Agents {
        panes: Vec<Option<Turn>>,
        sessions: Vec<weft_core::sessions::Row>,
        zone: i64,
    },
    /// One hook, from `weft signal` to the daemon: sent one way
    /// on a connection of its own, and answered by nothing.
    Signal(Box<weft_core::signal::Sent>),
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
            out.push(if i <= c.len() {
                ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
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
            Call::Open { path, rows, cols, raw } => (
                "project.open",
                json!({"path": path, "rows": rows, "cols": cols,
                       "output": if *raw { "raw" } else { "none" }}),
            ),
            Call::StartAgent { harness, spec, session } => {
                ("agent.start", json!({"harness": harness, "spec": spec, "session": session}))
            }
            Call::CloseAgent { pane } => ("agent.close", json!({"pane": pane})),
            Call::Input { pane, bytes } => {
                ("pane.input", json!({"pane": pane, "bytes": b64(bytes)}))
            }
            Call::Resize { pane, rows, cols } => {
                ("pane.resize", json!({"pane": pane, "rows": rows, "cols": cols}))
            }
            Call::Attach { pane, form } => ("pane.attach", json!({"pane": pane, "form": form})),
            Call::Suggest { pane, draft } => {
                ("pane.suggest", json!({"pane": pane, "draft": draft}))
            }
            Call::Stage { pane, bytes, how, what, why } => (
                "stage",
                json!({"pane": pane, "bytes": b64(bytes), "how": handoff_json(how),
                       "what": what, "why": why}),
            ),
            Call::Act { act, unit, pane, text, eval, start } => (
                "act",
                json!({"act": act, "unit": unit, "pane": pane, "text": text, "eval": eval,
                       "start": start}),
            ),
            Call::ConfirmAsk { unit } => ("unit.confirm", json!({"unit": unit})),
            Call::Read { what, unit, pane, lines, before, after } => (
                "read",
                json!({"what": what, "unit": unit, "pane": pane, "lines": lines,
                       "before": before, "after": after}),
            ),
            Call::Setup { act, harness, path } => {
                ("setup", json!({"act": act, "harness": harness, "path": path}))
            }
            Call::Available => ("agents.available", json!({})),
            Call::Turbo { on } => ("project.turbo", json!({"on": on})),
            Call::RemoteView { view } => ("remote.view", json!({"view": view})),
            Call::Remote { act } => ("remote", act.clone()),
            Call::StartAtLogin { on, program, path } => {
                ("service.login", json!({"on": on, "program": program, "path": path}))
            }
            Call::Resolve { pending, yes, force, to, start } => (
                "pending.resolve",
                json!({"pending": pending, "yes": yes, "force": force, "to": to, "start": start}),
            ),
            Call::Detach => ("detach", json!({})),
            Call::Dismiss => ("project.dismiss", json!({})),
            Call::Update { version } => ("update", json!({"version": version})),
            Call::Shutdown => ("shutdown", json!({})),
        }
    }

    fn of(method: &str, p: &Value) -> Option<Self> {
        let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| p.get(k).and_then(Value::as_u64);
        let bytes = |k: &str| un_b64(p.get(k)?.as_str()?);
        Some(match method {
            "hello" => Call::Hello { client: s("client")?, protocol: n("protocol")? },
            "project.open" => Call::Open {
                path: s("path")?,
                rows: n("rows")? as u16,
                cols: n("cols")? as u16,
                raw: match s("output").as_deref() {
                    None | Some("raw") => true,
                    Some("none") => false,
                    Some(_) => return None,
                },
            },
            "agent.start" => {
                Call::StartAgent { harness: s("harness")?, spec: s("spec")?, session: s("session") }
            }
            "agent.close" => Call::CloseAgent { pane: n("pane")? as u32 },
            "pane.input" => Call::Input { pane: n("pane")? as u32, bytes: bytes("bytes")? },
            "pane.resize" => Call::Resize {
                pane: n("pane")? as u32,
                rows: n("rows")? as u16,
                cols: n("cols")? as u16,
            },
            "pane.attach" => Call::Attach { pane: n("pane")? as u32, form: s("form")? },
            "pane.suggest" => Call::Suggest { pane: n("pane")? as u32, draft: s("draft")? },
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
                eval: s("eval"),
                start: s("start"),
            },
            "unit.confirm" => Call::ConfirmAsk { unit: s("unit")? },
            "read" => Call::Read {
                what: s("what")?,
                unit: s("unit").unwrap_or_default(),
                pane: n("pane").map(|n| n as u32),
                lines: n("lines"),
                before: s("before"),
                after: s("after"),
            },
            "setup" => Call::Setup {
                act: s("act")?,
                harness: s("harness").unwrap_or_default(),
                path: s("path").unwrap_or_default(),
            },
            "agents.available" => Call::Available,
            "project.turbo" => Call::Turbo { on: p.get("on")?.as_bool()? },
            "remote.view" => Call::RemoteView { view: p.get("view")?.clone() },
            "remote" => {
                p.get("act")?.as_str()?;
                Call::Remote { act: p.clone() }
            }
            "service.login" => Call::StartAtLogin {
                on: p.get("on")?.as_bool()?,
                program: s("program").unwrap_or_default(),
                path: s("path").unwrap_or_default(),
            },
            "pending.resolve" => {
                Call::Resolve {
                    pending: s("pending")?,
                    yes: p.get("yes")?.as_bool()?,
                    // Absent from an older client, which never forced.
                    force: p.get("force").and_then(Value::as_bool).unwrap_or(false),
                    to: n("to").map(|n| n as u32),
                    start: s("start"),
                }
            }
            "detach" => Call::Detach,
            "project.dismiss" => Call::Dismiss,
            "update" => Call::Update { version: s("version")? },
            "shutdown" => Call::Shutdown,
            _ => return None,
        })
    }
}

impl Event {
    fn parts(&self) -> (&'static str, Value) {
        match self {
            Event::Panes { panes } => ("panes", json!({"panes": panes_json(panes)})),
            Event::Output { pane, seq, bytes } => {
                ("pane.output", json!({"pane": pane, "seq": seq, "bytes": b64(bytes)}))
            }
            Event::Screen { pane, seq, full, rows, cols, bytes } => (
                "pane.screen",
                json!({"pane": pane, "seq": seq, "full": full, "rows": rows, "cols": cols,
                       "bytes": b64(bytes)}),
            ),
            Event::Added { pane, harness, spec, turbo } => (
                "pane.added",
                json!({"pane": pane, "harness": harness, "spec": spec, "turbo": turbo}),
            ),
            Event::Exited { pane } => ("pane.exited", json!({"pane": pane})),
            Event::Units { units, records, rounds } => {
                ("units", json!({"units": units, "records": records, "rounds": rounds}))
            }
            Event::Pending { id, pane, what, why, payload, sends } => {
                ("pending.added", pending_json(id, *pane, what, why, payload, sends.as_deref()))
            }
            Event::Resolved { id, yes } => ("pending.resolved", json!({"id": id, "yes": yes})),
            Event::Injected { pane, refusal, unrecorded } => {
                ("injected", json!({"pane": pane, "refusal": refusal, "unrecorded": unrecorded}))
            }
            Event::Presence { at } => ("presence", json!({"at": at})),
            Event::Turbo { on } => ("turbo", json!({"on": on})),
            Event::StartAtLogin { on } => ("start_at_login", json!({"on": on})),
            Event::Reopened { since } => ("project.reopened", json!({"since": since})),
            Event::Update { version } => ("update.waiting", json!({"version": version})),
            Event::Remote { view } => ("remote", json!({"view": view})),
            Event::RemoteAct { act } => ("remote.act", act.clone()),
            Event::Readiness { states } => ("readiness", json!({"states": states})),
            Event::Setup { view } => ("setup", json!({"view": view})),
            Event::Workspace { setup, gap } => ("workspace", json!({"setup": setup, "gap": gap})),
            Event::Agents { panes, sessions, zone } => {
                ("agents", json!({"panes": panes, "sessions": sessions, "zone": zone}))
            }
            Event::Signal(sent) => ("signal", serde_json::to_value(sent).unwrap_or_default()),
        }
    }

    fn of(method: &str, p: &Value) -> Option<Self> {
        let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| p.get(k).and_then(Value::as_u64);
        Some(match method {
            "panes" => Event::Panes { panes: panes_of(p.get("panes")?)? },
            "pane.output" => Event::Output {
                pane: n("pane")? as u32,
                seq: n("seq")?,
                bytes: un_b64(p.get("bytes")?.as_str()?)?,
            },
            "pane.screen" => Event::Screen {
                pane: n("pane")? as u32,
                seq: n("seq")?,
                full: p.get("full")?.as_bool()?,
                rows: n("rows")? as u16,
                cols: n("cols")? as u16,
                bytes: un_b64(p.get("bytes")?.as_str()?)?,
            },
            "pane.added" => Event::Added {
                pane: n("pane")? as u32,
                harness: s("harness")?,
                spec: s("spec")?,
                turbo: p.get("turbo").and_then(Value::as_bool).unwrap_or(false),
            },
            "pane.exited" => Event::Exited { pane: n("pane")? as u32 },
            "units" => Event::Units {
                units: serde_json::from_value(p.get("units")?.clone()).ok()?,
                records: p.get("records").cloned().unwrap_or_else(|| json!({})),
                rounds: serde_json::from_value(p.get("rounds").cloned().unwrap_or_default())
                    .unwrap_or_default(),
            },
            "pending.added" => Event::Pending {
                id: s("id")?,
                pane: n("pane")? as u32,
                what: s("what")?,
                why: s("why")?,
                payload: un_b64(p.get("payload")?.as_str()?)?,
                sends: s("sends"),
            },
            "pending.resolved" => Event::Resolved { id: s("id")?, yes: p.get("yes")?.as_bool()? },
            "presence" => Event::Presence { at: p.get("at")?.as_i64()? },
            "turbo" => Event::Turbo { on: p.get("on")?.as_bool()? },
            "start_at_login" => Event::StartAtLogin { on: p.get("on")?.as_bool()? },
            "project.reopened" => Event::Reopened { since: p.get("since")?.as_i64() },
            "update.waiting" => {
                Event::Update { version: p.get("version")?.as_str().map(str::to_string) }
            }
            "remote" => Event::Remote { view: p.get("view")?.clone() },
            "remote.act" => {
                p.get("act")?.as_str()?;
                Event::RemoteAct { act: p.clone() }
            }
            "readiness" => Event::Readiness { states: p.get("states")?.clone() },
            "setup" => Event::Setup { view: p.get("view")?.clone() },
            "workspace" => Event::Workspace {
                setup: serde_json::from_value(p.get("setup")?.clone()).ok()?,
                gap: s("gap"),
            },
            "agents" => Event::Agents {
                panes: serde_json::from_value(p.get("panes")?.clone()).ok()?,
                sessions: serde_json::from_value(p.get("sessions")?.clone()).ok()?,
                zone: p.get("zone").and_then(Value::as_i64).unwrap_or(0),
            },
            "signal" => Event::Signal(Box::new(serde_json::from_value(p.clone()).ok()?)),
            "injected" => Event::Injected {
                pane: n("pane")? as u32,
                refusal: p.get("refusal")?.as_str().map(str::to_string),
                unrecorded: s("unrecorded"),
            },
            _ => return None,
        })
    }
}

/// A pane's screen as `project.open` gives it to a `raw` client: escape
/// codes that draw it, history and all, on a fresh terminal of `rows` ×
/// `cols`, as of the pane's chunk `seq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenScreen {
    pub pane: u32,
    pub seq: u64,
    pub rows: u16,
    pub cols: u16,
    pub bytes: Vec<u8>,
}

pub fn screen_json(s: &OpenScreen) -> Value {
    json!({"pane": s.pane, "seq": s.seq, "rows": s.rows, "cols": s.cols, "bytes": b64(&s.bytes)})
}

/// Reads one back.
pub fn screen_of(v: &Value) -> Option<OpenScreen> {
    let n = |k: &str| v.get(k).and_then(Value::as_u64);
    Some(OpenScreen {
        pane: n("pane")? as u32,
        seq: n("seq")?,
        rows: n("rows")? as u16,
        cols: n("cols")? as u16,
        bytes: un_b64(v.get("bytes")?.as_str()?)?,
    })
}

/// A pending as `project.open` lists it: the shape `pending.added` carries.
pub fn pending_json(
    id: &str,
    pane: u32,
    what: &str,
    why: &str,
    payload: &[u8],
    sends: Option<&str>,
) -> Value {
    json!({"id": id, "pane": pane, "what": what, "why": why, "payload": b64(payload),
           "sends": sends})
}

/// Reads one back, as the event it would have been.
pub fn pending_of(v: &Value) -> Option<Event> {
    Event::of("pending.added", v)
}

/// Panes as they go on the wire. One shape, whether in a result or an event.
pub fn panes_json(panes: &[PaneInfo]) -> Value {
    Value::Array(
        panes
            .iter()
            .map(|p| {
                json!({"pane": p.pane, "harness": p.harness, "spec": p.spec, "running": p.running,
                       "turbo": p.turbo})
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
                turbo: p.get("turbo").and_then(Value::as_bool).unwrap_or(false),
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

/// Where the daemon listens. One per machine, so one name.
pub fn socket_path() -> PathBuf {
    let base =
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
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
///
/// It is alone in a folder of its own, so what a daemon keeps beside its
/// configuration (`projects.json`, receipts) is that daemon's alone: a daemon
/// reopens the projects its folder lists, never another's.
pub fn private_socket(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = format!("{name}-{}-{n}", std::process::id());
    let socket = socket_path_in(&std::env::temp_dir(), format!("{dir}/d.sock"));
    if let Some(folder) = socket.parent() {
        // Process ids come round again: what an earlier run left in a folder
        // of this name was another daemon's.
        let _ = std::fs::remove_dir_all(folder);
        let _ = std::fs::create_dir_all(folder);
    }
    socket
}

/// Is a session already listening there?
pub fn is_live(socket: &Path) -> bool {
    socket.exists() && UnixStream::connect(socket).is_ok()
}

/// Stop the daemon at `socket`, whatever protocol it speaks. `shutdown`
/// needs no hello, so a newer `weft` stops an older daemon it could not
/// otherwise talk to, as an upgrade does before starting the new one.
pub fn stop(socket: &Path) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket)?;
    send(&mut stream, &Line::Call { id: 1, call: Call::Shutdown }.encode())?;
    // Its whole answer, or the connection closing as it stops.
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let _ = BufReader::new(stream).read_line(&mut String::new());
    Ok(())
}

/// Tell the daemon at `socket` that Weft `version` is installed beside it.
/// Like `stop`, it needs no hello, so an older daemon is told by a newer
/// `weft update`. Its refusal, when it refused.
pub fn tell_update(socket: &Path, version: &str) -> io::Result<Result<(), String>> {
    let mut stream = UnixStream::connect(socket)?;
    let call = Call::Update { version: version.into() };
    send(&mut stream, &Line::Call { id: 1, call }.encode())?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(match Line::decode(line.as_bytes()) {
        Some(Line::Error { message, .. }) => Err(message),
        _ => Ok(()),
    })
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
        // name. One daemon holds every project now, and a client says which
        // it wants when it attaches.
        assert_eq!(socket_path(), socket_path(), "stable");
        assert!(socket_path().to_string_lossy().ends_with("weft.sock"));
    }

    #[test]
    fn a_deep_tmpdir_still_yields_a_path_a_socket_can_bind() {
        // Found by the W3 probe: an isolated host environment puts TMPDIR deep
        // enough that the socket path overran sun_path and no session started.
        let deep = std::path::Path::new(
            "/Users/someone/Documents/projects/example/sandbox/hosts/harness/run30/runtime/tmp",
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
            Call::Hello { client: "weft-desktop/0.1".into(), protocol: PROTOCOL },
            Call::Open { path: "/home/me/work/a thing".into(), rows: 40, cols: 120, raw: true },
            Call::Open { path: "/home/me/work".into(), rows: 40, cols: 120, raw: false },
            Call::StartAgent {
                harness: "codex".into(),
                spec: "codex resume 01a0".into(),
                session: Some("01a0".into()),
            },
            Call::StartAgent { harness: "codex".into(), spec: "codex".into(), session: None },
            Call::CloseAgent { pane: 3 },
            // Arbitrary bytes, including what JSON cannot hold as text.
            Call::Input { pane: 2, bytes: vec![0x00, 0x1b, 0xff, b'a'] },
            Call::Resize { pane: 1, rows: 24, cols: 80 },
            Call::Attach { pane: 1, form: "screen".into() },
            Call::Attach { pane: 1, form: "none".into() },
            Call::Suggest { pane: 1, draft: "run the lo".into() },
            Call::Suggest { pane: 2, draft: String::new() },
            Call::Resolve {
                pending: "pnd_1".into(),
                yes: false,
                force: false,
                to: None,
                start: None,
            },
            Call::Resolve {
                pending: "pnd_1".into(),
                yes: true,
                force: false,
                to: Some(3),
                start: None,
            },
            Call::Resolve {
                pending: "pnd_1".into(),
                yes: true,
                force: false,
                to: None,
                start: Some("codex".into()),
            },
            Call::Act {
                act: "send".into(),
                unit: Some("ask_1".into()),
                pane: None,
                text: None,
                eval: None,
                start: None,
            },
            Call::Act {
                act: "ask".into(),
                unit: None,
                pane: Some(1),
                text: Some("do it".into()),
                eval: None,
                start: None,
            },
            Call::Act {
                act: "ask".into(),
                unit: None,
                pane: None,
                text: Some("do it".into()),
                eval: None,
                start: Some("codex".into()),
            },
            Call::Act {
                act: "decide".into(),
                unit: None,
                pane: None,
                text: None,
                eval: Some("evl_1".into()),
                start: None,
            },
            Call::ConfirmAsk { unit: "ask_1".into() },
            Call::Read {
                what: "judges".into(),
                unit: "ask_1".into(),
                pane: None,
                lines: None,
                before: None,
                after: None,
            },
            Call::Read {
                what: "history".into(),
                unit: String::new(),
                pane: Some(2),
                lines: Some(40),
                before: None,
                after: None,
            },
            Call::Setup {
                act: "locate".into(),
                harness: "codex".into(),
                path: "/opt/codex".into(),
            },
            Call::Available,
            Call::Turbo { on: true },
            Call::RemoteView { view: json!({"state": "off"}) },
            Call::Remote { act: json!({"act": "allow", "device": "d_1", "yes": true}) },
            Call::StartAtLogin { on: true, program: "/w/weft".into(), path: "/usr/bin".into() },
            Call::Dismiss,
            Call::Update { version: "0.2.1".into() },
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
                    turbo: true,
                }],
            },
            Event::Output { pane: 0, seq: 7, bytes: vec![0x1b, b'[', b'2', b'J', 0xfe] },
            Event::Screen {
                pane: 0,
                seq: 41,
                full: true,
                rows: 24,
                cols: 80,
                bytes: vec![0x1b, b'[', b'H', 0xe6, 0xbc, 0xa2],
            },
            Event::Added {
                pane: 1,
                harness: "claude-code".into(),
                spec: "claude".into(),
                turbo: false,
            },
            Event::Exited { pane: 1 },
            Event::Units { units: Vec::new(), records: json!({}), rounds: Default::default() },
            Event::Units {
                units: Vec::new(),
                records: json!({}),
                rounds: weft_core::rounds::Rounds {
                    open: vec!["ask_2".into()],
                    since: Some(1),
                    ..Default::default()
                },
            },
            Event::Pending {
                id: "pnd_1".into(),
                pane: 2,
                what: "Ready to send".into(),
                why: "the exact wording".into(),
                payload: b"/plan ship it".to_vec(),
                sends: Some("ask_1".into()),
            },
            Event::Resolved { id: "pnd_1".into(), yes: true },
            Event::Presence { at: 1_790_000_000_000 },
            Event::Turbo { on: true },
            Event::StartAtLogin { on: false },
            Event::Reopened { since: Some(1_790_000_000_000) },
            Event::Reopened { since: None },
            Event::Update { version: Some("0.2.1".into()) },
            Event::Update { version: None },
            Event::Remote { view: json!({"state": "on"}) },
            Event::RemoteAct { act: json!({"act": "stop"}) },
            Event::Injected { pane: 0, refusal: Some("NoProcess".into()), unrecorded: None },
            Event::Injected { pane: 0, refusal: None, unrecorded: Some("ledger.io".into()) },
            Event::Readiness { states: json!({"codex": "ready"}) },
            Event::Setup { view: json!({"rows": []}) },
            Event::Workspace {
                setup: Some(weft_core::workspace::Needs::refused(Some("workspace.no_commit"))),
                gap: Some("no commit yet".into()),
            },
            Event::Workspace { setup: None, gap: None },
            Event::Agents {
                panes: vec![Some(Turn::TurnEnded), None],
                sessions: vec![weft_core::sessions::Row {
                    id: "s1".into(),
                    harness: "claude-code".into(),
                    state: weft_core::sessions::State::Quiet,
                    turn: Some(Turn::TurnEnded),
                    started: 1,
                    last: 2,
                    title: None,
                    asks: 0,
                    pane: None,
                    resumable: true,
                }],
                zone: 7 * 3_600_000,
            },
            Event::Signal(Box::new(weft_core::signal::Sent {
                harness: "claude-code".into(),
                hook: "Stop".into(),
                session: "s1".into(),
                workspace: Some("/w".into()),
                state: Some("turn_ended".into()),
                pid: Some(4821),
                pid_start: Some("1790000000.120000".into()),
                pane: Some(2),
                token: Some("0f".repeat(16)),
                ..Default::default()
            })),
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
    fn a_pending_listed_on_opening_reads_as_the_event_it_was() {
        let listed = pending_json("pnd_3", 1, "Ready to send", "why", b"/plan \x1b", None);
        let event = Event::Pending {
            id: "pnd_3".into(),
            pane: 1,
            what: "Ready to send".into(),
            why: "why".into(),
            payload: b"/plan \x1b".to_vec(),
            sends: None,
        };
        assert_eq!(pending_of(&listed), Some(event));
    }

    #[test]
    fn a_project_opened_without_saying_how_streams_raw_output() {
        let line =
            br#"{"id":1,"method":"project.open","params":{"path":"/p","rows":24,"cols":80}}"#;
        let Some(Line::Call { call: Call::Open { raw, .. }, .. }) = Line::decode(line) else {
            panic!("an open")
        };
        assert!(raw, "a client that names no output gets raw");
        let other = br#"{"id":1,"method":"project.open","params":{"path":"/p","rows":24,"cols":80,"output":"some"}}"#;
        assert_eq!(Line::decode(other), None, "only raw or none");
    }

    #[test]
    fn a_method_this_daemon_does_not_know_is_not_a_guess() {
        assert_eq!(Line::decode(br#"{"id":1,"method":"unit.teleport","params":{}}"#), None);
        assert_eq!(Line::decode(br#"{"method":"nothing.happened","params":{}}"#), None);
    }
}
