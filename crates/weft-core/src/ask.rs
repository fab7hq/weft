//! The Ask sheet: what the person chooses before Send, and the plan line it
//! offers for their words.

use serde::{Deserialize, Serialize};

use crate::bm25::{Bm25, words};

/// What the person chose before an act's Send: an Ask's route, task, plan
/// and practice rules, or a Seal's reason. A part left out is the default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Practice rules, by id; none chosen is an empty list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A route as the sheet names it: `native_goal` is "Goal", `human_handoff`
/// "Human handoff".
pub fn route_label(id: &str) -> String {
    let words = id.strip_prefix("native_").unwrap_or(id).replace('_', " ");
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// A plan the project holds, as RingFrame lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub slug: String,
    pub path: String,
    pub title: String,
}

/// The plan the words are most about, by BM25 over each plan's slug and
/// title; none when no word of theirs is in any.
pub fn plan_for<'a>(said: &str, plans: &'a [Plan]) -> Option<&'a Plan> {
    let docs = plans.iter().map(|p| words(&format!("{} {}", p.slug, p.title))).collect();
    let bm25 = Bm25::new(docs);
    let query = words(said);
    let scored = (0..plans.len()).map(|i| (i, bm25.score(i, &query)));
    let (best, score) = scored.max_by(|a, b| a.1.total_cmp(&b.1))?;
    (score > 0.0).then(|| &plans[best])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(slug: &str, title: &str) -> Plan {
        Plan { slug: slug.into(), path: format!("plans/{slug}/plan.md"), title: title.into() }
    }

    #[test]
    fn a_route_reads_as_a_word() {
        assert_eq!(route_label("native_goal"), "Goal");
        assert_eq!(route_label("human_handoff"), "Human handoff");
        assert_eq!(route_label(""), "");
    }

    #[test]
    fn the_plan_line_is_the_plan_the_words_name_and_none_for_other_words() {
        let plans = [
            plan("weft-rf-next", "Weft for RingFrame next: implementation sequence"),
            plan("sense", "Sense model implementation sequence"),
            plan("toile", "Toile implementation sequence"),
        ];
        let got = |said: &str| plan_for(said, &plans).map(|p| p.slug.as_str());
        assert_eq!(got("implement plans/weft-rf-next phase 1"), Some("weft-rf-next"));
        assert_eq!(got("retrain the Sense model"), Some("sense"));
        assert_eq!(got("fix the login bug"), None);
        assert_eq!(plan_for("toile", &[]), None, "no plans, no line");
    }
}
