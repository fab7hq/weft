//! The link to the daemon, its auto-start (`weft --serve`), and each opened
//! project as the daemon last said it (`ProjectClient` and its `PaneView`s).
//!
//! Nothing here waits for the daemon but connecting, which runs on a thread of
//! its own: a call is written by the link's writer thread, and its answer is
//! taken when it comes, with what it was for.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use serde_json::Value;
use weft_core::board::PaneInfo;
use weft_core::harness::Harnesses;
use weft_core::ledger::Unit;
use weft_core::readiness::Readiness;
use weft_core::routing::Routing;
use weft_core::turns::Turn;
use weft_core::work::Work;
use weft_proto::{Call, Event, Job, Line, Lines, PROTOCOL, clear_dead, is_live, socket_path};

/// The jobs whose failure the bar says when no wait of this window shows
/// it: a work's. Updates, Remote and a Seal's receipt say theirs in place.
const SAID_IN_THE_BAR: [&str; 6] = ["eval_open", "readers", "seal", "send", "ask", "undo"];

/// One pane, as `weft-desktop` sees it.
pub struct PaneView {
    pub id: u32,
    pub harness: String,
    pub spec: String,
    pub running: bool,
    pub turbo: bool,
    /// The Eval it judges, for a judge pane: shown under that Eval.
    pub judge: Option<String>,
    pub turn: Option<Turn>,
    term: weft_term::Term,
    /// The number of the last chunk the terminal holds: a chunk numbered no
    /// higher is already on the screen the daemon gave.
    seq: u64,
    /// The rows the window paints, each brought up to date only when the
    /// terminal says it changed.
    screen: Vec<Vec<weft_term::Cell>>,
}

impl PaneView {
    pub fn of(p: PaneInfo) -> Self {
        let mut v = Self::new(p.pane, p.harness, p.spec);
        v.running = p.running;
        v.turbo = p.turbo;
        v.judge = p.judge;
        v
    }

    pub fn new(id: u32, harness: String, spec: String) -> Self {
        let mut v = Self {
            id,
            harness,
            spec,
            running: true,
            turbo: false,
            judge: None,
            turn: None,
            term: Self::terminal(24, 80),
            seq: 0,
            screen: Vec::new(),
        };
        v.repaint();
        v
    }

    /// The daemon answers what a harness asks about its terminal; this copy
    /// of it never does.
    fn terminal(rows: u16, cols: u16) -> weft_term::Term {
        weft_term::Term::new(rows, cols, 10_000, |_| {})
    }

    /// Bring the rows the window paints up to date with the terminal.
    fn repaint(&mut self) {
        match self.term.changed_rows() {
            None => self.screen = self.term.cells(),
            Some(rows) => {
                for r in rows {
                    if let Some(row) = self.screen.get_mut(r as usize) {
                        *row = self.term.row(r);
                    }
                }
            }
        }
    }

    /// Start from the screen the daemon gave when the project was opened.
    pub fn show(&mut self, screen: &weft_proto::OpenScreen) {
        self.term = Self::terminal(screen.rows, screen.cols);
        self.term.feed(&screen.bytes);
        self.seq = screen.seq;
        self.repaint();
    }

    /// A chunk the pane printed, unless the screen already holds it.
    pub fn output(&mut self, seq: u64, bytes: &[u8]) {
        if seq > self.seq {
            self.seq = seq;
            self.feed(bytes);
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.term.feed(bytes);
        self.repaint();
    }

    pub fn contents(&self) -> String {
        self.term.contents()
    }

    /// The rows on view, a cell per column, as last brought up to date.
    pub fn screen(&self) -> &[Vec<weft_term::Cell>] {
        &self.screen
    }

    pub fn cursor(&self) -> (u16, u16) {
        self.term.cursor()
    }

    pub fn cursor_visible(&self) -> bool {
        self.term.cursor_visible()
    }

    pub fn modes(&self) -> weft_term::Modes {
        self.term.modes()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.bracketed_paste()
    }

    pub fn select_from(&mut self, at: (u16, u16)) {
        self.term.select_from(at);
        self.repaint();
    }

    pub fn select_to(&mut self, at: (u16, u16)) {
        self.term.select_to(at);
        self.repaint();
    }

    pub fn clear_selection(&mut self) {
        self.term.clear_selection();
        self.repaint();
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_text()
    }

    pub fn scroll(&mut self, delta: i32) {
        self.term.scroll(delta);
        self.repaint();
    }

    pub fn scroll_offset(&self) -> usize {
        self.term.scroll_offset()
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_to_bottom();
        self.repaint();
    }

    pub fn size(&self) -> (u16, u16) {
        self.term.size()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) != self.term.size() {
            self.term.resize(rows, cols);
            self.repaint();
        }
    }

    pub fn info(&self) -> PaneInfo {
        PaneInfo {
            pane: self.id,
            harness: self.harness.clone(),
            spec: self.spec.clone(),
            running: self.running,
            turbo: self.turbo,
            judge: self.judge.clone(),
        }
    }
}

/// A staged prompt waiting on the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub id: String,
    pub pane: u32,
    pub what: String,
    pub why: String,
    pub payload: Vec<u8>,
    pub sends: Option<String>,
    /// Its yes button, when not Send.
    pub go: Option<weft_proto::Go>,
}

impl Staged {
    /// What a dialog that types nothing says of itself in its payload: an
    /// Eval's, what it will read (`reads`).
    pub fn data(&self) -> Option<Value> {
        if self.payload.first() != Some(&b'{') {
            return None;
        }
        let v: Value = serde_json::from_slice(&self.payload).ok()?;
        v.get("reads").is_some_and(Value::is_object).then_some(v)
    }

    /// The bytes its yes types: none for a dialog whose payload says what it is.
    pub fn typed(&self) -> &[u8] {
        if self.data().is_some() { &[] } else { &self.payload }
    }
}

/// What a call's answer is for, kept until the answer comes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Then {
    /// An agent of `harness` started: show its pane; it resumes `session`
    /// when it names one.
    Started {
        harness: String,
        session: Option<String>,
    },
    /// A pane's conversation, read again.
    Conversation {
        pane: u32,
    },
    /// The page before a pane's conversation's first item.
    Older {
        pane: u32,
    },
    /// An Eval's record, when the board's records do not hold it.
    Judges {
        eval: String,
    },
    /// An Eval's view: its parts, steps and changes.
    EvalView {
        eval: String,
    },
    /// One change of an Eval, as lines.
    Window {
        eval: String,
        window: String,
    },
    /// A Seal's receipt, checked by RingFrame.
    Seal {
        seal: String,
    },
    /// What the person typed for an Ask, and the prompt composed from it.
    Source {
        unit: String,
    },
    Wording {
        unit: String,
    },
    /// Nothing but a refusal, which the person is shown.
    Refusal,
    /// Which files an Eval of the project should skip, looked at again.
    SkipCheck,
    /// A line a finding proposed, written into its file.
    SkipAdd {
        file: String,
        line: String,
    },
    /// An Eval's dialog staged: its pending id, or why not.
    Eval,
    /// RingFrame's practices and rules: every project's, or this project's too.
    Rules {
        project: bool,
    },
    /// The Rules block a route would get: `<host>|<route>|<task>`.
    Preview {
        unit: String,
    },
    /// What RingFrame says of a harness: its routes and its limits.
    Profile {
        host: String,
    },
    /// What the open Ask's sheet offers, for `{"harness", "words", "route"}`
    /// and the agent's pane it goes to, if it runs.
    AskSheet {
        query: String,
        pane: Option<u32>,
    },
    /// A follow-up's words for rejected work, from what its Eval found.
    Remedy {
        unit: String,
    },
    /// The commits Undo offers for rejected work.
    Commits {
        unit: String,
    },
    /// The questions an Eval left for its Seal.
    Questions {
        eval: String,
    },
    /// A page of the earlier sessions: every one, or what a search for `q`
    /// finds.
    Sessions {
        q: String,
    },
    /// A step of Updates (`look`, `run`, `all`, `locate`) for `target`: the
    /// daemon answers once it has ended.
    Setup {
        act: String,
        target: String,
    },
    /// A yes that waits on the machine: said in the bar, and on its work's
    /// row, until its answer comes; a refusal is said where it was.
    Working(Working),
}

