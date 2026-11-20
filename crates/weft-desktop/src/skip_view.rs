//! Files an Eval should skip, as the Eval dialog and a project's settings
//! show them, each with the line that keeps it out; and what an Eval
//! counted and did not read, in its Details.

use std::path::Path;

use serde_json::Value;
use weft_core::skip::{Finding, NotRead};
use weft_core::words::{count, fill, word};

use crate::action::SheetAct;
use crate::daemon::{ProjectClient, Then};
use crate::gpui::{Element, Run, div, palette, rich};

/// Where findings are shown: in the Eval dialog's box, or in a project's
/// settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Dialog,
    Settings,
}

/// The file of a line that keeps a folder out of Git altogether.
const IGNORE: &str = ".gitignore";

/// Text on one line, cut with an ellipsis where it does not fit.
pub fn one_line(e: Element) -> Element {
    e.style("min-width", "0")
        .style("white-space", "nowrap")
        .style("overflow", "hidden")
        .style("text-overflow", "ellipsis")
}

/// A size of text as a person reads it: "48 MB".
fn size(bytes: u64) -> String {
    let mb = count((bytes.saturating_add(500_000) / 1_000_000).max(1) as usize);
    fill("size.mb", &[("n", &mb)])
}

/// What a finding holds, said after its path: "329 files Git does not
/// track", "13,708 data files (json)", "is its own repository: …".
pub fn what(f: &Finding) -> String {
    let n = count(f.files);
    match f.kind.as_str() {
        "untracked_bulk" => fill("skip.untracked", &[("n", &n)]),
        "data_bulk" if !f.kinds_of_files.is_empty() => {
            let kinds: Vec<&str> =
                f.kinds_of_files.iter().take(3).map(|(ext, _)| ext.as_str()).collect();
            fill("skip.data_files", &[("n", &n), ("kinds", &kinds.join(", "))])
        }
        "committed_build" => fill("skip.built", &[("n", &n)]),
        "committed_media" => fill("skip.media", &[("n", &n)]),
        "large_text" => fill("skip.large_text", &[("size", &size(f.bytes))]),
        "sub_repo" => match (f.files, f.commits) {
            (1, _) => word("skip.sub_repo_one").to_string(),
            (0, 1) => word("skip.sub_repo_commit").to_string(),
            (0, c) => fill("skip.sub_repo_commits", &[("n", &count(c))]),
            _ => fill("skip.sub_repo", &[("n", &n)]),
        },
        _ if f.files == 1 => word("reads.one_file").to_string(),
        _ => fill("reads.files", &[("n", &n)]),
    }
}

/// Its button: the line goes into `.gitignore`, or marks it generated.
pub fn button_said(file: &str) -> &'static str {
    word(if file == IGNORE { "skip.add_ignore" } else { "skip.mark_generated" })
}

/// What its row says once its line is written.
pub fn added_said(file: &str) -> &'static str {
    word(if file == IGNORE { "skip.added_ignore" } else { "skip.added_generated" })
}

