//! Details, one level each (ADR-0029 D3, T11.2–T11.6): a work, an Eval, a
//! Seal, a session, and the earlier sessions. Each blocks the screen; `Esc`
//! or `←` goes back to where it was opened from. What each says is
//! `weft_core::offers`'; this lays it out as the cards do.

use super::*;
use weft_core::offers::{self, EvalDetail, SealDetail, SessionDetail, WorkDetail};
use weft_core::sessions::Group;

/// What a Details view is about.
#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Work(String),
    Eval(String),
    Seal(String),
    Session(String),
    Earlier,
}

/// Where `[Enter]` on a link goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    View(View),
    /// A running session's own pane, by its id.
    Pane(u32),
}

/// One Details view, laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub view: View,
    /// The header: its breadcrumb, each part with the link it is, if any.
    pub title: String,
    pub crumbs: Vec<(String, Option<usize>)>,
    /// What the header says at its right.
    pub harness: String,
    /// The body. A tab splits a line into what is drawn at its left and what
    /// is drawn against its right edge.
    pub lines: Vec<String>,
    /// What `[Enter]` opens: on a line of the body, or in the header.
    pub links: Vec<(Option<usize>, Link)>,
    pub pick: usize,
    /// Whether the picked link is drawn picked: always in a list, and in a
    /// work's Details once there is more than one place to go.
    pub marked: bool,
    pub offset: usize,
    /// The earlier sessions' search, and whether it is being typed.
    pub search: Option<String>,
    pub typing: bool,
}

impl Detail {
    fn new(view: View, crumbs: Vec<(String, Option<usize>)>, harness: String) -> Self {
        let title = crumbs.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>().join("  ›  ");
        Detail {
            view,
            title,
            crumbs,
            harness,
            lines: Vec::new(),
            links: Vec::new(),
            pick: 0,
            marked: true,
            offset: 0,
            search: None,
            typing: false,
        }
    }

    fn line(&mut self, text: impl Into<String>) {
        self.lines.push(text.into());
    }

    /// A line `[Enter]` follows.
    fn link(&mut self, text: impl Into<String>, to: Link) {
        self.links.push((Some(self.lines.len()), to));
        self.lines.push(text.into());
    }

    /// The body line the picked link is on, if it is on one.
    pub fn picked_line(&self) -> Option<usize> {
        self.marked.then(|| self.links.get(self.pick).and_then(|(l, _)| *l)).flatten()
    }

    /// The header part the picked link is, if it is one.
    pub fn picked_crumb(&self) -> Option<usize> {
        let picked = self.links.get(self.pick).filter(|(l, _)| self.marked && l.is_none())?;
        let _ = picked;
        self.crumbs.iter().position(|(_, link)| *link == Some(self.pick))
    }
}

