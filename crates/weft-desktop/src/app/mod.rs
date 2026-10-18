//! The desktop's state: the open projects as the daemon last said them,
//! what the person has open and selected, durable `~/.fab7/weft/state.json`,
//! and the element tree drawn from it.

mod acts;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use weft_core::board::Act;
use weft_core::conversation::Item;
use weft_core::harness::Harnesses;
use weft_core::sessions::State as SessionState;
use weft_core::state::{PaneViewMode, SavedPaneTab, SavedTabGroup, WeftState, WindowBounds};
use weft_core::tree::{self, Agent, Facts, Fold};
use weft_core::turns;
use weft_core::words;

use crate::action::Focus;
use crate::bottom_bar::{BottomBarProps, render_bottom_bar};
use crate::daemon::{Answered, PaneView, ProjectClient, Then};
use crate::gpui::{Element, div, palette};
use crate::header::{HeaderProps, render_header};
use crate::panes_view::{DropTarget, PanesProps, drag_tab, render_panes_workspace};
use crate::sheets::send_to::{SendTo, pane_key};
use crate::sheets::{MenuProps, SheetProps, render_menu_popover, render_sheet};
use crate::text_field::{Shape, TextField};
use crate::tree_view::{Listed, TreeProps, TreeSelection, render_plus_popover, render_tree};

/// Load `WeftState` from `<home>/.fab7/weft/state.json` (falling back to
/// `<home>/.fab7/weft/projects.json` when `state.json` is absent).
pub fn load_state_from_home(home: &Path) -> WeftState {
    let weft_dir = home.join(".fab7").join("weft");
    let state_json = std::fs::read_to_string(weft_dir.join("state.json")).unwrap_or_default();
    let projects_json = std::fs::read_to_string(weft_dir.join("projects.json")).ok();
    weft_core::state::read_with_projects(&state_json, projects_json.as_deref())
}

/// Atomically persist `state` to `<home>/.fab7/weft/state.json` and mirror
/// `open_projects` to `<home>/.fab7/weft/projects.json`.
pub fn save_state_to_home(home: &Path, state: &WeftState) -> Result<()> {
    let weft_dir = home.join(".fab7").join("weft");
    std::fs::create_dir_all(&weft_dir)?;
    let state_path = weft_dir.join("state.json");
    let tmp_state = weft_dir.join("state.json.tmp");
    std::fs::write(&tmp_state, weft_core::state::write(state))?;
    std::fs::rename(&tmp_state, &state_path)?;

    let projects_path = weft_dir.join("projects.json");
    let existing = std::fs::read_to_string(&projects_path)
        .map(|s| weft_core::projects::read(&s))
        .unwrap_or_default();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mirrored = weft_core::state::mirror_projects(state, &existing, now_ms);
    let tmp_proj = weft_dir.join("projects.json.tmp");
    std::fs::write(&tmp_proj, mirrored)?;
    std::fs::rename(&tmp_proj, &projects_path)?;
    Ok(())
}

/// A host name as the window shows it: up to its first dot; "this machine"
/// when there is none.
pub fn machine_name_of(host: &str) -> String {
    match host.split('.').next().map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => name.to_string(),
        None => words::word("desktop.this_machine").to_string(),
    }
}

/// Something to tell the person through the system, while Weft is not where
/// they look: `tag` names the agent it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub tag: String,
    pub title: String,
}

/// How long a draft stays the same before it is asked about.
const SETTLED: std::time::Duration = std::time::Duration::from_millis(100);
/// How long a why-not is said before it goes by itself.
const WHY_NOT_FOR: std::time::Duration = std::time::Duration::from_secs(6);

/// Right-pane sheet overlay or modal view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveSheet {
    Ask {
        project: PathBuf,
        send_to: Option<String>,
        follow_up_unit: Option<String>,
    },
    /// A unit of work's Details, by its root Ask.
    Unit {
        project: PathBuf,
        unit: String,
    },
    /// An Ask's own Details.
    WorkDetail {
        project: PathBuf,
        unit: String,
    },
    EvalDetail {
        project: PathBuf,
        eval: String,
    },
    /// The Eval view: steps or files, and the levels opened, the last in front.
    EvalView {
        project: PathBuf,
        eval: String,
        by_file: bool,
        at: Vec<String>,
    },
    SealDetail {
        project: PathBuf,
        seal: String,
    },
    SessionDetail {
        project: PathBuf,
        session: String,
    },
    Updates,
    Remote,
}

/// What the bar along the window's foot asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BottomBarState {
    None,
    WhyNot { sentence: String },
    QuitConfirm,
    AllowDevice { device_id: String, device_name: String, login: Option<String> },
    RemoveDeviceConfirm { device_id: String, device_name: String },
    CloseProjectConfirm { project: PathBuf },
    SealDisposition { project: PathBuf, eval: String, selected: String },
}

