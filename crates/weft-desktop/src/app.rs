//! Core `WeftDesktopApp` state, durable `~/.fab7/weft/state.json` persistence,
//! action dispatch, and top-level GPUI rendering (`D2.x`–`D15.x`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use weft_core::board::Act;
use weft_core::conversation::Item;
use weft_core::harness::Harnesses;
use weft_core::onboarding::State as SetupState;
use weft_core::state::{PaneViewMode, SavedPaneTab, SavedTabGroup, WeftState, WindowBounds};
use weft_core::tree::{self, Facts, Fold, Project};
use weft_core::words;

use crate::bottom_bar::{BottomBarProps, render_bottom_bar};
use crate::daemon::{PaneView, ProjectClient, Staged};
use crate::gpui::{Element, FileDialog, MacFileDialog, div, palette};
use crate::header::{HeaderProps, render_header, render_titlebar};
use crate::panes_view::{
    DropTarget, PanesProps, drag_tab, join_all_panes, move_tab_to_next_pane,
    render_panes_workspace, split_pane_down, split_pane_right,
};
use crate::sheets::{SheetProps, render_menu_popover, render_sheet};
use crate::text_field::{Shape, TextField};
use crate::tree_view::{TreeProps, TreeSelection, render_plus_popover, render_tree};

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

/// A suggestion asked for one agent's draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggesting {
    pub id: u64,
    key: (PathBuf, u32),
    draft: String,
}

/// How long a draft stays the same before it is asked about.
const SETTLED: std::time::Duration = std::time::Duration::from_millis(100);

/// Right-pane sheet overlay or modal view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveSheet {
    Ask { project: String, target_session: Option<String>, follow_up_unit: Option<String> },
    WorkDetail { project: String, unit: String },
    EvalDetail { project: String, eval: String },
    EvalView { project: String, eval: String, by_file: bool, selected_step: usize },
    SealDetail { project: String, seal: String },
    SessionDetail { project: String, session: String },
    Updates,
    Remote,
}

/// Bottom confirmation bar (`PendingBar`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BottomBarState {
    None,
    WhyNot { sentence: String },
    QuitConfirm,
    AllowDevice { device_id: String, device_name: String },
    RemoveDeviceConfirm { device_id: String, device_name: String },
    SealDisposition { project: String, eval: String, selected: String },
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
    pub plus_popover_project: Option<String>,
    pub menu_open: bool,
    pub layout_menu_pane: Option<usize>,
    pub folded_open: HashSet<String>,
    pub folded_evals: HashSet<(String, String)>,
    pub unfolded_sealed: HashSet<String>,
    pub unfolded_earlier: HashSet<String>,
    pub conversations: HashMap<(PathBuf, u32), Vec<Item>>,
    pub unfolded_steps: HashSet<(PathBuf, u32, usize)>,
    /// Each agent's message field, by its project and pane: switching
    /// agents brings each one's draft back. The open Ask's field.
    pub messages: HashMap<(PathBuf, u32), TextField>,
    pub ask: TextField,
    /// A suggestion asked of the daemon and not answered yet: one at a time.
    pub suggesting: Option<Suggesting>,
    /// What each agent's field could show after its caret, and the draft it is for.
    suggestions: HashMap<(PathBuf, u32), (String, String)>,
    /// The agent in front's draft as last seen, and since when: it is asked
    /// about once it has settled.
    settling: Option<((PathBuf, u32), String, std::time::Instant)>,
    /// The last draft asked about, so each is asked once.
    asked: Option<((PathBuf, u32), String)>,
    pub clipboard: String,
    pub active_sheet: Option<ActiveSheet>,
    pub bottom_bar: BottomBarState,
    pub file_dialog: Arc<dyn FileDialog>,
    pub quit_requested: bool,
    pub live: bool,
}

