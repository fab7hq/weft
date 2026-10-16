//! Right-pane sheets and popovers (`D4.1`–`D4.4` Ask/Follow up, `D11.2` Work
//! Details, `D11.3` Eval Details, `D11.4` Seal Details, `D11.5`/`D11.9` Session
//! Details, `D11.6` Earlier sessions, `D6.1`–`D6.7` Eval view, `D7.2` Updates,
//! `D8.3` Remote, and `D9.9` `☰` Mac menu popover).

use weft_core::harness::Harnesses;
use weft_core::ledger::Unit;
use weft_core::onboarding::{State as SetupState, View as SetupView};
use weft_core::sessions::State as SessionState;
use weft_core::tree::{Project, Word};
use weft_core::words;

use crate::app::ActiveSheet;
use crate::daemon::ProjectClient;
use crate::gpui::{Element, div, palette, textarea_el};

pub struct SheetProps<'a> {
    pub sheet: &'a ActiveSheet,
    pub projects: &'a [Project],
    pub clients: &'a [ProjectClient],
    pub harnesses: &'a Harnesses,
    pub updates_count: usize,
    pub bottom_bar: &'a crate::app::BottomBarState,
    /// What the open Ask's field holds.
    pub ask_draft: &'a str,
}

/// Render the active right-pane sheet (`D4.x`, `D6.x`, `D7.2`, `D8.3`, `D9.2`, `D11.x`).
pub fn render_sheet(props: &SheetProps) -> Element {
    match props.sheet {
        ActiveSheet::Ask { project, target_session, follow_up_unit } => render_ask_sheet(
            props,
            project,
            target_session.as_deref(),
            follow_up_unit.as_deref(),
            props.ask_draft,
        ),
        ActiveSheet::WorkDetail { project, unit } => render_work_detail(props, project, unit),
        ActiveSheet::EvalDetail { project, eval } => render_eval_detail(props, project, eval),
        ActiveSheet::EvalView { project, eval, by_file, selected_step } => {
            render_eval_view(props, project, eval, *by_file, *selected_step)
        }
        ActiveSheet::SealDetail { project, seal } => render_seal_detail(props, project, seal),
        ActiveSheet::SessionDetail { project, session } => {
            render_session_detail(props, project, session)
        }
        ActiveSheet::Updates => render_updates_sheet(props),
        ActiveSheet::Remote => render_remote_sheet(props),
    }
}

/// Render the `D9.9` `☰` Mac menu popover.
pub fn render_menu_popover(
    turbo: bool,
    start_at_login: bool,
    remote_on: bool,
    remote_devices: usize,
    updates_count: usize,
) -> Element {
    let turbo_badge = if turbo {
        div()
            .layer("turbo toggle badge")
            .pad(3, 9)
            .bg(palette::AMBER_BG)
            .border(palette::AMBER_BORDER)
            .font_mono()
            .font_size(11)
            .font_weight(600)
            .color(palette::AMBER)
            .shrink_0()
            .text("ON")
    } else {
        div()
            .layer("turbo toggle badge")
            .pad(3, 9)
            .bg(palette::SURFACE)
            .border(palette::LINE)
            .font_mono()
            .font_size(11)
            .font_weight(600)
            .color(palette::MUTED)
            .shrink_0()
            .text("OFF")
    };

    let login_badge = if start_at_login {
        div()
            .layer("login toggle badge")
            .pad(3, 9)
            .bg(palette::SELECTED_BG)
            .border(palette::BLUE_BORDER)
            .font_mono()
            .font_size(11)
            .font_weight(600)
            .color(palette::BLUE_BORDER)
            .shrink_0()
            .text("ON")
    } else {
        div()
            .layer("login toggle badge")
            .pad(3, 9)
            .bg(palette::SURFACE)
            .border(palette::LINE)
            .font_mono()
            .font_size(11)
            .font_weight(600)
            .color(palette::MUTED)
            .shrink_0()
            .text("OFF")
    };

    let remote_status = if remote_on {
        if remote_devices > 0 {
            format!(
                "● On · {} {}",
                remote_devices,
                if remote_devices == 1 { "device" } else { "devices" }
            )
        } else {
            "● On".to_string()
        }
    } else {
        "Off".to_string()
    };

    let updates_status = if updates_count > 0 {
        format!("↑ {updates_count} available")
    } else {
        "Up to date".to_string()
    };

    div()
        .layer("menu popover")
        .style("position", "absolute")
        .style("top", "8px")
        .style("right", "16px")
        .w(310)
        .flex_col()
        .bg(palette::SURFACE_ALT)
        .border(palette::LINE)
        .panel_corners()
        .child(
            div()
                .layer("menu Open project")
                .border_bottom(palette::LINE)
                .on_click("open_project_dialog")
                .flex_row()
                .pad(10, 14)
                .items_center()
                .justify_between()
                .child(div().font_size(13).text("Open project…"))
                .child(div().font_mono().font_size(11).color(palette::DIM).text("⌘O")),
        )
        .child(
            div()
                .layer("menu Turbo")
                .border_bottom(palette::LINE)
                .on_click("toggle_turbo")
                .flex_row()
                .pad(10, 14)
                .items_center()
                .justify_between()
                .gap(12)
                .child(
                    div()
                        .flex_1()
                        .style("min-width", "0")
                        .flex_col()
                        .gap(2)
                        .child(div().font_size(13).text("⚡ Turbo"))
                        .child(
                            div()
                                .font_size(11)
                                .color(palette::MUTED)
                                .text("New agents get every permission and ask nothing"),
                        ),
                )
                .child(turbo_badge),
        )
        .child(
            div()
                .layer("menu Start at login")
                .border_bottom(palette::LINE)
                .on_click("toggle_start_at_login")
                .flex_row()
                .pad(10, 14)
                .items_center()
                .justify_between()
                .gap(12)
                .child(
                    div()
                        .flex_1()
                        .style("min-width", "0")
                        .flex_col()
                        .gap(2)
                        .child(div().font_size(13).text("Start at login"))
                        .child(
                            div()
                                .font_size(11)
                                .color(palette::MUTED)
                                .text("Keep Weft running in the background"),
                        ),
                )
                .child(login_badge),
        )
        .child(
            div()
                .layer("menu Remote")
                .border_bottom(palette::LINE)
                .on_click("open_remote")
                .flex_row()
                .pad(10, 14)
                .items_center()
                .justify_between()
                .child(div().font_size(13).text("Remote…"))
                .child(
                    div()
                        .font_mono()
                        .font_size(11)
                        .color(if remote_on { palette::TEAL } else { palette::MUTED })
                        .text(remote_status),
                ),
        )
        .child(
            div()
                .layer("menu Updates")
                .border_bottom(palette::LINE)
                .on_click("open_updates")
                .flex_row()
                .pad(10, 14)
                .items_center()
                .justify_between()
                .child(div().font_size(13).text("Updates…"))
                .child(
                    div()
                        .font_mono()
                        .font_size(11)
                        .color(if updates_count > 0 {
                            palette::BLUE_BORDER
                        } else {
                            palette::MUTED
                        })
                        .text(updates_status),
                ),
        )
        .child(
            div()
                .layer("menu Quit Weft")
                .on_click("request_quit")
                .flex_row()
                .pad(10, 14)
                .items_center()
                .justify_between()
                .bg(palette::SELECTED_BG)
                .child(div().font_size(13).color(palette::BLUE_BORDER).text("Quit Weft…"))
                .child(div().font_mono().font_size(11).color(palette::BLUE_BORDER).text("⌘Q")),
        )
}