/// Top-level Weft Desktop GPUI application.
pub struct WeftDesktopApp {
    pub home: PathBuf,
    pub machine: String,
    pub state: WeftState,
    pub clients: Vec<ProjectClient>,
    pub selection: Option<TreeSelection>,
    /// The tree's search field.
    pub search: TextField,
    pub plus_popover_project: Option<PathBuf>,
    pub menu_open: bool,
    pub layout_menu_pane: Option<usize>,
    pub unfolded_sealed: HashSet<PathBuf>,
    pub unfolded_earlier: HashSet<PathBuf>,
    /// The Details a stage was opened from, the nearest last, each with its
    /// name: what `‹` goes back to.
    pub sheet_back: Vec<(ActiveSheet, String)>,
    /// When the bar's why-not was first shown.
    why_not_since: Option<std::time::Instant>,
    pub conversations: HashMap<(PathBuf, u32), Vec<Item>>,
    pub unfolded_steps: HashSet<(PathBuf, u32, usize)>,
    /// Each agent's message field, by its project and pane: switching
    /// agents brings each one's draft back. The open Ask's field.
    pub messages: HashMap<(PathBuf, u32), TextField>,
    pub ask: TextField,
    /// Where the keyboard goes.
    pub focus: Focus,
    /// What each agent's field could show after its caret, and the draft it is for.
    suggestions: HashMap<(PathBuf, u32), (String, String)>,
    /// The agent in front's draft as last seen, and since when: it is asked
    /// about once it has settled.
    settling: Option<((PathBuf, u32), String, std::time::Instant)>,
    /// The last draft asked about, so each is asked once.
    asked: Option<((PathBuf, u32), String)>,
    pub active_sheet: Option<ActiveSheet>,
    pub bottom_bar: BottomBarState,
    pub quit_requested: bool,
    /// The state changed since it was last written: the window writes it
    /// off the shared lock.
    unsaved: bool,
    /// The agents waiting for the person, as last seen, so each is told once.
    waiting: HashSet<(PathBuf, u32)>,
    /// The tunnel picked in Remote, until it is used.
    pub remote_pick: Option<String>,
    /// What the bar showed before a device asked to be allowed.
    before_allow: Option<BottomBarState>,
    /// SEND TO is open.
    pub send_to_open: bool,
    /// Where each staged send goes, when the person picked other than where
    /// it was staged.
    staged_to: HashMap<String, String>,
    /// Updates opened by itself once: it does not again.
    updates_offered: bool,
    /// The device last answered: the bar does not ask again while
    /// `weft remote` has not yet heard.
    allow_answered: Option<String>,
    /// What is to be told and has not been taken yet.
    notices: Vec<Notice>,
    /// Why each project last failed to open, as told.
    not_opened: HashMap<PathBuf, String>,
}

impl WeftDesktopApp {
    /// The desktop as `<home>/.fab7/weft/state.json` left it, each open
    /// project not connected yet: the window's own thread connects them.
    pub fn new(home: PathBuf, machine: impl Into<String>) -> Self {
        let state = load_state_from_home(&home);
        let mut app = Self {
            home,
            machine: machine.into(),
            state,
            clients: Vec::new(),
            selection: None,
            search: TextField::new(Shape::Line),
            plus_popover_project: None,
            menu_open: false,
            layout_menu_pane: None,
            unfolded_sealed: HashSet::new(),
            unfolded_earlier: HashSet::new(),
            sheet_back: Vec::new(),
            why_not_since: None,
            conversations: HashMap::new(),
            unfolded_steps: HashSet::new(),
            messages: HashMap::new(),
            ask: TextField::new(Shape::Area),
            focus: Focus::Terminal,
            suggestions: HashMap::new(),
            settling: None,
            asked: None,
            active_sheet: None,
            bottom_bar: BottomBarState::None,
            quit_requested: false,
            unsaved: false,
            waiting: HashSet::new(),
            remote_pick: None,
            updates_offered: false,
            send_to_open: false,
            staged_to: HashMap::new(),
            before_allow: None,
            allow_answered: None,
            notices: Vec::new(),
            not_opened: HashMap::new(),
        };
        for p in app.state.open_projects.clone() {
            app.add_project(&p);
        }
        app
    }

    /// A project the tree lists, not connected yet.
    fn add_project(&mut self, path: &Path) {
        let p = weft_core::state::normalize_project_path(path);
        if !self.clients.iter().any(|c| c.root == p) {
            self.clients.push(ProjectClient::disconnected(p.clone(), ProjectClient::name_of(&p)));
        }
    }

    /// The projects not connected to the daemon: the window's own thread
    /// connects each, never under the shared lock.
    pub fn disconnected(&self) -> Vec<(PathBuf, bool)> {
        self.clients
            .iter()
            .filter(|c| !c.is_connected())
            .map(|c| (c.root.clone(), c.lost))
            .collect()
    }

