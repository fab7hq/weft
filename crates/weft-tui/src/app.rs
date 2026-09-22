//! The running application: state, events, and the loop.
//!
//! Spec: `plans/weft/spec/interface.md` (v2). Two nouns, two surfaces: agents
//! are tabs over the pane, work is a list. Details expand inline under a row;
//! reading opens a drawer beside the list.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::DefaultTerminal;
use ratatui::layout::Rect;

use crate::blocked::{self, Evidence};
use crate::client::Session;
use crate::encode;
use crate::keys::{self, Action, Chord, Focus, Key, Toggle};
use crate::ledger::Unit;
use crate::theme::Theme;
pub use weft_core::board::Act;
use weft_core::board::{Board, PaneInfo};
use weft_core::harness;
pub use weft_core::offers::{Ended, ended_choices, say_handoff};
use weft_core::readiness::{Gap, Readiness};
use weft_core::sessions::Recorded;

/// What Weft is about to type, and where. Shown before anything is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// The daemon's id for this. Resolving by id is what makes two clients
    /// safe: whoever answers first answers for everyone.
    pub staged: String,
    pub pane: usize,
    pub payload: Vec<u8>,
    pub what: String,
    pub why: Vec<String>,
}

/// The surfaces that interrupt. Everything else in v2 is drawn in place.
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    Quit,
    Help,
    /// Composing an intent.
    Ask {
        text: String,
        target: usize,
    },
    /// Weft is about to type something. Always shown first.
    Confirm(Pending),
    /// Something did not work, said plainly.
    Note(String),
    /// Pick an agent to start. Weft starts none on its own.
    StartAgent {
        choice: usize,
    },
    /// What Weft is about to run to set an agent up, and where. Shown first,
    /// always: installing into an agent is a larger act than typing into one.
    SetUp {
        harness: String,
        commands: Vec<String>,
        gap: Gap,
    },
    /// The agent in this pane is gone — it was quit from inside, or it
    /// stopped. Weft never keeps a harness session of its own, so what it can
    /// offer is whichever session RingFrame has a receipt for.
    Ended {
        pane: usize,
        harness: String,
        session: Option<Recorded>,
    },
}

/// The two reading surfaces. Siblings, not a stack: `P` from the judges
/// drawer swaps the content rather than piling a second overlay on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    Wording,
    Judges,
    Seal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drawer {
    pub kind: Reading,
    pub title: String,
    pub lines: Vec<String>,
    pub offset: usize,
}

pub struct App {
    pub project: String,
    pub root: PathBuf,
    /// The session's panes are the server's; this is our view of them.
    session: Session,
    pub pane_focus: usize,
    units: Vec<Unit>,
    pub selected: usize,
    /// The row expanded in place, if any.
    expanded: Option<usize>,
    /// How far the work list is scrolled. A list can be longer than the pane.
    list_offset: usize,
    drawer: Option<Drawer>,
    /// Whether the work list is on screen. Hidden, the agent has the width.
    show_work: bool,
    pub focus: Focus,
    pub toggle: Toggle,
    pub theme: Theme,
    pub modal: Option<Modal>,
    pub modal_choice: usize,
    pub quit: bool,
    pub stop_agents_on_quit: bool,
    /// One sentence saying why the key just pressed did nothing. Never a
    /// dialog: an unavailable action explains itself and stays on the bar.
    hint: Option<String>,
    /// What the last frame drew, so a click lands on what the person sees.
    pane_area: Option<Rect>,
    row_spans: Vec<(u16, u16, usize)>,
    tab_spans: Vec<(u16, u16, Option<usize>)>,
    action_spans: Vec<(u16, u16, Act)>,
    action_row: u16,
    list_area: Option<Rect>,
    needs_you_span: Option<(u16, u16)>,
    /// Whether the CLI that owns the record is installed. Asked once: it is a
    /// property of the machine, not of the frame being drawn.
    record_available: bool,
    /// Why this workspace could not finish an Ask, if it could not. Asked once
    /// for the same reason, and re-asked after a `[R]EADY UP`.
    workspace_gap: Option<String>,
    /// What each harness is short of, by the name RingFrame records. Asked
    /// when a pane starts, because that is when a person would care.
    readiness: std::collections::HashMap<String, Readiness>,
    /// The panes whose agent Weft has already said had ended. Said once: a
    /// pane that is gone stays gone, and repeating it would take the screen
    /// back every tick.
    announced: std::collections::HashSet<usize>,
    /// What could be started here, as of the last time anyone asked.
    starts: Vec<Start>,
    /// Which harness takes which act here. Read once: a person edits the file
    /// by hand, and a restart is a fair price for that.
    routing: weft_core::routing::Routing,
}

impl App {
    // --- what the interface may ask about ------------------------------------

    /// Number of panes in the session, which is what the UI counts.
    pub fn pane_count(&self) -> usize {
        self.session.panes.len()
    }

    pub fn harness_at(&self, i: usize) -> Option<&str> {
        self.session.panes.get(i).map(|p| p.harness.as_str())
    }

    pub fn units(&self) -> &[Unit] {
        &self.units
    }

    pub fn unit(&self, i: usize) -> Option<&Unit> {
        self.units.get(i)
    }

    pub fn expanded(&self) -> Option<usize> {
        self.expanded
    }

    pub fn drawer(&self) -> Option<&Drawer> {
        self.drawer.as_ref()
    }

    pub fn show_work(&self) -> bool {
        self.show_work
    }

    pub fn list_offset(&self) -> usize {
        self.list_offset
    }

    pub fn hint_text(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// The last refusal the server reported, if any. A refusal is Weft
    /// declining to type; it is never a statement about the harness.
    pub fn last_refusal(&self) -> Option<String> {
        self.session.last_refusal.clone()
    }

    /// Whether there is a `ringframe` to read a record from. Weft still runs
    /// panes without one; it just has nothing to show on the board.
    pub fn record_available(&self) -> bool {
        self.record_available
    }

    /// The Eval record behind a check, as the daemon read it. Nothing opens a
    /// file to draw a frame.
    pub fn record(&self, eval_id: &str) -> Option<weft_core::record::Record> {
        serde_json::from_value(self.session.records.get(eval_id)?.clone()).ok()
    }

    /// What this harness is short of, if anything. A harness Weft does not
    /// support, or has not asked about, is `Unknown` — never assumed ready.
    pub fn readiness(&self, harness: &str) -> Readiness {
        if let Some(local) = self.readiness.get(harness) {
            return *local;
        }
        readiness_of(self.session.readiness.get(harness).and_then(|v| v.as_str()))
    }

    /// What this project routes, for showing. Empty when it routes nothing.
    pub fn routing(&self) -> &weft_core::routing::Routing {
        &self.routing
    }

    /// What stops an Ask in this workspace, if anything does.
    pub fn workspace_gap(&self) -> Option<&str> {
        self.workspace_gap.as_deref()
    }

    /// The harness that decides what can be done now, when it is not ready.
    /// The hint says so persistently: a dimmed action with no reason on screen
    /// is the thing v1 did wrong.
    pub fn not_ready(&self) -> Option<(String, Readiness)> {
        let name = self.deciding_harness(Act::Ask)?;
        let state = self.readiness(&name);
        (!state.is_ready()).then_some((name, state))
    }

    /// Test seam: what RingFrame would say about this workspace.
    pub fn set_workspace_gap(&mut self, gap: Option<String>) {
        self.workspace_gap = gap;
    }

    /// Test seam: what a harness would be found to be, without asking it.
    /// Test seam: the Eval records a daemon would have read off disk.
    pub fn set_records(&mut self, records: serde_json::Value) {
        self.session.records = records;
    }

    /// Test seam: the routing a project would have read off disk.
    pub fn set_routing(&mut self, routing: weft_core::routing::Routing) {
        self.routing = routing;
    }

    pub fn set_readiness(&mut self, harness: &str, state: Readiness) {
        self.readiness.insert(harness.to_string(), state);
    }

    /// Whether a pane looks like it is waiting for a person. Inference, and
    /// labelled as such everywhere it is shown.
    pub fn waiting(&self, pane: usize) -> Option<Evidence> {
        let view = self.session.panes.get(pane)?;
        blocked::looks_blocked(&view.contents())
    }

    /// The facts a decision reads, borrowed from the state that holds them.
    /// The decision itself is `weft_core::board` (ADR-0007).
    fn with_board<T>(&self, f: impl FnOnce(Board<'_>) -> T) -> T {
        let panes = self.pane_facts();
        f(Board {
            units: &self.units,
            selected: self.selected,
            panes: &panes,
            focused: self.pane_focus,
            readiness: &|h| self.readiness(h),
            routing: &self.routing,
            workspace_gap: self.workspace_gap.as_deref(),
        })
    }

    fn pane_facts(&self) -> Vec<PaneInfo> {
        self.session
            .panes
            .iter()
            .enumerate()
            .map(|(i, p)| PaneInfo {
                pane: i as u32,
                harness: p.harness.clone(),
                spec: p.spec.clone(),
                running: p.running,
            })
            .collect()
    }

    /// Why an action is not available now, as one sentence. `None` means it is.
    pub fn unavailable(&self, act: Act) -> Option<String> {
        self.with_board(|b| b.unavailable(act))
    }

    pub fn deciding_harness(&self, act: Act) -> Option<String> {
        self.with_board(|b| b.deciding_harness(act))
    }

    pub fn selected_unit(&self) -> Option<&Unit> {
        self.units.get(self.selected)
    }

    /// Open units, as the title bar counts them.
    pub fn open_count(&self) -> usize {
        self.with_board(|b| b.open_count())
    }

    /// Units waiting on a decision only the person can make.
    pub fn needs_you(&self) -> usize {
        self.with_board(|b| b.needs_you())
    }

    fn pane_for(&self, harness: &str) -> Option<usize> {
        self.session.panes.iter().position(|p| p.harness == harness)
    }

    /// Draw the pane at its drawn size. The emulator is the client's, so the
    /// resize and the screen both belong here rather than in the render code.
    pub fn with_pane_screen(
        &mut self,
        pane: usize,
        rows: u16,
        cols: u16,
        f: impl FnOnce(&vt100::Screen),
    ) {
        let _ = self.session.resize(pane, rows, cols);
        if let Some(p) = self.session.panes.get(pane) {
            p.with_screen(f);
        }
    }

    // --- construction ---------------------------------------------------------

    pub fn new(root: PathBuf, toggle: Toggle) -> Self {
        let session = Session::open(&root, 24, 80).expect("a session");
        Self::with_session(root, toggle, session)
    }

    pub fn with_session(root: PathBuf, toggle: Toggle, session: Session) -> Self {
        let project = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into());
        Self {
            project,
            root,
            session,
            pane_focus: 0,
            units: Vec::new(),
            selected: 0,
            expanded: None,
            list_offset: 0,
            drawer: None,
            show_work: true,
            focus: Focus::Weft,
            toggle,
            theme: Theme::new(),
            modal: None,
            modal_choice: 0,
            quit: false,
            stop_agents_on_quit: false,
            hint: None,
            pane_area: None,
            row_spans: Vec::new(),
            tab_spans: Vec::new(),
            action_spans: Vec::new(),
            action_row: 0,
            list_area: None,
            needs_you_span: None,
            record_available: true,
            workspace_gap: None,
            readiness: std::collections::HashMap::new(),
            announced: std::collections::HashSet::new(),
            starts: Vec::new(),
            routing: Default::default(),
        }
    }

    /// `spec` is the command line the person would have typed, e.g.
    /// `claude --model sonnet --effort medium`. Weft never chooses the model:
    /// that is the harness's configuration and the person's decision.
    pub fn add(&mut self, harness: &str, spec: &str) -> Result<()> {
        self.session.spawn(harness, spec)?;
        // The pane appears when the server says it has one, and what the
        // harness is short of follows a moment later — asking it costs a
        // process, so the daemon does that off its own loop.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let before = self.session.panes.len();
        let mut running = false;
        while std::time::Instant::now() < deadline {
            self.session.pump();
            running |= self.session.panes.len() > before;
            if running && self.session.readiness.get(harness).is_some() {
                self.take_the_board();
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.take_the_board();
        self.look_for_agents();
        while !self.quit {
            terminal.draw(|frame| crate::ui::draw(frame, &mut self))?;
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key)
                        if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                    {
                        self.on_key(key)?
                    }
                    Event::Mouse(m) => self.on_mouse(m)?,
                    Event::Paste(text) => self.on_paste(&text)?,
                    _ => {}
                }
            }
            if self.session.pump() {
                self.take_the_board();
            }
            self.notice_an_agent_that_ended();
        }
        Ok(())
    }

