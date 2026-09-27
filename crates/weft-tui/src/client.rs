//! The client side of a session.
//!
//! A client is one view of panes the server owns. It keeps its own terminal
//! emulator per pane, fed by the harness's own bytes off the socket, so what
//! it draws is what the harness printed — and scrollback, being a property of
//! the view, stays here rather than crossing the wire.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

use weft_proto::{Call, Event, Line, Lines, PROTOCOL};
use weft_proto::{clear_dead, is_live, socket_path};

/// One pane, as this client sees it.
pub struct PaneView {
    /// The daemon's name for it, which never changes while it runs. Every
    /// call about a pane carries this, never its place in the list.
    pub id: u32,
    pub harness: String,
    /// The command line this pane runs, as the server reports it.
    pub spec: String,
    pub running: bool,
    /// Started in turbo mode, as the daemon reports it.
    pub turbo: bool,
    /// Its agent's state, as its hooks reported it and the daemon read it.
    pub turn: Option<weft_core::turns::Turn>,
    parser: vt100::Parser,
    rows: u16,
    cols: u16,
}

impl PaneView {
    /// From what the daemon says a pane is.
    fn of(p: weft_proto::PaneInfo) -> Self {
        let mut v = Self::new(p.pane, p.harness, p.spec);
        v.running = p.running;
        v.turbo = p.turbo;
        v
    }

    fn new(id: u32, harness: String, spec: String) -> Self {
        Self {
            id,
            harness,
            spec,
            running: true,
            turbo: false,
            turn: None,
            parser: vt100::Parser::new(24, 80, 10_000),
            rows: 24,
            cols: 80,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn with_screen<T>(&self, f: impl FnOnce(&vt100::Screen) -> T) -> T {
        f(self.parser.screen())
    }

    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    /// Scrollback belongs to the view, not the session: two clients may be
    /// looking at different parts of the same pane.
    pub fn scroll(&mut self, delta: i32) {
        let screen = self.parser.screen_mut();
        let at = screen.scrollback() as i32;
        screen.set_scrollback((at + delta).max(0) as usize);
    }

    pub fn scroll_offset(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn scroll_to_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) != (self.rows, self.cols) {
            self.parser.screen_mut().set_size(rows, cols);
            self.rows = rows;
            self.cols = cols;
        }
    }
}

/// What a window's one loop wakes for: the person at the terminal, or
/// something that arrived for it to take in (a daemon connection's lines,
/// a newer release).
pub enum Wake {
    Terminal(crossterm::event::Event),
    Arrived,
}

/// Where a connection says it has something, once a window listens.
type Nudge = std::sync::Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<Wake>>>>;

/// A staged prompt, as a client sees one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub id: String,
    /// The pane's id.
    pub pane: u32,
    pub what: String,
    pub why: String,
    pub payload: Vec<u8>,
}

pub struct Session {
    /// The daemon this client is attached to. A second project opens on the
    /// same one: the daemon is per machine and already holds many.
    socket: std::path::PathBuf,
    stream: UnixStream,
    inbox: Receiver<Line>,
    nudge: Nudge,
    /// Answers the daemon has sent, by call id.
    answers: std::collections::HashMap<u64, std::result::Result<serde_json::Value, String>>,
    pub panes: Vec<PaneView>,
    /// The last refusal the server reported, for the UI to show.
    pub last_refusal: Option<String>,
    /// Why RingFrame would not record the last send that went, if it would
    /// not.
    pub unrecorded: Option<String>,
    /// This project's board, as the daemon read it. A client renders the
    /// record; it does not read it.
    pub units: Vec<weft_core::ledger::Unit>,
    /// What each harness is short of here, as the daemon found it.
    pub readiness: serde_json::Value,
    /// The RingFrame view, as the daemon last reported it.
    pub sync: Option<weft_core::sync::View>,
    /// The Eval record behind each check, by eval id. Read by the daemon so
    /// that drawing a frame opens no file.
    pub records: serde_json::Value,
    /// What this project routes, and what an Ask could not finish here.
    pub routing: weft_core::routing::Routing,
    pub gap: Option<String>,
    /// What is waiting on a person, newest last. The daemon's, not this
    /// client's: another client may answer one of these.
    pub waiting: Vec<Staged>,
    /// The pane (by id) and size this client last told the daemon it draws.
    /// Told once per change, since telling is focusing: the agent takes the
    /// size of the window that last focused it or typed into it.
    told: Option<(u32, u16, u16)>,
    /// Every session's latest event, as the daemon read the receipts.
    pub turns: Vec<weft_core::turns::Session>,
    /// Whether to tell the person when an agent needs them, from Weft's
    /// `config.toml` as the daemon read it.
    pub notify: bool,
    /// Whether agents started here from now on run in turbo mode.
    pub turbo: bool,
    /// The harnesses Weft has a file for, as the daemon read them, and the
    /// files it could not read.
    pub harnesses: weft_core::harness::Harnesses,
    pub unread: Vec<String>,
}

