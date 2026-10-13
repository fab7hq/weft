//! The tree (T11.1): a project, then AGENTS and WORK.
//!
//! The rows are `weft_core::tree`'s, laid out in the order the card draws
//! them, and built again from the daemon's facts each frame: there is one
//! source of truth and nothing to keep in step.

use super::*;
use weft_core::tree::{self, Agent, Fold, Group, SealRow, Why};

/// A row of the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Project {
        project: usize,
        name: String,
        folded: bool,
        /// The Open round's Asks.
        open: usize,
        /// Agents waiting for the person.
        waiting: usize,
        /// While a search narrows the tree: how many rows match here.
        matches: Option<usize>,
    },
    /// The empty line between sections: drawn, never selected.
    Gap {
        project: usize,
    },
    /// AGENTS, with how many run and how many have ended.
    Agents {
        project: usize,
        running: usize,
        ended: usize,
    },
    /// One session; `width` is its project's harness column, and `earlier`
    /// says it is listed under `▾ n earlier sessions`.
    Agent {
        project: usize,
        agent: Agent,
        width: usize,
        earlier: bool,
    },
    /// `▸ n earlier sessions`, and `▾` with them listed below.
    Earlier {
        project: usize,
        n: usize,
        open: bool,
    },
    Work {
        project: usize,
    },
    Open {
        project: usize,
        since: Option<usize>,
        count: usize,
        folded: bool,
        eval: Option<Why>,
    },
    Eval {
        project: usize,
        group: Group,
    },
    Sealed {
        project: usize,
        seals: usize,
        asks: usize,
        folded: bool,
    },
    Seal {
        project: usize,
        seal: SealRow,
    },
    /// An Ask in a round: `unit` is where it is on its project's board, and
    /// `column` where its harness starts.
    Ask {
        project: usize,
        unit: usize,
        ask: tree::Ask,
        column: usize,
    },
}

impl Row {
    /// Which project this row belongs to. Moving the selection moves the
    /// focus with it, so an act always lands in the project you are looking
    /// at and never in the one you were.
    pub fn project(&self) -> usize {
        match self {
            Row::Project { project, .. }
            | Row::Gap { project }
            | Row::Agents { project, .. }
            | Row::Agent { project, .. }
            | Row::Earlier { project, .. }
            | Row::Work { project }
            | Row::Open { project, .. }
            | Row::Eval { project, .. }
            | Row::Sealed { project, .. }
            | Row::Seal { project, .. }
            | Row::Ask { project, .. } => *project,
        }
    }

    /// Whether the arrows stop on it: a heading or a gap is only read.
    pub fn selectable(&self) -> bool {
        !matches!(self, Row::Gap { .. } | Row::Agents { .. } | Row::Work { .. })
    }

    /// The key this row folds under, or `None` for a leaf. Sealed is kept
    /// under its key while it is open, as it starts folded.
    pub fn fold_key(&self) -> Option<String> {
        match self {
            Row::Project { name, .. } => Some(name.clone()),
            Row::Open { project, .. } => Some(format!("{project}\u{0}open")),
            Row::Eval { project, group } => Some(format!("{project}\u{0}{}", group.eval)),
            Row::Sealed { project, .. } => Some(format!("{project}\u{0}sealed")),
            Row::Earlier { project, .. } => Some(format!("{project}\u{0}earlier")),
            _ => None,
        }
    }

    pub fn folded(&self) -> bool {
        match self {
            Row::Project { folded, .. } | Row::Open { folded, .. } | Row::Sealed { folded, .. } => {
                *folded
            }
            Row::Eval { group, .. } => group.folded,
            Row::Earlier { open, .. } => !open,
            _ => false,
        }
    }
}

