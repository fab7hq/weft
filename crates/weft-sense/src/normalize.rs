use crate::model::{PaneSnapshot, TranscriptSnippet};

pub const ADDED_TOKENS: &[&str] = &[
    "\n",
    "(<TIME>)",
    "[SURFACE]",
    "[CONV]",
    "[H:claude-code]",
    "[H:codex]",
    "[H:grok]",
    "[H:antigravity]",
    "[H:unknown]",
    "[HOOK:ready]",
    "[HOOK:working]",
    "[HOOK:waiting]",
    "[HOOK:turn_ended]",
    "[HOOK:ended]",
    "[HOOK:none]",
    "[ALT:0]",
    "[ALT:1]",
    "[QUIET:0]",
    "[QUIET:1]",
    "[QUIET:2]",
    "[QUIET:3]",
    "<USER_REQUEST>",
    "</USER_REQUEST>",
    "<ADDITIONAL_METADATA>",
    "</ADDITIONAL_METADATA>",
    "<SYSTEM_MESSAGE>",
    "</SYSTEM_MESSAGE>",
    "<user-prompt-submit-hook>",
    "</user-prompt-submit-hook>",
    "❯",
    "◦",
    "●",
    "⎿",
    "◈",
    "◆",
    "✓",
    "✗",
    "╭",
    "╰",
    "│",
    "─",
    "▶",
    "›",
];

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

pub fn compute_surface_dynamics(snap: &PaneSnapshot<'_>) -> [f32; 6] {
    let rows = snap.rows.max(1);
    let cols = snap.cols.max(1);
    let row_from_bottom = rows.saturating_sub(1).saturating_sub(snap.cursor_row).min(40);
    let q_idx = match quiet_bucket(snap.ms_since_last_output) {
        "0" => 0.0,
        "1" => 1.0,
        "2" => 2.0,
        _ => 3.0,
    };
    let hook = canonical_hook_state(snap.hook_state);
    [
        if snap.alt_screen { 1.0 } else { 0.0 },
        (row_from_bottom as f32) / 40.0,
        ((snap.cursor_col as f32) / (cols as f32)).clamp(0.0, 1.0),
        q_idx / 3.0,
        if hook == "waiting" { 1.0 } else { 0.0 },
        if hook == "turn_ended" { 1.0 } else { 0.0 },
    ]
}

pub fn normalize_surface_frame(snap: &PaneSnapshot<'_>, max_bottom_lines: usize) -> String {
    let harness = canonical_harness(snap.harness);
    let hook_state = canonical_hook_state(snap.hook_state);
    let alt = if snap.alt_screen { "1" } else { "0" };
    let rows = if snap.rows == 0 { 40 } else { snap.rows };
    let row_from_bottom = rows.saturating_sub(1).saturating_sub(snap.cursor_row).min(40);
    let q_bucket = quiet_bucket(snap.ms_since_last_output);

    let non_empty: Vec<&String> =
        snap.bottom_lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let start = non_empty.len().saturating_sub(max_bottom_lines);
    let selected: Vec<String> =
        non_empty[start..].iter().map(|l| normalize_terminal_line(l)).collect();

    let header = format!(
        "[SURFACE][H:{harness}][HOOK:{hook_state}][ALT:{alt}][CUR:{row_from_bottom},{}][QUIET:{q_bucket}]",
        snap.cursor_col
    );
    if selected.is_empty() { header } else { format!("{header}\n{}", selected.join("\n")) }
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

pub fn normalize_conversation_entry(item: &TranscriptSnippet<'_>, max_bytes: usize) -> String {
    let h = canonical_harness(item.harness);
    let t = {
        let trimmed = item.entry_type.trim();
        if trimmed.is_empty() { "unknown" } else { trimmed }
    };
    let mut cleaned = String::with_capacity(item.snippet.len());
    for ch in item.snippet.chars() {
        let cp = ch as u32;
        if matches!(cp, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f) {
            continue;
        }
        cleaned.push(ch);
    }
    let snippet = truncate_utf8_bytes(&cleaned, max_bytes);
    format!("[CONV][H:{h}][TYPE:{t}]\n{snippet}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_normalization_and_dynamics_match_contract() {
        let lines = vec![
            "".to_string(),
            "· Effecting… (12s)".to_string(),
            "────────────────────────".to_string(),
            "❯".to_string(),
            "────────────────────────".to_string(),
            "  ⏸ manual mode on · esc to interrupt · ← for agents".to_string(),
        ];
        let snap = PaneSnapshot {
            harness: "claude-code",
            hook_state: Some("working"),
            alt_screen: true,
            rows: 40,
            cols: 120,
            cursor_row: 37,
            cursor_col: 2,
            ms_since_last_output: 250,
            bottom_lines: &lines,
            seq: 42,
        };
        let norm = normalize_surface_frame(&snap, 15);
        assert_eq!(
            norm,
            concat!(
                "[SURFACE][H:claude-code][HOOK:working][ALT:1][CUR:2,2][QUIET:1]\n",
                "· Effecting… (<TIME>)\n",
                "───\n",
                "❯\n",
                "───\n",
                "  ⏸ manual mode on · esc to interrupt · ← for agents"
            )
        );
        let dyn_vec = compute_surface_dynamics(&snap);
        assert_eq!(dyn_vec[0], 1.0);
        assert!((dyn_vec[1] - (2.0 / 40.0)).abs() < 1e-5);
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
    fn conversation_normalization_strips_control_chars_and_truncates() {
        let item = TranscriptSnippet {
            harness: "antigravity",
            entry_type: "PLANNER_RESPONSE",
            snippet: "ok\x07\x00",
        };
        assert_eq!(
            normalize_conversation_entry(&item, 512),
            "[CONV][H:antigravity][TYPE:PLANNER_RESPONSE]\nok"
        );
    }
}
