//! An Eval's Details, and the Eval view of its record.

use std::path::Path;

use weft_core::eval_view::{self, EvalView, Mode, Row, RowKind, State};
use weft_core::record::Record;
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::{SheetAct, TreeAct};
use crate::daemon::Then;
use crate::gpui::{Element, div, palette, span};
use crate::ui::{
    Size, Tone, button, close_button, list_row, mark_colour, note, row_list, section_label, sheet,
    sheet_body, sheet_foot, sheet_head, verdict_colour,
};

/// The Eval's record: from the board's records, or as read through the
/// daemon. None until one of them has it.
fn judges(props: &SheetProps, project: &Path, eval: &str) -> Option<Record> {
    let c = props.client(project)?;
    let held = c.records.get(eval).map(|v| v.get("record").unwrap_or(v));
    let read = c.read.get(&Then::Judges { eval: eval.to_string() });
    held.into_iter().chain(read).find_map(Record::parse)
}

/// Marked lines in a box: ✓, ✗ or ? and what they say.
fn marked(lines: &[weft_core::offers::Marked], mono: bool) -> Element {
    row_list(
        lines
            .iter()
            .map(|m| {
                let text =
                    div().flex_1().font_size(if mono { 12 } else { 13 }).text(m.text.clone());
                let mut r = list_row()
                    .child(
                        div()
                            .font_weight(700)
                            .font_size(13)
                            .color(mark_colour(m.mark))
                            .text(m.mark),
                    )
                    .child(if mono { text.font_mono() } else { text });
                if let Some(res) = &m.result {
                    r = r.child(div().font_size(12).color(palette::MUTED).text(res.clone()));
                }
                r
            })
            .collect(),
    )
}

/// Asks in a box, each opening its Details.
pub(super) fn ask_rows(project: &Path, asks: &[weft_core::tree::Ask]) -> Element {
    row_list(
        asks.iter()
            .map(|a| {
                list_row()
                    .on_click(TreeAct::SelectAsk {
                        project: project.to_path_buf(),
                        unit: a.unit.clone(),
                    })
                    .child(div().flex_1().font_size(13).text(a.title.clone()))
                    .child(div().font_size(12).color(palette::MUTED).text(a.harness.clone()))
                    .child(div().font_size(12).color(palette::DIM).text("›"))
            })
            .collect(),
    )
}

pub(super) fn render_detail(props: &SheetProps, project: &Path, eval_id: &str) -> Element {
    let client = props.client(project);
    let record = judges(props, project, eval_id);
    let detail = client.and_then(|c| {
        let ev = c.work.record.evals.iter().find(|e| e.id == eval_id)?;
        // Seal from here seals its unit of work: that unit's row says why not.
        let seal = weft_core::work::sealed_already(&c.work.record, eval_id).or_else(|| {
            let unit = c.work.of_eval(eval_id)?;
            props.listed(project)?.work_row(&unit.root)?.seal.clone()
        });
        Some(weft_core::offers::eval_detail(ev, record.as_ref(), &c.units, &c.work.record, seal))
    });
    let title = match detail.as_ref().map(|d| d.n) {
        Some(n) => fill("tree.eval", &[("n", &n.to_string())]),
        None => eval_id.to_string(),
    };
    let judging = detail.as_ref().map(|d| d.judging);
    let verdict = detail.as_ref().and_then(|d| d.verdict);
    let agreement = detail.as_ref().and_then(|d| d.agreement.clone());
    let aside = match (judging, verdict) {
        (Some(true), _) => Some((word("tree.judging").to_string(), palette::AMBER)),
        (_, Some(v)) => Some((
            agreement.map_or_else(|| v.to_string(), |a| format!("{v} {a}")),
            verdict_colour(v),
        )),
        _ => None,
    };

    let mut body = sheet_body();
    match &detail {
        None => body = body.child(note(word("desktop.eval.gone"), palette::MUTED)),
        Some(d) => {
            if let Some(by) = &d.judged_by {
                let when = d.at.and_then(|t| client.map(|c| weft_core::offers::stamp(t, c.zone)));
                let said = match when {
                    Some(ts) => format!("{} {by} · {ts}", word("web.eval.judged_by")),
                    None => format!("{} {by}", word("web.eval.judged_by")),
                };
                body = body.child(note(said, palette::MUTED));
            }
            body = body.child(match &d.sealed {
                Some(sl) => div()
                    .on_click(TreeAct::SelectSeal {
                        project: project.to_path_buf(),
                        seal: sl.seal.clone(),
                    })
                    .font_size(13)
                    .color(palette::TEAL)
                    .text(format!("{} ›", fill("web.eval.sealed_in", &[("n", &sl.n.to_string())]))),
                None => note(
                    match d.asks.len() {
                        1 => word("web.eval.seal_what_one").to_string(),
                        n => fill("web.eval.seal_what", &[("n", &n.to_string())]),
                    },
                    palette::MUTED,
                ),
            });
            if !d.findings.is_empty() {
                body = body
                    .child(section_label(word("web.eval.findings")))
                    .child(marked(&d.findings, false));
            }
            if !d.checks.is_empty() {
                body = body
                    .child(section_label(word("eval_view.checks")))
                    .child(marked(&d.checks, true));
            }
            if !d.asks.is_empty() {
                body = body
                    .child(section_label(word("web.eval.asks")))
                    .child(ask_rows(project, &d.asks));
            }
        }
    }

    let ids = || (project.to_path_buf(), eval_id.to_string());
    let left = vec![
        button(
            "btn Seal",
            word("act.seal"),
            SheetAct::Seal { project: ids().0, eval: ids().1 },
            Tone::Primary,
            Size::Sheet,
        ),
        button(
            "btn Eval view",
            word("act.eval_view"),
            SheetAct::EvalView { project: ids().0, eval: ids().1 },
            Tone::Plain,
            Size::Sheet,
        ),
    ];
    sheet(
        "sheet EvalDetail",
        sheet_head(title, aside),
        body,
        sheet_foot(left, vec![close_button()]),
    )
}

