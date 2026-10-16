//! Suggestions from the person's own words: the rest of a message they are
//! typing, or what they usually send next after a reply like the agent's last.
//!
//! The entries are what the person sent, each with the agent's reply before it.
//! Nothing here reads them from anywhere or keeps them; the daemon does.

/// One message the person sent.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub text: String,
    /// The agent's reply just before it; empty for a session's first message.
    pub reply: String,
    pub harness: String,
    pub project: String,
    /// When it was sent, in seconds since the epoch.
    pub at: u64,
    /// Where it was read from, `ask:<id>` or `session:<id>`, so one source is read again on its own.
    #[serde(default)]
    pub source: String,
    /// The reply before it as Sense embeds it: what the next prompt is chosen
    /// by. Empty until it is embedded; BM25 stands in meanwhile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub near: Vec<f32>,
}

/// What goes after the caret, and how sure it is (0 to 1).
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub text: String,
    pub score: f64,
}

/// The entries one project sees: its own, then the person's habits from every project.
pub struct Index {
    own: Layer,
    habits: Layer,
}

/// A habit is this short at most, in characters.
const HABIT_LEN: usize = 80;
/// A message longer than this is never suggested.
const MAX_LEN: usize = 500;
/// Recency halves over this many seconds: 14 days.
const HALF_LIFE: f64 = 14.0 * 86_400.0;
/// A message sent to another harness counts this much.
const OTHER_HARNESS: f64 = 0.5;
/// Replies this close by Sense's embedding (cosine) or closer count as alike.
const NEAR: f64 = 0.6;

impl Index {
    pub fn new(entries: &[Entry], project: &str) -> Self {
        let kept: Vec<&Entry> = entries.iter().filter(|e| keep(&e.text)).collect();
        let own = Layer::grouped(kept.iter().copied().filter(|e| e.project == project), |e| {
            fold(&e.text)
        });
        let habits = Layer::grouped(
            kept.iter().copied().filter(|e| e.text.trim().chars().count() <= HABIT_LEN),
            |e| blanked(&e.text),
        );
        let habits = Layer::new(
            habits.items.into_iter().filter(|i| i.projects.len() >= 2 || i.count >= 3).collect(),
        );
        Index { own, habits }
    }

    /// The rest of `draft`, from what was sent before.
    pub fn complete(&self, draft: &str, harness: &str, now: u64) -> Option<Suggestion> {
        if draft.trim().is_empty() {
            return None;
        }
        self.own.complete(draft, harness, now).or_else(|| self.habits.complete(draft, harness, now))
    }

    /// What is usually sent next after a reply like `reply`, compared by
    /// Sense's embedding of it (`near`) where the entries have theirs.
    pub fn next(
        &self,
        reply: &str,
        near: Option<&[f32]>,
        harness: &str,
        now: u64,
    ) -> Option<Suggestion> {
        if words(reply).is_empty() {
            return None;
        }
        self.own
            .next(reply, near, harness, now)
            .or_else(|| self.habits.next(reply, near, harness, now))
    }
}

/// Messages that count as one, sent once or more.
struct Item {
    /// The latest one's words and the reply before it, with its embedding.
    text: String,
    reply: String,
    near: Vec<f32>,
    at: u64,
    count: usize,
    harnesses: Vec<String>,
    projects: Vec<String>,
}

impl Item {
    /// How much this one is worth before how well it matches.
    fn weight(&self, harness: &str, now: u64) -> f64 {
        let age = now.saturating_sub(self.at) as f64;
        let recency = 0.5f64.powf(age / HALF_LIFE);
        let same = if self.harnesses.iter().any(|h| h == harness) { 1.0 } else { OTHER_HARNESS };
        let often = 1.0 - 0.5f64.powi(self.count as i32);
        recency * same * often
    }
}

/// One layer's items, with BM25 over their words and over the replies before them.
struct Layer {
    items: Vec<Item>,
    texts: Bm25,
    replies: Bm25,
}

impl Layer {
    fn new(items: Vec<Item>) -> Self {
        let texts = Bm25::new(items.iter().map(|i| words(&i.text)).collect());
        let replies = Bm25::new(items.iter().map(|i| words(&i.reply)).collect());
        Layer { items, texts, replies }
    }