fn render_ask_sheet(
    props: &SheetProps,
    project: &str,
    target_session: Option<&str>,
    follow_up_unit: Option<&str>,
    draft: &str,
) -> Element {
    let title = if follow_up_unit.is_some() {
        words::word("act.follow_up")
    } else {
        words::word("ask.title")
    };

    let mut targets = div().flex_row().gap(6).style("flex-wrap", "wrap");
    if let Some(proj) = props.projects.iter().find(|p| p.name == project) {
        for a in proj.agents.rows.iter().filter(|a| a.state == SessionState::Running) {
            let is_sel = target_session == Some(a.session.as_str())
                || (target_session.is_none()
                    && proj.agents.rows.first().map(|f| f.session.as_str()) == Some(&a.session));
            targets = targets.child(
                div()
                    .layer(format!("send target {}", a.harness))
                    .on_click(format!("select_ask_target:{}:{}", project, a.session))
                    .flex_row()
                    .gap(8)
                    .pad(8, 12)
                    .items_center()
                    .bg(if is_sel { palette::SELECTED_BG } else { palette::SURFACE })
                    .border(if is_sel { palette::BLUE_BORDER } else { palette::LINE })
                    .child(div().font_size(14).font_weight(600).text(a.harness.clone()))
                    .child(div().font_size(12).color(palette::MUTED).text(
                        a.title.clone().unwrap_or_else(|| words::word("tree.no_ask").to_string()),
                    )),
            );
        }
    }

    div()
        .layer("sheet Ask")
        .self_stretch()
        .shrink_0()
        .flex_col()
        .gap(12)
        .pad(16, 20)
        .items_start()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
        .child(div().font_weight(600).font_size(15).text(title))
        .child(
            textarea_el()
                .layer("ask input")
                .self_stretch()
                .style("min-height", "78px")
                .pad(10, 12)
                .bg(palette::SURFACE)
                .border(palette::BLUE_BORDER)
                .font_size(14)
                .style("line-height", "20px")
                .color(palette::TEXT)
                .style("resize", "none")
                .style("outline", "none")
                .placeholder(words::word("ask.title"))
                .text(draft.to_string()),
        )
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(12)
                .items_center()
                .child(
                    div()
                        .font_mono()
                        .font_mono()
                        .font_weight(600)
                        .font_size(11)
                        .style("letter-spacing", "0.08em")
                        .color(palette::MUTED)
                        .text(words::word("ask.send_to")),
                )
                .child(targets)
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Cancel")
                        .on_click("close_sheet")
                        .pad(8, 18)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .color(palette::MUTED)
                        .text(words::word("act.cancel")),
                )
                .child(
                    div()
                        .layer("btn Ask")
                        .on_click("submit_ask_sheet")
                        .pad(8, 18)
                        .bg(palette::BLUE)
                        .border(palette::BLUE_BORDER)
                        .font_weight(600)
                        .font_size(14)
                        .color(palette::CANVAS)
                        .text(if follow_up_unit.is_some() {
                            words::word("act.follow_up")
                        } else {
                            words::word("act.ask")
                        }),
                ),
        )
}

