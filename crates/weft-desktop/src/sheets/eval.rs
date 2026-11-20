//! An Eval's Details, and the Eval view: its items in a list on the left,
//! the one picked in plain prose on the right.

use std::path::Path;

use weft_core::eval_view::{self, EvalView, Item, Kind, Light, Progress};
use weft_core::record::Record;
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::{SheetAct, TreeAct};
use crate::daemon::{PaneView, Then};
use crate::gpui::{Element, div, edge_bar, palette, rich, span, svg_el};
use crate::ui::{Size, Tone, button, note, row_list, sheet_foot};

/// The Eval's record: from the board's records, or as read through the
/// daemon. None until one of them has it.
fn judges(props: &SheetProps, project: &Path, eval: &str) -> Option<Record> {
    let c = props.client(project)?;
    let held = c.records.get(eval).map(|v| v.get("record").unwrap_or(v));
    let read = c.read.get(&Then::Judges { eval: eval.to_string() });
    held.into_iter().chain(read).find_map(Record::parse)
}

/// Asks in a box, each opening its Details.
pub(super) fn ask_rows(project: &Path, asks: &[weft_core::tree::Ask]) -> Element {
    row_list(
        asks.iter()
            .map(|a| {
                crate::ui::list_row()
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

/// An item's colour, on its shape and its dots only.
fn colour(kind: Kind) -> &'static str {
    match kind {
        Kind::Before => palette::TEAL,
        Kind::Look => palette::AMBER_BORDER,
        Kind::Unsettled => palette::BLUE_BORDER,
        Kind::Settled => palette::MUTED,
    }
}

/// How sure, one dot per reading, filled when it agrees.
fn dots(item: &Item) -> Element {
    let c = colour(item.kind);
    div().layer("dots").flex_row().gap(4).shrink_0().children(item.dots.iter().map(|on| {
        let dot = div().w(7).h(7).style("border-radius", "4px").border(c);
        if *on { dot.layer("dot on").bg(c) } else { dot.layer("dot") }
    }))
}

/// Its shape, in its colour.
fn mark(kind: Kind) -> Element {
    div()
        .layer("mark")
        .w(14)
        .shrink_0()
        .style("text-align", "center")
        .color(colour(kind))
        .text(kind.mark())
}

/// The text a Seal… button says.
fn seal_said() -> String {
    format!("{}…", word("act.seal"))
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
    let ids = || (project.to_path_buf(), eval_id.to_string());
    let mut body = div()
        .layer("eval detail body")
        .self_stretch()
        .flex_1()
        .flex_col()
        .style("overflow-y", "auto")
        .pad(28, 48);
    let Some(d) = &detail else {
        body = body
            .child(div().color(palette::MUTED).text(eval_id.to_string()))
            .child(note(word("desktop.eval.gone"), palette::MUTED));
        return div()
            .layer("sheet EvalDetail")
            .flex_1()
            .self_stretch()
            .flex_col()
            .child(body)
            .child(sheet_foot(Vec::new(), vec![crate::ui::close_button()]));
    };
    let title = d.title.clone();
    let mut first = vec![fill("tree.eval", &[("n", &d.n.to_string())])];
    if !title.is_empty() {
        first.push(title);
    }
    let mut top = div().self_stretch().flex_row().gap(8).items_center();
    top = match &d.sealed {
        Some(sl) => top.child(span(first.join(" · ")).color(palette::MUTED)).child(
            div()
                .on_click(TreeAct::SelectSeal {
                    project: project.to_path_buf(),
                    seal: sl.seal.clone(),
                })
                .color(palette::TEAL)
                .text(format!("· {} ›", fill("web.eval.sealed_in", &[("n", &sl.n.to_string())]))),
        ),
        None if d.judging => {
            first.push(word("tree.judging").into());
            top.child(span(first.join(" · ")).color(palette::MUTED))
        }
        None => {
            first.push(word("tree.not_sealed").into());
            top.child(span(first.join(" · ")).color(palette::MUTED))
        }
    };
    top = top.child(div().flex_1());
    if let Some(by) = &d.judged_by {
        let when = d.at.and_then(|t| client.map(|c| weft_core::offers::stamp(t, c.zone)));
        let said = match when {
            Some(ts) => fill("details.read_by", &[("who", by), ("time", &ts)]),
            None => fill("details.read_by_untimed", &[("who", by)]),
        };
        top = top.child(span(said).color(palette::DIM));
    }
    body = body.child(top);
    // Its record still read through the daemon: lines that breathe where its
    // findings will be, or why it could not be read.
    if record.is_none()
        && let Some(c) = client
    {
        let then = Then::Judges { eval: eval_id.to_string() };
        if c.owes(&then) {
            body = body.child(div().pad(16, 0).child(crate::waiting::loading(&[500, 590, 380])));
        } else if let Some(why) = c.refused.get(&then) {
            let again = SheetAct::TryAgain { project: ids().0, then };
            body = body.child(div().pad(16, 0).child(crate::waiting::failed(
                crate::waiting::could_not_read(why),
                Some(again.into()),
            )));
        }
    }
    if let Some(h) = &d.headline {
        let (look, rest) = h.split_once(" · ").map_or((h.as_str(), None), |(a, b)| (a, Some(b)));
        body = body.child(
            div()
                .layer("headline")
                .flex_row()
                .gap(6)
                .items_baseline()
                .style("padding-top", "16px")
                .pad(16, 0)
                .child(span(look.to_string()).font_size(20).font_weight(600))
                .children(rest.map(|r| span(format!("· {r}")).color(palette::MUTED))),
        );
    }
    if !d.items.is_empty() {
        let mut list = div().layer("items").self_stretch().flex_col().border_top(palette::LINE);
        let n = d.items.len();
        for (i, it) in d.items.iter().enumerate() {
            let act = SheetAct::EvalItem { project: ids().0, eval: ids().1, item: it.key.clone() };
            let mut row = div()
                .layer(format!("item {}", it.key))
                .on_click(act)
                .self_stretch()
                .flex_row()
                .gap(12)
                .items_center()
                .style("min-height", "44px")
                .pad(10, 0)
                .child(mark(it.kind))
                .child(div().flex_1().style("min-width", "0").text(it.text.clone()));
            if !it.dots.is_empty() {
                row = row.child(dots(it));
            }
            row = row.child(div().w(10).color(palette::DIM).text("›"));
            list = list.child(if i + 1 < n { row.border_bottom(palette::LINE) } else { row });
        }
        body = body.child(list);
    }
    if !d.asks.is_empty() {
        let titles: Vec<&str> = d.asks.iter().map(|a| a.title.as_str()).collect();
        let covers = match titles.len() {
            1 => fill("details.covers_one", &[("asks", titles[0])]),
            n => fill("details.covers", &[("n", &n.to_string()), ("asks", &titles.join(" · "))]),
        };
        body = body.child(div().pad(22, 0).child(note(covers, palette::MUTED)));
    }
    body = body.children(crate::skip_view::not_read(&d.not_read));
    let seal_what = match (d.questions, d.asks.len()) {
        (0, 1) => word("web.eval.seal_what_one").to_string(),
        (0, n) => fill("web.eval.seal_what", &[("n", &n.to_string())]),
        (1, _) => word("details.seal_asks_one").to_string(),
        (n, _) => fill("details.seal_asks", &[("n", &n.to_string())]),
    };
    let left = vec![button(
        "btn Eval view",
        word("act.eval_view"),
        SheetAct::EvalView { project: ids().0, eval: ids().1 },
        Tone::Quiet,
        Size::Sheet,
    )];
    let mut right = Vec::new();
    if d.sealed.is_none() {
        right.push(div().font_size(13).color(palette::DIM).text(seal_what));
        right.push(button(
            "btn Seal",
            seal_said(),
            SheetAct::Seal { project: ids().0, eval: ids().1 },
            Tone::Primary,
            Size::Sheet,
        ));
    }
    right.push(crate::ui::close_button());
    div()
        .layer("sheet EvalDetail")
        .flex_1()
        .self_stretch()
        .flex_col()
        .child(body)
        .child(sheet_foot(left, right))
}

// ---- the Eval view ----------------------------------------------------------------

/// A section's name over its items.
fn section(text: &str) -> Element {
    div().color(palette::MUTED).style("padding", "8px 20px").text(text.to_string())
}

/// One line of the list: its shape, its text, and its dots or "reading again".
fn list_row(it: &Item, picked: bool) -> Element {
    let text = div()
        .flex_1()
        .style("min-width", "0")
        .style("white-space", "nowrap")
        .style("overflow", "hidden")
        .style("text-overflow", "ellipsis")
        .text(it.text.clone());
    let text = if picked {
        text.color(palette::TEXT).font_weight(600)
    } else {
        text.color(palette::MUTED)
    };
    let mut row = div()
        .layer(format!("eval item {}", it.key))
        .on_click(SheetAct::EvalOpen(it.key.clone()))
        .self_stretch()
        .flex_row()
        .gap(10)
        .items_center()
        .h(44)
        .style("position", "relative")
        .style("padding", "0 20px 0 17px")
        .child(mark(it.kind))
        .child(text);
    if picked {
        row = row
            .layer(format!("eval item {} picked", it.key))
            .bg(palette::SELECTED_BG)
            .child(edge_bar("left", 3, palette::BLUE_BORDER).layer("picked bar"));
    }
    if it.again {
        row = row.child(div().shrink_0().color(palette::DIM).text(word("eval_view.reading_again")));
    } else if !it.dots.is_empty() {
        row = row.child(dots(it));
    }
    row
}

/// The list: "To look at", a line, then "Settled".
fn list_pane(v: &EvalView, crumb: &str, n: Option<usize>, picked: Option<&str>) -> Element {
    let mut pane = div()
        .layer("eval items")
        .w(340)
        .shrink_0()
        .flex_col()
        .border_right(palette::LINE)
        .style("padding-top", "20px")
        .style("overflow-y", "auto")
        .child(
            div()
                .self_stretch()
                .flex_row()
                .items_center()
                .style("padding", "0 20px 12px 20px")
                // The work's title is the agent's words: it ends in an ellipsis before Eval n.
                .child(
                    div()
                        .layer("eval crumb")
                        .on_click(SheetAct::EvalBack)
                        .flex_1()
                        .style("min-width", "0")
                        .style("white-space", "nowrap")
                        .style("overflow", "hidden")
                        .style("text-overflow", "ellipsis")
                        .color(palette::BLUE_BORDER)
                        .text(format!("‹ {crumb}")),
                )
                .children(n.map(|_| div().w(12).shrink_0()))
                .children(n.map(|n| {
                    div()
                        .shrink_0()
                        .color(palette::DIM)
                        .text(fill("tree.eval", &[("n", &n.to_string())]))
                })),
        );
    let (look, settled): (Vec<&Item>, Vec<&Item>) =
        v.items.iter().partition(|i| i.kind.to_look_at());
    if !look.is_empty() || v.waiting {
        pane = pane.child(section(word("eval_view.to_look_at")));
        for it in &look {
            pane = pane.child(list_row(it, picked == Some(it.key.as_str())));
        }
    }
    if v.waiting {
        pane = pane.child(
            div()
                .color(palette::DIM)
                .style("padding", "12px 20px")
                .text(word("eval_view.rest_later")),
        );
    }
    if !settled.is_empty() {
        if !look.is_empty() {
            pane = pane.child(
                div()
                    .self_stretch()
                    .style("padding", "12px 20px")
                    .child(div().layer("divider").self_stretch().h(1).bg(palette::LINE)),
            );
        }
        pane = pane.child(section(word("eval_view.settled")));
        for it in &settled {
            pane = pane.child(list_row(it, picked == Some(it.key.as_str())));
        }
    }
    pane
}

/// The picked item, in plain prose.
fn detail_pane(it: &Item, reads_open: bool) -> Element {
    let c = colour(it.kind);
    let mut pane = div()
        .layer("eval detail")
        .flex_1()
        .style("min-width", "0")
        .flex_col()
        .gap(14)
        .style("padding", "48px 48px 24px 48px")
        .style("overflow-y", "auto")
        .child(div().layer("detail head").color(c).font_weight(600).text(format!(
            "{} {}",
            it.kind.mark(),
            it.head
        )))
        .child(
            div()
                .layer("detail big")
                .self_stretch()
                .font_size(20)
                .font_weight(600)
                .style("line-height", "29px")
                .text(it.big.clone()),
        );
    for w in &it.why {
        pane = pane.child(note(w.clone(), palette::MUTED).font_size(14));
    }
    if !it.sure.is_empty() {
        pane = pane.child(
            div()
                .layer("how sure")
                .flex_row()
                .gap(8)
                .items_center()
                .child(dots(it))
                .child(span(it.sure.clone()).color(palette::DIM)),
        );
    }
    if let Some(q) = &it.said {
        let said = fill("eval_view.agent_said", &[("quote", q)]);
        let n = said.len();
        let italic =
            crate::gpui::Run { italic: true, color: Some(palette::MUTED), ..Default::default() };
        pane = pane.child(
            div().layer("agent said").self_stretch().child(rich(said, vec![(0..n, italic)])),
        );
    }
    if let Some(check) = &it.check {
        pane = pane.child(note(check.clone(), palette::MUTED).font_size(14));
    }
    let mut links = div().flex_row().gap(24);
    if !it.windows.is_empty() {
        links = links.child(
            div()
                .layer("link lines")
                .on_click(SheetAct::EvalOpen("lines".into()))
                .color(palette::MUTED)
                .text(format!("{} ›", word("eval_view.lines"))),
        );
    }
    if !it.reads.is_empty() {
        let arrow = if reads_open { "⌄" } else { "›" };
        links = links.child(
            div()
                .layer("link reads")
                .on_click(if reads_open {
                    SheetAct::EvalBack
                } else {
                    SheetAct::EvalOpen("reads".into())
                })
                .color(palette::MUTED)
                .text(format!("{} {arrow}", word("eval_view.reads"))),
        );
    }
    pane = pane.child(links);
    if reads_open {
        let mut reads = div().layer("reads").self_stretch().flex_col();
        for r in &it.reads {
            let who = format!("{} · ", r.who);
            let n = who.len();
            let faint = crate::gpui::Run { color: Some(palette::DIM), ..Default::default() };
            reads = reads.child(
                div()
                    .self_stretch()
                    .pad(10, 0)
                    .border_top(palette::LINE)
                    .child(rich(format!("{who}{}", r.said), vec![(0..n, faint)])),
            );
        }
        pane = pane.child(reads);
    }
    pane
}

/// The lines that show an item, the ones a reading quoted brighter.
/// One change's lines as read through the daemon: its text, still coming,
/// or why it could not be read.
enum Lines {
    Read(String),
    Coming,
    Failed(String),
    Unread,
}

fn lines_pane(
    project: &Path,
    eval: &str,
    v: &EvalView,
    it: &Item,
    texts: &dyn Fn(&str) -> Lines,
) -> Element {
    let mut pane = div()
        .layer("eval lines")
        .flex_1()
        .style("min-width", "0")
        .flex_col()
        .gap(14)
        .style("padding", "48px 48px 24px 48px")
        .style("overflow-y", "auto")
        .child(div().color(colour(it.kind)).font_weight(600).text(format!(
            "{} {}",
            it.kind.mark(),
            fill("eval_view.lines_of", &[("kind", it.kind.words())])
        )))
        .child(
            div()
                .layer("lines back")
                .on_click(SheetAct::EvalBack)
                .color(palette::BLUE_BORDER)
                .text(format!("‹ {}", it.text)),
        );
    for id in &it.windows {
        let w = v.window(id);
        let mut code = div()
            .layer(format!("lines {id}"))
            .self_stretch()
            .flex_col()
            .style("padding", "12px 14px")
            .bg(palette::TERMINAL_BG)
            .style("border-radius", "4px")
            .font_mono()
            .style("line-height", "22px")
            .style("white-space", "pre")
            .style("overflow", "hidden")
            .child(div().color(palette::DIM).text(w.map_or(id.clone(), |w| w.path.clone())));
        match texts(id) {
            Lines::Unread => {
                code = code.child(div().color(palette::MUTED).text(word("desktop.eval.no_lines")))
            }
            Lines::Coming => code = code.child(crate::waiting::loading(&[420, 520, 300])),
            Lines::Failed(why) => {
                let then = Then::Window { eval: eval.to_string(), window: id.clone() };
                let again = SheetAct::TryAgain { project: project.to_path_buf(), then };
                code = code.child(crate::waiting::failed(
                    crate::waiting::could_not_read(&why),
                    Some(again.into()),
                ))
            }
            Lines::Read(t) => {
                let quotes = w.map_or(&[][..], |w| &w.quotes);
                for l in eval_view::numbered(&t, quotes) {
                    let n = l.n.map_or_else(String::new, |n| n.to_string());
                    let text = span(l.text.clone());
                    let text = if l.lit {
                        text.color(palette::TEXT).font_weight(600)
                    } else {
                        text.color(palette::MUTED)
                    };
                    code = code.child(
                        div()
                            .layer(if l.lit { "line lit" } else { "line" })
                            .flex_row()
                            .child(span(n).w(32).shrink_0().color(palette::DIM))
                            .child(text),
                    );
                }
            }
        }
        pane = pane.child(code);
    }
    pane.child(div().color(palette::DIM).text(word("eval_view.quoted")))
}

/// One arc of the ring, as SVG: the `i`th of three.
fn arc_svg(i: usize, colour: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="150" height="150" viewBox="0 0 150 150"><g transform="rotate(-87 75 75)"><circle cx="75" cy="75" r="64" fill="none" stroke="{colour}" stroke-width="8" stroke-dasharray="122 280" stroke-dashoffset="{offset}"/></g></svg>"#,
        offset = -(134 * i as i64),
    )
}

/// The record's steps, as the steps of a wait show: ✓ done and muted, the
/// one under way with its dot, those to come faint.
fn lit_steps(steps: &[weft_core::eval_view::Lit]) -> Element {
    let rows = steps.iter().map(|s| {
        let (layer, mark, colour) = match s.light {
            Light::Done => {
                ("step done", div().font_mono().color(palette::TEAL).text("✓"), palette::MUTED)
            }
            Light::Now => ("step now", crate::waiting::dot(), palette::TEXT),
            Light::Next => {
                ("step next", div().font_mono().color(palette::DIM).text("○"), palette::DIM)
            }
        };
        div()
            .layer(layer)
            .self_stretch()
            .flex_row()
            .gap(10)
            .items_center()
            .child(div().w(14).shrink_0().flex_row().justify_center().child(mark))
            .child(div().layer("step said").color(colour).text(s.name.clone()))
    });
    div().layer("steps").self_stretch().flex_col().gap(10).children(rows)
}

/// While it runs: one ring with an arc per reading, the one under way
/// breathing, and its steps: as the machine says them, with their times
/// and how many reads are in, else as the record has them.
fn progress_pane(
    project: &Path,
    p: &Progress,
    judges: &[&PaneView],
    job: Option<&crate::waiting::Progress>,
) -> Element {
    let now = std::time::Instant::now();
    let mut ring =
        div().layer("progress ring").style("position", "relative").w(150).h(150).shrink_0();
    for (i, a) in p.arcs.iter().enumerate() {
        let c = if *a == Light::Next { palette::LINE } else { palette::BLUE_BORDER };
        let mut arc = div()
            .layer(match a {
                Light::Done => format!("arc {i} done"),
                Light::Now => format!("arc {i} now"),
                Light::Next => format!("arc {i} next"),
            })
            .style("position", "absolute")
            .style("top", "0")
            .style("left", "0")
            .child(svg_el(arc_svg(i, c)).w(150).h(150));
        if *a == Light::Now {
            arc = arc.style("animation", "pulse");
        }
        ring = ring.child(arc);
    }
    ring = ring.child(
        div()
            .layer("ring label")
            .style("position", "absolute")
            .style("top", "0")
            .style("left", "0")
            .w(150)
            .h(150)
            .flex_col()
            .items_center()
            .justify_center()
            .child(div().font_size(20).font_weight(600).text(p.label.clone()))
            .child(div().color(palette::DIM).text(p.note.clone())),
    );
    let steps = match job.filter(|j| !j.job.steps.is_empty()) {
        Some(j) => crate::waiting::steps(j, now, true).gap(10),
        None => lit_steps(&p.steps),
    };
    let steps =
        div().flex_1().style("min-width", "0").style("max-width", "520px").flex_col().child(steps);
    let mut more =
        div().flex_col().gap(10).style("padding-top", "14px").style("padding-left", "24px");
    for j in judges {
        // How long the readers have read, once the machine says.
        let said = match job {
            Some(p) => format!(
                "{} · {} · {}",
                fill("wait.reading", &[("harness", &j.harness)]),
                crate::waiting::elapsed(p.since, now),
                word("eval_view.watch")
            ),
            None => format!(
                "{} · {}",
                fill("eval_view.is_reading", &[("harness", &j.harness)]),
                word("eval_view.watch")
            ),
        };
        more = more.child(
            div()
                .layer(format!("judge {}", j.id))
                .on_click(SheetAct::WatchJudge { project: project.to_path_buf(), pane: j.id })
                .color(palette::DIM)
                .text(said),
        );
    }
    for t in &p.tests {
        more = more.child(div().color(palette::DIM).text(t.clone()));
    }
    div()
        .layer("eval progress")
        .flex_1()
        .style("min-width", "0")
        .flex_row()
        .gap(40)
        .items_start()
        .style("padding", "56px 48px 24px 48px")
        .child(ring)
        .child(steps.child(more))
}

pub(super) fn render_view(
    props: &SheetProps,
    project: &Path,
    eval_id: &str,
    at: &[String],
) -> Element {
    let client = props.client(project);
    let view: Option<EvalView> = client
        .and_then(|c| c.read.get(&Then::EvalView { eval: eval_id.to_string() }))
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let n = client.and_then(|c| c.work.record.evals.iter().find(|e| e.id == eval_id)).map(|e| e.n);
    let crumb = view
        .as_ref()
        .map(|v| v.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| props.back.unwrap_or(word("web.detail.title")).to_string());
    let runs = view.as_ref().is_some_and(|v| v.running().is_some());
    let panes = match &view {
        None => {
            // Not read, it has no record to show; else it is still to come.
            let judged = client.is_some_and(|c| {
                c.work.record.evals.iter().any(|e| e.id == eval_id && e.completed())
            });
            let said = if judged { "desktop.eval.no_record" } else { "desktop.eval.not_judged" };
            // Still being read: lines that breathe; not read: why, and Try again.
            let then = Then::EvalView { eval: eval_id.to_string() };
            let shown = match client {
                Some(c) if c.owes(&then) => crate::waiting::loading(&[520, 600, 360, 460]),
                Some(c) if c.refused.contains_key(&then) => {
                    let why = c.refused.get(&then).cloned().unwrap_or_default();
                    let again = SheetAct::TryAgain { project: project.to_path_buf(), then };
                    crate::waiting::failed(crate::waiting::could_not_read(&why), Some(again.into()))
                }
                _ => note(word(said), palette::MUTED),
            };
            div().self_stretch().flex_1().flex_col().style("padding", "20px").gap(12).children([
                div()
                    .layer("eval crumb")
                    .on_click(SheetAct::EvalBack)
                    .color(palette::BLUE_BORDER)
                    .text(format!("‹ {crumb}")),
                shown,
            ])
        }
        Some(v) => {
            // The item picked, else the first one; none while it runs and
            // nothing is picked, so its progress shows.
            let picked = at
                .first()
                .and_then(|k| v.item(k))
                .or_else(|| if runs { None } else { v.items.first() });
            let right = match (picked, at.get(1).map(String::as_str)) {
                (Some(it), Some("lines")) => {
                    let texts = |w: &str| {
                        let then =
                            Then::Window { eval: eval_id.to_string(), window: w.to_string() };
                        let Some(c) = client else { return Lines::Unread };
                        match c.read.get(&then).and_then(|t| t["text"].as_str()) {
                            Some(t) => Lines::Read(t.to_string()),
                            None if c.owes(&then) => Lines::Coming,
                            None => match c.refused.get(&then) {
                                Some(why) => Lines::Failed(why.clone()),
                                None => Lines::Unread,
                            },
                        }
                    };
                    lines_pane(project, eval_id, v, it, &texts)
                }
                (Some(it), more) => detail_pane(it, more == Some("reads")),
                (None, _) => match v.running() {
                    Some(p) => {
                        let judges = client.map(|c| c.judges(eval_id)).unwrap_or_default();
                        let job = client.and_then(|c| {
                            c.job(|j| j.kind == "readers" && j.on.eval.as_deref() == Some(eval_id))
                        });
                        progress_pane(project, p, &judges, job)
                    }
                    None => div().flex_1(),
                },
            };
            div()
                .self_stretch()
                .flex_1()
                .style("min-height", "0")
                .flex_row()
                .child(list_pane(v, &crumb, n, picked.map(|i| i.key.as_str())))
                .child(right)
        }
    };
    let close = crate::ui::close_button();
    let act = if runs {
        let stop = SheetAct::StopEval { project: project.to_path_buf(), eval: eval_id.to_string() };
        let said = word("dialog.stop_eval.go");
        button(format!("btn {said}"), said, stop, Tone::Plain, Size::Sheet)
    } else {
        let seal = SheetAct::Seal { project: project.to_path_buf(), eval: eval_id.to_string() };
        button("btn Seal", seal_said(), seal, Tone::Primary, Size::Sheet)
    };
    let foot = div()
        .self_stretch()
        .flex_row()
        .gap(16)
        .items_center()
        .style("padding", "16px 48px 20px 48px")
        .border_top(palette::LINE)
        .child(close)
        .child(div().flex_1())
        .child(act);
    div().layer("sheet EvalView").flex_1().self_stretch().flex_col().child(panes).child(foot)
}
