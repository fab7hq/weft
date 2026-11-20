//! A project's earlier sessions as the tree lists them: read from the daemon
//! a page at a time, every one of them or those a search finds.

use serde_json::Value;
use weft_core::sessions::{PAGE, Row};

/// A read to send: the page after the row this key names, or the first.
pub type After = Option<String>;

/// One list of earlier sessions, read a page at a time: every earlier
/// session, or those a search for `q` finds among them all.
#[derive(Debug, Clone, PartialEq)]
pub struct Pages {
    /// What it was searched for; empty for every earlier session.
    pub q: String,
    /// The rows read, newest first.
    pub rows: Vec<Row>,
    /// How many there are, as the daemon last said.
    pub total: usize,
    /// The key the next page comes after; none when every row is read.
    next: Option<String>,
    /// How many rows are wanted: a page, and a page more for each "more".
    wanted: usize,
    /// The read still to be answered, by its call's id.
    asked: Option<u64>,
    /// Rows read again from the start, shown once as many as are wanted
    /// are read.
    reading: Option<Vec<Row>>,
    /// Why the last read failed, until one is sent again.
    pub failed: Option<String>,
    /// The list moved while a read was owed: read again from the start once
    /// it is answered.
    again: bool,
}

impl Default for Pages {
    fn default() -> Self {
        Self::new("")
    }
}

impl Pages {
    pub fn new(q: &str) -> Self {
        Pages {
            q: q.to_string(),
            rows: Vec::new(),
            total: 0,
            next: None,
            wanted: PAGE,
            asked: None,
            reading: None,
            again: false,
            failed: None,
        }
    }

    /// A read is still to be answered.
    pub fn asking(&self) -> bool {
        self.asked.is_some()
    }

    /// Read it again from the start, as many rows as are shown: the read to
    /// send now, if one is to be sent now.
    pub fn restart(&mut self) -> Option<After> {
        if self.asking() {
            self.again = true;
            return None;
        }
        self.reading = Some(Vec::new());
        Some(None)
    }

    /// Show a page more: the read to send now, if one is to be sent now.
    pub fn more(&mut self) -> Option<After> {
        self.wanted = self.rows.len() + PAGE;
        if self.asking() {
            return None;
        }
        self.next.clone().map(Some)
    }

    /// Back to its first page, as it opens again.
    pub fn fold(&mut self) {
        self.wanted = PAGE;
        if self.rows.len() > PAGE {
            self.rows.truncate(PAGE);
            self.next = self.rows.last().map(Row::key);
        }
    }

    /// How many rows the next page adds; none when every row is shown.
    pub fn more_count(&self) -> usize {
        match self.next {
            Some(_) => self.total.saturating_sub(self.rows.len()).min(PAGE),
            None => 0,
        }
    }

    /// The read was sent as call `id`, or could not be sent, or will not be
    /// answered: the daemon went.
    pub fn sent(&mut self, id: Option<u64>) {
        self.asked = id;
        if id.is_some() {
            self.failed = None;
        }
        if id.is_none() {
            (self.reading, self.again) = (None, false);
        }
    }

    /// The daemon would not answer read `id`, and why.
    pub fn refused(&mut self, id: u64, why: &str) {
        if self.asked == Some(id) {
            (self.asked, self.reading, self.again) = (None, None, false);
            self.failed = Some(why.to_string());
        }
    }

    /// The read that failed, sent again: from the start when nothing was
    /// read yet, else the page after the last row read.
    pub fn again(&mut self) -> Option<After> {
        if self.failed.is_none() || self.asking() {
            return None;
        }
        match self.next.clone() {
            Some(next) if !self.rows.is_empty() => Some(Some(next)),
            _ => self.restart(),
        }
    }

