//! The sidebar: each project, its AGENTS and its WORK (T11.1).

use super::*;

/// One row per unit of work: what you asked for, and where it stands. The
/// selected row expands in place.
pub(super) fn work_list(app: &mut App, area: Rect, beside_agent: bool) -> Paragraph<'static> {
    let th = app.theme;
    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    // What the arrows have to keep the selection inside, now that there is no
    // wheel. One line goes to the "N above" marker when the list is scrolled.
    app.note_list_rows(area.height.saturating_sub(1) as usize);

    // An agent is on the list the moment it is opened, so the empty list is
    // only for a project with no agent and nothing asked. With more projects
    // open, the others stay listed and this one says it under its heading.
    let empty = app.units().is_empty() && app.pane_count() == 0 && !app.has_sessions();
    if empty && app.project_count() == 1 {
        lines.push(Line::raw(""));
        // RingFrame is not set up here: what it still needs, under the project.
        if let Some(needs) = app.needs() {
            lines.push(Line::styled(format!(" ▾ {}", app.project_name()), th.label()));
            lines.extend(
                checklist(&needs, width)
                    .into_iter()
                    .map(|l| Line::styled(format!(" {l}"), th.label())),
            );
            return Paragraph::new(lines);
        }
        lines.extend(nothing_here(app, width, beside_agent));
        return Paragraph::new(lines);
    }

    let all = app.rows();
    let offset = app.list_offset().min(all.len().saturating_sub(1));
    if offset > 0 {
        lines.push(Line::styled(format!("   ↑ {offset} above"), th.label()));
    }
    let room = area.height as usize;
    for (i, row) in all.iter().enumerate().skip(offset) {
        // The last line says what is below, when anything is.
        if lines.len() + 1 >= room && i + 1 < all.len() {
            lines.push(Line::styled(format!("   ↓ {} more", all.len() - i), th.label()));
            break;
        }
        let at = Rect { y: area.y + lines.len() as u16, height: 1, ..area };
        app.hits.push((at, Target::Row(i)));
        lines.push(sidebar_row(app, row, i == app.selected, width));
        if let crate::app::Row::Project { project, .. } = row
            && empty
            && app.project_count() > 1
            && app.is_focused(*project)
        {
            lines.extend(match app.needs() {
                Some(needs) => checklist(&needs, width)
                    .into_iter()
                    .map(|l| Line::styled(format!(" {l}"), th.label()))
                    .collect(),
                None => nothing_here(app, width, beside_agent),
            });
        }
    }
    // An agent open and nothing asked yet: say what comes next under it.
    if app.units().is_empty() && !(empty && app.project_count() > 1) {
        let next: Vec<String> = match (app.needs(), app.not_ready()) {
            (Some(needs), _) => checklist(&needs, width),
            (None, Some((name, state))) => {
                vec![state.say(&name).unwrap_or_default(), word("tree.written_down").into()]
            }
            (None, None) => vec!["[A]SK for something and Weft writes it down.".into()],
        };
        if lines.len() + 1 + next.len() <= room {
            lines.push(Line::raw(""));
            lines.extend(next.into_iter().map(|n| Line::styled(format!(" {n}"), th.label())));
        }
    }
    Paragraph::new(lines)
}

/// What a project with no agent and nothing asked says: what RingFrame
/// lacks, or that nothing is asked, and how to start an agent.
fn nothing_here(app: &App, _width: usize, beside_agent: bool) -> Vec<Line<'static>> {
    let th = app.theme;
    if let Some((name, state)) = app.not_ready() {
        return vec![
            Line::styled(format!(" {}", state.say(&name).unwrap_or_default()), th.label()),
            Line::raw(""),
            Line::styled(format!(" {}", word("tree.runs_either_way")), th.label()),
        ];
    }
    // When the agent area is beside the list, it says how to start one.
    let mut out = vec![Line::styled(format!(" {}", word("tree.nothing_asked")), th.label())];
    if !beside_agent {
        out.push(Line::raw(""));
        out.push(Line::styled(" [N] starts an agent to ask.", th.label()));
    }
    out
}

/// What RingFrame still needs before it is set up here, a line each under
/// its project, with how to make a line hold beside it, or under it where
/// the list is too narrow. Weft sets the project up as soon as they all hold.
fn checklist(needs: &weft_core::workspace::Needs, width: usize) -> Vec<String> {
    let lines = needs.lines();
    // Each fix starts in one column, three after the longest line that has one.
    let at =
        lines.iter().filter(|l| l.how.is_some()).map(|l| cells(l.say())).max().unwrap_or(0) + 3;
    let mut out = vec![format!("  {}", word("tree.not_set_up"))];
    for line in &lines {
        let (mark, said) = (line.mark, line.say());
        match line.how() {
            Some(how) if 5 + at + cells(how) <= width => {
                out.push(format!("  {mark} {}{how}", padded(said, at)))
            }
            Some(how) => out.extend([format!("  {mark} {said}"), format!("    {how}")]),
            None => out.push(format!("  {mark} {said}")),
        }
    }
    out.push(format!("  {}", word("tree.sets_it_up")));
    out
}