fn render_work_detail(props: &SheetProps, project: &str, unit_id: &str) -> Element {
    let client = props.clients.iter().find(|c| c.name == project);
    let unit: Option<&Unit> = client.and_then(|c| c.units.iter().find(|u| u.ask_id == unit_id));
    let title = unit.map(|u| u.title.as_str()).unwrap_or(unit_id);
    let harness = unit.map(|u| u.harness.as_str()).unwrap_or("agent");
    let word = unit.map(Word::of).map(Word::say).unwrap_or("ASKED");

    let ask_dir = client.map(|c| c.root.join(".fab7/rf/asks").join(unit_id));
    let source_txt =
        ask_dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("source.txt")).ok());
    let prompt_txt =
        ask_dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("prompt.txt")).ok());

    let proj = props.projects.iter().find(|p| p.name == project);
    let session_agent = unit.and_then(|u| {
        let sid = u.session_ref.as_deref()?;
        proj.and_then(|p| p.agents.rows.iter().find(|a| a.session == sid).cloned()).or_else(|| {
            client.and_then(|c| c.sessions.iter().find(|r| r.id == sid)).map(|r| {
                weft_core::tree::Agent {
                    pane: r.pane,
                    session: r.id.clone(),
                    harness: r.harness.clone(),
                    title: r.title.clone(),
                    state: r.state,
                    turn: r.turn,
                    at: r.last,
                    asks: r.asks,
                    ask: None,
                }
            })
        })
    });

    let detail = match (client, unit) {
        (Some(c), Some(u)) => Some(weft_core::offers::work_detail(
            u,
            &c.rounds.record,
            &c.rounds,
            session_agent,
            source_txt.as_deref(),
            prompt_txt.as_deref(),
            c.zone,
        )),
        _ => None,
    };

    let can_proceed = detail.as_ref().is_some_and(|d| d.proceed)
        || unit.is_some_and(|u| !u.confirmed && !u.cancelled);

    let mut actions = div()
        .self_stretch()
        .flex_row()
        .gap(10)
        .pad(10, 20)
        .items_center()
        .border_top(palette::LINE);

    if can_proceed {
        actions = actions.child(
            div()
                .layer("btn Proceed")
                .on_click(format!("act_proceed:{project}:{unit_id}"))
                .pad(9, 18)
                .bg(palette::BLUE)
                .border(palette::BLUE_BORDER)
                .font_weight(600)
                .font_size(14)
                .color(palette::CANVAS)
                .text(words::word("act.proceed")),
        );
    }

    actions = actions
        .child(
            div()
                .layer("btn Follow up")
                .on_click(format!("open_follow_up:{project}:{unit_id}"))
                .pad(9, 18)
                .bg(palette::SURFACE)
                .border(palette::LINE)
                .font_weight(600)
                .font_size(14)
                .text(words::word("act.follow_up")),
        )
        .child(div().flex_1())
        .child(
            div()
                .layer("btn Close")
                .on_click("close_sheet")
                .pad(9, 18)
                .bg(palette::SURFACE)
                .border(palette::LINE)
                .font_weight(600)
                .font_size(14)
                .text(words::word("act.close")),
        );

    let mut body = div()
        .self_stretch()
        .flex_1()
        .flex_col()
        .style("min-height", "0")
        .style("overflow-y", "auto")
        .gap(12)
        .pad(18, 20)
        .items_start();

    if let Some(d) = &detail {
        // Breadcrumb row (`› Eval N verdict › Seal M` or `› not evaluated yet`)
        let mut crumb_row = div()
            .self_stretch()
            .flex_row()
            .gap(8)
            .items_center()
            .pad(8, 12)
            .bg(palette::SURFACE_ALT)
            .border(palette::LINE);
        crumb_row =
            crumb_row.child(div().font_size(12).color(palette::DIM).text(unit_id.to_string()));
        crumb_row = crumb_row.child(div().font_size(12).color(palette::DIM).text("›"));
        if let Some(ev) = &d.crumb.eval {
            let ev_text = match ev.verdict {
                Some(v) => format!("Eval {} · {}", ev.n, v),
                None => format!("Eval {} · {}", ev.n, words::word("tree.judging")),
            };
            crumb_row = crumb_row.child(
                div()
                    .on_click(format!("select_eval:{project}:{}", ev.eval))
                    .font_size(12)
                    .color(palette::BLUE_BORDER)
                    .text(ev_text),
            );
            crumb_row = crumb_row.child(div().font_size(12).color(palette::DIM).text("›"));
            if let Some(sl) = &d.crumb.seal {
                crumb_row = crumb_row.child(
                    div()
                        .on_click(format!("select_seal:{project}:{}", sl.seal))
                        .font_size(12)
                        .color(palette::TEAL)
                        .text(format!("Seal {}", sl.n)),
                );
            } else {
                crumb_row = crumb_row.child(
                    div()
                        .font_size(12)
                        .color(palette::MUTED)
                        .text(words::word("web.detail.not_sealed")),
                );
            }
        } else {
            crumb_row = crumb_row.child(
                div()
                    .font_size(12)
                    .color(palette::MUTED)
                    .text(words::word("web.detail.not_evaluated")),
            );
        }
        body = body.child(crumb_row);

        if !d.ask.aside.is_empty() {
            body = body.child(
                div()
                    .font_size(12)
                    .color(palette::MUTED)
                    .text(format!("{} · {}", d.ask.state, d.ask.aside)),
            );
        }
    }

    // Session row
    body = body
        .child(div().font_size(12).color(palette::DIM).text(words::word("web.detail.its_session")));
    if let Some(sess) = detail.as_ref().and_then(|d| d.session.as_ref()) {
        body = body.child(
            div()
                .on_click(format!("select_agent:{project}:{}", sess.session))
                .font_size(14)
                .color(palette::BLUE_BORDER)
                .text(format!("{} · {} ›", sess.harness, sess.session)),
        );
    } else if let Some(sid) = unit.and_then(|u| u.session_ref.as_deref()) {
        body = body.child(
            div()
                .on_click(format!("select_agent:{project}:{sid}"))
                .font_size(14)
                .color(palette::BLUE_BORDER)
                .text(format!("{harness} · {sid} ›")),
        );
    } else {
        body = body
            .child(div().font_size(14).color(palette::BLUE_BORDER).text(format!("{harness} ›")));
    }

    // As typed (`source.txt` or `title`)
    let as_typed = source_txt.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(title);
    body = body
        .child(div().font_size(12).color(palette::DIM).text(words::word("web.detail.as_typed")))
        .child(
            div()
                .self_stretch()
                .pad(12, 14)
                .bg(palette::SURFACE)
                .border(palette::LINE)
                .font_size(14)
                .text(as_typed.to_string()),
        );

    // Composed prompt (`prompt.txt` when present)
    if let Some(prompt) = prompt_txt.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        body = body
            .child(
                div()
                    .font_size(12)
                    .color(palette::DIM)
                    .text(format!("The prompt {harness} composed:")),
            )
            .child(
                div()
                    .self_stretch()
                    .pad(12, 14)
                    .bg(palette::SURFACE)
                    .border(palette::LINE)
                    .font_size(13)
                    .color(palette::MUTED)
                    .text(prompt.to_string()),
            );
    }

    div()
        .layer("sheet WorkDetail")
        .flex_1()
        .self_stretch()
        .flex_col()
        .style("min-height", "0")
        .items_start()
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .pad(12, 20)
                .items_baseline()
                .border_bottom(palette::LINE)
                .child(div().flex_1().font_weight(600).font_size(16).text(title.to_string()))
                .child(div().font_size(12).color(palette::MUTED).text(word)),
        )
        .child(body)
        .child(actions)
}

