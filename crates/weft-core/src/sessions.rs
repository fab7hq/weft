//! The harness sessions RingFrame has a record of, in this workspace.
//!
//! A harness owns its own session and both supported harnesses can open one
//! again by id. Weft owns neither, so it does not keep one: it reads the id
//! out of the receipt RingFrame's hook already wrote, under
//! `<project>/.fab7/rf/sessions/<harness>/<id>/`.
//!
//! Weft reads that directory and never writes it. A session with no receipt is
//! a session Weft cannot name, and it says so rather than guessing.

/// A session a harness recorded a prompt in, here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub id: String,
    /// When it was last used, in milliseconds since the epoch.
    pub at: i64,
    /// The first line of that prompt, so the person can see what they would be
    /// reopening rather than being shown an opaque id.
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
