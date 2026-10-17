//! Versions side by side: `install.sh` puts each Weft in
//! `~/.fab7/weft/versions/<version>/weft` and points `weft` at the newest, so
//! an update never replaces a program under a process running it.
//!
//! Every Weft process holds its own version's folder while it runs: a shared
//! lock on `<folder>/.held`, which the system lets go of when the process
//! ends, however it ends. A folder is removed only when no process holds it.

use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// The folder of each version, under Weft's own.
pub fn dir(weft: &Path) -> PathBuf {
    weft.join("versions")
}

/// `~/.fab7/weft/versions`.
pub fn here() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| dir(&PathBuf::from(h).join(".fab7/weft")))
}

/// The version's folder this program is in, when it is one: the program's
/// real path, links followed, inside `versions`, beside it or deeper, as in
/// the app a version holds.
pub fn own(versions: &Path, exe: &Path) -> Option<PathBuf> {
    let real = exe.canonicalize().ok()?;
    let versions = versions.canonicalize().ok()?;
    real.ancestors().skip(1).find(|f| f.parent() == Some(versions.as_path())).map(Path::to_path_buf)
}

/// A version's folder, held while this lives.
pub struct Held(#[allow(dead_code)] File);

/// Hold `folder` for as long as the result lives (this process, as a rule).
pub fn hold(folder: &Path) -> std::io::Result<Held> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(folder.join(".held"))?;
    // SAFETY: a valid descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Held(file))
}

/// Hold this program's own version, when it runs from one.
pub fn hold_own() -> Option<Held> {
    let exe = std::env::current_exe().ok()?;
    hold(&own(&here()?, &exe)?).ok()
}

/// Remove every version but `keep` that no process holds; the ones removed.
pub fn prune(versions: &Path, keep: &[PathBuf]) -> Vec<PathBuf> {
    let keep: Vec<PathBuf> = keep.iter().filter_map(|k| k.canonicalize().ok()).collect();
    let Ok(entries) = std::fs::read_dir(versions) else { return Vec::new() };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let Ok(folder) = entry.path().canonicalize() else { continue };
        if !folder.is_dir() || keep.contains(&folder) {
            continue;
        }
        let held = folder.join(".held");
        let opened =
            std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(held);
        let Ok(file) = opened else { continue };
        // SAFETY: a valid descriptor this function owns.
        let free = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if free && std::fs::remove_dir_all(&folder).is_ok() {
            removed.push(folder);
        }
    }
    removed
}

/// The version the installed `weft` points at: the newest installed. The
/// link the installer made is read first, where it puts it: an app opened
/// from the desktop starts the daemon with a `PATH` that may not hold it.
pub fn newest(versions: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let installed = std::env::var_os("WEFT_BIN_DIR")
        .map(PathBuf::from)
        .into_iter()
        .chain(home.map(|h| h.join(".local/bin")))
        .map(|d| d.join("weft"));
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("weft")).collect::<Vec<_>>())
        .unwrap_or_default();
    newest_of(versions, installed.chain(on_path))
}

/// The version the first of `links` that exists points at.
pub fn newest_of(versions: &Path, links: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    let link = links.into_iter().find(|p| p.exists())?;
    own(versions, &link)
}

/// Active Sense bundle pointer in `<weft_dir>/sense/current.json` (`ADR-0003`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenseCurrent {
    pub version: String,
    pub path: PathBuf,
}

/// Read `<weft_dir>/sense/current.json`, resolving relative paths against
/// `<weft_dir>/sense/`.
pub fn sense_current(weft_dir: &Path) -> Option<SenseCurrent> {
    let sense_root = weft_dir.join("sense");
    let text = std::fs::read_to_string(sense_root.join("current.json")).ok()?;
    let val: serde_json::Value = serde_json::from_str(&text).ok()?;
    let version = val["version"].as_str()?.to_string();
    let mut path = PathBuf::from(val["path"].as_str()?);
    if !path.is_absolute() {
        path = sense_root.join(&path);
    }
    path.join("manifest.json").is_file().then_some(SenseCurrent { version, path })
}

