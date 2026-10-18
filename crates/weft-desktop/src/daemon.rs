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
use weft_proto::{Call, Event, Line, Lines, PROTOCOL, clear_dead, is_live, socket_path};

/// One pane, as `weft-desktop` sees it.
pub struct PaneView {
    pub id: u32,
    pub harness: String,
    pub spec: String,
    pub running: bool,
    pub turbo: bool,
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
        v
    }

    pub fn new(id: u32, harness: String, spec: String) -> Self {
        let mut v = Self {
            id,
            harness,
            spec,
            running: true,
            turbo: false,
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
}

/// What a call's answer is for, kept until the answer comes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Then {
    /// An agent started: show its pane; it resumes `session` when it names one.
    Started {
        session: Option<String>,
    },
    /// A pane's conversation, read again.
    Conversation {
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
    /// What the message field of `pane` could show after `draft`.
    Suggest {
        pane: u32,
        draft: String,
    },
    /// Nothing but a refusal, which the person is shown.
    Refusal,
}

/// An answer the daemon gave, with what it was for.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub then: Then,
    pub answer: std::result::Result<Value, String>,
}

/// The connection to the daemon: lines in from its reader thread, and lines
/// out to its writer thread.
struct Link {
    out: Sender<Vec<u8>>,
    inbox: Receiver<Line>,
    writer: Option<std::thread::JoinHandle<()>>,
    stream: UnixStream,
}

