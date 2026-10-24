//! Building what one of RingFrame's three acts would type.
//!
//! This is the daemon's, because it reads files and calls RingFrame. What to
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

use crate::ringframe;

/// A prompt ready to be put in front of a person.
pub struct Built {
    pub payload: Vec<u8>,
    pub how: Handoff,
    pub asking: Asking,
    /// Whether the person's yes still has to be recorded with RingFrame.
    pub confirm_first: bool,
    pub ask_id: Option<String>,
    /// The Ask its Send compiles, before the prompt shown is typed.
    pub compile: Option<ringframe::AskDraft>,
    /// The Eval its Send opens, then debates.
    pub eval: Option<Debate>,
}

/// An Eval of a unit of work, to debate in a harness once the person says
/// yes: the unit's own gathered Eval, or one RingFrame opens then.
#[derive(Debug, Clone)]
pub struct Debate {
    pub unit: Unit,
    pub stage: weft_core::eval_stages::Stage,
    pub open: Option<String>,
}

/// The dialog for an Eval: what it is of, and who checks it. Nothing is
/// typed or opened until its Send.
pub fn eval(debate: Debate, title: &str, asks: usize, judges: &str) -> Built {
    Built {
        payload: Vec::new(),
        how: Handoff::Whole,
        asking: weft_core::offers::evaluating(title, asks, judges),
        confirm_first: false,
        ask_id: None,
        compile: None,
        eval: Some(debate),
    }
}

/// Send a compiled prompt. The wording is RingFrame's, read back through its
/// CLI, and Weft does not touch it.
/// Into the harness it was composed in, it goes as RingFrame compiled it;
/// into another, in that harness's own way for the same capability, or as
/// plain text. The wording is the same either way.
pub fn send(root: &Path, unit: &Unit, into: &str) -> Result<Built, String> {
    let refused = |e| format!("RingFrame would not hand over the wording: {e:?}");
    let (payload, how) = if into == unit.harness {
        let payload = ringframe::ask_copy(root, &unit.ask_id).map_err(refused)?;
        (payload, Handoff::choose(&unit.delivery))
    } else {
        let theirs = ringframe::ask_for_host(root, &unit.ask_id, into).map_err(refused)?;
        let prompt = theirs["prompt"].as_str().unwrap_or_default().as_bytes().to_vec();
        (prompt, Handoff::choose(&weft_core::ledger::delivery_of(&theirs)))
    };
    Ok(Built {
        asking: weft_core::offers::sending(unit, &how),
        payload,
        how,
        confirm_first: unit.awaiting_yes(),
        ask_id: Some(unit.ask_id.clone()),
        compile: None,
        eval: None,
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
        compile: None,
        eval: None,
    }
}

/// Ask for something new: the exact prompt Send will type into the harness
/// `title` names, composed by RingFrame and recorded only on Send.
pub fn ask(
    root: &Path,
    draft: ringframe::AskDraft,
    over: Option<&str>,
    title: &str,
) -> Result<Built, String> {
    let composed = ringframe::compose(root, &draft, over).map_err(|e| {
        let ringframe::Error::Refused { message, .. } = e;
        message
    })?;
    let delivery = serde_json::json!({"delivery": composed.delivery});
    let how = Handoff::choose(&weft_core::ledger::delivery_of(&delivery));
    Ok(Built {
        asking: weft_core::offers::asking(title, &how, &composed.trimmed),
        payload: composed.text.into_bytes(),
        how,
        confirm_first: false,
        ask_id: None,
        compile: Some(draft),
        eval: None,
    })
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
#[derive(Default)]
pub struct Prefixes {
    prefixes: HashMap<String, String>,
}

impl Prefixes {
    fn prefix(&mut self, root: &Path, harness: &str) -> String {
        self.prefixes
            .entry(harness.to_string())
            .or_insert_with(|| {
                ringframe::invocation_prefix(root, harness).unwrap_or_else(|_| "/rf:".into())
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
        }
    }
}
