//! The harness sessions Weft has a receipt for, in a project.
//!
//! A harness owns its own session and can open one again by id. Weft keeps
//! only the id, the events its hooks signalled and the harness process, in
//! its own receipts (ADR-0030 D7): `~/.fab7/weft/sessions/<project key>/
//! <harness>/<id>.jsonl`. A session with no receipt is a session Weft cannot
//! name, and it says so rather than guessing.

/// A project's key: the first 12 hex digits of the SHA-256 of its path.
pub fn project_key(path: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// Whether a session id is only an id: letters, digits, `.`, `_` and `-`,
/// not starting with `-`. The ids come from a project's files (a receipt
/// directory's name, a ledger's `session_ref`), which a cloned repository can
/// write; anything else could become words of its own on the resume command
/// line, a flag among them.
pub fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('-')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A session a harness ran here, as Weft's receipts recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub id: String,
    /// When it was last used, in milliseconds since the epoch.
    pub at: i64,
    /// The first line of its last prompt, when one is known. Weft's receipts
    /// keep nothing from the conversation, so it is empty from them.
    pub last: String,
}

pub fn first_line(prompt: &str) -> String {
    prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// When a session was last used, from its turn receipts and its last
/// prompt's time: `None` when it never held a conversation (it only said it
/// was `ready`), so there is nothing to resume.
pub fn last_used(turns: Option<&crate::turns::Session>, prompted: Option<i64>) -> Option<i64> {
    let turned =
        turns.filter(|t| t.latest != crate::turns::Turn::Ready || t.first < t.at).map(|t| t.at);
    prompted.max(turned)
}

/// A time reads as its hour and minute, as the receipts count them (UTC).
/// The date is not shown: a session you would reopen is one you remember
/// starting.
pub fn clock(at: i64) -> String {
    let minutes = at.div_euclid(60_000).rem_euclid(24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turns::{Session, Turn};

    fn turns(first: i64, latest: Turn, at: i64) -> Session {
        Session { harness: "codex".into(), id: "s".into(), first, latest, at }
    }

    #[test]
    fn a_session_was_used_once_it_held_a_conversation() {
        assert_eq!(last_used(None, None), None);
        assert_eq!(last_used(Some(&turns(5, Turn::Ready, 5)), None), None, "only ready");
        assert_eq!(last_used(Some(&turns(5, Turn::TurnEnded, 9)), None), Some(9));
        assert_eq!(last_used(Some(&turns(5, Turn::Ready, 5)), Some(7)), Some(7), "a prompt");
        assert_eq!(last_used(Some(&turns(5, Turn::Working, 6)), Some(7)), Some(7), "the later");
    }
}
