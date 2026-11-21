//! Untracked files as `git diff --no-index /dev/null <file>` writes them, made
//! here rather than by one git process per file: a worktree can hold
//! thousands of files Git does not track.
//!
//! A text file is every line added. Git is still asked, once for all of them,
//! for what it would change: each file's object id, so the `index` line is
//! the one git writes, and whether a filter, an end-of-line rule or a diff
//! driver would alter the text. A file any of those touch, or whose name git
//! would quote, is left to git itself, one process for that file alone.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use crate::evaluate::{EvalError, binary_added, git_bytes, git_input, untracked_is_text};

/// The untracked files that are part of a change: those Git does not ignore,
/// made or changed after `since` (milliseconds since the epoch), when the
/// work began. A file left there before it is not part of this work, and the
/// change does not name it. A file copied in with its old time kept is not
/// seen.
pub(crate) fn listed(root: &Path, since: Option<i64>) -> Result<Vec<String>, EvalError> {
    let after = since.map(|ms| UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64));
    let all = git_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"], &[0])?;
    Ok(String::from_utf8_lossy(&all)
        .split('\0')
        .filter(|n| !n.is_empty())
        .filter(|n| {
            after.is_none_or(|after| {
                std::fs::symlink_metadata(root.join(n))
                    .and_then(|m| m.modified())
                    .is_ok_and(|t| t > after)
            })
        })
        .map(str::to_string)
        .collect())
}

/// The new-file diffs of `names`, paths under `root`, in their order, as
/// `git <config> diff <context> --no-color --no-index /dev/null <name>` writes
/// them: a file not read as text is named, never encoded. `config` is a list
/// of `-c key=value` arguments. With `room`, no further file is written once
/// the output is longer than that.
pub(crate) fn diffs(
    root: &Path,
    names: &[&str],
    config: &[&str],
    context: Option<&str>,
    room: Option<usize>,
) -> Result<Vec<u8>, EvalError> {
    // Each diff is at least as long as its file's text, so the files past the
    // room are known before any is read.
    let mut kinds: Vec<(&str, bool)> = Vec::new();
    let mut at_least = 0usize;
    for &name in names.iter().filter(|n| !n.is_empty()) {
        if room.is_some_and(|r| at_least > r) {
            break;
        }
        let path = root.join(name);
        let text = untracked_is_text(&path);
        at_least += if text {
            std::fs::metadata(&path).map_or(0, |m| m.len() as usize)
        } else {
            binary_added(name).len()
        };
        kinds.push((name, text));
    }
    let plain: Vec<&str> =
        kinds.iter().filter(|(n, text)| *text && !needs_git(root, n)).map(|(n, _)| *n).collect();
    let known = Known::ask(root, config, &plain);
    let mut out = Vec::new();
    for (name, text) in kinds {
        if room.is_some_and(|r| out.len() > r) {
            break;
        }
        if !text {
            out.extend(binary_added(name));
            continue;
        }
        let written = known.as_ref().and_then(|k| {
            let id = k.ids.get(name)?;
            let body = std::fs::read(root.join(name)).ok()?;
            let mode = mode_of(&root.join(name))?;
            Some(new_file(name, mode, &k.zeros, id, &body))
        });
        match written {
            Some(diff) => out.extend(diff),
            None => out.extend(by_git(root, name, config, context)?),
        }
    }
    Ok(out)
}

/// One file, as git writes it.
fn by_git(
    root: &Path,
    name: &str,
    config: &[&str],
    context: Option<&str>,
) -> Result<Vec<u8>, EvalError> {
    let mut args: Vec<&str> = config.to_vec();
    args.push("diff");
    args.extend(context);
    args.extend(["--no-color", "--no-index", "/dev/null", name]);
    git_bytes(root, &args, &[0, 1])
}

/// A name git quotes, or a link, is not written here.
fn needs_git(root: &Path, name: &str) -> bool {
    name.bytes().any(|b| b < 0x20 || b == 0x7f || b == b'"' || b == b'\\' || b >= 0x80)
        || std::fs::symlink_metadata(root.join(name)).is_ok_and(|m| m.file_type().is_symlink())
}

fn mode_of(path: &Path) -> Option<&'static str> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode();
    Some(if mode & 0o100 != 0 { "100755" } else { "100644" })
}

