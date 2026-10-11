//! Building what one of RingFrame's three acts would type.
//!
//! This is the daemon's, because it reads files and runs the CLI. What to
//! *say* about it is a rule and lives in [`weft_core::offers`]; which harness
//! it goes to is a rule and lives in [`weft_core::board`]. Here is only the
//! fetching.
//!
//! A client asks for an act by name. It never assembles bytes — the moment two
//! clients do that, they disagree.

use std::collections::HashMap;
use std::path::Path;

use weft_core::inject::{self, Handoff};
use weft_core::ledger::Unit;
use weft_core::offers::Asking;

use crate::outside::{Outside, Shared};
use crate::ringframe;

/// A prompt ready to be put in front of a person.
pub struct Built {
    pub payload: Vec<u8>,
    pub how: Handoff,
    pub asking: Asking,
    /// Whether the person's yes still has to be recorded with RingFrame.
    pub confirm_first: bool,
    pub ask_id: Option<String>,
}

/// Send a compiled prompt. The wording is RingFrame's, read back through its
/// CLI, and Weft does not touch it.
pub fn send(out: &dyn Outside, root: &Path, unit: &Unit) -> Result<Built, String> {
    let payload = ringframe::ask_copy(out, root, &unit.ask_id)
        .map_err(|e| format!("RingFrame would not hand over the wording: {e:?}"))?;
    let how = Handoff::choose(&unit.delivery);
    Ok(Built {
        asking: weft_core::offers::sending(unit, &how),
        payload,
        how,
        confirm_first: unit.awaiting_yes(),
        ask_id: Some(unit.ask_id.clone()),
    })
}

/// Run Check or Decide in the harness this project routes it to.
/// A skill with nothing more than its words: `/rf:eval gather context=…`, or
/// `/rf:seal ` and nothing after it.
pub fn skill(
    root: &Path,
    skill: &str,
    into: &str,
    worked_in: &str,
    args: &str,
    over: Option<&str>,
    prefixes: &mut Prefixes,
) -> Built {
    let command = ringframe::skill_command(&prefixes.prefix(root, into), skill);
    let payload = typed(&command, over, args);
    Built {
        how: how(&command),
        asking: weft_core::offers::running(skill, &command, into, worked_in),
        payload,
        confirm_first: false,
        ask_id: None,
    }
}

/// Ask for something new, in the harness the person is asking in.
pub fn ask(
    root: &Path,
    harness: &str,
    text: &str,
    over: Option<&str>,
    prefixes: &mut Prefixes,
) -> Built {
    let command = ringframe::skill_command(&prefixes.prefix(root, harness), "ask");
    let payload = typed(&command, over, text);
    Built {
        how: how(&command),
        asking: weft_core::offers::asking(&command, harness),
        payload,
        confirm_first: false,
        ask_id: None,
    }
}

/// The skill, then RingFrame's overrides when there are any, then its words.
/// The overrides are one single-quoted shell word, which the skill passes on
/// unchanged to every `ringframe` command it runs.
fn typed(command: &str, over: Option<&str>, words: &str) -> Vec<u8> {
    match over {
        Some(over) => format!("{command}--override '{over}' {words}"),
        None => format!("{command}{words}"),
    }
    .into_bytes()
}

/// The skill prefix RingFrame uses for each harness, asked once and kept: a
/// property of the machine, not of the moment.
pub struct Prefixes {
    prefixes: HashMap<String, String>,
    out: Shared,
}

impl Default for Prefixes {
    fn default() -> Self {
        Prefixes::asking(crate::outside::machine())
    }
}

impl Prefixes {
    /// Kept answers, asked of `out` the first time each is needed.
    pub fn asking(out: Shared) -> Self {
        Prefixes { prefixes: HashMap::new(), out }
    }

