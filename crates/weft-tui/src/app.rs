//! The running application: state, events, and the loop.
//!
//! Two nouns, two surfaces: agents are tabs over the pane, work is a sidebar
//! tree of project, harness, and action. Opening an action shows its whole
//! story in a blocking detail view.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEvent, KeyEventKind, KeyModifiers,
    MouseEvent, MouseEventKind,
};
use ratatui::DefaultTerminal;

use crate::blocked::{self, Evidence};
use crate::client::Session;
use crate::encode;
use crate::keys::{self, Action, Chord, Focus, Key, Toggle};
use crate::ledger::Unit;
use crate::theme::Theme;
pub use weft_core::board::Act;
use weft_core::board::{Board, Next, PaneInfo};
use weft_core::harness;
pub use weft_core::offers::{PickUp, pick_up_choices, say_handoff};
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
/// One project open in this window: its name, its root, and the client
/// connection that watches it. The daemon gives a connection one project to
/// watch, so a window with two projects holds two connections.
pub struct Open {
    pub name: String,
    pub root: PathBuf,
    pub session: Session,
}

impl Open {
    fn new(root: PathBuf, session: Session) -> Self {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into());
        Open { name, root, session }
    }
}

/// The focused project's connection. A macro rather than a method so that it
/// expands to a field access and borrows like one.
macro_rules! sess {
    ($self:expr) => {
        $self.projects[$self.at].session
    };
}

/// `~` is what people type for their home directory, and a path box that
/// does not know it is a path box people stop using.
fn shellexpand(typed: &str) -> String {
    let typed = typed.trim();
    match typed.strip_prefix('~') {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => format!("{}{rest}", home.to_string_lossy()),
            None => typed.to_string(),
        },
        None => typed.to_string(),
    }
}

/// A row of the sidebar. Project over harness over action, because a unit
/// belongs to the harness it was asked of.
///
/// The rows are computed from the units and the fold state each frame rather
/// than stored, so there is one source of truth and nothing to keep in step.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Project { project: usize, name: String, folded: bool, open: usize, waiting: usize },
    Harness { project: usize, name: String, folded: bool, waiting: usize, running: bool },
    Action { project: usize, unit: usize },
}

impl Row {
    /// Which project this row belongs to. Moving the selection moves the
    /// focus with it, so an act always lands in the project you are looking
    /// at and never in the one you were.
    pub fn project(&self) -> usize {
        match self {
            Row::Project { project, .. }
            | Row::Harness { project, .. }
            | Row::Action { project, .. } => *project,
        }
    }
}

impl Row {
    /// The key this row folds under, or `None` for a leaf.
    pub fn fold_key(&self) -> Option<String> {
        match self {
            Row::Project { name, .. } => Some(name.clone()),
            Row::Harness { project, name, .. } => Some(format!("{project}\u{0}{name}")),
            Row::Action { .. } => None,
        }
    }

    pub fn folded(&self) -> bool {
        match self {
            Row::Project { folded, .. } | Row::Harness { folded, .. } => *folded,
            Row::Action { .. } => false,
        }
    }
}

/// Weft's own operations, in the order the menu lists them.
pub const WEFT_MENU: [(char, &str, Act); 5] = [
    ('O', "Open project", Act::OpenProject),
    ('N', "New agent", Act::NewAgent),
    ('B', "Toggle Sidebar", Act::ToggleSidebar),
    ('H', "Help", Act::Help),
    ('X', "Quit", Act::Quit),
];

/// The work a follow-up is about: the unit it was asked from, and the one id
/// it carries (that unit's Eval, else its Ask).
#[derive(Debug, Clone, PartialEq)]
pub struct Follows {
    pub ask_id: String,
    pub title: String,
    pub carries: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    Quit,
    Help,
    /// The agent looks like it is waiting for its person, and the person has
    /// been shown what Weft read. Typing is theirs to allow.
    SendAnyway {
        pending: Pending,
        evidence: String,
    },
    /// Take a project out of this window. Nothing is stopped and nothing is
    /// deleted, which the panel says, because "close" is the word people
    /// expect to end an agent.
    CloseProject {
        name: String,
    },
    /// Which directory to open. Typed, because the daemon's own list of
    /// projects is not on the wire and a path always works.
    OpenProject {
        text: String,
    },
    /// Weft's own operations, gathered out of the bar. Every one of them also
    /// has its own key; this is for finding them, not for reaching them.
    Weft,
    /// Composing an intent; a follow-up also says what work it is about.
    Ask {
        text: String,
        target: usize,
        follows: Option<Follows>,
    },
    /// Weft is about to type something. Always shown first.
    Confirm(Pending),
    /// Something did not work, said plainly.
    Note(String),
    /// Pick an agent to start. Weft starts none on its own.
    StartAgent {
        choice: usize,
    },
    /// What the configuration and each harness need to reach RingFrame's
    /// latest release. Nothing runs until `[P]ROCEED`.
    RingFrame,
    /// Work whose agent is not running: resume the session it was asked in,
    /// or start fresh, then carry on with `then`.
    PickUp {
        harness: String,
        session: Option<Recorded>,
        then: Option<Act>,
    },
}

/// The two reading surfaces. Siblings, not a stack: `P` from the judges
/// drawer swaps the content rather than piling a second overlay on top.
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub title: String,
    pub harness: String,
    /// The whole story, already assembled. Folded and scrolled at draw time,
    /// because only the render knows how wide the screen is.
    pub lines: Vec<String>,
    pub offset: usize,
    /// What `[P]ROCEED` would do. `None` draws it dim.
    pub next: Option<Next>,
}

