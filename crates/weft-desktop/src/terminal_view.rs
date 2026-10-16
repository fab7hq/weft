//! A pane's terminal, painted from its cells: backgrounds as rectangles,
//! text as runs of one style placed on the cell grid, and the cursor. Each
//! pane takes the size its own rectangle holds.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, Font, FontFeatures, FontStyle, FontWeight, Hsla, IntoElement, Pixels, Rgba,
    SharedString, StrikethroughStyle, Styled, TextAlign, TextRun, UnderlineStyle, Window, canvas,
    fill, font, point, px, size,
};
use weft_term::palette;

use crate::window::SharedState;

pub const FONT: &str = crate::fonts::MONO;
pub const FONT_SIZE: f32 = 12.0;
pub const LINE_HEIGHT: f32 = 18.0;

/// The rows and columns a rectangle holds, at least one of each.
pub fn grid_size(width: f32, height: f32, cell_width: f32, line_height: f32) -> (u16, u16) {
    let rows = (height / line_height).floor().max(1.0);
    let cols = (width / cell_width).floor().max(1.0);
    (rows.min(u16::MAX as f32) as u16, cols.min(u16::MAX as f32) as u16)
}

/// What one frame paints.
struct Screen {
    cells: Vec<Vec<weft_term::Cell>>,
    cursor: Option<(u16, u16)>,
    cell_width: f32,
}

/// The terminal of `pane` in `project`, filling its parent.
pub(crate) fn terminal(
    shared: Arc<Mutex<SharedState>>,
    project: PathBuf,
    pane: u32,
) -> impl IntoElement {
    canvas(
        move |bounds, window, _cx| {
            let cell_width = cell_width(window);
            let (rows, cols) = grid_size(
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                cell_width,
                LINE_HEIGHT,
            );
            let Ok(mut guard) = shared.lock() else {
                return Screen { cells: Vec::new(), cursor: None, cell_width };
            };
            guard.grids.insert((project.clone(), pane), (bounds.origin, cell_width));
            guard.app.resize_pane(&project, pane, rows, cols);
            match guard.app.pane_view(&project, pane) {
                Some(view) => Screen {
                    cells: view.cells(),
                    cursor: view.cursor_visible().then(|| view.cursor()),
                    cell_width,
                },
                None => Screen { cells: Vec::new(), cursor: None, cell_width },
            }
        },
        paint,
    )
    .size_full()
}

/// The grid's font: one character to a cell, so no ligatures, as
/// xterm.js draws it on the web.
fn grid_font() -> Font {
    Font { features: FontFeatures::disable_ligatures(), ..font(FONT) }
}

fn cell_width(window: &Window) -> f32 {
    let text = window.text_system();
    let id = text.resolve_font(&grid_font());
    text.advance(id, px(FONT_SIZE), 'm').map(|s| f32::from(s.width)).unwrap_or(FONT_SIZE * 0.6)
}

fn paint(bounds: Bounds<Pixels>, screen: Screen, window: &mut Window, cx: &mut App) {
    let cw = screen.cell_width;
    let at = |row: usize, col: usize| {
        point(bounds.origin.x + px(col as f32 * cw), bounds.origin.y + px(row as f32 * LINE_HEIGHT))
    };
    for (r, row) in screen.cells.iter().enumerate() {
        let cursor_col = screen.cursor.filter(|c| c.0 as usize == r).map(|c| c.1 as usize);
        // Backgrounds: one rectangle per run of the same colour.
        let mut c = 0;
        while c < row.len() {
            let colour = background(&row[c]);
            let start = c;
            while c < row.len() && background(&row[c]) == colour {
                c += 1;
            }
            if let Some(colour) = colour {
                let area = size(px((c - start) as f32 * cw), px(LINE_HEIGHT));
                window.paint_quad(fill(Bounds { origin: at(r, start), size: area }, hsla(colour)));
            }
        }
        if let Some(col) = cursor_col {
            let width = if row.get(col).is_some_and(|cell| cell.wide) { 2.0 } else { 1.0 };
            let area = size(px(width * cw), px(LINE_HEIGHT));
            window
                .paint_quad(fill(Bounds { origin: at(r, col), size: area }, hsla(palette::CURSOR)));
        }
        // Text: runs of one style, each placed at its own column so a glyph
        // of another width never moves the ones after it.
        let mut c = 0;
        while c < row.len() {
            let start = c;
            let style = Style::of(&row[c], cursor_col == Some(c));
            let mut text = String::new();
            loop {
                let cell = &row[c];
                text.push_str(&cell.text);
                c += 1;
                let alone = !cell.text.is_ascii() || cursor_col == Some(start);
                if alone
                    || c >= row.len()
                    || !row[c].text.is_ascii()
                    || row[c].text.is_empty()
                    || cursor_col == Some(c)
                    || Style::of(&row[c], false) != style
                {
                    break;
                }
            }
            if text.trim().is_empty() && !style.underline && !style.strike {
                continue;
            }
            let run = style.run(text.len());
            let shaped = window.text_system().shape_line(
                SharedString::from(text),
                px(FONT_SIZE),
                &[run],
                None,
            );
            let _ = shaped.paint(at(r, start), px(LINE_HEIGHT), TextAlign::Left, None, window, cx);
        }
    }
}

