//! Weft's own session receipts:
//! `~/.fab7/weft/sessions/<project key>/<harness>/<session id>.jsonl`, one
//! line per change, written by weftd alone from the signals it accepts.
//! Nothing from the conversation: the event, the session, the time, the
//! harness, and the harness process when one was found. Read when a project
//! opens, which is how each pane's state survives a restart.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use weft_core::signal::Sent;

/// A session whose last line is older than this is pruned.
pub const KEEP_MILLIS: i64 = 90 * 24 * 60 * 60 * 1000;

/// Where the receipts live, beside Weft's `config.toml`.
pub fn base_of(config: &Path) -> PathBuf {
    config.with_file_name("sessions")
}

/// One project's receipts.
pub fn project_dir(base: &Path, root: &Path) -> PathBuf {
    base.join(weft_core::sessions::project_key(root))
}

/// Append the line this signal makes, when its event, its process or
/// whether a question is pending differs from the session's last line. A
/// signal with no event keeps the last line's, when it says whether a
/// question is pending; a line keeps the last line's `asking` unless its
/// signal says, and an ended session asks nothing. A harness or session that
/// is not only an id names no file. True when a line was added.
pub fn record(base: &Path, root: &Path, sent: &Sent, now: i64) -> std::io::Result<bool> {
    use weft_core::sessions::is_id;
    if !is_id(&sent.harness) || !is_id(&sent.session) {
        return Ok(false);
    }
    let dir = project_dir(base, root).join(&sent.harness);
    let path = dir.join(format!("{}.jsonl", sent.session));
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let last: Option<Value> = text.lines().last().and_then(|l| serde_json::from_str(l).ok());
    let event = match (sent.state.as_deref(), sent.asking, &last) {
        (Some(event), _, _) => event.to_string(),
        (None, Some(_), Some(l)) if l["event"].is_string() => {
            l["event"].as_str().unwrap_or_default().to_string()
        }
        _ => return Ok(false),
    };
    let was_asking = last.as_ref().is_some_and(|l| l["asking"] == true);
    let asking = event != "ended" && sent.asking.unwrap_or(was_asking);
    let same = last.is_some_and(|l| {
        l["event"] == event.as_str()
            && l["pid"] == json!(sent.pid)
            && l["pid_start"] == json!(sent.pid_start)
            && was_asking == asking
    });
    if same {
        return Ok(false);
    }
    let mut line = json!({"event": event, "session": sent.session,
                          "time": weft_core::turns::stamp(now), "harness": sent.harness});
    if let (Some(pid), Some(start)) = (sent.pid, &sent.pid_start) {
        line["pid"] = json!(pid);
        line["pid_start"] = json!(start);
    }
    if asking {
        line["asking"] = json!(true);
    }
    std::fs::create_dir_all(&dir)?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(format!("{line}\n").as_bytes())?;
    Ok(true)
}

