//! The documents an Ask names, and the obligations RingFrame reads out of them
//! without a model (ADR-0019 §1, D4).
//!
//! A path in an Ask's prompt that is a file is a document. When the Ask names
//! a part of it ("Phase 3.20 of plans/app/plan.md"), that section is read;
//! each row of a table in it with a "Done when" column is one obligation, and
//! a step range in the same sentence ("steps 2–41") narrows the rows. Without
//! a named part the document is context and yields none.
//!
//! What the agent wrote about its own work ("As built" notes, "Built …"
//! markers) is kept apart: it points retrieval at the evidence and is never
//! given to a judge, because it is the claim being tested.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::digest;
use crate::workspace::Workspace;

const PART_WORDS: [&str; 8] =
    ["phase", "slice", "section", "part", "chapter", "stage", "milestone", "step"];

/// One document an Ask names.
#[derive(Clone, Debug)]
pub struct Document {
    pub id: String,
    pub path: String,
    pub sha256: String,
    /// Each named part: its name, its section as a judge may read it, and the
    /// step range applied.
    pub parts: Vec<Value>,
}

/// One row of a "Done when" table.
#[derive(Clone, Debug)]
pub struct Obligation {
    pub id: String,
    pub document: String,
    pub part: String,
    pub row: String,
    pub text: String,
    pub done_when: String,
    pub line: usize,
    pub ask_id: String,
    /// The agent's own account of this row. For retrieval only.
    pub as_built: String,
}

impl Obligation {
    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "document": self.document, "part": self.part, "row": self.row,
               "text": self.text, "done_when": self.done_when, "line": self.line,
               "ask_id": self.ask_id})
    }
}

pub(crate) fn trim_punct(word: &str) -> &str {
    word.trim_matches(|c: char| {
        matches!(c, '(' | ')' | '[' | ']' | '"' | '\'' | '`' | ',' | ';' | ':' | '“' | '”' | '’')
    })
    .trim_end_matches('.')
}

fn is_number(word: &str) -> bool {
    let w = trim_punct(word);
    w.chars().next().is_some_and(|c| c.is_ascii_digit())
        && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// Sentences, split at a full stop followed by a space and a capital, at a
/// semicolon, and at line ends: "Phase 3.20" does not end one.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let ends = c == '\n'
            || (c == '.'
                && chars.get(i + 1).is_some_and(|n| n.is_whitespace())
                && chars.get(i + 2).is_none_or(|n| n.is_uppercase()));
        if ends {
            if c != '\n' {
                cur.push(c);
            }
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// `steps 2–41`, `steps 2 to 41`, `step order (1 to 8)`, `step 12`.
fn step_range(sentence: &str) -> Option<(u32, u32)> {
    let lower = sentence.to_lowercase();
    for (at, _) in lower.match_indices("step") {
        let tail: String = lower[at..].chars().take(40).collect();
        let digits: Vec<(usize, u32)> = {
            let mut v = Vec::new();
            let b = tail.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i].is_ascii_digit() {
                    let start = i;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    v.push((start, tail[start..i].parse().unwrap_or(0)));
                } else {
                    i += 1;
                }
            }
            v
        };
        if let [(a_at, a), (b_at, b), ..] = digits[..] {
            let between = &tail[a_at..b_at];
            if ["–", "—", "-", " to "].iter().any(|s| between.contains(s)) && a <= b {
                return Some((a, b));
            }
        }
        if let Some(&(_, n)) = digits.first() {
            let before = &tail[..digits[0].0];
            if before.trim() == "step" {
                return Some((n, n));
            }
        }
    }
    None
}

/// A part an Ask names, and the step range it applies to it.
type NamedPart = (String, Option<(u32, u32)>);

/// Every file an Ask's prompt names, with the parts it names in it.
fn named(ws: &Workspace, prompt: &str) -> Vec<(String, Vec<NamedPart>)> {
    let mut out: Vec<(String, Vec<NamedPart>)> = Vec::new();
    for sentence in sentences(prompt) {
        let words: Vec<&str> = sentence.split_whitespace().collect();
        let mut here: Vec<(String, Option<String>)> = Vec::new();
        for (i, w) in words.iter().enumerate() {
            let token = trim_punct(w);
            if !(token.contains('/') || token.ends_with(".md")) || token.contains("://") {
                continue;
            }
            if !ws.root.join(token).is_file() {
                continue;
            }
            let part = (i >= 3
                && trim_punct(words[i - 1]).eq_ignore_ascii_case("of")
                && is_number(words[i - 2])
                && PART_WORDS.contains(&trim_punct(words[i - 3]).to_lowercase().as_str()))
            .then(|| format!("{} {}", trim_punct(words[i - 3]), trim_punct(words[i - 2])));
            here.push((token.to_string(), part));
        }
        let parts_here = here.iter().filter(|(_, p)| p.is_some()).count();
        let range = if parts_here == 1 { step_range(&sentence) } else { None };
        for (path, part) in here {
            let slot = match out.iter().position(|(p, _)| *p == path) {
                Some(i) => i,
                None => {
                    out.push((path.clone(), Vec::new()));
                    out.len() - 1
                }
            };
            if let Some(p) = part
                && !out[slot].1.iter().any(|(q, _)| *q == p)
            {
                out[slot].1.push((p, range));
            }
        }
    }
    out
}