fn load_eval_judges(
    client: Option<&ProjectClient>,
    eval_id: &str,
) -> Option<weft_core::record::Record> {
    let client = client?;
    if let Some(val) = client.records.get(eval_id) {
        if let Some(rec_val) = val.get("record")
            && let Some(r) = weft_core::record::Record::parse(rec_val)
        {
            return Some(r);
        }
        if let Some(r) = weft_core::record::Record::parse(val) {
            return Some(r);
        }
    }
    let path = client.root.join(".fab7/rf/evals").join(eval_id).join("record.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&raw).ok()?;
    weft_core::record::Record::parse(&val)
}

fn render_eval_detail(props: &SheetProps, project: &str, eval_id: &str) -> Element {
    let proj = props.projects.iter().find(|p| p.name == project);
    let client = props.clients.iter().find(|c| c.name == project);
    let group = proj.and_then(|p| p.work.evals.iter().find(|g| g.eval == eval_id));
    let judges = load_eval_judges(client, eval_id);

    let detail = client.and_then(|c| {
        let ev = c.rounds.record.evals.iter().find(|e| e.id == eval_id)?;
        let seal_why = group.and_then(|g| g.seal.clone());
        Some(weft_core::offers::eval_detail(
            ev,
            judges.as_ref(),
            &c.units,
            &c.rounds.record,
            seal_why,
        ))
    });

    let n = detail.as_ref().map(|d| d.n).or_else(|| group.map(|g| g.n)).unwrap_or(1);
    let is_judging =
        detail.as_ref().map(|d| d.judging).or_else(|| group.map(|g| g.judging)).unwrap_or(false);
    let verdict = if is_judging {
        words::word("tree.judging")
    } else {
        detail
            .as_ref()
            .and_then(|d| d.verdict)
            .or_else(|| group.and_then(|g| g.verdict))
            .unwrap_or("aligned")
    };
    let agreement = detail
        .as_ref()
        .and_then(|d| d.agreement.clone())
        .or_else(|| group.and_then(|g| g.agreement.clone()))
        .unwrap_or_else(|| if is_judging { String::new() } else { "1.0".to_string() });
    let count =
        detail.as_ref().map(|d| d.asks.len()).or_else(|| group.map(|g| g.count)).unwrap_or(0);

    let mut body = div()
        .self_stretch()
        .flex_1()
        .flex_col()
        .style("min-height", "0")
        .style("overflow-y", "auto")
        .gap(12)
        .pad(18, 20)
        .items_start();

    if let Some(d) = &detail {
        if let Some(by) = &d.judged_by {
            let subtitle =
                match d.at.and_then(|t| client.map(|c| weft_core::offers::stamp(t, c.zone))) {
                    Some(ts) => format!("Judged by {by} · {ts}"),
                    None => format!("Judged by {by}"),
                };
            body = body.child(div().font_size(12).color(palette::MUTED).text(subtitle));
        }

        if let Some(sl) = &d.sealed {
            body = body.child(
                div()
                    .on_click(format!("select_seal:{project}:{}", sl.seal))
                    .font_size(13)
                    .color(palette::TEAL)
                    .text(format!("Sealed by Seal {} ›", sl.n)),
            );
        } else {
            body = body.child(
                div()
                    .font_size(13)
                    .color(palette::MUTED)
                    .text(words::fill("web.eval.seal_what", &[("n", &count.to_string())])),
            );
        }

        if !d.findings.is_empty() {
            body = body.child(
                div()
                    .font_mono()
                    .font_weight(600)
                    .font_size(11)
                    .style("letter-spacing", "0.08em")
                    .color(palette::DIM)
                    .text(words::word("web.eval.findings").to_uppercase()),
            );
            let mut fbox =
                div().self_stretch().flex_col().bg(palette::SURFACE).border(palette::LINE);
            let total_f = d.findings.len();
            for (idx, f) in d.findings.iter().enumerate() {
                let mut r = div().self_stretch().flex_row().gap(10).pad(10, 14).items_start();
                if idx + 1 < total_f {
                    r = r.border_bottom(palette::LINE);
                }
                let mark_color = match f.mark {
                    "✓" => palette::TEAL,
                    "✗" => palette::RED,
                    _ => palette::AMBER,
                };
                r = r
                    .child(div().font_weight(700).font_size(13).color(mark_color).text(f.mark))
                    .child(div().flex_1().font_size(13).text(f.text.clone()));
                fbox = fbox.child(r);
            }
            body = body.child(fbox);
        }

        if !d.checks.is_empty() {
            body = body.child(
                div()
                    .font_mono()
                    .font_weight(600)
                    .font_size(11)
                    .style("letter-spacing", "0.08em")
                    .color(palette::DIM)
                    .text(words::word("eval_view.checks").to_uppercase()),
            );
            let mut cbox =
                div().self_stretch().flex_col().bg(palette::SURFACE).border(palette::LINE);
            let total_c = d.checks.len();
            for (idx, c) in d.checks.iter().enumerate() {
                let mut r = div().self_stretch().flex_row().gap(10).pad(10, 14).items_center();
                if idx + 1 < total_c {
                    r = r.border_bottom(palette::LINE);
                }
                let mark_color = match c.mark {
                    "✓" => palette::TEAL,
                    "✗" => palette::RED,
                    _ => palette::AMBER,
                };
                r = r
                    .child(div().font_weight(700).font_size(13).color(mark_color).text(c.mark))
                    .child(div().flex_1().font_mono().font_size(12).text(c.text.clone()));
                if let Some(res) = &c.result {
                    r = r.child(div().font_size(12).color(palette::MUTED).text(res.clone()));
                }
                cbox = cbox.child(r);
            }
            body = body.child(cbox);
        }

        if !d.asks.is_empty() {
            body = body.child(
                div()
                    .font_mono()
                    .font_weight(600)
                    .font_size(11)
                    .style("letter-spacing", "0.08em")
                    .color(palette::DIM)
                    .text("ASKS IN THIS ROUND"),
            );
            let mut abox =
                div().self_stretch().flex_col().bg(palette::SURFACE).border(palette::LINE);
            let total_a = d.asks.len();
            for (idx, a) in d.asks.iter().enumerate() {
                let mut r = div()
                    .on_click(format!("select_ask:{project}:{}", a.unit))
                    .self_stretch()
                    .flex_row()
                    .gap(10)
                    .pad(10, 14)
                    .items_center();
                if idx + 1 < total_a {
                    r = r.border_bottom(palette::LINE);
                }
                r = r
                    .child(div().flex_1().font_size(13).text(a.title.clone()))
                    .child(div().font_size(12).color(palette::MUTED).text(a.harness.clone()))
                    .child(div().font_size(12).color(palette::DIM).text("›"));
                abox = abox.child(r);
            }
            body = body.child(abox);
        }
    } else {
        body = body.child(
            div()
                .font_size(13)
                .color(palette::MUTED)
                .text(words::fill("web.eval.seal_what", &[("n", &count.to_string())])),
        );
    }

    div()
        .layer("sheet EvalDetail")
        .flex_1()
        .self_stretch()
        .flex_col()
        .style("min-height", "0")
        .items_start()
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .pad(12, 20)
                .items_baseline()
                .border_bottom(palette::LINE)
                .child(
                    div()
                        .font_weight(600)
                        .font_size(16)
                        .text(words::fill("tree.eval", &[("n", &n.to_string())])),
                )
                .child(
                    div()
                        .font_size(13)
                        .color(if is_judging { palette::AMBER } else { palette::TEAL })
                        .text(verdict),
                )
                .child(div().font_size(13).color(palette::MUTED).text(agreement)),
        )
        .child(body)
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(10, 20)
                .items_center()
                .border_top(palette::LINE)
                .child(
                    div()
                        .layer("btn Seal")
                        .on_click(format!("open_seal:{project}:{eval_id}"))
                        .pad(9, 18)
                        .bg(palette::BLUE)
                        .border(palette::BLUE_BORDER)
                        .font_weight(600)
                        .font_size(14)
                        .color(palette::CANVAS)
                        .text(words::word("act.seal")),
                )
                .child(
                    div()
                        .layer("btn Eval view")
                        .on_click(format!("open_eval_view:{project}:{eval_id}"))
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.eval_view")),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Close")
                        .on_click("close_sheet")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.close")),
                ),
        )
}

