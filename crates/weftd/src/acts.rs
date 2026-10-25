//! Building what one of RingFrame's three acts would type, and the dialogs
//! that type nothing.
//!
//! This is the daemon's, because it reads files and calls RingFrame. What to
//! *say* about it is a rule and lives in [`weft_core::offers`]; which harness
//! it goes to is a rule and lives in [`weft_core::board`]. Here is only the
//! fetching.
//!
//! A client asks for an act by name. It never assembles bytes — the moment two
//! clients do that, they disagree.

use std::path::Path;

use weft_core::inject::Handoff;
use weft_core::ledger::Unit;
use weft_core::offers::Asking;

use crate::ringframe;

/// A prompt ready to be put in front of a person.
pub struct Built {
    pub payload: Vec<u8>,
    pub how: Handoff,
    pub asking: Asking,
    pub ask_id: Option<String>,
    /// The Ask its Send compiles, before the prompt shown is typed.
    pub compile: Option<ringframe::AskDraft>,
    /// The Eval its Send opens, then hands to judges.
    pub eval: Option<Debate>,
    /// The Eval its Stop voids.
    pub stop: Option<String>,
    /// Its yes button, when not Send.
    pub go: Option<weft_proto::Go>,
    /// The draft its Send asks an agent for, to read back once written.
    pub drafts: Option<Drafting>,
}

/// A rule a sealed unit's agent was asked to draft: the file it writes, the
/// unit's first Ask and the agent's harness.
#[derive(Debug, Clone)]
pub struct Drafting {
    pub path: std::path::PathBuf,
    pub unit: String,
    pub harness: String,
}

/// An Eval of a unit of work, for judges in a harness once the person says
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
        ask_id: None,
        compile: None,
        eval: Some(debate),
        stop: None,
        go: Some(weft_proto::Go { word: "act.eval".into(), warn: false }),
        drafts: None,
    }
}

/// The dialog that stops a running Eval: nothing is typed; Stop voids it.
pub fn stop(eval_id: &str, title: &str) -> Built {
    Built {
        payload: Vec::new(),
        how: Handoff::Whole,
        asking: weft_core::offers::stopping(title),
        ask_id: None,
        compile: None,
        eval: None,
        stop: Some(eval_id.to_string()),
        go: Some(weft_proto::Go { word: "dialog.stop_eval.go".into(), warn: true }),
        drafts: None,
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
        ask_id: Some(unit.ask_id.clone()),
        compile: None,
        eval: None,
        stop: None,
        go: None,
        drafts: None,
    })
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
        ask_id: None,
        compile: Some(draft),
        eval: None,
        stop: None,
        go: None,
        drafts: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_says_stop_eval_in_amber_and_types_nothing() {
        let built = stop("evl_1", "Build account erasure");
        let go = built.go.expect("its own button");
        assert_eq!((weft_core::words::word(&go.word), go.warn), ("Stop Eval", true));
        assert!(built.payload.is_empty());
        assert_eq!(built.stop.as_deref(), Some("evl_1"));
    }
}
