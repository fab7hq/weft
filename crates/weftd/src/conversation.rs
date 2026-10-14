//! An agent's conversation, read from its harness's own transcript only when
//! a device asks for it. weftd keeps the file's path per pane in memory, and
//! nothing it reads: each read opens the file, reads only the bytes it
//! needs, and turns them into items with `weft_core::conversation`.
//!
//! A file is read only when its canonical path is inside the harness's
//! configuration home and it is the pane's session's: named
//! `<session>.jsonl`, or in a folder named for the session. A hook's payload
//! is the harness's, but anything the agent runs can send a signal with the
//! pane's token, so a forged path can point nowhere else.

use serde_json::{Value, json};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use weft_core::conversation::{Conversation, Cursor, Item};

/// Most items in a page.
pub const PAGE: usize = 50;

/// How much more of the file a page reads each time it needs more.
const CHUNK: u64 = 64 << 10;

/// The pane's transcript, when one is known or found and allowed: the path a
/// hook named, else the newest file `find` gives for the session.
pub fn file(c: &Conversation, home: &Path, session: &str, named: Option<&Path>) -> Option<PathBuf> {
    let path = match named {
        Some(p) => p.to_path_buf(),
        None => {
            let pattern = c.find.as_deref()?.replace("{session}", session);
            newest(home, &pattern)?
        }
    };
    let path = path.canonicalize().ok()?;
    let home = home.canonicalize().ok()?;
    let inside = path.strip_prefix(&home).ok()?;
    let named_for = inside.file_name()?.to_str()?.ends_with(&format!("{session}.jsonl"))
        || inside.parent()?.components().any(|c| c.as_os_str() == session);
    (named_for && path.is_file()).then_some(path)
}

/// The newest file under `home` matching a pattern whose parts may hold `*`.
fn newest(home: &Path, pattern: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = pattern.split('/').filter(|p| !p.is_empty()).collect();
    let mut found = vec![home.to_path_buf()];
    for (n, part) in parts.iter().enumerate() {
        let last = n + 1 == parts.len();
        found = found
            .iter()
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(|name| matches(part, name)))
            .filter(|e| e.file_type().is_ok_and(|t| if last { t.is_file() } else { t.is_dir() }))
            .map(|e| e.path())
            .collect();
    }
    let modified = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    found.into_iter().max_by_key(modified)
}

/// Whether a name matches a part of a pattern, where `*` is any run of characters.
fn matches(part: &str, name: &str) -> bool {
    let mut pieces = part.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else { return false };
    let pieces: Vec<&str> = pieces.collect();
    for (n, piece) in pieces.iter().enumerate() {
        if n + 1 == pieces.len() {
            return rest.ends_with(piece);
        }
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    rest.is_empty()
}

/// What a read gives a device.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// A page: its items, newest last, and the cursors before and after it.
    Page { items: Vec<Item>, before: Option<Cursor>, after: Cursor },
    /// What was added since a cursor, and the next one.
    Grown { items: Vec<Item>, after: Cursor },
    /// Nothing was added.
    Same,
    /// The file shrank or was replaced: read the newest page again.
    Reset,
    /// No file, or one that is not lines of JSON.
    Unreadable,
}

impl Answer {
    /// As `read conversation` answers it.
    pub fn json(&self) -> Value {
        match self {
            Answer::Page { items, before, after } => json!({"conversation": {
                "readable": true, "items": items,
                "before": before.map(Cursor::text), "after": after.text()}}),
            Answer::Grown { items, after } => {
                json!({"conversation": {"readable": true, "items": items, "after": after.text()}})
            }
            Answer::Same => json!({"conversation": {"readable": true, "items": []}}),
            Answer::Reset => json!({"conversation": {"reset": true}}),
            Answer::Unreadable => json!({"conversation": {"readable": false, "items": []}}),
        }
    }
}