fn render_eval_view(
    props: &SheetProps,
    project: &str,
    eval_id: &str,
    by_file: bool,
    _selected_step: usize,
) -> Element {
    let client = props.clients.iter().find(|c| c.name == project);
    let judges = load_eval_judges(client, eval_id);
    let checks_count = judges.as_ref().map(|r| r.checks.len()).unwrap_or(1);
    let findings_count = judges.as_ref().map(|r| r.items.len()).unwrap_or(1);

    let mut list = div()
        .self_stretch()
        .flex_1()
        .flex_col()
        .style("min-height", "0")
        .style("overflow-y", "auto");

    list = list
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(11, 20)
                .items_center()
                .border_bottom(palette::LINE)
                .child(div().flex_1().font_size(15).text(words::word("eval_view.checks")))
                .child(div().font_size(12).color(palette::MUTED).text(checks_count.to_string()))
                .child(div().font_weight(600).font_size(16).color(palette::DIM).text("›")),
        )
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(11, 20)
                .items_center()
                .border_bottom(palette::LINE)
                .child(div().flex_1().font_size(15).text(words::word("eval_view.limitations")))
                .child(div().font_size(12).color(palette::MUTED).text(findings_count.to_string()))
                .child(div().font_weight(600).font_size(16).color(palette::DIM).text("›")),
        );

    if let Some(r) = &judges {
        for c in &r.checks {
            let ok = matches!(c.outcome.as_str(), "succeeded" | "passed");
            list = list.child(
                div()
                    .self_stretch()
                    .flex_row()
                    .gap(10)
                    .pad(10, 20)
                    .items_center()
                    .border_bottom(palette::LINE)
                    .child(
                        div()
                            .font_weight(700)
                            .font_size(13)
                            .color(if ok { palette::TEAL } else { palette::RED })
                            .text(if ok { "✓" } else { "✗" }),
                    )
                    .child(div().flex_1().font_mono().font_size(12).text(c.command.clone()))
                    .child(div().font_size(12).color(palette::MUTED).text(c.outcome.clone())),
            );
        }
        for item in &r.items {
            let mark = weft_core::offers::mark_for(item.plain_majority());
            let mark_color = match mark {
                "✓" => palette::TEAL,
                "✗" => palette::RED,
                _ => palette::AMBER,
            };
            list = list.child(
                div()
                    .self_stretch()
                    .flex_row()
                    .gap(10)
                    .pad(10, 20)
                    .items_start()
                    .border_bottom(palette::LINE)
                    .child(div().font_weight(700).font_size(13).color(mark_color).text(mark))
                    .child(div().flex_1().font_size(13).text(item.text.clone())),
            );
        }
    }

    div()
        .layer("sheet EvalView")
        .flex_1()
        .self_stretch()
        .flex_col()
        .style("min-height", "0")
        .items_start()
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(12, 20)
                .items_center()
                .border_bottom(palette::LINE)
                .child(
                    div()
                        .on_click(format!("select_eval:{project}:{eval_id}"))
                        .font_size(13)
                        .color(palette::BLUE_BORDER)
                        .text(format!("‹ {}", words::word("web.detail.title"))),
                )
                .child(
                    div()
                        .flex_1()
                        .font_weight(600)
                        .font_size(16)
                        .text(words::word("act.eval_view")),
                )
                .child(
                    div()
                        .layer("toggle eval view mode")
                        .on_click(format!("toggle_eval_view_mode:{project}:{eval_id}"))
                        .pad(4, 10)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_size(12)
                        .text(if by_file {
                            words::word("eval_view.by_file")
                        } else {
                            words::word("eval_view.by_step")
                        }),
                ),
        )
        .child(list)
        .child(
            div()
                .self_stretch()
                .flex_row()
                .pad(10, 20)
                .items_center()
                .border_top(palette::LINE)
                .child(
                    div()
                        .flex_1()
                        .font_size(12)
                        .color(palette::DIM)
                        .text(words::word("web.eval.click")),
                )
                .child(
                    div()
                        .layer("btn Close")
                        .on_click("close_sheet")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.close")),
                ),
        )
}

fn render_seal_detail(props: &SheetProps, project: &str, seal_id: &str) -> Element {
    let client = props.clients.iter().find(|c| c.name == project);
    let srow = client.and_then(|c| c.rounds.seals.iter().find(|s| s.seal == seal_id));
    let detail = client.and_then(|c| {
        let seal = c.rounds.record.seals.iter().find(|s| s.id == seal_id)?;
        let checked = c.records.get(seal_id);
        Some(weft_core::offers::seal_detail(seal, &c.units, &c.rounds.record, checked))
    });

    let n = detail.as_ref().map(|d| d.n).or_else(|| srow.map(|s| s.n)).unwrap_or(1);
    let decision = detail
        .as_ref()
        .map(|d| d.decision.clone())
        .or_else(|| srow.map(|s| weft_core::tree::decision(&s.decision)))
        .unwrap_or_else(|| "accepted".into());
    let eval_id = detail
        .as_ref()
        .and_then(|d| d.eval.as_ref().map(|e| e.eval.clone()))
        .or_else(|| srow.and_then(|s| s.eval.as_ref().map(|e| e.id.clone())))
        .unwrap_or_else(|| "evl_1".into());

    let mut body = div()
        .self_stretch()
        .flex_1()
        .flex_col()
        .style("min-height", "0")
        .style("overflow-y", "auto")
        .gap(12)
        .pad(18, 20)
        .items_start();

    if let Some(d) = &detail {
        if let Some(ev) = &d.eval {
            let ev_line = match (&ev.verdict, &ev.agreement) {
                (Some(v), Some(ag)) => format!("Seals Eval {} · {} {}", ev.n, v, ag),
                (Some(v), None) => format!("Seals Eval {} · {}", ev.n, v),
                _ => format!("Seals Eval {}", ev.n),
            };
            body = body.child(
                div()
                    .on_click(format!("select_eval:{project}:{}", ev.eval))
                    .font_size(13)
                    .color(palette::BLUE_BORDER)
                    .text(format!("{ev_line} ›")),
            );
        }

        body = body.child(
            div()
                .font_mono()
                .font_size(12)
                .color(palette::DIM)
                .text(format!("Receipt: {}", d.receipt)),
        );

        if d.holds == Some(false) && !d.broken.is_empty() {
            for b in &d.broken {
                body = body.child(div().font_size(13).color(palette::RED).text(format!("✗ {b}")));
            }
        } else {
            body = body.child(
                div().font_size(13).color(palette::TEAL).text(words::word("web.seal.holds")),
            );
        }

        if !d.asks.is_empty() {
            body = body.child(
                div()
                    .font_mono()
                    .font_weight(600)
                    .font_size(11)
                    .style("letter-spacing", "0.08em")
                    .color(palette::DIM)
                    .text("SEALED ASKS"),
            );
            let mut abox =
                div().self_stretch().flex_col().bg(palette::SURFACE).border(palette::LINE);
            let total_a = d.asks.len();
            for (idx, a) in d.asks.iter().enumerate() {
                let mut r = div()
                    .on_click(format!("select_ask:{project}:{}", a.unit))
                    .self_stretch()
                    .flex_row()
                    .gap(10)
                    .pad(10, 14)
                    .items_center();
                if idx + 1 < total_a {
                    r = r.border_bottom(palette::LINE);
                }
                r = r
                    .child(div().flex_1().font_size(13).text(a.title.clone()))
                    .child(div().font_size(12).color(palette::MUTED).text(a.harness.clone()))
                    .child(div().font_size(12).color(palette::DIM).text("›"));
                abox = abox.child(r);
            }
            body = body.child(abox);
        }
    } else {
        body = body
            .child(div().font_size(13).color(palette::TEAL).text(words::word("web.seal.holds")));
    }

    body =
        body.child(div().font_size(13).color(palette::MUTED).text(words::word("web.seal.final")));

    div()
        .layer("sheet SealDetail")
        .flex_1()
        .self_stretch()
        .flex_col()
        .style("min-height", "0")
        .items_start()
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .pad(12, 20)
                .items_baseline()
                .border_bottom(palette::LINE)
                .child(
                    div()
                        .font_weight(600)
                        .font_size(16)
                        .text(words::fill("tree.seal", &[("n", &n.to_string())])),
                )
                .child(div().font_size(13).color(palette::TEAL).text(decision)),
        )
        .child(body)
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(10, 20)
                .items_center()
                .border_top(palette::LINE)
                .child(
                    div()
                        .layer("btn Eval view")
                        .on_click(format!("open_eval_view:{project}:{eval_id}"))
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.eval_view")),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Close")
                        .on_click("close_sheet")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.close")),
                ),
        )
}

