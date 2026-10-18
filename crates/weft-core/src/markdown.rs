//! What an agent said, as blocks a screen draws: its markdown read the way a
//! terminal shows it. Both clients draw these, so a message reads the same in
//! the app and on a device.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::Serialize;

/// One block of a message.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Paragraph(Vec<Span>),
    /// `start` is an ordered list's first number; none for bullets.
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Code {
        text: String,
    },
    Quote(Vec<Block>),
    Rule,
    Table {
        head: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
}

/// A run of text in one style.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Span {
    pub text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub code: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strike: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// A message's text as blocks. Its line breaks are kept, as a terminal keeps
/// them; HTML is text.
pub fn blocks(text: &str) -> Vec<Block> {
    let events: Vec<Event> = Parser::new_ext(
        text,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    )
    .collect();
    let mut at = 0;
    read_blocks(&events, &mut at)
}

/// Blocks until the end of the container the reader is in.
fn read_blocks(ev: &[Event], at: &mut usize) -> Vec<Block> {
    let mut out = Vec::new();
    while let Some(e) = ev.get(*at) {
        match e {
            Event::End(_) => return out,
            Event::Start(Tag::Paragraph) => {
                *at += 1;
                out.push(Block::Paragraph(read_spans(ev, at)));
                *at += 1;
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let level = *level as u8;
                *at += 1;
                out.push(Block::Heading { level, spans: read_spans(ev, at) });
                *at += 1;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                *at += 1;
                out.push(Block::Quote(read_blocks(ev, at)));
                *at += 1;
            }
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => {
                let code = matches!(e, Event::Start(Tag::CodeBlock(_)));
                *at += 1;
                let mut text = String::new();
                while let Some(Event::Text(t) | Event::Html(t)) = ev.get(*at) {
                    text.push_str(t);
                    *at += 1;
                }
                *at += 1;
                out.push(if code {
                    Block::Code { text }
                } else {
                    Block::Paragraph(vec![Span {
                        text: text.trim_end().to_string(),
                        ..Span::default()
                    }])
                });
            }
            Event::Start(Tag::List(start)) => {
                let start = *start;
                *at += 1;
                let mut items = Vec::new();
                while let Some(Event::Start(Tag::Item)) = ev.get(*at) {
                    *at += 1;
                    items.push(read_blocks(ev, at));
                    *at += 1;
                }
                *at += 1;
                out.push(Block::List { start, items });
            }
            Event::Start(Tag::Table(_)) => {
                *at += 1;
                let (mut head, mut rows) = (Vec::new(), Vec::new());
                while let Some(Event::Start(Tag::TableHead | Tag::TableRow)) = ev.get(*at) {
                    let is_head = matches!(ev[*at], Event::Start(Tag::TableHead));
                    *at += 1;
                    let mut cells = Vec::new();
                    while let Some(Event::Start(Tag::TableCell)) = ev.get(*at) {
                        *at += 1;
                        cells.push(read_spans(ev, at));
                        *at += 1;
                    }
                    *at += 1;
                    if is_head { head = cells } else { rows.push(cells) }
                }
                *at += 1;
                out.push(Block::Table { head, rows });
            }
            Event::Rule => {
                *at += 1;
                out.push(Block::Rule);
            }
            // Any other container: what it holds, as blocks.
            Event::Start(
                Tag::FootnoteDefinition(_)
                | Tag::DefinitionList
                | Tag::DefinitionListTitle
                | Tag::DefinitionListDefinition
                | Tag::MetadataBlock(_),
            ) => {
                *at += 1;
                out.extend(read_blocks(ev, at));
                *at += 1;
            }
            // Text with no paragraph of its own: a tight list item's. An event
            // neither reader knows is passed over.
            _ => {
                let from = *at;
                let spans = read_spans(ev, at);
                if *at == from {
                    *at += 1;
                } else {
                    out.push(Block::Paragraph(spans));
                }
            }
        }
    }
    out
}

