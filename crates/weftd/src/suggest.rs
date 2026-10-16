//! The suggestion index, kept on the person's machine:
//! `~/.fab7/weft/suggest/<project key>.json`, readable only by them.
//!
//! It holds what the person sent: each Ask's own words (`source.txt`, never
//! the compiled prompt) and their messages in each session's transcript, each
//! with the agent's reply before it. It is derived: missing or of an old
//! version, it is built again from those sources, and it goes when its project
//! is dismissed. Nothing in it is sent anywhere.

use std::path::{Path, PathBuf};

use weft_core::conversation::Conversation;
use weft_core::ledger::Unit;
use weft_core::suggest::{Entry, Stored};

/// Where the indexes live, beside Weft's `config.toml`.
pub fn dir(config: &Path) -> PathBuf {
    config.with_file_name("suggest")
}

/// One project's index.
pub fn file(config: &Path, root: &Path) -> PathBuf {
    dir(config).join(format!("{}.json", weft_core::sessions::project_key(root)))
}

/// The project's kept entries, unless there are none or they are of an old version.
pub fn load(config: &Path, root: &Path) -> Option<Vec<Entry>> {
    let text = std::fs::read_to_string(file(config, root)).ok()?;
    Stored::from_json(&text, &root.to_string_lossy()).map(|s| s.entries)
}

/// Keep the project's entries, readable only by the person: the folder 0700,
/// the file 0600, written whole and then put in place.
pub fn save(config: &Path, root: &Path, entries: &[Entry]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let folder = dir(config);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder)?;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
    let path = file(config, root);
    let part = path.with_extension("part");
    let text = Stored::new(&root.to_string_lossy(), entries.to_vec()).to_json();
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&part)?;
    f.write_all(text.as_bytes())?;
    std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(part, path)
}

/// Forget the project's index.
pub fn forget(config: &Path, root: &Path) {
    let _ = std::fs::remove_file(file(config, root));
}