/// The installed Sense bundle version under `weft_dir`, if any.
pub fn sense_version(weft_dir: &Path) -> Option<String> {
    sense_current(weft_dir).map(|c| c.version)
}

/// The installed Sense bundle version in `$HOME/.fab7/weft/sense/current.json`
/// (falling back to `$HOME/.fab7/sense/fab7-sense-v1/manifest.json`).
pub fn default_sense_version() -> Option<String> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if let Some(ver) = sense_version(&home.join(".fab7/weft")) {
        return Some(ver);
    }
    let fallback = home.join(".fab7/sense/fab7-sense-v1/manifest.json");
    let text = std::fs::read_to_string(fallback).ok()?;
    let val: serde_json::Value = serde_json::from_str(&text).ok()?;
    val["version"].as_str().filter(|v| !v.is_empty()).map(str::to_string)
}

/// Load the active `SenseModel` from `<weft_dir>/sense/current.json`, falling
/// back to `$HOME/.fab7/sense/fab7-sense-v1/` when `fallback_home` is true.
pub fn load_sense(weft_dir: &Path, fallback_home: bool) -> Option<weft_sense::SenseModel> {
    if let Some(cur) = sense_current(weft_dir)
        && let Ok(model) = weft_sense::SenseModel::load(&cur.path)
    {
        return Some(model);
    }
    if fallback_home && let Some(home) = std::env::var_os("HOME") {
        return weft_sense::SenseModel::load_default_home(Path::new(&home)).ok().flatten();
    }
    None
}

