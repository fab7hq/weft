//! A harness's own transcript read as a conversation, as its harness file's
//! `[conversation]` says: the person's messages, the agent's replies, its tool
//! steps and their outcomes. Nothing else of a line is taken, and an entry no
//! rule matches is not drawn, so a newer release of the harness is read as far
//! as its lines still match. Pure: the caller reads the bytes and keeps nothing.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// A harness file's `[conversation]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
    /// The harness release the mapping was tested and verified against.
    pub verified: String,
    /// The hook payload's path of the transcript file.
    pub path: String,
    /// A glob under the configuration home that finds the file by `{session}`.
    #[serde(default)]
    pub find: Option<String>,
    /// An entry's path of its time: RFC 3339, or seconds (milliseconds
    /// past 10^12) since the epoch.
    pub time: String,
    /// Entry paths: an entry where any is `true` is never drawn.
    #[serde(default)]
    pub skip: Vec<String>,
    pub person: Rules<TextRule>,
    pub agent: Rules<TextRule>,
    pub steps: Rules<StepRule>,
    #[serde(default)]
    pub outcomes: Option<Rules<OutcomeRule>>,
    /// Per tool, the path in its input of its one-line argument.
    #[serde(default)]
    pub argument: BTreeMap<String, String>,
    /// Per tool, the name drawn in its place.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
}

/// One rule, or a list of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Rules<R> {
    One(R),
    Each(Vec<R>),
}

impl<R> Rules<R> {
    fn list(&self) -> &[R] {
        match self {
            Rules::One(r) => std::slice::from_ref(r),
            Rules::Each(all) => all,
        }
    }
}

/// The person's message or the agent's reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRule {
    pub when: Map<String, Value>,
    /// The path of its text: a string, or a list of blocks whose `type` is `text`.
    pub text: String,
}

/// A tool step: an entry, or each block of a list in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepRule {
    pub when: Map<String, Value>,
    /// The path of a list of blocks, each a step when its `type` is `kind`.
    #[serde(default, rename = "in")]
    pub within: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    /// The path of the tool's name, or a fixed `name`.
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub input: Option<String>,
    /// The path of the id a later outcome names. A step without one is
    /// settled by its own entry.
    #[serde(default)]
    pub call: Option<String>,
    /// Fields that say the step failed, in the step itself.
    #[serde(default)]
    pub failed: Option<Map<String, Value>>,
}

/// A step's outcome, written in a later entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeRule {
    pub when: Map<String, Value>,
    #[serde(default, rename = "in")]
    pub within: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    pub call: String,
    #[serde(default)]
    pub failed: Option<Map<String, Value>>,
}

/// One thing drawn, as a device receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// The line's byte offset and the item's place among its line's items:
    /// stable while the file only grows.
    pub id: String,
    pub kind: Kind,
    /// Milliseconds since the epoch.
    pub at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// A step: whether it failed, when its own entry settles it. An outcome:
    /// whether the step it settles failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Person,
    Agent,
    Step,
    Outcome,
}

/// Where a read stands in one transcript: which file (it may be replaced)
/// and the byte offset of a line's start. Opaque to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub file: u64,
    pub at: u64,
}

impl Cursor {
    pub fn text(self) -> String {
        format!("{}-{}", self.file, self.at)
    }

    pub fn of(text: &str) -> Option<Self> {
        let (file, at) = text.split_once('-')?;
        Some(Cursor { file: file.parse().ok()?, at: at.parse().ok()? })
    }

    /// The offset of the line an item came from, by its id.
    pub fn line(id: &str) -> Option<u64> {
        id.split('.').next()?.parse().ok()
    }

    /// In the same file, at the line an item came from.
    pub fn at_item(self, id: &str) -> Option<Self> {
        Some(Cursor { file: self.file, at: Self::line(id)? })
    }
}

/// The items of some whole lines, and how many bytes they took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub items: Vec<Item>,
    /// The bytes read, up to the end of the last whole line: an unfinished
    /// last line (the harness still writing) is left for the next read.
    pub end: usize,
}

