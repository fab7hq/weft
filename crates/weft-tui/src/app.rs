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

/// One notification, through the terminal Weft runs in: OSC 9 where the
/// terminal shows it, the bell where it shows none. Nothing leaves the
/// terminal (ADR-0017).
pub fn notification(term_program: Option<&str>, message: &str) -> Vec<u8> {
    const SHOWS_OSC_9: [&str; 3] = ["iTerm.app", "WezTerm", "ghostty"];
    if !term_program.is_some_and(|t| SHOWS_OSC_9.contains(&t)) {
        return b"\x07".to_vec();
    }
    let text: String = message.chars().filter(|c| !c.is_control()).collect();
    format!("\x1b]9;{text}\x07").into_bytes()
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

    /// What to tell the person, once per change: an agent they are not
    /// looking at started asking them something (ADR-0017).
    /// The agent in front is never announced, and nothing is said with
    /// `notify = false`.
    pub fn notices(&mut self) -> Vec<String> {
        use weft_core::turns;
        let mut out = Vec::new();
        let (at, focus) = (self.at, self.pane_focus);
        for (i, p) in self.projects.iter_mut().enumerate() {
            let now = p.session.panes.iter().map(|v| (v.id, v.turn)).collect();
            let Some(before) = p.heard.replace(now) else { continue };
            if !p.session.notify {
                continue;
            }
            for (pane, v) in p.session.panes.iter().enumerate() {
                // Only an agent asking for input: when a turn ends is the
                // person's workflow, not an alarm. A pane that just opened
                // was asking nothing before.
                let was = before.get(&v.id).copied().flatten();
                if !turns::is_asking(v.turn) || was == v.turn || (i == at && pane == focus) {
                    continue;
                }
                out.push(format!("{} in {}: needs your input", v.harness, p.name));
            }
        }
        out
    }

    /// An agent quit from inside — `Ctrl+D`, `/exit`, or it simply stopped.
    /// The pane is still on screen showing its last frame, which is worth
    /// keeping; what must not happen is keystrokes going to a dead process.
    pub fn notice_an_agent_that_ended(&mut self) {
        let panes = &sess!(self).panes;
        let Some(pane) = panes.iter().position(|p| !p.running && !self.announced.contains(&p.id))
        else {
            return;
        };
        self.announced.insert(panes[pane].id);
        let harness = self.harness_at(pane).unwrap_or("the agent").to_string();
        // The work is in the record, so the pane goes and the row says so.
        self.close_pane(pane);
        self.say(format!("{harness} has ended. Its work is still on the list."));
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
            None => h.spec(),
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
            Action::Turbo => self.act(Act::Turbo),
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

    /// A click selects, as the arrows would; the wheel, while an agent has
    /// the keys, scrolls its pane.
    pub fn on_mouse(&mut self, m: MouseEvent) {
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            return self.on_click(m.column, m.row);
        }
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
        let key = sess!(self).harnesses.find(&name).and_then(|h| h.transcript.clone());
        self.say(match key {
            Some(key) => {
                format!("{name} keeps its own history: press {key} in the agent to read it.")
            }
            None => format!("Nothing of {name}'s has scrolled away yet."),
        });
    }

    /// A click selects a row, as the arrows would, or goes to an agent, as
    /// its number would. It never types into an agent.
    fn on_click(&mut self, column: u16, line: u16) {
        if self.modal.is_some() || self.detail.is_some() {
            return;
        }
        let at = ratatui::layout::Position { x: column, y: line };
        let Some(&(_, target)) = self.hits.iter().find(|(area, _)| area.contains(at)) else {
            return;
        };
        match target {
            Target::Turbo => self.toggle_turbo(),
            Target::Row(row) => {
                self.focus = Focus::Weft;
                self.select_row(row);
            }
            Target::Tab(pane) => self.pane_focus = pane,
        }
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
        !self.show_work && self.waiting(self.pane_focus)
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

    pub(crate) fn ringframe_on_path() -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("ringframe").is_file())
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
            let _ = weftd::server::Session::serve_with(
                &listening,
                &listening.with_extension("no-config.toml"),
                Some(weft_core::harness::fixture::harnesses()),
            );
        });
        let session = connect_when_listening(&socket, &root);
        (root, session)
    }

    /// A daemon started on a thread listens a moment later.
    pub(crate) fn connect_when_listening(
        socket: &std::path::Path,
        root: &std::path::Path,
    ) -> crate::client::Session {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !weft_proto::is_live(socket) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        crate::client::Session::connect(socket, root, 24, 80).expect("connect")
    }

    pub(crate) fn app() -> App {
        let (root, session) = test_session("codex");
        let mut a = App::with_session(root, Toggle, session);
        a.add("codex", "/bin/cat").expect("spawn");
        a.settle();
        // Say what this fixture's readiness is instead of inheriting the
        // machine's. Otherwise these tests pass on a laptop with Codex and the
        // plugin installed and fail everywhere else, which is not a fact about
        // the code. Tests about readiness itself set their own.
        a.set_readiness("codex", Readiness::Ready);
        a.set_readiness("claude-code", Readiness::Ready);
        agent_says(&mut a, "codex", "fixture", "ready");
        a
    }

    /// Now, as a receipt writes it.
    fn now() -> String {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("t");
        weft_core::turns::stamp(ms.as_millis() as i64)
    }

    /// A hook's receipt, written where RingFrame's plugin writes it, now, and
    /// waited for until the daemon has read it.
    pub(crate) fn agent_says(a: &mut App, harness: &str, session: &str, event: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.session_mut().panes.is_empty() {
            a.session_mut().pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        let dir = a.root().join(".fab7/rf/sessions").join(harness).join(session);
        std::fs::create_dir_all(&dir).expect("session dir");
        let line = serde_json::json!({"event": event, "session_id": session,
                                      "time": now()});
        std::fs::write(dir.join("turns.jsonl"), format!("{line}\n")).expect("receipt");
        let want = weft_core::turns::Turn::recorded(event);
        while std::time::Instant::now() < deadline && a.pane_turn(0) != want {
            a.session_mut().pump();
            std::thread::sleep(Duration::from_millis(10));
        }
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
            arrived: None,
            attributed_at: None,
            gathered: None,
            sealed: None,
            seal_id: None,
            sealed_at: None,
            requested_only: false,
        }
    }

    /// A key, and whatever answers it brings.
    pub(crate) fn press(a: &mut App, code: KeyCode) {
        a.on_key(KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
        a.settle();
    }

    fn ctrl(a: &mut App, c: char) {
        a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)).expect("key");
        a.settle();
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

    /// The loop blocks on one channel that the terminal and the daemon both
    /// send into, so a key is handled the moment it arrives, not at the
    /// next look.
    #[test]
    fn a_key_reaches_the_agent_as_soon_as_it_is_pressed() {
        let mut a = app();
        a.focus = Focus::Agent;
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let began = std::time::Instant::now();
        a.wakes.send(crate::client::Wake::Terminal(Event::Key(key))).expect("sent");
        a.turn(Duration::from_secs(5)).expect("a turn");
        assert!(began.elapsed() < TICK / 5, "waited {:?}", began.elapsed());
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline
            && !a.pane_text(0).unwrap_or_default().contains('q')
        {
            a.turn(Duration::from_millis(50)).expect("a turn");
        }
        assert!(a.pane_text(0).unwrap_or_default().contains('q'), "the agent got it");
    }

    /// A frame is drawn only when something changed: none while nothing
    /// happens, and one for a burst of output that arrived before it.
    #[test]
    fn frames_are_drawn_for_what_changed_and_no_more() {
        let mut a = app();
        let mut t =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).expect("terminal");
        // Whatever the start-up still has to say, said.
        while a.frame(&mut t, Duration::from_millis(500)).expect("a frame") {}
        let (mut idle, end) = (0, std::time::Instant::now() + Duration::from_secs(1));
        while std::time::Instant::now() < end {
            idle += a.frame(&mut t, TICK).expect("a frame") as usize;
        }
        assert_eq!(idle, 0, "frames drawn over an idle second");

        let burst: String = (0..200).map(|i| format!("burst line {i}\n")).collect();
        a.input(0, burst.as_bytes()).expect("typed");
        std::thread::sleep(Duration::from_millis(400));
        let mut frames = 0;
        while a.frame(&mut t, TICK).expect("a frame") {
            frames += 1;
        }
        assert_eq!(frames, 1, "one frame for everything that had arrived");
        assert!(a.pane_text(0).unwrap_or_default().contains("burst line 199"));
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
        let known = Recorded { id: "01a0bdb6".into(), at: 0, last: String::new() };
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

    /// harness-profile.md §5.3: a made-up third harness, defined by a profile
    /// alone, is offered with no change to Weft.
    #[test]
    fn a_made_up_third_harness_is_offered_from_its_profile_alone() {
        let mut zed = weft_core::harness::fixture::profile("codex");
        zed["host"] = "zed-agent".into();
        zed["title"] = "Zed Agent".into();
        zed["program"] = "cat".into(); // on every machine's PATH
        let mut all = weft_core::harness::fixture::harnesses();
        all.0.push(weft_core::harness::Harness::from_profile(&zed).expect("a harness"));
        let (root, _) = test_session("zed");
        let socket = weft_proto::private_socket("weft-app-zed");
        let _ = std::fs::remove_file(&socket);
        let listening = socket.clone();
        std::thread::spawn(move || {
            let _ = weftd::server::Session::serve_with(
                &listening,
                &listening.with_extension("no-config.toml"),
                Some(all),
            );
        });
        let session = connect_when_listening(&socket, &root);
        let mut a = App::with_session(root, Toggle, session);
        a.look_for_agents(Opening::Nothing);
        a.settle();
        let zed: Vec<_> = a.starts().iter().filter(|s| s.harness == "zed-agent").collect();
        assert_eq!(zed.len(), 1, "offered fresh: {:?}", a.starts());
        assert_eq!(zed[0].spec, "cat");
        assert_eq!(a.harness_for_program("/bin/cat").as_deref(), Some("zed-agent"));
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
        let acts: String = pairs.iter().map(|(k, v)| format!("{k} = \"{v}\"\n")).collect();
        a.routing = weft_core::config::read(
            &format!("[routing]\n{acts}"),
            &weft_core::harness::fixture::harnesses(),
        )
        .routing(a.root());
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
    fn a_routed_act_with_no_pane_is_still_offered() {
        // The daemon starts a pane of the routed harness when none is free.
        let mut a = routed(&[("eval", "claude-code")]);
        a.set_readiness("claude-code", Readiness::Ready);
        assert_eq!(a.unavailable(Act::Eval), None);
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
    fn an_agent_asking_its_person_is_asked_about_first_and_can_be_overruled() {
        // Its hook said it is asking; typing now would answer it. Weft asks
        // first; sending is the default, and Cancel is one key
        // away.
        let mut a = recorded(Sent::ReadyToSend);
        a.session_mut().set_turns(vec![Some(weft_core::turns::Turn::Waiting)]);
        assert!(a.waiting(0));
        a.do_inject(&pending_for(0));
        let Some(Modal::SendAnyway { why, .. }) = a.modal.clone() else {
            panic!("it must ask first, got {:?}", a.modal)
        };
        assert_eq!(why, "the agent is asking you something");
        assert_eq!(a.modal_choice, 0, "Type it anyway is the default");
        press(&mut a, KeyCode::Down);
        assert_eq!(a.modal_choice, 1, "Cancel is one key away");
        press(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none(), "{:?}", a.modal);
    }

    fn pending_for(pane: usize) -> Pending {
        Pending {
            staged: "pnd_1".into(),
            pane,
            payload: b"anything".to_vec(),
            what: "send".into(),
            why: Vec::new(),
        }
    }

    /// turn-state.md §6 tests 3 and 4, on the panel: with no event, or with
    /// one that is not ready, Weft asks and says why; the person may still
    /// type it anyway.
    #[test]
    fn an_agent_that_has_not_said_it_is_ready_is_asked_about_first() {
        use weft_core::turns::Turn;
        for (state, why) in [
            (None, "Weft can't tell whether the agent is ready"),
            (Some(Turn::Working), "the agent is working"),
            (Some(Turn::Waiting), "the agent is asking you something"),
        ] {
            let mut a = recorded(Sent::ReadyToSend);
            a.session_mut().set_turns(vec![state]);
            a.do_inject(&pending_for(0));
            let Some(Modal::SendAnyway { why: said, .. }) = a.modal.clone() else {
                panic!("{state:?} must ask first, got {:?}", a.modal)
            };
            assert_eq!(said, why);
            let drawn = crate::ui::panel_text(&a).join("\n");
            assert!(drawn.contains(why), "{drawn}");
            assert!(drawn.contains("Type it anyway"), "{drawn}");
            // Only an agent asking is one the person would answer instead.
            let answer = drawn.contains("Cancel — I will answer the agent");
            assert_eq!(answer, state == Some(Turn::Waiting), "{drawn}");
        }
        for state in [Turn::Ready, Turn::TurnEnded] {
            let mut a = recorded(Sent::ReadyToSend);
            a.session_mut().set_turns(vec![Some(state)]);
            a.do_inject(&pending_for(0));
            assert!(a.modal.is_none(), "{state:?} is typed into: {:?}", a.modal);
        }
    }

    /// §6 test 1, drawn: the row carries the agent's state from its receipt.
    #[test]
    fn the_row_of_an_ask_shows_its_agents_state() {
        use weft_core::turns::{Session, Turn};
        let mut a = with_unit(Sent::Arrived { exact: true });
        let before = crate::ui::sidebar_text(&a);
        assert!(!before.contains("working"), "no receipt, no state: {before}");
        a.session_mut().turns = vec![Session {
            harness: "codex".into(),
            id: "01a0bdb6-1d1f-79c2-84b0-8b03496d7db0".into(),
            first: 1,
            latest: Turn::Working,
            at: 1,
        }];
        let after = crate::ui::sidebar_text(&a);
        assert!(after.contains("working · ASKED"), "{after}");
    }

    /// NEEDS YOU is an agent asking its person for input, and nothing
    /// else: not a prompt ready to send, not a finished turn. Its harness's row carries the badge, and Space goes to the agent.
    #[test]
    fn only_an_agent_asking_for_input_needs_you_and_space_goes_to_it() {
        let mut a = app();
        let mut ready = unit(Sent::ReadyToSend);
        ready.ask_id = "ask_2".into();
        a.set_units(vec![unit(Sent::Arrived { exact: true }), ready]);
        agent_says(&mut a, "codex", "fixture", "turn_ended");
        assert_eq!(a.needs_you(), 0, "a finished turn and a prompt to send ask nothing");
        press(&mut a, KeyCode::Char(' '));
        assert_eq!(a.hint_text(), Some("Nothing needs your input."));
        assert!(a.detail().is_none(), "Space opens no Ask");
        assert!(!crate::ui::sidebar_text(&a).contains('●'), "{}", crate::ui::sidebar_text(&a));

        agent_says(&mut a, "codex", "fixture", "waiting");
        assert_eq!(a.needs_you(), 1);
        let rows = crate::ui::sidebar_text(&a);
        let badge = rows.lines().find(|l| l.contains("needs your input")).expect("a badge");
        assert!(badge.contains("codex") && !badge.contains("ASKED"), "{rows}");
        press(&mut a, KeyCode::Char(' '));
        assert_eq!(a.pane_focus, 0);
        assert!(!a.show_work && a.detail().is_none(), "Space shows the agent asking");
    }

    /// Space into a folded project opens it to the harness row of the agent
    /// asking, so that row is picked and an arrow moves on from it.
    #[test]
    fn space_into_a_folded_project_opens_it_to_the_agent_asking() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        agent_says(&mut a, "codex", "fixture", "waiting");
        a.selected = 0;
        press(&mut a, KeyCode::Enter);
        assert!(a.rows()[0].folded(), "folded");
        press(&mut a, KeyCode::Char(' '));
        let rows = a.rows();
        assert!(
            matches!(&rows[a.selected], Row::Harness { name, .. } if name == "codex"),
            "{:?} at {}",
            rows,
            a.selected
        );
        press(&mut a, KeyCode::Down);
        assert!(matches!(a.rows()[a.selected], Row::Action { .. }), "the arrow moved on from it");
    }

    /// The list shows an agent the moment it is opened, before anything is
    /// asked, and OPEN counts it: OPEN is the harnesses open, not the Asks.
    #[test]
    fn an_agent_is_on_the_list_and_counted_as_soon_as_it_is_opened() {
        let mut a = app();
        let screen = drawn(&mut a);
        assert!(screen.contains("▾ codex"), "drawn before anything is asked:\n{screen}");
        assert!(screen.contains("1 open"), "{screen}");
        assert_eq!(a.open_count(), 1);
        a.set_units(vec![unit(Sent::ReadyToSend), unit(Sent::Arrived { exact: true })]);
        assert_eq!(a.open_count(), 1, "Asks are not agents");
        let drawn = drawn(&mut a);
        assert!(drawn.lines().next().is_some_and(|t| t.contains("OPEN  1")), "{drawn}");
    }

    /// The owner's case: someone asks, quits before the prompt is composed,
    /// and comes back. RingFrame recorded the Ask as asked, so the list shows
    /// it as ASKING and its view shows what was asked.
    #[test]
    fn an_ask_recorded_as_asked_is_on_the_list_and_its_view_shows_the_words() {
        let mut a = app();
        let rf = a.root().join(".fab7/rf");
        std::fs::create_dir_all(rf.join("asks/ask_1")).expect("asks");
        std::fs::write(rf.join("asks/ask_1/source.txt"), "fix the login bug\nplease").expect("src");
        let requested = serde_json::json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.requested",
            "time": "2026-09-27T10:00:00Z", "id": "ask_1",
            "actor": {"kind": "human", "id": "local-user"}, "links": [],
            "data": {"title": "fix the login bug", "captured_by": "prompt_hook",
                     "source": {"role": "source_intent", "path": "asks/ask_1/source.txt",
                                "bytes": 24, "sha256": "a"},
                     "host": {"name": "codex", "session_ref": "fixture"}}
        });
        std::fs::write(rf.join("ledger.jsonl"), format!("{requested}\n")).expect("ledger");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.session_mut().units.is_empty() {
            a.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        a.take_the_board();
        let rows = crate::ui::sidebar_text(&a);
        let row = rows.lines().find(|l| l.contains("fix the login bug")).expect("the row");
        assert!(row.contains("ASKING"), "{rows}");
        press(&mut a, KeyCode::Enter);
        let lines = a.detail().expect("its view").lines.clone();
        assert!(lines.contains(&"ASK [ASKING]".to_string()), "{lines:?}");
        assert!(lines.contains(&"  please".to_string()), "the words as asked: {lines:?}");
        assert!(!a.proceeds(), "nothing to send yet");
    }

    /// An agent with nothing asked of it has a row too, so its badge has
    /// somewhere to be.
    #[test]
    fn an_agent_with_no_asks_still_has_a_row_for_its_badge() {
        let mut a = app();
        agent_says(&mut a, "codex", "fixture", "waiting");
        let rows = crate::ui::sidebar_text(&a);
        assert!(
            rows.lines().any(|l| l.contains("codex") && l.contains("needs your input")),
            "{rows}"
        );
    }

    /// ADR-0017: an agent the person is not looking at that starts asking
    /// for input sends one notification; a turn that ends sends none; the agent in
    /// front sends none; one that stays waiting is not announced again;
    /// turned off, nothing is sent.
    #[test]
    fn an_agent_you_are_not_looking_at_tells_you_once_when_it_needs_you() {
        use weft_core::turns::Turn;
        let mut a = app();
        a.add("codex", "/bin/cat").expect("a second agent");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.pane_count() < 2 {
            a.pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        a.pane_focus = 0;
        let set = |a: &mut App, states: Vec<Option<Turn>>| {
            a.session_mut().set_turns(states);
            a.notices()
        };
        assert!(
            set(&mut a, vec![Some(Turn::Working), Some(Turn::Working)]).is_empty(),
            "first sight"
        );
        assert!(
            set(&mut a, vec![Some(Turn::Working), Some(Turn::TurnEnded)]).is_empty(),
            "a turn that ended is not an alarm"
        );
        let told = set(&mut a, vec![Some(Turn::Working), Some(Turn::Waiting)]);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].contains("codex") && told[0].contains("needs your input"), "{told:?}");
        assert!(
            set(&mut a, vec![Some(Turn::Working), Some(Turn::Waiting)]).is_empty(),
            "once per change"
        );
        assert!(
            set(&mut a, vec![Some(Turn::TurnEnded), Some(Turn::Waiting)]).is_empty(),
            "the agent in front is never announced"
        );
        a.session_mut().notify = false;
        set(&mut a, vec![Some(Turn::Working), Some(Turn::Working)]);
        assert!(
            set(&mut a, vec![Some(Turn::Working), Some(Turn::Waiting)]).is_empty(),
            "turned off"
        );
    }

    /// Panes are told apart by id, so one opening while another starts
    /// asking loses nothing and announces only the one asking.
    #[test]
    fn an_agent_opening_while_another_asks_sends_one_notification() {
        use weft_core::turns::Turn;
        let mut a = app();
        a.session_mut().set_turns(vec![Some(Turn::Working)]);
        assert!(a.notices().is_empty(), "first sight");
        a.add("codex", "/bin/cat").expect("a second agent");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.pane_count() < 2 {
            a.pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        a.pane_focus = 1;
        a.session_mut().set_turns(vec![Some(Turn::Waiting), None]);
        let told = a.notices();
        assert_eq!(told.len(), 1, "{told:?}");
    }

    #[test]
    fn a_notification_goes_through_the_terminal_or_rings_its_bell() {
        assert_eq!(
            notification(Some("iTerm.app"), "codex: turn ended"),
            b"\x1b]9;codex: turn ended\x07"
        );
        assert_eq!(notification(Some("ghostty"), "x"), b"\x1b]9;x\x07");
        assert_eq!(
            notification(Some("Apple_Terminal"), "x"),
            b"\x07",
            "the bell where there is none"
        );
        assert_eq!(notification(None, "x"), b"\x07");
        assert_eq!(
            notification(Some("WezTerm"), "a\x07b\x1bc"),
            b"\x1b]9;abc\x07",
            "nothing in a message can end the sequence early"
        );
    }

    fn drawn(a: &mut App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).expect("terminal");
        terminal.draw(|frame| crate::ui::draw(frame, a)).expect("draw");
        let b = terminal.backend().buffer().clone();
        (0..30)
            .map(|y| (0..120).map(|x| b.cell((x, y)).map_or(" ", |c| c.symbol())).collect())
            .collect::<Vec<String>>()
            .join("\n")
    }

    fn click(a: &mut App, column: u16, row: u16) {
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
        a.settle();
    }

    /// A click on a row selects it, as the arrows would,
    /// and types nothing into any agent.
    #[test]
    fn a_click_on_a_row_selects_it() {
        let mut a = app();
        let mut second = unit(Sent::Arrived { exact: true });
        second.ask_id = "ask_2".into();
        second.title = "the second one".into();
        a.set_units(vec![unit(Sent::Arrived { exact: true }), second]);
        let screen = drawn(&mut a);
        let (y, line) =
            screen.lines().enumerate().find(|(_, l)| l.contains("the second one")).expect("drawn");
        let x = line.find("the second one").expect("x") as u16;
        let before = a.pane_text(0);
        a.focus = Focus::Agent;
        click(&mut a, x, y as u16);
        assert_eq!(a.selected_unit().map(|u| u.ask_id.as_str()), Some("ask_2"));
        assert_eq!(a.focus, Focus::Weft, "the list has the keys, as after an arrow");
        std::thread::sleep(Duration::from_millis(200));
        a.pump();
        assert_eq!(a.pane_text(0), before, "a click never types into an agent");
    }

    /// A click in the agent, on the line a list row is drawn on, is the
    /// agent's: it selects nothing and the keys stay where they were.
    #[test]
    fn a_click_in_the_agent_beside_a_row_selects_nothing() {
        let mut a = app();
        let mut second = unit(Sent::Arrived { exact: true });
        second.ask_id = "ask_2".into();
        second.title = "the second one".into();
        a.set_units(vec![unit(Sent::Arrived { exact: true }), second]);
        let screen = drawn(&mut a);
        let y = screen.lines().position(|l| l.contains("the second one")).expect("drawn");
        let before = a.selected;
        a.focus = Focus::Agent;
        click(&mut a, 100, y as u16);
        assert_eq!(a.selected, before, "no row was selected");
        assert_eq!(a.focus, Focus::Agent, "the keys stay with the agent");
    }

    /// And a click on an agent's tab goes to that agent, as its number would.
    #[test]
    fn a_click_on_an_agents_tab_goes_to_it() {
        let mut a = app();
        a.add("codex", "/bin/cat").expect("a second agent");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.pane_count() < 2 {
            a.pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        a.pane_focus = 0;
        let screen = drawn(&mut a);
        let (y, line) =
            screen.lines().enumerate().find(|(_, l)| l.contains("2 codex")).expect("tabs");
        let x = line.find("2 codex").expect("x") as u16;
        click(&mut a, x + 2, y as u16);
        assert_eq!(a.pane_focus, 1);
        click(&mut a, 0, 29);
        assert_eq!(a.pane_focus, 1, "a click elsewhere goes nowhere");
    }

    /// Where `needle` starts on the screen, in cells: a wide character such
    /// as `⚡` takes two.
    fn cell_of(a: &mut App, needle: &str) -> (u16, u16) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).expect("terminal");
        terminal.draw(|frame| crate::ui::draw(frame, a)).expect("draw");
        let b = terminal.backend().buffer().clone();
        for y in 0..30 {
            for x in 0..120 {
                let from: String = (x..120).map(|x| b[(x, y)].symbol()).collect();
                if from.starts_with(needle) {
                    return (x, y);
                }
            }
        }
        panic!("{needle} is not on the screen")
    }

    /// `⚡` is two cells wide, and a tab after a turbo tab is where it is drawn.
    #[test]
    fn a_click_on_the_tab_after_a_turbo_tab_goes_to_it() {
        let mut a = app();
        a.add("codex", "/bin/cat").expect("a second agent");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && a.pane_count() < 2 {
            a.pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        a.session_mut().panes[0].turbo = true;
        a.pane_focus = 0;
        let (x, y) = cell_of(&mut a, "2 codex");
        click(&mut a, x + "2 codex".len() as u16 - 1, y);
        assert_eq!(a.pane_focus, 1, "its last cell is its own");
    }

    /// A send made from the detail view closes it, so the person lands in the
    /// agent the keys went to rather than on a view that blocks the screen.
    #[test]
    fn proceeding_from_the_detail_view_closes_it_and_lands_in_the_agent() {
        // The prompt is still to be sent, so [P]ROCEED sends it.
        let mut a = recorded(Sent::ReadyToSend);
        std::fs::write(a.root().join(".fab7/rf/y"), "Ship it.\n").expect("the prompt");
        press(&mut a, KeyCode::Enter); // the row's detail view
        assert!(a.detail().is_some(), "the detail view is open");
        press(&mut a, KeyCode::Char('p'));
        assert!(matches!(a.modal, Some(Modal::Confirm(_))), "{:?} / {:?}", a.modal, a.hint_text());
        press(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none(), "{:?}", a.modal);
        assert!(a.detail().is_none(), "the detail view closed");
        assert_eq!(a.focus, Focus::Agent, "and the keys are the agent's");
    }

    /// Sent on the person's yes, and RingFrame would not record that it went:
    /// the person is told, rather than being offered the send again blind.
    #[test]
    fn a_send_ringframe_would_not_record_is_said() {
        if !ringframe_on_path() {
            eprintln!("skipped: ringframe is not on PATH");
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let mut a = recorded(Sent::ReadyToSend);
        std::fs::write(a.root().join(".fab7/rf/y"), "Ship it.\n").expect("the prompt");
        let ledger = a.root().join(".fab7/rf/ledger.jsonl");
        std::fs::set_permissions(&ledger, std::fs::Permissions::from_mode(0o444)).expect("ro");
        press(&mut a, KeyCode::Char('p'));
        assert!(matches!(a.modal, Some(Modal::Confirm(_))), "{:?} / {:?}", a.modal, a.hint_text());
        press(&mut a, KeyCode::Enter);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && a.session_mut().unrecorded.is_none() {
            a.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        let said = drawn(&mut a).lines().last().unwrap_or_default().to_string();
        assert!(said.contains("RingFrame did not record it"), "{said}");
        assert!(said.contains("ledger.io"), "and why: {said}");
        std::fs::set_permissions(&ledger, std::fs::Permissions::from_mode(0o644)).expect("rw");
    }

    /// Turbo mode is switched from Weft, by `[T]` or a click on it in the
    /// title bar, and the title bar says where it stands.
    #[test]
    fn turbo_mode_is_switched_by_its_key_or_a_click_and_shown_in_the_title() {
        let mut a = app();
        let title = |a: &mut App| drawn(a).lines().next().unwrap_or("").to_string();
        assert!(title(&mut a).contains("[T]URBO OFF"), "{}", title(&mut a));
        press(&mut a, KeyCode::Char('t'));
        assert!(a.turbo());
        let on = title(&mut a);
        assert!(on.contains('⚡') && on.contains("[T]URBO ON"), "{on}");
        assert!(a.hint_text().is_some_and(|h| h.contains("next")), "{:?}", a.hint_text());
        // One character per cell on the drawn line, so this is the column.
        let x = on.find("[T]URBO").map(|b| on[..b].chars().count()).expect("x") as u16;
        click(&mut a, x + 3, 0);
        assert!(!a.turbo(), "a click switches it back");
    }

    /// A harness row says which harness has an agent running in turbo mode.
    #[test]
    fn a_harness_running_in_turbo_mode_carries_its_badge() {
        let mut a = app();
        assert!(!crate::ui::sidebar_text(&a).contains('⚡'));
        a.session_mut().panes[0].turbo = true;
        let rows = crate::ui::sidebar_text(&a);
        assert!(rows.lines().any(|l| l.contains("codex") && l.contains('⚡')), "{rows}");
    }

    /// An agent started in turbo mode says so on its tab, and only it.
    #[test]
    fn an_agent_in_turbo_mode_says_so_on_its_tab() {
        let mut a = app();
        let tabs = |a: &mut App| drawn(a).lines().nth(1).unwrap_or("").to_string();
        assert!(!tabs(&mut a).contains('⚡'), "{}", tabs(&mut a));
        a.session_mut().panes[0].turbo = true;
        assert!(tabs(&mut a).contains("1 codex ⚡"), "{}", tabs(&mut a));
    }

    /// An agent that has exited carries no badge: no `⚡` on its tab, and
    /// its harness's row says it is not running.
    #[test]
    fn an_agent_that_exited_carries_no_running_badge() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        a.session_mut().panes[0].turbo = true;
        a.session_mut().panes[0].running = false;
        let tabs = drawn(&mut a).lines().nth(1).unwrap_or("").to_string();
        assert!(!tabs.contains('⚡'), "{tabs}");
        let rows = crate::ui::sidebar_text(&a);
        assert!(rows.lines().any(|l| l.contains("codex") && l.contains("not running")), "{rows}");
    }

    /// The owner's case: an Ask sent through Weft, which no hook saw arrive.
    /// Nothing asks for the send again: [P] has nothing to do, and what
    /// comes next is the person's, through [F] FOLLOW UP or the bar's acts.
    #[test]
    fn a_sent_ask_is_never_sent_again_by_proceed() {
        let mut a = app();
        let mut u = unit(Sent::Unconfirmed);
        u.attributed_at = Some("2026-09-27T10:01:00.000Z".into());
        a.set_units(vec![u]);
        agent_says(&mut a, "codex", "fixture", "turn_ended");
        assert_eq!(a.needs_you(), 0, "a finished turn asks nothing");
        press(&mut a, KeyCode::Char('p'));
        assert!(a.modal.is_none(), "{:?}", a.modal);
        assert_eq!(
            a.hint_text(),
            Some("health endpoint has nothing to send. [F] FOLLOW UP asks for more.")
        );
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
        // harness's chooser, not on the person, so there is nothing to send.
        let mut a = recorded(Sent::NotSent);
        press(&mut a, KeyCode::Char('p'));
        assert!(a.modal.is_none(), "{:?}", a.modal);
        assert_eq!(
            a.hint_text(),
            Some("health endpoint has nothing to send. [F] FOLLOW UP asks for more.")
        );
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
        // A prompt still to be sent is the one thing [P] carries. A sent one
        // is not carried at all: its Eval is the bar's [E]VAL, when the
        // person wants it. A closed one only follows up.
        let a = with_unit(Sent::ReadyToSend);
        assert!(a.proceeds());
        assert_eq!(a.unavailable(Act::Proceed), None);

        let a = with_unit(Sent::Arrived { exact: true });
        assert!(!a.proceeds());
        assert!(a.unavailable(Act::Proceed).is_some_and(|s| s.contains("FOLLOW UP")));
        assert_eq!(a.unavailable(Act::Eval), None, "its Eval is the person's to run");

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
        a.settle();
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
    fn a_name_config_toml_does_not_know_is_said_once() {
        let mut a = app();
        a.session_mut().routing.ignored = vec!["eval.debate.judge".into()];
        a.take_the_board();
        assert_eq!(
            a.hint_text(),
            Some(
                "config.toml names something Weft does not know (eval.debate.judge). It is ignored."
            )
        );
    }

    #[test]
    fn a_leftover_json_file_is_said_once() {
        let mut a = app();
        a.session_mut().routing.leftover = vec!["routing.json".into(), "eval.json".into()];
        a.take_the_board();
        assert_eq!(
            a.hint_text(),
            Some(
                "Weft no longer reads routing.json, eval.json; move what it holds into \
                 ~/.fab7/weft/config.toml."
            )
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
    fn space_never_opens_an_ask() {
        // A prompt ready to send is the person's to send when they choose: it
        // is not NEEDS YOU, so Space leaves the selection where it was.
        let mut a = app();
        let mut ready = unit(Sent::ReadyToSend);
        ready.ask_id = "ask_2".into();
        a.set_units(vec![unit(Sent::TakenByAgent), ready]);
        let before = a.selected;
        press(&mut a, KeyCode::Char(' '));
        assert_eq!(a.selected, before);
        assert!(a.detail().is_none());
        assert_eq!(a.needs_you(), 0);
    }

    #[test]
    fn explaining_a_waiting_agent_says_its_hook_said_so() {
        let mut a = app();
        a.session_mut().set_turns(vec![Some(weft_core::turns::Turn::Waiting)]);
        press(&mut a, KeyCode::Char('y'));
        let hint = a.hint_text().expect("a sentence");
        assert!(hint.contains("reported, through its hook"), "{hint}");
        a.session_mut().set_turns(vec![Some(weft_core::turns::Turn::Working)]);
        press(&mut a, KeyCode::Char('y'));
        assert!(a.hint_text().expect("a sentence").contains("has not reported"));
    }

    #[test]
    fn weft_never_answers_a_waiting_agent_itself() {
        // [Enter] ANSWER IT puts the person in the pane; it types nothing.
        let mut a = app();
        a.session_mut().set_turns(vec![Some(weft_core::turns::Turn::Waiting)]);
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
            let _ = weftd::server::Session::serve(
                &listening,
                &listening.with_extension("no-config.toml"),
            );
        });
        let session = connect_when_listening(&socket, &dir);
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
        a.settle();
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
        a.settle();
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
            arrived: None,
            attributed_at: None,
            gathered: None,
            sealed: None,
            seal_id: None,
            sealed_at: None,
            requested_only: false,
        }]);
        a.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)).expect("key");
        a.settle();
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
