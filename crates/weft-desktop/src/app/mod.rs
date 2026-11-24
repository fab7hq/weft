//! The desktop's state: the open projects as the daemon last said them,
//! what the person has open and selected, durable `~/.fab7/weft/state.json`,
//! and the element tree drawn from it.

mod acts;
#[cfg(test)]
pub(crate) mod seal_tests;
#[cfg(test)]
pub(crate) mod tests;
// Named `tests.rs`, as every test file is, so it is read as tests.
#[cfg(test)]
#[path = "files/tests.rs"]
mod files_tests;
#[cfg(test)]
#[path = "progress/tests.rs"]
mod progress_tests;
#[cfg(test)]
#[path = "skip/tests.rs"]
mod skip_tests;
#[cfg(test)]
#[path = "waiting/tests.rs"]
mod waiting_tests;

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
use crate::daemon::{Answered, Doing, PaneView, ProjectClient, Then};
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

/// Something to tell the person through the system, while Weft is not where
/// they look: `tag` names the agent it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub tag: String,
    pub title: String,
}

/// How long a why-not is said before it goes by itself.
const WHY_NOT_FOR: std::time::Duration = std::time::Duration::from_secs(6);

/// The jobs of the machine, not of a project: Updates' and Remote's.
const MACHINE_JOBS: [&str; 2] = ["update", "remote"];
/// How often a shown conversation reads what its file added.
pub(crate) const FOLLOW_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// How long the Ask's words rest before they are read: Sense waits for the
/// person to stop typing.
pub(crate) const ASK_PAUSE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Right-pane sheet overlay or modal view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveSheet {
    /// Settings, on one of its tabs.
    Settings {
        tab: crate::action::SettingsTab,
    },
    /// A project's own settings: what differs from every project's.
    ProjectSettings {
        project: PathBuf,
    },
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
    /// The Eval view: the item picked, then `lines` or `reads` when one of
    /// them is open.
    EvalView {
        project: PathBuf,
        eval: String,
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

/// Who reads an Eval, picked in its dialog. Picking another stages the
/// dialog again for it; nothing runs until the person presses Eval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvalPick {
    pub project: PathBuf,
    /// The work, by any of its Asks.
    pub unit: String,
    /// The harness picked; none, the one the Eval starts on.
    pub picked: Option<String>,
    /// What was picked for the dialog shown, should the daemon refuse a new pick.
    pub was: Option<String>,
    /// The Read by list is open.
    pub open: bool,
    /// The dialog shown for this pick, once the daemon has staged it.
    pub pending: Option<String>,
    /// The dialog a new pick replaces, until the new one is staged: then it
    /// is answered no, so the person is never left without one.
    pub replacing: Option<String>,
    /// The person answered the dialog a new pick replaces before the new
    /// one came: the new one is answered no as it comes.
    pub dropped: bool,
}

/// What the bar along the window's foot asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BottomBarState {
    None,
    WhyNot {
        sentence: String,
    },
    QuitConfirm,
    AllowDevice {
        device_id: String,
        device_name: String,
        login: Option<String>,
    },
    RemoveDeviceConfirm {
        device_id: String,
        device_name: String,
    },
    CloseProjectConfirm {
        project: PathBuf,
    },
    SealDisposition {
        project: PathBuf,
        eval: String,
        selected: String,
        reason: Option<String>,
        questions: SealQuestions,
    },
    /// Turning a project's Git note on: what a note holds, and Turn on.
    GitNoteConfirm {
        project: PathBuf,
    },
    /// Undo's one dialog: the commits from where the work started, newest
    /// first, and the one picked.
    Undo {
        project: PathBuf,
        unit: String,
        commits: Vec<weft_core::after_seal::Commit>,
        picked: usize,
    },
}

/// The questions an Eval left for its Seal, as far as they are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealQuestions {
    /// Being read: the Seal waits for them.
    Reading,
    /// None to answer: the decision, the reason, then Seal.
    Unasked,
    /// They could not be read, and why: the Seal goes as with none, and
    /// they can be read again.
    Unread(String),
    /// One at a time, each answered before the next, then the review.
    Asking(Box<SealAsking>),
}

/// Where the person is in the Seal's questions, and since when the one
/// shown has been shown: an answer's time is counted from there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealAsking {
    pub answering: weft_core::seal_questions::Answering,
    pub shown: std::time::Instant,
}

