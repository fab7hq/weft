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
        self.landed = true;
        self.sync_panes();
        if self.layout_bar {
            self.on_layout_key(key.code);
            return Ok(());
        }
        // The tree's search takes what is typed into it.
        if self.focus == Focus::Weft && self.tree_search_key(&key) {
            return Ok(());
        }
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
        // A pane selected: Enter enters it, ← goes back to the list, and the
        // arrows go back to picking rows there.
        if self.pane_selected && self.detail.is_none() {
            match action {
                Action::Open => {
                    self.focus = Focus::Agent;
                    return Ok(());
                }
                Action::Back => {
                    self.pane_selected = false;
                    self.show_work = true;
                    return Ok(());
                }
                Action::Pick(_) => self.pane_selected = false,
                _ => {}
            }
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
                    self.move_in_detail(delta as i32);
                } else {
                    self.pick(delta);
                }
            }
            Action::Open => {
                if self.waiting_here() {
                    // [Enter] ANSWER IT: the person answers, never Weft.
                    self.focus = Focus::Agent;
                } else if self.detail.is_some() {
                    self.follow_link();
                } else {
                    self.open_row();
                }
            }
            Action::Back => self.back(),
            Action::NextNeedsYou => self.next_needs_you(),
            Action::ToggleSidebar => self.act(Act::ToggleSidebar),
            Action::Layout => {
                self.layout_bar = true;
                self.pane_selected = true;
            }
            // Picking an agent hands it the keys, as Enter on its harness does.
            Action::NextPane => {
                let tabs = self.tabs();
                if !tabs.is_empty() {
                    let next = tabs.iter().position(|t| t.active).map_or(0, |at| at + 1);
                    let t = &tabs[next % tabs.len()];
                    self.show_tab(t.project, t.pane);
                }
            }
            // The tab's digit: the tabs are every open project's agents running.
            Action::PickPane(n) => {
                if let Some(t) = self.tabs().get((n as usize).saturating_sub(1)) {
                    self.show_tab(t.project, t.pane);
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
            // In a session's Details, R resumes it (T11.5).
            Action::Remote => {
                if !self.resume_shown() {
                    self.act(Act::Remote)
                }
            }
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
        self.sync_panes();
        let at = ratatui::layout::Position { x: m.column, y: m.row };
        match m.kind {
            // A tab is taken by the press and shown by the release, unless
            // it was dragged: then it lands where it was let go.
            MouseEventKind::Down(MouseButton::Left) => {
                let tab = self.hits.iter().find(|(area, _)| area.contains(at)).map(|(_, t)| *t);
                if let (Some(Target::Tab(project, pane)), None) = (tab, &self.modal) {
                    let id = self.projects.get(project).and_then(|p| p.session.panes.get(pane));
                    if let Some(id) = id.map(|p| p.id) {
                        let at = (m.column, m.row);
                        self.drag = Some(Drag { agent: (project, id), at, moved: false });
                        return;
                    }
                }
                return self.on_click(m.column, m.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(d) = &mut self.drag {
                    d.moved |= d.at != (m.column, m.row);
                    d.at = (m.column, m.row);
                }
                return;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(d) = self.drag.take() else { return };
                if !d.moved {
                    return self.on_click(m.column, m.row);
                }
                if let Some(to) = self.drop_at(d.at) {
                    self.layout(|p, area| p.drop_tab(d.agent, to, area));
                }
                return;
            }
            _ => {}
        }
        let delta = match m.kind {
            MouseEventKind::ScrollUp => 3,
            MouseEventKind::ScrollDown => -3,
            _ => return,
        };
        if self.focus != Focus::Agent {
            return;
        }
        // An agent that asked for mouse reports scrolls itself, as in any
        // terminal: the wheel goes to it, where the mouse is on its screen.
        let at = ratatui::layout::Position { x: m.column, y: m.row };
        let (column, row) = self
            .hits
            .iter()
            .find(|(area, t)| *t == Target::Screen(self.pane_focus) && area.contains(at))
            .map_or((1, 1), |(area, _)| (m.column - area.x + 1, m.row - area.y + 1));
        let report =
            sess!(self).panes.get(self.pane_focus).and_then(|p| p.wheel(delta > 0, column, row));
        if let Some(bytes) = report {
            let _ = self.input(self.pane_focus, &bytes);
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
            Target::Tab(project, pane) => self.show_tab(project, pane),
            Target::Bar(_) => {}
            // Another pane: it takes the keys.
            Target::Pane(id) => {
                self.layout(|p, _| {
                    p.focus(id);
                    Ok(())
                });
                self.focus = Focus::Agent;
            }
            // The wheel's place on an agent's screen; a click there is the agent's own.
            Target::Screen(_) => {}
        }
    }

    /// Where a tab let go at a place lands: its place on a tab bar, a pane's
    /// outer fifth, or a pane's middle. Nowhere, off them.
    pub fn drop_at(&self, (x, y): (u16, u16)) -> Option<crate::panes::Drop> {
        use crate::panes::{Drop, Side};
        let at = ratatui::layout::Position { x, y };
        let (area, target) = self
            .hits
            .iter()
            .find(|(area, t)| matches!(t, Target::Bar(_) | Target::Pane(_)) && area.contains(at))?;
        match *target {
            Target::Bar(pane) => {
                let tabs = &self.panes.pane(pane)?.tabs;
                let mid = |&(project, id): &crate::panes::Agent| {
                    let i = self.running_pane(project, id)?;
                    self.hits.iter().find_map(|(r, t)| {
                        (*t == Target::Tab(project, i) && r.y == area.y)
                            .then_some(r.x + r.width / 2)
                    })
                };
                let at = tabs.iter().position(|t| mid(t).is_some_and(|m| m > x));
                Some(Drop::Bar { pane, at: at.unwrap_or(tabs.len()) })
            }
            Target::Pane(pane) => {
                let fx = f32::from(x - area.x) / f32::from(area.width.max(1));
                let fy = f32::from(y - area.y) / f32::from(area.height.max(1));
                let edges = [
                    (fx, Side::Left),
                    (1.0 - fx, Side::Right),
                    (fy, Side::Up),
                    (1.0 - fy, Side::Down),
                ];
                let (near, side) = edges
                    .into_iter()
                    .fold((f32::MAX, Side::Left), |best, e| if e.0 < best.0 { e } else { best });
                Some(if near < 0.2 { Drop::Edge { pane, side } } else { Drop::Middle { pane } })
            }
            _ => None,
        }
    }

    /// The layout bar's keys; every other key is ignored until Esc.
    fn on_layout_key(&mut self, code: event::KeyCode) {
        use crate::panes::Axis;
        use event::KeyCode as K;
        match code {
            K::Right => self.layout(|p, area| p.split(Axis::Right, area)),
            K::Down => self.layout(|p, area| p.split(Axis::Down, area)),
            // Tab selects the next pane, as it selects the next agent elsewhere.
            K::Tab => self.layout(|p, _| {
                let ids: Vec<u32> = p.panes().iter().map(|x| x.id).collect();
                if let Some(at) = ids.iter().position(|&id| id == p.active().id) {
                    p.focus(ids[(at + 1) % ids.len()]);
                }
                Ok(())
            }),
            K::Char('m' | 'M') => self.layout(|p, _| {
                p.to_next_pane();
                Ok(())
            }),
            K::Char('j' | 'J') => self.layout(|p, _| {
                p.join();
                Ok(())
            }),
            K::Esc | K::Left => self.layout_bar = false,
            _ => {}
        }
    }

    /// `Ctrl+]`: into the agent selected (from the list, the pane with the
    /// keys), or out of it, the agent still selected.
    pub(crate) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Weft => Focus::Agent,
            Focus::Agent => {
                self.pane_selected = true;
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
        if modal == Modal::Remote {
            self.on_remote_key(key);
            return Ok(());
        }
        if let Modal::Allow { device, .. } = &modal {
            self.on_allow_key(key, device);
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
                // SEND TO is a list, picked as Remote's devices are.
                event::KeyCode::Up | event::KeyCode::Down => {
                    let running = self.running_panes();
                    let n = running.len().max(1);
                    let at = running.iter().position(|&i| i == target).unwrap_or(0);
                    let step = if key.code == event::KeyCode::Up { n - 1 } else { 1 };
                    target = running.get((at + step) % n).copied().unwrap_or(target);
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
            Modal::OpenProject { .. } => 1,
            Modal::StartAgent { .. } => self.fresh_starts().len().max(1),
            _ => 1,
        };
        match key.code {
            // The agent picker is the same picker wherever it is drawn.
            event::KeyCode::Up if matches!(modal, Modal::StartAgent { .. }) => self.pick_agent(-1),
            event::KeyCode::Down if matches!(modal, Modal::StartAgent { .. }) => self.pick_agent(1),
            event::KeyCode::Up => self.modal_choice = self.modal_choice.saturating_sub(1),
            event::KeyCode::Down => self.modal_choice = (self.modal_choice + 1).min(options - 1),
            event::KeyCode::Char(c) if modal == Modal::Weft => {
                if let Some(at) = WEFT_MENU.iter().position(|(k, ..)| k.eq_ignore_ascii_case(&c)) {
                    self.menu_act(at);
                }
            }
            event::KeyCode::Left | event::KeyCode::Esc => match &modal {
                // A question nobody is going to answer is taken off the
                // daemon's queue, not left there for another client.
                Modal::Bar(Bar::Send(p)) => {
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
            Modal::Ask { .. }
            | Modal::RingFrame
            | Modal::Locate { .. }
            | Modal::Remote
            | Modal::Allow { .. } => {}
            Modal::Bar(Bar::Send(pending)) => {
                let pending = pending.clone();
                self.do_inject(&pending);
            }
            Modal::Bar(Bar::SendAnyway { pending, .. }) => {
                let pending = pending.clone();
                self.modal = None;
                self.do_inject_forcing(&pending, true);
            }
            Modal::StartAgent { .. } => self.start_chosen_agent(self.modal_choice),
            // Its own keyboard, handled above.
            Modal::OpenProject { .. } => {}
            Modal::CloseProject { name } => {
                self.modal = None;
                if self.modal_choice == 0
                    && let Some(at) = self.projects.iter().position(|p| p.name == *name)
                {
                    // The connection goes; the agents and the record do not.
                    self.projects.remove(at);
                    self.panes.forget_project(at);
                    self.at = self.at.min(self.projects.len() - 1);
                    self.pane_focus = 0;
                    self.selected = 0;
                    self.take_the_board();
                }
            }
            Modal::Weft => self.menu_act(self.modal_choice.min(WEFT_MENU.len() - 1)),
            Modal::Quit => match self.modal_choice {
                0 => {
                    // Leave; the server keeps the agents working.
                    sess!(self).detach();
                    self.quit = true;
                }
                1 => {
                    // The agents of the projects in this window, not the
                    // daemon: another window's agents keep running.
                    self.stop_agents_on_quit = true;
                    for open in &mut self.projects {
                        open.session.stop_agents();
                        open.session.detach();
                    }
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
