//! The Ask heads of bundle 3: what an Ask's batch of logits answers
//! (spec/bundle-3.md §3 and §7). Every answer is a default the person changes.

use crate::ask_graph::{AskGraph, alone_row, ask_rows};
use crate::manifest::{Head, HeadKind, Manifest};
use crate::pair::PairHead;
use crate::unigram::AskTokenizer;
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;

/// The Ask answer of spec/bundle-3.md §7.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskAnswer {
    pub sense: String,
    pub route: RouteAnswer,
    pub task: ClassesOn,
    pub effects: ClassesOn,
    pub result: String,
    pub rules: Vec<RuleScore>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleScore {
    pub id: String,
    pub p: f32,
}

/// A practice rule Sense may propose, one RingFrame offers for the Ask's
/// kind of work: its `when` line, its family (at most one proposed per
/// family) and the example intents people kept it for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub family: String,
    pub when: String,
    #[serde(default)]
    pub examples: Vec<String>,
}

/// The heads that answer an Ask, as the manifest describes them.
pub struct AskHeads {
    version: String,
    /// Every `noul` and `choice` head, in the manifest's order: an Ask's
    /// batch has one row per question of each, in this order.
    questioned: Vec<Head>,
    rule: Head,
}

impl AskHeads {
    /// The bundle's Ask heads; an error names a head it lacks.
    pub fn from_manifest(manifest: &Manifest) -> anyhow::Result<Self> {
        for name in ["ask_task", "ask_effects", "ask_route", "ask_result"] {
            manifest.head(name)?;
        }
        let questioned: Vec<Head> = manifest
            .heads
            .iter()
            .filter(|h| matches!(h.kind, HeadKind::Noul | HeadKind::Choice))
            .cloned()
            .collect();
        for head in &questioned {
            check_question_head(head)?;
        }
        let rule = manifest.head("rule_relevance")?.clone();
        if rule.kind != HeadKind::Pair || rule.threshold.is_none() {
            bail!("the rule_relevance head is not a pair head with a threshold");
        }
        Ok(Self { version: manifest.version.clone(), questioned, rule })
    }

    /// Every question, in the batch's row order.
    pub fn questions(&self) -> impl Iterator<Item = &str> {
        self.questioned.iter().flat_map(|h| h.questions.iter().map(String::as_str))
    }

    /// The answer to one Ask from its batch's logits, one per question in
    /// [`Self::questions`] order, and its rules' scores. Only the `routes`
    /// the harness has may be picked; none given, any may.
    pub fn answer(
        &self,
        logits: &[f32],
        routes: &[&str],
        rules: Vec<RuleScore>,
    ) -> anyhow::Result<AskAnswer> {
        let route = self.choice("ask_route", logits, routes)?;
        let task = self.noul("ask_task", logits)?;
        let effects = self.noul("ask_effects", logits)?;
        let top_route = top(&route.p);
        let result = derive_result(top_route, &task.on, &effects.on).to_string();
        Ok(AskAnswer { sense: self.version.clone(), route, task, effects, result, rules })
    }

    /// The probability the rule head gives a logit.
    pub fn rule_p(&self, logit: f32) -> f32 {
        sigmoid(logit / self.rule.temperature)
    }

    /// Of `scored` (each rule with its probability), those Sense proposes:
    /// at or above the threshold, at most one per family, highest first.
    pub fn propose(&self, mut scored: Vec<(&Rule, f32)>) -> Vec<RuleScore> {
        let threshold = self.rule.threshold.unwrap_or(1.0);
        scored.retain(|(_, p)| *p >= threshold);
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut families = std::collections::HashSet::new();
        scored
            .into_iter()
            .filter(|(rule, _)| families.insert(rule.family.as_str()))
            .map(|(rule, p)| RuleScore { id: rule.id.clone(), p })
            .collect()
    }