/// Top-level Weft Desktop GPUI application.
pub struct WeftDesktopApp {
    pub home: PathBuf,
    /// What the machine is called until the daemon says.
    pub machine: String,
    pub state: WeftState,
    pub clients: Vec<ProjectClient>,
    pub selection: Option<TreeSelection>,
    /// The tree's search field.
    pub search: TextField,
    /// A setting being typed in Settings, and where it goes: every
    /// project's, or a project's own.
    pub setting_field: TextField,
    pub(crate) setting_editing: Option<(Option<PathBuf>, Vec<String>)>,
    /// The setting whose choices are shown, by its dotted key; `add` for what
    /// a project can set for itself.
    pub(crate) choice: Option<String>,
    /// The project whose row's ⋯ is open.
    pub(crate) project_menu: Option<PathBuf>,
    /// The RingFrame preview's host (empty: the first harness), route and task.
    pub(crate) preview: [String; 3],
    /// The preview's text shown, not only what it keeps.
    pub(crate) preview_shown: bool,
    /// Workflow's "Each judge" is open.
    pub(crate) judges_shown: bool,
    /// The harness Settings › Harnesses shows (empty: the first).
    pub(crate) harness_shown: String,
    /// The practices whose every rule Settings › RingFrame shows, not the
    /// first few, by catalog.
    pub(crate) all_rules: HashSet<String>,
    pub plus_popover_project: Option<PathBuf>,
    pub menu_open: bool,
    pub layout_menu_pane: Option<usize>,
    /// The project whose Done list stands in the tree's place.
    pub done_of: Option<PathBuf>,
    pub unfolded_earlier: HashSet<PathBuf>,
    /// The Details a stage was opened from, the nearest last, each with its
    /// name: what `‹` goes back to.
    pub sheet_back: Vec<(ActiveSheet, String)>,
    /// When the bar's why-not was first shown.
    why_not_since: Option<std::time::Instant>,
    /// When the shown conversations were last read for what their files added.
    followed_at: Option<std::time::Instant>,
    pub conversations: HashMap<(PathBuf, u32), Vec<Item>>,
    /// Where each conversation held ends: the page before its first item,
    /// when there is one, and what the file adds after its last.
    conversation_cursors: HashMap<(PathBuf, u32), (Option<String>, String)>,
    pub unfolded_steps: HashSet<(PathBuf, u32, usize)>,
    /// Each agent's message field, by its project and pane: switching
    /// agents brings each one's draft back. The open Ask's field.
    pub messages: HashMap<(PathBuf, u32), TextField>,
    pub ask: TextField,
    /// What the open Ask's sheet offers, as the daemon last said it.
    pub ask_sheet: Option<serde_json::Value>,
    /// What the person picked of it.
    pub ask_choices: crate::sheets::ask::AskChoices,
    /// The words the shown sheet was made for: while they differ from the
    /// field's, the words are being read.
    pub ask_sheet_for: Option<String>,
    /// Why RingFrame would not read the open Ask's sheet, when it would not.
    pub ask_refused: Option<String>,
    /// The Ask's words as last seen, and when they last changed.
    ask_typed: Option<(String, std::time::Instant)>,
    /// How long the words rest before they are read.
    pub(crate) ask_pause: std::time::Duration,
    /// What Weft reads of the setup of the harness an Eval or a Seal goes
    /// to, said in its dialog: a warning, never a stop.
    pub act_warning: Option<String>,
    /// The person's own words for the Seal's question that takes them.
    pub seal_words: TextField,
    /// Where the keyboard goes.
    pub focus: Focus,
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
    /// The files open in the viewer, and what the windows are to do for them.
    pub files: crate::files::Files,
    /// Where each staged send goes, when the person picked other than where
    /// it was staged.
    staged_to: HashMap<String, String>,
    /// Who reads the Eval whose dialog is open, as picked there.
    pub(crate) eval_pick: Option<EvalPick>,
    /// Updates opened by itself once: it does not again.
    updates_offered: bool,
    /// The device last answered: the bar does not ask again while
    /// `weft remote` has not yet heard.
    allow_answered: Option<String>,
    /// What is to be told and has not been taken yet.
    notices: Vec<Notice>,
    /// Why each project last failed to open, as told.
    not_opened: HashMap<PathBuf, String>,
    /// Weft's own link to the daemon, made while no project is listed:
    /// what is installed is the machine's, and Updates shows it.
    pub(crate) own_link: Option<ProjectClient>,
    /// The link was made once: a daemon that went is not started again.
    own_link_made: bool,
    /// What the machine would not do, said where it was asked until the
    /// person acts again.
    pub failed: Option<String>,
    /// When the window was last drawn again for the time something has
    /// been working.
    ticked_at: Option<std::time::Instant>,
    /// Since when each harness's setup step has been running.
    pub(crate) running_since: HashMap<String, std::time::Instant>,
    /// An act for `weft remote` asked, and how many times it had said where
    /// it is then: until it says again, the sheet says it is asked.
    pub(crate) remote_asked: Option<u64>,
    /// An Eval this window said yes to, by its project and work: its view
    /// opens by itself once its readers start.
    eval_view_wanted: Option<(PathBuf, String)>,
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
            setting_field: TextField::new(Shape::Line),
            seal_words: TextField::new(Shape::Line),
            setting_editing: None,
            choice: None,
            project_menu: None,
            // No host picked yet: the first harness Weft has.
            preview: [String::new(), "native_plan".into(), "implement".into()],
            preview_shown: false,
            judges_shown: false,
            harness_shown: String::new(),
            all_rules: HashSet::new(),
            plus_popover_project: None,
            menu_open: false,
            layout_menu_pane: None,
            done_of: None,
            unfolded_earlier: HashSet::new(),
            sheet_back: Vec::new(),
            why_not_since: None,
            followed_at: None,
            conversations: HashMap::new(),
            conversation_cursors: HashMap::new(),
            unfolded_steps: HashSet::new(),
            messages: HashMap::new(),
            ask: TextField::new(Shape::Area),
            ask_sheet: None,
            ask_choices: Default::default(),
            ask_sheet_for: None,
            ask_refused: None,
            ask_typed: None,
            ask_pause: ASK_PAUSE,
            act_warning: None,
            focus: Focus::Terminal,
            active_sheet: None,
            bottom_bar: BottomBarState::None,
            quit_requested: false,
            unsaved: false,
            waiting: HashSet::new(),
            remote_pick: None,
            updates_offered: false,
            send_to_open: false,
            files: Default::default(),
            staged_to: HashMap::new(),
            eval_pick: None,
            before_allow: None,
            allow_answered: None,
            notices: Vec::new(),
            not_opened: HashMap::new(),
            own_link: None,
            own_link_made: false,
            failed: None,
            ticked_at: None,
            running_since: HashMap::new(),
            remote_asked: None,
            eval_view_wanted: None,
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

    /// What Weft and the person's devices call this machine, as the daemon
    /// last said: the name set in Settings, else the system's own.
    pub fn machine_name(&self) -> String {
        let said =
            self.clients.iter().find_map(|c| c.config.as_ref()?["effective"]["name"].as_str());
        let hello =
            || self.clients.iter().chain(&self.own_link).find_map(|c| c.machine_name.as_deref());
        said.or_else(hello).unwrap_or(&self.machine).to_string()
    }

    /// The machine's turbo, as the daemon last said.
    pub fn turbo(&self) -> bool {
        self.clients.iter().find(|c| c.config.is_some()).is_some_and(|c| c.machine_turbo())
    }

    /// A machine switch that is on until turned off (`notify`), as
    /// the daemon last said.
    pub(crate) fn machine_setting(&self, key: &str) -> bool {
        self.clients
            .iter()
            .find_map(|c| c.config.as_ref())
            .is_none_or(|c| c["machine"][key].as_bool() != Some(false))
    }

    /// One of the machine's settings turned, through the first daemon that
    /// answers: it tells every project.
    pub(crate) fn turn(&mut self, key: &str, on: bool) {
        self.set_setting(None, &[key.to_string()], Some(serde_json::json!(on)));
    }

    /// A setting set, or taken out: every project's through the first daemon
    /// that answers, or `project`'s own through its. A RingFrame rule's field
    /// (`ringframe.deltas.<catalog>.entries.<id>[.<field>]`) and a line of
    /// what judges must not do (`ringframe.eval.deny.<n | +>`) are written as
    /// the list that holds them.
    pub(crate) fn set_setting(
        &mut self,
        project: Option<&Path>,
        key: &[String],
        value: Option<serde_json::Value>,
    ) {
        let (key, value) = self.as_written(project, key, value);
        let (scope, client) = match project {
            Some(root) => ("project", self.clients.iter_mut().find(|c| c.root == root)),
            None => ("machine", self.clients.iter_mut().find(|c| c.is_connected())),
        };
        if let Some(c) = client
            && let Ok(id) = c.set_config(scope, &key, value)
        {
            let saving =
                crate::daemon::Working { doing: Doing::Saving, about: String::new(), work: None };
            c.owed_for(id, Then::Working(saving));
        }
    }