/// What the project's Asks give: each one's own words, when it was asked.
pub fn of_asks(root: &Path, units: &[Unit]) -> Vec<Entry> {
    let asks = root.join(".fab7/rf/asks");
    units
        .iter()
        .filter(|u| {
            !u.ask_id.is_empty() && u.ask_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .filter_map(|u| {
            let words = std::fs::read_to_string(asks.join(&u.ask_id).join("source.txt")).ok()?;
            let words = words.trim();
            (!words.is_empty()).then(|| Entry {
                text: words.to_string(),
                reply: String::new(),
                harness: u.harness.clone(),
                project: root.to_string_lossy().into_owned(),
                at: weft_core::turns::millis(&u.asked_at).map_or(0, |ms| (ms / 1000).max(0) as u64),
                source: format!("ask:{}", u.ask_id),
                near: Vec::new(),
            })
        })
        .collect()
}

/// A session's transcript, read whole; nothing when it cannot be found or read.
pub fn items_of(
    mapping: &Conversation,
    home: &Path,
    session: &str,
    named: Option<&Path>,
) -> Option<Vec<weft_core::conversation::Item>> {
    let path = crate::conversation::file(mapping, home, session, named)?;
    let bytes = std::fs::read(path).ok()?;
    mapping.read(&bytes, 0).ok().map(|r| r.items)
}

/// What one session's transcript gives, read whole; nothing when it cannot be found or read.
pub fn of_session(
    mapping: &Conversation,
    home: &Path,
    harness: &str,
    session: &str,
    named: Option<&Path>,
    root: &Path,
    at: u64,
) -> Vec<Entry> {
    let items = items_of(mapping, home, session, named).unwrap_or_default();
    weft_core::suggest::from_conversation(&items, harness, &root.to_string_lossy(), session, at)
}

/// The short messages every kept index holds, for the habits every project shares.
pub fn habits(config: &Path) -> Vec<Entry> {
    std::fs::read_dir(dir(config))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| {
            serde_json::from_str::<Stored>(&t).ok().filter(|s| s.version == Stored::VERSION)
        })
        .flat_map(|s| s.entries)
        .filter(|e| e.text.trim().chars().count() <= SHORT)
        .collect()
}

/// A habit is this short at most: longer messages stay in their own project.
const SHORT: usize = 80;

/// The index a project's suggestions come from: its own entries, and every
/// other project's short ones (`others`) for the habits.
pub fn index_of(own: &[Entry], others: &[Entry], root: &Path) -> weft_core::suggest::Index {
    let project = root.to_string_lossy();
    let all: Vec<Entry> =
        own.iter().chain(others.iter().filter(|e| e.project != project)).cloned().collect();
    weft_core::suggest::Index::new(&all, &project)
}

/// What `pane.suggest` answers from a project's index: the rest of `draft`,
/// or with none typed the next prompt after `reply` (and Sense's embedding of it).
pub fn ask(
    index: &weft_core::suggest::Index,
    harness: &str,
    draft: &str,
    reply: (&str, &[f32]),
    now: u64,
) -> Option<weft_core::suggest::Suggestion> {
    if draft.is_empty() {
        let near = (!reply.1.is_empty()).then_some(reply.1);
        index.next(reply.0, near, harness, now)
    } else {
        index.complete(draft, harness, now)
    }
}

/// [`ask`] of a fresh [`index_of`].
pub fn answer(
    own: &[Entry],
    others: &[Entry],
    root: &Path,
    harness: &str,
    draft: &str,
    reply: (&str, &[f32]),
    now: u64,
) -> Option<weft_core::suggest::Suggestion> {
    ask(&index_of(own, others, root), harness, draft, reply, now)
}

/// The whole index of a project, built again from its sources: every Ask in
/// its record and every session its receipts know, each read whole.
pub fn build(config: &Path, root: &Path, harnesses: &weft_core::harness::Harnesses) -> Vec<Entry> {
    use crate::harness::OnThisMachine;
    let mut ledger = crate::ledger::Ledger::at(root);
    ledger.refresh();
    let mut entries = of_asks(root, &ledger.units());
    let mut turns = crate::turns::Turns::default();
    turns.refresh(&crate::receipts::project_dir(&crate::receipts::base_of(config), root));
    for heard in turns.heard() {
        let s = heard.session;
        let Some(h) = harnesses.find(&s.harness) else { continue };
        let Some(mapping) = h.conversation.as_ref() else { continue };
        let at = (s.first / 1000).max(0) as u64;
        entries.extend(of_session(mapping, &h.config_home(), &s.harness, &s.id, None, root, at));
    }
    entries
}

/// Put `fresh` in place of the entries read from `source` (`ask:` for every Ask).
pub fn replace(entries: &mut Vec<Entry>, source: &str, fresh: Vec<Entry>) {
    let all_asks = source == "ask:";
    entries.retain(|e| if all_asks { !e.source.starts_with("ask:") } else { e.source != source });
    entries.extend(fresh);
}

/// An entry, as its embedding is matched back to it.
pub type Key = (String, String, u64);

fn key(e: &Entry) -> Key {
    (e.source.clone(), e.text.clone(), e.at)
}

/// The replies not embedded yet, each with its entry's key.
pub fn unembedded(entries: &[Entry]) -> Vec<(Key, String)> {
    entries
        .iter()
        .filter(|e| e.near.is_empty() && !e.reply.trim().is_empty())
        .map(|e| (key(e), e.reply.clone()))
        .collect()
}

/// Give each entry its reply's embedding; whether any was given.
pub fn embedded(entries: &mut [Entry], vectors: Vec<(Key, Vec<f32>)>) -> bool {
    let mut any = false;
    for (k, v) in vectors {
        for e in entries.iter_mut().filter(|e| e.near.is_empty() && key(e) == k) {
            e.near = v.clone();
            any = true;
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use weft_core::harness::fixture::file as harness_file;

    const GROK: &str = include_str!("../../weft-core/tests/fixtures/conversation/grok.jsonl");

    fn entry(text: &str, source: &str) -> Entry {
        Entry {
            text: text.into(),
            reply: String::new(),
            harness: "claude".into(),
            project: "/work/weft".into(),
            at: 1,
            source: source.into(),
            near: Vec::new(),
        }
    }

    #[test]
    fn the_index_is_kept_under_suggest_readable_only_by_the_person_and_forgotten() {
        let weft = tempfile::tempdir().unwrap();
        let config = weft.path().join("config.toml");
        let root = Path::new("/work/weft");
        assert_eq!(load(&config, root), None, "nothing kept yet");
        let kept = vec![entry("run the tests", "session:s1")];
        save(&config, root, &kept).unwrap();
        assert_eq!(load(&config, root), Some(kept));
        assert_eq!(load(&config, Path::new("/work/docs")), None, "another project's is its own");
        let path = file(&config, root);
        assert!(path.starts_with(weft.path().join("suggest")));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(dir(&config)).unwrap().permissions().mode() & 0o777, 0o700);
        // Nothing else is written: no part file is left beside it.
        let names: Vec<_> =
            std::fs::read_dir(weft.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("suggest")]);
        assert_eq!(std::fs::read_dir(dir(&config)).unwrap().count(), 1);
        forget(&config, root);
        assert!(!path.exists());
        assert_eq!(load(&config, root), None);
    }

    #[test]
    fn an_index_of_an_old_version_is_not_loaded() {
        let weft = tempfile::tempdir().unwrap();
        let config = weft.path().join("config.toml");
        let root = Path::new("/work/weft");
        save(&config, root, &[entry("run the tests", "session:s1")]).unwrap();
        let path = file(&config, root);
        let now = format!("\"version\":{}", Stored::VERSION);
        let old = std::fs::read_to_string(&path).unwrap().replace(&now, "\"version\":1");
        std::fs::write(&path, old).unwrap();
        assert_eq!(load(&config, root), None);
    }

    #[test]
    fn each_ask_gives_its_own_words_and_not_its_compiled_prompt() {
        let root = tempfile::tempdir().unwrap();
        let asks = root.path().join(".fab7/rf/asks");
        for (id, words) in [("ask_1", "Show the build number on /health"), ("ask_2", "  ")] {
            std::fs::create_dir_all(asks.join(id)).unwrap();
            std::fs::write(asks.join(id).join("source.txt"), words).unwrap();
            std::fs::write(asks.join(id).join("prompt.txt"), "COMPILED").unwrap();
        }
        let unit = |id: &str| Unit {
            ask_id: id.into(),
            harness: "codex".into(),
            asked_at: "2026-10-17T01:00:00Z".into(),
            ..Default::default()
        };
        let got =
            of_asks(root.path(), &[unit("ask_1"), unit("ask_2"), unit("ask_3"), unit("../x")]);
        assert_eq!(
            got.len(),
            1,
            "an empty Ask, a missing one and an id that is not one give nothing"
        );
        assert_eq!(got[0].text, "Show the build number on /health");
        assert_eq!((got[0].harness.as_str(), got[0].source.as_str()), ("codex", "ask:ask_1"));
        assert_eq!(
            got[0].at,
            weft_core::turns::millis("2026-10-17T01:00:00Z").unwrap() as u64 / 1000
        );
    }

    #[test]
    fn a_session_gives_the_persons_messages_from_its_whole_transcript() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions/%2Fw/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("updates.jsonl"), GROK).unwrap();
        let grok = Conversation::of(&harness_file("grok")["conversation"]).unwrap();
        let got = of_session(&grok, home.path(), "grok", "s1", None, Path::new("/w"), 5);
        assert!(!got.is_empty());
        assert!(
            got.iter()
                .all(|e| e.harness == "grok" && e.source == "session:s1" && e.project == "/w")
        );
        assert!(
            got.iter().skip(1).any(|e| !e.reply.is_empty()),
            "a later message has the reply before it"
        );
        assert!(of_session(&grok, home.path(), "grok", "s2", None, Path::new("/w"), 5).is_empty());
    }

    #[test]
    fn one_source_is_replaced_and_every_ask_together() {
        let mut all =
            vec![entry("a", "session:s1"), entry("b", "session:s2"), entry("c", "ask:ask_1")];
        replace(&mut all, "session:s1", vec![entry("a2", "session:s1")]);
        replace(&mut all, "ask:", vec![entry("d", "ask:ask_2")]);
        let texts: Vec<_> = all.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["b", "a2", "d"]);
    }

    #[test]
    fn replies_are_embedded_once_and_matched_back_to_their_entries() {
        let mut all =
            vec![entry("a", "session:s1"), entry("b", "session:s1"), entry("c", "ask:ask_1")];
        all[0].reply = "Done.".into();
        all[1].reply = "Failed.".into();
        let todo = unembedded(&all);
        assert_eq!(
            todo.iter().map(|(_, r)| r.as_str()).collect::<Vec<_>>(),
            ["Done.", "Failed."],
            "an Ask has no reply to embed"
        );
        let vectors = todo.into_iter().map(|(k, r)| (k, vec![r.len() as f32])).collect();
        assert!(embedded(&mut all, vectors));
        assert_eq!((all[0].near.clone(), all[1].near.clone()), (vec![5.0], vec![7.0]));
        assert!(unembedded(&all).is_empty());
        assert!(!embedded(&mut all, vec![(("session:s9".into(), "x".into(), 1), vec![1.0])]));
    }

    #[test]
    fn the_answer_completes_a_draft_or_gives_the_next_prompt_and_habits_come_from_every_index() {
        let root = Path::new("/work/weft");
        let mut done = entry("commit these changes", "session:s1");
        done.reply = "Done. All the tests pass.".into();
        let own = [done, entry("run the login tests", "session:s1")];
        let got = answer(&own, &[], root, "claude", "run the", ("", &[]), 10).unwrap();
        assert_eq!(got.text, " login tests");
        let next =
            answer(&own, &[], root, "claude", "", ("Done. The tests pass.", &[]), 10).unwrap();
        assert_eq!(next.text, "commit these changes");
        assert_eq!(answer(&own, &[], root, "claude", "", ("", &[]), 10), None, "no reply yet");

        // Another project's habit: sent in two projects.
        let other =
            |p: &str| Entry { project: p.into(), ..entry("open a pull request", "session:o") };
        let weft = tempfile::tempdir().unwrap();
        let config = weft.path().join("config.toml");
        save(&config, Path::new("/work/a"), &[other("/work/a")]).unwrap();
        save(
            &config,
            Path::new("/work/b"),
            &[
                other("/work/b"),
                Entry { project: "/work/b".into(), ..entry(&"long ".repeat(30), "session:o") },
            ],
        )
        .unwrap();
        let shared = habits(&config);
        assert_eq!(shared.len(), 2, "only the short ones: {shared:?}");
        assert_eq!(
            answer(&own, &shared, root, "claude", "open a", ("", &[]), 10).unwrap().text,
            " pull request"
        );
    }
}