impl App {
    /// The sidebar, top to bottom, with folded levels' children left out.
    pub(super) fn list(&self) -> Vec<Row> {
        let facts: Vec<_> = self.projects.iter().map(|p| (p, facts_of(&p.session))).collect();
        let ready = |h: &str| self.readiness(h);
        let projects: Vec<_> = facts
            .iter()
            .enumerate()
            .map(|(at, (p, (panes, agents)))| tree::Facts {
                name: &p.name,
                units: self.units_of(at),
                panes,
                agents,
                sessions: &p.session.sessions,
                rounds: &p.session.rounds,
                readiness: &ready,
                routing: &self.routing,
                setup: p.session.needs,
            })
            .collect();
        // A search shows what matches wherever it sits: nothing is folded.
        let search = self.tree_search.as_deref().filter(|q| !q.is_empty());
        let folded = |at: usize, fold: Fold| {
            search.is_none()
                && match fold {
                    Fold::Project => self.folded.contains(&self.projects[at].name),
                    Fold::Open => self.folded.contains(&format!("{at}\u{0}open")),
                    Fold::Eval(id) => self.folded.contains(&format!("{at}\u{0}{id}")),
                    Fold::Sealed => !self.folded.contains(&format!("{at}\u{0}sealed")),
                }
        };
        let mut out = Vec::new();
        for (at, p) in tree::rows(&projects, &folded).into_iter().enumerate() {
            if at > 0 {
                out.push(Row::Gap { project: at });
            }
            out.push(Row::Project {
                project: at,
                name: p.name,
                folded: p.folded,
                open: p.open,
                waiting: p.needs,
                matches: None,
            });
            if p.folded {
                continue;
            }
            let a = p.agents;
            if !a.rows.is_empty() || a.earlier > 0 {
                out.push(Row::Gap { project: at });
                out.push(Row::Agents { project: at, running: a.running, ended: a.ended });
                // The earlier sessions are listed under their row once it is
                // open, and wherever they match a search.
                let open = self.folded.contains(&format!("{at}\u{0}earlier"));
                let more: Vec<tree::Agent> = if a.earlier > 0 && (open || search.is_some()) {
                    weft_core::sessions::split(&self.projects[at].session.sessions)
                        .1
                        .iter()
                        .map(|s| tree::agent(&projects[at], s))
                        .collect()
                } else {
                    Vec::new()
                };
                let width =
                    a.rows.iter().chain(&more).map(|r| cells(&r.harness)).max().unwrap_or(0);
                out.extend(a.rows.into_iter().map(|agent| Row::Agent {
                    project: at,
                    agent,
                    width,
                    earlier: false,
                }));
                if a.earlier > 0 {
                    if search.is_none() {
                        out.push(Row::Earlier { project: at, n: a.earlier, open });
                    }
                    out.extend(more.into_iter().map(|agent| Row::Agent {
                        project: at,
                        agent,
                        width,
                        earlier: search.is_none(),
                    }));
                }
            }
            let units = self.units_of(at);
            if units.is_empty() {
                continue;
            }
            out.push(Row::Gap { project: at });
            out.push(Row::Work { project: at });
            let w = p.work;
            let asks = w.open.asks.iter().chain(w.evals.iter().flat_map(|g| &g.asks));
            let column = asks.map(|a| cells(&a.title)).max().unwrap_or(0) + 3;
            let ask = |a: tree::Ask| Row::Ask {
                project: at,
                unit: units.iter().position(|u| u.ask_id == a.unit).unwrap_or(usize::MAX),
                ask: a,
                column,
            };
            let (judging, judged): (Vec<Group>, Vec<Group>) =
                w.evals.into_iter().partition(|g| g.judging);
            let group = |out: &mut Vec<Row>, mut g: Group| {
                let asks = std::mem::take(&mut g.asks);
                out.push(Row::Eval { project: at, group: g });
                out.extend(asks.into_iter().map(ask));
            };
            for g in judging {
                group(&mut out, g);
            }
            out.push(Row::Open {
                project: at,
                since: w.open.since,
                count: w.open.count,
                folded: w.open.folded,
                eval: w.open.eval,
            });
            out.extend(w.open.asks.into_iter().map(ask));
            for g in judged {
                group(&mut out, g);
            }
            if w.sealed.seals > 0 {
                out.push(Row::Sealed {
                    project: at,
                    seals: w.sealed.seals,
                    asks: w.sealed.asks,
                    folded: w.sealed.folded,
                });
                out.extend(w.sealed.rows.into_iter().map(|seal| Row::Seal { project: at, seal }));
            }
        }
        match search {
            Some(q) => narrowed(out, q),
            None => out,
        }
    }