pub struct App {
    /// A newer Weft release, once someone has looked. Set from outside,
    /// because the client itself reaches nothing.
    pub newer: std::sync::Arc<std::sync::OnceLock<String>>,
    /// The projects open in this window, and which one has the focus. The
    /// daemon has always been able to hold many; this is the client catching
    /// up.
    projects: Vec<Open>,
    at: usize,
    pub pane_focus: usize,
    units: Vec<Unit>,
    /// An index into [`App::rows`], not into the units: the sidebar is a tree
    /// and a project or a harness can be selected too.
    pub selected: usize,
    /// The levels the person has folded, by [`Row::fold_key`]. Absent means
    /// open, so a project that appears while you are working is not hidden.
    folded: std::collections::BTreeSet<String>,
    /// How far the work list is scrolled. A list can be longer than the pane.
    list_offset: usize,
    detail: Option<Detail>,
    /// The furthest the drawer may be scrolled, as the last render measured it.
    detail_reach: usize,
    /// How many rows the sidebar had room for, as the last render measured
    /// it. Only the render knows, and moving the selection has to keep it in
    /// view now that there is no wheel.
    list_rows: usize,
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
    /// Whether the CLI that owns the record is installed. Asked once: it is a
    /// property of the machine, not of the frame being drawn.
    record_available: bool,
    /// Why this workspace could not finish an Ask, if it could not. Asked once
    /// for the same reason, and re-asked after an `[U]PDATE`.
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
        sess!(self).panes.len()
    }

    pub fn harness_at(&self, i: usize) -> Option<&str> {
        sess!(self).panes.get(i).map(|p| p.harness.as_str())
    }

    pub fn units(&self) -> &[Unit] {
        &self.units
    }

    pub fn unit(&self, i: usize) -> Option<&Unit> {
        self.units.get(i)
    }

    pub fn detail(&self) -> Option<&Detail> {
        self.detail.as_ref()
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
        sess!(self).last_refusal.clone()
    }

    /// Whether there is a `ringframe` to read a record from. Weft still runs
    /// panes without one; it just has nothing to show on the board.
    pub fn record_available(&self) -> bool {
        self.record_available
    }

    /// The Eval record behind a check, as the daemon read it. Nothing opens a
    /// file to draw a frame.
    pub fn record(&self, eval_id: &str) -> Option<weft_core::record::Record> {
        serde_json::from_value(sess!(self).records.get(eval_id)?.clone()).ok()
    }

    /// What this harness is short of, if anything. A harness Weft does not
    /// support, or has not asked about, is `Unknown` — never assumed ready.
    pub fn readiness(&self, harness: &str) -> Readiness {
        if let Some(local) = self.readiness.get(harness) {
            return *local;
        }
        readiness_of(sess!(self).readiness.get(harness).and_then(|v| v.as_str()))
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
        sess!(self).records = records;
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
        let view = sess!(self).panes.get(pane)?;
        blocked::looks_blocked(&view.contents())
    }

    /// The facts a decision reads, borrowed from the state that holds them.
    /// The decision itself is `weft_core::board`.
    fn with_board<T>(&self, f: impl FnOnce(Board<'_>) -> T) -> T {
        let panes = self.pane_facts();
        f(Board {
            units: &self.units,
            selected: self.selected_index().unwrap_or(usize::MAX),
            panes: &panes,
            focused: self.pane_focus,
            readiness: &|h| self.readiness(h),
            routing: &self.routing,
            workspace_gap: self.workspace_gap.as_deref(),
        })
    }

    fn pane_facts(&self) -> Vec<PaneInfo> {
        sess!(self)
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
    pub fn missing_agent(&self, act: Act) -> Option<String> {
        self.with_board(|b| b.missing_agent(act))
    }

    pub fn unavailable(&self, act: Act) -> Option<String> {
        self.with_board(|b| b.unavailable(act))
    }

    pub fn deciding_harness(&self, act: Act) -> Option<String> {
        self.with_board(|b| b.deciding_harness(act))
    }

    /// The sidebar, top to bottom, with folded levels' children left out.
    pub fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        for (at, p) in self.projects.iter().enumerate() {
            let units = self.units_of(at);
            let waiting = units.iter().filter(|u| u.needs_you()).count();
            let open = units.iter().filter(|u| u.is_open()).count();
            let folded = self.folded.contains(&p.name);
            out.push(Row::Project { project: at, name: p.name.clone(), folded, open, waiting });
            if folded {
                continue;
            }
            // Harnesses in the order they first appear, so the tree does not
            // reshuffle itself as work arrives.
            let mut seen: Vec<&str> = Vec::new();
            for u in units {
                if !seen.contains(&u.harness.as_str()) {
                    seen.push(&u.harness);
                }
            }
            for name in seen {
                let running = p.session.panes.iter().any(|pane| pane.harness == name);
                let open = units.iter().any(|u| u.harness == name && u.is_open());
                // Nothing to go back to: no agent, and nothing left open.
                if !running && !open {
                    continue;
                }
                let key = format!("{at}\u{0}{name}");
                let folded = self.folded.contains(&key);
                let waiting = units.iter().filter(|u| u.harness == name && u.needs_you()).count();
                out.push(Row::Harness {
                    project: at,
                    name: name.to_string(),
                    folded,
                    waiting,
                    running,
                });
                if folded {
                    continue;
                }
                out.extend(
                    units
                        .iter()
                        .enumerate()
                        .filter(|(_, u)| u.harness == name)
                        .map(|(i, _)| Row::Action { project: at, unit: i }),
                );
            }
        }
        out
    }

    /// Move the focus to the project the selected row is in. An act lands in
    /// the project you are looking at, never in the one you were.
    fn follow_the_selection(&mut self) {
        let Some(at) = self.rows().get(self.selected).map(Row::project) else { return };
        if at != self.at {
            self.at = at;
            self.pane_focus = 0;
            self.take_the_board();
        }
    }

    /// The focused project's root and name.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.projects[self.at].session
    }

    pub fn root(&self) -> &std::path::Path {
        &self.projects[self.at].root
    }

    pub fn project_name(&self) -> &str {
        &self.projects[self.at].name
    }

    /// The board of one project. The focused one is kept in `units` because
    /// everything that draws a frame wants it; the rest are read from their
    /// own connection.
    pub fn units_of(&self, at: usize) -> &[Unit] {
        if at == self.at { &self.units } else { &self.projects[at].session.units }
    }

    /// The unit the selected row is about, if it is about one.
    pub fn selected_index(&self) -> Option<usize> {
        match self.rows().get(self.selected) {
            Some(Row::Action { unit, .. }) => Some(*unit),
            _ => None,
        }
    }

    pub fn selected_unit(&self) -> Option<&Unit> {
        self.selected_index().and_then(|i| self.units.get(i))
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
        sess!(self).panes.iter().position(|p| p.harness == harness)
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
        let _ = sess!(self).resize(pane, rows, cols);
        if let Some(p) = sess!(self).panes.get(pane) {
            p.with_screen(f);
        }
    }

    // --- construction ---------------------------------------------------------

    pub fn new(root: PathBuf, toggle: Toggle) -> Self {
        let session = Session::open(&root, 24, 80).expect("a session");
        Self::with_session(root, toggle, session)
    }

    pub fn with_session(root: PathBuf, toggle: Toggle, session: Session) -> Self {
        Self {
            projects: vec![Open::new(root, session)],
            at: 0,
            pane_focus: 0,
            units: Vec::new(),
            selected: 0,
            folded: Default::default(),
            list_offset: 0,
            detail: None,
            detail_reach: 0,
            list_rows: 1,
            show_work: true,
            focus: Focus::Weft,
            toggle,
            theme: Theme::new(),
            modal: None,
            modal_choice: 0,
            quit: false,
            stop_agents_on_quit: false,
            hint: None,
            record_available: true,
            workspace_gap: None,
            readiness: std::collections::HashMap::new(),
            announced: std::collections::HashSet::new(),
            starts: Vec::new(),
            routing: Default::default(),
            newer: Default::default(),
        }
    }

    /// `spec` is the command line the person would have typed, e.g.
    /// `claude --model sonnet --effort medium`. Weft never chooses the model:
    /// that is the harness's configuration and the person's decision.
    pub fn add(&mut self, harness: &str, spec: &str) -> Result<()> {
        sess!(self).spawn(harness, spec)?;
        // The pane appears when the server says it has one, and what the
        // harness is short of follows a moment later — asking it costs a
        // process, so the daemon does that off its own loop.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let before = sess!(self).panes.len();
        let mut running = false;
        while std::time::Instant::now() < deadline {
            sess!(self).pump();
            running |= sess!(self).panes.len() > before;
            if running && sess!(self).readiness.get(harness).is_some() {
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
        // The mouse is Weft's only while an agent has the keys, so the wheel
        // scrolls that agent; in Weft it stays the terminal's.
        let mut captured = false;
        while !self.quit {
            let want = self.focus == Focus::Agent;
            if want != captured {
                captured = want;
                let mut out = std::io::stdout();
                let _ = if want {
                    crossterm::execute!(out, EnableMouseCapture)
                } else {
                    crossterm::execute!(out, DisableMouseCapture)
                };
            }
            terminal.draw(|frame| crate::ui::draw(frame, &mut self))?;
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key)
                        if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                    {
                        self.on_key(key)?
                    }
                    Event::Paste(text) => self.on_paste(&text)?,
                    Event::Mouse(m) => self.on_mouse(m),
                    _ => {}
                }
            }
            if sess!(self).pump() {
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
            .find(|i| !self.announced.contains(i) && !sess!(self).panes[*i].running)
        else {
            return;
        };
        self.announced.insert(pane);
        let harness = self.harness_at(pane).unwrap_or("the agent").to_string();
        // The work is in the record, so the pane goes and the row says so.
        self.close_pane(pane);
        self.say(format!("{harness} has ended. Its work is still on the list."));
    }

    /// Take a pane away and forget what was said about it. Every pane after it
    /// moves up one, so nothing may hold on to an index across this.
    fn close_pane(&mut self, pane: usize) {
        let before = self.pane_count();
        let _ = sess!(self).close(pane);
        // Wait for the server's new numbering before anything else acts on a
        // pane index. Every pane after this one moves up, and a spawn sent
        // into the gap would be counted against the old list.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && self.pane_count() >= before {
            sess!(self).pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        self.announced.clear();
        if self.pane_focus >= pane && self.pane_focus > 0 {
            self.pane_focus -= 1;
        }
        self.focus = Focus::Weft;
    }

    /// Offer to start the harness this work needs: the session the selected
    /// Ask was asked in, else the harness's latest on record, else fresh.
    fn offer_pick_up(&mut self, harness: String, then: Option<Act>) {
        self.look_for_agents();
        let own = self.selected_unit().filter(|u| u.harness == harness).and_then(|u| {
            let id = u.session_ref.clone()?;
            Some(Recorded { id, at: u.asked_at.clone(), last: u.title.clone() })
        });
        let session = own.or_else(|| {
            self.starts.iter().find(|s| s.harness == harness).and_then(|s| s.session.clone())
        });
        self.modal_choice = 0;
        self.modal = Some(Modal::PickUp { harness, session, then });
    }

    /// Start the harness where it left off or fresh, then carry on. Weft runs
    /// the command a person would type, and does not claim the session came
    /// back: the harness says that, in its own pane.
    fn pick_up(&mut self, harness: &str, session: Option<&Recorded>, then: Option<Act>) {
        self.modal = None;
        let Some(h) = harness::find(harness) else {
            self.modal = Some(Modal::Note(format!("Weft does not know how to start {harness}.")));
            return;
        };
        let spec = match session {
            Some(s) => h.resume_spec(&s.id),
            None => h.spec(),
        };
        if let Err(e) = self.add(harness, &spec) {
            self.modal = Some(Modal::Note(format!("Could not start {spec}: {e}")));
            return;
        }
        self.pane_focus = self.pane_count().saturating_sub(1);
        match then {
            Some(act) => self.act(act),
            None => self.focus = Focus::Agent,
        }
    }

    /// Take the board the daemon read, and notice anything it implies. The
    /// record is followed once, in the daemon, and every client is told.
    fn take_the_board(&mut self) {
        // Every project's connection is pumped, so a board in the background
        // is as current as the one on screen.
        for p in &mut self.projects {
            p.session.pump();
        }
        if self.units != sess!(self).units {
            self.units = sess!(self).units.clone();
            self.clamp_selection();
        }
        self.routing = sess!(self).routing.clone();
        self.workspace_gap = sess!(self).gap.clone();
        // A missing CLI is the same answer for every harness, so any one of
        // them saying so is the machine saying so.
        self.record_available =
            !sess!(self).readiness.as_object().is_some_and(|m| m.values().any(|v| v == "no_cli"));
        // A name in the routing file that is not a harness Weft knows. Said
        // once, on the way in: an act routed nowhere would otherwise just look
        // unavailable for no reason anyone could see.
        if !self.routing.unknown.is_empty() && self.hint.is_none() {
            let said = self.routing.unknown.join(", ");
            self.say(format!(
                "Routing names a harness Weft does not know ({said}). That act is not routed."
            ));
        }
        if !self.routing.ignored.is_empty() && self.hint.is_none() {
            let said = self.routing.ignored.join(", ");
            self.say(format!(
                "eval.json names something Weft does not know ({said}). It is ignored."
            ));
        }
    }

    /// Fold whatever the ledger has now. The run loop does this each tick;
    /// a probe or a test does it once.
    /// Pump until the daemon has said what the record holds, then take it.
    /// The run loop does this continuously; a probe or a test does it once.
    pub fn refresh_for_test(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && sess!(self).units.is_empty() {
            sess!(self).pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        self.take_the_board();
        self.look_for_agents();
    }

    fn clamp_selection(&mut self) {
        let rows = self.rows();
        self.selected = self.selected.min(rows.len().saturating_sub(1));
        // Land on the work, not on the project heading above it. Only while
        // nothing has been picked, so it never overrides a choice.
        if self.selected == 0
            && let Some(first) = rows.iter().position(|r| matches!(r, Row::Action { .. }))
        {
            self.selected = first;
        }
    }

    fn say(&mut self, sentence: impl Into<String>) {
        self.hint = Some(sentence.into());
    }

    /// Answer a question the person was asked. The daemon does the typing,
    /// because the daemon owns the pane.
    fn do_inject(&mut self, pending: &Pending) {
        // Weft must never answer a dialog meant for a person. What it reads
        // off the screen is an inference, though, and an inference that
        // cannot be overruled is a dead end — a stale chooser left in an
        // agent's scrollback would stop every Ask with no way through.
        if let Some(e) = self.waiting(pending.pane) {
            self.modal =
                Some(Modal::SendAnyway { pending: pending.clone(), evidence: e.line.clone() });
            self.modal_choice = 1;
            return;
        }
        self.do_inject_forcing(pending, false)
    }

    /// The pane looked busy and the person said to type anyway. What Weft read
    /// off the screen is an inference; this is the person overruling it.
    fn do_inject_forcing(&mut self, pending: &Pending, force: bool) {
        match sess!(self).resolve(&pending.staged, true, force) {
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
        if !self.guard(Act::Proceed) {
            return;
        }
        let Some(id) = self.selected_unit().map(|u| u.ask_id.clone()) else { return };
        self.ask_first("send", Some(&id), None, None);
    }

    /// Check or Decide, in whichever harness this project routes it to.
    fn start_skill(&mut self, skill: &str) {
        let act = if skill == "eval" { Act::Eval } else { Act::Seal };
        if !self.guard(act) {
            return;
        }
        let Some(id) = self.selected_unit().map(|u| u.ask_id.clone()) else { return };
        self.ask_first(if skill == "eval" { "check" } else { "decide" }, Some(&id), None, None);
    }

    fn send_ask(&mut self, text: String, target: usize, follows: Option<Follows>) {
        match follows {
            Some(f) => self.ask_first("follow_up", Some(&f.ask_id), Some(target), Some(&text)),
            None => self.ask_first("ask", None, Some(target), Some(&text)),
        }
    }

    /// One unit's whole story, in one blocking view: the Ask that started it,
    /// the Eval that judged it, the Seal that closed it.
    ///
    /// Three reads, because the record keeps the three apart. They are joined
    /// here rather than behind three keys, which is what `[P] WORDING`,
    /// `[J] JUDGES` and `[T] THE SEAL` were.
    fn open_detail(&mut self) {
        if !self.guard(Act::Detail) {
            return;
        }
        let Some(unit) = self.selected_unit().cloned() else { return };
        // A part that will not come back is a fact about the record, not a
        // reason to refuse the view: the section says so and the rest opens.
        let prompt = sess!(self)
            .read("wording", &unit.ask_id)
            .ok()
            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_string));
        let record = unit.check.as_ref().and_then(|c| {
            sess!(self)
                .read("judges", &c.eval_id)
                .ok()
                .and_then(|v| serde_json::from_value::<weft_core::record::Record>(v).ok())
        });
        let seal = unit.seal_id.as_ref().and_then(|id| sess!(self).read("seal", id).ok());
        self.detail = Some(Detail {
            lines: weft_core::offers::detail_read(
                &unit,
                prompt.as_deref(),
                record.as_ref(),
                seal.as_ref(),
            ),
            title: unit.title.clone(),
            harness: unit.harness.clone(),
            next: weft_core::board::next_step(&unit),
            offset: 0,
        });
    }

    /// Carry the selected unit forward, whatever that means for it.
    fn proceed(&mut self) {
        if !self.guard(Act::Proceed) {
            return;
        }
        let Some(unit) = self.selected_unit().cloned() else { return };
        match weft_core::board::next_step(&unit) {
            Some(Next::Confirm) | Some(Next::Send) => self.start_send(),
            Some(Next::Eval) => self.start_skill("eval"),
            Some(Next::Seal) => self.start_skill("seal"),
            None => {}
        }
    }

    /// Ask, off the draw loop, whether RingFrame or a harness is behind. The
    /// answer lights `↑ [U]PDATE`.
    pub fn look_for_updates(&mut self) {
        let _ = sess!(self).sync_now(false);
    }

    /// Open the RingFrame view, which asks again the moment it opens.
    fn open_ringframe(&mut self) {
        if let Err(e) = sess!(self).sync_now(false) {
            self.modal = Some(Modal::Note(format!("Weft could not ask: {e}")));
            return;
        }
        self.modal = Some(Modal::RingFrame);
    }

    /// The RingFrame view, once the daemon has answered.
    pub fn sync_view(&self) -> Option<&weft_core::sync::View> {
        sess!(self).sync.as_ref()
    }

    fn proceed_sync(&mut self) {
        if self.sync_view().is_some_and(|v| v.needs_anything() && !v.running) {
            let _ = sess!(self).sync_now(true);
        }
    }

    fn start_ask(&mut self, follows: Option<Follows>) {
        // The compose box opens on the harness this project asks in, when it
        // says. The person can still pick another before sending — routing is
        // where Weft starts, not somewhere it holds them.
        let target =
            self.routing.get("ask").and_then(|name| self.pane_for(name)).unwrap_or(self.pane_focus);
        self.modal = Some(Modal::Ask { text: String::new(), target, follows });
    }

    /// What starting an agent could mean here. Asked when the picker opens,
    /// not while drawing: it reads receipts off disk in the daemon.
    pub fn starts(&self) -> &[Start] {
        &self.starts
    }

    fn look_for_agents(&mut self) {
        self.starts = sess!(self)
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

    /// What `[N]` offers: each harness fresh. Picking a session back up is
    /// done from the work, which knows which session it was.
    pub fn fresh_starts(&self) -> Vec<Start> {
        self.starts.iter().filter(|s| s.session.is_none()).cloned().collect()
    }

    fn start_agent_picker(&mut self) {
        self.look_for_agents();
        if self.fresh_starts().is_empty() {
            self.say("No coding agent found. Install claude or codex first.");
            return;
        }
        self.modal = Some(Modal::StartAgent { choice: 0 });
        self.modal_choice = 0;
    }

    pub fn start_chosen_agent(&mut self, choice: usize) {
        let Some(start) = self.fresh_starts().get(choice).cloned() else { return };
        match self.add(start.harness, &start.spec) {
            Ok(()) => {
                self.modal = None;
                self.pane_focus = sess!(self).panes.len().saturating_sub(1);
                self.focus = Focus::Agent;
            }
            Err(e) => self.modal = Some(Modal::Note(format!("Could not run {}: {e}", start.spec))),
        }
    }

    /// Ask the daemon for an act, and show whatever it puts in front of the
    /// person. The daemon works out what to type, where it goes and how; this
    /// only names the act.
    fn ask_first(
        &mut self,
        act: &str,
        unit: Option<&str>,
        pane: Option<usize>,
        text: Option<&str>,
    ) {
        match sess!(self).act(act, unit, pane, text) {
            Ok(id) => {
                sess!(self).pump();
                let Some(w) = sess!(self).waiting.iter().find(|w| w.id == id).cloned() else {
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

    fn scroll_detail(&mut self, delta: i32) {
        // Held down, `↓` used to scroll the whole reading off the top and
        // leave an empty panel. It stops at the last line instead.
        let reach = self.detail_reach as i32;
        if let Some(d) = self.detail.as_mut() {
            d.offset = (d.offset as i32 + delta).clamp(0, reach) as usize;
        }
    }

    // --- moving about ---------------------------------------------------------

    fn pick(&mut self, delta: i8) {
        let n = self.rows().len() as i32;
        if n == 0 {
            return;
        }
        self.selected = ((self.selected as i32 + delta as i32).rem_euclid(n)) as usize;
        self.follow_the_selection();
        // Picking a row that is scrolled away brings it back into view, above
        // or below. There is no wheel any more, so the arrows have to.
        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        }
        let last_visible = self.list_offset + self.list_rows.saturating_sub(1);
        if self.selected > last_visible {
            self.list_offset = self.selected + 1 - self.list_rows;
        }
    }

    /// Move through the agents the `[N]` picker offers. It wraps, like the
    /// work list, because a three-item list that stops is a list you fight.
    fn pick_agent(&mut self, delta: i8) {
        let n = self.fresh_starts().len();
        if n == 0 {
            return;
        }
        self.modal_choice =
            ((self.modal_choice as i32 + delta as i32).rem_euclid(n as i32)) as usize;
    }

    /// `[Enter]` opens what is closed and acts on what is already open. The
    /// first press on a row always reveals; the second goes somewhere. On a
    /// project it only ever folds and unfolds — nothing in the sidebar
    /// removes anything on `[Enter]`.
    fn open_row(&mut self) {
        let Some(row) = self.rows().get(self.selected).cloned() else {
            self.say("Nothing has been asked for yet.");
            return;
        };
        match row {
            Row::Project { .. } => self.set_fold(&row, !row.folded()),
            Row::Harness { ref name, .. } if row.folded() => self.set_fold(&row, false),
            Row::Harness { ref name, .. } => self.go_to_harness(name),
            Row::Action { .. } => match self.selected_unit() {
                Some(u) if u.is_open() && self.pane_for(&u.harness).is_none() => {
                    let harness = u.harness.clone();
                    self.offer_pick_up(harness, None)
                }
                _ => self.act(Act::Detail),
            },
        }
    }

    /// `[→]` and `[←]` unfold and fold without ever activating, which is what
    /// a person wants when they are only looking.
    fn set_fold(&mut self, row: &Row, folded: bool) {
        let Some(key) = row.fold_key() else { return };
        if folded {
            self.folded.insert(key);
        } else {
            self.folded.remove(&key);
        }
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
    }

    fn fold_selected(&mut self, folded: bool) -> bool {
        let Some(row) = self.rows().get(self.selected).cloned() else { return false };
        if row.fold_key().is_none() || row.folded() == folded {
            return false;
        }
        self.set_fold(&row, folded);
        true
    }

    /// Open another project beside the ones already here. A window holds as
    /// many as the person opens; the daemon has always been able to.
    pub(crate) fn open_project(&mut self, typed: &str) {
        let root = match std::path::PathBuf::from(shellexpand(typed)).canonicalize() {
            Ok(r) if r.is_dir() => r,
            _ => {
                self.say("There is no directory there.");
                return;
            }
        };
        if let Some(at) = self.projects.iter().position(|p| p.root == root) {
            self.at = at;
            self.take_the_board();
            self.say("That project is already open.");
            return;
        }
        // The same daemon, not a new one: it already holds many projects,
        // and starting a second would need the installed binary.
        let socket = sess!(self).socket().to_path_buf();
        match Session::connect(&socket, &root, 24, 80) {
            Ok(session) => {
                self.projects.push(Open::new(root, session));
                self.at = self.projects.len() - 1;
                self.pane_focus = 0;
                self.selected = 0;
                self.take_the_board();
                self.look_for_agents();
            }
            Err(e) => self.modal = Some(Modal::Note(said(&e))),
        }
    }

    /// Take a project out of this window. The agents keep running and the
    /// record is untouched; Weft stops showing it. Always asks first,
    /// because it is the only thing in the sidebar that removes anything.
    fn close_project(&mut self) {
        match self.rows().get(self.selected) {
            Some(Row::Project { name, .. }) if self.projects.len() > 1 => {
                let name = name.clone();
                self.modal = Some(Modal::CloseProject { name });
                self.modal_choice = 0;
            }
            Some(Row::Project { .. }) => {
                self.say("This is the only project open. [X] QUIT closes the window.")
            }
            _ => self.say("Select a project to close it."),
        }
    }

    /// The pane that is running this harness, or the offer to pick it up.
    fn go_to_harness(&mut self, harness: &str) {
        if let Some(i) = sess!(self).panes.iter().position(|p| p.harness == harness) {
            self.pane_focus = i;
            self.focus = Focus::Agent;
            return;
        }
        self.offer_pick_up(harness.to_string(), None);
    }

    /// `←` is back everywhere in Weft: it closes the drawer, then collapses the
    /// row. `Esc` is never Weft's.
    fn back(&mut self) {
        if self.detail.take().is_some() {
            return;
        }
        // In the sidebar `←` folds the level it is on before it goes anywhere.
        if self.focus == Focus::Weft && self.modal.is_none() && self.fold_selected(true) {
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
            self.detail = None;
            return;
        }
        // The thing that needs you is an Ask, and its detail view is where
        // you act on it, so one key goes the whole way. When nothing needs
        // you it is the newest Ask, which is the one just made.
        let rows = self.rows();
        let actions: Vec<usize> =
            (0..rows.len()).filter(|i| matches!(rows[*i], Row::Action { .. })).collect();
        if actions.is_empty() {
            self.say("Nothing has been asked for yet.");
            return;
        }
        let pick = {
            let needs = |i: &usize| match &rows[*i] {
                Row::Action { project, unit } => {
                    self.units_of(*project).get(*unit).is_some_and(|u| u.needs_you())
                }
                _ => false,
            };
            actions
                .iter()
                .copied()
                .find(|i| *i > self.selected && needs(i))
                .or_else(|| actions.iter().copied().find(|i| needs(i)))
                .unwrap_or(*actions.last().expect("one"))
        };
        self.selected = pick;
        self.show_work = true;
        self.follow_the_selection();
        self.act(Act::Detail);
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
            self.detail = None;
        }
    }

    /// What an action does, wherever it was asked for. The bar shows a key for
    /// every one of these and each is also a click, so they meet here.
    pub fn act(&mut self, act: Act) {
        // Work whose agent is not running is picked back up, not refused.
        if matches!(act, Act::Proceed | Act::Eval | Act::Seal)
            && let Some(harness) = self.missing_agent(act)
        {
            return self.offer_pick_up(harness, Some(act));
        }
        match act {
            Act::Ask => {
                if self.guard(Act::Ask) {
                    self.start_ask(None);
                }
            }
            Act::Proceed => self.proceed(),
            Act::Detail => self.open_detail(),
            Act::Eval => self.start_skill("eval"),
            Act::Seal => self.start_skill("seal"),
            Act::FollowUp => {
                if self.guard(Act::FollowUp) {
                    self.detail = None;
                    let follows = self.selected_unit().map(|u| Follows {
                        ask_id: u.ask_id.clone(),
                        title: u.title.clone(),
                        carries: weft_core::board::follow_up_of(u).to_string(),
                    });
                    self.start_ask(follows);
                }
            }
            Act::ReadyUp => self.open_ringframe(),
            Act::NewAgent => self.start_agent_picker(),
            Act::ToggleSidebar => self.toggle_work(),
            // Opening another project is the next step's work; until then it
            // says so rather than pretending.
            Act::OpenProject => {
                self.modal = Some(Modal::OpenProject { text: String::new() });
            }
            Act::WeftMenu => {
                self.modal = Some(Modal::Weft);
                self.modal_choice = 0;
            }
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

    // --- events ------------------------------------------------------------

    /// One keystroke. Public so a probe or a wireframe run can drive the
    /// same path the terminal does.
    pub fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.modal.is_some() {
            return self.on_modal_key(key);
        }
        self.hint = None;
        let chord = chord_of(key);
        // Esc is the agent's, except where a view of Weft's own is open.
        let closing =
            key.code == event::KeyCode::Esc && self.focus == Focus::Weft && self.detail().is_some();
        let action =
            if closing { Action::Back } else { keys::route(chord, self.focus, self.toggle) };
        match action {
            Action::ToggleFocus => self.toggle_focus(),
            Action::ToAgent => {
                if let Some(bytes) = encode::encode(key) {
                    // Typing goes where the agent is now, so the view follows.
                    if let Some(p) = sess!(self).panes.get_mut(self.pane_focus) {
                        p.scroll_to_bottom();
                    }
                    let _ = sess!(self).input(self.pane_focus, &bytes);
                }
            }
            Action::Pick(delta) => {
                if self.detail.is_some() {
                    self.scroll_detail(delta as i32);
                } else {
                    self.pick(delta);
                }
            }
            Action::Open => {
                if self.waiting_here() {
                    // [Enter] ANSWER IT: the person answers, never Weft.
                    self.focus = Focus::Agent;
                } else if self.detail.is_none() {
                    self.open_row();
                }
            }
            Action::Back => self.back(),
            Action::NextNeedsYou => self.next_needs_you(),
            Action::ToggleSidebar => self.act(Act::ToggleSidebar),
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
            Action::Proceed => self.act(Act::Proceed),
            Action::NewAgent => self.act(Act::NewAgent),
            Action::Eval => self.act(Act::Eval),
            Action::Seal => self.act(Act::Seal),
            Action::FollowUp => self.act(Act::FollowUp),
            Action::Explain => self.explain_waiting(),
            Action::ReadyUp => self.act(Act::ReadyUp),
            Action::OpenProject => self.act(Act::OpenProject),
            Action::WeftMenu => self.act(Act::WeftMenu),
            Action::Unfold => {
                self.fold_selected(false);
            }
            Action::CloseProject => self.close_project(),
            Action::Quit => self.act(Act::Quit),
            Action::Help => self.act(Act::Help),
            Action::Ignore => {}
        }
        Ok(())
    }

    /// The wheel, while an agent has the keys: its pane's scrollback.
    pub fn on_mouse(&mut self, m: MouseEvent) {
        let delta = match m.kind {
            MouseEventKind::ScrollUp => 3,
            MouseEventKind::ScrollDown => -3,
            _ => return,
        };
        if self.focus != Focus::Agent {
            return;
        }
        let Some(p) = sess!(self).panes.get_mut(self.pane_focus) else { return };
        let before = p.scroll_offset();
        p.scroll(delta);
        if p.scroll_offset() != before || delta < 0 {
            return;
        }
        // Nothing moved: say whose history it is rather than doing nothing.
        let name = self.harness_at(self.pane_focus).unwrap_or("the agent").to_string();
        self.say(match harness::find(&name).and_then(|h| h.transcript) {
            Some(key) => {
                format!("{name} keeps its own history: press {key} in the agent to read it.")
            }
            None => format!("Nothing of {name}'s has scrolled away yet."),
        });
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
        if let Modal::OpenProject { mut text } = modal {
            match key.code {
                event::KeyCode::Left | event::KeyCode::Esc => self.modal = None,
                event::KeyCode::Enter if !text.trim().is_empty() => {
                    self.modal = None;
                    let typed = text.clone();
                    self.open_project(&typed);
                }
                event::KeyCode::Backspace => {
                    text.pop();
                    self.modal = Some(Modal::OpenProject { text });
                }
                event::KeyCode::Char(c) => {
                    text.push(c);
                    self.modal = Some(Modal::OpenProject { text });
                }
                _ => {}
            }
            return Ok(());
        }

        if let Modal::Ask { mut text, mut target, follows } = modal {
            match key.code {
                // `[←] CANCEL` is what the box offers, so it cancels whether
                // or not anything has been typed.
                event::KeyCode::Left | event::KeyCode::Esc => self.modal = None,
                event::KeyCode::Enter if !text.trim().is_empty() => {
                    self.send_ask(text, target, follows)
                }
                event::KeyCode::Backspace => {
                    text.pop();
                    self.modal = Some(Modal::Ask { text, target, follows });
                }
                event::KeyCode::Tab => {
                    if self.pane_count() > 0 {
                        target = (target + 1) % self.pane_count();
                    }
                    self.modal = Some(Modal::Ask { text, target, follows });
                }
                event::KeyCode::Char(c) => {
                    text.push(c);
                    self.modal = Some(Modal::Ask { text, target, follows });
                }
                _ => {}
            }
            return Ok(());
        }

        let options = match &modal {
            Modal::Quit => 3,
            Modal::Weft => WEFT_MENU.len(),
            Modal::CloseProject { .. } => 2,
            Modal::SendAnyway { .. } => 2,
            Modal::OpenProject { .. } => 1,
            Modal::StartAgent { .. } => self.fresh_starts().len().max(1),
            Modal::PickUp { session, .. } => pick_up_choices(session.as_ref()).len(),
            _ => 1,
        };
        match key.code {
            // The agent picker is the same picker wherever it is drawn.
            event::KeyCode::Up if matches!(modal, Modal::StartAgent { .. }) => self.pick_agent(-1),
            event::KeyCode::Down if matches!(modal, Modal::StartAgent { .. }) => self.pick_agent(1),
            event::KeyCode::Up => self.modal_choice = self.modal_choice.saturating_sub(1),
            event::KeyCode::Down => self.modal_choice = (self.modal_choice + 1).min(options - 1),
            event::KeyCode::Char('p' | 'P') if modal == Modal::RingFrame => self.proceed_sync(),
            event::KeyCode::Char(c) if modal == Modal::Weft => {
                if let Some(&(_, _, act)) =
                    WEFT_MENU.iter().find(|(k, ..)| k.eq_ignore_ascii_case(&c))
                {
                    self.modal = None;
                    self.act(act);
                }
            }
            event::KeyCode::Left | event::KeyCode::Esc => match &modal {
                // A question nobody is going to answer is taken off the
                // daemon's queue, not left there for another client.
                Modal::Confirm(p) => {
                    let id = p.staged.clone();
                    let _ = sess!(self).resolve(&id, false, false);
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
            Modal::RingFrame => {}
            Modal::PickUp { harness, session, then } => {
                let (harness, session, then) = (harness.clone(), session.clone(), *then);
                match pick_up_choices(session.as_ref()).get(self.modal_choice) {
                    Some(PickUp::Resume) => self.pick_up(&harness, session.as_ref(), then),
                    _ => self.pick_up(&harness, None, then),
                }
            }
            // Its own keyboard, handled above.
            Modal::OpenProject { .. } => {}
            Modal::SendAnyway { pending, .. } => {
                let pending = pending.clone();
                self.modal = None;
                if self.modal_choice == 0 {
                    self.do_inject_forcing(&pending, true);
                }
            }
            Modal::CloseProject { name } => {
                self.modal = None;
                if self.modal_choice == 0
                    && let Some(at) = self.projects.iter().position(|p| p.name == *name)
                {
                    // The connection goes; the agents and the record do not.
                    self.projects.remove(at);
                    self.at = self.at.min(self.projects.len() - 1);
                    self.pane_focus = 0;
                    self.selected = 0;
                    self.take_the_board();
                }
            }
            Modal::Weft => {
                let act = WEFT_MENU[self.modal_choice.min(WEFT_MENU.len() - 1)].2;
                self.modal = None;
                self.act(act);
            }
            Modal::Quit => match self.modal_choice {
                0 => {
                    // Leave; the server keeps the agents working.
                    sess!(self).detach();
                    self.quit = true;
                }
                1 => {
                    self.stop_agents_on_quit = true;
                    sess!(self).shutdown();
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
                let _ = sess!(self).input(self.pane_focus, text.as_bytes());
            }
            (None, Focus::Weft) => {}
        }
        Ok(())
    }

    // --- what the last frame drew --------------------------------------------

    /// How far the drawer can scroll: the folded line count less the room it
    /// has. Only the render knows both, so it says so each frame.
    /// How many rows the sidebar drew.
    pub fn note_list_rows(&mut self, rows: usize) {
        self.list_rows = rows.max(1);
    }

    pub fn note_detail_reach(&mut self, last: usize) {
        self.detail_reach = last;
    }

    // --- test seams -----------------------------------------------------------

    /// The units the board is showing.
    pub fn set_units(&mut self, units: Vec<Unit>) {
        // The board belongs to the project it came from, not to the window,
        // or it would vanish the moment the focus moved elsewhere.
        sess!(self).units = units.clone();
        self.units = units;
        self.clamp_selection();
    }

    pub fn input(&mut self, pane: usize, bytes: &[u8]) -> Result<()> {
        sess!(self).input(pane, bytes)
    }

    pub fn pump(&mut self) -> bool {
        sess!(self).pump()
    }

    pub fn pane_text(&self, pane: usize) -> Option<String> {
        sess!(self).panes.get(pane).map(|p| p.contents())
    }

    pub fn pane_scroll_offset(&self, pane: usize) -> Option<usize> {
        sess!(self).panes.get(pane).map(|p| p.scroll_offset())
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
        event::KeyCode::Right => Key::Right,
        event::KeyCode::Backspace => Key::Backspace,
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
        let rf = app.root().join(".fab7/rf");
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
        let rf = a.root().join(".fab7/rf");
        std::fs::create_dir_all(&rf).expect("rf");
        let lines: String = events.iter().map(|e| format!("{e}\n")).collect();
        std::fs::write(rf.join("ledger.jsonl"), lines).expect("ledger");
        // Wait for the daemon's next look, then take what it read.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.session_mut().units.is_empty() {
            a.session_mut().pump();
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
            gathered: None,
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

    #[test]
    fn an_agent_that_quits_closes_its_pane_and_keeps_its_work() {
        let mut a = with_unit(Sent::TakenByAgent);
        a.focus = Focus::Agent;
        a.input(0, &[4]).expect("Ctrl+D ends /bin/cat");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.pane_count() > 0 {
            a.pump();
            a.notice_an_agent_that_ended();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(a.pane_count(), 0, "the pane is gone, not left dead on screen");
        assert!(a.modal.is_none(), "no panel: the row says it");
        assert_eq!(a.focus, Focus::Weft, "keystrokes never go to a dead process");
        assert!(a.hint_text().is_some_and(|h| h.contains("still on the list")));
        assert!(
            a.rows().iter().any(|r| matches!(r, Row::Harness { running: false, .. })),
            "its work stays, marked not running"
        );
    }

    #[test]
    fn the_wheel_scrolls_the_agent_only_while_it_has_the_keys() {
        let mut a = app();
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(wheel);
        assert!(a.hint_text().is_none(), "in Weft the wheel is the terminal's");
        a.focus = Focus::Agent;
        a.on_mouse(wheel);
        assert!(a.hint_text().is_some(), "in the agent it is Weft's, and says why nothing moved");
    }

    #[test]
    fn resuming_is_offered_only_when_the_record_names_a_session() {
        assert_eq!(pick_up_choices(None), vec![PickUp::Fresh]);
        let known = Recorded { id: "01a0bdb6".into(), at: String::new(), last: String::new() };
        assert_eq!(pick_up_choices(Some(&known)), vec![PickUp::Resume, PickUp::Fresh]);
    }

    #[test]
    fn open_work_with_no_agent_offers_the_session_it_was_asked_in() {
        let mut a = app();
        let mut u = unit(Sent::TakenByAgent);
        u.harness = "claude-code".into();
        u.session_ref = Some("dfffa9be".into());
        a.set_units(vec![u]);
        a.selected = a.rows().iter().position(|r| matches!(r, Row::Action { .. })).expect("row");
        press(&mut a, KeyCode::Enter);
        let Some(Modal::PickUp { harness, session: Some(s), then: None }) = a.modal.clone() else {
            panic!("the choice, naming its own session: {:?}", a.modal)
        };
        assert_eq!((harness.as_str(), s.id.as_str()), ("claude-code", "dfffa9be"));
    }

    #[test]
    fn a_harness_with_nothing_open_and_no_agent_is_not_listed() {
        let mut a = app();
        let mut u = unit(Sent::TakenByAgent);
        u.harness = "claude-code".into();
        u.sealed = Some("accepted".into());
        a.set_units(vec![u]);
        assert!(
            !a.rows()
                .iter()
                .any(|r| matches!(r, Row::Harness { name, .. } if name == "claude-code"))
        );
    }

    #[test]
    fn the_latest_recorded_session_is_found_for_picking_up() {
        // `starts()` only offers a harness that is on this machine, so this
        // is about what Weft does when one is. A runner has no Codex.
        if !codex_on_path() {
            eprintln!("skipped: codex is not on PATH");
            return;
        }
        let mut a = app();
        let root = a.root().to_path_buf();
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
        assert_eq!(a.unavailable(Act::Proceed), None, "proceeding is the person's yes");
    }

    /// A project that routes its acts. Weft reads the file once, so this sets
    /// the routing directly rather than going through a temporary HOME.
    fn routed(pairs: &[(&str, &str)]) -> App {
        let mut a = app();
        let acts: serde_json::Map<String, serde_json::Value> =
            pairs.iter().map(|(k, v)| ((*k).to_string(), serde_json::json!(v))).collect();
        let text = serde_json::json!({ a.root().to_string_lossy().into_owned(): acts }).to_string();
        a.routing = weft_core::routing::read(&text, a.root());
        a.set_units(vec![unit(Sent::TakenByAgent)]);
        a
    }

    #[test]
    fn an_act_goes_to_the_harness_this_project_routes_it_to() {
        // Codex asks, Claude evaluates. The work was done in codex.
        let a = routed(&[("eval", "claude-code")]);
        assert_eq!(a.deciding_harness(Act::Eval).as_deref(), Some("claude-code"));
        // Seal was not routed, so it still follows the work.
        assert_eq!(a.deciding_harness(Act::Seal).as_deref(), Some("codex"));
        // And Send is never routed: it is the delivery of an Ask already made.
        assert_eq!(weft_core::board::Board::act_key(Act::Proceed), None);
    }

    #[test]
    fn readiness_is_asked_of_the_harness_the_act_will_go_to() {
        // The point of routing: Eval going to Claude Code needs Claude Code
        // set up, whatever the work was done in.
        let mut a = routed(&[("eval", "claude-code")]);
        a.set_readiness("codex", Readiness::Ready);
        a.set_readiness("claude-code", Readiness::Missing(Gap::Plugin));
        let said = a.unavailable(Act::Eval).expect("a reason");
        assert!(said.contains("claude-code"), "names the harness it would go to: {said}");
        // And the harness that did the work being ready does not help.
        assert_eq!(a.unavailable(Act::Seal), None, "seal still follows the work");
    }

    #[test]
    fn a_routed_act_with_no_pane_says_which_harness_is_missing() {
        // Readiness is answered first — that ordering is deliberate — so make
        // the routed harness ready and leave it without a pane.
        let mut a = routed(&[("eval", "claude-code")]);
        a.set_readiness("claude-code", Readiness::Ready);
        let said = a.unavailable(Act::Eval).expect("a reason");
        assert!(said.contains("claude-code"), "{said}");
        assert!(said.contains("nowhere to send"), "{said}");
    }

    #[test]
    fn a_project_that_routes_nothing_behaves_exactly_as_before() {
        let a = routed(&[]);
        assert!(a.routing().is_empty());
        for act in [Act::Ask, Act::Eval, Act::Seal] {
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
        press(&mut a, KeyCode::Char('b'));
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

    /// `recorded`, plus an Eval over that Ask, once the daemon has read both.
    fn recorded_and_evaluated() -> App {
        let mut a = recorded(Sent::TakenByAgent);
        let rf = a.root().join(".fab7/rf/ledger.jsonl");
        let mut ledger = std::fs::read_to_string(&rf).expect("ledger");
        ledger.push_str(
            &(serde_json::json!({
                "schema": "ringframe.ledger/1", "event_id": "evt_9", "type": "eval.completed",
                "time": "2026-09-19T15:00:00Z", "id": "evl_1",
                "actor": {"kind": "human", "id": "local-user"}, "links": [],
                "data": {"verdict": "drifted", "confidence": 0.67, "basis": {"asks": ["ask_1"]}}
            })
            .to_string()
                + "\n"),
        );
        std::fs::write(&rf, ledger).expect("ledger");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline
            && a.session_mut().units.first().is_none_or(|u| u.check.is_none())
        {
            a.session_mut().pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        a.take_the_board();
        a
    }

    fn follow_up_payload(mut a: App, intent: &str) -> String {
        press(&mut a, KeyCode::Char('f'));
        for c in intent.chars() {
            press(&mut a, KeyCode::Char(c));
        }
        press(&mut a, KeyCode::Enter);
        let Some(Modal::Confirm(p)) = a.modal.clone() else {
            panic!("expected a confirmation, got {:?}", a.modal)
        };
        String::from_utf8(p.payload).unwrap()
    }

    #[test]
    fn a_follow_up_carries_the_ask_or_the_eval_it_follows() {
        let text = follow_up_payload(recorded(Sent::TakenByAgent), "add a retry");
        assert!(text.ends_with("ask [follow-up ask_1] add a retry"), "got {text:?}");
        let text = follow_up_payload(recorded_and_evaluated(), "fix what it found");
        assert!(text.ends_with("ask [follow-up evl_1] fix what it found"), "got {text:?}");
    }

    #[test]
    fn a_plain_ask_carries_no_marker() {
        let mut a = recorded(Sent::TakenByAgent);
        press(&mut a, KeyCode::Char('a'));
        for c in "new thing".chars() {
            press(&mut a, KeyCode::Char(c));
        }
        press(&mut a, KeyCode::Enter);
        let Some(Modal::Confirm(p)) = a.modal.clone() else { panic!("{:?}", a.modal) };
        let text = String::from_utf8(p.payload).unwrap();
        assert!(!text.contains("[follow-up"), "got {text:?}");
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
    fn eval_and_seal_always_confirm_before_typing() {
        for (key, expect) in [('e', "eval"), ('s', "seal")] {
            let mut a = recorded(Sent::TakenByAgent);
            press(&mut a, KeyCode::Char(key));
            let Some(Modal::Confirm(p)) = a.modal.clone() else {
                panic!("{key} must confirm first, got {:?}", a.modal)
            };
            let text = String::from_utf8(p.payload).unwrap();
            // Whatever words follow, the command token is closed, so Enter
            // means send rather than pick a completion.
            assert!(text.starts_with(&format!("$rf:{expect} ")), "got {text:?}");
        }
    }

    #[test]
    fn a_pane_that_looks_busy_asks_before_typing_and_can_be_overruled() {
        // A stale chooser left in an agent's scrollback used to stop every
        // Ask, silently and with no way through: the daemon refused and
        // nothing drew the refusal.
        let mut a = recorded(Sent::ReadyToSend);
        a.input(0, b"Do you want to proceed?\r\n").expect("write");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.waiting(0).is_none() {
            a.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(a.waiting(0).is_some(), "the fixture has to look busy for this test");

        // Straight to the typing step; what composes the prompt is the send
        // path's own test.
        a.do_inject(&Pending {
            staged: "pnd_1".into(),
            pane: 0,
            payload: b"anything".to_vec(),
            what: "send".into(),
            why: Vec::new(),
        });
        let Some(Modal::SendAnyway { evidence, .. }) = a.modal.clone() else {
            panic!("it must say what it read, got {:?}", a.modal)
        };
        assert!(evidence.contains("Do you want to proceed?"), "{evidence}");

        // And the cancel half is the default, because answering the agent is
        // usually the right thing.
        assert_eq!(a.modal_choice, 1);
        press(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none(), "{:?}", a.modal);
    }

    #[test]
    fn cancelling_a_confirmation_types_nothing() {
        let mut a = recorded(Sent::TakenByAgent);
        press(&mut a, KeyCode::Char('e'));
        assert!(matches!(a.modal, Some(Modal::Confirm(_))));
        press(&mut a, KeyCode::Left); // [←] CANCEL
        assert!(a.modal.is_none());
        assert_eq!(a.focus, Focus::Weft, "cancelling never moves you into the agent");
    }

    #[test]
    fn confirming_types_it_and_puts_you_in_the_agent() {
        let mut a = recorded(Sent::TakenByAgent);
        press(&mut a, KeyCode::Char('e'));
        press(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none());
        assert_eq!(a.focus, Focus::Agent, "you land where the work is happening");
    }

    #[test]
    fn an_unavailable_action_explains_itself_in_the_hint_and_opens_nothing() {
        let mut a = app();
        press(&mut a, KeyCode::Char('e'));
        assert!(a.modal.is_none(), "never a dialog: {:?}", a.modal);
        assert_eq!(a.hint_text(), Some("Nothing to work on yet."));
    }

    #[test]
    fn proceed_does_the_step_the_selected_row_is_actually_waiting_on() {
        // A confirmed Ask nobody has submitted: there is a step to take, so
        // the verb is live. What it then types is the send path's own test.
        let a = recorded(Sent::ReadyToSend);
        assert_eq!(a.unavailable(Act::Proceed), None);

        // An Ask that was compiled and never answered is waiting on the
        // harness's chooser, not on the person, so there is nothing to carry.
        let mut a = recorded(Sent::NotSent);
        press(&mut a, KeyCode::Char('p'));
        assert!(a.modal.is_none(), "{:?}", a.modal);
        assert_eq!(a.hint_text(), Some("health endpoint is closed. [F] FOLLOW UP asks again."));
    }

    #[test]
    fn a_unit_whose_agent_is_not_open_is_not_silently_dropped() {
        let mut a = app();
        let mut u = unit(Sent::Arrived { exact: true });
        u.harness = "claude-code".into();
        a.set_units(vec![u]);
        press(&mut a, KeyCode::Char('e'));
        let Some(Modal::PickUp { harness, then, .. }) = a.modal.clone() else {
            panic!("the missing agent is offered, not refused: {:?}", a.modal)
        };
        assert_eq!((harness.as_str(), then), ("claude-code", Some(Act::Eval)));
    }

    #[test]
    fn enter_unfolds_what_is_closed_and_acts_on_what_is_open() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        // The selection lands on the work, so the first `[Enter]` opens it.
        assert!(matches!(a.rows()[a.selected], Row::Action { .. }));
        press(&mut a, KeyCode::Enter);
        assert!(a.detail().is_some(), "an action opens its detail view");
        press(&mut a, KeyCode::Left);
        assert!(a.detail().is_none());

        // On a project it only ever folds and unfolds. Nothing in the sidebar
        // removes anything on `[Enter]`.
        a.selected = 0;
        assert!(matches!(a.rows()[0], Row::Project { folded: false, .. }));
        press(&mut a, KeyCode::Enter);
        assert!(matches!(a.rows()[0], Row::Project { folded: true, .. }));
        assert_eq!(a.rows().len(), 1, "folded, it hides its harnesses");
        press(&mut a, KeyCode::Enter);
        assert!(matches!(a.rows()[0], Row::Project { folded: false, .. }));
    }

    #[test]
    fn back_folds_the_level_rather_than_quitting_anything() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        a.selected = 0;
        press(&mut a, KeyCode::Left);
        assert!(a.rows()[0].folded());
        assert!(!a.quit);
    }

    #[test]
    fn folding_stays_folded_when_the_selection_moves() {
        // A fold is a decision about the tree, not about the cursor. It used
        // to be undone by moving away, which made it useless for tidying.
        let mut a = app();
        let mut second = unit(Sent::NotSent);
        second.ask_id = "ask_2".into();
        a.set_units(vec![unit(Sent::NotSent), second]);
        a.selected = 0;
        press(&mut a, KeyCode::Enter);
        assert!(a.rows()[0].folded());
        press(&mut a, KeyCode::Down);
        assert!(a.rows()[0].folded(), "still folded after moving");
    }

    #[test]
    fn the_verb_is_live_when_there_is_a_step_and_dim_when_there_is_not() {
        // A row with a dot can always be carried forward. A sent and unjudged
        // row carries no dot — nothing is kept waiting by it — and can still
        // be carried, into its Eval. A closed one cannot be carried at all.
        let a = with_unit(Sent::ReadyToSend);
        assert!(a.selected_unit().expect("a unit").needs_you());
        assert_eq!(a.unavailable(Act::Proceed), None);

        let a = with_unit(Sent::Arrived { exact: true });
        assert!(!a.selected_unit().expect("a unit").needs_you());
        assert_eq!(a.unavailable(Act::Proceed), None, "its Eval is the step");

        let mut a = app();
        let mut sealed = unit(Sent::Arrived { exact: true });
        sealed.sealed = Some("accepted".into());
        a.set_units(vec![sealed]);
        assert!(a.unavailable(Act::Proceed).is_some_and(|s| s.contains("FOLLOW UP")));
    }

    #[test]
    fn two_projects_are_two_groups_and_neither_reads_the_other() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        let (other, _) = test_session("second");
        a.open_project(&other.to_string_lossy());
        assert_eq!(a.rows().iter().filter(|r| matches!(r, Row::Project { .. })).count(), 2);

        // The second project holds nothing, so it is a heading and no more.
        let second = a
            .rows()
            .iter()
            .position(|r| matches!(r, Row::Project { project: 1, .. }))
            .expect("the second project");
        a.selected = second;
        a.follow_the_selection();
        assert_eq!(a.at, 1);
        assert!(a.selected_unit().is_none(), "nothing has been asked for in it");

        // Selecting the first project's work moves the focus back, and an act
        // lands where you are looking.
        let work = a
            .rows()
            .iter()
            .position(|r| matches!(r, Row::Action { project: 0, .. }))
            .expect("the first project's work");
        a.selected = work;
        a.follow_the_selection();
        assert_eq!(a.at, 0);
        assert_eq!(a.selected_unit().map(|u| u.title.clone()), Some("health endpoint".into()));
    }

    #[test]
    fn a_project_closes_out_of_the_window_and_leaves_the_rest() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        let (other, _) = test_session("second");
        a.open_project(&other.to_string_lossy());
        assert_eq!(a.rows().iter().filter(|r| matches!(r, Row::Project { .. })).count(), 2);

        let second = a
            .rows()
            .iter()
            .position(|r| matches!(r, Row::Project { project: 1, .. }))
            .expect("the second project");
        a.selected = second;
        press(&mut a, KeyCode::Backspace);
        assert!(matches!(a.modal, Some(Modal::CloseProject { .. })), "it asks first");
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.rows().iter().filter(|r| matches!(r, Row::Project { .. })).count(), 1);
        assert_eq!(a.selected_unit().map(|u| u.title.clone()), Some("health endpoint".into()));
    }

    #[test]
    fn the_last_project_is_not_closed_out_from_under_you() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        a.selected = 0;
        press(&mut a, KeyCode::Backspace);
        assert!(a.modal.is_none(), "{:?}", a.modal);
        assert_eq!(
            a.hint_text(),
            Some("This is the only project open. [X] QUIT closes the window.")
        );
    }

    #[test]
    fn arrows_move_through_every_level_of_the_tree() {
        let mut a = app();
        let mut second = unit(Sent::NotSent);
        second.ask_id = "ask_2".into();
        second.title = "second".into();
        a.set_units(vec![unit(Sent::NotSent), second]);
        // project, harness, two actions
        assert_eq!(a.rows().len(), 4);
        a.selected = 0;
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

        let blocked = a.unavailable(Act::Eval).expect("codex is not set up");
        assert!(blocked.contains("codex is not set up"), "{blocked}");
        // The two units hang under different harnesses, so reaching the
        // second means walking past its harness heading.
        let claude = a
            .rows()
            .iter()
            .position(
                |r| matches!(r, Row::Action { unit, .. } if a.units()[*unit].harness == "claude-code"),
            )
            .expect("the claude-code row");
        a.selected = claude;
        assert_eq!(a.unavailable(Act::Eval), None, "claude-code is ready");
    }

    #[test]
    fn the_ringframe_view_runs_nothing_before_proceed() {
        let mut a = app();
        press(&mut a, KeyCode::Char('u'));
        assert_eq!(a.modal, Some(Modal::RingFrame));
        // Until the daemon has said what is behind, there is nothing to run.
        press(&mut a, KeyCode::Char('p'));
        assert!(a.sync_view().is_none());
        press(&mut a, KeyCode::Left);
        assert!(a.modal.is_none());
    }

    #[test]
    fn a_key_in_the_weft_menu_does_what_it_names() {
        let mut a = app();
        press(&mut a, KeyCode::Char('w'));
        assert_eq!(a.modal, Some(Modal::Weft));
        press(&mut a, KeyCode::Char('o'));
        assert!(matches!(a.modal, Some(Modal::OpenProject { .. })), "{:?}", a.modal);
    }

    #[test]
    fn a_name_eval_json_does_not_know_is_said_once() {
        let mut a = app();
        a.session_mut().routing.ignored = vec!["codex.judge".into()];
        a.take_the_board();
        assert_eq!(
            a.hint_text(),
            Some("eval.json names something Weft does not know (codex.judge). It is ignored.")
        );
    }

    #[test]
    fn a_harness_weft_cannot_ask_is_unknown_rather_than_missing() {
        let mut a = app();
        a.set_readiness("codex", Readiness::Unknown);
        let why = a.unavailable(Act::Ask).expect("not ready");
        assert!(why.contains("could not ask"), "{why}");
        assert!(!why.contains("[U]PDATE"), "nothing to offer: {why}");
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
        press(&mut a, KeyCode::Char('b'));
        assert!(!a.show_work());
        press(&mut a, KeyCode::Char('b'));
        assert!(a.show_work());
    }

    #[test]
    fn space_goes_to_the_ask_that_needs_you() {
        // The thing that needs you is an Ask, and its detail view is where
        // you act on it, so one key goes the whole way.
        let mut a = app();
        let mut ready = unit(Sent::ReadyToSend);
        ready.ask_id = "ask_2".into();
        ready.title = "readme fix".into();
        a.set_units(vec![unit(Sent::TakenByAgent), ready]);
        press(&mut a, KeyCode::Char(' '));
        assert_eq!(a.selected_unit().map(|u| u.title.clone()), Some("readme fix".into()));
    }

    #[test]
    fn space_falls_back_to_the_newest_ask_when_nothing_needs_you() {
        let mut a = app();
        let mut older = unit(Sent::TakenByAgent);
        older.title = "older".into();
        let mut newest = unit(Sent::TakenByAgent);
        newest.ask_id = "ask_2".into();
        newest.title = "newest".into();
        a.set_units(vec![older, newest]);
        press(&mut a, KeyCode::Char(' '));
        assert_eq!(a.selected_unit().map(|u| u.title.clone()), Some("newest".into()));
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
        press(&mut a, KeyCode::Char('y'));
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
            gathered: None,
            sealed: None,
            seal_id: None,
            sealed_at: None,
        }]);
        a.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)).expect("key");
        assert!(
            a.modal.is_some() || a.hint_text().is_some(),
            "S must do something when the bar offers it"
        );
    }
}

#[cfg(test)]
mod picker_tests {
    use super::tests::press;
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn the_agent_picker_moves_and_wraps() {
        let mut a = super::tests::app();
        let choices = a.fresh_starts().len();
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
}

/// Readiness as the daemon words it.
fn readiness_of(word: Option<&str>) -> Readiness {
    match word {
        Some("ready") => Readiness::Ready,
        Some("no_cli") => Readiness::Missing(Gap::Cli),
        Some("no_marketplace") => Readiness::Missing(Gap::Marketplace),
        Some("no_plugin") => Readiness::Missing(Gap::Plugin),
        _ => Readiness::Unknown,
    }
}

/// A refusal from the daemon, as one sentence for the person.
fn said(e: &anyhow::Error) -> String {
    let text = e.to_string();
    text.split_once(": ").map(|(_, rest)| rest.to_string()).unwrap_or(text)
}
