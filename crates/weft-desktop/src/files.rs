//! The files an agent pointed at: a chip for each under its messages and
//! steps. On macOS a chip opens its file in the viewer, a window of its own
//! whose page draws it; elsewhere, in the machine's own app.

use std::path::{Path, PathBuf};

use weft_core::conversation::Item;
use weft_core::files::{FileKind, Ref};

use crate::action::FileAct;
use crate::app::WeftDesktopApp;
use crate::gpui::{Element, div, palette};

/// The files a run of steps wrote, each once, in the order they were first
/// written.
pub fn written<'a>(steps: &[&'a Item]) -> Vec<&'a Ref> {
    let mut out: Vec<&Ref> = Vec::new();
    for r in steps.iter().flat_map(|s| &s.refs) {
        if !out.iter().any(|o| o.path == r.path) {
            out.push(r);
        }
    }
    out
}

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
/// they need; each opens its file.
pub fn chips(refs: &[&Ref], project: &Path, pane: u32, harness: &str) -> Element {
    div().layer("files").self_stretch().flex_row().style("flex-wrap", "wrap").gap(8).children(
        refs.iter().map(|r| {
            let open = FileAct::Open {
                project: project.to_path_buf(),
                pane,
                harness: harness.to_string(),
                file: (*r).clone(),
            };
            div()
                .layer("file")
                .aria_label(r.path.clone())
                .on_click(open)
                .flex_row()
                .items_center()
                .gap(7)
                .h(28)
                .pad(0, 10)
                .style("max-width", "360px")
                .bg(palette::SURFACE)
                .border(palette::LINE)
                .font_mono()
                .font_size(12)
                .color(palette::TEXT)
                .child(div().shrink_0().color(palette::MUTED).text(glyph(r.kind)))
                .child(
                    div()
                        .style("min-width", "0")
                        .style("white-space", "nowrap")
                        .style("overflow", "hidden")
                        .style("text-overflow", "ellipsis")
                        .text(name(&r.path).to_string()),
                )
                .child(div().shrink_0().font_size(11).color(palette::DIM).text(size(r.size)))
        }),
    )
}

/// One file open in the viewer.
#[derive(Debug)]
pub struct Viewer {
    pub id: u64,
    pub project: PathBuf,
    /// The project's name, for the window's title.
    pub project_name: String,
    pub pane: u32,
    pub harness: String,
    pub file: Ref,
    /// Where the file is on this machine, found when its chip was clicked:
    /// the one file the viewer's page may read.
    pub abs: PathBuf,
}

impl Viewer {
    /// The window's title: the file's name and its project's.
    pub fn title(&self) -> String {
        format!("{} — {}", name(&self.file.path), self.project_name)
    }

    /// The page the viewer's window loads: the file's address and what is
    /// known of it, after `#` so the page reads them without asking.
    pub fn page(&self) -> String {
        let kind = serde_json::to_value(self.file.kind).ok();
        let fields = [
            ("u", "weft://file/0"),
            ("path", self.file.path.as_str()),
            ("kind", kind.as_ref().and_then(|k| k.as_str()).unwrap_or("other")),
            ("size", &self.file.size.to_string()),
            ("modified", &self.file.modified.to_string()),
            ("agent", self.harness.as_str()),
            ("project", self.project_name.as_str()),
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

/// The open viewers, and what the windows are to do for them.
#[derive(Default)]
pub struct Files {
    pub viewers: Vec<Viewer>,
    next: u64,
    /// Viewers whose window is to open, or come in front.
    pub to_open: Vec<u64>,
    /// Files to hand to the machine's own app.
    pub outside: Vec<PathBuf>,
    /// Files to show in the machine's file manager.
    pub reveal: Vec<PathBuf>,
}

impl Files {
    pub fn get(&self, id: u64) -> Option<&Viewer> {
        self.viewers.iter().find(|v| v.id == id)
    }

    /// The viewer is gone: its window closed.
    pub fn close(&mut self, id: u64) {
        self.viewers.retain(|v| v.id != id);
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
            FileAct::Open { project, pane, harness, file } => {
                let abs = on_disk(&project, &file.path);
                if !VIEWER {
                    self.files.outside.push(abs);
                    return;
                }
                let project_name =
                    self.client(&project).map(|c| c.name.clone()).unwrap_or_default();
                // A file already open comes in front: one window for it.
                let open = self.files.viewers.iter().position(|v| {
                    v.project == project && v.pane == pane && v.file.path == file.path
                });
                let id = match open {
                    Some(at) => {
                        let v = &mut self.files.viewers[at];
                        (v.file, v.harness, v.abs) = (file, harness, abs);
                        v.id
                    }
                    None => {
                        self.files.next += 1;
                        let id = self.files.next;
                        self.files.viewers.push(Viewer {
                            id,
                            project,
                            project_name,
                            pane,
                            harness,
                            file,
                            abs,
                        });
                        id
                    }
                };
                self.files.to_open.push(id);
            }
            FileAct::Reveal(id) => {
                if let Some(v) = self.files.get(id) {
                    let abs = v.abs.clone();
                    self.files.reveal.push(abs);
                }
            }
            FileAct::Close(id) => self.files.close(id),
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
