//! A work's Details: where it stands, its session, what was asked and the
//! prompt composed from it, as RingFrame recorded them.

use std::path::Path;

use weft_core::tree::Word;
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::{SheetAct, TreeAct};
use crate::daemon::Then;
use crate::gpui::{Element, div, palette};
use crate::ui::{
    Size, Tone, button, close_button, note, sheet, sheet_body, sheet_foot, sheet_head,
};

/// A text read for an Ask through the daemon, once it has been.
fn read_text(props: &SheetProps, project: &Path, then: Then) -> Option<String> {
    let v = props.client(project)?.read.get(&then)?;
    Some(v["text"].as_str()?.trim().to_string()).filter(|t| !t.is_empty())
}

fn boxed(text: String, size: u32, colour: &str) -> Element {
    div()
        .self_stretch()
        .pad(12, 14)
        .bg(palette::SURFACE)
        .border(palette::LINE)
        .font_size(size)
        .color(colour)
        .text(text)
}

fn link(text: String, action: TreeAct, colour: &str) -> Element {
    div().on_click(action).font_size(12).color(colour).text(text)
}

pub(super) fn render(props: &SheetProps, project: &Path, unit_id: &str) -> Element {
    let client = props.client(project);
    let Some((c, unit)) =
        client.and_then(|c| Some((c, c.units.iter().find(|u| u.ask_id == unit_id)?)))
    else {
        return sheet(
            "sheet WorkDetail",
            sheet_head(unit_id, None),
            sheet_body().child(note(word("desktop.work.gone"), palette::MUTED)),
            sheet_foot(Vec::new(), vec![close_button()]),
        );
    };
    let source = read_text(props, project, Then::Source { unit: unit_id.to_string() });
    let prompt = read_text(props, project, Then::Wording { unit: unit_id.to_string() });
    let agent = unit.session_ref.as_deref().and_then(|sid| {
        props.listed(project).and_then(|l| l.project.agents.rows.iter().find(|a| a.session == sid))
    });
    let d = weft_core::offers::work_detail(
        unit,
        &c.rounds.record,
        &c.rounds,
        agent.cloned(),
        source.as_deref(),
        prompt.as_deref(),
        c.zone,
    );

    // Where it stands: `› Eval N verdict › Seal M`, or `› not evaluated yet`.
    let dim = |t: &str| div().font_size(12).color(palette::DIM).text(t);
    let mut crumb = div()
        .self_stretch()
        .flex_row()
        .gap(8)
        .items_center()
        .pad(8, 12)
        .bg(palette::SURFACE_ALT)
        .border(palette::LINE)
        .child(dim(unit_id))
        .child(dim("›"));
    match &d.crumb.eval {
        Some(ev) => {
            let said = ev.verdict.unwrap_or(word("tree.judging"));
            crumb = crumb
                .child(link(
                    format!("{} · {said}", fill("tree.eval", &[("n", &ev.n.to_string())])),
                    TreeAct::SelectEval { project: project.to_path_buf(), eval: ev.eval.clone() },
                    palette::BLUE_BORDER,
                ))
                .child(dim("›"))
                .child(match &d.crumb.seal {
                    Some(sl) => link(
                        fill("tree.seal", &[("n", &sl.n.to_string())]),
                        TreeAct::SelectSeal {
                            project: project.to_path_buf(),
                            seal: sl.seal.clone(),
                        },
                        palette::TEAL,
                    ),
                    None => div().font_size(12).color(palette::MUTED).text(word("tree.not_sealed")),
                });
        }
        None => {
            crumb = crumb.child(
                div().font_size(12).color(palette::MUTED).text(word("web.detail.not_evaluated")),
            )
        }
    }

    let mut body = sheet_body().child(crumb);
    if !d.ask.aside.is_empty() {
        body = body.child(note(format!("{} · {}", d.ask.state, d.ask.aside), palette::MUTED));
    }
    body = body.child(dim(word("web.detail.its_session")));
    let session = match (&d.session, unit.session_ref.as_deref()) {
        (Some(s), _) => Some((s.harness.clone(), s.session.clone())),
        (None, Some(sid)) => Some((unit.harness.clone(), sid.to_string())),
        (None, None) => None,
    };
    body = body.child(match session {
        Some((harness, sid)) => div()
            .on_click(TreeAct::SelectAgent {
                project: project.to_path_buf(),
                session: sid.clone(),
                pane: None,
            })
            .font_size(14)
            .color(palette::BLUE_BORDER)
            .text(format!("{harness} · {sid} ›")),
        None => div().font_size(14).color(palette::MUTED).text(word("desktop.work.no_session")),
    });
    if let Some(text) = source {
        body = body.child(dim(word("web.detail.as_typed"))).child(boxed(text, 14, palette::TEXT));
    }
    if let Some(text) = prompt {
        body = body
            .child(dim(&fill("desktop.work.composed", &[("harness", &unit.harness)])))
            .child(boxed(text, 13, palette::MUTED));
    }

    let mut left = Vec::new();
    if d.proceed {
        left.push(button(
            "btn Proceed",
            word("act.proceed"),
            SheetAct::Proceed { project: project.to_path_buf(), unit: unit_id.to_string() },
            Tone::Primary,
            Size::Sheet,
        ));
    }
    left.push(button(
        "btn Follow up",
        word("act.follow_up"),
        SheetAct::FollowUp { project: project.to_path_buf(), unit: unit_id.to_string() },
        Tone::Plain,
        Size::Sheet,
    ));
    sheet(
        "sheet WorkDetail",
        sheet_head(d.title.clone(), Some((Word::of(unit).say().to_string(), palette::MUTED))),
        body,
        sheet_foot(left, vec![close_button()]),
    )
}
