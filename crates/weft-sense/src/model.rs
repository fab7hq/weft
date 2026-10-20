use crate::normalize::{ADDED_TOKENS, normalize_conversation_entry, normalize_surface_frame};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The 8 surface states predicted by Head 1 of the fab7-sense-v1 model.
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

/// The 5 transcript entry classes predicted by Head 2 of the fab7-sense-v1 model.
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

/// Calibrated Head 1 reading returned when confidence >= surface_min_confidence (0.85).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceReading {
    pub state: SurfaceState,
    /// Calibrated probability in basis points (0..=10000).
    pub confidence_bp: u16,
    /// PTY sequence number at which this reading was taken.
    pub seq: u64,
}

/// Calibrated Head 2 reading returned when confidence >= conversation_min_confidence (0.80).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationReading {
    pub kind: ConversationEntry,
    /// Calibrated probability in basis points (0..=10000).
    pub confidence_bp: u16,
}

pub struct PaneSnapshot<'a> {
    pub harness: &'a str,
    pub hook_state: Option<&'a str>,
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

#[derive(Debug, Deserialize)]
struct ManifestFileEntry {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct ManifestFiles {
    model: ManifestFileEntry,
    tokenizer: ManifestFileEntry,
}

#[derive(Debug, Deserialize)]
struct BundleManifest {
    schema: String,
    version: String,
    #[serde(default = "default_max_seq_len")]
    max_seq_len: usize,
    #[serde(default = "default_max_bottom_lines")]
    max_bottom_lines: usize,
    #[serde(default = "default_temp")]
    temperature: f32,
    #[serde(default = "default_temp")]
    conversation_temperature: f32,
    #[serde(default = "default_min_conf_bp")]
    min_confidence_bp: u16,
    files: ManifestFiles,
}

fn default_max_seq_len() -> usize {
    256
}
fn default_max_bottom_lines() -> usize {
    15
}
fn default_temp() -> f32 {
    1.0
}
fn default_min_conf_bp() -> u16 {
    7500
}

#[derive(Debug, Deserialize, Default)]
struct CalibrationFile {
    #[serde(alias = "T_surface")]
    temperature: Option<f32>,
    #[serde(alias = "T_conversation")]
    conversation_temperature: Option<f32>,
    surface_min_confidence: Option<f32>,
    conversation_min_confidence: Option<f32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SenseCurrentPointer {
    pub version: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
struct TokenizerJsonAddedToken {
    id: u32,
    content: String,
}

#[derive(Debug, Deserialize)]
struct TokenizerJsonModel {
    vocab: HashMap<String, u32>,
}

#[derive(Debug, Deserialize)]
struct TokenizerJson {
    added_tokens: Vec<TokenizerJsonAddedToken>,
    model: TokenizerJsonModel,
}

struct SenseTokenizer {
    vocab: HashMap<String, u32>,
    added_longest_first: Vec<(String, u32)>,
}

impl SenseTokenizer {
    fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let parsed: TokenizerJson =
            serde_json::from_slice(bytes).context("parse tokenizer.json")?;
        let mut added_lookup = HashMap::new();
        let mut added_longest_first = Vec::with_capacity(parsed.added_tokens.len());
        for tok in parsed.added_tokens {
            added_lookup.insert(tok.content.clone(), tok.id);
            added_longest_first.push((tok.content, tok.id));
        }
        added_longest_first.sort_by_key(|t| std::cmp::Reverse(t.0.len()));

        for &req in ADDED_TOKENS {
            if !added_lookup.contains_key(req) && !parsed.model.vocab.contains_key(req) {
                bail!("tokenizer.json missing required AddedToken: {req:?}");
            }
        }

        Ok(Self { vocab: parsed.model.vocab, added_longest_first })
    }

    pub fn encode(&self, text: &str, max_seq_len: usize) -> Vec<u32> {
        let mut ids = Vec::with_capacity(64);
        let mut i = 0;
        let mut buf = String::new();
        let bytes = text.as_bytes();
        while i < bytes.len() {
            let b0 = bytes[i];
            let rem = &text[i..];
            let mut matched: Option<(&str, u32)> = None;
            if matches!(b0, b'\n' | b'(' | b'[' | b'<' | 0x80..=0xFF) {
                for (pat, tid) in &self.added_longest_first {
                    if rem.starts_with(pat.as_str()) {
                        matched = Some((pat.as_str(), *tid));
                        break;
                    }
                }
            }
            if let Some((pat, tid)) = matched {
                if !buf.is_empty() {
                    self.encode_segment(&buf, &mut ids);
                    buf.clear();
                }
                ids.push(tid);
                i += pat.len();
            } else {
                let ch = rem.chars().next().unwrap();
                buf.push(ch);
                i += ch.len_utf8();
            }
        }
        if !buf.is_empty() {
            self.encode_segment(&buf, &mut ids);
        }

        let max_inner = max_seq_len.saturating_sub(2);
        if ids.len() > max_inner {
            ids.truncate(max_inner);
        }
        let mut out = Vec::with_capacity(ids.len() + 2);
        out.push(101); // [CLS]
        out.extend_from_slice(&ids);
        out.push(102); // [SEP]
        out
    }

    fn encode_segment(&self, segment: &str, out: &mut Vec<u32>) {
        // BertNormalizer (clean_text=true, handle_chinese_chars=true, lowercase=true)
        let mut cleaned = String::with_capacity(segment.len());
        for ch in segment.chars() {
            let cp = ch as u32;
            if cp == 0 || cp == 0xFFFD || is_bert_control(ch) {
                continue;
            }
            if is_bert_whitespace(ch) {
                cleaned.push(' ');
            } else if is_chinese_char(cp) {
                cleaned.push(' ');
                cleaned.push(ch);
                cleaned.push(' ');
            } else {
                for lc in ch.to_lowercase() {
                    if !is_combining_mark(lc) {
                        if let Some(stripped) = strip_latin_accent(lc) {
                            cleaned.push(stripped);
                        } else {
                            cleaned.push(lc);
                        }
                    }
                }
            }
        }

        // BertPreTokenizer: split on whitespace and isolate punctuation
        let mut cur = String::new();
        for ch in cleaned.chars() {
            if is_bert_whitespace(ch) {
                if !cur.is_empty() {
                    self.wordpiece(&cur, out);
                    cur.clear();
                }
            } else if is_bert_punctuation(ch) {
                if !cur.is_empty() {
                    self.wordpiece(&cur, out);
                    cur.clear();
                }
                let mut p = [0u8; 4];
                self.wordpiece(ch.encode_utf8(&mut p), out);
            } else {
                cur.push(ch);
            }
        }
        if !cur.is_empty() {
            self.wordpiece(&cur, out);
        }
    }

    fn wordpiece(&self, word: &str, out: &mut Vec<u32>) {
        if word.len() <= 100
            && let Some(&id) = self.vocab.get(word)
        {
            out.push(id);
            return;
        }
        if word.chars().count() > 100 {
            out.push(100);
            return;
        }
        let start_out_len = out.len();
        let mut start = 0;
        let mut pref = String::with_capacity(word.len() + 2);
        while start < word.len() {
            let mut end = word.len();
            let mut found: Option<u32> = None;
            while start < end {
                let slice = &word[start..end];
                let id_opt = if start == 0 {
                    self.vocab.get(slice).copied()
                } else {
                    pref.clear();
                    pref.push_str("##");
                    pref.push_str(slice);
                    self.vocab.get(&pref).copied()
                };
                if let Some(id) = id_opt {
                    found = Some(id);
                    break;
                }
                end -= 1;
                while end > start && !word.is_char_boundary(end) {
                    end -= 1;
                }
            }
            if let Some(id) = found {
                out.push(id);
                start = end;
            } else {
                out.truncate(start_out_len);
                out.push(100);
                return;
            }
        }
    }
}

fn is_bert_whitespace(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\t' | '\n' | '\r' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

fn is_bert_control(ch: char) -> bool {
    if matches!(ch, '\t' | '\n' | '\r') {
        return false;
    }
    let cp = ch as u32;
    matches!(cp, 0x00..=0x1F | 0x7F..=0x9F | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0xFEFF)
}

fn is_chinese_char(cp: u32) -> bool {
    matches!(
        cp,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0xF900..=0xFAFF
            | 0x2F800..=0x2FA1F
    )
}

fn is_combining_mark(ch: char) -> bool {
    let cp = ch as u32;
    matches!(cp, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn strip_latin_accent(ch: char) -> Option<char> {
    match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => Some('a'),
        'ç' => Some('c'),
        'è' | 'é' | 'ê' | 'ë' => Some('e'),
        'ì' | 'í' | 'î' | 'ï' => Some('i'),
        'ñ' => Some('n'),
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => Some('o'),
        'ù' | 'ú' | 'û' | 'ü' => Some('u'),
        'ý' | 'ÿ' => Some('y'),
        _ => None,
    }
}

fn is_bert_punctuation(ch: char) -> bool {
    let cp = ch as u32;
    if matches!(cp, 33..=47 | 58..=64 | 91..=96 | 123..=126) {
        return true;
    }
    matches!(
        cp,
        0x00A1..=0x00BF
            | 0x2010..=0x2027
            | 0x2030..=0x205E
            | 0x2190..=0x2BFF
            | 0x3001..=0x303F
            | 0xFE30..=0xFE6F
            | 0xFF01..=0xFF0F
            | 0xFF1A..=0xFF20
            | 0xFF3B..=0xFF40
            | 0xFF5B..=0xFF65
    ) && !matches!(ch, '←' | '↑' | '→' | '↓' | '⏸' | '▀' | '▄' | '█' | '×' | '÷' | '≤' | '≥' | '≠')
}

#[cfg(target_os = "macos")]
#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {
    fn cblas_sgemm(
        order: i32,
        trans_a: i32,
        trans_b: i32,
        m: i32,
        n: i32,
        k: i32,
        alpha: f32,
        a: *const f32,
        lda: i32,
        b: *const f32,
        ldb: i32,
        beta: f32,
        c: *mut f32,
        ldc: i32,
    );
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
    fn dlsym(
        handle: *mut std::ffi::c_void,
        symbol: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
}

/// Keep Accelerate's BLAS on the calling thread, which avoids GCD's dispatch
/// overhead. `BLASSetThreading` is macOS 15's, so it is looked up when the
/// program runs: a Weft built or run on an older macOS goes on without it.
#[cfg(target_os = "macos")]
fn blas_single_threaded() {
    static SET: std::sync::OnceLock<Option<unsafe extern "C" fn(i32) -> i32>> =
        std::sync::OnceLock::new();
    let set = SET.get_or_init(|| {
        // RTLD_DEFAULT: every image loaded, Accelerate among them.
        let found =
            unsafe { dlsym(-2isize as *mut std::ffi::c_void, c"BLASSetThreading".as_ptr()) };
        (!found.is_null()).then(|| unsafe {
            std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn(i32) -> i32>(found)
        })
    });
    if let Some(set) = set {
        unsafe {
            set(1);
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn ld1q_f32(ptr: *const f32) -> std::arch::aarch64::float32x4_t {
    unsafe { ptr.cast::<std::arch::aarch64::float32x4_t>().read_unaligned() }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn st1q_f32(ptr: *mut f32, v: std::arch::aarch64::float32x4_t) {
    unsafe {
        ptr.cast::<std::arch::aarch64::float32x4_t>().write_unaligned(v);
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[allow(unsafe_op_in_unsafe_fn)]
#[inline]
unsafe fn minmax_f32_neon(x: &[f32]) -> (f32, f32) {
    use std::arch::aarch64::{vdupq_n_f32, vmaxnmq_f32, vmaxnmvq_f32, vminnmq_f32, vminnmvq_f32};
    let mut vmin0 = vdupq_n_f32(0.0);
    let mut vmin1 = vdupq_n_f32(0.0);
    let mut vmin2 = vdupq_n_f32(0.0);
    let mut vmin3 = vdupq_n_f32(0.0);
    let mut vmax0 = vdupq_n_f32(0.0);
    let mut vmax1 = vdupq_n_f32(0.0);
    let mut vmax2 = vdupq_n_f32(0.0);
    let mut vmax3 = vdupq_n_f32(0.0);
    let ptr = x.as_ptr();
    let n = x.len();
    let mut i = 0;
    while i + 16 <= n {
        let v0 = ld1q_f32(ptr.add(i));
        let v1 = ld1q_f32(ptr.add(i + 4));
        let v2 = ld1q_f32(ptr.add(i + 8));
        let v3 = ld1q_f32(ptr.add(i + 12));
        vmin0 = vminnmq_f32(vmin0, v0);
        vmin1 = vminnmq_f32(vmin1, v1);
        vmin2 = vminnmq_f32(vmin2, v2);
        vmin3 = vminnmq_f32(vmin3, v3);
        vmax0 = vmaxnmq_f32(vmax0, v0);
        vmax1 = vmaxnmq_f32(vmax1, v1);
        vmax2 = vmaxnmq_f32(vmax2, v2);
        vmax3 = vmaxnmq_f32(vmax3, v3);
        i += 16;
    }
    vmin0 = vminnmq_f32(vminnmq_f32(vmin0, vmin1), vminnmq_f32(vmin2, vmin3));
    vmax0 = vmaxnmq_f32(vmaxnmq_f32(vmax0, vmax1), vmaxnmq_f32(vmax2, vmax3));
    let mut xmin = vminnmvq_f32(vmin0);
    let mut xmax = vmaxnmvq_f32(vmax0);
    while i < n {
        let v = *ptr.add(i);
        xmin = xmin.min(v);
        xmax = xmax.max(v);
        i += 1;
    }
    (xmin, xmax)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[allow(unsafe_op_in_unsafe_fn)]
#[inline]
unsafe fn quantize_slice_neon(x: &[f32], inv_scale: f32, lo: f32, hi: f32, dst_slice: &mut [f32]) {
    use std::arch::aarch64::{vdupq_n_f32, vmaxnmq_f32, vminnmq_f32, vmulq_f32, vrndnq_f32};
    let v_inv = vdupq_n_f32(inv_scale);
    let v_lo = vdupq_n_f32(lo);
    let v_hi = vdupq_n_f32(hi);
    let src = x.as_ptr();
    let dst = dst_slice.as_mut_ptr();
    let n = x.len();
    let mut i = 0;
    while i + 16 <= n {
        let a = vrndnq_f32(vmulq_f32(ld1q_f32(src.add(i)), v_inv));
        let b = vrndnq_f32(vmulq_f32(ld1q_f32(src.add(i + 4)), v_inv));
        let c = vrndnq_f32(vmulq_f32(ld1q_f32(src.add(i + 8)), v_inv));
        let d = vrndnq_f32(vmulq_f32(ld1q_f32(src.add(i + 12)), v_inv));
        st1q_f32(dst.add(i), vminnmq_f32(vmaxnmq_f32(a, v_lo), v_hi));
        st1q_f32(dst.add(i + 4), vminnmq_f32(vmaxnmq_f32(b, v_lo), v_hi));
        st1q_f32(dst.add(i + 8), vminnmq_f32(vmaxnmq_f32(c, v_lo), v_hi));
        st1q_f32(dst.add(i + 12), vminnmq_f32(vmaxnmq_f32(d, v_lo), v_hi));
        i += 16;
    }
    while i < n {
        *dst.add(i) = (*src.add(i) * inv_scale).round_ties_even().clamp(lo, hi);
        i += 1;
    }
}

#[inline(always)]
fn minmax_slice(x: &[f32]) -> (f32, f32) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        minmax_f32_neon(x)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut xmin = 0.0_f32;
        let mut xmax = 0.0_f32;
        for &v in x {
            xmin = xmin.min(v);
            xmax = xmax.max(v);
        }
        (xmin, xmax)
    }
}

#[inline(always)]
fn quant_params(x_min: f32, x_max: f32) -> (f32, f32, f32, f32) {
    let x_scale = if x_max != x_min { (x_max - x_min) / 255.0_f32 } else { 1.0_f32 };
    let inv_scale = 1.0_f32 / x_scale;
    let zp_f32 = if x_scale != 0.0 {
        (-x_min * inv_scale).round_ties_even().clamp(0.0_f32, 255.0_f32)
    } else {
        0.0_f32
    };
    (x_scale, inv_scale, -zp_f32, 255.0_f32 - zp_f32)
}

#[inline(always)]
fn quantize_into(x: &[f32], inv_scale: f32, lo: f32, hi: f32, dst: &mut [f32]) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        quantize_slice_neon(x, inv_scale, lo, hi, dst);
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        for (d, &v) in dst.iter_mut().zip(x.iter()) {
            *d = (v * inv_scale).round_ties_even().clamp(lo, hi);
        }
    }
}

struct QuantizedActivation {
    data: Vec<f32>,
    scale: f32,
    rows: usize,
}

impl QuantizedActivation {
    fn from_f32(x: &[f32], rows: usize, _cols: usize, buf: &mut Vec<f32>) -> Self {
        let (x_min, x_max) = minmax_slice(x);
        Self::from_f32_with_minmax(x, rows, _cols, x_min, x_max, buf)
    }

    fn from_f32_with_minmax(
        x: &[f32],
        rows: usize,
        _cols: usize,
        x_min: f32,
        x_max: f32,
        buf: &mut Vec<f32>,
    ) -> Self {
        let (x_scale, inv_scale, lo, hi) = quant_params(x_min, x_max);
        buf.resize(x.len(), 0.0);
        quantize_into(x, inv_scale, lo, hi, buf);

        Self { data: std::mem::take(buf), scale: x_scale, rows }
    }

    fn recycle(mut self, buf: &mut Vec<f32>) {
        self.data.clear();
        *buf = self.data;
    }
}

/// Rows the product without Accelerate takes at once.
#[cfg(not(target_os = "macos"))]
const BLOCK: usize = 8;

/// Room for a block of rows' sums, kept between blocks.
#[cfg(not(target_os = "macos"))]
#[derive(Default)]
struct Sums {
    int: Vec<i32>,
    wide: Vec<f64>,
}

#[derive(Clone)]
struct QuantLinear {
    in_dim: usize,
    out_dim: usize,
    /// `(w_q - w_zp)` stored row-major as `[in_dim, out_dim]` in `f32`.
    w_f32: Vec<f32>,
    /// The same as integers, where they are (not the fused, pre-scaled
    /// attention weights), for the product without Accelerate.
    #[cfg(not(target_os = "macos"))]
    w_int: Vec<i16>,
    w_scale: f32,
    bias: Vec<f32>,
}

impl QuantLinear {
    #[inline(always)]
    fn forward_slice_no_bias(&self, xq_data: &[f32], rows: usize, x_scale: f32, out: &mut [f32]) {
        if rows == 0 {
            return;
        }
        let m = rows;
        let k = self.in_dim;
        let n = self.out_dim;
        let combined_scale = x_scale * self.w_scale;

        #[cfg(target_os = "macos")]
        unsafe {
            cblas_sgemm(
                101,
                111,
                111,
                m as i32,
                n as i32,
                k as i32,
                combined_scale,
                xq_data.as_ptr(),
                k as i32,
                self.w_f32.as_ptr(),
                n as i32,
                0.0_f32,
                out.as_mut_ptr(),
                n as i32,
            );
        }

        #[cfg(not(target_os = "macos"))]
        {
            let mut sums = Sums::default();
            for (x_rows, out_rows) in
                xq_data.chunks(k * BLOCK).zip(out[..m * n].chunks_mut(n * BLOCK))
            {
                self.unscaled_rows(x_rows, &mut sums, out_rows);
                for dst in out_rows.iter_mut() {
                    *dst *= combined_scale;
                }
            }
        }
    }

    #[inline(always)]
    fn forward_slice_with_bias(&self, xq_data: &[f32], rows: usize, x_scale: f32, out: &mut [f32]) {
        self.forward_slice_no_bias(xq_data, rows, x_scale, out);
        let n = self.out_dim;
        for row_dst in out.chunks_exact_mut(n) {
            for (dst, &b) in row_dst.iter_mut().zip(self.bias.iter()) {
                *dst += b;
            }
        }
    }

    #[inline(always)]
    fn forward_slice_accum_no_bias(
        &self,
        xq_data: &[f32],
        rows: usize,
        x_scale: f32,
        accum: &mut [f32],
    ) {
        if rows == 0 {
            return;
        }
        let m = rows;
        let k = self.in_dim;
        let n = self.out_dim;
        let combined_scale = x_scale * self.w_scale;

        #[cfg(target_os = "macos")]
        unsafe {
            cblas_sgemm(
                101,
                111,
                111,
                m as i32,
                n as i32,
                k as i32,
                combined_scale,
                xq_data.as_ptr(),
                k as i32,
                self.w_f32.as_ptr(),
                n as i32,
                1.0_f32,
                accum.as_mut_ptr(),
                n as i32,
            );
        }

        #[cfg(not(target_os = "macos"))]
        {
            let mut sums = Sums::default();
            let mut rows = vec![0.0_f32; n * BLOCK];
            for (x_rows, out_rows) in
                xq_data.chunks(k * BLOCK).zip(accum[..m * n].chunks_mut(n * BLOCK))
            {
                let rows = &mut rows[..out_rows.len()];
                self.unscaled_rows(x_rows, &mut sums, rows);
                for (dst, &s) in out_rows.iter_mut().zip(rows.iter()) {
                    *dst += combined_scale * s;
                }
            }
        }
    }

    /// Rows of the product, summed exactly and scaled by nothing, as the
    /// reference's integer matrix product is: scaling each term first, or
    /// summing in `f32`, rounds differently and the difference grows through
    /// the layers. Integers where the weights are, else `f64`. Up to
    /// [`BLOCK`] rows at once, so each weight row is read once for them all.
    #[cfg(not(target_os = "macos"))]
    fn unscaled_rows(&self, x_rows: &[f32], sums: &mut Sums, out: &mut [f32]) {
        let (k, n) = (self.in_dim, self.out_dim);
        let rows = x_rows.len() / k;
        if self.w_int.is_empty() {
            sums.wide.clear();
            sums.wide.resize(rows * n, 0.0);
            for (i, w_row) in self.w_f32.chunks_exact(n).enumerate() {
                for (r, acc) in sums.wide.chunks_exact_mut(n).enumerate() {
                    let xv = x_rows[r * k + i];
                    if xv != 0.0 {
                        let xv = f64::from(xv);
                        for (a, &wv) in acc.iter_mut().zip(w_row) {
                            *a += xv * f64::from(wv);
                        }
                    }
                }
            }
            for (o, &a) in out.iter_mut().zip(&sums.wide) {
                *o = a as f32;
            }
        } else {
            sums.int.clear();
            sums.int.resize(rows * n, 0);
            for (i, w_row) in self.w_int.chunks_exact(n).enumerate() {
                for (r, acc) in sums.int.chunks_exact_mut(n).enumerate() {
                    // An activation is an integer of at most 255 either way.
                    let xv = x_rows[r * k + i] as i32;
                    if xv != 0 {
                        for (a, &wv) in acc.iter_mut().zip(w_row) {
                            *a += xv * i32::from(wv);
                        }
                    }
                }
            }
            for (o, &a) in out.iter_mut().zip(&sums.int) {
                *o = a as f32;
            }
        }
    }

    fn forward_quant_no_bias(&self, xq: &QuantizedActivation, out: &mut [f32]) {
        self.forward_slice_no_bias(&xq.data, xq.rows, xq.scale, out);
    }

    fn forward_quant(&self, xq: &QuantizedActivation, out: &mut [f32]) {
        self.forward_slice_with_bias(&xq.data, xq.rows, xq.scale, out);
    }

    fn forward_quant_accum_no_bias(&self, xq: &QuantizedActivation, accum: &mut [f32]) {
        self.forward_slice_accum_no_bias(&xq.data, xq.rows, xq.scale, accum);
    }

    fn forward(&self, x: &[f32], rows: usize, out: &mut [f32], scratch: &mut Vec<f32>) {
        let xq = QuantizedActivation::from_f32(x, rows, self.in_dim, scratch);
        self.forward_quant(&xq, out);
        xq.recycle(scratch);
    }
}

#[derive(Clone)]
struct LayerNorm {
    weight: Vec<f32>,
    bias: Vec<f32>,
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[allow(unsafe_op_in_unsafe_fn)]
#[inline]
unsafe fn layernorm_row_neon(row: &mut [f32], weight: &[f32], bias: &[f32], inv_dim: f32) {
    use std::arch::aarch64::{vaddq_f32, vaddvq_f32, vdupq_n_f32, vfmaq_f32, vmulq_f32, vsubq_f32};
    let dim = row.len();
    let ptr = row.as_mut_ptr();
    let w_ptr = weight.as_ptr();
    let b_ptr = bias.as_ptr();

    let mut vs0 = vdupq_n_f32(0.0);
    let mut vs1 = vdupq_n_f32(0.0);
    let mut i = 0;
    while i + 8 <= dim {
        vs0 = vaddq_f32(vs0, ld1q_f32(ptr.add(i)));
        vs1 = vaddq_f32(vs1, ld1q_f32(ptr.add(i + 4)));
        i += 8;
    }
    let mean = vaddvq_f32(vaddq_f32(vs0, vs1)) * inv_dim;
    let v_mean = vdupq_n_f32(mean);

    let mut vv0 = vdupq_n_f32(0.0);
    let mut vv1 = vdupq_n_f32(0.0);
    i = 0;
    while i + 8 <= dim {
        let d0 = vsubq_f32(ld1q_f32(ptr.add(i)), v_mean);
        let d1 = vsubq_f32(ld1q_f32(ptr.add(i + 4)), v_mean);
        vv0 = vfmaq_f32(vv0, d0, d0);
        vv1 = vfmaq_f32(vv1, d1, d1);
        i += 8;
    }
    let inv_std = 1.0_f32 / (vaddvq_f32(vaddq_f32(vv0, vv1)) * inv_dim + 1e-12_f32).sqrt();
    let v_inv_std = vdupq_n_f32(inv_std);

    i = 0;
    while i + 8 <= dim {
        let x0 = vmulq_f32(vsubq_f32(ld1q_f32(ptr.add(i)), v_mean), v_inv_std);
        let x1 = vmulq_f32(vsubq_f32(ld1q_f32(ptr.add(i + 4)), v_mean), v_inv_std);
        let out0 = vfmaq_f32(ld1q_f32(b_ptr.add(i)), x0, ld1q_f32(w_ptr.add(i)));
        let out1 = vfmaq_f32(ld1q_f32(b_ptr.add(i + 4)), x1, ld1q_f32(w_ptr.add(i + 4)));
        st1q_f32(ptr.add(i), out0);
        st1q_f32(ptr.add(i + 4), out1);
        i += 8;
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[allow(unsafe_op_in_unsafe_fn)]
#[inline]
unsafe fn layernorm_add_bias_row_neon(
    row: &mut [f32],
    add_bias: &[f32],
    weight: &[f32],
    bias: &[f32],
    inv_dim: f32,
) {
    use std::arch::aarch64::{vaddq_f32, vaddvq_f32, vdupq_n_f32, vfmaq_f32, vmulq_f32, vsubq_f32};
    let dim = row.len();
    let ptr = row.as_mut_ptr();
    let ab_ptr = add_bias.as_ptr();
    let w_ptr = weight.as_ptr();
    let b_ptr = bias.as_ptr();

    let mut vs0 = vdupq_n_f32(0.0);
    let mut vs1 = vdupq_n_f32(0.0);
    let mut i = 0;
    while i + 8 <= dim {
        let v0 = vaddq_f32(ld1q_f32(ptr.add(i)), ld1q_f32(ab_ptr.add(i)));
        let v1 = vaddq_f32(ld1q_f32(ptr.add(i + 4)), ld1q_f32(ab_ptr.add(i + 4)));
        st1q_f32(ptr.add(i), v0);
        st1q_f32(ptr.add(i + 4), v1);
        vs0 = vaddq_f32(vs0, v0);
        vs1 = vaddq_f32(vs1, v1);
        i += 8;
    }
    let mean = vaddvq_f32(vaddq_f32(vs0, vs1)) * inv_dim;
    let v_mean = vdupq_n_f32(mean);

    let mut vv0 = vdupq_n_f32(0.0);
    let mut vv1 = vdupq_n_f32(0.0);
    i = 0;
    while i + 8 <= dim {
        let d0 = vsubq_f32(ld1q_f32(ptr.add(i)), v_mean);
        let d1 = vsubq_f32(ld1q_f32(ptr.add(i + 4)), v_mean);
        vv0 = vfmaq_f32(vv0, d0, d0);
        vv1 = vfmaq_f32(vv1, d1, d1);
        i += 8;
    }
    let inv_std = 1.0_f32 / (vaddvq_f32(vaddq_f32(vv0, vv1)) * inv_dim + 1e-12_f32).sqrt();
    let v_inv_std = vdupq_n_f32(inv_std);

    i = 0;
    while i + 8 <= dim {
        let x0 = vmulq_f32(vsubq_f32(ld1q_f32(ptr.add(i)), v_mean), v_inv_std);
        let x1 = vmulq_f32(vsubq_f32(ld1q_f32(ptr.add(i + 4)), v_mean), v_inv_std);
        let out0 = vfmaq_f32(ld1q_f32(b_ptr.add(i)), x0, ld1q_f32(w_ptr.add(i)));
        let out1 = vfmaq_f32(ld1q_f32(b_ptr.add(i + 4)), x1, ld1q_f32(w_ptr.add(i + 4)));
        st1q_f32(ptr.add(i), out0);
        st1q_f32(ptr.add(i + 4), out1);
        i += 8;
    }
}

impl LayerNorm {
    fn forward_in_place(&self, x: &mut [f32], dim: usize) {
        let inv_dim = 1.0_f32 / (dim as f32);
        #[cfg(target_arch = "aarch64")]
        {
            for row in x.chunks_exact_mut(dim) {
                unsafe {
                    layernorm_row_neon(row, &self.weight, &self.bias, inv_dim);
                }
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let eps = 1e-12_f32;
            for row in x.chunks_exact_mut(dim) {
                let mut sum = 0.0_f32;
                for &v in row.iter() {
                    sum += v;
                }
                let mean = sum * inv_dim;
                let mut var = 0.0_f32;
                for &v in row.iter() {
                    let d = v - mean;
                    var += d * d;
                }
                let inv_std = 1.0_f32 / (var * inv_dim + eps).sqrt();
                for ((v, &w), &b) in row.iter_mut().zip(self.weight.iter()).zip(self.bias.iter()) {
                    *v = (*v - mean) * inv_std * w + b;
                }
            }
        }
    }

    /// Add `add_bias` to each row of `x` and normalize in-place in a single fused pass.
    fn add_bias_and_forward_in_place(&self, x: &mut [f32], add_bias: &[f32], dim: usize) {
        let inv_dim = 1.0_f32 / (dim as f32);
        #[cfg(target_arch = "aarch64")]
        {
            for row in x.chunks_exact_mut(dim) {
                unsafe {
                    layernorm_add_bias_row_neon(row, add_bias, &self.weight, &self.bias, inv_dim);
                }
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let eps = 1e-12_f32;
            for row in x.chunks_exact_mut(dim) {
                let mut sum = 0.0_f32;
                for (v, &ab) in row.iter_mut().zip(add_bias.iter()) {
                    *v += ab;
                    sum += *v;
                }
                let mean = sum * inv_dim;
                let mut var = 0.0_f32;
                for &v in row.iter() {
                    let d = v - mean;
                    var += d * d;
                }
                let inv_std = 1.0_f32 / (var * inv_dim + eps).sqrt();
                for ((v, &w), &b) in row.iter_mut().zip(self.weight.iter()).zip(self.bias.iter()) {
                    *v = (*v - mean) * inv_std * w + b;
                }
            }
        }
    }
}

#[derive(Clone)]
struct EncoderLayer {
    /// Fused `[Q_scaled; K_scaled; V]` linear layer (`out_dim = 1152`, `in_dim = 384`, `w_scale = 1.0`).
    qkv: QuantLinear,
    attn_out: QuantLinear,
    attn_ln: LayerNorm,
    ffn_inter: QuantLinear,
    ffn_out: QuantLinear,
    ffn_ln: LayerNorm,
}

#[derive(Clone)]
struct OnnxDualHeadWeights {
    word_embeddings: Vec<f32>,
    position_embeddings: Vec<f32>,
    token_type_0: Vec<f32>,
    emb_ln: LayerNorm,
    layers: [EncoderLayer; 3],
    pool_linear: QuantLinear,
    pool_ln: LayerNorm,
    surface_fc0: QuantLinear,
    surface_fc2: QuantLinear,
    conv_fc0: QuantLinear,
    conv_fc2: QuantLinear,
}

pub struct SenseModel {
    version: String,
    max_seq_len: usize,
    max_bottom_lines: usize,
    temperature: f32,
    conversation_temperature: f32,
    surface_min_confidence_bp: u16,
    conversation_min_confidence_bp: u16,
    tokenizer: SenseTokenizer,
    weights: OnnxDualHeadWeights,
}

#[derive(Default)]
struct InferenceScratch {
    x: Vec<f32>,
    xq_buf: Vec<f32>,
    qkv_buf: Vec<f32>,
    ctx: Vec<f32>,
    ffn_mid: Vec<f32>,
    fq_buf: Vec<f32>,
    scores: [Vec<f32>; 4],
    quant_buf: Vec<f32>,
}

thread_local! {
    static INFER_SCRATCH: std::cell::RefCell<InferenceScratch> =
        std::cell::RefCell::new(InferenceScratch::default());
}

struct SpinBarrier {
    step: std::sync::atomic::AtomicUsize,
    count: std::sync::atomic::AtomicUsize,
}

impl SpinBarrier {
    const fn new() -> Self {
        Self {
            step: std::sync::atomic::AtomicUsize::new(0),
            count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    fn wait(&self) {
        use std::sync::atomic::Ordering;
        let s = self.step.load(Ordering::Acquire);
        let c = self.count.fetch_add(1, Ordering::AcqRel);
        if c + 1 == 4 {
            self.count.store(0, Ordering::Relaxed);
            self.step.fetch_add(1, Ordering::Release);
        } else {
            while self.step.load(Ordering::Acquire) == s {
                std::hint::spin_loop();
            }
        }
    }
}

struct ParallelLayerJob {
    weights: *const OnnxDualHeadWeights,
    seq_len: usize,
    x: *mut f32,
    xq_buf: *mut f32,
    qkv_buf: *mut f32,
    ctx: *mut f32,
    ffn_mid: *mut f32,
    fq_buf: *mut f32,
    scores: [*mut f32; 4],
    thread_min: [[std::cell::UnsafeCell<f32>; 4]; 2],
    thread_max: [[std::cell::UnsafeCell<f32>; 4]; 2],
    barrier: SpinBarrier,
}

unsafe impl Sync for ParallelLayerJob {}
unsafe impl Send for ParallelLayerJob {}

impl ParallelLayerJob {
    #[inline(always)]
    fn write_minmax(&self, slot: usize, tid: usize, mn: f32, mx: f32) {
        unsafe {
            *self.thread_min[slot & 1][tid].get() = mn;
            *self.thread_max[slot & 1][tid].get() = mx;
        }
    }

    #[inline(always)]
    fn reduce_minmax(&self, slot: usize) -> (f32, f32) {
        let s = slot & 1;
        let mut mn = 0.0_f32;
        let mut mx = 0.0_f32;
        for t in 0..4 {
            let tmn = unsafe { *self.thread_min[s][t].get() };
            let tmx = unsafe { *self.thread_max[s][t].get() };
            mn = mn.min(tmn);
            mx = mx.max(tmx);
        }
        (mn, mx)
    }
}

unsafe fn run_layers_worker(tid: usize, job: &ParallelLayerJob) {
    const D: usize = 384;
    const D3: usize = 1152;
    const HEAD_DIM: usize = 32;
    const FFN_DIM: usize = 1536;

    let seq_len = job.seq_len;
    let w = unsafe { &*job.weights };
    let r_start = (seq_len * tid) / 4;
    let r_end = (seq_len * (tid + 1)) / 4;
    let r_cnt = r_end - r_start;

    let x_rows = unsafe { std::slice::from_raw_parts_mut(job.x.add(r_start * D), r_cnt * D) };
    let xq_rows = unsafe { std::slice::from_raw_parts_mut(job.xq_buf.add(r_start * D), r_cnt * D) };
    let qkv_rows =
        unsafe { std::slice::from_raw_parts_mut(job.qkv_buf.add(r_start * D3), r_cnt * D3) };
    let ctx_rows = unsafe { std::slice::from_raw_parts_mut(job.ctx.add(r_start * D), r_cnt * D) };
    let ffn_rows = unsafe {
        std::slice::from_raw_parts_mut(job.ffn_mid.add(r_start * FFN_DIM), r_cnt * FFN_DIM)
    };
    let fq_rows = unsafe {
        std::slice::from_raw_parts_mut(job.fq_buf.add(r_start * FFN_DIM), r_cnt * FFN_DIM)
    };
    let scores_buf = unsafe { std::slice::from_raw_parts_mut(job.scores[tid], seq_len * seq_len) };

    let mut slot = 0usize;
    let (mn0, mx0) = minmax_slice(x_rows);
    job.write_minmax(slot, tid, mn0, mx0);
    job.barrier.wait();

    for (layer_idx, layer) in w.layers.iter().enumerate() {
        // Phase 1: quantize x_rows -> qkv GEMM on r_start..r_end
        let (x_min, x_max) = job.reduce_minmax(slot);
        let (x_scale, x_inv, x_lo, x_hi) = quant_params(x_min, x_max);
        quantize_into(x_rows, x_inv, x_lo, x_hi, xq_rows);
        layer.qkv.forward_slice_with_bias(xq_rows, r_cnt, x_scale, qkv_rows);
        job.barrier.wait();

        // Phase 2: 3 attention heads per thread
        for h in (tid * 3)..((tid + 1) * 3) {
            let h_col = h * HEAD_DIM;
            #[cfg(target_os = "macos")]
            unsafe {
                cblas_sgemm(
                    101,
                    111,
                    112,
                    seq_len as i32,
                    seq_len as i32,
                    HEAD_DIM as i32,
                    1.0_f32,
                    job.qkv_buf.add(h_col),
                    D3 as i32,
                    job.qkv_buf.add(D + h_col),
                    D3 as i32,
                    0.0_f32,
                    scores_buf.as_mut_ptr(),
                    seq_len as i32,
                );
            }
            #[cfg(not(target_os = "macos"))]
            {
                let qkv_all = unsafe { std::slice::from_raw_parts(job.qkv_buf, seq_len * D3) };
                for i in 0..seq_len {
                    let q_r = &qkv_all[i * D3 + h_col..i * D3 + h_col + HEAD_DIM];
                    let s_row = &mut scores_buf[i * seq_len..(i + 1) * seq_len];
                    for (j, s_val) in s_row.iter_mut().enumerate() {
                        let k_r = &qkv_all[j * D3 + D + h_col..j * D3 + D + h_col + HEAD_DIM];
                        // Eight sums side by side: one would be a chain of
                        // dependent adds, which the compiler may not reorder.
                        let mut sums = [0.0_f32; 8];
                        for (qc, kc) in q_r.as_chunks::<8>().0.iter().zip(k_r.as_chunks::<8>().0) {
                            for t in 0..8 {
                                sums[t] += qc[t] * kc[t];
                            }
                        }
                        *s_val = sums.iter().sum();
                    }
                }
            }
            for score_row in scores_buf.chunks_exact_mut(seq_len) {
                softmax_row_in_place(score_row);
            }
            #[cfg(target_os = "macos")]
            unsafe {
                cblas_sgemm(
                    101,
                    111,
                    111,
                    seq_len as i32,
                    HEAD_DIM as i32,
                    seq_len as i32,
                    1.0_f32,
                    scores_buf.as_ptr(),
                    seq_len as i32,
                    job.qkv_buf.add(2 * D + h_col),
                    D3 as i32,
                    0.0_f32,
                    job.ctx.add(h_col),
                    D as i32,
                );
            }
            #[cfg(not(target_os = "macos"))]
            {
                let qkv_all = unsafe { std::slice::from_raw_parts(job.qkv_buf, seq_len * D3) };
                let ctx_all = unsafe { std::slice::from_raw_parts_mut(job.ctx, seq_len * D) };
                for i in 0..seq_len {
                    let s_row = &scores_buf[i * seq_len..(i + 1) * seq_len];
                    let c_row = &mut ctx_all[i * D + h_col..i * D + h_col + HEAD_DIM];
                    c_row.fill(0.0);
                    for (j, &prob) in s_row.iter().enumerate() {
                        let v_r =
                            &qkv_all[j * D3 + 2 * D + h_col..j * D3 + 2 * D + h_col + HEAD_DIM];
                        for d in 0..HEAD_DIM {
                            c_row[d] += prob * v_r[d];
                        }
                    }
                }
            }
        }
        job.barrier.wait();

        slot += 1;
        let (cmn, cmx) = minmax_slice(ctx_rows);
        job.write_minmax(slot, tid, cmn, cmx);
        job.barrier.wait();

        // Phase 3: quantize ctx_rows -> attn_out -> attn_ln -> minmax(x_rows)
        let (c_min, c_max) = job.reduce_minmax(slot);
        let (c_scale, c_inv, c_lo, c_hi) = quant_params(c_min, c_max);
        quantize_into(ctx_rows, c_inv, c_lo, c_hi, xq_rows);
        layer.attn_out.forward_slice_accum_no_bias(xq_rows, r_cnt, c_scale, x_rows);
        layer.attn_ln.add_bias_and_forward_in_place(x_rows, &layer.attn_out.bias, D);

        slot += 1;
        let (xmn2, xmx2) = minmax_slice(x_rows);
        job.write_minmax(slot, tid, xmn2, xmx2);
        job.barrier.wait();

        // Phase 4: quantize x_rows -> ffn_inter -> gelu_bias_minmax
        let (x2_min, x2_max) = job.reduce_minmax(slot);
        let (x2_scale, x2_inv, x2_lo, x2_hi) = quant_params(x2_min, x2_max);
        quantize_into(x_rows, x2_inv, x2_lo, x2_hi, xq_rows);
        layer.ffn_inter.forward_slice_no_bias(xq_rows, r_cnt, x2_scale, ffn_rows);
        let (fmn, fmx) = gelu_bias_minmax_in_place(ffn_rows, &layer.ffn_inter.bias);

        slot += 1;
        job.write_minmax(slot, tid, fmn, fmx);
        job.barrier.wait();

        // Phase 5: quantize ffn_rows -> ffn_out -> ffn_ln -> next layer minmax(x_rows)
        let (f_min, f_max) = job.reduce_minmax(slot);
        let (f_scale, f_inv, f_lo, f_hi) = quant_params(f_min, f_max);
        quantize_into(ffn_rows, f_inv, f_lo, f_hi, fq_rows);
        layer.ffn_out.forward_slice_accum_no_bias(fq_rows, r_cnt, f_scale, x_rows);
        layer.ffn_ln.add_bias_and_forward_in_place(x_rows, &layer.ffn_out.bias, D);

        if layer_idx + 1 < w.layers.len() {
            slot += 1;
            let (nmn, nmx) = minmax_slice(x_rows);
            job.write_minmax(slot, tid, nmn, nmx);
        }
        job.barrier.wait();
    }
}

struct WorkerPoolState {
    job_ptr: std::sync::atomic::AtomicPtr<ParallelLayerJob>,
    job_gen: std::sync::atomic::AtomicUsize,
    done_count: std::sync::atomic::AtomicUsize,
}

struct WorkerPool {
    run_lock: std::sync::Mutex<()>,
    state: std::sync::Arc<WorkerPoolState>,
    workers: [std::thread::Thread; 3],
}

static WORKER_POOL: std::sync::OnceLock<WorkerPool> = std::sync::OnceLock::new();

fn get_worker_pool() -> &'static WorkerPool {
    WORKER_POOL.get_or_init(|| {
        let state = std::sync::Arc::new(WorkerPoolState {
            job_ptr: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            job_gen: std::sync::atomic::AtomicUsize::new(0),
            done_count: std::sync::atomic::AtomicUsize::new(0),
        });
        let mut handles = Vec::with_capacity(3);
        for worker_idx in 1..=3 {
            let st = std::sync::Arc::clone(&state);
            let h = std::thread::Builder::new()
                .name(format!("weft-sense-w{worker_idx}"))
                .spawn(move || {
                    use std::sync::atomic::Ordering;
                    #[cfg(target_os = "macos")]
                    unsafe {
                        let _ = pthread_set_qos_class_self_np(0x21, 0);
                    }
                    #[cfg(target_os = "macos")]
                    blas_single_threaded();
                    let mut last_gen = 0usize;
                    loop {
                        let mut spins = 0u32;
                        let cur_gen = loop {
                            let g = st.job_gen.load(Ordering::Acquire);
                            if g != last_gen {
                                break g;
                            }
                            spins += 1;
                            if spins < 200_000 {
                                std::hint::spin_loop();
                            } else {
                                std::thread::park();
                                spins = 0;
                            }
                        };
                        last_gen = cur_gen;
                        let ptr = st.job_ptr.load(Ordering::Acquire);
                        if !ptr.is_null() {
                            unsafe {
                                run_layers_worker(worker_idx, &*ptr);
                            }
                        }
                        st.done_count.fetch_add(1, Ordering::Release);
                    }
                })
                .expect("spawn weft-sense worker");
            handles.push(h.thread().clone());
        }
        let [w1, w2, w3]: [std::thread::Thread; 3] = handles.try_into().unwrap();
        WorkerPool { run_lock: std::sync::Mutex::new(()), state, workers: [w1, w2, w3] }
    })
}

impl SenseModel {
    /// Load and SHA-256 verify a bundle directory containing manifest.json,
    /// sense.int8.onnx (or model.int8.onnx), tokenizer.json, and optional calibration.json.
    pub fn load(bundle_dir: &Path) -> anyhow::Result<Self> {
        let manifest_path = bundle_dir.join("manifest.json");
        let manifest_bytes = fs::read(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?;
        let manifest: BundleManifest =
            serde_json::from_slice(&manifest_bytes).context("parse manifest.json")?;
        if manifest.schema != "fab7.sense.bundle/2" {
            bail!("unsupported Sense bundle schema: {}", manifest.schema);
        }

        let model_rel = &manifest.files.model.path;
        let model_path = {
            let p = bundle_dir.join(model_rel);
            if p.exists() {
                p
            } else if bundle_dir.join("sense.int8.onnx").exists() {
                bundle_dir.join("sense.int8.onnx")
            } else {
                bundle_dir.join("model.int8.onnx")
            }
        };
        let model_bytes =
            fs::read(&model_path).with_context(|| format!("read {}", model_path.display()))?;
        let actual_model_sha = sha256_hex(&model_bytes);
        if actual_model_sha != manifest.files.model.sha256 {
            bail!(
                "model SHA-256 mismatch for {}: expected {}, got {}",
                model_path.display(),
                manifest.files.model.sha256,
                actual_model_sha
            );
        }

        let tok_path = bundle_dir.join(&manifest.files.tokenizer.path);
        let tok_bytes =
            fs::read(&tok_path).with_context(|| format!("read {}", tok_path.display()))?;
        let actual_tok_sha = sha256_hex(&tok_bytes);
        if actual_tok_sha != manifest.files.tokenizer.sha256 {
            bail!(
                "tokenizer SHA-256 mismatch for {}: expected {}, got {}",
                tok_path.display(),
                manifest.files.tokenizer.sha256,
                actual_tok_sha
            );
        }

        let cal_path = bundle_dir.join("calibration.json");
        let cal: CalibrationFile = if cal_path.exists() {
            let cal_bytes = fs::read(&cal_path)?;
            serde_json::from_slice(&cal_bytes).unwrap_or_default()
        } else {
            CalibrationFile::default()
        };

        let temperature = cal.temperature.unwrap_or(manifest.temperature);
        let conversation_temperature =
            cal.conversation_temperature.unwrap_or(manifest.conversation_temperature);
        let surface_min_confidence_bp = cal
            .surface_min_confidence
            .map(|c| (c * 10000.0).round() as u16)
            .unwrap_or_else(|| manifest.min_confidence_bp.max(8500));
        let conversation_min_confidence_bp = cal
            .conversation_min_confidence
            .map(|c| (c * 10000.0).round() as u16)
            .unwrap_or_else(|| manifest.min_confidence_bp.max(8000));

        let tokenizer = SenseTokenizer::from_bytes(&tok_bytes)?;
        let weights = parse_onnx_weights(&model_bytes)?;
        if weights.position_embeddings.len() / 384 < manifest.max_seq_len {
            bail!("the model has fewer positions than max_seq_len {}", manifest.max_seq_len);
        }

        #[cfg(target_os = "macos")]
        unsafe {
            let _ = pthread_set_qos_class_self_np(0x21, 0); // QOS_CLASS_USER_INTERACTIVE
        }
        #[cfg(target_os = "macos")]
        blas_single_threaded();

        let model = Self {
            version: manifest.version,
            max_seq_len: manifest.max_seq_len,
            max_bottom_lines: manifest.max_bottom_lines,
            temperature,
            conversation_temperature,
            surface_min_confidence_bp,
            conversation_min_confidence_bp,
            tokenizer,
            weights,
        };
        // Accelerate is slow on its first products; the fallback is not, and
        // warming it would only hold up whoever loads the model.
        #[cfg(target_os = "macos")]
        for _ in 0..2 {
            let _ = model.forward_ids(&[101u32; 256]);
        }
        Ok(model)
    }

    /// Load the active bundle from `~/.fab7/weft/sense/current.json`, falling
    /// back to `~/.fab7/sense/fab7-sense-v1/`.
    pub fn load_default_home(home: &Path) -> anyhow::Result<Option<Self>> {
        let sense_root = home.join(".fab7").join("weft").join("sense");
        let current_path = sense_root.join("current.json");
        if current_path.exists()
            && let Ok(bytes) = fs::read(&current_path)
            && let Ok(ptr) = serde_json::from_slice::<SenseCurrentPointer>(&bytes)
        {
            let bundle_dir = {
                let p = PathBuf::from(&ptr.path);
                if p.is_absolute() { p } else { sense_root.join(p) }
            };
            if bundle_dir.join("manifest.json").exists() {
                return Self::load(&bundle_dir).map(Some);
            }
        }

        let fallback = home.join(".fab7").join("sense").join("fab7-sense-v1");
        if fallback.join("manifest.json").exists() {
            return Self::load(&fallback).map(Some);
        }

        Ok(None)
    }

    /// Return the loaded bundle's version string from manifest.json.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Evaluate Head 1 on a settled PTY snapshot. Returns `None` if calibrated
    /// probability < `surface_min_confidence` (0.85).
    pub fn evaluate_surface(&self, snap: &PaneSnapshot<'_>) -> Option<SurfaceReading> {
        let text = normalize_surface_frame(snap, self.max_bottom_lines);
        let (surf_logits, _, _) = self.infer_raw_text(&text);
        let (idx, conf_bp) = calibrated_argmax(&surf_logits, self.temperature);
        if conf_bp < self.surface_min_confidence_bp {
            return None;
        }
        Some(SurfaceReading {
            state: SurfaceState::from_index(idx)?,
            confidence_bp: conf_bp,
            seq: snap.seq,
        })
    }

    /// Evaluate Head 2 on a transcript entry or free-text tool outcome. Returns
    /// `None` if calibrated probability < `conversation_min_confidence` (0.80).
    pub fn classify_conversation_entry(
        &self,
        item: &TranscriptSnippet<'_>,
    ) -> Option<ConversationReading> {
        let text = normalize_conversation_entry(item, 512);
        let (_, conv_logits, _) = self.infer_raw_text(&text);
        let (idx, conf_bp) = calibrated_argmax(&conv_logits, self.conversation_temperature);
        if conf_bp < self.conversation_min_confidence_bp {
            return None;
        }
        Some(ConversationReading {
            kind: ConversationEntry::from_index(idx)?,
            confidence_bp: conf_bp,
        })
    }

    /// Run tokenization and dual-head ONNX inference on a pre-formatted `[SURFACE]` or `[CONV]` string.
    pub fn infer_raw_text(&self, text: &str) -> ([f32; 8], [f32; 5], Vec<u32>) {
        let ids = self.tokenizer.encode(text, self.max_seq_len);
        let (surf, conv, _) = self.forward_ids(&ids);
        (surf, conv, ids)
    }

    /// The pooled vector both heads read, for `text` as it is: what entries are
    /// compared by when the closest reply is looked for.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        let ids = self.tokenizer.encode(text, self.max_seq_len);
        self.forward_ids(&ids).2.to_vec()
    }

    fn forward_ids(&self, ids: &[u32]) -> ([f32; 8], [f32; 5], [f32; 384]) {
        INFER_SCRATCH.with(|cell| {
            let mut s = cell.borrow_mut();
            self.forward_ids_with_scratch(ids, &mut s)
        })
    }

    fn forward_ids_with_scratch(
        &self,
        ids: &[u32],
        s: &mut InferenceScratch,
    ) -> ([f32; 8], [f32; 5], [f32; 384]) {
        const D: usize = 384;
        const D3: usize = 1152;
        const HEADS: usize = 12;
        const HEAD_DIM: usize = 32;
        const FFN_DIM: usize = 1536;

        #[cfg(target_os = "macos")]
        blas_single_threaded();

        let seq_len = ids.len();
        let w = &self.weights;

        s.x.resize(seq_len * D, 0.0);
        s.xq_buf.resize(seq_len * D, 0.0);
        s.qkv_buf.resize(seq_len * D3, 0.0);
        s.ctx.resize(seq_len * D, 0.0);
        s.ffn_mid.resize(seq_len * FFN_DIM, 0.0);
        s.fq_buf.resize(seq_len * FFN_DIM, 0.0);
        for sc in &mut s.scores {
            sc.resize(seq_len * seq_len, 0.0);
        }

        let x = &mut s.x[..seq_len * D];
        let xq_buf = &mut s.xq_buf[..seq_len * D];
        let qkv_buf = &mut s.qkv_buf[..seq_len * D3];
        let ctx = &mut s.ctx[..seq_len * D];
        let ffn_mid = &mut s.ffn_mid[..seq_len * FFN_DIM];
        let fq_buf = &mut s.fq_buf[..seq_len * FFN_DIM];
        let scratch = &mut s.quant_buf;

        // 1. Embeddings + LayerNorm
        for (pos, &tok_id) in ids.iter().enumerate() {
            let tid = (tok_id as usize).min(w.word_embeddings.len() / D - 1);
            let w_slice = &w.word_embeddings[tid * D..(tid + 1) * D];
            let p_slice = &w.position_embeddings[pos * D..(pos + 1) * D];
            let t_slice = &w.token_type_0;
            let dst = &mut x[pos * D..(pos + 1) * D];
            for k in 0..D {
                dst[k] = (w_slice[k] + t_slice[k]) + p_slice[k];
            }
        }
        w.emb_ln.forward_in_place(x, D);

        if seq_len >= 64 {
            use std::sync::atomic::Ordering;
            let pool = get_worker_pool();
            let _guard = pool.run_lock.lock().unwrap();
            let [sc0, sc1, sc2, sc3] = &mut s.scores;
            let job = ParallelLayerJob {
                weights: w as *const OnnxDualHeadWeights,
                seq_len,
                x: x.as_mut_ptr(),
                xq_buf: xq_buf.as_mut_ptr(),
                qkv_buf: qkv_buf.as_mut_ptr(),
                ctx: ctx.as_mut_ptr(),
                ffn_mid: ffn_mid.as_mut_ptr(),
                fq_buf: fq_buf.as_mut_ptr(),
                scores: [sc0.as_mut_ptr(), sc1.as_mut_ptr(), sc2.as_mut_ptr(), sc3.as_mut_ptr()],
                thread_min: [
                    [
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                    ],
                    [
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                    ],
                ],
                thread_max: [
                    [
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                    ],
                    [
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                        std::cell::UnsafeCell::new(0.0),
                    ],
                ],
                barrier: SpinBarrier::new(),
            };
            pool.state.done_count.store(0, Ordering::Relaxed);
            pool.state
                .job_ptr
                .store((&job as *const ParallelLayerJob).cast_mut(), Ordering::Release);
            pool.state.job_gen.fetch_add(1, Ordering::AcqRel);
            for th in &pool.workers {
                th.unpark();
            }
            unsafe {
                run_layers_worker(0, &job);
            }
            while pool.state.done_count.load(Ordering::Acquire) < 3 {
                std::hint::spin_loop();
            }
            pool.state.job_ptr.store(std::ptr::null_mut(), Ordering::Release);
        } else {
            let scores = &mut s.scores[0][..seq_len * seq_len];
            for layer in &w.layers {
                let xq = QuantizedActivation::from_f32(x, seq_len, D, scratch);
                layer.qkv.forward_quant(&xq, qkv_buf);
                xq.recycle(scratch);

                for h in 0..HEADS {
                    let h_col = h * HEAD_DIM;
                    #[cfg(target_os = "macos")]
                    unsafe {
                        cblas_sgemm(
                            101,
                            111,
                            112,
                            seq_len as i32,
                            seq_len as i32,
                            HEAD_DIM as i32,
                            1.0_f32,
                            qkv_buf.as_ptr().add(h_col),
                            D3 as i32,
                            qkv_buf.as_ptr().add(D + h_col),
                            D3 as i32,
                            0.0_f32,
                            scores.as_mut_ptr(),
                            seq_len as i32,
                        );
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        for i in 0..seq_len {
                            let q_r = &qkv_buf[i * D3 + h_col..i * D3 + h_col + HEAD_DIM];
                            let s_row = &mut scores[i * seq_len..(i + 1) * seq_len];
                            for (j, s_val) in s_row.iter_mut().enumerate() {
                                let k_r =
                                    &qkv_buf[j * D3 + D + h_col..j * D3 + D + h_col + HEAD_DIM];
                                let mut dot = 0.0_f32;
                                for d in 0..HEAD_DIM {
                                    dot += q_r[d] * k_r[d];
                                }
                                *s_val = dot;
                            }
                        }
                    }

                    for score_row in scores.chunks_exact_mut(seq_len) {
                        softmax_row_in_place(score_row);
                    }

                    #[cfg(target_os = "macos")]
                    unsafe {
                        cblas_sgemm(
                            101,
                            111,
                            111,
                            seq_len as i32,
                            HEAD_DIM as i32,
                            seq_len as i32,
                            1.0_f32,
                            scores.as_ptr(),
                            seq_len as i32,
                            qkv_buf.as_ptr().add(2 * D + h_col),
                            D3 as i32,
                            0.0_f32,
                            ctx.as_mut_ptr().add(h_col),
                            D as i32,
                        );
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        for i in 0..seq_len {
                            let s_row = &scores[i * seq_len..(i + 1) * seq_len];
                            let c_row = &mut ctx[i * D + h_col..i * D + h_col + HEAD_DIM];
                            c_row.fill(0.0);
                            for (j, &prob) in s_row.iter().enumerate() {
                                let v_r = &qkv_buf
                                    [j * D3 + 2 * D + h_col..j * D3 + 2 * D + h_col + HEAD_DIM];
                                for d in 0..HEAD_DIM {
                                    c_row[d] += prob * v_r[d];
                                }
                            }
                        }
                    }
                }

                let ctx_q = QuantizedActivation::from_f32(ctx, seq_len, D, scratch);
                layer.attn_out.forward_quant_accum_no_bias(&ctx_q, x);
                ctx_q.recycle(scratch);
                layer.attn_ln.add_bias_and_forward_in_place(x, &layer.attn_out.bias, D);

                let xq2 = QuantizedActivation::from_f32(x, seq_len, D, scratch);
                layer.ffn_inter.forward_quant_no_bias(&xq2, ffn_mid);
                xq2.recycle(scratch);

                let (fmin, fmax) = gelu_bias_minmax_in_place(ffn_mid, &layer.ffn_inter.bias);

                let fq = QuantizedActivation::from_f32_with_minmax(
                    ffn_mid, seq_len, FFN_DIM, fmin, fmax, scratch,
                );
                layer.ffn_out.forward_quant_accum_no_bias(&fq, x);
                fq.recycle(scratch);
                layer.ffn_ln.add_bias_and_forward_in_place(x, &layer.ffn_out.bias, D);
            }
        }

        // Pooling: concat([CLS], mean) -> Linear(768, 384) -> GELU -> LayerNorm
        let mut concat = [0.0_f32; D * 2];
        concat[..D].copy_from_slice(&x[..D]);
        let denom = (seq_len as f32).max(1.0_f32);
        for r in 0..seq_len {
            let row = &x[r * D..(r + 1) * D];
            for k in 0..D {
                concat[D + k] += row[k];
            }
        }
        for k in 0..D {
            concat[D + k] /= denom;
        }

        let mut pooled = [0.0_f32; D];
        w.pool_linear.forward(&concat, 1, &mut pooled, scratch);
        gelu_in_place(&mut pooled);
        w.pool_ln.forward_in_place(&mut pooled, D);

        let pq = QuantizedActivation::from_f32(&pooled, 1, D, scratch);
        let mut surf_hidden = [0.0_f32; 192];
        let mut surf_logits = [0.0_f32; 8];
        w.surface_fc0.forward_quant(&pq, &mut surf_hidden);
        gelu_in_place(&mut surf_hidden);
        w.surface_fc2.forward(&surf_hidden, 1, &mut surf_logits, scratch);

        let mut conv_hidden = [0.0_f32; 192];
        let mut conv_logits = [0.0_f32; 5];
        w.conv_fc0.forward_quant(&pq, &mut conv_hidden);
        pq.recycle(scratch);
        gelu_in_place(&mut conv_hidden);
        w.conv_fc2.forward(&conv_hidden, 1, &mut conv_logits, scratch);

        (surf_logits, conv_logits, pooled)
    }

    pub fn temperature(&self) -> f32 {
        self.temperature
    }

    pub fn conversation_temperature(&self) -> f32 {
        self.conversation_temperature
    }
}

#[inline(always)]
fn fast_exp_neg(x: f32) -> f32 {
    let u = x.max(-20.0_f32) * 0.015625_f32; // x / 64.0
    let p = ((((0.008333334_f32 * u + 0.041666668_f32) * u + 0.16666667_f32) * u + 0.5_f32) * u
        + 1.0_f32)
        * u
        + 1.0_f32;
    let p2 = p * p;
    let p4 = p2 * p2;
    let p8 = p4 * p4;
    let p16 = p8 * p8;
    let p32 = p16 * p16;
    p32 * p32
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[allow(unsafe_op_in_unsafe_fn)]
#[inline]
unsafe fn softmax_row_neon(row: &mut [f32]) {
    use std::arch::aarch64::{
        vaddq_f32, vaddvq_f32, vdupq_n_f32, vfmaq_f32, vmaxnmq_f32, vmaxnmvq_f32, vmulq_f32,
        vsubq_f32,
    };
    let n = row.len();
    let ptr = row.as_mut_ptr();
    let mut vmax0 = vdupq_n_f32(f32::NEG_INFINITY);
    let mut vmax1 = vdupq_n_f32(f32::NEG_INFINITY);
    let mut i = 0;
    while i + 8 <= n {
        vmax0 = vmaxnmq_f32(vmax0, ld1q_f32(ptr.add(i)));
        vmax1 = vmaxnmq_f32(vmax1, ld1q_f32(ptr.add(i + 4)));
        i += 8;
    }
    let mut max_s = vmaxnmvq_f32(vmaxnmq_f32(vmax0, vmax1));
    while i < n {
        max_s = max_s.max(*ptr.add(i));
        i += 1;
    }

    let v_max = vdupq_n_f32(max_s);
    let v_lo = vdupq_n_f32(-20.0_f32);
    let v_inv64 = vdupq_n_f32(0.015625_f32);
    let c5 = vdupq_n_f32(0.008333334_f32);
    let c4 = vdupq_n_f32(0.041666668_f32);
    let c3 = vdupq_n_f32(0.16666667_f32);
    let c2 = vdupq_n_f32(0.5_f32);
    let c1 = vdupq_n_f32(1.0_f32);

    let mut vsum0 = vdupq_n_f32(0.0);
    let mut vsum1 = vdupq_n_f32(0.0);
    i = 0;
    while i + 8 <= n {
        let x0 = vmaxnmq_f32(vsubq_f32(ld1q_f32(ptr.add(i)), v_max), v_lo);
        let x1 = vmaxnmq_f32(vsubq_f32(ld1q_f32(ptr.add(i + 4)), v_max), v_lo);

        let u0 = vmulq_f32(x0, v_inv64);
        let u1 = vmulq_f32(x1, v_inv64);

        let mut p0 = vfmaq_f32(c4, c5, u0);
        let mut p1 = vfmaq_f32(c4, c5, u1);
        p0 = vfmaq_f32(c3, p0, u0);
        p1 = vfmaq_f32(c3, p1, u1);
        p0 = vfmaq_f32(c2, p0, u0);
        p1 = vfmaq_f32(c2, p1, u1);
        p0 = vfmaq_f32(c1, p0, u0);
        p1 = vfmaq_f32(c1, p1, u1);
        p0 = vfmaq_f32(c1, p0, u0);
        p1 = vfmaq_f32(c1, p1, u1);

        p0 = vmulq_f32(p0, p0);
        p1 = vmulq_f32(p1, p1);
        p0 = vmulq_f32(p0, p0);
        p1 = vmulq_f32(p1, p1);
        p0 = vmulq_f32(p0, p0);
        p1 = vmulq_f32(p1, p1);
        p0 = vmulq_f32(p0, p0);
        p1 = vmulq_f32(p1, p1);
        p0 = vmulq_f32(p0, p0);
        p1 = vmulq_f32(p1, p1);

        let e0 = vmulq_f32(p0, p0);
        let e1 = vmulq_f32(p1, p1);

        st1q_f32(ptr.add(i), e0);
        st1q_f32(ptr.add(i + 4), e1);

        vsum0 = vaddq_f32(vsum0, e0);
        vsum1 = vaddq_f32(vsum1, e1);
        i += 8;
    }
    let mut sum_exp = vaddvq_f32(vaddq_f32(vsum0, vsum1));
    while i < n {
        let e = fast_exp_neg(*ptr.add(i) - max_s);
        *ptr.add(i) = e;
        sum_exp += e;
        i += 1;
    }

    let inv_sum = 1.0_f32 / sum_exp;
    let v_inv_sum = vdupq_n_f32(inv_sum);
    i = 0;
    while i + 8 <= n {
        st1q_f32(ptr.add(i), vmulq_f32(ld1q_f32(ptr.add(i)), v_inv_sum));
        st1q_f32(ptr.add(i + 4), vmulq_f32(ld1q_f32(ptr.add(i + 4)), v_inv_sum));
        i += 8;
    }
    while i < n {
        *ptr.add(i) *= inv_sum;
        i += 1;
    }
}

#[inline(always)]
fn softmax_row_in_place(row: &mut [f32]) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        softmax_row_neon(row);
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut max_s = f32::NEG_INFINITY;
        for &s in row.iter() {
            max_s = max_s.max(s);
        }
        let mut sum_exp = 0.0_f32;
        for s in row.iter_mut() {
            *s = fast_exp_neg(*s - max_s);
            sum_exp += *s;
        }
        let inv_sum = 1.0_f32 / sum_exp;
        for s in row.iter_mut() {
            *s *= inv_sum;
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[allow(unsafe_op_in_unsafe_fn, clippy::excessive_precision)]
#[inline]
unsafe fn gelu_bias_minmax_neon(slice: &mut [f32], bias: &[f32]) -> (f32, f32) {
    use std::arch::aarch64::{
        vabsq_f32, vaddq_f32, vbslq_f32, vcltzq_f32, vdivq_f32, vdupq_n_f32, vfmaq_f32,
        vmaxnmq_f32, vmaxnmvq_f32, vminnmq_f32, vminnmvq_f32, vmulq_f32, vnegq_f32, vsubq_f32,
    };
    let max_abs = vdupq_n_f32(5.65685425_f32);
    let one = vdupq_n_f32(1.0_f32);
    let half = vdupq_n_f32(0.5_f32);
    let b6 = vdupq_n_f32(0.000005382975_f32);
    let b5 = vdupq_n_f32(0.00004889062_f32);
    let b4 = vdupq_n_f32(0.000038003575_f32);
    let b3 = vdupq_n_f32(0.0032776263_f32);
    let b2 = vdupq_n_f32(0.02114100615_f32);
    let b1 = vdupq_n_f32(0.0498673470_f32);

    let mut vmin0 = vdupq_n_f32(0.0);
    let mut vmin1 = vdupq_n_f32(0.0);
    let mut vmax0 = vdupq_n_f32(0.0);
    let mut vmax1 = vdupq_n_f32(0.0);

    let cols = bias.len();
    let b_ptr = bias.as_ptr();
    for row in slice.chunks_exact_mut(cols) {
        let ptr = row.as_mut_ptr();
        let mut i = 0;
        while i + 8 <= cols {
            let v0 = vaddq_f32(ld1q_f32(ptr.add(i)), ld1q_f32(b_ptr.add(i)));
            let v1 = vaddq_f32(ld1q_f32(ptr.add(i + 4)), ld1q_f32(b_ptr.add(i + 4)));

            let ab0 = vminnmq_f32(vabsq_f32(v0), max_abs);
            let ab1 = vminnmq_f32(vabsq_f32(v1), max_abs);

            let mut p0 = vfmaq_f32(b5, b6, ab0);
            let mut p1 = vfmaq_f32(b5, b6, ab1);
            p0 = vfmaq_f32(b4, p0, ab0);
            p1 = vfmaq_f32(b4, p1, ab1);
            p0 = vfmaq_f32(b3, p0, ab0);
            p1 = vfmaq_f32(b3, p1, ab1);
            p0 = vfmaq_f32(b2, p0, ab0);
            p1 = vfmaq_f32(b2, p1, ab1);
            p0 = vfmaq_f32(b1, p0, ab0);
            p1 = vfmaq_f32(b1, p1, ab1);
            p0 = vfmaq_f32(one, p0, ab0);
            p1 = vfmaq_f32(one, p1, ab1);

            p0 = vmulq_f32(p0, p0);
            p1 = vmulq_f32(p1, p1);
            p0 = vmulq_f32(p0, p0);
            p1 = vmulq_f32(p1, p1);
            p0 = vmulq_f32(p0, p0);
            p1 = vmulq_f32(p1, p1);
            p0 = vmulq_f32(p0, p0);
            p1 = vmulq_f32(p1, p1);

            let val0 = vsubq_f32(one, vdivq_f32(one, p0));
            let val1 = vsubq_f32(one, vdivq_f32(one, p1));

            let erf0 = vbslq_f32(vcltzq_f32(v0), vnegq_f32(val0), val0);
            let erf1 = vbslq_f32(vcltzq_f32(v1), vnegq_f32(val1), val1);

            let out0 = vmulq_f32(vmulq_f32(v0, vaddq_f32(erf0, one)), half);
            let out1 = vmulq_f32(vmulq_f32(v1, vaddq_f32(erf1, one)), half);

            vmin0 = vminnmq_f32(vmin0, out0);
            vmin1 = vminnmq_f32(vmin1, out1);
            vmax0 = vmaxnmq_f32(vmax0, out0);
            vmax1 = vmaxnmq_f32(vmax1, out1);

            st1q_f32(ptr.add(i), out0);
            st1q_f32(ptr.add(i + 4), out1);
            i += 8;
        }
    }

    (vminnmvq_f32(vminnmq_f32(vmin0, vmin1)), vmaxnmvq_f32(vmaxnmq_f32(vmax0, vmax1)))
}

fn gelu_bias_minmax_in_place(slice: &mut [f32], bias: &[f32]) -> (f32, f32) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        gelu_bias_minmax_neon(slice, bias)
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
        let mut xmin = 0.0_f32;
        let mut xmax = 0.0_f32;
        for row in slice.chunks_exact_mut(bias.len()) {
            for (x, &b) in row.iter_mut().zip(bias.iter()) {
                let v = *x + b;
                let out = (v * (erf_f32(v * INV_SQRT2) + 1.0_f32)) * 0.5_f32;
                *x = out;
                xmin = xmin.min(out);
                xmax = xmax.max(out);
            }
        }
        (xmin, xmax)
    }
}

fn gelu_in_place(slice: &mut [f32]) {
    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    for x in slice.iter_mut() {
        let v = *x;
        *x = (v * (erf_f32(v * INV_SQRT2) + 1.0_f32)) * 0.5_f32;
    }
}

#[allow(clippy::excessive_precision)]
#[inline(always)]
fn erf_f32(x: f32) -> f32 {
    let a = x.abs().min(4.0_f32);
    let p = ((((((0.0000430638_f32 * a + 0.0002765672_f32) * a + 0.0001520143_f32) * a
        + 0.0092705272_f32)
        * a
        + 0.0422820123_f32)
        * a
        + 0.0705230784_f32)
        * a)
        + 1.0_f32;
    let p2 = p * p;
    let p4 = p2 * p2;
    let p8 = p4 * p4;
    let p16 = p8 * p8;
    let val = 1.0_f32 - 1.0_f32 / p16;
    if x < 0.0_f32 { -val } else { val }
}

pub fn calibrated_argmax(logits: &[f32], temperature: f32) -> (usize, u16) {
    let inv_t = 1.0_f32 / temperature.max(1e-4);
    let mut max_s = f32::NEG_INFINITY;
    for &l in logits {
        let s = l / temperature.max(1e-4);
        if s > max_s {
            max_s = s;
        }
    }
    let _ = inv_t;
    let mut sum = 0.0_f32;
    let mut best_idx = 0;
    let mut best_exp = 0.0_f32;
    for (idx, &l) in logits.iter().enumerate() {
        let e = (l / temperature.max(1e-4) - max_s).exp();
        sum += e;
        if e > best_exp {
            best_exp = e;
            best_idx = idx;
        }
    }
    let prob = best_exp / sum;
    let bp = (prob * 10000.0_f32).round().clamp(0.0, 10000.0) as u16;
    (best_idx, bp)
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

// --- Minimal ONNX Protobuf Reader ---

#[derive(Default)]
struct RawTensor {
    name: String,
    dims: Vec<usize>,
    data_type: i32,
    float_data: Vec<f32>,
    int32_data: Vec<i32>,
    raw_data: Vec<u8>,
}

impl RawTensor {
    fn to_f32_vec(&self) -> Vec<f32> {
        if !self.float_data.is_empty() {
            return self.float_data.clone();
        }
        let mut out = Vec::with_capacity(self.raw_data.len() / 4);
        for chunk in self.raw_data.as_chunks::<4>().0 {
            out.push(f32::from_le_bytes(*chunk));
        }
        out
    }

    fn to_u8_vec(&self) -> Vec<u8> {
        if !self.raw_data.is_empty() {
            return self.raw_data.clone();
        }
        self.int32_data.iter().map(|&x| x as u8).collect()
    }

    /// How many values its shape holds; `None` past `usize`.
    fn count(&self) -> Option<usize> {
        self.dims.iter().try_fold(1usize, |n, &d| n.checked_mul(d))
    }

    /// Its values as bytes, refused unless there are as many as its shape holds.
    fn u8s(&self, name: &str) -> anyhow::Result<Vec<u8>> {
        let v = self.to_u8_vec();
        if self.count() != Some(v.len()) {
            bail!("{name} has {} values for its shape {:?}", v.len(), self.dims);
        }
        Ok(v)
    }

    /// Its values as floats, refused unless there are as many as its shape holds.
    fn f32s(&self, name: &str) -> anyhow::Result<Vec<f32>> {
        let v = self.to_f32_vec();
        if self.count() != Some(v.len()) {
            bail!("{name} has {} values for its shape {:?}", v.len(), self.dims);
        }
        Ok(v)
    }

    fn scalar_f32(&self) -> f32 {
        self.to_f32_vec().first().copied().unwrap_or(1.0)
    }

    fn scalar_u8(&self) -> u8 {
        self.to_u8_vec().first().copied().unwrap_or(0)
    }
}

#[derive(Default)]
struct RawNode {
    name: String,
    op_type: String,
    inputs: Vec<String>,
}

fn read_varint(buf: &[u8], pos: &mut usize) -> anyhow::Result<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    while *pos < buf.len() {
        let b = buf[*pos];
        *pos += 1;
        result |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            bail!("varint overflow");
        }
    }
    bail!("unexpected EOF in varint")
}

/// A length-delimited field's bytes, with `pos` moved past them.
fn read_bytes<'a>(buf: &'a [u8], pos: &mut usize) -> anyhow::Result<&'a [u8]> {
    let len = usize::try_from(read_varint(buf, pos)?)?;
    let end = pos.checked_add(len).filter(|&end| end <= buf.len());
    let end = end.context("protobuf field exceeds buffer")?;
    let bytes = &buf[*pos..end];
    *pos = end;
    Ok(bytes)
}

fn skip_field(buf: &[u8], pos: &mut usize, wire_type: u8) -> anyhow::Result<()> {
    match wire_type {
        0 => {
            let _ = read_varint(buf, pos)?;
        }
        1 => *pos = pos.saturating_add(8),
        2 => {
            let _ = read_bytes(buf, pos)?;
        }
        5 => *pos = pos.saturating_add(4),
        other => bail!("unsupported protobuf wire type {other}"),
    }
    if *pos > buf.len() {
        bail!("protobuf field exceeds buffer");
    }
    Ok(())
}

fn parse_node_proto(buf: &[u8]) -> anyhow::Result<RawNode> {
    let mut node = RawNode::default();
    let mut pos = 0;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos)?;
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x7) as u8;
        match (field_num, wire_type) {
            (1, 2) => {
                node.inputs.push(std::str::from_utf8(read_bytes(buf, &mut pos)?)?.to_string());
            }
            (3, 2) => node.name = std::str::from_utf8(read_bytes(buf, &mut pos)?)?.to_string(),
            (4, 2) => node.op_type = std::str::from_utf8(read_bytes(buf, &mut pos)?)?.to_string(),
            _ => skip_field(buf, &mut pos, wire_type)?,
        }
    }
    Ok(node)
}

fn parse_tensor_proto(buf: &[u8]) -> anyhow::Result<RawTensor> {
    let mut t = RawTensor::default();
    let mut pos = 0;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos)?;
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x7) as u8;
        match (field_num, wire_type) {
            (1, 0) => {
                t.dims.push(read_varint(buf, &mut pos)? as usize);
            }
            (1, 2) => {
                let packed = read_bytes(buf, &mut pos)?;
                let mut p = 0;
                while p < packed.len() {
                    t.dims.push(read_varint(packed, &mut p)? as usize);
                }
            }
            (2, 0) => {
                t.data_type = read_varint(buf, &mut pos)? as i32;
            }
            (4, 5) => {
                let v = buf.get(pos..pos + 4).context("protobuf field exceeds buffer")?;
                pos += 4;
                t.float_data.push(f32::from_le_bytes(v.try_into()?));
            }
            (4, 2) => {
                for chunk in read_bytes(buf, &mut pos)?.as_chunks::<4>().0 {
                    t.float_data.push(f32::from_le_bytes(*chunk));
                }
            }
            (5, 0) => {
                t.int32_data.push(read_varint(buf, &mut pos)? as i32);
            }
            (5, 2) => {
                let packed = read_bytes(buf, &mut pos)?;
                let mut p = 0;
                while p < packed.len() {
                    t.int32_data.push(read_varint(packed, &mut p)? as i32);
                }
            }
            (8, 2) => t.name = std::str::from_utf8(read_bytes(buf, &mut pos)?)?.to_string(),
            (9, 2) => t.raw_data = read_bytes(buf, &mut pos)?.to_vec(),
            _ => skip_field(buf, &mut pos, wire_type)?,
        }
    }
    Ok(t)
}

fn parse_onnx_weights(model_bytes: &[u8]) -> anyhow::Result<OnnxDualHeadWeights> {
    let mut nodes: HashMap<String, RawNode> = HashMap::new();
    let mut inits: HashMap<String, RawTensor> = HashMap::new();

    let mut pos = 0;
    while pos < model_bytes.len() {
        let tag = read_varint(model_bytes, &mut pos)?;
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x7) as u8;
        if field_num == 7 && wire_type == 2 {
            let graph_buf = read_bytes(model_bytes, &mut pos)?;

            let mut gpos = 0;
            while gpos < graph_buf.len() {
                let gtag = read_varint(graph_buf, &mut gpos)?;
                let gfield = (gtag >> 3) as u32;
                let gwire = (gtag & 0x7) as u8;
                match (gfield, gwire) {
                    (1, 2) => {
                        let n = parse_node_proto(read_bytes(graph_buf, &mut gpos)?)?;
                        if !n.name.is_empty() {
                            nodes.insert(n.name.clone(), n);
                        }
                    }
                    (5, 2) => {
                        let t = parse_tensor_proto(read_bytes(graph_buf, &mut gpos)?)?;
                        if !t.name.is_empty() {
                            inits.insert(t.name.clone(), t);
                        }
                    }
                    _ => skip_field(graph_buf, &mut gpos, gwire)?,
                }
            }
        } else {
            skip_field(model_bytes, &mut pos, wire_type)?;
        }
    }

    // The model's widths, as the forward pass reads them.
    const D: usize = 384;
    const FFN_DIM: usize = 1536;
    const HEAD_DIM: usize = 192;

    let dequant_emb = |prefix: &str| -> anyhow::Result<Vec<f32>> {
        let name = format!("{prefix}_quantized");
        let t = inits.get(&name).with_context(|| format!("missing {name}"))?;
        if t.dims.len() != 2 || t.dims[0] == 0 || t.dims[1] != D {
            bail!("{name} has shape {:?}, not rows of {D}", t.dims);
        }
        let q = t.u8s(&name)?;
        let zp = inits
            .get(&format!("{prefix}_zero_point"))
            .with_context(|| format!("missing {prefix}_zero_point"))?
            .scalar_u8() as f32;
        let sc = inits
            .get(&format!("{prefix}_scale"))
            .with_context(|| format!("missing {prefix}_scale"))?
            .scalar_f32();
        Ok(q.into_iter().map(|v| (v as f32 - zp) * sc).collect())
    };

    let load_ln = |w_name: &str, b_name: &str| -> anyhow::Result<LayerNorm> {
        let weight =
            inits.get(w_name).with_context(|| format!("missing {w_name}"))?.f32s(w_name)?;
        let bias = inits.get(b_name).with_context(|| format!("missing {b_name}"))?.f32s(b_name)?;
        if weight.len() != D || bias.len() != D {
            bail!("{w_name} and {b_name} have {} and {} values, not {D}", weight.len(), bias.len());
        }
        Ok(LayerNorm { weight, bias })
    };

    let load_qlinear = |node_name: &str,
                        bias_name: &str,
                        in_dim: usize,
                        out_dim: usize|
     -> anyhow::Result<QuantLinear> {
        let node =
            nodes.get(node_name).with_context(|| format!("missing ONNX node {node_name}"))?;
        let w_q_name =
            node.inputs.get(1).with_context(|| format!("node {node_name} missing weight input"))?;
        let prefix = w_q_name
            .strip_suffix("_quantized")
            .with_context(|| format!("unexpected weight name {w_q_name}"))?;
        let w_tensor =
            inits.get(w_q_name).with_context(|| format!("missing initializer {w_q_name}"))?;
        if w_tensor.dims != [in_dim, out_dim] {
            bail!("{w_q_name} has shape {:?}, not [{in_dim}, {out_dim}]", w_tensor.dims);
        }
        let w_raw = w_tensor.u8s(w_q_name)?;
        let w_zp = inits
            .get(&format!("{prefix}_zero_point"))
            .with_context(|| format!("missing {prefix}_zero_point"))?
            .scalar_u8() as f32;
        let w_scale = inits
            .get(&format!("{prefix}_scale"))
            .with_context(|| format!("missing {prefix}_scale"))?
            .scalar_f32();
        let bias = inits
            .get(bias_name)
            .with_context(|| format!("missing {bias_name}"))?
            .f32s(bias_name)?;
        if bias.len() != out_dim {
            bail!("{bias_name} has {} values, not {out_dim}", bias.len());
        }

        #[cfg(not(target_os = "macos"))]
        let w_int: Vec<i16> = w_raw.iter().map(|&v| i16::from(v) - w_zp as i16).collect();
        let w_f32: Vec<f32> = w_raw.into_iter().map(|v| (v as f32) - w_zp).collect();
        Ok(QuantLinear {
            in_dim,
            out_dim,
            w_f32,
            #[cfg(not(target_os = "macos"))]
            w_int,
            w_scale,
            bias,
        })
    };

    let word_embeddings = dequant_emb("backbone.embeddings.word_embeddings.weight")?;
    let position_embeddings = dequant_emb("backbone.embeddings.position_embeddings.weight")?;
    let token_type_all = dequant_emb("backbone.embeddings.token_type_embeddings.weight")?;
    let token_type_0 = token_type_all[..D].to_vec();
    let emb_ln =
        load_ln("backbone.embeddings.LayerNorm.weight", "backbone.embeddings.LayerNorm.bias")?;

    let load_encoder_layer = |idx: usize| -> anyhow::Result<EncoderLayer> {
        const ATTN_SCALE: f32 = 0.4204482_f32;
        let p = format!("/backbone/encoder/layer.{idx}");
        let bp = format!("backbone.encoder.layer.{idx}");
        let q = load_qlinear(
            &format!("{p}/attention/self/query/MatMul_quant"),
            &format!("{bp}.attention.self.query.bias"),
            D,
            D,
        )?;
        let k = load_qlinear(
            &format!("{p}/attention/self/key/MatMul_quant"),
            &format!("{bp}.attention.self.key.bias"),
            D,
            D,
        )?;
        let v = load_qlinear(
            &format!("{p}/attention/self/value/MatMul_quant"),
            &format!("{bp}.attention.self.value.bias"),
            D,
            D,
        )?;

        let in_dim = q.in_dim;
        let out_dim = q.out_dim + k.out_dim + v.out_dim;
        let mut qkv_w = Vec::with_capacity(in_dim * out_dim);
        let q_sc = q.w_scale * ATTN_SCALE;
        let k_sc = k.w_scale * ATTN_SCALE;
        let v_sc = v.w_scale;
        for row_idx in 0..in_dim {
            let qr = &q.w_f32[row_idx * q.out_dim..(row_idx + 1) * q.out_dim];
            let kr = &k.w_f32[row_idx * k.out_dim..(row_idx + 1) * k.out_dim];
            let vr = &v.w_f32[row_idx * v.out_dim..(row_idx + 1) * v.out_dim];
            qkv_w.extend(qr.iter().map(|&x| x * q_sc));
            qkv_w.extend(kr.iter().map(|&x| x * k_sc));
            qkv_w.extend(vr.iter().map(|&x| x * v_sc));
        }

        let mut qkv_bias = Vec::with_capacity(out_dim);
        qkv_bias.extend(q.bias.iter().map(|&b| b * ATTN_SCALE));
        qkv_bias.extend(k.bias.iter().map(|&b| b * ATTN_SCALE));
        qkv_bias.extend(v.bias.iter().copied());

        let qkv = QuantLinear {
            in_dim,
            out_dim,
            w_f32: qkv_w,
            #[cfg(not(target_os = "macos"))]
            w_int: Vec::new(),
            w_scale: 1.0_f32,
            bias: qkv_bias,
        };

        Ok(EncoderLayer {
            qkv,
            attn_out: load_qlinear(
                &format!("{p}/attention/output/dense/MatMul_quant"),
                &format!("{bp}.attention.output.dense.bias"),
                D,
                D,
            )?,
            attn_ln: load_ln(
                &format!("{bp}.attention.output.LayerNorm.weight"),
                &format!("{bp}.attention.output.LayerNorm.bias"),
            )?,
            ffn_inter: load_qlinear(
                &format!("{p}/intermediate/dense/MatMul_quant"),
                &format!("{bp}.intermediate.dense.bias"),
                D,
                FFN_DIM,
            )?,
            ffn_out: load_qlinear(
                &format!("{p}/output/dense/MatMul_quant"),
                &format!("{bp}.output.dense.bias"),
                FFN_DIM,
                D,
            )?,
            ffn_ln: load_ln(
                &format!("{bp}.output.LayerNorm.weight"),
                &format!("{bp}.output.LayerNorm.bias"),
            )?,
        })
    };

    let layers = [load_encoder_layer(0)?, load_encoder_layer(1)?, load_encoder_layer(2)?];

    let pool_linear =
        load_qlinear("/pool_proj/pool_proj.0/Gemm_MatMul_quant", "pool_proj.0.bias", 2 * D, D)?;
    let pool_ln = load_ln("pool_proj.2.weight", "pool_proj.2.bias")?;
    let surface_fc0 = load_qlinear(
        "/surface_head/surface_head.0/Gemm_MatMul_quant",
        "surface_head.0.bias",
        D,
        HEAD_DIM,
    )?;
    let surface_fc2 = load_qlinear(
        "/surface_head/surface_head.2/Gemm_MatMul_quant",
        "surface_head.2.bias",
        HEAD_DIM,
        8,
    )?;
    let conv_fc0 = load_qlinear(
        "/conversation_head/conversation_head.0/Gemm_MatMul_quant",
        "conversation_head.0.bias",
        D,
        HEAD_DIM,
    )?;
    let conv_fc2 = load_qlinear(
        "/conversation_head/conversation_head.2/Gemm_MatMul_quant",
        "conversation_head.2.bias",
        HEAD_DIM,
        5,
    )?;

    Ok(OnnxDualHeadWeights {
        word_embeddings,
        position_embeddings,
        token_type_0,
        emb_ln,
        layers,
        pool_linear,
        pool_ln,
        surface_fc0,
        surface_fc2,
        conv_fc0,
        conv_fc2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_bundle_dir() -> PathBuf {
        let workspace_bundle =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../sense/dist/fab7-sense-v1");
        if workspace_bundle.exists() {
            return workspace_bundle;
        }
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".fab7/sense/fab7-sense-v1")
    }

    #[test]
    fn a_tensor_field_past_its_buffer_is_refused() {
        // raw_data says 16 bytes and has 2; a float has 2 of its 4; packed dims say 5 and have 1.
        for bad in [&[0x4A, 0x10, 1, 2][..], &[0x25, 1, 2], &[0x0A, 0x05, 1]] {
            assert!(parse_tensor_proto(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_tensor_whose_values_are_not_its_shape_is_refused() {
        let t = RawTensor { dims: vec![2, 3], raw_data: vec![0; 5], ..Default::default() };
        assert!(t.u8s("w").is_err());
        let t = RawTensor { dims: vec![usize::MAX, 2], raw_data: vec![0; 2], ..Default::default() };
        assert!(t.u8s("w").is_err());
        let t = RawTensor { dims: vec![2], float_data: vec![1.0, 2.0], ..Default::default() };
        assert_eq!(t.f32s("b").unwrap(), [1.0, 2.0]);
    }

    #[test]
    fn a_damaged_model_is_refused_never_a_panic() {
        let bytes = std::fs::read(test_bundle_dir().join("sense.int8.onnx")).unwrap();
        assert!(parse_onnx_weights(&bytes).is_ok());
        // One byte in every 4099 spoiled in turn: lengths, tags, shapes and values alike.
        for at in (0..bytes.len()).step_by(4099).take(400) {
            let mut bad = bytes.clone();
            bad[at] ^= 0xFF;
            let _ = parse_onnx_weights(&bad);
        }
        for cut in (0..bytes.len()).step_by(bytes.len() / 50) {
            assert!(parse_onnx_weights(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn evaluate_surface_and_classify_conversation_entry_end_to_end() {
        let dir = test_bundle_dir();
        let model = SenseModel::load(&dir).unwrap();

        let lines = vec![
            "● Done.".to_string(),
            "◦ Verifying goal completion with judge… (12s · esc to interrupt)".to_string(),
            "❯ ".to_string(),
        ];
        let snap = PaneSnapshot {
            harness: "claude-code",
            hook_state: Some("turn_ended"),
            alt_screen: false,
            rows: 40,
            cols: 120,
            cursor_row: 38,
            cursor_col: 2,
            ms_since_last_output: 200,
            bottom_lines: &lines,
            seq: 99,
        };
        let reading = model.evaluate_surface(&snap).expect("should be confident");
        assert_eq!(reading.state, SurfaceState::WorkingPostHook);
        assert_eq!(reading.seq, 99);
        assert!(reading.confidence_bp >= 8500);

        let err_item = TranscriptSnippet {
            harness: "antigravity",
            entry_type: "GENERIC",
            snippet: "Created At: 2026-10-15T11:07:29Z\nCompleted At: 2026-10-15T11:07:29Z\nEncountered error in step execution: declaring permissions: cortex tool ask_question: convert tool call for permissions: model output error: invalid tool call error (invalid_args) each question requires at least 2 options, got 0 for: \"Pick one? The choices are A and B. Return 1 for A and 2 for B.\"",
        };
        let conv_reading =
            model.classify_conversation_entry(&err_item).expect("should classify tool error");
        assert_eq!(conv_reading.kind, ConversationEntry::ToolErr);
        assert!(conv_reading.confidence_bp >= 8000);
    }

    #[test]
    fn sha256_mismatch_is_rejected_and_default_home_resolves_current_json() {
        let src_dir = test_bundle_dir();
        let tmp = tempfile::tempdir().unwrap();
        let ver_dir = tmp.path().join(".fab7/weft/sense/versions/0.2.0");
        fs::create_dir_all(&ver_dir).unwrap();
        for name in ["manifest.json", "sense.int8.onnx", "tokenizer.json"] {
            fs::copy(src_dir.join(name), ver_dir.join(name)).unwrap();
        }
        let current_json = tmp.path().join(".fab7/weft/sense/current.json");
        fs::write(&current_json, r#"{"version":"0.2.0","path":"versions/0.2.0"}"#).unwrap();

        let loaded = SenseModel::load_default_home(tmp.path())
            .unwrap()
            .expect("should load from current.json");
        assert_eq!(loaded.version(), "0.2.0");

        // Tamper with tokenizer.json and verify SHA-256 rejection
        fs::write(ver_dir.join("tokenizer.json"), b"{}").unwrap();
        assert!(SenseModel::load(&ver_dir).is_err());
    }
}