fn background(cell: &weft_term::Cell) -> Option<weft_term::palette::Rgb> {
    if cell.selected { Some(palette::SELECTION) } else { cell.bg }
}

#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: weft_term::palette::Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

impl Style {
    /// A cell's style; under the cursor its text takes the background.
    fn of(cell: &weft_term::Cell, under_cursor: bool) -> Style {
        Style {
            fg: if under_cursor { palette::BACKGROUND } else { cell.fg },
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
            strike: cell.strike,
        }
    }

    fn run(&self, len: usize) -> TextRun {
        let mut f = grid_font();
        if self.bold {
            f.weight = FontWeight::BOLD;
        }
        if self.italic {
            f.style = FontStyle::Italic;
        }
        let color = hsla(self.fg);
        TextRun {
            len,
            font: f,
            color,
            background_color: None,
            underline: self.underline.then_some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: self
                .strike
                .then_some(StrikethroughStyle { thickness: px(1.0), color: Some(color) }),
        }
    }
}

fn hsla(c: weft_term::palette::Rgb) -> Hsla {
    Rgba { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a: 1.0 }.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_draws_each_character_in_its_own_cell_without_ligatures() {
        let f = grid_font();
        assert_eq!(f.family.as_ref(), crate::fonts::MONO);
        assert!(f.features.tag_value_list().contains(&("calt".to_string(), 0)));
    }

    #[test]
    fn a_rectangle_holds_whole_cells_only() {
        assert_eq!(grid_size(723.0, 541.0, 7.2, 18.0), (30, 100));
        assert_eq!(grid_size(0.0, 0.0, 7.2, 18.0), (1, 1), "never nothing");
    }

    /// The window's pane area cut as each split layout cuts it: every pane
    /// gets the size its own rectangle holds, and four panes four sizes.
    #[test]
    fn each_split_pane_gets_its_own_size() {
        let (w, h, cw, lh) = (1200.0, 720.0, 7.2, 18.0);
        let single = [grid_size(w, h, cw, lh)];
        let two_columns = [grid_size(w / 2.0, h, cw, lh), grid_size(w / 2.0, h, cw, lh)];
        let two_rows = [grid_size(w, h / 2.0, cw, lh), grid_size(w, h / 2.0, cw, lh)];
        let three_right = [
            grid_size(w / 2.0, h, cw, lh),
            grid_size(w / 2.0, h / 2.0, cw, lh),
            grid_size(w / 2.0, h / 2.0, cw, lh),
        ];
        assert_eq!(single, [(40, 166)]);
        assert_eq!(two_columns, [(40, 83), (40, 83)]);
        assert_eq!(two_rows, [(20, 166), (20, 166)]);
        assert_eq!(three_right, [(40, 83), (20, 83), (20, 83)]);
        // A grid whose panes the person dragged to four different sizes.
        let four: Vec<_> = [(700.0, 400.0), (500.0, 400.0), (700.0, 320.0), (500.0, 320.0)]
            .iter()
            .map(|&(pw, ph)| grid_size(pw, ph, cw, lh))
            .collect();
        assert_eq!(four, [(22, 97), (22, 69), (17, 97), (17, 69)]);
    }
}