    /// The daemon went and none came back: the person stopped it, or it
    /// ended. Say so, and go, as its agents went.
    pub fn session_ended(&mut self) {
        self.notices.push(Notice {
            tag: String::new(),
            title: words::word("desktop.notice.ended").to_string(),
        });
        self.quit_requested = true;
    }

    /// Put a connected project in place of the one it connects, and open a
    /// tab for each running agent no pane shows yet.
    pub fn integrate_connected_client(&mut self, mut client: ProjectClient) {
        let p = weft_core::state::normalize_project_path(&client.root);
        client.root = p.clone();
        // Closed while it connected: it is not shown again.
        let Some(pos) = self.clients.iter().position(|c| c.root == p) else {
            client.detach();
            return;
        };
        if self.state.settings.turbo && !client.turbo {
            let _ = client.set_turbo(true);
        }
        self.not_opened.remove(&p);
        let running: Vec<u32> = client.panes.iter().filter(|v| v.running).map(|v| v.id).collect();
        self.clients[pos] = client;
        // An agent already in some pane stays where it is: one agent in two
        // panes would be sized by both.
        for pane in running {
            let shown = self.state.panes.groups.iter().any(|g| {
                g.tabs.iter().any(|t| {
                    weft_core::state::normalize_project_path(&t.project) == p && t.pane == pane
                })
            });
            if !shown {
                self.ensure_tab_in_active_pane(SavedPaneTab {
                    project: p.clone(),
                    pane,
                    view: self.state.settings.default_view,
                });
            }
        }
        let _ = self.save_state();
    }

    /// Mark the state to be written to `<home>/.fab7/weft/state.json`.
    pub fn save_state(&mut self) -> Result<()> {
        self.unsaved = true;
        Ok(())
    }

    /// The state to write, and where, when it changed since: the caller
    /// writes it with [`save_state_to_home`], outside any lock.
    pub fn unsaved_state(&mut self) -> Option<(PathBuf, WeftState)> {
        std::mem::take(&mut self.unsaved).then(|| (self.home.clone(), self.state.clone()))
    }

    /// Take what every project's daemon link brought since; whether anything changed.
    pub fn pump(&mut self) -> bool {
        let (mut changed, mut moved) = (false, false);
        let mut answers = Vec::new();
        for c in &mut self.clients {
            let p = c.pump();
            changed |= p.changed;
            moved |= p.moved;
            answers.extend(p.answered.into_iter().map(|a| (c.root.clone(), a)));
        }
        for (root, a) in answers {
            self.take_answer(&root, a);
        }
        // An agent's turn or record moved: what it said may have too, and
        // an Eval still being judged.
        if moved {
            self.notice_waiting();
            self.refresh_active_conversations();
            self.refresh_eval_view();
        }
        if changed {
            self.offer_allow();
            self.offer_updates();
            self.drop_gone_tabs();
        }
        let now = std::time::Instant::now();
        self.expire_why_not(now) | self.suggest_tick(now) || changed
    }

    /// A why-not is said for a while, then goes: what it was about may have
    /// changed since. Whether the bar changed.
    pub fn expire_why_not(&mut self, now: std::time::Instant) -> bool {
        if !matches!(self.bottom_bar, BottomBarState::WhyNot { .. }) {
            self.why_not_since = None;
            return false;
        }
        let since = *self.why_not_since.get_or_insert(now);
        if now - since < WHY_NOT_FOR {
            return false;
        }
        self.bottom_bar = BottomBarState::None;
        self.why_not_since = None;
        true
    }

    /// A tab of a pane its project's daemon does not have, once it has said
    /// which it has, goes: a tab is an agent of its project.
    pub(crate) fn drop_gone_tabs(&mut self) {
        let gone = |t: &SavedPaneTab| {
            self.clients.iter().any(|c| c.root == t.project && c.opened && c.view(t.pane).is_none())
        };
        let panes = &mut self.state.panes;
        let before: usize = panes.groups.iter().map(|g| g.tabs.len()).sum();
        for g in &mut panes.groups {
            g.tabs.retain(|t| !gone(t));
            g.active_tab = g.active_tab.min(g.tabs.len().saturating_sub(1));
        }
        if panes.groups.len() > 1 {
            panes.groups.retain(|g| !g.tabs.is_empty());
        }
        panes.focused_group = panes.focused_group.min(panes.groups.len().saturating_sub(1));
        if panes.groups.iter().map(|g| g.tabs.len()).sum::<usize>() != before {
            let _ = self.save_state();
        }
    }

    /// Nothing is set up and something could be: Updates opens by itself,
    /// once, when nothing else is open.
    fn offer_updates(&mut self) {
        let wants =
            self.clients.iter().any(|c| c.setup.as_ref().is_some_and(|v| v.wants_setting_up()));
        if wants && !self.updates_offered && self.active_sheet.is_none() {
            self.updates_offered = true;
            self.active_sheet = Some(ActiveSheet::Updates);
        }
    }