    fn prefix(&mut self, root: &Path, harness: &str) -> String {
        self.prefixes
            .entry(harness.to_string())
            .or_insert_with(|| {
                ringframe::invocation_prefix(self.out.as_ref(), root, harness)
                    .unwrap_or_else(|_| "/rf:".into())
            })
            .clone()
    }
}

/// How a prompt Weft composed has to be typed. Its command is the skill
/// invocation, which on a harness whose prefix is `/rf:` is a real slash
/// command, and is then typed whatever the prompt's length.
fn how(command: &str) -> Handoff {
    match inject::leading_command(command.as_bytes()) {
        Some(command) => Handoff::TypeCommand { at: command.len() + 1, command },
        None => Handoff::Whole,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_carries_its_words_after_the_skill_on_either_harness() {
        let mut prefixes = Prefixes::default();
        prefixes.prefixes.insert("claude-code".into(), "/rf:".into());
        prefixes.prefixes.insert("codex".into(), "$rf:".into());
        let root = Path::new("/nowhere");
        let sent = |into: &str, args: &str, prefixes: &mut Prefixes| {
            String::from_utf8(
                skill(root, "eval", into, "claude-code", args, None, prefixes).payload,
            )
            .unwrap()
        };
        assert_eq!(
            sent("codex", "gather context=gpt-6-luna/low", &mut prefixes),
            "$rf:eval gather context=gpt-6-luna/low"
        );
        assert_eq!(
            sent("claude-code", "debate evl_1 drift=/high", &mut prefixes),
            "/rf:eval debate evl_1 drift=/high"
        );
        assert_eq!(sent("claude-code", "", &mut prefixes), "/rf:eval ");
    }

    #[test]
    fn every_rf_command_carries_the_override_right_after_the_skill() {
        let mut prefixes = Prefixes::default();
        prefixes.prefixes.insert("claude-code".into(), "/rf:".into());
        prefixes.prefixes.insert("codex".into(), "$rf:".into());
        let root = Path::new("/nowhere");
        let over = r#"[{"layer":"weft","ringframe":{"deltas":{}}}]"#;
        let text = |b: Built| String::from_utf8(b.payload).unwrap();
        for (harness, prefix) in [("claude-code", "/rf:"), ("codex", "$rf:")] {
            assert_eq!(
                text(skill(root, "eval", harness, harness, "gather", Some(over), &mut prefixes)),
                format!("{prefix}eval --override '{over}' gather")
            );
            assert_eq!(
                text(skill(root, "seal", harness, harness, "", Some(over), &mut prefixes)),
                format!("{prefix}seal --override '{over}' ")
            );
            assert_eq!(
                text(ask(root, harness, "[follow-up evl_1] fix it", Some(over), &mut prefixes)),
                format!("{prefix}ask --override '{over}' [follow-up evl_1] fix it")
            );
            assert_eq!(
                text(ask(root, harness, "fix it", None, &mut prefixes)),
                format!("{prefix}ask fix it"),
                "no overrides: nothing typed"
            );
        }
    }

    /// A command Weft composed is typed whatever the prompt's length, on a
    /// made-up harness whose prefix is a slash command and one whose is not.
    #[test]
    fn a_command_is_typed_for_a_short_prompt_and_a_long_one() {
        let mut prefixes = Prefixes::default();
        prefixes.prefixes.insert("loom".into(), "/rf:".into());
        prefixes.prefixes.insert("weave".into(), "$rf:".into());
        let root = Path::new("/nowhere");
        let long = "x".repeat(3500);
        for text in ["fix it", long.as_str()] {
            let built = ask(root, "loom", text, None, &mut prefixes);
            assert_eq!(built.how, Handoff::TypeCommand { command: "/rf:ask".into(), at: 8 });
            assert_eq!(built.how.body(&built.payload), text.as_bytes());
            let built = ask(root, "weave", text, None, &mut prefixes);
            assert_eq!(built.how, Handoff::Whole, "not a slash command: pasted whole");
        }
    }
}
