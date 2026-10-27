//! sense-d's tokenizer: DeBERTa-v3's SentencePiece Unigram, read from
//! `tokenizer.json` and run as the reference `tokenizers` runs it, without
//! special tokens or truncation. Its normalizer turns two or more blanks, or
//! a newline, return or tab, into one space, composes (NFC) and strips the
//! right; its pre-tokenizer is Metaspace (`▁`, always prepended, split).

use anyhow::{Context, bail};
use serde::Deserialize;
use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

/// A question row fits this many tokens (sense-d spec/bundle.md §4).
pub const MAX_ROW: usize = 256;
/// A question keeps up to its first this many tokens.
const MAX_QUESTION: usize = 128;
/// A text too long for its row keeps its start and this many last tokens.
const TEXT_LAST: usize = 32;

/// What a piece no vocabulary entry covers costs, below the cheapest piece.
const UNK_PENALTY: f64 = 10.0;
const METASPACE: char = '▁';

pub struct Tokenizer {
    pieces: HashMap<String, (u32, f64)>,
    longest_piece: usize,
    unk_id: u32,
    unk_score: f64,
    /// Special tokens, matched in the raw text before anything else.
    specials: Vec<(String, u32)>,
}

#[derive(Deserialize)]
struct TokenizerJson {
    added_tokens: Vec<AddedToken>,
    model: UnigramJson,
}

#[derive(Deserialize)]
struct AddedToken {
    id: u32,
    content: String,
    special: bool,
}

#[derive(Deserialize)]
struct UnigramJson {
    #[serde(rename = "type")]
    kind: String,
    unk_id: u32,
    vocab: Vec<(String, f64)>,
}

impl Tokenizer {
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let parsed: TokenizerJson =
            serde_json::from_slice(bytes).context("parse tokenizer.json")?;
        if parsed.model.kind != "Unigram" {
            bail!("tokenizer.json is a {} model, not Unigram", parsed.model.kind);
        }
        let vocab = parsed.model.vocab;
        let unk_score = vocab.iter().map(|(_, s)| *s).fold(f64::INFINITY, f64::min) - UNK_PENALTY;
        let longest_piece = vocab.iter().map(|(p, _)| p.len()).max().unwrap_or(1);
        let pieces =
            vocab.into_iter().enumerate().map(|(id, (p, s))| (p, (id as u32, s))).collect();
        let mut specials: Vec<(String, u32)> = parsed
            .added_tokens
            .into_iter()
            .filter(|t| t.special)
            .map(|t| (t.content, t.id))
            .collect();
        specials.sort_by_key(|(content, _)| std::cmp::Reverse(content.len()));
        Ok(Self { pieces, longest_piece, unk_id: parsed.model.unk_id, unk_score, specials })
    }

    /// The id of a piece, such as `[CLS]`.
    pub fn id(&self, piece: &str) -> anyhow::Result<u32> {
        let found = self.pieces.get(piece).map(|(id, _)| *id);
        found
            .or_else(|| self.specials.iter().find(|(c, _)| c == piece).map(|(_, id)| *id))
            .with_context(|| format!("tokenizer.json has no {piece}"))
    }

    /// `text`'s ids, with no special tokens added and no truncation.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let next = self.next_special(rest);
            let (plain, special) = match next {
                Some((at, len, id)) => (&rest[..at], Some((len, id))),
                None => (rest, None),
            };
            self.encode_plain(plain, &mut ids);
            match special {
                Some((len, id)) => {
                    ids.push(id);
                    rest = &rest[plain.len() + len..];
                }
                None => rest = "",
            }
        }
        ids
    }

    /// The first special token in `text`: where, how long and its id.
    fn next_special(&self, text: &str) -> Option<(usize, usize, u32)> {
        let found = self
            .specials
            .iter()
            .filter_map(|(c, id)| text.find(c.as_str()).map(|at| (at, c.len(), *id)));
        found.min_by_key(|&(at, len, _)| (at, std::cmp::Reverse(len)))
    }

    fn encode_plain(&self, text: &str, ids: &mut Vec<u32>) {
        let normalized = normalize(text);
        if normalized.is_empty() {
            return;
        }
        let mut spaced = normalized.replace(' ', "▁");
        if !spaced.starts_with(METASPACE) {
            spaced.insert(0, METASPACE);
        }
        for word in split_before_metaspace(&spaced) {
            self.viterbi(word, ids);
        }
    }

    /// `[CLS] text [SEP] question [SEP]`, from `text`'s ids: the question
    /// keeps its first 128 tokens, and a text too long for the rest keeps
    /// its start and its last 32.
    pub fn question_row(&self, text: &[u32], question: &str) -> anyhow::Result<Vec<u32>> {
        let (cls, sep) = (self.id("[CLS]")?, self.id("[SEP]")?);
        let mut q = self.encode(question);
        q.truncate(MAX_QUESTION);
        let room = MAX_ROW - q.len() - 3;
        let text = match text.len() > room {
            true => [&text[..room - TEXT_LAST], &text[text.len() - TEXT_LAST..]].concat(),
            false => text.to_vec(),
        };
        Ok([&[cls][..], &text, &[sep], &q, &[sep]].concat())
    }

    /// The best-scoring pieces of `word`, as the reference finds them: the
    /// first path found keeps a tie; consecutive unknown characters fuse.
    fn viterbi(&self, word: &str, ids: &mut Vec<u32>) {
        let n = word.len();
        // Per byte position: the best score ending there, where that piece
        // started, and its id.
        let mut best: Vec<Option<(f64, usize, u32)>> = vec![None; n + 1];
        best[0] = Some((0.0, 0, 0));
        for (start, ch) in word.char_indices() {
            let Some((base, _, _)) = best[start] else { continue };
            let char_end = start + ch.len_utf8();
            let mut has_single = false;
            for end in (start + 1..=n.min(start + self.longest_piece))
                .filter(|&e| word.is_char_boundary(e))
            {
                let Some(&(id, score)) = self.pieces.get(&word[start..end]) else { continue };
                has_single |= end == char_end;
                let candidate = base + score;
                if best[end].is_none_or(|(s, _, _)| candidate > s) {
                    best[end] = Some((candidate, start, id));
                }
            }
            if !has_single {
                let candidate = base + self.unk_score;
                if best[char_end].is_none_or(|(s, _, _)| candidate > s) {
                    best[char_end] = Some((candidate, start, self.unk_id));
                }
            }
        }
        let mut path = Vec::new();
        let mut end = n;
        while end > 0 {
            let Some((_, start, id)) = best[end] else { break };
            let fused = id == self.unk_id && path.last() == Some(&self.unk_id);
            if !fused {
                path.push(id);
            }
            end = start;
        }
        ids.extend(path.into_iter().rev());
    }
}