    /// SEND TO for `project`, `picked` ticked: its running agents, then a new
    /// agent of each harness.
    pub fn send_to_view(&self, project: &Path, picked: Option<&str>) -> SendTo {
        let listed = self.tree_projects().into_iter().find(|l| l.root == project);
        let agents = listed
            .map(|l| {
                let name = l.project.name.clone();
                l.project
                    .agents
                    .rows
                    .into_iter()
                    .filter(|a| a.state == SessionState::Running)
                    .filter_map(|a| Some((pane_key(a.pane?), format!("{name} · {}", a.harness))))
                    .collect()
            })
            .unwrap_or_default();
        let starts = self.harnesses().0.iter().map(|h| h.name.clone()).collect();
        SendTo {
            agents,
            starts,
            picked: picked.unwrap_or_default().to_string(),
            open: self.send_to_open,
        }
    }

    /// Where a staged send goes: what the person picked, else where it was
    /// staged.
    pub fn staged_send_to(&self, staged: &crate::daemon::Staged) -> String {
        self.staged_to.get(&staged.id).cloned().unwrap_or_else(|| pane_key(staged.pane))
    }

    /// The pane of `session` in `root`, as the tree has it.
    pub(crate) fn pane_of(&self, root: &Path, session: &str) -> Option<u32> {
        let listed = self.tree_projects().into_iter().find(|l| l.root == root)?;
        listed.project.agents.rows.into_iter().find(|a| a.session == session).and_then(|a| a.pane)
    }

    /// `weft remote`'s view, from whichever project heard it.
    pub fn remote_view(&self) -> Option<&serde_json::Value> {
        self.clients.iter().find_map(|c| c.remote.as_ref())
    }

    /// A device asking to be allowed opens the bar by itself, over whatever
    /// it showed; when the request goes (answered here or elsewhere, or
    /// expired), the bar goes with it and what was there comes back.
    fn offer_allow(&mut self) {
        let asking = self.remote_view().map(|v| v["asking"].clone()).filter(|a| a.is_object());
        let wanted = asking.as_ref().and_then(|a| a["device"].as_str()).map(str::to_string);
        if wanted.is_none() {
            self.allow_answered = None;
        }
        let wanted = wanted.filter(|d| self.allow_answered.as_ref() != Some(d));
        let shown = match &self.bottom_bar {
            BottomBarState::AllowDevice { device_id, .. } => Some(device_id.clone()),
            _ => None,
        };
        if shown == wanted {
            return;
        }
        let before = match shown {
            Some(_) => self.before_allow.take(),
            None => Some(std::mem::replace(&mut self.bottom_bar, BottomBarState::None)),
        };
        match (asking, wanted) {
            (Some(a), Some(device)) => {
                self.before_allow = before;
                self.bottom_bar = BottomBarState::AllowDevice {
                    device_id: device,
                    device_name: a["name"].as_str().unwrap_or_default().to_string(),
                    login: a["login"].as_str().map(str::to_string),
                };
            }
            _ => self.bottom_bar = before.unwrap_or(BottomBarState::None),
        }
    }

    /// An act for `weft remote`, through the daemon: one machine, one daemon,
    /// so through one project's link.
    pub fn remote_act(&mut self, act: serde_json::Value) {
        if let Some(c) = self.clients.iter_mut().find(|c| c.is_connected()) {
            let _ = c.remote_act(act);
        }
    }

    /// What an answer was for, done now it has come.
    fn take_answer(&mut self, root: &Path, a: Answered) {
        match (a.then, a.answer) {
            (Then::Started { .. }, Ok(v)) => {
                if let Some(pane) = v["pane"].as_u64() {
                    self.ensure_tab_in_active_pane(SavedPaneTab {
                        project: root.to_path_buf(),
                        pane: pane as u32,
                        view: self.state.settings.default_view,
                    });
                    let _ = self.save_state();
                }
            }
            (Then::Conversation { pane }, Ok(v)) => {
                if let Ok(items) = serde_json::from_value(v["conversation"]["items"].clone()) {
                    self.conversations.insert((root.to_path_buf(), pane), items);
                }
            }
            (Then::Suggest { pane, draft }, answer) => {
                let key = (root.to_path_buf(), pane);
                let front = self.active_tab().map(|t| (t.project, t.pane));
                // An answer for a draft no longer there, or a malformed one, shows nothing.
                let current = front.as_ref() == Some(&key) && self.message_text() == draft;
                let text = answer.ok().and_then(|v| v["text"].as_str().map(str::to_string));
                match text.filter(|t| current && !t.is_empty()) {
                    Some(t) => {
                        self.suggestions.insert(key, (draft, t));
                    }
                    None => {
                        self.suggestions.remove(&key);
                    }
                }
            }
            // A refusal of an act, said where the person looks, unless the
            // bar is asking something of its own. A read refused shows
            // nothing: the sheet says what it lacks.
            (Then::Started { .. } | Then::Refusal, Err(why))
                if matches!(
                    self.bottom_bar,
                    BottomBarState::None | BottomBarState::WhyNot { .. }
                ) =>
            {
                self.bottom_bar = BottomBarState::WhyNot { sentence: why }
            }
            _ => {}
        }
    }

