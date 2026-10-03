//! Installing `cloudflared` when the person says yes (ADR-0023, remote.md
//! §7a): by Homebrew when it is there, else Cloudflare's latest release, as
//! Weft installs itself. The facts are in the tool file,
//! `~/.fab7/weft/tools/cloudflared.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// The tool file, as read.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    /// The program's name, found on `PATH`.
    pub program: String,
    /// The Homebrew install, when the file names one.
    pub brew: Option<Vec<String>>,
    /// The GitHub repository whose latest release is downloaded.
    pub repo: Option<String>,
    /// Each `<os>-<arch>`'s asset.
    pub assets: BTreeMap<String, String>,
}

impl Tool {
    /// `tools/cloudflared.toml` under `weft` (`~/.fab7/weft`), or why not.
    pub fn load(weft: &Path) -> Result<Self, String> {
        let path = weft.join("tools").join("cloudflared.toml");
        let text = std::fs::read_to_string(&path).map_err(|_| {
            "Weft's tool file for cloudflared is missing: run weft update.".to_string()
        })?;
        Self::read(&text)
    }

    pub fn read(text: &str) -> Result<Self, String> {
        let broken =
            || "Weft's tool file for cloudflared cannot be read: run weft update.".to_string();
        let t: toml::Table = text.parse().map_err(|_| broken())?;
        let program =
            t.get("program").and_then(toml::Value::as_str).ok_or_else(broken)?.to_string();
        let install = t.get("install").and_then(toml::Value::as_table);
        let brew = install
            .and_then(|i| i.get("brew")?.get("argv")?.as_array().cloned())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>())
            .filter(|a| !a.is_empty());
        let download = install.and_then(|i| i.get("download")?.as_table());
        let repo = download.and_then(|d| d.get("repo")?.as_str()).map(str::to_string);
        let assets = download
            .and_then(|d| d.get("assets")?.as_table())
            .map(|a| {
                a.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()
            })
            .unwrap_or_default();
        Ok(Tool { program, brew, repo, assets })
    }

    /// This machine's asset: `darwin-arm64`, `linux-amd64`, …
    pub fn asset(&self) -> Option<&str> {
        self.assets.get(&machine()).map(String::as_str)
    }
}

/// This machine as the tool file names it.
pub fn machine() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// GitHub's API, where the latest release is asked for.
pub const GITHUB_API: &str = "https://api.github.com";
/// A release's program is tens of megabytes; nothing bigger is read.
const LARGEST: u64 = 300 * 1024 * 1024;

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Cloudflare's latest release, this machine's asset, checked against
/// GitHub's digest and the notes' checksum of the program, written to
/// `weft/bin/<program>` and named in `weft/config.toml` (remote.md §7a).
/// Nothing is written unless every check passes. `api` is GitHub's API.
pub fn download(api: &str, tool: &Tool, weft: &Path) -> Result<PathBuf, String> {
    let asset = tool.asset().ok_or_else(|| {
        format!("Cloudflare publishes no {} build for {}.", tool.program, machine())
    })?;
    let repo = tool.repo.as_deref().ok_or("Weft's tool file names no release to download.")?;
    let http: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(300))).build().into();
    let get = |url: &str| {
        http.get(url)
            .header("user-agent", "weft")
            .header("accept", "application/vnd.github+json")
            .call()
    };
    let unreachable = |_| format!("Could not reach {repo}'s releases on GitHub.");
    let text = get(&format!("{api}/repos/{repo}/releases/latest"))
        .map_err(unreachable)?
        .body_mut()
        .read_to_string()
        .map_err(unreachable)?;
    let release: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| format!("GitHub's answer about {repo} cannot be read."))?;
    let found = release["assets"].as_array().into_iter().flatten().find(|a| a["name"] == asset);
    let found = found.ok_or_else(|| format!("{repo}'s latest release has no {asset}."))?;
    let digest = found["digest"].as_str().and_then(|d| d.strip_prefix("sha256:"));
    let digest = digest
        .ok_or_else(|| format!("GitHub gives no checksum for {asset}; nothing installed."))?;
    // Cloudflare's own line for it under "SHA256 Checksums": the program's.
    let notes = release["body"].as_str().unwrap_or_default();
    let listed =
        notes.lines().find_map(|l| l.trim().strip_prefix(&format!("{asset}:")).map(str::trim));
    let url = found["browser_download_url"]
        .as_str()
        .ok_or_else(|| format!("{asset} has no download address."))?;
    let bytes = get(url)
        .map_err(|_| format!("Could not download {asset}."))?
        .body_mut()
        .with_config()
        .limit(LARGEST)
        .read_to_vec()
        .map_err(|_| format!("Could not download {asset}."))?;
    if hex(&bytes) != digest {
        return Err(format!("{asset} does not match GitHub's checksum; nothing installed."));
    }
    let program =
        if asset.ends_with(".tgz") { unpack(&bytes, &tool.program, weft)? } else { bytes };
    if listed != Some(hex(&program).as_str()) {
        return Err(format!(
            "{} does not match Cloudflare's checksum in the release's notes; nothing installed.",
            tool.program
        ));
    }
    let bin = weft.join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let path = bin.join(&tool.program);
    // Replaced rather than written through, so a running one is undisturbed.
    let new = bin.join(format!("{}.new", tool.program));
    std::fs::write(&new, &program).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&new, &path).map_err(|e| e.to_string())?;
    let config = weft.join("config.toml");
    let text = std::fs::read_to_string(&config).unwrap_or_default();
    let text = weft_core::config::set_cloudflared(&text, &path.to_string_lossy())?;
    std::fs::write(&config, text).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The program at the top of a `.tgz`, with the system's `tar`, as