/// A line that is not JSON: the file cannot be read as a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable;

impl Conversation {
    /// A harness file's `[conversation]`, when it has one that reads.
    pub fn of(table: &Value) -> Option<Self> {
        serde_json::from_value(table.clone()).ok()
    }

    /// The items of `bytes`, which start at `offset` in the file.
    pub fn read(&self, bytes: &[u8], offset: u64) -> Result<Read, Unreadable> {
        let mut items = Vec::new();
        let mut start = 0;
        while let Some(n) = bytes[start..].iter().position(|&b| b == b'\n') {
            let line = &bytes[start..start + n];
            let at = offset + start as u64;
            start += n + 1;
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let entry: Value = serde_json::from_slice(line).map_err(|_| Unreadable)?;
            let before = items.len();
            self.entry(&entry, &mut items);
            for (n, item) in items[before..].iter_mut().enumerate() {
                item.id = format!("{at}.{n}");
            }
        }
        Ok(Read { items, end: start })
    }

    /// One entry's items, in order: a message or reply, its steps, then the
    /// outcomes it holds.
    fn entry(&self, entry: &Value, items: &mut Vec<Item>) {
        if self.skip.iter().any(|p| path(entry, p) == Some(&Value::Bool(true))) {
            return;
        }
        let at = path(entry, &self.time).and_then(time);
        let item = |kind| Item {
            id: String::new(),
            kind,
            at,
            text: None,
            tool: None,
            arg: None,
            call: None,
            failed: None,
        };
        for (kind, rules) in [(Kind::Person, &self.person), (Kind::Agent, &self.agent)] {
            let said = rules
                .list()
                .iter()
                .find(|r| fields(entry, &r.when))
                .and_then(|r| path(entry, &r.text))
                .map(text_of);
            if let Some(said) = said.filter(|s| !s.is_empty()) {
                items.push(Item { text: Some(said), ..item(kind) });
                break;
            }
        }
        for rule in self.steps.list().iter().filter(|r| fields(entry, &r.when)) {
            for block in blocks(entry, rule.within.as_deref(), rule.kind.as_deref()) {
                let tool = rule.name.clone().or_else(|| string(block, rule.tool.as_deref()?));
                let Some(tool) = tool else { continue };
                let input = rule.input.as_deref().and_then(|p| input(block, p));
                let failed = rule.failed.as_ref().is_some_and(|f| fields(block, f));
                items.push(Item {
                    arg: Some(self.argument_of(&tool, input.as_ref())),
                    tool: Some(self.names.get(&tool).cloned().unwrap_or(tool)),
                    // A step joined to a later outcome is running until it comes.
                    failed: if rule.call.is_some() { failed.then_some(true) } else { Some(failed) },
                    call: rule.call.as_deref().and_then(|p| string(block, p)),
                    ..item(Kind::Step)
                });
            }
        }
        for rule in self.outcomes.iter().flat_map(Rules::list).filter(|r| fields(entry, &r.when)) {
            for block in blocks(entry, rule.within.as_deref(), rule.kind.as_deref()) {
                let Some(call) = string(block, &rule.call) else { continue };
                let failed = rule.failed.as_ref().is_some_and(|f| fields(block, f));
                items.push(Item { call: Some(call), failed: Some(failed), ..item(Kind::Outcome) });
            }
        }
    }

    /// A step's one line: its tool's own path, else a string's first line, a
    /// list's last string, or a table's first string value.
    fn argument_of(&self, tool: &str, input: Option<&Value>) -> String {
        let Some(input) = input else { return String::new() };
        let picked = self.argument.get(tool).and_then(|p| path(input, p)).unwrap_or(input);
        let text = match picked {
            Value::String(s) => Some(s.as_str()),
            Value::Array(all) => all.iter().rev().find_map(Value::as_str),
            Value::Object(table) => table.values().find_map(Value::as_str),
            _ => None,
        };
        text.and_then(|t| t.lines().next()).unwrap_or_default().to_string()
    }
}

