//! Text as sense-d reads it (sense-d spec/bundle.md §4,
//! `fab7.sense.decision.normalize/1`): a screen as its facts in words, then
//! its last lines, cleaned; a transcript entry as its harness and kind, then
//! its text; then every character the manifest's table names replaced.

use crate::model::{PaneSnapshot, TranscriptSnippet};
use anyhow::bail;
use std::collections::{BTreeMap, HashMap};

/// An entry keeps this many bytes of its text.
const ENTRY_BYTES: usize = 512;

/// The manifest's normalizing table: each character and what it becomes.
#[derive(Debug, Clone, Default)]
pub struct Replacements(HashMap<char, String>);

impl Replacements {
    /// The table, refused if a key is not one character.
    pub fn new(table: &BTreeMap<String, String>) -> anyhow::Result<Self> {
        let mut out = HashMap::new();
        for (from, to) in table {
            let mut chars = from.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                bail!("the normalizing table replaces {from:?}, not one character");
            };
            out.insert(ch, to.clone());
        }
        Ok(Self(out))
    }

    fn apply(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            match self.0.get(&ch) {
                Some(to) => out.push_str(to),
                None => out.push(ch),
            }
        }
        out
    }
}

pub fn canonical_harness(raw: &str) -> &'static str {
    match raw.trim() {
        "claude-code" | "claude" => "claude-code",
        "codex" => "codex",
        "grok" | "grok-build" => "grok",
        "antigravity" | "agy" => "antigravity",
        _ => "unknown",
    }
}

pub fn canonical_hook_state(raw: Option<&str>) -> &'static str {
    match raw.map(str::trim) {
        Some("ready") => "ready",
        Some("working") => "working",
        Some("waiting") => "waiting",
        Some("turn_ended") | Some("turn-ended") => "turn_ended",
        Some("ended") | Some("exited") => "ended",
        _ => "none",
    }
}

pub fn quiet_bucket(ms_since_last_output: u64) -> &'static str {
    if ms_since_last_output < 150 {
        "0"
    } else if ms_since_last_output <= 1000 {
        "1"
    } else if ms_since_last_output <= 5000 {
        "2"
    } else {
        "3"
    }
}

fn is_hrule_char(c: char) -> bool {
    matches!(c, '─' | '━' | '═')
}

