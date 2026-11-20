//! What the window shows while the machine is still doing something: a
//! breathing dot and its words at once, and past a few seconds a moving bar
//! and the time so far; lines that breathe where content is still coming;
//! and, when it fails, the reason in amber where the work was shown. When
//! the machine says how far along it is, the wait lists its steps, its
//! count, and a bar that fills.

use std::time::{Duration, Instant};

use weft_core::words::{fill, word};
use weft_proto::{Job, JobStep, StepState};

use crate::action::Action;
use crate::gpui::{Element, div, palette};
use crate::ui::{Size, Tone, asking_bar, button};

/// How long something works before its bar moves and its time shows.
pub const SHOW_TIME_AFTER: Duration = Duration::from_secs(3);

/// The time so far, as `m:ss`.
pub fn elapsed(since: Instant, now: Instant) -> String {
    clock(now.saturating_duration_since(since))
}

/// A length of time as `m:ss`.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// The dot that breathes while something works.
pub fn dot() -> Element {
    div()
        .layer("breathing dot")
        .w(8)
        .h(8)
        .shrink_0()
        .style("border-radius", "4px")
        .bg(palette::BLUE_BORDER)
        .style("animation", "pulse")
}

/// A bar whose stripe moves while something works.
pub fn moving_bar() -> Element {
    div()
        .layer("moving bar")
        .self_stretch()
        .h(3)
        .style("position", "relative")
        .style("overflow", "hidden")
        .style("border-radius", "2px")
        .bg(palette::LINE)
        .child(
            div()
                .style("position", "absolute")
                .style("top", "0")
                .style("bottom", "0")
                .w(120)
                .style("border-radius", "2px")
                .bg(palette::BLUE_BORDER)
                .style("animation", "slide"),
        )
}

/// A dot and its words, on one line: a row or a part that is working.
pub fn busy_line(said: impl Into<String>, colour: &str) -> Element {
    div()
        .layer("busy")
        .flex_row()
        .gap(8)
        .items_center()
        .child(dot())
        .child(div().font_size(13).color(colour).text(said))
}

/// Lines that breathe where content is still coming: never "none yet".
pub fn loading(widths: &[u32]) -> Element {
    div().layer("loading").self_stretch().flex_col().gap(10).style("max-width", "620px").children(
        widths.iter().map(|w| {
            div()
                .h(12)
                .w(*w)
                .style("max-width", "100%")
                .style("border-radius", "3px")
                .bg(palette::SURFACE)
                .style("animation", "pulse")
        }),
    )
}

/// A read that failed, said where its content would be, with Try again,
/// until the person tries again or moves on.
pub fn failed(said: impl Into<String>, again: Option<Action>) -> Element {
    div()
        .layer("failed")
        .self_stretch()
        .style("max-width", "720px")
        .flex_row()
        .gap(14)
        .items_center()
        .pad(12, 16)
        .bg(palette::AMBER_BG)
        .border(palette::AMBER_BORDER)
        .style("border-radius", "4px")
        .child(div().flex_1().font_size(13).color(palette::AMBER).text(said))
        .children(
            again.map(|a| {
                button("btn Try again", word("wait.try_again"), a, Tone::Plain, Size::Row)
            }),
        )
}

/// A job the machine runs, as it last said: since when it runs, worked
/// out from the age it came with, and when that word came.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub job: Job,
    pub since: Instant,
    pub got: Instant,
}

impl Progress {
    /// A job just told, at `now`.
    pub fn of(job: Job, now: Instant) -> Self {
        let since = now.checked_sub(Duration::from_millis(job.age_ms)).unwrap_or(now);
        Self { job, since, got: now }
    }

    pub fn running(&self) -> bool {
        self.job.end.is_none()
    }

    /// Why it failed, when it did.
    pub fn failed(&self) -> Option<&str> {
        match &self.job.end {
            Some(end) if !end.ok => Some(end.why.as_deref().unwrap_or_default()),
            _ => None,
        }
    }

    /// Its failure in words: what failed, and why.
    pub fn failure(&self) -> Option<String> {
        let why = self.failed()?;
        Some(match self.job.kind.as_str() {
            "eval_open" => fill("wait.could_not_open_eval", &[("why", why)]),
            "seal_check" => fill("wait.could_not_check_receipt", &[("why", why)]),
            _ => why.to_string(),
        })
    }

    /// The step under way.
    pub fn now_step(&self) -> Option<&JobStep> {
        self.job.steps.iter().find(|s| s.state == StepState::Now)
    }

    /// How long a step took, or, for the one under way, has taken so far.
    pub fn step_time(&self, step: &JobStep, now: Instant) -> Option<Duration> {
        let ms = Duration::from_millis(step.ms?);
        Some(match step.state {
            StepState::Now if self.running() => ms + now.saturating_duration_since(self.got),
            _ => ms,
        })
    }