/// The value at a dotted path: a number steps into a list, and `-1` is its
/// last entry.
fn path<'a>(v: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted.split('.').try_fold(v, |v, key| match (v, key) {
        (Value::Array(all), "-1") => all.last(),
        (Value::Array(all), k) => all.get(k.parse::<usize>().ok()?),
        (Value::Object(t), k) => t.get(k),
        _ => None,
    })
}

/// A step's input: a path, which may end in `@key`, a table's first key.
fn input(v: &Value, dotted: &str) -> Option<Value> {
    match dotted.strip_suffix("@key") {
        Some(table) => {
            let table = table.strip_suffix('.').map_or(Some(v), |p| path(v, p))?;
            Some(Value::String(table.as_object()?.keys().next()?.clone()))
        }
        None => path(v, dotted).cloned(),
    }
}

fn string(v: &Value, dotted: &str) -> Option<String> {
    path(v, dotted)?.as_str().map(str::to_string)
}

/// Whether every path holds its value, or one of a list of values.
fn fields(v: &Value, want: &Map<String, Value>) -> bool {
    want.iter().all(|(p, want)| {
        let got = path(v, p);
        match want {
            Value::Array(any) => any.iter().any(|w| got == Some(w)),
            one => got == Some(one),
        }
    })
}

/// The blocks a rule reads: the entry itself, or each block in its list
/// whose `type` is `kind`.
fn blocks<'a>(entry: &'a Value, within: Option<&str>, kind: Option<&str>) -> Vec<&'a Value> {
    let Some(within) = within else { return vec![entry] };
    let Some(Value::Array(all)) = path(entry, within) else { return Vec::new() };
    all.iter()
        .filter(|b| kind.is_none_or(|k| b.get("type").and_then(Value::as_str) == Some(k)))
        .collect()
}

/// A string, or the texts of a list of blocks whose `type` is `text` in any
/// case, joined by a blank line.
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(all) => all
            .iter()
            .filter(|b| {
                b.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.eq_ignore_ascii_case("text"))
            })
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

