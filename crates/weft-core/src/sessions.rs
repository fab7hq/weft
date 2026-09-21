//! The harness sessions RingFrame has a record of, in this workspace.
//!
//! Spec: `plans/weft/spec/readiness.md`. A harness owns its own session and
//! both supported harnesses can open one again by id. Weft owns neither, so it
//! does not keep one: it reads the id out of the receipt RingFrame's hook
//! already wrote, under `<project>/.fab7/rf/sessions/<harness>/<id>/`.
//!
//! Weft reads that directory and never writes it (ADR-0001). A session with no
//! receipt is a session Weft cannot name, and it says so rather than guessing.


/// A session a harness recorded a prompt in, here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub id: String,
    /// The last prompt's timestamp, exactly as the receipt spells it.
    pub at: String,
    /// The first line of that prompt, so the person can see what they would be
    /// reopening rather than being shown an opaque id.
    pub last: String,
}



pub fn first_line(prompt: &str) -> String {
    prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// `2026-09-20T07:27:14.769Z` reads as `07:27`. The date is not shown: a
/// session you would reopen is one you remember starting.
pub fn clock(at: &str) -> String {
    at.split('T').nth(1).map(|t| t.chars().take(5).collect()).unwrap_or_default()
}
