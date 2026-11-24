//! The files an agent named: a chip for each under its messages. On macOS a
//! chip opens its file in the agent's pane, beside the conversation, where a
//! page draws it; elsewhere, in the machine's own app.

use std::path::{Path, PathBuf};

use weft_core::files::{FileKind, Ref};

use crate::action::FileAct;
use crate::app::WeftDesktopApp;
use crate::gpui::{Element, Run, div, palette, rich};

/// A file's name: the last part of its path.
pub fn name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// A size as a chip shows it: `820 B`, `4.2 KB`, `41 KB`, `3.5 MB`.
pub fn size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    let (n, unit) = match bytes {
        0..1024 => return format!("{bytes} B"),
        1024..1_048_576 => (b / KB, "KB"),
        _ => (b / KB / KB, "MB"),
    };
    if n < 10.0 { format!("{n:.1} {unit}") } else { format!("{n:.0} {unit}") }
}

/// The glyph a chip shows for a kind.
fn glyph(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Markdown => "▤",
        FileKind::Document => "▧",
        FileKind::Notebook => "◧",
        FileKind::Pdf => "▥",
        FileKind::Sheet | FileKind::Csv => "▦",
        FileKind::Image => "▣",
        FileKind::Video => "▶",
        FileKind::Audio => "♪",
        FileKind::Html => "◫",
        FileKind::Other => "□",
        FileKind::Text => "≡",
    }
}

/// The chips for the files an item points at, wrapping onto more lines as
/// they need; each opens its file, and the one open beside them is marked.
pub fn chips(refs: &[&Ref], project: &Path, pane: u32, open: Option<&str>) -> Element {
    div().layer("files").self_stretch().flex_row().style("flex-wrap", "wrap").gap(8).children(
        refs.iter().map(|r| {
            let shown = open == Some(r.path.as_str());
            let open = FileAct::Open { project: project.to_path_buf(), pane, file: (*r).clone() };
            // One run of text, the glyph and the size dimmer than the name: a
            // message may name many files, and each chip is one box to lay out.
            let (glyph, name, size) = (glyph(r.kind), name(&r.path), size(r.size));
            let text = format!("{glyph} {name} {size}");
            let tail = text.len() - size.len();
            let runs = vec![
                (0..glyph.len(), Run { color: Some(palette::MUTED), ..Run::default() }),
                (tail..text.len(), Run { color: Some(palette::DIM), ..Run::default() }),
            ];
            rich(text, runs)
                .layer(if shown { "file open" } else { "file" })
                .aria_label(r.path.clone())
                .aria_selected(shown)
                .on_click(open)
                .h(28)
                .pad(0, 10)
                .style("max-width", "360px")
                .style("line-height", "26px")
                .style("white-space", "nowrap")
                .style("overflow", "hidden")
                .style("text-overflow", "ellipsis")
                .bg(if shown { palette::SELECTED_BG } else { palette::SURFACE })
                .border(if shown { palette::BLUE_BORDER } else { palette::LINE })
                .font_mono()
                .font_size(12)
                .color(if shown { palette::BLUE_BORDER } else { palette::TEXT })
        }),
    )
}

/// The file open in an agent's pane.
#[derive(Debug)]
pub struct Viewer {
    /// A new one each time a chip is clicked, so the page reads the file again.
    pub id: u64,
    pub project: PathBuf,
    pub pane: u32,
    pub file: Ref,
    /// Where the file is on this machine, found when its chip was clicked:
    /// the one file the viewer's page may read.
    pub abs: PathBuf,
}

impl Viewer {
    /// The page the viewer loads: the file's address and what is known of
    /// it, after `#` so the page reads them without asking.
    pub fn page(&self) -> String {
        let kind = serde_json::to_value(self.file.kind).ok();
        let fields = [
            ("u", "weft://file/0"),
            ("path", self.file.path.as_str()),
            ("kind", kind.as_ref().and_then(|k| k.as_str()).unwrap_or("other")),
            ("size", &self.file.size.to_string()),
            ("modified", &self.file.modified.to_string()),
        ];
        let query: Vec<String> =
            fields.iter().map(|(k, v)| format!("{k}={}", form_encoded(v))).collect();
        format!("weft://app/viewer.html#{}", query.join("&"))
    }
}

/// A value as a page's `URLSearchParams` writes it.
fn form_encoded(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The conversation's width beside a file, until the divider is dragged.
pub const CONVERSATION_WIDTH: f32 = 420.0;
/// The narrowest the conversation and the file each get beside the other.
pub const CONVERSATION_MIN: f32 = 280.0;
pub const FILE_MIN: f32 = 360.0;

/// The file open in each agent's pane, and what the window is to do for them.
pub struct Files {
    /// At most one for each agent.
    pub viewers: Vec<Viewer>,
    next: u64,
    /// The conversation's width beside a file, as the divider was left.
    pub width: f32,
    /// Files to hand to the machine's own app.
    pub outside: Vec<PathBuf>,
    /// Files to show in the machine's file manager.
    pub reveal: Vec<PathBuf>,
}

impl Default for Files {
    fn default() -> Self {
        Files {
            viewers: Vec::new(),
            next: 0,
            width: CONVERSATION_WIDTH,
            outside: Vec::new(),
            reveal: Vec::new(),
        }
    }
}

impl Files {
    pub fn get(&self, id: u64) -> Option<&Viewer> {
        self.viewers.iter().find(|v| v.id == id)
    }

    /// The file open in an agent's pane.
    pub fn of(&self, project: &Path, pane: u32) -> Option<&Viewer> {
        self.viewers.iter().find(|v| v.project == project && v.pane == pane)
    }

    pub fn close(&mut self, id: u64) {
        self.viewers.retain(|v| v.id != id);
    }

    /// The conversation's width beside a file in a pane `pane` wide: as the
    /// divider was left, leaving each side its least; `None` when the pane
    /// is too narrow for both, and the file covers the conversation.
    pub fn beside(&self, pane: f32) -> Option<f32> {
        (pane >= CONVERSATION_MIN + FILE_MIN)
            .then(|| self.width.clamp(CONVERSATION_MIN, pane - FILE_MIN))
    }
}

/// Where a file is on this machine: its canonical path when it is there.
fn on_disk(project: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    let p = if p.is_absolute() { p.to_path_buf() } else { project.join(p) };
    p.canonicalize().unwrap_or(p)
}

/// Whether this machine's viewer draws files: only a macOS window can hold
/// a page.
const VIEWER: bool = cfg!(target_os = "macos");

impl WeftDesktopApp {
    pub(crate) fn file_act(&mut self, act: FileAct) {
        match act {
            FileAct::Open { project, pane, file } => {
                let abs = on_disk(&project, &file.path);
                if !VIEWER {
                    self.files.outside.push(abs);
                    return;
                }
                // One file for each agent: the next one takes its place, and
                // the same one again is read again.
                self.files.viewers.retain(|v| !(v.project == project && v.pane == pane));
                self.files.next += 1;
                let id = self.files.next;
                self.files.viewers.push(Viewer { id, project, pane, file, abs });
            }
            FileAct::Reveal(id) => {
                if let Some(v) = self.files.get(id) {
                    let abs = v.abs.clone();
                    self.files.reveal.push(abs);
                }
            }
            FileAct::Close(id) => self.files.close(id),
            FileAct::Width(width) => self.files.width = width as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_a_chip_shows_them() {
        assert_eq!(size(820), "820 B");
        assert_eq!(size(4300), "4.2 KB");
        assert_eq!(size(41 * 1024), "41 KB");
        assert_eq!(size(3_670_016), "3.5 MB");
    }
}
