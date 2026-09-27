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

/// A time reads as its hour and minute, as the receipts count them (UTC).
/// The date is not shown: a session you would reopen is one you remember
/// starting.
pub fn clock(at: i64) -> String {
    let minutes = at.div_euclid(60_000).rem_euclid(24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}
