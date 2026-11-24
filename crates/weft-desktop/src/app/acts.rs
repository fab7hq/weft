//! What each action does, by the part of the window it comes from. Every
//! call to the daemon is queued; its answer is taken by `pump`.

use std::path::{Path, PathBuf};

use weft_core::board::Act;
use weft_core::state::{PaneViewMode, SavedPaneTab};
use weft_core::tree;
use weft_core::words;

use super::{ActiveSheet, BottomBarState, EvalPick, SealQuestions, WeftDesktopApp};
use crate::action::{Action, Answer, BarAct, Focus, PaneAct, SheetAct, TreeAct, WindowAct};
use crate::daemon::{Doing, Then, Working};
use crate::panes_view::{join_all_panes, move_tab_to_next_pane, split_pane_down, split_pane_right};
use crate::sheets::send_to::{pane_in, pane_key};
use crate::text_field::{Shape, TextField};
use crate::tree_view::TreeSelection;

impl WeftDesktopApp {
    /// Do what a click or a key asked.
    pub fn dispatch(&mut self, action: impl Into<Action>) {
        let action = action.into();
        // A file's acts touch nothing the main window shows.
        if let Action::File(a) = action {
            return self.file_act(a);
        }
        // What failed is said until the person acts where it is said.
        if self.bar_is_free() {
            self.failed = None;
        }
        // Why the last act did not run goes with the next one.
        if matches!(self.bottom_bar, BottomBarState::WhyNot { .. }) {
            self.bottom_bar = BottomBarState::None;
        }
        // A popover closes on anything but its own toggles.
        if !matches!(
            action,
            Action::Window(
                WindowAct::ToggleMenu | WindowAct::ToggleTurbo | WindowAct::ToggleStartAtLogin
            ) | Action::Tree(TreeAct::TogglePlus(_) | TreeAct::ToggleProjectMenu(_))
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
            Action::File(_) => {}
        }
        self.want_ask_sheet_at(std::time::Instant::now());
    }