/// A new text file: every line added.
pub(crate) fn new_file(name: &str, mode: &str, zeros: &str, id: &str, body: &[u8]) -> Vec<u8> {
    let mut out =
        format!("diff --git a/{name} b/{name}\nnew file mode {mode}\nindex {zeros}..{id}\n")
            .into_bytes();
    if body.is_empty() {
        return out;
    }
    let lines: Vec<&[u8]> = body.split_inclusive(|&b| b == b'\n').collect();
    let tab = if name.contains(' ') { "\t" } else { "" };
    let count = if lines.len() == 1 { String::new() } else { format!(",{}", lines.len()) };
    out.extend(format!("--- /dev/null\n+++ b/{name}{tab}\n@@ -0,0 +1{count} @@\n").bytes());
    for line in &lines {
        out.push(b'+');
        out.extend_from_slice(line);
    }
    if !body.ends_with(b"\n") {
        out.extend_from_slice(b"\n\\ No newline at end of file\n");
    }
    out
}

/// What git says of the files written here: each one's abbreviated object
/// id, for the files git would write as they are.
struct Known {
    zeros: String,
    ids: BTreeMap<String, String>,
}

impl Known {
    /// None when git cannot say, and every file is then git's to write.
    fn ask(root: &Path, config: &[&str], names: &[&str]) -> Option<Known> {
        if names.is_empty() {
            return Some(Known { zeros: String::new(), ids: BTreeMap::new() });
        }
        let run = |args: &[&str], input: &[u8]| -> Option<Vec<u8>> {
            let mut all: Vec<&str> = config.to_vec();
            all.extend(args);
            git_input(root, &all, input)
        };
        // Settings that change every file's header or body.
        let settings = run(&["config", "-z", "--get-regexp", "^diff\\."], b"").unwrap_or_default();
        let mut drivers: BTreeSet<String> = BTreeSet::new();
        for entry in settings.split(|&b| b == 0) {
            let entry = String::from_utf8_lossy(entry);
            let key = entry.split('\n').next().unwrap_or("").to_lowercase();
            if ["diff.noprefix", "diff.mnemonicprefix", "diff.srcprefix", "diff.dstprefix"]
                .contains(&key.as_str())
                || key == "diff.external"
            {
                return None;
            }
            if let Some(driver) = [".textconv", ".binary", ".command"]
                .iter()
                .find_map(|end| key.strip_prefix("diff.")?.strip_suffix(end))
            {
                drivers.insert(driver.to_string());
            }
        }
        let paths: Vec<u8> = names.iter().flat_map(|n| n.bytes().chain(*b"\n")).collect();
        let ids = |filters: bool| -> Option<Vec<String>> {
            let args: &[&str] = if filters {
                &["hash-object", "--stdin-paths"]
            } else {
                &["hash-object", "--no-filters", "--stdin-paths"]
            };
            let out = String::from_utf8(run(args, &paths)?).ok()?;
            let ids: Vec<String> = out.lines().map(str::to_string).collect();
            (ids.len() == names.len()).then_some(ids)
        };
        let (filtered, raw) = (ids(true)?, ids(false)?);
        let listed: Vec<u8> = names.iter().flat_map(|n| n.bytes().chain([0])).collect();
        let attrs = run(&["check-attr", "-z", "--stdin", "diff"], &listed)?;
        let attrs: Vec<&[u8]> = attrs.split(|&b| b == 0).collect();
        let driver: BTreeMap<String, String> = attrs
            .chunks(3)
            .filter(|c| c.len() == 3)
            .map(|c| (String::from_utf8_lossy(c[0]).into(), String::from_utf8_lossy(c[2]).into()))
            .collect();
        let zeros = String::from_utf8(run(&["rev-parse", "--short", &"0".repeat(40)], b"")?)
            .ok()?
            .trim()
            .to_string();
        let short = zeros.len();
        if !(4..=40).contains(&short) {
            return None;
        }
        // An id whose prefix names another object is made longer by git.
        let prefixes: Vec<u8> =
            filtered.iter().flat_map(|id| id[..short].bytes().chain(*b"\n")).collect();
        let found = String::from_utf8(run(&["cat-file", "--batch-check"], &prefixes)?).ok()?;
        let found: Vec<&str> = found.lines().collect();
        if found.len() != names.len() {
            return None;
        }
        let mut out = BTreeMap::new();
        for (i, name) in names.iter().enumerate() {
            let attr = driver.get(*name).map_or("unspecified", String::as_str);
            let plain_diff = matches!(attr, "unspecified" | "set") || !drivers.contains(attr);
            let unique = found[i].ends_with(" missing") || found[i].starts_with(&filtered[i]);
            if filtered[i] == raw[i] && attr != "unset" && plain_diff && unique {
                out.insert(name.to_string(), filtered[i][..short].to_string());
            }
        }
        Some(Known { zeros, ids: out })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    const DRIVERS: [&str; 2] =
        ["diff.ringframe-top.xfuncname=^\\b$", "diff.ringframe-toml.xfuncname=^\\[[^]]+\\]"];

    fn each_by_git(root: &Path, names: &[&str], config: &[&str], context: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for name in names {
            if untracked_is_text(&root.join(name)) {
                out.extend(by_git(root, name, config, Some(context)).unwrap());
            } else {
                out.extend(binary_added(name));
            }
        }
        out
    }

    /// Written here, every untracked text file reads byte for byte as git
    /// writes it, in both context modes and with the function-name drivers:
    /// one line or several, no final newline, an empty file, a name with a
    /// space, an executable, a CRLF file under an end-of-line rule, and one a
    /// diff driver rewrites.
    #[test]
    fn untracked_text_reads_as_git_writes_it() {
        let repo = repo();
        let root = repo.path();
        let files: [(&str, &[u8]); 10] = [
            ("one.txt", b"only\n"),
            ("two.rs", b"fn a() {}\n\n#[cfg(test)]\nmod tests {}\n"),
            ("nonl.md", b"# Title\nno newline"),
            ("empty.txt", b""),
            ("sp ace.txt", b"a b\n"),
            ("run.sh", b"#!/bin/sh\necho hi\n"),
            ("crlf.txt", b"x\r\ny\r\n"),
            ("conv.up", b"abc\n"),
            ("bin.dat", b"a\0b"),
            ("nested/deep.toml", b"[a]\nb = 1\n"),
        ];
        for (name, body) in files {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::write(root.join(".git/info/attributes"), "*.txt eol=lf\n*.up diff=up\n").unwrap();
        let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
        let attrs =
            format!("core.attributesFile={}", crate::evidence::function_names_file().display());
        let with_drivers = [
            "-c",
            attrs.as_str(),
            "-c",
            DRIVERS[0],
            "-c",
            DRIVERS[1],
            "-c",
            "diff.up.textconv=tr a-z A-Z <",
        ];
        for config in [&with_drivers[..], &with_drivers[..6]] {
            for context in ["-U1", "-U0"] {
                let ours = diffs(root, &names, config, Some(context), None).unwrap();
                let git = each_by_git(root, &names, config, context);
                assert_eq!(String::from_utf8_lossy(&ours), String::from_utf8_lossy(&git));
            }
        }
        // Git is left only the files it would alter: the CRLF one and, with
        // its driver set, the one a diff driver rewrites.
        let text: Vec<&str> =
            names.iter().copied().filter(|n| untracked_is_text(&root.join(n))).collect();
        let here = |config: &[&str]| -> Vec<String> {
            Known::ask(root, config, &text).unwrap().ids.into_keys().collect()
        };
        let plain = ["empty.txt", "nested/deep.toml", "nonl.md", "one.txt", "run.sh", "sp ace.txt"];
        assert_eq!(here(&with_drivers), [&plain[..], &["two.rs"]].concat());
        assert_eq!(here(&with_drivers[..6]), [&["conv.up"], &plain[..], &["two.rs"]].concat());
        let ours = String::from_utf8(diffs(root, &names, &[], None, None).unwrap()).unwrap();
        assert!(ours.contains("\n\\ No newline at end of file\n"), "{ours}");
        assert!(ours.contains("Binary files /dev/null and b/bin.dat differ"));
    }

    /// The new-file hunk itself: a count only past one line.
    #[test]
    fn a_new_file_is_every_line_added() {
        let one = String::from_utf8(new_file("a", "100644", "0000000", "1234567", b"x\n")).unwrap();
        assert!(one.ends_with("@@ -0,0 +1 @@\n+x\n"), "{one}");
        let two =
            String::from_utf8(new_file("a", "100644", "0000000", "1234567", b"x\ny")).unwrap();
        assert!(two.ends_with("@@ -0,0 +1,2 @@\n+x\n+y\n\\ No newline at end of file\n"), "{two}");
        let none = String::from_utf8(new_file("a", "100644", "0000000", "e69de29", b"")).unwrap();
        assert_eq!(none, "diff --git a/a b/a\nnew file mode 100644\nindex 0000000..e69de29\n");
    }

    /// Past the room, no further file is written.
    #[test]
    fn the_room_stops_the_files() {
        let repo = repo();
        let root = repo.path();
        let names: Vec<String> = (0..10).map(|i| format!("f{i}.txt")).collect();
        for n in &names {
            std::fs::write(root.join(n), "x".repeat(100) + "\n").unwrap();
        }
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let out = String::from_utf8(diffs(root, &names, &[], None, Some(250)).unwrap()).unwrap();
        assert_eq!(out.matches("diff --git").count(), 2, "the second passes 250 bytes");
    }
}
