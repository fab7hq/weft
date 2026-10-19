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

/// A string field of a JSON object, or empty.
fn s_of(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
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

/// A tunnel's installation guide, opened in the browser: `id` names its layer.
fn guide_link(id: &str, url: &str, shown: &str) -> Element {
    div()
        .layer(format!("guide {id}"))
        .font_size(13)
        .color(palette::BLUE_BORDER)
        .on_click(SheetAct::OpenGuide(url.to_string()))
        .text(shown)
}

/// What the tunnel's program last said before it gave an address, under
/// its name, word for word.
fn tunnel_why(title: &str, said: &str) -> Element {
    div()
        .layer("tunnel_why")
        .self_stretch()
        .flex_col()
        .gap(6)
        .pad(10, 14)
        .bg(palette::AMBER_BG)
        .border(palette::AMBER_BORDER)
        .child(
            div()
                .font_mono()
                .font_size(11)
                .font_weight(600)
                .color(palette::AMBER)
                .text(fill("desktop.remote.tunnel_said", &[("tunnel", &title.to_uppercase())])),
        )
        .child(div().font_mono().font_size(12).color(palette::AMBER).text(said))
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
    // Beside Turn off, while weft remote says there is another tunnel to use.
    let change = view.is_some_and(|v| v["can_change_tunnel"] == true).then(|| {
        button(
            "btn Change tunnel",
            word("desktop.remote.change_tunnel"),
            SheetAct::RemoteChangeTunnel,
            Tone::Plain,
            Size::Sheet,
        )
    });
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
            let chosen = view.and_then(|v| v["chosen"].as_str());
            let picked = props.remote_pick.or(chosen).or(first);
            let rows = tunnels.iter().filter_map(|t| {
                let (id, title) = (t["id"].as_str()?, t["title"].as_str()?);
                let on = picked == Some(id);
                let mut name = div().flex_1().flex_col().gap(2).child(
                    div()
                        .font_size(14)
                        .font_weight(if on { 600 } else { 400 })
                        .color(if on { palette::BLUE_BORDER } else { palette::TEXT })
                        .text(title),
                );
                if let Some(note) = t["note"].as_str() {
                    name = name.child(div().font_size(12).color(palette::MUTED).text(note));
                }
                let mut row = list_row()
                    .layer(format!("tunnel {title}"))
                    .gap(12)
                    .pad(12, 16)
                    .items_center()
                    .on_click(SheetAct::RemotePick(id.to_string()))
                    .child(name);
                if let Some(url) = t["guide"].as_str() {
                    row = row.child(guide_link(id, url, word("desktop.remote.guide")));
                }
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
            let check = button(
                "btn Check",
                word("desktop.remote.check"),
                SheetAct::RemoteUse,
                Tone::Primary,
                Size::Sheet,
            );
            (
                (word("desktop.remote.choose").to_string(), palette::MUTED),
                (vec![check], vec![close]),
            )
        }
        "not_installed" => {
            let tunnel = view.map(|v| v["tunnel"].clone()).unwrap_or_default();
            let (id, program) = (s_of(&tunnel, "id"), s_of(&tunnel, "program"));
            let title = tunnel["title"].as_str().unwrap_or(&program).to_string();
            body = body.child(
                div()
                    .layer("not found")
                    .self_stretch()
                    .flex_col()
                    .gap(6)
                    .pad(10, 14)
                    .bg(palette::AMBER_BG)
                    .border(palette::AMBER_BORDER)
                    .color(palette::AMBER)
                    .font_size(13)
                    .child(div().text(fill(
                        "desktop.remote.not_found_what",
                        &[("title", &title), ("program", &program)],
                    )))
                    .child(div().text(word("desktop.remote.not_found_then"))),
            );
            if let Some(url) = tunnel["guide"].as_str() {
                let shown = format!("{} {}", word("desktop.remote.guide"), url);
                body = body.child(guide_link(&id, url, &shown));
            }
            let check = button(
                "btn Check",
                word("desktop.remote.check"),
                SheetAct::RemoteLook,
                Tone::Primary,
                Size::Sheet,
            );
            (
                (word("desktop.remote.not_found").to_string(), palette::AMBER),
                ([Some(check), change].into_iter().flatten().collect(), vec![turn_off(), close]),
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
                (
                    vec![sign_in],
                    [Some(turn_off()), change, Some(close)].into_iter().flatten().collect(),
                ),
            )
        }
        "on" => {
            body = body.child(note(
                fill("desktop.remote.open_at", &[("login", &s("login"))]),
                palette::TEXT,
            ));
            let up = s("tunnel") == "up";
            let tunnel = if up {
                fill("desktop.remote.tunnel_up", &[("address", &s("address"))])
            } else {
                word("desktop.remote.tunnel_starting").to_string()
            };
            body = body.child(div().font_mono().font_size(12).color(palette::MUTED).text(tunnel));
            if !s("tunnel_why").is_empty() {
                body = body.child(tunnel_why(&s("tunnel_title"), &s("tunnel_why")));
            }
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
            let aside = if up {
                (word("desktop.remote.on").to_string(), palette::TEAL)
            } else {
                (word("desktop.remote.starting").to_string(), palette::AMBER)
            };
            (aside, ([Some(turn_off()), change].into_iter().flatten().collect(), vec![close]))
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
    if state != "on" && !s("tunnel_why").is_empty() {
        body = body.child(tunnel_why(&s("tunnel_title"), &s("tunnel_why")));
    }
    sheet(
        "sheet Remote",
        sheet_head(word("desktop.menu.remote_title"), Some(aside)),
        body,
        sheet_foot(left, right),
    )
}
