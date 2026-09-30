//! The panels anchored to the bottom: every question Weft asks.

use super::*;

/// The confirmation and the quit question are anchored to the bottom so the
/// row they are about stays in view above them.
/// Every interruption is a panel in the same place: anchored to the bottom,
/// where the quit question appears. Weft has no centred box — one covers
/// exactly what the person was reading, which is what v1 did wrong.
///
/// This is the sizing pass. `panel` draws the same content.
pub(super) fn panel_lines(app: &App) -> Option<Vec<String>> {
    let modal = app.modal.as_ref()?;
    let mut lines = vec![panel_title(app, modal)];
    lines.extend(panel_body(app, modal));
    lines.extend(panel_choices(app, modal).into_iter().map(|c| format!("  {c}")));
    Some(lines)
}

pub(super) fn panel_title(_app: &App, modal: &Modal) -> String {
    match modal {
        Modal::Quit => "QUIT WEFT?".into(),
        Modal::Weft => "WEFT".into(),
        Modal::CloseProject { .. } => "CLOSE THIS PROJECT?".into(),
        Modal::OpenProject { .. } => "OPEN A PROJECT".into(),
        Modal::SendAnyway { .. } => "THE AGENT HAS NOT SAID IT IS READY".into(),
        Modal::Help => "KEYS".into(),
        Modal::Note(_) => "WEFT".into(),
        Modal::Ask { .. } => "WHAT DO YOU WANT DONE?".into(),
        Modal::Confirm(p) => p.what.to_uppercase(),
        Modal::StartAgent { .. } => "START AN AGENT".into(),
        Modal::RingFrame => "RINGFRAME".into(),
        Modal::Locate { harness, .. } => format!("WHERE IS {}?", harness.to_uppercase()),
        Modal::PickUp { harness, .. } => format!("{} IS NOT RUNNING", harness.to_uppercase()),
    }
    .to_string()
}

pub(super) fn panel_body(app: &App, modal: &Modal) -> Vec<String> {
    match modal {
        // The menu is its choices; there is nothing to say above them.
        Modal::Weft => Vec::new(),
        Modal::SendAnyway { why, .. } => vec![
            format!("Weft did not type it: {why}."),
            String::new(),
            "Weft types on its own only into an agent that has said it is".into(),
            "ready. If you can see it is, say so and Weft will type.".into(),
            String::new(),
        ],
        Modal::OpenProject { text } => {
            vec![format!("{text}█"), String::new(), "The path of a repository to open.".into()]
        }
        Modal::CloseProject { name } => vec![
            name.clone(),
            String::new(),
            "The agents keep running and the record is untouched.".into(),
            "Weft stops showing it here.".into(),
            String::new(),
        ],
        Modal::Quit => vec![
            "Your agents are running in the background. They can keep going".into(),
            "without Weft open.".into(),
            String::new(),
        ],
        Modal::Help => help_lines(app).into_iter().chain(routing_lines(app)).collect(),
        Modal::Note(text) => text.lines().map(str::to_string).collect(),
        Modal::Ask { text, target, follows } => {
            let mut lines = Vec::new();
            if let Some(f) = follows {
                lines.push(format!("FOLLOW UP  {} · {}", f.title, f.carries));
                lines.push(String::new());
            }
            // Folded, not wrapped: `wrap` rejoins words with single spaces, so
            // a run of spaces vanished on screen and then reappeared in the
            // confirmation. An input shows what was typed.
            if !text.is_empty() {
                lines.extend(fold(text, 72));
            }
            match lines.last_mut() {
                Some(last) => last.push('_'),
                None => lines.push("_".into()),
            }
            lines.push(String::new());
            let tabs: Vec<String> = (0..app.pane_count())
                .map(|i| {
                    format!(
                        "{}{} {}",
                        if i == *target { "▸ " } else { "" },
                        i + 1,
                        app.harness_at(i).unwrap_or("agent")
                    )
                })
                .collect();
            lines.push(format!("SEND TO     {}", tabs.join("    ")));
            lines
        }
        Modal::Confirm(p) => {
            let mut lines = p.why.clone();
            lines.push(String::new());
            lines.extend(String::from_utf8_lossy(&p.payload).lines().map(str::to_string));
            lines
        }
        Modal::StartAgent { .. } => {
            vec!["It runs here, with your own settings.".into(), String::new()]
        }
        Modal::PickUp { .. } => Vec::new(),
        Modal::RingFrame => setup_body(app),
        Modal::Locate { text, .. } => vec![
            format!("{text}█"),
            String::new(),
            "The whole path of its program, as `/…` or `~/…`. Weft runs it with".into(),
            "--version to check it starts, and keeps it in ~/.fab7/weft/config.toml.".into(),
        ],
    }
}

