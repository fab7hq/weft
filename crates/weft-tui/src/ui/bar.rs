//! The action bar and the hint under it.

use super::*;

/// A refusal from the daemon, in words that say what to do about it.
pub(super) fn refused(code: &str) -> String {
    match code {
        "ModeNotEntered" => word("not_typed.mode_not_entered").into(),
        "NoProcess" => word("not_typed.no_process").into(),
        "InjectionInFlight" => word("not_typed.in_flight").into(),
        "ControlCharacters" => word("not_typed.control_characters").into(),
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
    match &app.modal {
        // `Enter` does what is picked everywhere; [H]ELP says so once, and no
        // bar spends its room on it.
        Some(Modal::Bar(bar)) => {
            let act = match bar {
                Bar::Send(_) => key_by("Enter", "act.send"),
                Bar::SendAnyway { .. } => key_by("Enter", "act.send_anyway"),
                Bar::Resume { .. } => key_by("Enter", "act.resume"),
                Bar::NothingToResume { .. } => "[N] NEW AGENT".into(),
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
                "off" => " [Enter] TURN ON   [ESC] CLOSE",
                "choose" => " [↑↓] PICK   [Enter] CHOOSE   [O] TURN OFF   [ESC] CLOSE",
                "not_installed" if offer.is_object() => {
                    " [I] INSTALL   [Enter] LOOK AGAIN   [O] TURN OFF   [ESC] CLOSE"
                }
                "not_installed" => " [Enter] LOOK AGAIN   [O] TURN OFF   [ESC] CLOSE",
                "installing" => " [ESC] CLOSE",
                "signing_in" => " [ESC] CANCEL",
                "signed_out" => " [Enter] SIGN IN   [O] TURN OFF   [ESC] CLOSE",
                "on" if devices => " [↑↓] PICK   [D] REMOVE   [O] TURN OFF   [ESC] CLOSE",
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
    if app.detail().is_some() {
        // `[P]ROCEED` is there only while the Ask's prompt is still to be
        // sent. Once it went, what happens next is the person's: the one
        // thing offered is a follow-up.
        let mut left = vec![Span::raw(" ")];
        let proceed = app.proceeds().then(|| (key_in("act.proceed"), Act::Proceed));
        let follow_up = (key_by("F", "act.follow_up"), Act::FollowUp);
        for (label, act) in proceed.into_iter().chain([follow_up]) {
            left.push(key(app, &label, act));
            left.push(Span::raw("   "));
        }
        if app.has_eval_view() {
            left.push(Span::styled(format!("{}   ", key_by("V", "act.eval_view")), th.label()));
        }
        left.push(Span::styled("[↑↓] SCROLL", th.label()));
        return Paragraph::new(spread(left, vec![Span::styled("[ESC] CLOSE ", th.label())], width));
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
        spans.push(key(app, &key_in(label), act));
    }
    let menu = key(app, "[W]EFT ⌄", Act::WeftMenu);
    Paragraph::new(spread(spans, vec![menu, Span::raw(" ")], width))
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
    if let Some(v) = app.newer.get().filter(|_| app.modal.is_none() && !app.waiting_here()) {
        let lit = Style::default().fg(th.accent()).add_modifier(Modifier::BOLD);
        return Paragraph::new(Line::styled(
            format!(" Weft {v} is out - update to use the latest version"),
            lit,
        ));
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
        (Some(Modal::Bar(Bar::Resume { .. } | Bar::NothingToResume { .. })), _) => {
            format!(" {}", word("resume.either_way"))
        }
        (Some(Modal::RingFrame), _) => {
            " Weft never changes an agent without asking you first.".into()
        }
        (Some(Modal::Remote), _) => {
            " Fab7 says who you are; only this machine says which devices get in.".into()
        }
        (Some(Modal::Allow { .. }), _) => " A device gets in only after you say yes here.".into(),
        (Some(_), _) => String::new(),
        (None, Focus::Agent) => " Every other key goes to the agent, Esc included.".into(),
        (None, Focus::Weft) if app.waiting_here() => format!(
            " {} is asking you something. Weft never answers for you.",
            app.harness_at(app.pane_focus).unwrap_or("the agent")
        ),
        (None, Focus::Weft) if app.detail().is_some() => String::new(),
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
        (None, Focus::Weft) => " [H]ELP lists every key.".into(),
    };
    Paragraph::new(Line::styled(text, th.label()))
}