impl Then {
    /// What Settings asks RingFrame, and the open Ask's sheet: asked again
    /// each time they open, so a refusal of them is forgotten then.
    pub fn is_settings(&self) -> bool {
        matches!(
            self,
            Then::Rules { .. }
                | Then::Preview { .. }
                | Then::Profile { .. }
                | Then::AskSheet { .. }
        )
    }

    /// A read whose answer is kept in [`ProjectClient::read`], or its
    /// refusal in [`ProjectClient::refused`].
    fn is_kept(&self) -> bool {
        self.is_settings()
            || matches!(
                self,
                Then::Judges { .. }
                    | Then::EvalView { .. }
                    | Then::Window { .. }
                    | Then::Seal { .. }
                    | Then::Source { .. }
                    | Then::Wording { .. }
            )
    }
}

/// What a yes waits on the machine for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Doing {
    /// A dialog being made ready: an Eval's, a stop's, a send's.
    Preparing,
    /// RingFrame opening an Eval, then its readers starting.
    Opening,
    Sealing,
    Sending,
    Asking,
    Stopping,
    Undoing,
    Saving,
}

impl Doing {
    /// What the bar says while it works.
    pub fn said(self) -> &'static str {
        weft_core::words::word(match self {
            Doing::Preparing => "wait.preparing",
            Doing::Opening => "wait.opening_eval",
            Doing::Sealing => "wait.sealing",
            Doing::Sending => "wait.sending",
            Doing::Asking => "wait.asking",
            Doing::Stopping => "wait.stopping_eval",
            Doing::Undoing => "wait.undoing",
            Doing::Saving => "wait.saving",
        })
    }

    /// The kind of job the machine runs for it, when it runs one.
    pub fn job(self) -> Option<&'static str> {
        match self {
            Doing::Opening => Some("eval_open"),
            Doing::Sealing => Some("seal"),
            Doing::Sending => Some("send"),
            Doing::Asking => Some("ask"),
            Doing::Undoing => Some("undo"),
            Doing::Preparing | Doing::Stopping | Doing::Saving => None,
        }
    }

    /// What its work's row says while it works, when its row says so.
    pub fn row(self) -> Option<&'static str> {
        match self {
            Doing::Opening => Some(weft_core::words::word("wait.row_opening_eval")),
            Doing::Sealing => Some(weft_core::words::word("wait.row_sealing")),
            _ => None,
        }
    }
}

/// A yes waiting on the machine: what it does, what about, and the work it
/// is for, by its root Ask.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Working {
    pub doing: Doing,
    pub about: String,
    pub work: Option<String>,
}

/// An answer the daemon gave, with what it was for.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub then: Then,
    pub answer: std::result::Result<Value, String>,
    /// A refusal's code, which a program branches on; empty for an answer.
    pub code: String,
}

/// The connection to the daemon: lines in from its reader thread, and lines
/// out to its writer thread.
struct Link {
    out: Sender<Vec<u8>>,
    inbox: Receiver<Line>,
    writer: Option<std::thread::JoinHandle<()>>,
    stream: UnixStream,
}

/// What a pump took: whether something other than a pane's terminal
/// changed, whether an event moved the project (not only a pane's output),
/// the panes whose terminal output came, and the answers.
#[derive(Debug, Default)]
pub struct Pumped {
    pub changed: bool,
    pub moved: bool,
    pub output: Vec<u32>,
    pub answered: Vec<Answered>,
}

/// What the window redraws after a pump: all of it, or only the terminals
/// of these panes.
#[derive(Debug, Default, PartialEq)]
pub struct Redraw {
    pub window: bool,
    pub terminals: Vec<(PathBuf, u32)>,
}

impl Link {
    fn over(stream: UnixStream) -> Result<Link> {
        let reading = stream.try_clone()?;
        let kept = stream.try_clone()?;
        let (tx, inbox) = channel();
        std::thread::spawn(move || {
            let mut lines = Lines::new(reading);
            while let Ok(Some(line)) = lines.next() {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let (out, queued) = channel::<Vec<u8>>();
        let mut writing = stream;
        let writer = std::thread::spawn(move || {
            for bytes in queued {
                if writing.write_all(&bytes).and_then(|()| writing.flush()).is_err() {
                    return;
                }
            }
        });
        Ok(Link { out, inbox, writer: Some(writer), stream: kept })
    }

    /// Close at once: the reader thread ends, and the daemon sees the client go.
    fn cut(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Hand over what is queued and close: the writer thread finishes first.
    fn close(mut self) {
        let Link { out, writer, .. } = &mut self;
        drop(std::mem::replace(out, channel().0));
        if let Some(w) = writer.take() {
            let _ = w.join();
        }
        self.cut();
    }
}

/// One opened project, as the daemon last said it.
pub struct ProjectClient {
    pub root: PathBuf,
    pub name: String,
    link: Option<Link>,
    /// What each call still unanswered is for, and since when.
    owed: HashMap<u64, (Then, Instant)>,
    /// The jobs the daemon says run here, and those that failed, until the
    /// person acts on them again.
    pub jobs: Vec<crate::waiting::Progress>,
    /// What failed of a job no wait of this window shows: said once, in
    /// the bar.
    pub jobs_failed: Vec<String>,
    /// Typing was refused: the window says it, once.
    pub typing_refused: Option<String>,
    /// How many times the daemon has said what `weft remote` is at.
    pub remotes: u64,
    told: HashMap<u32, (u16, u16)>,
    pub panes: Vec<PaneView>,
    pub unrecorded: Option<String>,
    pub units: Vec<Unit>,
    pub readiness: Value,
    pub setup: Option<weft_core::onboarding::View>,
    pub remote: Option<Value>,
    pub records: Value,
    pub work: Work,
    /// The daemon has said which panes the project has: a tab of any other is
    /// gone.
    pub opened: bool,
    pub boards: u64,
    pub routing: Routing,
    pub gap: Option<String>,
    pub needs: Option<weft_core::workspace::Needs>,
    pub waiting: Vec<Staged>,
    /// Which files an Eval of the project should skip, as the daemon's
    /// latest check found them: never a nested repository.
    pub skip: Vec<weft_core::skip::Finding>,
    /// Why the last line asked for was not written, or the check not run.
    pub skip_refused: Option<String>,
    /// The sessions the daemon carries as they are (the running, those
    /// panes hold, and the newest of today's), how many earlier ones it
    /// lists a page at a time, the pages of those read, and the pages a
    /// search through them found.
    pub sessions: Vec<weft_core::sessions::Row>,
    pub earlier: usize,
    pub earlier_rows: crate::earlier::Pages,
    pub earlier_found: Option<crate::earlier::Pages>,
    pub zone: i64,
    pub notify: bool,
    pub turbo: bool,
    pub start_at_login: bool,
    /// This project's settings, as the daemon last told them (`config`).
    pub config: Option<serde_json::Value>,
    /// What the daemon called this machine when it answered.
    pub machine_name: Option<String>,
    pub update: Option<String>,
    pub reopened_since: Option<i64>,
    pub harnesses: Harnesses,
    pub unread: Vec<String>,
    /// It was connected and the daemon went.
    pub lost: bool,
    /// What was read from RingFrame's record through the daemon, by what it
    /// was read for: an Eval's record, a Seal's checked receipt, an Ask's
    /// wording. What was not read, or could not be, is not here.
    pub read: HashMap<Then, Value>,
    /// What RingFrame would not say for Settings, by what it was asked for:
    /// its reason. Not asked again until a Settings sheet opens.
    pub refused: HashMap<Then, String>,
}

/// A daemon that speaks another protocol: another version of Weft.
#[derive(Debug)]
pub struct OtherVersion(String);

impl std::fmt::Display for OtherVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Another version of Weft is running ({}). `weft stop` ends it and its agents.",
            self.0
        )
    }
}

impl std::error::Error for OtherVersion {}

/// What became of a project whose daemon went, looked at again.
pub enum Again {
    /// Nothing listens.
    Waiting,
    /// A daemon listens, and the project is open on it again.
    Back(Box<ProjectClient>),
    /// A daemon of another protocol listens: a newer Weft. What it said.
    Newer(String),
    /// A daemon listens and would not open the project: why.
    Refused(String),
}

impl ProjectClient {
    /// A project not connected to the daemon (yet, or any more): it shows what
    /// it last knew, and every call is refused until it is connected.
    pub fn disconnected(root: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            name: name.into(),
            link: None,
            owed: HashMap::new(),
            jobs: Vec::new(),
            jobs_failed: Vec::new(),
            typing_refused: None,
            remotes: 0,
            told: HashMap::new(),
            panes: Vec::new(),
            unrecorded: None,
            units: Vec::new(),
            readiness: serde_json::json!({}),
            setup: None,
            remote: None,
            records: serde_json::json!({}),
            work: Default::default(),
            opened: false,
            boards: 0,
            routing: Routing::default(),
            gap: None,
            needs: None,
            waiting: Vec::new(),
            skip: Vec::new(),
            skip_refused: None,
            sessions: Vec::new(),
            earlier: 0,
            earlier_rows: Default::default(),
            earlier_found: None,
            zone: 0,
            notify: true,
            turbo: false,
            start_at_login: false,
            config: None,
            machine_name: None,
            update: None,
            reopened_since: None,
            harnesses: Default::default(),
            unread: Vec::new(),
            lost: false,
            read: HashMap::new(),
            refused: HashMap::new(),
        }
    }

