//! Tool files, the adapters for outside programs (remote.md §7a): how to
//! start one and read what it says, and where its installation guide is.
//! Installing it is the person's. The facts are in
//! `~/.fab7/weft/tools/<id>.toml`.

use std::path::Path;

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
    /// Where the person reads how to install it.
    pub guide: Option<String>,
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
        Ok(Tool {
            id: id.into(),
            title: s("title").unwrap_or_else(|| program.clone()),
            kind,
            program,
            run,
            address,
            note: s("note"),
            guide: s("guide"),
        })
    }
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
            tool.guide.as_deref(),
            Some("https://github.com/fab7hq/fab7/blob/main/products/weft/tools/cloudflared.md")
        );
        assert!(shipped.join("cloudflared.md").is_file(), "the guide ships beside it");
    }

    #[test]
    fn the_shipped_tailscale_file_is_a_tunnel_adapter() {
        let shipped =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fab7/products/weft/tools");
        let Ok(text) = std::fs::read_to_string(shipped.join("tailscale.toml")) else {
            return; // a checkout without fab7 beside it
        };
        let tool = Tool::read("tailscale", &text).unwrap();
        assert_eq!((tool.kind.as_str(), tool.title.as_str()), ("tunnel", "Tailscale Funnel"));
        assert_eq!(tool.program, "tailscale");
        assert_eq!(tool.run, ["funnel", "{port}"]);
        assert_eq!(tool.address, ".ts.net");
        assert!(tool.note.is_some());
        assert_eq!(
            tool.guide.as_deref(),
            Some("https://github.com/fab7hq/fab7/blob/main/products/weft/tools/tailscale.md")
        );
        assert!(shipped.join("tailscale.md").is_file(), "the guide ships beside it");
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
}
