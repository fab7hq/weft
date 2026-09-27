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
    MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::DefaultTerminal;

use crate::client::Session;
use crate::encode;
use crate::keys::{self, Action, Chord, Focus, Key, Toggle};
use crate::ledger::Unit;
use crate::theme::Theme;
pub use weft_core::board::Act;
use weft_core::board::{Board, Next, PaneInfo};
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

/// What a click can land on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A row of `rows()`.
    Row(usize),
    /// An agent's tab, by pane.
    Tab(usize),
    /// The turbo switch in the title bar.
    Turbo,
}

/// Where the last frame drew each thing a click can land on, as the area it
/// covers. Written by `ui::draw` where it draws them, read by `on_mouse`.
pub type Hits = Vec<(ratatui::layout::Rect, Target)>;

/// One project open in this window: its name, its root, and the client
/// connection that watches it. The daemon gives a connection one project to
/// watch, so a window with two projects holds two connections.
pub struct Open {
    pub name: String,
    pub root: PathBuf,
    pub session: Session,
    /// Each pane's state, by pane id, when the person was last told about
    /// it. `None` until first seen, so what was already so is never
    /// announced.
    heard: Option<std::collections::HashMap<u32, Option<weft_core::turns::Turn>>>,
}

impl Open {
    fn new(root: PathBuf, session: Session) -> Self {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into());
        Open { name, root, session, heard: None }
    }
}

/// The focused project's connection. A macro rather than a method so that it
/// expands to a field access and borrows like one.
macro_rules! sess {
    ($self:expr) => {
        $self.projects[$self.at].session
    };
}

mod input;
mod notices;
#[cfg(test)]
pub(crate) mod tests;

