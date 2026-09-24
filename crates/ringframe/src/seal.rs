//! Seal: the person's decision to close the open Asks. It records the latest
//! Eval as a fact and gates nothing.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::store::LedgerError;
use crate::workspace::Workspace;
use crate::{ask, digest, evaluate, ids, schema, sessions, store};

pub const DISPOSITIONS: [&str; 4] = ["accepted", "rejected", "deferred", "abandoned"];

fn limitations() -> Vec<String> {
    vec![
        "a Seal is a decision record; it does not merge, publish, deploy, or certify correctness"
            .to_string(),
        "the Eval it records is a judgement with a confidence, not a gate".to_string(),
    ]
}

/// The Seal was not written, and the codes say what stopped it. The refusal
/// itself is recorded.
#[derive(Debug)]
pub struct Refused {
    pub codes: Vec<String>,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.codes.join(", "))
    }
}

impl std::error::Error for Refused {}

#[derive(Debug)]
pub enum SealError {
    Refused(Refused),
    Other(ask::AskError),
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SealError::Refused(e) => write!(f, "{e}"),
            SealError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SealError {}

impl<T: Into<ask::AskError>> From<T> for SealError {
    fn from(e: T) -> Self {
        SealError::Other(e.into())
    }
}

fn ledger(code: &str, detail: impl Into<String>) -> SealError {
    SealError::Other(ask::AskError::Ledger(LedgerError::new_public(code, detail)))
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Interactive humans decide by being present; any other actor needs a
/// pre-authorization record.
fn authority(
    ws: &Workspace,
    actor: &Value,
    disposition: &str,
    subject_kind: &Value,
) -> (Option<String>, Option<String>) {
    let declared = {
        let a = str_of(actor, "authority");
        if a.is_empty() { "interactive".to_string() } else { a }
    };
    if str_of(actor, "kind") == "human" && declared == "interactive" {
        return (Some("interactive".into()), None);
    }
    let id = str_of(actor, "id");
    let path = ws.rf_dir().join("authorizations").join(format!("{id}.json"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (None, Some("seal.authority_missing".into()));
    };
    let grant: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let allowed = grant.get("allowed").cloned().unwrap_or_else(|| json!({}));
    let expires = grant.get("expires").and_then(Value::as_str);
    let live = expires.is_none()
        || expires.and_then(sessions::parse_time).is_some_and(|e| e > sessions::now_millis());
    let ok = grant.get("actor").and_then(Value::as_str)
        == Some(&format!("{}:{id}", str_of(actor, "kind")))
        && strings(&allowed, "dispositions").iter().any(|d| d == disposition)
        && allowed
            .get("subject_kinds")
            .and_then(Value::as_array)
            .is_some_and(|k| k.contains(subject_kind))
        && live;
    if ok {
        (Some(format!("preauthorized:authorizations/{id}.json")), None)
    } else {
        (None, Some("seal.authority_missing".into()))
    }
}

fn eval_fact(ws: &Workspace, record: Option<&Value>, subject_sha: &Value) -> Value {
    let Some(record) = record else { return Value::Null };
    let eval_id = str_of(record, "eval_id");
    let path = ws.rf_dir().join(format!("evals/{eval_id}/record.json"));
    let age = sessions::parse_time(&str_of(record, "time"))
        .map_or(0, |t| (sessions::now_millis() - t) / 1000);
    json!({
        "eval_id": eval_id, "verdict": record["verdict"], "confidence": record["confidence"],
        "sha256": digest::sha256_file(&path).unwrap_or_default(),
        "subject_matches": record["subject"]["sha256"] == *subject_sha,
        "age_s": age,
    })
}

pub fn create(
    ws: &Workspace,
    disposition: &str,
    eval_id: Option<&str>,
    note: Option<&str>,
    actor: Option<&Value>,
) -> Result<Value, SealError> {
    if !DISPOSITIONS.contains(&disposition) {
        return Err(ledger(
            "seal.disposition",
            format!(
                "'{disposition}' not in ({})",
                DISPOSITIONS.iter().map(|d| format!("'{d}'")).collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    let actor = actor.cloned().unwrap_or_else(
        || json!({"kind": "human", "id": "local-user", "authority": "interactive"}),
    );
    let mut codes: Vec<String> = Vec::new();
    let asks = ask::open_asks(ws)?;
    let ask_ids: Vec<String> = asks.iter().map(|a| str_of(a, "ask_id")).collect();
    if ask_ids.is_empty() {
        codes.push("seal.no_open_ask".into());
    }
    // Not a Git repository: the receipt still records the decision.
    let subject = match evaluate::default_subject(ws) {
        Ok((kind, reference)) => match evaluate::subject_digest(ws, &kind, &reference) {
            Ok(sha) => json!({"kind": kind, "ref": reference, "sha256": sha}),
            Err(_) => json!({"kind": null, "ref": null, "sha256": null}),
        },
        Err(_) => json!({"kind": null, "ref": null, "sha256": null}),
    };
    let mut record = match eval_id {
        Some(id) => evaluate::load_record(ws, id)?,
        None => evaluate::latest_for(ws, &ask_ids)?,
    };
    if eval_id.is_some() {
        match &record {
            None => codes.push("seal.eval_missing".into()),
            Some(r) => {
                let basis: BTreeSet<String> = strings(&r["basis"], "asks").into_iter().collect();
                if !ask_ids.iter().any(|a| basis.contains(a)) {
                    codes.push("seal.eval_unrelated".into());
                    record = None;
                }
            }
        }
    }
    let (granted, code) = authority(ws, &actor, disposition, &subject["kind"]);
    if let Some(code) = code {
        codes.push(code);
    }
    let seal_id = ids::new_id("sel");
    let fact = eval_fact(ws, record.as_ref(), &subject["sha256"]);
    let mut authority_block = actor.clone();
    authority_block["authority"] = granted
        .map_or_else(|| actor.get("authority").cloned().unwrap_or(Value::Null), Value::String);
    let mut base = json!({
        "basis": {"asks": ask_ids}, "eval": fact, "subject": subject,
        "disposition": disposition, "authority": authority_block,
    });
    if let Some(note) = note.filter(|n| !n.is_empty()) {
        base["note"] = json!(note);
    }
    let mut links: Vec<Value> = base["basis"]["asks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|a| json!({"rel": "seals", "id": a}))
        .collect();
    if let Some(r) = &record {
        links.push(json!({"rel": "seals", "id": r["eval_id"]}));
    }
    if !codes.is_empty() {
        let mut data = base.clone();
        data["refusal_codes"] = json!(codes);
        let ev = json!({
            "schema": store::SCHEMA, "event_id": ids::new_id("evt"), "type": "seal.refused",
            "time": sessions::now(), "id": seal_id, "actor": actor, "links": links, "data": data
        });
        schema::validate_event(&ev)?;
        store::append(ws, &ev)?;
        return Err(SealError::Refused(Refused { codes }));
    }
    let mut limits = limitations();
    if base["eval"].is_null() {
        limits.push("no Eval over these Asks; the decision rests on the person alone".into());
    } else if base["eval"]["subject_matches"] != json!(true) {
        limits.push(
            "the subject changed after the Eval; its verdict describes an earlier state".into(),
        );
    }
    let time = sessions::now();
    let mut receipt = json!({"schema": "ringframe.seal/1", "seal_id": seal_id});
    for (k, v) in base.as_object().into_iter().flatten() {
        receipt[k] = v.clone();
    }
    receipt["asks"] = json!(
        asks.iter()
            .map(|a| json!({"ask_id": a["ask_id"], "title": a["title"]}))
            .collect::<Vec<_>>()
    );
    receipt["limitations"] = json!(limits);
    receipt["time"] = json!(time);
    let mut bytes = store::canonical(&receipt);
    bytes.push(b'\n');
    let reference = store::publish(ws, &format!("seals/{seal_id}.json"), &bytes, "seal_receipt")?;
    let mut data = base;
    data["artifact"] = reference;
    let ev = json!({
        "schema": store::SCHEMA, "event_id": ids::new_id("evt"), "type": "seal.created",
        "time": time, "id": seal_id, "actor": actor, "links": links, "data": data
    });
    schema::validate_event(&ev)?;
    store::append(ws, &ev)?;
    Ok(receipt)
}

/// Re-verify the receipt and its Eval record now, and say whether the subject
/// still matches. It decides nothing.
pub fn check(ws: &Workspace, seal_id: &str) -> Result<Value, SealError> {
    let mut codes: Vec<String> = Vec::new();
    let p = ws.rf_dir().join(format!("seals/{seal_id}.json"));
    let created = store::events(ws)?
        .into_iter()
        .find(|e| e["type"] == "seal.created" && str_of(e, "id") == seal_id);
    let (Some(created), true) = (created, p.exists()) else {
        return Ok(json!({"seal_id": seal_id, "fresh": false, "codes": ["seal.receipt_missing"]}));
    };
    if digest::sha256_file(&p).unwrap_or_default() != str_of(&created["data"]["artifact"], "sha256")
    {
        codes.push("seal.receipt_tampered".into());
    }
    let receipt: Value = serde_json::from_slice(&std::fs::read(&p)?)
        .map_err(|e| ledger("seal.receipt_missing", e.to_string()))?;
    let fact = receipt["eval"].clone();
    if !fact.is_null() {
        let eval_path = ws.rf_dir().join(format!("evals/{}/record.json", str_of(&fact, "eval_id")));
        if !eval_path.exists()
            || digest::sha256_file(&eval_path).unwrap_or_default() != str_of(&fact, "sha256")
        {
            codes.push("seal.eval_changed".into());
        }
    }
    let subject = receipt["subject"].clone();
    let now_digest = if subject["kind"].is_null() {
        None
    } else {
        evaluate::subject_digest(ws, &str_of(&subject, "kind"), &str_of(&subject, "ref")).ok()
    };
    Ok(json!({
        "seal_id": seal_id, "fresh": codes.is_empty(), "codes": codes,
        "disposition": receipt["disposition"], "basis": receipt["basis"],
        "eval": if fact.is_null() { Value::Null } else {
            json!({"eval_id": fact["eval_id"], "verdict": fact["verdict"],
                   "confidence": fact["confidence"]})
        },
        "subject_matches": now_digest.is_some_and(|d| Value::String(d) == subject["sha256"]),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{commit, confirm_ask, eval_bench, intent_doc, judgement, opened};

    fn codes_of(e: SealError) -> Vec<String> {
        match e {
            SealError::Refused(r) => r.codes,
            other => panic!("wanted a refusal, got {other}"),
        }
    }

    /// Two Asks, a work commit, and an Eval closed with the given votes.
    fn judged(ws: &Workspace, votes: [&str; 3]) -> (String, String, Value) {
        let o = opened(ws);
        let items = json!([{"id": "i1", "text": "Expose an uptime endpoint", "ask_id": o.a, "status": "active"}]);
        let js: Vec<Value> = ["coverage", "drift", "adversary"]
            .iter()
            .zip(votes)
            .map(|(ang, v)| judgement(&o.brief_sha, ang, &[("i1", v)]))
            .collect();
        let rec = crate::evaluate::close_eval(
            ws,
            &str_of(&o.out, "eval_id"),
            &intent_doc(&o.brief_sha, items),
            &js,
            None,
            None,
        )
        .unwrap();
        (o.a, o.b, rec)
    }

    fn strings_at(v: &Value, key: &str) -> Vec<String> {
        strings(v, key)
    }

    #[test]
    fn seal_closes_the_open_asks_and_records_the_eval_as_a_fact() {
        eval_bench(|ws| {
            let (a, b, rec) = judged(ws, ["yes", "yes", "yes"]);
            let receipt =
                create(ws, "accepted", None, Some("good enough for the demo"), None).unwrap();
            assert!(str_of(&receipt, "seal_id").starts_with("sel_"));
            assert_eq!(receipt["basis"], json!({"asks": [a, b]}));
            assert_eq!(receipt["disposition"], "accepted");
            assert_eq!(receipt["eval"]["eval_id"], rec["eval_id"]);
            assert_eq!(receipt["eval"]["verdict"], "aligned");
            assert_eq!(receipt["eval"]["confidence"], 1.0);
            assert_eq!(receipt["eval"]["subject_matches"], true);
            assert_eq!(receipt["note"], "good enough for the demo");
            assert_eq!(receipt["authority"]["authority"], "interactive");
            assert_eq!(
                receipt["asks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| str_of(x, "title"))
                    .collect::<Vec<_>>(),
                ["Add uptime endpoint", "Skip the cache"]
            );
            let last = store::events(ws).unwrap().pop().unwrap();
            assert_eq!(last["type"], "seal.created");
            assert_eq!(
                last["links"],
                json!([{"rel": "seals", "id": a}, {"rel": "seals", "id": b},
                       {"rel": "seals", "id": rec["eval_id"]}])
            );
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
            assert!(ask::open_asks(ws).unwrap().is_empty());
            let states: BTreeSet<String> =
                ask::list_asks(ws).unwrap().iter().map(|x| str_of(x, "state")).collect();
            assert_eq!(states, ["sealed".to_string()].into());
            let chk = check(ws, &str_of(&receipt, "seal_id")).unwrap();
            assert_eq!(chk["fresh"], true);
            assert_eq!(chk["subject_matches"], true);
            assert_eq!(chk["eval"]["verdict"], "aligned");
            assert_eq!(chk["basis"], json!({"asks": [a, b]}));
        });
    }

    #[test]
    fn a_bad_verdict_never_blocks_the_seal() {
        eval_bench(|ws| {
            let (_a, _b, rec) = judged(ws, ["no", "no", "unknown"]);
            assert_eq!(rec["verdict"], "drifted");
            let receipt = create(ws, "accepted", None, None, None).unwrap();
            assert_eq!(receipt["eval"]["verdict"], "drifted");
            assert_eq!(receipt["disposition"], "accepted");
            assert_eq!(store::events(ws).unwrap().pop().unwrap()["type"], "seal.created");
        });
    }

    #[test]
    fn a_seal_without_an_eval_records_the_gap() {
        eval_bench(|ws| {
            opened(ws); // opened but never closed
            let receipt = create(ws, "deferred", None, None, None).unwrap();
            assert_eq!(receipt["eval"], json!(null));
            assert!(
                strings_at(&receipt, "limitations").iter().any(|l| l.contains("no Eval")),
                "{receipt}"
            );
            assert_eq!(check(ws, &str_of(&receipt, "seal_id")).unwrap()["eval"], json!(null));
        });
    }

    #[test]
    fn a_subject_change_after_the_eval_is_a_recorded_fact_not_a_refusal() {
        eval_bench(|ws| {
            let (_a, _b, rec) = judged(ws, ["yes", "yes", "yes"]);
            commit(&ws.root, &[("src/uptime.js", Some("changed after eval\n"))], "later");
            let receipt = create(ws, "accepted", None, None, None).unwrap();
            assert_eq!(receipt["eval"]["subject_matches"], false);
            assert!(
                strings_at(&receipt, "limitations").iter().any(|l| l.contains("subject changed"))
            );
            assert_ne!(receipt["subject"]["ref"], rec["subject"]["ref"]);
            let chk = check(ws, &str_of(&receipt, "seal_id")).unwrap();
            // The sealed commit's tree is immutable; later commits do not
            // touch it.
            assert_eq!(chk["fresh"], true);
            assert_eq!(chk["subject_matches"], true);
        });
    }

    #[test]
    fn refusals_name_a_missing_eval_an_unrelated_one_and_no_open_ask() {
        eval_bench(|ws| {
            let (_a, _b, rec) = judged(ws, ["yes", "yes", "yes"]);
            let e = create(ws, "accepted", Some("evl_missing"), None, None).unwrap_err();
            assert_eq!(codes_of(e), ["seal.eval_missing"]);
            let eval_id = str_of(&rec, "eval_id");
            assert_eq!(
                create(ws, "accepted", Some(&eval_id), None, None).unwrap()["eval"]["eval_id"],
                rec["eval_id"]
            );
            // A later basis cannot seal against an Eval of the earlier, closed Asks.
            let c = confirm_ask(ws, "Third", b"third\n", b"Third.\n");
            let e = create(ws, "accepted", Some(&eval_id), None, None).unwrap_err();
            assert_eq!(codes_of(e), ["seal.eval_unrelated"]);
            create(ws, "abandoned", None, None, None).unwrap();
            let listed = ask::list_asks(ws).unwrap();
            let last = listed.last().unwrap();
            assert_eq!(last["state"], "sealed");
            assert_eq!(last["ask_id"], c["ask_id"]);
            let e = create(ws, "accepted", None, None, None).unwrap_err();
            assert_eq!(codes_of(e), ["seal.no_open_ask"]);
            let types: Vec<String> =
                store::events(ws).unwrap().iter().map(|e| str_of(e, "type")).collect();
            assert_eq!(&types[types.len() - 2..], ["seal.created", "seal.refused"]);
            assert_eq!(std::fs::read_dir(ws.rf_dir().join("seals")).unwrap().count(), 2);
        });
    }

    #[test]
    fn a_non_human_actor_needs_a_grant() {
        eval_bench(|ws| {
            judged(ws, ["yes", "yes", "yes"]);
            let agent = json!({"kind": "agent", "id": "ci", "authority": "preauthorized"});
            let e = create(ws, "accepted", None, None, Some(&agent)).unwrap_err();
            assert_eq!(codes_of(e), ["seal.authority_missing"]);
            std::fs::create_dir_all(ws.rf_dir().join("authorizations")).unwrap();
            std::fs::write(
                ws.rf_dir().join("authorizations/ci.json"),
                serde_json::to_string(&json!({
                    "schema": "ringframe.authorization/1", "actor": "agent:ci",
                    "granted_by": "human:owner", "time": "t",
                    "allowed": {"dispositions": ["accepted"], "subject_kinds": ["git_commit"]},
                    "expires": "2999-01-01T00:00:00Z"
                }))
                .unwrap(),
            )
            .unwrap();
            let e = create(ws, "rejected", None, None, Some(&agent)).unwrap_err();
            assert_eq!(codes_of(e), ["seal.authority_missing"]);
            let r = create(ws, "accepted", None, None, Some(&agent)).unwrap();
            assert_eq!(r["authority"]["authority"], "preauthorized:authorizations/ci.json");
        });
    }

    #[test]
    fn check_detects_tampering() {
        eval_bench(|ws| {
            judged(ws, ["yes", "yes", "yes"]);
            let receipt = create(ws, "accepted", None, None, None).unwrap();
            let seal_id = str_of(&receipt, "seal_id");
            let p = ws.rf_dir().join(format!("seals/{seal_id}.json"));
            crate::testing::run(&["chmod", "600", &p.to_string_lossy()]);
            let text = std::fs::read_to_string(&p).unwrap().replace("accepted", "rejected");
            std::fs::write(&p, text).unwrap();
            let chk = check(ws, &seal_id).unwrap();
            assert_eq!(chk["fresh"], false);
            assert_eq!(chk["codes"], json!(["seal.receipt_tampered"]));
            assert_eq!(
                check(ws, "sel_nope").unwrap(),
                json!({"seal_id": "sel_nope", "fresh": false, "codes": ["seal.receipt_missing"]})
            );
        });
    }

    #[test]
    fn an_unknown_disposition_is_refused_before_anything_is_written() {
        eval_bench(|ws| {
            judged(ws, ["yes", "yes", "yes"]);
            let before = store::events(ws).unwrap().len();
            let e = create(ws, "vibes", None, None, None).unwrap_err();
            assert!(e.to_string().contains("seal.disposition"), "{e}");
            assert_eq!(store::events(ws).unwrap().len(), before);
        });
    }
}
