//! `ringframe observe`: facts someone else saw a harness do, recorded under
//! the name of whoever saw them (ADR-0022). RingFrame reads no hook payload:
//! the caller extracts the prompt, or the tool and its outcome, and passes
//! them here. RingFrame decides what each one means for the record.

use serde_json::{Value, json};

use crate::ask::AskError;
use crate::workspace::Workspace;
use crate::{ask, evaluate, profiles, sessions};

/// `observe prompt`: one prompt's bytes, seen submitted in `session`. Kept in
/// `prompts.jsonl` (in full only for a RingFrame invocation); an `ask`
/// invocation puts the Ask on record as asked, and a prompt that equals a
/// compiled one is that Ask's observed submission.
pub fn prompt(
    ws: &Workspace,
    by: &str,
    host: &str,
    session: &str,
    prompt: &str,
    host_version: Option<&str>,
) -> Result<Value, AskError> {
    let Some(rec) = sessions::observe_prompt(ws, host, session, prompt, host_version, by)? else {
        return Ok(json!({"recorded": false}));
    };
    let mut out = json!({"recorded": true});
    for (k, v) in rec.as_object().into_iter().flatten() {
        out[k] = v.clone();
    }
    out["submission"] = Value::Null;
    if rec.get("prompt").is_none() {
        let sha = rec["sha256"].as_str().unwrap_or_default();
        let mut sub = ask::submission_from_capture(ws, host, session, sha, by)?;
        // A host that wraps a long paste in markers of its own when it
        // reports what it received: compared without them (ADR-0033 D1).
        if sub.is_none()
            && let Some(inner) = unwrapped(host, prompt)
        {
            let inner = crate::digest::sha256_bytes(inner.as_bytes());
            sub = ask::submission_as(ws, host, session, &inner, by, Some("host_paste_unwrapped"))?;
        }
        if let Some(s) = sub {
            out["submission"] = s;
        }
    }
    Ok(out)
}

/// The prompt inside the host's paste wrapper, when the whole prompt is one:
/// its profile's `paste_open`, then the paste, then `paste_close`, both
/// naming the same `{id}`. None for a host that names no wrapper.
fn unwrapped<'a>(host: &str, prompt: &'a str) -> Option<&'a str> {
    let open = profiles::fact(host, "paste_open")?;
    let close = profiles::fact(host, "paste_close")?;
    let (open_a, open_b) = open.split_once("{id}")?;
    let (close_a, close_b) = close.split_once("{id}")?;
    let rest = prompt.strip_prefix(open_a)?;
    let id_len = rest.find(open_b)?;
    let id = &rest[..id_len];
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let inner = rest[id_len + open_b.len()..].strip_suffix(close_b)?;
    inner.strip_suffix(id)?.strip_suffix(close_a)
}

