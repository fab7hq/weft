//! The rounds: the board grouped by Eval round (board.md "Rounds").
//!
//! An Eval covers every Ask open when it was opened, and a Seal on an Eval
//! closes exactly the Asks it covers. So the record reads as rounds: an Eval being judged, the Asks no Eval covers
//! yet (Open), each Eval not sealed with its Asks, and the Seals. Pure, from
//! the ledger's events and the units projected from them; every field traces
//! to a ledger event.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ledger::{Unit, Verdict};
use crate::tree::Why;

/// An Eval on the record, numbered by its `eval.opened` in the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Eval {
    pub id: String,
    pub n: usize,
    pub opened_at: String,
    /// The Asks it covers: `basis.asks`, as completed when it has been.
    pub asks: Vec<String>,
    pub verdict: Option<Verdict>,
    pub agreement: Option<f64>,
    pub judged_by: Option<String>,
    pub completed_at: Option<String>,
    /// Withdrawn by a person: no longer built on or sealed on.
    pub voided: bool,
}

impl Eval {
    pub fn completed(&self) -> bool {
        self.verdict.is_some()
    }
}

/// A Seal on the record, numbered by its `seal.created` in the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seal {
    pub id: String,
    pub n: usize,
    pub at: String,
    pub disposition: String,
    /// Who decided: `you` for the person, else the actor the record names.
    pub by: String,
    /// The Eval it seals: the one its `links` name with `rel: "seals"`.
    pub eval: Option<String>,
}

/// The Evals and the Seals of a ledger, in ledger order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub evals: Vec<Eval>,
    pub seals: Vec<Seal>,
}

/// Read the Evals and the Seals out of a ledger's events.
pub fn record(events: &[Value]) -> Record {
    let text =
        |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let asks = |data: &Value| -> Vec<String> {
        data.pointer("/basis/asks")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let mut out = Record::default();
    for e in events {
        let (id, data) = (text(e, "id"), e.get("data").unwrap_or(&Value::Null));
        match e.get("type").and_then(Value::as_str).unwrap_or_default() {
            "eval.opened" if !out.evals.iter().any(|v| v.id == id) => out.evals.push(Eval {
                n: out.evals.len() + 1,
                opened_at: text(e, "time"),
                asks: asks(data),
                verdict: None,
                agreement: None,
                judged_by: None,
                completed_at: None,
                voided: false,
                id,
            }),
            "eval.completed" => {
                // One whose opening is not on record still counts, numbered here.
                if !out.evals.iter().any(|v| v.id == id) {
                    out.evals.push(Eval {
                        id: id.clone(),
                        n: out.evals.len() + 1,
                        opened_at: text(e, "time"),
                        asks: asks(data),
                        verdict: None,
                        agreement: None,
                        judged_by: None,
                        completed_at: None,
                        voided: false,
                    });
                }
                let Some(v) = out.evals.iter_mut().find(|v| v.id == id) else { continue };
                let Some(verdict) =
                    data.get("verdict").and_then(Value::as_str).and_then(Verdict::of)
                else {
                    continue;
                };
                let covers = asks(data);
                if !covers.is_empty() {
                    v.asks = covers;
                }
                v.verdict = Some(verdict);
                v.agreement = data.get("confidence").and_then(Value::as_f64);
                v.judged_by = data.get("judged_by").and_then(Value::as_str).map(str::to_string);
                v.completed_at = Some(text(e, "time"));
            }
            "eval.voided" => {
                if let Some(v) = out.evals.iter_mut().find(|v| v.id == id) {
                    v.voided = true;
                }
            }
            "seal.created" => {
                let actor = e.get("actor").unwrap_or(&Value::Null);
                let by = if actor.get("kind").and_then(Value::as_str) == Some("human") {
                    "you".to_string()
                } else {
                    text(actor, "id")
                };
                // Its links name each Ask it closed and the Eval it seals,
                // all as `seals`: the Eval is the one that is an Eval.
                let eval = e
                    .get("links")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|l| l.get("rel").and_then(Value::as_str) == Some("seals"))
                    .filter_map(|l| l.get("id").and_then(Value::as_str))
                    .find(|l| out.evals.iter().any(|v| v.id == *l))
                    .map(str::to_string);
                out.seals.push(Seal {
                    n: out.seals.len() + 1,
                    at: text(e, "time"),
                    disposition: text(data, "disposition"),
                    by,
                    eval,
                    id,
                });
            }
            _ => {}
        }
    }
    out
}

