//! The bar along the window's foot: Ask, Eval and Seal with why one cannot
//! be done; or what waits on the person: a staged prompt, a Seal's
//! disposition, a device asking to connect, a device to remove, quitting.

use weft_core::words::{fill, word};

use crate::action::{Answer, BarAct, SheetAct};
use crate::app::{BottomBarState, SealQuestions};
use crate::daemon::{ProjectClient, Staged};
use crate::gpui::{Element, Run, div, palette, rich, span};
use crate::tree_view::TreeSelection;
use crate::ui::{Size, Tone, asking_bar, bar_acts, button, note};

pub struct BottomBarProps<'a> {
    pub machine: &'a str,
    pub state: &'a BottomBarState,
    pub clients: &'a [ProjectClient],
    pub selection: Option<&'a TreeSelection>,
    /// The act put forward once the agent in front's turn has ended: outlined, never pressed.
    pub suggested: Option<weft_core::board::Act>,
    /// The staged send's SEND TO.
    pub staged_to: Option<&'a crate::sheets::send_to::SendTo>,
    /// What Weft reads of the setup of the harness the dialog's act goes to.
    pub warning: Option<&'a str>,
    /// The Eval dialog's Read by, when the dialog in front is an Eval's.
    pub read_by: Option<&'a ReadBy>,
    /// What a yes waits on the machine for, while it does.
    pub working: Option<&'a crate::waiting::Working>,
    /// What the machine would not do, until the person acts.
    pub failed: Option<&'a str>,
}

/// Who reads an Eval, picked in its dialog from every harness set up in the
/// project, by name and title; and why the one it started on, while it is
/// the one picked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadBy {
    pub harnesses: Vec<(String, String)>,
    pub picked: String,
    /// The word saying why the one it started on.
    pub note: Option<&'static str>,
    pub open: bool,
    /// The dialog is being made again for the harness picked.
    pub staging: bool,
}

/// The bar: what waits on the person, or the tree's acts.
pub fn render_bottom_bar(props: &BottomBarProps) -> Element {
    // A device asking to be allowed is answered first, over anything.
    if let BottomBarState::AllowDevice { device_id, device_name, login } = props.state {
        return render_allow_device_bar(props.machine, device_id, device_name, login.as_deref());
    }
    if let Some((client, staged)) =
        props.clients.iter().find_map(|c| c.waiting.last().map(|s| (c, s)))
    {
        return render_staged_bar(client, staged, props.staged_to, props.warning, props.read_by);
    }
    match props.state {
        BottomBarState::QuitConfirm => render_quit_bar(),
        BottomBarState::AllowDevice { device_id, device_name, login } => {
            render_allow_device_bar(props.machine, device_id, device_name, login.as_deref())
        }
        BottomBarState::RemoveDeviceConfirm { device_id, device_name } => {
            render_remove_device_bar(device_id, device_name)
        }
        BottomBarState::CloseProjectConfirm { project } => render_close_project_bar(project),
        BottomBarState::SealDisposition { project, eval, selected, reason, questions } => {
            let client = props.clients.iter().find(|c| &c.root == project);
            let picked = (selected.as_str(), reason.as_deref());
            match questions {
                // Until its questions are read, the Seal waits: nothing to seal yet.
                SealQuestions::Reading => asking_bar("Seal disposition bar", palette::BLUE_BORDER)
                    .child(title(word("dialog.seal.title")))
                    .child(crate::waiting::busy_line(word("seal.q.reading"), palette::MUTED))
                    .child(bar_acts(vec![button(
                        "btn Cancel",
                        word("act.cancel"),
                        BarAct::Cancel,
                        Tone::Quiet,
                        Size::Bar,
                    )])),
                // The questions take the window's body; the bar is theirs.
                SealQuestions::Asking(_) => div().layer("Seal asks"),
                SealQuestions::Unasked => {
                    render_seal_disposition_bar(client, project, eval, picked, props.warning)
                }
                // Not read: it goes as with none, and says why, with Try again.
                SealQuestions::Unread(why) => {
                    let again = crate::action::SheetAct::TryAgain {
                        project: project.clone(),
                        then: crate::daemon::Then::Questions { eval: eval.clone() },
                    };
                    let mut bar =
                        render_seal_disposition_bar(client, project, eval, picked, props.warning);
                    let failed = crate::waiting::failed(
                        crate::waiting::could_not_read(why),
                        Some(again.into()),
                    );
                    bar.children.insert(1, failed);
                    bar
                }
            }
        }
        BottomBarState::GitNoteConfirm { project } => render_git_note_bar(project),
        BottomBarState::Undo { commits, picked, .. } => render_undo_bar(commits, *picked),
        // A yes still waiting on the machine keeps the bar as its working
        // bar until the answer comes; what failed is said beneath it.
        BottomBarState::None | BottomBarState::WhyNot { .. } if props.working.is_some() => {
            let w = props.working.expect("working");
            let bar = crate::waiting::working_bar(w, std::time::Instant::now());
            let said = match props.state {
                BottomBarState::WhyNot { sentence } => Some(sentence.as_str()),
                _ => props.failed,
            };
            bar.children(said.map(|f| failed_sentence(f).self_stretch()))
        }
        BottomBarState::WhyNot { sentence } => render_tree_bar(props, Some(sentence.as_str())),
        BottomBarState::None => render_tree_bar(props, props.failed),
    }
}

