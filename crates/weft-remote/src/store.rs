//! What `weft remote` keeps on this machine, in `~/.fab7/weft/remote/`
//! (remote.md §11): the folder 0700, secrets 0600. Weft data only.

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// `~/.fab7/weft`, where `config.toml` and `projects.json` are.
pub fn weft_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    home.join(".fab7").join("weft")
}

/// `remote/` beside `config.toml`, made 0700 if it is not there.
pub fn remote_dir(weft: &Path) -> std::io::Result<PathBuf> {
    let dir = weft.join("remote");
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// Write a file whole, readable by its owner only.
pub fn write_secret(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let part = path.with_extension("part");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&part)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(part, path)
}

/// This machine on Fab7's account service: its id and name, its owner on
/// GitHub, and its own token (`machine.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub github_id: u64,
    pub login: String,
    pub token: String,
}

impl Machine {
    pub fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join("machine.toml")).ok()?;
        let t: toml::Table = text.parse().ok()?;
        let s = |k: &str| t.get(k)?.as_str().map(str::to_string);
        let owner = t.get("owner")?.as_table()?;
        Some(Machine {
            id: s("id")?,
            name: s("name")?,
            token: s("token")?,
            github_id: u64::try_from(owner.get("github_id")?.as_integer()?).ok()?,
            login: owner.get("login")?.as_str()?.to_string(),
        })
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let mut owner = toml::Table::new();
        owner.insert("github_id".into(), toml::Value::Integer(self.github_id as i64));
        owner.insert("login".into(), self.login.clone().into());
        let mut t = toml::Table::new();
        t.insert("id".into(), self.id.clone().into());
        t.insert("name".into(), self.name.clone().into());
        t.insert("token".into(), self.token.clone().into());
        t.insert("owner".into(), toml::Value::Table(owner));
        write_secret(&dir.join("machine.toml"), &t.to_string())
    }
}

/// What this machine is called, for the person's list of machines.
pub fn host_name() -> String {
    let said = std::process::Command::new("hostname").arg("-s").output().ok();
    said.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "this machine".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_is_kept_readable_by_its_owner_only() {
        let dir = std::env::temp_dir().join(format!("weft-remote-store-{}", std::process::id()));
        let remote = remote_dir(&dir).unwrap();
        let m = Machine {
            id: "m_1".into(),
            name: "studio".into(),
            github_id: 42,
            login: "octo".into(),
            token: "secret".into(),
        };
        m.save(&remote).unwrap();
        assert_eq!(Machine::load(&remote), Some(m));
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&remote), 0o700);
        assert_eq!(mode(&remote.join("machine.toml")), 0o600);
        std::fs::remove_dir_all(&dir).ok();
    }
}