/// The section a heading row heads, by its word.
const SECTIONS: [&str; 4] = ["findings", "unexplained", "checks", "limitations"];

fn section_of(row: &Row) -> Option<&'static str> {
    SECTIONS.into_iter().find(|k| word(&format!("eval_view.{k}")).to_uppercase() == row.text)
}

/// The rows under the top-level row at `at`.
fn under(rows: &[Row], at: usize) -> &[Row] {
    let end = rows[at + 1..].iter().position(|r| r.depth == 0).map_or(rows.len(), |n| at + 1 + n);
    &rows[at + 1..end]
}

fn state_colour(state: &Option<State>) -> &'static str {
    match state {
        Some(State::Met) => palette::TEAL,
        Some(State::NotMet) => palette::RED,
        _ => palette::DIM,
    }
}

/// A row to open, or only to read.
fn item(row: &Row, key: Option<&str>) -> Element {
    let name = row.key.split_once(':').map_or(row.key.as_str(), |(_, k)| k);
    let mut el = div()
        .layer(format!("eval row {name}"))
        .self_stretch()
        .flex_row()
        .gap(10)
        .pad(11, 20)
        .items_center()
        .border_bottom(palette::LINE);
    if !row.mark.is_empty() && !matches!(row.kind, RowKind::Change | RowKind::Group) {
        el = el.child(
            div().font_mono().font_size(13).color(state_colour(&row.state)).text(row.mark.clone()),
        );
    }
    el = el.child(
        div()
            .flex_1()
            .font_size(15)
            .text(row.text.split_whitespace().collect::<Vec<_>>().join(" ")),
    );
    if !row.aside.is_empty() {
        // Each part, amber where it is what needs reading.
        let mut aside = div().flex_row().font_mono().font_size(11).color(palette::MUTED);
        for (i, part) in row.aside.split(" · ").enumerate() {
            let needs = part.ends_with(word("eval.not_met")) || part.contains("unexplained");
            let said = if i == 0 { part.to_string() } else { format!(" · {part}") };
            aside =
                aside.child(span(said).color(if needs { palette::AMBER } else { palette::MUTED }));
        }
        el = el.child(aside);
    }
    match key {
        Some(k) => el
            .on_click(SheetAct::EvalOpen(k.to_string()))
            .child(div().font_weight(600).font_size(16).color(palette::DIM).text("›")),
        None => el,
    }
}

/// What a level is called, as the crumb back to it says.
fn title_of(view: &EvalView, rows: &[Row], at: Option<&String>) -> String {
    match at {
        None => word("act.eval_view").to_string(),
        Some(k) if k.starts_with("h:") => word(&format!("eval_view.{}", &k[2..])).to_string(),
        Some(k) if k.starts_with("s:") => word("eval_view.a_step").to_string(),
        Some(k) => rows
            .iter()
            .chain(eval_view::rows(view, Mode::Files, &Default::default()).iter())
            .find(|r| &r.key == k)
            .map_or_else(String::new, |r| {
                r.text.split(" · ").next().unwrap_or_default().to_string()
            }),
    }
}

