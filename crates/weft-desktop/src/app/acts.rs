//! What each action does, by the part of the window it comes from. Every
//! call to the daemon is queued; its answer is taken by `pump`.

use std::path::{Path, PathBuf};

use weft_core::board::Act;
use weft_core::state::{PaneViewMode, SavedPaneTab};
use weft_core::tree;
use weft_core::words;

use super::{ActiveSheet, BottomBarState, WeftDesktopApp};
use crate::action::{Action, Answer, BarAct, Focus, PaneAct, SheetAct, TreeAct, WindowAct};
use crate::daemon::Then;
use crate::panes_view::{join_all_panes, move_tab_to_next_pane, split_pane_down, split_pane_right};
use crate::text_field::{Shape, TextField};
use crate::tree_view::TreeSelection;

impl WeftDesktopApp {
    /// Do what a click or a key asked.
    pub fn dispatch(&mut self, action: impl Into<Action>) {
        let action = action.into();
        // A popover closes on anything but its own toggles.
        if !matches!(
            action,
            Action::Window(
                WindowAct::ToggleMenu | WindowAct::ToggleTurbo | WindowAct::ToggleStartAtLogin
            ) | Action::Tree(TreeAct::TogglePlus(_))
                | Action::Pane(PaneAct::ToggleLayoutMenu(_))
        ) {
            self.close_popovers();
        }
        match action {
            Action::Window(a) => self.window_act(a),
            Action::Tree(a) => self.tree_act(a),
            Action::Bar(a) => self.bar_act(a),
            Action::Sheet(a) => self.sheet_act(a),
            Action::Pane(a) => self.pane_act(a),
        }
    }

    fn close_popovers(&mut self) {
        self.menu_open = false;
        self.plus_popover_project = None;
        self.layout_menu_pane = None;
    }

    fn why_not(&mut self, sentence: impl Into<String>) {
        self.bottom_bar = BottomBarState::WhyNot { sentence: sentence.into() };
    }