/// `install.sh` unpacks Weft.
fn unpack(archive: &[u8], program: &str, weft: &Path) -> Result<Vec<u8>, String> {
    let work = weft.join(format!(".unpack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let file = work.join("archive.tgz");
    let result = std::fs::write(&file, archive).map_err(|e| e.to_string()).and_then(|()| {
        let ok = std::process::Command::new("tar")
            .args(["-xzf", &file.to_string_lossy(), "-C", &work.to_string_lossy(), program])
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            std::fs::read(work.join(program)).map_err(|e| e.to_string())
        } else {
            Err(format!("The archive holds no {program}."))
        }
    });
    let _ = std::fs::remove_dir_all(&work);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_tool_file_reads() {
        let shipped =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fab7/products/weft/tools");
        let Ok(text) = std::fs::read_to_string(shipped.join("cloudflared.toml")) else {
            return; // a checkout without fab7 beside it
        };
        let tool = Tool::read(&text).unwrap();
        assert_eq!(tool.program, "cloudflared");
        assert_eq!(
            tool.brew.as_deref(),
            Some(&["brew".to_string(), "install".into(), "cloudflared".into()][..])
        );
        assert_eq!(tool.repo.as_deref(), Some("cloudflare/cloudflared"));
        for m in ["darwin-arm64", "darwin-amd64", "linux-amd64", "linux-arm64"] {
            assert!(tool.assets.contains_key(m), "{m}");
        }
    }

    #[test]
    fn a_file_without_brew_or_a_download_offers_neither() {
        let tool = Tool::read("program = \"cloudflared\"\n").unwrap();
        assert_eq!(tool.brew, None);
        assert_eq!(tool.repo, None);
        assert!(tool.assets.is_empty());
        assert_eq!(tool.asset(), None);
    }

    #[test]
    fn a_missing_or_broken_file_says_so() {
        let dir = std::env::temp_dir().join(format!("wr-tool-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tools")).unwrap();
        assert!(Tool::load(&dir).unwrap_err().contains("missing"));
        std::fs::write(dir.join("tools/cloudflared.toml"), "program = [").unwrap();
        assert!(Tool::load(&dir).unwrap_err().contains("cannot be read"));
        std::fs::write(dir.join("tools/cloudflared.toml"), "nothing = 1").unwrap();
        assert!(Tool::load(&dir).unwrap_err().contains("cannot be read"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn this_machine_is_named_as_the_assets_are() {
        let m = machine();
        assert!(
            ["darwin-arm64", "darwin-amd64", "linux-amd64", "linux-arm64"].contains(&m.as_str()),
            "{m}"
        );
    }
}