/// Three sections side by side, one key to a line.
pub(super) fn help_lines(app: &App) -> Vec<String> {
    let toggle = app.toggle.label();
    let navigation: Vec<(&str, &str)> = vec![
        ("NAVIGATION", ""),
        ("↑ ↓", "move"),
        ("→ ←", "unfold · fold, back"),
        ("Enter", "open/proceed"),
        ("Space", "what needs you"),
        ("Tab", "next agent"),
        ("1-9", "that agent"),
        (toggle, "switch active pane"),
        ("⌫", "close the project"),
    ];
    let ringframe = [
        ("RINGFRAME", ""),
        ("A", "ask"),
        ("E", "eval"),
        ("S", "seal"),
        ("V", "eval view"),
        ("D", "a change"),
    ];
    let weft = [
        ("WEFT", ""),
        ("P", "proceed the action"),
        ("F", "follow up next action"),
        ("O", "open a project"),
        ("N", "new agent"),
        ("B", "sidebar"),
        ("T", "turbo mode"),
        ("W", "the Weft menu"),
        ("U", "update"),
        ("H", "help"),
        ("X", "quit"),
    ];
    let cell = |col: &[(&str, &str)], i: usize, key_w: usize, w: usize| match col.get(i) {
        Some((head, "")) => format!("{head:<w$}"),
        Some((key, what)) => format!("{key:<key_w$}{what:<0$}", w - key_w),
        None => " ".repeat(w),
    };
    let rows = navigation.len().max(weft.len());
    let mut lines: Vec<String> = (0..rows)
        .map(|i| {
            let line = format!(
                "{}  {}  {}",
                cell(&navigation, i, 9, 28),
                cell(&ringframe, i, 3, 13),
                cell(&weft, i, 3, 24)
            );
            line.trim_end().to_string()
        })
        .collect();
    lines.push(String::new());
    lines.push(
        "In an agent every key is the agent's, Esc included, and the wheel scrolls it.".into(),
    );
    lines
}