    fn window_act(&mut self, a: WindowAct) {
        match a {
            WindowAct::ToggleSidebar => {
                self.state.settings.sidebar_open = !self.state.settings.sidebar_open;
                let _ = self.save_state();
            }
            WindowAct::ToggleMenu => {
                self.menu_open = !self.menu_open;
                self.plus_popover_project = None;
                self.layout_menu_pane = None;
            }
            WindowAct::ClosePopovers => {}
            WindowAct::ToggleTurbo => {
                let next = !self.state.settings.turbo;
                self.state.settings.turbo = next;
                for c in &mut self.clients {
                    let _ = c.set_turbo(next);
                }
                let _ = self.save_state();
            }
            WindowAct::ToggleStartAtLogin => {
                let next = !self.state.settings.start_at_login;
                self.state.settings.start_at_login = next;
                let path = std::env::var("PATH").unwrap_or_default();
                // The service runs the installed link, so an update that moves
                // it is the one it starts; else the `weft` beside this app.
                let link = std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".local/bin/weft"))
                    .filter(|l| l.exists());
                let program = link
                    .or_else(|| crate::daemon::find_weft_exe().ok())
                    .map_or_else(|| "weft".to_string(), |p| p.to_string_lossy().into_owned());
                for c in &mut self.clients {
                    let _ = c.set_start_at_login(next, &program, &path);
                }
                let _ = self.save_state();
            }
            // The window asks the system for the folder, then calls `open_project_path`.
            WindowAct::OpenProject => {}
            WindowAct::OpenUpdates => {
                for c in &mut self.clients {
                    let _ = c.setup("look", "", "");
                }
                self.active_sheet = Some(ActiveSheet::Updates);
            }
            WindowAct::OpenRemote => self.active_sheet = Some(ActiveSheet::Remote),
            WindowAct::RequestQuit => {
                if self.clients.iter().any(|c| c.panes.iter().any(|p| p.running)) {
                    self.bottom_bar = BottomBarState::QuitConfirm;
                } else {
                    self.quit_requested = true;
                }
            }
            WindowAct::Focus(f) => self.focus = f,
        }
    }

    fn tree_act(&mut self, a: TreeAct) {
        match a {
            TreeAct::SelectProject(p) => {
                self.selection = Some(TreeSelection::Project { project: p })
            }
            TreeAct::ToggleProject(p) => {
                match self.state.folded_projects.iter().position(|f| *f == p) {
                    Some(at) => {
                        self.state.folded_projects.remove(at);
                    }
                    None => self.state.folded_projects.push(p),
                }
                let _ = self.save_state();
            }
            TreeAct::CloseProject(p) => self.close_project(&p),
            TreeAct::TogglePlus(p) => {
                if self.plus_popover_project.as_ref() == Some(&p) {
                    self.plus_popover_project = None;
                } else {
                    self.plus_popover_project = Some(p);
                    self.menu_open = false;
                    self.layout_menu_pane = None;
                }
            }
            TreeAct::StartAgent { project, harness } => {
                self.start_agent_in_project(&project, &harness, None);
            }
            TreeAct::SelectAgent { project, session, pane } => {
                self.selection = Some(TreeSelection::Agent {
                    project: project.clone(),
                    session: session.clone(),
                });
                self.focus_or_open_agent_session(&project, &session, pane);
            }
            TreeAct::OpenEarlier(p) => {
                if !self.unfolded_earlier.remove(&p) {
                    self.unfolded_earlier.insert(p.clone());
                }
                self.selection = Some(TreeSelection::EarlierSessions { project: p });
            }
            TreeAct::ToggleOpen(p) => {
                if !self.folded_open.remove(&p) {
                    self.folded_open.insert(p);
                }
            }
            TreeAct::SelectOpen(p) => {
                self.selection = Some(TreeSelection::OpenGroup { project: p })
            }
            TreeAct::SelectAsk { project, unit } => {
                if let Some(c) = self.client_mut(&project) {
                    let _ = c.read("source", &unit, None, Then::Source { unit: unit.clone() });
                    let _ = c.read("wording", &unit, None, Then::Wording { unit: unit.clone() });
                }
                self.selection =
                    Some(TreeSelection::Ask { project: project.clone(), unit: unit.clone() });
                self.active_sheet = Some(ActiveSheet::WorkDetail { project, unit });
            }
            TreeAct::ToggleEval { project, eval } => {
                let key = (project, eval);
                if !self.folded_evals.remove(&key) {
                    self.folded_evals.insert(key);
                }
            }
            TreeAct::SelectEval { project, eval } => {
                self.read_judges(&project, &eval);
                self.selection =
                    Some(TreeSelection::EvalGroup { project: project.clone(), eval: eval.clone() });
                self.active_sheet = Some(ActiveSheet::EvalDetail { project, eval });
            }
            TreeAct::ToggleSealed(p) => {
                if !self.unfolded_sealed.remove(&p) {
                    self.unfolded_sealed.insert(p.clone());
                }
                self.selection = Some(TreeSelection::SealedGroup { project: p });
            }
            TreeAct::SelectSeal { project, seal } => {
                // Whether its receipt still holds is RingFrame's to check, each time it is opened.
                if let Some(c) = self.client_mut(&project) {
                    let _ = c.read("seal", &seal, None, Then::Seal { seal: seal.clone() });
                }
                self.selection =
                    Some(TreeSelection::Seal { project: project.clone(), seal: seal.clone() });
                self.active_sheet = Some(ActiveSheet::SealDetail { project, seal });
            }
            TreeAct::ClearSearch => {
                self.search.set_text("");
                self.focus = Focus::Terminal;
            }
        }
    }

    /// Read an Eval's record through the daemon, when the board's records lack it.
    fn read_judges(&mut self, project: &Path, eval: &str) {
        if let Some(c) = self.client_mut(project)
            && c.records.get(eval).is_none()
        {
            let _ = c.read("judges", eval, None, Then::Judges { eval: eval.to_string() });
        }
    }

    fn bar_act(&mut self, a: BarAct) {
        match a {
            BarAct::Ask => match self.selection.clone() {
                Some(TreeSelection::Agent { project, session }) => {
                    let projects = self.tree_projects();
                    let why = projects
                        .iter()
                        .find(|l| l.root == project)
                        .and_then(|l| l.project.agents.rows.iter().find(|r| r.session == session))
                        .and_then(|r| r.ask.as_ref().map(|w| w.say()));
                    match why {
                        Some(why) => self.why_not(why),
                        None => self.open_ask(project, Some(session), None),
                    }
                }
                // A project's: SEND TO chooses among its running agents.
                Some(TreeSelection::Project { project }) => {
                    if self.client(&project).is_some_and(|c| c.panes.iter().any(|p| p.running)) {
                        self.open_ask(project, None, None);
                    } else {
                        self.why_not(words::word("why.start_an_agent"));
                    }
                }
                _ => self.why_not_for(Act::Ask),
            },
            BarAct::Eval => match self.selection.clone() {
                Some(TreeSelection::OpenGroup { project } | TreeSelection::Ask { project, .. }) => {
                    self.trigger_eval(&project)
                }
                _ => self.why_not_for(Act::Eval),
            },
            BarAct::Seal => match self.selection.clone() {
                Some(TreeSelection::EvalGroup { project, eval }) => self.offer_seal(project, eval),
                _ => self.why_not_for(Act::Seal),
            },
            BarAct::Cancel => self.bottom_bar = BottomBarState::None,
            BarAct::Resolve { project, pending, answer } => {
                let (yes, force) = (answer != Answer::No, answer == Answer::Anyway);
                let resolved =
                    self.client_mut(&project).map(|c| c.resolve(&pending, yes, force, None, None));
                if let Some(Err(e)) = resolved {
                    self.why_not(e.to_string());
                }
            }
            BarAct::PickDisposition { project, eval, disposition } => {
                self.bottom_bar =
                    BottomBarState::SealDisposition { project, eval, selected: disposition };
            }
            BarAct::ConfirmSeal { project, eval, disposition } => {
                if let Some(c) = self.client_mut(&project) {
                    let pane = c.panes.iter().find(|p| p.running).map(|p| p.id);
                    let _ = c.act("seal", None, pane, Some(&disposition), Some(&eval), None);
                }
                self.bottom_bar = BottomBarState::None;
                self.active_sheet = None;
            }
            BarAct::LeaveRunningAndQuit => {
                self.bottom_bar = BottomBarState::None;
                self.quit_requested = true;
            }
            BarAct::StopAgentsAndQuit => {
                for c in &mut self.clients {
                    c.stop_agents();
                }
                self.bottom_bar = BottomBarState::None;
                self.quit_requested = true;
            }
            BarAct::AllowDevice { device, yes } => {
                let act = serde_json::json!({"act": "allow", "device": device, "yes": yes});
                for c in &mut self.clients {
                    let _ = c.remote_act(act.clone());
                }
                self.bottom_bar = BottomBarState::None;
            }
        }
    }

    /// Why the bar's `act` does nothing for what is selected.
    fn why_not_for(&mut self, act: Act) {
        if let Some(key) = tree::select(act) {
            self.why_not(words::word(key));
        }
    }

    /// The Seal's disposition bar for `eval`, unless the tree says why not.
    fn offer_seal(&mut self, project: PathBuf, eval: String) {
        let why = self
            .tree_projects()
            .into_iter()
            .find(|l| l.root == project)
            .and_then(|l| l.project.work.evals.into_iter().find(|g| g.eval == eval))
            .and_then(|g| g.seal.map(|w| w.say()));
        match why {
            Some(why) => self.why_not(why),
            None => {
                self.bottom_bar =
                    BottomBarState::SealDisposition { project, eval, selected: "accepted".into() }
            }
        }
    }

    fn open_ask(&mut self, project: PathBuf, session: Option<String>, unit: Option<String>) {
        // No agent named: the one in front when it is this project's, else its
        // first running one. Named here, so the sheet shows where it goes.
        let session = session.or_else(|| {
            let front = self.active_tab().filter(|t| t.project == project).map(|t| t.pane);
            let listed = self.tree_projects().into_iter().find(|l| l.root == project)?;
            let running: Vec<_> = listed
                .project
                .agents
                .rows
                .into_iter()
                .filter(|a| a.state == weft_core::sessions::State::Running)
                .collect();
            let row = running.iter().find(|a| a.pane.is_some() && a.pane == front);
            row.or(running.first()).map(|a| a.session.clone())
        });
        self.ask = TextField::new(Shape::Area);
        self.active_sheet =
            Some(ActiveSheet::Ask { project, target_session: session, follow_up_unit: unit });
        self.focus = Focus::Ask;
    }

    fn sheet_act(&mut self, a: SheetAct) {
        match a {
            SheetAct::Close => {
                self.active_sheet = None;
                self.focus = Focus::Terminal;
            }
            SheetAct::AskTarget { session } => {
                if let Some(ActiveSheet::Ask { target_session, .. }) = &mut self.active_sheet {
                    *target_session = Some(session);
                }
            }
            SheetAct::SubmitAsk => {
                if let Some(ActiveSheet::Ask { project, target_session, follow_up_unit }) =
                    self.active_sheet.clone()
                {
                    let draft = self.ask.text().to_string();
                    // Not asked, the Ask stays open with what was written.
                    if self.submit_ask(
                        &project,
                        target_session.as_deref(),
                        follow_up_unit.as_deref(),
                        &draft,
                    ) {
                        self.active_sheet = None;
                        self.focus = Focus::Terminal;
                    }
                }
            }
            SheetAct::FollowUp { project, unit } => self.open_ask(project, None, Some(unit)),
            SheetAct::Proceed { project, unit } => {
                if let Some(c) = self.client_mut(&project) {
                    let pane = c.panes.iter().find(|p| p.running).map(|p| p.id);
                    let _ = c.confirm_ask(&unit);
                    let _ = c.act("proceed", Some(&unit), pane, None, None, None);
                }
                self.active_sheet = None;
            }
            SheetAct::Seal { project, eval } => self.offer_seal(project, eval),
            SheetAct::EvalView { project, eval } => {
                if let Some(c) = self.client_mut(&project) {
                    let _ = c.read("eval", &eval, None, Then::EvalView { eval: eval.clone() });
                }
                self.active_sheet =
                    Some(ActiveSheet::EvalView { project, eval, by_file: false, at: Vec::new() });
            }
            SheetAct::ToggleEvalViewMode => {
                if let Some(ActiveSheet::EvalView { by_file, at, .. }) = &mut self.active_sheet
                    && at.is_empty()
                {
                    *by_file = !*by_file;
                }
            }
            SheetAct::EvalOpen(key) => {
                let Some(ActiveSheet::EvalView { project, eval, at, .. }) = &mut self.active_sheet
                else {
                    return;
                };
                at.push(key.clone());
                let (project, eval) = (project.clone(), eval.clone());
                if let Some(window) = key.strip_prefix("c:")
                    && let Some(c) = self.client_mut(&project)
                {
                    let then = Then::Window { eval: eval.clone(), window: window.to_string() };
                    let _ = c.read("window", &format!("{eval}/{window}"), None, then);
                }
            }
            SheetAct::EvalBack => match &mut self.active_sheet {
                Some(ActiveSheet::EvalView { at, .. }) if !at.is_empty() => {
                    at.pop();
                }
                Some(ActiveSheet::EvalView { project, eval, .. }) => {
                    let (project, eval) = (project.clone(), eval.clone());
                    self.tree_act(TreeAct::SelectEval { project, eval });
                }
                _ => {}
            },
            SheetAct::Resume { project, harness, session } => {
                if self.start_agent_in_project(&project, &harness, Some(&session)) {
                    self.active_sheet = None;
                }
            }
            SheetAct::SetupAll => self.setup_everywhere("all", ""),
            SheetAct::SetupRun(target) => self.setup_everywhere("run", &target),
            // The window asks the system for the program, then calls `locate`.
            SheetAct::Locate(_) => {}
            SheetAct::RemoteOff => {
                for c in &mut self.clients {
                    let _ = c.remote_act(serde_json::json!({"act": "off"}));
                }
                self.bottom_bar = BottomBarState::None;
            }
            SheetAct::AskRemoveDevice { device, name } => {
                self.bottom_bar =
                    BottomBarState::RemoveDeviceConfirm { device_id: device, device_name: name };
            }
            SheetAct::RemoveDevice(device) => {
                for c in &mut self.clients {
                    let _ = c.remote_act(serde_json::json!({"act": "remove", "device": device}));
                }
                self.bottom_bar = BottomBarState::None;
            }
        }
    }

    /// A setup act, asked of every connected project; said when none is connected.
    fn setup_everywhere(&mut self, act: &str, harness: &str) {
        let asked: Vec<_> = self.clients.iter_mut().map(|c| c.setup(act, harness, "")).collect();
        if let Some(Err(e)) = asked.into_iter().reduce(|a, b| if a.is_ok() { a } else { b }) {
            self.why_not(e.to_string());
        }
    }

    /// The program the person found for `harness`, told to the daemon.
    pub fn locate(&mut self, harness: &str, path: &Path) {
        let path = path.to_string_lossy();
        for c in &mut self.clients {
            let _ = c.setup("locate", harness, &path);
        }
    }

    fn pane_act(&mut self, a: PaneAct) {
        let layout = |app: &mut Self, done: Result<(), &'static str>| match done {
            Ok(()) => {
                let _ = app.save_state();
            }
            Err(key) => app.why_not(words::word(key)),
        };
        match a {
            PaneAct::ToggleLayoutMenu(g) => {
                if self.layout_menu_pane == Some(g) {
                    self.layout_menu_pane = None;
                } else {
                    self.layout_menu_pane = Some(g);
                    self.menu_open = false;
                    self.plus_popover_project = None;
                }
            }
            PaneAct::SplitRight(g) => {
                let done = split_pane_right(&mut self.state.panes, g);
                layout(self, done)
            }
            PaneAct::SplitDown(g) => {
                let done = split_pane_down(&mut self.state.panes, g);
                layout(self, done)
            }
            PaneAct::MoveTabNext(g) => {
                let done = move_tab_to_next_pane(&mut self.state.panes, g);
                layout(self, done)
            }
            PaneAct::JoinAll => {
                join_all_panes(&mut self.state.panes);
                let _ = self.save_state();
            }
            PaneAct::Focus(g) => {
                if g < self.state.panes.groups.len() {
                    self.state.panes.focused_group = g;
                    let _ = self.save_state();
                }
                self.focus = Focus::Terminal;
            }
            PaneAct::SelectTab { group, tab } => {
                if let Some(gr) = self.state.panes.groups.get_mut(group)
                    && tab < gr.tabs.len()
                {
                    gr.active_tab = tab;
                    self.state.panes.focused_group = group;
                    let _ = self.save_state();
                }
                self.focus = Focus::Terminal;
            }
            PaneAct::DropTab { group, tab, target } => self.drag_pane_tab(group, tab, target),
            PaneAct::CloseTab { group, tab } => self.close_tab(group, tab),
            PaneAct::SetView { group, tab, view } => {
                let Some(t) =
                    self.state.panes.groups.get_mut(group).and_then(|g| g.tabs.get_mut(tab))
                else {
                    return;
                };
                t.view = view;
                let (proj, pane) = (t.project.clone(), t.pane);
                if view == PaneViewMode::Conversation
                    && let Some(c) = self.client_mut(&proj)
                {
                    let _ = c.read("conversation", "", Some(pane), Then::Conversation { pane });
                }
                let _ = self.save_state();
            }
            PaneAct::ToggleSteps { project, pane, block } => {
                let key = (project, pane, block);
                if !self.unfolded_steps.remove(&key) {
                    self.unfolded_steps.insert(key);
                }
            }
            PaneAct::TakeSuggestion => {
                self.take_suggestion();
            }
            PaneAct::SendMessage => {
                let text = self.message_field().text().to_string();
                let Some(tab) = self.active_tab().filter(|_| !text.is_empty()) else { return };
                let mut payload = text.into_bytes();
                payload.push(b'\r');
                let sent = self.client_mut(&tab.project).map(|c| c.input(tab.pane, &payload));
                match sent {
                    // Kept in the field until it is sent.
                    Some(Err(e)) => self.why_not(e.to_string()),
                    _ => self.message_field().set_text(""),
                }
            }
        }
    }

    fn close_tab(&mut self, group: usize, tab: usize) {
        let panes = &mut self.state.panes;
        let Some(g) = panes.groups.get_mut(group).filter(|g| tab < g.tabs.len()) else { return };
        g.tabs.remove(tab);
        if g.active_tab >= g.tabs.len() {
            g.active_tab = g.tabs.len().saturating_sub(1);
        }
        if g.tabs.is_empty() && panes.groups.len() > 1 {
            panes.groups.remove(group);
            if panes.focused_group >= panes.groups.len() {
                panes.focused_group = panes.groups.len().saturating_sub(1);
            }
        }
        let _ = self.save_state();
    }

    fn submit_ask(
        &mut self,
        project: &Path,
        target_session: Option<&str>,
        follow_up_unit: Option<&str>,
        draft: &str,
    ) -> bool {
        // The agent the sheet showed, by its pane.
        let pane = self
            .tree_projects()
            .into_iter()
            .find(|l| l.root == project)
            .and_then(|l| {
                l.project
                    .agents
                    .rows
                    .into_iter()
                    .find(|a| Some(a.session.as_str()) == target_session)
            })
            .and_then(|a| a.pane);
        let Some(client) = self.client_mut(project) else { return false };
        let act = if follow_up_unit.is_some() { "follow_up" } else { "ask" };
        match client.act(act, follow_up_unit, pane, Some(draft), None, None) {
            Ok(_) => true,
            Err(e) => {
                self.why_not(e.to_string());
                false
            }
        }
    }

    /// Open a folder as a project: listed at once, connected by the window's thread.
    pub fn open_project_path(&mut self, path: PathBuf) {
        let path = weft_core::state::normalize_project_path(&path);
        if !self
            .state
            .open_projects
            .iter()
            .any(|p| weft_core::state::normalize_project_path(p) == path)
        {
            self.state.open_projects.push(path.clone());
        }
        self.add_project(&path);
        let _ = self.save_state();
    }

    pub fn close_project(&mut self, root: &Path) {
        if let Some(at) = self.clients.iter().position(|c| c.root == root) {
            let mut c = self.clients.remove(at);
            let _ = c.dismiss();
            c.detach();
        }
        self.not_opened.remove(root);
        self.state.open_projects.retain(|p| p != root);
        self.state.folded_projects.retain(|p| p != root);
        let _ = self.save_state();
    }

    /// Start an agent of `harness` in `project`, or resume `session`; its
    /// tab opens when the daemon answers with its pane. Whether it was asked.
    pub fn start_agent_in_project(
        &mut self,
        project: &Path,
        harness: &str,
        session: Option<&str>,
    ) -> bool {
        let Some(client) = self.client(project) else { return false };
        if let Some(sid) = session
            && let Some(row) = client.sessions.iter().find(|r| r.id == sid)
        {
            if row.state == weft_core::sessions::State::Running
                && let Some(pane) = row.pane
            {
                let tab = SavedPaneTab {
                    project: project.to_path_buf(),
                    pane,
                    view: self.state.settings.default_view,
                };
                self.ensure_tab_in_active_pane(tab);
                let _ = self.save_state();
                return true;
            }
            if !row.resumable && row.state != weft_core::sessions::State::Running {
                self.why_not(words::word("resume.nothing"));
                return false;
            }
        }
        let spec = client.harnesses.find(harness).map(|h| match session {
            Some(sid) => h.resume_spec(sid).unwrap_or_else(|| h.spec()),
            None => h.spec(),
        });
        let Some(spec) = spec else {
            self.why_not(words::fill("desktop.no_harness", &[("harness", harness)]));
            return false;
        };
        let asked = self.client_mut(project).map(|c| c.spawn(harness, &spec, session));
        match asked {
            Some(Ok(_)) => true,
            Some(Err(e)) => {
                self.why_not(e.to_string());
                false
            }
            None => false,
        }
    }

    fn focus_or_open_agent_session(&mut self, project: &Path, session: &str, pane: Option<u32>) {
        let default_view = self.state.settings.default_view;
        // An agent with no session yet is known by its pane.
        if let (true, Some(pane)) = (session.is_empty(), pane) {
            self.active_sheet = None;
            self.ensure_tab_in_active_pane(SavedPaneTab {
                project: project.to_path_buf(),
                pane,
                view: default_view,
            });
            let _ = self.save_state();
            return;
        }
        let Some(row) =
            self.client(project).and_then(|c| c.sessions.iter().find(|r| r.id == session))
        else {
            return;
        };
        match (row.state, row.pane) {
            (weft_core::sessions::State::Running, Some(pane)) => {
                self.active_sheet = None;
                let shown = self.state.panes.groups.iter().enumerate().find_map(|(g, group)| {
                    group
                        .tabs
                        .iter()
                        .position(|t| t.project == project && t.pane == pane)
                        .map(|t| (g, t))
                });
                match shown {
                    Some((g, t)) => {
                        self.state.panes.groups[g].active_tab = t;
                        self.state.panes.focused_group = g;
                    }
                    None => self.ensure_tab_in_active_pane(SavedPaneTab {
                        project: project.to_path_buf(),
                        pane,
                        view: PaneViewMode::Terminal,
                    }),
                }
                let _ = self.save_state();
            }
            _ => {
                self.active_sheet = Some(ActiveSheet::SessionDetail {
                    project: project.to_path_buf(),
                    session: session.to_string(),
                });
            }
        }
    }

    pub fn trigger_eval(&mut self, project: &Path) {
        let why = self
            .tree_projects()
            .into_iter()
            .find(|l| l.root == project)
            .and_then(|l| l.project.work.open.eval.map(|w| w.say()));
        if let Some(why) = why {
            return self.why_not(why);
        }
        if let Some(c) = self.client_mut(project) {
            let pane = c.panes.iter().find(|p| p.running).map(|p| p.id);
            if let Err(e) = c.act("eval", None, pane, None, None, None) {
                self.why_not(e.to_string());
            }
        }
    }
}