fn is_block_art_char(c: char) -> bool {
    matches!(c, '▀' | '▄' | '█' | '▌' | '▐' | '░' | '▒' | '▓')
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn replace_paren_elapsed(input: &str) -> String {
    // Replace `\((?:\d+m\s*)?\d+s\)` with `(<TIME>)`
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            let mut j = i + 1;
            let d1_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > d1_start && j < bytes.len() {
                if bytes[j] == b'm' {
                    j += 1;
                    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    let d2_start = j;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j > d2_start
                        && j + 1 < bytes.len()
                        && bytes[j] == b's'
                        && bytes[j + 1] == b')'
                    {
                        out.push_str("(<TIME>)");
                        i = j + 2;
                        continue;
                    }
                } else if bytes[j] == b's' && j + 1 < bytes.len() && bytes[j + 1] == b')' {
                    out.push_str("(<TIME>)");
                    i = j + 2;
                    continue;
                }
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_bare_min_sec(input: &str) -> String {
    // Replace `\b\d+m\s+\d+s\b` with `(<TIME>)`
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let at_word_start = i == 0 || !is_word_byte(bytes[i - 1]);
        if at_word_start && bytes[i].is_ascii_digit() {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'm' {
                j += 1;
                let ws_start = j;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j > ws_start {
                    let d2_start = j;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j > d2_start && j < bytes.len() && bytes[j] == b's' {
                        let at_word_end = j + 1 == bytes.len() || !is_word_byte(bytes[j + 1]);
                        if at_word_end {
                            out.push_str("(<TIME>)");
                            i = j + 1;
                            continue;
                        }
                    }
                }
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_status_paren_sec(input: &str) -> String {
    // Replace `(?<=\()\d+s(?=\s*[·•])` with `<TIME>`
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if i > 0 && bytes[i - 1] == b'(' && bytes[i].is_ascii_digit() {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b's' {
                let mut k = j + 1;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if k < bytes.len() {
                    let rem = &input[k..];
                    if rem.starts_with('·') || rem.starts_with('•') {
                        out.push_str("<TIME>");
                        i = j + 1;
                        continue;
                    }
                }
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

pub fn normalize_terminal_line(line: &str) -> String {
    // 1. Braille spinner characters [\u2800-\u28ff] -> '*'
    let mut s1 = String::with_capacity(line.len());
    for ch in line.chars() {
        if ('\u{2800}'..='\u{28ff}').contains(&ch) {
            s1.push('*');
        } else {
            s1.push(ch);
        }
    }

    // 2. Elapsed counters
    let s2 = replace_paren_elapsed(&s1);
    let s3 = replace_bare_min_sec(&s2);
    let s4 = replace_status_paren_sec(&s3);

    // 3. Collapse runs of horizontal rules (4+ -> "───"), block art (2+ -> "▀"), and spaces/tabs (4+ -> "  ")
    let chars: Vec<char> = s4.chars().collect();
    let mut out = String::with_capacity(s4.len());
    let mut idx = 0;
    while idx < chars.len() {
        let c = chars[idx];
        if is_hrule_char(c) {
            let mut end = idx + 1;
            while end < chars.len() && is_hrule_char(chars[end]) {
                end += 1;
            }
            if end - idx >= 4 {
                out.push_str("───");
            } else {
                for &ch in &chars[idx..end] {
                    out.push(ch);
                }
            }
            idx = end;
        } else if is_block_art_char(c) {
            let mut end = idx + 1;
            while end < chars.len() && is_block_art_char(chars[end]) {
                end += 1;
            }
            if end - idx >= 2 {
                out.push('▀');
            } else {
                for &ch in &chars[idx..end] {
                    out.push(ch);
                }
            }
            idx = end;
        } else if c == ' ' || c == '\t' {
            let mut end = idx + 1;
            while end < chars.len() && (chars[end] == ' ' || chars[end] == '\t') {
                end += 1;
            }
            if end - idx >= 4 {
                out.push_str("  ");
            } else {
                for &ch in &chars[idx..end] {
                    out.push(ch);
                }
            }
            idx = end;
        } else {
            out.push(c);
            idx += 1;
        }
    }
    out
}

/// A screen as sense-d reads it: its facts in words (the harness, the hook
/// state, the hook that fired last, the alternate screen, the cursor from
/// the bottom, how long it has been quiet), then its last `last_lines`
/// lines that hold anything, cleaned.
pub fn screen_text(snap: &PaneSnapshot<'_>, last_lines: usize, table: &Replacements) -> String {
    let rows = if snap.rows == 0 { 40 } else { snap.rows };
    let row_from_bottom = rows.saturating_sub(1).saturating_sub(snap.cursor_row).min(40);
    let hook = canonical_hook_state(snap.hook_state).replace('_', " ");
    let mut facts = vec![canonical_harness(snap.harness).to_string(), format!("hook {hook}")];
    if let Some(last) = snap.last_hook.filter(|h| !h.is_empty()) {
        facts.push(format!("last hook {}", spaced_words(last)));
    }
    if snap.alt_screen {
        facts.push("alternate screen".into());
    }
    facts.push(format!("cursor {row_from_bottom} {}", snap.cursor_col));
    facts.push(format!("quiet {}", quiet_bucket(snap.ms_since_last_output)));
    let held: Vec<&String> = snap.bottom_lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let lines =
        held[held.len().saturating_sub(last_lines)..].iter().map(|l| normalize_terminal_line(l));
    let text: Vec<String> = std::iter::once(facts.join(", ")).chain(lines).collect();
    table.apply(&text.join("\n"))
}

/// A transcript entry as sense-d reads it: its harness and kind in words,
/// then its text without control characters, cut to 512 bytes.
pub fn entry_text(item: &TranscriptSnippet<'_>, table: &Replacements) -> String {
    let kind = item.entry_type.trim();
    let kind = if kind.is_empty() { "unknown" } else { kind }.replace('_', " ").to_lowercase();
    let cleaned: String = item.snippet.chars().filter(|c| !is_control(*c)).collect();
    let text = truncate_utf8_bytes(&cleaned, ENTRY_BYTES);
    table.apply(&format!("{}, {kind}\n{text}", canonical_harness(item.harness)))
}

/// A hook event's name in lower-case words: `PostToolUse` is `post tool use`.
fn spaced_words(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in name.chars().enumerate() {
        if i > 0 && ch.is_ascii_uppercase() {
            out.push(' ');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

fn is_control(ch: char) -> bool {
    matches!(ch as u32, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f)
}

pub fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Replacements {
        let pairs = [("\n", " ↵ "), ("…", "..."), ("⏸", "[pause]"), ("\u{fffd}", "")];
        Replacements::new(&pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect())
            .unwrap()
    }

    #[test]
    fn a_screen_is_its_facts_in_words_then_its_last_lines_cleaned() {
        let lines = vec![
            "".to_string(),
            "· Effecting… (12s)".to_string(),
            "────────────────────────".to_string(),
            "❯".to_string(),
            "  ⏸ manual mode on".to_string(),
        ];
        let snap = PaneSnapshot {
            harness: "claude-code",
            hook_state: Some("turn_ended"),
            last_hook: Some("PostToolUse"),
            alt_screen: true,
            rows: 40,
            cols: 120,
            cursor_row: 37,
            cursor_col: 2,
            ms_since_last_output: 250,
            bottom_lines: &lines,
            seq: 42,
        };
        assert_eq!(
            screen_text(&snap, 3, &table()),
            "claude-code, hook turn ended, last hook post tool use, alternate screen, \
             cursor 2 2, quiet 1 ↵ ─── ↵ ❯ ↵   [pause] manual mode on"
        );
        let quiet = PaneSnapshot { last_hook: None, alt_screen: false, ..snap };
        assert!(screen_text(&quiet, 15, &table()).starts_with(
            "claude-code, hook turn ended, cursor 2 2, quiet 1 ↵ · Effecting... (<TIME>)"
        ));
    }

    #[test]
    fn terminal_line_normalizes_spinners_elapsed_and_block_art() {
        assert_eq!(
            normalize_terminal_line(
                "◦ Verifying goal completion with judge… (14s · esc to interrupt)"
            ),
            "◦ Verifying goal completion with judge… (<TIME> · esc to interrupt)"
        );
        assert_eq!(
            normalize_terminal_line("⠋ Running for 3m 14s    ████████"),
            "* Running for (<TIME>)  ▀"
        );
    }

    #[test]
    fn an_entry_is_its_harness_and_kind_in_words_then_its_text() {
        let item = TranscriptSnippet {
            harness: "antigravity",
            entry_type: "PLANNER_RESPONSE",
            snippet: "ok\x07\x00\u{fffd}…",
        };
        assert_eq!(entry_text(&item, &table()), "antigravity, planner response ↵ ok...");
    }

    #[test]
    fn a_table_key_of_more_than_one_character_is_refused() {
        let bad = [("ab".to_string(), "x".to_string())].into_iter().collect();
        assert!(Replacements::new(&bad).is_err());
    }
}