/// Remove every session file whose last line is older than [`KEEP_MILLIS`],
/// and a folder left empty with it.
pub fn prune(base: &Path, now: i64) {
    let dirs = |p: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).collect()
    };
    for project in dirs(base).into_iter().filter(|p| p.is_dir()) {
        for harness in dirs(&project).into_iter().filter(|p| p.is_dir()) {
            for file in dirs(&harness) {
                let text = std::fs::read_to_string(&file).unwrap_or_default();
                let last =
                    text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).next_back();
                let at = last.and_then(|l| weft_core::turns::millis(l["time"].as_str()?));
                // A file with no time read yet may be one being written now.
                if at.is_some_and(|at| now - at > KEEP_MILLIS) {
                    let _ = std::fs::remove_file(&file);
                }
            }
            let _ = std::fs::remove_dir(&harness);
        }
        let _ = std::fs::remove_dir(&project);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(event: &str, pid: Option<u32>) -> Sent {
        Sent {
            harness: "claude-code".into(),
            hook: "Stop".into(),
            session: "s1".into(),
            state: Some(event.into()),
            pid,
            pid_start: pid.map(|p| format!("{p}.000001")),
            prompt: Some("never kept".into()),
            command: Some("never kept".into()),
            workspace: Some("/never/kept".into()),
            ..Sent::default()
        }
    }

    fn lines(base: &Path, root: &Path) -> Vec<Value> {
        let path = project_dir(base, root).join("claude-code/s1.jsonl");
        let text = std::fs::read_to_string(path).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn a_line_is_a_change_and_holds_nothing_from_the_conversation() {
        let base = tempfile::tempdir().unwrap();
        let (base, root) = (base.path(), Path::new("/work/thing"));
        assert!(record(base, root, &sent("working", Some(41)), 1000).unwrap());
        assert!(!record(base, root, &sent("working", Some(41)), 2000).unwrap(), "a repeat");
        assert!(record(base, root, &sent("turn_ended", Some(41)), 3000).unwrap());
        // A resumed session's new process is recorded, though its event is not new.
        assert!(record(base, root, &sent("turn_ended", Some(97)), 4000).unwrap());
        let got = lines(base, root);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0],
            json!({"event": "working", "session": "s1", "time": weft_core::turns::stamp(1000),
                   "harness": "claude-code", "pid": 41, "pid_start": "41.000001"})
        );
        assert_eq!(
            (got[2]["event"].as_str(), got[2]["pid"].as_u64()),
            (Some("turn_ended"), Some(97))
        );
        let text = std::fs::read_to_string(project_dir(base, root).join("claude-code/s1.jsonl"));
        assert!(!text.unwrap().contains("never"));
        let key = weft_core::sessions::project_key(root);
        assert_eq!(key.len(), 12);
        assert!(project_dir(base, root).ends_with(key));
    }

    #[test]
    fn a_pending_question_is_kept_until_a_hook_reads_none_and_an_ended_session_drops_it() {
        let base = tempfile::tempdir().unwrap();
        let (base, root) = (base.path(), Path::new("/work/thing"));
        let asked = |event: Option<&str>, asking: Option<bool>| Sent {
            state: event.map(str::to_string),
            asking,
            ..sent("working", None)
        };
        let reads = |base: &Path| -> Option<weft_core::turns::Turn> {
            let text =
                std::fs::read_to_string(project_dir(base, root).join("claude-code/s1.jsonl"))
                    .ok()?;
            weft_core::turns::read("claude-code", "s1", &text).map(|s| s.latest)
        };
        use weft_core::turns::Turn;
        assert!(record(base, root, &asked(Some("working"), Some(false)), 1000).unwrap());
        // A hook whose state the server's stream gives (none) still records the question.
        assert!(record(base, root, &asked(None, Some(true)), 2000).unwrap());
        assert_eq!(lines(base, root)[1]["event"], "working", "the last event, kept");
        assert_eq!(lines(base, root)[1]["asking"], true);
        assert_eq!(reads(base), Some(Turn::Waiting), "NEEDS YOU while the turn works");
        // The server's turn end says nothing of questions: the question stays.
        assert!(record(base, root, &asked(Some("turn_ended"), None), 3000).unwrap());
        assert_eq!(lines(base, root)[2]["asking"], true);
        assert_eq!(reads(base), Some(Turn::Waiting));
        assert!(
            !record(base, root, &asked(Some("turn_ended"), Some(true)), 4000).unwrap(),
            "a repeat"
        );
        // A hook that reads it answered clears it.
        assert!(record(base, root, &asked(None, Some(false)), 5000).unwrap());
        assert!(lines(base, root)[3].get("asking").is_none());
        assert_eq!(reads(base), Some(Turn::TurnEnded));
        // Asked again, then the session ends: an ended session asks nothing.
        assert!(record(base, root, &asked(None, Some(true)), 6000).unwrap());
        assert!(record(base, root, &asked(Some("ended"), None), 7000).unwrap());
        assert!(lines(base, root)[5].get("asking").is_none());
        assert_eq!(reads(base), Some(Turn::Ended));
        // No line yet and no event: nothing to keep.
        let fresh = tempfile::tempdir().unwrap();
        assert!(!record(fresh.path(), root, &asked(None, Some(true)), 1).unwrap());
    }

    #[test]
    fn no_event_or_an_id_that_is_not_one_names_no_file() {
        let base = tempfile::tempdir().unwrap();
        let (base, root) = (base.path(), Path::new("/work/thing"));
        let mut s = sent("working", None);
        s.state = None;
        assert!(!record(base, root, &s, 1).unwrap());
        let mut s = sent("working", None);
        s.session = "../../escape".into();
        assert!(!record(base, root, &s, 1).unwrap());
        assert!(!base.join(weft_core::sessions::project_key(root)).exists());
        // No process found: the line has none.
        assert!(record(base, root, &sent("ready", None), 1).unwrap());
        assert!(lines(base, root)[0].get("pid").is_none());
    }

    #[test]
    fn a_session_unheard_of_for_ninety_days_is_pruned_with_its_folders() {
        let base = tempfile::tempdir().unwrap();
        let base = base.path();
        let (old, new) = (Path::new("/work/old"), Path::new("/work/new"));
        record(base, old, &sent("ended", None), 0).unwrap();
        record(base, new, &sent("working", None), KEEP_MILLIS).unwrap();
        prune(base, KEEP_MILLIS + 1);
        assert!(!project_dir(base, old).exists(), "file and folders gone");
        assert_eq!(lines(base, new).len(), 1, "within ninety days, kept");
    }
}
