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
    // The title bar and the tree.
    ("title.open", "open"),
    ("title.needs_you", "needs you"),
    ("tree.not_running", "not running"),
    ("tree.nothing_asked", "Nothing asked for yet."),
    ("tree.no_ringframe", "No ringframe on PATH, so there is no record to read."),
    ("tree.install_ringframe", "The agents still run here. Install ringframe to see the work."),
    (
        "tree.runs_either_way",
        "Your agent runs here either way. Nothing is written down until it is set up.",
    ),
    ("tree.written_down", "Nothing is written down until it is set up."),
    ("start.title", "Start an agent"),
    ("start.fresh", "{harness} · start fresh"),
    ("start.none", "No harness is set up for RingFrame yet. [U]PDATE sets one up."),
    // The acts, as a bar or a button names them.
    ("act.ask", "Ask"),
    ("act.eval", "Eval"),
    ("act.seal", "Seal"),
    ("act.proceed", "Proceed"),
    ("act.follow_up", "Follow up"),
    ("act.eval_view", "Eval view"),
    ("act.cancel", "Cancel"),
    ("act.send", "Send"),
    ("act.send_anyway", "Send anyway"),
    ("act.resume", "Resume"),
    ("act.start", "Start"),
    ("act.close", "Close"),
    // Asking, and the bars that ask before Weft types.
    ("ask.title", "What do you want done?"),
    ("ask.send_to", "SEND TO"),
    ("bar.will_type", "Weft will type  {command}  into {harness}."),
    ("ask.which_approach", "The agent will ask you which approach to take, in its own pane."),
    ("not_ready.title", "The agent has not said it is ready"),
    (
        "not_ready.why",
        "Weft types on its own only into an agent that has said it is ready. If you can see it is, say so and Weft will type.",
    ),
    ("not_ready.working", "the agent is working"),
    ("not_ready.asking", "the agent is asking you something"),
    ("not_ready.unknown", "Weft can't tell whether the agent is ready"),
    ("not_typed.why", "Weft did not type it: {why}."),
    (
        "not_typed.mode_not_entered",
        "Weft did not type it: the agent never showed the command as on.",
    ),
    ("not_typed.no_process", "Weft did not type it: that agent is not running."),
    ("not_typed.in_flight", "Weft did not type it: it is still typing the last one."),
    ("resume.title", "{harness} is not running"),
    ("resume.session", "Resume the session this was asked in"),
    ("resume.either_way", "Your work is in the record either way."),
    ("resume.nothing", "Nothing to resume: the session this was asked in is not on record."),
    ("resume.fresh", "[N] starts a fresh {harness}."),
    ("agent.ended", "{harness} has ended. Its work is still on the list."),
    // The Eval view.
    ("eval.met", "met"),
    ("eval.not_met", "not met"),
    ("eval.unsettled", "unsettled"),
    ("eval.judging", "judging"),
    ("eval.so_far", "{state}, so far"),
    ("eval.judge", "JUDGE"),
    ("eval_view.by_step", "By step"),
    ("eval_view.by_file", "By file"),
    ("eval_view.a_step", "A step"),
    ("eval_view.findings", "Findings"),
    ("eval_view.unexplained", "Unexplained changes"),
    ("eval_view.checks", "Checks RingFrame ran"),
    ("eval_view.limitations", "Limitations"),
    ("eval_view.found", "Found"),
    ("eval_view.readings", "Readings"),
    ("eval_view.changes", "Changes ({n})"),
    // Why an act cannot run.
    ("why.no_ringframe", "RingFrame is not installed."),
    ("why.not_set_up", "{harness} is not set up for RingFrame."),
    ("why.cannot_ask", "Weft could not ask {harness} whether it is set up."),
    ("why.start_an_agent", "Start an agent first — [N]EW AGENT."),
    ("why.nothing_asked", "Nothing has been asked for yet."),
    ("why.nothing_to_work_on", "Nothing to work on yet."),
    ("why.no_pane", "No {harness} pane is open, so there is nowhere to send this."),
    ("why.closed", "{title} is closed. [F] FOLLOW UP asks again."),
    (
        "why.composing",
        "{title} is still being composed in {harness}. There is nothing to send yet.",
    ),
    ("why.nothing_to_send", "{title} has nothing to send. [F] FOLLOW UP asks for more."),
    ("why.no_eval", "No Eval of this work yet: [E]VAL starts one."),
    ("why.nothing_needs_you", "Nothing needs your input."),
    // The web app's own wording of the same things: a screen, not keys
    // (the owner, 2026-10-05), the same on a phone and a desktop.
    ("web.turn.waiting", "waiting"),
    ("web.tree.needs_you", "● needs you"),
    ("web.tree.ask_for_something", "Ask for something and Weft writes it down."),
    ("web.why.start_an_agent", "Start an agent first — on {machine}."),
    ("web.why.set_up_on", "Set it up in Weft on {machine}: U."),
    ("web.act.keys", "Keys"),
    ("web.agent.back", "Back to the tree"),
    ("web.bar.into", "into {harness}"),
    ("web.bar.answered", "Answered on the machine"),
    ("web.detail.as_typed", "the prompt as typed"),
    ("web.detail.debate_in", "Eval on the tree runs the debate in {harness}."),
    ("web.detail.title", "Details"),
    ("web.eval.findings", "what RingFrame noticed beside the steps"),
    ("web.eval.live", "updates as the judges record"),
    ("web.eval.quoted", "Lines a judge quoted are amber bold."),
    ("web.eval.click", "Click a row to open it."),
    ("web.link.connecting", "Connecting"),
    ("web.link.connected", "Connected"),
    ("web.link.reconnecting", "Reconnecting"),
    ("web.link.asking", "Asking"),
    ("web.link.refused", "Refused"),
    ("web.machines.title", "Your machines"),
    ("web.settings.title", "Settings"),
    ("web.add.title", "Add a machine"),
    ("web.add.signed_in", "Signed in as {login}"),
    ("web.add.none", "No machine is on your account yet. On the machine where Weft runs:"),
    ("web.add.menu", "Press W in Weft for its menu."),
    ("web.add.remote", "Press R for Remote, then Enter to turn it on."),
    ("web.add.code", "Enter the code it shows at github.com/login/device."),
    ("web.add.joins", "Your machine appears here as soon as it joins."),
    ("web.allow.phone", "Allow this phone on your machine"),
    ("web.allow.browser", "Allow this browser on your machine"),
    ("web.allow.go", "Go to {machine} and press {key}."),
    ("web.allow.waits_phone", "It waits two minutes. Nothing on this phone can answer it."),
    ("web.allow.waits_browser", "It waits two minutes. Nothing here can answer it."),
    ("web.refused.title", "Not allowed"),
    ("web.refused.denied", "Your machine said no to this device."),
    ("web.refused.expired", "Nobody answered on your machine in two minutes."),
    ("web.refused.not_owner", "This machine belongs to another account."),
    ("web.refused.busy", "Your machine was asked too often. Try again in a few minutes."),
    ("web.refused.other", "Your machine refused this device."),
    ("web.start.runs_on", "It runs on {machine}, with your own settings."),
    ("web.start.pick_one", "PICK ONE"),
    (
        "web.start.none",
        "No harness is set up for RingFrame yet. [U]PDATE on {machine} sets one up.",
    ),
];

