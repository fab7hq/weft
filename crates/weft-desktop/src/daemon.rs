//! Daemon connection, auto-start (`weft --serve`), and per-project session
//! state (`ProjectClient` + `PaneView`).

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use serde_json::Value;
use weft_core::board::PaneInfo;
use weft_core::harness::Harnesses;
use weft_core::ledger::Unit;
use weft_core::readiness::Readiness;
use weft_core::rounds::Rounds;
use weft_core::routing::Routing;
use weft_core::turns::Turn;
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
}

impl PaneView {
    pub fn of(p: PaneInfo) -> Self {
        let mut v = Self::new(p.pane, p.harness, p.spec);
        v.running = p.running;
        v.turbo = p.turbo;
        v
    }

    pub fn new(id: u32, harness: String, spec: String) -> Self {
        Self {
            id,
            harness,
            spec,
            running: true,
            turbo: false,
            turn: None,
            term: Self::terminal(24, 80),
            seq: 0,
        }
    }

    /// The daemon answers what a harness asks about its terminal; this copy
    /// of it never does.
    fn terminal(rows: u16, cols: u16) -> weft_term::Term {
        weft_term::Term::new(rows, cols, 10_000, |_| {})
    }

    /// Start from the screen the daemon gave when the project was opened.
    pub fn show(&mut self, screen: &weft_proto::OpenScreen) {
        self.term = Self::terminal(screen.rows, screen.cols);
        self.term.feed(&screen.bytes);
        self.seq = screen.seq;
    }