    fn close_popovers(&mut self) {
        self.menu_open = false;
        self.project_menu = None;
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
            // The machine's, in config.toml: one daemon tells every project.
            WindowAct::ToggleTurbo => self.turn("turbo", !self.turbo()),
            WindowAct::OpenSettings => {
                self.menu_open = false;
                self.active_sheet =
                    Some(ActiveSheet::Settings { tab: crate::action::SettingsTab::General });
                self.clients.iter_mut().for_each(|c| c.refused.retain(|t, _| !t.is_settings()));
                self.all_rules.clear();
                self.preview_shown = false;
            }
            WindowAct::ToggleStartAtLogin => {
                // The daemon's own setting is what is turned, whatever was saved here.
                let next = !self.clients.iter().any(|c| c.start_at_login);
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
                for c in self.setup_links() {
                    let _ = c.setup("look", "", "");
                }
                self.active_sheet = Some(ActiveSheet::Updates);
            }
            WindowAct::NextNeedsYou => self.next_needs_you(),
            WindowAct::OpenRemote => {
                self.remote_pick = None;
                self.active_sheet = Some(ActiveSheet::Remote);
            }
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
            TreeAct::CloseProject(p) => {
                self.bottom_bar = BottomBarState::CloseProjectConfirm { project: p }
            }
            TreeAct::WhyWaiting { project, session } => {
                let row =
                    self.client(&project).and_then(|c| c.sessions.iter().find(|r| r.id == session));
                if let Some(r) = row {
                    // The hook's own word, or only what Weft read from its screen.
                    let key = if weft_core::turns::is_asking(r.turn) {
                        "desktop.why.asking"
                    } else {
                        "desktop.why.looks_asking"
                    };
                    self.why_not(words::fill(key, &[("harness", &r.harness)]));
                }
            }
            TreeAct::ToggleProjectMenu(p) => {
                self.project_menu =
                    if self.project_menu.as_ref() == Some(&p) { None } else { Some(p) };
            }
            TreeAct::OpenProjectSettings(p) => {
                self.project_menu = None;
                self.choice = None;
                self.active_sheet = Some(ActiveSheet::ProjectSettings { project: p.clone() });
                self.clients.iter_mut().for_each(|c| c.refused.retain(|t, _| !t.is_settings()));
                // The decisions, read again each time: a file may have changed since.
                if let Some(c) = self.clients.iter_mut().find(|c| c.root == p) {
                    let _ = c.decisions();
                }
                self.want_rules();
            }
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
                if self.unfolded_earlier.remove(&p) {
                    // Opened again, it lists one page.
                    if let Some(c) = self.client_mut(&p) {
                        c.earlier_rows.fold();
                    }
                } else {
                    self.unfolded_earlier.insert(p.clone());
                }
                self.selection = Some(TreeSelection::EarlierSessions { project: p });
            }
            TreeAct::MoreEarlier(p) => {
                let found = !self.search.text().trim().is_empty();
                if let Some(c) = self.client_mut(&p) {
                    c.more_earlier(found);
                }
            }
            TreeAct::EarlierAgain(p) => {
                let found = !self.search.text().trim().is_empty();
                if let Some(c) = self.client_mut(&p) {
                    c.earlier_again(found);
                }
            }
            TreeAct::SelectWork { project, unit } => {
                let evals = self
                    .client(&project)
                    .and_then(|c| c.work.of_ask(&unit))
                    .map(|u| u.evals.clone())
                    .unwrap_or_default();
                for e in evals {
                    self.read_judges(&project, &e);
                }
                self.selection =
                    Some(TreeSelection::Work { project: project.clone(), unit: unit.clone() });
                self.sheet_back.clear();
                self.active_sheet = Some(ActiveSheet::Unit { project, unit });
            }
            TreeAct::SelectAsk { project, unit } => {
                if let Some(c) = self.client_mut(&project) {
                    let _ = c.read("source", &unit, None, Then::Source { unit: unit.clone() });
                    let _ = c.read("wording", &unit, None, Then::Wording { unit: unit.clone() });
                }
                self.select_work_of(&project, |w| w.of_ask(&unit));
                self.open_from_sheet(ActiveSheet::WorkDetail { project, unit });
            }
            TreeAct::SelectEval { project, eval } => {
                self.read_judges(&project, &eval);
                self.select_work_of(&project, |w| w.of_eval(&eval));
                self.open_from_sheet(ActiveSheet::EvalDetail { project, eval });
            }
            TreeAct::OpenDone(p) => {
                self.search.set_text("");
                self.selection = Some(TreeSelection::SealedEarlier { project: p.clone() });
                self.done_of = Some(p);
            }
            TreeAct::LeaveDone => {
                self.search.set_text("");
                self.done_of = None;
            }
            TreeAct::SelectSeal { project, seal } => {
                // Whether its receipt still holds is RingFrame's to check, each time it is opened.
                if let Some(c) = self.client_mut(&project) {
                    let _ = c.read("seal", &seal, None, Then::Seal { seal: seal.clone() });
                }
                self.select_work_of(&project, |w| w.units.iter().find(|u| u.seals.contains(&seal)));
                self.open_from_sheet(ActiveSheet::SealDetail { project, seal });
            }
            TreeAct::ClearSearch => {
                self.search.set_text("");
                self.focus = Focus::Terminal;
            }
        }
    }

    /// The Eval view of `eval`, read again from the daemon, with `at` picked.
    fn eval_view(&mut self, project: PathBuf, eval: String, at: Vec<String>) {
        if let Some(c) = self.client_mut(&project) {
            let _ = c.read("eval", &eval, None, Then::EvalView { eval: eval.clone() });
        }
        self.open_from_sheet(ActiveSheet::EvalView { project, eval, at });
    }

    /// Open `next` over the Details shown, which `‹` then goes back to.
    fn open_from_sheet(&mut self, next: ActiveSheet) {
        if let Some(shown) = self.active_sheet.take() {
            let details = matches!(
                shown,
                ActiveSheet::Unit { .. }
                    | ActiveSheet::WorkDetail { .. }
                    | ActiveSheet::EvalDetail { .. }
                    | ActiveSheet::SealDetail { .. }
                    | ActiveSheet::SessionDetail { .. }
            );
            if details && shown != next {
                let name = self.sheet_name(&shown);
                self.sheet_back.push((shown, name));
            }
        }
        self.active_sheet = Some(next);
    }

    /// What a Details sheet is called, as `‹` names it.
    fn sheet_name(&self, sheet: &ActiveSheet) -> String {
        let title = |project: &Path, unit: &str| {
            self.client(project)
                .and_then(|c| c.units.iter().find(|u| u.ask_id == unit))
                .map(|u| u.title.clone())
        };
        let record = |project: &Path| self.client(project).map(|c| &c.work.record);
        let named = match sheet {
            ActiveSheet::Unit { project, unit } | ActiveSheet::WorkDetail { project, unit } => {
                title(project, unit)
            }
            ActiveSheet::EvalDetail { project, eval } => record(project)
                .and_then(|r| r.evals.iter().find(|e| &e.id == eval))
                .map(|e| words::fill("tree.eval", &[("n", &e.n.to_string())])),
            ActiveSheet::SealDetail { project, seal } => record(project)
                .and_then(|r| r.seals.iter().find(|s| &s.id == seal))
                .map(|s| words::fill("tree.seal", &[("n", &s.n.to_string())])),
            ActiveSheet::SessionDetail { session, .. } => Some(session.clone()),
            _ => None,
        };
        named.unwrap_or_else(|| words::word("web.detail.title").to_string())
    }

    /// Select the unit of work `find` names in this project, when there is one.
    fn select_work_of(
        &mut self,
        project: &Path,
        find: impl Fn(&weft_core::work::Work) -> Option<&weft_core::work::UnitOfWork>,
    ) {
        let unit = self.client(project).and_then(|c| find(&c.work)).map(|u| u.root.clone());
        if let Some(unit) = unit {
            self.selection = Some(TreeSelection::Work { project: project.to_path_buf(), unit });
        }
    }

    /// A unit of work's row in the tree.
    fn work_row(&self, project: &Path, unit: &str) -> Option<tree::WorkRow> {
        let listed = self.tree_projects().into_iter().find(|l| l.root == project)?;
        listed.work_row(unit).cloned()
    }

    /// Eval the unit of work, unless its row says why not. What Weft reads
    /// of the harness's setup only warns, in the dialog.
    fn eval_work(&mut self, project: PathBuf, unit: String) {
        let why = self.work_row(&project, &unit).and_then(|w| w.eval);
        if let Some(why) = why.as_ref().filter(|w| !w.is_setup()) {
            return self.why_not(why.say());
        }
        self.act_warning = why.map(|w| w.warning());
        let Some(c) = self.client_mut(&project) else { return };
        let pane = c.panes.iter().find(|p| p.running).map(|p| p.id);
        match c.eval(&unit, pane, None) {
            Err(e) => self.why_not(e.to_string()),
            Ok(_) => {
                self.eval_pick = Some(EvalPick {
                    project,
                    unit,
                    picked: None,
                    was: None,
                    open: false,
                    pending: None,
                    replacing: None,
                    dropped: false,
                })
            }
        }
    }

    /// Another harness picked to read the Eval: its dialog is staged again
    /// for it, and the one shown goes once the new one is there. Nothing
    /// runs until Eval. A pick still being staged takes no other.
    fn read_by(&mut self, harness: String) {
        let Some(pick) = self.eval_pick.as_mut() else { return };
        pick.open = false;
        let pick = pick.clone();
        let shown = pick
            .picked
            .clone()
            .or_else(|| self.eval_reader(&pick.project, &pick.unit).map(|(h, _)| h));
        if pick.pending.is_none() || shown.as_deref() == Some(harness.as_str()) {
            return;
        }
        let Some(c) = self.client_mut(&pick.project) else { return };
        let pane = c.panes.iter().find(|p| p.running).map(|p| p.id);
        let warning = weft_core::tree::Why::not_ready(c.readiness_of(&harness), &harness)
            .map(|w| w.warning());
        match c.eval(&pick.unit, pane, Some(&harness)) {
            Err(e) => self.why_not(e.to_string()),
            Ok(_) => {
                self.act_warning = warning;
                self.eval_pick = Some(EvalPick {
                    was: pick.picked.clone(),
                    picked: Some(harness),
                    replacing: pick.pending.clone(),
                    pending: None,
                    ..pick
                });
            }
        }
    }

    /// The Seal's disposition bar for the unit of work, unless its row says
    /// why not. What Weft reads of the harness's setup only warns, in the bar.
    fn seal_work(&mut self, project: PathBuf, unit: String) {
        let why = self.work_row(&project, &unit).and_then(|w| w.seal);
        if let Some(why) = why.as_ref().filter(|w| !w.is_setup()) {
            return self.why_not(why.say());
        }
        self.act_warning = why.map(|w| w.warning());
        let eval = self.client(&project).and_then(|c| {
            let u = c.work.of_ask(&unit)?;
            c.work.to_seal(u).map(|e| e.id.clone())
        });
        match eval {
            Some(eval) => {
                // Its Eval's questions come first; until they are read the
                // Seal waits.
                let then = Then::Questions { eval: eval.clone() };
                let read =
                    self.client_mut(&project).map(|c| c.read("questions", &eval, None, then));
                let questions = match read {
                    Some(Ok(())) => SealQuestions::Reading,
                    _ => SealQuestions::Unasked,
                };
                self.bottom_bar = BottomBarState::SealDisposition {
                    project,
                    eval,
                    selected: "accepted".into(),
                    reason: None,
                    questions,
                }
            }
            None => self.why_not(words::word("why.no_eval")),
        }
    }

    /// A Seal's question is shown: its time counts from now, and one that
    /// takes words has them back in its field, which takes the keyboard.
    pub(super) fn show_seal_question(&mut self) {
        let BottomBarState::SealDisposition { questions: SealQuestions::Asking(a), .. } =
            &mut self.bottom_bar
        else {
            return;
        };
        a.shown = std::time::Instant::now();
        let at = a.answering.at;
        let words = a.answering.shown().filter(|q| q.answers().contains(&"words")).map(|_| {
            a.answering.given[at].as_ref().and_then(|g| g.words.clone()).unwrap_or_default()
        });
        match words {
            Some(w) => {
                self.seal_words.set_text(&w);
                self.focus = Focus::SealWords;
            }
            None if self.focus == Focus::SealWords => self.focus = Focus::Terminal,
            None => {}
        }
    }

    /// The Seal's questions, while it asks them, and its words field.
    fn seal_asking_mut(&mut self) -> Option<(&mut super::SealAsking, &mut TextField)> {
        match &mut self.bottom_bar {
            BottomBarState::SealDisposition { questions: SealQuestions::Asking(a), .. } => {
                Some((a, &mut self.seal_words))
            }
            _ => None,
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
                        .and_then(|r| r.ask.clone());
                    // What Weft reads of the harness's setup is said in the sheet.
                    match why {
                        Some(why) if !why.is_setup() => self.why_not(why.say()),
                        _ => self.open_ask(project, Some(session), None),
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
                Some(TreeSelection::Work { project, unit }) => {
                    // Eval pressed again: an open that failed is said no more.
                    if let Some(c) = self.client_mut(&project) {
                        let root = c.work.of_ask(&unit).map_or(unit.clone(), |w| w.root.clone());
                        c.forget_failed(|j| {
                            j.kind == "eval_open" && j.on.work.as_deref() == Some(root.as_str())
                        });
                    }
                    self.eval_work(project, unit)
                }
                _ => self.why_not_for(Act::Eval),
            },
            BarAct::Seal => match self.selection.clone() {
                Some(TreeSelection::Work { project, unit }) => self.seal_work(project, unit),
                _ => self.why_not_for(Act::Seal),
            },
            BarAct::Cancel => {
                (self.bottom_bar, self.act_warning) = (BottomBarState::None, None);
                if self.focus == Focus::SealWords {
                    self.focus = Focus::Terminal;
                }
            }
            // Picking an answer sends nothing: only Seal does.
            BarAct::SealAnswer(answer) => {
                if let Some((a, words)) = self.seal_asking_mut() {
                    let ms = a.shown.elapsed().as_millis() as u64;
                    // Own words are typed; Not sure in their place clears them.
                    if answer != "words" && a.answering.answer(&answer, None, ms) {
                        words.set_text("");
                    }
                }
            }
            BarAct::SealNext => {
                let moved = self.seal_asking_mut().is_some_and(|(a, words)| {
                    let ms = a.shown.elapsed().as_millis() as u64;
                    let at = a.answering.at;
                    let takes_words =
                        a.answering.shown().is_some_and(|q| q.answers().contains(&"words"));
                    if takes_words {
                        let typed = words.text().trim().to_string();
                        if !typed.is_empty() {
                            a.answering.answer("words", Some(&typed), ms);
                        } else if a.answering.given[at]
                            .as_ref()
                            .is_some_and(|g| g.answer == "words")
                        {
                            // Words taken back: the question waits for an answer again.
                            a.answering.given[at] = None;
                        }
                    }
                    a.answering.forward()
                });
                if moved {
                    self.show_seal_question();
                }
            }
            BarAct::SealBack => {
                if self.seal_asking_mut().is_some_and(|(a, _)| a.answering.back()) {
                    self.show_seal_question();
                }
            }
            BarAct::SealChange(at) => {
                if let Some((a, _)) = self.seal_asking_mut() {
                    a.answering.change(at);
                    self.show_seal_question();
                }
            }
            BarAct::PickCommit(at) => {
                if let BottomBarState::Undo { commits, picked, .. } = &mut self.bottom_bar
                    && at < commits.len()
                {
                    *picked = at;
                }
            }
            BarAct::Undo => {
                if let BottomBarState::Undo { project, unit, commits, picked, .. } =
                    std::mem::replace(&mut self.bottom_bar, BottomBarState::None)
                {
                    let working = self.working_on(&project, Doing::Undoing, &unit);
                    if let Some(c) = self.client_mut(&project) {
                        let id = commits[picked].id.clone();
                        if let Ok(call) = c.act("undo", Some(&unit), None, Some(&id), None, None) {
                            c.owed_for(call, Then::Working(working));
                        }
                    }
                }
            }
            BarAct::TurnOnGitNote => {
                if let BottomBarState::GitNoteConfirm { project } = self.bottom_bar.clone() {
                    self.git_note(&project, Some(serde_json::json!(true)));
                }
                self.bottom_bar = BottomBarState::None;
            }
            BarAct::Resolve { project, pending, answer } => {
                self.act_warning = None;
                // The Eval's dialog says yes for the work picked, read by whom.
                let eval_work = self
                    .eval_pick
                    .as_ref()
                    .filter(|p| p.project == project && p.pending.as_ref() == Some(&pending))
                    .map(|p| {
                        let picked = p.picked.clone();
                        let reader = picked
                            .or_else(|| self.eval_reader(&p.project, &p.unit).map(|(h, _)| h));
                        (p.unit.clone(), reader)
                    });
                // The Eval's dialog answered: its pick goes with it, and a
                // dialog still being staged for a new pick goes when it comes.
                if let Some(p) = self.eval_pick.as_mut().filter(|p| p.project == project) {
                    if p.replacing.as_ref() == Some(&pending) {
                        (p.replacing, p.dropped) = (None, true);
                    } else if p.pending.as_ref() == Some(&pending) {
                        self.eval_pick = None;
                    }
                }
                let yes = answer != Answer::No;
                // Where the person sent it, when not where it was staged.
                let staged = self
                    .client(&project)
                    .and_then(|c| c.waiting.iter().find(|s| s.id == pending))
                    .cloned();
                // A yes waits on the machine, and says what for until it is done.
                let working = staged
                    .as_ref()
                    .filter(|_| yes)
                    .map(|s| self.resolving(&project, s, eval_work.as_ref()));
                let picked = self.staged_to.remove(&pending);
                let (to, start) = match (picked.as_deref(), staged) {
                    (Some(key), Some(staged)) => match key.strip_prefix("start:") {
                        Some(h) => (None, Some(h.to_string())),
                        None => (pane_in(key).filter(|&p| p != staged.pane), None),
                    },
                    _ => (None, None),
                };
                self.send_to_open = false;
                let resolved = self.client_mut(&project).map(|c| {
                    let call = c.resolve(&pending, yes, to, start.as_deref());
                    if let (Ok(id), Some(w)) = (&call, working) {
                        c.owed_for(*id, Then::Working(w));
                    }
                    call
                });
                if let Some(Err(e)) = resolved {
                    self.why_not(e.to_string());
                }
            }
            BarAct::PickDisposition { project, eval, disposition } => {
                if let BottomBarState::SealDisposition {
                    project: p, eval: e, selected, reason, ..
                } = &mut self.bottom_bar
                    && *p == project
                    && *e == eval
                {
                    (*selected, *reason) = (disposition, None);
                }
            }
            BarAct::PickReason { project, eval, disposition, reason } => {
                if let BottomBarState::SealDisposition {
                    project: p,
                    eval: e,
                    selected,
                    reason: r,
                    ..
                } = &mut self.bottom_bar
                    && *p == project
                    && *e == eval
                {
                    (*selected, *r) = (disposition, reason);
                }
            }
            BarAct::ConfirmSeal { project, eval, disposition, reason } => {
                // The Seal goes with every answer, or not at all: never while
                // its questions are read or one waits for an answer.
                let answers = match &self.bottom_bar {
                    BottomBarState::SealDisposition { questions, .. } => match questions {
                        SealQuestions::Reading => return,
                        SealQuestions::Unasked | SealQuestions::Unread(_) => None,
                        SealQuestions::Asking(a) => match a.answering.answers() {
                            Some(answers) => Some(answers),
                            None => return,
                        },
                    },
                    _ => None,
                };
                self.act_warning = None;
                let root = self
                    .client(&project)
                    .and_then(|c| c.work.of_eval(&eval))
                    .map(|u| u.root.clone());
                let about =
                    root.as_deref().map(|r| self.work_title(&project, r)).unwrap_or_default();
                let decision = weft_core::tree::decision(&disposition);
                let about =
                    if about.is_empty() { decision } else { format!("{about} · {decision}") };
                let working = Working { doing: Doing::Sealing, about, work: root };
                if let Some(c) = self.client_mut(&project)
                    && let Ok(id) = c.seal(&eval, &disposition, reason.as_deref(), answers)
                {
                    c.owed_for(id, Then::Working(working));
                }
                self.bottom_bar = BottomBarState::None;
                self.active_sheet = None;
                if self.focus == Focus::SealWords {
                    self.focus = Focus::Terminal;
                }
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
            BarAct::ConfirmCloseProject(p) => {
                self.bottom_bar = BottomBarState::None;
                self.close_project(&p);
            }
            BarAct::ToggleReadBy => {
                if let Some(pick) = self.eval_pick.as_mut() {
                    pick.open = !pick.open;
                }
            }
            BarAct::ReadBy(harness) => self.read_by(harness),
            BarAct::SendTo { pending, to } => {
                self.staged_to.insert(pending, to);
                self.send_to_open = false;
            }
            BarAct::AllowDevice { device, yes } => {
                self.remote_act(serde_json::json!({"act": "allow", "device": device, "yes": yes}));
                self.allow_answered = Some(device);
                self.bottom_bar = self.before_allow.take().unwrap_or(BottomBarState::None);
            }
        }
    }

    /// Why the bar's `act` does nothing for what is selected.
    fn why_not_for(&mut self, act: Act) {
        if let Some(key) = tree::select(act) {
            self.why_not(words::word(key));
        }
    }

    /// Seal from an Eval's Details: its unit of work, unless that Eval is
    /// sealed already.
    fn offer_seal(&mut self, project: PathBuf, eval: String) {
        let (why, unit) = match self.client(&project) {
            Some(c) => (
                weft_core::work::sealed_already(&c.work.record, &eval).map(|w| w.say()),
                c.work.of_eval(&eval).map(|u| u.root.clone()),
            ),
            None => (None, None),
        };
        match (why, unit) {
            (Some(why), _) => self.why_not(why),
            (None, Some(unit)) => self.seal_work(project, unit),
            (None, None) => self.why_not(words::word("why.no_eval")),
        }
    }

    /// What sealed work offers. Follow up reads what its Eval found first;
    /// Undo reads the commits its dialog offers.
    fn offer(&mut self, project: PathBuf, unit: String, offer: weft_core::after_seal::Offer) {
        use weft_core::after_seal::Offer;
        let Some(c) = self.client_mut(&project) else { return };
        match offer {
            Offer::FollowUp => {
                let _ = c.read("remedy", &unit, None, Then::Remedy { unit: unit.clone() });
            }
            Offer::Undo => {
                let _ = c.read("commits", &unit, None, Then::Commits { unit: unit.clone() });
            }
        }
    }

    /// A project's own Git note setting, set or taken out.
    fn git_note(&mut self, project: &Path, value: Option<serde_json::Value>) {
        if let Some(c) = self.client_mut(project) {
            let _ = c.set_config("project", &["export", "git_note"], value);
        }
    }

    /// A follow-up's Ask, its words already written.
    pub(crate) fn open_follow_up(&mut self, project: PathBuf, unit: String, words: &str) {
        self.open_ask(project, None, Some(unit));
        self.ask.set_text(words);
    }

    /// The pane of the agent an Ask's work is with: where it arrived, else a
    /// running agent of its harness.
    fn work_pane(&self, project: &Path, ask: &str) -> Option<u32> {
        let u = self.client(project)?.units.iter().find(|u| u.ask_id == ask)?;
        let session =
            u.arrived.as_ref().map(|a| a.session.clone()).or_else(|| u.session_ref.clone());
        if let Some(pane) = session.and_then(|s| self.pane_of(project, &s)) {
            return Some(pane);
        }
        let listed = self.tree_projects().into_iter().find(|l| l.root == project)?;
        listed
            .project
            .agents
            .rows
            .into_iter()
            .filter(|a| a.state == weft_core::sessions::State::Running && a.harness == u.harness)
            .find_map(|a| a.pane)
    }

    fn open_ask(&mut self, project: PathBuf, session: Option<String>, unit: Option<String>) {
        self.send_to_open = false;
        // The agent named, else the one in front when it is this project's,
        // else its first running one: picked here, so the sheet shows where
        // it goes. An agent is picked by its pane.
        let named = session.and_then(|s| self.pane_of(&project, &s));
        // A follow-up goes to the work's own agent: its session's pane, else
        // a running agent of its harness.
        let work = unit.as_deref().and_then(|u| self.work_pane(&project, u));
        let pane = named.or(work).or_else(|| {
            let front = self.active_tab().filter(|t| t.project == project).map(|t| t.pane);
            let listed = self.tree_projects().into_iter().find(|l| l.root == project)?;
            let running: Vec<u32> = listed
                .project
                .agents
                .rows
                .into_iter()
                .filter(|a| a.state == weft_core::sessions::State::Running)
                .filter_map(|a| a.pane)
                .collect();
            running.iter().copied().find(|&p| Some(p) == front).or(running.first().copied())
        });
        self.ask = TextField::new(Shape::Area);
        (self.ask_sheet, self.ask_choices, self.ask_sheet_for) = (None, Default::default(), None);
        (self.ask_refused, self.ask_typed) = (None, None);
        self.active_sheet =
            Some(ActiveSheet::Ask { project, send_to: pane.map(pane_key), follow_up_unit: unit });
        self.focus = Focus::Ask;
    }

    fn sheet_act(&mut self, a: SheetAct) {
        match a {
            SheetAct::Back => {
                if let Some((sheet, _)) = self.sheet_back.pop() {
                    self.active_sheet = Some(sheet);
                }
            }
            SheetAct::SettingsTab(tab) => {
                if let Some(ActiveSheet::Settings { tab: shown }) = &mut self.active_sheet {
                    *shown = tab;
                }
                self.choice = None;
                // How each harness stands, as Updates asks it.
                if tab == crate::action::SettingsTab::Harnesses {
                    for c in self.setup_links() {
                        let _ = c.setup("look", "", "");
                    }
                }
                self.want_rules();
            }
            SheetAct::PreviewPick(i, value) => {
                self.choice = None;
                if let Some(slot) = self.preview.get_mut(i) {
                    *slot = value;
                }
                self.want_rules();
            }
            SheetAct::AllRules(catalog) => {
                self.all_rules.insert(catalog);
            }
            SheetAct::ShowHarness(name) => {
                self.harness_shown = name;
                self.choice = None;
                self.want_rules();
            }
            SheetAct::ShowPreview => self.preview_shown = !self.preview_shown,
            SheetAct::ShowJudges => self.judges_shown = !self.judges_shown,
            SheetAct::HowHard(how) => {
                let effort = match how.as_str() {
                    "quick" => Some(serde_json::json!("low")),
                    "thorough" => Some(serde_json::json!("high")),
                    _ => None,
                };
                for role in weft_core::eval_stages::DEBATE_ROLES {
                    let key = ["eval", "debate", role, "effort"].map(String::from);
                    self.set_setting(None, &key, effort.clone());
                }
            }
            SheetAct::OpenChoice(key) => {
                let key = key.join(".");
                self.choice = if self.choice.as_deref() == Some(&key) { None } else { Some(key) };
            }
            SheetAct::Choose { project, key, value } => {
                self.choice = None;
                self.set_setting(project.as_deref(), &key, value);
            }
            SheetAct::EditSetting { project, key } => {
                let now = self.field_now(project.as_deref(), &key);
                self.setting_field.set_text(&now);
                self.setting_editing = Some((project, key));
                self.focus = Focus::Setting;
            }
            SheetAct::CommitSetting => {
                if let Some((project, key)) = self.setting_editing.take() {
                    let text = self.setting_field.text().trim().to_string();
                    // A rule's tasks and concerns are lists, typed with commas.
                    let listed =
                        matches!(key.last().map(String::as_str), Some("applies_to" | "concerns"));
                    // A number of days is a number, or nothing: the default.
                    if key == ["sessions", "keep_days"] {
                        if let Ok(days) = text.parse::<u64>() {
                            self.set_setting(None, &key, Some(serde_json::json!(days)));
                        } else if text.is_empty() {
                            self.set_setting(None, &key, None);
                        }
                        self.focus = Focus::Terminal;
                        return;
                    }
                    let value = (!text.is_empty()).then(|| {
                        if listed {
                            serde_json::json!(
                                text.split(',')
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .collect::<Vec<_>>()
                            )
                        } else {
                            serde_json::json!(text)
                        }
                    });
                    self.set_setting(project.as_deref(), &key, value);
                }
                self.focus = Focus::Terminal;
            }
            SheetAct::AddProjectSetting => {
                self.choice =
                    if self.choice.as_deref() == Some("add") { None } else { Some("add".into()) };
            }
            SheetAct::Toggle(setting) => {
                let (key, on) = match setting {
                    crate::action::Setting::Notify => ("notify", self.machine_setting("notify")),
                };
                self.turn(key, !on);
            }
            SheetAct::Close => {
                self.send_to_open = false;
                self.sheet_back.clear();
                self.active_sheet = None;
                self.focus = Focus::Terminal;
            }
            SheetAct::ToggleSendTo => self.send_to_open = !self.send_to_open,
            SheetAct::AskTo(key) => {
                if let Some(ActiveSheet::Ask { send_to, .. }) = &mut self.active_sheet {
                    *send_to = Some(key);
                }
                self.send_to_open = false;
            }
            SheetAct::SubmitAsk => {
                if let Some(ActiveSheet::Ask { project, send_to, follow_up_unit }) =
                    self.active_sheet.clone()
                {
                    let draft = self.ask.text().to_string();
                    // Ask waits for the rules Sense is still reading.
                    if crate::sheets::ask::rules_pending(self.ask_sheet.as_ref()) {
                        return;
                    }
                    if self.ask_sheet.as_ref().is_some_and(|s| s["route"].is_null()) {
                        self.why_not(weft_core::words::word("ask.pick_to_ask"));
                        return;
                    }
                    // Not asked, the Ask stays open with what was written.
                    if self.submit_ask(
                        &project,
                        send_to.as_deref(),
                        follow_up_unit.as_deref(),
                        &draft,
                    ) {
                        self.active_sheet = None;
                        self.focus = Focus::Terminal;
                    }
                }
            }
            SheetAct::AskOpen(which) => {
                let c = &mut self.ask_choices;
                c.open = (c.open.as_deref() != Some(which.as_str())).then_some(which);
            }
            SheetAct::AskRoute(route) => {
                (self.ask_choices.route, self.ask_choices.open) = (Some(route), None);
            }
            SheetAct::AskPlan(plan) => {
                (self.ask_choices.plan, self.ask_choices.open) = (Some(plan), None);
            }
            SheetAct::AskRuleOut(id) => {
                self.ask_choices.picked.insert(id, false);
            }
            SheetAct::AskRuleBack(id) => {
                self.ask_choices.picked.insert(id, true);
            }
            SheetAct::AskMore | SheetAct::AskAddRule => self.ask_choices.all = true,
            SheetAct::FollowUp { project, unit } => self.open_ask(project, None, Some(unit)),
            SheetAct::Offer { project, unit, offer } => self.offer(project, unit, offer),
            SheetAct::GitNote { project, on: true } => {
                self.bottom_bar = BottomBarState::GitNoteConfirm { project }
            }
            SheetAct::GitNote { project, on: false } => self.git_note(&project, None),
            SheetAct::Proceed { project, unit } => {
                let working = Working {
                    doing: Doing::Preparing,
                    about: self.work_title(&project, &unit),
                    work: None,
                };
                if let Some(c) = self.client_mut(&project) {
                    let pane = c.panes.iter().find(|p| p.running).map(|p| p.id);
                    if let Ok(id) = c.act("proceed", Some(&unit), pane, None, None, None) {
                        c.owed_for(id, Then::Working(working));
                    }
                }
                self.active_sheet = None;
            }
            SheetAct::Seal { project, eval } => self.offer_seal(project, eval),
            SheetAct::EvalWork { project, unit } => self.eval_work(project, unit),
            SheetAct::SealWork { project, unit } => self.seal_work(project, unit),
            SheetAct::EvalView { project, eval } => self.eval_view(project, eval, Vec::new()),
            SheetAct::EvalItem { project, eval, item } => self.eval_view(project, eval, vec![item]),
            SheetAct::EvalOpen(key) => {
                let Some(ActiveSheet::EvalView { project, eval, at }) = &mut self.active_sheet
                else {
                    return;
                };
                if !matches!(key.as_str(), "lines" | "reads") {
                    *at = vec![key];
                    return;
                }
                let (project, eval, first) = (project.clone(), eval.clone(), at.first().cloned());
                // An item's lines or reads open on the item picked, or the first.
                let view: Option<weft_core::eval_view::EvalView> = self
                    .client(&project)
                    .and_then(|c| c.read.get(&Then::EvalView { eval: eval.clone() }))
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                let Some(v) = view else { return };
                let picked = first.as_deref().and_then(|k| v.item(k)).or(v.items.first());
                let Some(item) = picked.cloned() else { return };
                if let Some(ActiveSheet::EvalView { at, .. }) = &mut self.active_sheet {
                    *at = vec![item.key.clone(), key.clone()];
                }
                if key == "lines"
                    && let Some(c) = self.client_mut(&project)
                {
                    for w in &item.windows {
                        let then = Then::Window { eval: eval.clone(), window: w.clone() };
                        let _ = c.read("window", &format!("{eval}/{w}"), None, then);
                    }
                }
            }
            SheetAct::EvalBack => match &mut self.active_sheet {
                Some(ActiveSheet::EvalView { at, .. }) if at.len() > 1 => {
                    at.truncate(1);
                }
                // Opened from other Details: back to them.
                Some(ActiveSheet::EvalView { .. }) if !self.sheet_back.is_empty() => {
                    self.sheet_act(SheetAct::Back);
                }
                Some(ActiveSheet::EvalView { project, eval, .. }) => {
                    let (project, eval) = (project.clone(), eval.clone());
                    self.tree_act(TreeAct::SelectEval { project, eval });
                }
                _ => {}
            },
            SheetAct::WatchJudge { project, pane } => {
                self.active_sheet = None;
                self.bring_to_front(project, pane);
            }
            SheetAct::StopEval { project, eval } => {
                let about = self
                    .client(&project)
                    .and_then(|c| c.work.of_eval(&eval))
                    .map(|u| u.root.clone())
                    .map(|root| self.work_title(&project, &root))
                    .unwrap_or_default();
                let working = Working { doing: Doing::Preparing, about, work: None };
                if let Some(c) = self.client_mut(&project) {
                    match c.act("stop", None, None, None, Some(&eval), None) {
                        Ok(id) => c.owed_for(id, Then::Working(working)),
                        Err(e) => self.why_not(e.to_string()),
                    }
                }
            }
            SheetAct::Resume { project, harness, session } => {
                if self.start_agent_in_project(&project, &harness, Some(&session)) {
                    self.active_sheet = None;
                }
            }
            SheetAct::SetupAll => {
                // One update at a time: the others rest until it ends.
                if self.updating().is_none() {
                    self.forget_machine_failed(|j| j.kind != "remote");
                    self.setup_everywhere("all", "");
                }
            }
            SheetAct::SetupRun(target) => {
                if self.updating().is_none() {
                    self.forget_machine_failed(|j| {
                        j.kind != "remote" && j.on.harness.as_deref() == Some(target.as_str())
                    });
                    self.setup_everywhere("run", &target);
                }
            }
            SheetAct::TryAgain { project, then } => self.try_again(&project, then),
            SheetAct::SkipCheck(project) => {
                if let Some(c) = self.client_mut(&project) {
                    let _ = c.skip_check();
                }
            }
            SheetAct::SkipAdd { project, file, line } => {
                if let Some(c) = self.client_mut(&project) {
                    let _ = c.skip_add(&file, &line);
                }
            }
            // The window asks the system for the program, then calls `locate`.
            SheetAct::Locate(_) => {}
            SheetAct::RemoteOn => {
                self.forget_machine_failed(|j| j.kind == "remote");
                self.remote_act(serde_json::json!({"act": "on"}))
            }
            SheetAct::RemoteOff => {
                self.remote_act(serde_json::json!({"act": "off"}));
                self.bottom_bar = BottomBarState::None;
            }
            SheetAct::RemotePick(tunnel) => self.remote_pick = Some(tunnel),
            SheetAct::RemoteUse => {
                // The one chosen until now, else the first: as the list draws it.
                let shown = self
                    .remote_view()
                    .and_then(|v| v["chosen"].as_str().or(v["tunnels"][0]["id"].as_str()))
                    .map(str::to_string);
                if let Some(tunnel) = self.remote_pick.take().or(shown) {
                    self.forget_machine_failed(|j| j.kind == "remote");
                    self.remote_act(serde_json::json!({"act": "choose", "tunnel": tunnel}));
                }
            }
            SheetAct::RemoteLook => self.remote_act(serde_json::json!({"act": "look"})),
            // The window opens it in the browser.
            SheetAct::OpenGuide(_) => {}
            SheetAct::RemoteSignIn => self.remote_act(serde_json::json!({"act": "sign_in"})),
            SheetAct::RemoteCancel => self.remote_act(serde_json::json!({"act": "cancel"})),
            SheetAct::RemoteChangeTunnel => {
                self.remote_pick = None;
                self.remote_act(serde_json::json!({"act": "change_tunnel"}))
            }
            SheetAct::AskRemoveDevice { device, name } => {
                self.bottom_bar =
                    BottomBarState::RemoveDeviceConfirm { device_id: device, device_name: name };
            }
            SheetAct::RemoveDevice(device) => {
                self.remote_act(serde_json::json!({"act": "remove", "device": device}));
                self.bottom_bar = BottomBarState::None;
            }
        }
    }

    /// What a yes on `unit`'s work waits for, said with its title.
    fn working_on(&self, project: &Path, doing: Doing, unit: &str) -> Working {
        let root = self.client(project).and_then(|c| c.work.of_ask(unit)).map(|u| u.root.clone());
        Working { doing, about: self.work_title(project, unit), work: root }
    }

    /// What a dialog's yes waits for: an Eval opening, an Eval stopping, or
    /// a send, with what it is about.
    fn resolving(
        &self,
        project: &Path,
        staged: &crate::daemon::Staged,
        eval_work: Option<&(String, Option<String>)>,
    ) -> Working {
        // A dialog that types nothing names its work on its first line.
        let named = staged
            .why
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .filter(|l| !l.ends_with(['.', '?', '!', ':']))
            .map(str::to_string);
        match staged.go.as_ref().map(|g| g.word.as_str()) {
            Some("act.eval") => match eval_work {
                Some((unit, reader)) => {
                    let mut w = self.working_on(project, Doing::Opening, unit);
                    w.about = self.opening_about(project, unit, reader.as_deref());
                    w
                }
                None => {
                    Working { doing: Doing::Opening, about: named.unwrap_or_default(), work: None }
                }
            },
            Some("dialog.stop_eval.go") => {
                Working { doing: Doing::Stopping, about: named.unwrap_or_default(), work: None }
            }
            _ => {
                let about =
                    self.send_to_view(project, Some(&self.staged_send_to(staged))).picked_label();
                Working { doing: Doing::Sending, about, work: None }
            }
        }
    }

    /// A read that failed, sent again.
    fn try_again(&mut self, project: &Path, then: Then) {
        let (what, unit) = match &then {
            Then::Conversation { pane } => return self.read_conversation(project, *pane),
            Then::Older { pane } => return self.read_older(project, *pane),
            Then::Questions { eval } => {
                if let BottomBarState::SealDisposition { eval: e, questions, .. } =
                    &mut self.bottom_bar
                    && e == eval
                {
                    *questions = SealQuestions::Reading;
                }
                ("questions", eval.clone())
            }
            Then::Seal { seal } => ("seal", seal.clone()),
            Then::Source { unit } => ("source", unit.clone()),
            Then::Wording { unit } => ("wording", unit.clone()),
            Then::Judges { eval } => ("judges", eval.clone()),
            Then::EvalView { eval } => ("eval", eval.clone()),
            Then::Window { eval, window } => ("window", format!("{eval}/{window}")),
            // What Settings asks is asked again as it is shown.
            Then::Rules { .. } | Then::Preview { .. } | Then::Profile { .. } => {
                for c in &mut self.clients {
                    c.refused.remove(&then);
                }
                return self.want_rules();
            }
            _ => return,
        };
        if let Some(c) = self.client_mut(project) {
            let _ = c.read(what, &unit, None, then);
        }
    }

    /// A setup act, asked of every connected project; said when none is connected.
    fn setup_everywhere(&mut self, act: &str, harness: &str) {
        let asked: Vec<_> =
            self.setup_links().into_iter().map(|c| c.setup(act, harness, "")).collect();
        if let Some(Err(e)) = asked.into_iter().reduce(|a, b| if a.is_ok() { a } else { b }) {
            self.why_not(e.to_string());
        }
    }

    /// The program the person found for `harness`, told to the daemon.
    pub fn locate(&mut self, harness: &str, path: &Path) {
        let path = path.to_string_lossy();
        for c in self.setup_links() {
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
                if view == PaneViewMode::Conversation {
                    self.read_conversation(&proj, pane);
                }
                let _ = self.save_state();
            }
            PaneAct::Older { project, pane } => self.read_older(&project, pane),
            // The conversation's scroll is the window's; it scrolls that too.
            PaneAct::ToEnd { project, pane } => {
                if let Some(view) = self.pane_view_mut(&project, pane) {
                    view.scroll_to_bottom();
                }
            }
            PaneAct::ToggleSteps { project, pane, block } => {
                let key = (project, pane, block);
                if !self.unfolded_steps.remove(&key) {
                    self.unfolded_steps.insert(key);
                }
            }
            PaneAct::SendMessage => {
                let text = self.message_field().text().to_string();
                let Some(tab) = self.active_tab().filter(|_| !text.is_empty()) else { return };
                let sent = self.client_mut(&tab.project).map(|c| c.say(tab.pane, &text));
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
        panes.hidden.push(g.tabs.remove(tab));
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
        send_to: Option<&str>,
        follow_up_unit: Option<&str>,
        draft: &str,
    ) -> bool {
        // The agent the sheet showed, by its pane; or a new one of a harness.
        let start = send_to.and_then(|k| k.strip_prefix("start:")).map(str::to_string);
        let pane = send_to.and_then(pane_in);
        let chosen = self.ask_sheet.as_ref().map(|s| self.ask_choices.draft(s));
        // Where it goes, as SEND TO says it.
        let about = send_to
            .map(|k| self.send_to_view(project, Some(k)))
            .map(|v| v.picked_label())
            .unwrap_or_default();
        let working = Working { doing: Doing::Asking, about, work: None };
        let Some(client) = self.client_mut(project) else { return false };
        let act = if follow_up_unit.is_some() { "follow_up" } else { "ask" };
        match client.ask(act, follow_up_unit, (pane, start.as_deref()), draft, chosen) {
            Ok(id) => {
                client.owed_for(id, Then::Working(working));
                true
            }
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
        self.drop_gone_panes();
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
            && let Some(row) = client.session(sid)
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
        let Some(row) = self.client(project).and_then(|c| c.session(session)) else {
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
}
