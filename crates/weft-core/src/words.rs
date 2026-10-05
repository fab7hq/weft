//! Every word both clients show, by a key that does not change.
//!
//! The TUI takes its words from here, and `words.json` — the web app's
//! language pack — is written from this table, so a word changes in one
//! place. A sentence that names a harness or a work says `{harness}` or
//! `{title}`, filled where it is shown.

/// The words, by key.
pub const WORDS: &[(&str, &str)] = &[
    // Where a work stands: the furthest act that has happened.
    ("word.asking", "ASKING"),
    ("word.asked", "ASKED"),
    ("word.evaled", "EVALED"),
    ("word.sealed", "SEALED"),
    ("word.cancelled", "CANCELLED"),
    // An agent's turn, as its hooks reported it.
    ("turn.ready", "ready"),
    ("turn.working", "working"),
    ("turn.waiting", "waiting for you"),
    ("turn.ended", "turn ended"),
    // Why an act cannot run.
    ("why.no_ringframe", "RingFrame is not installed."),
    ("why.not_set_up", "{harness} is not set up for RingFrame. [U]PDATE sets it up."),
    ("why.cannot_ask", "Weft could not ask {harness} whether it is set up."),
];

/// The word for a key. A key the table does not hold is a bug in the caller,
/// shown as the key so it is seen rather than hidden.
pub fn word(key: &str) -> &str {
    WORDS.iter().find(|(k, _)| *k == key).map_or(key, |(_, w)| w)
}

/// A sentence with its `{harness}` filled.
pub fn say(key: &str, harness: &str) -> String {
    word(key).replace("{harness}", harness)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_held_once() {
        let mut keys: Vec<&str> = WORDS.iter().map(|(k, _)| *k).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), WORDS.len());
    }

    #[test]
    fn a_sentence_names_its_harness() {
        assert_eq!(
            say("why.not_set_up", "claude"),
            "claude is not set up for RingFrame. [U]PDATE sets it up."
        );
        assert_eq!(word("turn.waiting"), "waiting for you");
    }
}
