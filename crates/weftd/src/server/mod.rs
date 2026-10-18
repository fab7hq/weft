//! The session server.
//!
//! The server owns the pane processes. A client is one view of them: closing
//! it, losing an SSH connection, or quitting Weft leaves the agents working.
//!
//! What a client sees on attaching is each pane's screen, its modes and its
//! recent history, then every chunk printed after it, so a client that
//! arrives late shows the screen a client that never left would be showing.
//!
//! **One daemon, many projects.** A client names its project when it attaches
//! and sees only that project's panes, numbered from one within it. Nothing
//! crosses between projects here; that a single process holds them all is
//! what later makes it possible.

use std::collections::HashMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;

use crate::pane::Pane;
use crate::turns::Turn;

mod agents;
mod login;
mod remote;
mod screens;
mod typing;

use typing::*;

/// Each pane's agent state, and every session's latest event.
type Agents = (Vec<Option<Turn>>, Vec<weft_core::sessions::Row>);

/// What an act is on: a work, a pane, the words of an Ask, an Eval's round.
struct On<'a> {
    unit: Option<&'a str>,
    pane: Option<u32>,
    text: Option<&'a str>,
    eval: Option<&'a str>,
    /// A harness to start a new agent of, to ask in instead of `pane`.
    start: Option<&'a str>,
}
use weft_proto::{self, Call, Event as Out, Line, Lines, PROTOCOL, PaneInfo};

/// The most lines of a pane's history one `read` answers.
const HISTORY_MAX: usize = 2_000;

/// At most one `presence` in this long.
const PRESENCE_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// How often a project RingFrame is not set up in is asked again, while a
/// person works through its checklist.
const RECHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

struct Slot {
    /// Its id, which is given when it starts and never changed (closing
    /// another pane renumbers nothing a client holds), its harness and
    /// command line, and the session Weft bound to it: this pane's entry in
    /// `panes.json`.
    map: crate::turns::Started,
    pane: Pane,
    /// The size the agent was last given, and the size each window last
    /// drew it at. A pane takes the size of the window that last focused it
    /// or typed into it, so two windows of different sizes never squeeze an
    /// agent for the other.
    size: (u16, u16),
    wants: HashMap<u64, (u16, u16)>,
    /// Started in turbo mode: its harness's turbo flags were added to the
    /// command line it was asked for, which `spec` keeps as it was.
    turbo: bool,
    /// `$WEFT_PANE_TOKEN`: 128 random bits, kept here and nowhere else. A
    /// signal naming this pane counts only with it.
    token: String,
    /// The harness's own transcript, as its hooks last named it: kept here
    /// and nowhere else, for its Conversation.
    transcript: Option<std::path::PathBuf>,
    /// The agent's last reply and Sense's embedding of it, read when its turn
    /// ended: what the next prompt follows.
    reply: (String, Vec<f32>),
    /// When the pane last printed output, and the PTY sequence and reading
    /// from `weft-sense` Head 1 once output settles for 150 ms or a hook arrives.
    last_output_at: std::time::Instant,
    sense_settled_seq: u64,
    sensed_surface: Option<weft_sense::SurfaceReading>,
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

