//! What sealed work offers: follow up, undo. Each has
//! one dialog; what an offer types goes into the work's own agent, where the
//! person can watch it, and Weft runs no Git itself.

use weft_core::work::{Status, UnitOfWork};

use super::*;

/// What sealed work is, for an offer on it.
struct Sealed {
    unit: UnitOfWork,
    /// Its Asks, oldest first, as the ledger has them.
    asks: Vec<weft_core::ledger::Unit>,
}

impl Sealed {
    fn root(&self) -> &weft_core::ledger::Unit {
        &self.asks[0]
    }

    fn newest(&self) -> &weft_core::ledger::Unit {
        self.asks.last().expect("a unit has an Ask")
    }
}

impl Session {
    /// An offer on sealed work, named by any of its Asks.
    pub(super) fn offer(&mut self, at: usize, unit: Option<&str>, commit: Option<&str>) -> Answer {
        match self.sealed(at, unit, "rejected") {
            Ok(sealed) => self.undo(at, &sealed, commit),
            Err(no) => no,
        }
    }

    /// Undo, as its dialog in the client picked: the work's agent puts the
    /// code back as it was at `commit`, one of `commits`, by default where
    /// the work started. Typed at once: the dialog was the confirmation.
    fn undo(&mut self, at: usize, sealed: &Sealed, commit: Option<&str>) -> Answer {
        let commits = match self.commits_of(at, sealed) {
            Ok(commits) => commits,
            Err(no) => return no,
        };
        let picked = match commit {
            None => commits.iter().find(|c| c.start),
            Some(id) => commits.iter().find(|c| c.id == id || c.short == id),
        };
        let Some(picked) = picked else {
            return Answer::No(
                "no_commit",
                "that commit is not one this work can go back to".into(),
            );
        };
        let pane = match self.pane_of(at, sealed.newest()) {
            Ok(pane) => pane,
            Err(no) => return no,
        };
        let request = weft_core::after_seal::undo_request(&sealed.root().title, &picked.id);
        let typing = Waiting {
            id: String::new(),
            pane,
            what: String::new(),
            why: String::new(),
            payload: request.into_bytes(),
            how: weft_core::inject::Handoff::Whole,
            sends: None,
            compile: None,
            eval: None,
            stop: None,
            go: None,
            hidden: true,
        };
        self.type_it(at, typing, false);
        Answer::nothing()
    }

    /// The commits Undo offers for rejected work, named by any of its Asks.
    pub(super) fn commits(&self, at: usize, ask: &str) -> Answer {
        let sealed = match self.sealed(at, Some(ask), "rejected") {
            Ok(sealed) => sealed,
            Err(no) => return no,
        };
        match self.commits_of(at, &sealed) {
            Ok(commits) => Answer::Ok(serde_json::json!({ "commits": commits })),
            Err(no) => no,
        }
    }

    /// The project's commits from the one the work's first Ask was sent on
    /// up to now, newest first. Weft only reads them.
    fn commits_of(
        &self,
        at: usize,
        sealed: &Sealed,
    ) -> Result<Vec<weft_core::after_seal::Commit>, Answer> {
        let Some(base) = sealed.root().base_commit.as_deref() else {
            let why = "RingFrame recorded no commit for this work's first Ask".to_string();
            return Err(Answer::No("no_commit", why));
        };
        let root = &self.projects[at].root;
        let format = format!("--format={}", weft_core::after_seal::LOG_FORMAT);
        let read = |args: &[&str]| crate::export::git(root, args);
        let log = read(&["log", "--first-parent", &format, &format!("{base}..HEAD")])
            .and_then(|later| Ok(later + &read(&["log", "-1", &format, base])?))
            .map_err(|e| Answer::No("no_commit", e))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        Ok(weft_core::after_seal::commits(&log, base, now, crate::turns::zone()))
    }

    /// A follow-up's words for rejected work: what its Eval found, with that
    /// Eval, its number and the work's title for the sheet to name.
    pub(super) fn remedy(&self, at: usize, ask: &str) -> Answer {
        let sealed = match self.sealed(at, Some(ask), "rejected") {
            Ok(sealed) => sealed,
            Err(no) => return no,
        };
        let ("remediates", eval) = weft_core::after_seal::follow_up_link(&sealed.unit) else {
            return Answer::No("not_offered", "no Eval judged this work".into());
        };
        let root = &self.projects[at].root;
        let found = crate::record::eval_view(root, eval).map(|v| v.findings).unwrap_or_default();
        let n = self.projects[at].ledger.record().evals.iter().find(|e| e.id == eval).map(|e| e.n);
        Answer::Ok(serde_json::json!({
            "words": weft_core::after_seal::remedy_words(&found),
            "eval": eval,
            "n": n,
            "title": sealed.root().title,
        }))
    }

    /// The work an Ask is in, when it was sealed as `decided`.
    fn sealed(&self, at: usize, ask: Option<&str>, decided: &str) -> Result<Sealed, Answer> {
        let units = self.projects[at].ledger.units();
        let work = weft_core::work::work(&units, &self.projects[at].ledger.record());
        let Some(unit) = ask.and_then(|a| work.of_ask(a)) else {
            return Err(Answer::No("no_unit", "that Ask is not on the board".into()));
        };
        if !matches!(&unit.status, Status::Sealed { decision } if decision == decided) {
            let why = format!("this offer is for work sealed as {decided}");
            return Err(Answer::No("not_offered", why));
        }
        let asks = unit
            .asks
            .iter()
            .filter_map(|id| units.iter().find(|u| &u.ask_id == id).cloned())
            .collect::<Vec<_>>();
        if asks.is_empty() {
            return Err(Answer::No("no_unit", "that work's Asks are not on the board".into()));
        }
        Ok(Sealed { unit: unit.clone(), asks })
    }

    /// The pane of the work's agent: the one running its newest Ask's
    /// session, else any of its harness.
    pub(super) fn pane_of(
        &mut self,
        at: usize,
        unit: &weft_core::ledger::Unit,
    ) -> Result<u32, Answer> {
        let map: Vec<_> = self.projects[at]
            .panes
            .iter_mut()
            .map(|s| {
                s.map.running = s.pane.running();
                s.map.clone()
            })
            .collect();
        crate::turns::owning_pane(&map, unit)
            .ok_or_else(|| Answer::No("no_pane", format!("no {} pane is open here", unit.harness)))
    }
}