/// The word for a key. A key the table does not hold is a bug in the caller,
/// shown as the key so it is seen rather than hidden.
pub fn word(key: &str) -> &str {
    WORDS.iter().find(|(k, _)| *k == key).map_or(key, |(_, w)| w)
}

/// A sentence with its `{harness}` filled.
pub fn say(key: &str, harness: &str) -> String {
    fill(key, &[("harness", harness)])
}

/// A sentence with each `{name}` it holds filled.
pub fn fill(key: &str, with: &[(&str, &str)]) -> String {
    with.iter()
        .fold(word(key).to_string(), |s, (name, value)| s.replace(&format!("{{{name}}}"), value))
}

/// The table as `words.json`, the web app's language pack: one object, key
/// to word.
pub fn json() -> String {
    let map: serde_json::Map<String, serde_json::Value> =
        WORDS.iter().map(|(k, w)| (k.to_string(), (*w).into())).collect();
    serde_json::to_string_pretty(&map).unwrap_or_default() + "\n"
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
            fill("why.composing", &[("title", "telemetry"), ("harness", "claude")]),
            "telemetry is still being composed in claude. There is nothing to send yet."
        );
        assert_eq!(say("why.not_set_up", "claude"), "claude is not set up for RingFrame.");
        assert_eq!(word("turn.waiting"), "waiting for you");
    }
}
