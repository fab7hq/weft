//! What a harness's hooks mean, as its harness file's `[signal]` says
//! (ADR-0030 D4), and the one message `weft signal` sends weftd for a hook.
//!
//! Every hook of every harness runs `weft signal <harness> <hook>` with the
//! hook's payload on stdin. The row for that hook (and its tool, where the row
//! names one) says the agent's state and whether the hook forwards a prompt or
//! a tool to RingFrame; the table's paths say where in the payload the
//! session, the project, the prompt, the tool and its command are. Nothing
//! else of the payload is taken. A harness whose hooks carry no prompt may
//! name a file its payload points to instead: the prompt is that file's last
//! line, when the line says it is one, and nothing else of the file is read.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A harness file's `[signal]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    /// The payload path of the session id.
    pub session: String,
    /// The payload path of the project directory; for a list, its first entry.
    pub workspace: String,
    #[serde(default)]
    pub prompt: Option<Prompt>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    /// What every hook prints, unless its row says otherwise.
    #[serde(default)]
    pub reply: Option<String>,
    pub on: Vec<Row>,
}

/// Where a forwarded prompt is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Prompt {
    /// The payload path of its text.
    At(String),
    /// A file of JSON lines the payload names, whose last line holds it.
    InFile(InFile),
}

/// A prompt kept in a file the payload names (a transcript): the file's last
/// line, only when its fields are `last`'s, and of it only `field`, cut to
/// what lies between `between`'s two markers when it names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InFile {
    /// The payload path of the file's path.
    pub file: String,
    /// Dotted paths in the line, and the value each must have.
    #[serde(default)]
    pub last: serde_json::Map<String, Value>,
    /// The dotted path in the line of the text.
    pub field: String,
    #[serde(default)]
    pub between: Option<[String; 2]>,
}

/// One hook's meaning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub hook: String,
    /// The row matches only this tool.
    #[serde(default)]
    pub tool: Option<String>,
    /// `ready`, `working`, `waiting`, `turn_ended` or `ended`.
    #[serde(default)]
    pub state: Option<String>,
    /// `prompt` or `tool`: weftd may pass it to RingFrame.
    #[serde(default)]
    pub forward: Option<String>,
    /// `succeeded` or `failed`, for a row that forwards a tool.
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub reply: Option<String>,
}

/// The `signal` message: one hook, as `weft signal` saw it, and only what
/// `[signal]` names of its payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sent {
    pub harness: String,
    pub hook: String,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// The harness process the hook ran under (D8), and when it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start: Option<String>,
    /// `$WEFT_PANE` and `$WEFT_PANE_TOKEN`, when Weft started the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl Signal {
    /// A harness file's `[signal]`, when it has one that reads.
    pub fn of(table: &Value) -> Option<Self> {
        serde_json::from_value(table.clone()).ok()
    }

    /// The row for this hook and the tool its payload names: a row naming
    /// that tool first, else one naming none.
    pub fn row(&self, hook: &str, payload: &Value) -> Option<&Row> {
        let tool = self.tool.as_deref().and_then(|p| text_at(payload, p));
        let of_hook = || self.on.iter().filter(move |r| r.hook == hook);
        of_hook()
            .find(|r| r.tool.is_some() && r.tool == tool)
            .or_else(|| of_hook().find(|r| r.tool.is_none()))
    }

    /// What `weft signal` sends for this hook, or nothing when no row is for
    /// it or the payload names no session.
    pub fn sent(&self, harness: &str, hook: &str, payload: &Value) -> Option<Sent> {
        let row = self.row(hook, payload)?;
        let session = text_at(payload, &self.session).filter(|s| !s.is_empty())?;
        let at = |path: &Option<String>| path.as_deref().and_then(|p| text_at(payload, p));
        let mut sent = Sent {
            harness: harness.to_string(),
            hook: hook.to_string(),
            session,
            workspace: text_at(payload, &self.workspace).filter(|w| !w.is_empty()),
            state: row.state.clone(),
            forward: row.forward.clone(),
            ..Sent::default()
        };
        match row.forward.as_deref() {
            Some("prompt") => {
                if let Some(Prompt::At(path)) = &self.prompt {
                    sent.prompt = text_at(payload, path);
                }
            }
            Some("tool") => {
                sent.tool = at(&self.tool);
                sent.command = at(&self.command);
                sent.outcome = row.outcome.clone();
            }
            _ => {}
        }
        Some(sent)
    }

    /// The file this hook's prompt is in, when its row forwards a prompt and
    /// `[signal]` keeps it in a file the payload names. `weft signal` reads
    /// its end and gives it to `prompt_from`.
    pub fn prompt_file(&self, hook: &str, payload: &Value) -> Option<String> {
        let row = self.row(hook, payload)?;
        match (&self.prompt, row.forward.as_deref()) {
            (Some(Prompt::InFile(f)), Some("prompt")) => text_at(payload, &f.file),
            _ => None,
        }
    }

    /// The prompt in the end of such a file: its last line, when that line
    /// is one, and only its text.
    pub fn prompt_from(&self, tail: &str) -> Option<String> {
        let Some(Prompt::InFile(f)) = &self.prompt else { return None };
        let line: Value =
            serde_json::from_str(tail.lines().rfind(|l| !l.trim().is_empty())?).ok()?;
        if !f.last.iter().all(|(path, want)| value_at(&line, path) == Some(want)) {
            return None;
        }
        let text = value_at(&line, &f.field)?.as_str()?;
        match &f.between {
            None => Some(text.to_string()),
            Some([start, end]) => {
                let from = text.find(start.as_str())? + start.len();
                let to = from + text[from..].find(end.as_str())?;
                Some(text[from..to].to_string())
            }
        }
    }

    /// What this hook prints: its row's reply, else the table's.
    pub fn reply(&self, hook: &str, payload: &Value) -> Option<&str> {
        self.row(hook, payload).and_then(|r| r.reply.as_deref()).or(self.reply.as_deref())
    }
}

