//! A screen as escape codes: the whole of it for a fresh terminal, or the
//! change from one a terminal already shows.
//!
//! Only the visible screen is written. The normal screen behind the
//! alternate one is the emulator's own and cannot be read, so a terminal
//! that starts while the alternate screen is on has a blank one behind it.
//!
//! Each row is written whole: a changed row is erased and redrawn, so a
//! difference never depends on what the other terminal had in that row.

use std::fmt::Write;

use alacritty_terminal::Term;
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::Line;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// The modes another terminal must share to read keys, mouse and pastes as
/// this one would, and to show the cursor as it is.
const CARRIED: TermMode = TermMode::SHOW_CURSOR
    .union(TermMode::APP_CURSOR)
    .union(TermMode::APP_KEYPAD)
    .union(TermMode::BRACKETED_PASTE)
    .union(TermMode::MOUSE_REPORT_CLICK)
    .union(TermMode::MOUSE_DRAG)
    .union(TermMode::MOUSE_MOTION)
    .union(TermMode::UTF8_MOUSE)
    .union(TermMode::SGR_MOUSE);

/// Each carried mode and its private mode number, in the order written.
const MODE_NUMBERS: [(TermMode, u16); 8] = [
    (TermMode::SHOW_CURSOR, 25),
    (TermMode::APP_CURSOR, 1),
    (TermMode::BRACKETED_PASTE, 2004),
    (TermMode::MOUSE_REPORT_CLICK, 1000),
    (TermMode::MOUSE_DRAG, 1002),
    (TermMode::MOUSE_MOTION, 1003),
    (TermMode::UTF8_MOUSE, 1005),
    (TermMode::SGR_MOUSE, 1006),
];

/// The flags that change how a cell is drawn. Wrapping and wide-character
/// bookkeeping are the emulator's, and follow from the text written.
const DRAWN: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT)
    .union(Flags::WIDE_CHAR)
    .union(Flags::WIDE_CHAR_SPACER);

/// What a cell shows, for comparing two screens.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shown {
    c: char,
    marks: Vec<char>,
    fg: Color,
    bg: Color,
    flags: Flags,
}

impl Shown {
    fn of(cell: &Cell) -> Shown {
        let leading = cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER);
        Shown {
            c: if leading { ' ' } else { cell.c },
            marks: cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default(),
            fg: cell.fg,
            bg: cell.bg,
            flags: cell.flags & DRAWN,
        }
    }

    fn is_blank(&self) -> bool {
        self.c == ' '
            && self.marks.is_empty()
            && self.flags.is_empty()
            && self.fg == Color::Named(NamedColor::Foreground)
            && self.bg == Color::Named(NamedColor::Background)
    }
}

/// A screen as `diff` compares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    size: (usize, usize),
    alt: bool,
    rows: Vec<Vec<Shown>>,
    cursor: (i32, usize),
    modes: TermMode,
    pen: Pen,
}

impl Snapshot {
    /// `(rows, cols)` of the screen it holds.
    pub fn size(&self) -> (u16, u16) {
        (self.size.0 as u16, self.size.1 as u16)
    }

    pub(crate) fn of<T>(term: &Term<T>) -> Snapshot {
        let grid = term.grid();
        let point = grid.cursor.point;
        Snapshot {
            size: (grid.screen_lines(), grid.columns()),
            alt: term.mode().contains(TermMode::ALT_SCREEN),
            rows: (0..grid.screen_lines() as i32)
                .map(|i| grid[Line(i)][..].iter().map(Shown::of).collect())
                .collect(),
            cursor: (point.line.0, point.column.0),
            modes: *term.mode() & CARRIED,
            pen: Pen::of(&grid.cursor.template),
        }
    }
}

pub(crate) fn formatted<T>(term: &Term<T>, history: usize) -> Vec<u8> {
    let grid = term.grid();
    let alt = term.mode().contains(TermMode::ALT_SCREEN);
    let mut out = String::from("\x1bc");
    if alt {
        out.push_str("\x1b[?1049h");
    }
    // History lives on the normal screen only.
    let kept = if alt { 0 } else { history.min(grid.history_size()) };
    let top = -(kept as i32);
    for (n, i) in (top..grid.screen_lines() as i32).enumerate() {
        if n > 0 {
            out.push_str("\r\n");
        }
        write_row(&mut out, &grid[Line(i)]);
    }
    let now = Snapshot::of(term);
    write_cursor(&mut out, now.cursor);
    write_modes(&mut out, TermMode::SHOW_CURSOR, now.modes);
    if now.modes.contains(TermMode::APP_KEYPAD) {
        out.push_str("\x1b=");
    }
    now.pen.write(&mut out);
    out.into_bytes()
}