    /// An agent quit from inside — `Ctrl+D`, `/exit`, or it simply stopped.
    /// The pane is still on screen showing its last frame, which is worth
    /// keeping; what must not happen is keystrokes going to a dead process.
    pub fn notice_an_agent_that_ended(&mut self) {
        let Some(pane) = (0..self.pane_count())
            .find(|i| !self.announced.contains(i) && !self.session.panes[*i].running)
        else {
            return;
        };
        self.announced.insert(pane);
        let harness = self.harness_at(pane).unwrap_or("the agent").to_string();
        // Focus leaves a pane nothing is listening to, whatever else happens.
        self.focus = Focus::Weft;
        if self.modal.is_some() {
            return;
        }
        // The session to offer is whichever this harness last used here, which
        // the daemon reads out of RingFrame's receipts.
        self.look_for_agents();
        let session = self
            .starts
            .iter()
            .find(|s| s.harness == harness && s.session.is_some())
            .and_then(|s| s.session.clone());
        self.modal_choice = 0;
        self.modal = Some(Modal::Ended { pane, harness, session });
    }

    /// Take a pane away and forget what was said about it. Every pane after it
    /// moves up one, so nothing may hold on to an index across this.
    fn close_pane(&mut self, pane: usize) {
        let before = self.pane_count();
        let _ = self.session.close(pane);
        // Wait for the server's new numbering before anything else acts on a
        // pane index. Every pane after this one moves up, and a spawn sent
        // into the gap would be counted against the old list.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && self.pane_count() >= before {
            self.session.pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        self.announced.clear();
        if self.pane_focus >= pane && self.pane_focus > 0 {
            self.pane_focus -= 1;
        }
        self.modal = None;
        self.focus = Focus::Weft;
    }

    /// Close the ended pane and start the harness again, either where it left
    /// off or fresh. Weft runs the command a person would type, and does not
    /// claim the session came back: the harness says that, in its own pane.
    fn start_again(&mut self, pane: usize, harness: &str, session: Option<&Recorded>) {
        let Some(h) = harness::find(harness) else {
            self.modal = Some(Modal::Note(format!("Weft does not know how to start {harness}.")));
            return;
        };
        let spec = match session {
            Some(s) => h.resume_spec(&s.id),
            None => h.spec(),
        };
        self.close_pane(pane);
        if let Err(e) = self.add(harness, &spec) {
            self.modal = Some(Modal::Note(format!("Could not start {spec}: {e}")));
        } else {
            self.pane_focus = self.pane_count().saturating_sub(1);
        }
    }

    /// Take the board the daemon read, and notice anything it implies. The
    /// record is followed once, in the daemon, and every client is told.
    fn take_the_board(&mut self) {
        if self.units != self.session.units {
            self.units = self.session.units.clone();
            self.clamp_selection();
        }
        self.routing = self.session.routing.clone();
        self.workspace_gap = self.session.gap.clone();
        // A missing CLI is the same answer for every harness, so any one of
        // them saying so is the machine saying so.
        self.record_available =
            !self.session.readiness.as_object().is_some_and(|m| m.values().any(|v| v == "no_cli"));
        // A name in the routing file that is not a harness Weft knows. Said
        // once, on the way in: an act routed nowhere would otherwise just look
        // unavailable for no reason anyone could see.
        if !self.routing.unknown.is_empty() && self.hint.is_none() {
            let said = self.routing.unknown.join(", ");
            self.say(format!(
                "Routing names a harness Weft does not know ({said}). That act is not routed."
            ));
        }
    }

    /// Fold whatever the ledger has now. The run loop does this each tick;
    /// a probe or a test does it once.
    /// Pump until the daemon has said what the record holds, then take it.
    /// The run loop does this continuously; a probe or a test does it once.
    pub fn refresh_for_test(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && self.session.units.is_empty() {
            self.session.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        self.take_the_board();
        self.look_for_agents();
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.units.len().saturating_sub(1));
        if self.expanded.is_some_and(|i| i >= self.units.len()) {
            self.expanded = None;
        }
    }

    fn say(&mut self, sentence: impl Into<String>) {
        self.hint = Some(sentence.into());
    }

    /// Answer a question the person was asked. The daemon does the typing,
    /// because the daemon owns the pane.
    fn do_inject(&mut self, pending: &Pending) {
        match self.session.resolve(&pending.staged, true) {
            Ok(()) => {
                self.modal = None;
                self.focus = Focus::Agent;
                self.pane_focus = pending.pane;
            }
            Err(e) => self.modal = Some(Modal::Note(said(&e))),
        }
    }

    /// Guard an action behind its availability, so the bar and the keyboard
    /// agree on what can be done and give the same reason when it cannot.
    fn guard(&mut self, act: Act) -> bool {
        match self.unavailable(act) {
            Some(why) => {
                self.say(why);
                false
            }
            None => true,
        }
    }

    // --- actions -----------------------------------------------------------

    /// Send the compiled prompt for the selected work.
    fn start_send(&mut self) {
        if !self.guard(Act::Send) {
            return;
        }
        let Some(id) = self.selected_unit().map(|u| u.ask_id.clone()) else { return };
        self.ask_first("send", Some(&id), None, None);
    }

    /// Check or Decide, in whichever harness this project routes it to.
    fn start_skill(&mut self, skill: &str) {
        let act = if skill == "eval" { Act::Check } else { Act::Decide };
        if !self.guard(act) {
            return;
        }
        let Some(id) = self.selected_unit().map(|u| u.ask_id.clone()) else { return };
        self.ask_first(if skill == "eval" { "check" } else { "decide" }, Some(&id), None, None);
    }

    fn send_ask(&mut self, text: String, target: usize) {
        self.ask_first("ask", None, Some(target), Some(&text));
    }

    /// The exact wording RingFrame compiled, read back through the daemon.
    fn read_wording(&mut self) {
        if !self.guard(Act::Wording) {
            return;
        }
        let Some(unit) = self.selected_unit().cloned() else { return };
        match self.session.read("wording", &unit.ask_id) {
            Ok(v) => {
                let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let mut lines = vec![
                    format!("asked {} · {}", unit.asked_at, unit.harness),
                    "This is the exact wording. Weft did not change it.".into(),
                    String::new(),
                ];
                lines.extend(text.lines().map(str::to_string));
                self.drawer = Some(Drawer {
                    kind: Reading::Wording,
                    title: format!("THE WORDING · {}", unit.title),
                    lines,
                    offset: 0,
                });
            }
            Err(e) => self.modal = Some(Modal::Note(said(&e))),
        }
    }

    /// The judges, their votes and their reasons.
    fn read_judges(&mut self) {
        if !self.guard(Act::Judges) {
            return;
        }
        let Some(unit) = self.selected_unit().cloned() else { return };
        let Some(check) = unit.check.clone() else { return };
        let record = match self.session.read("judges", &check.eval_id) {
            Ok(v) => serde_json::from_value::<weft_core::record::Record>(v).ok(),
            Err(e) => {
                self.modal = Some(Modal::Note(said(&e)));
                return;
            }
        };
        let Some(record) = record else {
            self.modal = Some(Modal::Note("That check's record is not on disk.".into()));
            return;
        };
        self.drawer = Some(Drawer {
            kind: Reading::Judges,
            title: format!("THE JUDGES · {}", unit.title),
            lines: weft_core::offers::judges_read(&record, &check),
            offset: 0,
        });
    }

    /// The Seal that closed the work, re-verified as it is read.
    fn read_seal(&mut self) {
        if !self.guard(Act::Seal) {
            return;
        }
        let Some(unit) = self.selected_unit().cloned() else { return };
        // `guard` already refused a unit with no seal; this keeps the reader
        // honest rather than inventing an id.
        let Some(seal_id) = unit.seal_id.clone() else { return };
        match self.session.read("seal", &seal_id) {
            Ok(v) => {
                self.drawer = Some(Drawer {
                    kind: Reading::Seal,
                    title: format!("THE SEAL · {}", unit.title),
                    lines: weft_core::offers::seal_read(&unit, &v),
                    offset: 0,
                });
            }
            Err(e) => self.modal = Some(Modal::Note(said(&e))),
        }
    }

    fn do_set_up(&mut self, name: &str) {
        match self.session.set_up(name) {
            Ok(()) => {
                self.modal = None;
                self.session.pump();
                let now = self.readiness(name);
                self.say(match now {
                    Readiness::Ready => format!("{name} is set up for RingFrame."),
                    other => other
                        .say(name)
                        .map(|s| format!("{s} The commands ran, so something else is wrong."))
                        .unwrap_or_default(),
                });
            }
            Err(e) => self.modal = Some(Modal::Note(format!("That did not work: {e}"))),
        }
    }

