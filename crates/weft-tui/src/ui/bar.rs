//! The action bar and the hint under it.

use super::*;

/// A refusal from the daemon, in words that say what to do about it.
pub(super) fn refused(code: &str) -> String {
    match code {
        "ModeNotEntered" => word("not_typed.mode_not_entered").into(),
        "NoProcess" => word("not_typed.no_process").into(),
        "InjectionInFlight" => word("not_typed.in_flight").into(),
        "ControlCharacters" => word("not_typed.control_characters").into(),
        "PasteNotShown" => word("not_typed.paste_not_shown").into(),
        "NotTheAsk" => word("not_typed.not_the_ask").into(),
        "NoReceipt" => word("not_typed.no_receipt").into(),
        other => fill("not_typed.why", &[("why", other.trim_end_matches('.'))]),
    }
}

/// Every action shows its key. What cannot be done now is drawn muted and
/// stays where it was, so the shape of the bar never jumps.
pub(super) fn action_bar(app: &App, width: u16) -> Paragraph<'static> {
    let th = app.theme;
    let plain = |text: &str| Paragraph::new(Line::styled(text.to_string(), th.label()));
    if app.focus == Focus::Agent {
        // One key is Weft's in the agent: the way back.
        return plain(&format!("   {}  BACK TO WEFT", app.toggle.label()));
    }
    if app.layout_bar && app.modal.is_none() {
        let lit = Style::default().fg(th.primary()).add_modifier(Modifier::BOLD);
        let keys = format!(
            "   {}   {}    [ ]  {}   {}   {}   {}",
            key_by("→", "layout.right"),
            key_by("↓", "layout.down"),
            word("layout.move_tab").to_uppercase(),
            key_by("TAB", "layout.next_pane"),
            key_in("layout.join"),
            key_by("ESC", "layout.done"),
        );
        let title = format!(" {}", word("act.layout").to_uppercase());
        return Paragraph::new(Line::from(vec![
            Span::styled(title, lit),
            Span::styled(keys, Style::default().fg(th.primary())),
        ]));
    }
    match &app.modal {
        // `Enter` does what is picked everywhere; [H]ELP says so once, and no
        // bar spends its room on it.
        Some(Modal::Bar(bar)) => {
            let act = match bar {
                Bar::Send(_) => key_by("Enter", "act.send"),
                Bar::SendAnyway { .. } => key_by("Enter", "act.send_anyway"),
            };
            return plain(&format!(" {}   {act}", key_by("←", "act.cancel")));
        }
        Some(Modal::Quit) => return plain(" [↑↓] PICK   [←] CANCEL"),
        Some(Modal::Weft) => return plain(" [↑↓] PICK   [ESC] CLOSE"),
        Some(Modal::CloseProject { .. }) => {
            return plain(" [↑↓] PICK   [←] CANCEL");
        }
        Some(Modal::OpenProject { .. }) => {
            return plain(&format!(" {}", key_by("←", "act.cancel")));
        }
        Some(Modal::StartAgent { .. }) => return plain(" [↑↓] PICK   [←] CANCEL"),
        Some(Modal::Ask { .. }) => {
            let (to, ask) = (word("ask.send_to"), key_by("Enter", "act.ask"));
            return plain(&format!(" [↑↓] {to}   {ask}   {}", key_by("←", "act.cancel")));
        }
        Some(Modal::RingFrame) => {
            // Whether [P]ROCEED has anything to run: nothing is offered that
            // would do nothing.
            let view = app.setup_view();
            let mut keys = " [↑↓] PICK".to_string();
            if view.is_some_and(|v| v.proceeds()) {
                keys.push_str("   [P]ROCEED ALL");
            }
            keys.push_str("   [ESC] CLOSE");
            return plain(&keys);
        }
        Some(Modal::Locate { .. }) => return plain(" [←] BACK"),
        Some(Modal::Remote) => {
            let state = app.remote_view().and_then(|v| v["state"].as_str()).unwrap_or("off");
            let devices = !app.remote_devices().is_empty();
            let offer = app.remote_view().map(|v| v["offer"].clone()).unwrap_or_default();
            let keys = match state {
                _ if app.remote_run => " [Y] RUN   [N] NO",
                _ if app.remote_removing.is_some() => " [Y] PROCEED   [N] CANCEL",
                "off" => " [Enter] TURN ON   [ESC] CLOSE",
                "choose" => " [↑↓] PICK   [Enter] CHOOSE   [O] TURN OFF   [ESC] CLOSE",
                "not_installed" if offer.is_object() => {
                    " [I] INSTALL   [Enter] LOOK AGAIN   [O] TURN OFF   [ESC] CLOSE"
                }
                "not_installed" => " [Enter] LOOK AGAIN   [O] TURN OFF   [ESC] CLOSE",
                "installing" => " [ESC] CLOSE",
                "signing_in" => " [ESC] CANCEL",
                "signed_out" => " [Enter] SIGN IN   [O] TURN OFF   [ESC] CLOSE",
                // Remote stops at a restart while Start at login is off (T13.3).
                "on" if devices && !app.start_at_login() => {
                    " [↑↓] PICK   [D] REMOVE   [L] START AT LOGIN   [O] TURN OFF   [ESC] CLOSE"
                }
                "on" if devices => " [↑↓] PICK   [D] REMOVE   [O] TURN OFF   [ESC] CLOSE",
                "on" if !app.start_at_login() => " [L] START AT LOGIN   [O] TURN OFF   [ESC] CLOSE",
                _ => " [O] TURN OFF   [ESC] CLOSE",
            };
            return plain(keys);
        }
        Some(Modal::Allow { .. }) => return plain(" [Y] ALLOW   [N] REFUSE"),
        Some(Modal::Help) | Some(Modal::Note(_)) => return plain(" [ESC] CLOSE"),
        None => {}
    }
    if app.waiting_here() {
        return plain(" [Y] HOW WEFT KNOWS");
    }
    if let Some(e) = app.eval_screen() {
        let keys = match e.top() {
            crate::app::Layer::Hunk { .. } => " [↑↓] SCROLL".to_string(),
            crate::app::Layer::Step { .. } => " [↑↓] MOVE   [D] THE CHANGE".to_string(),
            crate::app::Layer::List { .. } => format!(
                " [↑↓] MOVE   [D] THE CHANGE   [TAB] {}",
                word(if e.mode == weft_core::eval_view::Mode::Steps {
                    "eval_view.by_file"
                } else {
                    "eval_view.by_step"
                })
                .to_uppercase()
            ),
        };
        let back = if e.layers.len() > 1 { "[ESC] BACK " } else { "[ESC] CLOSE " };
        return Paragraph::new(spread(
            vec![Span::styled(keys, th.label())],
            vec![Span::styled(back, th.label())],
            width,
        ));
    }
    if let Some(d) = app.detail() {
        use crate::app::View;
        use weft_core::sessions::State;
        let lit = |text: String| Span::styled(text, Style::default().fg(th.primary()));
        let mut keys: Vec<Span<'static>> = vec![Span::raw(" ")];
        let mut add = |span: Span<'static>| {
            keys.push(span);
            keys.push(Span::raw("   "));
        };
        match &d.view {
            // `[P]ROCEED` stays drawn (T11.2): once the prompt went, pressing
            // it says there is nothing to send, and `[F]` asks for more.
            View::Work(_) => {
                add(lit(key_in("act.proceed")));
                add(lit(key_by("F", "act.follow_up")));
            }
            View::Eval(_) => {
                add(lit(key_in("act.seal")));
                add(lit(key_by("V", "act.eval_view")));
                add(lit("[Enter] OPEN".into()));
            }
            View::Seal(_) => add(lit("[Enter] OPEN".into())),
            View::Session(id) => match app.session_row(id) {
                Some(r) if r.state == State::Running && !app.joins(r) => {
                    add(lit("[Enter] GO TO ITS PANE".into()))
                }
                _ => {
                    add(lit("[R]ESUME THIS".into()));
                    add(lit("[Enter] OPEN".into()));
                }
            },
        }
        keys.push(Span::styled("[↑↓] SCROLL", th.label()));
        return Paragraph::new(spread(keys, vec![Span::styled("[ESC] CLOSE ", th.label())], width));
    }

    // A search narrows the tree: its keys are the whole bar.
    if app.tree_search().is_some() {
        return plain(" [Enter] OPEN   [↑↓] PICK   [ESC] CLEAR");
    }

    // RingFrame's three acts, in the order they happen. Weft's own operations
    // are in the `[W]` menu, which is not an act on the record.
    let mut spans = vec![Span::raw(" ")];
    for (i, (label, act)) in
        [("act.ask", Act::Ask), ("act.eval", Act::Eval), ("act.seal", Act::Seal)]
            .into_iter()
            .enumerate()
    {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        // Drawn the same always: one that does not apply to
        // the row selected says why when pressed.
        let _ = act;
        spans.push(Span::styled(key_in(label), Style::default().fg(th.primary())));
    }
    let menu = key(app, "[W]EFT ⌄", Act::WeftMenu);
    let search = Span::styled("[/] SEARCH", Style::default().fg(th.primary()));
    let lit = Style::default().fg(th.primary());
    let split = Span::styled(
        format!("[\\] {}", word("act.layout").to_uppercase()),
        if app.panes().count() > 1 { lit.add_modifier(Modifier::BOLD) } else { lit },
    );
    let right = vec![search, Span::raw("   "), split, Span::raw("   "), menu, Span::raw(" ")];
    Paragraph::new(spread(spans, right, width))
}