    /// The project's name: its folder's.
    pub fn name_of(root: &Path) -> String {
        root.file_name().and_then(|n| n.to_str()).unwrap_or("project").to_string()
    }

    /// Connect to `weftd` at the default socket, starting or joining it with
    /// `weft --daemon` if none is listening. It waits for the daemon: never call it on
    /// the window's thread or under the shared lock.
    pub fn open(root: &Path, rows: u16, cols: u16) -> Result<Self> {
        let socket = socket_path();
        if !is_live(&socket) {
            clear_dead(&socket);
            start_server()?;
        } else if let Some(weft) = own_weft() {
            // A daemon an older Weft started starts again on this one once no
            // agent runs; one as new or newer ignores it.
            let _ = weft_proto::tell_update(&socket, env!("CARGO_PKG_VERSION"), &weft);
        }
        Self::connect(&socket, root, rows, cols)
    }

    /// Weft's own link to the daemon, with no project: what is installed is
    /// the machine's. It starts the daemon when `start` and none listens.
    pub fn machine(socket: &Path, start: bool) -> Result<Self> {
        if !is_live(socket) {
            if !start {
                return Err(anyhow!("no session at {}", socket.display()));
            }
            clear_dead(socket);
            start_server()?;
        }
        let stream = UnixStream::connect(socket)
            .map_err(|e| anyhow!("no session at {}: {e}", socket.display()))?;
        let mut client = Self::disconnected(PathBuf::new(), String::new());
        client.link = Some(Link::over(stream)?);
        let hello = client.send(Call::Hello {
            client: concat!("weft-desktop/", env!("CARGO_PKG_VERSION")).to_string(),
            protocol: PROTOCOL,
        })?;
        match client.wait(hello)? {
            Ok(said) => {
                client.machine_name = said["name"].as_str().map(String::from);
                Ok(client)
            }
            Err(no) => {
                if let Some(link) = client.link.take() {
                    link.cut();
                }
                Err(OtherVersion(no).into())
            }
        }
    }

    /// Open `root` again on whatever daemon listens at `socket` now, starting
    /// none: the one this project was on went, and the person may have
    /// stopped it.
    pub fn again(socket: &Path, root: &Path, rows: u16, cols: u16) -> Again {
        if !is_live(socket) {
            return Again::Waiting;
        }
        match Self::connect(socket, root, rows, cols) {
            Ok(client) => Again::Back(Box::new(client)),
            Err(e) if e.is::<OtherVersion>() => Again::Newer(e.to_string()),
            Err(e) => Again::Refused(e.to_string()),
        }
    }

    /// Connect to a daemon listening at `socket` and open `root`. It waits for
    /// the daemon, as [`ProjectClient::open`].
    pub fn connect(socket: &Path, root: &Path, rows: u16, cols: u16) -> Result<Self> {
        let stream = UnixStream::connect(socket)
            .map_err(|e| anyhow!("no session at {}: {e}", socket.display()))?;
        Self::over(stream, root, rows, cols)
    }

    /// Open `root` over a connection already made.
    pub fn over(stream: UnixStream, root: &Path, rows: u16, cols: u16) -> Result<Self> {
        let mut client = Self::disconnected(root.to_path_buf(), Self::name_of(root));
        client.link = Some(Link::over(stream)?);
        let opened = client.handshake(root, rows, cols);
        if opened.is_err()
            && let Some(link) = client.link.take()
        {
            link.cut();
        }
        opened.map(|()| client)
    }

    fn handshake(&mut self, root: &Path, rows: u16, cols: u16) -> Result<()> {
        let client = self;

        let hello = client.send(Call::Hello {
            client: concat!("weft-desktop/", env!("CARGO_PKG_VERSION")).to_string(),
            protocol: PROTOCOL,
        })?;
        match client.wait(hello)? {
            Ok(said) => client.machine_name = said["name"].as_str().map(String::from),
            Err(no) => return Err(OtherVersion(no).into()),
        }
        let open = client.send(Call::Open {
            rows,
            cols,
            path: root.to_string_lossy().into_owned(),
            raw: true,
        })?;
        let opened = client.wait(open)?.map_err(|e| anyhow!(e))?;
        client.take_opened(&opened);
        let config = client.send(Call::ConfigGet)?;
        client.config = client.wait(config)?.ok();
        Ok(())
    }

