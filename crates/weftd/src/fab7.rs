//! fab7's release as one unit: RingFrame's configuration and Weft's harness and tool files,
//! installed together from one tag, each recording it in `.revision`.
//!
//! Weft runs with the newest fab7 release it can read, not the one its archive came with:
//! a fab7 release names the oldest Weft it needs, and [`NEEDS`] is the oldest fab7 this Weft
//! runs with. At start, a home this Weft cannot run with takes the newest compatible release
//! from the network, and only a home with nothing in it falls back to the archive's copy.

use std::path::{Path, PathBuf};

/// The oldest fab7 release this Weft runs with: its harness files say what this Weft reads.
pub const NEEDS: &str = "0.8.0";

/// Weft's parts of a fab7 release, and where they go under `home`.
pub fn parts(home: &Path) -> Vec<ringframe::api::Part> {
    ["harnesses", "tools"]
        .into_iter()
        .map(|name| ringframe::api::Part {
            from: format!("weft/{name}"),
            to: home.join(".fab7/weft").join(name),
        })
        .collect()
}

fn read(dir: &Path, file: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(file)).ok().map(|s| s.trim().to_string())
}

/// The release a revision names: `v0.8.0`, or `bundled v0.8.0` for the archive's copy.
fn version_of(revision: &str) -> Option<&str> {
    revision.strip_prefix("bundled ").unwrap_or(revision).strip_prefix('v')
}

/// Whether this Weft runs with what `home` holds: RingFrame's configuration and Weft's files
/// from one release, no older than [`NEEDS`], needing no newer Weft. A configuration installed
/// from a checkout (`local`, as Weft Local's) is the person's own and is run with.
pub fn runs_with(home: &Path) -> bool {
    let config = home.join(".fab7/rf/config");
    let Some(revision) = read(&config, ".revision") else { return false };
    if revision == "local" {
        return home.join(".fab7/weft/harnesses").is_dir();
    }
    let together =
        parts(home).iter().all(|p| read(&p.to, ".revision").as_deref() == Some(&revision));
    let new_enough = version_of(&revision).is_some_and(|v| !weft_core::sync::older(v, NEEDS));
    let needs_newer = read(&config, ".requires")
        .is_some_and(|need| weft_core::sync::older(env!("CARGO_PKG_VERSION"), &need));
    together && new_enough && !needs_newer
}

/// Install the newest fab7 release this Weft runs with, whole.
pub fn sync(
    home: &Path,
    progress: &dyn Fn(ringframe::api::SyncStep),
) -> Result<serde_json::Value, ringframe::api::Error> {
    ringframe::api::release_sync_with(None, &parts(home), progress)
}

/// Before the daemon starts: a home this Weft cannot run with takes the newest compatible
/// release from the network; when that cannot be reached and nothing is installed, the copy
/// that came with this Weft (`bundled`, its `ringframe/config` and `weft/` folders) is used,
/// recorded as `bundled <tag>`. A home this Weft runs with is left as it is.
pub fn set_up(home: &Path, bundled: Option<&Path>) {
    if runs_with(home) {
        return;
    }
    if sync(home, &|_| {}).is_ok() {
        return;
    }
    let nothing = !home.join(".fab7/rf/config/.revision").exists();
    if let (true, Some(fab7)) = (nothing, bundled) {
        let _ = ringframe::api::release_sync_with(
            Some(&fab7.join("ringframe/config")),
            &parts(home),
            &|_| {},
        );
    }
}

/// The release Weft's files record, when it is not the configuration's: what `ringframe sync`
/// alone leaves behind, shown as behind until Update installs the release whole.
pub fn apart(home: &Path) -> bool {
    let config = read(&home.join(".fab7/rf/config"), ".revision");
    config.as_deref() != Some("local")
        && parts(home).iter().any(|p| read(&p.to, ".revision") != config)
}

/// Where Weft's archive keeps its fab7 copy, beside the program that is running.
pub fn bundled_beside(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let fab7 = if dir.ends_with("Contents/MacOS") {
        dir.parent()?.join("Resources/fab7")
    } else {
        dir.join("fab7")
    };
    fab7.is_dir().then_some(fab7)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(
        config: Option<&str>,
        weft: Option<&str>,
        requires: Option<&str>,
    ) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let rf = home.path().join(".fab7/rf/config");
        std::fs::create_dir_all(&rf).unwrap();
        if let Some(r) = config {
            std::fs::write(rf.join(".revision"), format!("{r}\n")).unwrap();
        }
        if let Some(need) = requires {
            std::fs::write(rf.join(".requires"), format!("{need}\n")).unwrap();
        }
        for p in parts(home.path()) {
            std::fs::create_dir_all(&p.to).unwrap();
            if let Some(r) = weft {
                std::fs::write(p.to.join(".revision"), format!("{r}\n")).unwrap();
            }
        }
        home
    }

    #[test]
    fn this_weft_runs_with_one_release_new_enough_and_needing_no_newer_weft() {
        let v = format!("v{NEEDS}");
        assert!(runs_with(home_with(Some(&v), Some(&v), None).path()));
        assert!(runs_with(
            home_with(Some(&format!("bundled {v}")), Some(&format!("bundled {v}")), None).path()
        ));
        assert!(runs_with(home_with(Some("local"), None, None).path()), "Weft Local's own");
        assert!(!runs_with(home_with(None, None, None).path()), "nothing installed");
        assert!(!runs_with(home_with(Some("v0.7.1"), Some("v0.7.1"), None).path()), "too old");
        assert!(!runs_with(home_with(Some(&v), None, None).path()), "Weft's files from no release");
        assert!(!runs_with(home_with(Some(&v), Some("v0.7.1"), None).path()), "not together");
        assert!(
            !runs_with(home_with(Some(&v), Some(&v), Some("99.0.0")).path()),
            "needs a newer Weft"
        );
    }

    #[test]
    fn files_apart_from_the_configuration_are_behind() {
        let v = format!("v{NEEDS}");
        assert!(!apart(home_with(Some(&v), Some(&v), None).path()));
        assert!(apart(home_with(Some("v9.9.9"), Some(&v), None).path()));
        assert!(!apart(home_with(Some("local"), None, None).path()));
    }
}
