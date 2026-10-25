//! Done: every sealed unit of work, found by its title, verdict, decision or
//! reason. Parked work stays at the top with its reason; the rest follow,
//! newest first. No dashboard: a list and one search.

use serde::{Deserialize, Serialize};

use crate::ledger::Unit;
use crate::work::Work;

/// One sealed unit as Done lists it, in the words a person reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Done {
    /// Its first Ask.
    pub unit: String,
    pub title: String,
    pub harness: String,
    /// The verdict of the Eval it was sealed on, as recorded: `aligned`,
    /// `drifted`, `incomplete`; none when it was sealed on no Eval.
    pub verdict: Option<String>,
    /// accepted, parked, rejected or dropped.
    pub decision: String,
    /// Why, when a reason was given: `broke something`.
    pub reason: Option<String>,
    /// When it was sealed, as the ledger writes a time.
    pub at: String,
    pub parked: bool,
}

/// Every sealed unit of `work`: parked ones first, then the rest, each
/// newest first.
pub fn done(work: &Work, units: &[Unit]) -> Vec<Done> {
    let mut rows: Vec<Done> = work
        .units
        .iter()
        .filter(|u| u.sealed())
        .filter_map(|u| {
            let seal =
                u.seals.last().and_then(|id| work.record.seals.iter().find(|s| &s.id == id))?;
            let root = units.iter().find(|a| a.ask_id == u.root)?;
            let verdict = seal
                .eval
                .as_deref()
                .and_then(|e| work.record.evals.iter().find(|v| v.id == e))
                .and_then(|e| e.verdict)
                .map(|v| v.recorded().to_string());
            let reason = seal
                .reason
                .as_deref()
                .map(|r| crate::words::word(&format!("seal.r.{r}")).to_string());
            Some(Done {
                unit: u.root.clone(),
                title: root.title.clone(),
                harness: root.harness.clone(),
                verdict,
                decision: crate::tree::decision(&seal.disposition),
                reason,
                at: seal.at.clone(),
                parked: seal.disposition == "deferred",
            })
        })
        .collect();
    rows.sort_by(|a, b| b.parked.cmp(&a.parked).then(b.at.cmp(&a.at)));
    rows
}

/// Whether a row holds every word of `q`, ignoring case, in its title,
/// harness, verdict, decision or reason.
pub fn matches(d: &Done, q: &str) -> bool {
    let text = [
        d.title.as_str(),
        d.harness.as_str(),
        d.verdict.as_deref().unwrap_or_default(),
        d.decision.as_str(),
        d.reason.as_deref().unwrap_or_default(),
    ]
    .join(" ")
    .to_lowercase();
    q.to_lowercase().split_whitespace().all(|w| text.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::project;
    use serde_json::{Value, json};

    fn at(minute: u32) -> String {
        format!("2026-10-25T10:{minute:02}:00Z")
    }

    fn asked(id: &str, title: &str, minute: u32) -> Value {
        json!({"type": "ask.compiled", "id": id, "time": at(minute), "links": [],
               "actor": {"kind": "human", "id": "local-user"},
               "data": {"title": title, "delivery_mode": "native_dispatch",
                        "host": {"name": "codex"}}})
    }

    /// An Ask judged `verdict` and sealed as `disposition`, for `reason`.
    fn sealed(
        ask: &str,
        n: u32,
        verdict: &str,
        disposition: &str,
        reason: Option<&str>,
    ) -> Vec<Value> {
        let (eval, seal) = (format!("evl_{n}"), format!("sel_{n}"));
        let basis = json!({"asks": [ask]});
        let mut data = json!({"basis": basis, "disposition": disposition});
        if let Some(r) = reason {
            data["reason"] = json!(r);
        }
        vec![
            json!({"type": "eval.opened", "id": eval, "time": at(n), "data": {"basis": basis}}),
            json!({"type": "eval.completed", "id": eval, "time": at(n),
                   "data": {"basis": basis, "verdict": verdict}}),
            json!({"type": "seal.created", "id": seal, "time": at(n + 1),
                   "actor": {"kind": "human", "id": "local-user"},
                   "links": [{"rel": "seals", "id": ask}, {"rel": "seals", "id": eval}],
                   "data": data}),
        ]
    }

    fn rows() -> Vec<Done> {
        let mut events = vec![
            asked("ask_1", "health endpoint", 1),
            asked("ask_2", "readme fix", 2),
            asked("ask_3", "build number", 3),
            asked("ask_4", "still open", 4),
        ];
        events.extend(sealed("ask_1", 10, "drifted", "rejected", Some("broke_something")));
        events.extend(sealed("ask_2", 20, "aligned", "deferred", Some("waiting_on_something")));
        events.extend(sealed("ask_3", 30, "aligned", "accepted", None));
        let units = project(&events);
        done(&crate::work::work(&units, &crate::work::record(&events)), &units)
    }

    #[test]
    fn done_lists_sealed_work_parked_first_then_newest_in_plain_words() {
        let rows = rows();
        let listed: Vec<_> = rows.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(
            listed,
            ["readme fix", "build number", "health endpoint"],
            "open work is not done"
        );
        let rejected = &rows[2];
        assert_eq!(rejected.verdict.as_deref(), Some("drifted"));
        assert_eq!(rejected.decision, "rejected");
        assert_eq!(rejected.reason.as_deref(), Some("broke something"));
        assert_eq!((rows[0].decision.as_str(), rows[0].parked), ("parked", true));
        assert_eq!(rows[1].reason, None);
    }

    #[test]
    fn done_is_searched_by_title_verdict_decision_or_reason() {
        let rows = rows();
        let found = |q: &str| -> Vec<String> {
            rows.iter().filter(|d| matches(d, q)).map(|d| d.title.clone()).collect()
        };
        assert_eq!(found("Broke"), ["health endpoint"], "by reason");
        assert_eq!(found("parked"), ["readme fix"], "by decision");
        assert_eq!(found("aligned"), ["readme fix", "build number"], "by verdict");
        assert_eq!(found("build"), ["build number"], "by title");
        assert_eq!(found("rejected drifted"), ["health endpoint"], "every word");
        assert_eq!(found("").len(), 3, "nothing typed lists all");
        assert!(found("superseded").is_empty());
    }
}
