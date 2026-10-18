//! Remote: everything `weft remote` says it is, and the acts it takes.

use serde_json::Value;
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::{SheetAct, WindowAct};
use crate::gpui::{Element, div, edge_bar, palette};
use crate::ui::{
    Size, Tone, button, close_button, list_row, note, row_list, section_label, sheet, sheet_body,
    sheet_foot, sheet_head,
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
    // No view from a daemon that answered: Remote is not running. From none
    // yet, nothing is known.
    let heard = props.clients.iter().any(|c| c.is_connected());
    let state = view.and_then(|v| v["state"].as_str()).unwrap_or(if heard { "off" } else { "" });
    let s = |k: &str| view.and_then(|v| v[k].as_str()).unwrap_or_default().to_string();
    let close = close_button();
    let turn_off = || {
        button(
            "btn Turn off",
            word("desktop.remote.turn_off"),
            SheetAct::RemoteOff,
            Tone::Quiet,
            Size::Sheet,
        )
    };
    let mut body = sheet_body();
    let (aside, (left, right)) = match state {
        "off" => {
            body = body.child(note(word("desktop.remote.off_what"), palette::MUTED));
            let on = button(
                "btn Turn on",
                word("desktop.remote.turn_on"),
                SheetAct::RemoteOn,
                Tone::Primary,
                Size::Sheet,
            );
            ((word("desktop.remote.off").to_string(), palette::MUTED), (vec![on], vec![close]))
        }
        "choose" => {
            let tunnels = view.and_then(|v| v["tunnels"].as_array()).cloned().unwrap_or_default();
            let first = tunnels.first().and_then(|t| t["id"].as_str());
            let picked = props.remote_pick.or(first);
            let rows = tunnels.iter().filter_map(|t| {
                let (id, title) = (t["id"].as_str()?, t["title"].as_str()?);
                let on = picked == Some(id);
                let mut row = list_row()
                    .layer(format!("tunnel {title}"))
                    .pad(12, 16)
                    .on_click(SheetAct::RemotePick(id.to_string()))
                    .child(
                        div()
                            .flex_1()
                            .font_size(14)
                            .font_weight(if on { 600 } else { 400 })
                            .color(if on { palette::BLUE_BORDER } else { palette::TEXT })
                            .text(title),
                    );
                if on {
                    row = row.bg(palette::SELECTED_BG).child(edge_bar(
                        "left",
                        3,
                        palette::BLUE_BORDER,
                    ));
                }
                Some(row)
            });
            body = body
                .child(note(word("desktop.remote.choose_what"), palette::MUTED))
                .child(row_list(rows.collect()));
            let use_it = button(
                "btn Use it",
                word("desktop.remote.use_it"),
                SheetAct::RemoteUse,
                Tone::Primary,
                Size::Sheet,
            );
            (
                (word("desktop.remote.choose").to_string(), palette::MUTED),
                (vec![use_it], vec![close]),
            )
        }
        "not_installed" => {
            let tunnel = view.map(|v| v["tunnel"].clone()).unwrap_or_default();
            let program = tunnel["program"].as_str().unwrap_or_default();
            let title = tunnel["title"].as_str().unwrap_or(program);
            body = body.child(note(
                fill("desktop.remote.needs", &[("title", title), ("program", program)]),
                palette::MUTED,
            ));
            let offer = view.map(|v| v["offer"].clone()).filter(Value::is_object);
            let mut left = Vec::new();
            match &offer {
                Some(o) => {
                    let (run, said) = match o["how"].as_str() {
                        Some("brew") => (
                            o["argv"].as_str().unwrap_or_default().to_string(),
                            fill("desktop.remote.by_brew", &[("program", program)]),
                        ),
                        _ => {
                            let from = o["from"].as_str().unwrap_or_default();
                            (
                                from.to_string(),
                                fill(
                                    "desktop.remote.by_download",
                                    &[("program", program), ("from", from)],
                                ),
                            )
                        }
                    };
                    body = body
                        .child(section_label(word("desktop.remote.run_this")))
                        .child(row_list(vec![
                            list_row().pad(12, 16).font_mono().font_size(12).text(run),
                        ]))
                        .child(note(said, palette::MUTED));
                    left.push(button(
                        "btn Run it",
                        word("desktop.remote.run_it"),
                        SheetAct::RemoteInstall,
                        Tone::Primary,
                        Size::Sheet,
                    ));
                    left.push(button(
                        "btn Cancel",
                        word("act.cancel"),
                        SheetAct::Close,
                        Tone::Plain,
                        Size::Sheet,
                    ));
                }
                None => body = body.child(note(s("why"), palette::AMBER)),
            }
            let needed = fill("desktop.remote.needed", &[("program", program)]);
            ((needed, palette::AMBER), (left, vec![turn_off()]))
        }
        "installing" => {
            body = body.child(note(s("line"), palette::MUTED));
            (
                (word("desktop.remote.installing").to_string(), palette::AMBER),
                (Vec::new(), vec![close]),
            )
        }
        "signing_in" => {
            body = body
                .child(note(
                    fill("desktop.remote.enter_code", &[("url", &s("url"))]),
                    palette::TEXT,
                ))
                .child(
                    div()
                        .layer("sign-in code")
                        .pad(10, 16)
                        .font_mono()
                        .font_size(22)
                        .font_weight(700)
                        .bg(palette::SELECTED_BG)
                        .border(palette::BLUE_BORDER)
                        .text(s("code")),
                );
            let cancel = button(
                "btn Cancel",
                word("act.cancel"),
                SheetAct::RemoteCancel,
                Tone::Plain,
                Size::Sheet,
            );
            (
                (word("desktop.remote.signing_in").to_string(), palette::AMBER),
                (vec![cancel], vec![close]),
            )
        }
        "signed_out" => {
            body = body.child(note(word("desktop.remote.signed_out_what"), palette::MUTED));
            let sign_in = button(
                "btn Sign in",
                word("desktop.remote.sign_in"),
                SheetAct::RemoteSignIn,
                Tone::Primary,
                Size::Sheet,
            );
            (
                (word("desktop.remote.signed_out").to_string(), palette::MUTED),
                (vec![sign_in], vec![turn_off(), close]),
            )
        }
        "on" => {
            body = body.child(note(
                fill("desktop.remote.open_at", &[("login", &s("login"))]),
                palette::TEXT,
            ));
            let tunnel = if s("tunnel") == "up" {
                fill("desktop.remote.tunnel_up", &[("address", &s("address"))])
            } else {
                word("desktop.remote.tunnel_starting").to_string()
            };
            body = body.child(div().font_mono().font_size(12).color(palette::MUTED).text(tunnel));
            if !s("note").is_empty() {
                body = body.child(note(s("note"), palette::MUTED));
            }
            if !props.clients.iter().any(|c| c.start_at_login) {
                body = body.child(
                    div()
                        .layer("start at login warning")
                        .self_stretch()
                        .flex_row()
                        .gap(8)
                        .pad(10, 14)
                        .items_center()
                        .bg(palette::AMBER_BG)
                        .border(palette::AMBER_BORDER)
                        .color(palette::AMBER)
                        .font_size(13)
                        .child(div().text("■"))
                        .child(div().flex_1().text(word("desktop.remote.login_off")))
                        .child(button(
                            "btn Turn on Start at login",
                            word("desktop.remote.login_on"),
                            WindowAct::ToggleStartAtLogin,
                            Tone::Warn,
                            Size::Row,
                        )),
                );
            }
            let devices: Vec<Element> = view
                .and_then(|v| v["devices"].as_array())
                .into_iter()
                .flatten()
                .filter_map(device_row)
                .collect();
            body = body.child(section_label(word("desktop.remote.devices")));
            body = if devices.is_empty() {
                body.child(note(word("desktop.remote.no_devices"), palette::MUTED))
            } else {
                body.child(row_list(devices))
            };
            (
                (word("desktop.remote.on").to_string(), palette::TEAL),
                (vec![turn_off()], vec![close]),
            )
        }
        "" => {
            ((word("web.link.connecting").to_string(), palette::MUTED), (Vec::new(), vec![close]))
        }
        // What the daemon says while `weft remote` starts, or anything newer.
        _ => (
            (word("desktop.remote.starting").to_string(), palette::MUTED),
            (vec![turn_off()], vec![close]),
        ),
    };
    if !s("failed").is_empty() {
        body = body.child(note(s("failed"), palette::AMBER));
    }
    sheet(
        "sheet Remote",
        sheet_head(word("desktop.menu.remote_title"), Some(aside)),
        body,
        sheet_foot(left, right),
    )
}