    fn start_ask(&mut self) {
        if !self.guard(Act::Ask) {
            return;
        }
        // The compose box opens on the harness this project asks in, when it
        // says. The person can still pick another before sending — routing is
        // where Weft starts, not somewhere it holds them.
        let target =
            self.routing.get("ask").and_then(|name| self.pane_for(name)).unwrap_or(self.pane_focus);
        self.modal = Some(Modal::Ask { text: String::new(), target });
    }

    /// What starting an agent could mean here. Asked when the picker opens,
    /// not while drawing: it reads receipts off disk in the daemon.
    pub fn starts(&self) -> &[Start] {
        &self.starts
    }

    fn look_for_agents(&mut self) {
        self.starts = self
            .session
            .available()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| {
                Some(Start {
                    harness: harness::find(v.get("harness")?.as_str()?)?.name,
                    label: v.get("label")?.as_str()?.to_string(),
                    spec: v.get("spec")?.as_str()?.to_string(),
                    session: v.get("session").and_then(|s| {
                        Some(Recorded {
                            id: s.get("id")?.as_str()?.to_string(),
                            at: s.get("at")?.as_str()?.to_string(),
                            last: s.get("last")?.as_str()?.to_string(),
                        })
                    }),
                })
            })
            .collect();
    }

    fn start_agent_picker(&mut self) {
        self.look_for_agents();
        let found = self.starts();
        if found.is_empty() {
            self.say("No coding agent found. Install claude or codex first.");
            return;
        }
        self.modal = Some(Modal::StartAgent { choice: 0 });
        self.modal_choice = 0;
    }

    pub fn start_chosen_agent(&mut self, choice: usize) {
        if self.starts.is_empty() {
            self.look_for_agents();
        }
        let Some(start) = self.starts.get(choice).cloned() else { return };
        match self.add(start.harness, &start.spec) {
            Ok(()) => {
                self.modal = None;
                self.pane_focus = self.session.panes.len().saturating_sub(1);
                self.focus = Focus::Agent;
            }
            Err(e) => self.modal = Some(Modal::Note(format!("Could not run {}: {e}", start.spec))),
        }
    }

    /// Show what setting this harness up would run, and run nothing yet.
    fn start_set_up(&mut self) {
        if !self.guard(Act::ReadyUp) {
            return;
        }
        let Some(name) = self.deciding_harness(Act::ReadyUp) else { return };
        let Some(h) = harness::find(&name) else { return };
        let gap = match self.readiness(&name) {
            Readiness::Missing(gap) => gap,
            _ => Gap::Plugin,
        };
        self.modal = Some(Modal::SetUp { harness: name, commands: h.setup_commands(), gap });
        self.modal_choice = 0;
    }

    fn decline_set_up(&mut self, name: &str) {
        self.readiness.insert(name.to_string(), Readiness::Declined);
        self.modal = None;
        self.say(format!("Not now for {name}. Your agent still runs here."));
    }

    /// Ask the daemon for an act, and show whatever it puts in front of the
    /// person. The daemon works out what to type, where it goes and how; this
    /// only names the act ([ADR-0007](../plans/weft/adr/0007-three-layers.md)).
    fn ask_first(
        &mut self,
        act: &str,
        unit: Option<&str>,
        pane: Option<usize>,
        text: Option<&str>,
    ) {
        match self.session.act(act, unit, pane, text) {
            Ok(id) => {
                self.session.pump();
                let Some(w) = self.session.waiting.iter().find(|w| w.id == id).cloned() else {
                    return;
                };
                self.modal = Some(Modal::Confirm(Pending {
                    staged: w.id,
                    pane: w.pane,
                    payload: w.payload,
                    what: w.what,
                    why: w.why.lines().map(str::to_string).collect(),
                }));
                self.modal_choice = 0;
            }
            Err(e) => self.modal = Some(Modal::Note(said(&e))),
        }
    }

    // --- the drawer ----------------------------------------------------------

    fn scroll_drawer(&mut self, delta: i32) {
        if let Some(d) = self.drawer.as_mut() {
            d.offset = (d.offset as i32 + delta).max(0) as usize;
        }
    }

    // --- moving about ---------------------------------------------------------

    fn pick(&mut self, delta: i8) {
        if self.units.is_empty() {
            return;
        }
        let n = self.units.len() as i32;
        self.selected = ((self.selected as i32 + delta as i32).rem_euclid(n)) as usize;
        // Picking a row that is scrolled away brings it back into view.
        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        }
        // The expansion belongs to the row it was opened on.
        if self.expanded.is_some_and(|i| i != self.selected) {
            self.expanded = None;
        }
    }

    /// Move through the agents the first-run picker offers. It wraps, like the
    /// work list, because a three-item list that stops is a list you fight.
    fn pick_agent(&mut self, delta: i8) {
        let n = self.starts().len();
        if n == 0 {
            return;
        }
        self.modal_choice =
            ((self.modal_choice as i32 + delta as i32).rem_euclid(n as i32)) as usize;
    }

    fn toggle_expand(&mut self) {
        if self.units.is_empty() {
            self.say("Nothing has been asked for yet.");
            return;
        }
        self.expanded = match self.expanded {
            Some(i) if i == self.selected => None,
            _ => Some(self.selected),
        };
    }

    /// `←` is back everywhere in Weft: it closes the drawer, then collapses the
    /// row. `Esc` is never Weft's.
    fn back(&mut self) {
        if self.drawer.take().is_some() {
            return;
        }
        if self.expanded.take().is_some() {
            return;
        }
        if !self.show_work {
            self.show_work = true;
        }
    }

    /// The next thing that needs you: a pane waiting for an answer first,
    /// because it is blocking whatever was asked of it, then the next row.
    fn next_needs_you(&mut self) {
        if let Some(pane) = self.next_waiting_pane() {
            self.pane_focus = pane;
            self.show_work = false;
            self.drawer = None;
            return;
        }
        let n = self.units.len();
        let start = self.selected;
        for step in 1..=n {
            let i = (start + step) % n;
            if self.units[i].needs_you() {
                self.selected = i;
                self.expanded = None;
                self.show_work = true;
                self.drawer = None;
                return;
            }
        }
        self.say("Nothing needs you.");
    }

    /// The next pane waiting for an answer, skipping the one already shown.
    fn next_waiting_pane(&self) -> Option<usize> {
        let n = self.pane_count();
        let showing_pane = !self.show_work || self.focus == Focus::Agent;
        for step in 1..=n {
            let i = (self.pane_focus + step) % n;
            if i == self.pane_focus && showing_pane {
                continue;
            }
            if self.waiting(i).is_some() {
                return Some(i);
            }
        }
        (!showing_pane && self.waiting(self.pane_focus).is_some()).then_some(self.pane_focus)
    }

    /// The one thing Weft infers rather than reads, and its evidence.
    fn explain_waiting(&mut self) {
        match self.waiting(self.pane_focus) {
            Some(e) => {
                let harness = self.harness_at(self.pane_focus).unwrap_or("the agent").to_string();
                self.say(format!(
                    "{harness}: the {} rule matched \"{}\" near the bottom of the screen.",
                    e.rule, e.line
                ));
            }
            None => self.say("Nothing on that screen looks like a question for you."),
        }
    }

    fn toggle_work(&mut self) {
        self.show_work = !self.show_work;
        if self.show_work {
            self.drawer = None;
        }
    }

    /// What an action does, wherever it was asked for. The bar shows a key for
    /// every one of these and each is also a click, so they meet here.
    pub fn act(&mut self, act: Act) {
        match act {
            Act::Ask => self.start_ask(),
            Act::Send => self.start_send(),
            Act::Check => self.start_skill("eval"),
            Act::Decide => self.start_skill("seal"),
            Act::Wording => self.read_wording(),
            Act::Judges => self.read_judges(),
            Act::Seal => self.read_seal(),
            Act::Fix => {
                if self.guard(Act::Fix) {
                    self.drawer = None;
                    self.modal = Some(Modal::Ask { text: String::new(), target: self.pane_focus });
                }
            }
            Act::ReadyUp => self.start_set_up(),
            Act::OpenAgent => self.open_the_agent(),
            Act::NewAgent => self.start_agent_picker(),
            Act::Work => self.toggle_work(),
            Act::Help => {
                self.modal = Some(Modal::Help);
                self.modal_choice = 0;
            }
            Act::Quit => {
                self.modal = Some(Modal::Quit);
                self.modal_choice = 0;
            }
        }
    }

    /// Open the agent that has the selected work, in the session it was asked
    /// in. The command is the one a person would type; Weft claims nothing
    /// about what comes back, which the harness shows in its own pane.
    fn open_the_agent(&mut self) {
        if !self.guard(Act::OpenAgent) {
            return;
        }
        let Some(u) = self.selected_unit() else { return };
        let (harness, id) = (u.harness.clone(), u.session_ref.clone().unwrap_or_default());
        let Some(h) = harness::find(&harness) else { return };
        let spec = h.resume_spec(&id);
        match self.add(&harness, &spec) {
            Ok(()) => {
                self.pane_focus = self.pane_count().saturating_sub(1);
                self.show_work = false;
                self.focus = Focus::Agent;
            }
            Err(e) => self.modal = Some(Modal::Note(format!("Could not run {spec}: {e}"))),
        }
    }

    fn scroll_list(&mut self, delta: i32) {
        let rows = self.units.len().saturating_sub(1);
        self.list_offset = (self.list_offset as i32 + delta).clamp(0, rows as i32) as usize;
    }

    // --- events ------------------------------------------------------------

    /// One keystroke. Public so a probe or a wireframe run can drive the
    /// same path the terminal does.
    pub fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.modal.is_some() {
            return self.on_modal_key(key);
        }
        self.hint = None;
        let chord = chord_of(key);
        match keys::route(chord, self.focus, self.toggle) {
            Action::ToggleFocus => self.toggle_focus(),
            Action::ToAgent => {
                if let Some(bytes) = encode::encode(key) {
                    let _ = self.session.input(self.pane_focus, &bytes);
                }
            }
            Action::Pick(delta) => {
                if self.pane_count() == 0 {
                    // First run: the picker is the surface, and it is drawn in
                    // the body rather than as a modal, so it owns the arrows.
                    self.pick_agent(delta);
                } else if self.drawer.is_some() {
                    self.scroll_drawer(delta as i32);
                } else {
                    self.pick(delta);
                }
            }
            Action::Open => {
                if self.pane_count() == 0 {
                    self.start_chosen_agent(self.modal_choice);
                } else if self.waiting_here() {
                    // [Enter] ANSWER IT: the person answers, never Weft.
                    self.focus = Focus::Agent;
                } else if self.drawer.is_none() {
                    self.toggle_expand();
                }
            }
            Action::Back => self.back(),
            Action::NextNeedsYou => self.next_needs_you(),
            Action::ToggleWork => self.act(Act::Work),
            Action::NextPane => {
                if self.pane_count() > 0 {
                    self.pane_focus = (self.pane_focus + 1) % self.pane_count();
                }
            }
            Action::PickPane(n) => {
                let i = (n as usize).saturating_sub(1);
                if i < self.pane_count() {
                    self.pane_focus = i;
                }
            }
            Action::Ask => self.act(Act::Ask),
            Action::Send => self.act(Act::Send),
            Action::NewAgent => self.act(Act::NewAgent),
            Action::Check => self.act(Act::Check),
            Action::Decide => self.act(Act::Decide),
            Action::Wording => self.act(Act::Wording),
            Action::Judges => self.act(Act::Judges),
            Action::Seal => self.act(Act::Seal),
            Action::Fix => self.act(Act::Fix),
            Action::Explain => self.explain_waiting(),
            Action::ReadyUp => self.act(Act::ReadyUp),
            Action::OpenAgent => self.act(Act::OpenAgent),
            Action::Quit => self.act(Act::Quit),
            Action::Help => self.act(Act::Help),
            Action::Ignore => {}
        }
        Ok(())
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Weft => Focus::Agent,
            Focus::Agent => {
                // Coming back lands on the work list, which is what `Ctrl+]`
                // out of the agent is for.
                self.show_work = true;
                Focus::Weft
            }
        };
    }

    /// Whether the agent on screen is the thing waiting for an answer.
    pub fn waiting_here(&self) -> bool {
        !self.show_work && self.waiting(self.pane_focus).is_some()
    }

    fn on_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        let modal = self.modal.clone().expect("a modal");

        // Composing text is its own keyboard.
        if let Modal::Ask { mut text, mut target } = modal {
            match key.code {
                // `[←] CANCEL` is what the box offers, so it cancels whether
                // or not anything has been typed.
                event::KeyCode::Left | event::KeyCode::Esc => self.modal = None,
                event::KeyCode::Enter if !text.trim().is_empty() => self.send_ask(text, target),
                event::KeyCode::Backspace => {
                    text.pop();
                    self.modal = Some(Modal::Ask { text, target });
                }
                event::KeyCode::Tab => {
                    if self.pane_count() > 0 {
                        target = (target + 1) % self.pane_count();
                    }
                    self.modal = Some(Modal::Ask { text, target });
                }
                event::KeyCode::Char(c) => {
                    text.push(c);
                    self.modal = Some(Modal::Ask { text, target });
                }
                _ => {}
            }
            return Ok(());
        }

        let options = match &modal {
            Modal::Quit => 3,
            Modal::StartAgent { .. } => self.starts().len().max(1),
            Modal::Ended { session, .. } => ended_choices(session.as_ref()).len(),
            _ => 1,
        };
        match key.code {
            // The agent picker is the same picker wherever it is drawn.
            event::KeyCode::Up if matches!(modal, Modal::StartAgent { .. }) => self.pick_agent(-1),
            event::KeyCode::Down if matches!(modal, Modal::StartAgent { .. }) => self.pick_agent(1),
            event::KeyCode::Up => self.modal_choice = self.modal_choice.saturating_sub(1),
            event::KeyCode::Down => self.modal_choice = (self.modal_choice + 1).min(options - 1),
            // `←` is back everywhere, and on this one panel it is an answer:
            // not now, remembered, rather than a question asked again.
            event::KeyCode::Left | event::KeyCode::Esc => match &modal {
                Modal::SetUp { harness, .. } => {
                    let harness = harness.clone();
                    self.decline_set_up(&harness);
                }
                // The agent is gone either way, so backing out of this panel
                // takes the pane with it rather than leaving a dead one up.
                Modal::Ended { pane, .. } => {
                    let pane = *pane;
                    self.close_pane(pane);
                }
                // A question nobody is going to answer is taken off the
                // daemon's queue, not left there for another client.
                Modal::Confirm(p) => {
                    let id = p.staged.clone();
                    let _ = self.session.resolve(&id, false);
                    self.modal = None;
                }
                _ => self.modal = None,
            },
            event::KeyCode::Enter => self.confirm_modal(&modal),
            _ => {}
        }
        Ok(())
    }

    fn confirm_modal(&mut self, modal: &Modal) {
        match modal {
            Modal::Help | Modal::Note(_) => self.modal = None,
            Modal::Ask { .. } => {}
            Modal::Confirm(pending) => {
                let pending = pending.clone();
                self.do_inject(&pending);
            }
            Modal::StartAgent { .. } => self.start_chosen_agent(self.modal_choice),
            Modal::SetUp { harness, .. } => {
                let harness = harness.clone();
                self.do_set_up(&harness);
            }
            Modal::Ended { pane, harness, session } => {
                let (pane, harness, session) = (*pane, harness.clone(), session.clone());
                match ended_choices(session.as_ref()).get(self.modal_choice) {
                    Some(Ended::Resume) => self.start_again(pane, &harness, session.as_ref()),
                    Some(Ended::Fresh) => self.start_again(pane, &harness, None),
                    _ => self.close_pane(pane),
                }
            }
            Modal::Quit => match self.modal_choice {
                0 => {
                    // Leave; the server keeps the agents working.
                    self.session.detach();
                    self.quit = true;
                }
                1 => {
                    self.stop_agents_on_quit = true;
                    self.session.shutdown();
                    self.quit = true;
                }
                _ => self.modal = None,
            },
        }
    }

    /// Pasted text belongs to whoever has focus. In the agent it is forwarded
    /// whole, so a multi-line paste arrives as one paste, not as keystrokes.
    fn on_paste(&mut self, text: &str) -> Result<()> {
        match (&mut self.modal, self.focus) {
            (Some(Modal::Ask { text: buf, .. }), _) => buf.push_str(text),
            (Some(_), _) => {}
            (None, Focus::Agent) => {
                let _ = self.session.input(self.pane_focus, text.as_bytes());
            }
            (None, Focus::Weft) => {}
        }
        Ok(())
    }

    /// One mouse event. Public for the same reason `on_key` is.
    pub fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        // The wheel moves whatever is being read: the drawer if one is open,
        // otherwise the focused pane's scrollback.
        let wheel = match m.kind {
            MouseEventKind::ScrollUp => Some(-3),
            MouseEventKind::ScrollDown => Some(3),
            _ => None,
        };
        if let Some(delta) = wheel {
            if self.drawer.is_some() {
                self.scroll_drawer(delta);
            } else if self.over_list(m.column, m.row) {
                self.scroll_list(delta);
            } else {
                self.scroll_pane(-delta);
            }
            return Ok(());
        }
        if self.modal.is_some() {
            return Ok(());
        }
        if m.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(());
        }
        if self.needs_you_span.is_some_and(|(x, w)| m.row == 0 && m.column >= x && m.column < x + w)
        {
            self.next_needs_you();
            return Ok(());
        }
        if let Some(act) = self.action_at(m.column, m.row) {
            self.focus = Focus::Weft;
            self.act(act);
            return Ok(());
        }
        // The left pane is Weft. Clicking it comes out of the agent even when
        // it lands between rows or on an empty list, which is where a person
        // clicks when there is no work on the board yet.
        if self.over_list(m.column, m.row) {
            self.focus = Focus::Weft;
        }
        if let Some(target) = self.tab_at(m.column, m.row) {
            match target {
                Some(pane) => {
                    self.pane_focus = pane;
                    self.focus = Focus::Weft;
                }
                None => self.start_agent_picker(),
            }
            return Ok(());
        }
        let in_pane = self.pane_area.is_some_and(|a| {
            m.column >= a.x && m.column < a.x + a.width && m.row >= a.y && m.row < a.y + a.height
        });
        if in_pane && self.drawer.is_none() {
            // Clicking the pane is a toggle: into the agent, and out again.
            // The same click should not mean two different things.
            self.focus = match self.focus {
                Focus::Weft => {
                    if let Some(p) = self.session.panes.get_mut(self.pane_focus) {
                        p.scroll_to_bottom();
                    }
                    Focus::Agent
                }
                Focus::Agent => Focus::Weft,
            };
            return Ok(());
        }
        if self.pane_count() == 0 {
            if let Some(index) = self.row_at(m.row) {
                // The first-run screen says to click anything, so a click on a
                // choice picks it and a second one starts it.
                if self.modal_choice == index {
                    self.start_chosen_agent(index);
                } else {
                    self.modal_choice = index;
                }
            }
            return Ok(());
        }
        if let Some(index) = self.row_at(m.row) {
            self.focus = Focus::Weft;
            if self.selected == index {
                self.toggle_expand();
            } else {
                self.selected = index;
                self.expanded = None;
            }
        }
        Ok(())
    }

    /// The wheel over a pane. Weft moves the scrollback it kept — and when it
    /// kept none, says whose scrollback it is rather than doing nothing at
    /// all. Codex repaints its viewport instead of scrolling, so Weft never
    /// sees a line leave; its history is behind its own key. Measured, not
    /// assumed: see `harness::Harness::transcript`.
    fn scroll_pane(&mut self, delta: i32) {
        let Some(p) = self.session.panes.get_mut(self.pane_focus) else { return };
        let before = p.scroll_offset();
        p.scroll(delta);
        if p.scroll_offset() != before || delta < 0 {
            return;
        }
        let name = self.harness_at(self.pane_focus).unwrap_or("the agent").to_string();
        let said = match harness::find(&name).and_then(|h| h.transcript) {
            Some(key) => {
                format!("{name} keeps its own history: press {key} in the agent to read it.")
            }
            None => format!("Nothing of {name}'s has scrolled away yet."),
        };
        self.say(said);
    }

    fn over_list(&self, column: u16, row: u16) -> bool {
        self.list_area.is_some_and(|a| {
            column >= a.x && column < a.x + a.width && row >= a.y && row < a.y + a.height
        })
    }

    /// An action word on the bar is the same action as its key.
    fn action_at(&self, column: u16, row: u16) -> Option<Act> {
        self.action_spans
            .iter()
            .find(|(x, w, _)| row == self.action_row && column >= *x && column < x + w)
            .map(|(_, _, act)| *act)
    }

    fn row_at(&self, row: u16) -> Option<usize> {
        self.row_spans.iter().find(|(y, h, _)| row >= *y && row < y + h).map(|(_, _, i)| *i)
    }

    /// `Some(Some(pane))` is an agent tab; `Some(None)` is the `+`.
    fn tab_at(&self, column: u16, row: u16) -> Option<Option<usize>> {
        self.tab_spans
            .iter()
            .find(|(x, w, _)| row == 1 && column >= *x && column < x + w)
            .map(|(_, _, pane)| *pane)
    }

    // --- what the last frame drew --------------------------------------------

    pub fn note_pane_area(&mut self, area: Rect) {
        self.pane_area = Some(area);
    }

    pub fn note_rows(&mut self, spans: Vec<(u16, u16, usize)>) {
        self.row_spans = spans;
    }

    pub fn note_tabs(&mut self, spans: Vec<(u16, u16, Option<usize>)>) {
        self.tab_spans = spans;
    }

    pub fn note_actions(&mut self, row: u16, spans: Vec<(u16, u16, Act)>) {
        self.action_row = row;
        self.action_spans = spans;
    }

    pub fn note_list_area(&mut self, area: Option<Rect>) {
        self.list_area = area;
    }

    pub fn note_needs_you(&mut self, span: Option<(u16, u16)>) {
        self.needs_you_span = span;
    }

    // --- test seams -----------------------------------------------------------

    /// The units the board is showing.
    pub fn set_units(&mut self, units: Vec<Unit>) {
        self.units = units;
        self.clamp_selection();
    }

    pub fn input(&mut self, pane: usize, bytes: &[u8]) -> Result<()> {
        self.session.input(pane, bytes)
    }

    pub fn pump(&mut self) -> bool {
        self.session.pump()
    }

    pub fn pane_text(&self, pane: usize) -> Option<String> {
        self.session.panes.get(pane).map(|p| p.contents())
    }

    pub fn pane_scroll_offset(&self, pane: usize) -> Option<usize> {
        self.session.panes.get(pane).map(|p| p.scroll_offset())
    }
}

