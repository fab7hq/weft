//! Installing `cloudflared` when the person says yes (ADR-0023, remote.md
//! §7a): by Homebrew when it is there, else Cloudflare's latest release, as
//! Weft installs itself. The facts are in the tool file,
//! `~/.fab7/weft/tools/cloudflared.toml`.

use std::collections::BTreeMap;
use std::path::Path;

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
        let text = std::fs::read_to_string(&path)
            .map_err(|_| "Weft's tool file for cloudflared is missing: run weft update.".to_string())?;
        Self::read(&text)
    }

    pub fn read(text: &str) -> Result<Self, String> {
        let broken = || "Weft's tool file for cloudflared cannot be read: run weft update.".to_string();
        let t: toml::Table = text.parse().map_err(|_| broken())?;
        let program = t.get("program").and_then(toml::Value::as_str).ok_or_else(broken)?.to_string();
        let install = t.get("install").and_then(toml::Value::as_table);
        let brew = install
            .and_then(|i| i.get("brew")?.get("argv")?.as_array().cloned())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>())
            .filter(|a| !a.is_empty());
        let download = install.and_then(|i| i.get("download")?.as_table());
        let repo = download.and_then(|d| d.get("repo")?.as_str()).map(str::to_string);
        let assets = download
            .and_then(|d| d.get("assets")?.as_table())
            .map(|a| a.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_tool_file_reads() {
        let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fab7/products/weft/tools");
        let Ok(text) = std::fs::read_to_string(shipped.join("cloudflared.toml")) else {
            return; // a checkout without fab7 beside it
        };
        let tool = Tool::read(&text).unwrap();
        assert_eq!(tool.program, "cloudflared");
        assert_eq!(tool.brew.as_deref(), Some(&["brew".to_string(), "install".into(), "cloudflared".into()][..]));
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
        assert!(["darwin-arm64", "darwin-amd64", "linux-amd64", "linux-arm64"].contains(&m.as_str()), "{m}");
    }
}