    /// Ask about the agent in front's draft once it has settled. Whether
    /// what its field shows changed.
    pub fn suggest_tick(&mut self, now: std::time::Instant) -> bool {
        let Some(key) = self.active_tab().map(|t| (t.project, t.pane)) else { return false };
        let draft = self.message_text().to_string();
        match &self.settling {
            Some((k, d, _)) if *k == key && *d == draft => {}
            _ => {
                self.settling = Some((key, draft, now));
                return false;
            }
        }
        let settled = self.settling.as_ref().is_some_and(|(_, _, since)| now - *since >= SETTLED);
        // An empty field follows the agent's last reply: once its turn has
        // ended, again after each turn.
        let turn = self.pane_view(&key.0, key.1).and_then(|v| v.turn);
        if draft.is_empty() && turn != Some(weft_core::turns::Turn::TurnEnded) {
            // Asked again once this turn ends.
            if self.asked.as_ref().is_some_and(|(k, d)| *k == key && d.is_empty()) {
                self.asked = None;
            }
            return false;
        }
        let fresh = self.asked.as_ref() != Some(&(key.clone(), draft.clone()));
        // One at a time.
        let asking = self.clients.iter().any(|c| c.owes_any(|t| matches!(t, Then::Suggest { .. })));
        if !settled || !fresh || asking {
            return false;
        }
        if let Some(client) = self.clients.iter_mut().find(|c| c.root == key.0)
            && client.suggest(key.1, &draft).is_ok()
        {
            self.asked = Some((key, draft));
        }
        false
    }

    /// The act the bar puts forward for the agent in front's latest work,
    /// once its turn has really ended.
    pub fn suggested_act(&self) -> Option<Act> {
        let t = self.active_tab()?;
        let client = self.client(&t.project)?;
        let row = client.sessions.iter().find(|r| r.pane == Some(t.pane))?;
        let turn = client.view(t.pane).and_then(|v| v.turn).or(row.turn);
        weft_core::board::suggested_for(&client.units, &row.id, turn)
    }

    /// What the agent in front's field shows after its caret, when its draft
    /// is the one it was asked for and the caret is at its end.
    pub fn ghost(&self) -> Option<&str> {
        let t = self.active_tab()?;
        let (draft, text) = self.suggestions.get(&(t.project.clone(), t.pane))?;
        let field = self.messages.get(&(t.project, t.pane));
        let at_end = field.is_none_or(|f| f.selection().is_empty() && f.cursor() == f.text().len());
        (draft == self.message_text() && at_end).then_some(text.as_str())
    }

    /// Tab: the suggestion typed into the field.
    pub fn take_suggestion(&mut self) -> bool {
        let Some(text) = self.ghost().map(str::to_string) else { return false };
        self.message_field().type_text(&text);
        if let Some(t) = self.active_tab() {
            self.suggestions.remove(&(t.project, t.pane));
        }
        true
    }

    /// Esc: the suggestion dropped, not asked for again until the draft changes.
    pub fn drop_suggestion(&mut self) -> bool {
        if self.ghost().is_none() {
            return false;
        }
        if let Some(t) = self.active_tab() {
            self.suggestions.remove(&(t.project, t.pane));
        }
        true
    }

    /// Read again the conversation of each pane showing one.
    pub fn refresh_active_conversations(&mut self) {
        let mut targets = Vec::new();
        for group in &self.state.panes.groups {
            if let Some(tab) =
                group.tabs.get(group.active_tab.min(group.tabs.len().saturating_sub(1)))
                && tab.view == PaneViewMode::Conversation
            {
                targets.push((tab.project.clone(), tab.pane));
            }
        }
        for (proj, pane) in targets {
            if let Some(client) = self.client_mut(&proj) {
                let _ = client.read("conversation", "", Some(pane), Then::Conversation { pane });
            }
        }
    }

    /// Tell, once, of each agent that started waiting for the person and is
    /// not the one in front, in a project that tells.
    fn notice_waiting(&mut self) {
        let front = self.active_tab().map(|t| (t.project, t.pane));
        let mut now = HashSet::new();
        for c in &self.clients {
            for p in c.panes.iter().filter(|p| weft_core::turns::is_asking(p.turn)) {
                let key = (c.root.clone(), p.id);
                if !self.waiting.contains(&key) && front.as_ref() != Some(&key) && c.notify {
                    self.notices.push(Notice {
                        tag: format!("{}#{}", c.root.display(), p.id),
                        title: words::fill(
                            "desktop.notice.waiting",
                            &[("harness", &p.harness), ("project", &c.name)],
                        ),
                    });
                }
                now.insert(key);
            }
        }
        self.waiting = now;
    }