/// One way to start an agent: a harness, and the command line that does it.
/// The command is shown before it runs and is the one a person would type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub harness: &'static str,
    pub label: String,
    pub spec: String,
    /// The session this would pick up, when it picks one up.
    pub session: Option<Recorded>,
}

fn chord_of(key: KeyEvent) -> Chord {
    let code = match key.code {
        event::KeyCode::Char(c) => Key::Char(c),
        event::KeyCode::Up => Key::Up,
        event::KeyCode::Down => Key::Down,
        event::KeyCode::Left => Key::Left,
        event::KeyCode::Enter => Key::Enter,
        event::KeyCode::Tab => Key::Tab,
        event::KeyCode::Esc => Key::Esc,
        _ => Key::Char('\0'),
    };
    Chord {
        code,
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ledger::Sent;
    use crossterm::event::KeyCode;
    use serde_json::json;
    use weft_core::offers::first_line;
    use weft_core::readiness::{Gap, Readiness};

    /// A session backed by a real server on a scratch root, because the app is
    /// a client now and there is no honest way to test it without one.
    /// Whether this machine has Codex at all. Some tests are about what Weft
    /// offers for a harness that exists, and there is nothing to offer when it
    /// does not.
    pub(crate) fn codex_on_path() -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("codex").is_file())
        })
    }

    pub(crate) fn test_session(name: &str) -> (PathBuf, crate::client::Session) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("weft-app-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).expect("root");
        // A real workspace is a Git repository with a commit, which is what
        // RingFrame requires before it will compile an Ask. A fixture that is
        // not one tests a situation Weft now refuses.
        for args in [vec!["init", "-q"], vec!["commit", "-q", "--allow-empty", "-m", "fixture"]] {
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(&args)
                .env("GIT_AUTHOR_NAME", "weft")
                .env("GIT_AUTHOR_EMAIL", "weft@example.invalid")
                .env("GIT_COMMITTER_NAME", "weft")
                .env("GIT_COMMITTER_EMAIL", "weft@example.invalid")
                .output();
        }
        let socket = weft_proto::private_socket("weft-app");
        let _ = std::fs::remove_file(&socket);
        let listening = socket.clone();
        std::thread::spawn(move || {
            let _ = weftd::server::Session::serve(&listening);
        });
        let session = crate::client::Session::connect(&socket, &root, 24, 80).expect("connect");
        (root, session)
    }

    pub(crate) fn app() -> App {
        let (root, session) = test_session("codex");
        let mut a = App::with_session(root, Toggle, session);
        a.add("codex", "/bin/cat").expect("spawn");
        // Say what this fixture's readiness is instead of inheriting the
        // machine's. Otherwise these tests pass on a laptop with Codex and the
        // plugin installed and fail everywhere else, which is not a fact about
        // the code. Tests about readiness itself set their own.
        a.set_readiness("codex", Readiness::Ready);
        a.set_readiness("claude-code", Readiness::Ready);
        a
    }

    fn with_unit(sent: Sent) -> App {
        let mut a = app();
        a.set_units(vec![unit(sent)]);
        a
    }

    /// Put these Asks on the project's real ledger, so the daemon knows them.
    pub(crate) fn record_units(app: &App, ids: &[&str]) {
        let lines: String = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                serde_json::json!({
                    "schema": "ringframe.ledger/1", "event_id": format!("evt_{i}"),
                    "type": "ask.compiled", "time": "2026-09-19T14:02:00Z", "id": id,
                    "actor": {"kind": "human", "id": "local-user"}, "links": [],
                    "data": {
                        "title": "t", "selected_capability": "native_plan",
                        "delivery_mode": "human_handoff",
                        "host": {"name": "codex", "profile_id": "codex"},
                        "source": {"role": "source_intent", "path": "x", "bytes": 2, "sha256": "a"},
                        "prompt": {"role": "generated_prompt", "path": "y", "bytes": 9, "sha256": "b"},
                        "source_verified": "exact", "limitations": [],
                        "classification": {}, "route_explanation": {}
                    }
                })
                .to_string()
                    + "\n"
            })
            .collect();
        let rf = app.root.join(".fab7/rf");
        std::fs::create_dir_all(&rf).expect("rf");
        std::fs::write(rf.join("ledger.jsonl"), lines).expect("ledger");
    }

    /// A project whose daemon can see this work, because it is really on the
    /// ledger. `set_units` puts a unit in front of the person; only the record
    /// puts it where an act can reach it.
    fn recorded(sent: Sent) -> App {
        let mut a = app();
        let mut events = vec![serde_json::json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
            "time": "2026-09-19T14:02:00Z", "id": "ask_1",
            "actor": {"kind": "human", "id": "local-user"}, "links": [],
            "data": {
                "title": "health endpoint", "selected_capability": "native_plan",
                "delivery_mode": "human_handoff",
                "host": {"name": "codex", "profile_id": "codex"},
                "source": {"role": "source_intent", "path": "x", "bytes": 2, "sha256": "a"},
                "prompt": {"role": "generated_prompt", "path": "y", "bytes": 9, "sha256": "b"},
                "source_verified": "exact", "limitations": [],
                "classification": {}, "route_explanation": {}
            }
        })];
        if sent == Sent::ReadyToSend {
            events.push(serde_json::json!({
                "schema": "ringframe.ledger/1", "event_id": "evt_2", "type": "ask.confirmed",
                "time": "2026-09-19T14:03:00Z", "id": "ask_1",
                "actor": {"kind": "human", "id": "local-user"}, "links": [],
                "data": {"confirmation": {"observed_by": "skill"}}
            }));
        }
        let rf = a.root.join(".fab7/rf");
        std::fs::create_dir_all(&rf).expect("rf");
        let lines: String = events.iter().map(|e| format!("{e}\n")).collect();
        std::fs::write(rf.join("ledger.jsonl"), lines).expect("ledger");
        // Wait for the daemon's next look, then take what it read.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.session.units.is_empty() {
            a.session.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        a.take_the_board();
        a
    }

    pub(crate) fn unit(sent: Sent) -> Unit {
        Unit {
            ask_id: "ask_1".into(),
            title: "health endpoint".into(),
            harness: "codex".into(),
            session_ref: Some("01a0bdb6-1d1f-79c2-84b0-8b03496d7db0".into()),
            delivery: Default::default(),
            route: "native_plan".into(),
            asked_at: "2026-09-19T14:02:00Z".into(),
            delivery_mode: "human_handoff".into(),
            cancelled: false,
            unanswered: false,
            confirmed: true,
            sent,
            check: None,
            sealed: None,
            seal_id: None,
            sealed_at: None,
        }
    }

    pub(crate) fn press(a: &mut App, code: KeyCode) {
        a.on_key(KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    }

    fn ctrl(a: &mut App, c: char) {
        a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)).expect("key");
    }

    /// Write the receipt RingFrame's hook would have written, so the panel has
    /// a session to name. Weft only ever reads this.
    fn record_a_session(root: &std::path::Path, harness: &str, id: &str) {
        let dir = root.join(".fab7/rf/sessions").join(harness).join(id);
        std::fs::create_dir_all(&dir).expect("session dir");
        let line = serde_json::json!({
            "session_id": id, "time": "2026-09-20T07:27:14.769Z",
            "prompt": "$rf:ask research crypto trading", "cwd": root.to_string_lossy(),
        });
        std::fs::write(dir.join("prompts.jsonl"), format!("{line}\n")).expect("receipt");
    }

    #[test]
    fn an_agent_that_quits_is_said_once_and_focus_leaves_its_pane() {
        let mut a = app();
        a.focus = Focus::Agent;
        a.input(0, &[4]).expect("Ctrl+D ends /bin/cat");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            a.pump();
            a.notice_an_agent_that_ended();
            if a.modal.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let Some(Modal::Ended { pane, harness, .. }) = a.modal.clone() else {
            panic!("the panel says the agent has gone: {:?}", a.modal)
        };
        assert_eq!((pane, harness.as_str()), (0, "codex"));
        assert_eq!(a.focus, Focus::Weft, "keystrokes never go to a dead process");

        // Said once. A pane that is gone stays gone.
        a.modal = None;
        a.notice_an_agent_that_ended();
        assert!(a.modal.is_none(), "not raised again every tick");
    }

    #[test]
    fn resuming_is_offered_only_when_the_record_names_a_session() {
        assert_eq!(ended_choices(None), vec![Ended::Fresh, Ended::Close]);
        let known = Recorded {
            id: "01a0bdb6".into(),
            at: "2026-09-20T07:27:14.769Z".into(),
            last: "$rf:ask research crypto trading".into(),
        };
        assert_eq!(
            ended_choices(Some(&known)),
            vec![Ended::Resume, Ended::Fresh, Ended::Close],
            "picking up where you were comes first"
        );
    }

    #[test]
    fn the_session_offered_is_the_one_the_hook_wrote_down() {
        // `starts()` only offers a harness that is on this machine, so this
        // is about what Weft does when one is. A runner has no Codex.
        if !codex_on_path() {
            eprintln!("skipped: codex is not on PATH");
            return;
        }
        let mut a = app();
        record_a_session(&a.root.clone(), "codex", "01a0bdb6-1d1f-79c2-84b0-8b03496d7db0");
        a.input(0, &[4]).expect("end it");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            a.pump();
            a.notice_an_agent_that_ended();
            if a.modal.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let Some(Modal::Ended { session: Some(s), .. }) = a.modal.clone() else {
            panic!("a recorded session is offered: {:?}", a.modal)
        };
        assert_eq!(s.id, "01a0bdb6-1d1f-79c2-84b0-8b03496d7db0");
        assert_eq!(
            harness::find("codex").expect("codex").resume_spec(&s.id),
            "codex resume 01a0bdb6-1d1f-79c2-84b0-8b03496d7db0"
        );
    }

    #[test]
    fn closing_an_ended_pane_takes_it_away() {
        let mut a = app();
        assert_eq!(a.pane_count(), 1);
        a.modal = Some(Modal::Ended { pane: 0, harness: "codex".into(), session: None });
        a.modal_choice =
            ended_choices(None).iter().position(|c| *c == Ended::Close).expect("close");
        press(&mut a, KeyCode::Enter);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.pane_count() > 0 {
            a.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(a.pane_count(), 0, "the pane is gone, not left dead on screen");
        assert!(a.modal.is_none());
    }

    #[test]
    fn backing_out_of_the_ended_panel_closes_the_pane_too() {
        // There is nothing to go back to: the agent has already gone.
        let mut a = app();
        a.modal = Some(Modal::Ended { pane: 0, harness: "codex".into(), session: None });
        press(&mut a, KeyCode::Left);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.pane_count() > 0 {
            a.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(a.pane_count(), 0);
    }

    #[test]
    fn a_row_of_work_leads_back_to_the_agent_that_has_it() {
        // The complaint this answers: reopening Weft showed rows of previous
        // work with no next action on any of them.
        let a = with_unit(Sent::TakenByAgent);
        assert_eq!(a.unavailable(Act::OpenAgent), None, "the record names the session");
    }

    #[test]
    fn a_unit_whose_record_names_no_session_offers_nothing_to_open() {
        let mut a = app();
        let mut u = unit(Sent::TakenByAgent);
        u.session_ref = None;
        a.set_units(vec![u]);
        let said = a.unavailable(Act::OpenAgent).expect("a reason");
        assert!(said.contains("does not name"), "{said}");
        assert!(said.contains("codex"), "and names the harness: {said}");
    }

    #[test]
    fn a_session_already_open_is_pointed_at_rather_than_opened_twice() {
        // Two processes on one session would be two writers on one record.
        let mut a = app();
        let mut u = unit(Sent::TakenByAgent);
        u.session_ref = Some("abc123".into());
        a.set_units(vec![u]);
        assert_eq!(a.unavailable(Act::OpenAgent), None, "nothing is running it yet");

        // The spec the pane runs is what says so, so it holds for a pane
        // another client opened.
        a.session.panes[0].spec = "codex resume abc123".into();
        let said = a.unavailable(Act::OpenAgent).expect("a reason");
        assert!(said.contains("already open"), "{said}");
        assert!(said.contains("[1]"), "and says which pane: {said}");
    }

    #[test]
    fn nothing_asked_for_yet_means_nothing_to_open() {
        let mut a = app();
        a.set_units(Vec::new());
        assert!(a.unavailable(Act::OpenAgent).is_some());
    }

    #[test]
    fn the_picker_offers_a_recorded_session_before_a_fresh_agent() {
        // `starts()` only offers a harness that is on this machine, so this
        // is about what Weft does when one is. A runner has no Codex.
        if !codex_on_path() {
            eprintln!("skipped: codex is not on PATH");
            return;
        }
        let mut a = app();
        let root = a.root.clone();
        let dir = root.join(".fab7/rf/sessions/codex/01a0bdb6");
        std::fs::create_dir_all(&dir).expect("dir");
        let line = serde_json::json!({
            "session_id": "01a0bdb6", "time": "2026-09-20T07:27:14.769Z",
            "prompt": "$rf:ask research crypto trading", "cwd": root.to_string_lossy(),
        });
        std::fs::write(dir.join("prompts.jsonl"), format!("{line}\n")).expect("receipt");
        a.refresh_for_test();

        let starts = a.starts();
        let codex: Vec<_> = starts.iter().filter(|c| c.harness == "codex").collect();
        assert_eq!(codex.len(), 2, "pick it up, or start fresh");
        assert_eq!(codex[0].spec, "codex resume 01a0bdb6", "picking it up comes first");
        assert!(codex[0].session.is_some());
        assert_eq!(codex[1].spec, "codex");
        assert!(codex[1].session.is_none());
    }

    #[test]
    fn a_harness_with_no_receipt_here_is_offered_fresh_only() {
        let mut a = app();
        a.refresh_for_test();
        for c in a.starts() {
            assert!(c.session.is_none(), "nothing is on record in a new workspace");
            assert!(!c.spec.contains("resume"), "and nothing is invented: {}", c.spec);
        }
    }

    #[test]
    fn send_is_offered_for_an_ask_nobody_answered() {
        // The defect this closes: a chooser that timed out left a row on the
        // board with nothing anyone could do about it.
        let mut a = app();
        let mut u = unit(Sent::NotSent);
        u.confirmed = false;
        u.unanswered = true;
        a.set_units(vec![u]);
        assert_eq!(a.unavailable(Act::Send), None, "[S]END is the person's yes");
    }

    /// A project that routes its acts. Weft reads the file once, so this sets
    /// the routing directly rather than going through a temporary HOME.
    fn routed(pairs: &[(&str, &str)]) -> App {
        let mut a = app();
        let acts: serde_json::Map<String, serde_json::Value> =
            pairs.iter().map(|(k, v)| ((*k).to_string(), serde_json::json!(v))).collect();
        let text = serde_json::json!({ a.root.to_string_lossy().into_owned(): acts }).to_string();
        a.routing = weft_core::routing::read(&text, &a.root);
        a.set_units(vec![unit(Sent::TakenByAgent)]);
        a
    }

    #[test]
    fn an_act_goes_to_the_harness_this_project_routes_it_to() {
        // Codex asks, Claude evaluates. The work was done in codex.
        let a = routed(&[("eval", "claude-code")]);
        assert_eq!(a.deciding_harness(Act::Check).as_deref(), Some("claude-code"));
        // Seal was not routed, so it still follows the work.
        assert_eq!(a.deciding_harness(Act::Decide).as_deref(), Some("codex"));
        // And Send is never routed: it is the delivery of an Ask already made.
        assert_eq!(weft_core::board::Board::act_key(Act::Send), None);
    }

    #[test]
    fn readiness_is_asked_of_the_harness_the_act_will_go_to() {
        // The point of routing: Eval going to Claude Code needs Claude Code
        // set up, whatever the work was done in.
        let mut a = routed(&[("eval", "claude-code")]);
        a.set_readiness("codex", Readiness::Ready);
        a.set_readiness("claude-code", Readiness::Missing(Gap::Plugin));
        let said = a.unavailable(Act::Check).expect("a reason");
        assert!(said.contains("claude-code"), "names the harness it would go to: {said}");
        // And the harness that did the work being ready does not help.
        assert_eq!(a.unavailable(Act::Decide), None, "seal still follows the work");
    }

    #[test]
    fn a_routed_act_with_no_pane_says_which_harness_is_missing() {
        // Readiness is answered first — that ordering is deliberate — so make
        // the routed harness ready and leave it without a pane.
        let mut a = routed(&[("eval", "claude-code")]);
        a.set_readiness("claude-code", Readiness::Ready);
        let said = a.unavailable(Act::Check).expect("a reason");
        assert!(said.contains("claude-code"), "{said}");
        assert!(said.contains("nowhere to send"), "{said}");
    }

    #[test]
    fn a_project_that_routes_nothing_behaves_exactly_as_before() {
        let a = routed(&[]);
        assert!(a.routing().is_empty());
        for act in [Act::Ask, Act::Check, Act::Decide] {
            assert_eq!(a.deciding_harness(act).as_deref(), Some("codex"), "{act:?}");
        }
    }

    #[test]
    fn asking_opens_on_the_harness_the_project_asks_in() {
        let mut a = routed(&[("ask", "codex")]);
        a.act(Act::Ask);
        let Some(Modal::Ask { target, .. }) = a.modal.clone() else { panic!("{:?}", a.modal) };
        assert_eq!(a.harness_at(target), Some("codex"));
    }

    #[test]
    fn weft_starts_in_weft_not_in_the_agent() {
        assert_eq!(app().focus, Focus::Weft);
    }

    #[test]
    fn the_toggle_moves_between_weft_and_the_agent_both_ways() {
        let mut a = app();
        ctrl(&mut a, ']');
        assert_eq!(a.focus, Focus::Agent);
        ctrl(&mut a, ']');
        assert_eq!(a.focus, Focus::Weft);
    }

    #[test]
    fn coming_back_out_of_the_agent_shows_the_work_list_again() {
        let mut a = app();
        press(&mut a, KeyCode::Char('w'));
        assert!(!a.show_work());
        ctrl(&mut a, ']');
        ctrl(&mut a, ']');
        assert!(a.show_work(), "Ctrl+] back from the agent lands on the list");
    }

    #[test]
    fn in_the_agent_x_is_typed_rather_than_quitting_weft() {
        let mut a = app();
        a.focus = Focus::Agent;
        press(&mut a, KeyCode::Char('x'));
        assert!(a.modal.is_none());
        assert!(!a.quit);
    }

    #[test]
    fn quitting_asks_first_and_leaves_the_agents_running_by_default() {
        let mut a = app();
        press(&mut a, KeyCode::Char('x'));
        assert!(matches!(a.modal, Some(Modal::Quit)));
        assert!(!a.quit, "quitting is never immediate");
        press(&mut a, KeyCode::Enter);
        assert!(a.quit);
        assert!(!a.stop_agents_on_quit);
    }

    #[test]
    fn asking_opens_a_box_and_types_nothing_yet() {
        let mut a = app();
        press(&mut a, KeyCode::Char('a'));
        assert!(matches!(a.modal, Some(Modal::Ask { .. })));
    }

    #[test]
    fn an_empty_intent_is_not_sent() {
        let mut a = app();
        press(&mut a, KeyCode::Char('a'));
        press(&mut a, KeyCode::Enter);
        assert!(matches!(a.modal, Some(Modal::Ask { .. })), "still composing");
    }

    #[test]
    fn a_typed_intent_becomes_a_confirmation_not_an_injection() {
        let mut a = app();
        press(&mut a, KeyCode::Char('a'));
        for c in "fix the build".chars() {
            press(&mut a, KeyCode::Char(c));
        }
        press(&mut a, KeyCode::Enter);
        let Some(Modal::Confirm(p)) = a.modal.clone() else {
            panic!("expected a confirmation, got {:?}", a.modal)
        };
        let text = String::from_utf8(p.payload).unwrap();
        assert!(text.ends_with("ask fix the build"), "got {text:?}");
        assert!(text.contains("rf:"), "the profile's prefix is used: {text:?}");
    }

    #[test]
    fn backspace_edits_the_intent() {
        let mut a = app();
        press(&mut a, KeyCode::Char('a'));
        for c in "abc".chars() {
            press(&mut a, KeyCode::Char(c));
        }
        press(&mut a, KeyCode::Backspace);
        let Some(Modal::Ask { text, .. }) = a.modal.clone() else { panic!() };
        assert_eq!(text, "ab");
    }

    #[test]
    fn check_and_decide_always_confirm_before_typing() {
        for (key, expect) in [('c', "eval"), ('d', "seal")] {
            let mut a = recorded(Sent::TakenByAgent);
            press(&mut a, KeyCode::Char(key));
            let Some(Modal::Confirm(p)) = a.modal.clone() else {
                panic!("{key} must confirm first, got {:?}", a.modal)
            };
            let text = String::from_utf8(p.payload).unwrap();
            assert!(text.trim_end().ends_with(expect), "got {text:?}");
            assert!(text.ends_with(' '), "the token is closed so Enter means send: {text:?}");
        }
    }

    #[test]
    fn cancelling_a_confirmation_types_nothing() {
        let mut a = recorded(Sent::TakenByAgent);
        press(&mut a, KeyCode::Char('c'));
        assert!(matches!(a.modal, Some(Modal::Confirm(_))));
        press(&mut a, KeyCode::Left); // [←] CANCEL
        assert!(a.modal.is_none());
        assert_eq!(a.focus, Focus::Weft, "cancelling never moves you into the agent");
    }

    #[test]
    fn confirming_types_it_and_puts_you_in_the_agent() {
        let mut a = recorded(Sent::TakenByAgent);
        press(&mut a, KeyCode::Char('c'));
        press(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none());
        assert_eq!(a.focus, Focus::Agent, "you land where the work is happening");
    }

    #[test]
    fn an_unavailable_action_explains_itself_in_the_hint_and_opens_nothing() {
        let mut a = app();
        press(&mut a, KeyCode::Char('c'));
        assert!(a.modal.is_none(), "never a dialog: {:?}", a.modal);
        assert_eq!(a.hint_text(), Some("Nothing to work on yet."));
    }

    #[test]
    fn send_names_the_row_that_is_ready_instead_of_failing_silently() {
        let mut a = app();
        let mut ready = unit(Sent::ReadyToSend);
        ready.ask_id = "ask_2".into();
        ready.title = "readme fix".into();
        a.set_units(vec![unit(Sent::TakenByAgent), ready]);
        press(&mut a, KeyCode::Char('s'));
        assert!(a.modal.is_none());
        assert_eq!(a.hint_text(), Some("readme fix is the one ready to send. ↓ to select it."));
    }

    #[test]
    fn a_unit_whose_agent_is_not_open_is_not_silently_dropped() {
        let mut a = app();
        let mut u = unit(Sent::Arrived { exact: true });
        u.harness = "claude-code".into();
        a.set_units(vec![u]);
        press(&mut a, KeyCode::Char('c'));
        let hint = a.hint_text().expect("a sentence");
        assert!(hint.contains("claude-code"), "names the missing agent: {hint}");
    }

    #[test]
    fn enter_expands_the_row_in_place_and_enter_again_collapses_it() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.expanded(), Some(0));
        assert!(a.modal.is_none(), "v2 expands in place; no overlay");
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.expanded(), None);
    }

    #[test]
    fn back_collapses_the_row_rather_than_quitting_anything() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        press(&mut a, KeyCode::Enter);
        press(&mut a, KeyCode::Left);
        assert_eq!(a.expanded(), None);
        assert!(!a.quit);
    }

    #[test]
    fn moving_off_an_expanded_row_collapses_it() {
        let mut a = app();
        let mut second = unit(Sent::NotSent);
        second.ask_id = "ask_2".into();
        a.set_units(vec![unit(Sent::NotSent), second]);
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.expanded(), Some(0));
        press(&mut a, KeyCode::Down);
        assert_eq!(a.expanded(), None);
    }

    #[test]
    fn arrows_move_between_units() {
        let mut a = app();
        let mut second = unit(Sent::NotSent);
        second.ask_id = "ask_2".into();
        second.title = "second".into();
        a.set_units(vec![unit(Sent::NotSent), second]);
        press(&mut a, KeyCode::Down);
        assert_eq!(a.selected, 1);
        press(&mut a, KeyCode::Up);
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn readiness_follows_the_harness_that_owns_the_work() {
        // Claude Code ready, Codex not: the same action is available on one
        // and not the other, because the plugin is installed per harness.
        let mut a = app();
        a.add("claude-code", "/bin/cat").expect("a second pane");
        a.set_readiness("codex", Readiness::Missing(Gap::Plugin));
        a.set_readiness("claude-code", Readiness::Ready);
        let mut codex_row = unit(Sent::Arrived { exact: true });
        codex_row.harness = "codex".into();
        let mut claude_row = unit(Sent::Arrived { exact: true });
        claude_row.ask_id = "ask_2".into();
        claude_row.harness = "claude-code".into();
        a.set_units(vec![codex_row, claude_row]);

        let blocked = a.unavailable(Act::Check).expect("codex is not set up");
        assert!(blocked.contains("codex is not set up"), "{blocked}");
        press(&mut a, KeyCode::Down);
        assert_eq!(a.unavailable(Act::Check), None, "claude-code is ready");
    }

    #[test]
    fn nothing_is_installed_without_an_explicit_yes() {
        let mut a = app();
        a.set_readiness("codex", Readiness::Missing(Gap::Plugin));
        press(&mut a, KeyCode::Char('r'));
        let Some(Modal::SetUp { harness, commands, .. }) = a.modal.clone() else {
            panic!("[R] must propose first, got {:?}", a.modal)
        };
        assert_eq!(harness, "codex");
        assert_eq!(
            commands,
            vec![
                "codex plugin marketplace add fab7hq/fab7".to_string(),
                "codex plugin add rf@fab7".to_string()
            ],
            "what is shown is what would run"
        );
        // Declining runs nothing, and is remembered.
        press(&mut a, KeyCode::Left);
        assert!(a.modal.is_none());
        assert_eq!(a.readiness("codex"), Readiness::Declined);
    }

    #[test]
    fn a_harness_that_said_no_is_not_asked_again_this_session() {
        let mut a = app();
        a.set_readiness("codex", Readiness::Declined);
        a.add("codex", "/bin/cat").expect("spawn");
        assert_eq!(a.readiness("codex"), Readiness::Declined, "the check does not overwrite a no");
    }

    #[test]
    fn weft_does_not_offer_to_install_the_cli_itself() {
        // The panel opens, because it carries the one command that fixes it —
        // and it offers nothing to run, because a tool on the machine is
        // RingFrame's to install, not Weft's.
        let mut a = app();
        a.set_readiness("codex", Readiness::Missing(Gap::Cli));
        press(&mut a, KeyCode::Char('r'));
        let Some(Modal::SetUp { gap, .. }) = a.modal.clone() else {
            panic!("expected the panel, got {:?}", a.modal)
        };
        assert_eq!(gap, Gap::Cli);
    }

    #[test]
    fn a_harness_weft_cannot_ask_is_unknown_rather_than_missing() {
        let mut a = app();
        a.set_readiness("codex", Readiness::Unknown);
        let why = a.unavailable(Act::Ask).expect("not ready");
        assert!(why.contains("could not ask"), "{why}");
        assert!(!why.contains("[R]EADY UP"), "nothing to offer: {why}");
    }

    #[test]
    fn an_ask_this_workspace_could_not_finish_is_refused_before_it_is_typed() {
        // RingFrame refuses at compile, which is after the skill has
        // classified, composed and staged. Weft asks first, so nothing is
        // typed and no turn is spent.
        let mut a = app();
        a.set_workspace_gap(Some("This is not a Git repository.".into()));
        press(&mut a, KeyCode::Char('a'));
        assert!(a.modal.is_none(), "nothing was opened: {:?}", a.modal);
        assert_eq!(a.hint_text(), Some("This is not a Git repository."));
    }

    #[test]
    fn a_refusal_is_shown_as_its_first_sentence() {
        assert_eq!(
            first_line(
                "/tmp/x is not in a Git repository; RingFrame evaluates it. Run `git init`."
            ),
            "/tmp/x is not in a Git repository; RingFrame evaluates it."
        );
        assert_eq!(first_line("one line only"), "one line only");
    }

    #[test]
    fn w_hides_the_work_list_and_brings_it_back() {
        let mut a = app();
        assert!(a.show_work());
        press(&mut a, KeyCode::Char('w'));
        assert!(!a.show_work());
        press(&mut a, KeyCode::Char('w'));
        assert!(a.show_work());
    }

    #[test]
    fn space_selects_the_next_row_that_needs_you() {
        let mut a = app();
        let mut ready = unit(Sent::ReadyToSend);
        ready.ask_id = "ask_2".into();
        a.set_units(vec![unit(Sent::TakenByAgent), ready]);
        press(&mut a, KeyCode::Char(' '));
        assert_eq!(a.selected, 1);
    }

    #[test]
    fn space_says_so_plainly_when_nothing_needs_you() {
        let mut a = app();
        a.set_units(vec![unit(Sent::TakenByAgent)]);
        press(&mut a, KeyCode::Char(' '));
        assert_eq!(a.hint_text(), Some("Nothing needs you."));
    }

    #[test]
    fn the_needs_you_count_is_derived_from_the_record() {
        let mut a = app();
        a.set_units(vec![unit(Sent::ReadyToSend), unit(Sent::Arrived { exact: true })]);
        assert_eq!(a.needs_you(), 1);
    }

    #[test]
    fn explaining_a_waiting_pane_quotes_the_line_it_matched() {
        let mut a = app();
        a.input(0, b"Allow command?\r\n").expect("type");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            a.pump();
            if a.waiting(0).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        press(&mut a, KeyCode::Char('e'));
        let hint = a.hint_text().expect("a sentence");
        assert!(hint.contains("Allow command?"), "quotes the evidence: {hint}");
        assert!(hint.contains("permission"), "names the rule: {hint}");
    }

    #[test]
    fn weft_never_answers_a_waiting_agent_itself() {
        // [Enter] ANSWER IT puts the person in the pane; it types nothing.
        let mut a = app();
        a.input(0, b"Allow command?\r\n").expect("type");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            a.pump();
            if a.waiting(0).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        press(&mut a, KeyCode::Char(' '));
        assert!(!a.show_work(), "Space shows the pane that is waiting");
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.focus, Focus::Agent);
        assert!(a.modal.is_none());
    }

    #[test]
    fn a_click_inside_the_pane_focuses_the_agent_and_a_row_click_comes_back() {
        let mut a = with_unit(Sent::NotSent);
        a.note_pane_area(ratatui::layout::Rect { x: 40, y: 2, width: 40, height: 20 });
        a.note_rows(vec![(3, 2, 0)]);
        let click = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(click(50, 10)).expect("mouse");
        assert_eq!(a.focus, Focus::Agent);
        a.on_mouse(click(5, 3)).expect("mouse");
        assert_eq!(a.focus, Focus::Weft);
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn clicking_the_left_pane_comes_out_of_the_agent_even_with_nothing_on_it() {
        // Found by hand: the click only came out of the agent when it landed
        // on a row, so on an empty board — which is every first run — the left
        // pane could not be clicked back to at all.
        let mut a = app();
        a.note_pane_area(ratatui::layout::Rect { x: 40, y: 2, width: 40, height: 20 });
        a.note_list_area(Some(ratatui::layout::Rect { x: 0, y: 2, width: 39, height: 20 }));
        a.note_rows(Vec::new());
        let click = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(click(50, 10)).expect("mouse");
        assert_eq!(a.focus, Focus::Agent);
        a.on_mouse(click(10, 15)).expect("mouse");
        assert_eq!(a.focus, Focus::Weft, "an empty list is still the left pane");
    }

    #[test]
    fn the_wheel_says_whose_history_it_is_when_weft_kept_none() {
        // Codex repaints its viewport rather than scrolling, so nothing ever
        // reaches Weft's scrollback and the wheel moved nothing, silently.
        let mut a = app();
        a.note_pane_area(ratatui::layout::Rect { x: 40, y: 2, width: 40, height: 20 });
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 50,
            row: 10,
            modifiers: KeyModifiers::NONE,
        })
        .expect("wheel");
        let said = a.hint_text().expect("a sentence rather than nothing at all");
        assert!(said.contains("Ctrl+T"), "names the key that does reach it: {said}");
        assert!(said.contains("codex"), "and names the harness: {said}");
    }

    #[test]
    fn clicking_the_pane_goes_in_and_clicking_again_comes_out() {
        let mut a = with_unit(Sent::NotSent);
        a.note_pane_area(ratatui::layout::Rect { x: 40, y: 2, width: 40, height: 20 });
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 50,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(click).expect("mouse");
        assert_eq!(a.focus, Focus::Agent);
        a.on_mouse(click).expect("mouse");
        assert_eq!(a.focus, Focus::Weft, "the same click does not mean two things");
    }

    #[test]
    fn clicking_a_row_twice_expands_it() {
        let mut a = app();
        let mut second = unit(Sent::NotSent);
        second.ask_id = "ask_2".into();
        a.set_units(vec![unit(Sent::NotSent), second]);
        a.note_rows(vec![(3, 2, 0), (5, 2, 1)]);
        let click = |row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(click(5)).expect("mouse");
        assert_eq!(a.selected, 1);
        assert_eq!(a.expanded(), None, "the first click only selects");
        a.on_mouse(click(5)).expect("mouse");
        assert_eq!(a.expanded(), Some(1));
    }

    #[test]
    fn clicking_a_tab_switches_agent_and_clicking_plus_offers_a_new_one() {
        let mut a = app();
        a.note_tabs(vec![(10, 8, Some(0)), (20, 12, Some(1)), (34, 1, None)]);
        let click = |col| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(click(11)).expect("mouse");
        assert_eq!(a.pane_focus, 0);
        a.on_mouse(click(34)).expect("mouse");
        match a.modal {
            Some(Modal::StartAgent { .. }) | None => {}
            ref other => panic!("+ offers a new agent, got {other:?}"),
        }
    }

    #[test]
    fn the_board_is_read_from_a_real_ledger_on_disk() {
        let dir = std::env::temp_dir().join(format!("weft-app-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".fab7/rf")).unwrap();
        let event = json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
            "time": "2026-09-19T14:02:00Z", "id": "ask_1",
            "actor": {"kind": "human", "id": "me"}, "links": [],
            "data": {
                "title": "health endpoint", "selected_capability": "native_plan",
                "delivery_mode": "human_handoff", "host": {"name": "codex"},
                "source": {}, "prompt": {}, "source_verified": "exact",
                "limitations": [], "classification": {}, "route_explanation": {}
            }
        });
        std::fs::write(
            dir.join(".fab7/rf/ledger.jsonl"),
            format!("{}\n", serde_json::to_string(&event).unwrap()),
        )
        .unwrap();

        let socket = weft_proto::private_socket("weft-app");
        let _ = std::fs::remove_file(&socket);
        let listening = socket.clone();
        std::thread::spawn(move || {
            let _ = weftd::server::Session::serve(&listening);
        });
        let session = crate::client::Session::connect(&socket, &dir, 24, 80).expect("connect");
        let mut a = App::with_session(dir.clone(), Toggle, session);
        a.refresh_for_test();
        assert_eq!(a.units().len(), 1);
        assert_eq!(a.units()[0].title, "health endpoint");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod start_tests {
    use super::*;
    use crate::ledger::Sent;
    use crossterm::event::KeyCode;

    fn bare() -> App {
        let (root, session) = super::tests::test_session("bare");
        App::with_session(root, Toggle, session)
    }

    #[test]
    fn weft_starts_no_agent_on_its_own() {
        let a = bare();
        assert_eq!(a.pane_count(), 0, "the panel stays empty until asked");
        assert_eq!(a.focus, Focus::Weft);
    }

    #[test]
    fn n_offers_the_agents_that_are_installed() {
        let mut a = bare();
        a.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)).expect("key");
        match a.modal {
            Some(Modal::StartAgent { .. }) => {}
            None => assert!(
                a.hint_text().is_some_and(|h| h.contains("No coding agent")),
                "either a picker or a plain sentence: {:?}",
                a.hint_text()
            ),
            ref other => panic!("expected a picker, got {other:?}"),
        }
    }

    #[test]
    fn s_sends_rather_than_doing_nothing() {
        // The action bar advertised [S]end it while nothing was wired to it.
        let mut a = bare();
        a.add("codex", "/bin/cat").expect("spawn");
        a.set_units(vec![Unit {
            ask_id: "ask_1".into(),
            title: "t".into(),
            harness: "codex".into(),
            session_ref: Some("01a0bdb6-1d1f-79c2-84b0-8b03496d7db0".into()),
            delivery: Default::default(),
            route: "native_plan".into(),
            asked_at: "now".into(),
            delivery_mode: "human_handoff".into(),
            cancelled: false,
            unanswered: false,
            confirmed: true,
            sent: Sent::ReadyToSend,
            check: None,
            sealed: None,
            seal_id: None,
            sealed_at: None,
        }]);
        a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)).expect("key");
        assert!(
            a.modal.is_some() || a.hint_text().is_some(),
            "S must do something when the bar offers it"
        );
    }

    #[test]
    fn scrolling_over_a_pane_moves_its_scrollback() {
        let mut a = bare();
        a.add("sh", "/bin/sh").expect("spawn");
        a.input(0, b"for i in $(seq 1 60); do echo line-$i; done\n").expect("send");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            a.pump();
            if a.pane_text(0).is_some_and(|t| t.contains("line-60")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        a.note_pane_area(ratatui::layout::Rect { x: 0, y: 2, width: 80, height: 20 });

        let scroll = |kind| MouseEvent { kind, column: 40, row: 10, modifiers: KeyModifiers::NONE };
        a.on_mouse(scroll(MouseEventKind::ScrollUp)).expect("scroll");
        a.on_mouse(scroll(MouseEventKind::ScrollUp)).expect("scroll");
        assert!(
            a.pane_scroll_offset(0).is_some_and(|o| o > 0),
            "the wheel moves through the scrollback"
        );

        a.on_mouse(scroll(MouseEventKind::ScrollDown)).expect("scroll");
        a.on_mouse(scroll(MouseEventKind::ScrollDown)).expect("scroll");
        assert_eq!(a.pane_scroll_offset(0), Some(0), "and back to the live output");
    }
}

