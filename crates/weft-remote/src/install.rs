//! Tool files, the adapters for outside programs, and installing a
//! tunnel's program when the person says yes (remote.md §7a): by
//! Homebrew when it is there, else the adapter's latest GitHub release, as
//! Weft installs itself. The facts are in `~/.fab7/weft/tools/<id>.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// A tool file, as read: an adapter for one outside program.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tool {
    /// Its file's name: what `[remote] tunnel` names.
    pub id: String,
    /// What it is an adapter for: `tunnel`.
    pub kind: String,
    /// The name a person reads.
    pub title: String,
    /// The program's name, found on `PATH`.
    pub program: String,
    /// What follows the program to start it; `{port}` is the loopback port.
    pub run: Vec<String>,
    /// The end of the host its address has.
    pub address: String,
    /// Said wherever it is on: what it can see.
    pub note: Option<String>,
    /// The Homebrew install, when the file names one.
    pub brew: Option<Vec<String>>,
    /// The GitHub repository whose latest release is downloaded.
    pub repo: Option<String>,
    /// Each `<os>-<arch>`'s asset.
    pub assets: BTreeMap<String, String>,
}

impl Tool {
    /// The tunnel adapters in `weft/tools`, by their file's name: the ones
    /// that read, and say they are tunnels.
    pub fn tunnels(weft: &Path) -> Vec<Tool> {
        let mut found: Vec<Tool> = std::fs::read_dir(weft.join("tools"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let id = path.file_stem()?.to_str()?.to_string();
                if path.extension()? != "toml" {
                    return None;
                }
                Tool::read(&id, &std::fs::read_to_string(&path).ok()?).ok()
            })
            .filter(|t| t.kind == "tunnel")
            .collect();
        found.sort_by(|a, b| a.id.cmp(&b.id));
        found
    }

    /// The tunnel adapter `id`, if its file is there and reads.
    pub fn tunnel(weft: &Path, id: &str) -> Option<Tool> {
        Self::tunnels(weft).into_iter().find(|t| t.id == id)
    }

    pub fn read(id: &str, text: &str) -> Result<Self, String> {
        let broken = || format!("Weft's tool file {id}.toml cannot be read: run weft update.");
        let t: toml::Table = text.parse().map_err(|_| broken())?;
        let s = |k: &str| t.get(k).and_then(toml::Value::as_str).map(str::to_string);
        let kind = s("kind").ok_or_else(broken)?;
        let program = s("program").ok_or_else(broken)?;
        let run: Vec<String> = t
            .get("run")
            .and_then(toml::Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let address = s("address").unwrap_or_default();
        if kind == "tunnel" && (run.is_empty() || address.is_empty()) {
            return Err(broken());
        }
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
        Ok(Tool {
            id: id.into(),
            title: s("title").unwrap_or_else(|| program.clone()),
            kind,
            program,
            run,
            address,
            note: s("note"),
            brew,
            repo,
            assets,
        })
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

/// The adapter's latest release, this machine's asset, checked against
/// GitHub's digest and the notes' checksum of the program, written to
/// `weft/bin/<program>` and named in `weft/config.toml` (remote.md §7a).
/// Nothing is written unless every check passes. `api` is GitHub's API.
pub fn download(api: &str, tool: &Tool, weft: &Path) -> Result<PathBuf, String> {
    let asset = tool.asset().ok_or_else(|| {
        format!("{} has no build of {} for {}.", tool.title, tool.program, machine())
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
            "{} does not match the checksum in {repo}'s release notes; nothing installed.",
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
    let text = weft_core::config::set_program(&text, &path.to_string_lossy())?;
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
    fn the_shipped_tool_file_is_a_tunnel_adapter() {
        let shipped =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fab7/products/weft/tools");
        let Ok(text) = std::fs::read_to_string(shipped.join("cloudflared.toml")) else {
            return; // a checkout without fab7 beside it
        };
        let tool = Tool::read("cloudflared", &text).unwrap();
        assert_eq!((tool.kind.as_str(), tool.title.as_str()), ("tunnel", "Cloudflare Tunnel"));
        assert_eq!(tool.program, "cloudflared");
        assert!(tool.run.iter().any(|w| w.contains("{port}")), "{:?}", tool.run);
        assert_eq!(tool.address, ".trycloudflare.com");
        assert!(tool.note.is_some());
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
    fn the_tunnels_are_the_tool_files_that_say_so_and_read() {
        let dir = std::env::temp_dir().join(format!("wr-tool-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tools")).unwrap();
        assert!(Tool::tunnels(&dir).is_empty());
        let tunnel = |title: &str| {
            format!(
                "kind = \"tunnel\"\ntitle = \"{title}\"\nprogram = \"p\"\nrun = [\"{{port}}\"]\naddress = \".x.net\"\n"
            )
        };
        std::fs::write(dir.join("tools/zeta.toml"), tunnel("Zeta")).unwrap();
        std::fs::write(dir.join("tools/alpha.toml"), tunnel("Alpha")).unwrap();
        std::fs::write(dir.join("tools/other.toml"), "kind = \"push\"\nprogram = \"q\"\n").unwrap();
        std::fs::write(dir.join("tools/broken.toml"), "kind = [").unwrap();
        std::fs::write(dir.join("tools/half.toml"), "kind = \"tunnel\"\nprogram = \"p\"\n")
            .unwrap();
        let ids: Vec<_> = Tool::tunnels(&dir).into_iter().map(|t| (t.id, t.title)).collect();
        assert_eq!(
            ids,
            [("alpha".into(), "Alpha".into()), ("zeta".to_string(), "Zeta".to_string())]
        );
        assert_eq!(Tool::tunnel(&dir, "zeta").map(|t| t.address), Some(".x.net".into()));
        assert_eq!(Tool::tunnel(&dir, "half"), None, "a tunnel without run or address is not one");
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