/// One row of the sidebar, as T11.1 draws it. `▾` and `▸` mark what folds;
/// `⌫` is the one gesture that removes anything, so it has a glyph of its own.
pub(super) fn sidebar_row(
    app: &App,
    row: &crate::app::Row,
    picked: bool,
    width: usize,
) -> Line<'static> {
    use crate::app::Row;
    use weft_core::sessions::State;
    let th = app.theme;
    let style = |muted: bool| {
        if picked {
            th.selected()
        } else if muted {
            th.label()
        } else {
            Style::default().fg(th.primary())
        }
    };
    // A row: what reads at its left, cut to make room for what reads at its
    // right edge, one cell in.
    let row_of = |left: String, right: String, muted: bool, right_style: Style| {
        let room = width.saturating_sub(cells(&right) + 1);
        Line::from(vec![
            Span::styled(padded(&clip(&left, room), room), style(muted)),
            Span::styled(format!("{right} "), if picked { th.selected() } else { right_style }),
        ])
    };
    let fold = |folded: bool| if folded { "▸" } else { "▾" };
    match row {
        Row::Project { name, folded, open, waiting, .. } => {
            let mut tail = format!("{open} {}", word("title.open"));
            if *waiting > 0 {
                tail.push_str(&format!(" · {waiting} ●"));
            }
            let head = format!(" {} {name}", fold(*folded));
            let room = width.saturating_sub(cells(&tail) + 4);
            Line::from(vec![
                Span::styled(padded(&clip(&head, room), room), style(false)),
                Span::styled(format!("{tail}  "), th.label()),
                Span::styled("⌫".to_string(), th.label()),
            ])
        }
        Row::Gap { .. } => Line::raw(""),
        Row::Agents { running, ended, .. } => {
            let counts =
                fill("tree.counts", &[("n", &running.to_string()), ("m", &ended.to_string())]);
            let room = width.saturating_sub(cells(&counts) + 1);
            Line::from(vec![
                Span::styled(
                    padded(&format!("  {}", word("tree.agents")), room),
                    th.label().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{counts} "), th.label()),
            ])
        }
        Row::Agent { agent, width: column, .. } => {
            let ended = agent.state != State::Running;
            let waiting = weft_core::turns::is_asking(agent.turn) && !ended;
            let time = weft_core::sessions::when(agent.at, app.now(), app.zone());
            let state = weft_core::tree::state_word(agent, &time);
            let right = if agent.state == State::Quiet { state } else { format!("{state} {time}") };
            let glyph = if ended { "○" } else { "●" };
            let title = agent.title.clone().unwrap_or_else(|| word("tree.no_ask").to_string());
            let head = format!("  {glyph} {} ", padded(&agent.harness, *column));
            // The title is cut with `…`, keeping two spaces before the state
            // and three after a cut one, as T11.1 draws it.
            let room = width.saturating_sub(cells(&head) + cells(&right) + 1);
            let title =
                if cells(&title) + 2 > room { clip(&title, room.saturating_sub(3)) } else { title };
            let state_style = if waiting { th.needs_you() } else { th.label() };
            Line::from(vec![
                Span::styled(head, if waiting && !picked { th.needs_you() } else { style(ended) }),
                Span::styled(padded(&title, room), style(ended)),
                Span::styled(format!("{right} "), if picked { th.selected() } else { state_style }),
            ])
        }
        Row::Earlier { n, .. } => {
            let said = if *n == 1 {
                word("tree.earlier_one").to_string()
            } else {
                fill("tree.earlier", &[("n", &n.to_string())])
            };
            row_of(format!("  ▸ {said}"), String::new(), true, th.label())
        }
        Row::Work { .. } => Line::styled(
            format!("  {}", word("tree.work")),
            th.label().add_modifier(Modifier::BOLD),
        ),
        Row::Open { since, count, folded, .. } => {
            let mut head = format!("  {} {}", fold(*folded), word("tree.open"));
            if let Some(n) = since {
                head.push_str(&format!(" · {}", fill("tree.since", &[("n", &n.to_string())])));
            }
            row_of(head, count.to_string(), false, th.label())
        }
        Row::Eval { group, .. } => {
            let mut head = format!(
                "  {} {}",
                fold(group.folded),
                fill("tree.eval", &[("n", &group.n.to_string())])
            );
            match (group.judging, group.verdict) {
                (true, _) | (_, None) => head.push_str(&format!(" · {}", word("tree.judging"))),
                (false, Some(v)) => {
                    head.push_str(&format!(" · {}", word(&format!("verdict.{v}"))));
                    if let Some(a) = &group.agreement {
                        head.push_str(&format!(" · {a}"));
                    }
                    head.push_str(&format!(" · {}", word("tree.not_sealed")));
                }
            }
            row_of(head, group.count.to_string(), false, th.label())
        }
        Row::Sealed { seals, asks, folded, .. } => {
            let n = if *seals == 1 {
                word("tree.seals_one").to_string()
            } else {
                fill("tree.seals", &[("n", &seals.to_string())])
            };
            let head = format!("  {} {} · {n}", fold(*folded), word("tree.sealed"));
            row_of(head, asks.to_string(), false, th.label())
        }
        Row::Seal { seal, .. } => {
            let mut head = format!("    Seal {} · {}", seal.n, seal.decision);
            if let Some(n) = seal.eval {
                head.push_str(&format!(" · {}", fill("tree.eval", &[("n", &n.to_string())])));
            }
            row_of(head, seal.asks.to_string(), false, th.label())
        }
        Row::Ask { ask, column, .. } => {
            // Only Open's Asks carry a word: a round says its Asks' state.
            let word = ask.word.map(|w| w.say().to_string()).unwrap_or_default();
            let room = width.saturating_sub(cells(&word) + 1 + 4);
            let harness = cells(&ask.harness);
            let title_room = (*column).min(room.saturating_sub(harness + 1));
            let title = padded(&clip(&ask.title, title_room.saturating_sub(2)), title_room);
            let left = format!("    {title}");
            let used = cells(&left);
            Line::from(vec![
                Span::styled(left, style(false)),
                Span::styled(
                    padded(&ask.harness, width.saturating_sub(used + cells(&word) + 1)),
                    if picked { th.selected() } else { th.label() },
                ),
                Span::styled(format!("{word} "), if picked { th.selected() } else { style(false) }),
            ])
        }
    }
}