    /// The unit the selected row is about, if it is an Ask; or the work
    /// whose Details are open.
    pub fn selected_index(&self) -> Option<usize> {
        if let Some(View::Work(id)) = self.detail.as_ref().map(|d| &d.view) {
            return self.units.iter().position(|u| &u.ask_id == id);
        }
        match self.rows().get(self.selected) {
            Some(Row::Ask { unit, project, .. }) if *project == self.at => Some(*unit),
            _ => None,
        }
    }

    pub fn selected_unit(&self) -> Option<&Unit> {
        self.selected_index().and_then(|i| self.units.get(i))
    }

    pub(super) fn clamp_selection(&mut self) {
        let rows = self.rows();
        self.selected = self.selected.min(rows.len().saturating_sub(1));
        // Land on an agent or a work, not on the project heading above it.
        // Only while nothing has been picked, so it never overrides a choice.
        let lands = |r: &Row| matches!(r, Row::Agent { .. } | Row::Ask { .. });
        if self.selected == 0
            && !self.landed
            && let Some(first) = rows.iter().position(lands)
        {
            self.selected = first;
        }
        if rows.get(self.selected).is_some_and(|r| !r.selectable())
            && let Some(next) = rows.iter().skip(self.selected).position(Row::selectable)
        {
            self.selected += next;
        }
    }

    /// Test seam: select the first row `pick` says yes to, as the arrows
    /// would arrive at it.
    pub fn select(&mut self, pick: impl Fn(&Row) -> bool) -> bool {
        let Some(at) = self.rows().iter().position(pick) else { return false };
        self.landed = true;
        self.select_row(at);
        true
    }

    /// Move the selection, past the rows that are only read. It wraps.
    pub(super) fn pick(&mut self, delta: i8) {
        let rows = self.rows();
        let n = rows.len() as i32;
        let mut at = self.selected as i32;
        for _ in 0..n {
            at = (at + delta as i32).rem_euclid(n.max(1));
            if rows.get(at as usize).is_some_and(Row::selectable) {
                return self.select_row(at as usize);
            }
        }
    }

    /// `[Enter]`: unfold or fold a group, go to a running agent, or open what
    /// a row is about in its Details.
    pub(super) fn open_row(&mut self) {
        let Some(row) = self.rows().get(self.selected).cloned() else {
            self.say(word("why.nothing_asked"));
            return;
        };
        match row {
            Row::Project { .. } | Row::Open { .. } | Row::Sealed { .. } => {
                self.set_fold(&row, !row.folded())
            }
            Row::Eval { group, .. } => self.open_view(View::Eval(group.eval)),
            Row::Seal { seal, .. } => self.open_view(View::Seal(seal.seal)),
            Row::Ask { ask, .. } => self.open_view(View::Work(ask.unit)),
            Row::Earlier { open, .. } => self.set_fold(&row, open),
            Row::Agent { agent, .. } => self.go_to_session(&agent.session, agent.pane, agent.state),
            Row::Gap { .. } | Row::Agents { .. } | Row::Work { .. } => {}
        }
    }

    /// A session: its pane while one holds it; one that runs with no pane of
    /// Weft's opens its Details when its harness's resume joins it there
    /// (Weft restarted, the harness's own server still holding it), and
    /// otherwise says another program runs it; an ended or quiet one opens
    /// its Details.
    pub(super) fn go_to_session(&mut self, id: &str, pane: Option<u32>, state: State) {
        let joins = self.session_row(id).is_some_and(|r| self.joins(r));
        match (state, pane.and_then(|p| sess!(self).index_of(p))) {
            (State::Running, Some(i)) => {
                self.detail = None;
                self.behind.clear();
                self.pane_focus = i;
                self.focus = Focus::Agent;
            }
            (State::Running, None) if joins => self.open_view(View::Session(id.to_string())),
            (State::Running, None) => {
                let harness = self.session_row(id).map(|r| r.harness.clone()).unwrap_or_default();
                self.say(Why::of("why.elsewhere", &harness).say());
            }
            _ => self.open_view(View::Session(id.to_string())),
        }
    }