/// A round's number and id, as a breadcrumb or a Seal names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    pub id: String,
    pub n: usize,
}

/// One Eval's round: the Asks it covers that are not sealed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Round {
    pub eval: String,
    pub n: usize,
    /// Opened, and not yet completed: being gathered or judged.
    pub judging: bool,
    pub verdict: Option<Verdict>,
    pub agreement: Option<f64>,
    /// Its Asks' ids, newest first.
    pub asks: Vec<String>,
    /// The Seal that sealed it, when one has.
    pub sealed: Option<Named>,
}

/// One Seal's round: the Asks it closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sealed {
    pub seal: String,
    pub n: usize,
    pub decision: String,
    pub at: String,
    pub by: String,
    pub eval: Option<Named>,
    /// Its Asks' ids, newest first.
    pub asks: Vec<String>,
}

/// WORK, as rounds, each in the order it is drawn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rounds {
    /// Evals being gathered or judged, newest first: drawn above Open.
    pub judging: Vec<Round>,
    /// The Asks no Eval covers yet, newest first.
    pub open: Vec<String>,
    /// The newest completed Eval's number: `Open · since Eval n`.
    pub since: Option<usize>,
    /// Each completed Eval with Asks not yet sealed, newest first.
    pub evals: Vec<Round>,
    /// Every Seal, newest first.
    pub seals: Vec<Sealed>,
    /// The Evals and Seals they were grouped by, for their Details.
    #[serde(default)]
    pub record: Record,
}

impl Rounds {
    /// How many Asks the Seals closed: the count `Sealed · n seals` shows.
    pub fn sealed_asks(&self) -> usize {
        self.seals.iter().map(|s| s.asks.len()).sum()
    }

    /// An Eval's round, by its id, judging or not.
    pub fn eval(&self, id: &str) -> Option<&Round> {
        self.judging.iter().chain(&self.evals).find(|r| r.eval == id)
    }
}

/// Group the units into rounds.
pub fn rounds(units: &[Unit], record: &Record) -> Rounds {
    let named = |id: &str| record.evals.iter().find(|e| e.id == id);
    let judging_over = |ask: &str| {
        record
            .evals
            .iter()
            .rev()
            .find(|e| !e.completed() && !e.voided && e.asks.iter().any(|a| a == ask))
    };
    // Newest first by when each was asked; a tie, the later on the board.
    let mut order: Vec<&Unit> = units.iter().filter(|u| !u.cancelled).collect();
    order.reverse();
    order.sort_by(|a, b| b.asked_at.cmp(&a.asked_at));

    let mut out = Rounds {
        since: record.evals.iter().rev().find(|e| e.completed() && !e.voided).map(|e| e.n),
        record: record.clone(),
        ..Rounds::default()
    };
    let mut by_eval: Vec<(&Eval, Vec<String>)> = Vec::new();
    let mut by_seal: Vec<(&str, Vec<String>)> = Vec::new();
    for u in order {
        let id = u.ask_id.clone();
        if let Some(seal) = &u.seal_id {
            match by_seal.iter_mut().find(|(s, _)| s == seal) {
                Some((_, asks)) => asks.push(id),
                None => by_seal.push((seal, vec![id])),
            }
            continue;
        }
        let eval = judging_over(&u.ask_id)
            .or_else(|| u.check.as_ref().and_then(|c| named(&c.eval_id)).filter(|e| !e.voided));
        match eval {
            Some(e) => match by_eval.iter_mut().find(|(v, _)| v.id == e.id) {
                Some((_, asks)) => asks.push(id),
                None => by_eval.push((e, vec![id])),
            },
            None => out.open.push(id),
        }
    }
    let seal_of = |eval: &str| {
        record
            .seals
            .iter()
            .rev()
            .find(|s| s.eval.as_deref() == Some(eval))
            .map(|s| Named { id: s.id.clone(), n: s.n })
    };
    by_eval.sort_by_key(|e| std::cmp::Reverse(e.0.n));
    for (e, asks) in by_eval {
        let round = Round {
            eval: e.id.clone(),
            n: e.n,
            judging: !e.completed(),
            verdict: e.verdict,
            agreement: e.agreement,
            asks,
            sealed: seal_of(&e.id),
        };
        if round.judging { out.judging.push(round) } else { out.evals.push(round) }
    }
    for s in record.seals.iter().rev() {
        let asks = by_seal.iter().find(|(id, _)| *id == s.id).map(|(_, a)| a.clone());
        out.seals.push(Sealed {
            seal: s.id.clone(),
            n: s.n,
            decision: s.disposition.clone(),
            at: s.at.clone(),
            by: s.by.clone(),
            eval: s.eval.as_deref().and_then(named).map(|e| Named { id: e.id.clone(), n: e.n }),
            asks: asks.unwrap_or_default(),
        });
    }
    out
}