/// `observe tool`: one tool call, seen in `session`, and whether it
/// succeeded. The host's profile says which tools matter (`[observe]`): its
/// `delivery_tool` is a delivery receipt, its `shell_tool` with a command a
/// tool fact. Any other tool records nothing.
pub fn tool(
    ws: &Workspace,
    by: &str,
    host: &str,
    session: &str,
    tool: &str,
    outcome: &str,
    command: Option<&str>,
) -> Result<Value, AskError> {
    let observe = profiles::of(host).map(|p| p["observe"].clone()).unwrap_or(Value::Null);
    let names = |key: &str| observe[key].as_str() == Some(tool);
    let nothing = json!({"recorded": false});
    if names("delivery_tool") {
        let failed = outcome == "failed";
        return Ok(match ask::delivery_observed(ws, host, session, tool, failed, by)? {
            Some(d) => json!({"recorded": true, "kind": "delivery", "ask_id": d["ask_id"],
                              "state": d["state"]}),
            None => nothing,
        });
    }
    if names("shell_tool")
        && let Some(command) = command
    {
        return Ok(match evaluate::record_fact(ws, host, session, outcome, command, by)? {
            Some(ev) => json!({"recorded": true, "kind": "fact", "fact_id": ev["id"],
                               "outcome": ev["data"]["outcome"]}),
            None => nothing,
        });
    }
    Ok(nothing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;
    use crate::testing::{confirm_ask, repo, with_config_home, ws_for};

    fn bench<T>(body: impl FnOnce(&Workspace) -> T) -> T {
        with_config_home(|_| {
            let repo = repo();
            let ws = ws_for(repo.path());
            body(&ws)
        })
    }

    fn events(ws: &Workspace) -> Vec<Value> {
        store::events(ws).unwrap()
    }

    fn prompts(ws: &Workspace, host: &str, session: &str) -> Vec<Value> {
        let path = ws.rf_dir().join("sessions").join(host).join(session).join("prompts.jsonl");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn a_prompt_keeps_the_rf_bytes_and_only_a_digest_otherwise() {
        bench(|ws| {
            let out = prompt(ws, "weft", "claude-code", "s1", "hello there", None).unwrap();
            assert_eq!(out["recorded"], true);
            let out =
                prompt(ws, "weft", "claude-code", "s1", "/rf:ask fix the login bug", None).unwrap();
            assert_eq!(out["recorded"], true);
            let lines = prompts(ws, "claude-code", "s1");
            assert_eq!(lines.len(), 2);
            assert!(lines[0].get("prompt").is_none(), "a plain prompt's text is never kept");
            assert_eq!(lines[0]["bytes"], 11);
            assert_eq!(lines[0]["sha256"], crate::digest::sha256_bytes(b"hello there"));
            assert_eq!(lines[0]["observed_by"], "weft");
            assert_eq!(lines[1]["prompt"], "/rf:ask fix the login bug");
            for line in &lines {
                assert!(line.get("cwd").is_none() && line.get("permission_mode").is_none());
            }
            // The Ask is on record as asked, graded by who saw it.
            let evs = events(ws);
            assert_eq!(evs.len(), 1);
            assert_eq!(evs[0]["type"], "ask.requested");
            assert_eq!(evs[0]["data"]["captured_by"], "weft");
            assert_eq!(evs[0]["data"]["host"]["session_ref"], "s1");
        });
    }

    #[test]
    fn a_prompt_keeps_the_host_version_it_is_given_and_an_empty_one_is_nothing() {
        bench(|ws| {
            prompt(ws, "weft", "codex", "c1", "$rf:ask fix it", Some("codex-cli 0.161.0\n"))
                .unwrap();
            let out = prompt(ws, "weft", "claude-code", "s1", "", None).unwrap();
            assert_eq!(out["recorded"], false, "an empty prompt records nothing");
            assert_eq!(prompts(ws, "codex", "c1")[0]["host_version"], "codex-cli 0.161.0");
            assert_eq!(prompts(ws, "codex", "c1")[0]["prompt"], "$rf:ask fix it");
        });
    }

    #[test]
    fn a_compiled_prompt_seen_submitted_is_an_observed_submission_by_the_caller() {
        bench(|ws| {
            let ask = confirm_ask(ws, "Login", b"fix login\n", b"Fix login.\n");
            let out = prompt(ws, "weft", "claude-code", "s1", "Fix login.\n", None).unwrap();
            assert_eq!(out["submission"]["ask_id"], ask["ask_id"]);
            let sub = events(ws).into_iter().find(|e| e["type"] == "ask.submission").unwrap();
            assert_eq!(sub["data"]["state"], "observed");
            assert_eq!(sub["data"]["observed_by"], "weft");
        });
    }

    /// A host that wraps a long paste in markers of its own when it reports
    /// the prompt it received: its profile names them (`paste_open`,
    /// `paste_close`, `{id}` for the id it gives each paste), and a prompt
    /// that is wholly so wrapped is compared without them (ADR-0033 D1).
    #[test]
    fn a_prompt_the_host_wrapped_as_a_paste_is_compared_without_the_wrapper() {
        bench(|ws| {
            let dir = crate::config::config_dir().join("harnesses");
            let unknown = std::fs::read_to_string(dir.join("unknown.toml")).unwrap();
            let loom = unknown.replace(
                "profile_id = \"unknown\"",
                "profile_id = \"loom\"\nhost = \"loom\"\npaste_open = \"\\n\\n<paste n=\\\"{id}\\\">\\n\"\n\
                 paste_close = \"\\n</paste n=\\\"{id}\\\">\\n\"",
            );
            std::fs::write(dir.join("loom.toml"), loom).unwrap();
            let ask = confirm_ask(ws, "Login", b"fix login\n", b"Fix login.\n");
            let none =
                |p: &str| prompt(ws, "weft", "loom", "s1", p, None).unwrap()["submission"].clone();
            assert!(
                none("Fix it first.\n\n<paste n=\"7f\">\nFix login.\n\n</paste n=\"7f\">\n")
                    .is_null(),
                "a wrapper that is not the whole prompt"
            );
            assert!(
                none("\n\n<paste n=\"7f\">\nFix login.\n\n</paste n=\"8a\">\n").is_null(),
                "two ids"
            );
            let out = prompt(
                ws,
                "weft",
                "loom",
                "s1",
                "\n\n<paste n=\"7f\">\nFix login.\n\n</paste n=\"7f\">\n",
                None,
            )
            .unwrap();
            assert_eq!(out["submission"]["ask_id"], ask["ask_id"]);
            let sub = events(ws).into_iter().find(|e| e["type"] == "ask.submission").unwrap();
            assert_eq!(sub["data"]["match"], "host_paste_unwrapped");
            assert_eq!(sub["data"]["state"], "observed");
            // A host whose profile names no wrapper compares the prompt as it came.
            let other = confirm_ask(ws, "Other", b"other\n", b"Other.\n");
            let as_came = prompt(
                ws,
                "weft",
                "unknown",
                "s2",
                "\n\n<paste n=\"1\">\nOther.\n\n</paste n=\"1\">\n",
                None,
            )
            .unwrap();
            assert!(as_came["submission"].is_null(), "{as_came}");
            let _ = other;
        });
    }

    #[test]
    fn the_delivery_tool_is_a_delivery_receipt_graded_by_the_caller() {
        bench(|ws| {
            let ask = confirm_ask(ws, "Login", b"fix login\n", b"Fix login.\n");
            let out =
                tool(ws, "weft", "claude-code", "s1", "EnterPlanMode", "succeeded", None).unwrap();
            assert_eq!(out["recorded"], true);
            assert_eq!(out["kind"], "delivery");
            assert_eq!(out["ask_id"], ask["ask_id"]);
            assert_eq!(out["state"], "native_accepted");
            let d = events(ws).into_iter().find(|e| e["type"] == "ask.delivery").unwrap();
            assert_eq!(
                d["data"]["receipt"],
                json!({"tool": "EnterPlanMode", "session_id": "s1", "captured_by": "weft"})
            );
            // Once: a second sighting has no Ask left to deliver.
            let again =
                tool(ws, "weft", "claude-code", "s1", "EnterPlanMode", "succeeded", None).unwrap();
            assert_eq!(again["recorded"], false);
        });
    }

    #[test]
    fn a_failed_delivery_tool_is_a_failed_delivery() {
        bench(|ws| {
            confirm_ask(ws, "Login", b"fix login\n", b"Fix login.\n");
            let out =
                tool(ws, "weft", "claude-code", "s1", "EnterPlanMode", "failed", None).unwrap();
            assert_eq!(out["state"], "delivery_failed");
        });
    }

    #[test]
    fn the_shell_tool_is_a_tool_fact_graded_by_the_caller() {
        bench(|ws| {
            confirm_ask(ws, "Uptime", b"add uptime\n", b"Add uptime.\n");
            let out =
                tool(ws, "weft", "claude-code", "s1", "Bash", "failed", Some("npm test")).unwrap();
            assert_eq!(out["recorded"], true);
            assert_eq!(out["kind"], "fact");
            assert_eq!(out["outcome"], "failed");
            let f = events(ws).into_iter().find(|e| e["type"] == "tool.fact").unwrap();
            assert_eq!(f["data"]["command"], "npm test");
            assert_eq!(f["data"]["session_ref"], "s1");
            assert_eq!(f["data"]["observed_by"], "weft");
            assert!(f["data"].get("tool_use_id").is_none());
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    #[test]
    fn another_tool_or_a_host_that_names_none_records_nothing() {
        bench(|ws| {
            confirm_ask(ws, "Uptime", b"add uptime\n", b"Add uptime.\n");
            let before = events(ws).len();
            let none = |host: &str, t: &str, command: Option<&str>| {
                let out = tool(ws, "weft", host, "s1", t, "succeeded", command).unwrap();
                assert_eq!(out, json!({"recorded": false}), "{host} {t}");
            };
            none("claude-code", "Write", None);
            none("claude-code", "Bash", None);
            none("claude-code", "Bash", Some("ringframe eval open"));
            none("codex", "Bash", Some("npm test"));
            none("codex", "shell", Some("npm test"));
            assert_eq!(events(ws).len(), before);
        });
    }

    /// ADR-0022, "weftd not running": with nothing calling `observe`, an Ask
    /// still compiles, and the record holds the weaker grades it already has.
    #[test]
    fn without_observe_ringframe_records_the_weaker_grades() {
        bench(|ws| {
            let staged =
                crate::testing::stage_in(ws, "prompt.txt", b"fix login\n", b"Fix login.\n");
            let out = ask::compile(
                ws,
                ask::Compile {
                    staged: &staged,
                    title: "Login",
                    capability: "native_plan",
                    classification: json!({"task": ["plan"], "result": "plan",
                        "interaction": "approval_gated", "horizon": "session", "effects": ["read"]}),
                    route: json!({"fits": "bounded", "alternatives": [], "continuation": "plan review",
                        "effects": "reads", "gaps": []}),
                    host: json!({"name": "claude-code", "surface": "native-tui"}),
                    links: Vec::new(),
                    limitations: Vec::new(),
                    actor: None,
                    overrides: &crate::config::Overrides::default(),
                },
            )
            .unwrap();
            let id = out["ask_id"].as_str().unwrap();
            ask::confirm(ws, id, None).unwrap();
            assert_eq!(out["source_verified"], "unverified", "the wording is the skill's");
            let compiled = events(ws).into_iter().find(|e| e["type"] == "ask.compiled").unwrap();
            assert!(compiled["data"]["host"]["session_ref"].is_null(), "no session_ref");
            let listed = ask::list_asks(ws).unwrap();
            assert_eq!(listed[0]["submission"], "unobserved");
            assert!(listed[0]["delivery"].is_null(), "no recorded delivery outcome");
            assert!(events(ws).iter().all(|e| e["type"] != "tool.fact"), "no tool facts");
            let seal = crate::seal::create(ws, "deferred", None, None, None).unwrap();
            let limits = seal["limitations"].to_string();
            assert!(limits.contains(crate::evaluate::NOTHING_OBSERVED), "{limits}");
        });
    }
}