/// Which file this is, so a replaced one is told apart.
fn identity(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

/// The newest page, or the page that ends where `before` starts.
pub fn page(c: &Conversation, path: Option<&Path>, before: Option<Cursor>) -> Answer {
    let Some(mut f) = path.and_then(|p| std::fs::File::open(p).ok()) else {
        return Answer::Unreadable;
    };
    let Ok(meta) = f.metadata() else { return Answer::Unreadable };
    let file = identity(&meta);
    if before.is_some_and(|b| b.file != file || b.at > meta.len()) {
        return Answer::Reset;
    }
    // The end of the last whole line: an unfinished one is read when it is.
    let Some(whole) = last_line_end(&mut f, meta.len()) else { return Answer::Unreadable };
    let end = before.map_or(whole, |b| b.at);
    // Read back a chunk at a time, keeping the bytes of a line cut at the
    // chunk's start for the next, until the page is full or the file starts.
    let (mut lo, mut cut, mut items) = (end, Vec::new(), Vec::new());
    while lo > 0 && items.len() < PAGE {
        let from = (lo - cut.len() as u64).saturating_sub(CHUNK);
        let mut bytes = vec![0; (lo - cut.len() as u64 - from) as usize];
        if f.seek(SeekFrom::Start(from)).and_then(|_| f.read_exact(&mut bytes)).is_err() {
            return Answer::Unreadable;
        }
        bytes.extend_from_slice(&cut);
        let start = match (from, bytes.iter().position(|&b| b == b'\n')) {
            (0, _) => 0,
            (_, Some(n)) => n + 1,
            // One line longer than what was read: read further back.
            (_, None) => {
                cut = bytes;
                continue;
            }
        };
        let Ok(read) = c.read(&bytes[start..], from + start as u64) else {
            return Answer::Unreadable;
        };
        items.splice(0..0, read.items);
        lo = from + start as u64;
        bytes.truncate(start);
        cut = bytes;
    }
    // At most a page, of whole lines: a line's items are never split.
    let line = |i: &Item| Cursor::line(&i.id).unwrap_or(0);
    let mut first = items.len().saturating_sub(PAGE);
    if first > 0 {
        let cut_line = line(&items[first]);
        if line(&items[first - 1]) == cut_line {
            first = items.iter().position(|i| line(i) > cut_line).unwrap_or(first);
            if first == items.len() {
                first = items.iter().position(|i| line(i) == cut_line).unwrap_or(0);
            }
        }
    }
    let items = items.split_off(first);
    let before = if first == 0 && lo == 0 {
        None
    } else {
        Some(Cursor { file, at: items.first().map_or(lo, line) })
    };
    Answer::Page { items, before, after: Cursor { file, at: whole } }
}

/// What was added after a cursor.
pub fn after(c: &Conversation, path: Option<&Path>, after: Cursor) -> Answer {
    let Some(mut f) = path.and_then(|p| std::fs::File::open(p).ok()) else {
        return Answer::Unreadable;
    };
    let Ok(meta) = f.metadata() else { return Answer::Unreadable };
    if identity(&meta) != after.file || meta.len() < after.at {
        return Answer::Reset;
    }
    if meta.len() == after.at {
        return Answer::Same;
    }
    let mut bytes = Vec::new();
    if f.seek(SeekFrom::Start(after.at)).and_then(|_| f.read_to_end(&mut bytes)).is_err() {
        return Answer::Unreadable;
    }
    match c.read(&bytes, after.at) {
        Err(_) => Answer::Unreadable,
        Ok(read) if read.end == 0 => Answer::Same,
        Ok(read) => Answer::Grown {
            items: read.items,
            after: Cursor { file: after.file, at: after.at + read.end as u64 },
        },
    }
}

/// Where the file's last whole line ends.
fn last_line_end(f: &mut std::fs::File, len: u64) -> Option<u64> {
    let mut end = len;
    let mut byte = [0u8; 4096];
    while end > 0 {
        let from = end.saturating_sub(byte.len() as u64);
        let n = (end - from) as usize;
        f.seek(SeekFrom::Start(from)).ok()?;
        f.read_exact(&mut byte[..n]).ok()?;
        if let Some(i) = byte[..n].iter().rposition(|&b| b == b'\n') {
            return Some(from + i as u64 + 1);
        }
        end = from;
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use weft_core::conversation::Kind;
    use weft_core::harness::fixture::file as harness_file;

    fn grok() -> Conversation {
        Conversation::of(&harness_file("grok")["conversation"]).unwrap()
    }

    const FIXTURE: &str = include_str!("../../weft-core/tests/fixtures/conversation/grok.jsonl");

    /// A configuration home holding one session's transcript, as Grok keeps it.
    fn home(session: &str, text: &str) -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions/%2Fw").join(session);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("updates.jsonl"), text).unwrap();
        let path = dir.join("updates.jsonl");
        (home, path)
    }

    #[test]
    fn the_file_is_the_sessions_inside_the_configuration_home() {
        let (h, path) = home("s1", FIXTURE);
        let found = file(&grok(), h.path(), "s1", None).expect("found by find");
        assert_eq!(found, path.canonicalize().unwrap());
        assert_eq!(file(&grok(), h.path(), "s1", Some(&path)), Some(found.clone()));
        assert_eq!(file(&grok(), h.path(), "s2", Some(&path)), None, "another session's file");
        assert_eq!(file(&grok(), h.path(), "s2", None), None);
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().join("s1/updates.jsonl");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, FIXTURE).unwrap();
        assert_eq!(file(&grok(), h.path(), "s1", Some(&elsewhere)), None, "outside the home");
        let link = h.path().join("sessions/%2Fw/s1/link.jsonl");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        assert_eq!(file(&grok(), h.path(), "s1", Some(&link)), None, "a link out of the home");
        assert_eq!(file(&grok(), h.path(), "s1", path.parent()), None, "a folder");
    }

    #[test]
    fn a_pattern_part_matches_any_run_for_a_star() {
        assert!(matches("rollout-*-abc.jsonl", "rollout-2026-10-14T01-02-03-abc.jsonl"));
        assert!(!matches("rollout-*-abc.jsonl", "rollout-x-abd.jsonl"));
        assert!(matches("*", "anything"));
        assert!(matches("abc.jsonl", "abc.jsonl") && !matches("abc.jsonl", "xabc.jsonl"));
    }

    /// Many short lines: pages from the end hold every item once, in order,
    /// and none reads more than it needs.
    #[test]
    fn pages_from_the_end_cover_every_item_once() {
        let line = |n: usize| {
            json!({"timestamp": n, "params": {"update": {"sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": format!("reply {n}")}}}})
        };
        let noise = json!({"timestamp": 1, "params": {"update": {"sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "x".repeat(70_000)}}}});
        let text: String =
            (0..130)
                .map(|n| {
                    if n == 60 {
                        format!("{noise}\n{}\n", line(n))
                    } else {
                        format!("{}\n", line(n))
                    }
                })
                .collect();
        let (_h, path) = home("s1", &format!("{text}{{\"unfinished"));
        let mut seen = Vec::new();
        let mut before = None;
        loop {
            let Answer::Page { items, before: b, after } = page(&grok(), Some(&path), before)
            else {
                panic!()
            };
            assert!(items.len() <= PAGE);
            assert_eq!(after.at as usize, text.len(), "the unfinished line is not read");
            seen.splice(0..0, items.into_iter().map(|i| i.text.unwrap()));
            match b {
                Some(b) => before = Some(b),
                None => break,
            }
        }
        assert_eq!(seen, (0..130).map(|n| format!("reply {n}")).collect::<Vec<_>>());
    }

    #[test]
    fn after_gives_only_new_lines_and_a_replaced_or_shrunk_file_resets() {
        let lines: Vec<&str> = FIXTURE.lines().collect();
        let (_h, path) = home("s1", &format!("{}\n", lines[..10].join("\n")));
        let Answer::Page { after: from, .. } = page(&grok(), Some(&path), None) else { panic!() };
        assert_eq!(after(&grok(), Some(&path), from), Answer::Same, "the length has not changed");
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        let Answer::Grown { items, after: next } = after(&grok(), Some(&path), from) else {
            panic!()
        };
        assert!(items.iter().any(|i| i.kind == Kind::Step));
        let whole = page(&grok(), Some(&path), None);
        let Answer::Page { items: all, .. } = &whole else { panic!() };
        assert_eq!(all[all.len() - items.len()..], items[..], "the same items and ids");
        assert_eq!(after(&grok(), Some(&path), next), Answer::Same);
        std::fs::write(&path, "{}\n").unwrap();
        assert_eq!(after(&grok(), Some(&path), next), Answer::Reset, "shrunk");
        let other = path.with_file_name("other.jsonl");
        std::fs::write(&other, FIXTURE).unwrap();
        std::fs::rename(&other, &path).unwrap();
        assert_eq!(after(&grok(), Some(&path), from), Answer::Reset, "replaced");
        assert_eq!(page(&grok(), Some(&path), Some(from)), Answer::Reset);
    }

    #[test]
    fn no_file_or_one_that_is_not_json_is_unreadable() {
        assert_eq!(page(&grok(), None, None), Answer::Unreadable);
        let (_h, path) = home("s1", "not json\n");
        assert_eq!(page(&grok(), Some(&path), None), Answer::Unreadable);
    }
}