    /// One session of the focused project, as the index holds it.
    pub fn session_row(&self, id: &str) -> Option<&weft_core::sessions::Row> {
        sess!(self).sessions.iter().find(|r| r.id == id)
    }

    /// `[→]` and `[←]` unfold and fold without ever activating, which is what
    /// a person wants when they are only looking.
    pub(super) fn set_fold(&mut self, row: &Row, folded: bool) {
        let Some(key) = row.fold_key() else { return };
        // Sealed and the earlier sessions are held open by their keys;
        // everything else is held folded.
        let present =
            if matches!(row, Row::Sealed { .. } | Row::Earlier { .. }) { !folded } else { folded };
        if present {
            self.folded.insert(key);
        } else {
            self.folded.remove(&key);
        }
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
    }

    pub(super) fn fold_selected(&mut self, folded: bool) -> bool {
        let Some(row) = self.rows().get(self.selected).cloned() else { return false };
        if row.fold_key().is_none() || row.folded() == folded {
            return false;
        }
        self.set_fold(&row, folded);
        true
    }

    /// `[A]`, `[E]` and `[S]` act where they apply: Ask on an
    /// agent, Eval on Open, Seal on an Eval. Pressed anywhere else, each says
    /// what to select; one that cannot run says why.
    pub(super) fn act_on_row(&mut self, act: Act) {
        let row = self.rows().get(self.selected).cloned();
        let eval_in_view = match self.detail.as_ref().map(|d| &d.view) {
            Some(View::Eval(id)) => Some(id.clone()),
            _ => None,
        };
        match (act, row) {
            // The project's own Ask goes where it routes, or to the agent in
            // front, as the Ask sheet lets the person pick (T4.1, T4.3).
            (Act::Ask, None | Some(Row::Project { .. })) if self.detail.is_none() => {
                if self.guard(Act::Ask) {
                    self.start_ask(None);
                }
            }
            (Act::Ask, Some(Row::Agent { agent, .. })) if self.detail.is_none() => {
                if let Some(why) = agent.ask {
                    return self.say(why.say());
                }
                let target = agent.pane.and_then(|p| sess!(self).index_of(p));
                if let Some(target) = target
                    && self.guard(Act::Ask)
                {
                    let target = Dest::Pane(target);
                    self.modal = Some(Modal::Ask { text: String::new(), target, follows: None });
                    self.look_for_agents(Opening::Nothing);
                }
            }
            (Act::Eval, Some(Row::Open { eval, .. })) if self.detail.is_none() => match eval {
                Some(why) => self.say(why.say()),
                None => self.ask_first("check", None, None, None, None),
            },
            (Act::Seal, _) if eval_in_view.is_some() => {
                let id = eval_in_view.unwrap_or_default();
                self.seal(&id);
            }
            (Act::Seal, Some(Row::Eval { group, .. })) if self.detail.is_none() => {
                self.seal(&group.eval)
            }
            (act, _) => {
                if let Some(key) = tree::select(act) {
                    self.say(word(key));
                }
            }
        }
    }

    /// Seal an Eval's round, or say why it cannot be.
    fn seal(&mut self, eval: &str) {
        let r = &sess!(self).rounds;
        let why = match r.eval(eval) {
            Some(round) => {
                let units = self.units.clone();
                let board =
                    |u: Option<&str>| self.with_board_on(&units, u, |b| b.not_ready(Act::Seal));
                weft_core::rounds::seal_why(round, &board)
            }
            None => self.sealed_already(eval),
        };
        match why {
            Some(why) => self.say(why.say()),
            None => self.ask_first("decide", None, None, None, Some(eval)),
        }
    }