    /// The step under way, as "step 2 of 3", when there are several.
    pub fn place(&self) -> Option<String> {
        let now = self.now_step()?;
        if now.unit.as_deref() == Some("steps") {
            return count(now);
        }
        let i = self.job.steps.iter().position(|s| s.state == StepState::Now)?;
        let n = self.job.steps.len();
        (n > 1).then(|| {
            fill("count.steps", &[("done", &(i + 1).to_string()), ("total", &n.to_string())])
        })
    }
}

/// A step in words: its key filled with its values.
pub fn step_said(step: &JobStep) -> String {
    let with: Vec<(&str, &str)> = step.args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    fill(&step.key, &with)
}

/// What a step counts, when it says: "38 of 103 files", "41 of 94 MB".
pub fn count(step: &JobStep) -> Option<String> {
    let (done, total) = (step.done?, step.total?);
    let n = |v: u64| weft_core::words::count(v as usize);
    let mb = |v: u64| n(v.saturating_add(500_000) / 1_000_000);
    let (key, done, total) = match step.unit.as_deref() {
        Some("bytes") => ("count.mb", mb(done), mb(total)),
        Some("reads") => ("count.read", n(done), n(total)),
        Some("readers") => ("count.readers", n(done), n(total)),
        Some("steps") => ("count.steps", n(done), n(total)),
        _ => ("count.files", n(done), n(total)),
    };
    Some(fill(key, &[("done", &done), ("total", &total)]))
}

/// How many of a step's files are new, when it says: "9 of them new".
fn new_of(step: &JobStep) -> Option<String> {
    let n = step.args.get("new")?.parse::<u64>().ok().filter(|n| *n > 0)?;
    Some(fill("count.new", &[("n", &weft_core::words::count(n as usize))]))
}

/// What a step adds beneath itself while under way, when its words have one:
/// "can take a minute or two the first time".
fn step_note(step: &JobStep) -> Option<String> {
    let key = format!("{}_note", step.key);
    let said = word(&key);
    (said != key).then(|| said.to_string())
}

/// A bar filled by the share done; with no share known, its stripe moves.
pub fn share_bar(share: Option<f32>) -> Element {
    let Some(share) = share else { return moving_bar() };
    let left = ((1.0 - share.clamp(0.0, 1.0)) * 100.0).round();
    div()
        .layer("filled bar")
        .self_stretch()
        .h(3)
        .style("position", "relative")
        .style("overflow", "hidden")
        .style("border-radius", "2px")
        .bg(palette::LINE)
        .child(
            div()
                .layer("filled")
                .attr("data-share", format!("{share:.2}"))
                .style("position", "absolute")
                .style("top", "0")
                .style("bottom", "0")
                .style("left", "0")
                .style("right", format!("{left}%"))
                .style("border-radius", "2px")
                .bg(palette::BLUE_BORDER),
        )
}

fn small_time(d: Duration) -> Element {
    div().layer("step time").font_mono().font_size(11).color(palette::DIM).text(clock(d))
}

/// A step's mark: ✓ done, a dot under way, ○ to come.
fn step_mark(state: StepState) -> Element {
    let mark = |t: &str, c: &str| {
        div().w(14).shrink_0().font_mono().color(c).style("text-align", "center").text(t)
    };
    match state {
        StepState::Done => mark("✓", palette::TEAL),
        StepState::Now => div().w(14).shrink_0().flex_row().justify_center().child(dot()),
        StepState::Next => mark("○", palette::DIM),
        StepState::Failed => mark("✗", palette::AMBER),
        StepState::Skipped => mark("–", palette::DIM),
    }
}

fn state_name(state: StepState) -> &'static str {
    match state {
        StepState::Done => "done",
        StepState::Now => "now",
        StepState::Next => "next",
        StepState::Failed => "failed",
        StepState::Skipped => "skipped",
    }
}

