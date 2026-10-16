//! A text field's words, caret and selection, and what each key does to them:
//! the part of the desktop's text input that knows nothing of a window.
//!
//! Keys come from `weft_core::text_keys`, the table the web's fields are
//! checked against too. Offsets are bytes into the text, always on a
//! character boundary.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use weft_core::text_keys::{self, Edit};

/// Which field it behaves like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Like `<input>`: one line; a pasted newline becomes a space.
    Line,
    /// Like `<textarea>`: Enter sends, Shift+Enter adds a line.
    Area,
}

/// What a key asks of whoever holds the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The words or the caret changed.
    Changed,
    /// Send the message, or do what a one-line field is for.
    Send,
    /// Take the suggestion; drop it.
    Take,
    Drop,
    /// Put this on the clipboard.
    Copy(String),
    /// Read the clipboard and hand it to [`TextField::paste`].
    Paste,
    /// Nothing to do: the caret is already there, or there is nothing to undo.
    Nothing,
}

/// The words and the caret as they were, for undo.
#[derive(Debug, Clone, PartialEq)]
struct Kept {
    text: String,
    selection: Range<usize>,
}

/// A text field: its words, its selection (the caret when empty), the
/// composing text an input method has marked, and its undo history.
#[derive(Debug, Clone)]
pub struct TextField {
    shape: Shape,
    text: String,
    selection: Range<usize>,
    /// The caret is at the selection's start, not its end.
    reversed: bool,
    /// What an input method is composing, underlined until it commits.
    pub marked: Option<Range<usize>>,
    undo: Vec<Kept>,
    redo: Vec<Kept>,
    /// Typing that one undo takes back together: it ends at a space or a move.
    typing: bool,
    /// The column up and down keep, in characters.
    column: Option<usize>,
}