    /// A key into a list written as the whole list, as the daemon last said it.
    fn as_written(
        &self,
        project: Option<&Path>,
        key: &[String],
        value: Option<serde_json::Value>,
    ) -> (Vec<String>, Option<serde_json::Value>) {
        use serde_json::{Value, json};
        let k: Vec<&str> = key.iter().map(String::as_str).collect();
        match k[..] {
            ["ringframe", "deltas", catalog, "entries", id, ref field @ ..] => {
                let list_key: Vec<String> =
                    ["ringframe", "deltas", catalog, "entries"].map(String::from).to_vec();
                let mut list =
                    self.setting_now(project, &list_key).as_array().cloned().unwrap_or_default();
                let at = list.iter().position(|e| e["id"] == id);
                match (field.first(), value) {
                    (None, None) => {
                        if let Some(i) = at {
                            list.remove(i);
                        }
                    }
                    // Several fields at once: the entry's own, with these on top.
                    (None, Some(Value::Object(fields))) => {
                        let i = at.unwrap_or_else(|| {
                            list.push(json!({"id": id}));
                            list.len() - 1
                        });
                        if let Some(entry) = list[i].as_object_mut() {
                            entry.extend(fields);
                        }
                    }
                    (None, Some(_)) => {}
                    (Some(f), v) => {
                        let i = at.unwrap_or_else(|| {
                            list.push(json!({"id": id}));
                            list.len() - 1
                        });
                        let entry = list[i].as_object_mut().expect("an entry is a table");
                        match v {
                            Some(v) => {
                                entry.insert(f.to_string(), v);
                            }
                            None => {
                                entry.remove(*f);
                            }
                        }
                        // A rule of the person's own: what RingFrame needs of one.
                        if id.starts_with("own.") {
                            entry.entry("applies_to").or_insert(json!(["implement"]));
                            entry.entry("tier").or_insert(json!("situational"));
                        }
                        if entry.len() == 1 {
                            list.remove(i);
                        }
                    }
                }
                (list_key, (!list.is_empty()).then_some(Value::Array(list)))
            }
            ["ringframe", "eval", "deny", which] => {
                let list_key: Vec<String> =
                    ["ringframe", "eval", "deny"].map(String::from).to_vec();
                // Nothing set here yet: the list that applies now, as RingFrame said it.
                let mut list =
                    self.setting_now(project, &list_key).as_array().cloned().unwrap_or_else(|| {
                        let then = crate::daemon::Then::Rules { project: project.is_some() };
                        self.clients
                            .iter()
                            .find_map(|c| c.read.get(&then))
                            .and_then(|r| r["judges"]["deny"].as_array().cloned())
                            .unwrap_or_default()
                    });
                match (which, value) {
                    ("+", Some(v)) => list.push(v),
                    (n, None) => {
                        if let Ok(i) = n.parse::<usize>()
                            && i < list.len()
                        {
                            list.remove(i);
                        }
                    }
                    _ => {}
                }
                // Emptied, the list denies nothing: it stays, as written.
                (list_key, Some(Value::Array(list)))
            }
            _ => (key.to_vec(), value),
        }
    }

    /// What a setting's field starts with: the setting as written, or a
    /// RingFrame rule's field as RingFrame merged it (a list with commas).
    pub(crate) fn field_now(&self, project: Option<&Path>, key: &[String]) -> String {
        let k: Vec<&str> = key.iter().map(String::as_str).collect();
        let v = match k[..] {
            ["ringframe", "deltas", catalog, "entries", id, field] => {
                let then = crate::daemon::Then::Rules { project: project.is_some() };
                self.clients
                    .iter()
                    .find_map(|c| c.read.get(&then))
                    .and_then(|r| r["catalogs"].as_array().cloned())
                    .into_iter()
                    .flatten()
                    .find(|c| c["catalog"] == catalog)
                    .and_then(|c| c["entries"].as_array().cloned())
                    .into_iter()
                    .flatten()
                    .find(|e| e["id"] == id)
                    .map_or(serde_json::Value::Null, |e| e[field].clone())
            }
            // The name starts as what the machine is called now.
            ["name"] if project.is_none() => serde_json::json!(self.machine_name()),
            // The days as they apply now, set or not.
            ["sessions", "keep_days"] => self
                .clients
                .iter()
                .find_map(|c| c.config.as_ref()?["effective"]["keep_days"].as_u64())
                .map_or(serde_json::Value::Null, |d| serde_json::json!(d.to_string())),
            _ => self.setting_now(project, key),
        };
        match v {
            serde_json::Value::String(s) => s,
            serde_json::Value::Array(items) => {
                items.iter().filter_map(|i| i.as_str()).collect::<Vec<_>>().join(", ")
            }
            _ => String::new(),
        }
    }

    /// What the preview asks for: `<host>|<route>|<task>`, the host the
    /// first harness Weft has until one is picked.
    /// Done's rows, when it stands in the tree's place: the project's sealed
    /// units, narrowed by the search.
    fn done_rows(&self) -> Option<(PathBuf, Vec<weft_core::done::Done>)> {
        let root = self.done_of.as_ref()?;
        let c = self.client(root)?;
        let q = self.search.text();
        let rows = weft_core::done::done(&c.work, &c.units)
            .into_iter()
            .filter(|d| weft_core::done::matches(d, q))
            .collect();
        Some((root.clone(), rows))
    }

    pub(crate) fn preview_unit(&self) -> String {
        let mut picked = self.preview.clone();
        if picked[0].is_empty() {
            picked[0] = self
                .clients
                .iter()
                .find_map(|c| c.harnesses.iter().next().map(|h| h.name.clone()))
                .unwrap_or_default();
        }
        picked.join("|")
    }

    /// The harness Settings › Harnesses shows: the one picked, or the first.
    pub(crate) fn harness_shown(&self) -> String {
        if !self.harness_shown.is_empty() {
            return self.harness_shown.clone();
        }
        self.clients
            .iter()
            .find_map(|c| c.harnesses.iter().next().map(|h| h.name.clone()))
            .unwrap_or_default()
    }

    /// RingFrame's rules, asked of the daemon while a sheet shows them; on
    /// Harnesses, the shown harness's profile too.
    pub(crate) fn want_rules(&mut self) {
        use crate::action::SettingsTab;
        if let Some(ActiveSheet::Settings { tab: SettingsTab::Harnesses }) = &self.active_sheet {
            let host = self.harness_shown();
            if let Some(c) = self.clients.iter_mut().find(|c| c.is_connected()) {
                let then = crate::daemon::Then::Profile { host: host.clone() };
                if !c.read.contains_key(&then) && !c.refused.contains_key(&then) && !host.is_empty()
                {
                    let _ = c.read("profile", &host, None, then);
                }
            }
        }
        let project = match &self.active_sheet {
            Some(ActiveSheet::Settings {
                tab: SettingsTab::RingFrame | SettingsTab::Harnesses,
            }) => None,
            Some(ActiveSheet::ProjectSettings { project }) => Some(project.clone()),
            _ => return,
        };
        let client = match &project {
            Some(root) => self.clients.iter_mut().find(|c| &c.root == root),
            None => self.clients.iter_mut().find(|c| c.is_connected()),
        };
        let Some(c) = client else { return };
        // A project's own settings say every project's beside them.
        let wanted = if project.is_some() { &[true, false][..] } else { &[false][..] };
        for &mine in wanted {
            let then = crate::daemon::Then::Rules { project: mine };
            if !c.read.contains_key(&then) && !c.refused.contains_key(&then) {
                let _ = c.read("rules", if mine { "project" } else { "machine" }, None, then);
            }
        }
        // On RingFrame, what the preview keeps, said beside Show.
        if let Some(ActiveSheet::Settings { tab: SettingsTab::RingFrame }) = &self.active_sheet {
            let unit = self.preview_unit();
            if let Some(c) = self.clients.iter_mut().find(|c| c.is_connected()) {
                let then = crate::daemon::Then::Preview { unit: unit.clone() };
                if !c.read.contains_key(&then) && !c.refused.contains_key(&then) {
                    let _ = c.read("preview", &unit, None, then);
                }
            }
        }
    }

    /// A setting as the daemon last said it: every project's, or `project`'s
    /// own; null when it says nothing.
    pub(crate) fn setting_now(&self, project: Option<&Path>, key: &[String]) -> serde_json::Value {
        let config = match project {
            Some(root) => {
                self.clients.iter().find(|c| c.root == root).and_then(|c| c.config.as_ref())
            }
            None => self.clients.iter().find_map(|c| c.config.as_ref()),
        };
        let table = if project.is_some() { "project" } else { "machine" };
        let mut v = config.map_or(&serde_json::Value::Null, |c| &c[table]);
        for k in key {
            v = &v[k.as_str()];
        }
        v.clone()
    }

