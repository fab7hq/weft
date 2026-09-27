//! The action bar and the hint under it.

use super::*;

/// A refusal from the daemon, in words that say what to do about it.
pub(super) fn refused(code: &str) -> String {
    match code {
        "ModeNotEntered" => {
            "Weft did not type it: the agent never showed the command as on.".into()
        }
        "NoProcess" => "Weft did not type it: that agent is not running.".into(),
        "InjectionInFlight" => "Weft did not type it: it is still typing the last one.".into(),
        other => format!("Weft did not type it: {other}"),
    }
}

/// Every action shows its key. What cannot be done now is drawn muted and
/// stays where it was, so the shape of the bar never jumps.
pub(super) fn action_bar(app: &App, width: u16) -> Paragraph<'static> {
    let th = app.theme;
    let plain = |text: &str| Paragraph::new(Line::styled(text.to_string(), th.label()));
    if app.focus == Focus::Agent {
        // One key is Weft's in the agent: the way back.
        return plain(&format!("  {}  BACK TO WEFT", app.toggle.label()));
    }
    match &app.modal {
        Some(Modal::Confirm(_)) => return plain("  [Enter] DO IT   [←] CANCEL"),
        Some(Modal::Quit) => return plain("  [↑↓] PICK   [Enter] DO IT   [←] CANCEL"),
        Some(Modal::Weft) => return plain("  [↑↓] PICK   [Enter] DO IT   [ESC] CLOSE"),
        Some(Modal::CloseProject { .. }) => {
            return plain("  [↑↓] PICK   [Enter] DO IT   [←] CANCEL");
        }
        Some(Modal::OpenProject { .. }) => return plain("  [Enter] OPEN   [←] CANCEL"),
        Some(Modal::SendAnyway { .. }) => {
            return plain("  [↑↓] PICK   [Enter] DO IT   [←] CANCEL");
        }
        Some(Modal::StartAgent { .. }) => return plain("  [↑↓] PICK   [Enter] START   [←] CANCEL"),
        Some(Modal::RingFrame) => {
            return match app.sync_view() {
                Some(v) if v.needs_anything() && !v.running => plain("  [P]ROCEED   [ESC] CLOSE"),
                _ => plain("  [ESC] CLOSE"),
            };
        }
        Some(Modal::Ask { .. }) => return plain("  [Enter] SEND   [←] CANCEL"),
        Some(Modal::PickUp { .. }) => return plain("  [↑↓] PICK   [Enter] DO IT   [←] CANCEL"),
        Some(Modal::Help) | Some(Modal::Note(_)) => return plain("  [ESC] CLOSE"),
        None => {}
    }
    if app.waiting_here() {
        return plain("  [Enter] ANSWER IT   [Y] HOW WEFT KNOWS");
    }
    if app.detail().is_some() {
        // `[P]ROCEED` is there only while the Ask's prompt is still to be
        // sent. Once it went, what happens next is the person's: the one
        // thing offered is a follow-up.
        let mut left = vec![Span::raw("  ")];
        let proceed = app.proceeds().then_some(("[P]ROCEED", Act::Proceed));
        for (label, act) in proceed.into_iter().chain([("[F] FOLLOW UP", Act::FollowUp)]) {
            left.push(key(app, label, act));
            left.push(Span::raw("   "));
        }
        left.push(Span::styled("[↑↓] SCROLL", th.label()));
        return Paragraph::new(spread(left, vec![Span::styled("[ESC] CLOSE ", th.label())], width));
    }

    // RingFrame's three acts, in the order they happen. Weft's own operations
    // are in the `[W]` menu, which is not an act on the record.
    let mut spans = vec![Span::raw("  ")];
    for (i, (label, act)) in
        [("[A]SK", Act::Ask), ("[E]VAL", Act::Eval), ("[S]EAL", Act::Seal)].into_iter().enumerate()
    {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(key(app, label, act));
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
        return Paragraph::new(Line::styled(format!(" Weft {v} is out - run weft update"), lit));
    }
    let text = match (&app.modal, app.focus) {
        (Some(Modal::Quit), _) => {
            " Nothing you asked for is lost either way. It is all written down.".into()
        }
        (Some(Modal::Confirm(_)), _) => {
            " Weft never types into an agent without asking you first.".into()
        }
        (Some(Modal::Ask { .. }), _) => {
            " The agent will ask you which approach to take, in its own pane.".into()
        }
        (Some(Modal::StartAgent { .. }), _) => " It runs here, with your own settings.".into(),
        (Some(Modal::PickUp { .. }), _) => " Your work is in the record either way.".into(),
        (Some(Modal::RingFrame), _) => {
            " Weft never changes an agent without asking you first.".into()
        }
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
