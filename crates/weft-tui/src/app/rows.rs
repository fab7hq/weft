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
    /// One session; `width` is its project's harness column.
    Agent {
        project: usize,
        agent: Agent,
        width: usize,
    },
    /// `▸ n earlier sessions`.
    Earlier {
        project: usize,
        n: usize,
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
            _ => None,
        }
    }

    pub fn folded(&self) -> bool {
        match self {
            Row::Project { folded, .. } | Row::Open { folded, .. } | Row::Sealed { folded, .. } => {
                *folded
            }
            Row::Eval { group, .. } => group.folded,
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
        let folded = |at: usize, fold: Fold| match fold {
            Fold::Project => self.folded.contains(&self.projects[at].name),
            Fold::Open => self.folded.contains(&format!("{at}\u{0}open")),
            Fold::Eval(id) => self.folded.contains(&format!("{at}\u{0}{id}")),
            Fold::Sealed => !self.folded.contains(&format!("{at}\u{0}sealed")),
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
            });
            if p.folded {
                continue;
            }
            let a = p.agents;
            if !a.rows.is_empty() || a.earlier > 0 {
                out.push(Row::Gap { project: at });
                out.push(Row::Agents { project: at, running: a.running, ended: a.ended });
                let width = a.rows.iter().map(|r| cells(&r.harness)).max().unwrap_or(0);
                out.extend(a.rows.into_iter().map(|agent| Row::Agent {
                    project: at,
                    agent,
                    width,
                }));
                if a.earlier > 0 {
                    out.push(Row::Earlier { project: at, n: a.earlier });
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
        out
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
            Row::Earlier { .. } => self.open_view(View::Earlier),
            Row::Agent { agent, .. } => self.go_to_session(&agent.session, agent.pane, agent.state),
            Row::Gap { .. } | Row::Agents { .. } | Row::Work { .. } => {}
        }
    }

    /// A session: its pane while one holds it; one another program runs says
    /// so; an ended or quiet one opens its Details (T11.5).
    pub(super) fn go_to_session(&mut self, id: &str, pane: Option<u32>, state: State) {
        match (state, pane.and_then(|p| sess!(self).index_of(p))) {
            (State::Running, Some(i)) => {
                self.detail = None;
                self.behind.clear();
                self.pane_focus = i;
                self.focus = Focus::Agent;
            }
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
        // Sealed is held open by its key; everything else is held folded.
        let present = if matches!(row, Row::Sealed { .. }) { !folded } else { folded };
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
                    self.modal = Some(Modal::Ask { text: String::new(), target, follows: None });
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
