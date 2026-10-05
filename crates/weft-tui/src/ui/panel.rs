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
        Modal::Bar(Bar::SendAnyway { .. }) => word("not_ready.title").to_uppercase(),
        Modal::Help => "KEYS".into(),
        Modal::Note(_) => "WEFT".into(),
        Modal::Ask { .. } => word("ask.title").to_uppercase(),
        Modal::Bar(Bar::Send(p)) => p.what.to_uppercase(),
        Modal::StartAgent { .. } => "START AN AGENT".into(),
        Modal::RingFrame => "RINGFRAME".into(),
        Modal::Remote => "REMOTE".into(),
        Modal::Allow { .. } => "ALLOW A DEVICE?".into(),
        Modal::Locate { harness, .. } => format!("WHERE IS {}?", harness.to_uppercase()),
        Modal::Bar(Bar::Resume { harness, .. } | Bar::NothingToResume { harness }) => {
            say("resume.title", harness).to_uppercase()
        }
    }
    .to_string()
}

pub(super) fn panel_body(app: &App, modal: &Modal) -> Vec<String> {
    match modal {
        // The menu is its choices; there is nothing to say above them.
        Modal::Weft => Vec::new(),
        Modal::Bar(Bar::SendAnyway { why, .. }) => {
            let mut lines = vec![fill("not_typed.why", &[("why", why)])];
            lines.extend(fold(word("not_ready.why"), 62).iter().map(|l| l.trim_end().to_string()));
            lines
        }
        Modal::Bar(Bar::Resume { session, .. }) => {
            let mut said =
                format!("{} · {}", word("resume.session"), crate::sessions::clock(session.at));
            // A harness with no prompt hook has no last prompt to show.
            if !session.last.is_empty() {
                said.push_str(&format!(" · \"{}\"", clip(&session.last, 28)));
            }
            vec![said]
        }
        Modal::Bar(Bar::NothingToResume { harness }) => {
            vec![word("resume.nothing").into(), say("resume.fresh", harness)]
        }
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
        Modal::Ask { text, follows, .. } => {
            let mut lines = Vec::new();
            if let Some(f) = follows {
                let head = word("act.follow_up").to_uppercase();
                lines.push(format!("{head}  {} · {}", f.title, f.carries));
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
            lines.extend([String::new(), word("ask.send_to").into(), String::new()]);
            lines
        }
        Modal::Bar(Bar::Send(p)) => {
            let mut lines = p.why.clone();
            lines.push(String::new());
            lines.extend(String::from_utf8_lossy(&p.payload).lines().map(str::to_string));
            lines
        }
        Modal::StartAgent { .. } => {
            vec!["It runs here, with your own settings.".into(), String::new()]
        }
        Modal::RingFrame => setup_body(app),
        Modal::Remote => remote_body(app),
        Modal::Allow { name, login, .. } => vec![
            format!("Allow \"{name}\", signed in as {login}?"),
            String::new(),
            "It will see your agents here and can type into them.".into(),
            "Say yes only to a device you are holding.".into(),
        ],
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
        ("R", "remote"),
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
        Modal::CloseProject { .. } => vec!["Close it".into(), word("act.cancel").into()],
        Modal::OpenProject { .. } => Vec::new(),
        Modal::Remote if app.remote_view().is_some_and(|v| v["state"] == "on") => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            app.remote_devices()
                .iter()
                .map(|d| {
                    let s = |k: &str| d[k].as_str().unwrap_or_default().to_string();
                    let seen = crate::app::ago(d["last_seen"].as_i64().unwrap_or(0), now);
                    format!("{}  ·  {}  ·  seen {seen}", s("name"), s("login"))
                })
                .collect()
        }
        Modal::RingFrame => app
            .setup_view()
            .map(|v| v.rows.iter().map(|r| clip(&r.line(), 72)).collect())
            .unwrap_or_default(),
        Modal::Quit => vec![
            "Quit, leave the agents running".into(),
            "Quit and stop the agents".into(),
            word("act.cancel").into(),
        ],
        Modal::StartAgent { .. } => app.fresh_starts().iter().map(|c| c.label.clone()).collect(),
        Modal::Ask { .. } => app
            .running_panes()
            .into_iter()
            .map(|i| app.harness_at(i).unwrap_or("agent").to_string())
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
    let at = match modal {
        Modal::Ask { target, .. } => {
            app.running_panes().iter().position(|i| i == target).unwrap_or(0)
        }
        _ => app.modal_choice,
    };
    for (i, choice) in choices.iter().enumerate() {
        let picked = i == at;
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
    let mut lines = if v.pending_release {
        vec!["Checking the latest release…".into(), String::new()]
    } else {
        vec![
            match (&v.latest, &v.plugin) {
                (Some(t), Some(p)) => format!("Latest release {t} · rf {p}"),
                (Some(t), None) => format!("Latest release {t}"),
                _ => "Weft could not reach the latest release.".into(),
            },
            format!("RingFrame's configuration: {}", v.configuration),
            String::new(),
        ]
    };
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

/// What the Remote view says, by the state `weft remote` is in.
fn remote_body(app: &App) -> Vec<String> {
    let view = app.remote_view();
    let s = |k: &str| view.and_then(|v| v[k].as_str()).unwrap_or_default().to_string();
    match view.and_then(|v| v["state"].as_str()).unwrap_or("off") {
        "starting" => vec!["Starting…".into(), String::new()],
        "choose" => {
            let mut lines = vec!["Pick how this machine is reached:".into(), String::new()];
            let tunnels = view.and_then(|v| v["tunnels"].as_array().cloned()).unwrap_or_default();
            for (i, t) in tunnels.iter().enumerate() {
                let mark = if i == app.modal_choice { "▸" } else { " " };
                lines.push(format!(" {mark} {}", t["title"].as_str().unwrap_or_default()));
            }
            lines.push(String::new());
            lines
        }
        "not_installed" if app.remote_run => {
            let offer = view.map(|v| v["offer"].clone()).unwrap_or_default();
            let tunnel = view.map(|v| v["tunnel"].clone()).unwrap_or_default();
            let program = tunnel["program"].as_str().unwrap_or_default();
            let (what, why) = match offer["how"].as_str() {
                Some("brew") => (
                    offer["argv"].as_str().unwrap_or_default().to_string(),
                    format!("It installs {program} with Homebrew."),
                ),
                _ => (
                    format!(
                        "Download {program} from {}",
                        offer["from"].as_str().unwrap_or_default()
                    ),
                    "Into ~/.fab7/weft/bin, checked against GitHub's checksum and the release's."
                        .into(),
                ),
            };
            vec!["RUN THIS?".into(), format!("  {what}"), String::new(), why, String::new()]
        }
        "not_installed" => {
            let offer = view.map(|v| v["offer"].clone()).unwrap_or_default();
            let tunnel = view.map(|v| v["tunnel"].clone()).unwrap_or_default();
            let (title, program) = (
                tunnel["title"].as_str().unwrap_or_default(),
                tunnel["program"].as_str().unwrap_or_default(),
            );
            let mut lines = vec![format!("{title} needs {program}, which is not on this machine.")];
            match offer["how"].as_str() {
                Some("brew") => {
                    lines.push("Install runs:".into());
                    lines.push(String::new());
                    lines.push(format!("  {}", offer["argv"].as_str().unwrap_or_default()));
                }
                Some(_) => {
                    lines.push("Install downloads it, for this machine:".into());
                    lines.push(String::new());
                    lines.push(format!(
                        "  from {} into ~/.fab7/weft/bin",
                        offer["from"].as_str().unwrap_or_default()
                    ));
                }
                None => lines.push(s("why")),
            }
            if !s("failed").is_empty() {
                lines.push(String::new());
                lines.push(format!("The last install did not finish: {}", s("failed")));
            }
            lines.push(String::new());
            lines
        }
        "installing" => vec!["Installing…".into(), format!("  {}", s("line")), String::new()],
        "signing_in" => vec![
            format!("Enter {} at {}", s("code"), s("url")),
            String::new(),
            "This machine joins your account. GitHub says who you are; this machine".into(),
            "still says which devices get in.".into(),
            String::new(),
        ],
        "signed_out" => {
            let mut lines = vec!["Sign in with GitHub to use Weft from your phone.".into()];
            if !s("failed").is_empty() {
                lines.push(format!("The last sign-in did not finish: {}", s("failed")));
            }
            lines.push(String::new());
            lines
        }
        "on" => {
            let tunnel = if s("tunnel") == "up" { "up" } else { "starting" };
            let mut lines = vec![
                format!("Open weft.getfab7.com, signed in as {}.", s("login")),
                format!("Tunnel {tunnel}  {}", s("address")),
                format!("{}.", capitalized(&s("note"))),
                String::new(),
            ];
            if app.remote_devices().is_empty() {
                lines.push(
                    "No device is allowed yet. Open the app on one, and say yes here.".into(),
                );
                lines.push(String::new());
            } else {
                lines.push("DEVICES".into());
            }
            lines
        }
        _ => vec![
            "Use Weft from your phone at weft.getfab7.com.".into(),
            String::new(),
            "Your phone reaches this machine through a tunnel you pick next, and".into(),
            "only a device you allow here gets in.".into(),
            String::new(),
        ],
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}
