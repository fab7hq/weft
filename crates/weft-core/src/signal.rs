//! What a harness's hooks mean, as its harness file's `[signal]` says, and
//! the one message `weft signal` sends weftd for a hook.
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
    /// Questions the harness asks without stopping the turn.
    #[serde(default)]
    pub questions: Option<Questions>,
    pub on: Vec<Row>,
}

/// Questions a harness asks its person without stopping the turn, kept in a
/// file of JSON lines the payload names (a transcript). A line with `asked`'s
/// fields asks one, its id at `id`; a line with `answered`'s fields whose
/// text at `answer` begins with `begins` answers every question whose id it
/// holds; a line with `typed`'s fields that holds no `begins` is the person
/// typing to the harness, past every question asked before it. A question is
/// pending when it is asked and no later line answers it or is typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Questions {
    /// The payload path of the file's path.
    pub file: String,
    pub asked: serde_json::Map<String, Value>,
    pub id: String,
    pub answered: serde_json::Map<String, Value>,
    pub answer: String,
    pub begins: String,
    pub typed: serde_json::Map<String, Value>,
}

impl Questions {
    /// Whether a question is pending in this end of the file. Only lines
    /// that hold `begins`, or every text value of `asked`, are read.
    pub fn pending(&self, tail: &str) -> bool {
        let fields = |v: &Value, want: &serde_json::Map<String, Value>| {
            want.iter().all(|(p, w)| value_at(v, p) == Some(w))
        };
        let mentions = |line: &str, want: &serde_json::Map<String, Value>| {
            want.values().filter_map(Value::as_str).all(|w| line.contains(w))
        };
        let mut open: Vec<String> = Vec::new();
        for line in tail.lines() {
            let answers = line.contains(self.begins.as_str());
            if !answers && !mentions(line, &self.asked) && !mentions(line, &self.typed) {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if !answers && fields(&v, &self.typed) {
                open.clear();
            } else if fields(&v, &self.asked)
                && let Some(id) = text_at(&v, &self.id).filter(|id| !id.is_empty())
            {
                open.push(id);
            } else if answers
                && fields(&v, &self.answered)
                && let Some(text) = text_at(&v, &self.answer)
                && text.starts_with(self.begins.as_str())
            {
                open.retain(|id| !text.contains(id.as_str()));
            }
        }
        !open.is_empty()
    }
}

/// Where a forwarded prompt is: one place, or a list tried in order, for a
/// harness that records some prompts in one place and some in another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Prompt {
    One(Place),
    Each(Vec<Place>),
}

/// One place a harness records the prompt it received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Place {
    /// The payload path of its text.
    At(String),
    /// A file of JSON lines the payload names, one of whose lines holds it.
    InFile(InFile),
}

/// A prompt kept in a file the payload names (a transcript): its last line,
/// or with `newest` its newest line, whose fields are `last`'s, and of it
/// only `field`, cut to what lies between `between`'s two markers when it
/// names them. When the text holds `names_file`'s first marker, the path
/// between the two markers is a file, and the prompt is that file's bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InFile {
    /// The payload path of the file's path.
    pub file: String,
    /// Dotted paths in the line, and the value each must have.
    #[serde(default)]
    pub last: serde_json::Map<String, Value>,
    /// Look back from the end for the newest line with `last`'s fields,
    /// rather than take only the last line.
    #[serde(default)]
    pub newest: bool,
    /// The dotted path in the line of the text.
    pub field: String,
    #[serde(default)]
    pub between: Option<[String; 2]>,
    #[serde(default)]
    pub names_file: Option<[String; 2]>,
}

/// A prompt found where the harness recorded it, and, for a line of a file,
/// where it was read (`<file>:<offset>`), so the same line is forwarded once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Received {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl Prompt {
    fn places(&self) -> &[Place] {
        match self {
            Prompt::One(p) => std::slice::from_ref(p),
            Prompt::Each(all) => all,
        }
    }
}

