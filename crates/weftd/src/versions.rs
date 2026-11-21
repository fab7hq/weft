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

/// The installed Sense bundle version in `$HOME/.fab7/weft/sense/current.json`.
pub fn default_sense_version() -> Option<String> {
    sense_version(&PathBuf::from(std::env::var_os("HOME")?).join(".fab7/weft"))
}

/// Load the active `SenseModel` from `<weft_dir>/sense/current.json`.
pub fn load_sense(weft_dir: &Path) -> Option<weft_sense::SenseModel> {
    let current = sense_current(weft_dir)?;
    let kept = crate::gpu::verdict(weft_dir, &current.version);
    let on_gpu = kept.filter(|p| *p != weft_sense::graph::Provider::Cpu);
    on_gpu
        .and_then(|gpu| weft_sense::SenseModel::load_on(&current.path, gpu).ok())
        .or_else(|| weft_sense::SenseModel::load(&current.path).ok())
}

/// Held while Sense is loaded and put in its slot, so one load never runs
/// behind another: the start's load is skipped once a newer one filled it.
pub fn loading() -> std::sync::MutexGuard<'static, ()> {
    static LOADING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOADING.lock().unwrap_or_else(|e| e.into_inner())
}

/// The Sense installed under `weft_dir`, loaded into `slot`.
pub fn load_into(weft_dir: &Path, slot: &std::sync::RwLock<Option<weft_sense::SenseModel>>) {
    let _loading = loading();
    if let Some(model) = load_sense(weft_dir)
        && let Ok(mut guard) = slot.write()
    {
        *guard = Some(model);
    }
}

/// Try this machine's GPU for the Sense in `slot`, once per Weft and Sense
/// version, with `program` as the child that tries it; a GPU kept takes the
/// slot's place.
pub fn sense_on_gpu(
    weft_dir: &Path,
    slot: &std::sync::RwLock<Option<weft_sense::SenseModel>>,
    program: &Path,
) {
    let _loading = loading();
    let Some(current) = sense_current(weft_dir) else { return };
    if crate::gpu::verdict(weft_dir, &current.version).is_some() {
        return;
    }
    let chosen = slot.read().ok().and_then(|guard| {
        let cpu = guard.as_ref()?;
        Some(crate::gpu::settle(weft_dir, &current.path, &current.version, cpu, program))
    });
    let Some(gpu) = chosen.filter(|p| *p != weft_sense::graph::Provider::Cpu) else { return };
    if let Ok(model) = weft_sense::SenseModel::load_on(&current.path, gpu)
        && let Ok(mut guard) = slot.write()
    {
        eprintln!("weft: Sense runs on {gpu:?}");
        *guard = Some(model);
    }
}

/// The files a bundle's manifest names, sense-xs's adapters among them,
/// each a path inside its folder.
fn bundle_files(manifest: &[u8]) -> anyhow::Result<Vec<String>> {
    let manifest: serde_json::Value = serde_json::from_slice(manifest)?;
    let files = manifest["files"].as_object().into_iter().flatten();
    let files = files.map(|(key, f)| (key.as_str(), &f["path"]));
    let adapters = manifest["adapters"].as_array().into_iter().flatten();
    let adapters = adapters.map(|a| (a["name"].as_str().unwrap_or("adapter"), &a["file"]));
    files
        .chain(adapters)
        .map(|(key, path)| {
            let name = path.as_str().unwrap_or_default();
            let inside = !name.contains('\\')
                && name.split('/').all(|part| !matches!(part, "" | "." | ".."));
            anyhow::ensure!(inside, "Sense's manifest names its {key} file {name:?}");
            Ok(name.to_string())
        })
        .collect()
}

