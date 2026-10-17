//! The screen as cells to paint, with colours already resolved against the
//! palette, and the selection a person makes on it.

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use crate::palette::{self, Rgb};

/// One cell on view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Its character and any marks on it; empty for the second half of a
    /// wide character.
    pub text: String,
    pub fg: Rgb,
    /// `None` is the terminal's own background.
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Takes two columns.
    pub wide: bool,
    pub selected: bool,
}

/// Every cell on view, row by row, a cell per column.
pub(crate) fn cells<T>(term: &Term<T>) -> Vec<Vec<Cell>> {
    (0..term.grid().screen_lines() as u16).map(|r| row(term, r)).collect()
}

/// One row on view, a cell per column.
pub(crate) fn row<T>(term: &Term<T>, row: u16) -> Vec<Cell> {
    let grid = term.grid();
    let offset = grid.display_offset() as i32;
    let selection = term.selection.as_ref().and_then(|s| s.to_range(term));
    let line = Line(row as i32 - offset);
    grid[line][..]
        .iter()
        .enumerate()
        .map(|(col, cell)| {
            let mut text = String::new();
            if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                text.push(cell.c);
                if let Some(marks) = cell.zerowidth() {
                    text.extend(marks);
                }
            }
            let mut fg = rgb(term, cell.fg).unwrap_or(palette::FOREGROUND);
            let mut bg = rgb(term, cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                let was = fg;
                fg = bg.unwrap_or(palette::BACKGROUND);
                bg = Some(was);
            }
            if cell.flags.contains(Flags::HIDDEN) {
                fg = bg.unwrap_or(palette::BACKGROUND);
            }
            Cell {
                text,
                fg,
                bg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                strike: cell.flags.contains(Flags::STRIKEOUT),
                wide: cell.flags.contains(Flags::WIDE_CHAR),
                selected: selection.is_some_and(|s| s.contains(Point::new(line, Column(col)))),
            }
        })
        .collect()
}

/// A colour as the palette draws it; `None` for the default background.
fn rgb<T>(term: &Term<T>, color: Color) -> Option<Rgb> {
    let index = match color {
        Color::Spec(c) => return Some(c.into()),
        Color::Indexed(i) => i as usize,
        Color::Named(NamedColor::Background) => return None,
        Color::Named(name) => name as usize,
    };
    Some(term.colors()[index].map(Rgb::from).unwrap_or_else(|| palette::color(index)))
}

/// Whether the cursor shows: the program did not hide it, and the view is
/// on the live screen.
pub(crate) fn cursor_visible<T>(term: &Term<T>) -> bool {
    term.mode().contains(TermMode::SHOW_CURSOR) && term.grid().display_offset() == 0
}

/// Start a selection at a cell on view, `(row, col)`.
pub(crate) fn select_from<T>(term: &mut Term<T>, at: (u16, u16)) {
    let point = on_view(term, at);
    term.selection = Some(Selection::new(SelectionType::Simple, point, Side::Left));
}

/// Stretch the selection to a cell on view, its right side included.
pub(crate) fn select_to<T>(term: &mut Term<T>, at: (u16, u16)) {
    let point = on_view(term, at);
    if let Some(selection) = term.selection.as_mut() {
        selection.update(point, Side::Right);
    }
}

pub(crate) fn clear_selection<T>(term: &mut Term<T>) {
    term.selection = None;
}

pub(crate) fn selection_text<T>(term: &Term<T>) -> Option<String> {
    term.selection_to_string().filter(|t| !t.is_empty())
}

/// A row and column on view, as a point in the grid's history.
fn on_view<T>(term: &Term<T>, (row, col): (u16, u16)) -> Point {
    let grid = term.grid();
    let row = (row as usize).min(grid.screen_lines().saturating_sub(1));
    let col = (col as usize).min(grid.columns().saturating_sub(1));
    Point::new(Line(row as i32 - grid.display_offset() as i32), Column(col))
}