#[cfg(test)]
mod first_run_tests {
    use super::tests::{press, test_session};
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn the_first_run_picker_moves_and_starts() {
        let (root, session) = test_session("first-run-keys");
        let mut a = App::with_session(root, Toggle, session);
        a.refresh_for_test();
        assert_eq!(a.pane_count(), 0, "first run: the picker is the whole screen");
        let choices = a.starts().len();
        if choices < 2 {
            eprintln!("skipped: needs two agents installed");
            return;
        }
        press(&mut a, KeyCode::Down);
        assert_eq!(a.modal_choice, 1, "the arrows move the picker");
        press(&mut a, KeyCode::Up);
        assert_eq!(a.modal_choice, 0);
        press(&mut a, KeyCode::Up);
        assert_eq!(a.modal_choice, choices - 1, "and it wraps, like the work list");
    }

    #[test]
    fn the_picker_behaves_the_same_wherever_it_is_drawn() {
        // On first run it is the body; pressing N over a running agent draws
        // it as a modal. A person should not have to know the difference.
        let mut a = super::tests::app();
        let choices = a.starts().len();
        if choices < 2 {
            eprintln!("skipped: needs two agents installed");
            return;
        }
        press(&mut a, KeyCode::Char('n'));
        assert!(matches!(a.modal, Some(Modal::StartAgent { .. })));
        press(&mut a, KeyCode::Down);
        assert_eq!(a.modal_choice, 1);
        press(&mut a, KeyCode::Up);
        press(&mut a, KeyCode::Up);
        assert_eq!(a.modal_choice, choices - 1, "the same wrap");
    }