/// One finding: its mark, its path and what it holds, the line that keeps
/// it out, and the button that writes it; once written, said in teal. A
/// nested repository is a muted line with no button.
fn finding_row(project: &Path, f: &Finding, place: Place, writing: bool) -> Element {
    let (font, line_height, pad) = match place {
        Place::Dialog => (13, "19px", (9, 14)),
        Place::Settings => (14, "20px", (11, 0)),
    };
    let nested = f.kind == "sub_repo";
    let mark = div().layer("mark").w(14).shrink_0().style("text-align", "center").font_mono();
    let mark = if f.added {
        mark.font_size(14).font_weight(700).color(palette::TEAL).text("✓")
    } else if nested {
        mark.font_size(11).color(palette::DIM).text("○")
    } else {
        mark.font_size(11).color(palette::AMBER).text("■")
    };
    let mono = |colour| Run { mono: true, color: Some(colour), ..Run::default() };
    let muted = Run { color: Some(palette::MUTED), ..Run::default() };
    let (text, runs) = if f.added {
        let label = added_said(f.fix.as_ref().map_or("", |x| x.file.as_str()));
        let gone = match f.files {
            1 => word("skip.no_longer_read_one").to_string(),
            n => fill("skip.no_longer_read", &[("n", &count(n))]),
        };
        let at = label.len() + 3;
        let text = format!("{label} · {} — {gone}", f.path);
        let teal = Run { bold: true, color: Some(palette::TEAL), ..Run::default() };
        let path = at..at + f.path.len();
        let runs = vec![
            (0..label.len(), teal),
            (label.len()..at, muted.clone()),
            (path.clone(), mono(palette::MUTED)),
            (path.end..text.len(), muted),
        ];
        (text, runs)
    } else if nested {
        let text = format!("{} {}", f.path, what(f));
        let dim = Run { color: Some(palette::DIM), ..Run::default() };
        (text.clone(), vec![(0..f.path.len(), mono(palette::DIM)), (f.path.len()..text.len(), dim)])
    } else {
        let text = format!("{} — {}", f.path, what(f));
        (
            text.clone(),
            vec![(0..f.path.len(), mono(palette::TEXT)), (f.path.len()..text.len(), muted)],
        )
    };
    let said = one_line(rich(text, runs).layer("finding said").self_stretch());
    let line = f.fix.as_ref().filter(|_| !nested).map(|fix| {
        div()
            .layer("finding line")
            .self_stretch()
            .flex_row()
            .gap(10)
            .items_baseline()
            .font_size(12)
            .child(div().w(104).shrink_0().font_mono().color(palette::DIM).text(fix.file.clone()))
            .child(one_line(
                div()
                    .layer("line")
                    .font_mono()
                    .color(palette::TEXT)
                    .bg(palette::TERMINAL_BG)
                    .pad(1, 6)
                    .style("border-radius", format!("{}px", palette::RADIUS_CONTROL))
                    .text(fix.line.clone()),
            ))
    });
    let gap = if place == Place::Dialog { 3 } else { 4 };
    let body = div().flex_1().flex_col().gap(gap).child(said).children(line);
    let act = f.fix.as_ref().filter(|_| !f.added && !nested).map(|fix| {
        if writing {
            return div()
                .layer("writing")
                .w(30)
                .shrink_0()
                .flex_row()
                .justify_center()
                .child(crate::waiting::dot());
        }
        let said = button_said(&fix.file);
        div()
            .layer(format!("btn {said}"))
            .on_click(SheetAct::SkipAdd {
                project: project.to_path_buf(),
                file: fix.file.clone(),
                line: fix.line.clone(),
            })
            .shrink_0()
            .h(30)
            .pad(0, 14)
            .flex_row()
            .items_center()
            .bg(palette::SURFACE)
            .border(palette::TEAL)
            .font_size(13)
            .font_weight(600)
            .color(palette::TEAL)
            .style("white-space", "nowrap")
            .text(said)
    });
    div()
        .layer(if f.added {
            format!("finding {} added", f.path)
        } else {
            format!("finding {}", f.path)
        })
        .self_stretch()
        .flex_row()
        .gap(12)
        .items_center()
        .pad(pad.0, pad.1)
        .font_size(font)
        .style("line-height", line_height)
        .child(mark)
        .child(body)
        .children(act)
}

/// Whether the line a finding proposed is being written.
fn writing(client: Option<&ProjectClient>, f: &Finding) -> bool {
    let (Some(c), Some(fix)) = (client, f.fix.as_ref()) else { return false };
    c.owes(&Then::SkipAdd { file: fix.file.clone(), line: fix.line.clone() })
}

/// Why the last line asked for was not written, in amber.
fn refused(client: Option<&ProjectClient>) -> Option<Element> {
    let why = client?.skip_refused.clone()?;
    Some(div().layer("skip refused").self_stretch().font_size(13).color(palette::AMBER).text(why))
}

/// The Eval dialog's "Check before it reads": the findings in its work's
/// range, those added first and said in teal, and how many of those a line
/// fixes are left. None when it found nothing.
pub fn dialog(client: &ProjectClient, reads: &Value) -> Option<Element> {
    let findings: Vec<Finding> =
        serde_json::from_value(reads.get("findings")?.clone()).unwrap_or_default();
    if findings.is_empty() {
        return None;
    }
    let fixable = findings.iter().filter(|f| f.fix.is_some() && f.kind != "sub_repo");
    let (all, left) = fixable.fold((0, 0), |(a, l), f| (a + 1, l + usize::from(!f.added)));
    let head = div()
        .flex_row()
        .gap(8)
        .items_center()
        .child(div().font_size(13).font_weight(600).text(word("skip.check_before")))
        .children((all > 0).then(|| {
            let said = fill("skip.left", &[("left", &count(left)), ("n", &count(all))]);
            div().layer("findings left").font_size(12).color(palette::DIM).text(said)
        }));
    let n = findings.len();
    let mut list = div()
        .layer("findings")
        .self_stretch()
        .flex_col()
        .bg(palette::CANVAS)
        .border(palette::LINE)
        .style("max-height", "240px")
        .style("overflow-y", "auto");
    for (i, f) in findings.iter().enumerate() {
        let row = finding_row(&client.root, f, Place::Dialog, writing(Some(client), f));
        list = list.child(if i + 1 < n { row.border_bottom(palette::LINE) } else { row });
    }
    Some(
        div()
            .layer("check before")
            .self_stretch()
            .flex_col()
            .gap(6)
            .style("padding", "2px 0 0 0")
            .child(head)
            .child(list)
            .children(refused(Some(client))),
    )
}

