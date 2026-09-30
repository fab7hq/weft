//! The Eval view: one Eval's results, requirements first, read level by
//! level. What each row means is weft-core's; this keeps the place the person
//! is at and asks the daemon for what to show.

use super::*;
use weft_core::eval_view::{EvalView, Mode, Row as ViewRow, RowKind, hunk_lines, rows, step_rows};

/// One open level of the view.
#[derive(Debug, Clone, PartialEq)]
pub enum Layer {
    /// The steps of each part, or the changes of each file.
    List { cursor: usize, offset: usize },
    /// One step: its readings and its changes.
    Step { id: String, cursor: usize, offset: usize },
    /// One change's hunk, with the lines a judge quoted lit.
    Hunk { window: String, lines: Vec<(String, bool)>, offset: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalScreen {
    pub eval_id: String,
    /// The work it is about, for the title bar.
    pub title: String,
    pub view: EvalView,
    pub mode: Mode,
    pub folded: std::collections::BTreeSet<String>,
    /// From the list down; the last is what is on screen.
    pub layers: Vec<Layer>,
    /// The board this view was read against: a new one means the ledger
    /// grew, and an Eval still running is read again.
    pub seen: u64,
    pub reading: bool,
}

impl EvalScreen {
    pub fn top(&self) -> &Layer {
        self.layers.last().expect("the list is always there")
    }

    fn top_mut(&mut self) -> &mut Layer {
        self.layers.last_mut().expect("the list is always there")
    }

    /// The rows of the level on screen; none for a hunk.
    pub fn rows(&self) -> Vec<ViewRow> {
        match self.top() {
            Layer::List { .. } => rows(&self.view, self.mode, &self.folded),
            Layer::Step { id, .. } => step_rows(&self.view, id),
            Layer::Hunk { .. } => Vec::new(),
        }
    }

    pub fn cursor(&self) -> usize {
        match self.top() {
            Layer::List { cursor, .. } | Layer::Step { cursor, .. } => *cursor,
            Layer::Hunk { .. } => 0,
        }
    }

    pub fn offset(&self) -> usize {
        match self.top() {
            Layer::List { offset, .. }
            | Layer::Step { offset, .. }
            | Layer::Hunk { offset, .. } => *offset,
        }
    }

    pub fn set_offset(&mut self, to: usize) {
        match self.top_mut() {
            Layer::List { offset, .. }
            | Layer::Step { offset, .. }
            | Layer::Hunk { offset, .. } => *offset = to,
        }
    }

    fn set_cursor(&mut self, to: usize) {
        if let Layer::List { cursor, .. } | Layer::Step { cursor, .. } = self.top_mut() {
            *cursor = to;
        }
    }

    /// The first row that can be picked, from `from` in `step`'s direction.
    fn pickable_from(rows: &[ViewRow], from: usize, step: i32) -> Option<usize> {
        let mut i = from as i32;
        while i >= 0 && (i as usize) < rows.len() {
            if rows[i as usize].pickable() {
                return Some(i as usize);
            }
            i += step;
        }
        None
    }

    /// Keep the cursor on a row that can be picked, after the rows changed.
    fn settle_cursor(&mut self) {
        let rows = self.rows();
        let at = self.cursor().min(rows.len().saturating_sub(1));
        let to = Self::pickable_from(&rows, at, 1).or_else(|| Self::pickable_from(&rows, at, -1));
        self.set_cursor(to.unwrap_or(0));
    }

    fn picked(&self) -> Option<ViewRow> {
        self.rows().get(self.cursor()).filter(|r| r.pickable()).cloned()
    }
}

impl App {
    pub fn eval_screen(&self) -> Option<&EvalScreen> {
        self.eval.as_ref()
    }

    /// The Eval of the selected work: the one running, else the last closed.
    fn eval_of_selected(&self) -> Option<(String, String)> {
        let u = self.selected_unit()?;
        let id = u
            .gathered
            .as_ref()
            .map(|g| g.eval_id.clone())
            .or_else(|| u.check.as_ref().map(|c| c.eval_id.clone()))?;
        Some((id, u.title.clone()))
    }

    /// Whether the selected work has an Eval to view.
    pub fn has_eval_view(&self) -> bool {
        self.eval_of_selected().is_some()
    }

    /// `[V]`: open the Eval view of the selected work.
    pub(super) fn open_eval_view(&mut self) {
        let Some((eval_id, title)) = self.eval_of_selected() else {
            self.hint = Some("No Eval of this work yet: [E]VAL starts one.".into());
            return;
        };
        self.detail = None;
        self.read_eval(eval_id, title);
    }

    fn read_eval(&mut self, eval_id: String, title: String) {
        if let Ok(call) = sess!(self).read("eval", &eval_id) {
            if let Some(e) = self.eval.as_mut() {
                e.reading = true;
            }
            self.await_answer(call, Then::EvalView { eval_id, title });
        }
    }

    pub(super) fn eval_read(
        &mut self,
        eval_id: String,
        title: String,
        answer: std::result::Result<serde_json::Value, String>,
    ) {
        let view = match answer.map(serde_json::from_value::<EvalView>) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                self.modal = Some(Modal::Note(format!("The Eval could not be read: {e}")));
                return;
            }
            Err(e) => {
                if self.eval.is_none() {
                    self.modal =
                        Some(Modal::Note(format!("The Eval could not be read: {}", said(&e))));
                }
                return;
            }
        };
        let seen = sess!(self).boards;
        match self.eval.as_mut() {
            Some(e) if e.eval_id == eval_id => {
                e.view = view;
                e.reading = false;
                e.seen = seen;
                e.settle_cursor();
            }
            _ => {
                let mut e = EvalScreen {
                    eval_id,
                    title,
                    view,
                    mode: Mode::Steps,
                    folded: Default::default(),
                    layers: vec![Layer::List { cursor: 0, offset: 0 }],
                    seen,
                    reading: false,
                };
                e.settle_cursor();
                self.eval = Some(e);
            }
        }
    }

