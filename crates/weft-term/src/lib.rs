//! Weft's terminal.
//!
//! One emulator, `alacritty_terminal`, for the daemon and the desktop. This
//! crate holds what both need from it and nothing else: the text the rest of
//! Weft reads, the answers a program asking about its terminal gets, and the
//! escape codes that redraw a screen on another terminal. No alacritty type
//! leaves it.

mod cells;
pub mod palette;
mod screen;
mod text;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Processor, Timeout};

pub use cells::Cell;
pub use screen::Snapshot;

/// The size a pixel question is answered with: Weft's daemon has no pixels.
const CELL_WIDTH: u16 = 8;
const CELL_HEIGHT: u16 = 16;

/// A terminal: what a program printed, as a screen and its history.
pub struct Term {
    term: alacritty_terminal::Term<Events>,
    parser: Processor<NoSync>,
    events: Events,
    answer: Answer,
    /// Every row is to be painted again: the selection changed, which the
    /// emulator's own record of changes leaves out.
    repaint: bool,
}

/// Where a terminal's answers go.
type Answer = Box<dyn FnMut(&[u8]) + Send>;

/// Which keys and mouse reports the program asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub mouse: Mouse,
    pub mouse_encoding: MouseEncoding,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mouse {
    #[default]
    None,
    Click,
    Drag,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseEncoding {
    #[default]
    Default,
    Utf8,
    Sgr,
}

impl Term {
    /// A terminal of `rows` × `cols` keeping `history` lines of scrollback.
    /// `answer` gets every reply to a program's question about its terminal,
    /// in the order asked; the daemon writes them to the program.
    pub fn new(
        rows: u16,
        cols: u16,
        history: usize,
        answer: impl FnMut(&[u8]) + Send + 'static,
    ) -> Term {
        let config = Config { scrolling_history: history, ..Config::default() };
        let events = Events::default();
        let term = alacritty_terminal::Term::new(config, &Size::new(rows, cols), events.clone());
        Term { term, parser: Processor::new(), events, answer: Box::new(answer), repaint: true }
    }

    /// Take what the program printed.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.answer_questions();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.term.resize(Size::new(rows, cols));
    }

    /// `(rows, cols)`.
    pub fn size(&self) -> (u16, u16) {
        (self.term.screen_lines() as u16, self.term.columns() as u16)
    }

    /// `(row, col)` on the screen, from 0.
    pub fn cursor(&self) -> (u16, u16) {
        let point = self.term.grid().cursor.point;
        (point.line.0.max(0) as u16, point.column.0 as u16)
    }

    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn modes(&self) -> Modes {
        let mode = self.term.mode();
        let mouse = if mode.contains(TermMode::MOUSE_MOTION) {
            Mouse::Motion
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            Mouse::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            Mouse::Click
        } else {
            Mouse::None
        };
        let mouse_encoding = if mode.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Default
        };
        Modes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            app_keypad: mode.contains(TermMode::APP_KEYPAD),
            mouse,
            mouse_encoding,
        }
    }

    /// The rows on view, each without its trailing blanks.
    pub fn rows(&self) -> Vec<String> {
        text::rows(&self.term)
    }

    /// The rows on view joined by newlines, trailing empty rows dropped.
    pub fn contents(&self) -> String {
        text::contents(&self.term)
    }

    /// The last `lines` lines printed, scrollback then screen.
    pub fn history(&self, lines: usize) -> String {
        text::history(&self.term, lines)
    }

    /// Move the view back (positive) or forward through history.
    pub fn scroll(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    pub fn scroll_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    /// Every cell on view, row by row, a cell per column, to paint.
    pub fn cells(&self) -> Vec<Vec<Cell>> {
        cells::cells(&self.term)
    }

    /// One row on view, a cell per column, to paint.
    pub fn row(&self, row: u16) -> Vec<Cell> {
        cells::row(&self.term, row)
    }

    /// The rows on view to paint again since this was last asked, from 0, or
    /// `None` for every row: a new or resized screen, a scroll through
    /// history, a selection. The cursor's row, and the one it left, are
    /// always among them.
    pub fn changed_rows(&mut self) -> Option<Vec<u16>> {
        let rows = match self.term.damage() {
            TermDamage::Full => None,
            TermDamage::Partial(lines) => Some(lines.map(|l| l.line as u16).collect()),
        };
        self.term.reset_damage();
        if std::mem::take(&mut self.repaint) { None } else { rows }
    }

    /// Whether to draw the cursor: shown, and the view on the live screen.
    pub fn cursor_visible(&self) -> bool {
        cells::cursor_visible(&self.term)
    }

    /// Start selecting at a cell on view, `(row, col)`.
    pub fn select_from(&mut self, at: (u16, u16)) {
        self.repaint = true;
        cells::select_from(&mut self.term, at);
    }

    /// Stretch the selection to a cell on view.
    pub fn select_to(&mut self, at: (u16, u16)) {
        self.repaint = true;
        cells::select_to(&mut self.term, at);
    }

    pub fn clear_selection(&mut self) {
        self.repaint |= self.term.selection.is_some();
        cells::clear_selection(&mut self.term);
    }

    /// The selected text, if anything is selected.
    pub fn selection_text(&self) -> Option<String> {
        cells::selection_text(&self.term)
    }

    /// Escape codes that make a fresh terminal of this size show this screen,
    /// with up to `history` lines of scrollback behind it.
    pub fn formatted(&self, history: usize) -> Vec<u8> {
        screen::formatted(&self.term, history)
    }

    /// What `diff` compares against.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::of(&self.term)
    }

    /// Escape codes that take a terminal showing `last` to this screen, or
    /// `None` when it needs the whole screen again: the size changed, or the
    /// alternate screen switched.
    pub fn diff(&self, last: &Snapshot) -> Option<Vec<u8>> {
        screen::diff(&self.term, last)
    }

    fn answer_questions(&mut self) {
        let events = std::mem::take(&mut *self.events.0.lock().expect("terminal events"));
        for event in events {
            let reply = match event {
                Event::PtyWrite(text) => text,
                Event::ColorRequest(index, format) => format(
                    self.term.colors()[index].unwrap_or_else(|| palette::color(index).into()),
                ),
                Event::TextAreaSizeRequest(format) => {
                    let (rows, cols) = self.size();
                    format(WindowSize {
                        num_lines: rows,
                        num_cols: cols,
                        cell_width: CELL_WIDTH,
                        cell_height: CELL_HEIGHT,
                    })
                }
                _ => continue,
            };
            (self.answer)(reply.as_bytes());
        }
    }
}

/// What the emulator raised while it parsed, kept until the parse is done.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<Event>>>);

impl EventListener for Events {
    fn send_event(&self, event: Event) {
        self.0.lock().expect("terminal events").push(event);
    }
}

/// Draws a synchronized update as it arrives rather than holding it back:
/// the daemon reads the screen on its own clock, and a held update would
/// need a timer to let go.
#[derive(Default)]
struct NoSync;

impl Timeout for NoSync {
    fn set_timeout(&mut self, _: Duration) {}
    fn clear_timeout(&mut self) {}
    fn pending_timeout(&self) -> bool {
        false
    }
}

struct Size {
    rows: usize,
    cols: usize,
}

impl Size {
    fn new(rows: u16, cols: u16) -> Size {
        Size { rows: rows.max(1) as usize, cols: cols.max(1) as usize }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}