/// Every step, one a line: done ones muted with their time, the one under
/// way with its count and its own time, those to come faint. `below` puts
/// the count under the step's words, as the Eval view does.
pub fn steps(p: &Progress, now: Instant, below: bool) -> Element {
    let rows = p.job.steps.iter().map(|s| {
        let colour = match s.state {
            StepState::Done => palette::MUTED,
            StepState::Now => palette::TEXT,
            StepState::Failed => palette::AMBER,
            StepState::Next | StepState::Skipped => palette::DIM,
        };
        let shown = matches!(s.state, StepState::Now | StepState::Done | StepState::Failed);
        let time = p.step_time(s, now).filter(|_| shown).map(small_time);
        let words = div().layer("step said").color(colour).text(step_said(s));
        let under_way = s.state == StepState::Now;
        let counted = count(s).filter(|_| under_way).map(|c| {
            div().layer("step count").font_mono().font_size(12).color(palette::MUTED).text(c)
        });
        let new = new_of(s).filter(|_| under_way).map(|n| {
            div().layer("step new").font_size(12).color(palette::DIM).text(format!("· {n}"))
        });
        let note = step_note(s)
            .filter(|_| under_way)
            .map(|n| div().layer("step note").font_size(12).color(palette::DIM).text(n));
        let line = |counted: Option<Element>| {
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .items_center()
                .child(words.clone())
                .children(counted)
                .children(new.clone())
                .child(div().flex_1())
                .children(time.clone())
        };
        let body = if below {
            div().flex_1().flex_col().child(line(None)).children(counted).children(note)
        } else {
            div().flex_1().flex_col().child(line(counted)).children(note)
        };
        div()
            .layer(format!("step {}", state_name(s.state)))
            .self_stretch()
            .flex_row()
            .gap(10)
            .items_start()
            .child(step_mark(s.state).style("padding-top", "2px"))
            .child(body)
    });
    div().layer("steps").self_stretch().flex_col().gap(6).font_size(13).children(rows)
}

/// The steps on one line, as a row of Updates shows them: "Finding the
/// newest release ✓ · Downloading ● · Installing ○".
pub fn steps_inline(p: &Progress) -> Element {
    let mut row = div().layer("steps").flex_row().gap(8).items_center().font_size(12);
    for (i, s) in p.job.steps.iter().enumerate() {
        if i > 0 {
            row = row.child(div().color(palette::DIM).text("·"));
        }
        let (colour, mark) = match s.state {
            StepState::Done => (palette::MUTED, div().font_mono().color(palette::TEAL).text("✓")),
            StepState::Now => (palette::TEXT, dot().w(6).h(6).style("border-radius", "3px")),
            StepState::Failed => {
                (palette::AMBER, div().font_mono().color(palette::AMBER).text("✗"))
            }
            StepState::Next | StepState::Skipped => {
                (palette::DIM, div().font_mono().color(palette::DIM).text("○"))
            }
        };
        row = row.child(
            div()
                .layer(format!("step {}", state_name(s.state)))
                .flex_row()
                .gap(8)
                .items_center()
                .child(div().color(colour).text(step_said(s)))
                .child(mark),
        );
    }
    row
}

/// What the bar says while a yes waits on the machine.
#[derive(Debug, Clone, PartialEq)]
pub struct Working {
    /// Its words: "Sealing…".
    pub said: String,
    /// What it is about, beside them.
    pub about: String,
    pub since: Instant,
    /// An Eval being opened: its note too, and its line until its steps come.
    pub opening: bool,
    /// What the machine said of it: its steps, its count, how far.
    pub progress: Option<Progress>,
}

/// The working bar: the dot, the words and what they are about; past a few
/// seconds the time so far and the moving bar. Once the machine says its
/// steps, they are listed under it and the bar fills by the share done.
pub fn working_bar(w: &Working, now: Instant) -> Element {
    let stepped = w.progress.as_ref().filter(|p| !p.job.steps.is_empty());
    let long = now.saturating_duration_since(w.since) >= SHOW_TIME_AFTER
        || w.opening
        || w.progress.is_some();
    let head = div()
        .layer("working head")
        .self_stretch()
        .flex_row()
        .gap(10)
        .items_center()
        .child(dot())
        .child(div().layer("working said").font_weight(600).font_size(15).text(w.said.clone()))
        .children((!w.about.is_empty()).then(|| {
            div()
                .layer("working about")
                .style("min-width", "0px")
                .style("white-space", "nowrap")
                .style("overflow", "hidden")
                .style("text-overflow", "ellipsis")
                .font_size(13)
                .color(palette::MUTED)
                .text(w.about.clone())
        }))
        .child(div().flex_1())
        .children(long.then(|| {
            div()
                .layer("working time")
                .font_mono()
                .font_size(12)
                .color(palette::DIM)
                .text(elapsed(w.since, now))
        }));
    let bar = match &w.progress {
        Some(p) => Some(share_bar(p.job.share)),
        None => long.then(moving_bar),
    };
    asking_bar("working bar", palette::BLUE_BORDER)
        .gap(10)
        .child(head)
        .children(bar)
        .children(stepped.map(|p| steps(p, now, false)))
        .children((w.opening && stepped.is_none()).then(|| {
            div().font_size(13).color(palette::MUTED).text(word("wait.opening_eval_line"))
        }))
}

/// A read's failure in words: `wait.could_not_read` with its reason.
pub fn could_not_read(why: &str) -> String {
    fill("wait.could_not_read", &[("why", why)])
}