    fn logits_of<'a>(&self, name: &str, logits: &'a [f32]) -> anyhow::Result<(&Head, &'a [f32])> {
        let mut start = 0;
        for head in &self.questioned {
            let end = start + head.questions.len();
            if head.name == name {
                let Some(slice) = logits.get(start..end) else {
                    bail!("{} logits for an Ask, not {}", logits.len(), self.questions().count());
                };
                return Ok((head, slice));
            }
            start = end;
        }
        bail!("Sense bundle {} has no questioned {name} head", self.version)
    }

    fn noul(&self, name: &str, logits: &[f32]) -> anyhow::Result<ClassesOn> {
        let (head, logits) = self.logits_of(name, logits)?;
        let p: Vec<f32> = logits.iter().map(|l| sigmoid(l / head.temperature)).collect();
        let on = head.classes.iter().zip(&p).zip(&head.thresholds);
        let on = on.filter(|((_, p), t)| p >= t).map(|((c, _), _)| c.clone()).collect();
        Ok(ClassesOn { on, p: head.classes.iter().cloned().zip(p).collect() })
    }

    /// A `choice` head's probabilities, and its pick among `allowed` (any,
    /// when empty): their top, renormalized, unless under `unsure_below`.
    fn choice(&self, name: &str, logits: &[f32], allowed: &[&str]) -> anyhow::Result<RouteAnswer> {
        let (head, logits) = self.logits_of(name, logits)?;
        let p = softmax(logits, head.temperature);
        let open = |c: &str| allowed.is_empty() || allowed.contains(&c);
        let candidates = head.classes.iter().zip(&p).filter(|(c, _)| open(c));
        let total: f32 = candidates.clone().map(|(_, p)| p).sum();
        let best = candidates.max_by(|a, b| a.1.total_cmp(b.1));
        let sure = |p: f32| total > 0.0 && p / total >= head.unsure_below.unwrap_or(0.0);
        let pick = best.filter(|(_, p)| sure(**p)).map(|(c, _)| c.clone());
        Ok(RouteAnswer { pick, p: head.classes.iter().cloned().zip(p).collect() })
    }
}

/// What answers an Ask: its heads, its graph and tokenizer, the rule head,
/// and the embeddings of the rules' lines met so far.
pub struct AskSense {
    heads: AskHeads,
    graph: AskGraph,
    tokenizer: AskTokenizer,
    rule_head: PairHead,
    embedded: Mutex<HashMap<String, Vec<f32>>>,
}

impl AskSense {
    /// The Ask side of the bundle in `dir`, each file checked against the
    /// manifest's digest.
    pub fn load(dir: &Path, manifest: &Manifest) -> anyhow::Result<Self> {
        let read = |key: &str| -> anyhow::Result<Vec<u8>> {
            let entry = manifest.file(key)?;
            crate::model::read_verified(&dir.join(&entry.path), &entry.sha256, key)
        };
        let heads = AskHeads::from_manifest(manifest)?;
        let weights =
            heads.rule.weights.as_deref().context("the rule_relevance head names no weights")?;
        let pair_key =
            manifest.files.iter().find(|(_, f)| f.path == weights).map(|(k, _)| k.clone());
        let pair_key = pair_key.with_context(|| format!("the manifest lists no file {weights}"))?;
        Ok(Self {
            heads,
            graph: AskGraph::load(&read("ask_model")?)?,
            tokenizer: AskTokenizer::from_bytes(&read("ask_tokenizer")?)?,
            rule_head: PairHead::from_safetensors(&read(&pair_key)?)?,
            embedded: Mutex::new(HashMap::new()),
        })
    }

    /// The answer to `text`: its route among `routes`, its task, effects
    /// and result, and which of `rules` Sense proposes.
    pub fn answer(&self, text: &str, routes: &[&str], rules: &[Rule]) -> anyhow::Result<AskAnswer> {
        let rows = ask_rows(&self.tokenizer, text, self.heads.questions())?;
        let out = self.graph.run(&rows)?;
        let u = out.embeddings.last().context("an Ask batch has its Ask alone last")?;
        let mut scored = Vec::with_capacity(rules.len());
        for rule in rules {
            let when = self.embed(&rule.when)?;
            let examples =
                rule.examples.iter().map(|e| self.embed(e)).collect::<anyhow::Result<Vec<_>>>()?;
            scored
                .push((rule, self.heads.rule_p(rule_logit(&self.rule_head, u, &when, &examples))));
        }
        let questions = self.heads.questions().count();
        self.heads.answer(&out.logits[..questions], routes, self.heads.propose(scored))
    }