    /// The answer to call `id`, taking every line before it as it comes.
    /// Only connecting waits so.
    fn wait(&mut self, id: u64) -> Result<std::result::Result<Value, String>> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .link
                .as_ref()
                .ok_or_else(|| anyhow!("not connected"))?
                .inbox
                .recv_timeout(left)
                .map_err(|_| anyhow!("the session did not answer"))?;
            match line {
                Line::Result { id: got, result } if got == id => return Ok(Ok(result)),
                Line::Error { id: got, message, .. } if got == id => return Ok(Err(message)),
                other => {
                    self.take(other);
                }
            }
        }
    }

    pub(crate) fn take_opened(&mut self, opened: &Value) {
        if let Some(panes) = opened.get("panes").and_then(weft_proto::panes_of) {
            self.panes = panes.into_iter().map(PaneView::of).collect();
            self.opened = true;
        }
        if let Some(units) = opened.get("units").cloned() {
            self.units = serde_json::from_value(units).unwrap_or_default();
        }
        self.readiness = opened.get("readiness").cloned().unwrap_or_else(|| serde_json::json!({}));
        self.records = opened.get("records").cloned().unwrap_or_else(|| serde_json::json!({}));
        self.work = serde_json::from_value(opened.get("work").cloned().unwrap_or_default())
            .unwrap_or_default();
        if let Some(agents) = opened.get("agents") {
            self.take_agents(agents);
        }
        self.harnesses = opened
            .get("harnesses")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        self.unread = opened
            .get("unread")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        self.notify = opened.get("notify").and_then(|v| v.as_bool()).unwrap_or(true);
        self.turbo = opened.get("turbo").and_then(|v| v.as_bool()).unwrap_or(false);
        self.start_at_login =
            opened.get("start_at_login").and_then(|v| v.as_bool()).unwrap_or(false);
        self.update = opened.get("update").and_then(|v| v.as_str()).map(str::to_string);
        self.gap = opened.get("gap").and_then(|v| v.as_str()).map(str::to_string);
        self.needs =
            opened.get("setup").and_then(|v| serde_json::from_value(v.clone()).ok()).flatten();
        self.remote = opened.get("remote").filter(|v| !v.is_null()).cloned();
        if let Some(r) = opened.get("routing") {
            self.routing = Routing::opened(r);
        }
        // The jobs running, and those that ended a moment ago or failed.
        self.jobs.clear();
        let jobs: Vec<Job> = opened
            .get("jobs")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        for job in jobs {
            self.take_job(job);
        }
        if let Some(skip) = opened.get("skip") {
            self.take_skip(serde_json::from_value(skip.clone()).unwrap_or_default());
        }
        for p in opened.get("pendings").and_then(|v| v.as_array()).into_iter().flatten() {
            if let Some(Event::Pending { id, pane, what, why, payload, sends, go }) =
                weft_proto::pending_of(p)
            {
                self.stage(Staged { id, pane, what, why, payload, sends, go });
            }
        }
        for screen in opened.get("screens").and_then(|v| v.as_array()).into_iter().flatten() {
            if let Some(s) = weft_proto::screen_of(screen)
                && let Some(p) = self.view_mut(s.pane)
            {
                p.show(&s);
            }
        }
    }

    /// A dialog staged, in the place of the one with its id, which the
    /// daemon says again once what it shows has moved.
    fn stage(&mut self, staged: Staged) {
        match self.waiting.iter_mut().find(|w| w.id == staged.id) {
            Some(w) => *w = staged,
            None => self.waiting.push(staged),
        }
    }

    /// What the daemon's latest check found an Eval should skip.
    fn take_skip(&mut self, mut findings: Vec<weft_core::skip::Finding>) {
        findings.retain(weft_core::skip::Finding::in_settings);
        self.skip = findings;
    }

    /// Look again at which files an Eval of the project should skip.
    pub fn skip_check(&mut self) -> Result<u64> {
        self.skip_refused = None;
        self.tell(Call::SkipCheck, Then::SkipCheck)
    }

    /// Write the line a finding proposed; once while it is being written.
    pub fn skip_add(&mut self, file: &str, line: &str) -> Result<Option<u64>> {
        let then = Then::SkipAdd { file: file.to_string(), line: line.to_string() };
        if self.owes(&then) {
            return Ok(None);
        }
        self.skip_refused = None;
        let call = Call::SkipAdd { file: file.to_string(), line: line.to_string() };
        self.tell(call, then).map(Some)
    }

    pub fn is_connected(&self) -> bool {
        self.link.is_some()
    }

    /// Link this project over `stream` as it is, without opening it: for a
    /// test that plays the daemon on the other end.
    #[cfg(test)]
    pub fn wire(&mut self, stream: UnixStream) {
        self.link = Some(Link::over(stream).expect("a link"));
    }

    fn take_agents(&mut self, agents: &Value) {
        let field = |k: &str| agents.get(k).cloned().unwrap_or_default();
        self.set_turns(serde_json::from_value(field("panes")).unwrap_or_default());
        self.sessions = serde_json::from_value(field("sessions")).unwrap_or_default();
        self.zone = field("zone").as_i64().unwrap_or(0);
        self.earlier_moved(field("earlier").as_u64().unwrap_or(0) as usize);
    }

    /// How many earlier sessions there are now: the pages read of them, and
    /// of a search through them, are read again when that moved.
    fn earlier_moved(&mut self, earlier: usize) {
        let was = std::mem::replace(&mut self.earlier, earlier);
        if earlier == 0 {
            self.earlier_rows = Default::default();
            self.earlier_found = None;
            return;
        }
        let unread = self.earlier_rows.rows.is_empty() && !self.earlier_rows.asking();
        if was != earlier || unread {
            let after = self.earlier_rows.restart();
            self.read_sessions(String::new(), after);
            if let Some(found) = &mut self.earlier_found {
                let (q, after) = (found.q.clone(), found.restart());
                self.read_sessions(q, after);
            }
        }
    }

    /// A session by its id: one carried as it is, or an earlier one read.
    pub fn session(&self, id: &str) -> Option<&weft_core::sessions::Row> {
        let found = self.earlier_found.iter().flat_map(|f| &f.rows);
        self.sessions.iter().chain(&self.earlier_rows.rows).chain(found).find(|r| r.id == id)
    }

    /// Search the earlier sessions for `q` through the daemon, among them
    /// all; nothing to search for drops what was found.
    pub fn search_earlier(&mut self, q: &str) {
        let q = q.trim();
        if q.is_empty() || self.earlier == 0 {
            self.earlier_found = None;
            return;
        }
        if self.earlier_found.as_ref().is_some_and(|f| f.q == q) {
            return;
        }
        let mut found = crate::earlier::Pages::new(q);
        let after = found.restart();
        self.earlier_found = Some(found);
        self.read_sessions(q.to_string(), after);
    }

    /// A page more of the earlier sessions, or of what the search found.
    pub fn more_earlier(&mut self, found: bool) {
        let pages = if found { self.earlier_found.as_mut() } else { Some(&mut self.earlier_rows) };
        if let Some(pages) = pages {
            let (q, after) = (pages.q.clone(), pages.more());
            self.read_sessions(q, after);
        }
    }

    /// The read of earlier sessions that failed, or of what the search
    /// found, sent again.
    pub fn earlier_again(&mut self, found: bool) {
        let pages = if found { self.earlier_found.as_mut() } else { Some(&mut self.earlier_rows) };
        if let Some(pages) = pages {
            let (q, after) = (pages.q.clone(), pages.again());
            self.read_sessions(q, after);
        }
    }

    /// The pages of earlier sessions a search for `q` reads, or every
    /// earlier session's when `q` is empty.
    fn earlier_pages(&mut self, q: &str) -> Option<&mut crate::earlier::Pages> {
        if q.is_empty() {
            Some(&mut self.earlier_rows)
        } else {
            self.earlier_found.as_mut().filter(|f| f.q == q)
        }
    }

    /// Read the page of earlier sessions after `after`, when one is to be
    /// read, for the pages a search for `q` reads.
    fn read_sessions(&mut self, q: String, after: Option<crate::earlier::After>) {
        let Some(after) = after else { return };
        let call = Call::Read {
            what: "sessions".into(),
            unit: String::new(),
            pane: None,
            lines: None,
            before: None,
            after,
            q: (!q.is_empty()).then(|| q.clone()),
        };
        let id = self.tell(call, Then::Sessions { q: q.clone() }).ok();
        if let Some(pages) = self.earlier_pages(&q) {
            pages.sent(id);
        }
    }

    pub fn set_turns(&mut self, turns: Vec<Option<Turn>>) {
        for (i, p) in self.panes.iter_mut().enumerate() {
            p.turn = turns.get(i).copied().flatten();
        }
    }

    pub fn view(&self, id: u32) -> Option<&PaneView> {
        self.panes.iter().find(|p| p.id == id)
    }

    pub fn view_mut(&mut self, id: u32) -> Option<&mut PaneView> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    /// The judge panes of an Eval, listed under it.
    pub fn judges(&self, eval: &str) -> Vec<&PaneView> {
        self.panes.iter().filter(|p| p.judge.as_deref() == Some(eval)).collect()
    }

    pub fn pane_infos(&self) -> Vec<PaneInfo> {
        self.panes.iter().map(PaneView::info).collect()
    }

    pub fn pane_turns(&self) -> Vec<Option<Turn>> {
        self.panes.iter().map(|p| p.turn).collect()
    }

    pub fn readiness_of(&self, harness: &str) -> Readiness {
        match self.readiness.get(harness).and_then(Value::as_str) {
            Some(w) => Readiness::of_word(Some(w)),
            None => Readiness::Ready,
        }
    }

    /// Write `call` to the daemon; its answer is taken by [`ProjectClient::pump`].
    fn send(&mut self, call: Call) -> Result<u64> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let link = self.link.as_ref().ok_or_else(|| anyhow!("{}", not_connected()))?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        link.out
            .send(Line::Call { id, call }.encode())
            .map_err(|_| anyhow!("{}", not_connected()))?;
        Ok(id)
    }

    /// Write `call`, keeping what its answer is for.
    pub fn tell(&mut self, call: Call, then: Then) -> Result<u64> {
        let id = self.send(call)?;
        self.owed.insert(id, (then, Instant::now()));
        Ok(id)
    }

    /// The answer to call `id`, still to come, is for `then`: a yes that
    /// waits says what it does.
    pub fn owed_for(&mut self, id: u64, then: Then) {
        if let Some((t, _)) = self.owed.get_mut(&id) {
            *t = then;
        }
    }

    /// Whether an answer for `then` is still to come.
    pub fn owes(&self, then: &Then) -> bool {
        self.owed.values().any(|(t, _)| t == then)
    }

    /// Whether any answer still to come is for something `kind` picks.
    pub fn owes_any(&self, kind: impl Fn(&Then) -> bool) -> bool {
        self.owed.values().any(|(t, _)| kind(t))
    }

    /// Every answer still to come: what it is for, and since when.
    pub fn owed(&self) -> impl Iterator<Item = (&Then, Instant)> {
        self.owed.values().map(|(t, at)| (t, *at))
    }

    /// Every answer still to come, by its call: what it is for, and since
    /// when.
    pub fn owed_calls(&self) -> impl Iterator<Item = (u64, &Then, Instant)> {
        self.owed.iter().map(|(id, (t, at))| (*id, t, *at))
    }

    /// A job as the daemon says it now: kept by its id, in the place of one
    /// that failed on the same thing. One that ended well goes, its result
    /// shown where it was; one that failed stays, and is said in the bar
    /// when no wait of this window shows it.
    fn take_job(&mut self, mut job: Job) {
        let now = Instant::now();
        let at = match self.jobs.iter().position(|p| p.job.id == job.id) {
            Some(i) => {
                let p = &mut self.jobs[i];
                if job.call.is_none() {
                    job.call = p.job.call;
                }
                if p.failed().is_some() {
                    return;
                }
                p.job = job;
                p.got = now;
                i
            }
            None => {
                self.jobs.retain(|p| {
                    p.failed().is_none() || p.job.kind != job.kind || p.job.on != job.on
                });
                self.jobs.push(crate::waiting::Progress::of(job, now));
                self.jobs.len() - 1
            }
        };
        let p = &self.jobs[at];
        if p.job.end.as_ref().is_some_and(|e| e.ok) {
            self.jobs.remove(at);
            return;
        }
        // A refusal of this window's own call says it, where it was asked.
        let asked_here = p.job.call.is_some_and(|id| self.owed.contains_key(&id));
        if !asked_here
            && SAID_IN_THE_BAR.contains(&p.job.kind.as_str())
            && let Some(said) = p.failure()
        {
            self.jobs_failed.push(said);
        }
    }

    /// The job running that the call `id` began.
    pub fn job_of_call(&self, id: u64) -> Option<&crate::waiting::Progress> {
        self.jobs.iter().find(|p| p.running() && p.job.call == Some(id))
    }

    /// The first job running that `pick` picks.
    pub fn job(&self, pick: impl Fn(&Job) -> bool) -> Option<&crate::waiting::Progress> {
        self.jobs.iter().find(|p| p.running() && pick(&p.job))
    }

    /// The job that failed that `pick` picks, still shown.
    pub fn job_failed(&self, pick: impl Fn(&Job) -> bool) -> Option<&crate::waiting::Progress> {
        self.jobs.iter().find(|p| p.failed().is_some() && pick(&p.job))
    }

    /// The person acted again on what failed: its failure goes.
    pub fn forget_failed(&mut self, pick: impl Fn(&Job) -> bool) {
        self.jobs.retain(|p| p.failed().is_none() || !pick(&p.job));
    }

    /// Whether a job runs here.
    pub fn jobs_run(&self) -> bool {
        self.jobs.iter().any(crate::waiting::Progress::running)
    }

    /// What the work `root`'s row says while something is done to it: its
    /// Eval opening, or its Seal, here or from anywhere.
    pub fn row_doing(&self, root: &str) -> Option<&'static str> {
        let on = self.job(|j| j.on.work.as_deref() == Some(root)).and_then(|p| {
            match p.job.kind.as_str() {
                "eval_open" => Doing::Opening.row(),
                "seal" => Doing::Sealing.row(),
                _ => None,
            }
        });
        if on.is_some() {
            return on;
        }
        self.owed.values().find_map(|(t, _)| match t {
            Then::Working(w) if w.work.as_deref() == Some(root) => w.doing.row(),
            _ => None,
        })
    }

    /// Take every line the daemon sent since: events change this project, a
    /// read is kept in [`ProjectClient::read`], and every other answer is
    /// handed back with what it was for. What changed, the panes whose
    /// output came, and the answers. A link that broke is dropped, so the
    /// project is connected again.
    pub fn pump(&mut self) -> Pumped {
        let mut p = Pumped::default();
        loop {
            let next = match &self.link {
                Some(link) => link.inbox.try_recv(),
                None => return p,
            };
            match next {
                // Output redraws its pane's terminal; the window only when the
                // pane came back to its end or left it, which its button shows.
                Ok(Line::Event(Event::Output { pane, seq, bytes })) => {
                    let back = |c: &Self| c.view(pane).map(|v| v.scroll_offset() > 0);
                    let before = back(self);
                    self.take(Line::Event(Event::Output { pane, seq, bytes }));
                    p.changed |= back(self) != before;
                    if !p.output.contains(&pane) {
                        p.output.push(pane);
                    }
                }
                Ok(line) => {
                    p.changed = true;
                    p.moved |= matches!(&line, Line::Event(_));
                    if let Some(a) = self.take(line) {
                        p.answered.push(a);
                    }
                }
                Err(TryRecvError::Empty) => return p,
                Err(TryRecvError::Disconnected) => {
                    self.lose();
                    p.changed = true;
                    return p;
                }
            }
        }
    }

    /// The daemon went: nothing it owed will come.
    fn lose(&mut self) {
        self.lost = true;
        self.link = None;
        self.owed.clear();
        self.jobs.clear();
        self.told.clear();
        self.earlier_rows.sent(None);
        if let Some(found) = &mut self.earlier_found {
            found.sent(None);
        }
    }

    /// One line from the daemon; an answer, with what it was for.
    pub fn take(&mut self, line: Line) -> Option<Answered> {
        let event = match line {
            Line::Event(e) => e,
            Line::Result { id, result } => {
                let (then, _) = self.owed.remove(&id)?;
                if let Then::Sessions { q } = then {
                    let after = self.earlier_pages(&q).and_then(|p| p.answered(id, &result));
                    self.read_sessions(q, after);
                    return None;
                }
                if matches!(then, Then::SkipCheck | Then::SkipAdd { .. }) {
                    self.take_skip(
                        serde_json::from_value(result["findings"].clone()).unwrap_or_default(),
                    );
                    return None;
                }
                self.refused.remove(&then);
                if then.is_kept() {
                    self.read.insert(then, result);
                    return None;
                }
                return Some(Answered { then, answer: Ok(result), code: String::new() });
            }
            Line::Error { id, message, code } => {
                let (then, _) = self.owed.remove(&id)?;
                if matches!(then, Then::SkipCheck | Then::SkipAdd { .. }) {
                    self.skip_refused = Some(message);
                    return None;
                }
                if let Then::Sessions { q } = &then {
                    if let Some(pages) = self.earlier_pages(q) {
                        pages.refused(id, &message);
                    }
                    return None;
                }
                // A read that failed says why where it would be shown, until
                // it is read again.
                if then.is_kept() || matches!(then, Then::Conversation { .. } | Then::Older { .. })
                {
                    self.refused.insert(then.clone(), message.clone());
                }
                if then.is_kept() {
                    return None;
                }
                return Some(Answered { then, answer: Err(message), code });
            }
            Line::Call { .. } => return None,
        };
        match event {
            Event::Panes { panes } => {
                let mut kept = std::mem::take(&mut self.panes);
                self.panes = panes
                    .into_iter()
                    .map(|info| match kept.iter().position(|v| v.id == info.pane) {
                        Some(i) => {
                            let mut v = kept.swap_remove(i);
                            v.running = info.running;
                            v.turbo = info.turbo;
                            v.judge = info.judge;
                            v
                        }
                        None => PaneView::of(info),
                    })
                    .collect();
            }
            Event::Output { pane, seq, bytes } => {
                if let Some(p) = self.view_mut(pane) {
                    p.output(seq, &bytes);
                }
            }
            Event::Turbo { on } => self.turbo = on,
            Event::Config { config } => {
                self.config = Some(config);
                // What RingFrame said of the rules may no longer hold.
                self.read.retain(|then, _| {
                    !matches!(
                        then,
                        Then::Rules { .. } | Then::Preview { .. } | Then::Profile { .. }
                    )
                });
                self.refused.retain(|then, _| !then.is_settings());
            }
            Event::StartAtLogin { on } => self.start_at_login = on,
            Event::Reopened { since } => self.reopened_since = since,
            Event::Update { version } => self.update = version,
            Event::Screen { .. }
            | Event::Presence { .. }
            | Event::RemoteAct { .. }
            | Event::Signal(_) => {}
            Event::Remote { view } => {
                self.remote = Some(view);
                self.remotes += 1;
            }
            Event::Added { pane, harness, spec, turbo } => {
                if self.view(pane).is_none() {
                    self.panes.push(PaneView::new(pane, harness, spec));
                }
                if let Some(p) = self.view_mut(pane) {
                    p.turbo = turbo;
                }
            }
            Event::Exited { pane } => {
                if let Some(p) = self.view_mut(pane) {
                    p.running = false;
                }
            }
            Event::Units { units, records, work } => {
                self.units = units;
                self.records = records;
                self.work = work;
                self.boards += 1;
            }
            Event::Pending { id, pane, what, why, payload, sends, go } => {
                self.stage(Staged { id, pane, what, why, payload, sends, go });
            }
            Event::Skip { findings } => self.take_skip(findings),
            Event::Progress { job } => self.take_job(job),
            Event::Resolved { id, .. } => self.waiting.retain(|w| w.id != id),
            Event::Injected { refusal, unrecorded, .. } => {
                if refusal.is_some() {
                    self.typing_refused = refusal;
                }
                self.unrecorded = unrecorded;
            }
            Event::Readiness { states } => self.readiness = states,
            Event::Setup { view } => self.setup = serde_json::from_value(view).ok(),
            Event::Workspace { setup, gap } => (self.needs, self.gap) = (setup, gap),
            Event::Agents { panes, changed, gone, earlier, zone } => {
                self.set_turns(panes);
                weft_core::sessions::apply(&mut self.sessions, &changed, &gone);
                self.zone = zone;
                self.earlier_moved(earlier);
            }
            // What the daemon no longer sends, or does not send to a window.
            #[allow(unreachable_patterns)]
            _ => {}
        }
        None
    }

    /// Start a new agent (or resume `session`); its pane comes in the answer.
    pub fn spawn(&mut self, harness: &str, spec: &str, session: Option<&str>) -> Result<u64> {
        let call = Call::StartAgent {
            harness: harness.to_string(),
            spec: spec.to_string(),
            session: session.map(str::to_string),
        };
        let then =
            Then::Started { harness: harness.to_string(), session: session.map(str::to_string) };
        self.tell(call, then)
    }

    pub fn input(&mut self, pane: u32, bytes: &[u8]) -> Result<()> {
        if let Some(p) = self.view_mut(pane) {
            p.scroll_to_bottom();
        }
        self.send(Call::Input { pane, bytes: bytes.to_vec() }).map(|_| ())
    }

    /// A message the person wrote: the daemon pastes it and presses Enter
    /// once the pane has taken it.
    pub fn say(&mut self, pane: u32, text: &str) -> Result<()> {
        if let Some(p) = self.view_mut(pane) {
            p.scroll_to_bottom();
        }
        self.tell(Call::Say { pane, text: text.to_string() }, Then::Refusal).map(|_| ())
    }

    pub fn scroll(&mut self, pane: u32, delta: i32) {
        if let Some(p) = self.view_mut(pane) {
            p.scroll(delta);
        }
    }

    /// Give `pane` the size its place on screen holds, telling the daemon
    /// once per size.
    pub fn resize(&mut self, pane: u32, rows: u16, cols: u16) -> Result<()> {
        if let Some(p) = self.view_mut(pane) {
            p.resize(rows, cols);
        }
        if self.told.get(&pane) == Some(&(rows, cols)) {
            return Ok(());
        }
        self.send(Call::Resize { pane, rows, cols })?;
        self.told.insert(pane, (rows, cols));
        Ok(())
    }

    pub fn close_pane(&mut self, pane: u32) -> Result<()> {
        self.tell(Call::CloseAgent { pane }, Then::Refusal).map(|_| ())
    }

    /// One key of `config.toml` set in `scope` (`machine` or `project`), or
    /// taken out; every client is told the settings that follow.
    pub fn set_config(
        &mut self,
        scope: &str,
        key: &[impl AsRef<str>],
        value: Option<Value>,
    ) -> Result<u64> {
        let key = key.iter().map(|k| k.as_ref().to_string()).collect();
        self.tell(Call::ConfigSet { scope: scope.into(), key, value }, Then::Refusal)
    }

    /// The machine's turbo, as `config.toml` says: off until it is turned on.
    pub fn machine_turbo(&self) -> bool {
        self.config.as_ref().and_then(|c| c["machine"]["turbo"].as_bool()).unwrap_or(false)
    }

    pub fn set_start_at_login(&mut self, on: bool, program: &str, path: &str) -> Result<u64> {
        let call = Call::StartAtLogin { on, program: program.to_string(), path: path.to_string() };
        self.tell(call, Then::Refusal)
    }

    pub fn act(
        &mut self,
        act: &str,
        unit: Option<&str>,
        pane: Option<u32>,
        text: Option<&str>,
        eval: Option<&str>,
        start: Option<&str>,
    ) -> Result<u64> {
        self.unrecorded = None;
        let call = Call::Act {
            act: act.to_string(),
            unit: unit.map(str::to_string),
            pane,
            text: text.map(str::to_string),
            eval: eval.map(str::to_string),
            start: start.map(str::to_string),
            draft: None,
        };
        self.tell(call, Then::Refusal)
    }

    /// Eval the work `unit` names, read by `start` when the person picked a
    /// harness in its dialog; the answer names the dialog staged.
    pub fn eval(&mut self, unit: &str, pane: Option<u32>, start: Option<&str>) -> Result<u64> {
        self.unrecorded = None;
        let call = Call::Act {
            act: "eval".into(),
            unit: Some(unit.to_string()),
            pane,
            text: None,
            eval: None,
            start: start.map(str::to_string),
            draft: None,
        };
        self.tell(call, Then::Eval)
    }

    /// Ask, or follow up, with what the person kept of the sheet's draft.
    pub fn ask(
        &mut self,
        act: &str,
        unit: Option<&str>,
        (pane, start): (Option<u32>, Option<&str>),
        text: &str,
        draft: Option<weft_core::ask::Draft>,
    ) -> Result<u64> {
        self.unrecorded = None;
        let call = Call::Act {
            act: act.to_string(),
            unit: unit.map(str::to_string),
            pane,
            text: Some(text.to_string()),
            eval: None,
            start: start.map(str::to_string),
            draft,
        };
        self.tell(call, Then::Refusal)
    }

    /// Seal the work an Eval judged, as decided in the Seal's dialog, with
    /// the person's answers to the Eval's questions, when it asked any.
    pub fn seal(
        &mut self,
        eval: &str,
        disposition: &str,
        reason: Option<&str>,
        answers: Option<Vec<weft_core::ask::SealAnswer>>,
    ) -> Result<u64> {
        let call = Call::Act {
            act: "seal".into(),
            unit: None,
            pane: None,
            text: Some(disposition.to_string()),
            eval: Some(eval.to_string()),
            start: None,
            draft: Some(weft_core::ask::Draft {
                reason: reason.map(str::to_string),
                answers,
                ..Default::default()
            }),
        };
        self.tell(call, Then::Refusal)
    }

    /// Read `what` of `unit` (or of `pane`), once: asked again only after the
    /// last one is answered.
    pub fn read(&mut self, what: &str, unit: &str, pane: Option<u32>, then: Then) -> Result<()> {
        if self.owes(&then) {
            return Ok(());
        }
        // What was read before may no longer hold: nothing is shown until it is read again.
        self.read.remove(&then);
        self.refused.remove(&then);
        let call = Call::Read {
            what: what.to_string(),
            unit: unit.to_string(),
            pane,
            lines: None,
            before: None,
            after: None,
            q: None,
        };
        self.tell(call, then).map(|_| ())
    }

    /// Read a pane's conversation: its newest page, the page before
    /// `before`, or what was added after `after`; once, until answered.
    pub fn conversation(
        &mut self,
        pane: u32,
        before: Option<String>,
        after: Option<String>,
    ) -> Result<()> {
        let then =
            if before.is_some() { Then::Older { pane } } else { Then::Conversation { pane } };
        if self.owes(&then) {
            return Ok(());
        }
        let call = Call::Read {
            what: "conversation".to_string(),
            unit: String::new(),
            pane: Some(pane),
            lines: None,
            before,
            after,
            q: None,
        };
        self.tell(call, then).map(|_| ())
    }

    /// Read `what` of `unit` again, keeping what was read before until the
    /// answer comes: for what moves while it is shown.
    pub fn reread(&mut self, what: &str, unit: &str, then: Then) -> Result<()> {
        if self.owes(&then) {
            return Ok(());
        }
        let call = Call::Read {
            what: what.to_string(),
            unit: unit.to_string(),
            pane: None,
            lines: None,
            before: None,
            after: None,
            q: None,
        };
        self.tell(call, then).map(|_| ())
    }

    pub fn resolve(
        &mut self,
        pending: &str,
        yes: bool,
        to: Option<u32>,
        start: Option<&str>,
    ) -> Result<u64> {
        let call = Call::Resolve {
            pending: pending.to_string(),
            yes,
            to,
            start: start.map(str::to_string),
        };
        self.tell(call, Then::Refusal)
    }

    pub fn setup(&mut self, act: &str, harness: &str, path: &str) -> Result<()> {
        let call = Call::Setup {
            act: act.to_string(),
            harness: harness.to_string(),
            path: path.to_string(),
        };
        let then = Then::Setup { act: act.to_string(), target: harness.to_string() };
        self.tell(call, then).map(|_| ())
    }

    pub fn remote_act(&mut self, act: Value) -> Result<()> {
        self.tell(Call::Remote { act }, Then::Refusal).map(|_| ())
    }

    pub fn dismiss(&mut self) -> Result<()> {
        self.tell(Call::Dismiss, Then::Refusal).map(|_| ())
    }

    pub fn stop_agents(&mut self) {
        let ids: Vec<u32> = self.panes.iter().map(|p| p.id).collect();
        for pane in ids.into_iter().rev() {
            let _ = self.close_pane(pane);
        }
    }

    /// Leave the daemon, without stopping anything, once every call queued
    /// has been written.
    pub fn detach(&mut self) {
        let _ = self.send(Call::Detach);
        if let Some(link) = self.link.take() {
            link.close();
        }
        self.owed.clear();
    }
}

