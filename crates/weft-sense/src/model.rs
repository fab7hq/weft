use crate::ask::{AskAnswer, AskHeads, Rule};
use crate::graph::{Graph, Provider};
use crate::heads::{TurnEnd, choice};
use crate::manifest::{Head, HeadKind, Manifest};
use crate::normalize::{Replacements, candidate_text, entry_text, reply_text, screen_text};
use crate::tokenizer::Tokenizer;
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

/// What a pane's screen shows: the `surface` head's classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceState {
    IdleReady,
    /// At work on the turn: thinking, writing, or running a tool.
    Working,
    WorkingPostHook,
    WaitingPermission,
    WaitingQuestion,
    WaitingTrustOrAuth,
    ErrorOrLimit,
}

impl SurfaceState {
    pub const ALL: [Self; 7] = [
        Self::IdleReady,
        Self::Working,
        Self::WorkingPostHook,
        Self::WaitingPermission,
        Self::WaitingQuestion,
        Self::WaitingTrustOrAuth,
        Self::ErrorOrLimit,
    ];

    pub fn from_index(idx: usize) -> Option<Self> {
        Self::ALL.get(idx).copied()
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::IdleReady => "idle_ready",
            Self::Working => "working",
            Self::WorkingPostHook => "working_post_hook",
            Self::WaitingPermission => "waiting_permission",
            Self::WaitingQuestion => "waiting_question",
            Self::WaitingTrustOrAuth => "waiting_trust_or_auth",
            Self::ErrorOrLimit => "error_or_limit",
        }
    }

    pub fn from_class_str(s: &str) -> Option<Self> {
        match s {
            "idle_ready" => Some(Self::IdleReady),
            "working" => Some(Self::Working),
            "working_post_hook" => Some(Self::WorkingPostHook),
            "waiting_permission" => Some(Self::WaitingPermission),
            "waiting_question" => Some(Self::WaitingQuestion),
            "waiting_trust_or_auth" => Some(Self::WaitingTrustOrAuth),
            "error_or_limit" => Some(Self::ErrorOrLimit),
            _ => None,
        }
    }
}

/// What a transcript entry is: the `conversation` head's classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationEntry {
    PersonMessage,
    AgentReply,
    #[serde(alias = "tool_step_ok")]
    ToolOk,
    #[serde(alias = "tool_step_failed")]
    ToolErr,
    InternalSkip,
}

impl ConversationEntry {
    pub const ALL: [Self; 5] =
        [Self::PersonMessage, Self::AgentReply, Self::ToolOk, Self::ToolErr, Self::InternalSkip];

    pub fn from_index(idx: usize) -> Option<Self> {
        Self::ALL.get(idx).copied()
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PersonMessage => "person_message",
            Self::AgentReply => "agent_reply",
            Self::ToolOk => "tool_step_ok",
            Self::ToolErr => "tool_step_failed",
            Self::InternalSkip => "internal_skip",
        }
    }

    pub fn from_class_str(s: &str) -> Option<Self> {
        match s {
            "person_message" => Some(Self::PersonMessage),
            "agent_reply" => Some(Self::AgentReply),
            "tool_step_ok" | "tool_ok" => Some(Self::ToolOk),
            "tool_step_failed" | "tool_err" => Some(Self::ToolErr),
            "internal_skip" => Some(Self::InternalSkip),
            _ => None,
        }
    }
}

/// The `surface` head's reading of a screen, when it is sure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceReading {
    pub state: SurfaceState,
    /// Calibrated probability in basis points (0..=10000).
    pub confidence_bp: u16,
    /// PTY sequence number at which this reading was taken.
    pub seq: u64,
}

/// The `conversation` head's reading of a transcript entry, when it is sure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationReading {
    pub kind: ConversationEntry,
    /// Calibrated probability in basis points (0..=10000).
    pub confidence_bp: u16,
}