/// One action on the bar, dimmed when it cannot be used.
pub(super) fn key(app: &App, label: &str, act: Act) -> Span<'static> {
    let th = app.theme;
    // Active in the terminal's own foreground, inactive in grey — the way a
    // menu reads. It was the other way round, and everything looked off.
    let style = match app.unavailable(act) {
        Some(_) => th.label(),
        None => Style::default().fg(th.primary()),
    };
    Span::styled(label.to_string(), style)
}

pub(super) fn hint(app: &App) -> Paragraph<'static> {
    let th = app.theme;
    // The search being typed stands where the hint does.
    if let Some(q) = app.tree_search().filter(|_| app.modal.is_none() && app.detail().is_none()) {
        let lit = Style::default().fg(th.accent()).add_modifier(Modifier::BOLD);
        return Paragraph::new(Line::styled(format!(" / {q}▌"), lit));
    }
    if let Some(said) = app.hint_text() {
        return Paragraph::new(Line::styled(format!(" {said}"), th.needs_you()));
    }
    // What is sitting unsent in their agent. The daemon has always reported
    // this and nothing drew it, so a refused injection was silence.
    if let Some(refusal) = app.last_refusal() {
        return Paragraph::new(Line::styled(format!(" {}", refused(&refusal)), th.needs_you()));
    }
    if let Some(why) = app.unrecorded() {
        let said = format!(" Sent, but RingFrame did not record it ({why}). It has gone once.");
        return Paragraph::new(Line::styled(said, th.needs_you()));
    }
    let quiet = app.modal.is_none() && !app.waiting_here();
    if let Some(v) = app.update_waiting().filter(|_| quiet) {
        return Paragraph::new(Line::styled(
            format!(
                " Weft {v} is installed - it starts when no agent is running ({} running)",
                app.running_everywhere()
            ),
            th.needs_you(),
        ));
    }
    let out = app.setup_view().filter(|v| v.weft_behind).and_then(|v| v.weft_latest.as_deref());
    if let Some(v) = out.filter(|_| quiet) {
        let lit = Style::default().fg(th.accent()).add_modifier(Modifier::BOLD);
        return Paragraph::new(Line::styled(format!(" Weft {v} is out, run update"), lit));
    }
    let text = match (&app.modal, app.focus) {
        (Some(Modal::Quit), _) => {
            " Nothing you asked for is lost either way. It is all written down.".into()
        }
        (Some(Modal::Bar(Bar::Send(_))), _) => {
            " Weft never types into an agent without asking you first.".into()
        }
        (Some(Modal::Bar(Bar::SendAnyway { .. })), _) => String::new(),
        (Some(Modal::Ask { .. }), _) => {
            format!(" {}", word("ask.which_approach"))
        }
        (Some(Modal::StartAgent { .. }), _) => " It runs here, with your own settings.".into(),
        (Some(Modal::RingFrame), _) => {
            " Weft never changes an agent without asking you first.".into()
        }
        (Some(Modal::Weft), _) if app.start_at_login_picked() => {
            " Start Weft when you log in, and again if it stops. Weft never turns this on by itself."
                .into()
        }
        (Some(Modal::Remote), _) => {
            " Fab7 says who you are; only this machine says which devices get in.".into()
        }
        (Some(Modal::Allow { .. }), _) => " A device gets in only after you say yes here.".into(),
        (Some(_), _) => String::new(),
        (None, _) if app.drag.is_some_and(|d| d.moved) => format!(" {}", word("hint.drag")),
        (None, Focus::Agent) => " Every other key goes to the agent, Esc included.".into(),
        (None, Focus::Weft) if app.layout_bar => {
            let tab = app.tabs().into_iter().find(|t| t.active);
            let name = tab.map(|t| format!("{} · {}", t.project_name, t.harness));
            format!(" {}", fill("hint.layout", &[("tab", &name.unwrap_or_default())]))
        }
        (None, Focus::Weft) if app.waiting_here() => format!(
            " {} is asking you something. Weft never answers for you.",
            app.harness_at(app.pane_focus).unwrap_or("the agent")
        ),
        (None, Focus::Weft) if app.detail().is_some() => String::new(),
        (None, Focus::Weft) if app.panes().count() > 1 => format!(" {}", word("hint.split")),
        (None, Focus::Weft) if app.not_ready().is_some() => {
            let (name, state) = app.not_ready().expect("not ready");
            let mut said = state.say(&name).unwrap_or_default();
            said.push_str(if state.can_be_set_up() {
                " [U]PDATE sets it up."
            } else {
                " Your agent still runs here."
            });
            format!(" {said}")
        }
        (None, Focus::Weft) if !app.show_work() => {
            " The list is hidden. [B] brings it back.".into()
        }
        // Tabs from more than one project: say that the keys reach them all.
        (None, Focus::Weft) if app.tabs().windows(2).any(|t| t[0].project != t[1].project) => {
            format!(" {}", word("hint.tabs"))
        }
        (None, Focus::Weft) => " [H]ELP lists every key.".into(),
    };
    Paragraph::new(Line::styled(text, th.label()))
}
