//! What sealed work offers: keeping a rule from it. Each offer has its own
//! dialog; what an offer types goes into the work's own agent, where the
//! person can watch it, and Weft runs no Git itself.

use weft_core::after_seal::Drafted;
use weft_core::work::{Status, UnitOfWork};

use super::*;

/// Where a person's own practice rules are kept: the base domain's catalog
/// in the machine's `[ringframe]` layer.
const MINE: [&str; 4] = ["ringframe", "deltas", "practices/software-development", "entries"];

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
    pub(super) fn offer(
        &mut self,
        (client, call_id): (u64, u64),
        at: usize,
        act: &str,
        (unit, draft): (Option<&str>, Option<&weft_core::ask::Draft>),
    ) -> Answer {
        let decided = if act == "undo" { "rejected" } else { "accepted" };
        let sealed = match self.sealed(at, unit, decided) {
            Ok(sealed) => sealed,
            Err(no) => return no,
        };
        match act {
            "draft_rule" => self.draft_rule(at, &sealed),
            "keep_rule" => self.keep_rule((client, call_id), at, &sealed, draft),
            "commit" => self.commit(at, &sealed),
            _ => self.undo(at, &sealed),
        }
    }

    /// Commit's dialog: the words typed into the work's agent, which writes
    /// the commit itself.
    fn commit(&mut self, at: usize, sealed: &Sealed) -> Answer {
        let harness = self.title_of(at, &sealed.newest().harness);
        let typed = weft_core::after_seal::commit_request();
        self.stage_typing(
            at,
            sealed,
            typed,
            weft_core::after_seal::committing(&harness),
            (None, None),
        )
    }

    /// Undo's dialog: the work back as it was at the commit its first Ask
    /// was sent on, typed into its agent; Send in amber.
    fn undo(&mut self, at: usize, sealed: &Sealed) -> Answer {
        let Some(base) = sealed.root().base_commit.clone() else {
            let why = "RingFrame recorded no commit for this work's first Ask".to_string();
            return Answer::No("no_commit", why);
        };
        let harness = self.title_of(at, &sealed.newest().harness);
        let typed = weft_core::after_seal::undo_request(&sealed.root().title, &base);
        let go = weft_proto::Go { word: "act.send".into(), warn: true };
        self.stage_typing(
            at,
            sealed,
            typed,
            weft_core::after_seal::undoing(&harness),
            (Some(go), None),
        )
    }

    /// A dialog whose Send types `typed` into the work's agent, with its own
    /// yes button and the draft it asks for, when it has them.
    fn stage_typing(
        &mut self,
        at: usize,
        sealed: &Sealed,
        typed: String,
        asking: weft_core::offers::Asking,
        (go, drafts): (Option<weft_proto::Go>, Option<crate::acts::Drafting>),
    ) -> Answer {
        let pane = match self.pane_of(at, sealed.newest()) {
            Ok(pane) => pane,
            Err(no) => return no,
        };
        let built = crate::acts::Built {
            payload: typed.into_bytes(),
            how: weft_core::inject::Handoff::Whole,
            asking,
            ask_id: None,
            compile: None,
            eval: None,
            stop: None,
            go,
            drafts,
        };
        self.stage(at, pane, built)
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

    fn title_of(&self, at: usize, harness: &str) -> String {
        self.projects[at].harnesses.find(harness).map_or(harness, |h| &h.title).to_string()
    }

    /// Dialog one of Keep a rule: the request that has the work's agent
    /// draft one, from what was asked and the follow-ups.
    fn draft_rule(&mut self, at: usize, sealed: &Sealed) -> Answer {
        let root = self.projects[at].root.clone();
        let words = |u: &weft_core::ledger::Unit| {
            let path = root.join(".fab7/rf/asks").join(&u.ask_id).join("source.txt");
            std::fs::read_to_string(path).unwrap_or_else(|_| u.title.clone())
        };
        // Named by the dialog about to be staged: the daemon's own id, never
        // one read from the project's files.
        let path = drafts_dir(&self.config).join(format!("pnd_{}.txt", self.next_pending));
        let _ = std::fs::create_dir_all(drafts_dir(&self.config));
        let _ = std::fs::remove_file(&path);
        let follow_ups: Vec<String> = sealed.asks[1..].iter().map(words).collect();
        let request = weft_core::after_seal::rule_request(
            &sealed.root().title,
            &words(sealed.root()),
            &follow_ups,
            &path.to_string_lossy(),
        );
        let harness = sealed.newest().harness.clone();
        let asking = weft_core::after_seal::drafting(&self.title_of(at, &harness));
        let drafts = crate::acts::Drafting { path, unit: sealed.unit.root.clone(), harness };
        self.stage_typing(at, sealed, request, asking, (None, Some(drafts)))
    }

    /// Each draft an agent has written is read once, taken, and told to
    /// every client of its project.
    pub(super) fn read_drafts(&mut self) {
        for at in 0..self.projects.len() {
            let mut told = Vec::new();
            self.projects[at].drafting.retain(|d| {
                let read = std::fs::read_to_string(&d.path).ok();
                match read.as_deref().and_then(weft_core::after_seal::drafted) {
                    Some(rule) => {
                        let _ = std::fs::remove_file(&d.path);
                        told.push(Out::Drafted {
                            unit: d.unit.clone(),
                            harness: d.harness.clone(),
                            rule,
                        });
                        false
                    }
                    None => true,
                }
            });
            for t in told {
                self.clients.broadcast(at, &t);
            }
        }
    }

    /// Dialog two's Save: the rule, as the person edited it, kept in their
    /// own layer, or shared in the project's `.fab7/rules.toml` through
    /// RingFrame. Off the run loop: RingFrame reads every layer first.
    fn keep_rule(
        &mut self,
        (client, call_id): (u64, u64),
        at: usize,
        sealed: &Sealed,
        draft: Option<&weft_core::ask::Draft>,
    ) -> Answer {
        let Some(rule) =
            draft.and_then(|d| d.rule.as_deref().cloned()).filter(|r| !r.rule.trim().is_empty())
        else {
            return Answer::No("no_rule", "Save keeps a rule: its line is empty".into());
        };
        let share = draft.is_some_and(|d| d.share);
        let tasks = match &sealed.root().tasks {
            none if none.is_empty() => vec!["implement".to_string()],
            some => some.clone(),
        };
        let (config, known) = (self.config.clone(), self.projects[at].harnesses.clone());
        let (root, tx) = (self.projects[at].root.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let answer = match keep(&config, &known, &root, (rule, tasks), share) {
                Ok((id, layer)) => Answer::Ok(serde_json::json!({"id": id, "layer": layer})),
                Err(why) => Answer::No("refused", why),
            };
            let _ = tx.send(Wake::Kept { client, call_id, answer });
        });
        Answer::Later
    }
}