/// A project's "FILES AN EVAL SHOULD SKIP", with Check again: each finding
/// with its line and button, never a nested repository; with none, that
/// every file to skip is already ignored.
pub fn settings(project: &Path, client: Option<&ProjectClient>) -> Element {
    let checking = client.is_some_and(|c| c.owes(&Then::SkipCheck));
    let again =
        div().flex_row().gap(8).items_center().children(checking.then(crate::waiting::dot)).child(
            div()
                .layer("check again")
                .on_click(SheetAct::SkipCheck(project.to_path_buf()))
                .font_size(13)
                .color(palette::BLUE_BORDER)
                .text(word("skip.check_again")),
        );
    let head = div()
        .layer("skip head")
        .self_stretch()
        .flex_row()
        .justify_between()
        .items_center()
        .style("padding", "14px 0 8px 0")
        .border_bottom(palette::LINE)
        .child(
            div()
                .font_mono()
                .font_weight(600)
                .font_size(11)
                .color(palette::DIM)
                .text(word("skip.title")),
        )
        .child(again);
    let findings: Vec<&Finding> =
        client.map(|c| c.skip.iter().filter(|f| f.in_settings()).collect()).unwrap_or_default();
    let mut section = div().layer("files to skip").self_stretch().flex_col().child(head);
    if findings.is_empty() {
        section = section.child(
            div()
                .layer("skip none")
                .self_stretch()
                .flex_row()
                .gap(12)
                .items_center()
                .pad(12, 0)
                .font_size(14)
                .child(
                    div()
                        .w(14)
                        .shrink_0()
                        .style("text-align", "center")
                        .font_mono()
                        .font_weight(700)
                        .color(palette::TEAL)
                        .text("✓"),
                )
                .child(one_line(div().flex_1().color(palette::MUTED).text(word("skip.none")))),
        );
    }
    for f in findings {
        let row = finding_row(project, f, Place::Settings, writing(client, f));
        section = section.child(row.border_bottom(palette::LINE));
    }
    section.children(refused(client))
}

/// Where the files of one kind are: one folder, a few files side by side,
/// or the largest folder and how many others.
fn folders_of(groups: &[&NotRead]) -> Element {
    let place = |text: String| {
        one_line(div().font_mono().style("font-size", "12.5px").color(palette::MUTED).text(text))
    };
    let named: Vec<&str> =
        groups.iter().map(|g| g.folder.as_str()).filter(|f| !f.is_empty()).collect();
    let row = div().flex_1().flex_row().gap(4).items_baseline();
    let Some(first) = named.first() else { return row };
    if groups.len() <= 3 && groups.iter().all(|g| g.files == 1) {
        return row.child(place(named.join(" · ")));
    }
    let others = groups.len() - 1;
    let said = match others {
        0 => None,
        1 => Some(word("not_read.other_folder").to_string()),
        n => Some(fill("not_read.other_folders", &[("n", &count(n))])),
    };
    row.child(place(first.to_string())).children(
        said.map(|s| div().shrink_0().color(palette::MUTED).style("white-space", "nowrap").text(s)),
    )
}

/// An Eval's "Not read": what its readers counted and did not read, by
/// kind, each with where it is. None when it read every file.
pub fn not_read(list: &[NotRead]) -> Option<Element> {
    if list.is_empty() {
        return None;
    }
    let total: usize = list.iter().map(|g| g.files).sum();
    let counted = match total {
        1 => word("not_read.counted_one").to_string(),
        n => fill("not_read.counted", &[("n", &count(n))]),
    };
    let head = div()
        .layer("not read head")
        .self_stretch()
        .flex_row()
        .gap(8)
        .items_center()
        .style("padding", "0 0 6px 0")
        .border_bottom(palette::LINE)
        .child(div().font_weight(600).text(word("not_read.title")))
        .child(div().color(palette::DIM).text(format!("· {counted}")));
    let kinds: Vec<(&str, Vec<&NotRead>)> = ["generated", "vendored", "binary", "lock"]
        .into_iter()
        .map(|k| (k, list.iter().filter(|g| g.kind == k).collect::<Vec<_>>()))
        .filter(|(_, groups)| !groups.is_empty())
        .collect();
    let n = kinds.len();
    let mut section =
        div().layer("not read").self_stretch().flex_col().style("padding", "4px 0 0 0").child(head);
    for (i, (kind, groups)) in kinds.iter().enumerate() {
        let files: usize = groups.iter().map(|g| g.files).sum();
        let row = div()
            .layer(format!("not read {kind}"))
            .self_stretch()
            .flex_row()
            .gap(16)
            .items_baseline()
            .pad(7, 0)
            .child(
                div()
                    .w(140)
                    .shrink_0()
                    .text(fill(&format!("reads.{kind}"), &[("n", &count(files))])),
            )
            .child(folders_of(groups));
        section = section.child(if i + 1 < n { row.border_bottom(palette::LINE) } else { row });
    }
    Some(section)
}