    /// The Seal's words field, while its question shown takes words.
    pub fn seal_words_mut(&mut self) -> Option<&mut TextField> {
        self.seal_asking()
            .and_then(|a| a.answering.shown())
            .is_some_and(|q| q.answers().contains(&"words"))
            .then_some(&mut self.seal_words)
    }

    /// The Seal's questions, while it asks them.
    pub fn seal_asking(&self) -> Option<&SealAsking> {
        match &self.bottom_bar {
            BottomBarState::SealDisposition { questions: SealQuestions::Asking(a), .. } => Some(a),
            _ => None,
        }
    }

    /// The setting field, while one is being typed.
    pub fn setting_field_mut(&mut self) -> Option<&mut TextField> {
        self.setting_editing.is_some().then_some(&mut self.setting_field)
    }

    /// Put a connected project in place of the one it connects, and open a
    /// tab for each running agent no pane shows yet; a judge shows under its
    /// Eval.
    pub fn integrate_connected_client(&mut self, mut client: ProjectClient) {
        let p = weft_core::state::normalize_project_path(&client.root);
        client.root = p.clone();
        // Closed while it connected: it is not shown again.
        let Some(pos) = self.clients.iter().position(|c| c.root == p) else {
            client.detach();
            return;
        };
        self.not_opened.remove(&p);
        let running: Vec<u32> =
            client.panes.iter().filter(|v| v.running && v.judge.is_none()).map(|v| v.id).collect();
        self.clients[pos] = client;
        let here = |t: &SavedPaneTab| weft_core::state::normalize_project_path(&t.project) == p;
        // A hidden agent that has stopped is forgotten.
        self.state.panes.hidden.retain(|t| !here(t) || running.contains(&t.pane));
        // An agent already in some pane stays where it is: one agent in two
        // panes would be sized by both. One whose tab was closed stays hidden.
        for pane in running {
            let shown = self
                .state
                .panes
                .groups
                .iter()
                .any(|g| g.tabs.iter().any(|t| here(t) && t.pane == pane));
            let hidden = self.state.panes.hidden.iter().any(|t| here(t) && t.pane == pane);
            if !shown && !hidden {
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

    /// Whether Weft's own link is wanted: no project is listed, and none is
    /// made; and whether making it may start the daemon.
    pub fn wants_own_link(&self) -> Option<bool> {
        (self.clients.is_empty() && self.own_link.as_ref().is_none_or(|m| !m.is_connected()))
            .then_some(!self.own_link_made)
    }

    /// Weft's own link is made: what is installed is asked at once.
    pub fn integrate_own_link(&mut self, mut client: ProjectClient) {
        let _ = client.setup("look", "", "");
        self.own_link = Some(client);
        self.own_link_made = true;
    }

    /// What is installed, as the daemon last said: through a project, or
    /// through Weft's own link when none is open.
    pub fn setup_view(&self) -> Option<&weft_core::onboarding::View> {
        self.clients
            .iter()
            .find_map(|c| c.setup.as_ref())
            .or_else(|| self.own_link.as_ref()?.setup.as_ref())
    }

    /// The links a setup act goes through: every project's, or Weft's own
    /// when no project is connected.
    pub(crate) fn setup_links(&mut self) -> Vec<&mut ProjectClient> {
        if self.clients.iter().any(ProjectClient::is_connected) {
            self.clients.iter_mut().collect()
        } else {
            self.own_link.iter_mut().collect()
        }
    }

    /// Take what every project's daemon link brought since; what the window
    /// redraws for it: all of it, or only the terminals that had output.
    pub fn pump(&mut self) -> crate::daemon::Redraw {
        self.want_ask_sheet_at(std::time::Instant::now());
        // The search reaches every earlier session through the daemon.
        let q = self.search.text().to_string();
        for c in &mut self.clients {
            c.search_earlier(&q);
        }
        let (mut changed, mut moved) = (false, false);
        if let Some(m) = &mut self.own_link {
            changed |= m.pump().changed;
        }
        let mut answers = Vec::new();
        let mut terminals = Vec::new();
        for c in &mut self.clients {
            let p = c.pump();
            changed |= p.changed;
            moved |= p.moved;
            terminals.extend(p.output.into_iter().map(|pane| (c.root.clone(), pane)));
            answers.extend(p.answered.into_iter().map(|a| (c.root.clone(), a)));
        }
        for (root, a) in answers {
            self.take_answer(&root, a);
        }
        // A message's refusal, with no dialog to say it, is said in the bar.
        let refused: Vec<String> =
            self.clients.iter_mut().filter_map(|c| c.typing_refused.take()).collect();
        for why in refused {
            self.fail(why);
            changed = true;
        }
        // A job of a work that failed, which no wait here shows: said there too.
        let failed: Vec<String> = self
            .clients
            .iter_mut()
            .chain(self.own_link.iter_mut())
            .flat_map(|c| std::mem::take(&mut c.jobs_failed))
            .collect();
        for why in failed {
            self.fail(why);
            changed = true;
        }
        // An agent's turn or record moved: what it said may have too, and
        // an Eval still being judged.
        if moved {
            self.notice_waiting();
            self.refresh_active_conversations();
            self.refresh_eval_view();
        }
        if changed {
            self.drop_answered_eval_pick();
            self.want_rules();
            self.offer_allow();
            self.offer_updates();
            self.drop_gone_tabs();
            self.note_running();
            self.open_wanted_eval_view();
            let remotes: u64 = self.clients.iter().map(|c| c.remotes).sum();
            if self.remote_asked.is_some_and(|was| was != remotes) {
                self.remote_asked = None;
            }
        }
        let now = std::time::Instant::now();
        self.follow_conversations(now);
        let ticked = self.tick(now);
        crate::daemon::Redraw { window: self.expire_why_not(now) || changed || ticked, terminals }
    }

    /// While something works, the window is drawn again each second, so
    /// the time it shows moves. Whether it is due now.
    fn tick(&mut self, now: std::time::Instant) -> bool {
        let working = self.working().is_some()
            || self.updating().is_some()
            || self.clients.iter().chain(&self.own_link).any(ProjectClient::jobs_run)
            || !self.running_since.is_empty()
            || self.remote_asked.is_some();
        if !working {
            self.ticked_at = None;
            return false;
        }
        if self.ticked_at.is_some_and(|at| now - at < std::time::Duration::from_secs(1)) {
            return false;
        }
        self.ticked_at = Some(now);
        true
    }

    /// What the machine would not do, said in the bar until the person acts.
    pub(crate) fn fail(&mut self, sentence: impl Into<String>) {
        self.failed = Some(sentence.into());
        if matches!(self.bottom_bar, BottomBarState::WhyNot { .. }) {
            self.bottom_bar = BottomBarState::None;
        }
    }

    /// Whether the bar shows the tree's acts, and so what failed: no dialog
    /// is in front.
    pub(crate) fn bar_is_free(&self) -> bool {
        matches!(self.bottom_bar, BottomBarState::None | BottomBarState::WhyNot { .. })
            && self.clients.iter().all(|c| c.waiting.is_empty())
    }

    /// What Updates is doing while the daemon still owes its answer, and
    /// since when: the row it runs on (RingFrame, Sense, a harness whose
    /// program is checked), or `all`. Update all runs RingFrame, then Sense,
    /// then the harnesses: the first of them still behind is the one running.
    pub(crate) fn updating(&self) -> Option<(String, std::time::Instant)> {
        // The machine says which update runs, wherever it was asked.
        let job = self.machine_job(|j| j.kind == "update");
        if let Some((p, target)) = job.and_then(|p| Some((p, p.job.on.harness.clone()?))) {
            return Some((target, p.since));
        }
        let (act, target, since) =
            self.clients.iter().chain(&self.own_link).flat_map(|c| c.owed()).find_map(
                |(then, since)| match then {
                    Then::Setup { act, target } if act != "look" => {
                        Some((act.clone(), target.clone(), since))
                    }
                    _ => None,
                },
            )?;
        if act != "all" {
            return Some((target, since));
        }
        let v = self.setup_view();
        let first = match v {
            Some(v) if v.configuration_behind => "ringframe",
            Some(v) if v.sense_behind => "sense",
            _ => "all",
        };
        Some((first.to_string(), since))
    }

    /// A job of the machine's that runs, as any link was told it.
    pub(crate) fn machine_job(
        &self,
        pick: impl Fn(&weft_proto::Job) -> bool,
    ) -> Option<&crate::waiting::Progress> {
        self.clients.iter().chain(&self.own_link).find_map(|c| c.job(&pick))
    }

    /// The machine's jobs, running or failed, each once: Updates' and
    /// Remote's.
    pub(crate) fn machine_jobs(&self) -> Vec<&crate::waiting::Progress> {
        let mut seen = HashSet::new();
        self.clients
            .iter()
            .chain(&self.own_link)
            .flat_map(|c| &c.jobs)
            .filter(|p| MACHINE_JOBS.contains(&p.job.kind.as_str()))
            .filter(|p| seen.insert(p.job.id.clone()))
            .collect()
    }

    /// The person acted again on what failed of the machine's: its failure goes.
    pub(crate) fn forget_machine_failed(&mut self, pick: impl Fn(&weft_proto::Job) -> bool) {
        for c in self.clients.iter_mut().chain(self.own_link.iter_mut()) {
            c.forget_failed(&pick);
        }
    }

    /// Since when each harness's step has been running, as the daemon says.
    fn note_running(&mut self) {
        let running: Vec<String> = self
            .setup_view()
            .map(|v| {
                v.rows
                    .iter()
                    .filter(|r| matches!(r.state, weft_core::onboarding::State::Running(_)))
                    .map(|r| r.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let now = std::time::Instant::now();
        self.running_since.retain(|name, _| running.contains(name));
        for name in running {
            self.running_since.entry(name).or_insert(now);
        }
    }

    /// The Eval this window said yes to opens by itself once its readers
    /// start, unless the person has gone to something else.
    fn open_wanted_eval_view(&mut self) {
        let Some((project, root)) = self.eval_view_wanted.clone() else { return };
        let free = match &self.active_sheet {
            None => true,
            Some(ActiveSheet::Unit { project: p, unit }) => *p == project && *unit == root,
            _ => false,
        };
        if !free {
            self.eval_view_wanted = None;
            return;
        }
        let running = self.client(&project).and_then(|c| {
            let eval = c.work.of_ask(&root)?.evals.last()?.clone();
            let opened = c.work.record.evals.iter().find(|e| e.id == eval)?;
            (!opened.completed()).then_some(eval)
        });
        if let Some(eval) = running {
            self.eval_view_wanted = None;
            if let Some(c) = self.client_mut(&project) {
                let _ = c.read("eval", &eval, None, Then::EvalView { eval: eval.clone() });
            }
            self.active_sheet = Some(ActiveSheet::EvalView { project, eval, at: Vec::new() });
        }
    }

    /// The work `unit` names in `root`, as its row says it while its Eval
    /// opens: its title, its Asks and who reads it.
    pub(crate) fn opening_about(&self, root: &Path, unit: &str, harness: Option<&str>) -> String {
        let Some(c) = self.client(root) else { return String::new() };
        let Some(w) = c.work.of_ask(unit) else { return String::new() };
        let title = c.units.iter().find(|u| u.ask_id == w.root).map(|u| u.title.clone());
        let asks = match w.asks.len() {
            1 => words::word("unit.one_ask").to_string(),
            n => words::fill("unit.asks", &[("n", &n.to_string())]),
        };
        let harness = harness
            .map(str::to_string)
            .or_else(|| self.eval_reader(root, &w.root).map(|(h, _)| h))
            .unwrap_or_default();
        let harness =
            c.harnesses.0.iter().find(|h| h.name == harness).map_or(harness, |h| h.title.clone());
        words::fill(
            "wait.opening_eval_about",
            &[("title", &title.unwrap_or_default()), ("asks", &asks), ("harness", &harness)],
        )
    }

    /// A work's title, by any of its Asks.
    pub(crate) fn work_title(&self, root: &Path, unit: &str) -> String {
        self.client(root)
            .and_then(|c| {
                let w = c.work.of_ask(unit)?;
                c.units.iter().find(|u| u.ask_id == w.root).map(|u| u.title.clone())
            })
            .unwrap_or_default()
    }

    /// What the bar says while the machine works on a yes: the newest of
    /// what is still to come, an Eval opening anywhere among them.
    pub fn working(&self) -> Option<crate::waiting::Working> {
        use crate::waiting::Working;
        let mut all = Vec::new();
        for c in &self.clients {
            let mut shown = HashSet::new();
            for (id, then, since) in c.owed_calls() {
                let (said, about, opening) = match then {
                    Then::Working(w) => {
                        (w.doing.said().to_string(), w.about.clone(), w.doing == Doing::Opening)
                    }
                    Then::Started { harness, session: None } => (
                        words::fill("wait.starting", &[("harness", harness)]),
                        String::new(),
                        false,
                    ),
                    Then::Started { harness, session: Some(_) } => {
                        (words::word("wait.resuming").to_string(), harness.clone(), false)
                    }
                    Then::Eval => {
                        let about = self
                            .eval_pick
                            .as_ref()
                            .filter(|p| p.project == c.root)
                            .map(|p| self.work_title(&c.root, &p.unit))
                            .unwrap_or_default();
                        (words::word("wait.preparing").to_string(), about, false)
                    }
                    Then::Remedy { unit } | Then::Commits { unit } => (
                        words::word("wait.preparing").to_string(),
                        self.work_title(&c.root, unit),
                        false,
                    ),
                    _ => continue,
                };
                // Its job: the one its call began, else the one on its work.
                let progress = c
                    .job_of_call(id)
                    .or_else(|| {
                        let Then::Working(w) = then else { return None };
                        let (kind, root) = (w.doing.job()?, w.work.as_deref()?);
                        c.job(|j| j.kind == kind && j.on.work.as_deref() == Some(root))
                    })
                    .cloned();
                if let Some(p) = &progress {
                    shown.insert(p.job.id.clone());
                }
                all.push(Working { said, about, since, opening, progress });
            }
            // An Eval opened from another window or device shows here too.
            let elsewhere = c
                .jobs
                .iter()
                .filter(|p| p.running() && p.job.kind == "eval_open" && !shown.contains(&p.job.id));
            for p in elsewhere {
                let Some(root) = p.job.on.work.as_deref() else { continue };
                all.push(Working {
                    said: Doing::Opening.said().to_string(),
                    about: self.opening_about(&c.root, root, p.job.on.harness.as_deref()),
                    since: p.since,
                    opening: true,
                    progress: Some(p.clone()),
                });
            }
        }
        all.into_iter().max_by_key(|w| w.since)
    }

    /// Once a second, read what each shown conversation's file added, one
    /// read at a time for each: a message reaches the file after the agent
    /// moves, so a read on the move alone can miss it.
    pub fn follow_conversations(&mut self, now: std::time::Instant) {
        if self.followed_at.is_some_and(|at| now - at < FOLLOW_EVERY) {
            return;
        }
        self.followed_at = Some(now);
        for (root, pane) in self.shown_conversations() {
            let owed = self.client(&root).is_none_or(|c| c.owes(&Then::Conversation { pane }));
            // Read whole once: until then, its first read is still to come.
            if !owed && self.conversation_cursors.contains_key(&(root.clone(), pane)) {
                self.read_conversation(&root, pane);
            }
        }
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
        self.drop_gone_panes();
    }

    /// What is kept for an agent, its conversation and draft,
    /// goes with it: once its project's daemon has said it has no such pane,
    /// or the project is closed. So does where a send no longer staged goes.
    pub(crate) fn drop_gone_panes(&mut self) {
        let clients = &self.clients;
        let here = |(root, pane): &(PathBuf, u32)| {
            clients.iter().any(|c| &c.root == root && (!c.opened || c.view(*pane).is_some()))
        };
        self.conversations.retain(|k, _| here(k));
        self.conversation_cursors.retain(|k, _| here(k));
        self.messages.retain(|k, _| here(k));
        let staged = |id: &String| {
            clients.iter().any(|c| !c.opened || c.waiting.iter().any(|w| &w.id == id))
        };
        self.staged_to.retain(|id, _| staged(id));
    }

    /// The Eval's dialog answered anywhere: its pick goes with it. Answered
    /// while a new pick is staged, the new dialog goes too as it comes.
    fn drop_answered_eval_pick(&mut self) {
        let Some(p) = &self.eval_pick else { return };
        let waiting = |id: &String| {
            self.client(&p.project).is_some_and(|c| c.waiting.iter().any(|w| &w.id == id))
        };
        match (&p.pending, &p.replacing) {
            (Some(id), _) if !waiting(id) => self.eval_pick = None,
            (None, Some(id)) if !waiting(id) => {
                if let Some(p) = self.eval_pick.as_mut() {
                    (p.replacing, p.dropped) = (None, true);
                }
            }
            _ => {}
        }
    }

    /// Nothing is set up and something could be: Updates opens by itself,
    /// once, when nothing else is open.
    fn offer_updates(&mut self) {
        let wants = self.setup_view().is_some_and(|v| v.wants_setting_up());
        if wants && !self.updates_offered && self.active_sheet.is_none() {
            self.updates_offered = true;
            self.active_sheet = Some(ActiveSheet::Updates);
        }
    }

    /// SEND TO for `project`, `picked` ticked: its running agents, then a new
    /// agent of each harness.
    /// Ask the daemon what the open Ask's sheet offers for its harness, words
    /// and route; keep its answer once it comes, and only the newest. One is
    /// asked at a time: the words are read for each, so the next waits. New
    /// words are read only once they have rested `ask_pause`, so Sense never
    /// runs while the person types.
    pub(crate) fn want_ask_sheet_at(&mut self, now: std::time::Instant) {
        let Some(ActiveSheet::Ask { project, send_to, .. }) = &self.active_sheet else { return };
        let words = self.ask.text();
        if self.ask_typed.as_ref().is_none_or(|(seen, _)| seen != words) {
            self.ask_typed = Some((words.to_string(), now));
        }
        let rested = self.ask_typed.as_ref().is_some_and(|(_, at)| now - *at >= self.ask_pause);
        let rested = rested || words.trim().is_empty();
        let (project, send_to) = (project.clone(), send_to.clone().unwrap_or_default());
        let Some(c) = self.clients.iter_mut().find(|c| c.root == project) else { return };
        let pane = crate::sheets::send_to::pane_in(&send_to);
        let harness = match (send_to.strip_prefix("start:"), pane) {
            (Some(h), _) => h.to_string(),
            (None, Some(pane)) => match c.view(pane) {
                Some(p) => p.harness.clone(),
                None => return,
            },
            (None, None) => return,
        };
        // Sense's heads first, shown at once; then the full sheet, its plan
        // and rules. An answer for other words is never shown.
        let query =
            |stage| crate::sheets::ask::query(&harness, self.ask.text(), &self.ask_choices, stage);
        let (heads, full) = (query("heads"), query("full"));
        let then = |query: &String| crate::daemon::Then::AskSheet { query: query.clone(), pane };
        let (heads_then, full_then) = (then(&heads), then(&full));
        let asking = |t: &crate::daemon::Then| matches!(t, crate::daemon::Then::AskSheet { .. });
        let shown = c.read.get(&full_then).or_else(|| c.read.get(&heads_then));
        if let Some(sheet) = shown {
            self.ask_sheet = Some(sheet.clone());
            self.ask_sheet_for = Some(self.ask.text().to_string());
            self.ask_refused = None;
            c.read.retain(|t, _| !asking(t) || *t == heads_then || *t == full_then);
        }
        let refused = c.refused.get(&full_then).or_else(|| c.refused.get(&heads_then));
        if let Some(why) = refused {
            (self.ask_sheet, self.ask_refused) = (None, Some(why.clone()));
        } else if !c.read.contains_key(&full_then) {
            // A read still running for other words is not waited for: its
            // answer is never shown.
            let next = match c.read.contains_key(&heads_then) {
                true => Some((full, full_then)),
                false => rested.then_some((heads, heads_then)),
            };
            if let Some((query, then)) = next {
                let _ = c.read("ask", &query, pane, then);
            }
        }
    }

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

    /// Who reads the work's Eval unless the person picks another, and
    /// whether a setting said so rather than the work's latest agent.
    pub(crate) fn eval_reader(&self, project: &Path, unit: &str) -> Option<(String, bool)> {
        let c = self.client(project)?;
        let w = c.work.of_ask(unit)?;
        let latest = w.route.as_deref().unwrap_or(&w.root);
        let ask = |id: &str| c.units.iter().find(|u| u.ask_id == id);
        let harness = ask(latest).or_else(|| ask(&w.root))?.harness.clone();
        let (stages, eval) = (c.routing.eval_stages.as_ref(), c.routing.get("eval"));
        Some(weft_core::eval_stages::reader(stages, eval, &harness))
    }

    /// The daemon staged the Eval's dialog for the pick, or would not. The
    /// dialog it replaces goes once the new one is there; refused, the one
    /// shown stays with its pick, and says why.
    fn eval_staged(&mut self, root: &Path, answer: std::result::Result<serde_json::Value, String>) {
        let Some(pick) = self.eval_pick.as_mut().filter(|p| p.project == root) else { return };
        match answer {
            Ok(v) => {
                let Some(id) = v["pending"].as_str().map(str::to_string) else { return };
                let mut no = pick.replacing.take();
                if pick.dropped {
                    no = Some(id);
                    self.eval_pick = None;
                } else {
                    pick.pending = Some(id);
                }
                if let (Some(old), Some(c)) = (no, self.client_mut(root)) {
                    let _ = c.resolve(&old, false, None, None);
                }
            }
            Err(why) if pick.replacing.is_some() => {
                pick.pending = pick.replacing.take();
                pick.picked = pick.was.take();
                self.act_warning = Some(why);
            }
            Err(why) => {
                self.eval_pick = None;
                self.fail(why);
            }
        }
    }

    /// The Eval dialog's Read by, when the dialog in front is the one picked
    /// for: every harness set up in the project, the one picked, and why
    /// the one it started on.
    pub(crate) fn read_by_view(&self) -> Option<crate::bottom_bar::ReadBy> {
        let pick = self.eval_pick.as_ref()?;
        let c = self.clients.iter().find(|c| !c.waiting.is_empty())?;
        let shown = &c.waiting.last()?.id;
        let ours = [&pick.pending, &pick.replacing].into_iter().flatten().any(|id| id == shown);
        if c.root != pick.project || !ours {
            return None;
        }
        let (default, set) = self.eval_reader(&pick.project, &pick.unit)?;
        let picked = pick.picked.clone().unwrap_or_else(|| default.clone());
        let mut harnesses: Vec<(String, String)> =
            c.harnesses.0.iter().map(|h| (h.name.clone(), h.title.clone())).collect();
        if !harnesses.iter().any(|(n, _)| *n == picked) {
            harnesses.insert(0, (picked.clone(), picked.clone()));
        }
        let why = if set { "dialog.eval.from_settings" } else { "dialog.eval.from_work" };
        let note = (picked == default).then_some(why);
        Some(crate::bottom_bar::ReadBy {
            harnesses,
            picked,
            note,
            open: pick.open,
            staging: pick.replacing.is_some(),
        })
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
        let was = self.clients.iter().map(|c| c.remotes).sum();
        if let Some(c) = self.clients.iter_mut().find(|c| c.is_connected())
            && c.remote_act(act).is_ok()
        {
            self.remote_asked = Some(was);
        }
    }

    /// What an answer was for, done now it has come.
    fn take_answer(&mut self, root: &Path, a: Answered) {
        let refused_by_ringframe = a.code == "refused";
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
            // A follow-up on rejected work starts from what its Eval found.
            (Then::Remedy { unit }, Ok(v)) => {
                self.open_follow_up(
                    root.to_path_buf(),
                    unit,
                    v["words"].as_str().unwrap_or_default(),
                );
            }
            // Undo opens on what it read: where the work started is picked.
            (Then::Commits { unit }, Ok(v)) => {
                let commits: Vec<weft_core::after_seal::Commit> =
                    serde_json::from_value(v["commits"].clone()).unwrap_or_default();
                let picked = commits.iter().position(|c| c.start).unwrap_or(commits.len());
                if picked < commits.len() {
                    let project = root.to_path_buf();
                    self.bottom_bar = BottomBarState::Undo { project, unit, commits, picked };
                } else {
                    self.fail(words::word("wait.no_start"));
                }
            }
            (Then::Remedy { .. } | Then::Commits { .. }, Err(why)) => self.fail(why),
            // The Seal asks what its Eval left, one at a time; with none, or
            // none that could be read, it goes as it always has.
            (Then::Questions { eval }, answer) => {
                if let BottomBarState::SealDisposition { project, eval: e, questions, .. } =
                    &mut self.bottom_bar
                    && project == root
                    && *e == eval
                    && *questions == SealQuestions::Reading
                {
                    let asked: Vec<weft_core::seal_questions::Question> = match answer {
                        Ok(v) => serde_json::from_value(v["questions"].clone()).unwrap_or_default(),
                        Err(why) => {
                            *questions = SealQuestions::Unread(why);
                            return;
                        }
                    };
                    *questions = if asked.is_empty() {
                        SealQuestions::Unasked
                    } else {
                        SealQuestions::Asking(Box::new(SealAsking {
                            answering: weft_core::seal_questions::Answering::new(asked),
                            shown: std::time::Instant::now(),
                        }))
                    };
                    self.show_seal_question();
                }
            }
            (Then::Conversation { pane }, Ok(v)) => {
                self.take_conversation(root, pane, false, &v["conversation"]);
            }
            (Then::Older { pane }, Ok(v)) => {
                self.take_conversation(root, pane, true, &v["conversation"]);
            }
            (Then::Eval, answer) => self.eval_staged(root, answer),
            // The Eval this window said yes to is open: its view opens by
            // itself once its readers start.
            (Then::Working(w), Ok(_)) if w.doing == Doing::Opening => {
                if let Some(work) = w.work {
                    self.eval_view_wanted = Some((root.to_path_buf(), work));
                    self.open_wanted_eval_view();
                }
            }
            // RingFrame's own refusal says it could not open the Eval; the
            // daemon's (already opening, no such harness) says itself.
            (Then::Working(w), Err(why)) => match w.doing {
                Doing::Opening if refused_by_ringframe => {
                    self.fail(words::fill("wait.could_not_open_eval", &[("why", &why)]))
                }
                _ => self.fail(why),
            },
            // A refusal of an act, said in the bar until the person acts. A
            // read refused is said where it would be shown.
            (Then::Started { .. } | Then::Refusal | Then::Setup { .. }, Err(why)) => self.fail(why),
            _ => {}
        }
    }

    /// The act the bar puts forward for the agent in front's latest work,
    /// once its turn has really ended.
    pub fn suggested_act(&self) -> Option<Act> {
        let t = self.active_tab()?;
        let client = self.client(&t.project)?;
        let row = client.sessions.iter().find(|r| r.pane == Some(t.pane))?;
        let turn = client.view(t.pane).and_then(|v| v.turn).or(row.turn);
        // A turn that ended asking its person waits on them, not on an Eval.
        let turn = turn.filter(|_| !row.asked);
        weft_core::board::suggested_for(&client.units, &row.id, turn)
    }

    /// Read again the conversation of each pane showing one.
    pub fn refresh_active_conversations(&mut self) {
        for (proj, pane) in self.shown_conversations() {
            self.read_conversation(&proj, pane);
        }
    }

    /// The conversations shown: each pane's tab in front, in Conversation.
    fn shown_conversations(&self) -> Vec<(PathBuf, u32)> {
        let groups = &self.state.panes.groups;
        let front = |g: &SavedTabGroup| {
            g.tabs.get(g.active_tab.min(g.tabs.len().saturating_sub(1))).cloned()
        };
        groups
            .iter()
            .filter_map(front)
            .filter(|t| t.view == PaneViewMode::Conversation)
            .map(|t| (t.project, t.pane))
            .collect()
    }

    /// Read a pane's conversation: what was added since it was last read,
    /// or its newest page.
    pub(crate) fn read_conversation(&mut self, root: &Path, pane: u32) {
        let after = self.conversation_cursors.get(&(root.to_path_buf(), pane)).map(|c| c.1.clone());
        if let Some(client) = self.client_mut(root) {
            let _ = client.conversation(pane, None, after);
        }
    }

    /// Read the page before a pane's conversation's first item, if there is one.
    pub(crate) fn read_older(&mut self, root: &Path, pane: u32) {
        let key = (root.to_path_buf(), pane);
        let Some(before) = self.conversation_cursors.get(&key).and_then(|c| c.0.clone()) else {
            return;
        };
        if let Some(client) = self.client_mut(root) {
            let _ = client.conversation(pane, Some(before), None);
        }
    }

    /// A conversation read: the newest page, an older page, or what the file
    /// added. A file replaced is read again from its newest page.
    fn take_conversation(&mut self, root: &Path, pane: u32, older: bool, c: &serde_json::Value) {
        let key = (root.to_path_buf(), pane);
        if c["reset"] == true {
            self.conversation_cursors.remove(&key);
            return self.read_conversation(root, pane);
        }
        let items: Vec<Item> = serde_json::from_value(c["items"].clone()).unwrap_or_default();
        let cursor = |k: &str| c[k].as_str().map(str::to_string);
        let held = self.conversations.entry(key.clone()).or_default();
        match self.conversation_cursors.get_mut(&key) {
            Some((before, _)) if older => {
                held.splice(0..0, items);
                *before = cursor("before");
            }
            Some((_, after)) => {
                held.extend(items);
                if let Some(next) = cursor("after") {
                    *after = next;
                }
            }
            None if !older => {
                *held = items;
                if let Some(after) = cursor("after") {
                    self.conversation_cursors.insert(key, (cursor("before"), after));
                }
            }
            None => {}
        }
    }

    /// Tell, once, of each agent that started waiting for the person and is
    /// not the one in front, in a project that tells.
    fn notice_waiting(&mut self) {
        let front = self.active_tab().map(|t| (t.project, t.pane));
        let mut now = HashSet::new();
        for c in &self.clients {
            let asking = |p: &&PaneView| p.judge.is_none() && weft_core::turns::is_asking(p.turn);
            for p in c.panes.iter().filter(asking) {
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
                earlier: client.earlier,
                work: &client.work,
                readiness: &ready_fn,
                routing: &client.routing,
                setup: client.needs,
            };
            let root = &client.root;
            let folded = |_: usize, fold: Fold| match fold {
                Fold::Project => self.state.folded_projects.iter().any(|f| f == root),
                Fold::Sealed => true,
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
        match self.setup_view() {
            Some(view) => crate::sheets::behind(view),
            None => usize::from(self.clients.iter().any(|c| c.update.is_some())),
        }
    }

    /// Set window bounds and persist to `state.json`.
    pub fn set_window_bounds(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.state.settings.window_bounds = Some(WindowBounds { x, y, width, height });
        let _ = self.save_state();
    }

    /// How wide the panes are together: the window's width, less the list
    /// beside them.
    fn panes_width(&self) -> f32 {
        let window = self.state.settings.window_bounds.map_or(1280.0, |b| b.width);
        let list = if self.state.settings.sidebar_open { 340.0 } else { 0.0 };
        window - list
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
        self.state.panes.hidden.retain(|h| h.project != tab.project || h.pane != tab.pane);
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

        let machine = self.machine_name();
        let header = render_header(&HeaderProps {
            machine: machine.clone(),
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

        // The Seal's questions and its review take the window's body.
        let seal = match &self.bottom_bar {
            BottomBarState::SealDisposition {
                project,
                eval,
                selected,
                reason,
                questions: SealQuestions::Asking(asking),
            } => Some(crate::seal_dialog::render(&crate::seal_dialog::SealProps {
                client: self.clients.iter().find(|c| &c.root == project),
                project,
                eval,
                picked: (selected, reason.as_deref()),
                asking,
                words: self.seal_words.text(),
                warning: self.act_warning.as_deref(),
            })),
            _ => None,
        };

        if self.state.settings.sidebar_open && seal.is_none() {
            body = body.child(render_tree(&TreeProps {
                projects: &projects,
                selection: self.selection.as_ref(),
                search_query: self.search.text(),
                searching: self.focus == Focus::Search,
                plus_popover_project: self.plus_popover_project.as_deref(),
                harnesses: &harnesses,
                turbo: self.turbo(),
                unfolded_earlier: &self.unfolded_earlier,
                clients: &self.clients,
                project_menu: self.project_menu.as_deref(),
                done: self.done_rows(),
            }));
        }

        let ask_send_to = match &self.active_sheet {
            Some(ActiveSheet::Ask { project, send_to, .. }) => {
                Some(self.send_to_view(project, send_to.as_deref()))
            }
            _ => None,
        };
        // What Weft reads of the setup of the harness SEND TO picks.
        let ask_warning = match (&self.active_sheet, &ask_send_to) {
            (Some(ActiveSheet::Ask { project, .. }), Some(to)) => {
                let harness = to.picked_harness();
                let client = self.clients.iter().find(|c| &c.root == project);
                client
                    .and_then(|c| {
                        weft_core::tree::Why::not_ready(c.readiness_of(&harness), &harness)
                    })
                    .filter(|w| w.is_setup())
                    .map(|w| w.warning())
            }
            _ => None,
        };
        let preview = self.preview_unit();
        let harness = self.harness_shown();
        let ask_reading = match &self.active_sheet {
            Some(ActiveSheet::Ask { project, .. }) => self
                .client(project)
                .is_some_and(|c| c.owes_any(|t| matches!(t, Then::AskSheet { .. }))),
            _ => false,
        };
        let sheet = self.active_sheet.as_ref().map(|sheet| {
            render_sheet(&SheetProps {
                sheet,
                projects: &projects,
                clients: &self.clients,
                setup: self.setup_view(),
                harnesses: &harnesses,
                bottom_bar: &self.bottom_bar,
                ask_draft: self.ask.text(),
                ask_sheet: self.ask_sheet.as_ref(),
                ask_choices: &self.ask_choices,
                ask_sheet_for: self.ask_sheet_for.as_deref(),
                ask_refused: self.ask_refused.as_deref(),
                ask_reading,
                ask_warning: ask_warning.as_deref(),
                remote_pick: self.remote_pick.as_deref(),
                send_to: ask_send_to.as_ref(),
                back: self.sheet_back.last().map(|(_, name)| name.as_str()),
                choice: self.choice.as_deref(),
                setting_editing: self.setting_editing.as_ref(),
                setting_text: self.setting_field.text(),
                preview: &preview,
                harness: &harness,
                all_rules: &self.all_rules,
                preview_shown: self.preview_shown,
                judges_shown: self.judges_shown,
                machine: &machine,
                updating: self.updating(),
                running_since: &self.running_since,
                remote_asked: self.remote_asked.is_some(),
                machine_jobs: self.machine_jobs(),
            })
        });
        // Ask docks under the panes it is about; every other sheet takes the
        // workspace's place.
        let docked = matches!(self.active_sheet, Some(ActiveSheet::Ask { .. }));
        match (seal, sheet) {
            (Some(seal), _) => body = body.child(seal),
            (None, Some(sheet)) if !docked => body = body.child(sheet),
            (None, dock) => {
                body = body.child(render_panes_workspace(&PanesProps {
                    panes: &self.state.panes,
                    clients: &self.clients,
                    layout_menu_pane: self.layout_menu_pane,
                    conversations: &self.conversations,
                    unfolded_steps: &self.unfolded_steps,
                    message_draft: self.message_text(),
                    typing: self.focus == Focus::Message,
                    dock,
                    files: &self.files,
                    width: self.panes_width(),
                    covered: self.menu_open
                        || self.plus_popover_project.is_some()
                        || self.layout_menu_pane.is_some()
                        || self.project_menu.is_some(),
                }))
            }
        }

        if let Some(root) = &self.plus_popover_project {
            body = body.child(render_plus_popover(root, &harnesses, self.turbo()));
        }
        if self.menu_open {
            body = body.child(render_menu_popover(&MenuProps {
                turbo: self.turbo(),
                start_at_login: self.clients.iter().any(|c| c.start_at_login),
                remote: self.clients.iter().find_map(|c| c.remote.as_ref()),
                heard: self.clients.iter().any(|c| c.is_connected()),
                updates: updates_available,
            }));
        }

        let read_by = self.read_by_view();
        let working = self.working();
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
                machine: &machine,
                state: &self.bottom_bar,
                clients: &self.clients,
                selection: self.selection.as_ref(),
                suggested: self.suggested_act(),
                warning: self.act_warning.as_deref(),
                read_by: read_by.as_ref(),
                working: working.as_ref(),
                failed: self.failed.as_deref(),
            }))
    }
}