fn heading_level(line: &str) -> Option<usize> {
    let n = line.chars().take_while(|c| *c == '#').count();
    (n > 0 && line.chars().nth(n) == Some(' ')).then_some(n)
}

/// Whether `heading` names `part` as a whole: "Phase 3.2" is not in
/// "Phase 3.20".
fn names_part(heading: &str, part: &str) -> bool {
    let h = heading.to_lowercase();
    let p = part.to_lowercase();
    h.match_indices(&p).any(|(at, _)| {
        let next = h[at + p.len()..].chars().next();
        !next.is_some_and(|c| {
            c.is_ascii_alphanumeric()
                || c == '.' && {
                    h[at + p.len() + 1..].chars().next().is_some_and(|d| d.is_ascii_digit())
                }
        })
    })
}

/// The lines of the section a heading names: to the next heading at its
/// level or above. `(first line index, lines)`.
fn section<'a>(lines: &[&'a str], part: &str) -> Option<(usize, Vec<&'a str>)> {
    let start = lines.iter().position(|l| heading_level(l).is_some() && names_part(l, part))?;
    let level = heading_level(lines[start]).expect("a heading");
    let end = lines[start + 1..]
        .iter()
        .position(|l| heading_level(l).is_some_and(|n| n <= level))
        .map_or(lines.len(), |i| start + 1 + i);
    Some((start, lines[start..end].to_vec()))
}

/// Table cells, split at `|` outside backticks.
fn cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut code = false;
    let mut prev = ' ';
    for c in inner.chars() {
        match c {
            '`' => {
                code = !code;
                cur.push(c);
            }
            '|' if !code && prev != '\\' => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
        prev = c;
    }
    out.push(cur.trim().to_string());
    out
}

/// A cell without the agent's own markers: `**Built 2026-09-27.**` and what
/// follows it in the cell is the agent's account, not the obligation.
fn split_built(cell: &str) -> (String, String) {
    for marker in ["**Built", "**As built", "**Done"] {
        if let Some(at) = cell.find(marker) {
            return (cell[..at].trim().to_string(), cell[at..].trim().to_string());
        }
    }
    (cell.trim().to_string(), String::new())
}

