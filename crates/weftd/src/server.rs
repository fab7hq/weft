//! The session server.
//!
//! Spec: `plans/weft/spec/runtime.md`, [ADR-0003]. The server owns the pane
//! processes. A client is one view of them: closing it, losing an SSH
//! connection, or quitting Weft leaves the agents working.
//!
//! What a client sees on attaching is a replay of everything the panes have
//! printed, so a client that arrives late reconstructs exactly the screen a
//! client that never left would be showing.
//!
//! **One daemon, many projects** ([ADR-0007](../plans/weft/adr/0007-three-layers.md)).
//! A client names its project when it attaches and sees only that project's
//! panes, numbered from one within it. Nothing crosses between projects here;
//! that a single process holds them all is what later makes it possible.

use std::collections::HashMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;

use crate::pane::Pane;
use weft_proto::{self, Call, Event as Out, Line, Lines, PROTOCOL, PaneInfo};

/// Everything a pane has printed since it started, so a late client can be
/// shown the same screen as an early one.
///
/// Bounded: a long-running agent would otherwise grow without limit. Dropping
/// the oldest bytes loses scrollback, never the current screen, because a
/// terminal's state is rebuilt by replaying what remains.
const REPLAY_LIMIT: usize = 4 * 1024 * 1024;

struct Slot {
    pane: Pane,
    harness: String,
    /// The command line this pane was started with. Kept so a client can tell
    /// whether a session is already open here before opening it a second time.
    spec: String,
    replay: Vec<u8>,
}

impl Slot {
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
/// the same question and only one of them can answer it (ADR-0007). Nothing is
/// written until it is resolved.
struct Waiting {
    id: String,
    pane: u32,
    payload: Vec<u8>,
    how: weft_core::inject::Handoff,
    /// The Ask whose yes has to be recorded before anything is typed, when the
    /// harness's own chooser never got one.
    confirm: Option<String>,
}

/// One project's panes. Panes are numbered within it, so a client counts from
/// one whatever else the daemon is holding.
struct Project {
    root: PathBuf,
    panes: Vec<Slot>,
    /// The record, followed here rather than in every client. One reader, and
    /// clients are told what it says (ADR-0007).
    ledger: crate::ledger::Ledger,
    waiting: Vec<Waiting>,
    /// Which harness takes which act here, and what each harness answers
    /// about itself. Both are the machine's, so both are asked here.
    routing: weft_core::routing::Routing,
    readiness: HashMap<String, weft_core::readiness::Readiness>,
    folds: crate::acts::Folds,
}

#[derive(Default)]
struct Clients {
    next: u64,
    sinks: HashMap<u64, UnixStream>,
    /// Which project each client attached to. A client that has not attached
    /// yet is in none, and hears nothing.
    watching: HashMap<u64, usize>,
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
}

pub struct Session {
    projects: Vec<Project>,
    clients: Clients,
    tx: Sender<Wake>,
    next_pending: u64,
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
fn starts(root: &Path) -> serde_json::Value {
    use crate::harness::OnThisMachine as _;
    let mut out = Vec::new();
    for h in weft_core::harness::SUPPORTED.iter().filter(|h| h.on_path()) {
        if let Some(s) = crate::sessions::latest(root, h.name) {
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
                    Readiness::Declined => "declined",
                    Readiness::Unknown => "unknown",
                };
                (k.clone(), serde_json::json!(word))
            })
            .collect(),
    )
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
    pub fn serve(socket: &Path) -> Result<()> {
        if let Some(dir) = socket.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // A socket left by a server that died is not a live server.
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket)?;

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

