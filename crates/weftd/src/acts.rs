//! Building what one of RingFrame's three acts would type.
//!
//! ADR-0007: this is the daemon's, because it reads files and runs the CLI.
//! What to *say* about it is a rule and lives in [`weft_core::offers`]; which
//! harness it goes to is a rule and lives in [`weft_core::board`]. Here is only
//! the fetching.
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
}

/// Send a compiled prompt. The wording is RingFrame's, read back through its
/// CLI, and Weft does not touch it.
pub fn send(root: &Path, unit: &Unit) -> Result<Built, String> {
    let payload = ringframe::ask_copy(root, &unit.ask_id)
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
pub fn skill(root: &Path, skill: &str, into: &str, worked_in: &str, folds: &mut Folds) -> Built {
    let command = ringframe::skill_command(&folds.prefix(root, into), skill);
    let payload = command.clone().into_bytes();
    Built {
        how: folds.how(root, into, &payload, &command),
        asking: weft_core::offers::running(skill, &command, into, worked_in),
        payload,
        confirm_first: false,
        ask_id: None,
    }
}

/// Ask for something new, in the harness the person is asking in.
pub fn ask(root: &Path, harness: &str, text: &str, folds: &mut Folds) -> Built {
    let command = ringframe::skill_command(&folds.prefix(root, harness), "ask");
    let payload = format!("{command}{text}").into_bytes();
    Built {
        how: folds.how(root, harness, &payload, &command),
        asking: weft_core::offers::asking(&command, harness),
        payload,
        confirm_first: false,
        ask_id: None,
    }
}

/// What each harness answers about itself, asked once and kept.
///
/// Both answers are properties of the machine, not of the moment: the skill
/// prefix RingFrame uses for this host, and the size at which its composer
/// stops reading a paste as text.
#[derive(Default)]
pub struct Folds {
    prefixes: HashMap<String, String>,
    at: HashMap<String, Option<usize>>,
}

impl Folds {
    fn prefix(&mut self, root: &Path, harness: &str) -> String {
        self.prefixes
            .entry(harness.to_string())
            .or_insert_with(|| {
                ringframe::invocation_prefix(root, harness).unwrap_or_else(|_| "/rf:".into())
            })
            .clone()
    }

    /// How a prompt Weft composed has to be typed. Its command is the skill
    /// invocation, which on Claude Code is `/rf:` — a real slash command, and
    /// so subject to the same fold as any other.
    fn how(&mut self, root: &Path, harness: &str, payload: &[u8], command: &str) -> Handoff {
        let Some(command) = inject::leading_command(command.as_bytes()) else {
            return Handoff::Whole;
        };
        let fold = *self
            .at
            .entry(harness.to_string())
            .or_insert_with(|| ringframe::paste_fold_chars(root, harness));
        match fold {
            Some(at) if payload.len() >= at => {
                Handoff::TypeCommand { at: command.len() + 1, command }
            }
            _ => Handoff::Whole,
        }
    }
}
