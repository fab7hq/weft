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
    /// The project whose plans folder holds it, in a workspace of several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub slug: String,
    pub path: String,
    pub title: String,
    /// The first paragraph after the title.
    #[serde(default)]
    pub paragraph: String,
}

impl Plan {
    /// `<project>/<slug>`, or the slug alone.
    pub fn name(&self) -> String {
        match &self.project {
            Some(project) => format!("{project}/{}", self.slug),
            None => self.slug.clone(),
        }
    }

    /// The plan as Sense's plan head reads it: `<project> / <slug>: <title>:
    /// <paragraph>`, or `<slug>: …` with no project.
    pub fn candidate(&self) -> String {
        let named = match &self.project {
            Some(project) => format!("{project} / {}", self.slug),
            None => self.slug.clone(),
        };
        format!("{named}: {}: {}", self.title, self.paragraph)
    }

    /// `plans/<project>/<slug>`, or `plans/<slug>`: its folder.
    fn folder(&self) -> String {
        format!("plans/{}", self.name())
    }
}

/// The one plan the words name, with no model: by its folder
/// (`plans/<project>/<slug>`, `plans/<slug>`, its `plan.md` too) or a
/// project's plan by `<project>/<slug>`, else by its slug as a whole word.
/// Two plans named are none.
pub fn named<'a>(said: &str, plans: &'a [Plan]) -> Option<&'a Plan> {
    let quoted = |c: char| c.is_whitespace() || "`\"'()[]<>,;".contains(c);
    let paths: Vec<&str> = said
        .split(quoted)
        .map(|t| t.trim_end_matches(['.', ':', '!', '?']))
        .map(|t| t.strip_suffix("/plan.md").unwrap_or(t).trim_end_matches('/'))
        .collect();
    let at = |p: &&Plan| {
        let (folder, name) = (p.folder(), p.project.as_ref().map(|_| p.name()));
        let named = |t: &&str| Some(*t) == name.as_deref();
        paths.iter().any(|t| *t == folder || t.ends_with(&format!("/{folder}")) || named(t))
    };
    let mut by_path = plans.iter().filter(at);
    if let Some(one) = by_path.next() {
        return by_path.next().is_none().then_some(one);
    }
    // A slug as a whole word: `ui` is not named by "build".
    let words: Vec<&str> = said.split(|c: char| !(c.is_alphanumeric() || c == '-')).collect();
    let mut by_slug = plans.iter().filter(|p| words.contains(&p.slug.as_str()));
    let one = by_slug.next()?;
    by_slug.next().is_none().then_some(one)
}

/// The plans Sense's plan head reads for these words: BM25's top `keep` over
/// each plan's candidate text, and the plan at `last` (the last Ask's) too. A
/// cheap filter, so the head's cost stays flat however many plans the
/// project holds; it picks nothing itself.
pub fn shortlist<'a>(
    said: &str,
    plans: &'a [Plan],
    keep: usize,
    last: Option<&str>,
) -> Vec<&'a Plan> {
    let bm25 = Bm25::new(plans.iter().map(|p| words(&p.candidate())).collect());
    let query = words(said);
    let mut ranked: Vec<(usize, f64)> =
        (0..plans.len()).map(|i| (i, bm25.score(i, &query))).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out: Vec<&Plan> = ranked.into_iter().take(keep).map(|(i, _)| &plans[i]).collect();
    let used = plans.iter().find(|p| Some(p.path.as_str()) == last);
    if let Some(used) = used.filter(|u| !out.contains(u)) {
        out.push(used);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(slug: &str, title: &str) -> Plan {
        let path = format!("plans/{slug}/plan.md");
        Plan {
            project: None,
            slug: slug.into(),
            path,
            title: title.into(),
            paragraph: String::new(),
        }
    }

    #[test]
    fn a_route_reads_as_a_word() {
        assert_eq!(route_label("native_goal"), "Goal");
        assert_eq!(route_label("human_handoff"), "Human handoff");
        assert_eq!(route_label(""), "");
    }

    fn in_project(project: &str, slug: &str) -> Plan {
        let path = format!("plans/{project}/{slug}/plan.md");
        Plan { project: Some(project.into()), path, ..plan(slug, slug) }
    }

    #[test]
    fn a_plan_named_by_its_path_or_slug_is_the_plan_and_two_named_are_none() {
        let plans = [
            plan("weft-harden", "Harden"),
            plan("ui", "Interface"),
            in_project("sense", "harden"),
            in_project("weft", "harden"),
            in_project("weft", "launch"),
        ];
        let got = |said: &str| named(said, &plans).map(Plan::name);
        assert_eq!(got("implement plans/weft-harden").as_deref(), Some("weft-harden"));
        assert_eq!(got("go on with `weft-harden`, phase 5").as_deref(), Some("weft-harden"));
        assert_eq!(got("see plans/weft-harden/plan.md.").as_deref(), Some("weft-harden"));
        assert_eq!(got("implement plans/weft/harden").as_deref(), Some("weft/harden"));
        assert_eq!(got("the launch plan").as_deref(), Some("weft/launch"));
        assert_eq!(got("implement weft/harden").as_deref(), Some("weft/harden"));
        assert_eq!(got("harden it"), None, "two plans are named harden");
        assert_eq!(got("weft-harden and the launch"), None, "two plans named");
        assert_eq!(got("rebuild the cache"), None, "a slug inside a word is not named");
        assert_eq!(got("fix the login bug"), None);
    }

    #[test]
    fn the_shortlist_is_bm25s_top_and_the_last_used_plan() {
        let mut plans: Vec<Plan> =
            (0..25).map(|i| plan(&format!("p{i:02}"), &format!("Plan number {i}"))).collect();
        plans.push(plan("sense", "Sense model implementation sequence"));
        plans.push(in_project("site", "copy"));
        plans[3].paragraph = "Retrain the model on fresh labels.".into();
        let got = |said: &str, keep, last: Option<&str>| -> Vec<String> {
            shortlist(said, &plans, keep, last).iter().map(|p| p.name()).collect()
        };
        assert_eq!(got("retrain the sense model", 2, None), ["sense", "p03"]);
        let last = Some("plans/site/copy/plan.md");
        assert_eq!(got("retrain the sense model", 2, last), ["sense", "p03", "site/copy"]);
        assert_eq!(got("site copy", 1, last), ["site/copy"], "never twice");
        assert_eq!(got("fix the login bug", 5, last).len(), 6, "never more than it keeps, and one");
        assert_eq!(got("anything", 100, None).len(), plans.len());
        assert_eq!(shortlist("site", &[], 5, last), Vec::<&Plan>::new(), "no plans, none");
    }

    #[test]
    fn a_plan_reads_as_its_slug_title_and_paragraph() {
        let mut p = plan("sync", "Offline sync");
        p.paragraph = "Notes sync when devices reconnect.".into();
        assert_eq!(p.candidate(), "sync: Offline sync: Notes sync when devices reconnect.");
        let p = Plan { project: Some("app".into()), ..p };
        assert_eq!(p.name(), "app/sync");
        assert_eq!(p.candidate(), "app / sync: Offline sync: Notes sync when devices reconnect.");
    }
}