fn render_session_detail(props: &SheetProps, project: &str, session_id: &str) -> Element {
    let client = props.clients.iter().find(|c| c.name == project);
    let row = client.and_then(|c| c.sessions.iter().find(|r| r.id == session_id));
    let harness = row.map(|r| r.harness.as_str()).unwrap_or("agent");
    let joins = client
        .and_then(|c| c.harnesses.find(harness))
        .or_else(|| props.harnesses.find(harness))
        .is_some_and(|h| h.resume_joins);
    let detail = match (client, row) {
        (Some(c), Some(r)) => Some(weft_core::offers::session_detail(
            r,
            &c.units,
            &weft_core::rounds::Record::default(),
            joins,
        )),
        _ => None,
    };

    let resumable = detail
        .as_ref()
        .map(|d| d.resume)
        .unwrap_or_else(|| row.map(|r| r.resumable).unwrap_or(true));
    let joins_now = detail.as_ref().is_some_and(|d| d.joins);
    let state_word = match detail.as_ref().map(|d| d.state) {
        Some(SessionState::Running) => words::word("state.running"),
        _ => words::word("state.ended"),
    };
    let resume_note = if joins_now {
        words::fill("web.session.resume_joins", &[("harness", harness), ("session", session_id)])
    } else {
        words::fill("web.session.resume_how", &[("harness", harness), ("session", session_id)])
    };

    let mut body = div()
        .self_stretch()
        .flex_1()
        .flex_col()
        .gap(12)
        .pad(18, 20)
        .items_start()
        .style("min-height", "0")
        .style("overflow-y", "auto");

    if let Some(d) = &detail {
        if !d.asks.is_empty() {
            let mut asks_box =
                div().self_stretch().flex_col().bg(palette::SURFACE).border(palette::LINE);
            let total = d.asks.len();
            for (idx, a) in d.asks.iter().enumerate() {
                let mut r = div()
                    .on_click(format!("select_ask:{project}:{}", a.unit))
                    .self_stretch()
                    .flex_row()
                    .gap(10)
                    .pad(10, 14)
                    .items_center();
                if idx + 1 < total {
                    r = r.border_bottom(palette::LINE);
                }
                r = r
                    .child(div().flex_1().font_size(14).text(a.title.clone()))
                    .child(div().font_size(12).color(palette::MUTED).text(a.word.say()));
                asks_box = asks_box.child(r);
            }
            body = body
                .child(
                    div().font_size(12).color(palette::DIM).text(words::word("web.session.made")),
                )
                .child(asks_box);
        } else if let Some(last) = row.and_then(|r| r.title.as_ref()) {
            body = body
                .child(
                    div().font_size(12).color(palette::DIM).text(words::word("web.session.made")),
                )
                .child(
                    div()
                        .self_stretch()
                        .pad(12, 14)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_size(14)
                        .text(last.clone()),
                );
        } else {
            body = body.child(
                div().font_size(13).color(palette::MUTED).text(words::word("tree.nothing_asked")),
            );
        }
    }

    if resumable {
        body = body.child(div().font_size(13).color(palette::MUTED).text(resume_note));
    } else if detail.as_ref().map(|d| d.state) == Some(SessionState::Running)
        && detail.as_ref().and_then(|d| d.pane).is_none()
    {
        body = body.child(
            div()
                .font_size(13)
                .color(palette::AMBER)
                .text(words::fill("why.elsewhere", &[("harness", harness)])),
        );
    } else {
        body = body
            .child(div().font_size(13).color(palette::AMBER).text(words::word("resume.nothing")));
    }

    let mut footer = div()
        .self_stretch()
        .flex_row()
        .gap(10)
        .pad(10, 20)
        .items_center()
        .border_top(palette::LINE);

    if resumable {
        footer = footer.child(
            div()
                .layer("btn Resume")
                .on_click(format!("resume_session:{project}:{harness}:{session_id}"))
                .pad(9, 18)
                .bg(palette::BLUE)
                .border(palette::BLUE_BORDER)
                .font_weight(600)
                .font_size(14)
                .color(palette::CANVAS)
                .text(words::word("act.resume")),
        );
    }

    footer = footer.child(div().flex_1()).child(
        div()
            .layer("btn Close")
            .on_click("close_sheet")
            .pad(9, 18)
            .bg(palette::SURFACE)
            .border(palette::LINE)
            .font_weight(600)
            .font_size(14)
            .text(words::word("act.close")),
    );

    div()
        .layer("sheet SessionDetail")
        .flex_1()
        .self_stretch()
        .flex_col()
        .style("min-height", "0")
        .items_start()
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .pad(12, 20)
                .items_baseline()
                .border_bottom(palette::LINE)
                .child(
                    div().font_weight(600).font_size(16).text(format!("{harness} · {session_id}")),
                )
                .child(div().font_size(13).color(palette::MUTED).text(state_word)),
        )
        .child(body)
        .child(footer)
}

