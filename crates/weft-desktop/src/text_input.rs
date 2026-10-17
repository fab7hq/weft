//! A text field on screen: its lines wrapped to the field's width, the
//! selection, the caret, and what an input method is composing underlined.
//!
//! The field's words live in a [`crate::text_field::TextField`]; this draws a
//! copy of them and, while the field has the keyboard, hands the window's text
//! input (typing, input methods, the character palette) to `view`.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use ::gpui::{
    App, AvailableSpace, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, PaintQuad, Pixels, Point, Rgba, SharedString, Style, TextAlign, TextRun,
    UnderlineStyle, Window, WrappedLine, fill, point, px, relative, size,
};

use crate::text_field::Shape;

/// How the field looks.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub line_height: f32,
    pub color: Rgba,
    pub placeholder: Rgba,
    pub caret: Rgba,
    pub selection: Rgba,
    /// A message grows to this many lines, then scrolls.
    pub max_lines: usize,
}

/// Where the field's lines were last painted, for the input method and the
/// pointer to measure against. Each line starts at its byte offset.
#[derive(Default)]
pub struct Painted {
    pub bounds: Option<Bounds<Pixels>>,
    lines: Vec<(usize, WrappedLine)>,
    line_height: Pixels,
    /// How far the text is moved up (a tall message) or left (a long line).
    scroll: Point<Pixels>,
}

impl Painted {
    /// Where index `ix` is drawn, in window coordinates: its row's top left.
    fn position(&self, ix: usize) -> Option<Point<Pixels>> {
        let bounds = self.bounds?;
        let mut top = Pixels::ZERO;
        for (start, line) in &self.lines {
            let end = start + line.len();
            if ix >= *start && ix <= end {
                let p = line.position_for_index(ix - start, self.line_height)?;
                return Some(bounds.origin + point(p.x, top + p.y) - self.scroll);
            }
            top += line.size(self.line_height).height;
        }
        None
    }

    /// The byte index nearest `at`, a point in window coordinates.
    pub fn index_for(&self, at: Point<Pixels>) -> Option<usize> {
        let bounds = self.bounds?;
        let local = at - bounds.origin + self.scroll;
        let mut top = Pixels::ZERO;
        for (i, (start, line)) in self.lines.iter().enumerate() {
            let height = line.size(self.line_height).height;
            let last = i + 1 == self.lines.len();
            if local.y < top + height || last {
                let inside = point(
                    local.x.max(Pixels::ZERO),
                    (local.y - top).clamp(Pixels::ZERO, height - px(1.)),
                );
                let ix = match line.closest_index_for_position(inside, self.line_height) {
                    Ok(ix) | Err(ix) => ix,
                };
                return Some(start + ix);
            }
            top += height;
        }
        Some(0)
    }

    /// The rectangle `range` takes, from its start: where an input method puts its window.
    pub fn bounds_for(&self, range: Range<usize>) -> Option<Bounds<Pixels>> {
        let a = self.position(range.start)?;
        let b = self.position(range.end).unwrap_or(a);
        let right = if b.y == a.y { b.x.max(a.x) } else { a.x };
        Some(Bounds::from_corners(a, point(right, a.y + self.line_height)))
    }
}

/// The field, drawn from a copy of its words.
pub struct FieldElement<V: 'static> {
    pub view: Entity<V>,
    pub focus: FocusHandle,
    /// It has the keyboard: the caret shows and typing comes to `view`.
    pub focused: bool,
    pub text: String,
    pub selection: Range<usize>,
    pub cursor: usize,
    pub marked: Option<Range<usize>>,
    pub placeholder: String,
    /// A suggestion drawn dim after the words, the caret staying before it.
    pub ghost: Option<String>,
    pub shape: Shape,
    pub look: Look,
    pub painted: Rc<RefCell<Painted>>,
}