/// Copy a candidate Sense bundle (its manifest and every file it names) into
/// `<weft_dir>/sense/versions/<version>.tmp`, verify them via `SenseModel::load`,
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

    // One install at a time: the daemon can be asked twice at once.
    static INSTALLING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
    let sense_root = weft_dir.join("sense");
    let versions_dir = sense_root.join("versions");
    std::fs::create_dir_all(&versions_dir)?;
    // This install's own names, so nothing another left behind is in its way.
    let own = format!("{}-{}", std::process::id(), crate::turns::now_millis());

    let tmp_dir = versions_dir.join(format!("{version}.tmp-{own}"));
    std::fs::create_dir_all(&tmp_dir)?;

    let names = bundle_files(manifest_text.as_bytes())?;
    for name in std::iter::once("manifest.json").chain(names.iter().map(String::as_str)) {
        if let Some(folder) = tmp_dir.join(name).parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::copy(source_dir.join(name), tmp_dir.join(name))?;
    }
    // The golden vectors, which a GPU is tried on, when the bundle has them.
    let golden = source_dir.join(weft_sense::trial::GOLDEN);
    if golden.is_file() {
        std::fs::copy(golden, tmp_dir.join(weft_sense::trial::GOLDEN))?;
    }

    // Verify SHA-256 digests and load the bundle before promoting; the model
    // holds its files in memory, so the one loaded here is the one kept.
    let model = match weft_sense::SenseModel::load(&tmp_dir) {
        Ok(model) => model,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(err);
        }
    };

    let final_dir = versions_dir.join(&version);
    let aside = versions_dir.join(format!("{version}.old-{own}"));
    let had = final_dir.exists() && std::fs::rename(&final_dir, &aside).is_ok();
    if let Err(e) = std::fs::rename(&tmp_dir, &final_dir) {
        if had {
            let _ = std::fs::rename(&aside, &final_dir);
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e.into());
    }
    if had {
        let _ = std::fs::remove_dir_all(&aside);
    }

    let current = SenseCurrent { version, path: final_dir.clone() };
    let current_tmp = sense_root.join(format!("current.json.tmp-{own}"));
    let current_path = sense_root.join("current.json");
    let mut json = serde_json::to_string_pretty(&serde_json::json!({
        "version": current.version,
        "path": current.path,
    }))?;
    json.push('\n');
    std::fs::write(&current_tmp, json)?;
    std::fs::rename(&current_tmp, &current_path)?;

    prune(&versions_dir, std::slice::from_ref(&final_dir));
    // Its convention may decide other rules: the counts are counted again.
    crate::memory::forget(&crate::memory::dir(weft_dir));
    Ok((current, model))
}

/// Install a new Sense bundle and atomically swap `slot` in memory with zero
/// downtime for running panes (`ADR-0003`).
pub fn hot_reload_sense(
    weft_dir: &Path,
    source_dir: &Path,
    slot: &std::sync::RwLock<Option<weft_sense::SenseModel>>,
) -> anyhow::Result<SenseCurrent> {
    let _loading = loading();
    let (current, model) = install_sense_bundle(weft_dir, source_dir)?;
    if let Ok(mut guard) = slot.write() {
        *guard = Some(model);
    }
    Ok(current)
}

/// Where a Sense release's files are downloaded from: its tag on Hugging Face.
pub fn sense_url(version: &str) -> String {
    format!("https://huggingface.co/{}/resolve/v{version}", weft_core::sync::SENSE_REPO)
}

/// Whether the latest Sense release is wanted: the bundle under `weft_dir`
/// is missing, not one this Weft reads (sense-d's, whatever its version), or
/// older than `latest`. With no release known, nothing is.
pub fn sense_wanted(weft_dir: &Path, latest: Option<&str>) -> bool {
    let Some(latest) = latest else { return false };
    let Some(current) = sense_current(weft_dir) else { return true };
    let manifest = std::fs::read(current.path.join("manifest.json")).unwrap_or_default();
    let schema = serde_json::from_slice::<serde_json::Value>(&manifest).ok();
    let read = schema.is_some_and(|m| m["schema"] == weft_sense::manifest::SCHEMA);
    !read || weft_core::sync::older(&current.version, latest)
}

/// Download Sense release `version` with `curl`, as `out` runs it, and
/// install it under `weft_dir`.
pub fn fetch_sense(
    weft_dir: &Path,
    out: &dyn crate::outside::Outside,
    version: &str,
) -> anyhow::Result<SenseCurrent> {
    fetch_sense_from(weft_dir, out, &sense_url(version), version, &|_| {})
}

/// Where downloading and installing Sense is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenseStep {
    /// Bytes of its files downloaded, of how many when the server said.
    Downloading { done: u64, total: Option<u64> },
    /// Checking the files and putting them in place.
    Installing,
}

/// Download Sense release `version`, install it and swap it into `slot`,
/// telling `told` the bytes downloaded as they come, then installing.
pub fn hot_reload_from_hub(
    weft_dir: &Path,
    out: &dyn crate::outside::Outside,
    slot: &std::sync::RwLock<Option<weft_sense::SenseModel>>,
    version: &str,
    told: &dyn Fn(SenseStep),
) -> anyhow::Result<SenseCurrent> {
    hot_reload_from(weft_dir, out, slot, (&sense_url(version), version), told)
}

