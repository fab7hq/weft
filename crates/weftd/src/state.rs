//! Atomic persistence for `~/.fab7/weft/state.json` and mirroring of
//! `open_projects` to `~/.fab7/weft/projects.json` (ADR-0003).

use std::path::Path;

use weft_core::projects::Opened;
use weft_core::state::WeftState;

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Load `WeftState` from `weft_dir/state.json`, falling back to
/// `weft_dir/projects.json` for `open_projects` if `state.json` is absent.
pub fn load(weft_dir: &Path) -> WeftState {
    let state_json = std::fs::read_to_string(weft_dir.join("state.json")).unwrap_or_default();
    let projects_json = std::fs::read_to_string(weft_dir.join("projects.json")).ok();
    weft_core::state::read_with_projects(&state_json, projects_json.as_deref())
}

/// Atomically write `state` to `weft_dir/state.json` (`state.json.tmp` ->
/// `state.json`) and mirror `state.open_projects` to `weft_dir/projects.json`.
pub fn save(weft_dir: &Path, state: &WeftState) -> std::io::Result<()> {
    write_atomic(&weft_dir.join("state.json"), &weft_core::state::write(state))?;
    let existing = std::fs::read_to_string(weft_dir.join("projects.json"))
        .map(|t| weft_core::projects::read(&t))
        .unwrap_or_default();
    let mirrored = weft_core::state::mirror_projects(state, &existing, crate::turns::now_millis());
    write_atomic(&weft_dir.join("projects.json"), &mirrored)
}

/// Update `weft_dir/projects.json` atomically with `open`, and ensure any
/// newly opened projects in `open` are recorded in `weft_dir/state.json`
/// without erasing existing persisted workspace projects when windows detach.
pub fn sync_open_projects(weft_dir: &Path, open: &[Opened]) {
    let normalized_open: Vec<Opened> = open
        .iter()
        .map(|o| Opened {
            path: weft_core::state::normalize_project_path(&o.path),
            opened: o.opened,
        })
        .collect();
    let _ = write_atomic(
        &weft_dir.join("projects.json"),
        &weft_core::projects::write(&normalized_open),
    );
    if normalized_open.is_empty() && !weft_dir.join("state.json").exists() {
        return;
    }
    let mut state = load(weft_dir);
    let mut changed = false;
    for o in &normalized_open {
        if !state.open_projects.contains(&o.path) {
            state.open_projects.push(o.path.clone());
            changed = true;
        }
    }
    if changed {
        let _ = write_atomic(&weft_dir.join("state.json"), &weft_core::state::write(&state));
    }
}

/// Remove an explicitly dismissed project from `weft_dir/state.json`.
pub fn remove_open_project(weft_dir: &Path, path: &Path) {
    if !weft_dir.join("state.json").exists() {
        return;
    }
    let norm = weft_core::state::normalize_project_path(path);
    let mut state = load(weft_dir);
    let before = state.open_projects.len();
    state.open_projects.retain(|p| weft_core::state::normalize_project_path(p) != norm);
    state.folded_projects.retain(|p| weft_core::state::normalize_project_path(p) != norm);
    if state.open_projects.len() != before {
        let _ = write_atomic(&weft_dir.join("state.json"), &weft_core::state::write(&state));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use weft_core::state::{
        PaneViewMode, SavedPaneTab, SavedPanesLayout, SavedTabGroup, SplitLayout, WeftSettings,
        WindowBounds,
    };

    #[test]
    fn atomic_round_trip_of_state_json_and_projects_json_mirror() {
        let tmp = tempfile::tempdir().unwrap();
        let weft_dir = tmp.path().join(".fab7/weft");

        let state = WeftState {
            version: 1,
            open_projects: vec![PathBuf::from("/work/p1"), PathBuf::from("/work/p2")],
            folded_projects: vec![PathBuf::from("/work/p2")],
            settings: WeftSettings {
                turbo: true,
                start_at_login: true,
                sidebar_open: false,
                notify_waiting: true,
                default_view: PaneViewMode::Conversation,
                window_bounds: Some(WindowBounds {
                    x: 100.0,
                    y: 60.0,
                    width: 1440.0,
                    height: 900.0,
                }),
            },
            panes: SavedPanesLayout {
                layout: SplitLayout::FourGrid,
                ratio: 0.5,
                secondary_ratio: 0.5,
                focused_group: 2,
                groups: vec![SavedTabGroup {
                    active_tab: 0,
                    tabs: vec![SavedPaneTab {
                        project: PathBuf::from("/work/p1"),
                        pane: 1,
                        view: PaneViewMode::Conversation,
                    }],
                }],
            },
        };

        save(&weft_dir, &state).unwrap();
        assert!(weft_dir.join("state.json").is_file(), "state.json written");
        assert!(!weft_dir.join("state.json.tmp").exists(), "temp file renamed away");
        assert!(weft_dir.join("projects.json").is_file(), "projects.json mirrored");

        let loaded = load(&weft_dir);
        assert_eq!(loaded, state);

        let projects_text = std::fs::read_to_string(weft_dir.join("projects.json")).unwrap();
        let mirrored: Vec<PathBuf> =
            weft_core::projects::read(&projects_text).into_iter().map(|o| o.path).collect();
        assert_eq!(mirrored, state.open_projects);
    }

    #[test]
    fn replicating_weft_directory_preserves_state_and_sense_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let src_weft = tmp.path().join("src_weft");
        let dst_weft = tmp.path().join("dst_weft");

        let state = WeftState {
            version: 1,
            open_projects: vec![PathBuf::from("/work/replicated")],
            folded_projects: Vec::new(),
            settings: WeftSettings { turbo: true, ..WeftSettings::default() },
            panes: SavedPanesLayout::default(),
        };
        save(&src_weft, &state).unwrap();
        std::fs::write(src_weft.join("config.toml"), "[routing]\nask = \"claude-code\"\n").unwrap();

        copy_dir_recursive(&src_weft, &dst_weft).unwrap();
        let restored = load(&dst_weft);
        assert_eq!(restored, state);
        assert!(dst_weft.join("projects.json").is_file());
        assert!(dst_weft.join("config.toml").is_file());
    }

    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let dest_path = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir_recursive(&entry.path(), &dest_path)?;
            } else {
                std::fs::copy(entry.path(), dest_path)?;
            }
        }
        Ok(())
    }
}
