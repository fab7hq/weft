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
    ("turn.waiting", "needs you"),
    ("turn.turn_ended", "turn ended"),
    // The title bar and the tree.
    ("title.open", "open"),
    ("title.needs_you", "needs you"),
    ("tree.nothing_asked", "Nothing asked for yet."),
    // AGENTS and WORK (T11.1).
    ("tree.agents", "AGENTS"),
    ("tree.work", "WORK"),
    ("tree.counts", "{n} running · {m} ended"),
    ("tree.no_ask", "no Ask yet"),
    ("tree.earlier", "{n} earlier sessions"),
    ("tree.earlier_one", "1 earlier session"),
    ("tree.match", "{n} match"),
    ("tree.search_placeholder", "Search agents and works"),
    ("tree.search_none", "Nothing matches {q}."),
    ("web.search.clear", "Clear the search"),
    ("tree.open", "Open"),
    ("tree.since", "since Eval {n}"),
    ("tree.eval", "Eval {n}"),
    ("tree.judging", "judging…"),
    ("tree.not_sealed", "not sealed"),
    ("tree.sealed", "Sealed"),
    ("tree.seals", "{n} seals"),
    ("tree.seals_one", "1 seal"),
    ("tree.seal", "Seal {n}"),
    ("tree.by_round", "by Eval round"),
    ("state.running", "running"),
    ("state.ended", "ended"),
    ("state.last_active", "last active {time}"),
    ("verdict.aligned", "aligned"),
    ("verdict.drifted", "drifted"),
    ("verdict.incomplete", "not enough proof"),
    // An act pressed on a row it does not act on.
    ("select.agent", "Select an agent to Ask."),
    ("select.open", "Select Open to Eval its Asks."),
    ("select.eval", "Select an Eval to Seal it."),
    // A project RingFrame is not set up in yet: what it still needs.
    ("tree.not_set_up", "RingFrame is not set up here yet."),
    ("tree.has_ringframe", "ringframe is installed"),
    ("tree.ringframe_how", "run Weft's installer again"),
    ("tree.has_git", "a Git repository"),
    ("tree.git_how", "git init"),
    ("tree.has_commit", "a first commit"),
    ("tree.commit_how", "commit once"),
    ("tree.sets_it_up", "Weft sets it up as soon as these hold."),
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
    (
        "not_typed.control_characters",
        "Weft did not type it: it holds control characters a terminal would act on.",
    ),
    ("not_typed.in_flight", "Weft did not type it: it is still typing the last one."),
    // A send is proven by what the harness reported it received.
    (
        "not_typed.paste_not_shown",
        "Weft did not press Enter: the agent's screen did not change when the Ask was pasted. The text may be in its composer.",
    ),
    (
        "not_typed.not_the_ask",
        "The agent received something other than the Ask, so the Ask was not delivered. You can stop the agent in Terminal.",
    ),
    (
        "not_typed.no_receipt",
        "Weft can't tell whether the Ask arrived: the agent reported no prompt in 10 seconds.",
    ),
    (
        "send.no_prompt",
        "Weft does not send an Ask to {harness}: its harness file does not say where the prompt it received is, so Weft could not tell whether the Ask arrived.",
    ),
    // Resuming a session by its id (T11.5).
    ("resume.how", "Weft starts {harness} again on this session, in a new pane."),
    ("resume.quiet", "Weft can't tell whether it still runs: it was last active {time}."),
    ("resume.either_way", "Your work is in the record either way."),
    ("resume.nothing", "Nothing to resume: this session never held a conversation."),
    ("earlier.title", "Earlier sessions"),
    ("earlier.today", "TODAY"),
    ("earlier.yesterday", "YESTERDAY"),
    ("earlier.week", "THIS WEEK"),
    ("earlier.earlier", "EARLIER"),
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
    // An act on a round: `{title}` is the round, "Eval 2".
    ("why.sealed_already", "{title} is sealed already."),
    ("why.ended", "{harness} has ended: resume this session to Ask it."),
    ("why.elsewhere", "{harness} runs in another program: Weft can't show it or type into it."),
    ("why.quiet", "Weft can't tell whether this {harness} session runs: resume it to Ask it."),
    ("why.judging", "{title} is still being judged."),
    ("why.split_needs_two", "A split needs two agents running."),
    ("act.split", "Split"),
    ("act.one_pane", "One pane"),
    ("hint.tabs", "Tab, 1–9: every open project's agents."),
    ("hint.split", "Ctrl+] moves the keys between the list and each side. [B] shows the list."),
    ("why.nothing_needs_you", "Nothing needs your input."),
    // The web app's own wording of the same things: a screen, not keys, the
    // same on a phone and a desktop.
    ("web.turn.waiting", "waiting"),
    ("web.tree.needs_you", "● needs you"),
    ("web.tree.ask_for_something", "Ask for something and Weft writes it down."),
    ("web.why.start_an_agent", "Start an agent first — on {machine}."),
    ("web.why.set_up_on", "Set it up in Weft on {machine}: U."),
    ("web.act.keys", "Keys"),
    ("web.agent.back", "Back to the tree"),
    ("web.back.tree", "weft"),
    ("web.agents", "Agents"),
    ("web.agent.message", "Message {harness}…"),
    ("web.why.not_connected", "Not connected to the machine."),
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
    ("web.machines.tunnel", "reached through its tunnel"),
    ("web.machines.no_address", "has not reported an address yet"),
    ("web.machines.remove", "Remove"),
    ("web.machines.remove_sure", "Remove {machine} from your account?"),
    (
        "web.machines.remove_what",
        "{machine} leaves this list. Weft on {machine} is not touched; it joins again with W → R.",
    ),
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
    (
        "web.refused.version",
        "This page and Weft on your machine are different versions. Reload this page; if it stays, update Weft on your machine.",
    ),
    // Remote that survives (Weft Phase 6.9, cards 13.1–13.6).
    ("web.unreachable.waiting", "Waiting for {machine} to answer."),
    ("web.unreachable.heard", "Last heard from it at {time}."),
    ("web.unreachable.never", "Not heard from it yet."),
    ("web.refused.update_title", "Update Weft on {machine}"),
    (
        "web.refused.update",
        "{machine} runs an older Weft than this page. On {machine}, run weft update, then try again.",
    ),
    ("web.refused.reload_title", "Reload this page"),
    ("web.refused.reload", "Weft on {machine} is newer than this page."),
    ("web.refused.signed_out_title", "Sign in again on {machine}"),
    (
        "web.refused.signed_out",
        "{machine} is not signed in to Weft. On {machine}, press W, then R, and enter the code it shows at github.com/login/device.",
    ),
    ("web.try_again", "Try again"),
    ("web.reload", "Reload"),
    ("web.reopened", "reopened"),
    (
        "web.reopened.notice",
        "Weft started again on {machine} at {time}, and these agents ended. Resume one, or dismiss this project.",
    ),
    ("web.reopened.dismiss", "Dismiss"),
    ("web.reopened.ended", "This session ended at {time}, when Weft stopped on {machine}."),
    ("web.update.waiting", "Weft {version} is waiting on {machine}."),
    ("web.update.running_one", "It starts when no agent is running there: 1 is running."),
    ("web.update.running", "It starts when no agent is running there: {n} are running."),
    ("web.settings.back", "Back"),
    ("web.settings.this_device", "This device"),
    ("web.settings.name", "Name, as your machines show it"),
    ("web.settings.notifications", "Notifications from {machine}"),
    (
        "web.settings.notifications_when",
        "When an agent needs you and you are away from your machine",
    ),
    ("web.settings.machines", "Machines"),
    ("web.settings.connected", "connected"),
    ("web.settings.account", "Account · {login}"),
    ("web.settings.sign_out", "Sign out"),
    ("web.settings.sign_out_everywhere", "Sign out everywhere"),
    ("web.settings.forget", "Forget this device"),
    ("web.settings.delete", "Delete my account"),
    // Each act that leaves asks first: the question, then what it does.
    ("web.settings.sign_out_ask", "Sign out of Weft here?"),
    (
        "web.settings.sign_out_what",
        "You sign out on this device. It stays allowed on your machines, so signing in again opens them.",
    ),
    ("web.settings.everywhere_ask", "Sign out everywhere?"),
    (
        "web.settings.everywhere_what",
        "Every phone and browser signed in as {login} signs out. Each stays allowed on your machines.",
    ),
    ("web.settings.forget_ask", "Forget this device?"),
    (
        "web.settings.forget_what",
        "This device drops its name, its tokens and its notifications. The next time it opens a machine, the machine asks you to allow it again.",
    ),
    ("web.settings.delete_ask", "Delete your account?"),
    (
        "web.settings.delete_what",
        "Your account, its sign-ins and its machines leave Weft's service, on every device. Weft on your machines is not touched; projects, setup, turbo and Remote stay there.",
    ),
    (
        "web.settings.stays",
        "Projects, setup, turbo and Remote stay on your machine. Signing out keeps this device allowed on your machines; remove it there with W → R.",
    ),
    ("web.settings.privacy", "Privacy"),
    ("web.menu.settings", "Settings…"),
    // The agent's pane (M3.6, M3.7): whose history, and the message box's own name.
    ("web.agent.message_box", "Message"),
    ("web.agent.nothing_scrolled", "Nothing of {harness}'s has scrolled away yet."),
    (
        "web.agent.own_history",
        "{harness} keeps its own history: press {key} in the agent to read it.",
    ),
    // The header's pill while a project starts agents in turbo mode (M2.9).
    ("web.turbo", "turbo"),
    ("web.push.home_screen", "Add Weft to your Home Screen first"),
    (
        "web.push.home_screen_how",
        "On iPhone and iPad a notification comes only to the app on the Home Screen: tap Share, then Add to Home Screen, then open Weft from there and turn notifications on.",
    ),
    ("web.push.unsupported", "This browser does not take pushes."),
    ("web.push.connect_first", "Connect to the machine first."),
    ("web.push.denied", "Notifications are turned off for Weft in this browser."),
    ("web.push.failed", "The browser could not subscribe to pushes."),
    // The web's own wording of agents and rounds (M/D11.x).
    ("web.bar.selected", "selected: {what}"),
    ("web.bar.open_round", "Open round"),
    ("web.detail.not_evaluated", "not evaluated yet"),
    ("web.detail.its_session", "Its session"),
    ("web.detail.covers", "covers {n} Asks"),
    ("web.detail.covers_one", "covers 1 Ask"),
    ("web.eval.judged", "judged {time}"),
    ("web.eval.verdict", "Verdict"),
    ("web.eval.agreement", "Agreement"),
    ("web.eval.judged_by", "Judged by"),
    ("web.eval.asks", "The Asks it covers"),
    ("web.eval.sealed_in", "Sealed in Seal {n}"),
    (
        "web.eval.seal_what",
        "Seal records your decision on this Eval and the {n} Asks it covers. It does not make the work right.",
    ),
    ("web.seal.sealed", "sealed {time}"),
    ("web.seal.decision", "Decision"),
    ("web.seal.when", "When"),
    ("web.seal.eval", "The Eval it seals"),
    ("web.seal.receipt", "Receipt"),
    ("web.seal.asks", "The Asks it covered"),
    ("web.seal.holds", "The receipt still matches what RingFrame wrote."),
    ("web.seal.broken", "THE RECEIPT NO LONGER HOLDS"),
    ("web.seal.final", "A Seal records what was decided. It does not make the work right."),
    ("web.session.asks", "{n} Asks made in it"),
    ("web.session.asks_one", "1 Ask made in it"),
    ("web.session.state", "State"),
    ("web.session.harness", "Harness"),
    (
        "web.session.resume_how",
        "Resume starts {harness} again on session {session} in a new agent.",
    ),
    ("web.session.resume_joins", "Resume joins {harness} on session {session} in a new agent."),
    ("web.session.resume_joins_here", "Resume joins {harness} on this session in a new agent."),
    ("web.session.started", "Started"),
    ("web.session.session", "Session"),
    ("web.session.project", "Project"),
    ("web.session.made", "Asks made in it"),
    ("web.session.resume", "Resume"),
    ("web.session.go", "Go to its agent"),
    ("web.resume.ask", "Resume {harness} {session}?"),
    ("web.earlier.search", "Search"),
    ("web.earlier.count", "{n} sessions"),
    ("web.earlier.wide", "{n} earlier"),
    ("web.earlier.foot", "newest first · scroll for more"),
    ("web.start.runs_on", "It runs on {machine}, with your own settings."),
    ("web.start.pick_one", "PICK ONE"),
    ("web.start.project", "IN WHICH PROJECT"),
    ("web.start.harness", "WHICH HARNESS"),
    ("web.start.nothing_yet", "nothing yet"),
    ("web.start.in", "Start an agent in {project}"),
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
        assert_eq!(word("turn.waiting"), "needs you");
    }
}
