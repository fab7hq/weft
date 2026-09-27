//! The session server.
//!
//! The server owns the pane processes. A client is one view of them: closing
//! it, losing an SSH connection, or quitting Weft leaves the agents working.
//!
//! What a client sees on attaching is a replay of everything the panes have
//! printed, so a client that arrives late reconstructs exactly the screen a
//! client that never left would be showing.
//!
//! **One daemon, many projects.** A client names its project when it attaches
//! and sees only that project's panes, numbered from one within it. Nothing
//! crosses between projects here; that a single process holds them all is
//! what later makes it possible.

use std::collections::HashMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;

use crate::pane::Pane;
use crate::turns::Turn;

/// Each pane's agent state, the session each pane runs, and every session's
/// latest event.
type Agents = (Vec<Option<Turn>>, Vec<Option<String>>, Vec<crate::turns::Session>);
use weft_proto::{self, Call, Event as Out, Line, Lines, PROTOCOL, PaneInfo};

/// Everything a pane has printed since it started, so a late client can be
/// shown the same screen as an early one.
///
/// Bounded: a long-running agent would otherwise grow without limit. Dropping
/// the oldest bytes loses scrollback, never the current screen, because a
/// terminal's state is rebuilt by replaying what remains.
const REPLAY_LIMIT: usize = 4 * 1024 * 1024;

struct Slot {
    /// Its id, which is given when it starts and never changed (closing
    /// another pane renumbers nothing a client holds), its harness and
    /// command line, and the session Weft bound to it: this pane's entry in
    /// `panes.json`.
    map: crate::turns::Started,
    pane: Pane,
    replay: Vec<u8>,
    /// The size the agent was last given, and the size each window last
    /// drew it at. A pane takes the size of the window that last focused it
    /// or typed into it, so two windows of different sizes never squeeze an
    /// agent for the other.
    size: (u16, u16),
    wants: HashMap<u64, (u16, u16)>,
    /// Started in turbo mode: its harness's turbo flags were added to the
    /// command line it was asked for, which `spec` keeps as it was.
    turbo: bool,
}

impl Slot {
    /// Give the agent this window's size, if it drew it at one.
    fn follow(&mut self, client: u64) {
        if let Some(&size) = self.wants.get(&client)
            && size != self.size
            && self.pane.resize(size.0, size.1).is_ok()
        {
            self.size = size;
        }
    }

    fn record(&mut self, bytes: &[u8]) {
        self.replay.extend_from_slice(bytes);
        if self.replay.len() > REPLAY_LIMIT {
            let cut = self.replay.len() - REPLAY_LIMIT;
            self.replay.drain(..cut);
        }
    }
}

/// Something that would be typed, waiting on a person.
///
/// It lives here rather than in a client so that every attached client sees
/// the same question and only one of them can answer it. Nothing is written
/// until it is resolved.
struct Waiting {
    id: String,
    pane: u32,
    payload: Vec<u8>,
    how: weft_core::inject::Handoff,
    /// The Ask whose yes has to be recorded before anything is typed, when the
    /// harness's own chooser never got one.
    confirm: Option<String>,
    /// The pane was started for this, and may still be starting up.
    fresh: bool,
    /// The Ask whose prompt this sends, recorded as submitted once it is.
    sends: Option<String>,
}

/// How long a pane Weft started for an act must be still before it is typed
/// into, and how long Weft waits for that.
const STARTED_QUIET: std::time::Duration = std::time::Duration::from_secs(1);
const STARTED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// A send into an agent that is still starting, until it may be typed or its
/// deadline passes.
struct Starting {
    w: Waiting,
    force: bool,
    until: std::time::Instant,
    /// The screen when it last changed, for a send the person forced.
    screen: (std::time::Instant, String),
}

/// One project's panes. Panes are numbered within it, so a client counts from
/// one whatever else the daemon is holding.
struct Project {
    root: PathBuf,
    /// When this daemon opened it, as receipts count time.
    opened: i64,
    panes: Vec<Slot>,
    /// The id the next pane started here is given.
    next_pane: u32,
    /// The record, followed here rather than in every client. One reader, and
    /// clients are told what it says.
    ledger: crate::ledger::Ledger,
    waiting: Vec<Waiting>,
    /// Which harness takes which act here, and what each harness answers
    /// about itself. Both are the machine's, so both are asked here.
    routing: weft_core::routing::Routing,
    /// The `--override` every `/rf:` command carries here, if any.
    ringframe: Option<String>,
    /// Whether clients tell the person when an agent needs them.
    notify: bool,
    /// Whether agents started here get their harness's turbo flags.
    turbo: bool,
    readiness: HashMap<String, weft_core::readiness::Readiness>,
    /// Sends said yes to that wait for an agent Weft just started, looked at
    /// on each tick rather than waited on, so nothing else stops meanwhile.
    starting: Vec<Starting>,
    folds: crate::acts::Folds,
    /// The harnesses RingFrame's profiles define, read when the project
    /// opened. Weft keeps no list of its own.
    harnesses: weft_core::harness::Harnesses,
    /// Each agent's state, from its hooks' receipts, and what clients were
    /// last told of it.
    turns: crate::turns::Turns,
    agents: Option<Agents>,
}

impl Project {
    fn slot(&mut self, id: u32) -> Option<&mut Slot> {
        self.panes.iter_mut().find(|s| s.map.pane == id)
    }

    /// Where the pane with this id is in the list, which is the order the
    /// per-pane answers come in.
    fn index_of(&self, id: u32) -> Option<usize> {
        self.panes.iter().position(|s| s.map.pane == id)
    }
}

