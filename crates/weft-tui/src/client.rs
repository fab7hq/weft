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
    pub harness: String,
    /// The command line this pane runs, as the server reports it.
    pub spec: String,
    pub running: bool,
    parser: vt100::Parser,
    rows: u16,
    cols: u16,
}

impl PaneView {
    /// From what the daemon says a pane is.
    fn of(p: weft_proto::PaneInfo) -> Self {
        let mut v = Self::new(p.harness, p.spec);
        v.running = p.running;
        v
    }

    fn new(harness: String, spec: String) -> Self {
        Self {
            harness,
            spec,
            running: true,
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

/// A staged prompt, as a client sees one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub id: String,
    pub pane: usize,
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
    /// The next call id. Answers come back against it.
    next_call: u64,
    /// Answers the daemon has sent, by call id.
    answers: std::collections::HashMap<u64, std::result::Result<serde_json::Value, String>>,
    pub panes: Vec<PaneView>,
    /// The last refusal the server reported, for the UI to show.
    pub last_refusal: Option<String>,
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

    /// Attach to a daemon on this socket, watching one project.
    pub fn connect(socket: &Path, root: &Path, rows: u16, cols: u16) -> Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match UnixStream::connect(socket) {
                Ok(s) => break s,
                Err(e) if Instant::now() >= deadline => {
                    return Err(anyhow!("no session at {}: {e}", socket.display()));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(25)),
            }
        };

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

        let mut session = Session {
            socket: socket.to_path_buf(),
            stream,
            inbox,
            panes: Vec::new(),
            last_refusal: None,
            units: Vec::new(),
            readiness: serde_json::json!({}),
            sync: None,
            records: serde_json::json!({}),
            routing: weft_core::routing::Routing::default(),
            gap: None,
            waiting: Vec::new(),
            next_call: 1,
            answers: std::collections::HashMap::new(),
        };
        // The handshake first: a daemon that speaks another protocol says so
        // with a number rather than leaving the client to guess.
        session.ask(Call::Hello {
            client: concat!("weft/", env!("CARGO_PKG_VERSION")).to_string(),
            protocol: PROTOCOL,
        })?;
        // The state comes back in the answer, so nothing is missed between
        // opening a project and the first pump.
        let opened =
            session.ask(Call::Open { rows, cols, path: root.to_string_lossy().into_owned() })?;
        if let Some(panes) = opened.get("panes").and_then(weft_proto::panes_of) {
            session.panes = panes.into_iter().map(PaneView::of).collect();
        }
        if let Some(units) = opened.get("units").cloned() {
            session.units = serde_json::from_value(units).unwrap_or_default();
        }
        session.readiness =
            opened.get("readiness").cloned().unwrap_or_else(|| serde_json::json!({}));
        session.records = opened.get("records").cloned().unwrap_or_else(|| serde_json::json!({}));
        session.gap = opened.get("gap").and_then(|v| v.as_str()).map(str::to_string);
        if let Some(r) = opened.get("routing") {
            session.routing = weft_core::routing::Routing::of(
                r.get("acts").cloned().unwrap_or_default(),
                r.get("unknown").cloned().unwrap_or_default(),
                r.get("ignored").cloned().unwrap_or_default(),
                r.get("eval_stages").cloned().unwrap_or_default(),
            );
        }
        for chunk in opened.get("replay").and_then(|v| v.as_array()).unwrap_or(&Vec::new()) {
            if let Some((pane, bytes)) = weft_proto::output_of(chunk)
                && let Some(p) = session.panes.get_mut(pane as usize)
            {
                p.feed(&bytes);
            }
        }
        Ok(session)
    }

    /// Make a call. The answer arrives on the same socket, against this id.
    fn tell(&mut self, call: Call) -> Result<u64> {
        let id = self.next_call;
        self.next_call += 1;
        self.stream.write_all(&Line::Call { id, call }.encode())?;
        self.stream.flush()?;
        Ok(id)
    }