/// What the machine would not do, in amber where it was asked.
fn failed_sentence(said: &str) -> Element {
    div().layer("failed sentence").font_size(13).color(palette::AMBER).text(said.to_string())
}

fn title(text: impl Into<String>) -> Element {
    div().font_weight(600).font_size(15).text(text)
}

/// What Weft reads of a harness's setup, in amber where the act is: a
/// warning, never a stop, since the reading may be wrong.
pub(crate) fn warned(said: &str) -> Element {
    div().layer("setup warning").font_size(13).color(palette::AMBER).text(said.to_string())
}

fn render_staged_bar(
    client: &ProjectClient,
    staged: &Staged,
    send_to: Option<&crate::sheets::send_to::SendTo>,
    warning: Option<&str>,
    read_by: Option<&ReadBy>,
) -> Element {
    let pane = client.view(staged.pane);
    let resolve = |answer| BarAct::Resolve {
        project: client.root.clone(),
        pending: staged.id.clone(),
        answer,
    };
    let cancel =
        button("btn Cancel", word("act.cancel"), resolve(Answer::No), Tone::Plain, Size::Sheet);

    // It keeps SEND TO: where it was staged, another agent, or a new one. A
    // dialog that types nothing names no agent.
    let choice = send_to.filter(|_| !staged.typed().is_empty()).map(|v| {
        let pick = |to: String| BarAct::SendTo { pending: staged.id.clone(), to }.into();
        div()
            .flex_row()
            .gap(10)
            .items_center()
            .child(
                div()
                    .font_mono()
                    .font_weight(600)
                    .font_size(11)
                    .color(palette::MUTED)
                    .text(word("ask.send_to")),
            )
            .child(crate::sheets::send_to::render(v, pick))
    });
    // Its title, why, and the exact bytes Send types, when it types any, the
    // route's command marked.
    let typed = String::from_utf8_lossy(staged.typed()).into_owned();
    let command = typed.split_once(' ').map(|(c, _)| c).filter(|c| c.starts_with(['/', '$']));
    let into = (!typed.is_empty()).then(|| {
        let name = client.root.file_name().map(|n| n.to_string_lossy().into_owned());
        let target = match (name, pane) {
            (Some(n), Some(p)) => format!("{n} · {}", p.harness),
            (_, Some(p)) => p.harness.clone(),
            _ => String::new(),
        };
        div()
            .font_size(13)
            .color(palette::MUTED)
            .text(fill("dialog.ask.into", &[("target", &target)]))
    });
    // One run of text, so it reads from its first line and wraps in the box.
    let runs = command
        .as_ref()
        .map(|c| {
            (0..c.trim_end().len(), Run { color: Some(palette::BLUE_BORDER), ..Run::default() })
        })
        .into_iter()
        .collect();
    let payload = (!typed.is_empty()).then(|| {
        div()
            .layer("payload")
            .self_stretch()
            .pad(10, 12)
            .bg(palette::SURFACE)
            .border(palette::LINE)
            .font_mono()
            .font_size(12)
            .style("white-space", "pre-wrap")
            .style("max-height", "240px")
            .style("overflow-y", "auto")
            .color(palette::TEXT)
            .child(rich(typed.clone(), runs).self_stretch())
    });
    // A dialog whose yes stops something is amber, as its yes is.
    let accent = if staged.go.as_ref().is_some_and(|g| g.warn) {
        palette::AMBER
    } else {
        palette::BLUE_BORDER
    };
    // A dialog that types nothing names its work in its head, beside its
    // title, and says the rest in one line.
    let data = staged.data();
    let reads = data.as_ref().map(|d| &d["reads"]);
    // An Eval that ends where later work began reads this work's own change.
    let own = reads.is_some_and(|r| r["until"].is_object());
    let lines: Vec<String> = staged
        .why
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| own.then_some(l).and_then(own_change).unwrap_or_else(|| l.to_string()))
        .collect();
    let work = match lines.as_slice() {
        [first, _, ..] if typed.is_empty() && !first.ends_with(['.', '?', '!', ':']) => {
            Some(first.as_str())
        }
        _ => None,
    };
    let said: Vec<String> = match work {
        Some(_) => vec![lines[1..].join(" ")],
        None if typed.is_empty() => {
            (!lines.is_empty()).then(|| lines.join(" ")).into_iter().collect()
        }
        None => staged.why.lines().map(str::to_string).collect(),
    };
    let work = work.map(|w| div().font_size(13).color(palette::MUTED).text(w.to_string()));
    // An Eval's dialog says what it will read; a large change, why in amber,
    // and its yes reads "Eval anyway".
    let large = reads.is_some_and(|r| r["large"] == true);
    let anyway = weft_proto::Go { word: "reads.anyway".into(), warn: false };
    let go = if large { Some(&anyway) } else { staged.go.as_ref() };
    asking_bar("Cancel / Send bar", accent)
        .child(
            div()
                .layer("dialog head")
                .flex_row()
                .gap(8)
                .items_center()
                .child(title(staged.what.clone()))
                .children(into)
                .children(work),
        )
        .children(read_by.filter(|_| typed.is_empty()).map(render_read_by))
        .children(reads.map(|r| render_reads(r, client.zone)))
        .children(reads.filter(|_| large).map(|r| render_large(r, client.zone)))
        .children(reads.and_then(|r| crate::skip_view::dialog(client, r)))
        .children(said.into_iter().map(|l| note(l, palette::MUTED)))
        .children(payload)
        .children(choice)
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .items_center()
                .children(warning.map(warned))
                .child(div().flex_1())
                .child(cancel)
                .child(yes_button(go, resolve(Answer::Yes))),
        )
}

