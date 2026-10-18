//! A session's Details: the Asks made in it, and whether it can be resumed.

use std::path::Path;

use weft_core::sessions::State as SessionState;
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::{SheetAct, TreeAct};
use crate::gpui::{Element, div, palette};
use crate::ui::{
    Size, Tone, button, close_button, list_row, note, row_list, sheet, sheet_body, sheet_foot,
    sheet_head,
};

pub(super) fn render(props: &SheetProps, project: &Path, session_id: &str) -> Element {
    let client = props.client(project);
    let row = client.and_then(|c| c.sessions.iter().find(|r| r.id == session_id));
    let (Some(c), Some(row)) = (client, row) else {
        return sheet(
            "sheet SessionDetail",
            sheet_head(session_id, None),
            sheet_body().child(note(word("desktop.session.gone"), palette::MUTED)),
            sheet_foot(Vec::new(), vec![close_button()]),
        );
    };
    let harness = row.harness.as_str();
    let joins = c
        .harnesses
        .find(harness)
        .or_else(|| props.harnesses.find(harness))
        .is_some_and(|h| h.resume_joins);
    let d = weft_core::offers::session_detail(row, &c.units, &c.work.record, joins);
    let state =
        word(if d.state == SessionState::Running { "state.running" } else { "state.ended" });
    let with = [("harness", harness), ("session", session_id)];

    let mut body = sheet_body();
    let dim = |t: &str| div().font_size(12).color(palette::DIM).text(t);
    if !d.asks.is_empty() {
        let rows = d
            .asks
            .iter()
            .map(|a| {
                list_row()
                    .on_click(TreeAct::SelectAsk {
                        project: project.to_path_buf(),
                        unit: a.unit.clone(),
                    })
                    .child(div().flex_1().font_size(14).text(a.title.clone()))
                    .child(div().font_size(12).color(palette::MUTED).text(a.word.say()))
            })
            .collect();
        body = body.child(dim(word("web.session.made"))).child(row_list(rows));
    } else {
        body = body.child(note(word("tree.nothing_asked"), palette::MUTED));
    }
    body = if d.resume {
        let how = if d.joins { "web.session.resume_joins" } else { "web.session.resume_how" };
        body.child(note(fill(how, &with), palette::MUTED))
    } else if d.state == SessionState::Running && d.pane.is_none() {
        body.child(note(fill("why.elsewhere", &[("harness", harness)]), palette::AMBER))
    } else if d.state == SessionState::Running {
        body
    } else {
        body.child(note(word("resume.nothing"), palette::AMBER))
    };

    let mut left = Vec::new();
    if d.resume {
        left.push(button(
            "btn Resume",
            word("act.resume"),
            SheetAct::Resume {
                project: project.to_path_buf(),
                harness: harness.to_string(),
                session: session_id.to_string(),
            },
            Tone::Primary,
            Size::Sheet,
        ));
    }
    sheet(
        "sheet SessionDetail",
        sheet_head(format!("{harness} · {session_id}"), Some((state.to_string(), palette::MUTED))),
        body,
        sheet_foot(left, vec![close_button()]),
    )
}