impl Session {
    /// Attach to this project's session, starting one if none is listening.
    pub fn open(root: &Path, rows: u16, cols: u16) -> Result<Self> {
        let socket = socket_path();
        if !is_live(&socket) {
            clear_dead(&socket);
            start_server()?;
        }
        Self::connect(&socket, root, rows, cols)
    }

    /// The daemon this client is attached to, so another project can open on
    /// the same one rather than starting a second.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Attach to a daemon listening on this socket, watching one project.
    pub fn connect(socket: &Path, root: &Path, rows: u16, cols: u16) -> Result<Self> {
        let stream = UnixStream::connect(socket)
            .map_err(|e| anyhow!("no session at {}: {e}", socket.display()))?;

        let reading = stream.try_clone()?;
        let (tx, inbox) = channel();
        let nudge: Nudge = Default::default();
        let nudging = nudge.clone();
        std::thread::spawn(move || {
            let mut lines = Lines::new(reading);
            while let Ok(Some(line)) = lines.next() {
                if tx.send(line).is_err() {
                    return;
                }
                if let Some(w) = nudging.lock().ok().as_deref().and_then(Option::as_ref) {
                    let _ = w.send(Wake::Arrived);
                }
            }
        });

        let mut session = Session {
            socket: socket.to_path_buf(),
            stream,
            inbox,
            nudge,
            panes: Vec::new(),
            last_refusal: None,
            unrecorded: None,
            units: Vec::new(),
            readiness: serde_json::json!({}),
            sync: None,
            records: serde_json::json!({}),
            routing: weft_core::routing::Routing::default(),
            gap: None,
            waiting: Vec::new(),
            told: None,
            turns: Vec::new(),
            notify: true,
            turbo: false,
            harnesses: Default::default(),
            unread: Vec::new(),
            answers: std::collections::HashMap::new(),
        };
        // The handshake first: a daemon that speaks another protocol says so
        // with a number rather than leaving the client to guess.
        let hello = session.tell(Call::Hello {
            client: concat!("weft/", env!("CARGO_PKG_VERSION")).to_string(),
            protocol: PROTOCOL,
        })?;
        session.answered(hello)?;
        // The state comes back in the answer, so nothing is missed between
        // opening a project and the first pump.
        let open =
            session.tell(Call::Open { rows, cols, path: root.to_string_lossy().into_owned() })?;
        let opened = session.answered(open)?;
        if let Some(panes) = opened.get("panes").and_then(weft_proto::panes_of) {
            session.panes = panes.into_iter().map(PaneView::of).collect();
        }
        if let Some(units) = opened.get("units").cloned() {
            session.units = serde_json::from_value(units).unwrap_or_default();
        }
        session.readiness =
            opened.get("readiness").cloned().unwrap_or_else(|| serde_json::json!({}));
        session.records = opened.get("records").cloned().unwrap_or_else(|| serde_json::json!({}));
        if let Some(agents) = opened.get("agents") {
            session.take_agents(agents);
        }
        session.harnesses = opened
            .get("harnesses")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        session.unread = opened
            .get("unread")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        session.notify = opened.get("notify").and_then(|v| v.as_bool()).unwrap_or(true);
        session.turbo = opened.get("turbo").and_then(|v| v.as_bool()).unwrap_or(false);
        session.gap = opened.get("gap").and_then(|v| v.as_str()).map(str::to_string);
        if let Some(r) = opened.get("routing") {
            session.routing = weft_core::routing::Routing::of(
                r.get("acts").cloned().unwrap_or_default(),
                r.get("unknown").cloned().unwrap_or_default(),
                r.get("ignored").cloned().unwrap_or_default(),
                r.get("leftover").cloned().unwrap_or_default(),
                r.get("eval_stages").cloned().unwrap_or_default(),
            );
        }
        for chunk in opened.get("replay").and_then(|v| v.as_array()).unwrap_or(&Vec::new()) {
            if let Some((pane, bytes)) = weft_proto::output_of(chunk)
                && let Some(p) = session.view(pane)
            {
                p.feed(&bytes);
            }
        }
        Ok(session)
    }