/// Spans until the end of the block they are in, or the next block.
fn read_spans(ev: &[Event], at: &mut usize) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut style = Span::default();
    let push = |out: &mut Vec<Span>, style: &Span, text: &str, code: bool| {
        let span = Span { text: text.to_string(), code, ..style.clone() };
        match out.last_mut() {
            Some(last)
                if Span { text: String::new(), ..last.clone() }
                    == Span { text: String::new(), ..span.clone() } =>
            {
                last.text.push_str(text)
            }
            _ => out.push(span),
        }
    };
    while let Some(e) = ev.get(*at) {
        match e {
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => {
                push(&mut out, &style, t, false)
            }
            Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                push(&mut out, &style, t, true)
            }
            Event::SoftBreak | Event::HardBreak => push(&mut out, &style, "\n", false),
            Event::TaskListMarker(done) => {
                push(&mut out, &style, if *done { "☑ " } else { "☐ " }, false)
            }
            Event::FootnoteReference(t) => push(&mut out, &style, &format!("[{t}]"), false),
            Event::Start(Tag::Emphasis) => style.italic = true,
            Event::Start(Tag::Strong) => style.bold = true,
            Event::Start(Tag::Strikethrough) => style.strike = true,
            Event::Start(Tag::Link { dest_url, .. }) => style.link = Some(dest_url.to_string()),
            Event::Start(Tag::Superscript | Tag::Subscript | Tag::Image { .. }) => {}
            Event::End(TagEnd::Emphasis) => style.italic = false,
            Event::End(TagEnd::Strong) => style.bold = false,
            Event::End(TagEnd::Strikethrough) => style.strike = false,
            Event::End(TagEnd::Link) => style.link = None,
            Event::End(TagEnd::Superscript | TagEnd::Subscript | TagEnd::Image) => {}
            _ => break,
        }
        *at += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn a_message_reads_as_its_markdown() {
        let text = "## Done\n\nAdded **/health** and `cargo test` passes.\n\n- one\n- two with *care*\n\n1. first\n2. second\n\n```rust\nfn main() {}\n```\n\n> quoted\n\n---\n\n| file | change |\n| --- | --- |\n| a.rs | new |\n";
        let b = blocks(text);
        assert_eq!(b.len(), 8, "{b:#?}");
        assert!(matches!(&b[0], Block::Heading { level: 2, spans } if plain(spans) == "Done"));
        let Block::Paragraph(p) = &b[1] else { panic!("{:?}", b[1]) };
        assert_eq!(plain(p), "Added /health and cargo test passes.");
        assert!(p.iter().any(|s| s.bold && s.text == "/health"));
        assert!(p.iter().any(|s| s.code && s.text == "cargo test"));
        let Block::List { start: None, items } = &b[2] else { panic!("{:?}", b[2]) };
        assert_eq!(items.len(), 2);
        assert!(
            matches!(&items[1][..], [Block::Paragraph(s)] if s.iter().any(|s| s.italic && s.text == "care"))
        );
        assert!(matches!(&b[3], Block::List { start: Some(1), items } if items.len() == 2));
        assert!(matches!(&b[4], Block::Code { text } if text == "fn main() {}\n"));
        assert!(matches!(&b[5], Block::Quote(q) if matches!(&q[..], [Block::Paragraph(_)])));
        assert!(matches!(&b[6], Block::Rule));
        let Block::Table { head, rows } = &b[7] else { panic!("{:?}", b[7]) };
        assert_eq!(head.iter().map(|c| plain(c)).collect::<Vec<_>>(), ["file", "change"]);
        assert_eq!(rows[0].iter().map(|c| plain(c)).collect::<Vec<_>>(), ["a.rs", "new"]);
    }

    /// Text that is not markdown reads as it was written: its lines kept,
    /// and HTML shown as text, never acted on.
    #[test]
    fn plain_text_and_html_read_as_written() {
        let b = blocks("first line\nsecond line\n\n<b>not bold</b>");
        let Block::Paragraph(p) = &b[0] else { panic!("{b:?}") };
        assert_eq!(plain(p), "first line\nsecond line");
        assert!(
            b.iter().any(|x| matches!(x, Block::Paragraph(s) if plain(s) == "<b>not bold</b>")),
            "{b:?}"
        );
        assert!(blocks("").is_empty());
    }
}