/// Milliseconds since the epoch.
fn time(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => crate::turns::millis(s),
        Value::Number(n) => {
            let n = n.as_f64()?;
            Some(if n > 1e12 { n as i64 } else { (n * 1000.0) as i64 })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::fixture::file;

    fn mapping(harness: &str) -> Conversation {
        Conversation::of(&file(harness)["conversation"]).expect("its [conversation]")
    }

    /// The items as a person reads them: each step settled by its outcome.
    fn drawn(items: &[Item]) -> Vec<String> {
        let settled = |call: &Option<String>| {
            items
                .iter()
                .find(|o| o.kind == Kind::Outcome && call.is_some() && o.call == *call)
                .and_then(|o| o.failed)
        };
        items
            .iter()
            .filter_map(|i| match i.kind {
                Kind::Person => Some(format!("person: {}", i.text.as_deref()?.lines().next()?)),
                Kind::Agent => Some(format!("agent: {}", i.text.as_deref()?)),
                Kind::Step => {
                    let outcome = match i.failed.or_else(|| settled(&i.call)) {
                        Some(true) => "✗",
                        Some(false) => "✓",
                        None => "running",
                    };
                    Some(format!("step: {} {} {outcome}", i.tool.as_deref()?, i.arg.as_deref()?))
                }
                Kind::Outcome => None,
            })
            .collect()
    }

    /// Grok Build 1.0.52's `updates.jsonl` (ACP session updates), recorded in
    /// HostLab on the local model: thoughts, hook runs and turn ends are not
    /// drawn; a step's outcome is a later `tool_call_update`.
    #[test]
    fn grok_reads_as_its_messages_replies_and_steps() {
        let text = include_str!("../tests/fixtures/conversation/grok.jsonl");
        let read = mapping("grok").read(text.as_bytes(), 0).expect("readable");
        assert_eq!(read.end, text.len());
        assert_eq!(
            drawn(&read.items),
            [
                "person: Reply with the single word ok.",
                "agent: ok",
                "person: Reply with the single word ok; that reply completes this task. Do not read or change any file. The lines below are context only and ask for nothing.",
                "agent: ok",
                "person: Run the shell command `echo probe-marker` and then reply with its output.",
                "step: Run echo probe-marker ✓",
                "agent: probe-marker",
                "person: Call the tool ask_user_question now with one question, \"Red or blue?\", and the options Red and Blue. Then reply with the answer.",
                "step: Ask Red or blue? ✓",
                "agent: Red",
                "person: Write a one-line plan that says ok.",
                "step: Write /home/hostlab/.grok/sessions/%2Fruns%2Fprobe-grok-observe%2F2026-10-14T064103565Z%2Fworkspace/01a13924-9f2e-7100-be91-deae6b63716f/plan.md ✓",
                "step: Plan  ✗",
            ]
        );
        assert!(read.items.iter().all(|i| i.at.is_some()));
        let ids: std::collections::BTreeSet<_> = read.items.iter().map(|i| &i.id).collect();
        assert_eq!(ids.len(), read.items.len(), "every id is its own");
    }

    /// Claude Code 2.1.296's session file, recorded in HostLab on the local
    /// model: thinking, attachments, tool results' output, an interruption
    /// and bookkeeping are not drawn.
    #[test]
    fn claude_code_reads_as_its_messages_replies_and_steps() {
        let text = include_str!("../tests/fixtures/conversation/claude-code.jsonl");
        let read = mapping("claude-code").read(text.as_bytes(), 0).expect("readable");
        assert_eq!(
            drawn(&read.items),
            [
                "person: Reply with the single word ok.",
                "agent: ok",
                "person: Run the shell command `cat notes.txt` and tell me how many lines it printed.",
                "step: Bash cat notes.txt ✓",
                "agent: The `cat notes.txt` command printed 2 lines.",
                "person: Run the shell command `cat no-such-file.txt` and tell me what happened.",
                "step: Bash cat no-such-file.txt ✗",
                "agent: The command `cat no-such-file.txt` failed because the file does not exist. The output was:\n\n```\nExit code 1\ncat: no-such-file.txt: No such file or directory\n```",
                "person: Reply with the single word ok; that reply completes this task. Do not read or change any file. The lines below are context only and ask for nothing.",
                "agent: ok",
                "person: Run the shell command `sleep 30` and then reply with done.",
                "step: Bash sleep 30 ✗",
            ]
        );
        assert!(!format!("{:?}", read.items).contains("one\\ntwo"), "a tool's output");
    }

    /// Codex 0.162.1's rollout, recorded in HostLab on the local model: what
    /// its screen drew, never the model's input, reasoning or token counts.
    #[test]
    fn codex_reads_as_its_messages_replies_and_steps() {
        let text = include_str!("../tests/fixtures/conversation/codex.jsonl");
        let read = mapping("codex").read(text.as_bytes(), 0).expect("readable");
        assert_eq!(
            drawn(&read.items),
            [
                "person: Reply with the single word ok.",
                "agent: ok",
                "person: Run the shell command `cat notes.txt` and tell me how many lines it printed.",
                "step: Ran cat notes.txt ✓",
                "agent: The output contains 2 lines.",
                "person: Run the shell command `cat no-such-file.txt` and tell me what happened.",
                "step: Ran cat no-such-file.txt ✗",
                "step: Ran wc -l no-such-file.txt ✗",
                "agent: The command `cat no-such-file.txt` failed because the file does not exist (exit code 1, \"No such file or directory\"). The `wc -l` command also returned an error about the file not existing.",
                "person: Reply with the single word ok; that reply completes this task. Do not read or change any file. The lines below are context only and ask for nothing.",
                "agent: ok",
                "person: Run the shell command `sleep 30` and then reply with done.",
                "agent: sleep 30\ndone",
            ]
        );
        assert!(!format!("{:?}", read.items).contains("one\\ntwo"), "a tool's output");
    }

    #[test]
    fn a_read_from_an_offset_gives_the_same_items_and_ids() {
        let text = include_str!("../tests/fixtures/conversation/grok.jsonl");
        let whole = mapping("grok").read(text.as_bytes(), 0).unwrap().items;
        let cut = text.match_indices('\n').nth(9).unwrap().0 + 1;
        let tail = mapping("grok").read(&text.as_bytes()[cut..], cut as u64).unwrap().items;
        assert_eq!(whole[whole.len() - tail.len()..], tail[..]);
    }

    #[test]
    fn an_unknown_entry_is_not_drawn() {
        let text = concat!(
            r#"{"timestamp":1791960102,"params":{"update":{"sessionUpdate":"something_new","content":{"type":"text","text":"never"}}}}"#,
            "\n",
            r#"{"timestamp":1791960102,"params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}"#,
            "\n"
        );
        let items = mapping("grok").read(text.as_bytes(), 0).unwrap().items;
        assert_eq!(drawn(&items), ["agent: hi"]);
    }

    /// A made-up harness, its transcript of a shape no real one has, read
    /// with no code change; a harness file with no `[conversation]` still
    /// reads, and one whose table has no `verified` does not.
    #[test]
    fn a_made_up_harness_reads_from_its_table_alone() {
        let toml = r#"
            verified = "made-up 1.0"
            path = "log"
            time = "when"
            skip = ["hidden"]
            person = { when = { kind = "said", who = "me" }, text = "body" }
            agent = [{ when = { kind = "said", who = "bot" }, text = "body" }]
            steps = { when = { kind = "did" }, in = "calls", kind = "tool", tool = "name", input = "args", call = "id" }
            outcomes = { when = { kind = "done" }, in = "results", kind = "result", call = "for", failed = { error = true } }
            argument = { Bash = "command" }
            names = { Bash = "Shell" }
        "#;
        let table = serde_json::to_value(toml.parse::<toml::Table>().unwrap()).unwrap();
        let c = Conversation::of(&table).expect("reads");
        let text = [
            r#"{"kind":"said","who":"me","body":"hi","when":"2026-10-14T00:00:00Z"}"#,
            r#"{"kind":"said","who":"me","body":"secret","hidden":true}"#,
            r#"{"kind":"said","who":"bot","body":[{"type":"Text","text":"one"},{"type":"image"},{"type":"text","text":"two"}]}"#,
            r#"{"kind":"did","calls":[{"type":"tool","id":"1","name":"Bash","args":{"command":"ls -la\nmore"}},{"type":"note"}]}"#,
            r#"{"kind":"did","calls":[{"type":"tool","id":"2","name":"Read","args":{"path":"a.txt"}}]}"#,
            r#"{"kind":"done","results":[{"type":"result","for":"1","error":true,"output":"never"}]}"#,
        ]
        .join("\n");
        let items = c.read(format!("{text}\n").as_bytes(), 0).unwrap().items;
        assert_eq!(
            drawn(&items),
            ["person: hi", "agent: one\n\ntwo", "step: Shell ls -la ✗", "step: Read a.txt running"]
        );
        assert!(
            !format!("{items:?}").contains("never") && !format!("{items:?}").contains("secret")
        );

        let mut file = file("grok");
        file.as_object_mut().unwrap().remove("conversation");
        assert!(
            crate::harness::Harness::of("grok", &file).is_some(),
            "no [conversation] still reads"
        );
        file["conversation"] = table.clone();
        file["conversation"].as_object_mut().unwrap().remove("verified");
        assert!(crate::harness::Harness::of("grok", &file).is_none(), "no verified");
    }

    #[test]
    fn a_line_that_is_not_json_is_unreadable_but_an_unfinished_last_line_waits() {
        let good = r#"{"timestamp":1,"params":{"update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"hi"}}}}"#;
        let grok = mapping("grok");
        assert_eq!(grok.read(format!("{good}\nnot json\n{good}\n").as_bytes(), 0), Err(Unreadable));
        let read = grok.read(format!("{good}\n{good}").as_bytes(), 0).unwrap();
        assert_eq!((read.items.len(), read.end), (1, good.len() + 1));
    }
}