    fn take_agents(&mut self, agents: &serde_json::Value) {
        let field = |k: &str| agents.get(k).cloned().unwrap_or_default();
        self.set_turns(serde_json::from_value(field("panes")).unwrap_or_default());
        self.turns = serde_json::from_value(field("sessions")).unwrap_or_default();
    }

    /// Wake this window's loop whenever the daemon sends something.
    pub fn wake(&mut self, loop_: std::sync::mpsc::Sender<Wake>) {
        if let Ok(mut n) = self.nudge.lock() {
            *n = Some(loop_);
        }
    }

    /// Each pane's agent state, one per pane in the daemon's order, which is
    /// this client's too: both follow the same events.
    pub fn set_turns(&mut self, turns: Vec<Option<weft_core::turns::Turn>>) {
        for (i, p) in self.panes.iter_mut().enumerate() {
            p.turn = turns.get(i).copied().flatten();
        }
    }

    /// Where the pane with this id is in the list.
    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.panes.iter().position(|p| p.id == id)
    }

    fn id_at(&self, pane: usize) -> Result<u32> {
        self.panes.get(pane).map(|p| p.id).ok_or_else(|| anyhow!("there is no pane {pane}"))
    }

    fn view(&mut self, id: u32) -> Option<&mut PaneView> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    /// Make a call. The answer arrives on the same socket, against this id,
    /// which no other connection in this process uses.
    pub fn tell(&mut self, call: Call) -> Result<u64> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        self.stream.write_all(&Line::Call { id, call }.encode())?;
        self.stream.flush()?;
        Ok(id)
    }

    /// The answer to a call made while connecting, before any window waits
    /// on this connection: the one wait there is, on the socket itself.
    fn answered(&mut self, id: u64) -> Result<serde_json::Value> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(answer) = self.answers.remove(&id) {
                return answer.map_err(|e| anyhow!(e));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let line =
                self.inbox.recv_timeout(left).map_err(|_| anyhow!("the session did not answer"))?;
            self.take(line);
        }
    }

    /// Every answer that has arrived, by call id, taken.
    pub fn answers(&mut self) -> Vec<(u64, std::result::Result<serde_json::Value, String>)> {
        self.answers.drain().collect()
    }

    /// Take in whatever has arrived. Returns true when anything changed.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        while let Ok(line) = self.inbox.try_recv() {
            changed = true;
            self.take(line);
        }
        changed
    }

    /// One line from the daemon: an answer, kept by call id, or an event.
    fn take(&mut self, line: Line) {
        {
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
                    // A pane already here keeps its screen and its state: its
                    // id is the same, so nothing needs replaying.
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
                Event::Output { pane, bytes } => {
                    if let Some(p) = self.view(pane) {
                        p.feed(&bytes);
                    }
                }
                Event::Added { pane, harness, spec, turbo } => {
                    if self.index_of(pane).is_none() {
                        self.panes.push(PaneView::new(pane, harness, spec));
                    }
                    if let Some(p) = self.view(pane) {
                        p.turbo = turbo;
                    }
                }
                Event::Exited { pane } => {
                    if let Some(p) = self.view(pane) {
                        p.running = false;
                    }
                }
                Event::Units { units, records } => {
                    self.units = units;
                    self.records = records;
                }
                Event::Pending { id, pane, what, why, payload } => {
                    self.waiting.push(Staged { id, pane, what, why, payload });
                }
                Event::Resolved { id, .. } => self.waiting.retain(|w| w.id != id),
                Event::Injected { refusal, unrecorded, .. } => {
                    self.last_refusal = refusal;
                    self.unrecorded = unrecorded;
                }
                Event::Readiness { states } => self.readiness = states,
                Event::Sync { view } => self.sync = serde_json::from_value(view).ok(),
                Event::Agents { panes, sessions } => {
                    self.set_turns(panes);
                    self.turns = sessions;
                }
            }
        }
    }

    /// Start an agent; `session` is the one it resumes, when it resumes one.
    /// Answered with its pane's id.
    pub fn spawn(&mut self, harness: &str, spec: &str, session: Option<&str>) -> Result<u64> {
        let (harness, spec, session) = (harness.into(), spec.into(), session.map(str::to_string));
        self.tell(Call::StartAgent { harness, spec, session })
    }

    pub fn input(&mut self, pane: usize, bytes: &[u8]) -> Result<()> {
        let pane = self.id_at(pane)?;
        self.tell(Call::Input { pane, bytes: bytes.to_vec() }).map(|_| ())
    }

    /// Put something in front of the person. The daemon holds it and tells
    /// every attached client; nothing is typed until it is resolved.
    pub fn stage(
        &mut self,
        pane: usize,
        bytes: &[u8],
        how: weft_core::inject::Handoff,
        what: &str,
        why: &str,
    ) -> Result<u64> {
        self.last_refusal = None;
        self.unrecorded = None;
        self.tell(Call::Stage {
            pane: self.id_at(pane)?,
            bytes: bytes.to_vec(),
            how,
            what: what.to_string(),
            why: why.to_string(),
        })
    }

    /// One of RingFrame's three acts, by name. The daemon works out what to
    /// type and where it goes; this only says which act. Answered with the
    /// pending it staged.
    pub fn act(
        &mut self,
        act: &str,
        unit: Option<&str>,
        pane: Option<usize>,
        text: Option<&str>,
    ) -> Result<u64> {
        self.last_refusal = None;
        self.unrecorded = None;
        let pane = pane.map(|p| self.id_at(p)).transpose()?;
        self.tell(Call::Act {
            act: act.to_string(),
            unit: unit.map(str::to_string),
            pane,
            text: text.map(str::to_string),
        })
    }

    /// The exact wording, or an Eval record, as RingFrame wrote them.
    pub fn read(&mut self, what: &str, unit: &str) -> Result<u64> {
        self.tell(Call::Read { what: what.to_string(), unit: unit.to_string() })
    }

    /// Turn turbo mode on or off for the agents started here from now on.
    /// Answered with `{"turbo": bool}`.
    pub fn set_turbo(&mut self, on: bool) -> Result<u64> {
        self.tell(Call::Turbo { on })
    }

    /// The agents that could be started here. Answered with `{"starts": …}`.
    pub fn available(&mut self) -> Result<u64> {
        self.tell(Call::Available)
    }

    /// Ask what is behind RingFrame's latest release; with `proceed`, catch
    /// it all up. The answer arrives as events.
    pub fn sync_now(&mut self, proceed: bool) -> Result<()> {
        self.tell(Call::Sync { proceed }).map(|_| ())
    }

    /// Yes or no, by id. Whoever answers first answers for everyone.
    pub fn resolve(&mut self, pending: &str, yes: bool, force: bool) -> Result<()> {
        self.tell(Call::Resolve { pending: pending.to_string(), yes, force }).map(|_| ())
    }

    pub fn resize(&mut self, pane: usize, rows: u16, cols: u16) -> Result<()> {
        let id = self.id_at(pane)?;
        if let Some(p) = self.panes.get_mut(pane) {
            p.resize(rows, cols);
        }
        if self.told == Some((id, rows, cols)) {
            return Ok(());
        }
        self.told = Some((id, rows, cols));
        self.tell(Call::Resize { pane: id, rows, cols }).map(|_| ())
    }

    /// Stop one agent and take its pane away. The server answers with the
    /// list again; every other pane keeps its id.
    pub fn close(&mut self, pane: usize) -> Result<()> {
        let pane = self.id_at(pane)?;
        self.tell(Call::CloseAgent { pane }).map(|_| ())
    }

    /// Leave without stopping anything. This is the ordinary way out.
    pub fn detach(&mut self) {
        let _ = self.tell(Call::Detach);
    }

    pub fn shutdown(&mut self) {
        let _ = self.tell(Call::Shutdown);
    }
}

