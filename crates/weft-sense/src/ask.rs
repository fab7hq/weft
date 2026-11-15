//! What Sense answers for an Ask: its route, task, effects and result, the
//! practice rules it ticks and every other one best first. Every answer is
//! a default the person changes.

use crate::heads::{choice, noul, sigmoid};
use crate::manifest::{Head, HeadKind, Manifest};
use crate::normalize::{Replacements, candidate_text};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskAnswer {
    pub sense: String,
    pub route: RouteAnswer,
    pub task: ClassesOn,
    pub effects: ClassesOn,
    pub result: String,
    /// The rules it ticks: the convention's, and each at or above the cut.
    pub rules: Vec<RuleScore>,
    /// Every other rule asked, highest probability first.
    #[serde(default)]
    pub unticked: Vec<RuleScore>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteAnswer {
    /// `None` below the head's `unsure_below`: the person picks.
    pub pick: Option<String>,
    pub p: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassesOn {
    pub on: Vec<String>,
    pub p: BTreeMap<String, f32>,
}

/// A rule Sense proposes, with the probability its question gave it; none
/// for a rule the convention decides from the Ask's task and effects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleScore {
    pub id: String,
    pub p: Option<f32>,
}

/// A practice rule Sense may propose. sense-xs reads it as `<label>:
/// <when>`, sense-d as its `when` line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub label: String,
    pub when: String,
    /// How often the person was offered it and kept it, when ever.
    pub counts: Option<Counts>,
}

/// A rule's Asks: those it was offered on, and those it was sent with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub candidate: u32,
    pub kept: u32,
}

/// The heads that answer an Ask, as the manifest describes them.
pub struct AskHeads {
    version: String,
    task: Head,
    effects: Head,
    route: Head,
    rule: Head,
}

impl AskHeads {
    /// The bundle's Ask heads; an error names a head it lacks.
    pub fn from_manifest(manifest: &Manifest) -> anyhow::Result<Self> {
        manifest.head_of("ask_result", HeadKind::Derived)?;
        let rule = manifest.head("rule_relevance")?;
        match rule.kind {
            HeadKind::NoulPerItem if rule.threshold.is_some() => {}
            HeadKind::Document if rule.sure.is_some() && rule.question.is_some() => {}
            _ => anyhow::bail!("the rule_relevance head has no question or cut"),
        }
        Ok(Self {
            version: manifest.version.clone(),
            task: manifest.head_of("ask_task", HeadKind::Noul)?.clone(),
            effects: manifest.head_of("ask_effects", HeadKind::Noul)?.clone(),
            route: manifest.head_of("ask_route", HeadKind::Choice)?.clone(),
            rule: rule.clone(),
        })
    }

    /// Every question of the task, effects and route heads, in that order:
    /// an Ask's first rows.
    pub fn questions(&self) -> impl Iterator<Item = &str> {
        [&self.task, &self.effects, &self.route]
            .into_iter()
            .flat_map(|h| h.questions.iter().map(String::as_str))
    }

    /// The adapter the Ask's rows are read with; none for sense-d.
    pub fn adapter(&self) -> Option<&str> {
        self.rule.adapter.as_deref()
    }

    /// The row's question for `rule`: a document head's template around
    /// its `<label>: <when>`, through `table`.
    pub fn question(&self, rule: &Rule, table: &Replacements) -> String {
        match &self.rule.question {
            Some(q) => {
                let document = candidate_text(&format!("{}: {}", rule.label, rule.when), table);
                q.replace("{document}", &document)
            }
            None => rule.when.clone(),
        }
    }

    /// Whether the convention decides `rule`, so it is never asked.
    pub fn decided(&self, rule: &str) -> bool {
        self.rule.convention.iter().any(|c| c.rules.iter().any(|r| r == rule))
    }

