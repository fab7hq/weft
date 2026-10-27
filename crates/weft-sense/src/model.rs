use crate::ask::{AskAnswer, AskHeads, Rule};
use crate::graph::Graph;
use crate::heads::choice;
use crate::manifest::{Head, HeadKind, Manifest};
use crate::normalize::{Replacements, entry_text, screen_text};
use crate::tokenizer::Tokenizer;
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// What a pane's screen shows: the `surface` head's classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceState {
    IdleReady,
    WorkingGenerating,
    WorkingTool,
    WorkingPostHook,
    WaitingPermission,
    WaitingQuestion,
    WaitingTrustOrAuth,
    ErrorOrLimit,
}

impl SurfaceState {
    pub const ALL: [Self; 8] = [
        Self::IdleReady,
        Self::WorkingGenerating,
        Self::WorkingTool,
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
            Self::WorkingGenerating => "working_generating",
            Self::WorkingTool => "working_tool",
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
            "working_generating" => Some(Self::WorkingGenerating),
            "working_tool" => Some(Self::WorkingTool),
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
    pub hook_state: Option<&'a str>,
    /// The name of the hook event that fired last, as the harness names it.
    pub last_hook: Option<&'a str>,
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

/// sense-d, loaded: one graph that answers every head.
pub struct SenseModel {
    version: String,
    table: Replacements,
    screen_last_lines: usize,
    surface: Head,
    surface_states: Vec<SurfaceState>,
    conversation: Head,
    conversation_kinds: Vec<ConversationEntry>,
    ask: AskHeads,
    tokenizer: Tokenizer,
    graph: Graph,
}

impl SenseModel {
    /// The sense-d bundle in `bundle_dir`, each file checked against the
    /// digest its manifest names.
    pub fn load(bundle_dir: &Path) -> anyhow::Result<Self> {
        let manifest_path = bundle_dir.join("manifest.json");
        let manifest_bytes = fs::read(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?;
        let manifest = Manifest::parse(&manifest_bytes)?;
        let read = |key: &str| -> anyhow::Result<Vec<u8>> {
            let entry = manifest.file(key)?;
            read_verified(&bundle_dir.join(&entry.path), &entry.sha256, key)
        };
        let surface = manifest.head_of("surface", HeadKind::Choice)?.clone();
        let conversation = manifest.head_of("conversation", HeadKind::Choice)?.clone();
        Ok(Self {
            version: manifest.version.clone(),
            table: Replacements::new(&manifest.normalize.replacements)?,
            screen_last_lines: manifest.normalize.screen_last_lines,
            surface_states: head_classes(&surface, SurfaceState::from_class_str)?,
            surface,
            conversation_kinds: head_classes(&conversation, ConversationEntry::from_class_str)?,
            conversation,
            ask: AskHeads::from_manifest(&manifest)?,
            tokenizer: Tokenizer::from_bytes(&read("tokenizer")?)?,
            graph: Graph::load(&read("model")?)?,
        })
    }

    /// Sense's answer to an Ask: its defaults, with the route among the
    /// harness's `routes` (any, when none are given) and which of `rules`
    /// it proposes. One batch: the Ask's questions, then a row for each
    /// rule the convention does not decide.
    pub fn ask(&self, text: &str, routes: &[&str], rules: &[Rule]) -> anyhow::Result<AskAnswer> {
        let ids = self.tokenizer.encode(text);
        let asked: Vec<&Rule> = rules.iter().filter(|r| !self.ask.decided(&r.id)).collect();
        let questions = self.ask.questions().chain(asked.iter().map(|r| r.when.as_str()));
        let rows = questions.map(|q| self.tokenizer.question_row(&ids, q));
        let logits = self.graph.logits(&rows.collect::<anyhow::Result<Vec<_>>>()?)?;
        let n = logits.len() - asked.len();
        let asked: Vec<(&Rule, f32)> = asked.into_iter().zip(logits[n..].iter().copied()).collect();
        self.ask.answer(&logits[..n], routes, &asked)
    }

    /// Return the loaded bundle's version string from manifest.json.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// What a settled screen shows, or `None` when the `surface` head is
    /// unsure or the graph fails.
    pub fn evaluate_surface(&self, snap: &PaneSnapshot<'_>) -> Option<SurfaceReading> {
        let text = screen_text(snap, self.screen_last_lines, &self.table);
        let (idx, confidence_bp) = self.read(&self.surface, self.tokenizer.encode(&text))?;
        let state = *self.surface_states.get(idx)?;
        Some(SurfaceReading { state, confidence_bp, seq: snap.seq })
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
        let logits = self.graph.logits(&rows.collect::<anyhow::Result<Vec<_>>>().ok()?).ok()?;
        let read = choice(head, &logits, &[]);
        let idx = head.classes.iter().position(|c| Some(c) == read.pick.as_ref())?;
        Some((idx, (read.top_p * 10000.0).round().clamp(0.0, 10000.0) as u16))
    }

    /// Each row's logit, as the graph gives it: for tests that hold golden rows.
    pub fn logits(&self, rows: &[Vec<u32>]) -> anyhow::Result<Vec<f32>> {
        self.graph.logits(rows)
    }

    /// The question rows for `text` and `questions`, as Sense builds them.
    pub fn rows(&self, text: &str, questions: &[&str]) -> anyhow::Result<Vec<Vec<u32>>> {
        let ids = self.tokenizer.encode(text);
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