pub(super) fn render_view(
    props: &SheetProps,
    project: &Path,
    eval_id: &str,
    by_file: bool,
    at: &[String],
) -> Element {
    let client = props.client(project);
    let view: Option<EvalView> = client
        .and_then(|c| c.read.get(&Then::EvalView { eval: eval_id.to_string() }))
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let mode = if by_file { Mode::Files } else { Mode::Steps };
    let rows =
        view.as_ref().map(|v| eval_view::rows(v, mode, &Default::default())).unwrap_or_default();
    let crumb = match (at.len(), &view) {
        (0, _) | (_, None) => word("web.detail.title").to_string(),
        (n, Some(v)) => title_of(v, &rows, at.get(n.wrapping_sub(2)).filter(|_| n >= 2)),
    };
    let head = div()
        .self_stretch()
        .flex_row()
        .gap(10)
        .pad(12, 20)
        .items_center()
        .border_bottom(palette::LINE)
        .child(
            div()
                .on_click(SheetAct::EvalBack)
                .font_size(13)
                .color(palette::BLUE_BORDER)
                .text(format!("‹ {crumb}")),
        )
        .child(div().flex_1())
        .child(div().font_mono().font_size(11).color(palette::DIM).text(eval_id.to_string()));

    let mut body = div().self_stretch().flex_1().flex_col().style("overflow-y", "auto");
    match (&view, at.last()) {
        (None, _) => {
            body = body.child(
                div().pad(18, 20).child(note(word("desktop.eval.no_record"), palette::MUTED)),
            )
        }
        (Some(v), None) => {
            let header = v.header();
            let parts: Vec<&str> = header.split(" · ").collect();
            let mut top =
                div().self_stretch().flex_col().gap(4).style("padding", "18px 20px 10px 20px");
            if v.closed {
                let verdict = parts.first().copied().unwrap_or_default();
                top = top
                    .child(
                        div()
                            .flex_row()
                            .gap(6)
                            .font_weight(600)
                            .font_size(17)
                            .child(span(verdict).color(verdict_colour(verdict)))
                            .children(parts.get(1).map(|c| span(format!("· {c}")))),
                    )
                    .child(note(
                        parts.get(2..).map(|p| p.join(" · ")).unwrap_or_default(),
                        palette::MUTED,
                    ));
            } else {
                top = top
                    .child(div().font_weight(600).font_size(17).text(header.clone()))
                    .child(note(word("web.eval.live"), palette::MUTED));
            }
            let chip = |files: bool| {
                let on = files == by_file;
                let mut c = div()
                    .layer(if files { "by file" } else { "by step" })
                    .pad(6, 12)
                    .font_size(13)
                    .bg(if on { palette::SELECTED_BG } else { palette::SURFACE })
                    .border(if on { palette::BLUE_BORDER } else { palette::LINE })
                    .color(if on { palette::BLUE_BORDER } else { palette::TEXT })
                    .text(word(if files { "eval_view.by_file" } else { "eval_view.by_step" }));
                if !on {
                    c = c.on_click(SheetAct::ToggleEvalViewMode);
                }
                c
            };
            body = body.child(top).child(
                div()
                    .flex_row()
                    .gap(8)
                    .style("padding", "0 20px 12px 20px")
                    .child(chip(false))
                    .child(chip(true)),
            );
            for (i, r) in rows.iter().enumerate().filter(|(_, r)| r.depth == 0) {
                body = body.child(match section_of(r) {
                    Some(k) => {
                        let n = under(&rows, i).len();
                        let mut row = r.clone();
                        row.text = word(&format!("eval_view.{k}")).to_string();
                        row.aside = n.to_string();
                        row.key = format!("h:{k}");
                        item(&row, Some(&row.key))
                    }
                    None => item(r, (!r.key.is_empty()).then_some(r.key.as_str())),
                });
            }
        }
        (Some(v), Some(k)) if k.starts_with("c:") => {
            let window = &k[2..];
            let change = v.change(window);
            let text = client
                .and_then(|c| {
                    c.read.get(&Then::Window {
                        eval: eval_id.to_string(),
                        window: window.to_string(),
                    })
                })
                .and_then(|t| t["text"].as_str().map(str::to_string));
            let title = change.map_or(window.to_string(), |c| {
                format!("{} {}", c.path, c.lines.as_deref().unwrap_or(window))
            });
            body = body.child(div().pad(18, 20).font_weight(600).font_size(16).text(title));
            match text {
                None => {
                    body = body.child(
                        div().pad(0, 20).child(note(word("desktop.eval.no_lines"), palette::MUTED)),
                    )
                }
                Some(t) => {
                    let lines = eval_view::numbered(&t, change.map_or(&[][..], |c| &c.quotes));
                    let width =
                        lines.iter().filter_map(|l| l.n).max().map_or(0, |n| n.to_string().len());
                    let mut code = div()
                        .self_stretch()
                        .flex_col()
                        .pad(10, 12)
                        .bg(palette::TERMINAL_BG)
                        .font_mono()
                        .font_size(12)
                        .style("line-height", "18px")
                        .style("white-space", "pre");
                    for l in lines {
                        let n = l.n.map_or_else(String::new, |n| n.to_string());
                        let (tag, rest) =
                            l.text.split_at(l.text.chars().next().map_or(0, char::len_utf8));
                        let tag_colour = match tag {
                            "+" => palette::TEAL,
                            "-" => palette::RED,
                            _ => palette::DIM,
                        };
                        let mut text = span(rest.to_string());
                        if l.lit {
                            text = text.color(palette::AMBER).font_weight(700);
                        }
                        code = code.child(
                            div()
                                .layer(if l.lit { "line lit" } else { "line" })
                                .flex_row()
                                .gap(8)
                                .child(span(format!("{n:>width$}")).color(palette::DIM))
                                .child(
                                    span(if tag == "+" || tag == "-" { tag } else { " " })
                                        .color(tag_colour),
                                )
                                .child(text),
                        );
                    }
                    body = body
                        .child(div().self_stretch().style("padding", "0 20px").child(code))
                        .child(
                            div().pad(10, 20).child(note(word("web.eval.quoted"), palette::DIM)),
                        );
                }
            }
        }
        (Some(v), Some(k)) if k.starts_with("s:") => {
            let step = eval_view::step_rows(v, &k[2..]);
            // Its head says what its row in the list said: its state and changes.
            let said = eval_view::rows(v, Mode::Steps, &Default::default())
                .into_iter()
                .find(|r| r.key == *k)
                .map(|r| r.aside);
            if let Some((first, rest)) = step.split_first() {
                body = body.child(
                    div()
                        .flex_col()
                        .gap(4)
                        .style("padding", "18px 20px 6px 20px")
                        .child(
                            div()
                                .flex_row()
                                .gap(6)
                                .font_weight(600)
                                .font_size(16)
                                .child(span(first.mark.clone()).color(state_colour(&first.state)))
                                .child(span(
                                    first.text.split_whitespace().collect::<Vec<_>>().join(" "),
                                )),
                        )
                        .child(note(said.unwrap_or_else(|| first.aside.clone()), palette::MUTED)),
                );
                let mut lines =
                    div().self_stretch().flex_col().gap(6).style("padding", "0 20px 10px 20px");
                for r in rest {
                    lines = match r.kind {
                        RowKind::Heading => lines.child(section_label(r.text.clone())),
                        RowKind::Change => {
                            body = body.child(lines);
                            lines = div()
                                .self_stretch()
                                .flex_col()
                                .gap(6)
                                .style("padding", "0 20px 10px 20px");
                            body = body.child(item(r, Some(&r.key)));
                            continue;
                        }
                        _ => {
                            let said = r.text.split_whitespace().collect::<Vec<_>>().join(" ");
                            let said = if r.aside.is_empty() {
                                said
                            } else {
                                format!("{said} · {}", r.aside)
                            };
                            lines.child(note(
                                said,
                                if r.depth > 1 { palette::MUTED } else { palette::TEXT },
                            ))
                        }
                    };
                }
                body = body.child(lines);
            }
        }
        (Some(_), Some(k)) => {
            let section = k.strip_prefix("h:");
            let at = rows.iter().position(|r| match section {
                Some(s) => section_of(r) == Some(s),
                None => &r.key == k,
            });
            if let Some(i) = at {
                let title = section.map_or_else(
                    || rows[i].text.clone(),
                    |s| word(&format!("eval_view.{s}")).to_string(),
                );
                body = body.child(div().pad(18, 20).font_weight(600).font_size(16).text(title));
                for r in under(&rows, i) {
                    body = body.child(item(r, (!r.key.is_empty()).then_some(r.key.as_str())));
                }
            }
        }
    }
    sheet(
        "sheet EvalView",
        head,
        body,
        sheet_foot(vec![note(word("web.eval.click"), palette::DIM).flex_1()], vec![close_button()]),
    )
}