    /// While an Eval runs, each new board means the ledger grew: read it
    /// again, one read at a time.
    pub(super) fn follow_eval(&mut self) {
        let boards = sess!(self).boards;
        let Some(e) = self.eval.as_ref() else { return };
        if e.view.closed || e.reading || e.seen == boards {
            return;
        }
        let (id, title) = (e.eval_id.clone(), e.title.clone());
        self.read_eval(id, title);
    }

    pub(super) fn hunk_read(
        &mut self,
        window: String,
        answer: std::result::Result<serde_json::Value, String>,
    ) {
        let Some(e) = self.eval.as_mut() else { return };
        let text = match answer {
            Ok(v) => v.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
            Err(err) => format!("The change could not be read: {}", said(&err)),
        };
        let quotes = e.view.change(&window).map(|c| c.quotes.clone()).unwrap_or_default();
        e.layers.push(Layer::Hunk { window, lines: hunk_lines(&text, &quotes), offset: 0 });
    }

    fn open_hunk(&mut self, window: String) {
        let Some(e) = self.eval.as_ref() else { return };
        let unit = format!("{}/{window}", e.eval_id);
        if let Ok(call) = sess!(self).read("window", &unit) {
            self.await_answer(call, Then::Hunk { window });
        }
    }

    /// A key while the view is open. It blocks: what it does not use does
    /// nothing, as in the detail view.
    pub(super) fn on_eval_action(&mut self, action: Action) {
        let reach = self.eval_reach;
        let Some(e) = self.eval.as_mut() else { return };
        match action {
            Action::Pick(delta) => {
                if let Layer::Hunk { offset, lines, .. } = e.top_mut() {
                    let last = lines.len().saturating_sub(reach.max(1));
                    *offset = (*offset as i32 + delta as i32).clamp(0, last as i32) as usize;
                    return;
                }
                let rows = e.rows();
                let from = e.cursor() as i32 + delta as i32;
                if from >= 0
                    && let Some(to) = EvalScreen::pickable_from(&rows, from as usize, delta as i32)
                {
                    e.set_cursor(to);
                }
            }
            Action::Open => match e.picked() {
                Some(r) if r.kind == RowKind::Group => {
                    if !e.folded.remove(&r.key) {
                        e.folded.insert(r.key);
                    }
                    e.settle_cursor();
                }
                Some(r) if r.kind == RowKind::Step => {
                    let id = r.key.trim_start_matches("s:").to_string();
                    e.layers.push(Layer::Step { id, cursor: 0, offset: 0 });
                    e.settle_cursor();
                }
                Some(r) if r.kind == RowKind::Change => {
                    let w = r.key.trim_start_matches("c:").to_string();
                    self.open_hunk(w);
                }
                _ => {}
            },
            Action::Diff => {
                if let Some(r) = e.picked().filter(|r| r.kind == RowKind::Change) {
                    let w = r.key.trim_start_matches("c:").to_string();
                    self.open_hunk(w);
                }
            }
            Action::NextPane => {
                if matches!(e.top(), Layer::List { .. }) {
                    e.mode = match e.mode {
                        Mode::Steps => Mode::Files,
                        Mode::Files => Mode::Steps,
                    };
                    e.layers = vec![Layer::List { cursor: 0, offset: 0 }];
                    e.settle_cursor();
                }
            }
            Action::Back | Action::EvalView => {
                if e.layers.len() > 1 && action == Action::Back {
                    e.layers.pop();
                } else {
                    self.eval = None;
                }
            }
            _ => {}
        }
    }