/// The Eval dialog's Read by: closed, the harness picked and ▾, then why it
/// is the one it started on; open, every harness, the picked one ✓. Picking
/// stages the dialog again; only Eval runs it.
fn render_read_by(r: &ReadBy) -> Element {
    let one_line = |e: Element| {
        e.style("min-width", "0px")
            .style("white-space", "nowrap")
            .style("overflow", "hidden")
            .style("text-overflow", "ellipsis")
    };
    let title = |name: &str| {
        r.harnesses.iter().find(|(n, _)| n == name).map_or(name, |(_, t)| t.as_str()).to_string()
    };
    let control = if r.open {
        let mut list = div()
            .layer("read by list")
            .w(220)
            .flex_col()
            .pad(4, 0)
            .style("max-height", "240px")
            .style("overflow-y", "auto")
            .shows(r.harnesses.iter().position(|(n, _)| *n == r.picked).unwrap_or_default())
            .bg(palette::SURFACE)
            .border(palette::BLUE_BORDER)
            .panel_corners()
            .font_size(14);
        for (name, t) in &r.harnesses {
            let mut row = div()
                .layer(format!("read by {name}"))
                .on_click(BarAct::ReadBy(name.clone()))
                .flex_row()
                .gap(8)
                .pad(0, 12)
                .h(34)
                .items_center()
                .shrink_0()
                .child(one_line(div().flex_1().text(t.clone())));
            if *name == r.picked {
                row = row.bg(palette::SELECTED_BG).color(palette::BLUE_BORDER).child(span("✓"));
            }
            list = list.child(row);
        }
        list
    } else {
        div()
            .layer("read by choice")
            .on_click(BarAct::ToggleReadBy)
            .flex_row()
            .gap(8)
            .pad(5, 10)
            .items_center()
            .bg(palette::SURFACE)
            .border(palette::LINE)
            .font_weight(600)
            .font_size(13)
            .style("max-width", "240px")
            .child(one_line(span(title(&r.picked))))
            .child(span("▾").font_size(10).color(palette::MUTED))
    };
    div()
        .layer("read by")
        .self_stretch()
        .flex_row()
        .gap(14)
        .items_start()
        .child(
            div()
                .shrink_0()
                .pad(6, 0)
                .font_size(13)
                .color(palette::MUTED)
                .text(word("dialog.eval.read_by")),
        )
        .child(control)
        // Its dialog made again for the pick: it breathes until it comes.
        .children(r.staging.then(|| div().pad(9, 0).child(crate::waiting::dot())))
        .children(r.note.map(|n| {
            one_line(div().flex_1().pad(7, 0).font_size(12).color(palette::DIM).text(word(n)))
        }))
}