/// Start the daemon, detached from this process so it survives the client that
/// asked for it. One per machine; it holds every project anyone opens.
fn start_server() -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    // Its own session, so closing the terminal does not take the agents with it.
    unsafe {
        cmd.pre_exec(|| {
            libc_setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    // It says one line once it listens; that line is waited for, not polled.
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

    #[test]
    fn a_view_rebuilds_the_screen_from_the_harnesses_own_bytes() {
        let mut v = PaneView::new(0, "codex".into(), "codex".into());
        v.feed(b"hello from the pane");
        assert!(v.contents().contains("hello from the pane"));
    }

    #[test]
    fn scrollback_is_the_views_own_business() {
        let mut v = PaneView::new(0, "sh".into(), "/bin/sh".into());
        for i in 1..=60 {
            v.feed(format!("line-{i}\r\n").as_bytes());
        }
        assert_eq!(v.scroll_offset(), 0);
        assert!(v.contents().contains("line-60"));

        v.scroll(30);
        assert_eq!(v.scroll_offset(), 30);
        assert!(v.contents().contains("line-20"), "older output is reachable");

        v.scroll_to_bottom();
        assert!(v.contents().contains("line-60"));
    }

    #[test]
    fn two_views_of_one_pane_scroll_independently() {
        let mut a = PaneView::new(0, "sh".into(), "/bin/sh".into());
        let mut b = PaneView::new(0, "sh".into(), "/bin/sh".into());
        for i in 1..=60 {
            let line = format!("line-{i}\r\n");
            a.feed(line.as_bytes());
            b.feed(line.as_bytes());
        }
        a.scroll(25);
        assert_eq!(a.scroll_offset(), 25);
        assert_eq!(b.scroll_offset(), 0, "one client scrolling does not move another");
    }

    fn until(s: &mut Session, done: impl Fn(&Session) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !done(s) {
            s.pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(done(s), "it never happened");
    }

    /// The daemon names a pane once, and closing one before it renumbers
    /// nothing: the pane keeps its id, its screen, and its keys.
    #[test]
    fn a_pane_keeps_its_id_when_one_before_it_closes() {
        let (_root, mut s) = crate::app::tests::test_session("ids");
        s.spawn("codex", "/bin/cat", None).expect("first");
        s.spawn("codex", "/bin/cat", None).expect("second");
        until(&mut s, |s| s.panes.len() == 2);
        let second = s.panes[1].id;
        assert_ne!(s.panes[0].id, second);
        s.input(1, b"second-pane\n").expect("typed");
        until(&mut s, |s| s.panes[1].contents().contains("second-pane"));
        s.close(0).expect("closed");
        until(&mut s, |s| s.panes.len() == 1);
        assert_eq!(s.panes[0].id, second, "the same id");
        assert!(s.panes[0].contents().contains("second-pane"), "and the same screen");
        s.input(0, b"still-here\n").expect("typed");
        until(&mut s, |s| s.panes[0].contents().contains("still-here"));
    }

    #[test]
    fn a_view_resizes_without_losing_what_is_on_it() {
        let mut v = PaneView::new(0, "sh".into(), "/bin/sh".into());
        v.feed(b"resize me");
        v.resize(30, 100);
        assert_eq!(v.with_screen(|s| s.size()), (30, 100));
        assert!(v.contents().contains("resize me"));
    }
}