    /// The answer from an Ask's question logits, in [`Self::questions`]
    /// order, and the logit of each rule asked. Only the `routes` the
    /// harness has may be picked; none given, any may.
    pub fn answer(
        &self,
        logits: &[f32],
        routes: &[&str],
        asked: &[(&Rule, f32)],
    ) -> anyhow::Result<AskAnswer> {
        let (n_task, n_effects) = (self.task.questions.len(), self.effects.questions.len());
        let n = n_task + n_effects + self.route.questions.len();
        if logits.len() != n {
            anyhow::bail!("{} logits for an Ask, not {n}", logits.len());
        }
        let (task_on, task_p) = noul(&self.task, &logits[..n_task]);
        let (effects_on, effects_p) = noul(&self.effects, &logits[n_task..n_task + n_effects]);
        let route = choice(&self.route, &logits[n_task + n_effects..], routes);
        // From the top route even when it shows "unsure"; the plan is not known yet.
        let result = derive_result(top(&route.p), &task_on, &effects_on, false).to_string();
        let (rules, unticked) = self.propose(&task_on, &effects_on, asked);
        Ok(AskAnswer {
            sense: self.version.clone(),
            route: RouteAnswer { pick: route.pick, p: route.p },
            task: ClassesOn { on: task_on, p: task_p },
            effects: ClassesOn { on: effects_on, p: effects_p },
            result,
            rules,
            unticked,
        })
    }

    /// The convention's rules for this task and effects and each rule asked
    /// whose probability reaches the cut, by id; then every other rule asked,
    /// best first.
    fn propose(
        &self,
        task: &[String],
        effects: &[String],
        asked: &[(&Rule, f32)],
    ) -> (Vec<RuleScore>, Vec<RuleScore>) {
        let has = |list: &[String], c: &String| list.contains(c);
        let decided = self.rule.convention.iter().filter(|c| {
            c.task.iter().all(|t| has(task, t))
                && (c.effects.is_empty() || c.effects.iter().any(|e| has(effects, e)))
        });
        let mut rules: BTreeMap<String, Option<f32>> =
            decided.flat_map(|c| c.rules.iter().map(|r| (r.clone(), None))).collect();
        let cut = self.rule.threshold.or(self.rule.sure).unwrap_or(1.0);
        let mut unticked = Vec::new();
        for (rule, logit) in asked.iter().filter(|(r, _)| !self.decided(&r.id)) {
            let p = sigmoid(logit / self.rule.temperature + self.memory(rule.counts));
            match p >= cut {
                true => drop(rules.insert(rule.id.clone(), Some(p))),
                false => unticked.push(RuleScore { id: rule.id.clone(), p: Some(p) }),
            }
        }
        unticked.sort_by(|a, b| b.p.unwrap_or(0.0).total_cmp(&a.p.unwrap_or(0.0)));
        (rules.into_iter().map(|(id, p)| RuleScore { id, p }).collect(), unticked)
    }

    /// What a rule's counts add to its logit: `w · m`, where
    /// `m = ln((kept + a) / (candidate − kept + a))`; none without counts or
    /// a `memory` on the head.
    fn memory(&self, counts: Option<Counts>) -> f32 {
        let (Some(c), Some(memory)) = (counts, self.rule.memory) else { return 0.0 };
        let (kept, left) = (c.kept as f32, c.candidate.saturating_sub(c.kept) as f32);
        memory.w * ((kept + memory.a) / (left + memory.a)).ln()
    }
}

/// What the turn delivers, by the labelling rule (sense-d spec/labels.md
/// §7): a goal with a plan works that plan.
pub fn derive_result(
    route: &str,
    tasks: &[String],
    effects: &[String],
    has_plan: bool,
) -> &'static str {
    let has = |list: &[String], names: &[&str]| list.iter().any(|c| names.contains(&c.as_str()));
    if route == "goal" {
        return if has_plan { "workspace_change" } else { "continuing_objective" };
    }
    let plan_only = has(tasks, &["plan"]) && !has(tasks, &["implement", "operate", "document"]);
    if route == "plan" || plan_only {
        return "plan";
    }
    if route == "review" {
        return "evidence";
    }
    if has(effects, &["write"]) {
        return "workspace_change";
    }
    if has(tasks, &["review", "diagnose", "operate"]) {
        return "evidence";
    }
    "answer"
}