/// One call to `ringframe observe` (ADR-0030 D9): its arguments after
/// `--workspace <project>`, and what it reads on stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    pub args: Vec<String>,
    pub input: Option<String>,
}

/// What weftd passes RingFrame for this signal, if anything: a forwarded
/// prompt, byte for byte; a forwarded tool only when the harness's profile's
/// `[observe]` (as `ringframe profile show` gives it) names it as its
/// `delivery_tool` or `shell_tool`. Turn states are never forwarded.
pub fn forward(sent: &Sent, observe: &Value) -> Option<Forward> {
    let head = |what: &str| -> Vec<String> {
        ["observe", what, "--by", "weft", "--host", &sent.harness, "--session", &sent.session]
            .map(str::to_string)
            .to_vec()
    };
    match sent.forward.as_deref()? {
        "prompt" => Some(Forward { args: head("prompt"), input: Some(sent.prompt.clone()?) }),
        "tool" => {
            let tool = sent.tool.as_deref()?;
            let named = |key: &str| observe.get(key).and_then(Value::as_str) == Some(tool);
            if !named("delivery_tool") && !named("shell_tool") {
                return None;
            }
            let mut args = head("tool");
            args.extend(["--tool".into(), tool.into(), "--outcome".into(), sent.outcome.clone()?]);
            if let Some(command) = &sent.command {
                args.extend(["--command".into(), command.clone()]);
            }
            Some(Forward { args, input: None })
        }
        _ => None,
    }
}

/// The value at a dotted path.
fn value_at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |v, key| v.get(key))
}

