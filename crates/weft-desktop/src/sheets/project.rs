//! A project's own settings, from its row's ⋯: only what differs from every
//! project's, each beside every project's value, with Reset; "+ Change a
//! setting here" adds one. Turbo is every project's only, so never here.

use std::path::Path;

use serde_json::Value;
use weft_core::words::{fill, word};

use super::SheetProps;
use super::settings::{Entry, choices, control, entries, now, shown};
use crate::action::SheetAct;
use crate::gpui::{Element, div, palette};
use crate::ui::{close_button, note, row_list, sheet, sheet_body, sheet_foot, sheet_head};

pub(super) fn render(props: &SheetProps, project: &Path) -> Element {
    let name = props.listed(project).map_or_else(
        || project.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        |l| l.project.name.clone(),
    );
    let all = entries();
    let adding = props.choice == Some("add");
    // What this project sets, and the one whose choices were opened to add it.
    let shown_here: Vec<&Entry> = all
        .iter()
        .filter(|e| {
            !now(props, Some(project), e.key).is_null()
                || props.choice == Some(e.key.join(".").as_str())
        })
        .collect();
    let mut body = sheet_body();
    if shown_here.is_empty() {
        body =
            body.child(note(fill("desktop.project.none", &[("project", &name)]), palette::MUTED));
    } else {
        body = body.child(row_list(shown_here.iter().map(|e| row(props, project, e)).collect()));
    }
    body = body.child(
        div()
            .layer("project add")
            .on_click(SheetAct::AddProjectSetting)
            .font_size(13)
            .color(palette::BLUE_BORDER)
            .text(word("desktop.project.add")),
    );
    if adding {
        let mut list = div().layer("choices add").self_stretch().flex_col().border(palette::LINE);
        for e in all.iter().filter(|e| now(props, Some(project), e.key).is_null()) {
            let key: Vec<String> = e.key.iter().map(|k| k.to_string()).collect();
            list = list.child(
                div()
                    .layer(format!("choice {}", e.name))
                    .on_click(SheetAct::OpenChoice(key))
                    .self_stretch()
                    .pad(8, 14)
                    .font_size(13)
                    .text(e.label.clone()),
            );
        }
        body = body.child(list);
    }
    let key = crate::keys::hint_here(crate::keys::Shortcut::Settings);
    body = body.child(note(fill("desktop.project.foot", &[("key", &key)]), palette::DIM));
    sheet(
        "sheet Project settings",
        sheet_head(
            fill("desktop.project.title", &[("project", &name)]),
            Some((word("desktop.project.differs").to_string(), palette::MUTED)),
        ),
        body,
        sheet_foot(Vec::new(), vec![close_button()]),
    )
}

/// One setting of the project's: every project's value beside it, and Reset.
fn row(props: &SheetProps, project: &Path, e: &Entry) -> Element {
    let every = shown(props, e.kind, &now(props, None, e.key));
    let key: Vec<String> = e.key.iter().map(|k| k.to_string()).collect();
    div()
        .layer(format!("project setting {}", e.key.join(".")))
        .self_stretch()
        .flex_col()
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(11, 16)
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .flex_col()
                        .gap(2)
                        .child(div().font_weight(600).font_size(14).text(e.label.clone()))
                        .child(
                            div()
                                .font_size(12)
                                .color(palette::MUTED)
                                .text(fill("desktop.project.every", &[("value", &every)])),
                        ),
                )
                .child(control(props, Some(project), e))
                .child(
                    div()
                        .layer("reset")
                        .on_click(SheetAct::Choose {
                            project: Some(project.to_path_buf()),
                            key,
                            value: None::<Value>,
                        })
                        .font_size(12)
                        .color(palette::BLUE_BORDER)
                        .text(word("act.reset")),
                ),
        )
        .children(choices(props, Some(project), e))
}