pub struct PaneSnapshot<'a> {
    pub harness: &'a str,
    pub alt_screen: bool,
    pub rows: u16,
    pub cols: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub ms_since_last_output: u64,
    pub bottom_lines: &'a [String],
    pub seq: u64,
}

pub struct TranscriptSnippet<'a> {
    pub harness: &'a str,
    pub entry_type: &'a str,
    pub snippet: &'a str,
}

/// Sense, loaded: one graph that answers every head, sense-xs's with the
/// adapter of each head's job.
pub struct SenseModel {
    version: String,
    /// The rules its convention decides: never scored, never counted.
    convention: Vec<String>,
    table: Replacements,
    screen_last_lines: usize,
    surface: Head,
    surface_states: Vec<SurfaceState>,
    conversation: Head,
    conversation_kinds: Vec<ConversationEntry>,
    /// None when the bundle lacks a head the Ask needs.
    ask: Option<AskHeads>,
    turn_end: Option<Head>,
    plan: Option<Head>,
    tokenizer: Tokenizer,
    graph: Graph,
    /// The last Ask's text and its head logits: reading its rules next
    /// reads no head row again.
    last_heads: Mutex<Option<(String, Vec<f32>)>>,
}

impl SenseModel {
    /// The Sense bundle in `bundle_dir`, each file checked against the
    /// digest its manifest names.
    pub fn load(bundle_dir: &Path) -> anyhow::Result<Self> {
        Self::load_on(bundle_dir, Provider::Cpu)
    }