/// Render the `D7.2` Updates sheet (**Weft**, **RingFrame**, **Sense**, and
/// installed agents in plain English + **`Find app…`** + **`Update all`**).
fn render_updates_sheet(props: &SheetProps) -> Element {
    let view: SetupView = props.clients.iter().find_map(|c| c.setup.clone()).unwrap_or_default();

    let count = props.updates_count;
    let header_status = if count == 0 {
        "All up to date".to_string()
    } else if count == 1 {
        "1 update available".to_string()
    } else {
        format!("{count} updates available")
    };

    let weft_ver = if view.weft.is_empty() {
        format!("v{}", env!("CARGO_PKG_VERSION"))
    } else if view.weft.starts_with('v') {
        view.weft.clone()
    } else {
        format!("v{}", view.weft)
    };

    let weft_behind = view.weft_behind;
    let weft_status = if weft_behind {
        match &view.weft_latest {
            Some(v) => {
                format!("{weft_ver} → v{} · New version available", v.trim_start_matches('v'))
            }
            None => format!("{weft_ver} · Update available"),
        }
    } else {
        format!("{weft_ver} · Up to date")
    };

    let rf_ver = view
        .plugin
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|v| if v.starts_with('v') { v.to_string() } else { format!("v{v}") })
        .unwrap_or_else(|| weft_ver.clone());
    let rf_behind = view.configuration_behind;
    let rf_status = if rf_behind {
        format!("{rf_ver} · Update available")
    } else {
        format!("{rf_ver} · Up to date")
    };

    let sense_behind = view.sense_behind;
    let sense_status = if sense_behind {
        format!("{weft_ver} · Update available")
    } else {
        format!("{weft_ver} · Up to date")
    };

    let core_row = |name: &str, behind: bool, status: &str, act: &str, last: bool| {
        let mut r = div()
            .layer(format!("update row {name}"))
            .self_stretch()
            .flex_row()
            .gap(12)
            .pad(10, 16)
            .items_center();
        if !last {
            r = r.border_bottom(palette::LINE);
        }
        r = r
            .child(
                div()
                    .font_weight(700)
                    .font_size(14)
                    .color(if behind { palette::BLUE_BORDER } else { palette::TEAL })
                    .w(16)
                    .style("text-align", "center")
                    .text(if behind { "↑" } else { "✓" }),
            )
            .child(div().w(140).font_weight(600).font_size(14).text(name))
            .child(div().flex_1().font_size(13).color(palette::MUTED).text(status));
        if behind {
            r = r.child(
                div()
                    .layer(format!("btn Update {name}"))
                    .on_click(act)
                    .pad(5, 12)
                    .bg(palette::SURFACE_ALT)
                    .border(palette::LINE)
                    .font_weight(600)
                    .font_size(12)
                    .text("Update"),
            );
        }
        r
    };

    let weft_box = div()
        .self_stretch()
        .flex_col()
        .pad(2, 0)
        .items_start()
        .bg(palette::SURFACE)
        .border(palette::LINE)
        .child(core_row("Weft", weft_behind, &weft_status, "setup_run:weft", false))
        .child(core_row("RingFrame", rf_behind, &rf_status, "setup_run:ringframe", false))
        .child(core_row("Sense", sense_behind, &sense_status, "setup_run:sense", true));

    let mut agents_box = div()
        .self_stretch()
        .flex_col()
        .pad(2, 0)
        .items_start()
        .bg(palette::SURFACE)
        .border(palette::LINE);

    let rows_source: Vec<(String, String, SetupState)> = if !view.rows.is_empty() {
        view.rows.iter().map(|r| (r.name.clone(), r.title.clone(), r.state.clone())).collect()
    } else {
        props
            .harnesses
            .0
            .iter()
            .map(|h| (h.name.clone(), h.title.clone(), SetupState::Ready { version: None }))
            .collect()
    };

    let total_rows = rows_source.len();
    for (idx, (h_name, h_title, state)) in rows_source.into_iter().enumerate() {
        let is_last = idx + 1 == total_rows;
        let (mark, mark_color, plain_status, btn) = match state {
            SetupState::Ready { version } => {
                let ver_label = version
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|v| if v.starts_with('v') { v.to_string() } else { format!("v{v}") })
                    .unwrap_or_else(|| rf_ver.clone());
                ("✓", palette::TEAL, format!("{ver_label} · Ready"), None)
            }
            SetupState::Behind { have, latest } => (
                "↑",
                palette::BLUE_BORDER,
                format!(
                    "v{} → v{} · Update available",
                    have.trim_start_matches('v'),
                    latest.trim_start_matches('v')
                ),
                Some(("Update", format!("setup_run:{h_name}"))),
            ),
            SetupState::NotSetUp | SetupState::Failed { .. } => (
                "↑",
                palette::BLUE_BORDER,
                "Set up available".to_string(),
                Some(("Set up", format!("setup_run:{h_name}"))),
            ),
            SetupState::NotFound => (
                "✗",
                palette::MUTED,
                "Not installed".to_string(),
                Some(("Find app…", format!("locate_harness:{h_name}"))),
            ),
            _ => ("✓", palette::TEAL, format!("{rf_ver} · Ready"), None),
        };

        let mut row = div()
            .layer(format!("update row {h_title}"))
            .self_stretch()
            .flex_row()
            .gap(12)
            .pad(10, 16)
            .items_center();
        if !is_last {
            row = row.border_bottom(palette::LINE);
        }
        row = row
            .child(
                div()
                    .font_weight(700)
                    .font_size(14)
                    .color(mark_color)
                    .w(16)
                    .style("text-align", "center")
                    .text(mark),
            )
            .child(div().w(140).font_weight(600).font_size(14).text(h_title.clone()))
            .child(div().flex_1().font_size(13).color(palette::MUTED).text(plain_status));
        if let Some((btn_label, btn_act)) = btn {
            row = row.child(
                div()
                    .layer(format!("btn {btn_label} {h_name}"))
                    .on_click(btn_act)
                    .pad(5, 12)
                    .bg(palette::SURFACE_ALT)
                    .border(palette::LINE)
                    .font_weight(600)
                    .font_size(12)
                    .text(btn_label),
            );
        }
        agents_box = agents_box.child(row);
    }

    div()
        .layer("sheet Updates")
        .flex_1()
        .self_stretch()
        .flex_col()
        .style("min-height", "0")
        .items_start()
        .child(
            div()
                .self_stretch()
                .flex_row()
                .pad(12, 20)
                .items_center()
                .border_bottom(palette::LINE)
                .child(div().flex_1().font_weight(600).font_size(16).text("Updates"))
                .child(
                    div()
                        .font_size(13)
                        .color(palette::BLUE_BORDER)
                        .text(header_status),
                ),
        )
        .child(
            div()
                .self_stretch()
                .flex_1()
                .flex_col()
                .style("min-height", "0")
                .style("overflow-y", "auto")
                .gap(12)
                .pad(18, 20)
                .items_start()
                .child(
                    div()
                        .font_mono()
                        .font_weight(600)
                        .font_size(11)
                        .style("letter-spacing", "0.08em")
                        .color(palette::DIM)
                        .text("WEFT"),
                )
                .child(weft_box)
                .child(
                    div()
                        .font_mono()
                        .font_weight(600)
                        .font_size(11)
                        .style("letter-spacing", "0.08em")
                        .color(palette::DIM)
                        .style("padding-top", "4px")
                        .text("AGENTS"),
                )
                .child(
                    div()
                        .self_stretch()
                        .font_size(13)
                        .style("line-height", "19px")
                        .color(palette::MUTED)
                        .text("Sign in to each agent first. Weft sets up its plugin without touching your settings."),
                )
                .child(agents_box),
        )
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(10, 20)
                .items_center()
                .border_top(palette::LINE)
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Close")
                        .on_click("close_sheet")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.close")),
                )
                .child(
                    div()
                        .layer("btn Update all")
                        .on_click("setup_update_all")
                        .pad(9, 18)
                        .bg(palette::BLUE).border(palette::BLUE_BORDER)
                        .font_weight(600)
                        .font_size(14)
                        .color(palette::CANVAS)
                        .text("Update all"),
                ),
        )
}