        let mut session =
            Session { projects: Vec::new(), clients: Clients::default(), tx, next_pending: 1 };
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
                    if let Some(slot) =
                        self.projects.get_mut(project).and_then(|p| p.panes.get_mut(pane as usize))
                    {
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
    fn type_it(&mut self, project: usize, w: Waiting, force: bool) {
        let refusal = match self.projects[project].panes.get_mut(w.pane as usize) {
            Some(slot) => {
                let screen = slot.pane.with_screen(|s| s.contents());
                // Whether the pane looks busy is Weft's inference. When the
                // person has seen what it read and said to type anyway, the
                // decision is theirs (ADR-0004).
                let blocked = !force && weft_core::blocked::looks_blocked(&screen).is_some();
                match slot.pane.inject_as(&w.payload, blocked, &w.how) {
                    Err(r) => Some(format!("{r:?}")),
                    Ok(a) if !a.submitted => Some(withheld(&a)),
                    Ok(_) => None,
                }
            }
            None => Some("NoProcess".to_string()),
        };
        self.clients.broadcast(project, &Out::Injected { pane: w.pane, refusal });
    }

    /// One of RingFrame's three acts. The daemon works out what to type and
    /// where it goes; a client only names the act.
    fn act(
        &mut self,
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
            "ask" => {
                let Some(pane) = pane else {
                    return Answer::No("no_pane", "an Ask needs a pane to go into".into());
                };
                let Some(harness) =
                    self.projects[at].panes.get(pane as usize).map(|s| s.harness.clone())
                else {
                    return Answer::No("no_pane", format!("there is no pane {pane} here"));
                };
                let built = crate::acts::ask(
                    &root,
                    &harness,
                    text.unwrap_or_default(),
                    &mut self.projects[at].folds,
                );
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
                let skill = if act == "check" { "eval" } else { "seal" };
                // Where it goes is the project's to say (spec/routing.md).
                let key = if act == "check" { "eval" } else { "seal" };
                let into = self.projects[at].routing.get(key).unwrap_or(&unit.harness).to_string();
                let Some(pane) = self.pane_of(at, &into) else {
                    return Answer::No("no_pane", format!("no {into} pane is open here"));
                };
                let built = crate::acts::skill(
                    &root,
                    skill,
                    &into,
                    &unit.harness,
                    &mut self.projects[at].folds,
                );
                (built, pane)
            }
            other => return Answer::No("unknown_act", format!("there is no act {other}")),
        };
        let (built, pane) = built;
        self.stage(at, pane, built)
    }

    fn pane_of(&self, at: usize, harness: &str) -> Option<u32> {
        self.projects[at].panes.iter().position(|s| s.harness == harness).map(|i| i as u32)
    }

