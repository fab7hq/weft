//! The files an agent's conversation points at: which kinds Weft knows, and
//! the paths a message names. Pure: nothing here touches the disk; weftd
//! finds which of these paths are files.
//!
//! The kinds are data (`files.json`), read by both clients, so they agree on
//! what they draw and what they hand to the device's own app.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// What a file is, for drawing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Markdown,
    Text,
    Image,
    Video,
    Audio,
    Csv,
    Sheet,
    /// A Word document.
    Document,
    /// A Jupyter notebook.
    Notebook,
    Pdf,
    Html,
    /// Known, and handed to the device's own app.
    Other,
}

/// A file an item points at, as a device receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    /// Relative to the project when the file is inside it, else absolute.
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    /// Milliseconds since the epoch.
    pub modified: i64,
}

fn table() -> &'static BTreeMap<String, FileKind> {
    static TABLE: OnceLock<BTreeMap<String, FileKind>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let kinds: BTreeMap<String, Vec<String>> =
            serde_json::from_str(include_str!("../files.json")).expect("files.json");
        let mut table = BTreeMap::new();
        for (name, exts) in kinds {
            let kind: FileKind =
                serde_json::from_value(serde_json::Value::String(name)).expect("a kind");
            table.extend(exts.into_iter().map(|e| (e, kind)));
        }
        table
    })
}

/// The kind of a path by its extension, when Weft knows it.
pub fn kind_of(path: &str) -> Option<FileKind> {
    let name = path.rsplit(['/', '\\']).next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() && !name[1..].contains('.') {
        // A dotfile such as `.env` has no extension.
        return None;
    }
    table().get(&ext.to_ascii_lowercase()).copied()
}

/// The media type a file is served with, by its extension: what a browser
/// needs to draw or play it. Unknown ones are plain bytes.
pub fn mime_of(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" | "mdx" => "text/markdown; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "tsv" => "text/tab-separated-values; charset=utf-8",
        "json" | "ipynb" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ if kind_of(path) == Some(FileKind::Text) => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The kind, corrected by the file's first bytes: a "text" file holding a
/// NUL byte is not text.
pub fn sniffed(kind: FileKind, first: &[u8]) -> FileKind {
    if kind == FileKind::Text && first.contains(&0) { FileKind::Other } else { kind }
}

/// The paths a message names: its inline code, its links' targets and its
/// `file://` addresses, each cleaned, in the order first named. Code blocks
/// and plain words are not read.
pub fn named_in(text: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Parser, Tag};
    let mut found: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        if let Some(p) = clean(raw)
            && !found.contains(&p)
        {
            found.push(p);
        }
    };
    let mut in_block = 0usize;
    for event in Parser::new(text) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_block += 1,
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => in_block = in_block.saturating_sub(1),
            Event::Code(code) if in_block == 0 => push(&code),
            Event::Start(Tag::Link { dest_url, .. }) => push(&dest_url),
            Event::Text(t) if in_block == 0 => {
                for word in t.split_whitespace().filter(|w| w.contains("file://")) {
                    push(&word[word.find("file://").unwrap_or(0)..]);
                }
            }
            _ => {}
        }
    }
    found
}

/// A named path made plain, or none when it cannot be a file Weft knows:
/// quotes, `file://`, percent escapes, a `#L12` anchor and a `:12:3` line
/// suffix go; another scheme, a glob or an unknown extension is refused.
pub fn clean(raw: &str) -> Option<String> {
    let mut p = raw.trim().trim_matches(['"', '\'', '`', '<', '>']);
    p = p.trim_end_matches(['.', ',', ';', ')', ']']);
    if p.is_empty() || p.len() > 4096 || p.contains('\n') {
        return None;
    }
    let mut owned = match p.strip_prefix("file://") {
        Some(rest) => decode(rest.strip_prefix("localhost").unwrap_or(rest))?,
        None if p.contains("://") || p.starts_with("mailto:") => return None,
        None => p.to_string(),
    };
    if let Some(at) = owned.find("#L") {
        owned.truncate(at);
    }
    let owned = strip_line(&owned).to_string();
    if owned.contains(['*', '?']) || owned.ends_with('/') {
        return None;
    }
    kind_of(&owned).map(|_| owned)
}