    /// A chunk the pane printed, unless the screen already holds it.
    pub fn output(&mut self, seq: u64, bytes: &[u8]) {
        if seq > self.seq {
            self.seq = seq;
            self.term.feed(bytes);
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.term.feed(bytes);
    }

    pub fn contents(&self) -> String {
        self.term.contents()
    }

    pub fn cells(&self) -> Vec<Vec<weft_term::Cell>> {
        self.term.cells()
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
    }

    pub fn select_to(&mut self, at: (u16, u16)) {
        self.term.select_to(at);
    }

    pub fn clear_selection(&mut self) {
        self.term.clear_selection();
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_text()
    }

    pub fn scroll(&mut self, delta: i32) {
        self.term.scroll(delta);
    }

    pub fn scroll_offset(&self) -> usize {
        self.term.scroll_offset()
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_to_bottom();
    }

    pub fn size(&self) -> (u16, u16) {
        self.term.size()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) != self.term.size() {
            self.term.resize(rows, cols);
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

/// Live or synthetic daemon connection for a single opened project.
pub struct ProjectClient {
    pub root: PathBuf,
    pub name: String,
    socket: Option<PathBuf>,
    stream: Option<UnixStream>,
    inbox: Option<Receiver<Line>>,
    answers: HashMap<u64, std::result::Result<Value, String>>,
    told: HashMap<u32, (u16, u16)>,
    pub panes: Vec<PaneView>,
    pub last_refusal: Option<String>,
    pub unrecorded: Option<String>,
    pub units: Vec<Unit>,
    pub readiness: Value,
    pub setup: Option<weft_core::onboarding::View>,
    pub remote: Option<Value>,
    pub records: Value,
    pub rounds: Rounds,
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
    pub lost: bool,
    pub harnesses: Harnesses,
    pub unread: Vec<String>,
    /// Whether a local `weft-tui` client is concurrently attached and owns PTY
    /// `Call::Resize`. When `false`, `weft-desktop` owns `Call::Resize`.
    pub tui_attached: bool,
    /// Sent calls recorded in disconnected mode for deterministic unit tests.
    pub sent_calls: Vec<Call>,
}

impl ProjectClient {
    /// Create a disconnected project state for deterministic UI unit tests.
    pub fn disconnected(root: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            name: name.into(),
            socket: None,
            stream: None,
            inbox: None,
            answers: HashMap::new(),
            told: HashMap::new(),
            panes: Vec::new(),
            last_refusal: None,
            unrecorded: None,
            units: Vec::new(),
            readiness: serde_json::json!({}),
            setup: None,
            remote: None,
            records: serde_json::json!({}),
            rounds: Default::default(),
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
            lost: false,
            harnesses: Default::default(),
            unread: Vec::new(),
            tui_attached: false,
            sent_calls: Vec::new(),
        }
    }

    /// Connect to `weftd` at the default socket, auto-starting `weft --serve`
    /// if no daemon is listening.
    pub fn open(root: &Path, rows: u16, cols: u16) -> Result<Self> {
        let socket = socket_path();
        if !is_live(&socket) {
            clear_dead(&socket);
            start_server()?;
        }
        Self::connect(&socket, root, rows, cols)
    }

    /// Connect to a daemon listening at `socket` and open `root`.
    pub fn connect(socket: &Path, root: &Path, rows: u16, cols: u16) -> Result<Self> {
        let stream = UnixStream::connect(socket)
            .map_err(|e| anyhow!("no session at {}: {e}", socket.display()))?;
        let reading = stream.try_clone()?;
        let (tx, inbox) = channel();
        std::thread::spawn(move || {
            let mut lines = Lines::new(reading);
            while let Ok(Some(line)) = lines.next() {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });

        let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("project").to_string();
        let mut client = Self::disconnected(root.to_path_buf(), name);
        client.socket = Some(socket.to_path_buf());
        client.stream = Some(stream);
        client.inbox = Some(inbox);

        let hello = client.tell(Call::Hello {
            client: concat!("weft-desktop/", env!("CARGO_PKG_VERSION")).to_string(),
            protocol: PROTOCOL,
        })?;
        if let Err(no) = client.answer(hello)? {
            return Err(anyhow!(
                "Another version of Weft is running ({no}). `weft stop` ends it and its agents."
            ));
        }

        let open = client.tell(Call::Open {
            rows,
            cols,
            path: root.to_string_lossy().into_owned(),
            raw: true,
        })?;
        let opened = client.answered(open)?;
        if let Some(panes) = opened.get("panes").and_then(weft_proto::panes_of) {
            client.panes = panes.into_iter().map(PaneView::of).collect();
        }
        if let Some(units) = opened.get("units").cloned() {
            client.units = serde_json::from_value(units).unwrap_or_default();
        }
        client.readiness =
            opened.get("readiness").cloned().unwrap_or_else(|| serde_json::json!({}));
        client.records = opened.get("records").cloned().unwrap_or_else(|| serde_json::json!({}));
        client.rounds = serde_json::from_value(opened.get("rounds").cloned().unwrap_or_default())
            .unwrap_or_default();
        if let Some(agents) = opened.get("agents") {
            client.take_agents(agents);
        }
        client.harnesses = opened
            .get("harnesses")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        client.unread = opened
            .get("unread")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        client.notify = opened.get("notify").and_then(|v| v.as_bool()).unwrap_or(true);
        client.turbo = opened.get("turbo").and_then(|v| v.as_bool()).unwrap_or(false);
        client.start_at_login =
            opened.get("start_at_login").and_then(|v| v.as_bool()).unwrap_or(false);
        client.update = opened.get("update").and_then(|v| v.as_str()).map(str::to_string);
        client.gap = opened.get("gap").and_then(|v| v.as_str()).map(str::to_string);
        client.needs =
            opened.get("setup").and_then(|v| serde_json::from_value(v.clone()).ok()).flatten();
        client.remote = opened.get("remote").filter(|v| !v.is_null()).cloned();
        if let Some(r) = opened.get("routing") {
            client.routing = Routing::opened(r);
        }
        if let Some(pendings) = opened.get("pendings").and_then(|v| v.as_array()) {
            for p in pendings {
                if let (Some(id), Some(pane), Some(what), Some(why)) = (
                    p.get("id").and_then(|v| v.as_str()),
                    p.get("pane").and_then(|v| v.as_u64()),
                    p.get("what").and_then(|v| v.as_str()),
                    p.get("why").and_then(|v| v.as_str()),
                ) {
                    let payload = p
                        .get("payload")
                        .and_then(|v| v.as_str())
                        .map(|s| s.as_bytes().to_vec())
                        .unwrap_or_default();
                    let sends = p.get("sends").and_then(|v| v.as_str()).map(str::to_string);
                    client.waiting.push(Staged {
                        id: id.to_string(),
                        pane: pane as u32,
                        what: what.to_string(),
                        why: why.to_string(),
                        payload,
                        sends,
                    });
                }
            }
        }
        for screen in opened.get("screens").and_then(|v| v.as_array()).unwrap_or(&Vec::new()) {
            if let Some(s) = weft_proto::screen_of(screen)
                && let Some(p) = client.view_mut(s.pane)
            {
                p.show(&s);
            }
        }
        Ok(client)
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
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

    pub fn tell(&mut self, call: Call) -> Result<u64> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        self.sent_calls.push(call.clone());
        if let Some(stream) = &mut self.stream {
            stream.write_all(&Line::Call { id, call }.encode())?;
            stream.flush()?;
        }
        Ok(id)
    }

    pub fn answered(&mut self, id: u64) -> Result<Value> {
        self.answer(id)?.map_err(|e| anyhow!(e))
    }

    pub fn answer(&mut self, id: u64) -> Result<std::result::Result<Value, String>> {
        if self.inbox.is_none() {
            return Ok(Ok(serde_json::json!({})));
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(answer) = self.answers.remove(&id) {
                return Ok(answer);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .inbox
                .as_ref()
                .ok_or_else(|| anyhow!("no inbox"))?
                .recv_timeout(left)
                .map_err(|_| anyhow!("the session did not answer"))?;
            self.take(line);
        }
    }

    /// The answer to call `id`, once it has come; nothing is waited for.
    pub fn take_answer(&mut self, id: u64) -> Option<std::result::Result<Value, String>> {
        self.answers.remove(&id)
    }

    /// An answer as if the daemon had sent it, for tests.
    #[cfg(test)]
    pub fn give_answer(&mut self, id: u64, answer: Value) {
        self.answers.insert(id, Ok(answer));
    }

    pub fn answers(&mut self) -> Vec<(u64, std::result::Result<Value, String>)> {
        self.answers.drain().collect()
    }

    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        loop {
            let next = self.inbox.as_ref().map(Receiver::try_recv);
            match next {
                Some(Ok(line)) => {
                    changed = true;
                    self.take(line);
                }
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) if !self.lost => {
                    self.lost = true;
                    return true;
                }
                _ => return changed,
            }
        }
    }

    pub fn take(&mut self, line: Line) {
        let event = match line {
            Line::Event(e) => e,
            Line::Result { id, result } => {
                self.answers.insert(id, Ok(result));
                return;
            }
            Line::Error { id, code, message } => {
                self.answers.insert(id, Err(format!("{code}: {message}")));
                return;
            }
            Line::Call { .. } => return,
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
                    self.panes.push(PaneView::new(pane, harness.clone(), spec));
                }
                if let Some(p) = self.view_mut(pane) {
                    p.turbo = turbo;
                }
                if !self.sessions.iter().any(|s| s.pane == Some(pane)) {
                    let mut row = weft_core::sessions::Row::of(
                        &harness,
                        &format!("s-{pane}"),
                        weft_core::sessions::State::Running,
                        Some(Turn::Ready),
                        0,
                    );
                    row.pane = Some(pane);
                    self.sessions.insert(0, row);
                }
            }
            Event::Exited { pane } => {
                if let Some(p) = self.view_mut(pane) {
                    p.running = false;
                }
            }
            Event::Units { units, records, rounds } => {
                self.units = units;
                self.records = records;
                self.rounds = rounds;
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
    }

    /// Start a new agent (or resume `session`). In disconnected test mode,
    /// immediately adds the pane locally so UI state updates deterministically.
    pub fn spawn(&mut self, harness: &str, spec: &str, session: Option<&str>) -> Result<u32> {
        let call = Call::StartAgent {
            harness: harness.to_string(),
            spec: spec.to_string(),
            session: session.map(str::to_string),
        };
        let call_id = self.tell(call)?;
        if self.stream.is_some() {
            let ans = self.answered(call_id)?;
            let pane_id = ans.get("pane").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            self.pump();
            if let Some(sid) = session {
                let placeholder = format!("s-{pane_id}");
                if self.sessions.iter().any(|r| r.id == sid) {
                    self.sessions.retain(|r| r.id != placeholder);
                    if let Some(existing) = self.sessions.iter_mut().find(|r| r.id == sid) {
                        existing.state = weft_core::sessions::State::Running;
                        existing.pane = Some(pane_id);
                        if existing.turn.is_none() {
                            existing.turn = Some(Turn::Ready);
                        }
                    }
                } else if let Some(ph) = self.sessions.iter_mut().find(|r| r.id == placeholder) {
                    ph.id = sid.to_string();
                }
            }
            Ok(pane_id)
        } else {
            let next_id = self.panes.iter().map(|p| p.id).max().map_or(0, |m| m + 1);
            let mut pv = PaneView::new(next_id, harness.to_string(), spec.to_string());
            pv.turbo = self.turbo;
            pv.turn = Some(Turn::Ready);
            self.panes.push(pv);
            let sid = session.map(str::to_string).unwrap_or_else(|| format!("sess-{next_id}"));
            if let Some(existing) = self.sessions.iter_mut().find(|r| r.id == sid) {
                existing.state = weft_core::sessions::State::Running;
                existing.pane = Some(next_id);
                existing.turn = Some(Turn::Ready);
            } else {
                let mut row = weft_core::sessions::Row::of(
                    harness,
                    &sid,
                    weft_core::sessions::State::Running,
                    Some(Turn::Ready),
                    1000,
                );
                row.pane = Some(next_id);
                self.sessions.insert(0, row);
            }
            Ok(next_id)
        }
    }

    pub fn input(&mut self, pane: u32, bytes: &[u8]) -> Result<()> {
        if let Some(p) = self.view_mut(pane) {
            p.scroll_to_bottom();
        }
        self.tell(Call::Input { pane, bytes: bytes.to_vec() }).map(|_| ())
    }

    pub fn scroll(&mut self, pane: u32, delta: i32) {
        if let Some(p) = self.view_mut(pane) {
            p.scroll(delta);
        }
    }

    /// Resize `pane` to `(rows, cols)`. When a local `weft-tui` is attached,
    /// `weft-desktop` updates its own `vt100` viewport without stealing PTY
    /// `Call::Resize` ownership; when no `weft-tui` is attached, `weft-desktop`
    /// sends `Call::Resize` to `weftd`.
    pub fn resize(&mut self, pane: u32, rows: u16, cols: u16) -> Result<()> {
        if let Some(p) = self.view_mut(pane) {
            p.resize(rows, cols);
        }
        if self.tui_attached {
            return Ok(());
        }
        if self.told.get(&pane) == Some(&(rows, cols)) {
            return Ok(());
        }
        self.told.insert(pane, (rows, cols));
        self.tell(Call::Resize { pane, rows, cols }).map(|_| ())
    }

    pub fn close_pane(&mut self, pane: u32) -> Result<()> {
        self.tell(Call::CloseAgent { pane })?;
        if self.stream.is_none() {
            if let Some(p) = self.view_mut(pane) {
                p.running = false;
            }
        }
        Ok(())
    }

    pub fn set_turbo(&mut self, on: bool) -> Result<u64> {
        self.turbo = on;
        self.tell(Call::Turbo { on })
    }

    pub fn set_start_at_login(&mut self, on: bool, program: &str, path: &str) -> Result<u64> {
        self.start_at_login = on;
        self.tell(Call::StartAtLogin { on, program: program.to_string(), path: path.to_string() })
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
        self.tell(Call::Act {
            act: act.to_string(),
            unit: unit.map(str::to_string),
            pane,
            text: text.map(str::to_string),
            eval: eval.map(str::to_string),
            start: start.map(str::to_string),
        })
    }

    pub fn read(
        &mut self,
        what: &str,
        unit: &str,
        pane: Option<u32>,
        lines: Option<u64>,
        before: Option<&str>,
        after: Option<&str>,
    ) -> Result<u64> {
        self.tell(Call::Read {
            what: what.to_string(),
            unit: unit.to_string(),
            pane,
            lines,
            before: before.map(str::to_string),
            after: after.map(str::to_string),
        })
    }

    pub fn resolve(
        &mut self,
        pending: &str,
        yes: bool,
        force: bool,
        to: Option<u32>,
        start: Option<&str>,
    ) -> Result<()> {
        self.tell(Call::Resolve {
            pending: pending.to_string(),
            yes,
            force,
            to,
            start: start.map(str::to_string),
        })?;
        if self.stream.is_none() {
            self.waiting.retain(|w| w.id != pending);
        }
        Ok(())
    }

    pub fn setup(&mut self, act: &str, harness: &str, path: &str) -> Result<()> {
        self.tell(Call::Setup {
            act: act.to_string(),
            harness: harness.to_string(),
            path: path.to_string(),
        })
        .map(|_| ())
    }

    pub fn remote_act(&mut self, act: Value) -> Result<()> {
        self.tell(Call::Remote { act }).map(|_| ())
    }

    pub fn dismiss(&mut self) -> Result<()> {
        self.tell(Call::Dismiss).map(|_| ())
    }

    pub fn detach(&mut self) {
        let _ = self.tell(Call::Detach);
    }

    pub fn shutdown(&mut self) {
        let _ = self.tell(Call::Shutdown);
    }

    pub fn stop_agents(&mut self) {
        let ids: Vec<u32> = self.panes.iter().map(|p| p.id).collect();
        for pane in ids.into_iter().rev() {
            let _ = self.tell(Call::CloseAgent { pane });
        }
    }

    /// Fetch the latest Conversation items for `pane` from `weftd` when connected.
    pub fn fetch_conversation(&mut self, pane: u32) -> Option<Vec<weft_core::conversation::Item>> {
        if !self.is_connected() {
            return None;
        }
        let id = self.read("conversation", "", Some(pane), None, None, None).ok()?;
        let ans = self.answered(id).ok()?;
        let items_val = ans.get("conversation")?.get("items")?.clone();
        serde_json::from_value(items_val).ok()
    }

    /// Fetch the lightweight judges record (`record.json`) for `eval_id` without
    /// blocking on full `changes.json` / `ledger.jsonl` diff construction.
    pub fn fetch_eval_record(&mut self, eval_id: &str) {
        if self.records.get(eval_id).is_some() {
            return;
        }
        let record_path = self.root.join(".fab7/rf/evals").join(eval_id).join("record.json");
        if let Ok(raw) = std::fs::read_to_string(&record_path)
            && let Ok(val) = serde_json::from_str::<Value>(&raw)
        {
            if let Some(obj) = self.records.as_object_mut() {
                obj.insert(eval_id.to_string(), val);
            } else {
                self.records = serde_json::json!({ eval_id: val });
            }
            return;
        }
        if !self.is_connected() {
            return;
        }
        if let Ok(id) = self.read("judges", eval_id, None, None, None, None)
            && let Ok(val) = self.answered(id)
            && !val.is_null()
        {
            if let Some(obj) = self.records.as_object_mut() {
                obj.insert(eval_id.to_string(), val);
            } else {
                self.records = serde_json::json!({ eval_id: val });
            }
        }
    }
}

/// Start `weft --serve` detached when no daemon is listening.
fn start_server() -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = find_weft_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc_setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let out = child.stdout.take().ok_or_else(|| anyhow!("the session server said nothing"))?;
    let (said, heard) = channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(out), &mut line);
        let _ = said.send(line);
    });
    match heard.recv_timeout(Duration::from_secs(5)) {
        Ok(line) if line.contains("listening") => Ok(()),
        _ => Err(anyhow!("the session server did not come up")),
    }
}

fn find_weft_exe() -> Result<PathBuf> {
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
