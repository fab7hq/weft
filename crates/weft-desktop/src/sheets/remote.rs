//! Remote: whether `weft remote` is on, and the devices allowed here.

use serde_json::Value;
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::{BarAct, SheetAct};
use crate::app::BottomBarState;
use crate::gpui::{Element, div, palette};
use crate::ui::{
    Size, Tone, button, close_button, list_row, note, row_list, sheet, sheet_body, sheet_foot,
    sheet_head,
};

/// How long ago `millis` was, said plainly.
pub fn ago(millis: i64, now: i64) -> String {
    let minutes = (now - millis).max(0) / 60_000;
    match minutes {
        0 => word("desktop.ago.now").to_string(),
        1..=59 => fill("desktop.ago.minutes", &[("n", &minutes.to_string())]),
        60..=2879 => fill("desktop.ago.hours", &[("n", &(minutes / 60).to_string())]),
        _ => fill("desktop.ago.days", &[("n", &(minutes / 1440).to_string())]),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A device as the view lists it: what it says, and nothing it does not.
fn device_row(d: &Value) -> Option<Element> {
    let id = d["id"].as_str()?;
    let name = d["name"].as_str().unwrap_or(id);
    let mut said = Vec::new();
    if let Some(login) = d["login"].as_str() {
        said.push(login.to_string());
    }
    if let Some(at) = d["last_seen"].as_i64() {
        said.push(fill("desktop.remote.seen", &[("when", &ago(at, now()))]));
    }
    Some(
        list_row()
            .layer(format!("device row {name}"))
            .gap(12)
            .pad(12, 16)
            .child(div().flex_1().font_weight(600).font_size(14).text(name))
            .child(div().font_size(13).color(palette::MUTED).text(said.join(" · ")))
            .child(button(
                format!("btn Remove {id}"),
                word("web.machines.remove"),
                SheetAct::AskRemoveDevice { device: id.to_string(), name: name.to_string() },
                Tone::Quiet,
                Size::Row,
            )),
    )
}

pub(super) fn render(props: &SheetProps) -> Element {
    let view = props.clients.iter().find_map(|c| c.remote.as_ref());
    let on = view.is_some_and(|v| v["state"] == "on");
    let devices: Vec<Element> = view
        .and_then(|v| v["devices"].as_array())
        .into_iter()
        .flatten()
        .filter_map(device_row)
        .collect();

    let mut body = sheet_body();
    if let BottomBarState::RemoveDeviceConfirm { device_id, device_name } = props.bottom_bar {
        body = body.child(
            div()
                .layer("remove device confirm card")
                .self_stretch()
                .flex_col()
                .gap(10)
                .pad(14, 16)
                .bg(palette::SURFACE_ALT)
                .border(palette::AMBER)
                .child(
                    div()
                        .font_weight(600)
                        .font_size(14)
                        .color(palette::AMBER)
                        .text(fill("desktop.remote.remove_sure", &[("device", device_name)])),
                )
                .child(note(word("desktop.remote.remove_what"), palette::MUTED))
                .child(
                    div()
                        .self_stretch()
                        .flex_row()
                        .gap(10)
                        .items_center()
                        .child(div().flex_1())
                        .child(button(
                            "btn Cancel remove",
                            word("act.cancel"),
                            BarAct::Cancel,
                            Tone::Plain,
                            Size::Bar,
                        ))
                        .child(button(
                            "btn Confirm remove",
                            word("web.machines.remove"),
                            SheetAct::RemoveDevice(device_id.clone()),
                            Tone::Warn,
                            Size::Bar,
                        )),
                ),
        );
    }
    body = if devices.is_empty() {
        body.child(note(word("desktop.remote.no_devices"), palette::MUTED))
    } else {
        body.child(row_list(devices))
    };

    let state = if on {
        (word("desktop.remote.on").to_string(), palette::TEAL)
    } else {
        (word("desktop.remote.off").to_string(), palette::MUTED)
    };
    let left = if on {
        vec![button(
            "btn Turn off",
            word("desktop.remote.turn_off"),
            SheetAct::RemoteOff,
            Tone::Quiet,
            Size::Sheet,
        )]
    } else {
        Vec::new()
    };
    sheet(
        "sheet Remote",
        sheet_head(word("desktop.menu.remote_title"), Some(state)),
        body,
        sheet_foot(left, vec![close_button()]),
    )
}