    /// A project would not open: tell why, once while the reason stays the
    /// same.
    pub fn not_opened(&mut self, root: &Path, why: &str) {
        if self.not_opened.get(root).is_some_and(|told| told == why) {
            return;
        }
        self.not_opened.insert(root.to_path_buf(), why.to_string());
        let project = ProjectClient::name_of(root);
        self.notices.push(Notice {
            tag: root.display().to_string(),
            title: words::fill("desktop.notice.not_opened", &[("project", &project), ("why", why)]),
        });
    }

    /// What is to be told, taken once.
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// The person opened a notice: its agent in front.
    pub fn open_notice(&mut self, tag: &str) {
        let Some((root, pane)) = tag.rsplit_once('#') else { return };
        let Ok(pane) = pane.parse::<u32>() else { return };
        self.bring_to_front(PathBuf::from(root), pane);
    }

    /// The next agent waiting for the person in front: this project's
    /// first, then the others', after the one in front now; said when none
    /// waits.
    pub fn next_needs_you(&mut self) {
        let front = self.active_tab().map(|t| (t.project, t.pane));
        let here = front.as_ref().map(|(p, _)| p.clone());
        let mut projects = self.tree_projects();
        // This project's agents first, each project's in the tree's order.
        projects.sort_by_key(|l| Some(&l.root) != here.as_ref());
        let asking: Vec<(PathBuf, Agent)> = projects
            .into_iter()
            .flat_map(|l| {
                let root = l.root;
                l.project.agents.rows.into_iter().map(move |a| (root.clone(), a))
            })
            .filter(|(_, a)| a.state == SessionState::Running && turns::is_asking(a.turn))
            .collect();
        let shown: Vec<(PathBuf, u32)> =
            asking.iter().filter_map(|(root, a)| Some((root.clone(), a.pane?))).collect();
        let after = front.and_then(|f| shown.iter().position(|w| *w == f)).map_or(0, |i| i + 1);
        let sentence = match (shown.get(after % shown.len().max(1)).cloned(), asking.first()) {
            (Some((root, pane)), _) => return self.bring_to_front(root, pane),
            // It waits in a program Weft does not show.
            (None, Some((_, a))) => words::fill("why.elsewhere", &[("harness", &a.harness)]),
            (None, None) => words::word("why.nothing_needs_you").to_string(),
        };
        self.bottom_bar = BottomBarState::WhyNot { sentence };
    }

    /// `pane` of `root` in front: its tab where it is, else a new one.
    fn bring_to_front(&mut self, root: PathBuf, pane: u32) {
        let shown = self.state.panes.groups.iter().enumerate().find_map(|(g, group)| {
            group.tabs.iter().position(|t| t.project == root && t.pane == pane).map(|t| (g, t))
        });
        match shown {
            Some((g, t)) => {
                self.state.panes.groups[g].active_tab = t;
                self.state.panes.focused_group = g;
            }
            None => self.ensure_tab_in_active_pane(SavedPaneTab {
                project: root,
                pane,
                view: self.state.settings.default_view,
            }),
        }
        let _ = self.save_state();
    }

    /// Read the open Eval view again while it is being judged.
    fn refresh_eval_view(&mut self) {
        let Some(ActiveSheet::EvalView { project, eval, .. }) = self.active_sheet.clone() else {
            return;
        };
        let then = Then::EvalView { eval: eval.clone() };
        if let Some(c) = self.client_mut(&project) {
            let closed = c.read.get(&then).and_then(|v| v["closed"].as_bool()).unwrap_or(false);
            if !closed {
                let _ = c.reread("eval", &eval, then);
            }
        }
    }

    pub fn client(&self, root: &Path) -> Option<&ProjectClient> {
        self.clients.iter().find(|c| c.root == root)
    }

    pub fn client_mut(&mut self, root: &Path) -> Option<&mut ProjectClient> {
        self.clients.iter_mut().find(|c| c.root == root)
    }

    /// The harness files, as the daemon gave them.
    pub fn harnesses(&self) -> Harnesses {
        self.clients
            .iter()
            .find(|c| !c.harnesses.0.is_empty())
            .map(|c| c.harnesses.clone())
            .unwrap_or_default()
    }