fn hot_reload_from(
    weft_dir: &Path,
    out: &dyn crate::outside::Outside,
    slot: &std::sync::RwLock<Option<weft_sense::SenseModel>>,
    (url, version): (&str, &str),
    told: &dyn Fn(SenseStep),
) -> anyhow::Result<SenseCurrent> {
    let _loading = loading();
    let current = fetch_sense_from(weft_dir, out, url, version, told)?;
    let model = weft_sense::SenseModel::load(&current.path)?;
    if let Ok(mut guard) = slot.write() {
        *guard = Some(model);
    }
    Ok(current)
}

/// How long a file at `url` is, as its server says, following redirects.
fn length_of(out: &dyn crate::outside::Outside, url: &str) -> Option<u64> {
    let said = out.run("curl", &["-fsSIL", "--max-time", "30", url], None, ANSWERS).ok()?;
    let said = String::from_utf8_lossy(&said.stdout).to_ascii_lowercase();
    // A redirect prints its own headers first: the last length is the file's.
    said.lines().filter_map(|l| l.strip_prefix("content-length:")).next_back()?.trim().parse().ok()
}

/// How long a server has to say how long a file is.
const ANSWERS: std::time::Duration = std::time::Duration::from_secs(30);

/// Download a bundle from `url` into `<weft_dir>/sense/download`: the manifest
/// first, refused unless it is release `version`'s, then the files it names,
/// which [`install_sense_bundle`] checks against it. The bytes of those files
/// are told as they arrive, of their sum when every server said its length.
fn fetch_sense_from(
    weft_dir: &Path,
    out: &dyn crate::outside::Outside,
    url: &str,
    version: &str,
    told: &dyn Fn(SenseStep),
) -> anyhow::Result<SenseCurrent> {
    let dir = weft_dir.join("sense").join("download");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let got = (|| {
        let get = |name: &str, before: u64, total: Option<u64>| -> anyhow::Result<u64> {
            let (to, from) = (dir.join(name), format!("{url}/{name}"));
            let path = to.clone();
            let to = to.to_string_lossy();
            let args = [
                "-fsSL",
                "--retry",
                "3",
                "--retry-all-errors",
                "--create-dirs",
                "--max-time",
                "300",
                "-o",
                &to,
                &from,
            ];
            // The file grows as curl writes it: its size is what has come.
            let size = || std::fs::metadata(&path).map_or(0, |m| m.len());
            let ran = std::thread::scope(|scope| {
                let run =
                    scope.spawn(|| out.run("curl", &args, None, crate::outside::SETS_UP_WITHIN));
                while !run.is_finished() {
                    told(SenseStep::Downloading { done: before + size(), total });
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                run.join().unwrap_or_else(|_| Err(std::io::Error::other("curl stopped")))
            });
            anyhow::ensure!(ran?.status.success(), "could not download Sense's {name}");
            let done = before + size();
            told(SenseStep::Downloading { done, total });
            Ok(done)
        };
        get("manifest.json", 0, None)?;
        let bytes = std::fs::read(dir.join("manifest.json"))?;
        let manifest: serde_json::Value = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            manifest["version"] == version,
            "Sense's manifest.json is not release {version}'s"
        );
        let mut names = bundle_files(&bytes)?;
        names.push(weft_sense::trial::GOLDEN.to_string());
        let lengths: Option<Vec<u64>> =
            names.iter().map(|n| length_of(out, &format!("{url}/{n}"))).collect();
        let total = lengths.map(|l| l.iter().sum());
        let mut done = 0;
        told(SenseStep::Downloading { done, total });
        for name in &names {
            done = get(name, done, total)?;
        }
        told(SenseStep::Installing);
        install_sense_bundle(weft_dir, &dir).map(|(current, _)| current)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    got
}

#[cfg(test)]
pub(crate) mod tests {
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

    /// The published bundle tests read: sense-xs 0.0.2's manifest digest.
    pub(crate) const TESTED_SHA256: &str =
        "1f40648840e416d0f0319ef2fea0a5bf937cb8209be4b24d7027923c02a46818";
    /// Its version, as tests stage it.
    pub(crate) const TESTED: &str = "0.0.2";

    /// The tested bundle, copied to `dst` as `version`, with every file its
    /// manifest names.
    pub(crate) fn stage_sense_bundle(dst: &Path, version: &str) {
        let src = weft_sense::manifest::bundle_for_tests(TESTED_SHA256).unwrap();
        let bytes = std::fs::read(src.join("manifest.json")).unwrap();
        let golden = [weft_sense::trial::GOLDEN.to_string()];
        for name in bundle_files(&bytes).unwrap().iter().chain(&golden) {
            std::fs::create_dir_all(dst.join(name).parent().unwrap()).unwrap();
            std::fs::copy(src.join(name), dst.join(name)).unwrap();
        }
        let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        manifest["version"] = serde_json::json!(version);
        std::fs::write(dst.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap())
            .unwrap();
    }

    #[test]
    fn sense_is_downloaded_from_its_releases_tag_and_wanted_while_older_than_the_latest() {
        assert_eq!(sense_url("0.0.2"), "https://huggingface.co/fab7hq/sense-xs/resolve/v0.0.2");
        let tmp = tempfile::tempdir().unwrap();
        let weft_dir = tmp.path().join(".fab7/weft");
        assert!(!sense_wanted(&weft_dir, None), "no release known: nothing to download");
        assert!(sense_wanted(&weft_dir, Some(TESTED)), "none installed");
        install_sense_bundle(&weft_dir, &{
            let src = tmp.path().join("src");
            stage_sense_bundle(&src, TESTED);
            src
        })
        .unwrap();
        assert!(!sense_wanted(&weft_dir, Some(TESTED)), "the latest is installed");
        assert!(!sense_wanted(&weft_dir, Some("0.0.1")), "an older release is never wanted");
        assert!(sense_wanted(&weft_dir, Some("0.0.10")), "a newer release is");
        assert!(!sense_wanted(&weft_dir, None));
    }

    #[test]
    fn a_bundle_this_weft_does_not_read_is_wanted_whatever_its_version() {
        let tmp = tempfile::tempdir().unwrap();
        let weft_dir = tmp.path().join(".fab7/weft");
        for (schema, version) in
            [("fab7.sense.bundle/2", "0.2.0"), ("fab7.sense.decision/2", "0.1.1")]
        {
            let old = weft_dir.join("sense/versions").join(version);
            std::fs::create_dir_all(&old).unwrap();
            let manifest = serde_json::json!({"schema": schema, "version": version, "files": {}});
            std::fs::write(old.join("manifest.json"), manifest.to_string()).unwrap();
            let pointer = serde_json::json!({"version": version, "path": old});
            std::fs::write(weft_dir.join("sense/current.json"), pointer.to_string()).unwrap();
            assert_eq!(sense_version(&weft_dir).as_deref(), Some(version));
            assert!(sense_wanted(&weft_dir, Some(version)), "{schema} {version}: not sense-xs");
        }
    }

    #[test]
    fn a_manifest_naming_a_file_outside_its_folder_is_refused() {
        for path in ["../sense.int8.onnx", "a/../b.onnx", "/b.onnx", "a//b", "a\\b", "", ".."] {
            let manifest = serde_json::json!({"files": {"model": {"path": path, "sha256": "ab"}}});
            assert!(bundle_files(manifest.to_string().as_bytes()).is_err(), "{path:?}");
            let adapter = serde_json::json!({"files": {}, "adapters": [{"file": path}]});
            assert!(bundle_files(adapter.to_string().as_bytes()).is_err(), "{path:?}");
        }
    }

    #[test]
    fn a_sense_xs_bundle_names_its_adapters_files_in_their_folder() {
        let manifest = serde_json::json!({
            "files": {"model": {"path": "sense-xs.int8.onnx"}, "tokenizer": {"path": "tokenizer.json"}},
            "adapters": [{"name": "state", "file": "adapters/state.onnx_adapter"},
                         {"name": "ask", "file": "adapters/ask.onnx_adapter"}]});
        let mut files = bundle_files(manifest.to_string().as_bytes()).unwrap();
        files.sort();
        assert_eq!(
            files,
            [
                "adapters/ask.onnx_adapter",
                "adapters/state.onnx_adapter",
                "sense-xs.int8.onnx",
                "tokenizer.json"
            ]
        );
    }

    #[test]
    fn a_sense_release_is_fetched_only_when_its_manifest_is_that_releases() {
        let tmp = tempfile::tempdir().unwrap();
        let weft_dir = tmp.path().join(".fab7/weft");
        let hosted = tmp.path().join("hosted");
        stage_sense_bundle(&hosted, "0.0.7");
        let url = format!("file://{}", hosted.display());
        let manifest = std::fs::read(hosted.join("manifest.json")).unwrap();
        let out = crate::outside::machine();

        assert!(fetch_sense_from(&weft_dir, &*out, &url, "0.0.8", &|_| {}).is_err());
        assert_eq!(sense_version(&weft_dir), None, "another release's manifest installs nothing");

        let seen = std::sync::Mutex::new(Vec::new());
        let told = |s: SenseStep| seen.lock().unwrap().push(s);
        let current = fetch_sense_from(&weft_dir, &*out, &url, "0.0.7", &told).unwrap();
        // Its files' bytes, counted as they came, of their sum; then installing.
        let seen = seen.into_inner().unwrap();
        let names =
            bundle_files(&manifest).unwrap().into_iter().chain([weft_sense::trial::GOLDEN.into()]);
        let sum: u64 = names.map(|n| std::fs::metadata(hosted.join(n)).unwrap().len()).sum();
        let counts: Vec<(u64, Option<u64>)> = seen
            .iter()
            .filter_map(|s| match s {
                SenseStep::Downloading { done, total: Some(total) } => Some((*done, Some(*total))),
                _ => None,
            })
            .collect();
        assert_eq!(counts.first(), Some(&(0, Some(sum))), "the sum is known first");
        assert_eq!(counts.last(), Some(&(sum, Some(sum))));
        assert!(counts.windows(2).all(|w| w[0].0 <= w[1].0), "{counts:?}");
        assert_eq!(seen.last(), Some(&SenseStep::Installing));
        assert_eq!(current.version, "0.0.7");
        assert!(!sense_wanted(&weft_dir, Some("0.0.7")));
        for name in bundle_files(&manifest).unwrap() {
            assert!(current.path.join(&name).is_file(), "{name} is installed");
        }
        assert!(current.path.join(weft_sense::trial::GOLDEN).is_file(), "and its golden vectors");
        assert!(!weft_dir.join("sense/download").exists(), "the download folder goes");

        // A file that is not the one its manifest names leaves the installed one.
        let other = tmp.path().join("other");
        stage_sense_bundle(&other, "0.0.9");
        std::fs::write(other.join("tokenizer.json"), b"{}").unwrap();
        let url = format!("file://{}", other.display());
        assert!(fetch_sense_from(&weft_dir, &*out, &url, "0.0.9", &|_| {}).is_err());
        assert_eq!(sense_version(&weft_dir).as_deref(), Some("0.0.7"));
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

        let rules = slot.read().unwrap().as_ref().map(|m| m.convention().to_vec()).unwrap();
        assert!(rules.contains(&"practice.kiss".to_string()), "its convention's rules: {rules:?}");
        let counts = crate::memory::dir(&weft_dir).join("counts.json");
        std::fs::create_dir_all(counts.parent().unwrap()).unwrap();
        std::fs::write(&counts, "{}").unwrap();

        // 2. Tamper with a candidate bundle and verify SHA-256 rejection leaves 0.1.0 active
        let bad_src = tmp.path().join("src_bad");
        stage_sense_bundle(&bad_src, "0.1.5");
        std::fs::write(bad_src.join("tokenizer.json"), b"{}").unwrap();
        assert!(hot_reload_sense(&weft_dir, &bad_src, &slot).is_err());
        assert_eq!(sense_version(&weft_dir).as_deref(), Some("0.1.0"));
        assert!(counts.exists(), "a refused bundle keeps the counts");
        assert_eq!(slot.read().unwrap().as_ref().map(|m| m.version()), Some("0.1.0"));

        // 3. Hot-reload 0.2.0 in place and verify 0.1.0 is pruned while 0.2.0 is active
        let c2 = hot_reload_sense(&weft_dir, &v2_src, &slot).unwrap();
        assert_eq!(c2.version, "0.2.0");
        assert_eq!(sense_version(&weft_dir).as_deref(), Some("0.2.0"));
        assert_eq!(slot.read().unwrap().as_ref().map(|m| m.version()), Some("0.2.0"));
        assert!(weft_dir.join("sense/versions/0.2.0/manifest.json").is_file());
        assert!(!weft_dir.join("sense/versions/0.1.0").exists(), "old Sense version pruned");
        assert!(!counts.exists(), "a new bundle has the counts counted again");
    }
}