/// Two or more blanks, or one newline, return or tab, become one space; then
/// NFC; then the right is stripped.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if !ch.is_whitespace() {
            out.push(ch);
            continue;
        }
        let mut run = 1;
        while chars.next_if(|c| c.is_whitespace()).is_some() {
            run += 1;
        }
        let one_kept = run == 1 && !matches!(ch, '\n' | '\r' | '\t');
        out.push(if one_kept { ch } else { ' ' });
    }
    let composed: String = out.nfc().collect();
    composed.trim_end().to_string()
}

/// `text` cut before every `▁`, each piece keeping the `▁` it starts with.
fn split_before_metaspace(text: &str) -> impl Iterator<Item = &str> {
    let mut starts: Vec<usize> = text.match_indices(METASPACE).map(|(at, _)| at).collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    let ends: Vec<usize> = starts.iter().skip(1).copied().chain([text.len()]).collect();
    starts.into_iter().zip(ends).map(move |(s, e)| &text[s..e]).filter(|w| !w.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanks_and_line_breaks_become_one_space_and_the_right_is_stripped() {
        assert_eq!(normalize("hello  world\n\tok "), "hello world ok");
        assert_eq!(normalize("a\nb"), "a b");
        assert_eq!(normalize("a\u{a0}b"), "a\u{a0}b", "one blank that is not a line break stays");
        assert_eq!(normalize("cafe\u{301}"), "caf\u{e9}", "composed");
    }

    #[test]
    fn a_long_text_keeps_its_start_and_last_32_tokens_and_a_question_its_first_128() {
        let json = r#"{"added_tokens": [{"id": 0, "content": "[CLS]", "special": true},
            {"id": 1, "content": "[SEP]", "special": true}],
            "model": {"type": "Unigram", "unk_id": 2,
                      "vocab": [["[CLS]", 0.0], ["[SEP]", 0.0], ["[UNK]", 0.0], ["\u2581q", -1.0]]}}"#;
        let tok = Tokenizer::from_bytes(json.as_bytes()).unwrap();
        let text: Vec<u32> = (100..400).collect();
        let row = tok.question_row(&text, &"q ".repeat(200)).unwrap();
        assert_eq!(row.len(), MAX_ROW);
        let room = MAX_ROW - 128 - 3;
        assert_eq!(row[0], 0);
        assert_eq!(&row[1..room - 32 + 1], &text[..room - 32]);
        assert_eq!(&row[room - 32 + 1..room + 1], &text[300 - 32..]);
        assert_eq!(&row[room + 1..room + 2], &[1]);
        assert_eq!(row[room + 2..MAX_ROW - 1], [3; 128]);
        let short = tok.question_row(&[7, 8], "q").unwrap();
        assert_eq!(short, [0, 7, 8, 1, 3, 1]);
    }

    #[test]
    fn words_are_cut_before_each_metaspace() {
        let words: Vec<&str> = split_before_metaspace("▁how▁does▁it").collect();
        assert_eq!(words, ["▁how", "▁does", "▁it"]);
    }
}
