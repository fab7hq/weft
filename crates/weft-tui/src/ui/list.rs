//! The work list: one row per project, harness and unit of work.

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
    // only for a project with no agent and nothing asked.
    if app.units().is_empty() && app.pane_count() == 0 {
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
        if let Some((name, state)) = app.not_ready() {
            lines.push(Line::styled(
                format!(" {}", state.say(&name).unwrap_or_default()),
                th.label(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(format!(" {}", word("tree.runs_either_way")), th.label()));
            return Paragraph::new(lines);
        }
        // When the agent area is beside the list, it says how to start one.
        let next = if beside_agent { "" } else { "[N] starts an agent to ask." };
        lines.push(Line::styled(format!(" {}", word("tree.nothing_asked")), th.label()));
        if !next.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(format!(" {next}"), th.label()));
        }
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
    }
    // An agent open and nothing asked yet: say what comes next under it.
    if app.units().is_empty() {
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

/// One row of the sidebar. `▾` and `▸` mark the levels that hold something;
/// an action is a leaf and carries no marker. `⌫` is the one gesture that
/// removes anything, so it is the one thing with a glyph of its own.
pub(super) fn sidebar_row(
    app: &App,
    row: &crate::app::Row,
    picked: bool,
    width: usize,
) -> Line<'static> {
    use crate::app::Row;
    let th = app.theme;
    let pick = |s: String| {
        if picked { Span::styled(s, th.selected()) } else { Span::styled(s, th.label()) }
    };
    match row {
        Row::Project { name, folded, open, waiting, .. } => {
            let mut tail = format!("{open} {}", word("title.open"));
            if *waiting > 0 {
                tail.push_str(&format!(" · {waiting} ●"));
            }
            let head = format!(" {} {name}", if *folded { "▸" } else { "▾" });
            let room = width.saturating_sub(cells(&tail) + 4);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(format!("{tail}  "), th.label()),
                Span::styled("⌫".to_string(), th.label()),
            ])
        }
        Row::Harness { name, folded, waiting, running, turbo, .. } => {
            // The badge of an agent asking for input: the one thing NEEDS YOU is.
            let tail = match waiting {
                0 => String::new(),
                1 => "● needs your input".to_string(),
                n => format!("● {n} need your input"),
            };
            let gone =
                if *running { String::new() } else { format!(" · {}", word("tree.not_running")) };
            // ⚡: an agent of it runs in turbo mode, as its tab says too.
            let turbo = if *turbo { " ⚡" } else { "" };
            let head = format!("   {} {name}{turbo}{gone}", if *folded { "▸" } else { "▾" });
            let room = width.saturating_sub(cells(&tail) + 1);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(tail, th.needs_you()),
            ])
        }
        Row::Action { project, unit, word, turn } => {
            // The row's own project, not the focused one: a unit in the
            // background was being drawn from whichever board was in front.
            let Some(unit) = app.units_of(*project).get(*unit) else { return Line::raw("") };
            // Where the Ask stands, and never an alarm: what needs you is an
            // agent asking, and that is its harness's row. The agent's state
            // shows when its session reported one; nothing rather than a guess.
            let agent = turn.map(|t| format!("{} · ", t.say()));
            let state = format!("{}{}", agent.unwrap_or_default(), word.say());
            let head = format!("      {}", unit.title);
            let room = width.saturating_sub(cells(&state) + 1);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(state, th.label()),
            ])
        }
    }
}
