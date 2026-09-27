//! The title bar and the agents' tabs.

use super::*;

/// The title bar, and the columns its turbo switch covers, for a click.
pub(super) fn title_bar(app: &App, width: u16) -> (Paragraph<'static>, Option<(u16, u16)>) {
    let th = app.theme;
    // Where you are is shown by what is lit, not by a word: WEFT in the accent
    // means the keys are Weft's, and the agent's own name in the accent means
    // they are the agent's.
    let here = app.focus == Focus::Weft || app.pane_count() == 0;
    let left = vec![
        Span::raw(" "),
        Span::styled("╸", Style::default().fg(th.accent())),
        Span::styled("┃", Style::default().fg(th.warp())),
        Span::styled("╺ ", Style::default().fg(th.accent())),
        Span::styled(
            "WEFT".to_string(),
            if here {
                Style::default().fg(th.accent()).add_modifier(Modifier::BOLD)
            } else {
                th.label()
            },
        ),
        // No path: with more than one project the name is wrong the moment
        // focus moves, and the sidebar names them where they can be acted on.
    ];
    let lit = Style::default().fg(th.accent()).add_modifier(Modifier::BOLD);
    let mut right = Vec::new();
    if app.sync_view().is_some_and(|v| v.needs_anything()) {
        right.push(Span::styled("↑ [U]PDATE   ", lit));
    }
    // The turbo switch: always there, because it matters most before the
    // first agent starts. It decides how the next agents start.
    let switch = right.len();
    right.push(Span::styled(
        if app.turbo() { "⚡ [T]URBO ON" } else { "[T]URBO OFF" },
        if app.turbo() { lit } else { th.label() },
    ));
    right.push(Span::raw("   "));
    right.extend(match (app.pane_count(), app.focus) {
        (0, _) => vec![Span::styled("NO AGENT RUNNING ", th.label())],
        // The counts keep their place whichever surface has the keys: they
        // are facts about the work, not about where you are.
        (_, _) => {
            let mut spans =
                vec![Span::styled(pair("open", &app.open_count().to_string()), th.label())];
            if app.needs_you() > 0 {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(
                    pair("needs you", &app.needs_you().to_string()),
                    th.needs_you(),
                ));
            }
            spans.push(Span::raw(" "));
            spans
        }
    });
    // Where the switch is, read off the line as it is drawn: what comes
    // before it on the left, the gap, and on the right.
    let at = left.len() + 1 + switch;
    let line = spread(left, right, width);
    let from = line.spans[..at].iter().map(Span::width).sum::<usize>() as u16;
    let to = from + line.spans[at].width() as u16;
    (Paragraph::new(line), (to <= width).then_some((from, to)))
}

/// Agents are tabs over the pane. The focused surface's header is the accent.
/// The tab row, and the columns each agent's tab covers, for a click.
pub(super) fn tab_row(app: &App, geo: &Geo) -> (Line<'static>, Vec<(u16, u16, usize)>) {
    let th = app.theme;
    let in_weft = app.focus == Focus::Weft;
    let mut spans: Vec<Span> = Vec::new();
    let mut col = 0u16;
    let push = |spans: &mut Vec<Span<'static>>, col: &mut u16, text: String, style: Style| {
        *col += cells(&text) as u16;
        spans.push(Span::styled(text, style));
    };

    // No header word: the list is directly below, and where the keys go is
    // shown by what is lit in the title bar.
    match (app.show_work(), geo.divider) {
        (_, Some(at)) => {
            push(&mut spans, &mut col, " ".repeat(at as usize), th.label());
            push(&mut spans, &mut col, "│ ".into(), th.rule());
        }
        _ => push(&mut spans, &mut col, " ".into(), th.label()),
    }

    // The detail view replaces the tabs with the unit it is about.
    if let Some(d) = app.detail() {
        let width = geo.tabs.width.saturating_sub(col);
        let back = format!("{} ", d.harness);
        let title = clip(&d.title, width.saturating_sub(cells(&back) as u16 + 1) as usize);
        push(
            &mut spans,
            &mut col,
            padded(&title, (width as usize).saturating_sub(cells(&back))),
            th.title(),
        );
        push(&mut spans, &mut col, back, th.label());
        return (Line::from(spans), Vec::new());
    }
    let mut tabs = Vec::new();

    for i in 0..app.pane_count() {
        let active = i == app.pane_focus;
        let waiting = app.waiting(i);
        let harness = app.harness_at(i).unwrap_or("agent").to_string();
        let ready = app.readiness(&harness).is_ready();
        // ⚡: started in turbo mode, every permission granted.
        let turbo = app.pane_turbo(i);
        let text = format!(
            "{}{} {}{}{}{}",
            if active { "▸ " } else { "" },
            i + 1,
            harness,
            if turbo { " ⚡" } else { "" },
            if waiting { " ●" } else { "" },
            if ready { "" } else { " ⚠" }
        );
        let style = match (active, in_weft, waiting) {
            // In the agent the active tab carries the accent; in Weft the
            // `WORK` header does, so the tab steps back.
            (true, false, _) => Style::default().fg(th.accent()).add_modifier(Modifier::BOLD),
            (true, true, _) => th.selected(),
            (_, _, true) => th.needs_you(),
            _ => th.label(),
        };
        let from = geo.tabs.x + col;
        push(&mut spans, &mut col, text, style);
        tabs.push((from, geo.tabs.x + col, i));
        push(&mut spans, &mut col, "    ".into(), th.label());
    }
    push(&mut spans, &mut col, "+".into(), th.label());
    (Line::from(spans), tabs)
}
