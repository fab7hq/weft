//! What the person does: keys, pastes and clicks, each turned into what
//! the window does about it.

use super::*;

impl App {
    /// One keystroke. Public so a probe or a wireframe run can drive the
    /// same path the terminal does.
    pub fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.modal.is_some() {
            return self.on_modal_key(key);
        }
        self.hint = None;
        let chord = chord_of(key);
        // Esc is the agent's, except where a view of Weft's own is open.
        let closing = key.code == event::KeyCode::Esc
            && self.focus == Focus::Weft
            && (self.detail().is_some() || self.eval.is_some());
        let action =
            if closing { Action::Back } else { keys::route(chord, self.focus, self.toggle) };
        // The Eval view blocks: its keys are its own.
        if self.eval.is_some() && !matches!(action, Action::ToggleFocus | Action::ToAgent) {
            self.on_eval_action(action);
            return Ok(());
        }
        match action {
            Action::EvalView => self.open_eval_view(),
            Action::Diff => {}
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
    pub(crate) fn on_click(&mut self, column: u16, line: u16) {
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

    pub(crate) fn toggle_focus(&mut self) {
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

    pub(crate) fn on_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        let modal = self.modal.clone().expect("a modal");

        // Where a harness's program is: typed, like a project's path.
        if let Modal::Locate { harness, mut text } = modal {
            match key.code {
                event::KeyCode::Left | event::KeyCode::Esc => self.modal = Some(Modal::RingFrame),
                event::KeyCode::Enter if !text.trim().is_empty() => {
                    self.setup_locate(&harness, &text)
                }
                event::KeyCode::Backspace => {
                    text.pop();
                    self.modal = Some(Modal::Locate { harness, text });
                }
                event::KeyCode::Char(c) => {
                    text.push(c);
                    self.modal = Some(Modal::Locate { harness, text });
                }
                _ => {}
            }
            return Ok(());
        }
        if modal == Modal::RingFrame {
            let rows = self.setup_view().map_or(0, |v| v.rows.len());
            match key.code {
                event::KeyCode::Up => self.modal_choice = self.modal_choice.saturating_sub(1),
                event::KeyCode::Down => {
                    self.modal_choice = (self.modal_choice + 1).min(rows.saturating_sub(1))
                }
                event::KeyCode::Enter => self.setup_selected(),
                event::KeyCode::Char('p' | 'P') => self.setup_all(),
                event::KeyCode::Left | event::KeyCode::Esc => self.modal = None,
                _ => {}
            }
            return Ok(());
        }

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

    pub(crate) fn confirm_modal(&mut self, modal: &Modal) {
        match modal {
            Modal::Help | Modal::Note(_) => self.modal = None,
            // Their keys are their own, above; Enter never reaches here.
            Modal::Ask { .. } | Modal::RingFrame | Modal::Locate { .. } => {}
            Modal::Confirm(pending) => {
                let pending = pending.clone();
                self.do_inject(&pending);
            }
            Modal::StartAgent { .. } => self.start_chosen_agent(self.modal_choice),
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
    pub(crate) fn on_paste(&mut self, text: &str) -> Result<()> {
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