pub use notices::notification;

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
pub const WEFT_MENU: [(char, &str, Act); 6] = [
    ('O', "Open project", Act::OpenProject),
    ('N', "New agent", Act::NewAgent),
    ('B', "Toggle Sidebar", Act::ToggleSidebar),
    ('T', "Turbo mode on or off", Act::Turbo),
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

/// The surfaces that interrupt, each a panel anchored to the bottom.
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    Quit,
    Help,
    /// The agent has not reported that it is ready, which is the only state
    /// Weft types into on its own: `why` says what it reported instead.
    /// Typing is the person's to allow, and the default.
    SendAnyway {
        pending: Pending,
        why: &'static str,
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
}

/// How long the loop waits when nothing wakes it.
pub const TICK: Duration = Duration::from_millis(250);

/// The shortest time between two frames: at most 60 a second.
pub const FRAME: Duration = Duration::from_micros(16_667);

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
    announced: std::collections::HashSet<u32>,
    /// What a click can land on, as the last frame drew it.
    pub hits: Hits,
    /// What could be started here, as of the last time anyone asked.
    starts: Vec<Start>,
    /// Which harness takes which act here. Read once: a person edits the file
    /// by hand, and a restart is a fair price for that.
    routing: weft_core::routing::Routing,
    /// The one channel the loop waits on: the terminal and every project's
    /// connection send into it.
    wakes: std::sync::mpsc::Sender<crate::client::Wake>,
    woken: std::sync::mpsc::Receiver<crate::client::Wake>,
    /// When the last frame was drawn.
    drawn: std::time::Instant,
    /// The calls whose answers are awaited, by call id, and what each is for.
    asked: std::collections::HashMap<u64, Then>,
    /// The detail view being read, until every part is in.
    reading: Option<Reading>,
    /// The sidebar's rows, built once for the frame being drawn: nothing a
    /// frame draws changes what they are built from.
    listed: std::cell::RefCell<Option<std::rc::Rc<Vec<Row>>>>,
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

    /// Why RingFrame would not record the last send that went, if it would
    /// not.
    pub fn unrecorded(&self) -> Option<String> {
        sess!(self).unrecorded.clone()
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

    /// Whether a pane's agent is asking its person something, as its hooks
    /// reported it. Weft reads nothing off the screen (ADR-0013).
    pub fn waiting(&self, pane: usize) -> bool {
        weft_core::turns::is_asking(self.pane_turn(pane))
    }

    /// The harness a command line's program starts, by its profile.
    pub fn harness_for_program(&self, program: &str) -> Option<String> {
        sess!(self).harnesses.for_program(program).map(|h| h.name.clone())
    }

    /// Whether agents started in this project from now on run in turbo mode.
    pub fn turbo(&self) -> bool {
        sess!(self).turbo
    }

    /// Switch turbo mode for the agents started next. An agent already
    /// running keeps the mode it was started in: a harness takes its flags
    /// only when it starts.
    fn toggle_turbo(&mut self) {
        let on = !self.turbo();
        match sess!(self).set_turbo(on) {
            Ok(id) => self.await_answer(id, Then::Turbo),
            Err(e) => self.say(format!("Weft could not switch turbo mode: {e}")),
        }
    }

    /// Turbo mode switched, as the daemon answered.
    fn turbo_switched(
        &mut self,
        at: usize,
        answer: std::result::Result<serde_json::Value, String>,
    ) {
        let on = match answer {
            Ok(v) => v.get("turbo").and_then(|t| t.as_bool()).unwrap_or(false),
            Err(e) => return self.say(format!("Weft could not switch turbo mode: {}", said(&e))),
        };
        self.projects[at].session.turbo = on;
        self.say(if on {
            "Turbo mode on: the next agents you start get every permission and ask nothing. \
             Agents already running keep their mode."
        } else {
            "Turbo mode off: the next agents you start ask as they normally do. \
             Agents already running keep their mode."
        });
    }

    /// Whether any agent of this harness in a project runs in turbo mode.
    pub fn harness_turbo(&self, project: usize, harness: &str) -> bool {
        self.projects[project]
            .session
            .panes
            .iter()
            .any(|p| p.running && p.turbo && p.harness == harness)
    }

    /// Whether a pane's agent is running in turbo mode.
    pub fn pane_turbo(&self, pane: usize) -> bool {
        sess!(self).panes.get(pane).is_some_and(|p| p.running && p.turbo)
    }

    /// A pane's agent state, as its hooks reported it. None is not ready.
    pub fn pane_turn(&self, pane: usize) -> Option<weft_core::turns::Turn> {
        sess!(self).panes.get(pane).and_then(|p| p.turn)
    }

    /// The agent state of a unit of work, from the session the ledger names
    /// for it.
    pub fn unit_turn(&self, project: usize, unit: &Unit) -> Option<weft_core::turns::Turn> {
        let turns = if project == self.at {
            &sess!(self).turns
        } else {
            &self.projects[project].session.turns
        };
        weft_core::turns::state_of(unit, turns)
    }

    /// A project's agents asking their person for input, of one harness or
    /// of all of them. This is NEEDS YOU, and nothing else is.
    pub fn agents_waiting(&self, project: usize, harness: Option<&str>) -> usize {
        let (panes, states) = facts_of(&self.projects[project].session);
        weft_core::turns::asking(&panes, &states, harness)
    }

    /// The facts a decision reads, borrowed from the state that holds them.
    /// The decision itself is `weft_core::board`.
    fn with_board<T>(&self, f: impl FnOnce(Board<'_>) -> T) -> T {
        let (panes, agents) = facts_of(&sess!(self));
        f(Board {
            units: &self.units,
            selected: self.selected_index().unwrap_or(usize::MAX),
            panes: &panes,
            focused: self.pane_focus,
            readiness: &|h| self.readiness(h),
            routing: &self.routing,
            workspace_gap: self.workspace_gap.as_deref(),
            agents: &agents,
        })
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
    /// While a frame is drawn, the ones built for it.
    pub fn rows(&self) -> std::rc::Rc<Vec<Row>> {
        if let Some(rows) = self.listed.borrow().as_ref() {
            return rows.clone();
        }
        std::rc::Rc::new(self.list())
    }

    /// Build the rows once for a frame, or let them go after it.
    pub fn hold_rows(&self, holding: bool) {
        *self.listed.borrow_mut() = holding.then(|| std::rc::Rc::new(self.list()));
    }

    fn list(&self) -> Vec<Row> {
        let mut out = Vec::new();
        for (at, p) in self.projects.iter().enumerate() {
            let units = self.units_of(at);
            let waiting = self.agents_waiting(at, None);
            // The agents running, as the title bar counts them.
            let open = weft_core::turns::open(&facts_of(&p.session).0);
            let folded = self.folded.contains(&p.name);
            out.push(Row::Project { project: at, name: p.name.clone(), folded, open, waiting });
            if folded {
                continue;
            }
            // Harnesses in the order they first appear, so the tree does not
            // reshuffle itself as work arrives.
            // An agent with nothing asked of it yet still has a row, so
            // there is somewhere to say it needs your input.
            let mut seen: Vec<&str> = Vec::new();
            let running = p.session.panes.iter().map(|pane| pane.harness.as_str());
            for name in units.iter().map(|u| u.harness.as_str()).chain(running) {
                if !seen.contains(&name) {
                    seen.push(name);
                }
            }
            for name in seen {
                let running =
                    p.session.panes.iter().any(|pane| pane.running && pane.harness == name);
                let open = units.iter().any(|u| u.harness == name && u.is_open());
                // Nothing to go back to: no agent, and nothing left open.
                if !running && !open {
                    continue;
                }
                let key = format!("{at}\u{0}{name}");
                let folded = self.folded.contains(&key);
                let waiting = self.agents_waiting(at, Some(name));
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

    /// Agents running, as the title bar's OPEN counts them.
    pub fn open_count(&self) -> usize {
        self.with_board(|b| b.open_count())
    }

    /// Agents in this project asking their person for input.
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

    pub fn with_session(root: PathBuf, toggle: Toggle, mut session: Session) -> Self {
        let (wakes, woken) = std::sync::mpsc::channel();
        session.wake(wakes.clone());
        Self {
            wakes,
            woken,
            drawn: std::time::Instant::now(),
            asked: Default::default(),
            reading: None,
            listed: Default::default(),
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
            hits: Hits::default(),
            starts: Vec::new(),
            routing: Default::default(),
            newer: Default::default(),
        }
    }

    /// `spec` is the command line the person would have typed, e.g.
    /// `claude --model sonnet --effort medium`. Weft never chooses the model:
    /// that is the harness's configuration and the person's decision.
    ///
    /// The pane appears when the daemon says it has one, and what the harness
    /// is short of a moment later: asking it costs a process, so the daemon
    /// does that off its own loop.
    pub fn add(&mut self, harness: &str, spec: &str) -> Result<()> {
        self.start(harness, spec, None, Then::Started { go: false, then: None })
    }

    /// Start an agent, resuming `session` when it names one, and do `then`
    /// once the daemon says it has.
    fn start(
        &mut self,
        harness: &str,
        spec: &str,
        session: Option<&str>,
        then: Then,
    ) -> Result<()> {
        let id = sess!(self).spawn(harness, spec, session)?;
        self.await_answer(id, then);
        Ok(())
    }

    fn await_answer(&mut self, call: u64, then: Then) {
        self.asked.insert(call, then);
    }

    /// Take in answers until none is awaited, for a probe or a test that
    /// drives the keys itself. The loop never does this: it acts on each
    /// answer as it arrives.
    pub fn settle(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !self.asked.is_empty() && std::time::Instant::now() < deadline {
            let _ = self.turn(Duration::from_millis(20));
        }
    }

    /// Do what an answer was awaited for.
    fn answered(
        &mut self,
        at: usize,
        then: Then,
        answer: std::result::Result<serde_json::Value, String>,
    ) {
        match then {
            Then::Confirm => self.confirm_staged(at, answer),
            Then::Read(part) => self.read_part(part, answer),
            Then::Turbo => self.turbo_switched(at, answer),
            Then::Starts(opening) => {
                self.take_starts(answer.ok());
                match opening {
                    Opening::Nothing => {}
                    Opening::Picker => self.open_agent_picker(),
                    Opening::PickUp { harness, then } => self.show_pick_up(harness, then),
                }
            }
            Then::Started { go, then } => match answer {
                Err(e) => {
                    self.modal = Some(Modal::Note(format!("Could not start it: {}", said(&e))))
                }
                Ok(v) => {
                    self.take_the_board();
                    let pane = v.get("pane").and_then(|p| p.as_u64()).map(|p| p as u32);
                    let Some(i) = pane.and_then(|id| self.projects[at].session.index_of(id)) else {
                        return;
                    };
                    if go && at == self.at {
                        self.pane_focus = i;
                        match then {
                            Some(act) => self.act(act),
                            None => self.focus = Focus::Agent,
                        }
                    }
                }
            },
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.take_the_board();
        self.look_for_agents(Opening::Nothing);
        // The mouse is Weft's: a click selects a row or goes to an agent,
        // and while an agent has the keys the wheel scrolls that agent. A click never reaches an agent.
        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
        // The terminal is read on its own thread, into the one channel.
        let keys = self.wakes.clone();
        std::thread::spawn(move || {
            while let Ok(e) = event::read() {
                if keys.send(crate::client::Wake::Terminal(e)).is_err() {
                    return;
                }
            }
        });
        terminal.draw(|frame| crate::ui::draw(frame, &mut self))?;
        while !self.quit {
            self.frame(terminal, TICK)?;
        }
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        Ok(())
    }

    /// A way to wake this window's loop, for news from outside it.
    pub fn waker(&self) -> std::sync::mpsc::Sender<crate::client::Wake> {
        self.wakes.clone()
    }

    /// Wait up to `within` for something to change; when it does, take in
    /// everything that arrives before the next frame is due, and draw that
    /// frame. True when one was drawn.
    pub fn frame<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut ratatui::Terminal<B>,
        within: Duration,
    ) -> Result<bool>
    where
        B::Error: Send + Sync + 'static,
    {
        if !self.turn(within)? {
            return Ok(false);
        }
        let due = self.drawn + FRAME;
        while let Some(left) = due.checked_duration_since(std::time::Instant::now())
            && !self.quit
        {
            self.turn(left)?;
        }
        terminal.draw(|frame| crate::ui::draw(frame, self))?;
        self.drawn = std::time::Instant::now();
        Ok(true)
    }

    /// One turn of the loop: wait up to `tick` for something to wake it,
    /// then take in that and everything that arrived behind it. True when
    /// anything did.
    pub fn turn(&mut self, tick: Duration) -> Result<bool> {
        let mut woke: Vec<_> = self.woken.recv_timeout(tick).into_iter().collect();
        woke.extend(self.woken.try_iter());
        let changed = !woke.is_empty();
        for wake in woke {
            match wake {
                crate::client::Wake::Terminal(Event::Key(key))
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    self.on_key(key)?
                }
                crate::client::Wake::Terminal(Event::Paste(text)) => self.on_paste(&text)?,
                crate::client::Wake::Terminal(Event::Mouse(m)) => self.on_mouse(m),
                _ => {}
            }
        }
        if self.pump_all() {
            self.take_the_board();
        }
        self.notice_an_agent_that_ended();
        let told = self.notices();
        if !told.is_empty() {
            use std::io::Write as _;
            let program = std::env::var("TERM_PROGRAM").ok();
            let mut out = std::io::stdout();
            for said in told {
                let _ = out.write_all(&notification(program.as_deref(), &said));
            }
            let _ = out.flush();
        }
        Ok(changed)
    }

    /// Take a pane away. Every other pane keeps its id, so nothing waits for
    /// the daemon's new list; the keys come back to Weft.
    fn close_pane(&mut self, pane: usize) {
        let _ = sess!(self).close(pane);
        self.focus = Focus::Weft;
    }

    /// Offer to start the harness this work needs: the session the selected
    /// Ask was asked in, else the harness's latest on record, else fresh.
    /// Offered once the daemon has said what could be started.
    fn offer_pick_up(&mut self, harness: String, then: Option<Act>) {
        self.look_for_agents(Opening::PickUp { harness, then });
    }

    fn show_pick_up(&mut self, harness: String, then: Option<Act>) {
        let own = self.selected_unit().filter(|u| u.harness == harness).and_then(|u| {
            let id = u.session_ref.clone()?;
            let at = weft_core::turns::millis(&u.asked_at).unwrap_or_default();
            Some(Recorded { id, at, last: u.title.clone() })
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
        let Some(h) = sess!(self).harnesses.find(harness).cloned() else {
            self.modal = Some(Modal::Note(format!("Weft does not know how to start {harness}.")));
            return;
        };
        let spec = match session {
            Some(s) => h.resume_spec(&s.id),
            None => h.program.clone(),
        };
        let resumes = session.map(|s| s.id.as_str());
        if let Err(e) = self.start(harness, &spec, resumes, Then::Started { go: true, then }) {
            self.modal = Some(Modal::Note(format!("Could not start {spec}: {e}")));
        }
    }

    /// Take the board the daemon read, and notice anything it implies. The
    /// record is followed once, in the daemon, and every client is told.
    /// Take in what every project's connection has sent, so a board in the
    /// background is as current as the one on screen. The agent in front
    /// stays in front when one before it closes.
    /// Every answer that arrived is acted on here.
    fn pump_all(&mut self) -> bool {
        let focused = sess!(self).panes.get(self.pane_focus).map(|p| p.id);
        let mut changed = false;
        let mut answers = Vec::new();
        for (at, p) in self.projects.iter_mut().enumerate() {
            changed |= p.session.pump();
            answers.extend(p.session.answers().into_iter().map(|(id, a)| (at, id, a)));
        }
        match focused.and_then(|id| sess!(self).index_of(id)) {
            Some(i) => self.pane_focus = i,
            None => self.pane_focus = self.pane_focus.min(self.pane_count().saturating_sub(1)),
        }
        for (at, id, answer) in answers {
            if let Some(then) = self.asked.remove(&id) {
                self.answered(at, then, answer);
            }
        }
        changed
    }

    fn take_the_board(&mut self) {
        self.pump_all();
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
                "config.toml names something Weft does not know ({said}). It is ignored."
            ));
        }
        if !sess!(self).unread.is_empty() && self.hint.is_none() {
            let said = sess!(self).unread.join(", ");
            self.say(format!(
                "Weft could not read the harness file of {said} in ~/.fab7/weft/harnesses/; \
                 it is not offered."
            ));
        }
        if !self.routing.leftover.is_empty() && self.hint.is_none() {
            let said = self.routing.leftover.join(", ");
            self.say(format!(
                "Weft no longer reads {said}; move what it holds into ~/.fab7/weft/config.toml."
            ));
        }
    }

    /// Take in what the daemon says until it has said what the record holds,
    /// and what could be started here. The run loop takes it in as it
    /// arrives; a probe or a test does this once.
    pub fn refresh_for_test(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && sess!(self).units.is_empty() {
            let _ = self.turn(Duration::from_millis(20));
        }
        self.take_the_board();
        self.look_for_agents(Opening::Nothing);
        self.settle();
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
        // Weft types on its own only into an agent that said it is ready
        // (ADR-0013). Anything else is the person's to allow; sending is the
        // default, since they asked for it a key ago.
        if let Err(why) = weft_core::turns::may_type(self.pane_turn(pending.pane)) {
            self.modal = Some(Modal::SendAnyway { pending: pending.clone(), why });
            self.modal_choice = 0;
            return;
        }
        self.do_inject_forcing(pending, false)
    }

    /// The agent had not said it was ready, and the person said to type
    /// anyway: the decision is theirs.
    fn do_inject_forcing(&mut self, pending: &Pending, force: bool) {
        match sess!(self).resolve(&pending.staged, true, force) {
            Ok(()) => {
                self.modal = None;
                // The detail view blocks the screen; the person goes where the
                // keys went, into the agent.
                self.detail = None;
                self.focus = Focus::Agent;
                self.pane_focus = pending.pane;
            }
            Err(e) => self.modal = Some(Modal::Note(said(&e.to_string()))),
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
        // Asked and not composed yet: what the person asked is what there is.
        let part = if unit.requested_only { "source" } else { "wording" };
        let mut reads = vec![(Part::Prompt, part, unit.ask_id.clone())];
        if let Some(c) = &unit.check {
            reads.push((Part::Record, "judges", c.eval_id.clone()));
        }
        if let Some(id) = &unit.seal_id {
            reads.push((Part::Seal, "seal", id.clone()));
        }
        let mut left = 0;
        for (which, what, id) in reads {
            if let Ok(call) = sess!(self).read(what, &id) {
                self.await_answer(call, Then::Read(which));
                left += 1;
            }
        }
        self.reading = Some(Reading { unit, prompt: None, record: None, seal: None, left });
        if left == 0 {
            self.finish_reading();
        }
    }

    /// One part of the detail view, read; once every part is in, the view
    /// opens. A part that did not come back is said in its section.
    fn read_part(&mut self, part: Part, answer: std::result::Result<serde_json::Value, String>) {
        let Some(r) = self.reading.as_mut() else { return };
        r.left = r.left.saturating_sub(1);
        if let Ok(v) = answer {
            match part {
                Part::Prompt => {
                    r.prompt = v.get("text").and_then(|t| t.as_str()).map(str::to_string)
                }
                Part::Record => r.record = serde_json::from_value(v).ok(),
                Part::Seal => r.seal = Some(v),
            }
        }
        if r.left == 0 {
            self.finish_reading();
        }
    }

    fn finish_reading(&mut self) {
        let Some(r) = self.reading.take() else { return };
        self.detail = Some(Detail {
            lines: weft_core::offers::detail_read(
                &r.unit,
                r.prompt.as_deref(),
                r.record.as_ref(),
                r.seal.as_ref(),
                self.deciding_harness(Act::Eval).as_deref(),
            ),
            title: r.unit.title.clone(),
            harness: r.unit.harness.clone(),
            offset: 0,
        });
    }

    /// Whether the selected Ask has a prompt still to send: the one thing
    /// `[P]ROCEED` does. Once it went, what comes next is the person's call.
    pub fn proceeds(&self) -> bool {
        self.selected_unit().is_some_and(|u| {
            matches!(weft_core::board::next_step(u), Some(Next::Send | Next::Confirm))
        })
    }

    /// Send the selected Ask's prompt.
    fn proceed(&mut self) {
        if self.guard(Act::Proceed) {
            self.start_send();
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

    /// Ask what could be started here, then open what was waiting on it.
    fn look_for_agents(&mut self, then: Opening) {
        match sess!(self).available() {
            Ok(call) => self.await_answer(call, Then::Starts(then)),
            Err(_) => self.answered(self.at, Then::Starts(then), Err(String::new())),
        }
    }

    fn take_starts(&mut self, answer: Option<serde_json::Value>) {
        let listed = answer.and_then(|a| a.get("starts").and_then(|v| v.as_array()).cloned());
        self.starts = listed
            .unwrap_or_default()
            .iter()
            .filter_map(|v| {
                Some(Start {
                    harness: v.get("harness")?.as_str()?.to_string(),
                    label: v.get("label")?.as_str()?.to_string(),
                    spec: v.get("spec")?.as_str()?.to_string(),
                    session: v.get("session").and_then(|s| {
                        Some(Recorded {
                            id: s.get("id")?.as_str()?.to_string(),
                            at: s.get("at")?.as_i64()?,
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
        self.look_for_agents(Opening::Picker);
    }

    fn open_agent_picker(&mut self) {
        if self.fresh_starts().is_empty() {
            let titles = sess!(self).harnesses.titles();
            self.say(format!("No coding agent found. Install {titles} first."));
            return;
        }
        self.modal = Some(Modal::StartAgent { choice: 0 });
        self.modal_choice = 0;
    }

    pub fn start_chosen_agent(&mut self, choice: usize) {
        let Some(start) = self.fresh_starts().get(choice).cloned() else { return };
        self.modal = None;
        let then = Then::Started { go: true, then: None };
        if let Err(e) = self.start(&start.harness, &start.spec, None, then) {
            self.modal = Some(Modal::Note(format!("Could not run {}: {e}", start.spec)));
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
            Ok(call) => self.await_answer(call, Then::Confirm),
            Err(e) => self.modal = Some(Modal::Note(said(&e.to_string()))),
        }
    }

    /// What an act staged, in front of the person, once the daemon says so.
    fn confirm_staged(
        &mut self,
        at: usize,
        answer: std::result::Result<serde_json::Value, String>,
    ) {
        let staged = match answer {
            Ok(v) => v.get("pending").and_then(|p| p.as_str()).map(str::to_string),
            Err(e) => return self.modal = Some(Modal::Note(said(&e))),
        };
        let s = &self.projects[at].session;
        let Some(w) = s.waiting.iter().find(|w| Some(&w.id) == staged.as_ref()).cloned() else {
            return;
        };
        let Some(pane) = s.index_of(w.pane) else { return };
        self.modal = Some(Modal::Confirm(Pending {
            staged: w.id,
            pane,
            payload: w.payload,
            what: w.what,
            why: w.why.lines().map(str::to_string).collect(),
        }));
        self.modal_choice = 0;
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
        self.select_row(((self.selected as i32 + delta as i32).rem_euclid(n)) as usize);
    }

    /// Select one row of the list, the way the arrows arrive at it.
    fn select_row(&mut self, row: usize) {
        self.selected = row;
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
            Ok(mut session) => {
                session.wake(self.wakes.clone());
                self.projects.push(Open::new(root, session));
                self.at = self.projects.len() - 1;
                self.pane_focus = 0;
                self.selected = 0;
                self.take_the_board();
                self.look_for_agents(Opening::Nothing);
            }
            Err(e) => self.modal = Some(Modal::Note(said(&e.to_string()))),
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

    /// The next agent asking for input: in this project first, then in the
    /// next project that has one. Nothing else is NEEDS YOU.
    fn next_needs_you(&mut self) {
        if let Some(pane) = self.next_waiting_pane() {
            self.go_to_pane(pane);
            return;
        }
        let n = self.projects.len();
        let elsewhere = (1..n).map(|step| (self.at + step) % n).find_map(|at| {
            let s = &self.projects[at].session;
            let pane = s.panes.iter().position(|p| weft_core::turns::is_asking(p.turn));
            pane.map(|pane| (at, pane))
        });
        let Some((at, pane)) = elsewhere else {
            self.say("Nothing needs your input.");
            return;
        };
        self.at = at;
        self.take_the_board();
        self.go_to_pane(pane);
    }

    /// Show a pane, with its harness's row picked in the sidebar: the row
    /// that carries the badge, and the project any act then lands in.
    fn go_to_pane(&mut self, pane: usize) {
        if let Some(name) = self.harness_at(pane).map(str::to_string) {
            let at = self.at;
            // Folded away, the row would not be there to pick: open to it.
            self.folded.remove(&self.projects[at].name);
            let row = self.rows().iter().position(
                |r| matches!(r, Row::Harness { project, name: n, .. } if *project == at && *n == name),
            );
            if let Some(row) = row {
                self.selected = row;
            }
        }
        self.pane_focus = pane;
        self.show_work = false;
        self.detail = None;
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
            if self.waiting(i) {
                return Some(i);
            }
        }
        (!showing_pane && self.waiting(self.pane_focus)).then_some(self.pane_focus)
    }

    /// Why Weft says an agent is waiting: its own hook said so. Nothing on
    /// the screen is read.
    fn explain_waiting(&mut self) {
        let harness = self.harness_at(self.pane_focus).unwrap_or("the agent").to_string();
        self.say(if self.waiting(self.pane_focus) {
            format!("{harness} reported, through its hook, that it is asking you something.")
        } else {
            format!("{harness} has not reported that it is asking you anything.")
        });
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
            Act::Turbo => self.toggle_turbo(),
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

    /// Whether the agent on screen is the thing waiting for an answer.
    pub fn waiting_here(&self) -> bool {
        !self.show_work && self.waiting(self.pane_focus)
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

/// What to do with a call's answer, when it arrives.
enum Then {
    /// Put what the act staged in front of the person.
    Confirm,
    /// One part of the detail view being read.
    Read(Part),
    /// Turbo mode was switched.
    Turbo,
    /// What could be started here is known: open what was waiting on it.
    Starts(Opening),
    /// An agent started: go to it, when asked to, then carry on.
    Started { go: bool, then: Option<Act> },
}

/// A part of the detail view.
#[derive(Clone, Copy)]
enum Part {
    Prompt,
    Record,
    Seal,
}

/// What was waiting on the list of agents that could be started.
enum Opening {
    Nothing,
    Picker,
    PickUp { harness: String, then: Option<Act> },
}

/// The detail view, while its parts are read.
struct Reading {
    unit: Unit,
    prompt: Option<String>,
    record: Option<weft_core::record::Record>,
    seal: Option<serde_json::Value>,
    left: usize,
}

/// One way to start an agent: a harness, and the command line that does it.
/// The command is shown before it runs and is the one a person would type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub harness: String,
    pub label: String,
    pub spec: String,
    /// The session this would pick up, when it picks one up.
    pub session: Option<Recorded>,
}

/// A project's panes as the rules read them, and each one's agent state.
fn facts_of(s: &Session) -> (Vec<PaneInfo>, Vec<Option<weft_core::turns::Turn>>) {
    let panes = s
        .panes
        .iter()
        .map(|p| PaneInfo {
            pane: p.id,
            harness: p.harness.clone(),
            spec: p.spec.clone(),
            running: p.running,
            turbo: p.turbo,
        })
        .collect();
    (panes, s.panes.iter().map(|p| p.turn).collect())
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
fn said(e: &str) -> String {
    let text = e.to_string();
    text.split_once(": ").map(|(_, rest)| rest.to_string()).unwrap_or(text)
}