#[derive(Default)]
struct Clients {
    next: u64,
    sinks: HashMap<u64, UnixStream>,
    /// Which project each client attached to. A client that has not attached
    /// yet is in none, and hears nothing.
    watching: HashMap<u64, usize>,
    /// The size each window opened its project at, then the size it last
    /// drew an agent at: the size an agent it asks for starts at.
    sizes: HashMap<u64, (u16, u16)>,
}

impl Clients {
    fn add(&mut self, stream: UnixStream) -> u64 {
        let id = self.next;
        self.next += 1;
        self.sinks.insert(id, stream);
        id
    }

    /// Send to everyone watching this project. A client that has gone is
    /// simply dropped — its departure is not an error, it is how a client
    /// leaves.
    fn broadcast(&mut self, project: usize, event: &Out) {
        let wire = Line::Event(event.clone()).encode();
        let watching = &self.watching;
        self.sinks.retain(|id, s| {
            watching.get(id) != Some(&project) || weft_proto::send(s, &wire).is_ok()
        });
    }

    fn send_to(&mut self, id: u64, line: &Line) {
        let wire = line.encode();
        if let Some(s) = self.sinks.get_mut(&id)
            && weft_proto::send(s, &wire).is_err()
        {
            self.sinks.remove(&id);
        }
    }
}

/// What the accept loop and the pane readers hand to the one thread that owns
/// the panes, so nothing needs a lock held across a blocking read.
enum Wake {
    Client(UnixStream),
    Request {
        client: u64,
        call_id: u64,
        call: Call,
    },
    Output {
        project: usize,
        pane: u32,
        bytes: Vec<u8>,
    },
    Exited {
        project: usize,
        pane: u32,
    },
    /// Time to look at the ledgers again.
    Tick,
    /// A harness answered what it has installed. Asked off the run loop,
    /// because asking costs a process and panes must not wait on it.
    Readiness {
        project: usize,
        harness: String,
        state: weft_core::readiness::Readiness,
    },
    /// The RingFrame view moved: a check came back, or a step started or ended.
    Sync {
        project: usize,
        view: serde_json::Value,
    },
}

pub struct Session {
    projects: Vec<Project>,
    clients: Clients,
    tx: Sender<Wake>,
    next_pending: u64,
    /// Weft's `config.toml`, read each time a project opens.
    config: PathBuf,
    /// The harnesses to use instead of asking RingFrame, for a test that
    /// must not depend on the machine's profiles.
    harnesses: Option<weft_core::harness::Harnesses>,
}

/// What a call answers with. Everything gets one, so a client is never left
/// waiting on a call the daemon quietly dropped.
enum Answer {
    Ok(serde_json::Value),
    /// A code a program can branch on, and a sentence for a person.
    No(&'static str, String),
    /// The session is ending.
    Done,
}

impl Answer {
    fn nothing() -> Self {
        Answer::Ok(serde_json::json!({}))
    }

    fn failed(e: anyhow::Error) -> Self {
        Answer::No("failed", e.to_string())
    }

    fn line(self, id: u64) -> Line {
        match self {
            Answer::Ok(result) => Line::Result { id, result },
            Answer::Done => Line::Result { id, result: serde_json::json!({}) },
            Answer::No(code, message) => Line::Error { id, code: code.into(), message },
        }
    }
}

/// The person's yes, on record before anything is typed.
///
/// The order is the point: if the record cannot be written there is no
/// confirmed Ask, and typing the prompt anyway would be a send with nothing
/// behind it. An Ask that was already confirmed has nothing to write.
fn recorded_first(root: &Path, confirm: Option<&str>) -> Result<(), String> {
    let Some(ask_id) = confirm else { return Ok(()) };
    crate::ringframe::ask_confirm(root, ask_id)
        .map_err(|e| format!("RingFrame would not record your yes: {e:?}. Nothing was typed."))
}

/// What starting an agent could mean here, in the order it is offered.
///
/// Picking a session up comes before a fresh one, because a workspace with
/// history is a workspace you are coming back to. The session is whatever
/// RingFrame's receipts name; nothing is invented to fill a gap.
fn starts(root: &Path, harnesses: &weft_core::harness::Harnesses) -> serde_json::Value {
    use crate::harness::OnThisMachine as _;
    let mut out = Vec::new();
    for h in harnesses.iter().filter(|h| h.on_path()) {
        if let Some(s) = crate::sessions::latest(root, &h.name) {
            out.push(serde_json::json!({
                "harness": h.name,
                "label": format!("{} · pick up where you left off", h.name),
                "spec": h.resume_spec(&s.id),
                "session": {"id": s.id, "at": s.at, "last": s.last},
            }));
        }
        out.push(serde_json::json!({
            "harness": h.name,
            "label": format!("{} · start fresh", h.name),
            "spec": h.spec(),
            "session": serde_json::Value::Null,
        }));
    }
    serde_json::Value::Array(out)
}

/// What this project routes, by act.
fn routing_json(routing: &weft_core::routing::Routing) -> serde_json::Value {
    serde_json::json!({
        "acts": routing
            .each()
            .into_iter()
            .map(|(a, h)| (a.to_string(), serde_json::json!(h)))
            .collect::<serde_json::Map<_, _>>(),
        "unknown": routing.unknown,
        "ignored": routing.ignored,
        "leftover": routing.leftover,
        "eval_stages": routing.eval_stages,
    })
}

/// Whether an Ask could finish here at all, asked of RingFrame.
fn workspace_gap(root: &Path) -> serde_json::Value {
    match crate::ringframe::ask_preflight(root) {
        Ok(()) => serde_json::Value::Null,
        // Not installed is said elsewhere, once.
        Err(crate::ringframe::Error::NotInstalled) => serde_json::Value::Null,
        Err(crate::ringframe::Error::Refused { message, .. }) => {
            serde_json::json!(weft_core::offers::first_line(&message))
        }
    }
}

/// Readiness as a client reads it: one word per harness.
fn readiness_json(states: &HashMap<String, weft_core::readiness::Readiness>) -> serde_json::Value {
    use weft_core::readiness::{Gap, Readiness};
    serde_json::Value::Object(
        states
            .iter()
            .map(|(k, v)| {
                let word = match v {
                    Readiness::Ready => "ready",
                    Readiness::Missing(Gap::Cli) => "no_cli",
                    Readiness::Missing(Gap::Marketplace) => "no_marketplace",
                    Readiness::Missing(Gap::Plugin) => "no_plugin",
                    Readiness::Unknown => "unknown",
                };
                (k.clone(), serde_json::json!(word))
            })
            .collect(),
    )
}

/// Write a file whole or not at all, so a reader never sees half of one.
fn write_whole(path: &Path, text: String) {
    let part = path.with_extension("json.part");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&part, text).is_ok() {
        let _ = std::fs::rename(&part, path);
    }
}