    /// How many rows the view had room for, as the last render measured it.
    pub fn note_eval_reach(&mut self, rows: usize) {
        self.eval_reach = rows;
    }

    /// Scroll the level so its cursor is in view, as the render sees it.
    pub fn keep_eval_cursor_in_view(&mut self, room: usize) {
        let Some(e) = self.eval.as_mut() else { return };
        if matches!(e.top(), Layer::Hunk { .. }) || room == 0 {
            return;
        }
        let (cursor, offset) = (e.cursor(), e.offset());
        if cursor < offset {
            e.set_offset(cursor);
        } else if cursor >= offset + room {
            e.set_offset(cursor + 1 - room);
        }
    }
}

// --- the RingFrame view -------------------------------------------------------

impl App {
    pub fn setup_view(&self) -> Option<&weft_core::onboarding::View> {
        sess!(self).setup.as_ref()
    }

    /// Asked for once a run, and opened by itself once, when no harness is
    /// ready and one could be. A person already set up never sees it.
    pub(super) fn offer_setup(&mut self) {
        if !self.setup_asked {
            self.setup_asked = true;
            let _ = sess!(self).setup("look", "", "");
            return;
        }
        if self.setup_offered || self.modal.is_some() {
            return;
        }
        if self.setup_view().is_some_and(|v| v.wants_setting_up()) {
            self.setup_offered = true;
            self.modal = Some(Modal::RingFrame);
            self.modal_choice = 0;
        }
    }

    fn setup_row(&self) -> Option<weft_core::onboarding::Row> {
        self.setup_view()?.rows.get(self.modal_choice).cloned()
    }

    /// `Enter` on a row: what it needs next.
    pub(super) fn setup_selected(&mut self) {
        use weft_core::onboarding::Next;
        let Some(row) = self.setup_row() else { return };
        match row.state.next() {
            Some(Next::Locate) => {
                self.modal = Some(Modal::Locate { harness: row.name, text: String::new() })
            }
            Some(Next::SetUp | Next::Update) => {
                let _ = sess!(self).setup("run", &row.name, "");
            }
            None => {}
        }
    }

    /// `[P]ROCEED`: everything behind, the configuration first, then each
    /// harness by title.
    pub(super) fn setup_all(&mut self) {
        if self.setup_view().is_some_and(|v| v.proceeds()) {
            let _ = sess!(self).setup("all", "", "");
        }
    }

    pub(super) fn setup_locate(&mut self, harness: &str, path: &str) {
        let _ = sess!(self).setup("locate", harness, path);
        self.modal = Some(Modal::RingFrame);
    }
}