/// Copy a candidate Sense bundle into `<weft_dir>/sense/versions/<version>.tmp`,
/// verify all SHA-256 digests and ONNX/tokenizer weights via `SenseModel::load`,
/// atomically promote to `<weft_dir>/sense/versions/<version>`, update
/// `<weft_dir>/sense/current.json`, and prune older versions (`ADR-0003`).
pub fn install_sense_bundle(
    weft_dir: &Path,
    source_dir: &Path,
) -> anyhow::Result<(SenseCurrent, weft_sense::SenseModel)> {
    let manifest_text = std::fs::read_to_string(source_dir.join("manifest.json"))?;
    let manifest_val: serde_json::Value = serde_json::from_str(&manifest_text)?;
    let version = manifest_val["version"]
        .as_str()
        .filter(|v| {
            !v.is_empty()
                && !v.starts_with('.')
                && v.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        })
        .ok_or_else(|| anyhow::anyhow!("invalid or missing version in Sense manifest.json"))?
        .to_string();

    let sense_root = weft_dir.join("sense");
    let versions_dir = sense_root.join("versions");
    std::fs::create_dir_all(&versions_dir)?;

    let tmp_dir = versions_dir.join(format!("{version}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir)?;

    for name in [
        "manifest.json",
        "sense.int8.onnx",
        "model.int8.onnx",
        "tokenizer.json",
        "calibration.json",
    ] {
        let src_file = source_dir.join(name);
        if src_file.is_file() {
            std::fs::copy(&src_file, tmp_dir.join(name))?;
        }
    }

    // Verify SHA-256 digests and load the ONNX/tokenizer bundle before promoting.
    if let Err(err) = weft_sense::SenseModel::load(&tmp_dir) {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(err);
    }

    let final_dir = versions_dir.join(&version);
    if final_dir.exists() {
        let _ = std::fs::remove_dir_all(&final_dir);
    }
    std::fs::rename(&tmp_dir, &final_dir)?;

    let model = weft_sense::SenseModel::load(&final_dir)?;
    let current = SenseCurrent { version, path: final_dir.clone() };
    let current_tmp = sense_root.join("current.json.tmp");
    let current_path = sense_root.join("current.json");
    let mut json = serde_json::to_string_pretty(&serde_json::json!({
        "version": current.version,
        "path": current.path,
    }))?;
    json.push('\n');
    std::fs::write(&current_tmp, json)?;
    std::fs::rename(&current_tmp, &current_path)?;

    prune(&versions_dir, std::slice::from_ref(&final_dir));
    Ok((current, model))
}

/// Install a new Sense bundle and atomically swap `slot` in memory with zero
/// downtime for running panes (`ADR-0003`).
pub fn hot_reload_sense(
    weft_dir: &Path,
    source_dir: &Path,
    slot: &std::sync::RwLock<Option<weft_sense::SenseModel>>,
) -> anyhow::Result<SenseCurrent> {
    let (current, model) = install_sense_bundle(weft_dir, source_dir)?;
    if let Ok(mut guard) = slot.write() {
        *guard = Some(model);
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("weft-versions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let versions = dir(&root);
        let (old, new) = (versions.join("0.2.0"), versions.join("0.2.1"));
        for v in [&old, &new] {
            std::fs::create_dir_all(v).unwrap();
            std::fs::write(v.join("weft"), "#!/bin/sh\n").unwrap();
        }
        (versions, old, new)
    }

    #[test]
    fn a_program_knows_its_version_through_the_link_it_was_started_by() {
        let (versions, old, _) = layout("own");
        let link = versions.parent().unwrap().join("weft");
        std::os::unix::fs::symlink(old.join("weft"), &link).unwrap();
        assert_eq!(own(&versions, &link), Some(old.canonicalize().unwrap()));
        assert_eq!(own(&versions, Path::new("/bin/sh")), None, "not one of Weft's");
        let _ = std::fs::remove_dir_all(versions.parent().unwrap());
    }

    #[test]
    fn a_program_inside_a_versions_app_knows_its_version() {
        let (versions, _, new) = layout("app");
        let inside = new.join("Weft.app/Contents/MacOS");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("weft"), "#!/bin/sh\n").unwrap();
        // `versions/<v>/weft` and `~/.local/bin/weft` link into the app.
        let _ = std::fs::remove_file(new.join("weft"));
        std::os::unix::fs::symlink(inside.join("weft"), new.join("weft")).unwrap();
        let link = versions.parent().unwrap().join("weft");
        std::os::unix::fs::symlink(new.join("weft"), &link).unwrap();
        assert_eq!(own(&versions, &inside.join("weft")), Some(new.canonicalize().unwrap()));
        assert_eq!(own(&versions, &link), Some(new.canonicalize().unwrap()));
        let _ = std::fs::remove_dir_all(versions.parent().unwrap());
    }

    #[test]
    fn the_newest_is_the_installed_link_even_off_path() {
        let (versions, _, new) = layout("newest");
        let bin = versions.parent().unwrap().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(new.join("weft"), bin.join("weft")).unwrap();
        let none = versions.parent().unwrap().join("nowhere/weft");
        assert_eq!(
            newest_of(&versions, [none, bin.join("weft")]),
            Some(new.canonicalize().unwrap())
        );
        let _ = std::fs::remove_dir_all(versions.parent().unwrap());
    }

    /// An old version is removed only once nothing runs it; the one kept
    /// (this program's, and the newest) never is.
    #[test]
    fn an_old_version_is_removed_only_when_no_process_holds_it() {
        let (versions, old, new) = layout("prune");
        let held = hold(&old).unwrap();
        assert!(prune(&versions, std::slice::from_ref(&new)).is_empty(), "held: kept");
        assert!(old.join("weft").exists());
        drop(held);
        assert_eq!(prune(&versions, std::slice::from_ref(&new)).len(), 1);
        assert!(!old.exists() && new.join("weft").exists(), "the kept one stays");
        let _ = std::fs::remove_dir_all(versions.parent().unwrap());
    }

    /// Held by another process, as a window of the old version would.
    #[test]
    fn a_version_another_process_runs_is_kept() {
        let (versions, old, new) = layout("other");
        let mut other = std::process::Command::new("/usr/bin/env")
            .args(["perl", "-e"])
            .arg(
                "use Fcntl qw(:flock); open(my $f, '>>', $ARGV[0]) or die; \
                 flock($f, LOCK_SH) or die; print \"held\\n\"; $| = 1; sleep 30;",
            )
            .arg(old.join(".held"))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(other.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        assert_eq!(line.trim(), "held");
        assert!(prune(&versions, std::slice::from_ref(&new)).is_empty());
        other.kill().unwrap();
        other.wait().unwrap();
        assert_eq!(prune(&versions, std::slice::from_ref(&new)).len(), 1, "it ended: removed");
        let _ = std::fs::remove_dir_all(versions.parent().unwrap());
    }

    fn stage_sense_bundle(dst: &Path, version: &str) {
        let workspace_bundle =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../sense/dist/fab7-sense-v1");
        let src = if workspace_bundle.exists() {
            workspace_bundle
        } else {
            let home = std::env::var_os("HOME").expect("HOME");
            PathBuf::from(home).join(".fab7/sense/fab7-sense-v1")
        };
        std::fs::create_dir_all(dst).unwrap();
        for name in ["sense.int8.onnx", "tokenizer.json", "calibration.json"] {
            let p = src.join(name);
            if p.exists() {
                std::fs::copy(p, dst.join(name)).unwrap();
            }
        }
        let mut manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(src.join("manifest.json")).unwrap())
                .unwrap();
        manifest["version"] = serde_json::json!(version);
        std::fs::write(dst.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap())
            .unwrap();
    }

    #[test]
    fn sense_bundle_install_verifies_sha256_prunes_old_and_hot_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let weft_dir = tmp.path().join(".fab7/weft");
        let v1_src = tmp.path().join("src_0.1.0");
        let v2_src = tmp.path().join("src_0.2.0");
        stage_sense_bundle(&v1_src, "0.1.0");
        stage_sense_bundle(&v2_src, "0.2.0");

        let slot = std::sync::Arc::new(std::sync::RwLock::new(None));

        // 1. Install and hot-reload 0.1.0
        let c1 = hot_reload_sense(&weft_dir, &v1_src, &slot).unwrap();
        assert_eq!(c1.version, "0.1.0");
        assert_eq!(sense_version(&weft_dir).as_deref(), Some("0.1.0"));
        assert_eq!(slot.read().unwrap().as_ref().map(|m| m.version()), Some("0.1.0"));
        assert!(weft_dir.join("sense/versions/0.1.0/manifest.json").is_file());

        // 2. Tamper with a candidate bundle and verify SHA-256 rejection leaves 0.1.0 active
        let bad_src = tmp.path().join("src_bad");
        stage_sense_bundle(&bad_src, "0.1.5");
        std::fs::write(bad_src.join("tokenizer.json"), b"{}").unwrap();
        assert!(hot_reload_sense(&weft_dir, &bad_src, &slot).is_err());
        assert_eq!(sense_version(&weft_dir).as_deref(), Some("0.1.0"));
        assert_eq!(slot.read().unwrap().as_ref().map(|m| m.version()), Some("0.1.0"));

        // 3. Hot-reload 0.2.0 in place and verify 0.1.0 is pruned while 0.2.0 is active
        let c2 = hot_reload_sense(&weft_dir, &v2_src, &slot).unwrap();
        assert_eq!(c2.version, "0.2.0");
        assert_eq!(sense_version(&weft_dir).as_deref(), Some("0.2.0"));
        assert_eq!(slot.read().unwrap().as_ref().map(|m| m.version()), Some("0.2.0"));
        assert!(weft_dir.join("sense/versions/0.2.0/manifest.json").is_file());
        assert!(!weft_dir.join("sense/versions/0.1.0").exists(), "old Sense version pruned");
    }
}