    /// A line's embedding alone, as an Ask alone is embedded; kept once made.
    fn embed(&self, line: &str) -> anyhow::Result<Vec<f32>> {
        if let Some(v) = self.embedded.lock().unwrap_or_else(|e| e.into_inner()).get(line) {
            return Ok(v.clone());
        }
        let out = self.graph.run(&[alone_row(&self.tokenizer, line)?])?;
        let v = out.embeddings.into_iter().next().context("one row, one embedding")?;
        let mut cache = self.embedded.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(line.to_string(), v.clone());
        Ok(v)
    }
}

fn check_question_head(head: &Head) -> anyhow::Result<()> {
    let (classes, questions) = (head.classes.len(), head.questions.len());
    if classes == 0 || classes != questions {
        bail!("the {} head has {classes} classes and {questions} questions", head.name);
    }
    if head.kind == HeadKind::Noul && head.thresholds.len() != classes {
        bail!(
            "the {} head has {} thresholds for {classes} classes",
            head.name,
            head.thresholds.len()
        );
    }
    Ok(())
}

/// What the turn delivers, by the labelling rule (spec/labels.md §7), from
/// the top route even when it shows "unsure".
pub fn derive_result(route: &str, tasks: &[String], effects: &[String]) -> &'static str {
    let has = |list: &[String], names: &[&str]| list.iter().any(|c| names.contains(&c.as_str()));
    if route == "goal" {
        return "continuing_objective";
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

fn sigmoid(z: f32) -> f32 {
    1.0 / (1.0 + (-z).exp())
}

fn softmax(logits: &[f32], temperature: f32) -> Vec<f32> {
    let scaled: Vec<f32> = logits.iter().map(|l| l / temperature).collect();
    let max = scaled.iter().copied().fold(f32::MIN, f32::max);
    let exp: Vec<f32> = scaled.iter().map(|s| (s - max).exp()).collect();
    let sum: f32 = exp.iter().sum();
    exp.iter().map(|e| e / sum).collect()
}

/// The highest cosine between `u` and any of `examples`; 0 with none.
pub fn best_cosine(u: &[f32], examples: &[Vec<f32>]) -> f32 {
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let cos = |v: &[f32]| u.iter().zip(v).map(|(a, b)| a * b).sum::<f32>() / (norm(u) * norm(v));
    examples
        .iter()
        .map(|e| cos(e))
        .fold(None, |m: Option<f32>, c| Some(m.map_or(c, |m| m.max(c))))
        .unwrap_or(0.0)
}

/// A pair head's logit for an Ask's embedding `u` and a rule's: its `when`
/// line's embedding and its examples'.
pub fn rule_logit(head: &PairHead, u: &[f32], when: &[f32], examples: &[Vec<f32>]) -> f32 {
    head.logit(u, when, best_cosine(u, examples))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{"schema": "fab7.sense.bundle/3", "version": "0.3.0", "files": {},
      "heads": [
        {"name": "ask_task", "kind": "noul", "classes": ["question", "plan", "implement"],
         "questions": ["q1", "q2", "q3"], "temperature": 2.0, "thresholds": [0.5, 0.5, 0.9]},
        {"name": "ask_result", "kind": "derived", "classes": ["answer", "plan"]},
        {"name": "ask_effects", "kind": "noul", "classes": ["read", "write"],
         "questions": ["e1", "e2"], "thresholds": [0.5, 0.1]},
        {"name": "ask_route", "kind": "choice", "classes": ["direct", "plan", "goal", "review"],
         "questions": ["r1", "r2", "r3", "r4"], "unsure_below": 0.6},
        {"name": "rule_relevance", "kind": "pair", "weights": "w", "threshold": 0.5}]}"#;

    fn heads() -> AskHeads {
        AskHeads::from_manifest(&Manifest::parse(MANIFEST.as_bytes()).unwrap()).unwrap()
    }

    #[test]
    fn the_batch_asks_every_question_in_the_manifests_head_order() {
        let heads = heads();
        let q: Vec<&str> = heads.questions().collect();
        assert_eq!(q, ["q1", "q2", "q3", "e1", "e2", "r1", "r2", "r3", "r4"]);
    }

    #[test]
    fn each_head_reads_its_logits_by_its_kind_temperature_and_thresholds() {
        // task at T=2: σ(4/2)=0.88 on, σ(0)=0.5 on (at its threshold), σ(2/2)=0.73 under 0.9.
        let logits = [4.0, 0.0, 2.0, 3.0, -3.0, 0.0, 5.0, 0.0, 0.0];
        let a = heads().answer(&logits, &[], Vec::new()).unwrap();
        assert_eq!(a.task.on, ["question", "plan"]);
        assert!((a.task.p["question"] - 0.880_797).abs() < 1e-5);
        assert_eq!(a.effects.on, ["read"], "σ(-3) = 0.047 is under write's 0.1");
        assert_eq!(a.route.pick.as_deref(), Some("plan"));
        let total: f32 = a.route.p.values().sum();
        assert!((total - 1.0).abs() < 1e-5);
        assert_eq!(a.result, "plan");
        assert_eq!(a.sense, "0.3.0");
    }

    #[test]
    fn a_route_below_unsure_below_is_not_picked_and_the_result_still_follows_it() {
        let logits = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0];
        let a = heads().answer(&logits, &[], Vec::new()).unwrap();
        assert_eq!(a.route.pick, None, "goal is top at 0.35, under 0.6");
        assert_eq!(a.result, "continuing_objective");
    }

    #[test]
    fn a_route_the_harness_lacks_is_never_picked_and_the_rest_renormalize() {
        let routes = ["direct", "plan", "review"];
        // Goal 0.84 is out; plan is 0.08 of the 0.16 left, under 0.6.
        let logits = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.7, 3.0, -0.3];
        let a = heads().answer(&logits, &routes, Vec::new()).unwrap();
        assert_eq!(a.route.pick, None);
        assert_eq!(a.result, "continuing_objective", "the result still follows the top route");
        // Plan is 0.94 of what is left.
        let logits = [0.0, 0.0, 0.0, 0.0, 0.0, -2.0, 1.5, 3.0, -2.0];
        let a = heads().answer(&logits, &routes, Vec::new()).unwrap();
        assert_eq!(a.route.pick.as_deref(), Some("plan"));
        assert!(a.route.p["goal"] > 0.7, "the probabilities stay the head's");
    }

    #[test]
    fn the_result_follows_the_labelling_rule() {
        let s = |v: &[&str]| v.iter().map(|c| c.to_string()).collect::<Vec<_>>();
        assert_eq!(derive_result("goal", &s(&["plan"]), &[]), "continuing_objective");
        assert_eq!(derive_result("direct", &s(&["plan"]), &[]), "plan");
        assert_eq!(
            derive_result("direct", &s(&["plan", "implement"]), &s(&["write"])),
            "workspace_change"
        );
        assert_eq!(derive_result("review", &[], &s(&["write"])), "evidence");
        assert_eq!(derive_result("direct", &s(&["diagnose"]), &s(&["read"])), "evidence");
        assert_eq!(derive_result("direct", &s(&["question"]), &s(&["read"])), "answer");
    }

    #[test]
    fn rules_are_proposed_at_or_above_the_threshold_one_per_family_highest_first() {
        let rule = |id: &str, family: &str| Rule {
            id: id.into(),
            family: family.into(),
            when: String::new(),
            examples: Vec::new(),
        };
        let (a, b, c, d) =
            (rule("a", "scope"), rule("b", "scope"), rule("c", "tests"), rule("d", "x"));
        let proposed = heads().propose(vec![(&a, 0.6), (&b, 0.9), (&c, 0.5), (&d, 0.49)]);
        let ids: Vec<&str> = proposed.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"], "b beats a in scope; d is under 0.5");
    }

    #[test]
    fn a_bundle_without_the_ask_heads_is_an_error_naming_one() {
        let bundle_2 = r#"{"schema": "fab7.sense.bundle/2", "version": "0.2.0", "files": {}}"#;
        let m = Manifest::parse(bundle_2.as_bytes()).unwrap();
        let err = AskHeads::from_manifest(&m).err().unwrap().to_string();
        assert!(err.contains("ask_task"), "{err}");
    }

    #[test]
    fn x_is_the_best_cosine_to_the_examples_or_0_without_any() {
        assert_eq!(best_cosine(&[1.0, 0.0], &[]), 0.0);
        let x = best_cosine(&[1.0, 0.0], &[vec![0.0, 1.0], vec![1.0, 1.0]]);
        assert!((x - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }
}
