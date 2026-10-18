//! WORK: the record as units of work.
//!
//! A unit of work is an Ask that follows nothing, and every Ask that follows,
//! revises or remediates it; a link to an Eval reaches the Asks that Eval
//! judged. Its Evals and its Seals are the ones over its Asks. Pure, from the
//! ledger's events and the Asks projected from them; every field traces to a
//! ledger event.

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
    /// Left open, and a later Eval over the same Asks superseded it.
    #[serde(default)]
    pub superseded: bool,
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
        let kind = e.get("type").and_then(Value::as_str).unwrap_or_default();
        if kind == "eval.opened" {
            let gone = e
                .get("links")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|l| l.get("rel").and_then(Value::as_str) == Some("supersedes"))
                .filter_map(|l| l.get("id").and_then(Value::as_str));
            for old in gone {
                if let Some(v) = out.evals.iter_mut().find(|v| v.id == old) {
                    v.superseded = true;
                }
            }
        }
        match kind {
            "eval.opened" if !out.evals.iter().any(|v| v.id == id) => out.evals.push(Eval {
                n: out.evals.len() + 1,
                opened_at: text(e, "time"),
                asks: asks(data),
                verdict: None,
                agreement: None,
                judged_by: None,
                completed_at: None,
                voided: false,
                superseded: false,
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
                        superseded: false,
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

/// Where a unit of work stands, in one status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    /// Its newest Ask is still being composed.
    Asking,
    /// Asked, and no Eval has judged its newest Asks.
    Asked,
    /// Its newest Eval is open: not judged yet.
    Judging,
    /// Its newest Eval's verdict, and not sealed.
    Judged {
        #[serde(with = "recorded")]
        verdict: Verdict,
        agreement: Option<f64>,
    },
    /// Sealed: what was decided, as recorded.
    Sealed { decision: String },
}

/// A verdict as the record writes it: `aligned`, `drifted`, `incomplete`.
mod recorded {
    use crate::ledger::Verdict;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(v: &Verdict, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(v.recorded())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Verdict, D::Error> {
        let text = String::deserialize(d)?;
        Verdict::of(&text).ok_or_else(|| D::Error::custom(format!("no verdict {text}")))
    }
}

/// One unit of work: its Asks, Evals and Seals, and where it stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitOfWork {
    /// The Ask it started with: its title and harness are the unit's.
    pub root: String,
    /// Its Asks, oldest first: the root, then each follow-up. None cancelled.
    pub asks: Vec<String>,
    /// Its Asks no Seal has closed, oldest first: what Eval and Seal act on.
    pub open: Vec<String>,
    /// Its Evals, oldest first. None voided.
    pub evals: Vec<String>,
    /// Its Seals, oldest first.
    pub seals: Vec<String>,
    pub status: Status,
    /// When it last moved, as the ledger writes a time.
    pub at: String,
    /// The Ask an act on it goes by: its newest open one that is composed.
    #[serde(default)]
    pub route: Option<String>,
    /// The id a follow-up on it carries: its newest Eval, else its newest
    /// composed Ask.
    #[serde(default)]
    pub follows: String,
}

impl UnitOfWork {
    pub fn sealed(&self) -> bool {
        matches!(self.status, Status::Sealed { .. })
    }
}

/// WORK: the units, open ones first, then the sealed, each newest first;
/// and the Evals and Seals they were built from, for their Details.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Work {
    pub units: Vec<UnitOfWork>,
    pub record: Record,
}

impl Work {
    /// The unit an Ask is in.
    pub fn of_ask(&self, ask: &str) -> Option<&UnitOfWork> {
        self.units.iter().find(|u| u.asks.iter().any(|a| a == ask) || u.root == ask)
    }

    /// The unit an Eval judged.
    pub fn of_eval(&self, eval: &str) -> Option<&UnitOfWork> {
        self.units.iter().find(|u| u.evals.iter().any(|e| e == eval))
    }

    /// Whether Eval on this unit goes on with the open Eval `id`: only its
    /// own, its newest, over none but its Asks.
    pub fn goes_on_with(&self, unit: &UnitOfWork, id: &str) -> bool {
        unit.evals.last().is_some_and(|e| e == id)
            && self
                .record
                .evals
                .iter()
                .find(|e| e.id == id)
                .is_some_and(|e| e.asks.iter().all(|a| unit.asks.contains(a)))
    }

    /// The Eval a Seal on this unit seals: its newest, once judged, while
    /// it judged every Ask still open.
    pub fn to_seal(&self, unit: &UnitOfWork) -> Option<&Eval> {
        matches!(unit.status, Status::Judged { .. })
            .then(|| unit.evals.last())
            .flatten()
            .and_then(|id| self.record.evals.iter().find(|e| &e.id == id))
    }
}

/// Group the Asks into units of work.
pub fn work(units: &[Unit], record: &Record) -> Work {
    let ask = |id: &str| units.iter().find(|u| u.ask_id == id);
    // What an Ask follows: the Ask, or the first Ask the Eval it names judged.
    let parent = |u: &Unit| -> Option<&Unit> {
        let to = u.follows.as_deref()?;
        match record.evals.iter().find(|e| e.id == to) {
            Some(e) => e.asks.iter().find_map(|a| ask(a)),
            None => ask(to),
        }
    };
    let root = |u: &Unit| -> String {
        let mut at = u;
        let mut seen = vec![u.ask_id.as_str()];
        while let Some(up) = parent(at).filter(|up| !seen.contains(&up.ask_id.as_str())) {
            seen.push(&up.ask_id);
            at = up;
        }
        at.ask_id.clone()
    };
    let mut roots: Vec<(String, Vec<&Unit>)> = Vec::new();
    for u in units {
        let r = root(u);
        match roots.iter_mut().find(|(id, _)| *id == r) {
            Some((_, mine)) => mine.push(u),
            None => roots.push((r, vec![u])),
        }
    }
    let mut out: Vec<UnitOfWork> = roots
        .into_iter()
        .filter_map(|(root, mut mine)| {
            mine.retain(|u| !u.cancelled);
            mine.sort_by(|a, b| a.asked_at.cmp(&b.asked_at));
            let newest = *mine.last()?;
            let covers = |e: &&Eval| e.asks.iter().any(|a| mine.iter().any(|u| &u.ask_id == a));
            let evals: Vec<&Eval> =
                record.evals.iter().filter(|e| !e.voided && !e.superseded).filter(covers).collect();
            let seals: Vec<&Seal> = record
                .seals
                .iter()
                .filter(|s| {
                    mine.iter().any(|u| u.seal_id.as_deref() == Some(s.id.as_str()))
                        || s.eval.as_deref().is_some_and(|e| evals.iter().any(|v| v.id == e))
                })
                .collect();
            let open: Vec<String> =
                mine.iter().filter(|u| u.seal_id.is_none()).map(|u| u.ask_id.clone()).collect();
            let last = evals.last();
            let status = if open.is_empty() {
                Status::Sealed {
                    decision: seals.last().map(|s| s.disposition.clone()).unwrap_or_default(),
                }
            } else if last
                .is_some_and(|e| !e.completed() && e.asks.iter().any(|a| open.contains(a)))
            {
                Status::Judging
            } else if let Some((e, verdict)) = last
                .filter(|e| open.iter().all(|a| e.asks.contains(a)))
                .and_then(|e| Some((e, e.verdict?)))
            {
                Status::Judged { verdict, agreement: e.agreement }
            } else if mine
                .iter()
                .rev()
                .find(|u| u.seal_id.is_none())
                .is_some_and(|u| u.requested_only)
            {
                Status::Asking
            } else {
                Status::Asked
            };
            let at = mine
                .iter()
                .map(|u| u.asked_at.as_str())
                .chain(evals.iter().map(|e| e.completed_at.as_deref().unwrap_or(&e.opened_at)))
                .chain(seals.iter().map(|s| s.at.as_str()))
                .max()
                .unwrap_or(&newest.asked_at)
                .to_string();
            let composed = |u: &&&Unit| !u.requested_only;
            let route = mine
                .iter()
                .rev()
                .filter(|u| u.seal_id.is_none())
                .find(composed)
                .map(|u| u.ask_id.clone());
            let follows = evals
                .last()
                .map(|e| e.id.clone())
                .or_else(|| mine.iter().rev().find(composed).map(|u| u.ask_id.clone()))
                .unwrap_or_else(|| root.clone());
            Some(UnitOfWork {
                route,
                follows,
                root,
                asks: mine.iter().map(|u| u.ask_id.clone()).collect(),
                open,
                evals: evals.iter().map(|e| e.id.clone()).collect(),
                seals: seals.iter().map(|s| s.id.clone()).collect(),
                status,
                at,
            })
        })
        .collect();
    // Open first, then sealed; each newest first, a tie the later on the ledger.
    out.reverse();
    out.sort_by(|a, b| a.sealed().cmp(&b.sealed()).then(b.at.cmp(&a.at)));
    Work { units: out, record: record.clone() }
}

/// Why Eval cannot run on this unit, if it cannot. It opens an Eval of its
/// open Asks, or sends the debate of the one being judged; `ready` is the
/// board's why-not for an Eval with that Ask selected, so it goes where the
/// project routes it.
pub fn eval_why(
    unit: &UnitOfWork,
    root: &Unit,
    ready: &dyn Fn(Option<&str>) -> Option<Why>,
) -> Option<Why> {
    match (unit.open.is_empty(), &unit.route) {
        (true, _) => Some(Why::about("why.closed", root)),
        (false, None) => Some(Why::about("why.composing", root)),
        (false, Some(ask)) => ready(Some(ask)),
    }
}

/// Why Seal cannot run on this unit, if it cannot: sealed, still being
/// judged, not judged since its last Ask, or as the harness it goes to.
pub fn seal_why(
    w: &Work,
    unit: &UnitOfWork,
    root: &Unit,
    ready: &dyn Fn(Option<&str>) -> Option<Why>,
) -> Option<Why> {
    let named = |key| Some(Why::about(key, root));
    match unit.status {
        Status::Sealed { .. } => named("why.closed"),
        Status::Judging => named("why.judging"),
        Status::Asking | Status::Asked if unit.evals.is_empty() => named("why.no_eval"),
        Status::Asking | Status::Asked => named("why.eval_again"),
        Status::Judged { .. } => match w.to_seal(unit) {
            Some(_) => ready(unit.route.as_deref()),
            None => named("why.no_eval"),
        },
    }
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

    fn following(mut e: Value, to: &str) -> Value {
        e["links"] = json!([{"rel": "follows", "id": to}]);
        e
    }

    fn grouped(events: &[Value]) -> Work {
        work(&project(events), &record(events))
    }

    fn roots(w: &Work) -> Vec<&str> {
        w.units.iter().map(|u| u.root.as_str()).collect()
    }

    /// A new Ask starts a unit; a follow-up joins the one it follows, also
    /// through an Eval it names, and also when it follows a follow-up.
    #[test]
    fn a_follow_up_joins_the_unit_it_follows_and_a_new_ask_starts_one() {
        let w = grouped(&[
            asked("ask_1", 1),
            asked("ask_2", 2),
            opened("evl_1", &["ask_1"], 3),
            completed("evl_1", &["ask_1"], "drifted", 4),
            following(asked("ask_3", 5), "evl_1"),
            following(requested("ask_4", 6), "ask_3"),
        ]);
        assert_eq!(roots(&w), ["ask_1", "ask_2"], "newest first by when it last moved");
        let u = &w.units[0];
        assert_eq!(u.asks, ["ask_1", "ask_3", "ask_4"]);
        assert_eq!((u.evals.clone(), u.status.clone()), (vec!["evl_1".into()], Status::Asking));
        assert_eq!(w.of_ask("ask_4").map(|u| u.root.as_str()), Some("ask_1"));
        assert_eq!(w.of_eval("evl_1").map(|u| u.root.as_str()), Some("ask_1"));
        assert_eq!(w.units[1].status, Status::Asked);
    }

    /// One status each: judging while an Eval of it is open, its verdict once
    /// that Eval judged every Ask still open, sealed once nothing is open.
    #[test]
    fn a_unit_has_one_status() {
        let status = |events: &[Value]| grouped(events).units[0].status.clone();
        let mut events = vec![asked("ask_1", 1), opened("evl_1", &["ask_1"], 2)];
        assert_eq!(status(&events), Status::Judging);
        events.push(completed("evl_1", &["ask_1"], "drifted", 3));
        let judged = Status::Judged { verdict: Verdict::DoesntMatch, agreement: Some(0.67) };
        assert_eq!(status(&events), judged);
        events.push(sealed("sel_1", &["ask_1"], Some("evl_1"), 4));
        assert_eq!(status(&events), Status::Sealed { decision: "accepted".into() });
        events.push(following(asked("ask_2", 5), "ask_1"));
        let w = grouped(&events);
        assert_eq!((w.units.len(), w.units[0].status.clone()), (1, Status::Asked));
        assert_eq!(
            (w.units[0].open.clone(), w.units[0].seals.clone()),
            (vec!["ask_2".into()], vec!["sel_1".into()])
        );
    }

    /// Open units come first, then the sealed, each newest first. A cancelled
    /// Ask is in no unit; a voided Eval is in none.
    #[test]
    fn open_units_come_before_sealed_ones_and_cancelled_asks_are_left_out() {
        let w = grouped(&[
            asked("ask_1", 1),
            asked("ask_2", 2),
            opened("evl_2", &["ask_2"], 3),
            completed("evl_2", &["ask_2"], "aligned", 4),
            sealed("sel_2", &["ask_2"], Some("evl_2"), 5),
            asked("ask_3", 6),
            cancelled("ask_3", 7),
            opened("evl_1", &["ask_1"], 8),
            json!({"type": "eval.voided", "id": "evl_1", "time": at(9), "data": {}}),
        ]);
        assert_eq!(roots(&w), ["ask_1", "ask_2"]);
        assert!(w.units[0].evals.is_empty() && w.units[0].status == Status::Asked);
        assert!(w.units[1].sealed());
    }

    /// An Eval left open when a later one superseded it judges nothing:
    /// the unit takes the later one's verdict.
    #[test]
    fn a_superseded_eval_leaves_its_unit_to_the_one_after_it() {
        let mut later = opened("evl_2", &["ask_1"], 3);
        later["links"] = json!([{"rel": "supersedes", "id": "evl_1"}]);
        let w = grouped(&[
            asked("ask_1", 1),
            opened("evl_1", &["ask_1"], 2),
            later,
            completed("evl_2", &["ask_1"], "aligned", 4),
        ]);
        let u = &w.units[0];
        assert_eq!(u.evals, ["evl_2"]);
        assert!(matches!(u.status, Status::Judged { .. }), "{:?}", u.status);
        assert!(w.record.evals[0].superseded);
        // An Eval left open over Asks a Seal has since closed judges nothing open.
        let w = grouped(&[
            asked("ask_1", 1),
            opened("evl_1", &["ask_1"], 2),
            sealed("sel_1", &["ask_1"], None, 3),
            following(asked("ask_2", 4), "ask_1"),
        ]);
        assert_eq!(w.units[0].status, Status::Asked);
    }

    /// A unit stands where its newest Eval does: an older Eval left open,
    /// one opened over more work before units, says nothing of it now.
    #[test]
    fn a_unit_stands_where_its_newest_eval_does() {
        let w = grouped(&[
            asked("ask_1", 1),
            asked("ask_2", 2),
            opened("evl_1", &["ask_1", "ask_2"], 3),
            opened("evl_2", &["ask_1"], 4),
            completed("evl_2", &["ask_1"], "aligned", 5),
        ]);
        let one = w.of_ask("ask_1").unwrap();
        assert!(matches!(one.status, Status::Judged { .. }), "{:?}", one.status);
        assert_eq!(w.to_seal(one).map(|e| e.id.as_str()), Some("evl_2"));
        assert_eq!(w.of_ask("ask_2").unwrap().status, Status::Judging, "evl_1 is still its newest");
    }

    /// Eval goes on with an open Eval only when it is the unit's own: its
    /// newest, over none but its Asks.
    #[test]
    fn eval_goes_on_only_with_the_units_own_eval() {
        let w = grouped(&[
            asked("ask_1", 1),
            asked("ask_2", 2),
            opened("evl_1", &["ask_1", "ask_2"], 3),
            asked("ask_3", 4),
            opened("evl_2", &["ask_3"], 5),
        ]);
        let one = w.of_ask("ask_1").unwrap();
        assert!(!w.goes_on_with(one, "evl_1"), "evl_1 judges ask_2's unit too");
        let three = w.of_ask("ask_3").unwrap();
        assert!(w.goes_on_with(three, "evl_2"));
        assert!(!w.goes_on_with(three, "evl_1"));
    }

    /// Eval and a follow-up go by an Ask that is composed: one still being
    /// composed is not yet one RingFrame can name.
    #[test]
    fn eval_and_a_follow_up_go_by_a_composed_ask() {
        let w = grouped(&[asked("ask_1", 1), following(requested("ask_2", 2), "ask_1")]);
        let u = &w.units[0];
        assert_eq!((u.route.as_deref(), u.follows.as_str()), (Some("ask_1"), "ask_1"));
        let w = grouped(&[requested("ask_1", 1)]);
        let units = project(&[requested("ask_1", 1)]);
        let picked = std::cell::RefCell::new(None);
        let ready = |a: Option<&str>| {
            *picked.borrow_mut() = a.map(str::to_string);
            None
        };
        assert_eq!(eval_why(&w.units[0], &units[0], &ready).map(|w| w.key), Some("why.composing"));
        assert_eq!(*picked.borrow(), None, "nothing to route by");
        let w = grouped(&[
            asked("ask_1", 1),
            opened("evl_1", &["ask_1"], 2),
            completed("evl_1", &["ask_1"], "drifted", 3),
        ]);
        assert_eq!(w.units[0].follows, "evl_1");
    }

    fn why(key: &'static str, title: &str) -> Option<Why> {
        Some(Why { key, harness: Some("codex".into()), title: Some(title.into()) })
    }

    /// The root Ask the why-nots name: titled `t`.
    fn t() -> Unit {
        Unit { title: "t".into(), harness: "codex".into(), ..Default::default() }
    }

    /// Eval and Seal act on one unit, and say why when they cannot.
    #[test]
    fn each_act_on_a_unit_says_why_it_cannot_run() {
        let ready: &dyn Fn(Option<&str>) -> Option<Why> = &|_| None;
        let mut events = vec![asked("ask_1", 1)];
        let w = grouped(&events);
        let u = &w.units[0];
        assert_eq!(eval_why(u, &t(), ready), None);
        assert_eq!(seal_why(&w, u, &t(), ready), why("why.no_eval", "t"));

        events.push(opened("evl_1", &["ask_1"], 2));
        let w = grouped(&events);
        assert_eq!(seal_why(&w, &w.units[0], &t(), ready), why("why.judging", "t"));

        events.push(completed("evl_1", &["ask_1"], "aligned", 3));
        let w = grouped(&events);
        assert_eq!(w.to_seal(&w.units[0]).map(|e| e.id.as_str()), Some("evl_1"));
        // The harness the act goes to is the one its newest open Ask names.
        let picked = std::cell::RefCell::new(None);
        let named = |a: Option<&str>| {
            *picked.borrow_mut() = a.map(str::to_string);
            Some(Why::new("why.not_set_up"))
        };
        assert_eq!(seal_why(&w, &w.units[0], &t(), &named), Some(Why::new("why.not_set_up")));
        assert_eq!(picked.borrow().as_deref(), Some("ask_1"));

        events.push(following(asked("ask_2", 4), "evl_1"));
        let w = grouped(&events);
        assert_eq!(seal_why(&w, &w.units[0], &t(), ready), why("why.eval_again", "t"));
        assert_eq!(eval_why(&w.units[0], &t(), &named), Some(Why::new("why.not_set_up")));
        assert_eq!(picked.borrow().as_deref(), Some("ask_2"));

        events.push(sealed("sel_1", &["ask_1", "ask_2"], None, 5));
        let w = grouped(&events);
        assert_eq!(eval_why(&w.units[0], &t(), ready), why("why.closed", "t"));
        assert_eq!(seal_why(&w, &w.units[0], &t(), ready), why("why.closed", "t"));
    }
}