/// What a pump took: whether anything came, whether an event moved the
/// project (not only a pane's output), and the answers.
#[derive(Debug, Default)]
pub struct Pumped {
    pub changed: bool,
    pub moved: bool,
    pub answered: Vec<Answered>,
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
    /// What each call still unanswered is for.
    owed: HashMap<u64, Then>,
    told: HashMap<u32, (u16, u16)>,
    pub panes: Vec<PaneView>,
    pub last_refusal: Option<String>,
    pub unrecorded: Option<String>,
    pub units: Vec<Unit>,
    pub readiness: Value,
    pub setup: Option<weft_core::onboarding::View>,
    pub remote: Option<Value>,
    pub records: Value,
    pub work: Work,
    pub boards: u64,
    pub routing: Routing,
    pub gap: Option<String>,
    pub needs: Option<weft_core::workspace::Needs>,
    pub waiting: Vec<Staged>,
    pub sessions: Vec<weft_core::sessions::Row>,
    pub zone: i64,
    pub notify: bool,
    pub turbo: bool,
    pub start_at_login: bool,
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
            told: HashMap::new(),
            panes: Vec::new(),
            last_refusal: None,
            unrecorded: None,
            units: Vec::new(),
            readiness: serde_json::json!({}),
            setup: None,
            remote: None,
            records: serde_json::json!({}),
            work: Default::default(),
            boards: 0,
            routing: Routing::default(),
            gap: None,
            needs: None,
            waiting: Vec::new(),
            sessions: Vec::new(),
            zone: 0,
            notify: true,
            turbo: false,
            start_at_login: false,
            update: None,
            reopened_since: None,
            harnesses: Default::default(),
            unread: Vec::new(),
            lost: false,
            read: HashMap::new(),
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
        }
        Self::connect(&socket, root, rows, cols)
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
        if let Err(no) = client.wait(hello)? {
            return Err(OtherVersion(no).into());
        }
        let open = client.send(Call::Open {
            rows,
            cols,
            path: root.to_string_lossy().into_owned(),
            raw: true,
        })?;
        let opened = client.wait(open)?.map_err(|e| anyhow!(e))?;
        client.take_opened(&opened);
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

    fn take_opened(&mut self, opened: &Value) {
        if let Some(panes) = opened.get("panes").and_then(weft_proto::panes_of) {
            self.panes = panes.into_iter().map(PaneView::of).collect();
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
        for p in opened.get("pendings").and_then(|v| v.as_array()).into_iter().flatten() {
            if let Some(Event::Pending { id, pane, what, why, payload, sends }) =
                weft_proto::pending_of(p)
            {
                self.waiting.push(Staged { id, pane, what, why, payload, sends });
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
        self.owed.insert(id, then);
        Ok(id)
    }

    /// Whether an answer for `then` is still to come.
    pub fn owes(&self, then: &Then) -> bool {
        self.owed.values().any(|t| t == then)
    }

    /// Whether any answer still to come is for something `kind` picks.
    pub fn owes_any(&self, kind: impl Fn(&Then) -> bool) -> bool {
        self.owed.values().any(kind)
    }

    /// Take every line the daemon sent since: events change this project, a
    /// read is kept in [`ProjectClient::read`], and every other answer is
    /// handed back with what it was for. Whether anything
    /// came, and the answers. A link that broke is dropped, so the project
    /// is connected again.
    pub fn pump(&mut self) -> Pumped {
        let mut p = Pumped::default();
        loop {
            let next = match &self.link {
                Some(link) => link.inbox.try_recv(),
                None => return p,
            };
            match next {
                Ok(line) => {
                    p.changed = true;
                    p.moved |=
                        matches!(&line, Line::Event(e) if !matches!(e, Event::Output { .. }));
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
        self.told.clear();
    }

    /// One line from the daemon; an answer, with what it was for.
    pub fn take(&mut self, line: Line) -> Option<Answered> {
        let event = match line {
            Line::Event(e) => e,
            Line::Result { id, result } => {
                let then = self.owed.remove(&id)?;
                if matches!(
                    then,
                    Then::Judges { .. }
                        | Then::EvalView { .. }
                        | Then::Window { .. }
                        | Then::Seal { .. }
                        | Then::Source { .. }
                        | Then::Wording { .. }
                ) {
                    self.read.insert(then, result);
                    return None;
                }
                return Some(Answered { then, answer: Ok(result) });
            }
            Line::Error { id, message, .. } => {
                let then = self.owed.remove(&id)?;
                return Some(Answered { then, answer: Err(message) });
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
            Event::StartAtLogin { on } => self.start_at_login = on,
            Event::Reopened { since } => self.reopened_since = since,
            Event::Update { version } => self.update = version,
            Event::Screen { .. }
            | Event::Presence { .. }
            | Event::RemoteAct { .. }
            | Event::Signal(_) => {}
            Event::Remote { view } => self.remote = Some(view),
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
            Event::Pending { id, pane, what, why, payload, sends } => {
                self.waiting.push(Staged { id, pane, what, why, payload, sends });
            }
            Event::Resolved { id, .. } => self.waiting.retain(|w| w.id != id),
            Event::Injected { refusal, unrecorded, .. } => {
                self.last_refusal = refusal;
                self.unrecorded = unrecorded;
            }
            Event::Readiness { states } => self.readiness = states,
            Event::Setup { view } => self.setup = serde_json::from_value(view).ok(),
            Event::Workspace { setup, gap } => (self.needs, self.gap) = (setup, gap),
            Event::Agents { panes, sessions, zone } => {
                self.set_turns(panes);
                (self.sessions, self.zone) = (sessions, zone);
            }
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
        self.tell(call, Then::Started { session: session.map(str::to_string) })
    }

    pub fn input(&mut self, pane: u32, bytes: &[u8]) -> Result<()> {
        if let Some(p) = self.view_mut(pane) {
            p.scroll_to_bottom();
        }
        self.send(Call::Input { pane, bytes: bytes.to_vec() }).map(|_| ())
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

    pub fn set_turbo(&mut self, on: bool) -> Result<u64> {
        self.tell(Call::Turbo { on }, Then::Refusal)
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
        self.last_refusal = None;
        self.unrecorded = None;
        let call = Call::Act {
            act: act.to_string(),
            unit: unit.map(str::to_string),
            pane,
            text: text.map(str::to_string),
            eval: eval.map(str::to_string),
            start: start.map(str::to_string),
        };
        self.tell(call, Then::Refusal)
    }

    pub fn confirm_ask(&mut self, unit: &str) -> Result<u64> {
        self.tell(Call::ConfirmAsk { unit: unit.to_string() }, Then::Refusal)
    }

    /// Read `what` of `unit` (or of `pane`), once: asked again only after the
    /// last one is answered.
    pub fn read(&mut self, what: &str, unit: &str, pane: Option<u32>, then: Then) -> Result<()> {
        if self.owes(&then) {
            return Ok(());
        }
        // What was read before may no longer hold: nothing is shown until it is read again.
        self.read.remove(&then);
        let call = Call::Read {
            what: what.to_string(),
            unit: unit.to_string(),
            pane,
            lines: None,
            before: None,
            after: None,
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
        };
        self.tell(call, then).map(|_| ())
    }

    pub fn suggest(&mut self, pane: u32, draft: &str) -> Result<u64> {
        let call = Call::Suggest { pane, draft: draft.to_string() };
        self.tell(call, Then::Suggest { pane, draft: draft.to_string() })
    }

    pub fn resolve(
        &mut self,
        pending: &str,
        yes: bool,
        force: bool,
        to: Option<u32>,
        start: Option<&str>,
    ) -> Result<()> {
        let call = Call::Resolve {
            pending: pending.to_string(),
            yes,
            force,
            to,
            start: start.map(str::to_string),
        };
        self.tell(call, Then::Refusal).map(|_| ())
    }

    pub fn setup(&mut self, act: &str, harness: &str, path: &str) -> Result<()> {
        let call = Call::Setup {
            act: act.to_string(),
            harness: harness.to_string(),
            path: path.to_string(),
        };
        self.tell(call, Then::Refusal).map(|_| ())
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
