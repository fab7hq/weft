//! The running application: state, events, and the loop.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::DefaultTerminal;

use crate::blocked::{self, Evidence};
use crate::encode;
use crate::inject::Refusal;
use crate::keys::{self, Action, Chord, Focus, Key, Toggle};
use crate::ledger::{Ledger, Sent, Unit};
use crate::pane::Pane;
use crate::record::Record;
use crate::ringframe;

/// What Weft is about to type, and where. Shown before anything is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub payload: Vec<u8>,
    pub pane: usize,
    pub what: String,
    pub why: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    Quit,
    Help,
    /// Composing an intent.
    Ask { text: String, target: usize },
    /// Weft is about to type something. Always shown first.
    Confirm(Pending),
    /// A unit of work, in full.
    Detail { unit: usize, record: Option<Record> },
    /// Something did not work, said plainly.
    Note(String),
}

pub struct Agent {
    pub title: String,
    pub harness: String,
    pub status: String,
    pub pty: Pane,
    rows: u16,
    cols: u16,
}

impl Agent {
    pub fn fit(&mut self, rows: u16, cols: u16) -> Result<()> {
        if rows == self.rows && cols == self.cols {
            return Ok(());
        }
        self.pty.resize(rows, cols)?;
        self.rows = rows;
        self.cols = cols;
        Ok(())
    }
}

pub struct App {
    pub project: String,
    pub root: PathBuf,
    pub panes: Vec<Agent>,
    pub pane_focus: usize,
    pub units: Vec<Unit>,
    pub selected: usize,
    pub focus: Focus,
    pub toggle: Toggle,
    pub modal: Option<Modal>,
    pub modal_choice: usize,
    pub quit: bool,
    pub stop_agents_on_quit: bool,
    ledger: Ledger,
    prefixes: std::collections::HashMap<String, String>,
    pane_area: Option<ratatui::layout::Rect>,
}