/// The class with the highest probability.
fn top(p: &BTreeMap<String, f32>) -> &str {
    let best = p.iter().max_by(|a, b| a.1.total_cmp(b.1));
    best.map(|(c, _)| c.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{"schema": "fab7.sense.decision/1", "version": "0.1.0",
      "normalizer": "fab7.sense.decision.normalize/2",
      "normalize": {"replacements": {}, "screen_last_lines": 15}, "files": {},
      "heads": [
        {"name": "ask_task", "kind": "noul", "classes": ["question", "plan", "implement"],
         "questions": ["q1", "q2", "q3"], "temperature": 2.0, "thresholds": [0.5, 0.5, 0.9]},
        {"name": "ask_effects", "kind": "noul", "classes": ["read", "write"],
         "questions": ["e1", "e2"], "thresholds": [0.5, 0.1]},
        {"name": "ask_route", "kind": "choice", "classes": ["direct", "plan", "goal", "review"],
         "questions": ["r1", "r2", "r3", "r4"], "unsure_below": 0.6},
        {"name": "ask_result", "kind": "derived", "classes": ["answer", "plan"]},
        {"name": "rule_relevance", "kind": "noul_per_item", "threshold": 0.5,
         "convention": [{"rules": ["practice.kiss", "practice.yagni"], "task": ["implement"],
                         "effects": ["write"]},
                        {"rules": ["practice.pesticide"], "task": ["plan", "implement"],
                         "effects": []}]}]}"#;

    fn heads() -> AskHeads {
        AskHeads::from_manifest(&Manifest::parse(MANIFEST.as_bytes()).unwrap()).unwrap()
    }

    fn rule(id: &str) -> Rule {
        Rule { id: id.into(), label: id.into(), when: format!("when {id}"), counts: None }
    }

    #[test]
    fn the_rows_ask_the_task_effects_and_route_questions_in_that_order() {
        let heads = heads();
        let q: Vec<&str> = heads.questions().collect();
        assert_eq!(q, ["q1", "q2", "q3", "e1", "e2", "r1", "r2", "r3", "r4"]);
    }

    #[test]
    fn each_head_reads_its_logits_and_the_result_follows_the_top_route() {
        let logits = [4.0, 0.0, 2.0, 3.0, -3.0, 0.0, 5.0, 0.0, 0.0];
        let a = heads().answer(&logits, &[], &[]).unwrap();
        assert_eq!(a.task.on, ["question", "plan"]);
        assert_eq!(a.effects.on, ["read"]);
        assert_eq!(a.route.pick.as_deref(), Some("plan"));
        assert_eq!((a.result.as_str(), a.sense.as_str()), ("plan", "0.1.0"));
        let unsure = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0];
        let a = heads().answer(&unsure, &[], &[]).unwrap();
        assert_eq!(a.route.pick, None, "goal is top at 0.35, under 0.6");
        assert_eq!(a.result, "continuing_objective");
    }

    #[test]
    fn a_route_the_harness_lacks_is_never_picked() {
        let logits = [0.0, 0.0, 0.0, 0.0, 0.0, -2.0, 1.5, 3.0, -2.0];
        let a = heads().answer(&logits, &["direct", "plan", "review"], &[]).unwrap();
        assert_eq!(a.route.pick.as_deref(), Some("plan"));
        assert!(a.route.p["goal"] > 0.7, "the probabilities stay the head's");
    }

    #[test]
    fn rules_are_the_conventions_and_each_rule_asked_at_its_threshold_with_no_limit() {
        // At T = 2 the task is implement alone (σ(4) = 0.98), the effects read and write.
        let implement_and_write = [-4.0, -4.0, 8.0, 0.0, 4.0, 0.0, 0.0, 0.0, 0.0];
        let (a, b, c, kiss) = (rule("a"), rule("b"), rule("c"), rule("practice.kiss"));
        let asked = [(&a, 1.0), (&b, 0.0), (&c, -0.1), (&kiss, -9.0)];
        let answer = heads().answer(&implement_and_write, &[], &asked).unwrap();
        let proposed: Vec<(&str, Option<f32>)> =
            answer.rules.iter().map(|r| (r.id.as_str(), r.p)).collect();
        assert_eq!(proposed[..2], [("a", Some(sigmoid(1.0))), ("b", Some(0.5))]);
        assert_eq!(
            proposed[2..],
            [("practice.kiss", None), ("practice.yagni", None)],
            "a decided rule is the convention's, whatever its row said; c is under 0.5; \
             pesticide needs plan as well"
        );
    }

    #[test]
    fn a_convention_with_no_effects_needs_only_its_tasks() {
        let plan_and_implement = [-4.0, 8.0, 8.0, -4.0, -4.0, 0.0, 0.0, 0.0, 0.0];
        let a = heads().answer(&plan_and_implement, &[], &[]).unwrap();
        assert_eq!(a.rules, [RuleScore { id: "practice.pesticide".into(), p: None }]);
    }

    #[test]
    fn the_result_follows_the_labelling_rule() {
        let s = |v: &[&str]| v.iter().map(|c| c.to_string()).collect::<Vec<_>>();
        assert_eq!(derive_result("goal", &s(&["plan"]), &[], false), "continuing_objective");
        assert_eq!(derive_result("goal", &s(&["implement"]), &[], true), "workspace_change");
        assert_eq!(derive_result("direct", &s(&["plan"]), &[], false), "plan");
        assert_eq!(
            derive_result("direct", &s(&["plan", "implement"]), &s(&["write"]), false),
            "workspace_change"
        );
        assert_eq!(derive_result("review", &[], &s(&["write"]), false), "evidence");
        assert_eq!(derive_result("direct", &s(&["diagnose"]), &s(&["read"]), false), "evidence");
        assert_eq!(derive_result("direct", &s(&["question"]), &s(&["read"]), false), "answer");
    }

    /// sense-xs's Ask heads: the rule head a `document`, its question a
    /// template, its rules weighed by their counts.
    fn xs_heads() -> AskHeads {
        let json = MANIFEST.replace(
            r#"{"name": "rule_relevance", "kind": "noul_per_item", "threshold": 0.5,"#,
            r#"{"name": "rule_relevance", "kind": "document", "question": "applies? {document}",
                "sure": 0.6, "memory": {"w": 0.5, "a": 1.0},"#,
        );
        AskHeads::from_manifest(&Manifest::parse(json.as_bytes()).unwrap()).unwrap()
    }

    #[test]
    fn a_documents_question_is_its_template_around_the_rules_label_and_when_through_the_table() {
        let table = Replacements::new(&[("\n".to_string(), " ↵ ".to_string())].into()).unwrap();
        let r = Rule { label: "Keep it\nsimple".into(), ..rule("practice.kiss") };
        let q = xs_heads().question(&r, &table);
        assert_eq!(q, "applies? Keep it ↵ simple: when practice.kiss");
        let r = Rule { label: "Keep it simple".into(), ..rule("practice.kiss") };
        assert_eq!(heads().question(&r, &table), "when practice.kiss", "sense-d asks the when");
    }

    #[test]
    fn a_rule_kept_on_every_ask_scores_higher_and_the_rest_are_listed_best_first() {
        let question_only = [8.0, -8.0, -8.0, -8.0, -8.0, 0.0, 0.0, 0.0, 0.0];
        let kept = Rule { counts: Some(Counts { candidate: 3, kept: 3 }), ..rule("mine") };
        let (fresh, never) = (
            rule("fresh"),
            Rule { counts: Some(Counts { candidate: 3, kept: 0 }), ..rule("never") },
        );
        let low = rule("low");
        let asked = [(&low, -1.0), (&never, 0.0), (&fresh, 0.0), (&kept, 0.0)];
        let a = xs_heads().answer(&question_only, &[], &asked).unwrap();
        // m = ln(4/1) for mine, ln(1/4) for never: p = σ(±0.5 · 1.386).
        let mine = sigmoid(0.5 * 4f32.ln());
        assert_eq!(a.rules, [RuleScore { id: "mine".into(), p: Some(mine) }], "over 0.6");
        let unticked: Vec<&str> = a.unticked.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(unticked, ["fresh", "never", "low"], "0.5, then 0.33, then 0.27");
        assert_eq!(a.unticked[0].p, Some(0.5), "no counts, no term");
    }

    #[test]
    fn a_head_without_memory_ignores_counts() {
        let question_only = [8.0, -8.0, -8.0, -8.0, -8.0, 0.0, 0.0, 0.0, 0.0];
        let kept = Rule { counts: Some(Counts { candidate: 3, kept: 3 }), ..rule("mine") };
        let a = heads().answer(&question_only, &[], &[(&kept, -0.1)]).unwrap();
        assert_eq!(a.unticked, [RuleScore { id: "mine".into(), p: Some(sigmoid(-0.1)) }]);
    }

    #[test]
    fn a_bundle_without_an_ask_head_is_an_error_naming_it() {
        let json = MANIFEST.replace("\"ask_route\"", "\"ask_way\"");
        let m = Manifest::parse(json.as_bytes()).unwrap();
        let err = AskHeads::from_manifest(&m).err().unwrap().to_string();
        assert!(err.contains("ask_route"), "{err}");
    }
}