fn row_number(id: &str) -> Option<u32> {
    let digits: String = id.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn clean_id(cell: &str) -> String {
    cell.trim().trim_matches('*').trim_matches('`').trim().to_string()
}

/// "As built" bullets by the row they name: `- **Step 12, …**` or
/// `- **S9.1, …**`.
fn as_built_notes(lines: &[&str]) -> BTreeMap<String, String> {
    let mut notes = BTreeMap::new();
    let mut in_notes: Option<usize> = None;
    let mut cur: Option<(String, String)> = None;
    for l in lines {
        if let Some(level) = heading_level(l) {
            if let Some((k, v)) = cur.take() {
                notes.insert(k, v);
            }
            in_notes = l.to_lowercase().contains("as built").then_some(level);
            continue;
        }
        if in_notes.is_none() {
            continue;
        }
        if let Some(rest) = l.strip_prefix("- **") {
            if let Some((k, v)) = cur.take() {
                notes.insert(k, v);
            }
            let head: String = rest.chars().take_while(|c| !matches!(c, ',' | '*' | ':')).collect();
            let key =
                head.trim().strip_prefix("Step ").unwrap_or(head.trim()).trim_end_matches('.');
            cur = Some((key.to_string(), l.to_string()));
        } else if let Some((_, v)) = cur.as_mut() {
            if l.starts_with(' ') || l.is_empty() {
                v.push('\n');
                v.push_str(l);
            } else {
                let (k, v) = cur.take().expect("a note");
                notes.insert(k, v);
            }
        }
    }
    if let Some((k, v)) = cur {
        notes.insert(k, v);
    }
    notes
}

/// A section as a judge may read it: without its "As built" subsection.
fn readable(lines: &[&str]) -> String {
    let mut out = Vec::new();
    let mut skip: Option<usize> = None;
    for l in lines {
        if let Some(level) = heading_level(l) {
            if skip.is_some_and(|s| level <= s) {
                skip = None;
            }
            if l.to_lowercase().contains("as built") {
                skip = Some(level);
                continue;
            }
        }
        if skip.is_none() {
            out.push(*l);
        }
    }
    out.join("\n")
}

/// The documents the Asks name, their named parts, and the obligations their
/// "Done when" tables hold. `asks` is `(ask_id, prompt text)` in order.
pub fn read(
    ws: &Workspace,
    asks: &[(String, String)],
    limitations: &mut Vec<String>,
) -> (Vec<Document>, Vec<Obligation>) {
    let mut docs: Vec<Document> = Vec::new();
    let mut obligations: Vec<Obligation> = Vec::new();
    for (ask_id, prompt) in asks {
        for (path, parts) in named(ws, prompt) {
            let full = ws.root.join(&path);
            let Ok(text) = std::fs::read_to_string(&full) else { continue };
            let id = format!("d_{}", &digest::sha256_bytes(path.as_bytes())[..12]);
            let slot = match docs.iter().position(|d| d.id == id) {
                Some(i) => i,
                None => {
                    docs.push(Document {
                        id: id.clone(),
                        path: path.clone(),
                        sha256: digest::sha256_bytes(text.as_bytes()),
                        parts: Vec::new(),
                    });
                    docs.len() - 1
                }
            };
            let lines: Vec<&str> = text.lines().collect();
            for (part, range) in parts {
                let Some((first, sec)) = section(&lines, &part) else {
                    limitations.push(format!(
                        "{part} is not a heading in {path}; it yields no obligations"
                    ));
                    continue;
                };
                let notes = as_built_notes(&sec);
                let mut rows = 0;
                let mut i = 0;
                while i < sec.len() {
                    if !sec[i].trim_start().starts_with('|') {
                        i += 1;
                        continue;
                    }
                    let header = cells(sec[i]);
                    let done = header.iter().position(|h| h.to_lowercase().contains("done when"));
                    i += 1;
                    if i < sec.len() && sec[i].contains("---") {
                        i += 1;
                    }
                    while i < sec.len() && sec[i].trim_start().starts_with('|') {
                        let Some(done) = done else {
                            i += 1;
                            continue;
                        };
                        let row = cells(sec[i]);
                        let row_id = clean_id(row.first().map_or("", String::as_str));
                        let in_range = match (range, row_number(&row_id)) {
                            (Some((a, b)), Some(n)) => (a..=b).contains(&n),
                            (Some(_), None) => false,
                            (None, _) => true,
                        };
                        if in_range && !row_id.is_empty() && done < row.len() {
                            let (done_when, built) = split_built(&row[done]);
                            let text = row[1..done].join(" | ");
                            let mut as_built = notes.get(&row_id).cloned().unwrap_or_default();
                            if !built.is_empty() {
                                as_built = format!("{built}\n{as_built}");
                            }
                            obligations.push(Obligation {
                                id: format!("{path}#{part}/{row_id}"),
                                document: id.clone(),
                                part: part.clone(),
                                row: row_id,
                                text: split_built(&text).0,
                                done_when,
                                line: first + i + 1,
                                ask_id: ask_id.clone(),
                                as_built,
                            });
                            rows += 1;
                        }
                        i += 1;
                    }
                }
                docs[slot].parts.push(json!({
                    "part": part, "ask_id": ask_id, "line": first + 1,
                    "range": range.map(|(a, b)| json!([a, b])), "rows": rows,
                    "text": readable(&sec),
                }));
            }
        }
    }
    (docs, obligations)
}

impl Document {
    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "path": self.path, "sha256": self.sha256, "parts": self.parts})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{repo, ws_for};

    const PLAN: &str = "# Plan\n\n## Phase 2 — Tidy\n\nDecided: keep it small.\n\n| # | Step | Done when |\n| --- | --- | --- |\n| 1 | **Commit** the work. | Trees clean. **Built 2026-09-27.** |\n| 2 | The send waits on the tick: `type_it`. | A `tests/detach.rs` test answers. **Built 2026-09-27.** |\n| 3 | Every pane has an id. | Protocol tests pass. |\n| 4 | Nothing else. | n/a |\n\n### As built\n\n- **Step 2, 2026-09-27.** `weft` 4ef780c. `Pane::gone_quiet` decides.\n  More on it.\n- **Step 3, 2026-09-27.** Ids from a counter.\n\n## Phase 20 — Later\n\n| # | Step | Done when |\n| --- | --- | --- |\n| 1 | Not asked for. | x |\n";

    fn with_plan<T>(body: impl FnOnce(&Workspace) -> T) -> T {
        let dir = repo();
        let ws = ws_for(dir.path());
        std::fs::create_dir_all(ws.root.join("plans/x")).unwrap();
        std::fs::write(ws.root.join("plans/x/plan.md"), PLAN).unwrap();
        body(&ws)
    }

    #[test]
    fn a_named_part_and_step_range_yield_exactly_those_rows() {
        with_plan(|ws| {
            let mut lim = Vec::new();
            let asks = vec![(
                "ask_1".to_string(),
                "Build Phase 2 of plans/x/plan.md: steps 2–3; step 1 is built.".to_string(),
            )];
            let (docs, obs) = read(ws, &asks, &mut lim);
            assert_eq!(lim, Vec::<String>::new());
            assert_eq!(docs.len(), 1);
            let ids: Vec<&str> = obs.iter().map(|o| o.id.as_str()).collect();
            assert_eq!(ids, ["plans/x/plan.md#Phase 2/2", "plans/x/plan.md#Phase 2/3"]);
            assert_eq!(obs[0].text, "The send waits on the tick: `type_it`.");
            assert_eq!(obs[0].done_when, "A `tests/detach.rs` test answers.", "no Built marker");
            assert!(
                obs[0].as_built.contains("gone_quiet") && obs[0].as_built.contains("More on it")
            );
            assert!(obs[0].as_built.contains("**Built 2026-09-27.**"));
            let part = &docs[0].parts[0];
            assert_eq!(part["range"], json!([2, 3]));
            let text = part["text"].as_str().unwrap();
            assert!(text.contains("Decided: keep it small."));
            assert!(
                !text.contains("As built") && !text.contains("gone_quiet"),
                "no self-report: {text}"
            );
            assert!(!text.contains("Phase 20"));
        });
    }

    #[test]
    fn a_document_named_without_a_part_yields_nothing() {
        with_plan(|ws| {
            let mut lim = Vec::new();
            let asks =
                vec![("ask_1".to_string(), "Read plans/x/plan.md and tidy things.".to_string())];
            let (docs, obs) = read(ws, &asks, &mut lim);
            assert_eq!(docs.len(), 1);
            assert!(docs[0].parts.is_empty());
            assert!(obs.is_empty());
        });
    }

    #[test]
    fn two_parts_in_one_sentence_take_no_range() {
        with_plan(|ws| {
            std::fs::write(ws.root.join("plans/x/other.md"), "## Slice 9\n\n| Step | What | Done when |\n| --- | --- | --- |\n| S9.1 | Turns | a test |\n").unwrap();
            let mut lim = Vec::new();
            let asks = vec![("a".to_string(), "Implement Phase 2 of plans/x/plan.md, with Slice 9 of plans/x/other.md, in step order (1 to 2).".to_string())];
            let (_, obs) = read(ws, &asks, &mut lim);
            let ids: Vec<&str> = obs.iter().map(|o| o.id.as_str()).collect();
            assert_eq!(ids.len(), 5, "{ids:?}");
            assert!(ids.contains(&"plans/x/other.md#Slice 9/S9.1"));
        });
    }

    #[test]
    fn a_missing_part_is_a_limitation() {
        with_plan(|ws| {
            let mut lim = Vec::new();
            let asks = vec![("a".to_string(), "Build Phase 7 of plans/x/plan.md.".to_string())];
            let (_, obs) = read(ws, &asks, &mut lim);
            assert!(obs.is_empty());
            assert_eq!(
                lim,
                ["Phase 7 is not a heading in plans/x/plan.md; it yields no obligations"]
            );
        });
    }

    #[test]
    fn step_ranges_in_their_usual_shapes() {
        assert_eq!(step_range("steps 2–41, including 4a"), Some((2, 41)));
        assert_eq!(step_range("steps 2 to 41"), Some((2, 41)));
        assert_eq!(step_range("in step order (1 to 8)"), Some((1, 8)));
        assert_eq!(step_range("step 12 only"), Some((12, 12)));
        assert_eq!(step_range("every step's Done when"), None);
    }

    #[test]
    fn a_part_is_named_as_a_whole() {
        assert!(names_part("## Phase 3.20 — Tidy", "Phase 3.20"));
        assert!(!names_part("## Phase 3.20 — Tidy", "Phase 3.2"));
        assert!(!names_part("## Phase 20", "Phase 2"));
    }
}