    /// One item per key, the latest message speaking for it.
    fn grouped<'a>(
        entries: impl Iterator<Item = &'a Entry>,
        key: impl Fn(&Entry) -> String,
    ) -> Self {
        let mut items: Vec<Item> = Vec::new();
        let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for e in entries {
            match at.get(&key(e)).map(|&n| &mut items[n]) {
                Some(i) => {
                    i.count += 1;
                    if e.at > i.at {
                        i.text = e.text.clone();
                        i.reply = e.reply.clone();
                        i.near = e.near.clone();
                        i.at = e.at;
                    }
                    if !i.harnesses.contains(&e.harness) {
                        i.harnesses.push(e.harness.clone());
                    }
                    if !i.projects.contains(&e.project) {
                        i.projects.push(e.project.clone());
                    }
                }
                None => {
                    at.insert(key(e), items.len());
                    items.push(Item {
                        text: e.text.clone(),
                        reply: e.reply.clone(),
                        near: e.near.clone(),
                        at: e.at,
                        count: 1,
                        harnesses: vec![e.harness.clone()],
                        projects: vec![e.project.clone()],
                    });
                }
            }
        }
        Layer::new(items)
    }

    fn complete(&self, draft: &str, harness: &str, now: u64) -> Option<Suggestion> {
        // A message the draft begins wins over one that only shares its words.
        let prefixed = self.best(harness, now, |i, _| {
            after_prefix(&i.text, draft).map(|rest| (rest.to_string(), 1.0))
        });
        if prefixed.is_some() {
            return prefixed;
        }
        let query = words(draft);
        let last = query.last()?.clone();
        let partial = !draft.ends_with(char::is_whitespace);
        self.best(harness, now, |i, n| {
            let rest = after_word(&i.text, &last, partial)?;
            let b = self.texts.score(n, &query);
            Some((rest.to_string(), (b + 1.0) / (b + 2.0)))
        })
    }

    fn next(
        &self,
        reply: &str,
        near: Option<&[f32]>,
        harness: &str,
        now: u64,
    ) -> Option<Suggestion> {
        let query = words(reply);
        self.best(harness, now, |i, n| {
            // By embedding where both have one, else by the reply's words.
            if let Some(q) = near.filter(|q| !i.near.is_empty() && q.len() == i.near.len()) {
                let c = cosine(q, &i.near);
                return (c >= NEAR).then(|| (i.text.clone(), c));
            }
            let b = self.replies.score(n, &query);
            (b > 0.0).then(|| (i.text.clone(), b / (b + 1.0)))
        })
    }

    /// The best match, `matched` giving an item's text and how well it matches.
    fn best(
        &self,
        harness: &str,
        now: u64,
        matched: impl Fn(&Item, usize) -> Option<(String, f64)>,
    ) -> Option<Suggestion> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(n, i)| {
                let (text, sim) = matched(i, n)?;
                (!text.is_empty()).then(|| Suggestion { text, score: sim * i.weight(harness, now) })
            })
            .max_by(|a, b| a.score.total_cmp(&b.score))
    }
}

/// How alike two embeddings are, from -1 to 1.
fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| f64::from(*x) * f64::from(*y)).sum();
    let norm = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    let n = norm(a) * norm(b);
    if n == 0.0 { 0.0 } else { dot / n }
}

/// Okapi BM25 over a fixed set of documents.
struct Bm25 {
    docs: Vec<Vec<String>>,
    avg: f64,
    /// How many documents hold each word.
    df: std::collections::HashMap<String, usize>,
}

impl Bm25 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;

    fn new(docs: Vec<Vec<String>>) -> Self {
        let total: usize = docs.iter().map(Vec::len).sum();
        let avg = if docs.is_empty() { 0.0 } else { total as f64 / docs.len() as f64 };
        let mut df = std::collections::HashMap::new();
        for d in &docs {
            let mut seen: Vec<&String> = Vec::new();
            for w in d {
                if !seen.contains(&w) {
                    seen.push(w);
                    *df.entry(w.clone()).or_insert(0) += 1;
                }
            }
        }
        Bm25 { docs, avg, df }
    }

    fn score(&self, doc: usize, query: &[String]) -> f64 {
        let d = &self.docs[doc];
        if d.is_empty() {
            return 0.0;
        }
        let n = self.docs.len() as f64;
        let mut seen: Vec<&String> = Vec::new();
        let mut sum = 0.0;
        for q in query {
            if seen.contains(&q) {
                continue;
            }
            seen.push(q);
            let tf = d.iter().filter(|w| *w == q).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let df = self.df.get(q).copied().unwrap_or(0) as f64;
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
            let norm = 1.0 - Self::B + Self::B * d.len() as f64 / self.avg;
            sum += idf * tf * (Self::K1 + 1.0) / (tf + Self::K1 * norm);
        }
        sum
    }
}

