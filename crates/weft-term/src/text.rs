//! The text of a screen: what Sense reads, what a pane's history is, and
//! what the tests compare. Each row loses its trailing blanks and keeps its
//! leading ones; the second half of a wide character is not a character.

use alacritty_terminal::Term;
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::Line;
use alacritty_terminal::term::cell::{Cell, Flags};

pub(crate) fn row_text(row: &Row<Cell>) -> String {
    let mut text = String::new();
    for cell in &row[..] {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            text.push(' ');
            continue;
        }
        text.push(cell.c);
        if let Some(marks) = cell.zerowidth() {
            text.extend(marks);
        }
    }
    text.truncate(text.trim_end_matches(' ').len());
    text
}

pub(crate) fn rows<T>(term: &Term<T>) -> Vec<String> {
    let grid = term.grid();
    let offset = grid.display_offset() as i32;
    (0..grid.screen_lines() as i32).map(|i| row_text(&grid[Line(i - offset)])).collect()
}

pub(crate) fn contents<T>(term: &Term<T>) -> String {
    let mut text = rows(term).join("\n");
    text.truncate(text.trim_end_matches('\n').len());
    text
}

pub(crate) fn history<T>(term: &Term<T>, lines: usize) -> String {
    let grid = term.grid();
    let top = -(grid.history_size() as i32);
    let mut out: Vec<String> =
        (top..grid.screen_lines() as i32).map(|i| row_text(&grid[Line(i)])).collect();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.split_off(out.len().saturating_sub(lines)).join("\n")
}