/// Why a call could not be made.
fn not_connected() -> &'static str {
    weft_core::words::word("desktop.not_connected")
}

/// Start or join the daemon when none is listening: `weft --daemon`, which
/// goes through Weft's service when "Start at login" installed it.
fn start_server() -> Result<()> {
    let mut cmd = detached(&find_weft_exe()?, &["--daemon"]);
    let mut child = cmd.stdout(std::process::Stdio::piped()).spawn()?;
    let out = child.stdout.take().ok_or_else(|| anyhow!("the session server said nothing"))?;
    let (said, heard) = channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(out), &mut line);
        let _ = said.send(line);
    });
    reaped(child);
    match heard.recv_timeout(Duration::from_secs(5)) {
        Ok(line) if line.contains("listening") => Ok(()),
        _ => Err(anyhow!("the session server did not come up")),
    }
}

/// Wait for a `weft` this app started, off every thread that draws, so it
/// does not linger as a zombie once it ends.
fn reaped(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// `weft` run with `args` in a session of its own, so it outlives this app,
/// saying nothing.
fn detached(weft: &Path, args: &[&str]) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(weft);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc_setsid();
            Ok(())
        });
    }
    cmd
}

/// The installed `weft` under `home` when it is another version than this
/// app: `~/.local/bin/weft` links into `~/.fab7/weft/versions/<version>`.
pub fn newer_weft(home: &Path) -> Option<PathBuf> {
    let link = home.join(".local/bin/weft");
    let real = link.canonicalize().ok()?;
    let versions = home.join(".fab7/weft/versions").canonicalize().ok()?;
    let version = real.strip_prefix(&versions).ok()?.components().next()?;
    (version.as_os_str() != env!("CARGO_PKG_VERSION")).then_some(link)
}