/// Whether a message may be suggested at all.
pub fn keep(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && t.chars().count() <= MAX_LEN
        && !t.starts_with('/')
        && !text.split_whitespace().any(secret)
}

/// A word that looks like a key, a token or a private key's header.
fn secret(word: &str) -> bool {
    let w = word.trim_matches(|c: char| ".,;:()\"'`".contains(c));
    const PREFIXES: [&str; 7] = ["sk-", "ghp_", "gho_", "github_pat_", "glpat-", "xoxb-", "xoxp-"];
    if PREFIXES.iter().any(|p| w.starts_with(p)) && w.len() >= 16 {
        return true;
    }
    if w.starts_with("AKIA") && w.len() >= 16
        || w.contains("-----BEGIN")
        || w.contains("PRIVATE") && w.contains("-----")
    {
        return true;
    }
    let tokenish = w.chars().all(|c| c.is_ascii_alphanumeric() || "-_+/=".contains(c));
    let digits = w.chars().any(|c| c.is_ascii_digit());
    let letters = w.chars().any(|c| c.is_ascii_alphabetic());
    w.len() >= 24 && tokenish && digits && letters
}

/// The lower-case words of a text, for matching.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A text with case and runs of spaces folded, so messages that differ only in those count as one.
fn fold(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// A text folded with its numbers and paths blanked, so a habit counts across projects.
fn blanked(text: &str) -> String {
    fold(text)
        .split(' ')
        .map(|w| {
            if w.chars().all(|c| c.is_ascii_digit()) {
                "#"
            } else if path(w) {
                "<path>"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A word that names a file: it has a slash, or a name and an extension.
fn path(w: &str) -> bool {
    w.contains('/')
        || w.split_once('.').is_some_and(|(a, b)| {
            !a.is_empty() && !b.is_empty() && b.chars().all(char::is_alphanumeric)
        })
}

/// What `text` has after `draft`, when it begins with it, ignoring case and runs of spaces.
fn after_prefix<'a>(text: &'a str, draft: &str) -> Option<&'a str> {
    let mut t = text.trim_start().char_indices().peekable();
    let base = text.len() - text.trim_start().len();
    let mut d = draft.trim_start().chars().peekable();
    while let Some(dc) = d.next() {
        if dc.is_whitespace() {
            while d.next_if(|c| c.is_whitespace()).is_some() {}
            let mut any = false;
            while t.next_if(|(_, c)| c.is_whitespace()).is_some() {
                any = true;
            }
            if !any {
                return None;
            }
            continue;
        }
        let (_, tc) = t.next()?;
        if !tc.to_lowercase().eq(dc.to_lowercase()) {
            return None;
        }
    }
    let at = t.peek().map_or(text.len(), |(i, _)| base + i);
    Some(&text[at..])
}

/// What `text` has after the first word that is (or, `partial`, begins with) `word`.
fn after_word<'a>(text: &'a str, word: &str, partial: bool) -> Option<&'a str> {
    let lower = text.to_lowercase();
    // Lower-casing can change lengths; only use positions when it did not.
    if lower.len() != text.len() {
        return None;
    }
    let mut from = 0;
    while let Some(at) = lower[from..].find(word) {
        let start = from + at;
        let end = start + word.len();
        let starts = lower[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let ends = lower[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if starts && (partial || ends) {
            let rest = &text[end..];
            return Some(if partial { rest } else { rest.trim_start() });
        }
        from = end;
    }
    None
}

/// The entries a session's transcript gives: each message the person sent,
/// with the agent's whole reply before it. `at` is when the session began, for
/// items with no time of their own.
pub fn from_conversation(
    items: &[crate::conversation::Item],
    harness: &str,
    project: &str,
    session: &str,
    at: u64,
) -> Vec<Entry> {
    use crate::conversation::Kind;
    let mut out = Vec::new();
    let mut reply: Vec<&str> = Vec::new();
    for item in items {
        let text = item.text.as_deref().unwrap_or("").trim();
        match item.kind {
            Kind::Agent if !text.is_empty() => reply.push(text),
            Kind::Person => {
                if !text.is_empty() {
                    out.push(Entry {
                        text: text.to_string(),
                        reply: reply.join("\n"),
                        harness: harness.into(),
                        project: project.into(),
                        at: item.at.map_or(at, |ms| (ms / 1000).max(0) as u64),
                        source: format!("session:{session}"),
                        near: Vec::new(),
                    });
                }
                reply.clear();
            }
            _ => {}
        }
    }
    out
}

/// The agent's whole reply after the person's last message: what the next
/// prompt follows. Empty while nothing has been said since.
pub fn last_reply(items: &[crate::conversation::Item]) -> String {
    use crate::conversation::Kind;
    let from = items.iter().rposition(|i| i.kind == Kind::Person).map_or(0, |n| n + 1);
    items[from..]
        .iter()
        .filter(|i| i.kind == Kind::Agent)
        .filter_map(|i| i.text.as_deref().map(str::trim).filter(|t| !t.is_empty()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// How a replay of the person's own messages scored, at one confidence floor.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Score {
    /// Messages a suggestion could have been shown for.
    pub eligible: usize,
    /// Suggestions shown, at or above the floor.
    pub shown: usize,
    /// Shown, and what the person sent.
    pub hits: usize,
}

impl Score {
    pub fn hit_rate(&self) -> f64 {
        ratio(self.hits, self.shown)
    }
    pub fn wrong_rate(&self) -> f64 {
        ratio(self.shown - self.hits, self.shown)
    }
    pub fn coverage(&self) -> f64 {
        ratio(self.shown, self.eligible)
    }
}

fn ratio(a: usize, b: usize) -> f64 {
    if b == 0 { 0.0 } else { a as f64 / b as f64 }
}

/// Replay the person's messages in the order they were sent, each suggested
/// from those before it only: the next prompt when the field is empty, and the
/// rest of the message from its first third. One score per floor in `floors`.
pub fn replay(entries: &[Entry], floors: &[f64]) -> (Vec<Score>, Vec<Score>) {
    let mut order: Vec<&Entry> = entries.iter().collect();
    order.sort_by_key(|e| e.at);
    let (mut next, mut rest) =
        (vec![Score::default(); floors.len()], vec![Score::default(); floors.len()]);
    for (k, e) in order.iter().enumerate() {
        let before: Vec<Entry> = order[..k].iter().map(|x| (*x).clone()).collect();
        let index = Index::new(&before, &e.project);
        if !e.reply.trim().is_empty() {
            let near = (!e.near.is_empty()).then_some(e.near.as_slice());
            let got = index.next(&e.reply, near, &e.harness, e.at);
            tally(&mut next, floors, got.map(|s| (fold(&s.text) == fold(&e.text), s.score)));
        }
        if let Some(draft) = first_third(&e.text) {
            let got = index.complete(draft, &e.harness, e.at);
            tally(
                &mut rest,
                floors,
                got.map(|s| (fold(&format!("{draft}{}", s.text)) == fold(&e.text), s.score)),
            );
        }
    }
    (next, rest)
}

/// Count one message: shown at each floor its score reaches, and whether it was right.
fn tally(scores: &mut [Score], floors: &[f64], got: Option<(bool, f64)>) {
    for (score, floor) in scores.iter_mut().zip(floors) {
        score.eligible += 1;
        if let Some((hit, s)) = got
            && s >= *floor
        {
            score.shown += 1;
            score.hits += usize::from(hit);
        }
    }
}

/// A message's first third, to the end of the word it reaches: what a
/// replay types before asking for the rest. None when that is all of it.
fn first_third(text: &str) -> Option<&str> {
    let text = text.trim();
    let third = text.char_indices().nth(text.chars().count() / 3).map_or(text.len(), |(i, _)| i);
    let end = text[third..].find(char::is_whitespace).map_or(text.len(), |n| third + n);
    let draft = text[..end].trim_end();
    (!draft.is_empty() && draft.len() < text.len()).then_some(draft)
}

/// The index as it is kept on the person's machine: one project's entries.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stored {
    pub version: u32,
    pub project: String,
    pub entries: Vec<Entry>,
}

impl Stored {
    /// The version written now. A file of another version is rebuilt from its sources.
    pub const VERSION: u32 = 1;

    pub fn new(project: &str, entries: Vec<Entry>) -> Self {
        Stored { version: Self::VERSION, project: project.into(), entries }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The file's index, unless it is of another version, another project, or not one at all.
    pub fn from_json(text: &str, project: &str) -> Option<Self> {
        let s: Stored = serde_json::from_str(text).ok()?;
        (s.version == Self::VERSION && s.project == project).then_some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;
    const NOW: u64 = 1_800_000_000;

    fn sent(text: &str, reply: &str, harness: &str, project: &str, days_ago: u64) -> Entry {
        Entry {
            text: text.into(),
            reply: reply.into(),
            harness: harness.into(),
            project: project.into(),
            at: NOW - days_ago * DAY,
            source: String::new(),
            near: Vec::new(),
        }
    }

    #[test]
    fn long_messages_slash_commands_and_secrets_are_never_suggested() {
        assert!(keep("run the login tests"));
        assert!(keep("line one\nline two"));
        assert!(!keep(&"a".repeat(501)));
        assert!(keep(&"a".repeat(500)));
        assert!(!keep("/rf:eval debate"));
        assert!(!keep("  /compact"));
        assert!(!keep("use sk-ant-api03-abcdefghijklmnop for it"));
        assert!(!keep("token ghp_16C7e42F292c6912E7710c838347Ae178B4a"));
        assert!(!keep("key AKIAIOSFODNN7EXAMPLE"));
        assert!(!keep("it is 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"));
        assert!(!keep("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(!keep("   "));
        // A long word that is a word is not a key.
        assert!(keep("internationalisation_of_everything_everywhere"));
    }

    #[test]
    fn a_draft_is_completed_from_a_message_it_begins_ignoring_case_and_spaces() {
        let ix = Index::new(
            &[sent("Run the login tests and paste the failures", "", "claude", "weft", 1)],
            "weft",
        );
        let s = ix.complete("run  the LOGIN", "claude", NOW).unwrap();
        assert_eq!(s.text, " tests and paste the failures");
        assert!(s.score > 0.0 && s.score <= 1.0);
        // Nothing to add to a message already typed in full.
        assert_eq!(ix.complete("Run the login tests and paste the failures", "claude", NOW), None);
        assert_eq!(ix.complete("", "claude", NOW), None);
    }

    #[test]
    fn without_a_prefix_the_best_bm25_match_continues_after_the_drafts_last_word() {
        let ix = Index::new(
            &[
                sent("then run the login tests and paste the failures", "", "claude", "weft", 1),
                sent("update the readme", "", "claude", "weft", 1),
            ],
            "weft",
        );
        assert_eq!(
            ix.complete("please run the log", "claude", NOW).unwrap().text,
            "in tests and paste the failures"
        );
        assert_eq!(ix.complete("now the readme", "claude", NOW), None, "nothing comes after it");
        assert_eq!(ix.complete("zebra", "claude", NOW), None);
    }

    #[test]
    fn ranking_favours_the_recent_the_same_harness_and_the_frequent() {
        let older = Index::new(
            &[
                sent("fix the build now", "", "claude", "weft", 30),
                sent("fix the bug first", "", "claude", "weft", 1),
            ],
            "weft",
        );
        assert_eq!(older.complete("fix the", "claude", NOW).unwrap().text, " bug first");

        let harness = Index::new(
            &[
                sent("fix the build now", "", "codex", "weft", 1),
                sent("fix the bug first", "", "claude", "weft", 1),
            ],
            "weft",
        );
        assert_eq!(harness.complete("fix the", "claude", NOW).unwrap().text, " bug first");
        assert_eq!(harness.complete("fix the", "codex", NOW).unwrap().text, " build now");

        let often = Index::new(
            &[
                sent("fix the build now", "", "claude", "weft", 2),
                sent("fix the build now", "", "claude", "weft", 2),
                sent("fix the build now", "", "claude", "weft", 2),
                sent("fix the bug first", "", "claude", "weft", 1),
            ],
            "weft",
        );
        assert_eq!(often.complete("fix the", "claude", NOW).unwrap().text, " build now");
    }

    #[test]
    fn recency_halves_every_fourteen_days() {
        let fresh = Index::new(&[sent("fix the bug first", "", "claude", "weft", 0)], "weft");
        let old = Index::new(&[sent("fix the bug first", "", "claude", "weft", 14)], "weft");
        let a = fresh.complete("fix the", "claude", NOW).unwrap().score;
        let b = old.complete("fix the", "claude", NOW).unwrap().score;
        assert!((b / a - 0.5).abs() < 1e-9, "{a} {b}");
    }

    #[test]
    fn the_next_prompt_follows_the_reply_most_like_the_agents_last() {
        let ix = Index::new(
            &[
                sent("commit these changes", "Done. All the tests pass.", "claude", "weft", 1),
                sent(
                    "try again with logging on",
                    "The build failed with a linker error.",
                    "claude",
                    "weft",
                    1,
                ),
            ],
            "weft",
        );
        assert_eq!(
            ix.next("Done. The login tests pass.", None, "claude", NOW).unwrap().text,
            "commit these changes"
        );
        assert_eq!(
            ix.next("The linker failed again.", None, "claude", NOW).unwrap().text,
            "try again with logging on"
        );
        assert_eq!(ix.next("Something else entirely.", None, "claude", NOW), None);
        assert_eq!(ix.next("", None, "claude", NOW), None);
    }

    #[test]
    fn filtered_messages_are_left_out_of_the_index() {
        let ix = Index::new(
            &[sent("/rf:eval debate evl_2", "Done. All the tests pass.", "claude", "weft", 1)],
            "weft",
        );
        assert_eq!(ix.complete("/rf", "claude", NOW), None);
        assert_eq!(ix.next("Done. All the tests pass.", None, "claude", NOW), None);
    }

    #[test]
    fn another_projects_long_message_never_shows_but_a_habit_does_below_the_projects_own() {
        let long = "rewrite the payment reconciliation job so it batches by merchant and retries each batch on its own";
        let entries = [
            sent(long, "", "claude", "billing", 1),
            sent("commit and push", "", "claude", "billing", 1),
            sent("commit and push", "", "claude", "docs", 1),
            sent("commit the lockfile too", "", "claude", "weft", 3),
        ];
        let ix = Index::new(&entries, "weft");
        assert_eq!(ix.complete("rewrite the payment", "claude", NOW), None);
        // Sent in two projects: a habit everywhere, but the project's own comes first.
        assert_eq!(ix.complete("commit", "claude", NOW).unwrap().text, " the lockfile too");
        let fresh = Index::new(&entries, "fresh");
        assert_eq!(fresh.complete("commit", "claude", NOW).unwrap().text, " and push");
    }

    #[test]
    fn a_habit_is_short_and_sent_in_two_projects_or_three_times() {
        let thrice = [
            sent("run the tests", "", "claude", "a", 1),
            sent("run the tests", "", "claude", "a", 2),
            sent("run the tests", "", "claude", "a", 3),
        ];
        assert!(Index::new(&thrice, "b").complete("run", "claude", NOW).is_some());
        let twice = [
            sent("run the tests", "", "claude", "a", 1),
            sent("run the tests", "", "claude", "a", 2),
        ];
        assert!(Index::new(&twice, "b").complete("run", "claude", NOW).is_none());
        let long = "x".repeat(40) + " " + &"y".repeat(40);
        let wide = [sent(&long, "", "claude", "a", 1), sent(&long, "", "claude", "c", 1)];
        assert!(Index::new(&wide, "b").complete("xxx", "claude", NOW).is_none());
    }

    #[test]
    fn numbers_and_paths_are_blanked_when_habits_are_counted() {
        let entries = [
            sent("look at issue 41", "", "claude", "a", 1),
            sent("look at issue 97", "", "claude", "c", 2),
            sent("open src/main.rs", "", "claude", "a", 1),
            sent("open lib/app.ts", "", "claude", "c", 2),
        ];
        let ix = Index::new(&entries, "b");
        // The same habit: the latest one is offered.
        assert_eq!(ix.complete("look at", "claude", NOW).unwrap().text, " issue 41");
        assert_eq!(ix.complete("open", "claude", NOW).unwrap().text, " src/main.rs");
    }

    #[test]
    fn the_kept_index_reads_back_and_an_old_or_foreign_one_does_not() {
        let kept = Stored::new("weft", vec![sent("run the tests", "Done.", "claude", "weft", 1)]);
        let text = kept.to_json();
        assert_eq!(Stored::from_json(&text, "weft"), Some(kept.clone()));
        assert_eq!(Stored::from_json(&text, "docs"), None, "another project's");
        let old = text.replace("\"version\":1", "\"version\":0");
        assert_ne!(old, text);
        assert_eq!(Stored::from_json(&old, "weft"), None, "an old version is rebuilt");
        assert_eq!(Stored::from_json("not json", "weft"), None);
    }

    #[test]
    fn a_transcript_gives_each_message_with_the_whole_reply_before_it() {
        use crate::conversation::{Item, Kind};
        let item = |kind, text: &str, at: Option<i64>| Item {
            id: String::new(),
            kind,
            at,
            text: Some(text.into()),
            tool: None,
            arg: None,
            call: None,
            failed: None,
        };
        let items = [
            item(Kind::Person, "fix the login", Some(1_000_000)),
            item(Kind::Agent, "Reading it.", None),
            item(Kind::Step, "Read src/login.rs", None),
            item(Kind::Agent, "Done. Both tests pass.", None),
            item(Kind::Person, "commit these changes", Some(2_000_000)),
            item(Kind::Person, "", None),
        ];
        let got = from_conversation(&items, "claude", "/work/weft", "8b2e", 7);
        assert_eq!(got.len(), 2, "an empty message is no entry");
        assert_eq!(
            (got[0].text.as_str(), got[0].reply.as_str(), got[0].at),
            ("fix the login", "", 1000)
        );
        assert_eq!(got[1].reply, "Reading it.\nDone. Both tests pass.");
        assert_eq!(
            (got[1].at, got[1].source.as_str(), got[1].harness.as_str()),
            (2000, "session:8b2e", "claude")
        );
        let untimed =
            from_conversation(&[item(Kind::Person, "hi", None)], "claude", "/work/weft", "8b2e", 7);
        assert_eq!(untimed[0].at, 7);
        assert_eq!(last_reply(&items[..4]), "Reading it.\nDone. Both tests pass.");
        assert_eq!(last_reply(&items), "", "nothing said since the last message");
    }

    #[test]
    fn with_embeddings_the_next_prompt_follows_the_nearest_reply_whatever_its_words() {
        let with = |text: &str, reply: &str, near: Vec<f32>| Entry {
            near,
            ..sent(text, reply, "claude", "weft", 1)
        };
        let ix = Index::new(
            &[
                with("commit these changes", "Every check is green.", vec![1.0, 0.0, 0.0]),
                with("try again with logging on", "The linker gave up.", vec![0.0, 1.0, 0.0]),
            ],
            "weft",
        );
        // No word in common, but the embeddings are close.
        let green = [0.9, 0.1, 0.0];
        assert_eq!(
            ix.next("All tests pass now.", Some(&green), "claude", NOW).unwrap().text,
            "commit these changes"
        );
        // Far from both: nothing, however many words it shares.
        let far = [0.0, 0.0, 1.0];
        assert_eq!(ix.next("Every check is green.", Some(&far), "claude", NOW), None);
        // An entry not embedded yet is compared by its words.
        let plain = Index::new(
            &[sent("commit these changes", "Every check is green.", "claude", "weft", 1)],
            "weft",
        );
        assert_eq!(
            plain.next("Every check is green.", Some(&green), "claude", NOW).unwrap().text,
            "commit these changes"
        );
    }

    #[test]
    fn a_replay_suggests_each_message_from_those_before_it_and_scores_each_floor() {
        let e =
            |text: &str, reply: &str, days_ago: u64| sent(text, reply, "claude", "weft", days_ago);
        let entries = [
            e("run the login tests", "", 5),
            e("commit these changes", "Done. All tests pass.", 4),
            e("run the login tests", "", 3),
            e("commit these changes", "Done. All tests pass.", 2),
            e("open a pull request", "Done. All tests pass.", 1),
        ];
        let (next, rest) = replay(&entries, &[0.0, 0.99]);
        // Next prompt: three had a reply before them; the 4th is foretold by the 2nd, the 5th wrongly by them.
        assert_eq!(next[0], Score { eligible: 3, shown: 2, hits: 1 });
        assert_eq!((next[0].hit_rate(), next[0].wrong_rate()), (0.5, 0.5));
        assert!((next[0].coverage() - 2.0 / 3.0).abs() < 1e-9);
        // The rest from the first third: the 3rd and the 4th are what was sent before.
        assert_eq!(rest[0].eligible, 5);
        assert_eq!(rest[0].hits, 2);
        assert!(rest[0].shown >= 2);
        // A floor no score reaches shows nothing.
        assert_eq!((next[1].shown, rest[1].shown), (0, 0));
        assert_eq!(replay(&[], &[0.0]), (vec![Score::default()], vec![Score::default()]));
    }
}
