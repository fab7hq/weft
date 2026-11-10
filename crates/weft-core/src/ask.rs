//! The Ask sheet: what the person chooses before Send, and the plans Sense
//! reads for their words.

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
    /// The first paragraph after the title.
    #[serde(default)]
    pub paragraph: String,
}

impl Plan {
    /// The plan as Sense's plan head reads it: `<slug>: <title>: <paragraph>`.
    pub fn candidate(&self) -> String {
        format!("{}: {}: {}", self.slug, self.title, self.paragraph)
    }
}

/// The plans Sense's plan head reads for these words, at most `keep`: those
/// the words name by slug, then the rest by BM25 over slug, title and first
/// paragraph. A cheap filter, so the head's cost stays flat however many
/// plans the project holds; it picks nothing itself.
pub fn shortlist<'a>(said: &str, plans: &'a [Plan], keep: usize) -> Vec<&'a Plan> {
    let docs = plans.iter().map(|p| words(&format!("{} {} {}", p.slug, p.title, p.paragraph)));
    let bm25 = Bm25::new(docs.collect());
    let query = words(said);
    // A slug as a whole word: `ui` is not named by "build".
    let said_words: Vec<&str> = said.split(|c: char| !(c.is_alphanumeric() || c == '-')).collect();
    let names = |p: &Plan| said_words.contains(&p.slug.as_str());
    let mut ranked: Vec<(usize, bool, f64)> =
        (0..plans.len()).map(|i| (i, names(&plans[i]), bm25.score(i, &query))).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.total_cmp(&a.2)).then(a.0.cmp(&b.0)));
    ranked.into_iter().take(keep).map(|(i, _, _)| &plans[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(slug: &str, title: &str) -> Plan {
        let path = format!("plans/{slug}/plan.md");
        Plan { slug: slug.into(), path, title: title.into(), paragraph: String::new() }
    }

    #[test]
    fn a_route_reads_as_a_word() {
        assert_eq!(route_label("native_goal"), "Goal");
        assert_eq!(route_label("human_handoff"), "Human handoff");
        assert_eq!(route_label(""), "");
    }

    #[test]
    fn the_shortlist_keeps_named_plans_first_then_the_closest_words() {
        let mut plans: Vec<Plan> =
            (0..25).map(|i| plan(&format!("p{i:02}"), &format!("Plan number {i}"))).collect();
        plans.push(plan("sense", "Sense model implementation sequence"));
        plans.push(plan("toile", "Toile mockups"));
        plans.push(plan("ui", "Interface"));
        plans[3].paragraph = "Retrain the model on fresh labels.".into();
        let got = |said: &str, keep| -> Vec<String> {
            shortlist(said, &plans, keep).iter().map(|p| p.slug.clone()).collect()
        };
        assert_eq!(got("implement plans/toile phase 1", 8)[0], "toile", "named first");
        assert_eq!(got("retrain the sense model", 2), ["sense", "p03"]);
        assert_eq!(got("fix the login bug", 8).len(), 8, "never more than it keeps");
        assert_eq!(got("anything", 100).len(), plans.len());
        assert_ne!(got("rebuild the cache", 1), ["ui"], "a slug inside a word is not named");
        assert_eq!(shortlist("toile", &[], 8), Vec::<&Plan>::new(), "no plans, none");
    }

    #[test]
    fn a_plan_reads_as_its_slug_title_and_paragraph() {
        let mut p = plan("sync", "Offline sync");
        p.paragraph = "Notes sync when devices reconnect.".into();
        assert_eq!(p.candidate(), "sync: Offline sync: Notes sync when devices reconnect.");
    }
}