/// A title in a column `width` wide: cut with `…`, at least two spaces after.
fn column(text: &str, width: usize) -> String {
    let fits = width.saturating_sub(2);
    let mut out: String = if text.chars().count() > fits {
        text.chars().take(fits.saturating_sub(1)).chain(['…']).collect()
    } else {
        text.to_string()
    };
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

fn verdict(v: Option<&str>) -> String {
    v.map_or_else(
        || word("tree.judging").to_string(),
        |v| word(&format!("verdict.{v}")).to_string(),
    )
}

fn eval_n(n: usize) -> String {
    fill("tree.eval", &[("n", &n.to_string())])
}

fn asks_n(n: usize) -> String {
    if n == 1 { "1 Ask".into() } else { format!("{n} Asks") }
}

impl App {
    /// Open a Details view over whatever is in front, which `←` comes back to.
    pub(super) fn open_view(&mut self, view: View) {
        self.reading = None;
        match &view {
            View::Work(id) => {
                let Some(unit) = self.units.iter().find(|u| &u.ask_id == id).cloned() else {
                    return self.say(word("why.nothing_asked"));
                };
                let mut reads = vec![(Part::Source, "source")];
                if !unit.requested_only {
                    reads.push((Part::Prompt, "wording"));
                }
                self.read_parts(view.clone(), &reads, &unit.ask_id);
            }
            View::Seal(id) => self.read_parts(view.clone(), &[(Part::Seal, "seal")], &id.clone()),
            _ => {
                if let Some(d) = self.lay_out(&view, &Reading::default()) {
                    self.show(d);
                }
            }
        }
    }

    /// Test seam: open a view as if its reads had come back with these.
    pub fn open_view_read(
        &mut self,
        view: View,
        source: Option<&str>,
        prompt: Option<&str>,
        seal: Option<serde_json::Value>,
    ) {
        let r = Reading {
            view: Some(view.clone()),
            source: source.map(str::to_string),
            prompt: prompt.map(str::to_string),
            seal,
            left: 0,
        };
        if let Some(d) = self.lay_out(&view, &r) {
            self.show(d);
        }
    }

    fn read_parts(&mut self, view: View, reads: &[(Part, &str)], id: &str) {
        let mut left = 0;
        for (which, what) in reads {
            if let Ok(call) = sess!(self).read(what, id) {
                self.await_answer(call, Then::Read(*which));
                left += 1;
            }
        }
        self.reading = Some(Reading { view: Some(view), left, ..Reading::default() });
        if left == 0 {
            self.finish_reading();
        }
    }

    /// One part of a view, read; once every part is in, the view opens. A
    /// part that did not come back is said where it would have been.
    pub(super) fn read_part(
        &mut self,
        part: Part,
        answer: std::result::Result<serde_json::Value, String>,
    ) {
        let Some(r) = self.reading.as_mut() else { return };
        r.left = r.left.saturating_sub(1);
        if let Ok(v) = answer {
            let text = || v.get("text").and_then(|t| t.as_str()).map(str::to_string);
            match part {
                Part::Source => r.source = text(),
                Part::Prompt => r.prompt = text(),
                Part::Seal => r.seal = Some(v),
            }
        }
        if r.left == 0 {
            self.finish_reading();
        }
    }

    fn finish_reading(&mut self) {
        let Some(r) = self.reading.take() else { return };
        let Some(view) = r.view.clone() else { return };
        if let Some(d) = self.lay_out(&view, &r) {
            self.show(d);
        }
    }

    fn show(&mut self, d: Detail) {
        if let Some(open) = self.detail.take() {
            self.behind.push(open);
        }
        self.detail = Some(d);
    }

    /// Close every Details view, back to the tree.
    pub fn close_all_details(&mut self) {
        self.detail = None;
        self.behind.clear();
    }

    /// `←`: back to the view this one was opened from, else to the tree.
    pub(super) fn close_detail(&mut self) -> bool {
        match self.detail.take() {
            Some(_) => {
                self.detail = self.behind.pop();
                true
            }
            None => false,
        }
    }

    fn lay_out(&self, view: &View, r: &Reading) -> Option<Detail> {
        let s = &sess!(self);
        let record = s.record_of();
        let zone = s.zone;
        let now = self.now();
        Some(match view {
            View::Work(id) => {
                let unit = self.units.iter().find(|u| &u.ask_id == id)?;
                let session = unit.session_ref.as_deref().and_then(|id| self.agent_of(id));
                let d = offers::work_detail(
                    unit,
                    &record,
                    &s.rounds,
                    session,
                    r.source.as_deref(),
                    r.prompt.as_deref(),
                    zone,
                );
                work(view.clone(), &d, now, zone)
            }
            View::Eval(id) => {
                let e = record.evals.iter().find(|e| &e.id == id)?;
                let judges = self.record(id);
                let why = match s.rounds.eval(id) {
                    Some(round) => {
                        let units = self.units.clone();
                        let board = |u: Option<&str>| {
                            self.with_board_on(&units, u, |b| b.not_ready(Act::Seal))
                        };
                        weft_core::rounds::seal_why(round, &board)
                    }
                    None => weft_core::rounds::sealed_already(&record, id),
                };
                let d = offers::eval_detail(e, judges.as_ref(), &self.units, &record, why);
                eval(view.clone(), &d, zone)
            }
            View::Seal(id) => {
                let seal = record.seals.iter().find(|s| &s.id == id)?;
                let d = offers::seal_detail(seal, &self.units, &record, r.seal.as_ref());
                seal_view(view.clone(), &d, zone)
            }
            View::Session(id) => {
                let row = self.session_row(id)?;
                let d = offers::session_detail(row, &self.units, &record);
                session(view.clone(), &d, now, zone)
            }
            View::Earlier => earlier(&self.projects[self.at].name, &s.sessions, None, now, zone),
        })
    }

    /// A session as AGENTS reads it, for a work's Details.
    fn agent_of(&self, id: &str) -> Option<weft_core::tree::Agent> {
        let row = self.session_row(id)?;
        Some(weft_core::tree::Agent {
            session: row.id.clone(),
            harness: row.harness.clone(),
            state: row.state,
            turn: row.turn,
            title: row.title.clone(),
            at: row.last,
            asks: row.asks,
            pane: row.pane,
            ask: None,
        })
    }

    /// `↑` `↓` in a Details view: to the next link when it is in view, else
    /// one line of scrolling (T11.2 scrolls; T11.3–T11.6 move between rows).
    pub(super) fn move_in_detail(&mut self, delta: i32) {
        let room = self.detail_room;
        let reach = self.detail_reach;
        let Some(d) = self.detail.as_mut() else { return };
        let next = d.pick as i32 + delta;
        let target = (next >= 0).then(|| d.links.get(next as usize)).flatten();
        let in_view = |line: Option<usize>, offset: usize| match line {
            None => true,
            Some(l) => l >= offset && l < offset + room,
        };
        let moves = d.marked
            && target.is_some_and(|(line, _)| {
                // Down past what is shown scrolls first; up to the header waits
                // for the top.
                in_view(*line, d.offset) && (line.is_some() || d.offset == 0)
            });
        if moves {
            d.pick = next as usize;
            return;
        }
        let offset = (d.offset as i32 + delta).clamp(0, reach as i32) as usize;
        if offset != d.offset {
            d.offset = offset;
        } else if d.marked
            && let Some(_) = target
        {
            d.pick = next as usize;
        }
    }

    /// `[Enter]` in a Details view: where the picked link goes.
    pub(super) fn follow_link(&mut self) {
        let Some(d) = self.detail.as_ref() else { return };
        if d.typing {
            if let Some(d) = self.detail.as_mut() {
                d.typing = false;
            }
            return;
        }
        let Some((_, link)) = d.links.get(d.pick).cloned() else { return };
        match link {
            Link::View(View::Session(id)) => {
                let (pane, state) = match self.session_row(&id) {
                    Some(r) => (r.pane, r.state),
                    None => (None, State::Ended),
                };
                // A running session is its pane; one ended, its Details.
                self.go_to_session(&id, pane, state)
            }
            Link::View(view) => self.open_view(view),
            Link::Pane(p) => {
                if let Some(i) = sess!(self).index_of(p) {
                    self.detail = None;
                    self.behind.clear();
                    self.pane_focus = i;
                    self.focus = Focus::Agent;
                }
            }
        }
    }

    /// `[R]ESUME THIS` on a session's Details: asks first, naming the id
    /// (D4), and its last active time when Weft cannot tell whether it runs.
    pub(super) fn ask_to_resume(&mut self) -> bool {
        let Some(View::Session(id)) = self.detail.as_ref().map(|d| d.view.clone()) else {
            return false;
        };
        let Some(row) = self.session_row(&id).cloned() else { return true };
        if row.state == State::Running {
            return true;
        }
        if !row.resumable {
            self.say(word("resume.nothing"));
            return true;
        }
        let now = self.now();
        let quiet = (row.state == State::Quiet)
            .then(|| weft_core::sessions::when(row.last, now, sess!(self).zone));
        self.modal = Some(Modal::Bar(Bar::Resume { harness: row.harness, session: row.id, quiet }));
        self.modal_choice = 0;
        true
    }

    /// Resume that session in a new pane, by its id: the daemon runs the
    /// harness file's own resume line, and the row turns `●`.
    pub(super) fn resume(&mut self, harness: &str, id: &str) {
        self.modal = None;
        if let Err(e) = self.start(harness, "", Some(id), Then::Started { go: true }) {
            self.modal = Some(Modal::Note(format!("Could not resume {id}: {e}")));
        }
    }

    /// `/` and what follows it in the earlier sessions: the list narrows as
    /// the search is typed. True when the key was the search's.
    pub(super) fn search_key(&mut self, key: &KeyEvent) -> bool {
        use event::KeyCode;
        let Some(d) = self.detail.as_ref().filter(|d| d.view == View::Earlier) else {
            return false;
        };
        let (typing, mut q) = (d.typing, d.search.clone().unwrap_or_default());
        match key.code {
            KeyCode::Char('/') if !typing => {}
            KeyCode::Char(c) if typing => q.push(c),
            KeyCode::Backspace if typing => {
                q.pop();
            }
            KeyCode::Esc if typing => q.clear(),
            _ => return false,
        }
        let typing = !matches!(key.code, KeyCode::Esc);
        let now = self.now();
        let s = &sess!(self);
        let mut d = earlier(&self.projects[self.at].name, &s.sessions, Some(&q), now, s.zone);
        d.typing = typing;
        self.detail = Some(d);
        true
    }
}

/// T11.2.
fn work(view: View, d: &WorkDetail, now: i64, zone: i64) -> Detail {
    let mut crumbs = vec![(d.title.clone(), None)];
    let mut links = Vec::new();
    match &d.crumb.eval {
        Some(e) => {
            links.push((None, Link::View(View::Eval(e.eval.clone()))));
            crumbs.push((format!("{} {}", eval_n(e.n), verdict(e.verdict)), Some(links.len() - 1)));
            match &d.crumb.seal {
                Some(s) => {
                    links.push((None, Link::View(View::Seal(s.seal.clone()))));
                    crumbs.push((format!("Seal {}", s.n), Some(links.len() - 1)));
                }
                None if e.verdict.is_some() => crumbs.push((word("tree.not_sealed").into(), None)),
                None => {}
            }
        }
        None => match &d.crumb.seal {
            Some(s) => {
                links.push((None, Link::View(View::Seal(s.seal.clone()))));
                crumbs.push((format!("Seal {}", s.n), Some(links.len() - 1)));
            }
            None => crumbs.push(("not evaluated yet".into(), None)),
        },
    }
    let mut out = Detail::new(view, crumbs, d.harness.clone());
    out.links = links;
    out.line(format!("ASK [{}]", d.ask.state));
    out.line(format!("  {}", d.ask.aside));
    for l in &d.ask.lines {
        out.line(format!("  {l}"));
    }
    if let Some(s) = &d.session {
        out.line("  Session:");
        let to = match (s.state, s.pane) {
            (State::Running, Some(p)) => Link::Pane(p),
            _ => Link::View(View::Session(s.session.clone())),
        };
        out.link(
            format!("  {} · {}  ›", s.harness, weft_core::sessions::when(s.at, now, zone)),
            to,
        );
    }
    // The session first: it is the one place the body goes.
    out.pick = out.links.len().saturating_sub(1);
    out.marked = out.links.len() > 1;
    out
}

/// T11.3.
fn eval(view: View, d: &EvalDetail, zone: i64) -> Detail {
    let state = match (&d.sealed, d.judging) {
        (Some(s), _) => format!("Seal {}", s.n),
        (None, true) => String::new(),
        (None, false) => word("tree.not_sealed").into(),
    };
    let mut crumbs = vec![(eval_n(d.n), None), (verdict(d.verdict), None)];
    if !state.is_empty() {
        crumbs.push((state, None));
    }
    let host = d.judged_by.as_deref().and_then(|b| b.split(" · ").next()).unwrap_or("");
    let mut out = Detail::new(view, crumbs, host.to_string());
    let head = match (&d.verdict, &d.agreement) {
        (Some(v), Some(a)) => {
            format!("{}   {} · {a} agreed", eval_n(d.n).to_uppercase(), verdict(Some(v)))
        }
        _ => format!("{}   {}", eval_n(d.n).to_uppercase(), verdict(d.verdict)),
    };
    out.line(head);
    let covers = asks_n(d.asks.len());
    match d.at {
        Some(at) => out.line(format!("  judged {} · covers {covers}", offers::stamp(at, zone))),
        None => out.line(format!("  covers {covers}")),
    }
    if !d.judging {
        out.line(format!("  VERDICT     {}", verdict(d.verdict)));
        out.line(format!("  AGREEMENT   {}", d.agreement.as_deref().unwrap_or("?")));
        if let Some(by) = &d.judged_by {
            out.line(format!("  JUDGED BY   {by}"));
        }
    }
    if !d.findings.is_empty() {
        out.line("FINDINGS");
        for f in &d.findings {
            out.line(format!("  {} {}", f.mark, f.text));
        }
    }
    if !d.checks.is_empty() {
        out.line("CHECKS");
        for c in &d.checks {
            let result = c.result.as_deref().unwrap_or("");
            out.line(format!("  {} {}{result}", c.mark, column(&c.text, 20)));
        }
    }
    out.line(format!("ASKS IT COVERS  {}", d.asks.len()));
    for a in &d.asks {
        out.link(
            format!("  {}{}›", column(&a.title, 22), column(&a.harness, 9)),
            Link::View(View::Work(a.unit.clone())),
        );
    }
    match &d.sealed {
        Some(s) => {
            out.link(format!("Sealed by Seal {}  ›", s.n), Link::View(View::Seal(s.seal.clone())))
        }
        None if !d.judging => out.line("Not sealed — [S]EAL"),
        None => {}
    }
    out.line("  A check is a judgement, not a guarantee.");
    out
}

/// T11.4.
fn seal_view(view: View, d: &SealDetail, zone: i64) -> Detail {
    let crumbs = vec![(format!("Seal {}", d.n), None), (d.decision.clone(), None)];
    let mut out = Detail::new(view, crumbs, String::new());
    let when = d.at.map(|at| offers::stamp(at, zone)).unwrap_or_default();
    out.line(format!("SEAL {}   {}", d.n, d.decision));
    out.line(format!("  sealed {when} by {} · covers {}", d.by, asks_n(d.asks.len())));
    out.line(format!("  DECISION    {}", d.decision));
    out.line(format!("  WHEN        {when}"));
    out.line(format!("  BY          {}", d.by));
    match &d.eval {
        Some(e) => {
            let agreement = e.agreement.as_deref().map(|a| format!(" · {a}")).unwrap_or_default();
            out.link(
                format!("  SEALS       {} · {}{agreement}  ›", eval_n(e.n), verdict(e.verdict)),
                Link::View(View::Eval(e.eval.clone())),
            );
        }
        None => out.line("  SEALS       none — sealed without an Eval"),
    }
    out.line(format!("  RECEIPT     {}", d.receipt));
    match d.holds {
        Some(true) => out.line("  The receipt still matches what RingFrame wrote."),
        Some(false) => {
            out.line("  THE RECEIPT NO LONGER HOLDS");
            for b in &d.broken {
                out.line(format!("  {b}"));
            }
        }
        None => out.line("  RingFrame did not say whether the receipt still holds."),
    }
    out.line(format!("ASKS IT COVERED  {}", d.asks.len()));
    for a in &d.asks {
        out.link(
            format!("  {}{}›", column(&a.title, 22), column(&a.harness, 9)),
            Link::View(View::Work(a.unit.clone())),
        );
    }
    out.line("  A Seal records what was decided. It does not make the work right.");
    out
}

/// T11.5.
fn session(view: View, d: &SessionDetail, now: i64, zone: i64) -> Detail {
    let when = |at| weft_core::sessions::when(at, now, zone);
    let state = match (d.state, d.turn) {
        (State::Running, Some(t)) => format!("{} {}", t.plain(), when(d.last)),
        (State::Running, None) => format!("{} {}", word("state.running"), when(d.last)),
        (State::Ended, _) => format!("{} {}", word("state.ended"), when(d.last)),
        (State::Quiet, _) => fill("state.last_active", &[("time", &when(d.last))]),
    };
    let short = format!("s {}", weft_core::sessions::short(&d.session));
    let crumbs = vec![(d.harness.clone(), None), (state.clone(), None)];
    let mut out = Detail::new(view, crumbs, short.clone());
    let glyph = if d.state == State::Running { "●" } else { "○" };
    let shown =
        if d.state == State::Ended { word("state.ended").to_string() } else { state.clone() };
    out.line(format!("SESSION  {glyph} {shown}"));
    out.line(format!(
        "  {} ran {} – {} · {}",
        d.harness,
        when(d.started),
        when(d.last),
        asks_n(d.asks.len())
    ));
    out.line(format!("  HARNESS      {}", d.harness));
    out.line(format!("  STATE        {state}"));
    out.line(format!("  STARTED      {}", when(d.started)));
    out.line(format!("  SESSION ID   {short}"));
    out.line(format!("ASKS MADE IN IT  {}", d.asks.len()));
    for a in &d.asks {
        let at = match (a.seal, a.eval) {
            (Some(n), _) => format!("  Seal {n}"),
            (None, Some(n)) => format!("  {}", eval_n(n)),
            (None, None) => String::new(),
        };
        out.link(
            format!("  {}{}{at}  ›", column(&a.title, 22), a.word.say()),
            Link::View(View::Work(a.unit.clone())),
        );
    }
    match (d.state, d.pane, d.resume) {
        (State::Running, Some(p), _) => {
            out.link("[Enter] GO TO ITS PANE".to_string(), Link::Pane(p));
            out.pick = out.links.len() - 1;
        }
        (State::Running, None, _) => out.line(fill("why.elsewhere", &[("harness", &d.harness)])),
        (_, _, true) => out.line(format!(
            "[R]ESUME THIS starts {} again on session {short}. Weft asks first.",
            d.harness
        )),
        (_, _, false) => out.line(word("resume.nothing")),
    }
    out
}

/// T11.6: every ended session past the tree's, by day, as the search narrows.
fn earlier(
    project: &str,
    rows: &[weft_core::sessions::Row],
    q: Option<&str>,
    now: i64,
    zone: i64,
) -> Detail {
    let past = weft_core::sessions::split(rows).1;
    let found: Vec<_> =
        past.iter().filter(|r| q.is_none_or(|q| weft_core::sessions::matches(r, q))).collect();
    let crumbs = vec![(word("earlier.title").to_string(), None), (project.to_string(), None)];
    let mut out = Detail::new(View::Earlier, crumbs, found.len().to_string());
    if let Some(q) = q {
        out.line(format!("SEARCH  {q}"));
    }
    let harness = found.iter().map(|r| r.harness.chars().count()).max().unwrap_or(0) + 2;
    for group in [Group::Today, Group::Yesterday, Group::Week, Group::Earlier] {
        let mine: Vec<_> = found.iter().filter(|r| Group::of(r.last, now, zone) == group).collect();
        if mine.is_empty() {
            continue;
        }
        out.line(format!("{}  {}", word(&format!("earlier.{}", group_key(group))), mine.len()));
        for r in mine {
            let title = r.title.clone().unwrap_or_else(|| word("tree.no_ask").to_string());
            out.link(
                format!(
                    "  ○ {}{}{}s {}\t{}",
                    column(&r.harness, harness),
                    column(&title, 27),
                    column(&asks_n(r.asks), 12),
                    weft_core::sessions::short(&r.id),
                    weft_core::sessions::when(r.last, now, zone)
                ),
                Link::View(View::Session(r.id.clone())),
            );
        }
    }
    out.search = q.map(str::to_string);
    out
}

fn group_key(g: Group) -> &'static str {
    match g {
        Group::Today => "today",
        Group::Yesterday => "yesterday",
        Group::Week => "week",
        Group::Earlier => "earlier",
    }
}