impl TextField {
    pub fn new(shape: Shape) -> Self {
        TextField {
            shape,
            text: String::new(),
            selection: 0..0,
            reversed: false,
            marked: None,
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            column: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    pub fn reversed(&self) -> bool {
        self.reversed
    }

    /// Where the caret is: the end of the selection that moves.
    pub fn cursor(&self) -> usize {
        if self.reversed { self.selection.start } else { self.selection.end }
    }

    /// Do what `chord` does, when it is a text key.
    pub fn key(&mut self, chord: &str) -> Option<Outcome> {
        text_keys::edit(chord).map(|e| self.apply(e))
    }

    pub fn apply(&mut self, edit: Edit) -> Outcome {
        let before = (self.selection.clone(), self.reversed);
        if !matches!(edit, Edit::Up | Edit::Down | Edit::SelectUp | Edit::SelectDown) {
            self.column = None;
        }
        let moved = |f: &Self| {
            if (f.selection.clone(), f.reversed) == before {
                Outcome::Nothing
            } else {
                Outcome::Changed
            }
        };
        match edit {
            Edit::Send => Outcome::Send,
            Edit::Take => Outcome::Take,
            Edit::Drop => Outcome::Drop,
            Edit::NewLine => match self.shape {
                Shape::Line => Outcome::Nothing,
                Shape::Area => {
                    self.replace(None, "\n", false);
                    Outcome::Changed
                }
            },
            Edit::Left
            | Edit::Right
            | Edit::Up
            | Edit::Down
            | Edit::WordLeft
            | Edit::WordRight
            | Edit::LineStart
            | Edit::LineEnd
            | Edit::Start
            | Edit::End => {
                self.typing = false;
                let to = match (edit, self.selection.is_empty()) {
                    (Edit::Left, false) => self.selection.start,
                    (Edit::Right, false) => self.selection.end,
                    _ => self.target(edit),
                };
                self.reversed = false;
                self.selection = to..to;
                moved(self)
            }
            Edit::SelectLeft
            | Edit::SelectRight
            | Edit::SelectUp
            | Edit::SelectDown
            | Edit::SelectWordLeft
            | Edit::SelectWordRight
            | Edit::SelectLineStart
            | Edit::SelectLineEnd
            | Edit::SelectStart
            | Edit::SelectEnd => {
                self.typing = false;
                let to = self.target(unselect(edit));
                self.select_to(to);
                moved(self)
            }
            Edit::SelectAll => {
                self.typing = false;
                self.reversed = false;
                self.selection = 0..self.text.len();
                moved(self)
            }
            Edit::DeleteBack
            | Edit::DeleteForward
            | Edit::DeleteWordBack
            | Edit::DeleteWordForward
            | Edit::DeleteLineBack => {
                let range = if self.selection.is_empty() {
                    let at = self.cursor();
                    match edit {
                        Edit::DeleteBack => self.previous(at)..at,
                        Edit::DeleteForward => at..self.next(at),
                        Edit::DeleteWordBack => self.word_left(at)..at,
                        Edit::DeleteWordForward => at..self.word_right(at),
                        _ => self.line_start(at)..at,
                    }
                } else {
                    self.selection.clone()
                };
                if range.is_empty() {
                    return Outcome::Nothing;
                }
                self.typing = false;
                self.replace(Some(range), "", false);
                Outcome::Changed
            }
            Edit::Copy | Edit::Cut => {
                if self.selection.is_empty() {
                    return Outcome::Nothing;
                }
                let taken = self.text[self.selection.clone()].to_string();
                if edit == Edit::Cut {
                    self.typing = false;
                    self.replace(None, "", false);
                }
                Outcome::Copy(taken)
            }
            Edit::Paste => Outcome::Paste,
            Edit::Undo => self.step(true),
            Edit::Redo => self.step(false),
        }
    }

    /// Typed characters, in place of the selection.
    pub fn type_text(&mut self, text: &str) {
        let line = self.lined(text);
        self.replace(None, &line, true);
    }

    /// The clipboard's text, in place of the selection.
    pub fn paste(&mut self, text: &str) {
        self.typing = false;
        let line = self.lined(text);
        self.replace(None, &line, false);
    }

    /// Start again with `text`, the caret at its end and nothing to undo.
    pub fn set_text(&mut self, text: &str) {
        self.text = self.lined(text);
        self.selection = self.text.len()..self.text.len();
        self.reversed = false;
        self.marked = None;
        self.undo.clear();
        self.redo.clear();
        self.typing = false;
        self.column = None;
    }

    /// Select `range`, kept on character boundaries and within the text.
    pub fn select(&mut self, range: Range<usize>) {
        let fit = |i: usize| {
            let mut i = i.min(self.text.len());
            while !self.text.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        let (a, b) = (fit(range.start), fit(range.end));
        self.reversed = b < a;
        self.selection = a.min(b)..a.max(b);
        self.typing = false;
        self.column = None;
    }

    /// Move the caret's end of the selection to `to`, the other end staying.
    pub fn select_to(&mut self, to: usize) {
        let anchor = if self.reversed { self.selection.end } else { self.selection.start };
        self.reversed = to < anchor;
        self.selection = anchor.min(to)..anchor.max(to);
    }

    /// Put `text` in place of `range` (the marked text, else the selection),
    /// the caret after it. An input method's commit comes this way.
    pub fn replace(&mut self, range: Option<Range<usize>>, text: &str, typing: bool) {
        let range = range.or(self.marked.clone()).unwrap_or(self.selection.clone());
        // A word and the space after it are one undo.
        let joins = typing && self.typing;
        if !joins {
            self.undo.push(Kept { text: self.text.clone(), selection: self.selection.clone() });
        }
        self.redo.clear();
        self.typing = typing && !text.ends_with(char::is_whitespace);
        self.text.replace_range(range.clone(), text);
        let at = range.start + text.len();
        self.selection = at..at;
        self.reversed = false;
        self.marked = None;
    }

    /// An input method's composing text in place of `range` (the marked text,
    /// else the selection), marked, with `selected` inside it selected.
    pub fn mark(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let range = range.or(self.marked.clone()).unwrap_or(self.selection.clone());
        if self.marked.is_none() {
            self.undo.push(Kept { text: self.text.clone(), selection: self.selection.clone() });
            self.redo.clear();
        }
        self.typing = false;
        self.text.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        let inside = selected.unwrap_or(text.len()..text.len());
        self.selection =
            range.start + inside.start.min(text.len())..range.start + inside.end.min(text.len());
        self.reversed = false;
    }

    fn step(&mut self, back: bool) -> Outcome {
        let (from, to) =
            if back { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(kept) = from.pop() else {
            return Outcome::Nothing;
        };
        to.push(Kept {
            text: std::mem::replace(&mut self.text, kept.text),
            selection: self.selection.clone(),
        });
        self.selection = kept.selection;
        self.reversed = false;
        self.typing = false;
        self.marked = None;
        Outcome::Changed
    }

    /// A one-line field's text has spaces where the lines broke.
    fn lined(&self, text: &str) -> String {
        match self.shape {
            Shape::Area => text.replace("\r\n", "\n"),
            Shape::Line => text.replace("\r\n", " ").replace(['\n', '\r'], " "),
        }
    }

    /// Where a caret move from the caret lands.
    fn target(&mut self, edit: Edit) -> usize {
        let at = self.cursor();
        match edit {
            Edit::Left => self.previous(at),
            Edit::Right => self.next(at),
            Edit::WordLeft => self.word_left(at),
            Edit::WordRight => self.word_right(at),
            Edit::LineStart => self.line_start(at),
            Edit::LineEnd => self.line_end(at),
            Edit::Start => 0,
            Edit::End => self.text.len(),
            Edit::Up | Edit::Down => self.vertical(at, edit == Edit::Up),
            _ => at,
        }
    }

    fn previous(&self, at: usize) -> usize {
        self.text[..at].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..].graphemes(true).next().map_or(at, |g| at + g.len())
    }

    fn word_left(&self, at: usize) -> usize {
        let before = &self.text[..at];
        let end = before.trim_end_matches(|c: char| !c.is_alphanumeric()).len();
        before[..end].trim_end_matches(char::is_alphanumeric).len()
    }

    fn word_right(&self, at: usize) -> usize {
        let after = &self.text[at..];
        let skip = after.len() - after.trim_start_matches(|c: char| !c.is_alphanumeric()).len();
        let rest = &after[skip..];
        at + skip + (rest.len() - rest.trim_start_matches(char::is_alphanumeric).len())
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..].find('\n').map_or(self.text.len(), |i| at + i)
    }

    /// The same column a line up or down; past the first or last line, its start or end.
    fn vertical(&mut self, at: usize, up: bool) -> usize {
        let start = self.line_start(at);
        let column = *self.column.get_or_insert_with(|| self.text[start..at].chars().count());
        let line = if up {
            if start == 0 {
                self.column = None;
                return 0;
            }
            self.line_start(start - 1)
        } else {
            let end = self.line_end(at);
            if end == self.text.len() {
                self.column = None;
                return end;
            }
            end + 1
        };
        let end = self.line_end(line);
        self.text[line..end].char_indices().nth(column).map_or(end, |(i, _)| line + i)
    }
}

/// The move a selecting key makes.
fn unselect(edit: Edit) -> Edit {
    match edit {
        Edit::SelectLeft => Edit::Left,
        Edit::SelectRight => Edit::Right,
        Edit::SelectUp => Edit::Up,
        Edit::SelectDown => Edit::Down,
        Edit::SelectWordLeft => Edit::WordLeft,
        Edit::SelectWordRight => Edit::WordRight,
        Edit::SelectLineStart => Edit::LineStart,
        Edit::SelectLineEnd => Edit::LineEnd,
        Edit::SelectStart => Edit::Start,
        _ => Edit::End,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field holding `text` with the caret where `|` is.
    fn at(shape: Shape, text: &str) -> TextField {
        let caret = text.find('|').expect("a caret");
        let mut f = TextField::new(shape);
        f.set_text(&text.replace('|', ""));
        f.select(caret..caret);
        f
    }

    /// The field's text with the caret, or the selection in `[ ]`.
    fn shown(f: &TextField) -> String {
        let s = f.selection();
        let t = f.text();
        if s.is_empty() {
            format!("{}|{}", &t[..s.start], &t[s.start..])
        } else {
            format!("{}[{}]{}", &t[..s.start], &t[s.start..s.end], &t[s.end..])
        }
    }

    fn keys(f: &mut TextField, chords: &[&str]) -> Vec<Option<Outcome>> {
        chords.iter().map(|c| f.key(c)).collect()
    }

    #[test]
    fn in_a_message_enter_sends_and_shift_enter_adds_a_line() {
        let mut f = at(Shape::Area, "fix the bug|");
        assert_eq!(f.key("shift-enter"), Some(Outcome::Changed));
        f.type_text("then test");
        assert_eq!(shown(&f), "fix the bug\nthen test|");
        assert_eq!(f.key("enter"), Some(Outcome::Send));
        assert_eq!(f.text(), "fix the bug\nthen test", "sending is the holder's");
    }

    #[test]
    fn a_one_line_field_has_no_new_line_and_pastes_lines_as_spaces() {
        let mut f = at(Shape::Line, "find|");
        assert_eq!(f.key("shift-enter"), Some(Outcome::Nothing));
        f.paste(" login\ntests\r\nnow");
        assert_eq!(shown(&f), "find login tests now|");
        let mut m = at(Shape::Area, "|");
        m.paste("one\ntwo");
        assert_eq!(shown(&m), "one\ntwo|", "a message keeps its lines");
    }

    #[test]
    fn the_caret_moves_by_character_word_line_and_field() {
        let mut f = at(Shape::Area, "run the lo|gin tests\nand paste");
        f.key("alt-left");
        assert_eq!(shown(&f), "run the |login tests\nand paste");
        f.key("alt-left");
        assert_eq!(shown(&f), "run |the login tests\nand paste");
        f.key("alt-right");
        assert_eq!(shown(&f), "run the| login tests\nand paste");
        f.key("cmd-right");
        assert_eq!(shown(&f), "run the login tests|\nand paste");
        f.key("right");
        assert_eq!(shown(&f), "run the login tests\n|and paste");
        f.key("cmd-left");
        assert_eq!(shown(&f), "run the login tests\n|and paste");
        f.key("cmd-up");
        assert_eq!(shown(&f), "|run the login tests\nand paste");
        assert_eq!(f.key("left"), Some(Outcome::Nothing));
        f.key("cmd-down");
        assert_eq!(shown(&f), "run the login tests\nand paste|");
    }

    #[test]
    fn up_and_down_keep_the_column_and_stop_at_the_ends() {
        let mut f = at(Shape::Area, "run the login tests\nand pas|te\nok");
        f.key("up");
        assert_eq!(shown(&f), "run the| login tests\nand paste\nok");
        f.key("down");
        f.key("down");
        assert_eq!(
            shown(&f),
            "run the login tests\nand paste\nok|",
            "a short line takes the caret to its end"
        );
        f.key("up");
        assert_eq!(shown(&f), "run the login tests\nand pas|te\nok", "and the column comes back");
        f.key("up");
        f.key("up");
        assert_eq!(
            shown(&f),
            "|run the login tests\nand paste\nok",
            "up from the first line goes to its start"
        );
        f.key("shift-down");
        assert_eq!(shown(&f), "[run the login tests\n]and paste\nok");
    }

    #[test]
    fn a_character_is_a_whole_grapheme() {
        let mut f = at(Shape::Area, "日本語|");
        f.key("backspace");
        assert_eq!(shown(&f), "日本|");
        let mut e = at(Shape::Area, "ok 👍🏽|");
        e.key("left");
        assert_eq!(shown(&e), "ok |👍🏽");
    }

    #[test]
    fn shift_selects_and_typing_replaces_the_selection() {
        let mut f = at(Shape::Area, "run the |login tests");
        keys(&mut f, &["alt-shift-right", "shift-right"]);
        assert_eq!(shown(&f), "run the [login ]tests");
        f.type_text("unit ");
        assert_eq!(shown(&f), "run the unit |tests");
        f.key("cmd-a");
        assert_eq!(shown(&f), "[run the unit tests]");
        f.key("left");
        assert_eq!(shown(&f), "|run the unit tests", "left from a selection goes to its start");
        f.key("shift-cmd-right");
        f.key("shift-left");
        assert_eq!(shown(&f), "[run the unit test]s");
    }

    #[test]
    fn deleting_by_character_word_and_line() {
        let mut f = at(Shape::Area, "one\nrun the login| tests");
        f.key("alt-backspace");
        assert_eq!(shown(&f), "one\nrun the | tests");
        f.key("delete");
        assert_eq!(shown(&f), "one\nrun the |tests");
        f.key("alt-delete");
        assert_eq!(shown(&f), "one\nrun the |");
        f.key("cmd-backspace");
        assert_eq!(shown(&f), "one\n|");
        f.key("backspace");
        assert_eq!(shown(&f), "one|");
        let mut s = at(Shape::Area, "|run the login tests");
        s.select(4..13);
        s.key("backspace");
        assert_eq!(shown(&s), "run | tests");
    }

    #[test]
    fn copy_cut_and_paste_go_through_the_holder() {
        let mut f = at(Shape::Area, "run the |login tests");
        assert_eq!(f.key("cmd-c"), Some(Outcome::Nothing), "nothing selected");
        f.key("alt-shift-right");
        assert_eq!(f.key("cmd-c"), Some(Outcome::Copy("login".into())));
        assert_eq!(f.key("cmd-x"), Some(Outcome::Copy("login".into())));
        assert_eq!(shown(&f), "run the | tests");
        assert_eq!(f.key("cmd-v"), Some(Outcome::Paste));
        f.paste("unit");
        assert_eq!(shown(&f), "run the unit| tests");
    }

    #[test]
    fn undo_takes_back_a_typed_word_at_a_time_and_redo_brings_it_back() {
        let mut f = at(Shape::Area, "|");
        for c in ["r", "u", "n", " ", "i", "t"] {
            f.type_text(c);
        }
        assert_eq!(f.key("cmd-z"), Some(Outcome::Changed));
        assert_eq!(shown(&f), "run |");
        f.key("cmd-z");
        assert_eq!(shown(&f), "|");
        assert_eq!(f.key("cmd-z"), Some(Outcome::Nothing));
        f.key("shift-cmd-z");
        assert_eq!(shown(&f), "run |");
        f.key("shift-cmd-z");
        assert_eq!(shown(&f), "run it|");
        // A new change ends what redo could bring back.
        f.key("cmd-z");
        f.type_text("x");
        assert_eq!(f.key("shift-cmd-z"), Some(Outcome::Nothing));
    }

    #[test]
    fn tab_and_escape_are_for_the_suggestion() {
        let mut f = at(Shape::Area, "run|");
        assert_eq!(f.key("tab"), Some(Outcome::Take));
        assert_eq!(f.key("escape"), Some(Outcome::Drop));
        assert_eq!(f.key("ctrl-q"), None, "not a text key");
    }
}