impl InFile {
    /// The line this place reads in a file's end, and where in it the line
    /// starts.
    fn line(&self, tail: &str) -> Option<(usize, Value)> {
        let matches = |v: &Value| self.last.iter().all(|(p, want)| value_at(v, p) == Some(want));
        let mut at = tail.len();
        for line in tail.split_inclusive('\n').collect::<Vec<_>>().into_iter().rev() {
            at -= line.len();
            if line.trim().is_empty() {
                continue;
            }
            let read = serde_json::from_str::<Value>(line).ok().filter(|v| matches(v));
            match (read, self.newest) {
                (Some(v), _) => return Some((at, v)),
                (None, false) => return None,
                (None, true) => continue,
            }
        }
        None
    }

    /// The text of a line: `field`, cut by `between`.
    fn text(&self, line: &Value) -> Option<String> {
        let text = value_at(line, &self.field)?.as_str()?;
        match &self.between {
            None => Some(text.to_string()),
            Some([start, end]) => {
                let from = text.find(start.as_str())? + start.len();
                let to = from + text[from..].find(end.as_str())?;
                Some(text[from..to].to_string())
            }
        }
    }
}

/// One hook's meaning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub hook: String,
    /// The row matches only this tool.
    #[serde(default)]
    pub tool: Option<String>,
    /// The row matches only a payload with these fields: dotted paths and the
    /// value each must have, or a list of values one of which it must have.
    #[serde(default)]
    pub when: serde_json::Map<String, Value>,
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
    /// Where a prompt read from a file was read, so weftd forwards it once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_key: Option<String>,
    /// The same submission as the harness also recorded it, after `prompt`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<Received>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// The harness process the hook ran under, and when it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start: Option<String>,
    /// `$WEFT_PANE` and `$WEFT_PANE_TOKEN`, when Weft started the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The hook ran under the harness's shared server, whose variables are
    /// those of whichever pane started it: it names no pane.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
    /// Whether a question asked without stopping the turn is pending, when
    /// this hook read them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asking: Option<bool>,
    /// The harness's own transcript, where `[conversation]` `path` names it:
    /// kept per pane in weftd's memory, never recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
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
        let of_hook =
            || self.on.iter().filter(move |r| r.hook == hook && fields_match(payload, &r.when));
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
                // What the payload alone holds; `weft signal` reads the rest.
                sent.prompt = self.prompt_in(hook, payload, &|_| None, &|_| None).map(|r| r.text);
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

    /// The prompt this hook reports the harness received, when its row
    /// forwards one: the first place that has it.
    pub fn prompt_in(
        &self,
        hook: &str,
        payload: &Value,
        tail: &dyn Fn(&str) -> Option<(u64, String)>,
        whole: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Received> {
        self.received_in(hook, payload, tail, whole).into_iter().next()
    }

    /// Every place that records the prompt this hook forwards, in order:
    /// the first is the prompt received, the others the same submission as
    /// the harness also recorded it. `tail` reads a file's end (where it
    /// starts, and the text); `whole` reads a file a text names. `weft
    /// signal` gives them; they read nothing else.
    pub fn received_in(
        &self,
        hook: &str,
        payload: &Value,
        tail: &dyn Fn(&str) -> Option<(u64, String)>,
        whole: &dyn Fn(&str) -> Option<String>,
    ) -> Vec<Received> {
        let forwards =
            self.row(hook, payload).is_some_and(|r| r.forward.as_deref() == Some("prompt"));
        let Some(prompt) = self.prompt.as_ref().filter(|_| forwards) else { return Vec::new() };
        prompt
            .places()
            .iter()
            .filter_map(|place| match place {
                Place::At(path) => text_at(payload, path).map(|text| Received { text, key: None }),
                Place::InFile(f) => {
                    let file = text_at(payload, &f.file)?;
                    let (start, end) = tail(&file)?;
                    let (at, line) = f.line(&end)?;
                    let mut text = f.text(&line)?;
                    if let Some([open, close]) = &f.names_file
                        && let Some(from) = text.find(open.as_str()).map(|i| i + open.len())
                    {
                        let to = from + text[from..].find(close.as_str())?;
                        text = whole(&text[from..to])?;
                    }
                    Some(Received { text, key: Some(format!("{file}:{}", start + at as u64)) })
                }
            })
            .collect()
    }

    /// Whether a question is pending, when the table names questions and
    /// the payload's file reads. `tail` reads a file's end, as for a prompt.
    pub fn asking(
        &self,
        payload: &Value,
        tail: &dyn Fn(&str) -> Option<(u64, String)>,
    ) -> Option<bool> {
        let q = self.questions.as_ref()?;
        let file = text_at(payload, &q.file)?;
        let (_, end) = tail(&file)?;
        Some(q.pending(&end))
    }

    /// What this hook prints: its row's reply, else the table's.
    pub fn reply(&self, hook: &str, payload: &Value) -> Option<&str> {
        self.row(hook, payload).and_then(|r| r.reply.as_deref()).or(self.reply.as_deref())
    }
}