/// Render the `D8.3` Remote sheet (connected devices, `Remove`, `Turn off`, `Close`).
fn render_remote_sheet(props: &SheetProps) -> Element {
    let remote = props.clients.iter().find_map(|c| c.remote.as_ref());
    let remote_on = remote
        .map(|v| {
            v.get("state").and_then(|s| s.as_str()) == Some("on")
                || v.get("on").and_then(|b| b.as_bool()) == Some(true)
        })
        .unwrap_or(false);
    let devices: Vec<(String, String, bool, String)> = remote
        .and_then(|v| v.get("devices"))
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .map(|item| {
                    let id = item.get("id").and_then(|s| s.as_str()).unwrap_or("dev-1").to_string();
                    let name =
                        item.get("name").and_then(|s| s.as_str()).unwrap_or("iPhone").to_string();
                    let online = item.get("online").and_then(|b| b.as_bool()).unwrap_or(true);
                    let status = item
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or(if online { "Connected now" } else { "Last seen recently" })
                        .to_string();
                    (id, name, online, status)
                })
                .collect()
        })
        .unwrap_or_default();

    let mut body = div()
        .self_stretch()
        .flex_1()
        .flex_col()
        .style("min-height", "0")
        .style("overflow-y", "auto")
        .gap(12)
        .pad(18, 20)
        .items_start();

    if let crate::app::BottomBarState::RemoveDeviceConfirm { device_id, device_name } =
        props.bottom_bar
    {
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
                        .text(format!("Remove \"{device_name}\"?")),
                )
                .child(
                    div()
                        .font_size(13)
                        .color(palette::MUTED)
                        .text(words::word("web.machines.remove_ask")),
                )
                .child(
                    div()
                        .self_stretch()
                        .flex_row()
                        .gap(10)
                        .items_center()
                        .child(div().flex_1())
                        .child(
                            div()
                                .layer("btn Cancel remove")
                                .on_click("cancel_bottom_bar")
                                .pad(7, 14)
                                .bg(palette::SURFACE)
                                .border(palette::LINE)
                                .font_weight(600)
                                .font_size(13)
                                .text(words::word("act.cancel")),
                        )
                        .child(
                            div()
                                .layer("btn Confirm remove")
                                .on_click(format!("remote_remove_device:{device_id}"))
                                .pad(7, 14)
                                .bg(palette::SURFACE)
                                .border(palette::AMBER)
                                .font_weight(600)
                                .font_size(13)
                                .color(palette::AMBER)
                                .text(words::word("web.machines.remove")),
                        ),
                ),
        );
    }

    if devices.is_empty() {
        body = body.child(
            div()
                .self_stretch()
                .pad(16, 18)
                .bg(palette::SURFACE)
                .border(palette::LINE)
                .font_size(13)
                .color(palette::MUTED)
                .text(words::word("web.machines.none")),
        );
    } else {
        let mut devices_box =
            div().self_stretch().flex_col().bg(palette::SURFACE).border(palette::LINE);

        let total = devices.len();
        for (idx, (dev_id, dev_name, online, status)) in devices.into_iter().enumerate() {
            let mut r = div()
                .layer(format!("device row {dev_name}"))
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(12, 16)
                .items_center();
            if idx + 1 < total {
                r = r.border_bottom(palette::LINE);
            }
            r = r
                .child(
                    div()
                        .font_size(12)
                        .color(if online { palette::TEAL } else { palette::DIM })
                        .text(if online { "●" } else { "□" }),
                )
                .child(div().flex_1().font_weight(600).font_size(14).text(dev_name.clone()))
                .child(div().font_size(13).color(palette::MUTED).text(status))
                .child(
                    div()
                        .layer(format!("btn Remove {dev_id}"))
                        .on_click(format!("request_remove_device:{dev_id}:{dev_name}"))
                        .pad(5, 12)
                        .bg(palette::SURFACE_ALT)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(12)
                        .color(palette::MUTED)
                        .text(words::word("web.machines.remove")),
                );
            devices_box = devices_box.child(r);
        }
        body = body.child(devices_box);
    }

    div()
        .layer("sheet Remote")
        .flex_1()
        .self_stretch()
        .flex_col()
        .style("min-height", "0")
        .items_start()
        .child(
            div()
                .self_stretch()
                .flex_row()
                .pad(12, 20)
                .items_center()
                .border_bottom(palette::LINE)
                .child(div().flex_1().font_weight(600).font_size(16).text("Remote"))
                .child(
                    div()
                        .font_size(13)
                        .color(if remote_on { palette::TEAL } else { palette::MUTED })
                        .text(if remote_on { "● On" } else { "Off" }),
                ),
        )
        .child(body)
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(10, 20)
                .items_center()
                .border_top(palette::LINE)
                .child(
                    div()
                        .layer("btn Turn off")
                        .on_click("remote_turn_off")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .color(palette::MUTED)
                        .text("Turn off"),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Close")
                        .on_click("close_sheet")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.close")),
                ),
        )
}