/// When an Eval's reading starts, as the person reads it: "today 09:12",
/// "yesterday 18:40", or the day, "Nov 8".
fn reads_when(at: &str, zone: i64) -> String {
    let Some(at_ms) = millis_of(at) else { return at.to_string() };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let said = weft_core::sessions::day_time(at_ms, now, zone);
    match weft_core::sessions::Group::of(at_ms, now, zone) {
        weft_core::sessions::Group::Today | weft_core::sessions::Group::Yesterday => {
            let mut c = said.chars();
            c.next().map_or_else(String::new, |f| f.to_lowercase().chain(c).collect())
        }
        _ => said.rsplit_once(' ').map_or(said.clone(), |(day, _)| day.to_string()),
    }
}

/// Milliseconds since the epoch from an RFC 3339 time, in UTC or with an
/// offset.
fn millis_of(at: &str) -> Option<i64> {
    if let Some(ms) = weft_core::turns::millis(at) {
        return Some(ms);
    }
    let (base, offset) = at.split_at(at.len().checked_sub(6)?);
    let sign = match offset.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let h: i64 = offset.get(1..3)?.parse().ok()?;
    let m: i64 = offset.get(4..6)?.parse().ok()?;
    Some(weft_core::turns::millis(&format!("{base}Z"))? - sign * (h * 60 + m) * 60_000)
}