/// Open the app `weft` came with, in place of this one.
pub fn open_app_of(weft: &Path) -> Result<()> {
    reaped(detached(weft, &[]).spawn()?);
    Ok(())
}

/// The `weft` beside this app, by its real path: the one of its version.
fn own_weft() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let weft = exe.with_file_name("weft");
    weft.is_file().then_some(weft)
}

/// The `weft` this app came with: beside it, else where Weft installs it.
pub fn find_weft_exe() -> Result<PathBuf> {
    if let Ok(cur) = std::env::current_exe() {
        let sibling = cur.with_file_name("weft");
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for rel in [".local/bin/weft", ".fab7/bin/weft"] {
            let candidate = home.join(rel);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Ok(PathBuf::from("weft"))
}

fn libc_setsid() {
    unsafe extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        setsid();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, kind: &str, work: &str, age_ms: u64) -> Job {
        serde_json::from_value(serde_json::json!({
            "id": id, "kind": kind, "on": {"work": work}, "age_ms": age_ms,
            "steps": [{"key": "step.reading_change", "state": "now", "ms": 1000,
                       "done": 38, "total": 103, "unit": "files"}],
        }))
        .unwrap()
    }

    /// Opening a project, the daemon names the jobs that run: they show at
    /// once, timed from their age, and each keeps its time as it moves.
    #[test]
    fn the_jobs_come_with_the_project_and_keep_their_time() {
        let mut c = ProjectClient::disconnected("/work/weft", "weft");
        c.take_opened(&serde_json::json!({"jobs": [job("job_1", "eval_open", "ask_1", 18_000)]}));
        let first = c.jobs[0].since;
        assert!(first.elapsed() >= Duration::from_secs(18), "begun 18 s ago");
        assert_eq!(c.row_doing("ask_1"), Some(weft_core::words::word("wait.row_opening_eval")));
        c.take(Line::Event(Event::Progress { job: job("job_1", "eval_open", "ask_1", 19_000) }));
        c.take(Line::Event(Event::Progress { job: job("job_2", "seal", "ask_3", 500) }));
        assert_eq!(c.jobs.len(), 2);
        assert_eq!(c.jobs[0].since, first, "the same job keeps its time");
        assert_eq!(c.row_doing("ask_3"), Some(weft_core::words::word("wait.row_sealing")));
        // Ended well: it goes, and its row says nothing more.
        let mut done = job("job_1", "eval_open", "ask_1", 20_000);
        done.end = Some(weft_proto::JobEnd { ok: true, why: None });
        c.take(Line::Event(Event::Progress { job: done }));
        assert!(c.row_doing("ask_1").is_none());
        assert!(c.jobs_failed.is_empty());
    }

    /// A job that failed stays, said once in the bar when no wait here
    /// asked for it, until one on the same thing begins or the person acts.
    #[test]
    fn a_failed_job_stays_and_is_said_once() {
        let mut c = ProjectClient::disconnected("/work/weft", "weft");
        let mut failed = job("job_1", "eval_open", "ask_1", 3_000);
        failed.end = Some(weft_proto::JobEnd { ok: false, why: Some("git exited 128".into()) });
        c.take_opened(&serde_json::json!({"jobs": [failed.clone()]}));
        let said = weft_core::words::fill("wait.could_not_open_eval", &[("why", "git exited 128")]);
        assert_eq!(c.jobs_failed, vec![said]);
        c.take(Line::Event(Event::Progress { job: failed }));
        assert_eq!(c.jobs_failed.len(), 1, "said once");
        assert!(c.job_failed(|j| j.kind == "eval_open").is_some());
        assert!(c.row_doing("ask_1").is_none(), "it no longer runs");
        // Opened again: the new job takes the failed one's place.
        c.take(Line::Event(Event::Progress { job: job("job_2", "eval_open", "ask_1", 400) }));
        assert!(c.job_failed(|j| j.kind == "eval_open").is_none());
        assert_eq!(c.jobs.len(), 1);
        // Acted on: forgotten.
        let mut again = job("job_2", "eval_open", "ask_1", 900);
        again.end = Some(weft_proto::JobEnd { ok: false, why: Some("no".into()) });
        c.take(Line::Event(Event::Progress { job: again }));
        c.forget_failed(|j| j.on.work.as_deref() == Some("ask_1"));
        assert!(c.jobs.is_empty());
    }

    /// An Eval dialog's payload says what it will read; it types nothing.
    #[test]
    fn an_eval_dialogs_payload_says_what_it_reads_and_types_nothing() {
        let staged = |payload: &[u8]| Staged {
            id: "p1".into(),
            pane: 0,
            what: String::new(),
            why: String::new(),
            payload: payload.to_vec(),
            sends: None,
            go: None,
        };
        let eval = staged(br#"{"reads": {"commits": 3, "files": 14}}"#);
        assert_eq!(eval.data().unwrap()["reads"]["files"], 14);
        assert!(eval.typed().is_empty());
        let prompt = staged(b"{ not json, a prompt");
        assert!(prompt.data().is_none());
        assert_eq!(prompt.typed(), b"{ not json, a prompt");
    }

    /// A dialog the daemon says again, under the same id, takes the place of
    /// the one it was: it is never shown twice.
    #[test]
    fn a_dialog_said_again_replaces_the_one_with_its_id() {
        let pending = |id: &str, files: u64| {
            Line::Event(Event::Pending {
                id: id.into(),
                pane: u32::MAX,
                what: "Eval this work?".into(),
                why: String::new(),
                payload: serde_json::to_vec(&serde_json::json!({"reads": {"files": files}}))
                    .unwrap(),
                sends: None,
                go: None,
            })
        };
        let mut c = ProjectClient::disconnected("/work/weft", "weft");
        c.take(pending("p1", 14_181));
        c.take(pending("p2", 3));
        c.take(pending("p1", 13_852));
        let ids: Vec<&str> = c.waiting.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["p1", "p2"], "once each, in its place");
        assert_eq!(c.waiting[0].data().unwrap()["reads"]["files"], 13_852, "as said again");
        // Said again as a project opens: the same.
        let again = weft_proto::pending_json("p2", u32::MAX, "Eval", "", b"x", None, None);
        c.take_opened(&serde_json::json!({ "pendings": [again] }));
        assert_eq!(c.waiting.len(), 2);
        assert_eq!(c.waiting[1].payload, b"x");
    }

    /// What an Eval should skip comes as the project opens and as the
    /// daemon says it again; a nested repository is never kept here. A line
    /// refused says why until another is asked for.
    #[test]
    fn the_findings_come_as_the_project_opens_and_as_they_move() {
        let finding = |kind: &str, path: &str| {
            serde_json::json!({"kind": kind, "path": path, "files": 3,
                               "fix": {"file": ".gitignore", "line": format!("/{path}")}})
        };
        let mut c = ProjectClient::disconnected("/work/weft", "weft");
        c.take_opened(&serde_json::json!({
            "skip": [finding("untracked_bulk", "output/"), finding("sub_repo", "site/")],
        }));
        let paths: Vec<&str> = c.skip.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["output/"]);
        c.take(Line::Event(Event::Skip { findings: Vec::new() }));
        assert!(c.skip.is_empty());
        c.owed.insert(
            7,
            (Then::SkipAdd { file: ".gitignore".into(), line: "/a/".into() }, Instant::now()),
        );
        assert!(c.skip_add(".gitignore", "/a/").unwrap().is_none(), "once while it is written");
        c.take(Line::Error { id: 7, code: "refused".into(), message: "read-only".into() });
        assert_eq!(c.skip_refused.as_deref(), Some("read-only"));
    }

    /// A `weft` this app starts is waited for once it ends, so none lingers
    /// as a zombie: one that found a daemon already listening ends at once.
    #[test]
    fn a_weft_this_app_started_is_reaped_when_it_ends() {
        let child = std::process::Command::new("/bin/sh").args(["-c", "exit 0"]).spawn().unwrap();
        let pid = child.id().to_string();
        reaped(child);
        let gone = || {
            !std::process::Command::new("/bin/kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while !gone() {
            assert!(std::time::Instant::now() < until, "{pid} is still there, unreaped");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_newer_weft_is_the_installed_one_of_another_version() {
        let home = std::env::temp_dir().join(format!("weft-newer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        assert_eq!(newer_weft(&home), None, "nothing installed");
        let link = bin.join("weft");
        for (version, newer) in [(env!("CARGO_PKG_VERSION"), false), ("99.0.0", true)] {
            let folder = home.join(".fab7/weft/versions").join(version);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("weft"), "").unwrap();
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(folder.join("weft"), &link).unwrap();
            assert_eq!(newer_weft(&home), newer.then(|| link.clone()), "{version}");
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