impl<V: 'static> IntoElement for FieldElement<V>
where
    V: EntityInputHandler,
{
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The text to shape and its runs: the words, the composing part underlined,
/// or the placeholder when there are none.
fn runs<V>(field: &FieldElement<V>, window: &Window) -> (SharedString, Vec<TextRun>) {
    let style = window.text_style();
    let run = |len: usize, color: Rgba| TextRun {
        len,
        font: style.font(),
        color: Hsla::from(color),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let ghost = field.ghost.as_deref().filter(|g| !g.is_empty());
    if field.text.is_empty() && ghost.is_none() {
        let shown =
            if field.placeholder.is_empty() { " ".to_string() } else { field.placeholder.clone() };
        let len = shown.len();
        return (shown.into(), vec![run(len, field.look.placeholder)]);
    }
    let mut text = field.text.clone();
    let mut runs = Vec::new();
    match field.marked.clone().filter(|m| m.end <= text.len()) {
        Some(m) => {
            runs.push(run(m.start, field.look.color));
            let mut composing = run(m.end - m.start, field.look.color);
            composing.underline = Some(UnderlineStyle {
                color: Some(Hsla::from(field.look.color)),
                thickness: px(1.),
                wavy: false,
            });
            runs.push(composing);
            runs.push(run(text.len() - m.end, field.look.color));
        }
        None => runs.push(run(text.len(), field.look.color)),
    }
    if let Some(g) = ghost {
        text.push_str(g);
        runs.push(run(g.len(), field.look.placeholder));
    }
    runs.retain(|r| r.len > 0);
    (text.into(), runs)
}

/// Shape the field's lines, wrapped at `width` when it is a message.
fn shape<V>(
    field: &FieldElement<V>,
    width: Option<Pixels>,
    window: &Window,
) -> Vec<(usize, WrappedLine)> {
    let (text, runs) = runs(field, window);
    let font_size = window.text_style().font_size.to_pixels(window.rem_size());
    let wrap = match field.shape {
        Shape::Area => width,
        Shape::Line => None,
    };
    let lines =
        window.text_system().shape_text(text, font_size, &runs, wrap, None).unwrap_or_default();
    let mut start = 0;
    lines
        .into_iter()
        .map(|line| {
            let at = start;
            start += line.len() + 1;
            (at, line)
        })
        .collect()
}

impl<V> Element for FieldElement<V>
where
    V: EntityInputHandler,
{
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        // Beside what sits in the field with it, it takes the room that is left.
        style.min_size.width = px(0.).into();
        style.flex_shrink = 1.;
        let line_height = px(self.look.line_height);
        let (rows_of, max) = (self.shape, self.look.max_lines.max(1));
        // A copy for the measure, which may run after this element is gone.
        let copy = FieldElement {
            view: self.view.clone(),
            focus: self.focus.clone(),
            focused: self.focused,
            text: self.text.clone(),
            selection: self.selection.clone(),
            cursor: self.cursor,
            marked: self.marked.clone(),
            placeholder: self.placeholder.clone(),
            ghost: self.ghost.clone(),
            shape: self.shape,
            look: self.look,
            painted: Rc::clone(&self.painted),
        };
        let id = window.request_measured_layout(style, move |known, available, window, _cx| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let rows = match rows_of {
                Shape::Line => 1,
                Shape::Area => shape(&copy, width, window)
                    .iter()
                    .map(|(_, l)| l.wrap_boundaries().len() + 1)
                    .sum::<usize>(),
            };
            size(width.unwrap_or(Pixels::ZERO), line_height * rows.clamp(1, max) as f32)
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) {
        let lines = shape(self, Some(bounds.size.width), window);
        let mut painted = self.painted.borrow_mut();
        painted.lines = lines;
        painted.line_height = px(self.look.line_height);
        painted.bounds = Some(bounds);
        // Keep the caret in sight: a tall message scrolls up, a long line left.
        painted.scroll = Point::default();
        if !self.text.is_empty()
            && let Some(caret) = painted.position(self.cursor)
        {
            let local = caret - bounds.origin;
            let below = local.y + painted.line_height - bounds.size.height;
            let past = local.x + px(2.) - bounds.size.width;
            painted.scroll = point(past.max(Pixels::ZERO), below.max(Pixels::ZERO));
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.focused {
            window.handle_input(
                &self.focus,
                ElementInputHandler::new(bounds, self.view.clone()),
                cx,
            );
        }
        let painted = self.painted.borrow();
        let lh = painted.line_height;
        let mut quads: Vec<PaintQuad> = Vec::new();
        if self.focused && !self.text.is_empty() && !self.selection.is_empty() {
            for (start, line) in &painted.lines {
                let end = start + line.len();
                let (a, b) = (self.selection.start.max(*start), self.selection.end.min(end));
                if a >= b && !(self.selection.start <= end && self.selection.end > end) {
                    continue;
                }
                let (Some(from), Some(to)) = (painted.position(a), painted.position(b.max(a)))
                else {
                    continue;
                };
                let mut y = from.y;
                while y <= to.y {
                    let left = if y == from.y { from.x } else { bounds.left() - painted.scroll.x };
                    let right = if y == to.y { to.x.max(left + px(4.)) } else { bounds.right() };
                    quads.push(fill(
                        Bounds::from_corners(point(left, y), point(right, y + lh)),
                        self.look.selection,
                    ));
                    y += lh;
                }
            }
        }
        let caret = (self.focused && self.selection.is_empty())
            .then(|| {
                if self.text.is_empty() {
                    Some(bounds.origin)
                } else {
                    painted.position(self.cursor)
                }
            })
            .flatten()
            .map(|at| fill(Bounds::new(at, size(px(1.5), lh)), self.look.caret));
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for q in quads {
                window.paint_quad(q);
            }
            let mut top = bounds.top() - painted.scroll.y;
            for (_, line) in &painted.lines {
                let _ = line.paint(
                    point(bounds.left() - painted.scroll.x, top),
                    lh,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                top += line.size(lh).height;
            }
            if let Some(c) = caret {
                window.paint_quad(c);
            }
        });
    }
}

/// UTF-16 offsets, as the input method counts, and byte offsets, as the field does.
pub fn to_utf16(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].chars().map(char::len_utf16).sum()
}

pub fn from_utf16(text: &str, utf16: usize) -> usize {
    let mut count = 0;
    for (i, c) in text.char_indices() {
        if count >= utf16 {
            return i;
        }
        count += c.len_utf16();
    }
    text.len()
}

pub fn range_to_utf16(text: &str, r: &Range<usize>) -> Range<usize> {
    to_utf16(text, r.start)..to_utf16(text, r.end)
}

pub fn range_from_utf16(text: &str, r: &Range<usize>) -> Range<usize> {
    from_utf16(text, r.start)..from_utf16(text, r.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_count_utf16_for_the_input_method_and_bytes_for_the_field() {
        let t = "a日👍b";
        assert_eq!(to_utf16(t, 0), 0);
        assert_eq!(to_utf16(t, 1), 1);
        assert_eq!(to_utf16(t, 4), 2, "日 is one UTF-16 unit and three bytes");
        assert_eq!(to_utf16(t, 8), 4, "👍 is two UTF-16 units and four bytes");
        assert_eq!(from_utf16(t, 4), 8);
        assert_eq!(from_utf16(t, 99), t.len());
        assert_eq!(range_from_utf16(t, &(1..2)), 1..4);
    }
}
