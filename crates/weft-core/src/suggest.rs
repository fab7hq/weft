//! Suggestions from the person's own words: the rest of a message they are
//! typing, or what they usually send next after a reply like the agent's last.
//!
//! The entries are what the person sent, each with the agent's reply before it.
//! Nothing here reads them from anywhere or keeps them; the daemon does.

/// One message the person sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub text: String,
    /// The agent's reply just before it; empty for a session's first message.
    pub reply: String,
    pub harness: String,
    pub project: String,
    /// When it was sent, in seconds since the epoch.
    pub at: u64,
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

    /// What is usually sent next after a reply like `reply`.
    pub fn next(&self, reply: &str, harness: &str, now: u64) -> Option<Suggestion> {
        if words(reply).is_empty() {
            return None;
        }
        self.own.next(reply, harness, now).or_else(|| self.habits.next(reply, harness, now))
    }
}

/// Messages that count as one, sent once or more.
struct Item {
    /// The latest one's words and the reply before it.
    text: String,
    reply: String,
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
        let mut items: Vec<(String, Item)> = Vec::new();
        for e in entries {
            let k = key(e);
            match items.iter_mut().find(|(have, _)| *have == k) {
                Some((_, i)) => {
                    i.count += 1;
                    if e.at > i.at {
                        i.text = e.text.clone();
                        i.reply = e.reply.clone();
                        i.at = e.at;
                    }
                    if !i.harnesses.contains(&e.harness) {
                        i.harnesses.push(e.harness.clone());
                    }
                    if !i.projects.contains(&e.project) {
                        i.projects.push(e.project.clone());
                    }
                }
                None => items.push((
                    k,
                    Item {
                        text: e.text.clone(),
                        reply: e.reply.clone(),
                        at: e.at,
                        count: 1,
                        harnesses: vec![e.harness.clone()],
                        projects: vec![e.project.clone()],
                    },
                )),
            }
        }
        Layer::new(items.into_iter().map(|(_, i)| i).collect())
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

    fn next(&self, reply: &str, harness: &str, now: u64) -> Option<Suggestion> {
        let query = words(reply);
        self.best(harness, now, |i, n| {
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

/// Okapi BM25 over a fixed set of documents.
struct Bm25 {
    docs: Vec<Vec<String>>,
    avg: f64,
}

impl Bm25 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;

    fn new(docs: Vec<Vec<String>>) -> Self {
        let total: usize = docs.iter().map(Vec::len).sum();
        let avg = if docs.is_empty() { 0.0 } else { total as f64 / docs.len() as f64 };
        Bm25 { docs, avg }
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
            let df = self.docs.iter().filter(|d| d.contains(q)).count() as f64;
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
            ix.next("Done. The login tests pass.", "claude", NOW).unwrap().text,
            "commit these changes"
        );
        assert_eq!(
            ix.next("The linker failed again.", "claude", NOW).unwrap().text,
            "try again with logging on"
        );
        assert_eq!(ix.next("Something else entirely.", "claude", NOW), None);
        assert_eq!(ix.next("", "claude", NOW), None);
    }

    #[test]
    fn filtered_messages_are_left_out_of_the_index() {
        let ix = Index::new(
            &[sent("/rf:eval debate evl_2", "Done. All the tests pass.", "claude", "weft", 1)],
            "weft",
        );
        assert_eq!(ix.complete("/rf", "claude", NOW), None);
        assert_eq!(ix.next("Done. All the tests pass.", "claude", NOW), None);
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
}