/// The text at a dotted payload path; for a list, its first entry.
pub fn text_at(payload: &Value, path: &str) -> Option<String> {
    match value_at(payload, path)? {
        Value::Array(all) => all.first()?.as_str().map(str::to_string),
        other => other.as_str().map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::fixture::file;
    use serde_json::json;

    fn signal(name: &str) -> Signal {
        Signal::of(&file(name)["signal"]).expect("a [signal] that reads")
    }

    #[test]
    fn every_fixture_harness_file_says_what_its_hooks_mean() {
        for name in ["claude-code", "codex", "antigravity"] {
            let s = signal(name);
            assert!(!s.on.is_empty(), "{name}");
            for row in &s.on {
                let states = ["ready", "working", "waiting", "turn_ended", "ended"];
                assert!(row.state.as_deref().is_none_or(|st| states.contains(&st)), "{row:?}");
            }
        }
    }

    #[test]
    fn a_row_naming_a_tool_matches_only_that_tool() {
        let s = signal("claude-code");
        let asking = json!({"session_id": "s1", "tool_name": "AskUserQuestion"});
        assert_eq!(s.row("PreToolUse", &asking).unwrap().state.as_deref(), Some("waiting"));
        let other = json!({"session_id": "s1", "tool_name": "Bash"});
        assert_eq!(s.row("PreToolUse", &other), None, "no row for another tool");
        assert_eq!(s.row("NoSuchHook", &asking), None);
        assert_eq!(s.sent("claude-code", "PreToolUse", &other), None, "nothing to send");
    }

    #[test]
    fn only_what_signal_names_is_taken_from_the_payload() {
        let s = signal("claude-code");
        let payload = json!({
            "session_id": "s1", "cwd": "/w", "transcript_path": "/secret/transcript.jsonl",
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": {"command": "npm test", "description": "SECRET-INPUT"},
            "tool_response": {"stdout": "SECRET-OUT"}, "prompt": "never here"
        });
        let sent = s.sent("claude-code", "PostToolUse", &payload).unwrap();
        assert_eq!(
            sent,
            Sent {
                harness: "claude-code".into(),
                hook: "PostToolUse".into(),
                session: "s1".into(),
                workspace: Some("/w".into()),
                state: Some("working".into()),
                forward: Some("tool".into()),
                tool: Some("Bash".into()),
                command: Some("npm test".into()),
                outcome: Some("succeeded".into()),
                ..Sent::default()
            }
        );
        let wire = serde_json::to_string(&sent).unwrap();
        assert!(
            !wire.contains("SECRET") && !wire.contains("transcript") && !wire.contains("never")
        );
        let failed = s.sent("claude-code", "PostToolUseFailure", &payload).unwrap();
        assert_eq!(failed.outcome.as_deref(), Some("failed"));
    }

    #[test]
    fn a_prompt_is_taken_only_where_its_row_forwards_it() {
        let s = signal("claude-code");
        let payload = json!({"session_id": "s1", "cwd": "/w", "prompt": "/rf:ask fix it"});
        let asked = s.sent("claude-code", "UserPromptSubmit", &payload).unwrap();
        assert_eq!(asked.prompt.as_deref(), Some("/rf:ask fix it"));
        let stopped = s.sent("claude-code", "Stop", &payload).unwrap();
        assert_eq!((stopped.prompt, stopped.state.as_deref()), (None, Some("turn_ended")));
        let ended = s.sent("claude-code", "SessionEnd", &payload).unwrap();
        assert_eq!(ended.state.as_deref(), Some("ended"));
    }

    #[test]
    fn a_tool_row_that_forwards_nothing_sends_no_tool() {
        let s = signal("codex");
        let payload = json!({"session_id": "c1", "cwd": "/w", "tool_name": "shell",
                             "tool_input": {"command": "npm test"}});
        let sent = s.sent("codex", "PostToolUse", &payload).unwrap();
        assert_eq!((sent.tool, sent.command, sent.forward), (None, None, None));
    }

    #[test]
    fn a_list_is_its_first_entry_and_a_reply_is_the_rows_or_the_tables() {
        let s = signal("antigravity");
        let payload = json!({"conversationId": "a1", "workspacePaths": ["/w", "/other"]});
        let sent = s.sent("antigravity", "Stop", &payload).unwrap();
        assert_eq!((sent.session.as_str(), sent.workspace.as_deref()), ("a1", Some("/w")));
        assert_eq!(s.reply("Stop", &payload), Some("{}"));
        assert_eq!(s.reply("PreToolUse", &payload), Some(r#"{"decision":"allow"}"#));
        assert_eq!(s.reply("NoSuchHook", &payload), Some("{}"), "the table's, with no row");
        assert_eq!(signal("claude-code").reply("Stop", &payload), None);
    }

    #[test]
    fn a_typed_prompt_is_forwarded_byte_for_byte_and_a_state_never() {
        let s = signal("claude-code");
        let typed = "/rf:ask fix the login bug\n  with spaces  ";
        let payload = json!({"session_id": "s1", "cwd": "/w", "prompt": typed});
        let sent = s.sent("claude-code", "UserPromptSubmit", &payload).unwrap();
        let f = forward(&sent, &json!(null)).expect("a prompt goes");
        assert_eq!(
            f.args,
            ["observe", "prompt", "--by", "weft", "--host", "claude-code", "--session", "s1"]
        );
        assert_eq!(f.input.as_deref(), Some(typed));
        let stopped = s.sent("claude-code", "Stop", &payload).unwrap();
        assert_eq!(forward(&stopped, &json!(null)), None, "a turn state is never forwarded");
    }

    #[test]
    fn a_tool_is_forwarded_only_when_the_profile_names_it() {
        let s = signal("claude-code");
        let observe = json!({"delivery_tool": "EnterPlanMode", "shell_tool": "Bash"});
        let ran = |tool: &str, hook: &str| {
            let payload = json!({"session_id": "s1", "cwd": "/w", "tool_name": tool,
                                 "tool_input": {"command": "npm test"}});
            s.sent("claude-code", hook, &payload).unwrap()
        };
        let shell = forward(&ran("Bash", "PostToolUseFailure"), &observe).expect("the shell tool");
        assert_eq!(
            shell.args[8..],
            ["--tool", "Bash", "--outcome", "failed", "--command", "npm test"].map(String::from)
        );
        assert_eq!(shell.input, None);
        let plan = forward(&ran("EnterPlanMode", "PostToolUse"), &observe).expect("delivery");
        assert_eq!(plan.args[8..12], ["--tool", "EnterPlanMode", "--outcome", "succeeded"]);
        assert_eq!(forward(&ran("Write", "PostToolUse"), &observe), None, "another tool");
        assert_eq!(forward(&ran("Bash", "PostToolUse"), &json!({})), None, "a profile naming none");
    }

    /// A harness whose hooks carry no prompt but name a file holding it:
    /// `prompt` as a table. Only the file's last line is read, and only when
    /// its fields are `last`'s; `field` is its text, cut by `between`.
    fn from_a_file() -> Signal {
        Signal::of(&json!({
            "session": "id", "workspace": "dir",
            "prompt": {"file": "log.path", "last": {"kind": "typed"}, "field": "body.text",
                       "between": ["<said>\n", "\n</said>"]},
            "on": [{"hook": "Began", "state": "working", "forward": "prompt"},
                   {"hook": "Ended", "state": "turn_ended"}]
        }))
        .expect("a [signal] whose prompt is in a file")
    }

    #[test]
    fn a_prompt_can_be_the_last_line_of_a_file_the_payload_names() {
        let s = from_a_file();
        let payload = json!({"id": "s1", "dir": "/w", "log": {"path": "/h/log.jsonl"}});
        assert_eq!(s.prompt_file("Began", &payload).as_deref(), Some("/h/log.jsonl"));
        assert_eq!(s.prompt_file("Ended", &payload), None, "its row forwards nothing");
        let sent = s.sent("made-up", "Began", &payload).unwrap();
        assert_eq!(
            (sent.prompt, sent.forward.as_deref()),
            (None, Some("prompt")),
            "read by weft signal"
        );
        let typed = "/rf:ask fix it — now\n  with lines  ";
        let line = |kind: &str, text: &str| {
            json!({"kind": kind, "body": {"text": format!("<said>\n{text}\n</said>\n<meta>9:00</meta>")}})
                .to_string()
        };
        let tail = format!("{}\n{}\n", line("answer", "earlier"), line("typed", typed));
        assert_eq!(
            s.prompt_from(&tail).as_deref(),
            Some(typed),
            "byte for byte, between the markers"
        );
    }

    #[test]
    fn a_file_prompt_is_none_unless_its_last_line_is_one() {
        let s = from_a_file();
        let typed =
            json!({"kind": "typed", "body": {"text": "<said>\nhello\n</said>"}}).to_string();
        let tool = json!({"kind": "tool", "body": {"text": "ran"}}).to_string();
        assert_eq!(
            s.prompt_from(&format!("{typed}\n{tool}\n")),
            None,
            "a later step: no new prompt"
        );
        let unmarked = json!({"kind": "typed", "body": {"text": "no markers"}}).to_string();
        assert_eq!(s.prompt_from(&unmarked), None, "markers named and absent");
        assert_eq!(s.prompt_from("not json\n"), None);
        assert_eq!(s.prompt_from(""), None);
        let plain = Signal::of(&json!({"session": "id", "workspace": "dir",
            "prompt": {"file": "f", "last": {"kind": "typed"}, "field": "body.text"},
            "on": [{"hook": "Began", "forward": "prompt"}]}))
        .unwrap();
        assert_eq!(
            plain.prompt_from(&format!("{typed}\n")).as_deref(),
            Some("<said>\nhello\n</said>")
        );
        assert_eq!(
            signal("claude-code").prompt_from(&format!("{typed}\n")),
            None,
            "a payload-path prompt"
        );
    }

    /// The Antigravity fixture's transcript, as agy 1.3.2 writes it (probe A,
    /// `probe-agy-ab`): a typed prompt, then a tool step.
    #[test]
    fn a_transcript_line_gives_the_typed_prompt_once() {
        let s = signal("antigravity");
        let payload =
            json!({"conversationId": "a1", "workspacePaths": ["/w"], "transcriptPath": "/t.jsonl"});
        assert_eq!(s.prompt_file("PreInvocation", &payload).as_deref(), Some("/t.jsonl"));
        let typed = json!({"step_index": 0, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE",
            "content": "<USER_REQUEST>\n/rf:ask Plan it.\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-10-09T09:24:12Z.\n</ADDITIONAL_METADATA>"});
        let step = json!({"step_index": 2, "source": "MODEL", "type": "GENERIC", "status": "DONE", "content": "Created file"});
        assert_eq!(s.prompt_from(&format!("{typed}\n")).as_deref(), Some("/rf:ask Plan it."));
        assert_eq!(
            s.prompt_from(&format!("{typed}\n{step}\n")),
            None,
            "a later invocation of the same turn"
        );
    }

    #[test]
    fn a_payload_with_no_session_sends_nothing() {
        let s = signal("claude-code");
        assert_eq!(s.sent("claude-code", "Stop", &json!({"cwd": "/w"})), None);
        assert_eq!(s.sent("claude-code", "Stop", &json!({"session_id": ""})), None);
        assert_eq!(s.sent("claude-code", "Stop", &json!("not an object")), None);
    }
}