fn n_of(v: &serde_json::Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

/// The Eval dialog's Reads: its commits and files, since when, and what
/// is counted but not read.
fn render_reads(r: &serde_json::Value, zone: i64) -> Element {
    let n = |v: u64| weft_core::words::count(v as usize);
    let commits = match n_of(&r["commits"]) {
        1 => word("reads.one_commit").to_string(),
        c => fill("reads.commits", &[("n", &n(c))]),
    };
    let files = match n_of(&r["files"]) {
        1 => word("reads.one_file").to_string(),
        f => fill("reads.files", &[("n", &n(f))]),
    };
    // A line written since the dialog was made: the count it had, struck.
    let counts = format!("{commits} · ");
    let counts = match r["files_before"].as_u64().filter(|b| *b != n_of(&r["files"])) {
        Some(before) => {
            let before = n(before);
            let struck = counts.len()..counts.len() + before.len();
            let run = Run { strike: true, color: Some(palette::DIM), ..Run::default() };
            rich(format!("{counts}{before} {files}"), vec![(struck, run)])
        }
        None => div().text(format!("{counts}{files}")),
    };
    let when = reads_when(r["when"].as_str().unwrap_or_default(), zone);
    let since = reads_since(r, &when, zone);
    let kinds: Vec<(u64, &str)> = ["generated", "vendored", "binary", "lock"]
        .iter()
        .map(|k| (n_of(&r["skipped"][*k]), *k))
        .filter(|(c, _)| *c > 0)
        .collect();
    let skipped = (!kinds.is_empty()).then(|| {
        let total: u64 = kinds.iter().map(|(c, _)| c).sum();
        let listed: Vec<String> =
            kinds.iter().map(|(c, k)| fill(&format!("reads.{k}"), &[("n", &n(*c))])).collect();
        let said = fill("reads.skipped", &[("n", &n(total)), ("kinds", &listed.join(", "))]);
        div().layer("reads skipped").font_size(12).color(palette::DIM).text(format!("· {said}"))
    });
    div()
        .layer("reads")
        .self_stretch()
        .flex_row()
        .gap(10)
        .items_center()
        .font_size(13)
        .child(div().w(64).shrink_0().color(palette::MUTED).text(word("reads.label")))
        .child(counts.layer("reads counts").font_mono())
        .child(since.layer("reads since").color(palette::MUTED))
        .children(skipped)
}

/// Since when the Eval reads, and, when a later work began on top of this
/// one's commits, until where: `from its first Ask, today 09:12, until
/// health endpoint began, 10:40`, the later work's title in the text's colour.
fn reads_since(r: &serde_json::Value, when: &str, zone: i64) -> Element {
    const DAY: i64 = 86_400_000;
    let seal = r["since"] == "seal";
    let until = &r["until"];
    if !until.is_object() {
        let since = if seal { "reads.since_seal" } else { "reads.since_ask" };
        return div().text(fill(since, &[("when", when)]));
    }
    let from = fill(if seal { "reads.from_seal" } else { "reads.from_ask" }, &[("when", when)]);
    let title = until["title"].as_str().unwrap_or_default();
    let time = until["time"].as_str().unwrap_or_default();
    let mut then = reads_when(time, zone);
    // The same day as it began, said with its time: its time alone.
    let day = |at: &str| millis_of(at).map(|ms| (ms + zone).div_euclid(DAY));
    let began = r["when"].as_str().unwrap_or_default();
    if day(time).is_some_and(|d| day(began) == Some(d))
        && let Some((_, clock)) = then.rsplit_once(' ').filter(|(_, c)| c.contains(':'))
    {
        then = clock.to_string();
    }
    let began = fill("reads.began", &[("when", &then)]);
    let at = from.len() + 1;
    let run = Run { color: Some(palette::TEXT), ..Run::default() };
    rich(format!("{from} {title} {began}"), vec![(at..at + title.len(), run)])
}

/// The dialog's sentence for an Eval that reads only its work's own change,
/// up to where a later work began: the same harness reads it.
fn own_change(line: &str) -> Option<String> {
    let (pre, post) = word("dialog.eval.judges").split_once("{harness}")?;
    let harness = line.strip_prefix(pre)?.strip_suffix(post)?;
    Some(fill("dialog.eval.judges_own", &[("harness", harness)]))
}

/// A large change, said in amber before its yes: where most of it is, and
/// how to make the next one smaller.
fn render_large(r: &serde_json::Value, zone: i64) -> Element {
    let when = reads_when(r["when"].as_str().unwrap_or_default(), zone);
    let folder = r["folder"].as_str().unwrap_or_default();
    let n = weft_core::words::count(n_of(&r["folder_files"]) as usize);
    div()
        .layer("reads large")
        .self_stretch()
        .flex_col()
        .gap(4)
        .pad(10, 12)
        .bg(palette::AMBER_BG)
        .border(palette::AMBER_BORDER)
        .style("border-radius", "4px")
        .font_size(13)
        .child(div().font_weight(600).color(palette::AMBER).text(word("reads.large_title")))
        .children((!folder.is_empty()).then(|| {
            let said = fill("reads.large_where", &[("when", &when), ("folder", folder), ("n", &n)]);
            mono_in(said, folder)
        }))
        .child(mono_in(word("reads.large_fix").to_string(), ".gitattributes"))
}

/// A sentence in muted words with `path` in it set in mono, as a path is.
fn mono_in(said: String, path: &str) -> Element {
    let runs = match said.find(path) {
        Some(at) if !path.is_empty() => {
            vec![(at..at + path.len(), Run { mono: true, ..Default::default() })]
        }
        _ => Vec::new(),
    };
    rich(said, runs).color(palette::MUTED)
}

/// A dialog's yes: Send, or the button it names, amber when it stops
/// something.
fn yes_button(go: Option<&weft_proto::Go>, yes: BarAct) -> Element {
    let (said, tone) = match go {
        Some(g) => (word(&g.word), if g.warn { Tone::Warn } else { Tone::Primary }),
        None => (word("act.send"), Tone::Primary),
    };
    button(format!("btn {said}"), said, yes, tone, Size::Sheet)
}

/// Turning a project's Git note on (17.19): what a note holds, and Turn on.
fn render_git_note_bar(project: &std::path::Path) -> Element {
    let name = project.file_name().unwrap_or_default().to_string_lossy().into_owned();
    asking_bar("Git note bar", palette::BLUE_BORDER)
        .child(title(word("dialog.export.title")))
        .child(note(fill("dialog.export.why", &[("project", &name)]), palette::MUTED))
        .child(
            div()
                .layer("sample")
                .self_stretch()
                .pad(10, 12)
                .bg(palette::SURFACE)
                .border(palette::LINE)
                .font_mono()
                .font_size(12)
                .style("white-space", "pre-wrap")
                .text(word("dialog.export.sample")),
        )
        .child(bar_acts(vec![
            button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Quiet, Size::Bar),
            button(
                "btn Turn on",
                word("dialog.export.go"),
                BarAct::TurnOnGitNote,
                Tone::Primary,
                Size::Bar,
            ),
        ]))
}