    /// Every open project's rows, in the tree's order.
    pub fn tree_projects(&self) -> Vec<Listed> {
        let mut out = Vec::with_capacity(self.clients.len());
        for client in &self.clients {
            let pane_infos = client.pane_infos();
            let pane_turns = client.pane_turns();
            let ready_fn = |h: &str| client.readiness_of(h);
            let facts = Facts {
                name: &client.name,
                units: &client.units,
                panes: &pane_infos,
                agents: &pane_turns,
                sessions: &client.sessions,
                work: &client.work,
                readiness: &ready_fn,
                routing: &client.routing,
                setup: client.needs,
            };
            let root = &client.root;
            let folded = |_: usize, fold: Fold| match fold {
                Fold::Project => self.state.folded_projects.iter().any(|f| f == root),
                Fold::Sealed => !self.unfolded_sealed.contains(root),
            };
            let open = |_: usize, _: Fold| false;
            let rows = tree::rows(std::slice::from_ref(&facts), &folded).into_iter().next();
            let work = tree::rows(&[facts], &open).into_iter().next().map(|p| p.work.units);
            if let Some(project) = rows {
                let routes =
                    client.routing.each().into_iter().map(|(a, h)| (a, h.to_string())).collect();
                out.push(Listed {
                    root: root.clone(),
                    project,
                    routes,
                    work: work.unwrap_or_default(),
                });
            }
        }
        out
    }

    /// How many things the setup view says are behind, or one when only
    /// the daemon's update note says so.
    pub fn updates_available_count(&self) -> usize {
        match self.clients.iter().find_map(|c| c.setup.as_ref()) {
            Some(view) => crate::sheets::behind(view),
            None => usize::from(self.clients.iter().any(|c| c.update.is_some())),
        }
    }

    /// Set window bounds and persist to `state.json`.
    pub fn set_window_bounds(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.state.settings.window_bounds = Some(WindowBounds { x, y, width, height });
        let _ = self.save_state();
    }

    /// The message field of the agent in front.
    pub fn message_field(&mut self) -> &mut TextField {
        let key = self.active_tab().map_or_else(Default::default, |t| (t.project, t.pane));
        self.messages.entry(key).or_insert_with(|| TextField::new(Shape::Area))
    }

    /// What the agent in front's message field holds.
    fn message_text(&self) -> &str {
        self.active_tab()
            .and_then(|t| self.messages.get(&(t.project, t.pane)))
            .map_or("", TextField::text)
    }

    /// The tab in front of the focused pane group.
    fn active_tab(&self) -> Option<SavedPaneTab> {
        let panes = &self.state.panes;
        let group =
            panes.groups.get(panes.focused_group.min(panes.groups.len().saturating_sub(1)))?;
        group.tabs.get(group.active_tab.min(group.tabs.len().saturating_sub(1))).cloned()
    }

    /// One pane's terminal, by the project it is in.
    pub fn pane_view(&self, project: &Path, pane: u32) -> Option<&PaneView> {
        self.client(project)?.view(pane)
    }

    pub fn pane_view_mut(&mut self, project: &Path, pane: u32) -> Option<&mut PaneView> {
        self.client_mut(project)?.view_mut(pane)
    }

    /// The terminal of the tab in front of the focused pane group.
    pub fn active_pane_view_mut(&mut self) -> Option<&mut PaneView> {
        let tab = self.active_tab()?;
        self.pane_view_mut(&tab.project, tab.pane)
    }

    /// Give one pane the size its place on screen holds.
    pub fn resize_pane(&mut self, project: &Path, pane: u32, rows: u16, cols: u16) {
        if let Some(client) = self.client_mut(project) {
            let _ = client.resize(pane, rows, cols);
        }
    }

    /// Drag a tab and persist the resulting split layout.
    pub fn drag_pane_tab(&mut self, from_group: usize, from_tab: usize, target: DropTarget) {
        if let Err(why_key) = drag_tab(&mut self.state.panes, from_group, from_tab, target) {
            self.bottom_bar = BottomBarState::WhyNot { sentence: words::word(why_key).to_string() };
        } else {
            let _ = self.save_state();
        }
    }

    /// The open Ask's field.
    pub fn ask_draft_mut(&mut self) -> Option<&mut TextField> {
        matches!(self.active_sheet, Some(ActiveSheet::Ask { .. })).then_some(&mut self.ask)
    }

    pub fn send_active_pane_input(&mut self, bytes: &[u8]) {
        let Some(tab) = self.active_tab() else { return };
        self.send_pane_input(&tab.project, tab.pane, bytes);
    }

    pub fn send_pane_input(&mut self, project: &Path, pane: u32, bytes: &[u8]) {
        if let Some(client) = self.client_mut(project) {
            let _ = client.input(pane, bytes);
        }
    }

    pub fn ensure_tab_in_active_pane(&mut self, tab: SavedPaneTab) {
        if self.state.panes.groups.is_empty() {
            self.state.panes.groups.push(SavedTabGroup { active_tab: 0, tabs: vec![tab] });
            self.state.panes.focused_group = 0;
            return;
        }
        let idx = self.state.panes.focused_group.min(self.state.panes.groups.len() - 1);
        let group = &mut self.state.panes.groups[idx];
        if let Some(pos) =
            group.tabs.iter().position(|t| t.project == tab.project && t.pane == tab.pane)
        {
            group.active_tab = pos;
        } else {
            group.tabs.push(tab);
            group.active_tab = group.tabs.len() - 1;
        }
    }

