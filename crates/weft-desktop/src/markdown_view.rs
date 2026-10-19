//! An agent's message drawn from its markdown: headings, lists, code, quotes
//! and tables as blocks, bold, italic, code and links within a line.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use weft_core::markdown::{Block, Span, blocks};

use crate::gpui::{Element, Run, div, edge_bar, palette, rich};

/// How many messages' blocks are kept parsed.
const KEPT: usize = 512;

thread_local! {
    /// Each message's blocks by its text: a window draws every message each
    /// frame, and parses only the ones it has not seen.
    static PARSED: RefCell<HashMap<String, Rc<Vec<Block>>>> = RefCell::default();
}

/// An agent's message, from its text.
pub fn message(text: &str) -> Element {
    column(&parsed(text))
}

/// `text`'s blocks, parsed once while kept. When more are kept than
/// `KEPT`, all go and are parsed again as they are drawn.
fn parsed(text: &str) -> Rc<Vec<Block>> {
    PARSED.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(b) = cache.get(text) {
            return Rc::clone(b);
        }
        if cache.len() >= KEPT {
            cache.clear();
        }
        let b = Rc::new(blocks(text));
        cache.insert(text.to_string(), Rc::clone(&b));
        b
    })
}

fn column(blocks: &[Block]) -> Element {
    div()
        .self_stretch()
        .flex_col()
        .gap(8)
        .items_start()
        .children(blocks.iter().map(block).collect::<Vec<_>>())
}

fn block(b: &Block) -> Element {
    match b {
        Block::Paragraph(spans) => {
            line(spans).layer("md paragraph").font_size(14).style("line-height", "21px")
        }
        Block::Heading { level, spans } => line(spans)
            .layer("md heading")
            .font_weight(600)
            .font_size(match level {
                1 => 18,
                2 => 16,
                _ => 15,
            })
            .style("line-height", "24px"),
        Block::List { start, items } => {
            div().layer("md list").self_stretch().flex_col().gap(4).children(
                items
                    .iter()
                    .enumerate()
                    .map(|(n, item)| {
                        let marker = match start {
                            Some(first) => format!("{}.", first + n as u64),
                            None => "•".to_string(),
                        };
                        div()
                            .self_stretch()
                            .flex_row()
                            .gap(8)
                            .items_start()
                            .child(
                                div()
                                    .shrink_0()
                                    .font_size(14)
                                    .style("line-height", "21px")
                                    .color(palette::MUTED)
                                    .text(marker),
                            )
                            .child(column(item).flex_1().style("min-width", "0"))
                    })
                    .collect::<Vec<_>>(),
            )
        }
        Block::Code { text } => div()
            .layer("md code")
            .self_stretch()
            .pad(10, 12)
            .bg(palette::SURFACE)
            .border(palette::LINE)
            .font_mono()
            .font_size(12)
            .style("line-height", "18px")
            .style("white-space", "pre")
            .text(text.trim_end_matches('\n').to_string()),
        Block::Quote(inner) => div()
            .layer("md quote")
            .self_stretch()
            .style("position", "relative")
            .style("padding", "0 0 0 14px")
            .color(palette::MUTED)
            .child(edge_bar("left", 2, palette::LINE))
            .child(column(inner)),
        Block::Rule => div().layer("md rule").self_stretch().h(1).bg(palette::LINE),
        Block::Table { head, rows } => {
            let row = |cells: &[Vec<Span>], bold: bool| {
                div().self_stretch().flex_row().border_bottom(palette::LINE).children(
                    cells
                        .iter()
                        .map(|c| {
                            let cell =
                                line(c).flex_1().style("min-width", "0").pad(6, 10).font_size(13);
                            if bold { cell.font_weight(600) } else { cell }
                        })
                        .collect::<Vec<_>>(),
                )
            };
            div()
                .layer("md table")
                .self_stretch()
                .flex_col()
                .border(palette::LINE)
                .child(row(head, true))
                .children(rows.iter().map(|r| row(r, false)).collect::<Vec<_>>())
        }
    }
}

/// A line of spans: rich text, each span's range in its own style.
fn line(spans: &[Span]) -> Element {
    let mut text = String::new();
    let mut runs = Vec::new();
    for s in spans {
        let at = text.len();
        text.push_str(&s.text);
        let run = Run {
            bold: s.bold,
            italic: s.italic,
            mono: s.code,
            strike: s.strike,
            underline: s.link.is_some(),
            color: if s.link.is_some() { Some(palette::BLUE_BORDER) } else { None },
            // Code reads as a chip, as on the conversation's card.
            background: s.code.then_some(palette::SURFACE),
        };
        if run != Run::default() {
            runs.push((at..text.len(), run));
        }
    }
    rich(text, runs).self_stretch()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_drawn_from_its_markdown() {
        let el = message(
            "## Done\n\nAdded **/health**; `cargo test` passes.\n\n- one\n- two\n\n```\nfn main() {}\n```\n\n> quoted\n\n| file | change |\n| --- | --- |\n| a.rs | new |\n",
        );
        for layer in ["md heading", "md list", "md code", "md quote", "md table"] {
            assert!(el.find_layer(layer).is_some(), "{layer}");
        }
        let text = el.all_text();
        for raw in ["##", "**", "```", "| ---"] {
            assert!(!text.contains(raw), "{raw} in {text}");
        }
        let para = el.find_layer("md paragraph").expect("the paragraph");
        assert_eq!(para.text.as_deref(), Some("Added /health; cargo test passes."));
        let styled: Vec<_> = para
            .runs
            .iter()
            .map(|(r, s)| (&para.text.as_deref().unwrap()[r.clone()], s.bold, s.mono))
            .collect();
        assert_eq!(styled, [("/health", true, false), ("cargo test", false, true)]);
    }

    #[test]
    fn a_message_is_parsed_once_for_as_long_as_its_text_is_the_same() {
        let first = parsed("Done. **Both** pass.");
        assert!(std::rc::Rc::ptr_eq(&first, &parsed("Done. **Both** pass.")));
        assert!(!std::rc::Rc::ptr_eq(&first, &parsed("Done. **Both** pass. More")));
        for n in 0..=KEPT {
            parsed(&format!("message {n}"));
        }
        assert!(PARSED.with(|p| p.borrow().len()) <= KEPT, "the cache is bounded");
    }
}