    /// The daemon's answer to read `id`: `{rows, total, next}`. The read to
    /// send next, if one is to be sent now. An answer to any other read is
    /// let go.
    pub fn answered(&mut self, id: u64, answer: &Value) -> Option<After> {
        if self.asked != Some(id) {
            return None;
        }
        self.asked = None;
        if std::mem::take(&mut self.again) {
            return self.restart();
        }
        let rows: Vec<Row> = serde_json::from_value(answer["rows"].clone()).unwrap_or_default();
        let next = answer["next"].as_str().map(str::to_string);
        self.total = answer["total"].as_u64().unwrap_or(0) as usize;
        let read = self.reading.as_mut().unwrap_or(&mut self.rows);
        read.extend(rows);
        if read.len() < self.wanted && next.is_some() {
            return Some(next);
        }
        if let Some(read) = self.reading.take() {
            self.rows = read;
        }
        self.next = next;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use weft_core::sessions::State;

    /// The daemon's answer for `total` earlier sessions, `old-<n>` newest
    /// first, the page after `after`.
    fn page(total: usize, after: Option<&str>) -> (Value, After) {
        let key = |i: usize| format!("codex/old-{i}");
        let start = after.map_or(0, |a| (0..total).position(|i| key(i) == a).unwrap() + 1);
        let end = (start + PAGE).min(total);
        let rows: Vec<Row> = (start..end)
            .map(|i| Row::of("codex", &format!("old-{i}"), State::Ended, None, 0))
            .collect();
        let next = (end < total).then(|| key(end - 1));
        (json!({"rows": rows, "total": total, "next": next}), next)
    }

    /// Answer every read `ask` names, as a daemon of `total` would, until
    /// none is left to send.
    fn serve(p: &mut Pages, total: usize, mut ask: Option<After>, id: &mut u64) -> usize {
        let mut reads = 0;
        while let Some(after) = ask {
            *id += 1;
            p.sent(Some(*id));
            reads += 1;
            ask = p.answered(*id, &page(total, after.as_deref()).0);
        }
        reads
    }

    #[test]
    fn a_page_then_a_page_more_until_every_row_is_shown() {
        let (mut p, mut id) = (Pages::new(""), 0);
        let ask = p.restart();
        assert_eq!(ask, Some(None), "the first page");
        serve(&mut p, 120, ask, &mut id);
        assert_eq!((p.rows.len(), p.total, p.more_count()), (50, 120, 50));
        let ask = p.more();
        assert_eq!(ask, Some(Some("codex/old-49".into())));
        serve(&mut p, 120, ask, &mut id);
        assert_eq!((p.rows.len(), p.more_count()), (100, 20), "fewer than a page are left");
        assert_eq!(p.rows[50].id, "old-50", "appended in order");
        let ask = p.more();
        serve(&mut p, 120, ask, &mut id);
        assert_eq!((p.rows.len(), p.more_count()), (120, 0), "all shown: no more");
        assert_eq!(p.more(), None, "nothing past the last");
    }

    #[test]
    fn read_again_it_keeps_as_many_rows_as_were_shown_and_swaps_them_in_at_once() {
        let (mut p, mut id) = (Pages::new(""), 0);
        let ask = p.restart();
        serve(&mut p, 300, ask, &mut id);
        let ask = p.more();
        serve(&mut p, 300, ask, &mut id);
        assert_eq!(p.rows.len(), 100);
        // The list moved: read from the start, the shown rows kept meanwhile.
        let ask = p.restart();
        id += 1;
        p.sent(Some(id));
        let more = p.answered(id, &page(301, ask.unwrap().as_deref()).0);
        assert_eq!(p.rows.len(), 100, "still the old rows until all are read");
        assert!(more.is_some(), "a second page to read");
        serve(&mut p, 301, more, &mut id);
        assert_eq!((p.rows.len(), p.total), (100, 301));
        // Folded and opened again: one page.
        p.fold();
        assert_eq!((p.rows.len(), p.more_count()), (50, 50));
        assert_eq!(p.more(), Some(Some("codex/old-49".into())));
    }

    #[test]
    fn a_move_while_a_read_is_owed_reads_again_once_it_comes() {
        let (mut p, mut id) = (Pages::new(""), 0);
        let ask = p.restart();
        id += 1;
        p.sent(Some(id));
        assert_eq!(p.restart(), None, "owed: not sent twice");
        let again = p.answered(id, &page(60, ask.unwrap().as_deref()).0);
        assert_eq!(again, Some(None), "read again from the start");
        assert!(p.rows.is_empty(), "the stale answer is not shown");
        serve(&mut p, 61, again, &mut id);
        assert_eq!((p.rows.len(), p.total), (50, 61));
        assert_eq!(p.answered(1, &page(60, None).0), None, "an old read is let go");
        assert_eq!(p.rows.len(), 50);
    }

    #[test]
    fn a_read_not_sent_or_refused_leaves_it_as_it_was() {
        let mut p = Pages::new("login");
        let _ = p.restart();
        p.sent(None);
        assert!(!p.asking());
        let ask = p.restart();
        p.sent(Some(7));
        p.refused(7, "no");
        assert!(!p.asking() && p.rows.is_empty() && ask == Some(None));
        assert_eq!(p.restart(), Some(None), "it can be read again");
    }
}
