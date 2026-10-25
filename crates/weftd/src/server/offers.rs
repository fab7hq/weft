//! What sealed work offers: keep a rule, follow up, undo. Each has
//! one dialog; what an offer types goes into the work's own agent, where the
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
            "drop_draft" => {
                self.drop_drafts(at, &sealed.unit.root);
                Answer::nothing()
            }
            "keep_rule" => self.keep_rule((client, call_id), at, &sealed, draft),
            _ => self.undo(at, &sealed),
        }
    }

    /// Undo's dialog: the work's agent puts the files back as they were at
    /// the commit its first Ask was sent on; its yes in amber.
    fn undo(&mut self, at: usize, sealed: &Sealed) -> Answer {
        let Some(base) = sealed.root().base_commit.clone() else {
            let why = "RingFrame recorded no commit for this work's first Ask".to_string();
            return Answer::No("no_commit", why);
        };
        let (harness, title) = (self.title_of(at, &sealed.newest().harness), &sealed.root().title);
        let typed = weft_core::after_seal::undo_request(title, &base);
        let go = weft_proto::Go { word: "offer.undo".into(), warn: true };
        self.stage_typing(at, sealed, typed, weft_core::after_seal::undoing(&harness, title), go)
    }

    /// A dialog that says what happens and names its yes, which types
    /// `typed` into the work's agent; what is typed is not shown.
    fn stage_typing(
        &mut self,
        at: usize,
        sealed: &Sealed,
        typed: String,
        asking: weft_core::offers::Asking,
        go: weft_proto::Go,
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
            go: Some(go),
            hidden: true,
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

    /// Keep a rule's opening: the request that has the work's agent draft
    /// one, from what was asked and the follow-ups, typed at once.
    fn draft_rule(&mut self, at: usize, sealed: &Sealed) -> Answer {
        let pane = match self.pane_of(at, sealed.newest()) {
            Ok(pane) => pane,
            Err(no) => return no,
        };
        let root = self.projects[at].root.clone();
        // Named by the daemon's own count, never by an id read from the
        // project's files.
        self.next_pending += 1;
        let path = drafts_dir(&self.config).join(format!("draft_{}.txt", self.next_pending));
        let _ = std::fs::create_dir_all(drafts_dir(&self.config));
        let _ = std::fs::remove_file(&path);
        let follow_ups: Vec<String> = sealed.asks[1..].iter().map(|u| words_of(&root, u)).collect();
        let request = weft_core::after_seal::rule_request(
            &sealed.root().title,
            &words_of(&root, sealed.root()),
            &follow_ups,
            &path.to_string_lossy(),
        );
        let (harness, unit) = (sealed.newest().harness.clone(), sealed.unit.root.clone());
        // A request asked again takes the place of the one before.
        self.drop_drafts(at, &unit);
        let stage = crate::acts::DraftStage::Typing;
        self.projects[at].drafting.push(crate::acts::Drafting { path, unit, harness, pane, stage });
        // The Keep dialog is the one confirmation: its opening has the agent
        // write the draft, and only its Save keeps anything.
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

    /// Stop waiting for a unit's draft: the person cancelled, kept a rule, or
    /// asked again.
    fn drop_drafts(&mut self, at: usize, unit: &str) {
        self.projects[at].drafting.retain(|d| {
            let mine = d.unit == unit;
            if mine {
                let _ = std::fs::remove_file(&d.path);
            }
            !mine
        });
    }

    /// The request typed into `pane`, or refused there: a refused one will
    /// bring no draft, and its dialog is told.
    pub(super) fn draft_typed(&mut self, at: usize, pane: u32, typed: bool) {
        use crate::acts::DraftStage;
        let mut refused = Vec::new();
        self.projects[at].drafting.retain_mut(|d| {
            if d.pane != pane || d.stage != DraftStage::Typing {
                return true;
            }
            if typed {
                d.stage = DraftStage::Typed;
                return true;
            }
            let _ = std::fs::remove_file(&d.path);
            refused.push(Out::NotDrafted {
                unit: d.unit.clone(),
                harness: d.harness.clone(),
                why: "not_typed".into(),
            });
            false
        });
        for r in refused {
            self.clients.broadcast(at, &r);
        }
    }

    /// Each draft is told once: as soon as its four lines are written, or
    /// when the agent's turn on it is over, with what it wrote, or that it
    /// wrote nothing. The turn is the pane's, as its hooks and Sense read it.
    pub(super) fn read_drafts(&mut self) {
        use crate::acts::DraftStage;
        for at in 0..self.projects.len() {
            let drafting = std::mem::take(&mut self.projects[at].drafting);
            let mut waiting = Vec::new();
            for mut d in drafting {
                let text = std::fs::read_to_string(&d.path).unwrap_or_default();
                let turn = self.pane_state(at, d.pane);
                if d.stage == DraftStage::Typed && turn == Some(Turn::Working) {
                    d.stage = DraftStage::Working;
                }
                let over = d.stage == DraftStage::Working
                    && !matches!(turn, Some(Turn::Working | Turn::Waiting));
                let told = match weft_core::after_seal::drafted(&text) {
                    Some(rule) => Some(Some(rule)),
                    None if over => Some(weft_core::after_seal::settled(&text)),
                    None => None,
                };
                let Some(rule) = told else {
                    waiting.push(d);
                    continue;
                };
                let _ = std::fs::remove_file(&d.path);
                let (unit, harness) = (d.unit, d.harness);
                let said = match rule {
                    Some(rule) => Out::Drafted { unit, harness, rule },
                    None => Out::NotDrafted { unit, harness, why: "no_draft".into() },
                };
                self.clients.broadcast(at, &said);
            }
            self.projects[at].drafting.extend(waiting);
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
        // Kept: a draft still coming is no longer wanted.
        self.drop_drafts(at, &sealed.unit.root);
        let (config, known) = (self.config.clone(), self.projects[at].harnesses.clone());
        let (root, tx) = (self.projects[at].root.clone(), self.tx.clone());
        // The work's words: the rule's example intents.
        let intent: Vec<String> = sealed.asks.iter().map(|u| words_of(&root, u)).collect();
        let harness = sealed.newest().harness.clone();
        std::thread::spawn(move || {
            let answer = match keep(&config, &known, &root, (rule, tasks), share) {
                Ok((id, layer)) => {
                    let row = crate::labels::kept(&harness, &id, layer, intent.join("\n").trim());
                    if let Err(e) = crate::labels::append(&config, &root, &row) {
                        eprintln!("weft: the kept rule's label was not kept: {e}");
                    }
                    Answer::Ok(serde_json::json!({"id": id, "layer": layer}))
                }
                Err(why) => Answer::No("refused", why),
            };
            let _ = tx.send(Wake::Kept { client, call_id, answer });
        });
        Answer::Later
    }
}

/// What the person asked in an Ask, as RingFrame kept it, else its title.
fn words_of(root: &Path, u: &weft_core::ledger::Unit) -> String {
    let path = root.join(".fab7/rf/asks").join(&u.ask_id).join("source.txt");
    std::fs::read_to_string(path).unwrap_or_else(|_| u.title.clone())
}

/// Where agents write their drafts: beside Weft's `config.toml`.
fn drafts_dir(config: &Path) -> PathBuf {
    config.with_file_name("drafts")
}

/// The person's rule as RingFrame reads one: its id, label and text, when it
/// matters and its family when the draft said, for the work's tasks.
fn entry(id: &str, rule: &Drafted, tasks: &[String]) -> serde_json::Value {
    // A rule the person wrote themselves is named by its first words.
    let named = weft_core::after_seal::settled(&rule.rule).map(|d| d.name).unwrap_or_default();
    let label = if rule.name.trim().is_empty() { named } else { rule.name.trim().to_string() };
    let mut e = serde_json::json!({
        "id": id, "label": label, "text": rule.rule.trim(),
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