    #[test]
    fn clicking_a_choice_on_first_run_picks_it() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let (root, session) = test_session("first-run-click");
        let mut a = App::with_session(root, Toggle, session);
        a.refresh_for_test();
        if a.starts().len() < 2 {
            eprintln!("skipped: needs two agents installed");
            return;
        }
        a.note_rows(vec![(10, 1, 0), (11, 1, 1)]);
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 20,
            row: 11,
            modifiers: KeyModifiers::NONE,
        })
        .expect("click");
        assert_eq!(a.modal_choice, 1, "the screen says to click anything");
    }
}

/// Readiness as the daemon words it.
fn readiness_of(word: Option<&str>) -> Readiness {
    match word {
        Some("ready") => Readiness::Ready,
        Some("no_cli") => Readiness::Missing(Gap::Cli),
        Some("no_marketplace") => Readiness::Missing(Gap::Marketplace),
        Some("no_plugin") => Readiness::Missing(Gap::Plugin),
        Some("declined") => Readiness::Declined,
        _ => Readiness::Unknown,
    }
}

/// A refusal from the daemon, as one sentence for the person.
fn said(e: &anyhow::Error) -> String {
    let text = e.to_string();
    text.split_once(": ").map(|(_, rest)| rest.to_string()).unwrap_or(text)
}