    /// The bundle in `bundle_dir`, its graph on `provider`.
    pub fn load_on(bundle_dir: &Path, provider: Provider) -> anyhow::Result<Self> {
        let manifest_path = bundle_dir.join("manifest.json");
        let manifest_bytes = fs::read(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?;
        let manifest = Manifest::parse(&manifest_bytes)?;
        let read = |key: &str| -> anyhow::Result<Vec<u8>> {
            let entry = manifest.file(key)?;
            read_verified(&bundle_dir.join(&entry.path), &entry.sha256, key)
        };
        let adapters = manifest.adapters.iter().map(|a| {
            let bytes = read_verified(&bundle_dir.join(&a.file), &a.sha256, &a.name)?;
            Ok((a.name.clone(), bytes))
        });
        let adapters = adapters.collect::<anyhow::Result<Vec<_>>>()?;
        let surface = manifest.head_of("surface", HeadKind::Choice)?.clone();
        let conversation = manifest.head_of("conversation", HeadKind::Choice)?.clone();
        Ok(Self {
            version: manifest.version.clone(),
            convention: manifest.convention_rules(),
            table: Replacements::new(&manifest.normalize.replacements)?,
            screen_last_lines: manifest.normalize.screen_last_lines,
            surface_states: head_classes(&surface, SurfaceState::from_class_str)?,
            surface,
            conversation_kinds: head_classes(&conversation, ConversationEntry::from_class_str)?,
            conversation,
            ask: manifest
                .head("ask_task")
                .is_ok()
                .then(|| AskHeads::from_manifest(&manifest))
                .transpose()?,
            turn_end: manifest.head("turn_end").ok().filter(|h| h.kind == HeadKind::Noul).cloned(),
            plan: manifest.head("plan").ok().filter(|h| h.kind == HeadKind::Document).cloned(),
            tokenizer: Tokenizer::from_bytes(&read("tokenizer")?)?,
            graph: Graph::load_on(&read("model")?, &adapters, provider)?,
            last_heads: Mutex::new(None),
        })
    }

    /// Sense's answer to an Ask: its defaults, with the route among the
    /// harness's `routes` (any, when none are given) and which of `rules`
    /// it proposes, a row for each the convention does not decide. Without
    /// rules it reads the Ask's heads alone; the same text read again with
    /// its rules reads only theirs.
    pub fn ask(&self, text: &str, routes: &[&str], rules: &[Rule]) -> anyhow::Result<AskAnswer> {
        let heads = self
            .ask
            .as_ref()
            .with_context(|| format!("Sense bundle {} has no ask_task head", self.version))?;
        let text = self.ask_text(text);
        let ids = self.tokenizer.encode(&text);
        let read = |questions: &mut dyn Iterator<Item = String>| -> anyhow::Result<Vec<f32>> {
            let rows = questions.map(|q| self.tokenizer.question_row(&ids, &q));
            let rows = rows.collect::<anyhow::Result<Vec<_>>>()?;
            self.graph.logits(&rows, heads.adapter())
        };
        let mut last = self.last_heads.lock().unwrap_or_else(|e| e.into_inner());
        let head_logits = match last.as_ref().filter(|(seen, _)| *seen == text) {
            Some((_, logits)) => logits.clone(),
            None => {
                let logits = read(&mut heads.questions().map(str::to_string))?;
                *last = Some((text.clone(), logits.clone()));
                logits
            }
        };
        drop(last);
        let asked: Vec<&Rule> = rules.iter().filter(|r| !heads.decided(&r.id)).collect();
        let logits = match asked.is_empty() {
            true => Vec::new(),
            false => read(&mut asked.iter().map(|r| heads.question(r, &self.table)))?,
        };
        let asked: Vec<(&Rule, f32)> = asked.into_iter().zip(logits).collect();
        heads.answer(&head_logits, routes, &asked)
    }

    /// How many rows the graph has read: for tests that count them.
    pub fn rows_read(&self) -> usize {
        self.graph.rows_read()
    }

    /// An Ask's words as Sense reads them: through the table.
    pub fn ask_text(&self, text: &str) -> String {
        self.table.apply(text)
    }

    /// An Ask's words as Sense reads them under what the sheet holds: the
    /// route and the plan's candidate text before them.
    pub fn chosen_text(&self, words: &str, route: Option<&str>, plan: Option<&str>) -> String {
        crate::normalize::chosen_text(words, route, plan, &self.table)
    }

    /// Whether a turn's last reply asks its person, or `None`
    /// when the bundle has no `turn_end` head or the graph fails.
    pub fn turn_end(&self, reply: &str) -> Option<TurnEnd> {
        let head = self.turn_end.as_ref()?;
        let rows =
            self.rows(&reply_text(reply, &self.table), &[head.questions.first()?.as_str()]).ok()?;
        let logits = self.graph.logits(&rows, &head.adapter).ok()?;
        Some(crate::heads::turn_end(head, *logits.first()?))
    }

    /// The plan an Ask's sheet holds by default among `candidates` (each a
    /// slug and its `<slug>: <title>: <paragraph>` text), or none.
    /// An error when the bundle has no `plan` head or the graph fails.
    pub fn plan(
        &self,
        ask: &str,
        candidates: &[(String, String)],
    ) -> anyhow::Result<Option<String>> {
        let head = self.plan.as_ref().context("this Sense bundle has no plan head")?;
        let template = head.question.as_deref().context("the plan head has no question")?;
        if candidates.is_empty() {
            return Ok(None);
        }
        let questions: Vec<String> = candidates
            .iter()
            .map(|(_, text)| {
                let text = candidate_text(text, &self.table);
                template.replace("{document}", &text)
            })
            .collect();
        let questions: Vec<&str> = questions.iter().map(String::as_str).collect();
        let rows = self.rows(&self.ask_text(ask), &questions)?;
        let logits = self.graph.logits(&rows, &head.adapter)?;
        let scored: Vec<(String, f32)> =
            candidates.iter().map(|(slug, _)| slug.clone()).zip(logits).collect();
        Ok(crate::heads::plan_default(head, &scored))
    }

    /// The rules its convention decides, which the counts leave out.
    pub fn convention(&self) -> &[String] {
        &self.convention
    }

    /// Return the loaded bundle's version string from manifest.json.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// What a settled screen shows, or `None` when the `surface` head is
    /// unsure or the graph fails.
    pub fn evaluate_surface(&self, snap: &PaneSnapshot<'_>) -> Option<SurfaceReading> {
        self.read_screen(&self.screen_text(snap), snap.seq)
    }

    /// A screen as Sense reads it: two screens with the same text read the same.
    pub fn screen_text(&self, snap: &PaneSnapshot<'_>) -> String {
        screen_text(snap, self.screen_last_lines, &self.table)
    }

    /// What a screen's text shows, as [`Self::evaluate_surface`] reads it.
    pub fn read_screen(&self, text: &str, seq: u64) -> Option<SurfaceReading> {
        let (idx, confidence_bp) = self.read(&self.surface, self.tokenizer.encode(text))?;
        let state = *self.surface_states.get(idx)?;
        Some(SurfaceReading { state, confidence_bp, seq })
    }

    /// What a transcript entry or a tool's outcome is, or `None` when the
    /// `conversation` head is unsure or the graph fails. The entry keeps
    /// its first tokens, as the head's `cut` says.
    pub fn classify_conversation_entry(
        &self,
        item: &TranscriptSnippet<'_>,
    ) -> Option<ConversationReading> {
        let mut ids = self.tokenizer.encode(&entry_text(item, &self.table));
        if let Some(cut) = self.conversation.cut {
            ids.truncate(cut.first);
        }
        let (idx, confidence_bp) = self.read(&self.conversation, ids)?;
        Some(ConversationReading { kind: *self.conversation_kinds.get(idx)?, confidence_bp })
    }

    /// A `choice` head's pick on a text's ids: its class's index and its
    /// probability in basis points.
    fn read(&self, head: &Head, ids: Vec<u32>) -> Option<(usize, u16)> {
        let rows = head.questions.iter().map(|q| self.tokenizer.question_row(&ids, q));
        let rows = rows.collect::<anyhow::Result<Vec<_>>>().ok()?;
        let logits = self.graph.logits(&rows, &head.adapter).ok()?;
        let read = choice(head, &logits, &[]);
        let idx = head.classes.iter().position(|c| Some(c) == read.pick.as_ref())?;
        Some((idx, (read.top_p * 10000.0).round().clamp(0.0, 10000.0) as u16))
    }

    /// Each row's logit, as the graph gives it with `adapter`: for tests
    /// that hold golden rows.
    pub fn logits(&self, rows: &[Vec<u32>], adapter: &str) -> anyhow::Result<Vec<f32>> {
        self.graph.logits(rows, adapter)
    }

    /// The question rows for `text` and `questions`, as Sense builds them.
    pub fn rows(&self, text: &str, questions: &[&str]) -> anyhow::Result<Vec<Vec<u32>>> {
        self.cut_rows(text, usize::MAX, questions)
    }

    /// The question rows for `text`'s first `first` tokens and `questions`.
    pub fn cut_rows(
        &self,
        text: &str,
        first: usize,
        questions: &[&str],
    ) -> anyhow::Result<Vec<Vec<u32>>> {
        let mut ids = self.tokenizer.encode(text);
        ids.truncate(first);
        questions.iter().map(|q| self.tokenizer.question_row(&ids, q)).collect()
    }
}

/// A file's SHA-256, as a manifest writes it: lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// A head's classes as this runtime's own kinds, refused when the head names
/// one it does not know.
fn head_classes<T>(head: &Head, known: impl Fn(&str) -> Option<T>) -> anyhow::Result<Vec<T>> {
    let classes = head.classes.iter();
    classes
        .map(|c| known(c).with_context(|| format!("the {} head's class {c} is unknown", head.name)))
        .collect()
}

/// A bundle file's bytes, refused unless their SHA-256 is the manifest's.
fn read_verified(path: &Path, sha256: &str, what: &str) -> anyhow::Result<Vec<u8>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let actual = sha256_hex(&bytes);
    if actual != sha256 {
        bail!("{what} SHA-256 mismatch for {}: expected {sha256}, got {actual}", path.display());
    }
    Ok(bytes)
}