/// Why Enter was withheld. Written once here because it is the daemon that
/// knows, and the person has to be told the text is sitting in their agent.
fn withheld(a: &crate::pane::Attempt) -> String {
    match &a.folded_command {
        // The prompt is all there, but folded into a placeholder, and a folded
        // paste is not read for commands. Sending it would ask for an ordinary
        // answer instead of the mode.
        Some(command) => format!(
            "the agent folded the paste, so {command} would not run — it would be taken as \
             ordinary text. The prompt is in its composer; type {command} there yourself, or \
             press Enter to send it without the mode."
        ),
        None => "the agent did not show the whole prompt, so it was not sent. The text is in \
                 its composer."
            .to_string(),
    }
}

impl Session {
    /// Serve until told to shut down. The socket is removed on the way out.
    /// Serve on `socket`, reading Weft's configuration from `config`. `weft
    /// --serve` passes `~/.fab7/weft/config.toml`; a test passes a path of its
    /// own, so nothing it does depends on the person's file.
    pub fn serve(socket: &Path, config: &Path) -> Result<()> {
        Self::serve_with(socket, config, None)
    }

    /// Serve with these harnesses rather than the ones RingFrame's profiles
    /// define on this machine. For tests; `weft --serve` asks RingFrame.
    pub fn serve_with(
        socket: &Path,
        config: &Path,
        harnesses: Option<weft_core::harness::Harnesses>,
    ) -> Result<()> {
        if let Some(dir) = socket.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // A socket left by a server that died is not a live server.
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket)?;
        // Whoever started this daemon waits for this line rather than
        // polling the socket.
        {
            use std::io::Write as _;
            let mut out = std::io::stdout();
            let _ = writeln!(out, "weft: listening on {}", socket.display());
            let _ = out.flush();
        }

        let (tx, rx) = channel();
        // The record changes on disk without telling anyone, so it is polled.
        // A stat every quarter second is cheap and the board is never stale
        // for longer than that.
        let tick_tx = tx.clone();
        std::thread::spawn(move || {
            while tick_tx.send(Wake::Tick).is_ok() {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
        let accept_tx = tx.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if accept_tx.send(Wake::Client(stream)).is_err() {
                    return;
                }
            }
        });