impl WeftDesktopApp {
    /// Load durable state from `<home>/.fab7/weft/state.json` and connect to
    /// `weftd` for each open project.
    pub fn launch(home: PathBuf, machine: impl Into<String>) -> Self {
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
            folded_open: HashSet::new(),
            folded_evals: HashSet::new(),
            unfolded_sealed: HashSet::new(),
            unfolded_earlier: HashSet::new(),
            conversations: HashMap::new(),
            unfolded_steps: HashSet::new(),
            messages: HashMap::new(),
            ask: TextField::new(Shape::Area),
            suggesting: None,
            suggestions: HashMap::new(),
            settling: None,
            asked: None,
            clipboard: String::new(),
            active_sheet: None,
            bottom_bar: BottomBarState::None,
            file_dialog: Arc::new(MacFileDialog),
            quit_requested: false,
            live: true,
        };
        let projects = app.state.open_projects.clone();
        for p in projects {
            app.connect_or_add_project(p);
        }
        if let Some(first) = app.clients.first_mut() {
            let _ = first.setup("look", "", "");
        }
        app
    }

    /// Load durable state from `<home>/.fab7/weft/state.json` without blocking
    /// the UI thread on daemon socket handshakes. The interactive window loop
    /// connects each project in the background.
    pub fn launch_deferred(home: PathBuf, machine: impl Into<String>) -> Self {
        let mut app = Self::from_state_disconnected(home, machine);
        app.live = true;
        app
    }

    /// Replace a placeholder/disconnected `ProjectClient` with a newly connected
    /// live `ProjectClient` and open tabs for any running panes.
    pub fn integrate_connected_client(&mut self, mut client: ProjectClient) {
        let p = weft_core::state::normalize_project_path(&client.root);
        client.root = p.clone();
        if self.state.settings.turbo && !client.turbo {
            let _ = client.set_turbo(true);
        }
        let running_panes: Vec<u32> =
            client.panes.iter().filter(|pv| pv.running).map(|pv| pv.id).collect();
        if let Some(pos) = self.clients.iter().position(|c| {
            weft_core::state::normalize_project_path(&c.root) == p || c.name == client.name
        }) {
            self.clients[pos] = client;
        } else {
            self.clients.push(client);
        }
        // An agent already in some pane stays where it is: one agent in two
        // panes would be sized by both.
        for pane_id in running_panes {
            let shown = self.state.panes.groups.iter().any(|g| {
                g.tabs.iter().any(|t| {
                    weft_core::state::normalize_project_path(&t.project) == p && t.pane == pane_id
                })
            });
            if !shown {
                self.ensure_tab_in_active_pane(SavedPaneTab {
                    project: p.clone(),
                    pane: pane_id,
                    view: self.state.settings.default_view,
                });
            }
        }
    }

    fn connect_or_add_project(&mut self, p: PathBuf) {
        let p = weft_core::state::normalize_project_path(&p);
        if self.live
            && let Ok(client) = ProjectClient::open(&p, 28, 100)
        {
            self.integrate_connected_client(client);
            return;
        }
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("project").to_string();
        let mut client = ProjectClient::disconnected(p, name);
        client.turbo = self.state.settings.turbo;
        client.start_at_login = self.state.settings.start_at_login;
        self.clients.push(client);
    }

    /// Create an app backed by `<home>/.fab7/weft/state.json` using disconnected
    /// `ProjectClient` instances for fast deterministic tests.
    pub fn from_state_disconnected(home: PathBuf, machine: impl Into<String>) -> Self {
        let state = load_state_from_home(&home);
        let mut clients = Vec::new();
        for p in &state.open_projects {
            let p = weft_core::state::normalize_project_path(p);
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("project").to_string();
            let mut c = ProjectClient::disconnected(p, name);
            c.turbo = state.settings.turbo;
            c.start_at_login = state.settings.start_at_login;
            clients.push(c);
        }
        Self {
            home,
            machine: machine.into(),
            state,
            clients,
            selection: None,
            search: TextField::new(Shape::Line),
            plus_popover_project: None,
            menu_open: false,
            layout_menu_pane: None,
            folded_open: HashSet::new(),
            folded_evals: HashSet::new(),
            unfolded_sealed: HashSet::new(),
            unfolded_earlier: HashSet::new(),
            conversations: HashMap::new(),
            unfolded_steps: HashSet::new(),
            messages: HashMap::new(),
            ask: TextField::new(Shape::Area),
            suggesting: None,
            suggestions: HashMap::new(),
            settling: None,
            asked: None,
            clipboard: String::new(),
            active_sheet: None,
            bottom_bar: BottomBarState::None,
            file_dialog: Arc::new(MacFileDialog),
            quit_requested: false,
            live: false,
        }
    }

    /// Persist `self.state` atomically to `<home>/.fab7/weft/state.json` (and
    /// mirror `projects.json`).
    pub fn save_state(&self) -> Result<()> {
        save_state_to_home(&self.home, &self.state)
    }

    /// Pump all project clients for incoming daemon lines.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        for c in &mut self.clients {
            if c.pump() {
                changed = true;
            }
        }
        if changed {
            self.refresh_active_conversations();
        }
        self.suggest_tick(std::time::Instant::now()) || changed
    }

    /// Take a suggestion's answer, and ask about the agent in front's draft
    /// once it has settled. Whether what its field shows changed.
    pub fn suggest_tick(&mut self, now: std::time::Instant) -> bool {
        let front = self.active_tab().map(|t| (t.project, t.pane));
        let draft = self.message_text().to_string();
        let mut changed = false;
        if let Some(s) = self.suggesting.clone()
            && let Some(client) = self.clients.iter_mut().find(|c| c.root == s.key.0)
            && let Some(answer) = client.take_answer(s.id)
        {
            self.suggesting = None;
            // An answer for a draft no longer there, or a malformed one, shows nothing.
            let text = answer.ok().and_then(|v| v.get("text")?.as_str().map(str::to_string));
            let current = front.as_ref() == Some(&s.key) && draft == s.draft;
            match text.filter(|t| current && !t.is_empty()) {
                Some(t) => {
                    self.suggestions.insert(s.key, (s.draft, t));
                }
                None => {
                    self.suggestions.remove(&s.key);
                }
            }
            changed = true;
        }
        let Some(key) = front else { return changed };
        match &self.settling {
            Some((k, d, _)) if *k == key && *d == draft => {}
            _ => {
                self.settling = Some((key, draft, now));
                return changed;
            }
        }
        let settled = self.settling.as_ref().is_some_and(|(_, _, since)| now - *since >= SETTLED);
        // An empty field follows the agent's last reply: once its turn has
        // ended, again after each turn.
        let turn = self
            .clients
            .iter()
            .find(|c| c.root == key.0)
            .and_then(|c| c.view(key.1))
            .and_then(|v| v.turn);
        if draft.is_empty() && turn != Some(weft_core::turns::Turn::TurnEnded) {
            // Asked again once this turn ends.
            if self.asked.as_ref().is_some_and(|(k, d)| *k == key && d.is_empty()) {
                self.asked = None;
            }
            return changed;
        }
        let fresh = self.asked.as_ref() != Some(&(key.clone(), draft.clone()));
        if !settled || !fresh || self.suggesting.is_some() {
            return changed;
        }
        if let Some(client) = self.clients.iter_mut().find(|c| c.root == key.0)
            && let Ok(id) =
                client.tell(weft_proto::Call::Suggest { pane: key.1, draft: draft.clone() })
        {
            self.suggesting = Some(Suggesting { id, key: key.clone(), draft: draft.clone() });
            self.asked = Some((key, draft));
        }
        changed
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

    pub fn refresh_active_conversations(&mut self) {
        let mut targets = Vec::new();
        for group in &self.state.panes.groups {
            if !group.tabs.is_empty() {
                let idx = group.active_tab.min(group.tabs.len() - 1);
                let tab = &group.tabs[idx];
                if tab.view == PaneViewMode::Conversation {
                    targets.push((tab.project.clone(), tab.pane));
                }
            }
        }
        for (proj, pane) in targets {
            if let Some(client) = self.clients.iter_mut().find(|c| c.root == proj)
                && let Some(items) = client.fetch_conversation(pane)
            {
                self.conversations.insert((proj, pane), items);
            }
        }
    }

    pub fn client(&self, name_or_path: &str) -> Option<&ProjectClient> {
        self.clients
            .iter()
            .find(|c| c.name == name_or_path || c.root.to_string_lossy() == name_or_path)
    }

    pub fn client_mut(&mut self, name_or_path: &str) -> Option<&mut ProjectClient> {
        self.clients
            .iter_mut()
            .find(|c| c.name == name_or_path || c.root.to_string_lossy() == name_or_path)
    }

    pub fn harnesses(&self) -> Harnesses {
        for c in &self.clients {
            if !c.harnesses.0.is_empty() {
                return c.harnesses.clone();
            }
        }
        let dir = self.home.join(".fab7/weft/harnesses");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut files: Vec<(String, String)> = entries
                .flatten()
                .filter_map(|e| {
                    let path = e.path();
                    (path.extension().and_then(|x| x.to_str()) == Some("toml")).then(|| {
                        let name = path.file_name()?.to_str()?.to_string();
                        let text = std::fs::read_to_string(&path).ok()?;
                        Some((name, text))
                    })?
                })
                .collect();
            files.sort_by(|a, b| a.0.cmp(&b.0));
            return Harnesses::read(files).0;
        }
        Harnesses::default()
    }

    /// Build `weft_core::tree::Project` rows across all open projects.
    pub fn tree_projects(&self) -> Vec<Project> {
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
                rounds: &client.rounds,
                readiness: &ready_fn,
                routing: &client.routing,
                setup: client.needs,
            };
            let folded = |_: usize, fold: Fold| match fold {
                Fold::Project => self
                    .state
                    .folded_projects
                    .iter()
                    .any(|f| f == &client.root || f.to_string_lossy() == client.name),
                Fold::Open => self.folded_open.contains(&client.name),
                Fold::Eval(id) => {
                    self.folded_evals.contains(&(client.name.clone(), id.to_string()))
                }
                Fold::Sealed => !self.unfolded_sealed.contains(&client.name),
            };
            if let Some(p) = tree::rows(&[facts], &folded).into_iter().next() {
                out.push(p);
            }
        }
        out
    }

    /// Count total agents currently waiting (`Turn::Waiting`) across all open projects.
    pub fn total_needs_you(&self) -> usize {
        self.tree_projects().iter().map(|p| p.needs).sum()
    }

    /// Count available updates reported by `weftd` across Weft, RingFrame, Sense, and harnesses.
    pub fn updates_available_count(&self) -> usize {
        if let Some(view) = self.clients.iter().find_map(|c| c.setup.as_ref()) {
            let mut count = 0;
            if view.weft_behind {
                count += 1;
            }
            if view.configuration_behind {
                count += 1;
            }
            if view.sense_behind {
                count += 1;
            }
            count +=
                view.rows.iter().filter(|r| matches!(r.state, SetupState::Behind { .. })).count();
            return count;
        }
        if self.clients.iter().any(|c| c.update.is_some()) {
            return 1;
        }
        0
    }

    /// Count connected remote devices across `remote` views.
    pub fn remote_devices_count(&self) -> usize {
        self.clients
            .iter()
            .find_map(|c| c.remote.as_ref())
            .and_then(|v| v.get("devices"))
            .and_then(|d| d.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    }

    /// Whether Remote is enabled (`state == "on"` or `on == true`), independent
    /// of how many devices are currently paired.
    pub fn is_remote_on(&self) -> bool {
        self.clients
            .iter()
            .find_map(|c| c.remote.as_ref())
            .map(|v| {
                v.get("state").and_then(|s| s.as_str()) == Some("on")
                    || v.get("on").and_then(|b| b.as_bool()) == Some(true)
            })
            .unwrap_or(false)
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
        self.clients.iter().find(|c| c.root == project)?.view(pane)
    }

    pub fn pane_view_mut(&mut self, project: &Path, pane: u32) -> Option<&mut PaneView> {
        self.clients.iter_mut().find(|c| c.root == project)?.view_mut(pane)
    }

    /// The terminal of the tab in front of the focused pane group.
    pub fn active_pane_view_mut(&mut self) -> Option<&mut PaneView> {
        let tab = self.active_tab()?;
        self.pane_view_mut(&tab.project, tab.pane)
    }

    /// Give one pane the size its place on screen holds.
    pub fn resize_pane(&mut self, project: &Path, pane: u32, rows: u16, cols: u16) {
        if let Some(client) = self.clients.iter_mut().find(|c| c.root == project) {
            let _ = client.resize(pane, rows, cols);
        }
    }

    /// Drag a tab (`D15.4`) and persist the resulting split layout.
    pub fn drag_pane_tab(&mut self, from_group: usize, from_tab: usize, target: DropTarget) {
        if let Err(why_key) = drag_tab(&mut self.state.panes, from_group, from_tab, target) {
            self.bottom_bar = BottomBarState::WhyNot { sentence: words::word(why_key).to_string() };
        } else {
            let _ = self.save_state();
        }
    }

    /// Set the draft text in the open `Ask` / `Follow up` sheet.
    pub fn set_ask_draft(&mut self, text: &str) {
        if matches!(self.active_sheet, Some(ActiveSheet::Ask { .. })) {
            self.ask.set_text(text);
        }
    }

    /// The open `Ask` / `Follow up` sheet's field.
    pub fn ask_draft_mut(&mut self) -> Option<&mut TextField> {
        matches!(self.active_sheet, Some(ActiveSheet::Ask { .. })).then_some(&mut self.ask)
    }

    /// Scroll the active pane's terminal scrollback buffer by `delta` lines.
    pub fn scroll_active_pane(&mut self, delta: i32) {
        if let Some(pv) = self.active_pane_view_mut() {
            pv.scroll(delta);
        }
    }

    /// Dispatch a UI action string (`on_click` / keyboard shortcut).
    pub fn dispatch(&mut self, action: &str) {
        if action == "close_popovers" {
            self.menu_open = false;
            self.plus_popover_project = None;
            self.layout_menu_pane = None;
            return;
        }
        if !matches!(action, "toggle_menu" | "toggle_turbo" | "toggle_start_at_login")
            && !action.starts_with("toggle_plus:")
            && !action.starts_with("toggle_layout_menu:")
        {
            self.menu_open = false;
            self.plus_popover_project = None;
            self.layout_menu_pane = None;
        }

        if action == "toggle_sidebar" {
            self.state.settings.sidebar_open = !self.state.settings.sidebar_open;
            let _ = self.save_state();
            return;
        }
        if action == "toggle_menu" {
            self.menu_open = !self.menu_open;
            self.plus_popover_project = None;
            self.layout_menu_pane = None;
            return;
        }
        if action == "toggle_turbo" {
            let next = !self.state.settings.turbo;
            self.state.settings.turbo = next;
            for c in &mut self.clients {
                let _ = c.set_turbo(next);
            }
            let _ = self.save_state();
            return;
        }
        if action == "toggle_start_at_login" {
            let next = !self.state.settings.start_at_login;
            self.state.settings.start_at_login = next;
            let path = std::env::var("PATH").unwrap_or_default();
            for c in &mut self.clients {
                let _ = c.set_start_at_login(next, "weft", &path);
            }
            let _ = self.save_state();
            return;
        }
        if action == "clear_search" {
            self.search.set_text("");
            return;
        }
        if action == "open_project_dialog" {
            self.menu_open = false;
            if let Some(folder) = self.file_dialog.pick_folder() {
                self.open_project_path(folder);
            }
            return;
        }
        if action == "open_updates" {
            self.menu_open = false;
            for c in &mut self.clients {
                let _ = c.setup("look", "", "");
            }
            self.active_sheet = Some(ActiveSheet::Updates);
            return;
        }
        if action == "open_remote" {
            self.menu_open = false;
            self.active_sheet = Some(ActiveSheet::Remote);
            return;
        }
        if action == "close_sheet" {
            self.active_sheet = None;
            return;
        }
        if action == "request_quit" {
            self.menu_open = false;
            let any_running = self.clients.iter().any(|c| c.panes.iter().any(|p| p.running));
            if any_running {
                self.bottom_bar = BottomBarState::QuitConfirm;
            } else {
                self.quit_requested = true;
            }
            return;
        }
        if action == "cancel_bottom_bar" {
            self.bottom_bar = BottomBarState::None;
            return;
        }
        if action == "quit_leave_running" {
            for c in &mut self.clients {
                c.detach();
            }
            self.bottom_bar = BottomBarState::None;
            self.quit_requested = true;
            return;
        }
        if action == "quit_stop_agents" {
            for c in &mut self.clients {
                c.stop_agents();
                c.detach();
            }
            self.bottom_bar = BottomBarState::None;
            self.quit_requested = true;
            return;
        }

        // TreeBar actions (`Ask`, `Eval`, `Seal` on the bottom bar, D10.1/D10.2)
        if action == "bar_act_ask" {
            match &self.selection {
                Some(TreeSelection::Agent { project, session }) => {
                    let projects = self.tree_projects();
                    if let Some(tp) = projects.iter().find(|p| &p.name == project)
                        && let Some(arow) = tp.agents.rows.iter().find(|r| &r.session == session)
                    {
                        if let Some(why) = &arow.ask {
                            self.bottom_bar = BottomBarState::WhyNot { sentence: why.say() };
                            return;
                        }
                    }
                    self.ask = TextField::new(Shape::Area);
                    self.active_sheet = Some(ActiveSheet::Ask {
                        project: project.clone(),
                        target_session: Some(session.clone()),
                        follow_up_unit: None,
                    });
                }
                _ => {
                    if let Some(key) = tree::select(Act::Ask) {
                        self.bottom_bar =
                            BottomBarState::WhyNot { sentence: words::word(key).to_string() };
                    }
                }
            }
            return;
        }
        if action == "bar_act_eval" {
            match &self.selection {
                Some(TreeSelection::OpenGroup { project })
                | Some(TreeSelection::Ask { project, .. }) => {
                    let proj = project.clone();
                    self.trigger_eval(&proj);
                }
                _ => {
                    if let Some(key) = tree::select(Act::Eval) {
                        self.bottom_bar =
                            BottomBarState::WhyNot { sentence: words::word(key).to_string() };
                    }
                }
            }
            return;
        }
        if action == "bar_act_seal" {
            match &self.selection {
                Some(TreeSelection::EvalGroup { project, eval }) => {
                    let projects = self.tree_projects();
                    if let Some(tp) = projects.iter().find(|p| &p.name == project)
                        && let Some(eg) = tp.work.evals.iter().find(|g| &g.eval == eval)
                        && let Some(why) = &eg.seal
                    {
                        self.bottom_bar = BottomBarState::WhyNot { sentence: why.say() };
                        return;
                    }
                    self.bottom_bar = BottomBarState::SealDisposition {
                        project: project.clone(),
                        eval: eval.clone(),
                        selected: "accepted".to_string(),
                    };
                }
                _ => {
                    if let Some(key) = tree::select(Act::Seal) {
                        self.bottom_bar =
                            BottomBarState::WhyNot { sentence: words::word(key).to_string() };
                    }
                }
            }
            return;
        }

        // Ask sheet & PendingBar actions (D4.1–D4.14)
        if let Some(rest) = action.strip_prefix("select_ask_target:")
            && let Some((_proj, sess)) = rest.split_once(':')
        {
            if let Some(ActiveSheet::Ask { target_session, .. }) = &mut self.active_sheet {
                *target_session = Some(sess.to_string());
            }
            return;
        }
        if action == "submit_ask_sheet" {
            if let Some(ActiveSheet::Ask { project, target_session, follow_up_unit }) =
                self.active_sheet.take()
            {
                let draft = self.ask.text().to_string();
                self.submit_ask(
                    &project,
                    target_session.as_deref(),
                    follow_up_unit.as_deref(),
                    &draft,
                );
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("open_follow_up:")
            && let Some((proj, unit)) = rest.split_once(':')
        {
            self.ask = TextField::new(Shape::Area);
            self.active_sheet = Some(ActiveSheet::Ask {
                project: proj.to_string(),
                target_session: None,
                follow_up_unit: Some(unit.to_string()),
            });
            return;
        }
        if let Some(rest) = action.strip_prefix("act_proceed:")
            && let Some((proj, unit)) = rest.split_once(':')
        {
            if let Some(c) = self.client_mut(proj) {
                let pane = c.panes.iter().find(|p| p.running).map(|p| p.id);
                let _ = c.tell(weft_proto::Call::ConfirmAsk { unit: unit.to_string() });
                let _ = c.act("proceed", Some(unit), pane, None, None, None);
                if let Some(u) = c.units.iter_mut().find(|u| u.ask_id == unit) {
                    u.confirmed = true;
                }
            }
            self.active_sheet = None;
            return;
        }
        if let Some(rest) = action.strip_prefix("resolve_staged:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 3 {
                let (proj, id, mode) = (parts[0], parts[1], parts[2]);
                if let Some(c) = self.client_mut(proj) {
                    let yes = mode != "no";
                    let force = mode == "force";
                    let _ = c.resolve(id, yes, force, None, None);
                }
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("select_seal_disposition:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 3 {
                self.bottom_bar = BottomBarState::SealDisposition {
                    project: parts[0].to_string(),
                    eval: parts[1].to_string(),
                    selected: parts[2].to_string(),
                };
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("confirm_seal_disposition:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 3 {
                let (proj, eval_id, decision) = (parts[0], parts[1], parts[2]);
                if let Some(c) = self.client_mut(proj) {
                    let pane = c.panes.iter().find(|p| p.running).map(|p| p.id);
                    let _ = c.act("seal", None, pane, Some(decision), Some(eval_id), None);
                }
                self.bottom_bar = BottomBarState::None;
                self.active_sheet = None;
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("open_eval_view:")
            && let Some((proj, eval_id)) = rest.split_once(':')
        {
            if let Some(c) = self.client_mut(proj) {
                c.fetch_eval_record(eval_id);
            }
            self.active_sheet = Some(ActiveSheet::EvalView {
                project: proj.to_string(),
                eval: eval_id.to_string(),
                by_file: false,
                selected_step: 0,
            });
            return;
        }
        if let Some(rest) = action.strip_prefix("toggle_eval_view_mode:")
            && let Some((proj, eval_id)) = rest.split_once(':')
        {
            let next_by_file = match &self.active_sheet {
                Some(ActiveSheet::EvalView { by_file, .. }) => !*by_file,
                _ => true,
            };
            self.active_sheet = Some(ActiveSheet::EvalView {
                project: proj.to_string(),
                eval: eval_id.to_string(),
                by_file: next_by_file,
                selected_step: 0,
            });
            return;
        }
        if let Some(rest) = action.strip_prefix("resume_session:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 3 {
                let (proj, harness, session_id) = (parts[0], parts[1], parts[2]);
                if self.start_agent_in_project(proj, harness, Some(session_id)).is_some() {
                    self.active_sheet = None;
                }
            }
            return;
        }

        // Updates & Setup actions (D7.2)
        if action == "setup_update_all" {
            for c in &mut self.clients {
                let _ = c.setup("all", "", "");
            }
            return;
        }
        if let Some(target) = action.strip_prefix("setup_run:") {
            for c in &mut self.clients {
                let _ = c.setup("run", target, "");
            }
            return;
        }
        if let Some(harness) = action.strip_prefix("locate_harness:") {
            if let Some(path) = self.file_dialog.pick_file(&format!("Find {harness} application")) {
                let path_str = path.to_string_lossy().into_owned();
                for c in &mut self.clients {
                    let _ = c.setup("locate", harness, &path_str);
                }
            }
            return;
        }

        // Remote actions (D8.3)
        if action == "remote_turn_off" {
            for c in &mut self.clients {
                let _ = c.remote_act(serde_json::json!({"act": "off"}));
                if let Some(rem) = c.remote.as_mut().and_then(|v| v.as_object_mut()) {
                    rem.insert("on".into(), serde_json::Value::Bool(false));
                    rem.insert("state".into(), serde_json::Value::String("off".into()));
                }
            }
            self.bottom_bar = BottomBarState::None;
            return;
        }
        if let Some(rest) = action.strip_prefix("request_remove_device:") {
            let (dev_id, dev_name) = rest.split_once(':').unwrap_or((rest, rest));
            self.bottom_bar = BottomBarState::RemoveDeviceConfirm {
                device_id: dev_id.to_string(),
                device_name: dev_name.to_string(),
            };
            return;
        }
        if let Some(dev_id) = action.strip_prefix("remote_remove_device:") {
            for c in &mut self.clients {
                let _ = c.remote_act(serde_json::json!({"act": "remove", "device": dev_id}));
                if let Some(rem) = c.remote.as_mut().and_then(|v| v.as_object_mut()) {
                    if let Some(devs) = rem.get_mut("devices").and_then(|d| d.as_array_mut()) {
                        devs.retain(|item| item.get("id").and_then(|s| s.as_str()) != Some(dev_id));
                    }
                    // Removing all devices does NOT turn off Remote
                    rem.insert("on".into(), serde_json::Value::Bool(true));
                    rem.insert("state".into(), serde_json::Value::String("on".into()));
                }
            }
            self.bottom_bar = BottomBarState::None;
            return;
        }
        if let Some(rest) = action.strip_prefix("remote_allow_device:")
            && let Some((dev_id, ans)) = rest.split_once(':')
        {
            let allow = ans == "yes";
            for c in &mut self.clients {
                let _ = c.remote_act(serde_json::json!({
                    "act": "allow",
                    "device": dev_id,
                    "allow": allow,
                }));
            }
            self.bottom_bar = BottomBarState::None;
            return;
        }

        // Layout menu and split-pane actions (D15.1–D15.4)
        if let Some(g_str) = action.strip_prefix("toggle_layout_menu:")
            && let Ok(g_idx) = g_str.parse::<usize>()
        {
            if self.layout_menu_pane == Some(g_idx) {
                self.layout_menu_pane = None;
            } else {
                self.layout_menu_pane = Some(g_idx);
                self.menu_open = false;
                self.plus_popover_project = None;
            }
            return;
        }
        if let Some(g_str) = action.strip_prefix("split_right:")
            && let Ok(g_idx) = g_str.parse::<usize>()
        {
            self.layout_menu_pane = None;
            match split_pane_right(&mut self.state.panes, g_idx) {
                Ok(()) => {
                    let _ = self.save_state();
                }
                Err(why_key) => {
                    self.bottom_bar =
                        BottomBarState::WhyNot { sentence: words::word(why_key).to_string() };
                }
            }
            return;
        }
        if let Some(g_str) = action.strip_prefix("split_down:")
            && let Ok(g_idx) = g_str.parse::<usize>()
        {
            self.layout_menu_pane = None;
            match split_pane_down(&mut self.state.panes, g_idx) {
                Ok(()) => {
                    let _ = self.save_state();
                }
                Err(why_key) => {
                    self.bottom_bar =
                        BottomBarState::WhyNot { sentence: words::word(why_key).to_string() };
                }
            }
            return;
        }
        if let Some(g_str) = action.strip_prefix("move_tab_next:")
            && let Ok(g_idx) = g_str.parse::<usize>()
        {
            self.layout_menu_pane = None;
            match move_tab_to_next_pane(&mut self.state.panes, g_idx) {
                Ok(()) => {
                    let _ = self.save_state();
                }
                Err(why_key) => {
                    self.bottom_bar =
                        BottomBarState::WhyNot { sentence: words::word(why_key).to_string() };
                }
            }
            return;
        }
        if action == "join_all_panes" {
            self.layout_menu_pane = None;
            join_all_panes(&mut self.state.panes);
            let _ = self.save_state();
            return;
        }
        if let Some(g_str) = action.strip_prefix("focus_pane_group:")
            && let Ok(g_idx) = g_str.parse::<usize>()
        {
            if g_idx < self.state.panes.groups.len() {
                self.state.panes.focused_group = g_idx;
                let _ = self.save_state();
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("select_tab:")
            && let Some((g_str, t_str)) = rest.split_once(':')
            && let (Ok(g_idx), Ok(t_idx)) = (g_str.parse::<usize>(), t_str.parse::<usize>())
        {
            if let Some(group) = self.state.panes.groups.get_mut(g_idx)
                && t_idx < group.tabs.len()
            {
                group.active_tab = t_idx;
                self.state.panes.focused_group = g_idx;
                let _ = self.save_state();
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("close_tab:")
            && let Some((g_str, t_str)) = rest.split_once(':')
            && let (Ok(g_idx), Ok(t_idx)) = (g_str.parse::<usize>(), t_str.parse::<usize>())
        {
            if let Some(group) = self.state.panes.groups.get_mut(g_idx)
                && t_idx < group.tabs.len()
            {
                group.tabs.remove(t_idx);
                if group.active_tab >= group.tabs.len() {
                    group.active_tab = group.tabs.len().saturating_sub(1);
                }
                if group.tabs.is_empty() && self.state.panes.groups.len() > 1 {
                    self.state.panes.groups.remove(g_idx);
                    if self.state.panes.focused_group >= self.state.panes.groups.len() {
                        self.state.panes.focused_group =
                            self.state.panes.groups.len().saturating_sub(1);
                    }
                }
                let _ = self.save_state();
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("set_tab_view:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 3
                && let (Ok(g_idx), Ok(t_idx)) =
                    (parts[0].parse::<usize>(), parts[1].parse::<usize>())
                && let Some(group) = self.state.panes.groups.get_mut(g_idx)
                && let Some(tab) = group.tabs.get_mut(t_idx)
            {
                tab.view = match parts[2] {
                    "conversation" => PaneViewMode::Conversation,
                    _ => PaneViewMode::Terminal,
                };
                let (proj, pane, is_conv) =
                    (tab.project.clone(), tab.pane, tab.view == PaneViewMode::Conversation);
                if is_conv && let Some(client) = self.clients.iter_mut().find(|c| c.root == proj) {
                    if client.is_connected() {
                        if let Some(items) = client.fetch_conversation(pane) {
                            self.conversations.insert((proj, pane), items);
                        }
                    } else {
                        let _ = client.read("conversation", "", Some(pane), None, None, None);
                    }
                }
                let _ = self.save_state();
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("toggle_steps:") {
            let mut rsplit = rest.rsplitn(3, ':');
            if let (Some(b_str), Some(p_str), Some(proj_str)) =
                (rsplit.next(), rsplit.next(), rsplit.next())
                && let (Ok(pane), Ok(block_idx)) = (p_str.parse::<u32>(), b_str.parse::<usize>())
            {
                let key = (PathBuf::from(proj_str), pane, block_idx);
                if !self.unfolded_steps.remove(&key) {
                    self.unfolded_steps.insert(key);
                }
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("send_choice_key:")
            && let Some((g_str, key)) = rest.split_once(':')
            && let Ok(g_idx) = g_str.parse::<usize>()
        {
            let bytes: &[u8] = match key {
                "up" => b"\x1b[A",
                "down" => b"\x1b[B",
                "enter" => b"\r",
                "esc" => b"\x1b",
                _ => return,
            };
            if let Some(group) = self.state.panes.groups.get(g_idx)
                && let Some(tab) =
                    group.tabs.get(group.active_tab.min(group.tabs.len().saturating_sub(1)))
                && let Some(client) = self.clients.iter_mut().find(|c| c.root == tab.project)
            {
                let _ = client.input(tab.pane, bytes);
            }
            return;
        }
        if let Some(rest) = action.strip_prefix("send_special_key:")
            && let Some((g_str, key)) = rest.split_once(':')
            && let Ok(g_idx) = g_str.parse::<usize>()
        {
            let bytes: &[u8] = match key {
                "esc" => b"\x1b",
                "tab" => b"\t",
                "shift_tab" => b"\x1b[Z",
                "up" => b"\x1b[A",
                "down" => b"\x1b[B",
                "enter" => b"\r",
                "ctrl_c" => b"\x03",
                _ => return,
            };
            if let Some(group) = self.state.panes.groups.get(g_idx)
                && let Some(tab) =
                    group.tabs.get(group.active_tab.min(group.tabs.len().saturating_sub(1)))
                && let Some(client) = self.clients.iter_mut().find(|c| c.root == tab.project)
            {
                let _ = client.input(tab.pane, bytes);
            }
            return;
        }
        if action == "take_suggestion" {
            self.take_suggestion();
            return;
        }
        if action == "send_message_row" {
            let text = self.message_field().text().to_string();
            self.message_field().set_text("");
            if !text.is_empty() {
                let g_idx = self
                    .state
                    .panes
                    .focused_group
                    .min(self.state.panes.groups.len().saturating_sub(1));
                if let Some(group) = self.state.panes.groups.get(g_idx)
                    && let Some(tab) =
                        group.tabs.get(group.active_tab.min(group.tabs.len().saturating_sub(1)))
                    && let Some(client) = self.clients.iter_mut().find(|c| c.root == tab.project)
                {
                    let mut payload = text.into_bytes();
                    payload.push(b'\r');
                    let _ = client.input(tab.pane, &payload);
                }
            }
            return;
        }

        if let Some(proj) = action.strip_prefix("toggle_plus:") {
            if self.plus_popover_project.as_deref() == Some(proj) {
                self.plus_popover_project = None;
            } else {
                self.plus_popover_project = Some(proj.to_string());
                self.menu_open = false;
            }
            return;
        }

        if let Some(rest) = action.strip_prefix("start_agent:")
            && let Some((proj, harness)) = rest.split_once(':')
        {
            self.start_agent_in_project(proj, harness, None);
            self.plus_popover_project = None;
            return;
        }

        if let Some(proj) = action.strip_prefix("close_project:") {
            self.close_project(proj);
            return;
        }

        if let Some(proj) = action.strip_prefix("toggle_project:") {
            self.toggle_project_fold(proj);
            return;
        }

        if let Some(proj) = action.strip_prefix("select_project:") {
            self.selection = Some(TreeSelection::Project { project: proj.to_string() });
            return;
        }

        if let Some(rest) = action.strip_prefix("select_agent:")
            && let Some((proj, session)) = rest.split_once(':')
        {
            self.selection = Some(TreeSelection::Agent {
                project: proj.to_string(),
                session: session.to_string(),
            });
            self.focus_or_open_agent_session(proj, session);
            return;
        }

        if let Some(rest) = action.strip_prefix("open_ask:")
            && let Some((proj, session)) = rest.split_once(':')
        {
            self.ask = TextField::new(Shape::Area);
            self.active_sheet = Some(ActiveSheet::Ask {
                project: proj.to_string(),
                target_session: Some(session.to_string()),
                follow_up_unit: None,
            });
            return;
        }

        if let Some(proj) = action.strip_prefix("open_earlier:") {
            self.selection = Some(TreeSelection::EarlierSessions { project: proj.to_string() });
            if !self.unfolded_earlier.remove(proj) {
                self.unfolded_earlier.insert(proj.to_string());
            }
            return;
        }

        if let Some(proj) = action.strip_prefix("toggle_open:") {
            if !self.folded_open.remove(proj) {
                self.folded_open.insert(proj.to_string());
            }
            return;
        }

        if let Some(proj) = action.strip_prefix("select_open:") {
            self.selection = Some(TreeSelection::OpenGroup { project: proj.to_string() });
            return;
        }

        if let Some(rest) = action.strip_prefix("select_ask:")
            && let Some((proj, unit)) = rest.split_once(':')
        {
            self.selection =
                Some(TreeSelection::Ask { project: proj.to_string(), unit: unit.to_string() });
            self.active_sheet =
                Some(ActiveSheet::WorkDetail { project: proj.to_string(), unit: unit.to_string() });
            return;
        }

        if let Some(rest) = action.strip_prefix("toggle_eval:")
            && let Some((proj, eval)) = rest.split_once(':')
        {
            let key = (proj.to_string(), eval.to_string());
            if !self.folded_evals.remove(&key) {
                self.folded_evals.insert(key);
            }
            return;
        }

        if let Some(rest) = action.strip_prefix("select_eval:")
            && let Some((proj, eval)) = rest.split_once(':')
        {
            if let Some(c) = self.client_mut(proj) {
                c.fetch_eval_record(eval);
            }
            self.selection = Some(TreeSelection::EvalGroup {
                project: proj.to_string(),
                eval: eval.to_string(),
            });
            self.active_sheet =
                Some(ActiveSheet::EvalDetail { project: proj.to_string(), eval: eval.to_string() });
            return;
        }

        if let Some(proj) = action.strip_prefix("toggle_sealed:") {
            self.selection = Some(TreeSelection::SealedGroup { project: proj.to_string() });
            if !self.unfolded_sealed.remove(proj) {
                self.unfolded_sealed.insert(proj.to_string());
            }
            return;
        }

        if let Some(rest) = action.strip_prefix("select_seal:")
            && let Some((proj, seal)) = rest.split_once(':')
        {
            self.selection =
                Some(TreeSelection::Seal { project: proj.to_string(), seal: seal.to_string() });
            self.active_sheet =
                Some(ActiveSheet::SealDetail { project: proj.to_string(), seal: seal.to_string() });
            return;
        }

        if let Some(proj) = action.strip_prefix("act_eval:") {
            self.trigger_eval(proj);
            return;
        }

        if let Some(rest) = action.strip_prefix("open_seal:")
            && let Some((proj, eval)) = rest.split_once(':')
        {
            self.bottom_bar = BottomBarState::SealDisposition {
                project: proj.to_string(),
                eval: eval.to_string(),
                selected: "accepted".to_string(),
            };
        }
    }

    pub fn send_active_pane_input(&mut self, bytes: &[u8]) {
        let Some(tab) = self.active_tab() else { return };
        self.send_pane_input(&tab.project, tab.pane, bytes);
    }

    pub fn send_pane_input(&mut self, project: &Path, pane: u32, bytes: &[u8]) {
        if let Some(client) = self.clients.iter_mut().find(|c| c.root == project) {
            let _ = client.input(pane, bytes);
        }
    }

    fn submit_ask(
        &mut self,
        proj: &str,
        target_session: Option<&str>,
        follow_up_unit: Option<&str>,
        draft: &str,
    ) {
        let Some(client) = self.client_mut(proj) else {
            return;
        };
        let pane = target_session
            .and_then(|sid| client.sessions.iter().find(|r| r.id == sid))
            .and_then(|r| r.pane)
            .filter(|&pid| client.view(pid).is_some())
            .or_else(|| client.panes.iter().find(|p| p.running).map(|p| p.id));

        let act = if follow_up_unit.is_some() { "follow_up" } else { "ask" };
        let _ = client.act(act, follow_up_unit, pane, Some(draft), None, None);

        // In disconnected mode, stage a pending prompt so the full D4.5–D4.11
        // PendingBar lifecycle can be tested deterministically.
        if !client.is_connected()
            && let Some(pid) = pane
        {
            let pid_num = client.waiting.len() + 1;
            client.waiting.push(Staged {
                id: format!("pend_{pid_num}"),
                pane: pid,
                what: format!("/rf:ask {draft}"),
                why: draft.to_string(),
                payload: draft.as_bytes().to_vec(),
                sends: follow_up_unit.map(str::to_string),
            });
        } else {
            client.pump();
        }
    }

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
        if !self.clients.iter().any(|c| weft_core::state::normalize_project_path(&c.root) == path) {
            self.connect_or_add_project(path);
        }
        let _ = self.save_state();
    }

    pub fn close_project(&mut self, proj: &str) {
        let removed_paths: Vec<PathBuf> = self
            .clients
            .iter()
            .filter(|c| c.name == proj || c.root.to_string_lossy() == proj)
            .map(|c| c.root.clone())
            .collect();
        for c in &mut self.clients {
            if c.name == proj || c.root.to_string_lossy() == proj {
                let _ = c.dismiss();
                c.detach();
            }
        }
        self.clients.retain(|c| c.name != proj && c.root.to_string_lossy() != proj);
        self.state
            .open_projects
            .retain(|p| p.to_string_lossy() != proj && !removed_paths.contains(p));
        self.state
            .folded_projects
            .retain(|p| p.to_string_lossy() != proj && !removed_paths.contains(p));
        let _ = self.save_state();
    }

    pub fn toggle_project_fold(&mut self, proj: &str) {
        let target_path =
            self.client(proj).map(|c| c.root.clone()).unwrap_or_else(|| PathBuf::from(proj));
        if let Some(pos) = self
            .state
            .folded_projects
            .iter()
            .position(|p| p == &target_path || p.to_string_lossy() == proj)
        {
            self.state.folded_projects.remove(pos);
        } else {
            self.state.folded_projects.push(target_path);
        }
        let _ = self.save_state();
    }

    /// Start an agent of `harness` in `proj` and open/focus its tab in the active pane.
    pub fn start_agent_in_project(
        &mut self,
        proj: &str,
        harness: &str,
        session: Option<&str>,
    ) -> Option<u32> {
        // If we are in live mode and the project client is still disconnected,
        // connect to weftd first so we never spawn into a fake disconnected client.
        if self.live {
            let needs_connect = self.client(proj).map(|c| (!c.is_connected(), c.root.clone()));
            if let Some((true, root)) = needs_connect {
                match ProjectClient::open(&root, 28, 100) {
                    Ok(connected) => self.integrate_connected_client(connected),
                    Err(err) => {
                        self.bottom_bar = BottomBarState::WhyNot {
                            sentence: format!("Could not connect to Weft daemon: {err}"),
                        };
                        return None;
                    }
                }
            }
        }

        if let Some(sid) = session
            && let Some(client) = self.client(proj)
            && let Some(row) = client.sessions.iter().find(|r| r.id == sid)
        {
            if row.state == weft_core::sessions::State::Running
                && let Some(pid) = row.pane
            {
                let proj_path = client.root.clone();
                let default_view = self.state.settings.default_view;
                self.ensure_tab_in_active_pane(SavedPaneTab {
                    project: proj_path,
                    pane: pid,
                    view: default_view,
                });
                let _ = self.save_state();
                return Some(pid);
            }
            if !row.resumable && row.state != weft_core::sessions::State::Running {
                self.bottom_bar =
                    BottomBarState::WhyNot { sentence: words::word("resume.nothing").to_string() };
                return None;
            }
        }

        let spec = self
            .harnesses()
            .find(harness)
            .map(|h| match session {
                Some(sid) => h.resume_spec(sid).unwrap_or_else(|| h.spec()),
                None => h.spec(),
            })
            .unwrap_or_else(|| harness.to_string());

        let spawn_res = {
            let client = self.client_mut(proj)?;
            client.spawn(harness, &spec, session).map(|pid| {
                client.pump();
                (client.root.clone(), pid)
            })
        };

        let (proj_path, pane_id) = match spawn_res {
            Ok(pair) => pair,
            Err(err) => {
                self.bottom_bar = BottomBarState::WhyNot { sentence: err.to_string() };
                return None;
            }
        };

        let default_view = self.state.settings.default_view;
        self.ensure_tab_in_active_pane(SavedPaneTab {
            project: proj_path,
            pane: pane_id,
            view: default_view,
        });
        let _ = self.save_state();
        Some(pane_id)
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

    fn focus_or_open_agent_session(&mut self, proj: &str, session: &str) {
        if let Some(pid_str) = session.strip_prefix("pane:")
            && let Ok(pane_id) = pid_str.parse::<u32>()
            && let Some(proj_path) = self.client(proj).map(|c| c.root.clone())
        {
            self.active_sheet = None;
            self.ensure_tab_in_active_pane(SavedPaneTab {
                project: proj_path,
                pane: pane_id,
                view: self.state.settings.default_view,
            });
            let _ = self.save_state();
            return;
        }

        let Some((proj_path, state, pane_id)) = self.client(proj).and_then(|client| {
            client
                .sessions
                .iter()
                .find(|r| r.id == session)
                .map(|r| (client.root.clone(), r.state, r.pane))
        }) else {
            return;
        };
        if state == weft_core::sessions::State::Running
            && let Some(pane_id) = pane_id
        {
            self.active_sheet = None;
            for (g_idx, group) in self.state.panes.groups.iter_mut().enumerate() {
                if let Some(t_idx) = group.tabs.iter().position(|t| {
                    (t.project == proj_path || t.project.to_string_lossy() == proj)
                        && t.pane == pane_id
                }) {
                    group.active_tab = t_idx;
                    self.state.panes.focused_group = g_idx;
                    let _ = self.save_state();
                    return;
                }
            }
            self.ensure_tab_in_active_pane(SavedPaneTab {
                project: proj_path,
                pane: pane_id,
                view: PaneViewMode::Terminal,
            });
            let _ = self.save_state();
        } else {
            self.active_sheet = Some(ActiveSheet::SessionDetail {
                project: proj.to_string(),
                session: session.to_string(),
            });
        }
    }

    pub fn trigger_eval(&mut self, proj: &str) {
        let projects = self.tree_projects();
        if let Some(tp) = projects.iter().find(|p| p.name == proj)
            && let Some(why) = &tp.work.open.eval
        {
            self.bottom_bar = BottomBarState::WhyNot { sentence: why.say() };
            return;
        }
        if let Some(c) = self.client_mut(proj) {
            let pane = c.panes.iter().find(|p| p.running).map(|p| p.id);
            let _ = c.act("eval", None, pane, None, None, None);
            if !c.is_connected()
                && let Some(pid) = pane
            {
                let pid_num = c.waiting.len() + 1;
                c.waiting.push(Staged {
                    id: format!("pend_{pid_num}"),
                    pane: pid,
                    what: "/rf:eval".into(),
                    why: "eval".into(),
                    payload: b"/rf:eval".to_vec(),
                    sends: None,
                });
            } else {
                c.pump();
            }
        }
    }

    /// Render the complete `weft-desktop` GPUI element tree.
    pub fn render(&self) -> Element {
        let projects = self.tree_projects();
        let harnesses = self.harnesses();
        let needs_you = self.total_needs_you();
        let updates_available = self.updates_available_count();
        let remote_on = self.is_remote_on();
        let remote_devices = self.remote_devices_count();

        let header = render_header(&HeaderProps {
            machine: self.machine.clone(),
            sidebar_open: self.state.settings.sidebar_open,
            needs_you,
            menu_open: self.menu_open,
        });

        let mut body = div()
            .layer("body")
            .self_stretch()
            .flex_1()
            .flex_row()
            .items_center()
            .style("min-height", "0")
            .style("position", "relative");

        if self.state.settings.sidebar_open {
            body = body.child(render_tree(&TreeProps {
                projects: &projects,
                selection: self.selection.as_ref(),
                search_query: self.search.text(),
                plus_popover_project: self.plus_popover_project.as_deref(),
                harnesses: &harnesses,
                turbo: self.state.settings.turbo,
                unfolded_earlier: &self.unfolded_earlier,
                clients: &self.clients,
            }));
        }

        let sheet = self.active_sheet.as_ref().map(|sheet| {
            render_sheet(&SheetProps {
                sheet,
                projects: &projects,
                clients: &self.clients,
                harnesses: &harnesses,
                updates_count: updates_available,
                bottom_bar: &self.bottom_bar,
                ask_draft: self.ask.text(),
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
                    ghost: self.ghost(),
                    dock,
                }))
            }
        }

        if self.menu_open || self.plus_popover_project.is_some() || self.layout_menu_pane.is_some()
        {
            body = body.child(
                div()
                    .layer("popover backdrop")
                    .on_click("close_popovers")
                    .style("position", "absolute")
                    .style("inset", "0"),
            );
        }

        if let Some(plus_proj) = &self.plus_popover_project {
            body =
                body.child(render_plus_popover(plus_proj, &harnesses, self.state.settings.turbo));
        }

        if self.menu_open {
            body = body.child(render_menu_popover(
                self.state.settings.turbo,
                self.state.settings.start_at_login,
                remote_on,
                remote_devices,
                updates_available,
            ));
        }

        let (w, h) = self
            .state
            .settings
            .window_bounds
            .map(|b| (b.width as u32, b.height as u32))
            .unwrap_or((1280, 800));

        div()
            .layer("window")
            .w(w)
            .h(h)
            .style("position", "relative")
            .style("overflow", "hidden")
            .shrink_0()
            .style("font-family", "Sora")
            .color(palette::TEXT)
            .flex_col()
            .items_start()
            .bg(palette::CANVAS)
            .child(render_titlebar(&self.machine))
            .child(header)
            .child(body)
            .child(render_bottom_bar(&BottomBarProps {
                machine: &self.machine,
                state: &self.bottom_bar,
                clients: &self.clients,
                selection: self.selection.as_ref(),
            }))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;
    use weft_core::conversation::{Item, Kind};
    use weft_core::harness::Harness;
    use weft_core::ledger::{Check, Sent, Unit, Verdict};
    use weft_core::onboarding::{Row as SetupRow, State as SetupState, View as SetupView};
    use weft_core::rounds::{Named, Round, Rounds, Sealed as SealedRound};
    use weft_core::sessions::{Row, State};
    use weft_core::state::SplitLayout;
    use weft_core::turns::Turn;
    use weft_proto::Call;

    struct MockFileDialog {
        folder: Mutex<Option<PathBuf>>,
        file: Mutex<Option<PathBuf>>,
    }

    impl FileDialog for MockFileDialog {
        fn pick_folder(&self) -> Option<PathBuf> {
            self.folder.lock().unwrap().clone()
        }
        fn pick_file(&self, _title: &str) -> Option<PathBuf> {
            self.file.lock().unwrap().clone()
        }
    }

    fn sample_harnesses() -> Harnesses {
        let mk = |name: &str, title: &str, prog: &str| Harness {
            name: name.to_string(),
            title: title.to_string(),
            program: prog.to_string(),
            args: Vec::new(),
            config_env: None,
            config_default: format!(".{name}"),
            list: vec!["list".into()],
            add_marketplace: None,
            install_plugin: None,
            update_marketplace: None,
            update_plugin: None,
            listing: None,
            manifest: None,
            resume: vec!["--resume".into()],
            transcript: None,
            turbo: vec!["--turbo".into()],
            unset_env: Vec::new(),
            signal: None,
            listen: None,
            resume_joins: false,
            server_args: Vec::new(),
            conversation: None,
        };
        Harnesses(vec![
            mk("antigravity", "Antigravity", "agy"),
            mk("claude", "Claude Code", "claude"),
            mk("codex", "Codex", "codex"),
            mk("grok", "Grok Build", "grok"),
        ])
    }

    fn populate_sample_project(c: &mut ProjectClient) {
        c.harnesses = sample_harnesses();
        let u1 = Unit {
            ask_id: "ask_1".into(),
            title: "fix login".into(),
            harness: "antigravity".into(),
            confirmed: true,
            sent: Sent::TakenByAgent,
            ..Default::default()
        };
        let u2 = Unit {
            ask_id: "ask_2".into(),
            title: "telemetry".into(),
            harness: "claude".into(),
            confirmed: false,
            sent: Sent::TakenByAgent,
            requested_only: true,
            ..Default::default()
        };
        let check = Check {
            eval_id: "evl_3".into(),
            verdict: Verdict::Matches,
            agreement: 0.67,
            judged_by: None,
        };
        let u3 = Unit {
            ask_id: "ask_3".into(),
            title: "health endpoint".into(),
            harness: "codex".into(),
            confirmed: true,
            sent: Sent::TakenByAgent,
            check: Some(check.clone()),
            ..Default::default()
        };
        let u4 = Unit {
            ask_id: "ask_4".into(),
            title: "build number".into(),
            harness: "codex".into(),
            confirmed: true,
            sent: Sent::TakenByAgent,
            check: Some(check),
            ..Default::default()
        };
        c.units = vec![u1, u2, u3, u4];

        let mut r1 = Row::of("antigravity", "s1", State::Running, Some(Turn::Waiting), 1124);
        r1.title = Some("fix login".into());
        r1.pane = Some(0);
        let mut r2 = Row::of("codex", "s2", State::Running, Some(Turn::Working), 1131);
        r2.pane = Some(1);
        let mut r3 = Row::of("claude", "s3", State::Running, Some(Turn::TurnEnded), 1052);
        r3.title = Some("health endpoint".into());
        r3.pane = Some(2);
        let mut r4 = Row::of("claude", "s4", State::Ended, None, 912);
        r4.title = Some("readme fix".into());
        c.sessions = vec![r1, r2, r3, r4];
        for i in 0..14 {
            c.sessions.push(Row::of("claude", &format!("old_{i}"), State::Ended, None, 800 - i));
        }

        c.rounds = Rounds {
            judging: Vec::new(),
            open: vec!["ask_1".into(), "ask_2".into()],
            since: Some(3),
            evals: vec![Round {
                eval: "evl_3".into(),
                n: 3,
                judging: false,
                verdict: Some(Verdict::Matches),
                agreement: Some(0.67),
                asks: vec!["ask_3".into(), "ask_4".into()],
                sealed: None,
            }],
            seals: vec![SealedRound {
                seal: "sel_4".into(),
                n: 4,
                decision: "accepted".into(),
                at: String::new(),
                by: "you".into(),
                eval: Some(Named { id: "evl_2".into(), n: 2 }),
                asks: vec!["old_a".into(), "old_b".into()],
            }],
            record: Default::default(),
        };
    }

    #[test]
    fn restores_window_bounds_and_open_projects_from_durable_state_json() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut initial = WeftState::default();
        initial.open_projects = vec![PathBuf::from("/work/weft"), PathBuf::from("/work/docs")];
        initial.folded_projects = vec![PathBuf::from("/work/docs")];
        initial.settings.turbo = true;
        initial.settings.sidebar_open = true;
        initial.settings.window_bounds =
            Some(WindowBounds { x: 120.0, y: 80.0, width: 1440.0, height: 900.0 });
        save_state_to_home(&home, &initial).unwrap();

        let mut app = WeftDesktopApp::from_state_disconnected(home.clone(), "studio");
        assert_eq!(app.clients.len(), 2);
        assert_eq!(app.clients[0].name, "weft");
        assert_eq!(app.clients[1].name, "docs");
        assert!(app.state.settings.turbo);
        assert_eq!(
            app.state.settings.window_bounds,
            Some(WindowBounds { x: 120.0, y: 80.0, width: 1440.0, height: 900.0 })
        );

        let projects = app.tree_projects();
        assert!(!projects[0].folded);
        assert!(projects[1].folded, "docs was folded in state.json");

        // Closing docs updates open_projects and persists atomically to state.json
        app.dispatch("close_project:docs");
        let reloaded = load_state_from_home(&home);
        assert_eq!(reloaded.open_projects, vec![PathBuf::from("/work/weft")]);
    }

    #[test]
    fn renders_header_and_collapsible_tree_matching_d2_6_d2_9_d11_1_d11_8_and_d15_7() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut initial = WeftState::default();
        initial.open_projects = vec![PathBuf::from("/work/weft")];
        initial.settings.turbo = true;
        save_state_to_home(&home, &initial).unwrap();

        let mut app = WeftDesktopApp::from_state_disconnected(home.clone(), "studio");
        populate_sample_project(&mut app.clients[0]);
        app.clients[0].update = Some("0.4.2".into());

        // Select running agent s1 so inline Ask button appears (D11.1 / D9.9)
        app.selection = Some(TreeSelection::Agent { project: "weft".into(), session: "s1".into() });

        let root = app.render();
        // Top bar checks (D2.6, D2.9, D9.9)
        assert!(root.find_layer("the list's toggle").is_some());
        // Turbo is on the agents' titles and Updates in ☰, never in the bar.
        assert!(root.find_layer("turbo pill").is_none());
        assert!(root.find_layer("updates pill").is_none());
        let needs_tag = root.find_layer("needs you tag").expect("needs you tag");
        assert_eq!(needs_tag.all_text(), "■ NEEDS YOU 1");

        // Tree checks (D11.1)
        let tree = root.find_layer("tree").expect("tree open");
        assert!(tree.find_layer("search").is_some());
        assert!(tree.find_layer("row weft").is_some());
        assert!(tree.find_layer("section AGENTS").is_some());
        assert!(tree.find_layer("row agent antigravity fix login").is_some());
        assert!(tree.find_layer("row earlier sessions").is_some());
        assert!(tree.find_layer("section WORK").is_some());
        assert!(tree.find_layer("row group Open").is_some());
        assert!(tree.find_layer("row ask fix login").is_some());
        assert!(tree.find_layer("row ask telemetry").is_some());
        assert!(tree.find_layer("row group Eval 3").is_some());
        assert!(tree.find_layer("row group Sealed").is_some());

        // Search filtering (D11.8)
        app.search.set_text("login");
        let searched = app.render();
        let stree = searched.find_layer("tree").unwrap();
        assert!(stree.all_text().contains("2 match"));
        assert!(stree.find_layer("row ask fix login").is_some());
        assert!(stree.find_layer("row ask telemetry").is_none());
        app.dispatch("clear_search");
        assert!(app.search.text().is_empty());

        // Collapsing the tree (D15.7)
        app.dispatch("toggle_sidebar");
        assert!(!app.state.settings.sidebar_open);
        let collapsed = app.render();
        assert!(collapsed.find_layer("tree").is_none());
        let toggle = collapsed.find_layer("the list's toggle").unwrap();
        assert_eq!(toggle.aria_label.as_deref(), Some("Show the list"));
        assert!(!load_state_from_home(&home).settings.sidebar_open);
    }

    #[test]
    fn opens_new_agent_from_one_click_plus_project_popover_d9_9() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut initial = WeftState::default();
        initial.open_projects = vec![PathBuf::from("/work/weft")];
        initial.settings.turbo = true;
        save_state_to_home(&home, &initial).unwrap();

        let mut app = WeftDesktopApp::from_state_disconnected(home.clone(), "studio");
        app.clients[0].harnesses = sample_harnesses();

        // Click `+` on `row weft`
        app.dispatch("toggle_plus:weft");
        assert_eq!(app.plus_popover_project.as_deref(), Some("weft"));
        let root = app.render();
        let pop = root.find_layer("plus popover").expect("plus popover shown");
        assert!(pop.all_text().contains("NEW AGENT IN WEFT"));
        assert!(pop.find_layer("plus harness antigravity").is_some());
        assert!(pop.find_layer("plus harness claude").is_some());
        assert!(pop.find_layer("plus harness codex").is_some());
        assert!(pop.find_layer("plus harness grok").is_some());

        // Click `Antigravity` in the `+` popover
        app.dispatch("start_agent:weft:antigravity");
        assert!(app.plus_popover_project.is_none(), "popover closes on start");
        assert_eq!(app.clients[0].panes.len(), 1);
        assert_eq!(app.clients[0].panes[0].harness, "antigravity");

        // Active pane group in durable state now has the new tab
        let saved = load_state_from_home(&home);
        assert_eq!(saved.panes.groups[0].tabs.len(), 1);
        assert_eq!(saved.panes.groups[0].tabs[0].pane, 0);
        assert_eq!(saved.panes.groups[0].tabs[0].project, PathBuf::from("/work/weft"));
    }

    #[test]
    fn splits_up_to_four_panes_and_restores_layout_across_restarts_for_four_harnesses() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut initial = WeftState::default();
        initial.open_projects = vec![PathBuf::from("/work/weft")];
        initial.settings.turbo = true;
        save_state_to_home(&home, &initial).unwrap();

        let mut app = WeftDesktopApp::from_state_disconnected(home.clone(), "studio");
        app.clients[0].harnesses = sample_harnesses();

        // Start all 4 harnesses in group 0: claude, codex, grok, antigravity
        for h in ["claude", "codex", "grok", "antigravity"] {
            app.dispatch(&format!("start_agent:weft:{h}"));
        }
        assert_eq!(app.state.panes.groups.len(), 1);
        assert_eq!(app.state.panes.groups[0].tabs.len(), 4);

        // Open layout menu on pane 0 and split right -> 2 panes (TwoColumns, D15.1/D15.2)
        app.dispatch("toggle_layout_menu:0");
        assert!(app.render().find_layer("layout menu").is_some());
        app.dispatch("split_right:0");
        assert_eq!(app.state.panes.groups.len(), 2);
        assert_eq!(app.state.panes.layout, SplitLayout::TwoColumns);

        // Split pane 0 down -> 3 panes (ThreeRight)
        app.dispatch("split_down:0");
        assert_eq!(app.state.panes.groups.len(), 3);
        assert_eq!(app.state.panes.layout, SplitLayout::ThreeRight);

        // Split pane 0 right -> 4 panes (FourGrid, D15.3)
        app.dispatch("split_right:0");
        assert_eq!(app.state.panes.groups.len(), 4);
        assert_eq!(app.state.panes.layout, SplitLayout::FourGrid);

        // Attempting a 5th split says why-not ("Four panes at most...")
        app.dispatch("split_right:0");
        assert_eq!(app.state.panes.groups.len(), 4);
        assert_eq!(
            app.bottom_bar,
            BottomBarState::WhyNot { sentence: words::word("web.why.four_panes").to_string() }
        );

        // Restarting weft-desktop from state.json restores all 4 panes and their tabs
        let restarted = WeftDesktopApp::from_state_disconnected(home.clone(), "studio");
        assert_eq!(restarted.state.panes.groups.len(), 4);
        assert_eq!(restarted.state.panes.layout, SplitLayout::FourGrid);

        // Join all panes collapses back into a single pane with all 4 tabs
        app.dispatch("join_all_panes");
        assert_eq!(app.state.panes.groups.len(), 1);
        assert_eq!(app.state.panes.groups[0].tabs.len(), 4);
        assert_eq!(app.state.panes.layout, SplitLayout::Single);
    }

    #[test]
    fn switches_terminal_and_conversation_views_and_renders_waiting_strip_d12_1_to_d12_4_and_d3_4()
    {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let proj = PathBuf::from("/work/weft");
        let mut initial = WeftState::default();
        initial.open_projects = vec![proj.clone()];
        save_state_to_home(&home, &initial).unwrap();

        let mut app = WeftDesktopApp::from_state_disconnected(home.clone(), "studio");
        app.clients[0].harnesses = sample_harnesses();

        for (idx, h) in ["claude", "codex", "grok", "antigravity"].iter().enumerate() {
            app.dispatch(&format!("start_agent:weft:{h}"));
            let pane_id = idx as u32;
            app.clients[0]
                .view_mut(pane_id)
                .unwrap()
                .feed(format!("terminal output from {h}\r\n").as_bytes());
            app.conversations.insert(
                (proj.clone(), pane_id),
                vec![
                    Item {
                        id: "0.0".into(),
                        kind: Kind::Person,
                        at: Some(1118),
                        text: Some(format!("fix login in {h}")),
                        tool: None,
                        arg: None,
                        call: None,
                        failed: None,
                    },
                    Item {
                        id: "10.0".into(),
                        kind: Kind::Agent,
                        at: Some(1120),
                        text: Some("I'll update src/login.rs and run tests.".into()),
                        tool: None,
                        arg: None,
                        call: None,
                        failed: None,
                    },
                    Item {
                        id: "20.0".into(),
                        kind: Kind::Step,
                        at: Some(1121),
                        text: None,
                        tool: Some("Read".into()),
                        arg: Some("src/login.rs".into()),
                        call: Some("c1".into()),
                        failed: Some(false),
                    },
                    Item {
                        id: "30.0".into(),
                        kind: Kind::Step,
                        at: Some(1122),
                        text: None,
                        tool: Some("Edit".into()),
                        arg: Some("src/login.rs".into()),
                        call: Some("c2".into()),
                        failed: Some(false),
                    },
                    Item {
                        id: "40.0".into(),
                        kind: Kind::Step,
                        at: Some(1123),
                        text: None,
                        tool: Some("Command".into()),
                        arg: Some("cargo test -p app-core".into()),
                        call: Some("c3".into()),
                        failed: Some(true),
                    },
                    Item {
                        id: "50.0".into(),
                        kind: Kind::Agent,
                        at: Some(1124),
                        text: Some("Should a failed refresh retry once?".into()),
                        tool: None,
                        arg: None,
                        call: None,
                        failed: None,
                    },
                ],
            );
        }

        // Antigravity (tab 3, pane 3) is active; put it in Turn::Waiting (D12.4)
        app.clients[0].view_mut(3).unwrap().turn = Some(Turn::Waiting);

        // Switch tab 3 to Conversation view (D12.4)
        app.dispatch("set_tab_view:0:3:conversation");
        let conv_dom = app.render();
        assert!(conv_dom.find_layer("seg Conversation selected").is_some());
        assert!(conv_dom.find_layer("pane conversation").is_some());
        assert!(conv_dom.find_layer("msg you").is_some());
        assert!(conv_dom.find_layer("msg antigravity").is_some());
        assert!(conv_dom.find_layer("tool steps folded").is_some());
        assert!(conv_dom.find_layer("msg antigravity 2").is_some());
        let waiting_line = conv_dom.find_layer("waiting line").expect("waiting line");
        assert!(waiting_line.all_text().contains("antigravity is waiting for you —"));
        assert!(waiting_line.all_text().contains("answer in Terminal ›"));

        // Unfold tool steps in place (D12.1 -> D12.2 / D12.4)
        app.dispatch("toggle_steps:/work/weft:3:0");
        let open_steps_dom = app.render();
        assert!(open_steps_dom.find_layer("tool steps open").is_some());
        assert!(open_steps_dom.find_layer("step Read").is_some());
        assert!(open_steps_dom.find_layer("step Edit").is_some());
        assert!(open_steps_dom.find_layer("step Command").is_some());

        // The waiting line's action switches to Terminal, where the keyboard
        // answers the agent: no key strip above the message row.
        app.dispatch("set_tab_view:0:3:terminal");
        let term_dom = app.render();
        assert!(term_dom.find_layer("seg Terminal selected").is_some());
        assert!(term_dom.find_layer("NEEDS YOU strip").is_none());

        // Clicking `⏎` sends `\r` to pane 3 via Call::Input
        app.dispatch("send_choice_key:0:enter");
        assert!(
            app.clients[0].sent_calls.contains(&Call::Input { pane: 3, bytes: b"\r".to_vec() })
        );

        // Local PTY Call::Resize ownership when no weft-tui is attached
        let root = app.clients[0].root.clone();
        app.resize_pane(&root, 3, 40, 120);
        assert!(app.clients[0].sent_calls.contains(&Call::Resize { pane: 3, rows: 40, cols: 120 }));
        // When weft-tui is attached, weft-desktop updates its viewport without sending Call::Resize
        app.clients[0].tui_attached = true;
        app.resize_pane(&root, 3, 50, 150);
        assert!(!app.clients[0].sent_calls.contains(&Call::Resize {
            pane: 3,
            rows: 50,
            cols: 150,
        }));
    }

    #[test]
    fn a_restored_split_keeps_each_agent_in_its_own_pane() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let root = PathBuf::from("/work/weft");
        let mut initial = WeftState::default();
        initial.open_projects = vec![root.clone()];
        let tab = |pane| SavedPaneTab { project: root.clone(), pane, view: PaneViewMode::Terminal };
        initial.panes.layout = weft_core::state::SplitLayout::TwoColumns;
        initial.panes.groups = vec![
            SavedTabGroup { active_tab: 0, tabs: vec![tab(0)] },
            SavedTabGroup { active_tab: 0, tabs: vec![tab(1)] },
        ];
        save_state_to_home(&home, &initial).unwrap();
        let mut app = WeftDesktopApp::from_state_disconnected(home, "studio");
        let mut client = ProjectClient::disconnected(root.clone(), "weft");
        client.panes = vec![
            PaneView::new(0, "sh".into(), "sh".into()),
            PaneView::new(1, "sh".into(), "sh".into()),
        ];
        app.integrate_connected_client(client);
        assert_eq!(
            app.state.panes.groups, initial.panes.groups,
            "nothing shown twice, nothing moved"
        );
    }

    #[test]
    fn four_panes_each_tell_the_daemon_their_own_size() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut initial = WeftState::default();
        initial.open_projects = vec![PathBuf::from("/work/weft")];
        save_state_to_home(&home, &initial).unwrap();
        let mut app = WeftDesktopApp::from_state_disconnected(home, "studio");
        populate_sample_project(&mut app.clients[0]);
        let root = app.clients[0].root.clone();
        let sizes = [(1, 22, 97), (2, 22, 69), (3, 17, 97), (4, 17, 69)];
        for (pane, rows, cols) in sizes {
            app.resize_pane(&root, pane, rows, cols);
            app.resize_pane(&root, pane, rows, cols);
        }
        let told: Vec<_> = app.clients[0]
            .sent_calls
            .iter()
            .filter_map(|c| match c {
                Call::Resize { pane, rows, cols } => Some((*pane, *rows, *cols)),
                _ => None,
            })
            .collect();
        assert_eq!(told, sizes, "one each, and never twice for the same size");
    }

    #[test]
    fn full_ask_eval_seal_lifecycle_and_d4_11_cancel_send_anyway_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut initial = WeftState::default();
        initial.open_projects = vec![PathBuf::from("/work/weft")];
        save_state_to_home(&home, &initial).unwrap();

        let mut app = WeftDesktopApp::from_state_disconnected(home, "studio");
        populate_sample_project(&mut app.clients[0]);
        app.dispatch("start_agent:weft:claude");

        // 1. Open Ask sheet for running claude session, enter draft, submit (D4.1 -> D4.5)
        app.dispatch("open_ask:weft:s3");
        assert!(app.render().find_layer("sheet Ask").is_some());
        app.set_ask_draft("refresh token 60s before expiry");
        app.dispatch("submit_ask_sheet");

        // Pane 0 is Ready -> shows D4.5 `Cancel / Send bar` with `btn Send`
        let ready_bar = app.render();
        assert!(ready_bar.find_layer("Cancel / Send bar").is_some());
        assert!(ready_bar.find_layer("btn Send").is_some());
        app.dispatch("resolve_staged:weft:pend_1:yes");
        assert!(app.clients[0].waiting.is_empty());

        // 2. D4.11: When the agent is working (e.g. post-Stop /goal continuation),
        // staging a prompt shows `The agent has not said it is ready` + `btn Send anyway`
        app.clients[0].view_mut(0).unwrap().turn = Some(Turn::Working);
        app.dispatch("open_ask:weft:s3");
        app.set_ask_draft("second ask while working");
        app.dispatch("submit_ask_sheet");

        let not_ready_dom = app.render();
        let bar = not_ready_dom.find_layer("Cancel / Send bar").expect("D4.11 bar");
        assert!(bar.all_text().contains("The agent has not said it is ready"));
        assert!(bar.all_text().contains("Weft did not type it: the agent is working."));
        assert!(bar.find_layer("btn Cancel").is_some());
        assert!(bar.find_layer("btn Send anyway").is_some());

        // Clicking `Send anyway` resolves with `force: true`
        app.dispatch("resolve_staged:weft:pend_1:force");
        assert!(app.clients[0].sent_calls.contains(&Call::Resolve {
            pending: "pend_1".into(),
            yes: true,
            force: true,
            to: None,
            start: None,
        }));

        // 3. Work Details (`D11.2`): Proceed & Follow up
        app.dispatch("select_ask:weft:ask_2");
        let work_dom = app.render();
        assert!(work_dom.find_layer("sheet WorkDetail").is_some());
        assert!(work_dom.find_layer("btn Proceed").is_some());
        assert!(work_dom.find_layer("btn Follow up").is_some());
        // Follow up covers the detail's acts: nothing is left to press twice.
        app.dispatch("open_follow_up:weft:ask_2");
        let following = app.render();
        assert!(following.find_layer("btn Follow up").is_none());
        assert!(following.find_layer("btn Close").is_none());
        app.dispatch("close_sheet");
        app.dispatch("select_ask:weft:ask_2");
        app.dispatch("act_proceed:weft:ask_2");
        assert!(app.clients[0].sent_calls.contains(&Call::ConfirmAsk { unit: "ask_2".into() }));

        // 4. Eval on Open round (`D4.8`)
        app.clients[0].view_mut(0).unwrap().turn = Some(Turn::Ready);
        app.dispatch("act_eval:weft");
        assert!(app.render().find_layer("Cancel / Send bar").is_some());
        app.dispatch("resolve_staged:weft:pend_1:yes");

        // 5. Eval Details (`D11.3`) & Eval view (`D6.1`–`D6.2`)
        app.dispatch("select_eval:weft:evl_3");
        let eval_dom = app.render();
        assert!(eval_dom.find_layer("sheet EvalDetail").is_some());
        assert!(eval_dom.find_layer("btn Eval view").is_some());
        app.dispatch("open_eval_view:weft:evl_3");
        let ev_view = app.render();
        assert!(ev_view.find_layer("sheet EvalView").is_some());
        assert!(ev_view.all_text().contains("By step"));
        app.dispatch("toggle_eval_view_mode:weft:evl_3");
        assert!(app.render().all_text().contains("By file"));

        // 6. Seal Eval 3 (`D5.1`–`D5.4`) and inspect Seal Details (`D11.4`)
        app.dispatch("open_seal:weft:evl_3");
        let seal_bar = app.render();
        assert!(seal_bar.find_layer("Seal disposition bar").is_some());
        app.dispatch("select_seal_disposition:weft:evl_3:accepted");
        app.dispatch("confirm_seal_disposition:weft:evl_3:accepted");
        assert!(app.clients[0].sent_calls.contains(&Call::Act {
            act: "seal".into(),
            unit: None,
            pane: Some(0),
            text: Some("accepted".into()),
            eval: Some("evl_3".into()),
            start: None,
        }));

        app.dispatch("toggle_sealed:weft");
        app.dispatch("select_seal:weft:sel_4");
        assert!(app.render().find_layer("sheet SealDetail").is_some());

        // 7. Ended session Details (`D11.5`) & Earlier sessions (`D11.6`)
        app.dispatch("select_agent:weft:s4");
        let sess_dom = app.render();
        assert!(sess_dom.find_layer("sheet SessionDetail").is_some());
        assert!(sess_dom.find_layer("btn Resume").is_some());
        app.dispatch("resume_session:weft:claude:s4");
        assert!(app.active_sheet.is_none());

        assert!(app.render().find_layer("row earlier sessions").is_some());
        app.dispatch("open_earlier:weft");
        assert!(app.unfolded_earlier.contains("weft"));
        assert!(app.active_sheet.is_none());
        let expanded_tree = app.render();
        assert!(
            expanded_tree.find_layer("row earlier sessions").is_none(),
            "earlier sessions row drops once expanded"
        );
        assert!(expanded_tree.find_layer("btn Keys").is_none());
        let search_box = expanded_tree.find_layer("search").expect("search box");
        assert!(search_box.styles.contains(&("height".into(), "38px".into())));
    }

    #[test]
    fn local_mac_ux_open_project_d7_2_updates_d8_3_remote_and_d9_9_menu_and_quit_bar() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut initial = WeftState::default();
        initial.open_projects = vec![PathBuf::from("/work/weft")];
        initial.settings.turbo = true;
        save_state_to_home(&home, &initial).unwrap();

        let mut app = WeftDesktopApp::from_state_disconnected(home.clone(), "studio");
        populate_sample_project(&mut app.clients[0]);
        app.dispatch("start_agent:weft:antigravity");

        // Configure mock native macOS NSOpenPanel file/folder dialogs
        app.file_dialog = Arc::new(MockFileDialog {
            folder: Mutex::new(Some(PathBuf::from("/work/new-repo"))),
            file: Mutex::new(Some(PathBuf::from("/Applications/Codex.app/Contents/MacOS/codex"))),
        });

        // Populate D7.2 setup view (Weft behind, Sense behind, Claude behind, Codex NotFound)
        app.clients[0].setup = Some(SetupView {
            weft: "0.4.3".into(),
            weft_latest: Some("0.4.4".into()),
            weft_behind: true,
            weft_waiting: false,
            latest: Some("0.4.4".into()),
            plugin: Some("0.4.4".into()),
            configuration: "current".into(),
            configuration_behind: false,
            sense: "fab7-sense-v1".into(),
            sense_latest: Some("fab7-sense-v2".into()),
            sense_behind: true,
            pending_release: false,
            rows: vec![
                SetupRow {
                    name: "antigravity".into(),
                    title: "Antigravity".into(),
                    program: "agy".into(),
                    state: SetupState::Ready { version: None },
                },
                SetupRow {
                    name: "claude".into(),
                    title: "Claude Code".into(),
                    program: "claude".into(),
                    state: SetupState::Behind { have: "0.4.1".into(), latest: "0.4.4".into() },
                },
                SetupRow {
                    name: "codex".into(),
                    title: "Codex".into(),
                    program: "codex".into(),
                    state: SetupState::NotFound,
                },
                SetupRow {
                    name: "grok".into(),
                    title: "Grok Build".into(),
                    program: "grok".into(),
                    state: SetupState::Ready { version: None },
                },
            ],
            note: None,
        });

        // Populate D8.3 remote view (2 connected devices)
        app.clients[0].remote = Some(serde_json::json!({
            "on": true,
            "state": "on",
            "devices": [
                {"id": "dev_iphone", "name": "iPhone", "online": true, "status": "Connected now"},
                {"id": "dev_chrome", "name": "Mac · Chrome", "online": false, "status": "Last seen 12m ago"}
            ]
        }));

        // 1. D9.9 `☰` Mac menu popover + click-outside backdrop
        app.dispatch("toggle_menu");
        let menu_dom = app.render();
        assert!(menu_dom.find_layer("popover backdrop").is_some());
        let menu = menu_dom.find_layer("menu popover").expect("menu popover");
        assert!(menu.find_layer("menu Open project").is_some());
        assert!(menu.find_layer("menu Turbo").is_some());
        assert!(menu.find_layer("menu Start at login").is_some());
        assert!(menu.find_layer("menu Remote").is_some());
        assert!(menu.find_layer("menu Updates").is_some());
        assert!(menu.find_layer("menu Keyboard shortcuts").is_none());
        assert!(menu.find_layer("menu Quit Weft").is_some());
        app.dispatch("close_popovers");
        assert!(!app.menu_open);

        // Clicking `Open project…` opens the native folder dialog and adds `/work/new-repo`
        app.dispatch("open_project_dialog");
        assert!(app.state.open_projects.contains(&PathBuf::from("/work/new-repo")));

        // 1-click toggles for Turbo and Start at login persist to state.json
        app.dispatch("toggle_start_at_login");
        assert!(load_state_from_home(&home).settings.start_at_login);

        // 2. D7.2 Updates sheet: Weft, RingFrame, Sense, and 4 agents + current versions + Find app… + Update all
        app.dispatch("open_updates");
        let upd_dom = app.render();
        let upd = upd_dom.find_layer("sheet Updates").expect("Updates sheet");
        assert!(upd.find_layer("update row Weft").is_some());
        assert!(upd.find_layer("update row RingFrame").is_some());
        assert!(upd.find_layer("update row Sense").is_some());
        assert!(upd.find_layer("update row Antigravity").is_some());
        assert!(upd.find_layer("update row Claude Code").is_some());
        assert!(upd.find_layer("update row Codex").is_some());
        assert!(upd.find_layer("update row Grok Build").is_some());
        assert!(upd.all_text().contains("v0.4.3 → v0.4.4"));
        assert!(upd.all_text().contains("v0.4.1 → v0.4.4"));
        assert!(upd.all_text().contains("v0.4.4 · Up to date"));
        assert!(upd.find_layer("btn Find app… codex").is_some());
        assert!(upd.find_layer("btn Update all").is_some());

        // Clicking `Find app…` opens native file picker and sends Call::Setup { act: "locate", ... }
        app.dispatch("locate_harness:codex");
        assert!(app.clients[0].sent_calls.contains(&Call::Setup {
            act: "locate".into(),
            harness: "codex".into(),
            path: "/Applications/Codex.app/Contents/MacOS/codex".into(),
        }));

        // Clicking `Update all` sends Call::Setup { act: "all", ... }
        app.dispatch("setup_update_all");
        assert!(app.clients[0].sent_calls.contains(&Call::Setup {
            act: "all".into(),
            harness: String::new(),
            path: String::new(),
        }));

        // 3. D8.3 Remote sheet + Remove device confirmation + keeping Remote On when 0 devices remain
        app.dispatch("open_remote");
        let rem_dom = app.render();
        let rem = rem_dom.find_layer("sheet Remote").expect("Remote sheet");
        assert!(rem.find_layer("device row iPhone").is_some());
        assert!(rem.find_layer("device row Mac · Chrome").is_some());
        assert!(rem.find_layer("btn Turn off").is_some());

        // Clicking Remove opens confirmation first
        app.dispatch("request_remove_device:dev_iphone:iPhone");
        assert_eq!(
            app.bottom_bar,
            BottomBarState::RemoveDeviceConfirm {
                device_id: "dev_iphone".into(),
                device_name: "iPhone".into(),
            }
        );
        let confirm_dom = app.render();
        assert!(confirm_dom.find_layer("remove device confirm card").is_some());
        assert!(confirm_dom.find_layer("Remove device bar").is_some());

        // Confirming removal of both devices keeps Remote ON (`● On`)
        app.dispatch("remote_remove_device:dev_iphone");
        app.dispatch("remote_remove_device:dev_chrome");
        assert_eq!(app.remote_devices_count(), 0);
        assert!(app.is_remote_on());
        app.dispatch("toggle_menu");
        let menu_after = app.render();
        assert!(menu_after.find_layer("menu Remote").unwrap().all_text().contains("● On"));
        app.dispatch("close_popovers");

        app.bottom_bar = BottomBarState::AllowDevice {
            device_id: "dev_ipad".into(),
            device_name: "iPad".into(),
        };
        let allow_dom = app.render();
        let allow_bar = allow_dom.find_layer("Allow a device bar").expect("Allow a device bar");
        assert!(allow_bar.all_text().contains("Allow \"iPad\" to connect to studio?"));
        assert!(allow_bar.find_layer("btn Refuse").is_some());
        assert!(allow_bar.find_layer("btn Allow").is_some());
        app.dispatch("remote_allow_device:dev_ipad:yes");
        assert_eq!(app.bottom_bar, BottomBarState::None);

        // 4. D9.9 `Quit Weft?` bar when agents are running
        app.dispatch("request_quit");
        assert_eq!(app.bottom_bar, BottomBarState::QuitConfirm);
        let quit_dom = app.render();
        let quit_bar = quit_dom.find_layer("Quit Weft bar").expect("Quit Weft bar");
        assert!(quit_bar.all_text().contains("Quit Weft?"));
        assert!(quit_bar.find_layer("btn Stop agents and quit").is_some());
        assert!(quit_bar.find_layer("btn Leave running and quit").is_some());
        app.dispatch("quit_leave_running");
        assert!(app.quit_requested);
    }

    /// Every element of a tree, depth first.
    fn every(el: &crate::gpui::Element) -> Vec<&crate::gpui::Element> {
        let mut out = vec![el];
        for c in &el.children {
            out.extend(every(c));
        }
        out
    }

    pub(crate) fn sample_app(turbo: bool) -> (tempfile::TempDir, WeftDesktopApp) {
        let tmp = tempfile::tempdir().unwrap();
        let initial = WeftState {
            open_projects: vec![PathBuf::from("/work/weft")],
            settings: weft_core::state::WeftSettings { turbo, ..Default::default() },
            ..Default::default()
        };
        save_state_to_home(tmp.path(), &initial).unwrap();
        let mut app = WeftDesktopApp::from_state_disconnected(tmp.path().to_path_buf(), "studio");
        app.clients[0].turbo = turbo;
        populate_sample_project(&mut app.clients[0]);
        (tmp, app)
    }

    fn style<'a>(el: &'a crate::gpui::Element, k: &str) -> Option<&'a str> {
        el.styles.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn the_tree_rows_are_one_line_with_the_status_shape() {
        let (_tmp, app) = sample_app(true);
        let dom = app.render();
        let tree = dom.find_layer("tree").expect("tree");
        assert_eq!(style(tree, "width"), Some("340px"));
        // Running is a circle, needs you a square, ended a hollow square.
        for (layer, shape) in [
            ("row agent antigravity fix login", "■"),
            ("row agent codex", "●"),
            ("row agent claude health endpoint", "●"),
            ("row agent claude readme fix", "□"),
        ] {
            let row = tree.find_layer(layer).unwrap_or_else(|| panic!("{layer}"));
            assert_eq!(style(row, "height"), Some("36px"), "{layer}");
            assert_eq!(row.children[0].all_text(), shape, "{layer}");
            let text = row.all_text();
            for word in ["working", "needs you", "ended", "No Ask yet"] {
                assert!(!text.contains(word), "{layer} says {word:?}: {text}");
            }
        }
        // Turbo shows on the running agent's title, never on an ended one.
        assert!(tree.find_layer("row agent codex").unwrap().all_text().contains('⚡'));
        assert!(!tree.find_layer("row agent claude readme fix").unwrap().all_text().contains('⚡'));
        // The sections carry their names only, and no row repeats the bar's acts.
        assert_eq!(tree.find_layer("section AGENTS").unwrap().all_text(), "AGENTS");
        assert_eq!(tree.find_layer("section WORK").unwrap().all_text(), "WORK");
        for act in ["btn Ask (row)", "btn Eval (row)", "btn Seal (row)"] {
            assert!(dom.find_layer(act).is_none(), "{act}");
        }
    }

    #[test]
    fn the_agent_in_front_is_suggested_the_rest_of_a_settled_draft_and_a_late_answer_is_dropped() {
        use std::time::{Duration, Instant};
        let (_tmp, mut app) = sample_app(false);
        app.state.panes.groups.clear();
        app.dispatch("start_agent:weft:claude");
        app.dispatch("start_agent:weft:codex");
        let t0 = Instant::now();
        let asked = |app: &WeftDesktopApp| {
            app.clients[0].sent_calls.iter().filter(|c| matches!(c, Call::Suggest { .. })).count()
        };
        app.message_field().type_text("run the");
        app.suggest_tick(t0);
        app.suggest_tick(t0 + Duration::from_millis(50));
        assert_eq!(asked(&app), 0, "not before the draft settles");
        app.suggest_tick(t0 + Duration::from_millis(120));
        assert_eq!(asked(&app), 1);
        app.suggest_tick(t0 + Duration::from_millis(300));
        assert_eq!(asked(&app), 1, "once per draft");
        let Some(Call::Suggest { draft, .. }) = app.clients[0].sent_calls.last().cloned() else {
            panic!()
        };
        assert_eq!(draft, "run the");
        let id = app.suggesting.as_ref().unwrap().id;
        app.clients[0].give_answer(id, serde_json::json!({"text": " login tests", "score": 0.8}));
        app.suggest_tick(t0 + Duration::from_millis(320));
        assert_eq!(app.ghost(), Some(" login tests"));
        let dom = app.render();
        assert!(dom.find_layer("the message row").unwrap().find_layer("take suggestion").is_some());
        // Only the agent in front shows one: another agent's field shows none.
        app.dispatch("select_tab:0:0");
        assert_eq!(app.ghost(), None);
        app.dispatch("select_tab:0:1");
        assert_eq!(app.ghost(), Some(" login tests"));
        // Tab takes it; typing on asks again, and an answer for the old draft is dropped.
        app.take_suggestion();
        assert_eq!(app.message_field().text(), "run the login tests");
        assert_eq!(app.ghost(), None);
        app.message_field().type_text(" now");
        app.suggest_tick(t0 + Duration::from_millis(400));
        app.suggest_tick(t0 + Duration::from_millis(520));
        let id = app.suggesting.as_ref().unwrap().id;
        app.message_field().type_text("!");
        app.clients[0].give_answer(id, serde_json::json!({"text": " please", "score": 0.9}));
        app.suggest_tick(t0 + Duration::from_millis(540));
        assert_eq!(app.ghost(), None, "an answer to a draft no longer there");
        // Esc drops one, and it is not asked for again until the draft changes.
        app.message_field().set_text("fix");
        app.suggest_tick(t0 + Duration::from_millis(600));
        app.suggest_tick(t0 + Duration::from_millis(720));
        let id = app.suggesting.as_ref().unwrap().id;
        app.clients[0].give_answer(id, serde_json::json!({"text": " the lint", "score": 0.9}));
        app.suggest_tick(t0 + Duration::from_millis(740));
        assert!(app.drop_suggestion());
        assert_eq!(app.ghost(), None);
        let before = asked(&app);
        app.suggest_tick(t0 + Duration::from_millis(900));
        assert_eq!(asked(&app), before);
        assert!(!app.drop_suggestion(), "none left to drop");
        // A malformed answer shows nothing.
        app.message_field().set_text("ok");
        app.suggest_tick(t0 + Duration::from_millis(1000));
        app.suggest_tick(t0 + Duration::from_millis(1120));
        let id = app.suggesting.as_ref().unwrap().id;
        app.clients[0].give_answer(id, serde_json::json!({"text": 7}));
        app.suggest_tick(t0 + Duration::from_millis(1140));
        assert_eq!(app.ghost(), None);
    }

    #[test]
    fn an_empty_field_asks_for_the_next_prompt_once_each_turn_has_ended() {
        use std::time::{Duration, Instant};
        let (_tmp, mut app) = sample_app(false);
        app.state.panes.groups.clear();
        app.dispatch("start_agent:weft:claude");
        let pane = app.active_tab().unwrap().pane;
        let asks = |app: &WeftDesktopApp| {
            app.clients[0]
                .sent_calls
                .iter()
                .filter(|c| matches!(c, Call::Suggest { draft, .. } if draft.is_empty()))
                .count()
        };
        let t0 = Instant::now();
        let settle = |app: &mut WeftDesktopApp, ms: u64| {
            app.suggest_tick(t0 + Duration::from_millis(ms));
            app.suggest_tick(t0 + Duration::from_millis(ms + 150));
        };
        app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Working);
        settle(&mut app, 0);
        assert_eq!(asks(&app), 0, "nothing to follow while it works");
        app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::TurnEnded);
        settle(&mut app, 300);
        assert_eq!(asks(&app), 1);
        settle(&mut app, 600);
        assert_eq!(asks(&app), 1, "once for this turn");
        let id = app.suggesting.as_ref().unwrap().id;
        app.clients[0]
            .give_answer(id, serde_json::json!({"text": "commit these changes", "score": 0.7}));
        app.suggest_tick(t0 + Duration::from_millis(900));
        assert_eq!(app.ghost(), Some("commit these changes"));
        // The next turn ends: asked again.
        app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Working);
        settle(&mut app, 1000);
        app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::TurnEnded);
        settle(&mut app, 1400);
        assert_eq!(asks(&app), 2);
    }

    #[test]
    fn a_tab_whose_agent_is_gone_leads_with_the_ended_shape() {
        let (_tmp, mut app) = sample_app(false);
        app.state.panes.groups.clear();
        let gone = SavedPaneTab {
            project: PathBuf::from("/work/weft"),
            pane: 99,
            view: Default::default(),
        };
        app.ensure_tab_in_active_pane(gone);
        let strip = app.render().find_layer("tab strip").expect("tab strip").clone();
        assert!(strip.all_text().starts_with('□'), "{}", strip.all_text());
    }

    #[test]
    fn each_agent_keeps_its_own_draft_and_switching_never_carries_one_across() {
        let (_tmp, mut app) = sample_app(false);
        app.state.panes.groups.clear();
        app.dispatch("start_agent:weft:claude");
        app.dispatch("start_agent:weft:codex");
        // codex is in front.
        app.message_field().type_text("fix the lint");
        app.dispatch("select_tab:0:0");
        assert_eq!(app.message_field().text(), "", "claude's own field");
        app.message_field().type_text("run the tests");
        app.dispatch("select_tab:0:1");
        assert_eq!(app.message_field().text(), "fix the lint");
        let dom = app.render();
        let row = dom.find_layer("the message row").expect("the message row");
        assert!(row.all_text().contains("fix the lint"));
        app.dispatch("send_message_row");
        assert_eq!(app.message_field().text(), "", "sent");
        app.dispatch("select_tab:0:0");
        assert_eq!(app.message_field().text(), "run the tests");
    }

    #[test]
    fn the_workspace_is_edge_to_edge_panes_or_the_blueprint_grid() {
        let (_tmp, mut app) = sample_app(false);
        app.state.panes.groups.clear();
        let empty = app.render();
        let grid = empty.find_layer("empty panes").expect("the empty workspace");
        assert_eq!(style(grid, "background"), Some(palette::TERMINAL_BG));
        assert!(grid.find_layer("hairline grid").is_some());
        assert!(grid.find_layer("woven mark").is_some());
        // An agent starts only from a project's +: the empty workspace says so and holds no button.
        assert_eq!(
            grid.find_layer("woven said").unwrap().all_text(),
            words::word("workspace.empty")
        );
        assert!(grid.find_layer("btn Start an agent").is_none());

        app.dispatch("start_agent:weft:claude");
        app.dispatch("start_agent:weft:codex");
        let one = app.render();
        let strip = one.find_layer("tab strip").expect("tab strip");
        assert_eq!(style(strip, "height"), Some("38px"));
        assert!(strip.find_layer("view switch").is_some(), "alone, the pane's own strip");
        assert!(one.find_layer("split toolbar").is_none());
        let pane = one.find_layer("pane").expect("the pane");
        assert_eq!(style(pane, "border"), None, "alone, no frame");
        assert_eq!(style(pane, "background"), Some(palette::TERMINAL_BG));

        app.dispatch("split_right:0");
        let two = app.render();
        let bar = two.find_layer("split toolbar").expect("one bar for the split");
        assert!(bar.all_text().contains("2 PANES"));
        assert!(bar.find_layer("view switch").is_some());
        let switches = every(&two)
            .into_iter()
            .filter(|e| e.layer_name.as_deref() == Some("view switch"))
            .count();
        assert_eq!(switches, 1, "the view switch once, not per pane");
        for g in 0..2 {
            let group = two.find_layer(&format!("pane group {g}")).unwrap();
            assert_eq!(style(group.find_layer("tab strip").unwrap(), "height"), Some("30px"));
        }
        let focused = two.find_layer("pane").expect("the focused pane");
        assert_eq!(style(focused, "border"), Some("2px solid #58a6ff"));

        // Ask docks under the panes, in the message row's place.
        app.dispatch("open_ask:weft:s1");
        assert!(matches!(app.active_sheet, Some(ActiveSheet::Ask { .. })));
        let asking = app.render();
        let workspace = asking.find_layer("workspace").expect("the panes stay");
        assert!(workspace.find_layer("sheet Ask").is_some(), "docked in the workspace");
        assert!(workspace.find_layer("pane").is_some());
        assert!(workspace.find_layer("the message row").is_none());
    }

    #[test]
    fn corners_are_the_guidelines_two_radii_and_every_button_is_flat() {
        let (_tmp, mut app) = sample_app(false);
        let mut screens = vec![app.render()];
        for action in [
            "toggle_menu",
            "close_popovers",
            "bar_act_ask",
            "close_sheet",
            "open_updates",
            "close_sheet",
            "open_remote",
            "close_sheet",
            "request_quit",
        ] {
            app.dispatch(action);
            screens.push(app.render());
        }
        let radii =
            [format!("{}px", palette::RADIUS_CONTROL), format!("{}px", palette::RADIUS_PANEL)];
        // A menu floats over the page as a panel.
        let menu = screens[1].find_layer("menu popover").expect("the menu");
        assert_eq!(style(menu, "border-radius"), Some(radii[1].as_str()));
        for screen in &screens {
            for el in every(screen) {
                // A boxed control takes the small radius.
                if el.styles.iter().any(|(k, v)| k == "border" && v.starts_with("1px")) {
                    assert!(
                        style(el, "border-radius").is_some(),
                        "{:?} is boxed but square",
                        el.layer_name
                    );
                }
                for (k, v) in &el.styles {
                    assert!(
                        k != "border-radius" || radii.contains(v),
                        "{:?} rounds by {v}",
                        el.layer_name
                    );
                    // Buttons are flat: no shadow and no bevel.
                    assert!(k != "box-shadow", "{:?} casts a shadow ({v})", el.layer_name);
                }
            }
        }
    }

    #[test]
    fn w_atlas_desktop_screens_match_toile_atlas_cards_with_zero_unapproved_differences() {
        use crate::gpui::palette;
        use weft_core::conversation::{Item, Kind};

        // Verify Toile atlas color palette tokens match exact hex specifications
        assert_eq!(palette::CANVAS, "#090d16");
        assert_eq!(palette::SURFACE, "#101726");
        assert_eq!(palette::SURFACE_ALT, "#0b111d");
        assert_eq!(palette::TERMINAL_BG, "#05080f");
        assert_eq!(palette::TITLEBAR_BG, "#151b28");
        assert_eq!(palette::LINE, "#253550");
        assert_eq!(palette::TEAL, "#39c5bb");
        assert_eq!(palette::BLUE, "#388bfd");
        assert_eq!(palette::AMBER, "#edbb5b");
        assert_eq!(palette::RED, "#f85149");
        assert_eq!(palette::TEXT, "#f1f3f7");
        assert_eq!(palette::MUTED, "#a2adbd");
        assert_eq!(palette::DIM, "#6e7b8f");

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut initial = WeftState::default();
        initial.open_projects = vec![PathBuf::from("/work/weft")];
        initial.settings.turbo = true;
        save_state_to_home(&home, &initial).unwrap();

        let mut app = WeftDesktopApp::from_state_disconnected(home, "studio");
        populate_sample_project(&mut app.clients[0]);
        app.clients[0].update = Some("0.4.2".into());
        app.dispatch("start_agent:weft:antigravity");
        app.clients[0].view_mut(0).unwrap().turn = Some(Turn::Waiting);
        app.conversations.insert(
            (PathBuf::from("/work/weft"), 0),
            vec![
                Item {
                    id: "10.0".into(),
                    kind: Kind::Person,
                    at: Some(1123),
                    text: Some("deploy staging".into()),
                    tool: None,
                    arg: None,
                    call: None,
                    failed: None,
                },
                Item {
                    id: "20.0".into(),
                    kind: Kind::Agent,
                    at: Some(1124),
                    text: Some("Allow bash tool to run deploy.sh?".into()),
                    tool: None,
                    arg: None,
                    call: None,
                    failed: None,
                },
            ],
        );

        // D12.4: Conversation view with Antigravity waiting for you + top bar 48px + tree 340px
        app.dispatch("set_tab_view:0:0:conversation");
        let d12_4 = app.render();
        assert!(d12_4.styles.contains(&("background".into(), palette::CANVAS.into())));
        let top_bar = d12_4.find_layer("header").expect("48px top bar");
        assert!(top_bar.styles.contains(&("height".into(), "48px".into())));
        let tree = d12_4.find_layer("tree").expect("340px tree");
        assert!(tree.styles.contains(&("width".into(), "340px".into())));
        assert!(d12_4.find_layer("seg Conversation selected").is_some());
        assert!(d12_4.find_layer("waiting line").is_some());
        assert!(d12_4.find_layer("the message row").is_some());

        // D7.2: Plain-English Updates sheet with zero internal paths, hashes, or model IDs
        app.dispatch("open_updates");
        let d7_2 = app.render();
        let upd_text = d7_2.find_layer("sheet Updates").unwrap().all_text();
        for forbidden in ["fab7-sense-v1", "sha256:", "/Users/", ".onnx", "weft-sense"] {
            assert!(
                !upd_text.contains(forbidden),
                "D7.2 Updates must use plain English without internal ID/path `{forbidden}`"
            );
        }
    }
}
