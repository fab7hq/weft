//! A Seal's Details: what it decided, the Eval it seals, and its receipt as
//! RingFrame checked it through the daemon.

use std::path::Path;

use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::{SheetAct, TreeAct};
use crate::daemon::Then;
use crate::gpui::{Element, div, palette};
use crate::ui::{
    Size, Tone, button, close_button, note, section_label, sheet, sheet_body, sheet_foot,
    sheet_head,
};

pub(super) fn render(props: &SheetProps, project: &Path, seal_id: &str) -> Element {
    let detail = props.client(project).and_then(|c| {
        let seal = c.work.record.seals.iter().find(|s| s.id == seal_id)?;
        let checked = c.read.get(&Then::Seal { seal: seal_id.to_string() });
        Some(weft_core::offers::seal_detail(seal, &c.units, &c.work.record, checked))
    });
    let Some(d) = detail else {
        return sheet(
            "sheet SealDetail",
            sheet_head(seal_id, None),
            sheet_body().child(note(word("desktop.seal.gone"), palette::MUTED)),
            sheet_foot(Vec::new(), vec![close_button()]),
        );
    };

    let mut body = sheet_body();
    if let Some(ev) = &d.eval {
        let mut said = fill("desktop.seal.seals", &[("n", &ev.n.to_string())]);
        if let Some(v) = ev.verdict {
            said.push_str(&format!(" · {v}"));
        }
        if let Some(a) = &ev.agreement {
            said.push_str(&format!(" {a}"));
        }
        body = body.child(
            div()
                .on_click(TreeAct::SelectEval {
                    project: project.to_path_buf(),
                    eval: ev.eval.clone(),
                })
                .font_size(13)
                .color(palette::BLUE_BORDER)
                .text(format!("{said} ›")),
        );
    }
    body = body.child(div().font_mono().font_size(12).color(palette::DIM).text(format!(
        "{}: {}",
        word("web.seal.receipt"),
        d.receipt
    )));
    // Whether it still holds is RingFrame's to say; until it has, nothing is claimed.
    body = match d.holds {
        Some(true) => body.child(note(word("web.seal.holds"), palette::TEAL)),
        Some(false) => {
            let mut b = body.child(note(word("web.seal.broken"), palette::RED));
            for broken in &d.broken {
                b = b.child(note(format!("✗ {broken}"), palette::RED));
            }
            b
        }
        None => body.child(note(word("desktop.seal.unchecked"), palette::MUTED)),
    };
    if !d.asks.is_empty() {
        body = body
            .child(section_label(word("web.seal.asks")))
            .child(super::eval::ask_rows(project, &d.asks));
    }
    body = body.child(note(word("web.seal.final"), palette::MUTED));

    let mut left = Vec::new();
    if let Some(ev) = &d.eval {
        left.push(button(
            "btn Eval view",
            word("act.eval_view"),
            SheetAct::EvalView { project: project.to_path_buf(), eval: ev.eval.clone() },
            Tone::Plain,
            Size::Sheet,
        ));
    }
    sheet(
        "sheet SealDetail",
        sheet_head(
            fill("tree.seal", &[("n", &d.n.to_string())]),
            Some((d.decision.clone(), palette::TEAL)),
        ),
        body,
        sheet_foot(left, vec![close_button()]),
    )
}