        let mut session = Session {
            projects: Vec::new(),
            clients: Clients::default(),
            tx,
            next_pending: 1,
            config: config.to_path_buf(),
            harnesses,
        };
        let outcome = session.run(rx);
        let _ = std::fs::remove_file(socket);
        outcome
    }

    fn run(&mut self, rx: Receiver<Wake>) -> Result<()> {
        while let Ok(event) = rx.recv() {
            match event {
                Wake::Client(stream) => self.accept(stream),
                Wake::Request { client, call_id, call } => {
                    // Every call is answered, so a client never waits on one
                    // the daemon quietly dropped.
                    let answer = self.request(client, call);
                    let done = matches!(answer, Ok(Answer::Done));
                    self.clients
                        .send_to(client, &answer.unwrap_or_else(Answer::failed).line(call_id));
                    if done {
                        return Ok(());
                    }
                }
                Wake::Output { project, pane, bytes } => {
                    if let Some(slot) = self.projects.get_mut(project).and_then(|p| p.slot(pane)) {
                        slot.record(&bytes);
                    }
                    self.clients.broadcast(project, &Out::Output { pane, bytes });
                }
                Wake::Exited { project, pane } => {
                    self.clients.broadcast(project, &Out::Exited { pane })
                }
                Wake::Tick => self.follow_the_record(),
                Wake::Readiness { project, harness, state } => {
                    if let Some(p) = self.projects.get_mut(project) {
                        p.readiness.insert(harness, state);
                    }
                    self.tell_readiness(project);
                }
                Wake::Sync { project, view } => {
                    self.clients.broadcast(project, &Out::Sync { view })
                }
            }
        }
        Ok(())
    }

    fn accept(&mut self, stream: UnixStream) {
        let Ok(reading) = stream.try_clone() else { return };
        let id = self.clients.add(stream);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut lines = Lines::new(reading);
            while let Ok(Some(line)) = lines.next() {
                let Line::Call { id: call_id, call } = line else { continue };
                let leaving = call == Call::Detach;
                if tx.send(Wake::Request { client: id, call_id, call }).is_err() || leaving {
                    return;
                }
            }
        });
    }

    /// Type a prompt that was said yes to. The refusal, if there is one, is
    /// the person's to see: it says what is sitting unsent in their agent.
    ///
    /// An agent Weft just started is not ready until it says so, and one that
    /// never will still draws its composer a moment after it starts. Either
    /// is waited for on the tick, never here.
    fn type_it(&mut self, project: usize, w: Waiting, force: bool) {
        if w.fresh {
            let now = std::time::Instant::now();
            let screen = (now, String::new());
            let until = now + STARTED_TIMEOUT;
            self.projects[project].starting.push(Starting { w, force, until, screen });
            return;
        }
        self.type_now(project, w, force);
    }

    /// Type each send whose agent is ready now, or whose wait is over.
    fn type_what_started(&mut self, project: usize) {
        let waiting = std::mem::take(&mut self.projects[project].starting);
        let now = std::time::Instant::now();
        for mut s in waiting {
            let go = now >= s.until
                || if s.force {
                    // The words on it are not read, only whether it has gone
                    // still.
                    match self.projects[project].slot(s.w.pane) {
                        Some(slot) => slot.pane.gone_quiet(&mut s.screen, STARTED_QUIET),
                        None => true,
                    }
                } else {
                    crate::turns::may_type(self.pane_state(project, s.w.pane)).is_ok()
                };
            if go {
                self.type_now(project, s.w, s.force);
            } else {
                self.projects[project].starting.push(s);
            }
        }
    }

    fn type_now(&mut self, project: usize, w: Waiting, force: bool) {
        // Weft types on its own only into an agent that reported it is ready
        // (ADR-0013). When the person has been told and said to type anyway,
        // the decision is theirs.
        let not_ready =
            if force { Ok(()) } else { crate::turns::may_type(self.pane_state(project, w.pane)) };
        let mut unrecorded = None;
        let refusal = match self.projects[project].slot(w.pane) {
            Some(slot) => match not_ready {
                Err(why) if slot.pane.running() => Some(why.to_string()),
                _ => match slot.pane.inject_as(&w.payload, &w.how) {
                    Err(r) => Some(format!("{r:?}")),
                    Ok(a) if !a.submitted => Some(withheld(&a)),
                    Ok(_) => {
                        slot.map.enter(crate::turns::now_millis());
                        // Sent on the person's yes: their submission, on
                        // record, so the Ask moves on to its Eval even
                        // where no hook sees it arrive. When RingFrame will
                        // not record it, the person is told: the row would
                        // otherwise offer the send again.
                        if let Some(ask) = &w.sends {
                            let root = self.projects[project].root.clone();
                            unrecorded = crate::ringframe::ask_submitted(&root, ask).err().map(
                                |e| match e {
                                    crate::ringframe::Error::NotInstalled => {
                                        "ringframe is not installed".to_string()
                                    }
                                    crate::ringframe::Error::Refused { message, .. } => {
                                        weft_core::offers::first_line(&message).to_string()
                                    }
                                },
                            );
                        }
                        None
                    }
                },
            },
            None => Some("NoProcess".to_string()),
        };
        self.clients.broadcast(project, &Out::Injected { pane: w.pane, refusal, unrecorded });
    }

    /// One of RingFrame's three acts. The daemon works out what to type and
    /// where it goes; a client only names the act.
    fn act(
        &mut self,
        client: u64,
        at: usize,
        act: &str,
        unit_id: Option<&str>,
        pane: Option<u32>,
        text: Option<&str>,
    ) -> Answer {
        let root = self.projects[at].root.clone();
        let unit = unit_id
            .and_then(|id| self.projects[at].ledger.units().into_iter().find(|u| u.ask_id == id));
        let built = match act {
            "ask" | "follow_up" => {
                let Some(pane) = pane else {
                    return Answer::No("no_pane", "an Ask needs a pane to go into".into());
                };
                let Some(harness) = self.projects[at].slot(pane).map(|s| s.map.harness.clone())
                else {
                    return Answer::No("no_pane", format!("there is no pane {pane} here"));
                };
                let mut text = text.unwrap_or_default().to_string();
                // A follow-up is an Ask about this work: it carries one id, and
                // RingFrame's Ask reads what that id points at.
                if act == "follow_up" {
                    let Some(unit) = &unit else {
                        return Answer::No("no_unit", "that Ask is not on the board".into());
                    };
                    text = format!("[follow-up {}] {text}", weft_core::board::follow_up_of(unit));
                }
                let project = &mut self.projects[at];
                let over = project.ringframe.as_deref();
                let built = crate::acts::ask(&root, &harness, &text, over, &mut project.folds);
                (built, pane)
            }
            "send" => {
                let Some(unit) = unit else {
                    return Answer::No("no_unit", "that Ask is not on the board".into());
                };
                let Some(pane) = self.pane_of(at, &unit.harness) else {
                    return Answer::No("no_pane", format!("no {} pane is open here", unit.harness));
                };
                match crate::acts::send(&root, &unit) {
                    Ok(built) => (built, pane),
                    Err(why) => return Answer::No("refused", why),
                }
            }
            "check" | "decide" => {
                let Some(unit) = unit else {
                    return Answer::No("no_unit", "that Ask is not on the board".into());
                };
                // Where it goes is the project's to say. An Eval also says
                // which of its stages runs next, and on what.
                let project = &self.projects[at];
                let (skill, into, args) = if act == "check" {
                    let (gather, debate) = weft_core::eval_stages::resolve(
                        project.routing.eval_stages.as_ref(),
                        project.routing.get("eval"),
                        &unit.harness,
                    );
                    let gathered = unit.gathered.as_ref().map(|g| g.eval_id.as_str());
                    let (into, args) =
                        weft_core::eval_stages::next_send(&gather, &debate, gathered);
                    ("eval", into, args)
                } else {
                    let into = project.routing.get("seal").unwrap_or(&unit.harness).to_string();
                    ("seal", into, String::new())
                };
                let (pane, fresh) = match self.free_pane_of(at, &into) {
                    Some(pane) => (pane, false),
                    None => match self.start_for(client, at, &into) {
                        Some(pane) => (pane, true),
                        None => {
                            return Answer::No("no_pane", format!("Weft could not start {into}"));
                        }
                    },
                };
                let project = &mut self.projects[at];
                let mut built = crate::acts::skill(
                    &root,
                    skill,
                    &into,
                    &unit.harness,
                    &args,
                    project.ringframe.as_deref(),
                    &mut project.folds,
                );
                if fresh {
                    built
                        .asking
                        .why
                        .push(format!("No {into} agent here was free, so Weft started one."));
                }
                return self.stage(at, pane, built, fresh);
            }
            other => return Answer::No("unknown_act", format!("there is no act {other}")),
        };
        let (built, pane) = built;
        self.stage(at, pane, built, false)
    }

    /// The first of this harness's agents that is free for new work: one
    /// whose latest event is `ready` or `turn_ended` (turn-state.md §4).
    fn free_pane_of(&mut self, at: usize, harness: &str) -> Option<u32> {
        let states = self.agents_of(at).0;
        self.projects[at]
            .panes
            .iter()
            .zip(states)
            .find(|(s, state)| s.map.harness == harness && crate::turns::may_type(*state).is_ok())
            .map(|(s, _)| s.map.pane)
    }

    /// Start a pane of this harness for an act, on its own command: never
    /// another pane's, which may be resuming someone else's session.
    fn start_for(&mut self, client: u64, at: usize, harness: &str) -> Option<u32> {
        let spec = self.projects[at].harnesses.find(harness)?.spec();
        let id = self.spawn(client, at, harness, &spec, None)?;
        self.look_at(at, harness);
        Some(id)
    }

    fn pane_of(&self, at: usize, harness: &str) -> Option<u32> {
        self.projects[at].panes.iter().find(|s| s.map.harness == harness).map(|s| s.map.pane)
    }

    /// Put a built prompt in front of everyone watching, and answer with its id.
    fn stage(&mut self, at: usize, pane: u32, built: crate::acts::Built, fresh: bool) -> Answer {
        let id = format!("pnd_{}", self.next_pending);
        self.next_pending += 1;
        let message = Out::Pending {
            id: id.clone(),
            pane,
            what: built.asking.what,
            why: built.asking.why.join("\n"),
            payload: built.payload.clone(),
        };
        self.projects[at].waiting.push(Waiting {
            id: id.clone(),
            pane,
            payload: built.payload,
            how: built.how,
            confirm: built.confirm_first.then(|| built.ask_id.clone()).flatten(),
            fresh,
            sends: built.ask_id.clone(),
        });
        self.clients.broadcast(at, &message);
        Answer::Ok(serde_json::json!({"pending": id}))
    }

    /// The exact wording, or an Eval record, as RingFrame wrote them.
    fn read(&mut self, at: usize, what: &str, unit: &str) -> Answer {
        let root = self.projects[at].root.clone();
        match what {
            "wording" => match crate::ringframe::ask_copy(&root, unit) {
                Ok(bytes) => Answer::Ok(serde_json::json!({
                    "text": String::from_utf8_lossy(&bytes)
                })),
                Err(e) => Answer::No("refused", format!("RingFrame would not hand it over: {e:?}")),
            },
            // What the person asked, as RingFrame recorded it: for an Ask whose
            // prompt is not composed yet, it is all there is to show.
            "source" => {
                let safe =
                    !unit.is_empty() && unit.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                let path = root.join(".fab7/rf/asks").join(unit).join("source.txt");
                match std::fs::read(&path) {
                    Ok(bytes) if safe => Answer::Ok(serde_json::json!({
                        "text": String::from_utf8_lossy(&bytes)
                    })),
                    _ => Answer::No("no_record", "what was asked is not on disk".into()),
                }
            }
            // `unit` is an eval id here: a record is named by the Eval, not the Ask.
            "judges" => match crate::record::read(&root, unit) {
                Some(r) => Answer::Ok(serde_json::to_value(&r).unwrap_or(serde_json::Value::Null)),
                None => Answer::No("no_record", "that check's record is not on disk".into()),
            },
            // `unit` is a seal id here, for the same reason: a receipt is
            // named by the Seal, not by the Ask it closed.
            "seal" => match crate::ringframe::seal_checked(&root, unit) {
                Ok(v) => Answer::Ok(v),
                Err(e) => Answer::No("refused", format!("RingFrame would not check it: {e:?}")),
            },
            other => Answer::No("unknown_read", format!("there is nothing called {other}")),
        }
    }

    /// Look, and with `proceed` run what is behind and look again, off the
    /// run loop. Every change reaches the client as it happens.
    fn sync(&mut self, at: usize, proceed: bool) -> Answer {
        let tx = self.tx.clone();
        let harnesses = self.projects[at].harnesses.clone();
        std::thread::spawn(move || {
            let show = |v: &weft_core::sync::View| {
                let view = serde_json::to_value(v).unwrap_or_default();
                let _ = tx.send(Wake::Sync { project: at, view });
            };
            let (mut v, states) = crate::sync::look(&harnesses);
            // Looking is asking each harness again, so what it said replaces
            // an older answer — a stale "could not tell" included.
            for (harness, state) in states {
                let _ = tx.send(Wake::Readiness { project: at, harness, state });
            }
            if !proceed {
                return show(&v);
            }
            crate::sync::proceed(&mut v, show);
            if v.steps.iter().any(|s| s.mark == weft_core::sync::Mark::Failed) {
                return;
            }
            let (v, states) = crate::sync::look(&harnesses);
            show(&v);
            for (harness, state) in states {
                let _ = tx.send(Wake::Readiness { project: at, harness, state });
            }
        });
        Answer::nothing()
    }

    /// Ask one harness what it has, off the run loop.
    ///
    /// Asking costs two processes and can take seconds; doing it here would
    /// stop every pane in every project while it ran. The answer comes back as
    /// a `Wake` and is told to whoever is watching.
    fn look_at(&mut self, at: usize, name: &str) {
        use weft_core::readiness::Readiness;
        let (tx, name) = (self.tx.clone(), name.to_string());
        let found = self.projects[at].harnesses.find(&name).cloned();
        std::thread::spawn(move || {
            let cli = crate::ringframe::installed();
            // No CLI is no profile either; that is the gap to name.
            if !cli {
                let state = Readiness::Missing(weft_core::readiness::Gap::Cli);
                let _ = tx.send(Wake::Readiness { project: at, harness: name, state });
                return;
            }
            let Some(h) = found.as_ref() else {
                let _ = tx.send(Wake::Readiness {
                    project: at,
                    harness: name,
                    state: Readiness::Unknown,
                });
                return;
            };
            // A harness that is starting up can fail to answer once; ask again
            // before settling on "could not tell".
            let mut state = crate::readiness::check(h, cli);
            for _ in 0..2 {
                if state != Readiness::Unknown {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
                state = crate::readiness::check(h, cli);
            }
            let _ = tx.send(Wake::Readiness { project: at, harness: name, state });
        });
    }

    fn tell_readiness(&mut self, at: usize) {
        let states = readiness_json(&self.projects[at].readiness);
        self.clients.broadcast(at, &Out::Readiness { states });
    }

    /// This project's panes, as everything outside the daemon sees them.
    fn pane_infos(&mut self, project: usize) -> Vec<PaneInfo> {
        let Some(p) = self.projects.get_mut(project) else { return Vec::new() };
        p.panes
            .iter_mut()
            .map(|s| PaneInfo {
                pane: s.map.pane,
                harness: s.map.harness.clone(),
                spec: s.map.spec.clone(),
                running: s.pane.running(),
                turbo: s.turbo,
            })
            .collect()
    }

    /// Tell each project's clients when its record has moved, or an agent's
    /// state has.
    fn follow_the_record(&mut self) {
        for at in 0..self.projects.len() {
            self.type_what_started(at);
            if self.projects[at].ledger.refresh() {
                let board = self.board_of(at);
                self.clients.broadcast(at, &board);
            }
            let agents = self.agents_of(at);
            if self.projects[at].agents.as_ref() != Some(&agents) {
                self.projects[at].agents = Some(agents.clone());
                let (panes, bound, sessions) = agents;
                self.clients.broadcast(at, &Out::Agents { panes, bound, sessions });
            }
        }
    }

    /// Each pane's state and every session's latest event, re-read from the
    /// receipts. A new receipt may bind its session to a pane, once.
    fn agents_of(&mut self, at: usize) -> Agents {
        let p = &mut self.projects[at];
        let heard = p.turns.refresh(&p.root);
        let mut map: Vec<crate::turns::Started> = p
            .panes
            .iter_mut()
            .map(|slot| {
                slot.map.running = slot.pane.running();
                slot.map.clone()
            })
            .collect();
        let mut moved = false;
        for s in &heard {
            moved |= crate::turns::heard(&mut map, s);
        }
        for (slot, m) in p.panes.iter_mut().zip(map) {
            slot.map = m;
        }
        if moved {
            self.write_map(at);
        }
        let p = &self.projects[at];
        let sessions = p.turns.sessions();
        let map: Vec<_> = p.panes.iter().map(|s| s.map.clone()).collect();
        let states = crate::turns::pane_states(&map, &sessions);
        let bound = map
            .iter()
            .map(|m| m.session.as_ref().filter(|_| m.running).map(|b| b.session.clone()))
            .collect();
        (states, bound, sessions)
    }

    /// The projects open here, beside `config.toml`, for a window's picker.
    fn write_projects(&self) {
        let open: Vec<_> = self
            .projects
            .iter()
            .map(|p| weft_core::projects::Opened { path: p.root.clone(), opened: p.opened })
            .collect();
        write_whole(
            &self.config.with_file_name("projects.json"),
            weft_core::projects::write(&open),
        );
    }

    /// Weft's own pane map, `<project>/.fab7/weft/panes.json`: which pane
    /// runs which harness and session, and how Weft knows. RingFrame never
    /// reads it; nothing in `.fab7/rf/` names a pane.
    fn write_map(&self, at: usize) {
        let p = &self.projects[at];
        let dir = p.root.join(".fab7").join("weft");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            let _ = std::fs::write(&ignore, "*\n");
        }
        let panes: Vec<_> = p.panes.iter().map(|s| &s.map).collect();
        let mut text =
            serde_json::to_string_pretty(&serde_json::json!({"panes": panes})).unwrap_or_default();
        text.push('\n');
        write_whole(&dir.join("panes.json"), text);
    }

    /// One pane's state, read now.
    fn pane_state(&mut self, at: usize, pane: u32) -> Option<Turn> {
        let i = self.projects[at].index_of(pane)?;
        self.agents_of(at).0.get(i).copied().flatten()
    }

    /// The board and the records behind it. Read here so that nothing opens a
    /// file to draw a frame.
    fn board_of(&mut self, at: usize) -> Out {
        let units = self.projects[at].ledger.units();
        let root = self.projects[at].root.clone();
        let records = units
            .iter()
            .filter_map(|u| u.check.as_ref())
            .filter_map(|c| {
                crate::record::read(&root, &c.eval_id)
                    .and_then(|r| serde_json::to_value(r).ok())
                    .map(|v| (c.eval_id.clone(), v))
            })
            .collect::<serde_json::Map<_, _>>();
        Out::Units { units, records: serde_json::Value::Object(records) }
    }

    /// The project this client is watching, and its panes.
    fn watched(&mut self, client: u64) -> Option<(usize, &mut Project)> {
        let at = *self.clients.watching.get(&client)?;
        self.projects.get_mut(at).map(|p| (at, p))
    }

    /// The project at this path, opened if this is the first client to ask.
    fn project_at(&mut self, root: PathBuf) -> usize {
        match self.projects.iter().position(|p| p.root == root) {
            Some(at) => at,
            None => {
                let ledger = crate::ledger::Ledger::at(&root);
                let harnesses =
                    self.harnesses.clone().unwrap_or_else(|| crate::ringframe::harnesses(&root));
                let config = crate::routing::read_at(&self.config, &harnesses);
                self.projects.push(Project {
                    opened: crate::turns::now_millis(),
                    routing: config.routing(&root),
                    ringframe: config.ringframe_override(&root),
                    notify: config.notify(),
                    turbo: config.turbo(&root),
                    root,
                    panes: Vec::new(),
                    next_pane: 0,
                    ledger,
                    waiting: Vec::new(),
                    readiness: HashMap::new(),
                    starting: Vec::new(),
                    folds: crate::acts::Folds::default(),
                    turns: crate::turns::Turns::default(),
                    harnesses,
                    agents: None,
                });
                let at = self.projects.len() - 1;
                // A map a daemon before this one left names panes that are gone.
                self.write_map(at);
                self.write_projects();
                at
            }
        }
    }

    /// Returns true when the session should end.
    /// Answers every call. `Answer::Done` ends the session.
    fn request(&mut self, client: u64, call: Call) -> Result<Answer> {
        match call {
            Call::Hello { protocol, .. } => {
                return Ok(if protocol == PROTOCOL {
                    Answer::Ok(serde_json::json!({"protocol": PROTOCOL}))
                } else {
                    Answer::No(
                        "protocol",
                        format!("this daemon speaks protocol {PROTOCOL}, not {protocol}"),
                    )
                });
            }
            // Opening resizes nothing: another window may be drawing these
            // agents at its own size.
            Call::Open { path: project, rows, cols } => {
                self.clients.sizes.insert(client, (rows, cols));
                let at = self.project_at(PathBuf::from(project));
                self.clients.watching.insert(client, at);
                // An act routed to a harness with no pane yet still needs its
                // readiness, or it reads as "could not ask" rather than as a
                // missing pane.
                for harness in self.projects[at].routing.harnesses() {
                    if !self.projects[at].readiness.contains_key(&harness) {
                        self.look_at(at, &harness);
                    }
                }
                self.projects[at].ledger.refresh();
                let Out::Units { units, records } = self.board_of(at) else { unreachable!() };
                let panes = self.pane_infos(at);
                let (agent_panes, bound, sessions) = self.agents_of(at);
                // Everything comes back in the answer — the board, the panes,
                // and a replay of what each has printed — rather than as
                // events sent before it. A client that waits for its answer
                // cannot miss what it asked for, and events from here on carry
                // only what changes.
                let replay: Vec<_> = self.projects[at]
                    .panes
                    .iter()
                    .filter(|s| !s.replay.is_empty())
                    .map(|s| weft_proto::output_json(s.map.pane, &s.replay))
                    .collect();
                return Ok(Answer::Ok(serde_json::json!({
                    "panes": weft_proto::panes_json(&panes),
                    "units": serde_json::to_value(&units).unwrap_or_default(),
                    "records": records,
                    "replay": replay,
                    "readiness": readiness_json(&self.projects[at].readiness),
                    "routing": routing_json(&self.projects[at].routing),
                    "agents": {"panes": agent_panes, "bound": bound, "sessions": sessions},
                    "notify": self.projects[at].notify,
                    "turbo": self.projects[at].turbo,
                    "harnesses": &self.projects[at].harnesses,
                    // Why this workspace could not finish an Ask, if it could not.
                    "gap": workspace_gap(&self.projects[at].root),
                })));
            }
            Call::Input { pane, bytes } => {
                if let Some(slot) = self.watched(client).and_then(|(_, p)| p.slot(pane)) {
                    slot.follow(client);
                    if slot.pane.send(&bytes).is_ok() && bytes.contains(&b'\r') {
                        slot.map.enter(crate::turns::now_millis());
                    }
                }
            }
            Call::StartAgent { harness, spec, session } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                let started = self.spawn(client, at, &harness, &spec, session);
                // Asked when a pane starts, because that is when a person
                // would care.
                self.look_at(at, &harness);
                return Ok(match started {
                    Some(pane) => Answer::Ok(serde_json::json!({"pane": pane})),
                    None => Answer::No("not_started", format!("{spec} did not start")),
                });
            }
            Call::CloseAgent { pane } => {
                if let Some(at) = self.clients.watching.get(&client).copied() {
                    self.close(at, pane);
                }
            }
            Call::Resize { pane, rows, cols } => {
                self.clients.sizes.insert(client, (rows, cols));
                if let Some(slot) = self.watched(client).and_then(|(_, p)| p.slot(pane)) {
                    slot.wants.insert(client, (rows, cols));
                    slot.follow(client);
                }
            }
            Call::Stage { pane, bytes, how, what, why } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                let id = format!("pnd_{}", self.next_pending);
                self.next_pending += 1;
                let message =
                    Out::Pending { id: id.clone(), pane, what, why, payload: bytes.clone() };
                self.projects[at].waiting.push(Waiting {
                    id: id.clone(),
                    pane,
                    payload: bytes,
                    how,
                    confirm: None,
                    fresh: false,
                    sends: None,
                });
                self.clients.broadcast(at, &message);
                return Ok(Answer::Ok(serde_json::json!({"pending": id})));
            }
            Call::Act { act, unit, pane, text } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                return Ok(self.act(client, at, &act, unit.as_deref(), pane, text.as_deref()));
            }
            Call::ConfirmAsk { unit } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                let root = self.projects[at].root.clone();
                return Ok(match crate::ringframe::ask_confirm(&root, &unit) {
                    Ok(()) => Answer::nothing(),
                    Err(e) => Answer::No("refused", format!("{e:?}")),
                });
            }
            Call::Read { what, unit } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                return Ok(self.read(at, &what, &unit));
            }
            Call::Sync { proceed } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                return Ok(self.sync(at, proceed));
            }
            Call::Available => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                let root = self.projects[at].root.clone();
                let found = &self.projects[at].harnesses;
                return Ok(Answer::Ok(serde_json::json!({"starts": starts(&root, found)})));
            }
            Call::Turbo { on } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                // Held while the daemon runs; `config.toml` says where it starts.
                self.projects[at].turbo = on;
                return Ok(Answer::Ok(serde_json::json!({"turbo": on})));
            }
            Call::Resolve { pending, yes, force } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                // Taken, not read: whoever answers first answers for everyone,
                // and a second answer finds nothing to answer.
                let Some(i) = self.projects[at].waiting.iter().position(|w| w.id == pending) else {
                    return Ok(Answer::No("gone", "that was already answered".into()));
                };
                let w = self.projects[at].waiting.remove(i);
                self.clients.broadcast(at, &Out::Resolved { id: pending, yes });
                if !yes {
                    return Ok(Answer::nothing());
                }
                let root = self.projects[at].root.clone();
                if let Err(why) = recorded_first(&root, w.confirm.as_deref()) {
                    return Ok(Answer::No("not_recorded", why));
                }
                self.type_it(at, w, force);
            }
            // A client leaving is not the session ending. That is the point.
            Call::Detach => {}
            Call::Shutdown => return Ok(Answer::Done),
        }
        Ok(Answer::nothing())
    }

    /// Take a pane away. The agent is stopped if it is somehow still running:
    /// closing is the person saying they are done with it, and a pane nobody
    /// can see is a process nobody can stop. Every other pane keeps its id,
    /// so the list is all a client needs to be told.
    fn close(&mut self, project: usize, pane: u32) {
        let Some(p) = self.projects.get_mut(project) else { return };
        let Some(index) = p.index_of(pane) else { return };
        let mut slot = p.panes.remove(index);
        slot.pane.stop();
        self.write_map(project);
        let panes = self.pane_infos(project);
        self.clients.broadcast(project, &Out::Panes { panes });
    }

    /// Start an agent at the size of the window that asked, and answer with
    /// its pane's id.
    fn spawn(
        &mut self,
        client: u64,
        project: usize,
        harness: &str,
        spec: &str,
        resumed: Option<String>,
    ) -> Option<u32> {
        let p = self.projects.get_mut(project)?;
        let cwd = p.root.to_string_lossy().into_owned();
        // Turbo mode, when this project's config turns it on: the harness's
        // own flags, after the person's own.
        let flags = if p.turbo {
            p.harnesses.find(harness).map(|h| h.turbo.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let turbo = !flags.is_empty();
        let words = weft_core::harness::with_turbo(spec, &flags);
        let program = words.first().cloned().unwrap_or_default();
        let argv: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();

        let id = p.next_pane;
        p.next_pane += 1;
        let (tx, harness_name) = (self.tx.clone(), harness.to_string());
        let size = self.clients.sizes.get(&client).copied().unwrap_or((24, 80));
        match Pane::spawn_args(harness, &program, &argv, &cwd, size.0, size.1) {
            Ok(mut pane) => {
                // Every byte a pane prints goes to the session, which records
                // it for replay and forwards it to whoever is attached.
                let sink = pane.stream_output();
                std::thread::spawn(move || {
                    while let Ok(bytes) = sink.recv() {
                        if tx.send(Wake::Output { project, pane: id, bytes }).is_err() {
                            return;
                        }
                    }
                    let _ = tx.send(Wake::Exited { project, pane: id });
                });
                let now = crate::turns::now_millis();
                self.projects[project].panes.push(Slot {
                    map: crate::turns::Started::new(id, harness, spec, now, resumed),
                    pane,
                    replay: Vec::new(),
                    size,
                    wants: HashMap::from([(client, size)]),
                    turbo,
                });
                self.write_map(project);
                self.clients.broadcast(
                    project,
                    &Out::Added { pane: id, harness: harness_name, spec: spec.to_string(), turbo },
                );
                Some(id)
            }
            Err(e) => {
                self.clients.broadcast(
                    project,
                    &Out::Injected { pane: id, refusal: Some(e.to_string()), unrecorded: None },
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod confirming {
    use super::*;

    #[test]
    fn nothing_is_typed_until_the_yes_is_on_record() {
        // The order matters. An Ask that was already confirmed has nothing to
        // write; one RingFrame will not confirm stops the send, and says so.
        let nowhere = Path::new("/tmp");
        assert_eq!(recorded_first(nowhere, None), Ok(()), "nothing to record");
        let refused = recorded_first(nowhere, Some("ask_nothing_here")).unwrap_err();
        assert!(refused.contains("Nothing was typed"), "{refused}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replay_keeps_the_recent_past_and_drops_the_distant_one() {
        let mut slot_replay = Vec::new();
        let mut record = |bytes: &[u8]| {
            slot_replay.extend_from_slice(bytes);
            if slot_replay.len() > REPLAY_LIMIT {
                let cut = slot_replay.len() - REPLAY_LIMIT;
                slot_replay.drain(..cut);
            }
        };
        record(&vec![b'o'; REPLAY_LIMIT]);
        record(b"the newest output");
        assert_eq!(slot_replay.len(), REPLAY_LIMIT, "bounded");
        assert!(slot_replay.ends_with(b"the newest output"), "the present survives");
    }
}