    fn record(&mut self) {
        self.last_output_at = std::time::Instant::now();
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
    /// What the person is shown, kept for a client that opens the project
    /// after it was staged (remote.md D5).
    what: String,
    why: String,
    payload: Vec<u8>,
    how: weft_core::inject::Handoff,
    /// The Ask whose yes has to be recorded before anything is typed, when the
    /// harness's own chooser never got one.
    confirm: Option<String>,
    /// The Ask whose prompt this sends: delivered only when its pane reports
    /// receiving it.
    sends: Option<String>,
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
    /// What the person sent here, for suggestions: loaded or built in the
    /// background when the project opens, nothing until then.
    suggest: Option<Vec<weft_core::suggest::Entry>>,
    /// Its replies are being embedded: one pass at a time.
    embedding: bool,
    /// `suggest = true`: the message field gets suggestions here.
    suggest_on: bool,
    /// The index its suggestions come from, and the change it was built at.
    index: Option<(u64, weft_core::suggest::Index)>,
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
    /// Sends said yes to that wait for their agent to be ready, looked at on
    /// each tick rather than waited on, so nothing else stops meanwhile.
    starting: Vec<Starting>,
    /// Sends of an Ask typed and submitted, each waiting for what its pane
    /// reports it received.
    due: Vec<typing::Due>,
    prefixes: crate::acts::Prefixes,
    /// The harnesses Weft has a file for, read when the project opened, and
    /// the files it could not read.
    harnesses: weft_core::harness::Harnesses,
    unread: Vec<String>,
    /// Each agent's state, from its hooks' receipts, and what clients were
    /// last told of it.
    turns: crate::turns::Turns,
    agents: Option<Agents>,
    /// Harness processes a receipt named and that have since gone: one
    /// gone with its start time never comes back, so it is not asked again.
    gone: std::collections::HashSet<(u32, String)>,
    /// What this project still needs before RingFrame is set up in it, if it
    /// is not; when that was last asked, and whether it is being asked now.
    setup: Option<weft_core::workspace::Needs>,
    asked: std::time::Instant,
    asking: bool,
    /// Closed: no window had it open and none of its agents was running when
    /// the last one left. Its place in the list stays, so every other
    /// project keeps its number; the next window to open its path gets a new
    /// one.
    closed: bool,
    /// Reopened after an unclean end, at this time: kept open
    /// with no window until someone acts in it or dismisses it.
    reopened: Option<i64>,
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
    /// Clients that opened their project with `output: "none"`: they are
    /// never sent a pane's raw bytes (remote.md D4).
    quiet: std::collections::HashSet<u64>,
    /// What each client called itself in its hello.
    names: HashMap<u64, String>,
    /// When `presence` was last sent.
    presence: Option<std::time::Instant>,
    /// `weft remote`'s last view, and the client that published it, which
    /// is where acts for it go (remote.md D8).
    remote: Option<(u64, serde_json::Value)>,
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

    /// Send to every client, whatever project it watches, or none.
    fn broadcast_all(&mut self, event: &Out) {
        let wire = Line::Event(event.clone()).encode();
        self.sinks.retain(|_, s| weft_proto::send(s, &wire).is_ok());
    }

    fn named(&self, client: u64, name: &str) -> bool {
        self.names.get(&client).is_some_and(|n| n.starts_with(name))
    }

    /// The person typed into Weft here: say so, at most once in ten seconds.
    /// Only the app's typing counts; a remote device's is not the person at
    /// the machine (remote.md D6).
    fn typed(&mut self, client: u64) {
        if !self.named(client, "weft-desktop")
            || self.presence.is_some_and(|at| at.elapsed() < PRESENCE_EVERY)
        {
            return;
        }
        self.presence = Some(std::time::Instant::now());
        self.broadcast_all(&Out::Presence { at: crate::turns::now_millis() });
    }

    /// A pane's bytes, to everyone watching this project who asked for them.
    fn broadcast_output(&mut self, project: usize, pane: u32, seq: u64, bytes: Vec<u8>) {
        let wire = Line::Event(Out::Output { pane, seq, bytes }).encode();
        let (watching, quiet) = (&self.watching, &self.quiet);
        self.sinks.retain(|id, s| {
            watching.get(id) != Some(&project)
                || quiet.contains(id)
                || weft_proto::send(s, &wire).is_ok()
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
    /// A client's connection ended, with or without saying Detach.
    Left(u64),
    Request {
        client: u64,
        call_id: u64,
        call: Call,
    },
    /// A chunk a pane printed, and its number: a client that drew the
    /// pane's screen as of a number skips the chunks it already holds.
    Output {
        project: usize,
        pane: u32,
        seq: u64,
        bytes: Vec<u8>,
    },
    /// A pane's PTY output settled for 150 ms at `seq`: run `weft-sense` Head 1.
    SenseSettle {
        project: usize,
        pane: u32,
        seq: u64,
    },
    Exited {
        project: usize,
        pane: u32,
    },
    /// Time to look at the ledgers again.
    Tick,
    /// A project's suggestion entries were read: all of them (`source` none),
    /// or again from one source. `built`: built again, so not kept yet.
    Suggested {
        root: PathBuf,
        source: Option<String>,
        entries: Vec<weft_core::suggest::Entry>,
        built: bool,
    },
    /// The habits every project's suggestions share, read at start.
    Habits(Vec<weft_core::suggest::Entry>),
    /// An agent's turn ended: its last reply, and Sense's embedding of it.
    Replied {
        root: PathBuf,
        pane: u32,
        reply: String,
        near: Vec<f32>,
    },
    /// Sense embedded a project's replies.
    Embedded {
        root: PathBuf,
        vectors: Vec<(crate::suggest::Key, Vec<f32>)>,
    },
    /// Time to send screen frames: ten times a second.
    Frames,
    /// A harness answered what it has installed. Asked off the run loop,
    /// because asking costs a process and panes must not wait on it.
    Readiness {
        project: usize,
        harness: String,
        state: weft_core::readiness::Readiness,
    },
    /// The latest release was asked again: each open project's view is
    /// looked at again, so Updates says what is behind now.
    Checked,
    /// The RingFrame view moved.
    Setup {
        project: usize,
        view: serde_json::Value,
    },
    /// The person said where a harness's program is: every project runs it
    /// from there from now on.
    Program {
        harness: String,
        program: String,
    },
    /// RingFrame was asked again to set a project up, and this is what it
    /// still needs, if anything.
    Workspace {
        project: usize,
        setup: Option<weft_core::workspace::Needs>,
    },
    /// A hook, from `weft signal`.
    Signal(Box<weft_core::signal::Sent>),
    /// A session's turn, as its harness's own stream said it.
    Heard {
        harness: String,
        heard: weft_core::listen::Heard,
    },
    /// A harness's stream ended: its server closed it.
    Unheard {
        harness: String,
    },
    /// What RingFrame made of the prompt a pane received after Weft's send of
    /// `ask`.
    Received {
        project: usize,
        pane: u32,
        ask: String,
        answer: Result<serde_json::Value, String>,
        /// The session to tie the pane to, when it is the Ask: a shared
        /// server's hook named no pane.
        tie: Option<String>,
    },
}

pub struct Session {
    projects: Vec<Project>,
    /// The socket this daemon listens on, which every harness it starts is
    /// told as `$WEFT_SOCKET`.
    socket: PathBuf,
    /// Weft's own session receipts, beside `config`.
    receipts: PathBuf,
    /// What signals forward to `ringframe observe`, one queue per project.
    forwarders: crate::forward::Forwarders,
    /// The file lines already forwarded as a prompt, by harness and session.
    forwarded: std::collections::HashSet<String>,
    /// The harnesses whose own stream is open.
    streams: std::collections::HashSet<String>,
    /// The sessions, by harness, whose turn an open stream has spoken of: a
    /// hook no longer moves it.
    streamed: std::collections::HashSet<(String, String)>,
    clients: Clients,
    tx: Sender<Wake>,
    next_pending: u64,
    /// Weft's `config.toml`, read each time a project opens.
    config: PathBuf,
    /// The harnesses to use instead of the files beside `config`, for a test
    /// that must not depend on the machine's.
    harnesses: Option<weft_core::harness::Harnesses>,
    /// The machine, or the world a test describes: every program the daemon
    /// runs, and every question it asks of the machine, goes through it.
    outside: crate::outside::Shared,
    /// Handed harnesses through [`Session::serve_with`] are taken as set up
    /// for RingFrame without asking; in a world of its own, a test says so.
    assume_ready: bool,
    /// The panes clients asked to be sent screens of (`pane.attach`).
    attached: Vec<screens::Attachment>,
    /// Keeps `weft remote` running while remote access is on.
    supervisor: remote::Supervisor,
    /// A project a client dismissed, closed once its answer is sent.
    dismissed: Option<usize>,
    /// A newer Weft installed beside this one, by its version: started on
    /// once no agent runs.
    update: Option<String>,
    /// The short messages every kept index holds, read in the background at
    /// start: the habits every project's suggestions share.
    habits: Vec<weft_core::suggest::Entry>,
    /// Counts each change to any project's entries or the habits: an index
    /// built at an older count is built again.
    suggest_changes: u64,
    /// Active `weft-sense` model bundle, hot-reloaded in place (`ADR-0003`).
    #[allow(dead_code)]
    sense: Arc<std::sync::RwLock<Option<weft_sense::SenseModel>>>,
    /// This daemon's own version, held while it runs so it is never removed
    /// from under it.
    _held: Option<crate::versions::Held>,
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

    fn no_project() -> Self {
        Answer::No("no_project", "open a project first".into())
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

/// What can be started here: each harness set up for RingFrame, fresh. A
/// session is resumed by its id, from its own Details, never
/// offered here as "the latest". A harness whose plugin is not in it is
/// offered by SET UP, not here: an agent started in it would record nothing.
fn starts(
    out_there: &dyn crate::outside::Outside,
    harnesses: &weft_core::harness::Harnesses,
    readiness: &HashMap<String, weft_core::readiness::Readiness>,
) -> serde_json::Value {
    let mut out = Vec::new();
    let ready =
        |h: &&weft_core::harness::Harness| readiness.get(&h.name).is_some_and(|r| r.is_ready());
    for h in harnesses.iter().filter(|h| out_there.on_path(&h.program)).filter(ready) {
        out.push(serde_json::json!({
            "harness": h.name,
            "label": h.name,
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
fn workspace_gap(out: &dyn crate::outside::Outside, root: &Path) -> Option<String> {
    match crate::ringframe::ask_preflight(out, root) {
        Ok(()) => None,
        // Not installed is said elsewhere, once.
        Err(crate::ringframe::Error::NotInstalled) => None,
        Err(crate::ringframe::Error::Refused { message, .. }) => {
            Some(weft_core::offers::first_line(&message).to_string())
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
/// A folder only this user can reach: made 0700, and refused when another
/// user owns it.
fn private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if std::fs::metadata(dir)?.uid() != unsafe { libc::geteuid() } {
        anyhow::bail!("{} belongs to another user", dir.display());
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn write_whole(path: &Path, text: String) {
    let part = path.with_extension("json.part");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&part, text).is_ok() {
        let _ = std::fs::rename(&part, path);
    }
}

/// Set for a daemon an update started, which ended nothing.
const UPDATED: &str = "WEFT_UPDATED";

impl Session {
    /// Serve until told to shut down. The socket is removed on the way out.
    /// Serve on `socket`, reading Weft's configuration from `config`. `weft
    /// --serve` passes `~/.fab7/weft/config.toml`; a test passes a path of its
    /// own, so nothing it does depends on the person's file.
    pub fn serve(socket: &Path, config: &Path) -> Result<()> {
        Self::serve_with(socket, config, None)
    }

    /// Serve with these harnesses rather than the harness files beside
    /// `config`. For tests; `weft --serve` reads the files.
    pub fn serve_with(
        socket: &Path,
        config: &Path,
        harnesses: Option<weft_core::harness::Harnesses>,
    ) -> Result<()> {
        let assume_ready = harnesses.is_some();
        Self::serve_on(socket, config, harnesses, crate::outside::machine(), assume_ready)
    }

    /// Serve in this world rather than the machine's: a UI test says which
    /// programs exist and what they answer, and nothing else is reached.
    pub fn serve_in(
        socket: &Path,
        config: &Path,
        harnesses: Option<weft_core::harness::Harnesses>,
        outside: crate::outside::Shared,
    ) -> Result<()> {
        Self::serve_on(socket, config, harnesses, outside, false)
    }

    fn serve_on(
        socket: &Path,
        config: &Path,
        harnesses: Option<weft_core::harness::Harnesses>,
        outside: crate::outside::Shared,
        assume_ready: bool,
    ) -> Result<()> {
        // Only this user may reach the daemon: its folder is 0700 and theirs,
        // and the socket 0600.
        if let Some(dir) = socket.parent() {
            private_dir(dir)?;
        }
        // Started again by an update, which ended nothing. Read
        // before any thread of this daemon runs, and not passed on.
        let updated = std::env::var_os(UPDATED).is_some();
        if updated {
            // SAFETY: the daemon's first act in a process that `exec` just
            // started; no other thread reads the environment yet.
            unsafe { std::env::remove_var(UPDATED) };
        }
        // A live daemon is left alone: this one ends cleanly, so a service
        // started beside it is not restarted over it.
        if weft_proto::is_live(socket) {
            // Still the line a starting client waits for: one is listening.
            println!("weft: already listening on {}", socket.display());
            return Ok(());
        }
        // A socket left by a server that died is not a live server.
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket)?;
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
        }
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
        let frames_tx = tx.clone();
        std::thread::spawn(move || {
            while frames_tx.send(Wake::Frames).is_ok() {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        // What the latest release is, asked when the daemon starts and then
        // twice a day, never at every look: the kept answer is what each view
        // reads. A test's daemon, handed its harnesses, reaches no network.
        if harnesses.is_none() {
            let (check_tx, out) = (tx.clone(), outside.clone());
            std::thread::spawn(move || {
                loop {
                    let _ = out.release(true);
                    if check_tx.send(Wake::Checked).is_err() {
                        return;
                    }
                    std::thread::sleep(crate::onboarding::CHECK_EVERY);
                }
            });
        }
        // Weft's receipts are pruned when it starts and once a day after.
        let receipts = crate::receipts::base_of(config);
        let pruned = receipts.clone();
        std::thread::spawn(move || {
            loop {
                crate::receipts::prune(&pruned, crate::turns::now_millis());
                std::thread::sleep(std::time::Duration::from_secs(24 * 60 * 60));
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

        let weft_dir = config.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        let sense = Arc::new(std::sync::RwLock::new(crate::versions::load_sense(
            &weft_dir,
            harnesses.is_none(),
        )));
        let mut session = Session {
            projects: Vec::new(),
            socket: socket.to_path_buf(),
            receipts,
            forwarders: crate::forward::Forwarders::new(outside.clone()),
            forwarded: Default::default(),
            streams: Default::default(),
            streamed: Default::default(),
            clients: Clients::default(),
            tx,
            next_pending: 1,
            config: config.to_path_buf(),
            harnesses,
            outside,
            assume_ready,
            attached: Vec::new(),
            supervisor: Default::default(),
            dismissed: None,
            update: None,
            habits: Vec::new(),
            suggest_changes: 0,
            sense,
            _held: crate::versions::hold_own(),
        };
        // Older versions nothing runs any more are removed: never this one,
        // nor the one `weft` points at. A daemon not run from a version
        // (a test's, a build's) removes nothing.
        let versions = config.with_file_name("versions");
        if let Some(own) =
            std::env::current_exe().ok().and_then(|e| crate::versions::own(&versions, &e))
        {
            let newest = crate::versions::newest(&versions);
            let keep: Vec<PathBuf> = [Some(own), newest].into_iter().flatten().collect();
            crate::versions::prune(&versions, &keep);
        }
        // The projects an earlier daemon had open when it ended without a
        // stop, reopened with their agents ended; after a stop
        // the list is empty. After an update nothing ended: they are open.
        session.reopen(!updated);
        session.remote_from_config();
        let (habits_of, tx) = (config.to_path_buf(), session.tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Wake::Habits(crate::suggest::habits(&habits_of)));
        });
        let outcome = session.run(rx);
        let _ = std::fs::remove_file(socket);
        outcome
    }

    fn run(&mut self, rx: Receiver<Wake>) -> Result<()> {
        while let Ok(event) = rx.recv() {
            match event {
                Wake::Client(stream) => self.accept(stream),
                Wake::Left(client) => self.left(client),
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
                    if let Some(at) = self.dismissed.take() {
                        self.close_if_idle(at);
                    }
                }
                Wake::Output { project, pane, seq: chunk, bytes } => {
                    let mut seq = None;
                    if let Some(slot) = self.projects.get_mut(project).and_then(|p| p.slot(pane)) {
                        slot.record();
                        seq = Some(slot.pane.current_seq());
                    }
                    self.printed(project, pane);
                    self.clients.broadcast_output(project, pane, chunk, bytes);
                    if let Some(seq) = seq {
                        let tx = self.tx.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(150));
                            let _ = tx.send(Wake::SenseSettle { project, pane, seq });
                        });
                    }
                }
                Wake::SenseSettle { project, pane, seq } => {
                    self.sense_pane_if_settled(project, pane, seq);
                }
                Wake::Frames => self.frames(),
                Wake::Exited { project, pane } => {
                    self.clients.broadcast(project, &Out::Exited { pane })
                }
                Wake::Tick => {
                    self.follow_the_record();
                    self.update_when_idle();
                }
                Wake::Suggested { root, source, entries, built } => {
                    self.suggested(&root, source, entries, built)
                }
                Wake::Embedded { root, vectors } => self.embedded(&root, vectors),
                Wake::Habits(entries) => {
                    self.habits = entries;
                    self.suggest_changes += 1;
                }
                Wake::Replied { root, pane, reply, near } => {
                    if let Some(slot) = self
                        .projects
                        .iter_mut()
                        .find(|p| p.root == root && !p.closed)
                        .and_then(|p| p.panes.iter_mut().find(|s| s.map.pane == pane))
                    {
                        slot.reply = (reply, near);
                    }
                }
                Wake::Checked => {
                    let open: Vec<usize> =
                        (0..self.projects.len()).filter(|&at| !self.projects[at].closed).collect();
                    for at in open {
                        self.setup(at, "look", "", "");
                    }
                }
                Wake::Readiness { project, harness, state } => {
                    // Harnesses handed over through `serve_with` are taken as
                    // set up for RingFrame: what the real machine answers does
                    // not overwrite them. A world of its own answers for itself.
                    let fixture = self.assume_ready
                        && self.harnesses.as_ref().is_some_and(|h| h.find(&harness).is_some());
                    if let Some(p) = self.projects.get_mut(project).filter(|_| !fixture) {
                        p.readiness.insert(harness, state);
                    }
                    self.tell_readiness(project);
                }
                Wake::Setup { project, view } => {
                    self.clients.broadcast(project, &Out::Setup { view })
                }
                Wake::Program { harness, program } => {
                    for p in &mut self.projects {
                        p.harnesses = std::mem::take(&mut p.harnesses)
                            .with_programs(|n| (n == harness).then(|| program.clone()));
                    }
                }
                Wake::Workspace { project, setup } => self.set_up(project, setup),
                Wake::Signal(sent) => self.signal(*sent),
                Wake::Heard { harness, heard } => self.heard(harness, heard),
                Wake::Unheard { harness } => {
                    self.streams.remove(&harness);
                    self.streamed.retain(|(h, _)| *h != harness);
                }
                Wake::Received { project, pane, ask, answer, tie } => {
                    self.received(project, pane, &ask, answer, tie)
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
                if let Line::Event(Out::Signal(sent)) = line {
                    let _ = tx.send(Wake::Signal(sent));
                    continue;
                }
                let Line::Call { id: call_id, call } = line else { continue };
                let leaving = call == Call::Detach;
                if tx.send(Wake::Request { client: id, call_id, call }).is_err() || leaving {
                    break;
                }
            }
            let _ = tx.send(Wake::Left(id));
        });
    }

    /// One of RingFrame's three acts. The daemon works out what to type and
    /// where it goes; a client only names the act.
    fn act(&mut self, client: u64, at: usize, act: &str, on: On) -> Answer {
        let act = match act {
            "eval" => "check",
            "seal" => "decide",
            "proceed" => "send",
            other => other,
        };
        let On { unit: unit_id, pane, text, eval, start } = on;
        let root = self.projects[at].root.clone();
        let units = self.projects[at].ledger.units();
        // Eval and Seal act on one unit of work, named by any of its Asks,
        // or Seal by its Eval: the Ask that routes it is its newest open one.
        let record = self.projects[at].ledger.record();
        let work = weft_core::work::work(&units, &record);
        let mine = match (act, eval) {
            ("decide", Some(id)) => work.of_eval(id),
            _ => unit_id.and_then(|id| work.of_ask(id)),
        };
        let root_of = |u: &weft_core::work::UnitOfWork| {
            units.iter().find(|a| a.ask_id == u.root).cloned().unwrap_or_default()
        };
        let no =
            |why: weft_core::tree::Why| Answer::No(why.key.trim_start_matches("why."), why.say());
        let mut eval = eval.map(str::to_string);
        let unit_id = match act {
            "check" | "decide" => {
                let Some(u) = mine else {
                    if let Some(why) =
                        eval.as_deref().and_then(|e| weft_core::work::sealed_already(&record, e))
                    {
                        return no(why);
                    }
                    return Answer::No("no_unit", "Eval and Seal name the work they act on".into());
                };
                let ready = |_: Option<&str>| None;
                let why = if act == "check" {
                    weft_core::work::eval_why(u, &root_of(u), &ready)
                } else {
                    eval.as_deref()
                        .and_then(|e| weft_core::work::sealed_already(&record, e))
                        .or_else(|| weft_core::work::seal_why(&work, u, &root_of(u), &ready))
                };
                if let Some(why) = why {
                    return no(why);
                }
                if act == "decide" {
                    eval = work.to_seal(u).map(|e| e.id.clone());
                }
                u.route.as_deref()
            }
            _ => unit_id,
        };
        // A follow-up carries the unit's newest Eval, else its newest composed
        // Ask, so RingFrame links it into that unit.
        let follows = mine.map(|u| u.follows.clone());
        let unit = unit_id.and_then(|id| units.iter().find(|u| u.ask_id == id).cloned());
        let built = match act {
            "ask" | "follow_up" => {
                // A new agent, when the person picked one: started now, on
                // their Enter, and asked once it is ready.
                let pane = match start {
                    Some(harness) => match self.start_for(client, at, harness) {
                        Some(pane) => Some(pane),
                        None => {
                            return Answer::No(
                                "no_pane",
                                format!("Weft could not start {harness}"),
                            );
                        }
                    },
                    None => pane,
                };
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
                    let to = follows.as_deref().unwrap_or(weft_core::board::follow_up_of(unit));
                    text = format!("[follow-up {to}] {text}");
                }
                let project = &mut self.projects[at];
                let over = project.ringframe.as_deref();
                let built = crate::acts::ask(&root, &harness, &text, over, &mut project.prefixes);
                (built, pane)
            }
            "send" => {
                let Some(unit) = unit else {
                    return Answer::No("no_unit", "that Ask is not on the board".into());
                };
                // The pane that owns the Ask: the one running its session.
                let map: Vec<_> = self.projects[at]
                    .panes
                    .iter_mut()
                    .map(|s| {
                        s.map.running = s.pane.running();
                        s.map.clone()
                    })
                    .collect();
                let Some(pane) = crate::turns::owning_pane(&map, &unit) else {
                    return Answer::No("no_pane", format!("no {} pane is open here", unit.harness));
                };
                // A send is proven by what the harness reports it received, so
                // Weft sends only to one whose file says where that is.
                if let Some(why) = self.cannot_prove(at, pane) {
                    return Answer::No("no_receipt", why);
                }
                match crate::acts::send(self.outside.as_ref(), &root, &unit, &unit.harness) {
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
                    let debate = weft_core::eval_stages::resolve(
                        project.routing.eval_stages.as_ref(),
                        project.routing.get("eval"),
                        &unit.harness,
                    );
                    // Weft gathers: RingFrame reads the change, and only the
                    // debate goes to a harness.
                    // It goes on with the unit's own open Eval; one opened over
                    // other work too is left, and RingFrame opens the unit's.
                    let own = unit
                        .gathered
                        .as_ref()
                        .filter(|g| mine.is_some_and(|u| work.goes_on_with(u, &g.eval_id)));
                    let eval_id = match own {
                        Some(g) => g.eval_id.clone(),
                        None => {
                            let agents = weft_core::eval_stages::agents(&debate);
                            match crate::ringframe::eval_open(
                                self.outside.as_ref(),
                                &root,
                                &unit.ask_id,
                                agents.as_ref(),
                                project.ringframe.as_deref(),
                            ) {
                                Ok(id) => id,
                                Err(crate::ringframe::Error::NotInstalled) => {
                                    return Answer::No(
                                        "no_ringframe",
                                        "the ringframe CLI is not installed".into(),
                                    );
                                }
                                Err(crate::ringframe::Error::Refused { message, .. }) => {
                                    return Answer::No("refused", message);
                                }
                            }
                        }
                    };
                    let (into, args) = weft_core::eval_stages::debate_send(&debate, &eval_id);
                    ("eval", into, args)
                } else {
                    let into = project.routing.get("seal").unwrap_or(&unit.harness).to_string();
                    // `seal create --eval <id>` closes exactly that Eval's Asks.
                    ("seal", into, eval.map(|e| format!("--eval {e}")).unwrap_or_default())
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
                    &mut project.prefixes,
                );
                if fresh {
                    built
                        .asking
                        .why
                        .push(format!("No {into} agent here was free, so Weft started one."));
                }
                return self.stage(at, pane, built);
            }
            other => return Answer::No("unknown_act", format!("there is no act {other}")),
        };
        let (built, pane) = built;
        self.stage(at, pane, built)
    }

    /// The exact wording, or an Eval record, as RingFrame wrote them.
    fn read(&mut self, at: usize, what: &str, unit: &str) -> Answer {
        let root = self.projects[at].root.clone();
        match what {
            "wording" => match crate::ringframe::ask_copy(self.outside.as_ref(), &root, unit) {
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
            // `unit` is an eval id: the Eval view, from RingFrame's files as
            // they are now, so it can be read while the Eval runs.
            "eval" => match crate::record::eval_view(&root, unit) {
                Some(v) => Answer::Ok(serde_json::to_value(&v).unwrap_or(serde_json::Value::Null)),
                None if root.join(".fab7/rf/evals").join(unit).join("record.json").exists() => {
                    Answer::No(
                        "no_view",
                        "this Eval was judged before the Eval view; its Details show its record"
                            .into(),
                    )
                }
                None => Answer::No("no_record", "that Eval is not on disk".into()),
            },
            // `unit` is `<eval id>/<window id>`: one change's hunk.
            "window" => match unit
                .split_once('/')
                .and_then(|(e, w)| crate::record::window_text(&root, e, w))
            {
                Some(text) => Answer::Ok(serde_json::json!({ "text": text })),
                None => Answer::No("no_record", "that change is not on disk".into()),
            },
            // `unit` is a seal id here, for the same reason: a receipt is
            // named by the Seal, not by the Ask it closed.
            "seal" => match crate::ringframe::seal_checked(self.outside.as_ref(), &root, unit) {
                Ok(v) => Answer::Ok(v),
                Err(e) => Answer::No("refused", format!("RingFrame would not check it: {e:?}")),
            },
            other => Answer::No("unknown_read", format!("there is nothing called {other}")),
        }
    }

    /// The RINGFRAME view: `look` at RingFrame's configuration and every
    /// harness file, `locate` one's program at `path`, `run` one's setup or
    /// update, or run `all` that is behind (the configuration first, then each
    /// harness by title). Off the run loop: asking a harness costs processes,
    /// and its commands can take a minute. Every change goes to the clients as
    /// `Event::Setup`, and each harness's readiness is told as it is asked.
    fn setup(&mut self, at: usize, act: &str, harness: &str, path: &str) -> Answer {
        let tx = self.tx.clone();
        let config_path = self.config.clone();
        let sense_slot = Arc::clone(&self.sense);
        let harnesses = self.projects[at].harnesses.clone();
        let (act, harness, path) = (act.to_string(), harness.to_string(), path.to_string());
        let out = self.outside.clone();
        if !matches!(act.as_str(), "look" | "locate" | "run" | "all" | "sense") {
            return Answer::No("unknown_act", format!("the RingFrame view has no {act}"));
        }
        if matches!(act.as_str(), "locate" | "run")
            && harness != "sense"
            && harnesses.find(&harness).is_none()
        {
            return Answer::No(
                "unknown_harness",
                format!("Weft has no harness file for {harness}"),
            );
        }
        std::thread::spawn(move || {
            use crate::onboarding::{look, look_as_they_answer, pending, proceed, run, runs, with};
            let weft_dir = config_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
            let show = |v: &weft_core::onboarding::View| {
                let view = serde_json::to_value(v).unwrap_or_default();
                let _ = tx.send(Wake::Setup { project: at, view });
            };
            let out = out.as_ref();
            // Opening the view draws it at once: each harness found reads as
            // being asked, and the release as pending, until they answer.
            if act == "look" {
                show(&pending(out, &harnesses));
            }
            let cli = crate::ringframe::installed(out);
            // Opening the view asks both releases again, at once: Weft's and
            // RingFrame's; the other acts read the kept answers.
            let fresh = act == "look";
            let (mut check, mut weft) = std::thread::scope(|scope| {
                let release = scope.spawn(|| out.release(fresh));
                let weft = scope.spawn(|| crate::onboarding::weft_check(out, fresh));
                (
                    release.join().unwrap_or_else(|_| Err("the check did not finish".into())),
                    weft.join().unwrap_or_default(),
                )
            });
            let mut harnesses = harnesses;
            let mut note = None;
            // A step's row shows it running; a failed one stays on its row
            // until the next look.
            let mut failed: Option<(String, weft_core::onboarding::State)> = None;
            match act.as_str() {
                "locate" => match out.check_program(&path).and_then(|p| {
                    crate::onboarding::save(&config_path, &harness, "program", p.clone().into())
                        .map(|()| p)
                }) {
                    Ok(program) => {
                        harnesses =
                            harnesses.with_programs(|n| (n == harness).then(|| program.clone()));
                        let _ = tx.send(Wake::Program { harness: harness.clone(), program });
                    }
                    Err(e) => note = Some(e),
                },
                "sense" => {
                    if !path.is_empty() {
                        if let Err(e) = crate::versions::hot_reload_sense(
                            &weft_dir,
                            Path::new(&path),
                            &sense_slot,
                        ) {
                            note = Some(format!("Sense update failed: {e}"));
                        }
                    } else {
                        let step = vec![weft_core::sync::Step::new("weft", &["update", "--sense"])];
                        if let Some(weft_core::onboarding::State::Failed { step, said }) =
                            proceed(out, step, |_| {})
                        {
                            note = Some(format!("{step} failed: {said}"));
                        } else if let Some(model) = crate::versions::load_sense(&weft_dir, false)
                            && let Ok(mut guard) = sense_slot.write()
                        {
                            *guard = Some(model);
                        }
                    }
                }
                "run" | "all" => {
                    if act == "run" && harness == "sense" {
                        if !path.is_empty() {
                            if let Err(e) = crate::versions::hot_reload_sense(
                                &weft_dir,
                                Path::new(&path),
                                &sense_slot,
                            ) {
                                note = Some(format!("Sense update failed: {e}"));
                            }
                        } else {
                            let step =
                                vec![weft_core::sync::Step::new("weft", &["update", "--sense"])];
                            if let Some(weft_core::onboarding::State::Failed { step, said }) =
                                proceed(out, step, |_| {})
                            {
                                note = Some(format!("{step} failed: {said}"));
                            } else if let Some(model) =
                                crate::versions::load_sense(&weft_dir, false)
                                && let Ok(mut guard) = sense_slot.write()
                            {
                                *guard = Some(model);
                            }
                        }
                    } else {
                        let (base, _) = look(out, &harnesses, &check, &weft, cli);
                        // Weft first: it brings RingFrame's program with it, and the
                        // steps after it run on that.
                        if act == "all" && base.weft_behind {
                            let updating = weft_core::onboarding::View {
                                weft: "updating…".into(),
                                ..base.clone()
                            };
                            show(&updating);
                            let step = vec![weft_core::sync::Step::new("weft", &["update"])];
                            if let Some(weft_core::onboarding::State::Failed { step, said }) =
                                proceed(out, step, |_| {})
                            {
                                note = Some(format!("{step} failed: {said}"));
                            }
                            weft = crate::onboarding::weft_check(out, true);
                        }
                        let (base, _) = look(out, &harnesses, &check, &weft, cli);
                        if act == "all" && base.configuration_behind {
                            let sync = vec![weft_core::sync::Step::new("ringframe", &["sync"])];
                            if let Some(weft_core::onboarding::State::Failed { step, said }) =
                                proceed(out, sync, |_| {})
                            {
                                note = Some(format!("{step} failed: {said}"));
                            }
                            check = out.release(true);
                        }
                        let (base, _) = look(out, &harnesses, &check, &weft, cli);
                        if act == "all" && base.sense_behind {
                            if !path.is_empty() {
                                if let Err(e) = crate::versions::hot_reload_sense(
                                    &weft_dir,
                                    Path::new(&path),
                                    &sense_slot,
                                ) {
                                    note = Some(format!("Sense update failed: {e}"));
                                }
                            } else {
                                let step = vec![weft_core::sync::Step::new(
                                    "weft",
                                    &["update", "--sense"],
                                )];
                                if let Some(weft_core::onboarding::State::Failed { step, said }) =
                                    proceed(out, step, |_| {})
                                {
                                    note = Some(format!("{step} failed: {said}"));
                                } else if let Some(model) =
                                    crate::versions::load_sense(&weft_dir, false)
                                    && let Ok(mut guard) = sense_slot.write()
                                {
                                    *guard = Some(model);
                                }
                            }
                        }
                        let (base, _) = look(out, &harnesses, &check, &weft, cli);
                        let todo: Vec<String> = base
                            .rows
                            .iter()
                            .filter(|r| if act == "run" { r.name == harness } else { runs(r) })
                            .map(|r| r.name.clone())
                            .collect();
                        for name in todo {
                            let h = harnesses.find(&name).cloned().expect("a row is a harness");
                            let latest = base.plugin.clone();
                            if let Some(state) = run(out, &h, cli, latest.as_deref(), |state| {
                                show(&with(base.clone(), &name, state));
                            }) {
                                failed = Some((name, state));
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
            // Looked at again, whatever was done: the tick is the harness's
            // answer now, never a command's exit code.
            let (mut v, states) = look_as_they_answer(out, &harnesses, &check, &weft, cli, |v| {
                let mut v = v.clone();
                v.note = note.clone();
                show(&v);
            });
            if let Some(ver) = crate::versions::sense_version(&weft_dir).or_else(|| {
                sense_slot.read().ok().and_then(|g| g.as_ref().map(|m| m.version().to_string()))
            }) {
                let (line, step) = weft_core::sync::sense(Some(&ver), v.sense_latest.as_deref());
                v.sense = line;
                v.sense_behind = step.is_some();
            }
            v.note = note;
            if let Some((name, state)) = failed {
                v = with(v, &name, state);
            }
            for (harness, state) in states {
                let _ = tx.send(Wake::Readiness { project: at, harness, state });
            }
            show(&v);
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
        let out = self.outside.clone();
        std::thread::spawn(move || {
            let out = out.as_ref();
            let cli = crate::ringframe::installed(out);
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
            let mut state = crate::readiness::check(out, h, cli);
            for _ in 0..2 {
                if state != Readiness::Unknown {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
                state = crate::readiness::check(out, h, cli);
            }
            let _ = tx.send(Wake::Readiness { project: at, harness: name, state });
        });
    }

    fn tell_readiness(&mut self, at: usize) {
        let states = readiness_json(&self.projects[at].readiness);
        self.clients.broadcast(at, &Out::Readiness { states });
    }

    /// Load a project's suggestion entries, or build them again when there
    /// are none kept or they are of an old version, off the run loop.
    fn read_suggestions(&mut self, at: usize) {
        let (config, root) = (self.config.clone(), self.projects[at].root.clone());
        let harnesses = self.projects[at].harnesses.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let (entries, built) = match crate::suggest::load(&config, &root) {
                Some(kept) => (kept, false),
                None => (crate::suggest::build(&config, &root, &harnesses), true),
            };
            let _ = tx.send(Wake::Suggested { root, source: None, entries, built });
        });
    }

    /// Take a project's suggestion entries, or one source's again, and keep
    /// them. Only this loop writes the index, so two writes never cross.
    fn suggested(
        &mut self,
        root: &Path,
        source: Option<String>,
        entries: Vec<weft_core::suggest::Entry>,
        built: bool,
    ) {
        self.suggest_changes += 1;
        let Some(p) = self.projects.iter_mut().find(|p| p.root == root && !p.closed) else {
            return;
        };
        match source {
            None => {
                if built {
                    let _ = crate::suggest::save(&self.config, root, &entries);
                }
                p.suggest = Some(entries);
            }
            Some(source) => {
                // Not loaded yet: the load reads this source too.
                let Some(all) = p.suggest.as_mut() else { return };
                let before = all.clone();
                crate::suggest::replace(all, &source, entries);
                if *all != before {
                    let _ = crate::suggest::save(&self.config, root, all);
                }
            }
        }
        self.embed_suggestions(root);
    }

    /// `pane.suggest`: what the pane's message field could show, or nothing:
    /// suggestions off here, no such pane, or nothing near enough.
    fn suggestion(&mut self, client: u64, pane: u32, draft: &str) -> Answer {
        let Some(at) = self.project_of(client) else { return Answer::no_project() };
        let p = &self.projects[at];
        let Some(slot) = p.panes.iter().find(|s| s.map.pane == pane) else {
            return Answer::No("no_pane", "there is no such pane here".into());
        };
        if !p.suggest_on {
            return Answer::Ok(serde_json::json!({}));
        }
        let (harness, reply) = (slot.map.harness.clone(), slot.reply.clone());
        // Built again only after a project's entries or the habits changed.
        let changes = self.suggest_changes;
        if p.index.as_ref().is_none_or(|(at_change, _)| *at_change != changes) {
            // Every other open project's short messages count as habits too.
            let open: Vec<weft_core::suggest::Entry> = self
                .projects
                .iter()
                .filter(|o| o.root != p.root && !o.closed)
                .flat_map(|o| o.suggest.iter().flatten().cloned())
                .collect();
            let others: Vec<_> = self.habits.iter().cloned().chain(open).collect();
            let own = p.suggest.as_deref().unwrap_or_default();
            let index = crate::suggest::index_of(own, &others, &p.root);
            self.projects[at].index = Some((changes, index));
        }
        let Some((_, index)) = self.projects[at].index.as_ref() else {
            return Answer::Ok(serde_json::json!({}));
        };
        let now = (crate::turns::now_millis() / 1000).max(0) as u64;
        match crate::suggest::ask(index, &harness, draft, (&reply.0, &reply.1), now) {
            Some(s) => Answer::Ok(serde_json::json!({"text": s.text, "score": s.score})),
            None => Answer::Ok(serde_json::json!({})),
        }
    }

    /// Embed the replies not embedded yet, off the run loop, when Sense is loaded.
    fn embed_suggestions(&mut self, root: &Path) {
        let Some(p) = self.projects.iter_mut().find(|p| p.root == root && !p.closed) else {
            return;
        };
        let todo = p.suggest.as_deref().map(crate::suggest::unembedded).unwrap_or_default();
        if p.embedding || todo.is_empty() {
            return;
        }
        p.embedding = true;
        let (sense, tx, root) = (Arc::clone(&self.sense), self.tx.clone(), root.to_path_buf());
        std::thread::spawn(move || {
            let vectors = match sense.read().ok().as_deref().and_then(Option::as_ref) {
                Some(model) => {
                    todo.into_iter().map(|(k, reply)| (k, model.embed(&reply))).collect()
                }
                None => Vec::new(),
            };
            let _ = tx.send(Wake::Embedded { root, vectors });
        });
    }

    /// Take Sense's embeddings of a project's replies, and keep them.
    fn embedded(&mut self, root: &Path, vectors: Vec<(crate::suggest::Key, Vec<f32>)>) {
        let Some(p) = self.projects.iter_mut().find(|p| p.root == root && !p.closed) else {
            return;
        };
        p.embedding = false;
        let got = !vectors.is_empty();
        if let Some(all) = p.suggest.as_mut()
            && crate::suggest::embedded(all, vectors)
        {
            let _ = crate::suggest::save(&self.config, root, all);
            self.suggest_changes += 1;
        }
        // What changed meanwhile is embedded next; with no Sense, nothing is.
        if got {
            self.embed_suggestions(root);
        }
    }

    /// Tell each project's clients when its record has moved, or an agent's
    /// state has.
    fn follow_the_record(&mut self) {
        for at in 0..self.projects.len() {
            if self.projects[at].closed {
                continue;
            }
            self.recheck(at);
            self.type_what_started(at);
            self.nothing_received(at);
            if self.projects[at].ledger.refresh() {
                let board = self.board_of(at);
                self.clients.broadcast(at, &board);
                let p = &self.projects[at];
                let asks = crate::suggest::of_asks(&p.root, &p.ledger.units());
                let root = p.root.clone();
                self.suggested(&root, Some("ask:".into()), asks, false);
            }
            self.tell_agents(at);
        }
    }

    /// Tell a project's clients each agent's state, when it moved.
    fn tell_agents(&mut self, at: usize) {
        let agents = self.agents_of(at);
        if self.projects[at].agents.as_ref() != Some(&agents) {
            self.projects[at].agents = Some(agents.clone());
            let (panes, sessions) = agents;
            let zone = crate::turns::zone();
            self.clients.broadcast(at, &Out::Agents { panes, sessions, zone });
        }
    }

    /// While RingFrame is not set up in a project, ask it again every few
    /// seconds, off the run loop, so each line of the checklist turns as the
    /// person fixes it, and the project is set up as soon as they all hold.
    fn recheck(&mut self, at: usize) {
        let p = &mut self.projects[at];
        if p.setup.is_none() || p.asking || p.asked.elapsed() < RECHECK_EVERY {
            return;
        }
        p.asking = true;
        let (tx, out, root) = (self.tx.clone(), self.outside.clone(), p.root.clone());
        std::thread::spawn(move || {
            let setup = crate::ringframe::set_up(out.as_ref(), &root);
            let _ = tx.send(Wake::Workspace { project: at, setup });
        });
    }

    /// What RingFrame said when it was asked again: told to the project's
    /// clients when it moved, with the Ask's gap asked again beside it.
    fn set_up(&mut self, at: usize, setup: Option<weft_core::workspace::Needs>) {
        let Some(p) = self.projects.get_mut(at) else { return };
        (p.asking, p.asked) = (false, std::time::Instant::now());
        if p.setup == setup {
            return;
        }
        p.setup = setup;
        let gap = workspace_gap(self.outside.as_ref(), &p.root);
        self.clients.broadcast(at, &Out::Workspace { setup, gap });
    }

    /// Open again the projects `projects.json` (or `state.json` when absent) lists, as an
    /// earlier daemon left it: each reopened now, with no agent running in it.
    fn reopen(&mut self, mark: bool) {
        let weft_dir = self.config.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        let proj_file = weft_dir.join("projects.json");
        let paths: Vec<PathBuf> = if proj_file.exists() {
            let text = std::fs::read_to_string(&proj_file).unwrap_or_default();
            weft_core::projects::read(&text)
                .into_iter()
                .map(|o| weft_core::state::normalize_project_path(&o.path))
                .collect()
        } else {
            crate::state::load(&weft_dir).open_projects
        };
        let now = crate::turns::now_millis();
        for path in paths {
            if path.is_dir() && self.projects.iter().all(|p| p.root != path) {
                let at = self.project_at(path);
                self.projects[at].reopened = mark.then_some(now);
            }
        }
        self.write_projects();
    }

    /// `update {version}`: a newer Weft is in `versions/<version>`; start on
    /// it once no agent runs. Every client is told it waits.
    fn update_to(&mut self, version: String) -> Answer {
        let plain = !version.is_empty()
            && version.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            && !version.starts_with('.');
        let program = self.config.with_file_name("versions").join(&version).join("weft");
        if !plain || !program.is_file() {
            return Answer::No("no_version", format!("Weft {version} is not installed here"));
        }
        // Already this one: nothing to wait for, and no restart to make.
        if version == env!("CARGO_PKG_VERSION") {
            return Answer::Ok(serde_json::json!({"update": version}));
        }
        self.update = Some(version.clone());
        self.clients.broadcast_all(&Out::Update { version: Some(version.clone()) });
        Answer::Ok(serde_json::json!({"update": version}))
    }

    /// With an update waiting and no agent running in any project, become
    /// the new version: the same process, so a service that runs it keeps
    /// it; windows and `weft remote` lose their connections and come back.
    fn update_when_idle(&mut self) {
        let Some(version) = self.update.clone() else { return };
        let busy = self
            .projects
            .iter_mut()
            .filter(|p| !p.closed)
            .any(|p| p.panes.iter_mut().any(|s| s.pane.running()));
        if busy {
            return;
        }
        let program = self.config.with_file_name("versions").join(&version).join("weft");
        use std::os::unix::process::CommandExt as _;
        let failed = std::process::Command::new(&program).arg("--serve").env(UPDATED, "1").exec();
        // Only reached when it could not start: stay on this version, and say so.
        eprintln!("weft: could not start {}: {failed}", program.display());
        self.update = None;
        self.clients.broadcast_all(&Out::Update { version: None });
    }

    /// Someone acted in this project: it is no longer only reopened, and the
    /// rule every project closes by holds for it again.
    fn acted_in(&mut self, at: usize) {
        if self.projects.get_mut(at).and_then(|p| p.reopened.take()).is_some() {
            self.clients.broadcast(at, &Out::Reopened { since: None });
        }
    }

    /// The projects open here, beside `config.toml`, synced atomically to both
    /// `state.json` and `projects.json` (`ADR-0003`).
    fn write_projects(&self) {
        let open: Vec<_> = self
            .projects
            .iter()
            .filter(|p| !p.closed)
            .map(|p| weft_core::projects::Opened { path: p.root.clone(), opened: p.opened })
            .collect();
        let weft_dir = self.config.parent().unwrap_or_else(|| Path::new("."));
        crate::state::sync_open_projects(weft_dir, &open);
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
        let work = weft_core::work::work(&units, &self.projects[at].ledger.record());
        Out::Units { units, records: serde_json::Value::Object(records), work }
    }

    /// The project this client is watching, if it has opened one.
    fn project_of(&self, client: u64) -> Option<usize> {
        self.clients.watching.get(&client).copied()
    }

    /// A client's connection ended. A client leaving is not the session
    /// ending: its agents keep running. Only what was its own is forgotten,
    /// and a project it leaves with no window and no agent running closes.
    fn left(&mut self, client: u64) {
        let had = self.clients.watching.remove(&client);
        self.clients.sinks.remove(&client);
        self.clients.sizes.remove(&client);
        self.clients.quiet.remove(&client);
        self.clients.names.remove(&client);
        self.attached.retain(|a| a.client != client);
        for slot in self.projects.iter_mut().flat_map(|p| &mut p.panes) {
            slot.wants.remove(&client);
        }
        if let Some(at) = had {
            self.close_if_idle(at);
        }
    }

    /// Close a project no client has open and none of whose agents runs: it
    /// stays listed only while there is someone or something in it. `weft
    /// remote` opens every listed project for the devices, so it does not
    /// count; its connection to a closed project ends, and it lets go of the
    /// project too.
    fn close_if_idle(&mut self, at: usize) {
        let windows = self
            .clients
            .watching
            .iter()
            .any(|(c, p)| *p == at && !self.clients.named(*c, "weft-remote"));
        let Some(p) = self.projects.get_mut(at).filter(|p| !p.closed) else { return };
        if windows || p.reopened.is_some() || p.panes.iter_mut().any(|s| s.pane.running()) {
            return;
        }
        p.closed = true;
        let others: Vec<u64> =
            self.clients.watching.iter().filter(|(_, p)| **p == at).map(|(c, _)| *c).collect();
        for c in others {
            if let Some(s) = self.clients.sinks.remove(&c) {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            self.clients.watching.remove(&c);
        }
        self.write_projects();
    }

    /// The project this client is watching, and its panes.
    fn watched(&mut self, client: u64) -> Option<(usize, &mut Project)> {
        let at = self.project_of(client)?;
        self.projects.get_mut(at).map(|p| (at, p))
    }

    /// The project at this path, opened if this is the first client to ask.
    fn project_at(&mut self, root: PathBuf) -> usize {
        let root = weft_core::state::normalize_project_path(&root);
        match self.projects.iter().position(|p| p.root == root && !p.closed) {
            Some(at) => {
                self.projects[at].root = root;
                at
            }
            None => {
                let ledger = crate::ledger::Ledger::at(&root);
                let (harnesses, unread) = match &self.harnesses {
                    Some(h) => (h.clone(), Vec::new()),
                    None => crate::harness::installed(&self.config.with_file_name("harnesses")),
                };
                let config = crate::routing::read_at(&self.config, &harnesses);
                // Where the person said a harness's program is, when not on PATH.
                let harnesses = harnesses.with_programs(|n| config.program(n).map(str::to_string));
                // Set up for RingFrame on the way in, when it can be: the
                // person is not asked, and the tree says what is missing.
                let setup = crate::ringframe::set_up(self.outside.as_ref(), &root);
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
                    suggest: None,
                    embedding: false,
                    suggest_on: config.suggest(),
                    index: None,
                    waiting: Vec::new(),
                    // Harnesses a test hands over are its fixture of a machine
                    // set up for RingFrame; the files' harnesses are asked.
                    readiness: match self.harnesses.as_ref().filter(|_| self.assume_ready) {
                        Some(h) => h
                            .iter()
                            .map(|h| (h.name.clone(), weft_core::readiness::Readiness::Ready))
                            .collect(),
                        None => HashMap::new(),
                    },
                    starting: Vec::new(),
                    due: Vec::new(),
                    prefixes: crate::acts::Prefixes::asking(self.outside.clone()),
                    turns: crate::turns::Turns::default(),
                    harnesses,
                    unread,
                    agents: None,
                    gone: Default::default(),
                    setup,
                    asked: std::time::Instant::now(),
                    asking: false,
                    closed: false,
                    reopened: None,
                });
                let at = self.projects.len() - 1;
                self.read_suggestions(at);
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
            Call::Hello { client: name, protocol } => {
                self.clients.names.insert(client, name);
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
            Call::Open { path: project, rows, cols, raw } => {
                self.clients.sizes.insert(client, (rows, cols));
                if raw {
                    self.clients.quiet.remove(&client);
                } else {
                    self.clients.quiet.insert(client);
                }
                let at = self.project_at(PathBuf::from(project));
                self.clients.watching.insert(client, at);
                // A window holds it now; `weft remote` opening it is not one.
                if !self.clients.named(client, "weft-remote") {
                    self.acted_in(at);
                }
                // An act routed to a harness with no pane yet still needs its
                // readiness, or it reads as "could not ask" rather than as a
                // missing pane.
                for harness in self.projects[at].routing.harnesses() {
                    if !self.projects[at].readiness.contains_key(&harness) {
                        self.look_at(at, &harness);
                    }
                }
                self.projects[at].ledger.refresh();
                let Out::Units { units, records, work } = self.board_of(at) else { unreachable!() };
                let panes = self.pane_infos(at);
                let (agent_panes, sessions) = self.agents_of(at);
                // Everything comes back in the answer — the board, the panes,
                // and each pane's screen with its history — rather than as
                // events sent before it. A client that waits for its answer
                // cannot miss what it asked for, and events from here on carry
                // only what changes: output numbered after the screen's.
                let screens: Vec<_> = self.projects[at]
                    .panes
                    .iter()
                    .filter(|_| raw)
                    .map(|s| {
                        let (bytes, seq, (rows, cols)) = s.pane.screen_at(HISTORY_MAX);
                        weft_proto::screen_json(&weft_proto::OpenScreen {
                            pane: s.map.pane,
                            seq,
                            rows,
                            cols,
                            bytes,
                        })
                    })
                    .collect();
                return Ok(Answer::Ok(serde_json::json!({
                    "panes": weft_proto::panes_json(&panes),
                    "units": serde_json::to_value(&units).unwrap_or_default(),
                    "records": records,
                    "work": work,
                    "screens": screens,
                    // What is already waiting on a person, so a client that
                    // arrives after it was staged can still answer it.
                    "remote": self.clients.remote.as_ref().map(|(_, v)| v),
                    "pendings": self.projects[at]
                        .waiting
                        .iter()
                        .map(|w| {
                            weft_proto::pending_json(
                                &w.id,
                                w.pane,
                                &w.what,
                                &w.why,
                                &w.payload,
                                w.sends.as_deref(),
                            )
                        })
                        .collect::<Vec<_>>(),
                    "readiness": readiness_json(&self.projects[at].readiness),
                    "routing": routing_json(&self.projects[at].routing),
                    "agents": {"panes": agent_panes, "sessions": sessions,
                               "zone": crate::turns::zone()},
                    "notify": self.projects[at].notify,
                    "turbo": self.projects[at].turbo,
                    "start_at_login": self.start_at_login(),
                    "reopened": self.projects[at].reopened,
                    "update": self.update,
                    "harnesses": &self.projects[at].harnesses,
                    // Harness files that would not read, to be said once.
                    "unread": &self.projects[at].unread,
                    // Why this workspace could not finish an Ask, if it could not.
                    "gap": workspace_gap(self.outside.as_ref(), &self.projects[at].root),
                    // What it still needs before RingFrame is set up in it.
                    "setup": self.projects[at].setup,
                })));
            }
            Call::Input { pane, bytes } => {
                self.clients.typed(client);
                if let Some(slot) = self.watched(client).and_then(|(_, p)| p.slot(pane)) {
                    slot.follow(client);
                    let _ = slot.pane.send(&bytes);
                }
            }
            Call::StartAgent { harness, spec, session } => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                // A resume names a session the index holds, and runs that id
                // with the harness file's own resume line.
                let spec = match &session {
                    None => spec,
                    Some(id) => match self.resumable(at, &harness, id) {
                        Ok(spec) => spec,
                        Err(no) => return Ok(no),
                    },
                };
                self.acted_in(at);
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
                if let Some(at) = self.project_of(client) {
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
            Call::Attach { pane, form } => return Ok(self.attach(client, pane, &form)),
            Call::Suggest { pane, draft } => return Ok(self.suggestion(client, pane, &draft)),
            Call::Stage { pane, bytes, how, what, why } => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                let id = format!("pnd_{}", self.next_pending);
                self.next_pending += 1;
                let message = Out::Pending {
                    id: id.clone(),
                    pane,
                    what: what.clone(),
                    why: why.clone(),
                    payload: bytes.clone(),
                    sends: None,
                };
                self.projects[at].waiting.push(Waiting {
                    id: id.clone(),
                    pane,
                    what,
                    why,
                    payload: bytes,
                    how,
                    confirm: None,
                    sends: None,
                });
                self.clients.broadcast(at, &message);
                return Ok(Answer::Ok(serde_json::json!({"pending": id})));
            }
            Call::Act { act, unit, pane, text, eval, start } => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                let on = On {
                    unit: unit.as_deref(),
                    pane,
                    text: text.as_deref(),
                    eval: eval.as_deref(),
                    start: start.as_deref(),
                };
                return Ok(self.act(client, at, &act, on));
            }
            Call::ConfirmAsk { unit } => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                let root = self.projects[at].root.clone();
                return Ok(
                    match crate::ringframe::ask_confirm(self.outside.as_ref(), &root, &unit) {
                        Ok(()) => Answer::nothing(),
                        Err(e) => Answer::No("refused", format!("{e:?}")),
                    },
                );
            }
            Call::Read { what, unit, pane, lines, before, after } => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                if what == "history" {
                    let lines = lines.map_or(HISTORY_MAX, |n| n as usize).min(HISTORY_MAX);
                    return Ok(match pane.and_then(|p| self.projects[at].slot(p)) {
                        Some(slot) => {
                            Answer::Ok(serde_json::json!({"text": slot.pane.history(lines)}))
                        }
                        None => Answer::No("no_pane", "there is no such pane here".into()),
                    });
                }
                if what == "conversation" {
                    return Ok(self.conversation(at, pane, before, after));
                }
                return Ok(self.read(at, &what, &unit));
            }
            Call::Setup { act, harness, path } => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                return Ok(self.setup(at, &act, &harness, &path));
            }
            Call::Available => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                // A harness not asked yet is asked now, off the loop: it is
                // offered once it says it is set up.
                let unasked: Vec<String> = {
                    let p = &self.projects[at];
                    p.harnesses
                        .iter()
                        .filter(|h| {
                            self.outside.on_path(&h.program) && !p.readiness.contains_key(&h.name)
                        })
                        .map(|h| h.name.clone())
                        .collect()
                };
                for name in unasked {
                    self.look_at(at, &name);
                }
                let p = &self.projects[at];
                let found = starts(self.outside.as_ref(), &p.harnesses, &p.readiness);
                return Ok(Answer::Ok(serde_json::json!({"starts": found})));
            }
            Call::Turbo { on } => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                // Held while the daemon runs; `config.toml` says where it starts.
                self.projects[at].turbo = on;
                // Every client of the project, a device's tree among them.
                self.clients.broadcast(at, &Out::Turbo { on });
                return Ok(Answer::Ok(serde_json::json!({"turbo": on})));
            }
            Call::RemoteView { view } => {
                if !self.clients.named(client, "weft-remote") {
                    return Ok(Answer::No("not_remote", "only weft remote has a view".into()));
                }
                self.clients.remote = Some((client, view.clone()));
                self.clients.broadcast_all(&Out::Remote { view });
                // Turned off before it first spoke: it was told nothing then.
                if !self.remote_wanted() {
                    let stop = Out::RemoteAct { act: serde_json::json!({"act": "stop"}) };
                    self.clients.send_to(client, &Line::Event(stop));
                }
            }
            Call::StartAtLogin { on, program, path } => {
                return Ok(self.turn_start_at_login(on, &program, &path));
            }
            Call::Remote { act } if matches!(act["act"].as_str(), Some("on" | "off")) => {
                return Ok(self.turn_remote(act["act"] == "on"));
            }
            Call::Remote { act } => {
                let to = self.clients.remote.as_ref().map(|(c, _)| *c);
                match to.filter(|c| self.clients.sinks.contains_key(c)) {
                    Some(to) => self.clients.send_to(to, &Line::Event(Out::RemoteAct { act })),
                    None => {
                        return Ok(Answer::No("remote_off", "weft remote is not running".into()));
                    }
                }
            }
            Call::Resolve { pending, yes, force, to, start } => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                // Taken, not read: whoever answers first answers for everyone,
                // and a second answer finds nothing to answer.
                let Some(i) = self.projects[at].waiting.iter().position(|w| w.id == pending) else {
                    return Ok(Answer::No("gone", "that was already answered".into()));
                };
                // Another agent, or a new one: the person's pick beside a send.
                let into = match (yes, to, start) {
                    (true, Some(pane), _)
                        if Some(pane) != Some(self.projects[at].waiting[i].pane) =>
                    {
                        match self.send_elsewhere(at, i, Into::Pane(pane)) {
                            Ok(into) => Some(into),
                            Err((code, why)) => return Ok(Answer::No(code, why)),
                        }
                    }
                    (true, None, Some(harness)) => {
                        match self.send_elsewhere(at, i, Into::Start(harness)) {
                            Ok(into) => Some(into),
                            Err((code, why)) => return Ok(Answer::No(code, why)),
                        }
                    }
                    _ => None,
                };
                let mut w = self.projects[at].waiting.remove(i);
                self.clients.broadcast(at, &Out::Resolved { id: pending, yes });
                if !yes {
                    return Ok(Answer::nothing());
                }
                let root = self.projects[at].root.clone();
                if let Err(why) = recorded_first(self.outside.as_ref(), &root, w.confirm.as_deref())
                {
                    return Ok(Answer::No("not_recorded", why));
                }
                if let Some(into) = into {
                    let pane = match into.to {
                        Into::Pane(pane) => pane,
                        Into::Start(harness) => match self.start_for(client, at, &harness) {
                            Some(pane) => pane,
                            None => {
                                let why = format!("Weft could not start {harness}");
                                return Ok(Answer::No("no_pane", why));
                            }
                        },
                    };
                    (w.pane, w.payload, w.how) = (pane, into.built.payload, into.built.how);
                }
                self.type_it(at, w, force);
            }
            // A client leaving is not the session ending. That is the point.
            // What it is done with is when its connection ends (`left`).
            Call::Detach => {}
            Call::Update { version } => return Ok(self.update_to(version)),
            Call::Dismiss => {
                let Some(at) = self.project_of(client) else { return Ok(Answer::no_project()) };
                let root = self.projects[at].root.clone();
                let weft_dir = self.config.parent().unwrap_or_else(|| Path::new("."));
                crate::state::remove_open_project(weft_dir, &root);
                crate::suggest::forget(&self.config, &root);
                self.projects[at].suggest = None;
                self.acted_in(at);
                // Closed once the answer is sent: closing ends this client's
                // connection to it.
                self.dismissed = Some(at);
            }
            Call::Shutdown => {
                // The person's stop: the next daemon opens nothing again.
                let weft_dir = self.config.parent().unwrap_or_else(|| Path::new("."));
                crate::state::sync_open_projects(weft_dir, &[]);
                return Ok(Answer::Done);
            }
        }
        Ok(Answer::nothing())
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
        let world = crate::outside::Fixed::new().answers(
            "ringframe",
            &["ask", "confirm"],
            2,
            "no such Ask",
        );
        assert_eq!(recorded_first(&world, nowhere, None), Ok(()), "nothing to record");
        let refused = recorded_first(&world, nowhere, Some("ask_nothing_here")).unwrap_err();
        assert!(refused.contains("Nothing was typed"), "{refused}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage_sense_bundle_at(dst: &Path, version: &str) {
        let workspace_bundle =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../sense/dist/fab7-sense-v1");
        let src = if workspace_bundle.exists() {
            workspace_bundle
        } else {
            let home = std::env::var_os("HOME").expect("HOME");
            PathBuf::from(home).join(".fab7/sense/fab7-sense-v1")
        };
        std::fs::create_dir_all(dst).unwrap();
        for name in ["sense.int8.onnx", "tokenizer.json", "calibration.json"] {
            let p = src.join(name);
            if p.exists() {
                std::fs::copy(p, dst.join(name)).unwrap();
            }
        }
        let mut manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(src.join("manifest.json")).unwrap())
                .unwrap();
        manifest["version"] = serde_json::json!(version);
        std::fs::write(dst.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap())
            .unwrap();
    }

    #[test]
    fn hot_reloading_sense_bundle_in_weftd_keeps_running_panes_alive() {
        let tmp = tempfile::tempdir().unwrap();
        let weft_dir = tmp.path().join("weft");
        let proj_dir = tmp.path().join("proj");
        std::fs::create_dir_all(&weft_dir).unwrap();
        std::fs::create_dir_all(&proj_dir).unwrap();

        let v1_src = tmp.path().join("sense_0.1.0");
        let v2_src = tmp.path().join("sense_0.2.0");
        stage_sense_bundle_at(&v1_src, "0.1.0");
        stage_sense_bundle_at(&v2_src, "0.2.0");
        crate::versions::install_sense_bundle(&weft_dir, &v1_src).unwrap();

        let sock = weft_dir.join("weft.sock");
        let cfg = weft_dir.join("config.toml");
        std::fs::write(&cfg, "").unwrap();

        let harnesses = weft_core::harness::fixture::harnesses();
        let sock_srv = sock.clone();
        let cfg_srv = cfg.clone();
        let srv = std::thread::spawn(move || {
            let _ = Session::serve_with(&sock_srv, &cfg_srv, Some(harnesses));
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !weft_proto::is_live(&sock) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let stream = UnixStream::connect(&sock).unwrap();
        let mut reader = Lines::new(stream.try_clone().unwrap());
        let mut writer = stream;

        weft_proto::send(
            &mut writer,
            &Line::Call { id: 1, call: Call::Hello { client: "test".into(), protocol: PROTOCOL } }
                .encode(),
        )
        .unwrap();
        let _ = reader.next().unwrap();

        weft_proto::send(
            &mut writer,
            &Line::Call {
                id: 2,
                call: Call::Open {
                    path: proj_dir.display().to_string(),
                    rows: 24,
                    cols: 80,
                    raw: true,
                },
            }
            .encode(),
        )
        .unwrap();
        let _ = reader.next().unwrap();

        // Verify state.json and projects.json were atomically written on project open
        let saved_state = crate::state::load(&weft_dir);
        assert_eq!(saved_state.open_projects, vec![proj_dir.clone()]);

        // Start a persistent shell pane and print its PID before Sense hot-reload
        weft_proto::send(
            &mut writer,
            &Line::Call {
                id: 3,
                call: Call::StartAgent {
                    harness: "claude-code".into(),
                    spec: "/bin/sh".into(),
                    session: None,
                },
            }
            .encode(),
        )
        .unwrap();
        let mut pane_id = None;
        while std::time::Instant::now() < deadline {
            if let Ok(Some(Line::Result { id: 3, result })) = reader.next() {
                pane_id = result["pane"].as_u64().map(|n| n as u32);
                break;
            }
        }
        let pane_id = pane_id.expect("started pane");
        weft_proto::send(
            &mut writer,
            &Line::Call {
                id: 4,
                call: Call::Input { pane: pane_id, bytes: b"echo PID_BEFORE=$$\n".to_vec() },
            }
            .encode(),
        )
        .unwrap();

        // Trigger Sense bundle hot-reload to 0.2.0 via Call::Setup
        weft_proto::send(
            &mut writer,
            &Line::Call {
                id: 5,
                call: Call::Setup {
                    act: "sense".into(),
                    harness: "sense".into(),
                    path: v2_src.display().to_string(),
                },
            }
            .encode(),
        )
        .unwrap();

        let mut updated_to_v2 = false;
        while std::time::Instant::now() < deadline {
            if let Ok(Some(Line::Event(Out::Setup { view }))) = reader.next()
                && view["sense"].as_str().is_some_and(|s| s.contains("0.2.0"))
            {
                updated_to_v2 = true;
                break;
            }
        }
        assert!(updated_to_v2, "weftd hot-reloaded Sense 0.2.0 in place");
        assert_eq!(crate::versions::sense_version(&weft_dir).as_deref(), Some("0.2.0"));

        // Verify the running pane was never restarted and still has the exact same shell PID
        weft_proto::send(
            &mut writer,
            &Line::Call {
                id: 6,
                call: Call::Input { pane: pane_id, bytes: b"echo PID_AFTER=$$\n".to_vec() },
            }
            .encode(),
        )
        .unwrap();

        let mut pid_before = None;
        let mut pid_after = None;
        while std::time::Instant::now() < deadline && (pid_before.is_none() || pid_after.is_none())
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
            weft_proto::send(
                &mut writer,
                &Line::Call {
                    id: 7,
                    call: Call::Read {
                        what: "history".into(),
                        unit: String::new(),
                        pane: Some(pane_id),
                        lines: Some(50),
                        before: None,
                        after: None,
                    },
                }
                .encode(),
            )
            .unwrap();
            while let Ok(Some(line)) = reader.next() {
                if let Line::Result { id: 7, result } = line {
                    let hist = result["text"].as_str().unwrap_or_default();
                    for l in hist.lines() {
                        // After a prompt, when it was typed before the shell drew one.
                        if let Some((_, rest)) = l.split_once("PID_BEFORE=")
                            && rest.chars().all(|c| c.is_ascii_digit())
                            && !rest.is_empty()
                        {
                            pid_before = Some(rest.to_string());
                        }
                        if let Some((_, rest)) = l.split_once("PID_AFTER=")
                            && rest.chars().all(|c| c.is_ascii_digit())
                            && !rest.is_empty()
                        {
                            pid_after = Some(rest.to_string());
                        }
                    }
                    break;
                }
            }
        }
        assert!(pid_before.is_some(), "pane printed PID before hot-reload");
        assert_eq!(pid_before, pid_after, "pane PID unchanged across Sense hot-reload");

        weft_proto::send(&mut writer, &Line::Call { id: 8, call: Call::Shutdown }.encode())
            .unwrap();
        let _ = srv.join();
    }

    #[test]
    fn sense_refines_unhooked_waiting_and_post_stop_goal_continuation_in_weftd() {
        let dir = tempfile::tempdir().unwrap();
        let weft_dir = dir.path().join(".fab7/weft");
        std::fs::create_dir_all(&weft_dir).unwrap();
        let socket = weft_dir.join("weft.sock");
        let config = weft_dir.join("config.toml");
        std::fs::write(&config, "").unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();

        let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../sense/dist/fab7-sense-v1");
        crate::versions::install_sense_bundle(&weft_dir, &bundle).unwrap();

        let harnesses = weft_core::harness::fixture::harnesses();
        let srv_sock = socket.clone();
        let srv_cfg = config.clone();
        let srv = std::thread::spawn(move || {
            let _ = Session::serve_with(&srv_sock, &srv_cfg, Some(harnesses));
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !weft_proto::is_live(&socket) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let stream = UnixStream::connect(&socket).unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_millis(500))).unwrap();
        let mut reader = Lines::new(stream.try_clone().unwrap());
        let mut writer = stream;

        fn call(
            writer: &mut UnixStream,
            reader: &mut Lines<UnixStream>,
            id: u64,
            c: Call,
        ) -> (serde_json::Value, Vec<Out>) {
            weft_proto::send(writer, &Line::Call { id, call: c }.encode()).unwrap();
            let mut events = Vec::new();
            while let Ok(Some(line)) = reader.next() {
                match line {
                    Line::Result { id: rid, result } if rid == id => return (result, events),
                    Line::Event(ev) => events.push(ev),
                    _ => {}
                }
            }
            panic!("did not receive Result for call {id}");
        }

        call(
            &mut writer,
            &mut reader,
            1,
            Call::Hello { client: "weft-desktop".into(), protocol: PROTOCOL },
        );
        call(
            &mut writer,
            &mut reader,
            2,
            Call::Open { path: project.display().to_string(), rows: 40, cols: 120, raw: true },
        );

        // 1. Spawn an Antigravity pane and emit `PreInvocation` ("working"), then print an
        // un-hooked Antigravity permission prompt and verify weftd transitions it to Turn::Waiting.
        let token_file = dir.path().join("agy.token");
        let agy_script = dir.path().join("agy-mock.sh");
        std::fs::write(
            &agy_script,
            format!(
                "#!/bin/sh\nstty raw -echo 2>/dev/null || true\nprintf '%s' \"$WEFT_PANE_TOKEN\" > '{}'\nexec /bin/cat\n",
                token_file.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&agy_script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let (res, _) = call(
            &mut writer,
            &mut reader,
            3,
            Call::StartAgent {
                harness: "antigravity".into(),
                spec: agy_script.display().to_string(),
                session: None,
            },
        );
        let agy_pane = res["pane"].as_u64().unwrap() as u32;

        while !token_file.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let agy_token = std::fs::read_to_string(&token_file).unwrap();

        let agy_transcript = dir.path().join("agy-transcript.jsonl");
        std::fs::write(
            &agy_transcript,
            include_str!("../../../weft-core/tests/fixtures/conversation/antigravity.jsonl"),
        )
        .unwrap();

        // Send PreInvocation hook ("working") tying session "s-agy-1"
        weft_proto::send(
            &mut writer,
            &Line::Event(Out::Signal(Box::new(weft_core::signal::Sent {
                pane: Some(agy_pane),
                token: Some(agy_token.clone()),
                harness: "antigravity".into(),
                session: "s-agy-1".into(),
                state: Some("working".into()),
                ..Default::default()
            })))
            .encode(),
        )
        .unwrap();

        let perm_prompt = concat!(
            "\x1b[2J\x1b[H",
            "◈ Propose command: rm -f /tmp/hostlab-perm-probe && touch /tmp/hostlab-perm-probe\r\n",
            "  Allow execution of this command?\r\n",
            "  ❯ 1. Yes, run command\r\n",
            "    2. Always allow in this workspace\r\n",
            "    3. No, cancel (Esc)\r\n"
        );
        call(
            &mut writer,
            &mut reader,
            4,
            Call::Input { pane: agy_pane, bytes: perm_prompt.as_bytes().to_vec() },
        );

        let mut saw_waiting = false;
        let wait_until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < wait_until && !saw_waiting {
            if let Ok(Some(Line::Event(Out::Agents { panes, .. }))) = reader.next()
                && panes.first().copied().flatten() == Some(Turn::Waiting)
            {
                saw_waiting = true;
            }
        }
        assert!(
            saw_waiting,
            "un-hooked Antigravity permission prompt transitions effective Turn to Turn::Waiting after 150ms settle"
        );

        // 2. Spawn a Claude Code pane, print a post-Stop /goal continuation check screen,
        // emit Stop ("turn_ended"), and verify weftd keeps Turn::Working and refuses direct send (D10.2 / D4.11).
        let claude_token_file = dir.path().join("claude.token");
        let claude_script = dir.path().join("claude-mock.sh");
        std::fs::write(
            &claude_script,
            format!(
                "#!/bin/sh\nstty raw -echo 2>/dev/null || true\nprintf '%s' \"$WEFT_PANE_TOKEN\" > '{}'\nexec /bin/cat\n",
                claude_token_file.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&claude_script, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let (res2, _) = call(
            &mut writer,
            &mut reader,
            5,
            Call::StartAgent {
                harness: "claude-code".into(),
                spec: claude_script.display().to_string(),
                session: None,
            },
        );
        let claude_pane = res2["pane"].as_u64().unwrap() as u32;
        while !claude_token_file.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let claude_token = std::fs::read_to_string(&claude_token_file).unwrap();

        let post_hook_screen = concat!(
            "\x1b[2J\x1b[38;1H",
            "● Done.\r\n",
            "◦ Verifying goal completion with judge… (12s · esc to interrupt)\r\n",
            "❯ "
        );
        call(
            &mut writer,
            &mut reader,
            6,
            Call::Input { pane: claude_pane, bytes: post_hook_screen.as_bytes().to_vec() },
        );
        std::thread::sleep(std::time::Duration::from_millis(200));

        weft_proto::send(
            &mut writer,
            &Line::Event(Out::Signal(Box::new(weft_core::signal::Sent {
                pane: Some(claude_pane),
                token: Some(claude_token),
                harness: "claude-code".into(),
                session: "s-claude-1".into(),
                state: Some("turn_ended".into()),
                ..Default::default()
            })))
            .encode(),
        )
        .unwrap();

        // Even though the Stop ("turn_ended") hook arrived, WorkingPostHook keeps effective Turn::Working!
        let mut saw_claude_working = false;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < until {
            if let Ok(Some(Line::Event(Out::Agents { panes, .. }))) = reader.next()
                && panes.get(claude_pane as usize).copied().flatten() == Some(Turn::Working)
            {
                saw_claude_working = true;
                break;
            }
        }
        assert!(
            saw_claude_working,
            "post-Stop /goal continuation check keeps effective Turn::Working in weftd"
        );

        // Once the screen settles to IdleReady, effective Turn transitions to Turn::TurnEnded.
        let idle_screen = concat!(
            "\x1b[2J\x1b[38;1H",
            "● Done.\r\n",
            "❯ \r\n",
            "  ⏵⏵ bypass permissions on · 42% context"
        );
        call(
            &mut writer,
            &mut reader,
            7,
            Call::Input { pane: claude_pane, bytes: idle_screen.as_bytes().to_vec() },
        );
        let mut saw_claude_ended = false;
        let until2 = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < until2 {
            if let Ok(Some(Line::Event(Out::Agents { panes, .. }))) = reader.next()
                && panes.get(claude_pane as usize).copied().flatten() == Some(Turn::TurnEnded)
            {
                saw_claude_ended = true;
                break;
            }
        }
        assert!(
            saw_claude_ended,
            "once screen settles to IdleReady after WorkingPostHook, effective Turn transitions to TurnEnded"
        );

        weft_proto::send(&mut writer, &Line::Call { id: 99, call: Call::Shutdown }.encode())
            .unwrap();
        let _ = srv.join();
    }
}