/// One call to `ringframe observe`: its arguments after
/// `--workspace <project>`, and what it reads on stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    pub args: Vec<String>,
    pub input: Option<String>,
}

/// What weftd passes RingFrame for this signal, if anything: a forwarded
/// prompt, byte for byte, or with the harness's other records of it, a JSON
/// list after `--layers`; a forwarded tool only when the harness's profile's
/// `[observe]` (as `ringframe profile show` gives it) names it as its
/// `delivery_tool` or `shell_tool`. Turn states are never forwarded.
pub fn forward(sent: &Sent, observe: &Value) -> Option<Forward> {
    let head = |what: &str| -> Vec<String> {
        ["observe", what, "--by", "weft", "--host", &sent.harness, "--session", &sent.session]
            .map(str::to_string)
            .to_vec()
    };
    match sent.forward.as_deref()? {
        "prompt" if sent.also.is_empty() => {
            Some(Forward { args: head("prompt"), input: Some(sent.prompt.clone()?) })
        }
        "prompt" => {
            let mut layers = vec![sent.prompt.clone()?];
            layers.extend(sent.also.iter().map(|r| r.text.clone()));
            let mut args = head("prompt");
            args.push("--layers".into());
            Some(Forward { args, input: Some(serde_json::to_string(&layers).ok()?) })
        }
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

/// Whether every dotted path holds its value, or one of a list of values.
/// The value at a path may itself be a list that holds it.
pub(crate) fn fields_match(v: &Value, want: &serde_json::Map<String, Value>) -> bool {
    want.iter().all(|(path, want)| {
        let got = value_at(v, path);
        let is = |want: &Value| match got {
            Some(Value::Array(items)) => items.contains(want),
            Some(got) => got == want,
            None => false,
        };
        match want {
            Value::Array(any) => any.iter().any(is),
            one => is(one),
        }
    })
}

/// The value at a dotted path; a number steps into a list.
pub(crate) fn value_at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |v, key| match v {
        Value::Array(all) => all.get(key.parse::<usize>().ok()?),
        _ => v.get(key),
    })
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
    fn a_row_with_when_matches_only_payloads_with_those_fields() {
        let s = Signal::of(&json!({
            "session": "session_id", "workspace": "cwd",
            "on": [
                {"hook": "Notification", "when": {"notificationType": "permission_prompt"}, "state": "waiting"},
                {"hook": "Notification", "when": {"notificationType": ["elicitation_dialog", "other_dialog"]}, "state": "waiting"},
                {"hook": "Stop", "when": {"reason": "end_turn"}, "state": "turn_ended"},
            ],
        }))
        .expect("a [signal] that reads");
        let note = |kind: &str| json!({"session_id": "s1", "notificationType": kind});
        assert_eq!(
            s.row("Notification", &note("permission_prompt")).unwrap().state.as_deref(),
            Some("waiting")
        );
        assert_eq!(
            s.row("Notification", &note("elicitation_dialog")).unwrap().state.as_deref(),
            Some("waiting"),
            "one of a list"
        );
        assert_eq!(s.row("Notification", &note("idle_prompt")), None, "another notification");
        assert_eq!(s.row("Notification", &json!({"session_id": "s1"})), None, "the field missing");
        let stop = |reason: &str| json!({"session_id": "s1", "reason": reason});
        assert_eq!(s.row("Stop", &stop("end_turn")).unwrap().state.as_deref(), Some("turn_ended"));
        assert_eq!(
            s.sent("grok", "Stop", &stop("shutdown")),
            None,
            "a session's last Stop is no turn's end"
        );
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
    fn a_row_that_forwards_no_tool_sends_no_tool() {
        // Codex's PostToolUse looks for a goal it received,
        // never at the tool: a failed command also arrives there.
        let s = signal("codex");
        let payload = json!({"session_id": "c1", "cwd": "/w", "tool_name": "shell",
                             "tool_input": {"command": "npm test"}});
        let sent = s.sent("codex", "PostToolUse", &payload).unwrap();
        assert_eq!((sent.tool, sent.command, sent.prompt), (None, None, None));
        assert_eq!(sent.forward.as_deref(), Some("prompt"));
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

    /// The prompt `hook` gives when the file the payload names ends with
    /// `tail`.
    fn read(s: &Signal, hook: &str, payload: &Value, tail: &str) -> Option<String> {
        let end = |_: &str| Some((0, tail.to_string()));
        s.prompt_in(hook, payload, &end, &|_| None).map(|r| r.text)
    }

    #[test]
    fn a_prompt_can_be_the_last_line_of_a_file_the_payload_names() {
        let s = from_a_file();
        let payload = json!({"id": "s1", "dir": "/w", "log": {"path": "/h/log.jsonl"}});
        let asked = std::cell::RefCell::new(Vec::new());
        let end = |p: &str| {
            asked.borrow_mut().push(p.to_string());
            None
        };
        s.prompt_in("Began", &payload, &end, &|_| None);
        assert_eq!(*asked.borrow(), ["/h/log.jsonl"], "the file the payload names");
        let typed = "/rf:ask fix it — now\n  with lines  ";
        let line = |kind: &str, text: &str| {
            json!({"kind": kind, "body": {"text": format!("<said>\n{text}\n</said>\n<meta>9:00</meta>")}})
                .to_string()
        };
        let tail = format!("{}\n{}\n", line("answer", "earlier"), line("typed", typed));
        assert_eq!(read(&s, "Ended", &payload, &tail), None, "its row forwards nothing");
        assert_eq!(
            read(&s, "Began", &payload, &tail).as_deref(),
            Some(typed),
            "byte for byte, between the markers"
        );
        let sent = s.sent("made-up", "Began", &payload).unwrap();
        assert_eq!(
            (sent.prompt, sent.forward.as_deref()),
            (None, Some("prompt")),
            "read by weft signal"
        );
    }

    #[test]
    fn a_file_prompt_is_none_unless_its_last_line_is_one() {
        let s = from_a_file();
        let payload = json!({"id": "s1", "dir": "/w", "log": {"path": "/h/log.jsonl"}});
        let typed =
            json!({"kind": "typed", "body": {"text": "<said>\nhello\n</said>"}}).to_string();
        let tool = json!({"kind": "tool", "body": {"text": "ran"}}).to_string();
        assert_eq!(
            read(&s, "Began", &payload, &format!("{typed}\n{tool}\n")),
            None,
            "a later step: no new prompt"
        );
        let unmarked = json!({"kind": "typed", "body": {"text": "no markers"}}).to_string();
        assert_eq!(read(&s, "Began", &payload, &unmarked), None, "markers named and absent");
        assert_eq!(read(&s, "Began", &payload, "not json\n"), None);
        assert_eq!(read(&s, "Began", &payload, ""), None);
        let plain = Signal::of(&json!({"session": "id", "workspace": "dir",
            "prompt": {"file": "f", "last": {"kind": "typed"}, "field": "body.text"},
            "on": [{"hook": "Began", "forward": "prompt"}]}))
        .unwrap();
        assert_eq!(
            read(&plain, "Began", &json!({"f": "/f"}), &format!("{typed}\n")).as_deref(),
            Some("<said>\nhello\n</said>")
        );
        assert_eq!(
            read(&signal("claude-code"), "UserPromptSubmit", &json!({"session_id": "s"}), &typed),
            None,
            "a payload-path prompt: no file is read"
        );
    }

    /// The Antigravity fixture's transcript, as agy 1.3.2 writes it (probe A,
    /// `probe-agy-ab`): a typed prompt, then a tool step.
    #[test]
    fn a_transcript_line_gives_the_typed_prompt_once() {
        let s = signal("antigravity");
        let payload =
            json!({"conversationId": "a1", "workspacePaths": ["/w"], "transcriptPath": "/t.jsonl"});
        let typed = json!({"step_index": 0, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE",
            "content": "<USER_REQUEST>\n/rf:ask Plan it.\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-10-09T09:24:12Z.\n</ADDITIONAL_METADATA>"});
        let step = json!({"step_index": 2, "source": "MODEL", "type": "GENERIC", "status": "DONE", "content": "Created file"});
        assert_eq!(
            read(&s, "PreInvocation", &payload, &format!("{typed}\n")).as_deref(),
            Some("/rf:ask Plan it.")
        );
        assert_eq!(
            read(&s, "PreInvocation", &payload, &format!("{typed}\n{step}\n")),
            None,
            "a later invocation of the same turn"
        );
    }

    /// A made-up harness that records some prompts only in its transcript:
    /// a list of places, the payload's first, then the newest matching line
    /// of the transcript, whose text may name a file holding the prompt.
    fn in_several_places() -> Signal {
        Signal::of(&json!({
            "session": "id", "workspace": "dir",
            "prompt": ["said", {"file": "log", "newest": true, "last": {"kind": "goal"},
                                "field": "goal.text", "names_file": ["kept at: ", ". Read it"]}],
            "on": [{"hook": "Submitted", "state": "working", "forward": "prompt"},
                   {"hook": "Done", "state": "turn_ended", "forward": "prompt"},
                   {"hook": "Began", "state": "ready"}]
        }))
        .expect("a [signal] with several places")
    }

    /// A transcript, read from offset 100, with a goal, then steps after it.
    fn transcript(goal: &str) -> (u64, String) {
        let line = |v: Value| format!("{v}\n");
        let text = [
            line(json!({"kind": "typed", "text": "earlier"})),
            line(json!({"kind": "goal", "goal": {"text": goal}})),
            line(json!({"kind": "step", "text": "ran a tool"})),
            line(json!({"kind": "tokens", "n": 9})),
        ]
        .concat();
        (100, text)
    }

    #[test]
    fn the_payload_is_the_first_place_and_the_transcript_the_next() {
        let s = in_several_places();
        let typed = json!({"id": "s1", "dir": "/w", "said": "fix it", "log": "/t.jsonl"});
        let got = s.prompt_in("Submitted", &typed, &|_| None, &|_| None).expect("from the payload");
        assert_eq!((got.text.as_str(), got.key), ("fix it", None));
        let ended = json!({"id": "s1", "dir": "/w", "log": "/t.jsonl"});
        let tail = |p: &str| (p == "/t.jsonl").then(|| transcript("ship the endpoint"));
        let got = s.prompt_in("Done", &ended, &tail, &|_| None).expect("from the transcript");
        assert_eq!(got.text, "ship the endpoint", "the newest goal line, not the last line");
        let first = transcript("x").1.lines().next().unwrap().len() as u64 + 1;
        assert_eq!(got.key, Some(format!("/t.jsonl:{}", 100 + first)), "where it was read");
        assert_eq!(
            s.prompt_in("Began", &ended, &tail, &|_| None),
            None,
            "its row forwards nothing"
        );
    }

    #[test]
    fn a_text_that_names_a_file_is_that_files_bytes() {
        let s = in_several_places();
        let ended = json!({"id": "s1", "dir": "/w", "log": "/t.jsonl"});
        let tail = |_: &str| Some(transcript("kept at: /a/pasted.txt. Read it first."));
        let whole = |p: &str| (p == "/a/pasted.txt").then(|| "a long prompt\nof lines".to_string());
        let got = s.prompt_in("Done", &ended, &tail, &whole).expect("the named file");
        assert_eq!(got.text, "a long prompt\nof lines");
        let gone = |_: &str| None;
        assert_eq!(s.prompt_in("Done", &ended, &tail, &gone), None, "named and unreadable");
        let plain = |_: &str| Some(transcript("no file named here"));
        let got = s.prompt_in("Done", &ended, &plain, &gone).expect("a text naming no file");
        assert_eq!(got.text, "no file named here");
    }

    #[test]
    fn without_newest_only_the_last_line_counts() {
        let s = from_a_file();
        let payload = json!({"id": "s1", "dir": "/w", "log": {"path": "/h/log.jsonl"}});
        let line = json!({"kind": "typed", "body": {"text": "<said>\nhi\n</said>"}}).to_string();
        let step = json!({"kind": "tool", "body": {"text": "ran"}}).to_string();
        let last = |_: &str| Some((0, format!("{line}\n")));
        let got = s.prompt_in("Began", &payload, &last, &|_| None).map(|r| r.text);
        assert_eq!(got.as_deref(), Some("hi"));
        let later = |_: &str| Some((0, format!("{line}\n{step}\n")));
        assert_eq!(s.prompt_in("Began", &payload, &later, &|_| None), None, "a later step");
    }

    /// The Codex fixture's transcript, as codex 0.162.0 writes a `/goal`
    /// (HostLab probe goal-hooks): the goal line, then the goal turn's lines;
    /// a long paste is a file the objective names.
    #[test]
    fn a_codex_goal_is_read_from_its_transcript() {
        let s = signal("codex");
        let payload = json!({"session_id": "c1", "cwd": "/w", "transcript_path": "/r.jsonl",
            "turn_id": "t2", "hook_event_name": "Stop", "last_assistant_message": "ok"});
        let goal = |objective: &str| {
            json!({"timestamp": "2026-10-10T14:26:55.065Z", "type": "event_msg",
            "payload": {"type": "thread_goal_updated", "threadId": "c1",
                        "goal": {"threadId": "c1", "objective": objective, "status": "active"}}})
        };
        let after = [
            json!({"type": "event_msg", "payload": {"type": "task_started", "turn_attribution": {"turn_trigger": "goal"}}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "content": []}}),
            json!({"type": "event_msg", "payload": {"type": "token_count"}}),
        ]
        .map(|v| format!("{v}\n"))
        .concat();
        let short = format!("{}\n{after}", goal("Reply with the single word ok."));
        let end = |_: &str| Some((0, short.clone()));
        let got = s.prompt_in("Stop", &payload, &end, &|_| None).expect("the goal line");
        assert_eq!(got.text, "Reply with the single word ok.");
        let named = "pasted text file: /h/.codex/attachments/a1/pasted-text-1.txt. Read this file before continuing.";
        let long = format!("{}\n{after}", goal(named));
        let end = |_: &str| Some((0, long.clone()));
        let whole = |p: &str| {
            (p == "/h/.codex/attachments/a1/pasted-text-1.txt").then(|| "the pasted prompt".into())
        };
        let got = s.prompt_in("PostToolUse", &payload, &end, &whole).expect("the file it names");
        assert_eq!(got.text, "the pasted prompt");
        let typed = json!({"session_id": "c1", "cwd": "/w", "transcript_path": "/r.jsonl", "prompt": "fix it"});
        let got = s.prompt_in("UserPromptSubmit", &typed, &end, &whole).expect("typed");
        assert_eq!((got.text.as_str(), got.key), ("fix it", None), "the payload first");
    }

    /// The Claude Code fixture's transcript, as claude-code 2.1.296 writes a
    /// long `/goal` (HostLab, ringframe-ask-ledger-q42): its prompt hook says
    /// `Goal set: <objective>`, and the transcript keeps the objective whole
    /// in a goal_status line, written again on every turn the goal is set.
    /// Every place is read: the hook's text first, then the transcript's.
    #[test]
    fn every_place_a_claude_goal_is_recorded_is_read_in_order() {
        let s = signal("claude-code");
        let status = |condition: &str| {
            json!({"type": "attachment", "attachment": {"type": "goal_status", "met": false,
                   "sentinel": true, "condition": condition}})
        };
        let snapshot = json!({"type": "file-history-snapshot"});
        let objective = "[command-3500] Reply with the single word ok.\nEnd of the prompt.";
        let tail = [status("an older goal"), status(objective), snapshot]
            .map(|v| format!("{v}\n"))
            .concat();
        let end = |_: &str| Some((0, tail.clone()));
        let said = |prompt: &str| {
            json!({"session_id": "c1", "cwd": "/w", "transcript_path": "/r.jsonl",
                   "hook_event_name": "UserPromptSubmit", "prompt": prompt})
        };
        let host = format!("Goal set: {objective}");
        let got = s.received_in("UserPromptSubmit", &said(&host), &end, &|_| None);
        let texts: Vec<&str> = got.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            texts,
            [host.as_str(), objective],
            "the hook's words, then the newest goal line"
        );
        assert_eq!(got[0].key, None);
        assert!(got[1].key.is_some(), "a line is forwarded once");
        let got = s.prompt_in("UserPromptSubmit", &said("fix it"), &end, &|_| None).expect("typed");
        assert_eq!(
            (got.text.as_str(), got.key),
            ("fix it", None),
            "the received prompt is the first"
        );
        let none = |_: &str| None;
        let got = s.received_in("UserPromptSubmit", &said("fix it"), &none, &|_| None);
        assert_eq!(got.len(), 1, "an unreadable transcript is no place");
    }

    #[test]
    fn the_other_records_go_to_ringframe_as_layers_after_the_received_prompt() {
        let sent = |also: Vec<Received>| Sent {
            harness: "claude-code".into(),
            hook: "UserPromptSubmit".into(),
            session: "c1".into(),
            forward: Some("prompt".into()),
            prompt: Some("Goal set: ship it".into()),
            also,
            ..Sent::default()
        };
        let one = forward(&sent(vec![]), &Value::Null).expect("a prompt");
        assert!(!one.args.contains(&"--layers".to_string()));
        assert_eq!(one.input.as_deref(), Some("Goal set: ship it"), "its bytes, as before");
        let two = forward(
            &sent(vec![Received { text: "ship it".into(), key: Some("/r:9".into()) }]),
            &Value::Null,
        )
        .expect("a prompt");
        assert_eq!(two.args.last().map(String::as_str), Some("--layers"));
        let layers: Vec<String> = serde_json::from_str(two.input.as_deref().unwrap()).unwrap();
        assert_eq!(layers, ["Goal set: ship it", "ship it"]);
    }

    #[test]
    fn a_codex_question_that_does_not_stop_the_turn_is_pending_until_answered() {
        let s = signal("codex");
        let payload = json!({"session_id": "c1", "cwd": "/w", "transcript_path": "/r.jsonl"});
        // As Codex 0.162.1 records them: the question, the call's output, and
        // later the person's answer naming the call.
        let asked = |id: &str| {
            json!({"type": "event_msg", "payload": {"type": "item_completed", "thread_id": "c1",
                "item": {"type": "AgentMessage", "id": id, "phase": "final_answer", "delivery": "async",
                         "content": [{"type": "Text", "text": "Red or blue?"}],
                         "questions": [{"title": "Red or blue?", "options": null}]}}})
        };
        let accepted = |id: &str| {
            json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": id,
                "output": "{\"accepted\":true}"}})
        };
        let answer = |id: &str| {
            let reply = format!(
                "<send_user_message_question_reply>\n[{{\"answer\":\"blue\",\"question\":\"Red or blue?\",\"questionItemId\":\"[\\\"request_user_input_async\\\",\\\"{id}\\\",0]\"}}]\n</send_user_message_question_reply>"
            );
            json!({"type": "response_item", "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": reply}]}})
        };
        let typed = json!({"type": "event_msg", "payload": {"type": "item_completed",
            "item": {"type": "UserMessage", "content": [{"type": "text", "text": "carry on"}]}}});
        let replied = |id: &str| {
            json!({"type": "event_msg", "payload": {"type": "item_completed",
                "item": {"type": "UserMessage", "content": [{"type": "text",
                    "text": format!("<send_user_message_question_reply>\n[{{\"questionItemId\":\"[\\\"request_user_input_async\\\",\\\"{id}\\\",0]\"}}]")}]}}})
        };
        let file = |lines: &[Value]| lines.iter().map(|v| format!("{v}\n")).collect::<String>();
        let at = |text: String| s.asking(&payload, &move |_| Some((0, text.clone())));

        assert_eq!(at(file(&[asked("call_a"), accepted("call_a")])), Some(true), "asked");
        assert_eq!(
            at(file(&[asked("call_a"), typed.clone()])),
            Some(false),
            "the person typed past it"
        );
        assert_eq!(at(file(&[typed.clone(), asked("call_a")])), Some(true), "typed before it");
        assert_eq!(
            at(file(&[asked("call_a"), asked("call_b"), replied("call_a")])),
            Some(true),
            "an answer is not typing past the others"
        );
        assert_eq!(at(file(&[asked("call_a"), answer("call_a")])), Some(false), "answered");
        assert_eq!(
            at(file(&[asked("call_a"), asked("call_b"), answer("call_a")])),
            Some(true),
            "one of two answered"
        );
        assert_eq!(at(file(&[answer("call_a"), asked("call_a")])), Some(true), "asked again after");
        assert_eq!(at(file(&[accepted("call_a")])), Some(false), "nothing asked");
        // A harness without the table, or a payload naming no file, reads none.
        assert_eq!(signal("claude-code").asking(&payload, &|_| Some((0, String::new()))), None);
        assert_eq!(s.asking(&json!({"session_id": "c1"}), &|_| Some((0, String::new()))), None);
    }

    /// Grok Build's hooks, as 1.0.52 sends them: both namings, `/plan <text>`
    /// arriving as `<text>`, a plan's approval only as a notification, and a
    /// closing session's `Stop` that ends no turn.
    #[test]
    fn grok_hooks_read_as_its_harness_file_says() {
        let s = signal("grok");
        let base = json!({"hookEventName": "x", "sessionId": "g1", "session_id": "g1", "cwd": "/w", "workspaceRoot": "/w/"});
        let with = |extra: Value| {
            let mut p = base.clone();
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            p
        };
        let state =
            |hook: &str, extra: Value| s.sent("grok", hook, &with(extra)).and_then(|m| m.state);
        assert_eq!(state("SessionStart", json!({"source": "new"})).as_deref(), Some("ready"));
        let typed = s
            .sent("grok", "UserPromptSubmit", &with(json!({"prompt": "Write a one-line plan."})))
            .unwrap();
        assert_eq!(
            (typed.state.as_deref(), typed.prompt.as_deref()),
            (Some("working"), Some("Write a one-line plan."))
        );
        assert_eq!(
            state("PreToolUse", json!({"tool_name": "ask_user_question"})).as_deref(),
            Some("waiting")
        );
        assert_eq!(state("PreToolUse", json!({"tool_name": "run_terminal_command"})), None);
        assert_eq!(
            state("Notification", json!({"notificationType": "permission_prompt"})).as_deref(),
            Some("waiting")
        );
        assert_eq!(state("Notification", json!({"notificationType": "idle_prompt"})), None);
        let ran = s.sent("grok", "PostToolUse", &with(json!({"tool_name": "run_terminal_command", "tool_input": {"command": "echo hi"}}))).unwrap();
        assert_eq!(
            (ran.tool.as_deref(), ran.command.as_deref(), ran.outcome.as_deref()),
            (Some("run_terminal_command"), Some("echo hi"), Some("succeeded"))
        );
        assert_eq!(state("Stop", json!({"reason": "end_turn"})).as_deref(), Some("turn_ended"));
        assert_eq!(
            state("StopCancelled", json!({"reason": "user_interrupt"})).as_deref(),
            Some("turn_ended")
        );
        assert_eq!(state("SessionEnd", json!({"reason": "shutdown"})).as_deref(), Some("ended"));
        assert_eq!(
            state("Stop", json!({"reason": "shutdown"})),
            None,
            "the Stop after SessionEnd ends no turn"
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