    /// Put a built prompt in front of everyone watching, and answer with its id.
    fn stage(&mut self, at: usize, pane: u32, built: crate::acts::Built) -> Answer {
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

    /// Run a harness's install commands, then ask it again rather than
    /// believing an exit code.
    fn set_up(&mut self, at: usize, name: &str) -> Answer {
        let Some(h) = weft_core::harness::find(name) else {
            return Answer::No("unknown_harness", format!("Weft does not know {name}"));
        };
        if let Err(why) = crate::readiness::set_up(h) {
            return Answer::No("failed", why);
        }
        self.projects[at].readiness.remove(name);
        self.look_at(at, name);
        Answer::nothing()
    }

    /// Ask one harness what it has, off the run loop.
    ///
    /// Asking costs two processes and can take seconds; doing it here would
    /// stop every pane in every project while it ran. The answer comes back as
    /// a `Wake` and is told to whoever is watching.
    fn look_at(&mut self, at: usize, name: &str) {
        use weft_core::readiness::Readiness;
        if matches!(self.projects[at].readiness.get(name), Some(Readiness::Declined)) {
            return; // a no is not re-asked this session
        }
        let (tx, name) = (self.tx.clone(), name.to_string());
        std::thread::spawn(move || {
            let cli = crate::ringframe::installed();
            let state = match weft_core::harness::find(&name) {
                Some(h) => crate::readiness::check(h, cli),
                None => Readiness::Unknown,
            };
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
            .enumerate()
            .map(|(i, s)| PaneInfo {
                pane: i as u32,
                harness: s.harness.clone(),
                spec: s.spec.clone(),
                running: s.pane.running(),
            })
            .collect()
    }

    /// Tell each project's clients when its record has moved.
    fn follow_the_record(&mut self) {
        for at in 0..self.projects.len() {
            if self.projects[at].ledger.refresh() {
                let board = self.board_of(at);
                self.clients.broadcast(at, &board);
            }
        }
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
                let routing = crate::routing::for_project(&root);
                self.projects.push(Project {
                    root,
                    panes: Vec::new(),
                    ledger,
                    waiting: Vec::new(),
                    routing,
                    readiness: HashMap::new(),
                    folds: crate::acts::Folds::default(),
                });
                self.projects.len() - 1
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
            Call::Open { rows, cols, path: project } => {
                let at = self.project_at(PathBuf::from(project));
                self.clients.watching.insert(client, at);
                for slot in &mut self.projects[at].panes {
                    let _ = slot.pane.resize(rows, cols);
                }
                self.projects[at].ledger.refresh();
                let Out::Units { units, records } = self.board_of(at) else { unreachable!() };
                let panes = self.pane_infos(at);
                // Everything comes back in the answer — the board, the panes,
                // and a replay of what each has printed — rather than as
                // events sent before it. A client that waits for its answer
                // cannot miss what it asked for, and events from here on carry
                // only what changes.
                let replay: Vec<_> = (0..self.projects[at].panes.len())
                    .filter(|i| !self.projects[at].panes[*i].replay.is_empty())
                    .map(|i| weft_proto::output_json(i as u32, &self.projects[at].panes[i].replay))
                    .collect();
                return Ok(Answer::Ok(serde_json::json!({
                    "panes": weft_proto::panes_json(&panes),
                    "units": serde_json::to_value(&units).unwrap_or_default(),
                    "records": records,
                    "replay": replay,
                    "readiness": readiness_json(&self.projects[at].readiness),
                    "routing": routing_json(&self.projects[at].routing),
                    // Why this workspace could not finish an Ask, if it could not.
                    "gap": workspace_gap(&self.projects[at].root),
                })));
            }
            Call::Input { pane, bytes } => {
                if let Some(slot) =
                    self.watched(client).and_then(|(_, p)| p.panes.get_mut(pane as usize))
                {
                    let _ = slot.pane.send(&bytes);
                }
            }
            Call::StartAgent { harness, spec } => {
                if let Some(at) = self.clients.watching.get(&client).copied() {
                    self.spawn(at, &harness, &spec);
                    // Asked when a pane starts, because that is when a person
                    // would care.
                    self.look_at(at, &harness);
                }
            }
            Call::CloseAgent { pane } => {
                if let Some(at) = self.clients.watching.get(&client).copied() {
                    self.close(at, pane);
                }
            }
            Call::Resize { pane, rows, cols } => {
                if let Some(slot) =
                    self.watched(client).and_then(|(_, p)| p.panes.get_mut(pane as usize))
                {
                    let _ = slot.pane.resize(rows, cols);
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
                });
                self.clients.broadcast(at, &message);
                return Ok(Answer::Ok(serde_json::json!({"pending": id})));
            }
            Call::Act { act, unit, pane, text } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                return Ok(self.act(at, &act, unit.as_deref(), pane, text.as_deref()));
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
            Call::SetUp { harness } => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                return Ok(self.set_up(at, &harness));
            }
            Call::Available => {
                let Some(at) = self.clients.watching.get(&client).copied() else {
                    return Ok(Answer::No("no_project", "open a project first".into()));
                };
                let root = self.projects[at].root.clone();
                return Ok(Answer::Ok(serde_json::json!({"starts": starts(&root)})));
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
    /// can see is a process nobody can stop.
    fn close(&mut self, project: usize, pane: u32) {
        let index = pane as usize;
        let Some(p) = self.projects.get_mut(project) else { return };
        if index >= p.panes.len() {
            return;
        }
        let mut slot = p.panes.remove(index);
        slot.pane.stop();
        self.resync(project);
    }

    /// Say the whole list again, and replay each pane into it. Panes are
    /// numbered by position, so a client that kept the old numbers would be
    /// typing into the wrong agent — the one thing this must never allow.
    fn resync(&mut self, project: usize) {
        let panes = self.pane_infos(project);
        self.clients.broadcast(project, &Out::Panes { panes });
        for i in 0..self.projects[project].panes.len() {
            let bytes = self.projects[project].panes[i].replay.clone();
            if !bytes.is_empty() {
                self.clients.broadcast(project, &Out::Output { pane: i as u32, bytes });
            }
        }
    }

    fn spawn(&mut self, project: usize, harness: &str, spec: &str) {
        let Some(p) = self.projects.get(project) else { return };
        let cwd = p.root.to_string_lossy().into_owned();
        let mut parts = spec.split_whitespace();
        let program = parts.next().unwrap_or(spec).to_string();
        let args: Vec<String> = parts.map(str::to_string).collect();
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();

        let id = self.projects[project].panes.len() as u32;
        let (tx, harness_name) = (self.tx.clone(), harness.to_string());
        match Pane::spawn_args(harness, &program, &argv, &cwd, 24, 80) {
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
                self.projects[project].panes.push(Slot {
                    pane,
                    harness: harness_name.clone(),
                    spec: spec.to_string(),
                    replay: Vec::new(),
                });
                self.clients.broadcast(
                    project,
                    &Out::Added { pane: id, harness: harness_name, spec: spec.to_string() },
                );
            }
            Err(e) => {
                self.clients
                    .broadcast(project, &Out::Injected { pane: id, refusal: Some(e.to_string()) });
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