/// Undo's one dialog: the commits from where the work started up to now,
/// newest first, in a list that scrolls and keeps the one picked in view.
fn render_undo_bar(commits: &[weft_core::after_seal::Commit], picked: usize) -> Element {
    let row = |(at, c): (usize, &weft_core::after_seal::Commit)| {
        let on = at == picked;
        div()
            .layer(format!("commit {}", c.short))
            .on_click(BarAct::PickCommit(at))
            .flex_row()
            .items_center()
            .gap(12)
            .shrink_0()
            .h(34)
            .pad(0, 14)
            .bg(if on { palette::SELECTED_BG } else { palette::SURFACE })
            .border_bottom(palette::LINE)
            .child(
                div()
                    .w(12)
                    .h(12)
                    .shrink_0()
                    .style("border-radius", "6px")
                    .bg(if on { palette::BLUE_BORDER } else { palette::SURFACE })
                    .border(if on { palette::BLUE_BORDER } else { palette::DIM }),
            )
            .child(
                div()
                    .w(62)
                    .shrink_0()
                    .font_mono()
                    .font_size(12)
                    .color(palette::DIM)
                    .text(c.short.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .style("min-width", "0px")
                    .style("white-space", "nowrap")
                    .style("overflow", "hidden")
                    .style("text-overflow", "ellipsis")
                    .font_size(13)
                    .font_weight(if on { 600 } else { 400 })
                    .color(if on { palette::TEXT } else { palette::MUTED })
                    .text(c.subject.clone()),
            )
            .children(c.start.then(|| {
                div()
                    .font_mono()
                    .font_size(11)
                    .color(palette::BLUE_BORDER)
                    .text(word("dialog.undo.started"))
            }))
            .child(div().font_mono().font_size(11).color(palette::DIM).text(c.when.clone()))
    };
    let count = match commits.len() {
        1 => word("dialog.undo.count_one").to_string(),
        n => fill("dialog.undo.count", &[("n", &n.to_string())]),
    };
    let label = div()
        .flex_col()
        .gap(4)
        .w(96)
        .shrink_0()
        .child(
            div()
                .font_mono()
                .font_weight(600)
                .font_size(10)
                .color(palette::DIM)
                .text(word("dialog.undo.put_back")),
        )
        .child(div().font_size(12).color(palette::DIM).text(count));
    let tree = div()
        .layer("commits")
        .flex_col()
        .flex_1()
        .style("min-width", "0px")
        .style("max-height", "156px")
        .style("overflow-y", "auto")
        .bg(palette::SURFACE)
        .border(palette::LINE)
        .shows(picked)
        .children(commits.iter().enumerate().map(row));
    asking_bar("Undo bar", palette::AMBER_BORDER)
        .child(title(word("dialog.undo.title")))
        .child(
            div()
                .layer("undo list")
                .self_stretch()
                .flex_row()
                .gap(10)
                .items_start()
                .child(label)
                .child(tree),
        )
        .child(
            bar_acts(vec![
                button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Quiet, Size::Bar),
                button("btn Undo", word("offer.undo"), BarAct::Undo, Tone::Warn, Size::Bar),
            ])
            .layer("undo acts"),
        )
}