pub(crate) fn diff<T>(term: &Term<T>, last: &Snapshot) -> Option<Vec<u8>> {
    let now = Snapshot::of(term);
    if now.size != last.size || now.alt != last.alt {
        return None;
    }
    let grid = term.grid();
    let mut out = String::new();
    for (i, (row, was)) in now.rows.iter().zip(&last.rows).enumerate() {
        if row != was {
            let _ = write!(out, "\x1b[{};1H\x1b[m\x1b[2K", i + 1);
            write_row(&mut out, &grid[Line(i as i32)]);
        }
    }
    if !out.is_empty() || now.cursor != last.cursor {
        write_cursor(&mut out, now.cursor);
    }
    write_modes(&mut out, last.modes, now.modes);
    if now.modes.contains(TermMode::APP_KEYPAD) != last.modes.contains(TermMode::APP_KEYPAD) {
        out.push_str(if now.modes.contains(TermMode::APP_KEYPAD) { "\x1b=" } else { "\x1b>" });
    }
    if !out.is_empty() || now.pen != last.pen {
        now.pen.write(&mut out);
    }
    Some(out.into_bytes())
}

/// One row: its cells up to the last that shows anything, then the style
/// reset, so a newline after it paints nothing.
fn write_row(out: &mut String, row: &Row<Cell>) {
    let shown: Vec<Shown> = row[..].iter().map(Shown::of).collect();
    let end = shown.iter().rposition(|c| !c.is_blank()).map_or(0, |i| i + 1);
    let mut pen = Pen::default();
    for cell in &shown[..end] {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let want = Pen { fg: cell.fg, bg: cell.bg, flags: cell.flags & !WIDE };
        if want != pen {
            want.write(out);
            pen = want;
        }
        out.push(cell.c);
        out.extend(&cell.marks);
    }
    if pen != Pen::default() {
        out.push_str("\x1b[m");
    }
}

const WIDE: Flags = Flags::WIDE_CHAR.union(Flags::WIDE_CHAR_SPACER);

fn write_cursor(out: &mut String, (line, column): (i32, usize)) {
    let _ = write!(out, "\x1b[{};{}H", line.max(0) + 1, column + 1);
}

fn write_modes(out: &mut String, was: TermMode, now: TermMode) {
    for (mode, number) in MODE_NUMBERS {
        if was.contains(mode) != now.contains(mode) {
            let set = if now.contains(mode) { 'h' } else { 'l' };
            let _ = write!(out, "\x1b[?{number}{set}");
        }
    }
}

/// The style the next character is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pen {
    fg: Color,
    bg: Color,
    flags: Flags,
}

impl Default for Pen {
    fn default() -> Pen {
        Pen {
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
        }
    }
}

impl Pen {
    fn of(cell: &Cell) -> Pen {
        Pen { fg: cell.fg, bg: cell.bg, flags: cell.flags & DRAWN & !WIDE }
    }

    /// The whole style from a reset: one rule, whatever came before.
    fn write(&self, out: &mut String) {
        out.push_str("\x1b[0");
        for (flag, code) in [
            (Flags::BOLD, "1"),
            (Flags::DIM, "2"),
            (Flags::ITALIC, "3"),
            (Flags::UNDERLINE, "4"),
            (Flags::DOUBLE_UNDERLINE, "4:2"),
            (Flags::UNDERCURL, "4:3"),
            (Flags::DOTTED_UNDERLINE, "4:4"),
            (Flags::DASHED_UNDERLINE, "4:5"),
            (Flags::INVERSE, "7"),
            (Flags::HIDDEN, "8"),
            (Flags::STRIKEOUT, "9"),
        ] {
            if self.flags.contains(flag) {
                out.push(';');
                out.push_str(code);
            }
        }
        write_color(out, self.fg, 30, 90, 38);
        write_color(out, self.bg, 40, 100, 48);
        out.push('m');
    }
}

fn write_color(out: &mut String, color: Color, base: u8, bright: u8, extended: u8) {
    let _ = match color {
        Color::Named(name) => match name as usize {
            n @ 0..=7 => write!(out, ";{}", base as usize + n),
            n @ 8..=15 => write!(out, ";{}", bright as usize + n - 8),
            _ => Ok(()),
        },
        Color::Indexed(i) => write!(out, ";{extended};5;{i}"),
        Color::Spec(c) => write!(out, ";{extended};2;{};{};{}", c.r, c.g, c.b),
    };
}
