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
use crate::ledger::{Ledger, Sent, Unit};
use crate::record::Record;
use crate::ringframe;
use crate::theme::Theme;

/// What Weft is about to type, and where. Shown before anything is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub payload: Vec<u8>,
    pub pane: usize,
    pub what: String,
    pub why: Vec<String>,
    /// Set when this is a confirmed prompt being typed for the person.
    pub ask_id: Option<String>,
}

/// The surfaces that interrupt. Everything else in v2 is drawn in place.
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    Quit,
    Help,
    /// Composing an intent.
    Ask { text: String, target: usize },
    /// Weft is about to type something. Always shown first.
    Confirm(Pending),
    /// Something did not work, said plainly.
    Note(String),
    /// Pick an agent to start. Weft starts none on its own.
    StartAgent { choice: usize },
}

/// The two reading surfaces. Siblings, not a stack: `P` from the judges
/// drawer swaps the content rather than piling a second overlay on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    Wording,
    Judges,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drawer {
    pub kind: Reading,
    pub title: String,
    pub lines: Vec<String>,
    pub offset: usize,
}

/// Something the interface offers a key for. One home for whether it can be
/// used right now, and for the sentence that says what would make it work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Ask,
    Send,
    Check,
    Decide,
    Wording,
    Judges,
    Fix,
    NewAgent,
    Work,
    Help,
    Quit,
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
    ledger: Ledger,
    prefixes: std::collections::HashMap<String, String>,
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

    pub fn selected_unit(&self) -> Option<&Unit> {
        self.units.get(self.selected)
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

    /// Open units, as the title bar counts them.
    pub fn open_count(&self) -> usize {
        self.units.iter().filter(|u| u.sealed.is_none() && !u.cancelled).count()
    }

    /// Units waiting on a decision only the person can make. Panes waiting for
    /// an answer are an inference and carry their own dot on the tab, so they
    /// are not folded into a count the board claims to have read.
    pub fn needs_you(&self) -> usize {
        self.units.iter().filter(|u| u.needs_you()).count()
    }

    /// Whether a pane looks like it is waiting for a person. Inference, and
    /// labelled as such everywhere it is shown.
    pub fn waiting(&self, pane: usize) -> Option<Evidence> {
        let view = self.session.panes.get(pane)?;
        blocked::looks_blocked(&view.contents())
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

    /// Why an action is not available now, as one sentence. `None` means it is.
    pub fn unavailable(&self, act: Act) -> Option<String> {
        let no_pane = |harness: &str| {
            format!("No {harness} pane is open, so there is nowhere to send this.")
        };
        match act {
            Act::NewAgent | Act::Work | Act::Help | Act::Quit => None,
            Act::Ask => (self.pane_count() == 0)
                .then(|| "Start an agent first — [N]EW AGENT.".to_string()),
            Act::Fix => match self.selected_unit() {
                None => Some("Nothing has been asked for yet.".into()),
                Some(_) if self.pane_count() == 0 => {
                    Some("Start an agent first — [N]EW AGENT.".into())
                }
                Some(_) => None,
            },
            Act::Send => match self.selected_unit() {
                None => Some("Nothing has been asked for yet.".into()),
                Some(u) if u.sent != Sent::ReadyToSend => Some(
                    match self.units.iter().find(|o| o.sent == Sent::ReadyToSend) {
                        Some(other) => {
                            format!("{} is the one ready to send. ↓ to select it.", other.title)
                        }
                        None => "Nothing is ready to send.".into(),
                    },
                ),
                Some(u) => self.pane_for(&u.harness).is_none().then(|| no_pane(&u.harness)),
            },
            Act::Check | Act::Decide => match self.selected_unit() {
                None => Some("Nothing to work on yet.".into()),
                Some(u) => self.pane_for(&u.harness).is_none().then(|| no_pane(&u.harness)),
            },
            Act::Wording => self
                .selected_unit()
                .is_none()
                .then(|| "Nothing has been asked for yet.".to_string()),
            Act::Judges => match self.selected_unit() {
                None => Some("Nothing has been asked for yet.".into()),
                Some(u) => u.check.is_none().then(|| {
                    "No check has been run on this yet — [C]HECK asks the judges to look."
                        .to_string()
                }),
            },
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
        let ledger = Ledger::at(&root);
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
            ledger,
            prefixes: std::collections::HashMap::new(),
            pane_area: None,
            row_spans: Vec::new(),
            tab_spans: Vec::new(),
            action_spans: Vec::new(),
            action_row: 0,
            list_area: None,
            needs_you_span: None,
            record_available: ringframe::installed(),
        }
    }

    /// `spec` is the command line the person would have typed, e.g.
    /// `claude --model sonnet --effort medium`. Weft never chooses the model:
    /// that is the harness's configuration and the person's decision.
    pub fn add(&mut self, harness: &str, spec: &str) -> Result<()> {
        self.session.spawn(harness, spec)?;
        // The pane appears when the server says it has one.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let before = self.session.panes.len();
        while std::time::Instant::now() < deadline {
            self.session.pump();
            if self.session.panes.len() > before {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.reload();
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
            if self.ledger.refresh() {
                self.units = self.ledger.units();
                self.clamp_selection();
            }
            self.session.pump();
        }
        Ok(())
    }

    fn reload(&mut self) {
        self.ledger.refresh();
        self.units = self.ledger.units();
    }

    /// Fold whatever the ledger has now. The run loop does this each tick;
    /// a probe or a test does it once.
    pub fn refresh_for_test(&mut self) {
        self.reload();
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.units.len().saturating_sub(1));
        if self.expanded.is_some_and(|i| i >= self.units.len()) {
            self.expanded = None;
        }
    }

    /// How this harness's skills are invoked, asked of RingFrame once per host.
    fn prefix_for(&mut self, harness: &str) -> String {
        if let Some(p) = self.prefixes.get(harness) {
            return p.clone();
        }
        let p = ringframe::invocation_prefix(&self.root, harness).unwrap_or_else(|_| "/rf:".into());
        self.prefixes.insert(harness.to_string(), p.clone());
        p
    }

    fn pane_for(&self, harness: &str) -> Option<usize> {
        self.session.panes.iter().position(|p| p.harness == harness)
    }

    fn say(&mut self, sentence: impl Into<String>) {
        self.hint = Some(sentence.into());
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

    fn start_ask(&mut self) {
        if !self.guard(Act::Ask) {
            return;
        }
        self.modal = Some(Modal::Ask { text: String::new(), target: self.pane_focus });
    }

    fn send_ask(&mut self, text: String, target: usize) {
        let Some(harness) = self.session.panes.get(target).map(|p| p.harness.clone()) else {
            return;
        };
        let prefix = self.prefix_for(&harness);
        let command = ringframe::skill_command(&prefix, "ask");
        self.modal = Some(Modal::Confirm(Pending {
            payload: format!("{command} {text}").into_bytes(),
            pane: target,
            ask_id: None,
            what: format!("Ready to ask {harness}"),
            why: vec![
                format!("Weft will type  {command}  into {harness}."),
                "The agent will ask you which approach to take, in its own pane.".into(),
            ],
        }));
        self.modal_choice = 0;
    }

    /// Which agents are installed, for the start picker.
    pub fn available_agents() -> Vec<&'static str> {
        ["claude", "codex"].into_iter().filter(|a| which(a)).collect()
    }

    fn start_agent_picker(&mut self) {
        let found = Self::available_agents();
        if found.is_empty() {
            self.say("No coding agent found. Install claude or codex first.");
            return;
        }
        self.modal = Some(Modal::StartAgent { choice: 0 });
        self.modal_choice = 0;
    }

    pub fn start_chosen_agent(&mut self, choice: usize) {
        let found = Self::available_agents();
        let Some(program) = found.get(choice).copied() else { return };
        let harness = if program == "claude" { "claude-code" } else { program };
        match self.add(harness, program) {
            Ok(()) => {
                self.modal = None;
                self.pane_focus = self.session.panes.len().saturating_sub(1);
                self.focus = Focus::Agent;
            }
            Err(e) => self.modal = Some(Modal::Note(format!("Could not start {program}: {e}"))),
        }
    }

    /// Check or Decide: type the skill into the pane that owns the work.
    fn start_skill(&mut self, skill: &str) {
        let act = if skill == "eval" { Act::Check } else { Act::Decide };
        if !self.guard(act) {
            return;
        }
        let Some(unit) = self.selected_unit().cloned() else { return };
        let Some(pane) = self.pane_for(&unit.harness) else { return };
        let prefix = self.prefix_for(&unit.harness);
        let command = ringframe::skill_command(&prefix, skill);
        let why = if skill == "eval" {
            vec![
                format!("Weft will type  {command}  into {}.", unit.harness),
                "The agent will look at everything still open and have three".into(),
                "judges check it. None of them did the work.".into(),
            ]
        } else {
            vec![
                format!("Weft will type  {command}  into {}.", unit.harness),
                "The agent will ask what you decided, and write a receipt.".into(),
            ]
        };
        self.modal = Some(Modal::Confirm(Pending {
            payload: command.into_bytes(),
            pane,
            ask_id: None,
            what: if skill == "eval" { "Check this work?".into() } else { "Decide on this work?".into() },
            why,
        }));
        self.modal_choice = 0;
    }

    /// A confirmed Ask whose route has to be typed by hand.
    fn start_send(&mut self) {
        if !self.guard(Act::Send) {
            return;
        }
        let Some(unit) = self.selected_unit().cloned() else { return };
        let Some(pane) = self.pane_for(&unit.harness) else { return };
        match ringframe::ask_copy(&self.root, &unit.ask_id) {
            Ok(bytes) => {
                self.modal = Some(Modal::Confirm(Pending {
                    payload: bytes,
                    pane,
                    ask_id: Some(unit.ask_id.clone()),
                    what: format!("Ready to send to {}", unit.harness),
                    why: vec!["This is the exact wording. Weft will not change it.".into()],
                }));
                self.modal_choice = 0;
            }
            Err(e) => {
                self.modal = Some(Modal::Note(format!(
                    "RingFrame would not hand over the wording: {e:?}"
                )))
            }
        }
    }

    fn do_inject(&mut self, pending: &Pending) {
        if self.session.panes.get(pending.pane).is_none() {
            return;
        }
        // The server owns the pane, so the server does the typing.
        match self.session.inject(pending.pane, &pending.payload) {
            Ok(()) => {
                self.modal = None;
                self.focus = Focus::Agent;
                self.pane_focus = pending.pane;
                // Weft typed it. Whether it arrived is the hook's to say, so
                // the board reports only what Weft itself did until then.
                if let Some(ask_id) = pending.ask_id.clone() {
                    if let Some(u) = self.units.iter_mut().find(|u| u.ask_id == ask_id) {
                        if u.sent == Sent::ReadyToSend {
                            u.sent = Sent::Unconfirmed;
                        }
                    }
                }
            }
            Err(e) => self.modal = Some(Modal::Note(format!("Could not type into that agent: {e}"))),
        }
    }

    // --- the drawer ----------------------------------------------------------

    /// The exact wording RingFrame compiled, read back through its CLI.
    fn read_wording(&mut self) {
        if !self.guard(Act::Wording) {
            return;
        }
        let Some(unit) = self.selected_unit().cloned() else { return };
        match ringframe::ask_copy(&self.root, &unit.ask_id) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).to_string();
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
            Err(e) => {
                self.modal = Some(Modal::Note(format!("RingFrame would not hand it over: {e:?}")))
            }
        }
    }

    /// Every judge, every vote, every reason.
    fn read_judges(&mut self) {
        if !self.guard(Act::Judges) {
            return;
        }
        let Some(unit) = self.selected_unit().cloned() else { return };
        let Some(check) = unit.check.clone() else { return };
        let Some(record) = Record::read(&self.root, &check.eval_id) else {
            self.modal = Some(Modal::Note("That check's record is not on disk.".into()));
            return;
        };
        let mut lines = vec![
            format!(
                "judged by {} · recorded as {}, {:.2}",
                record.judged_by().join(" and "),
                check.verdict.recorded(),
                check.agreement
            ),
            format!(
                "{} judges checked the work · none of them did it",
                record.judges.len()
            ),
            String::new(),
        ];
        for j in &record.judges {
            lines.push(format!("JUDGE      {} · {} · {}", j.angle, j.host, j.model));
        }
        for item in &record.items {
            lines.push(String::new());
            lines.push(format!("{} {}", mark_for(item.plain_majority()), item.text));
            lines.push(format!(
                "  {} · {}",
                item.plain_majority(),
                record.agreed(item.agreement)
            ));
            for r in &item.reasons {
                lines.push(format!("  {r}"));
            }
        }
        if !record.unexplained.is_empty() {
            lines.push(String::new());
            lines.push("CHANGED WITH NO JUDGE ABLE TO TIE IT TO WHAT YOU ASKED".into());
            for p in &record.unexplained {
                lines.push(format!("  {p}"));
            }
        }
        lines.push(String::new());
        lines.push("A check is a judgement, not a guarantee.".into());
        self.drawer = Some(Drawer {
            kind: Reading::Judges,
            title: format!("THE JUDGES · {}", unit.title),
            lines,
            offset: 0,
        });
    }

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
            Act::Fix => {
                if self.guard(Act::Fix) {
                    self.drawer = None;
                    self.modal = Some(Modal::Ask { text: String::new(), target: self.pane_focus });
                }
            }
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
                if self.drawer.is_some() {
                    self.scroll_drawer(delta as i32);
                } else {
                    self.pick(delta);
                }
            }
            Action::Open => {
                if self.waiting_here() {
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
            Action::Fix => self.act(Act::Fix),
            Action::Explain => self.explain_waiting(),
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
            Modal::StartAgent { .. } => App::available_agents().len().max(1),
            _ => 1,
        };
        match key.code {
            event::KeyCode::Up => self.modal_choice = self.modal_choice.saturating_sub(1),
            event::KeyCode::Down => {
                self.modal_choice = (self.modal_choice + 1).min(options - 1)
            }
            // `←` is back everywhere, and cancels rather than choosing.
            event::KeyCode::Left | event::KeyCode::Esc => self.modal = None,
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
            } else if let Some(p) = self.session.panes.get_mut(self.pane_focus) {
                p.scroll(-delta);
            }
            return Ok(());
        }
        if self.modal.is_some() {
            return Ok(());
        }
        if m.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(());
        }
        if self.needs_you_span.is_some_and(|(x, w)| m.row == 0 && m.column >= x && m.column < x + w) {
            self.next_needs_you();
            return Ok(());
        }
        if let Some(act) = self.action_at(m.column, m.row) {
            self.focus = Focus::Weft;
            self.act(act);
            return Ok(());
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
            self.focus = Focus::Agent;
            if let Some(p) = self.session.panes.get_mut(self.pane_focus) {
                p.scroll_to_bottom();
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
        self.row_spans
            .iter()
            .find(|(y, h, _)| row >= *y && row < y + h)
            .map(|(_, _, i)| *i)
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

fn mark_for(vote: &str) -> &'static str {
    match vote {
        "yes" => "✓",
        "no" => "✗",
        _ => "?",
    }
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
    })
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
    use crossterm::event::KeyCode;
    use serde_json::json;

    /// A session backed by a real server on a scratch root, because the app is
    /// a client now and there is no honest way to test it without one.
    pub(crate) fn test_session(name: &str) -> (PathBuf, crate::client::Session) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir()
            .join(format!("weft-app-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).expect("root");
        let socket = crate::server::socket_path(&root);
        let _ = std::fs::remove_file(&socket);
        let serving = root.clone();
        let listening = socket.clone();
        std::thread::spawn(move || {
            let _ = crate::server::Session::serve(serving, &listening);
        });
        let session = crate::client::Session::connect(&socket, 24, 80).expect("connect");
        (root, session)
    }

    pub(crate) fn app() -> App {
        let (root, session) = test_session("codex");
        let mut a = App::with_session(root, Toggle, session);
        a.add("codex", "/bin/cat").expect("spawn");
        a
    }

    fn with_unit(sent: Sent) -> App {
        let mut a = app();
        a.set_units(vec![unit(sent)]);
        a
    }

    pub(crate) fn unit(sent: Sent) -> Unit {
        Unit {
            ask_id: "ask_1".into(),
            title: "health endpoint".into(),
            harness: "codex".into(),
            route: "native_plan".into(),
            asked_at: "2026-09-19T14:02:00Z".into(),
            delivery_mode: "human_handoff".into(),
            cancelled: false,
            confirmed: true,
            sent,
            check: None,
            sealed: None,
        }
    }

    pub(crate) fn press(a: &mut App, code: KeyCode) {
        a.on_key(KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    }

    fn ctrl(a: &mut App, c: char) {
        a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)).expect("key");
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
            let mut a = with_unit(Sent::Arrived { exact: true });
            press(&mut a, KeyCode::Char(key));
            let Some(Modal::Confirm(p)) = a.modal.clone() else {
                panic!("{key} must confirm first, got {:?}", a.modal)
            };
            let text = String::from_utf8(p.payload).unwrap();
            assert!(text.ends_with(expect), "got {text:?}");
        }
    }

    #[test]
    fn cancelling_a_confirmation_types_nothing() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        press(&mut a, KeyCode::Char('c'));
        assert!(matches!(a.modal, Some(Modal::Confirm(_))));
        press(&mut a, KeyCode::Left); // [←] CANCEL
        assert!(a.modal.is_none());
        assert_eq!(a.focus, Focus::Weft, "cancelling never moves you into the agent");
    }

    #[test]
    fn confirming_types_it_and_puts_you_in_the_agent() {
        let mut a = with_unit(Sent::Arrived { exact: true });
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
        assert_eq!(
            a.hint_text(),
            Some("readme fix is the one ready to send. ↓ to select it.")
        );
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

        let socket = crate::server::socket_path(&dir);
        let _ = std::fs::remove_file(&socket);
        let serving = dir.clone();
        let listening = socket.clone();
        std::thread::spawn(move || {
            let _ = crate::server::Session::serve(serving, &listening);
        });
        let session = crate::client::Session::connect(&socket, 24, 80).expect("connect");
        let mut a = App::with_session(dir.clone(), Toggle, session);
        a.reload();
        assert_eq!(a.units().len(), 1);
        assert_eq!(a.units()[0].title, "health endpoint");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod start_tests {
    use super::*;
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
            route: "native_plan".into(),
            asked_at: "now".into(),
            delivery_mode: "human_handoff".into(),
            cancelled: false,
            confirmed: true,
            sent: Sent::ReadyToSend,
            check: None,
            sealed: None,
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