/// Without a trailing `:12` or `:12:3`.
fn strip_line(p: &str) -> &str {
    let mut p = p;
    for _ in 0..2 {
        match p.rsplit_once(':') {
            Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => p = head,
            _ => break,
        }
    }
    p
}

/// `%20` and the like decoded; none when the result is not UTF-8.
fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_comes_from_the_extension_in_any_case() {
        assert_eq!(kind_of("out/report.md"), Some(FileKind::Markdown));
        assert_eq!(kind_of("Q3.XLSX"), Some(FileKind::Sheet));
        assert_eq!(kind_of("/tmp/a/board.pdf"), Some(FileKind::Pdf));
        assert_eq!(kind_of("src/main.rs"), Some(FileKind::Text));
        assert_eq!(kind_of("deck.pptx"), Some(FileKind::Other));
        assert_eq!(kind_of("e2e/flow.MP4"), Some(FileKind::Video));
        assert_eq!(kind_of("note.m4a"), Some(FileKind::Audio));
        assert_eq!(kind_of("proposal.docx"), Some(FileKind::Document));
        assert_eq!(kind_of("q3.ipynb"), Some(FileKind::Notebook));
        assert_eq!(kind_of("Makefile"), None);
        assert_eq!(kind_of(".env"), None);
        assert_eq!(kind_of("v1.2"), None);
        assert_eq!(kind_of("archive.unknown"), None);
    }

    #[test]
    fn a_file_is_served_as_its_media_type() {
        assert_eq!(mime_of("e2e/flow.MP4"), "video/mp4");
        assert_eq!(mime_of("out/report.html"), "text/html; charset=utf-8");
        assert_eq!(mime_of("src/main.rs"), "text/plain; charset=utf-8");
        assert_eq!(mime_of("archive.unknown"), "application/octet-stream");
    }

    #[test]
    fn a_text_file_with_a_nul_byte_is_not_text() {
        assert_eq!(sniffed(FileKind::Text, b"fn main() {}"), FileKind::Text);
        assert_eq!(sniffed(FileKind::Text, b"\x00\x01"), FileKind::Other);
        assert_eq!(sniffed(FileKind::Pdf, b"\x00"), FileKind::Pdf);
    }

    #[test]
    fn only_code_links_and_file_addresses_name_a_file() {
        let text = "I wrote `out/report.md` and [the sheet](out/q3.xlsx), see \
                    file:///tmp/board%20pack.pdf. Plain report.md is a word; \
                    `cargo test -p app` is a command.\n\n```\nsrc/hidden.rs\n```\n\
                    Again `out/report.md`, at `src/login.rs:12:3` and \
                    [web](https://example.com/page.html).";
        assert_eq!(
            named_in(text),
            ["out/report.md", "out/q3.xlsx", "/tmp/board pack.pdf", "src/login.rs"]
        );
    }

    #[test]
    fn cleaning_drops_anchors_quotes_and_refuses_what_is_no_file() {
        assert_eq!(clean("\"src/a.rs#L10\"").as_deref(), Some("src/a.rs"));
        assert_eq!(
            clean("~/Downloads/q2-final.xlsx,").as_deref(),
            Some("~/Downloads/q2-final.xlsx")
        );
        assert_eq!(clean("file://localhost/tmp/x.md").as_deref(), Some("/tmp/x.md"));
        assert_eq!(clean("src/*.rs"), None);
        assert_eq!(clean("http://a.b/c.html"), None);
        assert_eq!(clean("out/"), None);
        assert_eq!(clean(""), None);
    }
}
