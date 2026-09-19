//! The running application: state, events, and the loop.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::DefaultTerminal;

use crate::encode;
use crate::keys::{self, Action, Chord, Focus, Key, Toggle};
use crate::pane::Pane;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    Quit,
    Help,
}

pub struct Agent {
    pub title: String,
    pub status: String,
    pub pty: Pane,
    rows: u16,
    cols: u16,
}

impl Agent {
    /// Keep the pseudo-terminal the same size as the area it is drawn in.
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
    pub panes: Vec<Agent>,
    pub selected: usize,
    pub focus: Focus,
    pub toggle: Toggle,
    pub modal: Option<Modal>,
    pub modal_choice: usize,
    pub quit: bool,
    pub stop_agents_on_quit: bool,
    pane_area: Option<ratatui::layout::Rect>,
}

impl App {
    pub fn new(project: impl Into<String>, toggle: Toggle) -> Self {
        Self {
            project: project.into(),
            panes: Vec::new(),
            selected: 0,
            focus: Focus::Weft,
            toggle,
            modal: None,
            modal_choice: 0,
            quit: false,
            stop_agents_on_quit: false,
            pane_area: None,
        }
    }

    pub fn add(&mut self, title: impl Into<String>, program: &str, cwd: &str) -> Result<()> {
        let title = title.into();
        let pty = Pane::spawn(title.clone(), program, cwd, 24, 80)?;
        self.panes.push(Agent {
            title,
            status: "running".into(),
            pty,
            rows: 24,
            cols: 80,
        });
        Ok(())
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            terminal.draw(|frame| {
                self.pane_area = None;
                crate::ui::draw(frame, &mut self);
            })?;
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key)?,
                    Event::Mouse(m) => self.on_mouse(m)?,
                    _ => {}
                }
            }
            self.refresh_status();
        }
        Ok(())
    }

    fn refresh_status(&mut self) {
        for pane in &mut self.panes {
            pane.status = if pane.pty.running() { "running" } else { "exited" }.into();
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(modal) = self.modal {
            return self.on_modal_key(modal, key);
        }

        let chord = chord_of(key);
        match keys::route(chord, self.focus, self.toggle) {
            Action::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Weft => Focus::Agent,
                    Focus::Agent => Focus::Weft,
                };
            }
            Action::ToAgent => {
                if let (Some(bytes), Some(pane)) = (encode::encode(key), self.panes.get_mut(self.selected)) {
                    pane.pty.send(&bytes)?;
                }
            }
            Action::Pick(delta) => self.pick(delta),
            Action::Open => self.focus = Focus::Agent,
            Action::NextPane => self.pick(1),
            Action::Quit => {
                self.modal = Some(Modal::Quit);
                self.modal_choice = 0;
            }
            Action::Help => {
                self.modal = Some(Modal::Help);
                self.modal_choice = 0;
            }
            // Ask, Check and Decide arrive in Phase 3; they are RingFrame-aware.
            Action::Ask | Action::Check | Action::Decide | Action::Back => {}
        }
        Ok(())
    }

    fn on_modal_key(&mut self, modal: Modal, key: KeyEvent) -> Result<()> {
        let options = match modal {
            Modal::Quit => 3,
            Modal::Help => 1,
        };
        match key.code {
            event::KeyCode::Up => self.modal_choice = self.modal_choice.saturating_sub(1),
            event::KeyCode::Down => self.modal_choice = (self.modal_choice + 1).min(options - 1),
            event::KeyCode::Esc | event::KeyCode::Left => self.modal = None,
            event::KeyCode::Enter => self.confirm_modal(modal),
            _ => {}
        }
        Ok(())
    }

    fn confirm_modal(&mut self, modal: Modal) {
        match modal {
            Modal::Help => self.modal = None,
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
        if self.panes.is_empty() {
            return;
        }
        let n = self.panes.len() as i32;
        let next = (self.selected as i32 + delta as i32).rem_euclid(n);
        self.selected = next as usize;
    }

    fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        if self.modal.is_some() {
            return Ok(());
        }
        let in_pane = self
            .pane_area
            .is_some_and(|a| m.column >= a.x && m.column < a.x + a.width && m.row >= a.y && m.row < a.y + a.height);

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
        // The list starts two rows down and gives each agent three rows.
        let body_row = row.saturating_sub(2);
        if body_row == 0 {
            return;
        }
        let index = ((body_row - 1) / 3) as usize;
        if index < self.panes.len() {
            self.selected = index;
        }
    }

    pub fn note_pane_area(&mut self, area: ratatui::layout::Rect) {
        self.pane_area = Some(area);
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

    fn app() -> App {
        let mut a = App::new("acme", Toggle::CtrlRightBracket);
        a.add("sh", "/bin/sh", "/tmp").expect("spawn");
        a
    }

    fn press(a: &mut App, code: KeyCode, mods: KeyModifiers) {
        a.on_key(KeyEvent::new(code, mods)).expect("key");
    }

    #[test]
    fn weft_starts_in_weft_not_in_the_agent() {
        assert_eq!(app().focus, Focus::Weft);
    }

    #[test]
    fn the_toggle_moves_between_weft_and_the_agent_both_ways() {
        let mut a = app();
        press(&mut a, KeyCode::Char(']'), KeyModifiers::CONTROL);
        assert_eq!(a.focus, Focus::Agent);
        press(&mut a, KeyCode::Char(']'), KeyModifiers::CONTROL);
        assert_eq!(a.focus, Focus::Weft);
    }

    #[test]
    fn in_the_agent_x_does_not_quit_it_is_typed() {
        let mut a = app();
        a.focus = Focus::Agent;
        press(&mut a, KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(a.modal.is_none(), "X in the agent must not open Weft's quit dialog");
        assert!(!a.quit);
    }

    #[test]
    fn in_weft_x_asks_before_quitting() {
        let mut a = app();
        press(&mut a, KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(a.modal, Some(Modal::Quit));
        assert!(!a.quit, "quitting is never immediate");
    }

    #[test]
    fn quitting_leaves_the_agents_running_by_default() {
        let mut a = app();
        press(&mut a, KeyCode::Char('x'), KeyModifiers::NONE);
        press(&mut a, KeyCode::Enter, KeyModifiers::NONE);
        assert!(a.quit);
        assert!(!a.stop_agents_on_quit, "the default choice keeps the agents alive");
    }

    #[test]
    fn cancelling_the_quit_dialog_keeps_weft_open() {
        let mut a = app();
        press(&mut a, KeyCode::Char('x'), KeyModifiers::NONE);
        press(&mut a, KeyCode::Down, KeyModifiers::NONE);
        press(&mut a, KeyCode::Down, KeyModifiers::NONE);
        press(&mut a, KeyCode::Enter, KeyModifiers::NONE);
        assert!(!a.quit);
        assert!(a.modal.is_none());
    }

    #[test]
    fn enter_in_weft_goes_into_the_agent() {
        let mut a = app();
        press(&mut a, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(a.focus, Focus::Agent);
    }

    #[test]
    fn ask_check_and_decide_do_nothing_until_phase_three() {
        let mut a = app();
        for c in ['a', 'c', 'd'] {
            press(&mut a, KeyCode::Char(c), KeyModifiers::NONE);
            assert!(a.modal.is_none(), "{c} must not open a dialog yet");
        }
    }

    #[test]
    fn a_click_inside_the_pane_focuses_the_agent() {
        let mut a = app();
        a.note_pane_area(ratatui::layout::Rect { x: 19, y: 2, width: 60, height: 20 });
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 40,
            row: 10,
            modifiers: KeyModifiers::NONE,
        })
        .expect("mouse");
        assert_eq!(a.focus, Focus::Agent);
    }

    #[test]
    fn a_click_outside_the_pane_comes_back_to_weft() {
        let mut a = app();
        a.focus = Focus::Agent;
        a.note_pane_area(ratatui::layout::Rect { x: 19, y: 2, width: 60, height: 20 });
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 3,
            modifiers: KeyModifiers::NONE,
        })
        .expect("mouse");
        assert_eq!(a.focus, Focus::Weft);
    }
}