impl App {
    pub fn new(root: PathBuf, toggle: Toggle) -> Self {
        let project = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into());
        let ledger = Ledger::at(&root);
        Self {
            project,
            root,
            panes: Vec::new(),
            pane_focus: 0,
            units: Vec::new(),
            selected: 0,
            focus: Focus::Weft,
            toggle,
            modal: None,
            modal_choice: 0,
            quit: false,
            stop_agents_on_quit: false,
            ledger,
            prefixes: std::collections::HashMap::new(),
            pane_area: None,
        }
    }

    /// `spec` is the command line the person would have typed, e.g.
    /// `claude --model sonnet --effort medium`. Weft never chooses the model:
    /// that is the harness's configuration and the person's decision.
    pub fn add(&mut self, harness: &str, spec: &str) -> Result<()> {
        let cwd = self.root.to_string_lossy().into_owned();
        let mut parts = spec.split_whitespace();
        let program = parts.next().unwrap_or(spec);
        let args: Vec<&str> = parts.collect();
        let pty = Pane::spawn_args(harness, program, &args, &cwd, 24, 80)?;
        self.panes.push(Agent {
            title: harness.to_string(),
            harness: harness.to_string(),
            status: "running".into(),
            pty,
            rows: 24,
            cols: 80,
        });
        Ok(())
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.reload();
        while !self.quit {
            terminal.draw(|frame| {
                self.pane_area = None;
                crate::ui::draw(frame, &mut self);
            })?;
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
                self.selected = self.selected.min(self.units.len().saturating_sub(1));
            }
            for pane in &mut self.panes {
                pane.status = if pane.pty.running() { "running" } else { "exited" }.into();
            }
        }
        Ok(())
    }

    fn reload(&mut self) {
        self.ledger.refresh();
        self.units = self.ledger.units();
    }

    pub fn needs_you(&self) -> usize {
        self.units.iter().filter(|u| u.needs_you()).count()
    }

    pub fn selected_unit(&self) -> Option<&Unit> {
        self.units.get(self.selected)
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
        self.panes.iter().position(|p| p.harness == harness)
    }

    // --- actions -----------------------------------------------------------

    fn start_ask(&mut self) {
        if self.panes.is_empty() {
            self.modal = Some(Modal::Note("Start an agent first.".into()));
            return;
        }
        self.modal = Some(Modal::Ask { text: String::new(), target: self.pane_focus });
    }

    fn send_ask(&mut self, text: String, target: usize) {
        let Some(harness) = self.panes.get(target).map(|p| p.harness.clone()) else {
            return;
        };
        let prefix = self.prefix_for(&harness);
        let command = ringframe::skill_command(&prefix, "ask");
        self.modal = Some(Modal::Confirm(Pending {
            payload: format!("{command} {text}").into_bytes(),
            pane: target,
            what: format!("Ask {harness}"),
            why: vec![
                format!("Weft will type  {command}  into {harness}."),
                "The agent will ask you which approach to take, in its own pane.".into(),
            ],
        }));
        self.modal_choice = 0;
    }

    /// Check or Decide: type the skill into the pane that owns the work.
    fn start_skill(&mut self, skill: &str) {
        let Some(unit) = self.selected_unit().cloned() else {
            self.modal = Some(Modal::Note("Nothing to work on yet.".into()));
            return;
        };
        let Some(pane) = self.pane_for(&unit.harness) else {
            self.modal = Some(Modal::Note(format!(
                "No {} pane is open, so there is nowhere to send this.",
                unit.harness
            )));
            return;
        };
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
            what: if skill == "eval" { "Check this work?".into() } else { "Decide on this work?".into() },
            why,
        }));
        self.modal_choice = 0;
    }

    /// A confirmed Ask whose route has to be typed by hand.
    fn start_send(&mut self) {
        let Some(unit) = self.selected_unit().cloned() else { return };
        if unit.sent != Sent::ReadyToSend {
            return;
        }
        let Some(pane) = self.pane_for(&unit.harness) else {
            self.modal = Some(Modal::Note(format!("No {} pane is open.", unit.harness)));
            return;
        };
        match ringframe::ask_copy(&self.root, &unit.ask_id) {
            Ok(bytes) => {
                self.modal = Some(Modal::Confirm(Pending {
                    payload: bytes,
                    pane,
                    what: format!("Ready to send to {}", unit.harness),
                    why: vec!["This is the exact wording. Weft will not change it.".into()],
                }));
                self.modal_choice = 0;
            }
            Err(e) => self.modal = Some(Modal::Note(format!("RingFrame would not hand over the wording: {e:?}"))),
        }
    }

    /// Whether a pane looks like it is waiting for a person. Inference, and
    /// labelled as such everywhere it is shown.
    pub fn waiting(&self, pane: usize) -> Option<Evidence> {
        let agent = self.panes.get(pane)?;
        blocked::looks_blocked(&agent.pty.with_screen(|s| s.contents()))
    }

    fn do_inject(&mut self, pending: &Pending) {
        let blocked = self.waiting(pending.pane).is_some();
        let Some(agent) = self.panes.get_mut(pending.pane) else { return };
        match agent.pty.inject(&pending.payload, blocked) {
            Ok(_) => {
                self.modal = None;
                self.focus = Focus::Agent;
                self.pane_focus = pending.pane;
            }
            Err(Refusal::PaneBlocked) => {
                let why = self
                    .waiting(pending.pane)
                    .map(|e| format!("It is showing:  {}", e.line))
                    .unwrap_or_default();
                self.modal = Some(Modal::Note(format!(
                    "That agent is waiting for your answer, so Weft did not type.\n{why}\nAnswer it in the pane first."
                )))
            }
            Err(Refusal::NoProcess) => {
                self.modal = Some(Modal::Note("That agent is no longer running.".into()))
            }
            Err(Refusal::InjectionInFlight) => {
                self.modal = Some(Modal::Note("Weft is already typing into that agent.".into()))
            }
        }
    }

    fn open_detail(&mut self) {
        let Some(unit) = self.selected_unit().cloned() else { return };
        let record = unit
            .check
            .as_ref()
            .and_then(|c| Record::read(&self.root, &c.eval_id));
        self.modal = Some(Modal::Detail { unit: self.selected, record });
        self.modal_choice = 0;
    }

    // --- events ------------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.modal.is_some() {
            return self.on_modal_key(key);
        }
        let chord = chord_of(key);
        match keys::route(chord, self.focus, self.toggle) {
            Action::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Weft => Focus::Agent,
                    Focus::Agent => Focus::Weft,
                }
            }
            Action::ToAgent => {
                if let (Some(bytes), Some(pane)) = (encode::encode(key), self.panes.get_mut(self.pane_focus)) {
                    pane.pty.send(&bytes)?;
                }
            }
            Action::Pick(delta) => self.pick(delta),
            Action::Open => {
                if self.units.is_empty() {
                    self.focus = Focus::Agent;
                } else {
                    self.open_detail();
                }
            }
            Action::NextPane => {
                if !self.panes.is_empty() {
                    self.pane_focus = (self.pane_focus + 1) % self.panes.len();
                }
            }
            Action::Ask => self.start_ask(),
            Action::Check => self.start_skill("eval"),
            Action::Decide => self.start_skill("seal"),
            Action::Back => {}
            Action::Quit => {
                self.modal = Some(Modal::Quit);
                self.modal_choice = 0;
            }
            Action::Help => {
                self.modal = Some(Modal::Help);
                self.modal_choice = 0;
            }
        }
        Ok(())
    }

    fn on_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        let modal = self.modal.clone().expect("a modal");

        // Composing text is its own keyboard.
        if let Modal::Ask { mut text, mut target } = modal {
            match key.code {
                event::KeyCode::Esc => self.modal = None,
                event::KeyCode::Enter if !text.trim().is_empty() => self.send_ask(text, target),
                event::KeyCode::Backspace => {
                    text.pop();
                    self.modal = Some(Modal::Ask { text, target });
                }
                event::KeyCode::Tab => {
                    if !self.panes.is_empty() {
                        target = (target + 1) % self.panes.len();
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
            Modal::Confirm(_) => 2,
            _ => 1,
        };
        match key.code {
            event::KeyCode::Up | event::KeyCode::Left => {
                self.modal_choice = self.modal_choice.saturating_sub(1)
            }
            event::KeyCode::Down | event::KeyCode::Right => {
                self.modal_choice = (self.modal_choice + 1).min(options - 1)
            }
            event::KeyCode::Esc => self.modal = None,
            event::KeyCode::Enter => self.confirm_modal(&modal),
            event::KeyCode::Char('s') if matches!(modal, Modal::Detail { .. }) => {
                self.modal = None;
                self.start_skill("seal");
            }
            event::KeyCode::Char('f') if matches!(modal, Modal::Detail { .. }) => {
                self.modal = None;
                self.start_ask();
            }
            _ => {}
        }
        Ok(())
    }

    fn confirm_modal(&mut self, modal: &Modal) {
        match modal {
            Modal::Help | Modal::Note(_) | Modal::Detail { .. } => self.modal = None,
            Modal::Ask { .. } => {}
            Modal::Confirm(pending) => {
                if self.modal_choice == 0 {
                    let pending = pending.clone();
                    self.do_inject(&pending);
                } else {
                    self.modal = None;
                }
            }
            Modal::Quit => match self.modal_choice {
                0 => self.quit = true,
                1 => {
                    self.stop_agents_on_quit = true;
                    self.quit = true;
                }
                _ => self.modal = None,
            },
        }
    }

    fn pick(&mut self, delta: i8) {
        if self.units.is_empty() {
            return;
        }
        let n = self.units.len() as i32;
        self.selected = ((self.selected as i32 + delta as i32).rem_euclid(n)) as usize;
    }

    /// Pasted text belongs to whoever has focus. In the agent it is forwarded
    /// whole, so a multi-line paste arrives as one paste, not as keystrokes.
    fn on_paste(&mut self, text: &str) -> Result<()> {
        match (&mut self.modal, self.focus) {
            (Some(Modal::Ask { text: buf, .. }), _) => buf.push_str(text),
            (Some(_), _) => {}
            (None, Focus::Agent) => {
                if let Some(pane) = self.panes.get_mut(self.pane_focus) {
                    pane.pty.send(text.as_bytes())?;
                }
            }
            (None, Focus::Weft) => {}
        }
        Ok(())
    }

    fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        if self.modal.is_some() {
            return Ok(());
        }
        let in_pane = self.pane_area.is_some_and(|a| {
            m.column >= a.x && m.column < a.x + a.width && m.row >= a.y && m.row < a.y + a.height
        });
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if in_pane => self.focus = Focus::Agent,
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = Focus::Weft;
                self.click_list(m.row);
            }
            _ => {}
        }
        Ok(())
    }

    fn click_list(&mut self, row: u16) {
        let body_row = row.saturating_sub(2);
        if body_row == 0 {
            return;
        }
        let index = ((body_row - 1) / 3) as usize;
        if index < self.units.len() {
            self.selected = index;
        }
    }

    pub fn note_pane_area(&mut self, area: ratatui::layout::Rect) {
        self.pane_area = Some(area);
    }

    /// Test seam: the units the board is showing.
    pub fn set_units(&mut self, units: Vec<Unit>) {
        self.units = units;
    }
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
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use serde_json::json;

    fn app() -> App {
        let dir = std::env::temp_dir();
        let mut a = App::new(dir, Toggle::CtrlRightBracket);
        a.add("codex", "/bin/cat").expect("spawn");
        a
    }

    fn with_unit(sent: Sent) -> App {
        let mut a = app();
        a.set_units(vec![unit(sent)]);
        a
    }

    fn unit(sent: Sent) -> Unit {
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

    fn press(a: &mut App, code: KeyCode) {
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
        press(&mut a, KeyCode::Down); // move to [ Cancel ]
        press(&mut a, KeyCode::Enter);
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
    fn checking_with_nothing_to_check_says_so_plainly() {
        let mut a = app();
        press(&mut a, KeyCode::Char('c'));
        let Some(Modal::Note(text)) = a.modal.clone() else { panic!("{:?}", a.modal) };
        assert!(text.contains("Nothing"), "got {text:?}");
    }

    #[test]
    fn a_unit_whose_agent_is_not_open_is_not_silently_dropped() {
        let mut a = app();
        let mut u = unit(Sent::Arrived { exact: true });
        u.harness = "claude-code".into();
        a.set_units(vec![u]);
        press(&mut a, KeyCode::Char('c'));
        let Some(Modal::Note(text)) = a.modal.clone() else { panic!("{:?}", a.modal) };
        assert!(text.contains("claude-code"), "names the missing agent: {text:?}");
    }

    #[test]
    fn enter_on_a_unit_opens_its_detail() {
        let mut a = with_unit(Sent::Arrived { exact: true });
        press(&mut a, KeyCode::Enter);
        assert!(matches!(a.modal, Some(Modal::Detail { .. })));
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
    fn the_needs_you_count_is_derived_from_the_record() {
        let mut a = app();
        a.set_units(vec![unit(Sent::ReadyToSend), unit(Sent::Arrived { exact: true })]);
        assert_eq!(a.needs_you(), 1);
    }

    #[test]
    fn a_click_inside_the_pane_focuses_the_agent_and_outside_comes_back() {
        let mut a = app();
        a.note_pane_area(ratatui::layout::Rect { x: 19, y: 2, width: 60, height: 20 });
        let click = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(click(40, 10)).expect("mouse");
        assert_eq!(a.focus, Focus::Agent);
        a.on_mouse(click(5, 3)).expect("mouse");
        assert_eq!(a.focus, Focus::Weft);
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

        let mut a = App::new(dir.clone(), Toggle::CtrlRightBracket);
        a.reload();
        assert_eq!(a.units.len(), 1);
        assert_eq!(a.units[0].title, "health endpoint");
        std::fs::remove_dir_all(&dir).ok();
    }
}