    /// Render the complete `weft-desktop` GPUI element tree.
    pub fn render(&self) -> Element {
        let projects = self.tree_projects();
        let harnesses = self.harnesses();
        let needs_you = projects.iter().map(|p| p.project.needs).sum();
        let updates_available = self.updates_available_count();

        let header = render_header(&HeaderProps {
            machine: self.machine.clone(),
            sidebar_open: self.state.settings.sidebar_open,
            needs_you,
            link: self.clients.iter().find(|c| !c.is_connected()).map(|c| {
                words::word(if c.lost { "web.link.reconnecting" } else { "web.link.connecting" })
            }),
            menu_open: self.menu_open,
        });

        let mut body = div()
            .layer("body")
            .self_stretch()
            .flex_1()
            .flex_row()
            .items_center()
            .style("position", "relative");
        // Under everything: a click nothing else takes closes the popovers, and
        // each popover, a pane's layout menu among them, is drawn over it.
        if self.menu_open || self.plus_popover_project.is_some() || self.layout_menu_pane.is_some()
        {
            body = body.child(
                div()
                    .layer("popover backdrop")
                    .on_click(crate::action::WindowAct::ClosePopovers)
                    .style("position", "absolute")
                    .style("inset", "0"),
            );
        }

        if self.state.settings.sidebar_open {
            body = body.child(render_tree(&TreeProps {
                projects: &projects,
                selection: self.selection.as_ref(),
                search_query: self.search.text(),
                searching: self.focus == Focus::Search,
                plus_popover_project: self.plus_popover_project.as_deref(),
                harnesses: &harnesses,
                turbo: self.state.settings.turbo,
                unfolded_earlier: &self.unfolded_earlier,
                clients: &self.clients,
            }));
        }

        let ask_send_to = match &self.active_sheet {
            Some(ActiveSheet::Ask { project, send_to, .. }) => {
                Some(self.send_to_view(project, send_to.as_deref()))
            }
            _ => None,
        };
        let sheet = self.active_sheet.as_ref().map(|sheet| {
            render_sheet(&SheetProps {
                sheet,
                projects: &projects,
                clients: &self.clients,
                harnesses: &harnesses,
                bottom_bar: &self.bottom_bar,
                ask_draft: self.ask.text(),
                remote_pick: self.remote_pick.as_deref(),
                send_to: ask_send_to.as_ref(),
                back: self.sheet_back.last().map(|(_, name)| name.as_str()),
            })
        });
        // Ask docks under the panes it is about; every other sheet takes the
        // workspace's place.
        let docked = matches!(self.active_sheet, Some(ActiveSheet::Ask { .. }));
        match sheet {
            Some(sheet) if !docked => body = body.child(sheet),
            dock => {
                body = body.child(render_panes_workspace(&PanesProps {
                    panes: &self.state.panes,
                    clients: &self.clients,
                    layout_menu_pane: self.layout_menu_pane,
                    conversations: &self.conversations,
                    unfolded_steps: &self.unfolded_steps,
                    message_draft: self.message_text(),
                    typing: self.focus == Focus::Message,
                    ghost: self.ghost(),
                    dock,
                }))
            }
        }

        if let Some(root) = &self.plus_popover_project {
            body = body.child(render_plus_popover(root, &harnesses, self.state.settings.turbo));
        }
        if self.menu_open {
            body = body.child(render_menu_popover(&MenuProps {
                turbo: self.state.settings.turbo,
                start_at_login: self.clients.iter().any(|c| c.start_at_login),
                remote: self.clients.iter().find_map(|c| c.remote.as_ref()),
                heard: self.clients.iter().any(|c| c.is_connected()),
                updates: updates_available,
            }));
        }

        // A staged send's SEND TO: where it goes now.
        let staged_to = self.clients.iter().find_map(|c| {
            let staged = c.waiting.last()?;
            Some(self.send_to_view(&c.root, Some(&self.staged_send_to(staged))))
        });
        div()
            .layer("window")
            .style("width", "100%")
            .style("height", "100%")
            .style("position", "relative")
            .style("overflow", "hidden")
            .style("font-family", "Sora")
            .color(palette::TEXT)
            .flex_col()
            .items_start()
            .bg(palette::CANVAS)
            .child(header)
            .child(body)
            .child(render_bottom_bar(&BottomBarProps {
                staged_to: staged_to.as_ref(),
                machine: &self.machine,
                state: &self.bottom_bar,
                clients: &self.clients,
                selection: self.selection.as_ref(),
                suggested: self.suggested_act(),
            }))
    }
}