    /// An Eval no round holds: sealed already, when a Seal names it.
    fn sealed_already(&self, eval: &str) -> Option<Why> {
        let r = &sess!(self).rounds;
        let n = r.seals.iter().find_map(|s| s.eval.as_ref().filter(|e| e.id == eval).map(|e| e.n));
        n.map(|n| Why {
            key: "why.sealed_already",
            harness: None,
            title: Some(fill("tree.eval", &[("n", &n.to_string())])),
        })
    }
}

/// How many cells a text takes on screen.
fn cells(s: &str) -> usize {
    s.chars().count()
}

/// The rows a search leaves: in each project, the agents and works that hold
/// `q`, under the headings they sit under, and the project with how many.
/// A project with none goes.
fn narrowed(rows: Vec<Row>, q: &str) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    let mut at = 0;
    while at < rows.len() {
        let end = rows[at + 1..]
            .iter()
            .position(|r| matches!(r, Row::Project { .. }))
            .map_or(rows.len(), |n| at + 1 + n);
        let (mut found, mut section, mut groups): (Vec<Row>, Option<Row>, Vec<Row>) =
            (Vec::new(), None, Vec::new());
        let mut count = 0;
        for row in &rows[at + 1..end] {
            match row {
                Row::Agents { .. } | Row::Work { .. } => {
                    section = Some(row.clone());
                    groups.clear();
                }
                Row::Open { .. } | Row::Eval { .. } | Row::Sealed { .. } => {
                    groups = vec![row.clone()]
                }
                Row::Seal { .. } => {
                    groups.truncate(1);
                    groups.push(row.clone());
                }
                Row::Agent { agent, .. } if tree::agent_matches(agent, q) => {
                    found.extend(section.take());
                    found.push(row.clone());
                    count += 1;
                }
                Row::Ask { ask, .. } if tree::ask_matches(ask, q) => {
                    if let Some(s) = section.take() {
                        if !found.is_empty() {
                            found.push(Row::Gap { project: row.project() });
                        }
                        found.push(s);
                    }
                    found.append(&mut groups);
                    found.push(row.clone());
                    count += 1;
                }
                _ => {}
            }
        }
        if count > 0 {
            if !out.is_empty() {
                out.push(Row::Gap { project: rows[at].project() });
            }
            if let Row::Project { project, name, folded, open, waiting, .. } = rows[at].clone() {
                out.push(Row::Project {
                    project,
                    name,
                    folded,
                    open,
                    waiting,
                    matches: Some(count),
                });
            }
            out.extend(found);
        }
        at = end;
    }
    out
}

impl App {
    /// The search over the tree: `/` opens it, what is typed narrows the rows,
    /// `Esc` clears it. The arrows and `Enter` are the tree's own. True when
    /// the key was the search's.
    pub(super) fn tree_search_key(&mut self, key: &KeyEvent) -> bool {
        use event::KeyCode;
        if self.detail.is_some() || self.eval.is_some() {
            return false;
        }
        let plain =
            !key.modifiers.intersects(event::KeyModifiers::CONTROL | event::KeyModifiers::ALT);
        match (&mut self.tree_search, key.code) {
            (None, KeyCode::Char('/')) if plain => self.tree_search = Some(String::new()),
            (Some(q), KeyCode::Char(c)) if plain => q.push(c),
            (Some(q), KeyCode::Backspace) => {
                q.pop();
            }
            (Some(_), KeyCode::Esc) => self.tree_search = None,
            _ => return false,
        }
        // The first match, not wherever the selection was in the longer list.
        if self.tree_search.as_deref().is_some_and(|q| !q.is_empty()) {
            let rows = self.rows();
            let lands = |r: &Row| matches!(r, Row::Agent { .. } | Row::Ask { .. });
            self.selected = rows.iter().position(lands).unwrap_or(0);
            self.follow_the_selection();
        }
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
        true
    }

    /// The tree's search, while one is open.
    pub fn tree_search(&self) -> Option<&str> {
        self.tree_search.as_deref()
    }

    /// Whether the selected row is an agent: for tests of where a search lands.
    #[cfg(test)]
    pub fn selected_row_is_agent(&self) -> bool {
        matches!(self.rows().get(self.selected), Some(Row::Agent { .. }))
    }
}