/// The Seal's one dialog when its Eval asked nothing: the work it seals, the
/// decision, and for anything but a yes an optional reason.
fn render_seal_disposition_bar(
    client: Option<&ProjectClient>,
    project: &std::path::Path,
    eval: &str,
    (selected, picked_reason): (&str, Option<&str>),
    warning: Option<&str>,
) -> Element {
    let row = |label: &str| {
        div().flex_row().gap(10).items_center().font_size(13).child(
            div()
                .w(64)
                .font_mono()
                .font_weight(600)
                .font_size(10)
                .color(palette::DIM)
                .text(word(label)),
        )
    };
    // The work, by its first Ask's title and its Asks, and the Eval's verdict.
    let unit = client.and_then(|c| c.work.of_eval(eval));
    let title_of = |root: &str| {
        client.and_then(|c| c.units.iter().find(|u| u.ask_id == root)).map(|u| u.title.clone())
    };
    let work = unit.map(|u| {
        let t = title_of(&u.root).unwrap_or_default();
        match u.asks.len() {
            1 => fill("dialog.eval.work_one", &[("title", &t)]),
            n => fill("dialog.eval.work", &[("title", &t), ("n", &n.to_string())]),
        }
    });
    let said = fill(
        if picked_reason.is_some() { "dialog.seal.records_why" } else { "dialog.seal.records" },
        &[
            ("disposition", &weft_core::tree::decision(selected)),
            ("work", work.as_deref().unwrap_or(eval)),
        ],
    );
    asking_bar("Seal disposition bar", palette::BLUE_BORDER)
        .child(
            div()
                .flex_row()
                .gap(8)
                .items_center()
                .child(title(word("dialog.seal.title")))
                .children(work.map(|w| div().font_size(13).color(palette::MUTED).text(w))),
        )
        .child(
            row("dialog.seal.decision")
                .child(crate::seal_dialog::decision_chips(project, eval, selected)),
        )
        .children(crate::seal_dialog::reason_chips(project, eval, selected, picked_reason).map(
            |chips| {
                row("dialog.seal.why")
                    .child(chips)
                    .child(span(word("dialog.seal.optional")).font_size(12).color(palette::DIM))
            },
        ))
        // A setup warning takes the acts' left, as in the Ask; what the Seal
        // records then goes on its own line above them.
        .children(warning.is_some().then(|| note(said.clone(), palette::MUTED)))
        .child(
            div()
                .layer("seal acts")
                .self_stretch()
                .flex_row()
                .gap(10)
                .items_center()
                .children(warning.map(warned))
                .child(match warning {
                    Some(_) => div().flex_1(),
                    None => div().flex_1().font_size(13).color(palette::MUTED).text(said),
                })
                .child(button(
                    "btn Cancel",
                    word("act.cancel"),
                    BarAct::Cancel,
                    Tone::Quiet,
                    Size::Bar,
                ))
                .child(button(
                    "btn Seal",
                    word("act.seal"),
                    BarAct::ConfirmSeal {
                        project: project.to_path_buf(),
                        eval: eval.to_string(),
                        disposition: selected.to_string(),
                        reason: picked_reason.map(str::to_string),
                    },
                    Tone::Primary,
                    Size::Bar,
                )),
        )
}

fn render_quit_bar() -> Element {
    div()
        .layer("Quit Weft bar")
        .self_stretch()
        .flex_row()
        .gap(12)
        .pad(12, 20)
        .items_center()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
        .child(
            div()
                .flex_1()
                .flex_col()
                .gap(2)
                .child(div().font_weight(600).font_size(14).text(word("desktop.quit.title")))
                .child(div().font_size(12).color(palette::MUTED).text(word("desktop.quit.what"))),
        )
        .child(button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Quiet, Size::Bar))
        .child(button(
            "btn Stop agents and quit",
            word("desktop.quit.stop"),
            BarAct::StopAgentsAndQuit,
            Tone::Warn,
            Size::Bar,
        ))
        .child(button(
            "btn Leave running and quit",
            word("desktop.quit.leave"),
            BarAct::LeaveRunningAndQuit,
            Tone::Primary,
            Size::Bar,
        ))
}