/// The choices a panel offers, if it is the kind that picks one.
pub(super) fn panel_choices(app: &App, modal: &Modal) -> Vec<String> {
    match modal {
        Modal::Weft => crate::app::WEFT_MENU
            .iter()
            .map(|(key, label, _)| format!("[{key}]  {label}"))
            .collect(),
        Modal::CloseProject { .. } => vec!["Close it".into(), "Cancel".into()],
        Modal::SendAnyway { why, .. } => {
            let cancel = if *why == weft_core::turns::ASKING {
                "Cancel — I will answer the agent"
            } else {
                "Cancel"
            };
            vec!["Type it anyway".into(), cancel.into()]
        }
        Modal::OpenProject { .. } => Vec::new(),
        Modal::RingFrame => app
            .setup_view()
            .map(|v| {
                v.rows
                    .iter()
                    .map(|r| {
                        clip(&format!("{}  {:<14}{}", r.state.mark(), r.title, r.state.words()), 72)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Modal::Quit => vec![
            "Quit, leave the agents running".into(),
            "Quit and stop the agents".into(),
            "Cancel".into(),
        ],
        Modal::StartAgent { .. } => app.fresh_starts().iter().map(|c| c.label.clone()).collect(),
        Modal::PickUp { harness, session, .. } => crate::app::pick_up_choices(session.as_ref())
            .into_iter()
            .map(|c| match (c, session) {
                (crate::app::PickUp::Resume, Some(s)) => {
                    let mut said = format!(
                        "Resume the session this was asked in · {}",
                        crate::sessions::clock(s.at)
                    );
                    // A harness with no prompt hook has no last prompt to show.
                    if !s.last.is_empty() {
                        said.push_str(&format!(" · {}", s.last));
                    }
                    clip(&said, 72)
                }
                _ => format!("Start a fresh {harness}"),
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn panel(app: &App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let Some(modal) = app.modal.as_ref() else { return Paragraph::new("") };
    let mut drawn = vec![Line::styled(format!(" {}", panel_title(app, modal)), th.title())];

    let room = (area.height as usize).saturating_sub(1);
    let body = panel_body(app, modal);
    let body: Vec<String> = body
        .iter()
        .flat_map(|l| {
            if l.chars().count() > area.width.saturating_sub(2) as usize {
                fold(l, area.width.saturating_sub(2) as usize)
            } else {
                vec![l.clone()]
            }
        })
        .collect();
    let choices = panel_choices(app, modal);
    let for_body = room.saturating_sub(choices.len());
    for (i, l) in body.iter().take(for_body).enumerate() {
        let more = body.len() > for_body && i + 1 == for_body;
        drawn.push(Line::styled(
            if more { format!(" {l}  ▼") } else { format!(" {l}") },
            th.label(),
        ));
    }
    for (i, choice) in choices.iter().enumerate() {
        let picked = i == app.modal_choice;
        drawn.push(Line::styled(
            format!("   {} {choice}", if picked { "▸" } else { " " }),
            if picked { th.selected() } else { th.label() },
        ));
    }
    Paragraph::new(drawn)
}

/// Left spans, right spans, and the gap between them.
/// What this project routes, shown where a person looks for how it is set up.
/// Nothing at all when it routes nothing, which is most projects.
pub(super) fn routing_lines(app: &App) -> Vec<String> {
    let routed = app.routing().each();
    if routed.is_empty() {
        return Vec::new();
    }
    let said: Vec<String> = routed.iter().map(|(act, h)| format!("{act} → {h}")).collect();
    vec![String::new(), format!("This project routes  {}", said.join("   "))]
}

/// What the RingFrame view says above its rows: the latest release, where
/// RingFrame's configuration stands, what the last thing tried came to, and a
/// failed step's own last lines.
fn setup_body(app: &App) -> Vec<String> {
    let Some(v) = app.setup_view() else {
        return vec!["Checking the latest release…".into(), String::new()];
    };
    let mut lines = vec![
        match (&v.latest, &v.plugin) {
            (Some(t), Some(p)) => format!("Latest release {t} · rf {p}"),
            (Some(t), None) => format!("Latest release {t}"),
            _ => "Weft could not reach the latest release.".into(),
        },
        format!("RingFrame's configuration: {}", v.configuration),
        String::new(),
    ];
    if v.rows.is_empty() {
        lines.push("Weft has no harness files in ~/.fab7/weft/harnesses.".into());
        lines.push(String::new());
        return lines;
    }
    lines.push("Sign in to a harness yourself first: Weft runs its own plugin".into());
    lines.push("commands, then asks it again. Your rules are not touched.".into());
    lines.push(String::new());
    if let Some(note) = &v.note {
        lines.push(note.clone());
        lines.push(String::new());
    }
    if let Some(r) = v.rows.get(app.modal_choice)
        && let weft_core::onboarding::State::Failed { said, .. } = &r.state
    {
        lines.extend(said.lines().map(|l| format!("  {l}")));
        lines.push(String::new());
    }
    lines
}