/// Why Eval cannot run on the Open round, if it cannot. It sends the debate
/// of an Eval being judged, else opens one on the Open Asks; `ready` is the
/// board's why-not for an Eval with that Ask selected, so it goes where the
/// project routes it, as today.
pub fn eval_why(r: &Rounds, ready: &dyn Fn(Option<&str>) -> Option<Why>) -> Option<Why> {
    let judged = r.judging.first().and_then(|j| j.asks.first());
    let Some(ask) = r.open.first().or(judged) else {
        return Some(Why::new("why.nothing_to_work_on"));
    };
    ready(Some(ask))
}

/// An Eval a Seal names: sealed already when a Seal on the record
/// seals it, however many of its Asks are left.
pub fn sealed_already(record: &Record, id: &str) -> Option<Why> {
    let e = record.evals.iter().find(|e| e.id == id)?;
    record.seals.iter().any(|s| s.eval.as_deref() == Some(id)).then(|| Why {
        key: "why.sealed_already",
        harness: None,
        title: Some(format!("Eval {}", e.n)),
    })
}

/// Why Seal cannot run on this Eval's round, if it cannot: sealed already
/// (D12.6), still being judged, or as the harness it goes to.
pub fn seal_why(round: &Round, ready: &dyn Fn(Option<&str>) -> Option<Why>) -> Option<Why> {
    let named = |key| Some(Why { key, harness: None, title: Some(format!("Eval {}", round.n)) });
    if round.sealed.is_some() {
        return named("why.sealed_already");
    }
    if round.judging {
        return named("why.judging");
    }
    ready(round.asks.first().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::project;
    use serde_json::json;

    fn at(minute: u32) -> String {
        format!("2026-10-08T10:{minute:02}:00Z")
    }

    fn asked(id: &str, minute: u32) -> Value {
        json!({"type": "ask.compiled", "id": id, "time": at(minute),
               "actor": {"kind": "human", "id": "local-user"}, "links": [],
               "data": {"title": format!("t {id}"), "delivery_mode": "native_dispatch",
                        "host": {"name": "codex", "session_ref": "s1",
                                 "session_ref_source": "capture"}}})
    }

    fn requested(id: &str, minute: u32) -> Value {
        json!({"type": "ask.requested", "id": id, "time": at(minute),
               "data": {"title": format!("t {id}"), "host": {"name": "claude-code"}}})
    }

    fn cancelled(id: &str, minute: u32) -> Value {
        json!({"type": "ask.cancelled", "id": id, "time": at(minute), "data": {}})
    }

    fn opened(id: &str, asks: &[&str], minute: u32) -> Value {
        json!({"type": "eval.opened", "id": id, "time": at(minute),
               "data": {"basis": {"asks": asks}}})
    }

    fn completed(id: &str, asks: &[&str], verdict: &str, minute: u32) -> Value {
        json!({"type": "eval.completed", "id": id, "time": at(minute),
               "data": {"basis": {"asks": asks}, "verdict": verdict, "confidence": 0.67,
                        "judged_by": "codex"}})
    }

    fn sealed(id: &str, asks: &[&str], eval: Option<&str>, minute: u32) -> Value {
        let mut links: Vec<Value> = asks.iter().map(|a| json!({"rel": "seals", "id": a})).collect();
        if let Some(e) = eval {
            links.push(json!({"rel": "seals", "id": e}));
        }
        json!({"type": "seal.created", "id": id, "time": at(minute),
               "actor": {"kind": "human", "id": "local-user"}, "links": links,
               "data": {"basis": {"asks": asks}, "disposition": "accepted"}})
    }

    fn grouped(events: &[Value]) -> Rounds {
        rounds(&project(events), &record(events))
    }

    fn ids(r: &Round) -> Vec<&str> {
        r.asks.iter().map(String::as_str).collect()
    }

    #[test]
    fn the_record_numbers_evals_and_seals_in_ledger_order_and_names_a_seals_eval() {
        let events = [
            asked("ask_1", 1),
            opened("evl_a", &["ask_1"], 2),
            completed("evl_a", &["ask_1"], "aligned", 3),
            sealed("sel_a", &["ask_1"], Some("evl_a"), 4),
            asked("ask_2", 5),
            opened("evl_b", &["ask_2"], 6),
            json!({"type": "eval.voided", "id": "evl_b", "time": at(7), "data": {"reason": "x"}}),
            sealed("sel_b", &["ask_2"], None, 8),
        ];
        let r = record(&events);
        let evals: Vec<_> = r.evals.iter().map(|e| (e.id.as_str(), e.n, e.voided)).collect();
        assert_eq!(evals, [("evl_a", 1, false), ("evl_b", 2, true)]);
        let unopened = record(&[asked("ask_1", 1), completed("evl_x", &["ask_1"], "drifted", 2)]);
        assert_eq!(
            (unopened.evals[0].n, unopened.evals[0].verdict),
            (1, Some(Verdict::DoesntMatch))
        );
        assert_eq!(r.evals[0].verdict, Some(Verdict::Matches));
        assert_eq!(r.evals[0].agreement, Some(0.67));
        assert_eq!(r.evals[0].completed_at.as_deref(), Some(at(3).as_str()));
        let seals: Vec<_> =
            r.seals.iter().map(|s| (s.id.as_str(), s.n, s.eval.as_deref())).collect();
        assert_eq!(seals, [("sel_a", 1, Some("evl_a")), ("sel_b", 2, None)]);
        assert_eq!(r.seals[0].by, "you");
    }

    /// An Eval takes every open Ask, an earlier unsealed Eval's among them:
    /// those sit under the later.
    #[test]
    fn an_eval_takes_an_earlier_unsealed_evals_asks() {
        let r = grouped(&[
            asked("ask_1", 1),
            opened("evl_1", &["ask_1"], 2),
            completed("evl_1", &["ask_1"], "drifted", 3),
            asked("ask_2", 4),
            opened("evl_2", &["ask_1", "ask_2"], 5),
            completed("evl_2", &["ask_1", "ask_2"], "aligned", 6),
            asked("ask_3", 7),
        ]);
        assert_eq!(r.evals.len(), 1, "evl_1 has nothing left under it");
        assert_eq!((r.evals[0].n, ids(&r.evals[0])), (2, vec!["ask_2", "ask_1"]));
        assert_eq!((r.open.clone(), r.since), (vec!["ask_3".to_string()], Some(2)));
        assert!(r.judging.is_empty() && r.seals.is_empty());
    }

    #[test]
    fn a_seal_without_an_eval_is_a_round_of_its_own() {
        let r = grouped(&[asked("ask_1", 1), sealed("sel_1", &["ask_1"], None, 2)]);
        assert_eq!(r.seals.len(), 1);
        let s = &r.seals[0];
        assert_eq!((s.n, s.eval.as_ref(), s.asks.clone()), (1, None, vec!["ask_1".to_string()]));
        assert_eq!((r.open.len(), r.since, r.sealed_asks()), (0, None, 1));
    }

    #[test]
    fn asking_is_open_and_an_eval_being_judged_is_its_own_round() {
        let r = grouped(&[
            asked("ask_1", 1),
            requested("ask_2", 2),
            opened("evl_1", &["ask_1"], 3),
            requested("ask_3", 4),
        ]);
        assert_eq!(r.judging.len(), 1);
        let j = &r.judging[0];
        assert_eq!((j.n, j.judging, j.verdict, ids(j)), (1, true, None, vec!["ask_1"]));
        assert_eq!(r.open, ["ask_3", "ask_2"], "ASKING is Open, newest first");
        assert_eq!(r.since, None, "no Eval completed");
        assert!(r.eval("evl_1").is_some_and(|r| r.judging));
    }

    #[test]
    fn a_cancelled_ask_is_in_no_round() {
        let r = grouped(&[
            asked("ask_1", 1),
            asked("ask_2", 2),
            cancelled("ask_2", 3),
            opened("evl_1", &["ask_1", "ask_2"], 4),
            completed("evl_1", &["ask_1", "ask_2"], "aligned", 5),
        ]);
        assert_eq!(ids(&r.evals[0]), ["ask_1"]);
        assert!(r.open.is_empty());
    }

    #[test]
    fn evals_and_seals_are_numbered_after_many_and_drawn_newest_first() {
        let mut events = Vec::new();
        for i in 1..=12u32 {
            let (ask, evl, sel) = (format!("ask_{i}"), format!("evl_{i}"), format!("sel_{i}"));
            events.push(asked(&ask, i));
            events.push(opened(&evl, &[&ask], i));
            events.push(completed(&evl, &[&ask], "aligned", i));
            if i < 11 {
                events.push(sealed(&sel, &[&ask], Some(&evl), i));
            }
        }
        let r = grouped(&events);
        assert_eq!(r.evals.iter().map(|e| e.n).collect::<Vec<_>>(), [12, 11]);
        assert_eq!(r.seals.iter().map(|s| s.n).take(3).collect::<Vec<_>>(), [10, 9, 8]);
        assert_eq!(r.seals[0].eval, Some(Named { id: "evl_10".into(), n: 10 }));
        assert_eq!((r.seals.len(), r.sealed_asks(), r.since), (10, 10, Some(12)));
    }

    #[test]
    fn a_voided_eval_is_no_round_to_seal() {
        let r = grouped(&[
            asked("ask_1", 1),
            opened("evl_1", &["ask_1"], 2),
            completed("evl_1", &["ask_1"], "aligned", 3),
            json!({"type": "eval.voided", "id": "evl_1", "time": at(4), "data": {}}),
        ]);
        assert!(r.evals.is_empty());
        assert_eq!((r.open.clone(), r.since), (vec!["ask_1".to_string()], None));
    }

    fn why(key: &'static str, title: &str) -> Option<Why> {
        Some(Why { key, harness: None, title: Some(title.into()) })
    }

    /// Every why-not of a round: Eval on Open with nothing to judge, Seal on
    /// a sealed Eval (D12.6), on one still being judged, and the harness's.
    #[test]
    fn each_act_on_a_round_says_why_it_cannot_run() {
        let ready: &dyn Fn(Option<&str>) -> Option<Why> = &|_| None;
        assert_eq!(eval_why(&Rounds::default(), ready), Some(Why::new("why.nothing_to_work_on")));
        let r = grouped(&[
            asked("ask_1", 1),
            opened("evl_1", &["ask_1"], 2),
            completed("evl_1", &["ask_1"], "aligned", 3),
            asked("ask_2", 4),
            asked("ask_3", 5),
            opened("evl_2", &["ask_2"], 6),
        ]);
        assert_eq!(eval_why(&r, ready), None);
        // The harness the act goes to is the one its Ask names, as today.
        let picked = std::cell::RefCell::new(Vec::new());
        let named = |u: Option<&str>| {
            picked.borrow_mut().push(u.map(str::to_string));
            Some(Why::new("why.not_set_up"))
        };
        assert_eq!(eval_why(&r, &named), Some(Why::new("why.not_set_up")));
        assert_eq!(picked.borrow().last().cloned().flatten().as_deref(), Some("ask_3"));
        // Only a round being judged: Eval sends its debate.
        let judging_only = Rounds { open: Vec::new(), ..r.clone() };
        assert_eq!(eval_why(&judging_only, ready), None);

        assert_eq!(seal_why(&r.evals[0], ready), None);
        assert_eq!(seal_why(&r.judging[0], ready), why("why.judging", "Eval 2"));
        let mut done = r.evals[0].clone();
        done.sealed = Some(Named { id: "sel_1".into(), n: 4 });
        assert_eq!(seal_why(&done, ready), why("why.sealed_already", "Eval 1"));
        let all = record(&[
            asked("ask_1", 1),
            opened("evl_1", &["ask_1"], 2),
            completed("evl_1", &["ask_1"], "aligned", 3),
            sealed("sel_1", &["ask_1"], Some("evl_1"), 4),
        ]);
        assert_eq!(sealed_already(&all, "evl_1"), why("why.sealed_already", "Eval 1"));
        assert_eq!(sealed_already(&all, "evl_9"), None);
        assert_eq!(seal_why(&r.evals[0], &named), Some(Why::new("why.not_set_up")));
        assert_eq!(
            crate::words::fill("why.sealed_already", &[("title", "Eval 2")]),
            "Eval 2 is sealed already."
        );
    }
}
