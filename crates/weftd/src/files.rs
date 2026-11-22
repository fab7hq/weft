//! The files a conversation points at, found on the disk, and located for a
//! device that opens one.
//!
//! Weft takes the files the agent returns: a path a message names or a step
//! wrote is shown wherever it is, since what an agent may touch is its
//! harness's permission. A device may read only a file its pane's
//! conversation pointed at, never a path of its own choosing.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use weft_core::conversation::Item;
use weft_core::files::{FileKind, Ref, kind_of, sniffed};

/// Where relative and `~/` paths start.
pub struct Base<'a> {
    pub root: &'a Path,
    pub home: Option<&'a Path>,
}

impl Base<'_> {
    /// The canonical file a named path is, when it is a regular file.
    pub fn locate(&self, named: &str) -> Option<PathBuf> {
        let path = match named.strip_prefix("~/") {
            Some(rest) => self.home?.join(rest),
            None if Path::new(named).is_absolute() => PathBuf::from(named),
            None => self.root.join(named),
        };
        let found = path.canonicalize().ok()?;
        found.is_file().then_some(found)
    }

    /// A file as a device names it: relative inside the project, else absolute.
    pub fn shown(&self, file: &Path) -> String {
        let root = self.root.canonicalize().unwrap_or_else(|_| self.root.to_path_buf());
        file.strip_prefix(&root).unwrap_or(file).to_string_lossy().into_owned()
    }
}

/// A file's kind, its first bytes looked at when it says it is text.
fn kind_on_disk(file: &Path) -> Option<FileKind> {
    let kind = kind_of(&file.to_string_lossy())?;
    if kind != FileKind::Text {
        return Some(kind);
    }
    let mut first = [0u8; 512];
    let n = std::fs::File::open(file).and_then(|mut f| f.read(&mut first)).unwrap_or(0);
    Some(sniffed(kind, &first[..n]))
}

fn millis(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// The paths each agent message names, found on the disk, as its refs, each
/// once per item; a path that is no regular file gives none. Every file found
/// joins `seen`, the files the pane's devices may open.
pub fn refs(items: &mut [Item], base: &Base, seen: &mut HashSet<PathBuf>) {
    let mut looked: HashMap<String, Option<(PathBuf, Ref)>> = HashMap::new();
    for item in items {
        for named in std::mem::take(&mut item.mentions) {
            let found = looked
                .entry(named.clone())
                .or_insert_with(|| {
                    let file = base.locate(&named)?;
                    let kind = kind_on_disk(&file)?;
                    let meta = std::fs::metadata(&file).ok()?;
                    let shown = base.shown(&file);
                    let r = Ref { path: shown, kind, size: meta.len(), modified: millis(&meta) };
                    Some((file, r))
                })
                .clone();
            let Some((file, r)) = found else { continue };
            if item.refs.iter().any(|h| h.path == r.path) {
                continue;
            }
            seen.insert(file);
            item.refs.push(r);
        }
    }
}

/// A file the pane's conversation pointed at, where it is on the machine:
/// what a device needs to fetch it. `None` when it is not one of those files
/// or is no longer a file.
pub fn located(base: &Base, seen: &HashSet<PathBuf>, named: &str) -> Option<Value> {
    let file = base.locate(named).filter(|f| seen.contains(f))?;
    let meta = std::fs::metadata(&file).ok()?;
    let kind = kind_on_disk(&file).unwrap_or(FileKind::Other);
    Some(json!({"file": {
        "path": base.shown(&file), "abs": file.to_string_lossy(), "kind": kind,
        "size": meta.len(), "modified": millis(&meta)}}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use weft_core::conversation::Kind;

    fn item(mentions: &[&str]) -> Item {
        Item {
            id: "0.0".into(),
            kind: Kind::Agent,
            at: None,
            text: None,
            tool: None,
            arg: None,
            call: None,
            failed: None,
            refs: Vec::new(),
            mentions: mentions.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn paths(item: &Item) -> Vec<&str> {
        item.refs.iter().map(|r| r.path.as_str()).collect()
    }

    /// A project, a folder outside it, and a home.
    fn places() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("project");
        let outside = t.path().join("tmp");
        let home = t.path().join("home");
        for d in [root.join("out"), root.join("src"), outside.clone(), home.join("Downloads")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(root.join("out/report.md"), "# Q3\n").unwrap();
        std::fs::write(root.join("src/q3.py"), "print(1)\n").unwrap();
        std::fs::write(root.join("out/blob.txt"), b"\x00\x01binary").unwrap();
        std::fs::write(outside.join("board.pdf"), b"%PDF-1.7").unwrap();
        std::fs::write(home.join("Downloads/q2.xlsx"), b"PK").unwrap();
        (t, root, outside, home)
    }

    #[test]
    fn named_files_show_wherever_they_are_and_nothing_else_does() {
        let (_t, root, outside, home) = places();
        let base = Base { root: &root, home: Some(&home) };
        let board = outside.join("board.pdf").to_string_lossy().into_owned();
        let link = root.join("out/link.pdf");
        std::os::unix::fs::symlink(outside.join("board.pdf"), &link).unwrap();
        let mut items = [
            item(&[
                "out/report.md",
                "src/q3.py",
                &board,
                "~/Downloads/q2.xlsx",
                "out/report.md",
                "out/gone.md",
                "out",
            ]),
            item(&["out/link.pdf"]),
        ];
        let mut seen = HashSet::new();
        refs(&mut items, &base, &mut seen);
        let board_canon = outside.join("board.pdf").canonicalize().unwrap();
        let q2 = home.join("Downloads/q2.xlsx").canonicalize().unwrap();
        assert_eq!(
            paths(&items[0]),
            ["out/report.md", "src/q3.py", board_canon.to_str().unwrap(), q2.to_str().unwrap()]
        );
        assert_eq!(paths(&items[1]), [board_canon.to_str().unwrap()], "a link is its target");
        assert!(items.iter().all(|i| i.mentions.is_empty()), "mentions are used up");
        assert_eq!(items[0].refs[0].kind, FileKind::Markdown);
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn a_text_file_holding_binary_is_other() {
        let (_t, root, _, _) = places();
        let base = Base { root: &root, home: None };
        let mut items = [item(&["out/blob.txt"])];
        refs(&mut items, &base, &mut HashSet::new());
        assert_eq!(items[0].refs[0].kind, FileKind::Other);
    }

    #[test]
    fn only_a_file_pointed_at_is_located_however_large() {
        let (_t, root, _, _) = places();
        let base = Base { root: &root, home: None };
        let f = std::fs::File::create(root.join("out/huge.pdf")).unwrap();
        f.set_len(3 << 30).unwrap();
        assert_eq!(located(&base, &HashSet::new(), "out/huge.pdf"), None, "not pointed at");
        let mut items = [item(&["out/huge.pdf"])];
        let mut seen = HashSet::new();
        refs(&mut items, &base, &mut seen);
        let v = located(&base, &seen, "out/huge.pdf").unwrap();
        let canon = root.join("out/huge.pdf").canonicalize().unwrap();
        assert_eq!(v["file"]["path"], "out/huge.pdf");
        assert_eq!(v["file"]["abs"], canon.to_str().unwrap());
        assert_eq!(
            (v["file"]["kind"].as_str(), v["file"]["size"].as_u64()),
            (Some("pdf"), Some(3 << 30))
        );
        std::fs::remove_file(root.join("out/huge.pdf")).unwrap();
        assert_eq!(located(&base, &seen, "out/huge.pdf"), None, "gone");
    }
}
