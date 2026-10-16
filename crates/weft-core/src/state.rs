//! Durable machine-level Weft state (`~/.fab7/weft/state.json`), and how its
//! open project list mirrors to `~/.fab7/weft/projects.json` (ADR-0003).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::projects::Opened;

/// The split geometry of up to four editor-style panes (`D15.1`–`D15.4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitLayout {
    #[default]
    Single,
    TwoColumns,
    TwoRows,
    ThreeRight,
    FourGrid,
}

fn default_version() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

fn default_half() -> f32 {
    0.5
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeftState {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Ordered list of currently open project directories.
    #[serde(default)]
    pub open_projects: Vec<PathBuf>,
    /// Folded project paths in the left Tree.
    #[serde(default)]
    pub folded_projects: Vec<PathBuf>,
    /// Durable user & window settings.
    #[serde(default)]
    pub settings: WeftSettings,
    /// Saved 4-pane split layout and per-tab Terminal | Conversation view mode.
    #[serde(default)]
    pub panes: SavedPanesLayout,
}

impl Default for WeftState {
    fn default() -> Self {
        Self {
            version: default_version(),
            open_projects: Vec::new(),
            folded_projects: Vec::new(),
            settings: WeftSettings::default(),
            panes: SavedPanesLayout::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeftSettings {
    /// When true, newly started agents receive full permissions (--dangerously-skip-permissions / --yolo).
    #[serde(default)]
    pub turbo: bool,
    /// When true, com.fab7.weft launchd service starts Weft at login.
    #[serde(default)]
    pub start_at_login: bool,
    /// Whether the left 380px project/agent/work tree is open.
    #[serde(default = "default_true")]
    pub sidebar_open: bool,
    /// Whether macOS desktop notifications fire when an agent enters Waiting.
    #[serde(default = "default_true")]
    pub notify_waiting: bool,
    #[serde(default)]
    pub default_view: PaneViewMode,
    #[serde(default)]
    pub window_bounds: Option<WindowBounds>,
}

impl Default for WeftSettings {
    fn default() -> Self {
        Self {
            turbo: false,
            start_at_login: false,
            sidebar_open: true,
            notify_waiting: true,
            default_view: PaneViewMode::Terminal,
            window_bounds: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneViewMode {
    #[default]
    Terminal,
    Conversation,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPanesLayout {
    #[serde(default)]
    pub layout: SplitLayout,
    #[serde(default = "default_half")]
    pub ratio: f32,
    #[serde(default = "default_half")]
    pub secondary_ratio: f32,
    #[serde(default)]
    pub focused_group: usize,
    #[serde(default)]
    pub groups: Vec<SavedTabGroup>,
}

impl Default for SavedPanesLayout {
    fn default() -> Self {
        Self {
            layout: SplitLayout::Single,
            ratio: 0.5,
            secondary_ratio: 0.5,
            focused_group: 0,
            groups: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SavedTabGroup {
    #[serde(default)]
    pub active_tab: usize,
    #[serde(default)]
    pub tabs: Vec<SavedPaneTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPaneTab {
    pub project: PathBuf,
    pub pane: u32,
    #[serde(default)]
    pub view: PaneViewMode,
}

/// Read `WeftState` from JSON text; empty or unreadable text starts fresh.
pub fn read(text: &str) -> WeftState {
    if text.trim().is_empty() {
        return WeftState::default();
    }
    let mut state: WeftState = serde_json::from_str(text).unwrap_or_default();
    if state.version == 0 {
        state.version = 1;
    }
    dedup_paths(&mut state.open_projects);
    dedup_paths(&mut state.folded_projects);
    state
}

/// Read `WeftState` from `state_json`, falling back to `projects_json` for
/// `open_projects` when `state_json` is empty or missing.
pub fn read_with_projects(state_json: &str, projects_json: Option<&str>) -> WeftState {
    if !state_json.trim().is_empty()
        && let Ok(mut state) = serde_json::from_str::<WeftState>(state_json)
    {
        if state.version == 0 {
            state.version = 1;
        }
        if state.open_projects.is_empty()
            && let Some(proj_text) = projects_json
        {
            state.open_projects =
                crate::projects::read(proj_text).into_iter().map(|o| o.path).collect();
        }
        dedup_paths(&mut state.open_projects);
        dedup_paths(&mut state.folded_projects);
        return state;
    }
    let mut state = WeftState::default();
    if let Some(proj_text) = projects_json {
        state.open_projects =
            crate::projects::read(proj_text).into_iter().map(|o| o.path).collect();
        dedup_paths(&mut state.open_projects);
    }
    state
}

/// Format `WeftState` as pretty-printed JSON with a trailing newline.
pub fn write(state: &WeftState) -> String {
    let mut s = serde_json::to_string_pretty(state).unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

/// Produce `projects.json` text mirroring `state.open_projects`, preserving
/// `opened` timestamps from `existing` where already present.
pub fn mirror_projects(state: &WeftState, existing: &[Opened], now_millis: i64) -> String {
    let opened: Vec<Opened> = state
        .open_projects
        .iter()
        .map(|path| {
            let ts =
                existing.iter().find(|o| &o.path == path).map(|o| o.opened).unwrap_or(now_millis);
            Opened { path: path.clone(), opened: ts }
        })
        .collect();
    crate::projects::write(&opened)
}

pub fn normalize_project_path(path: &std::path::Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.len() > 1 { PathBuf::from(s.trim_end_matches('/')) } else { path.to_path_buf() }
}

fn dedup_paths(paths: &mut Vec<PathBuf>) {
    for p in paths.iter_mut() {
        *p = normalize_project_path(p);
    }
    let mut seen = Vec::with_capacity(paths.len());
    paths.retain(|p| {
        if seen.contains(p) {
            false
        } else {
            seen.push(p.clone());
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_projects_settings_and_four_pane_layout() {
        let state = WeftState {
            version: 1,
            open_projects: vec![PathBuf::from("/work/alpha"), PathBuf::from("/work/beta")],
            folded_projects: vec![PathBuf::from("/work/beta")],
            settings: WeftSettings {
                turbo: true,
                start_at_login: true,
                sidebar_open: false,
                notify_waiting: true,
                default_view: PaneViewMode::Conversation,
                window_bounds: Some(WindowBounds {
                    x: 120.0,
                    y: 80.0,
                    width: 1440.0,
                    height: 900.0,
                }),
            },
            panes: SavedPanesLayout {
                layout: SplitLayout::TwoColumns,
                ratio: 0.45,
                secondary_ratio: 0.55,
                focused_group: 1,
                groups: vec![
                    SavedTabGroup {
                        active_tab: 0,
                        tabs: vec![SavedPaneTab {
                            project: PathBuf::from("/work/alpha"),
                            pane: 1,
                            view: PaneViewMode::Terminal,
                        }],
                    },
                    SavedTabGroup {
                        active_tab: 0,
                        tabs: vec![SavedPaneTab {
                            project: PathBuf::from("/work/alpha"),
                            pane: 2,
                            view: PaneViewMode::Conversation,
                        }],
                    },
                ],
            },
        };

        let encoded = write(&state);
        assert!(encoded.contains("\"two_columns\""), "snake_case layout in state.json");
        assert!(encoded.contains("\"conversation\""), "snake_case view mode in state.json");
        let decoded = read(&encoded);
        assert_eq!(decoded, state);
    }

    #[test]
    fn falls_back_to_projects_json_when_state_json_is_absent_and_mirrors_back() {
        let projects_json = crate::projects::write(&[
            Opened { path: PathBuf::from("/work/one"), opened: 1_700_000_000_000 },
            Opened { path: PathBuf::from("/work/two"), opened: 1_700_000_005_000 },
        ]);
        let state = read_with_projects("", Some(&projects_json));
        assert_eq!(
            state.open_projects,
            vec![PathBuf::from("/work/one"), PathBuf::from("/work/two")]
        );

        let existing = crate::projects::read(&projects_json);
        let mut updated = state.clone();
        updated.open_projects.push(PathBuf::from("/work/three"));
        let mirrored = mirror_projects(&updated, &existing, 1_700_000_010_000);
        let mirrored_list = crate::projects::read(&mirrored);
        assert_eq!(
            mirrored_list,
            vec![
                Opened { path: PathBuf::from("/work/one"), opened: 1_700_000_000_000 },
                Opened { path: PathBuf::from("/work/two"), opened: 1_700_000_005_000 },
                Opened { path: PathBuf::from("/work/three"), opened: 1_700_000_010_000 },
            ]
        );
    }
}