/// Where agents write their drafts: beside Weft's `config.toml`.
fn drafts_dir(config: &Path) -> PathBuf {
    config.with_file_name("drafts")
}

/// The person's rule as RingFrame reads one: its id, label and text, when it
/// matters and its family when the draft said, for the work's tasks.
fn entry(id: &str, rule: &Drafted, tasks: &[String]) -> serde_json::Value {
    let mut e = serde_json::json!({
        "id": id, "label": rule.name.trim(), "text": rule.rule.trim(),
        "applies_to": {"task": tasks},
    });
    for (key, value) in [("when", &rule.when), ("family", &rule.family)] {
        if !value.trim().is_empty() {
            e[key] = serde_json::json!(value.trim());
        }
    }
    e
}

/// Keep `rule`: the person's rules with it added must read for RingFrame;
/// shared, RingFrame writes it to the team's file from those layers and the
/// person's own file is left as it was. Answers its id and layer.
fn keep(
    config: &Path,
    known: &weft_core::harness::Harnesses,
    root: &Path,
    (rule, tasks): (Drafted, Vec<String>),
    share: bool,
) -> Result<(String, &'static str), String> {
    let text = std::fs::read_to_string(config).unwrap_or_default();
    let layers: serde_json::Value = weft_core::config::read(&text, known)
        .machine_ringframe_override()
        .and_then(|l| serde_json::from_str(&l).ok())
        .unwrap_or_default();
    let mine: Vec<serde_json::Value> = layers[0]["ringframe"]["deltas"][MINE[2]]["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let taken = |id: &str| mine.iter().any(|e| e["id"] == id);
    let id =
        (mine.len() + 1..).map(|n| format!("own.{n}")).find(|id| !taken(id)).unwrap_or_default();
    let mut entries = mine.clone();
    entries.push(entry(&id, &rule, &tasks));
    let path: Vec<String> = MINE.iter().map(|k| k.to_string()).collect();
    let value = serde_json::Value::Array(entries);
    let candidate = config::checked_edit(&text, &path, Some(&value), known)?;
    let layers = weft_core::config::read(&candidate, known)
        .machine_ringframe_override()
        .ok_or("the rule did not reach your layer")?;
    let refused = |e: crate::ringframe::Error| {
        let crate::ringframe::Error::Refused { message, .. } = e;
        format!("RingFrame would not keep it: {message}")
    };
    crate::ringframe::reads_overrides(&layers).map_err(refused)?;
    if share {
        crate::ringframe::share(root, &id, &layers).map_err(refused)?;
        return Ok((id, "repo"));
    }
    crate::routing::change(config, |text| config::checked_edit(text, &path, Some(&value), known))?;
    Ok((id, "weft"))
}