    /// Make a call and wait for its answer.
    fn ask(&mut self, call: Call) -> Result<serde_json::Value> {
        let id = self.tell(call)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.pump();
            if let Some(answer) = self.answers.remove(&id) {
                return answer.map_err(|e| anyhow!(e));
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("the session did not answer"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Take in whatever has arrived. Returns true when anything changed.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        while let Ok(line) = self.inbox.try_recv() {
            changed = true;
            let event = match line {
                Line::Event(e) => e,
                Line::Result { id, result } => {
                    self.answers.insert(id, Ok(result));
                    continue;
                }
                Line::Error { id, code, message } => {
                    self.answers.insert(id, Err(format!("{code}: {message}")));
                    continue;
                }
                Line::Call { .. } => continue,
            };
            match event {
                Event::Panes { panes } => {
                    self.panes = panes.into_iter().map(PaneView::of).collect();
                }
                Event::Output { pane, bytes } => {
                    if let Some(p) = self.panes.get_mut(pane as usize) {
                        p.feed(&bytes);
                    }
                }
                Event::Added { pane, harness, spec } => {
                    while self.panes.len() <= pane as usize {
                        self.panes.push(PaneView::new(harness.clone(), spec.clone()));
                    }
                }
                Event::Exited { pane } => {
                    if let Some(p) = self.panes.get_mut(pane as usize) {
                        p.running = false;
                    }
                }
                Event::Units { units, records } => {
                    self.units = units;
                    self.records = records;
                }
                Event::Pending { id, pane, what, why, payload } => {
                    self.waiting.push(Staged { id, pane: pane as usize, what, why, payload });
                }
                Event::Resolved { id, .. } => self.waiting.retain(|w| w.id != id),
                Event::Injected { refusal, .. } => self.last_refusal = refusal,
                Event::Readiness { states } => self.readiness = states,
                Event::Sync { view } => self.sync = serde_json::from_value(view).ok(),
            }
        }
        changed
    }

    pub fn spawn(&mut self, harness: &str, spec: &str) -> Result<()> {
        self.tell(Call::StartAgent { harness: harness.into(), spec: spec.into() }).map(|_| ())
    }

    pub fn input(&mut self, pane: usize, bytes: &[u8]) -> Result<()> {
        self.tell(Call::Input { pane: pane as u32, bytes: bytes.to_vec() }).map(|_| ())
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
    ) -> Result<String> {
        self.last_refusal = None;
        let answer = self.ask(Call::Stage {
            pane: pane as u32,
            bytes: bytes.to_vec(),
            how,
            what: what.to_string(),
            why: why.to_string(),
        })?;
        answer
            .get("pending")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("the session staged nothing"))
    }

    /// One of RingFrame's three acts, by name. The daemon works out what to
    /// type and where it goes; this only says which act.
    pub fn act(
        &mut self,
        act: &str,
        unit: Option<&str>,
        pane: Option<usize>,
        text: Option<&str>,
    ) -> Result<String> {
        self.last_refusal = None;
        let answer = self.ask(Call::Act {
            act: act.to_string(),
            unit: unit.map(str::to_string),
            pane: pane.map(|p| p as u32),
            text: text.map(str::to_string),
        })?;
        answer
            .get("pending")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("the session staged nothing"))
    }

    /// The exact wording, or an Eval record, as RingFrame wrote them.
    pub fn read(&mut self, what: &str, unit: &str) -> Result<serde_json::Value> {
        self.ask(Call::Read { what: what.to_string(), unit: unit.to_string() })
    }

    /// The agents that could be started here.
    pub fn available(&mut self) -> Result<Vec<serde_json::Value>> {
        let answer = self.ask(Call::Available)?;
        Ok(answer.get("starts").and_then(|v| v.as_array()).cloned().unwrap_or_default())
    }

    /// Ask what is behind RingFrame's latest release; with `proceed`, catch
    /// it all up. The answer arrives as events.
    pub fn sync_now(&mut self, proceed: bool) -> Result<()> {
        self.ask(Call::Sync { proceed }).map(|_| ())
    }

    /// Yes or no, by id. Whoever answers first answers for everyone.
    pub fn resolve(&mut self, pending: &str, yes: bool, force: bool) -> Result<()> {
        self.tell(Call::Resolve { pending: pending.to_string(), yes, force }).map(|_| ())
    }

    pub fn resize(&mut self, pane: usize, rows: u16, cols: u16) -> Result<()> {
        if let Some(p) = self.panes.get_mut(pane) {
            p.resize(rows, cols);
        }
        self.tell(Call::Resize { pane: pane as u32, rows, cols }).map(|_| ())
    }

    /// Stop one agent and take its pane away. The server answers with the
    /// whole list again, so this client's numbering cannot go stale.
    pub fn close(&mut self, pane: usize) -> Result<()> {
        self.tell(Call::CloseAgent { pane: pane as u32 }).map(|_| ())
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
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Its own session, so closing the terminal does not take the agents with it.
    unsafe {
        cmd.pre_exec(|| {
            libc_setsid();
            Ok(())
        });
    }
    cmd.spawn()?;

    let socket = socket_path();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if is_live(&socket) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(anyhow!("the session server did not come up"))
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
        let mut v = PaneView::new("codex".into(), "codex".into());
        v.feed(b"hello from the pane");
        assert!(v.contents().contains("hello from the pane"));
    }

    #[test]
    fn scrollback_is_the_views_own_business() {
        let mut v = PaneView::new("sh".into(), "/bin/sh".into());
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
        let mut a = PaneView::new("sh".into(), "/bin/sh".into());
        let mut b = PaneView::new("sh".into(), "/bin/sh".into());
        for i in 1..=60 {
            let line = format!("line-{i}\r\n");
            a.feed(line.as_bytes());
            b.feed(line.as_bytes());
        }
        a.scroll(25);
        assert_eq!(a.scroll_offset(), 25);
        assert_eq!(b.scroll_offset(), 0, "one client scrolling does not move another");
    }

    #[test]
    fn a_view_resizes_without_losing_what_is_on_it() {
        let mut v = PaneView::new("sh".into(), "/bin/sh".into());
        v.feed(b"resize me");
        v.resize(30, 100);
        assert_eq!(v.with_screen(|s| s.size()), (30, 100));
        assert!(v.contents().contains("resize me"));
    }
}