fn render_allow_device_bar(
    machine: &str,
    device_id: &str,
    device_name: &str,
    login: Option<&str>,
) -> Element {
    let answer = |yes| BarAct::AllowDevice { device: device_id.to_string(), yes };
    let mut bar = asking_bar("Allow a device bar", palette::BLUE_BORDER)
        .child(title(fill("desktop.allow.ask", &[("device", device_name), ("machine", machine)])));
    if let Some(login) = login {
        bar = bar.child(note(fill("web.add.signed_in", &[("login", login)]), palette::MUTED));
    }
    bar.child(note(word("desktop.allow.holding"), palette::MUTED)).child(bar_acts(vec![
        button("btn Refuse", word("desktop.allow.refuse"), answer(false), Tone::Plain, Size::Sheet),
        button("btn Allow", word("desktop.allow.allow"), answer(true), Tone::Primary, Size::Sheet),
    ]))
}

fn render_close_project_bar(project: &std::path::Path) -> Element {
    let name = ProjectClient::name_of(project);
    asking_bar("Close project bar", palette::BLUE_BORDER)
        .child(title(fill("desktop.close.ask", &[("project", &name)])))
        .child(note(word("desktop.close.what"), palette::MUTED))
        .child(bar_acts(vec![
            button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Plain, Size::Sheet),
            button(
                "btn Close it",
                word("desktop.close.yes"),
                BarAct::ConfirmCloseProject(project.to_path_buf()),
                Tone::Primary,
                Size::Sheet,
            ),
        ]))
}

fn render_remove_device_bar(device_id: &str, device_name: &str) -> Element {
    asking_bar("Remove device bar", palette::AMBER)
        .child(title(fill("desktop.remote.remove_sure", &[("device", device_name)])))
        .child(note(word("desktop.remote.remove_what"), palette::MUTED))
        .child(bar_acts(vec![
            button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Plain, Size::Sheet),
            button(
                "btn Confirm Remove",
                word("web.machines.remove"),
                SheetAct::RemoveDevice(device_id.to_string()),
                Tone::Warn,
                Size::Sheet,
            ),
        ]))
}

fn render_tree_bar(props: &BottomBarProps, why_not: Option<&str>) -> Element {
    use weft_core::board::Act;
    // Eval put forward in teal, Seal in amber, as the guideline's outlined buttons.
    let btn = |layer: &str, label: &str, action: BarAct, act: Act| {
        let accent = match act {
            Act::Eval if props.suggested == Some(Act::Eval) => Some(palette::TEAL),
            Act::Seal if props.suggested == Some(Act::Seal) => Some(palette::AMBER),
            _ => None,
        };
        div()
            .layer(layer)
            .on_click(action)
            .flex_row()
            .gap(6)
            .pad(7, 18)
            .items_center()
            .justify_center()
            .bg(palette::SURFACE)
            .border(accent.unwrap_or(palette::LINE))
            .child(
                div()
                    .font_weight(600)
                    .font_size(13)
                    .color(accent.unwrap_or(palette::TEXT))
                    .text(label),
            )
    };
    let selected = match props.selection {
        Some(TreeSelection::Agent { session, .. }) => Some(session.as_str()),
        Some(TreeSelection::Work { project, unit }) => props
            .clients
            .iter()
            .find(|c| &c.root == project)
            .and_then(|c| c.units.iter().find(|u| &u.ask_id == unit))
            .map(|u| u.title.as_str()),
        _ => None,
    };
    let mut row = div()
        .layer("tree bar")
        .self_stretch()
        // Room from the window's rounded bottom corners.
        .h(56)
        .shrink_0()
        .flex_row()
        .gap(10)
        .pad(0, 20)
        .items_center()
        .bg(palette::SURFACE_ALT)
        .border_top(palette::LINE)
        .child(btn("btn Ask", word("act.ask"), BarAct::Ask, Act::Ask))
        .child(btn("btn Eval", word("act.eval"), BarAct::Eval, Act::Eval))
        .child(btn("btn Seal", word("act.seal"), BarAct::Seal, Act::Seal));
    if let Some(sentence) = why_not {
        let layer =
            if props.failed == Some(sentence) { "failed sentence" } else { "why not sentence" };
        row = row.child(
            div()
                .layer(layer)
                .flex_1()
                .style("min-width", "0px")
                .font_size(13)
                .color(palette::AMBER)
                .text(sentence),
        );
    }
    row = row.child(div().flex_1());
    if let Some(what) = selected {
        row = row.child(
            div()
                .font_size(12)
                .color(palette::DIM)
                .text(fill("web.bar.selected", &[("what", what)])),
        );
    }
    row
}
