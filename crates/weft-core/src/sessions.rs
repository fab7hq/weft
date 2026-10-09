//! The harness sessions Weft has a receipt for, in a project.
//!
//! A harness owns its own session and can open one again by id. Weft keeps
//! only the id, the events its hooks signalled and the harness process, in
//! its own receipts (ADR-0030 D7): `~/.fab7/weft/sessions/<project key>/
//! <harness>/<id>.jsonl`. A session with no receipt is a session Weft cannot
//! name, and it says so rather than guessing.

/// A project's key: the first 12 hex digits of the SHA-256 of its path.
pub fn project_key(path: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// Whether a session id is only an id: letters, digits, `.`, `_` and `-`,
/// not starting with `-`. The ids come from a project's files (a receipt
/// directory's name, a ledger's `session_ref`), which a cloned repository can
/// write; anything else could become words of its own on the resume command
/// line, a flag among them.
pub fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('-')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A session a harness ran here, as Weft's receipts recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub id: String,
    /// When it was last used, in milliseconds since the epoch.
    pub at: i64,
    /// The first line of its last prompt, when one is known. Weft's receipts
    /// keep nothing from the conversation, so it is empty from them.
    pub last: String,
}

pub fn first_line(prompt: &str) -> String {
    prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// When a session was last used, from its turn receipts and its last
/// prompt's time: `None` when it never held a conversation (it only said it
/// was `ready`), so there is nothing to resume.
pub fn last_used(turns: Option<&crate::turns::Session>, prompted: Option<i64>) -> Option<i64> {
    let turned =
        turns.filter(|t| t.latest != crate::turns::Turn::Ready || t.first < t.at).map(|t| t.at);
    prompted.max(turned)
}

/// A time reads as its hour and minute, as the receipts count them (UTC).
/// The date is not shown: a session you would reopen is one you remember
/// starting.
pub fn clock(at: i64) -> String {
    let minutes = at.div_euclid(60_000).rem_euclid(24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

// --- The session index (ADR-0029, sessions.md "The session index") -------

use serde::{Deserialize, Serialize};

use crate::ledger::Unit;
use crate::turns::{Started, Turn};

/// The harness process a session's receipts name (ADR-0029 D11): its pid,
/// and when it started, in the kernel's own units. A pid with another start
/// time is another process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub start: String,
}

/// The process a session's receipts name last: the latest line that carries
/// one. A resumed session's new process is written on a line of its own.
pub fn process(text: &str) -> Option<Process> {
    text.lines().rev().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
        Some(Process { pid, start: v.get("pid_start")?.as_str()?.to_string() })
    })
}

/// One session, as Weft's receipts for it read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub session: crate::turns::Session,
    pub process: Option<Process>,
}

/// Whether a session runs: a fact read from the machine, never a guess from
/// how long it has been quiet (ADR-0029 D11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    Ended,
    /// Neither can be known: no process and no end on record. Read as
    /// "last active 11:25".
    Quiet,
}

/// One session of a project, as weftd keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub harness: String,
    pub state: State,
    /// Its latest turn event; none when it has no turn receipt, or ended.
    #[serde(default)]
    pub turn: Option<Turn>,
    /// When it was first heard of, in milliseconds since the epoch.
    pub started: i64,
    /// When it was last used, else `started`.
    pub last: i64,
    /// The title of the latest Ask made in it.
    #[serde(default)]
    pub title: Option<String>,
    /// How many Asks were made in it.
    pub asks: usize,
    /// The Weft pane that holds it, while one runs.
    #[serde(default)]
    pub pane: Option<u32>,
    /// It held a conversation, so there is something to resume.
    pub resumable: bool,
}

/// Every session of a project: each one Weft's receipts name, and each one a
/// pane is bound to. `alive` says whether a recorded harness process still
/// runs with its recorded start time. Running first, then the rest, each
/// newest first.
pub fn index(
    heard: &[Heard],
    panes: &[Started],
    units: &[Unit],
    alive: &dyn Fn(&Process) -> bool,
) -> Vec<Row> {
    let named = |harness: &str, id: &str| is_id(harness) && is_id(id);
    let mut rows: Vec<Row> = Vec::new();
    for h in heard.iter().filter(|h| named(&h.session.harness, &h.session.id)) {
        let s = &h.session;
        let state = if s.latest == Turn::Ended {
            State::Ended
        } else {
            match &h.process {
                Some(p) if alive(p) => State::Running,
                Some(_) => State::Ended,
                None => State::Quiet,
            }
        };
        let used = last_used(Some(s), None);
        rows.push(Row {
            id: s.id.clone(),
            harness: s.harness.clone(),
            state,
            turn: Some(s.latest).filter(|t| *t != Turn::Ended),
            started: s.first,
            last: used.unwrap_or(s.first),
            title: None,
            asks: 0,
            pane: None,
            resumable: used.is_some(),
        });
    }
    // A Weft pane bound to a session is a fact about it: running while the
    // pane runs, ended once the pane's harness has exited.
    for p in panes {
        let Some(bound) = p.session.as_ref().filter(|b| named(&p.harness, &b.session)) else {
            continue;
        };
        let at = rows.iter().position(|r| r.harness == p.harness && r.id == bound.session);
        let row = match at {
            Some(i) => &mut rows[i],
            None => {
                rows.push(Row {
                    id: bound.session.clone(),
                    harness: p.harness.clone(),
                    state: State::Quiet,
                    turn: None,
                    started: p.at,
                    last: p.at,
                    title: None,
                    asks: 0,
                    pane: None,
                    resumable: false,
                });
                rows.last_mut().expect("just pushed")
            }
        };
        if p.running {
            row.state = State::Running;
            row.pane = Some(p.pane);
        } else if row.pane.is_none() && row.state != State::Running {
            row.state = State::Ended;
        }
    }
    for row in &mut rows {
        let mine = units.iter().filter(|u| u.session_ref.as_deref() == Some(row.id.as_str()));
        row.asks = mine.clone().count();
        row.title = mine.max_by(|a, b| a.asked_at.cmp(&b.asked_at)).map(|u| u.title.clone());
    }
    rows.sort_by(|a, b| {
        (b.state == State::Running)
            .cmp(&(a.state == State::Running))
            .then(b.last.cmp(&a.last))
            .then(a.id.cmp(&b.id))
    });
    rows
}

/// How many ended sessions the tree shows after the running ones.
pub const ENDED_IN_TREE: usize = 3;

/// The tree's rows, and the earlier sessions it counts but does not list:
/// every running row, then the first [`ENDED_IN_TREE`] of the rest.
pub fn split(rows: &[Row]) -> (&[Row], &[Row]) {
    let running = rows.iter().take_while(|r| r.state == State::Running).count();
    rows.split_at((running + ENDED_IN_TREE).min(rows.len()))
}

/// Where an earlier session is listed, by the Mac's local date of its `last`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Today,
    Yesterday,
    /// The seven days before yesterday.
    Week,
    Earlier,
}

impl Group {
    /// `offset` is the Mac's distance from UTC, in milliseconds.
    pub fn of(at: i64, now: i64, offset: i64) -> Self {
        match local_day(now, offset) - local_day(at, offset) {
            ..=0 => Group::Today,
            1 => Group::Yesterday,
            2..=8 => Group::Week,
            _ => Group::Earlier,
        }
    }
}

/// Whether a row is one a search for `q` finds: a substring of its latest
/// Ask's title, its harness or its id, whatever the case.
pub fn matches(row: &Row, q: &str) -> bool {
    let q = q.to_lowercase();
    let has = |text: &str| text.to_lowercase().contains(&q);
    row.title.as_deref().is_some_and(has) || has(&row.harness) || has(&row.id)
}

/// A session's id as a row shows it: its first eight characters.
pub fn short(id: &str) -> &str {
    id.char_indices().nth(8).map_or(id, |(i, _)| &id[..i])
}

/// A time as the Mac reads it: `11:29`, or with its weekday in the week
/// before yesterday, `Tue 17:48`, or its date before that, `2026-09-30`.
pub fn when(at: i64, now: i64, offset: i64) -> String {
    let local = at + offset;
    let time = clock(local);
    match Group::of(at, now, offset) {
        Group::Today | Group::Yesterday => time,
        Group::Week => {
            const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
            format!("{} {time}", DAYS[local_day(at, offset).rem_euclid(7) as usize])
        }
        Group::Earlier => crate::turns::stamp(local)[..10].to_string(),
    }
}

/// Days since the epoch on the Mac's calendar.
fn local_day(at: i64, offset: i64) -> i64 {
    (at + offset).div_euclid(24 * 60 * 60 * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turns::{Session, Turn};

    fn turns(first: i64, latest: Turn, at: i64) -> Session {
        Session { harness: "codex".into(), id: "s".into(), first, latest, at }
    }

    #[test]
    fn a_session_was_used_once_it_held_a_conversation() {
        assert_eq!(last_used(None, None), None);
        assert_eq!(last_used(Some(&turns(5, Turn::Ready, 5)), None), None, "only ready");
        assert_eq!(last_used(Some(&turns(5, Turn::TurnEnded, 9)), None), Some(9));
        assert_eq!(last_used(Some(&turns(5, Turn::Ready, 5)), Some(7)), Some(7), "a prompt");
        assert_eq!(last_used(Some(&turns(5, Turn::Working, 6)), Some(7)), Some(7), "the later");
    }

    // --- The session index -------------------------------------------------

    const MIN: i64 = 60_000;
    const DAY: i64 = 24 * 60 * MIN;
    /// 2026-10-08T12:00:00Z, a Thursday.
    const NOON: i64 = 1_791_460_800_000;

    fn heard(harness: &str, id: &str, first: i64, latest: Turn, at: i64) -> Heard {
        Heard { session: turns(first, latest, at).named(harness, id), process: None }
    }

    impl Session {
        fn named(mut self, harness: &str, id: &str) -> Self {
            self.harness = harness.into();
            self.id = id.into();
            self
        }
    }

    fn with_process(mut h: Heard, pid: u32, start: &str) -> Heard {
        h.process = Some(Process { pid, start: start.into() });
        h
    }

    fn asked(id: &str, title: &str, session: &str, at: &str) -> Unit {
        Unit {
            ask_id: id.into(),
            title: title.into(),
            harness: "claude-code".into(),
            session_ref: Some(session.into()),
            asked_at: at.into(),
            ..Default::default()
        }
    }

    fn pane(pane: u32, harness: &str, session: &str, running: bool) -> Started {
        let mut p = Started::new(pane, harness, harness, NOON - 60 * MIN, None);
        crate::turns::signalled(std::slice::from_mut(&mut p), pane, session);
        p.running = running;
        p
    }

    const DEAD: &dyn Fn(&Process) -> bool = &|_| false;

    fn ids(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn the_receipts_name_their_latest_process() {
        let text = [
            r#"{"event":"ready","session":"s","time":"2026-10-08T10:00:00.000Z","harness":"codex","pid":41,"pid_start":"41.1"}"#,
            r#"{"event":"working","session":"s","time":"2026-10-08T10:01:00.000Z","harness":"codex"}"#,
            r#"{"event":"working","session":"s","time":"2026-10-08T11:00:00.000Z","harness":"codex","pid":97,"pid_start":"97.2"}"#,
            "{torn",
        ]
        .join("\n");
        assert_eq!(process(&text), Some(Process { pid: 97, start: "97.2".into() }));
        assert_eq!(process(r#"{"event":"ready","session":"s"}"#), None);
    }

    /// A session run in another program (receipts, no pane) is a row, with
    /// its harness, turn, latest Ask's title and last time.
    #[test]
    fn a_session_another_program_runs_is_a_row() {
        let alive = |p: &Process| p.pid == 41 && p.start == "41.1";
        let h = with_process(
            heard("claude-code", "s1", NOON, Turn::Working, NOON + 5 * MIN),
            41,
            "41.1",
        );
        let units = [
            asked("ask_1", "readme fix", "s1", "2026-10-08T12:01:00Z"),
            asked("ask_2", "fix login", "s1", "2026-10-08T12:02:00Z"),
            asked("ask_3", "elsewhere", "s9", "2026-10-08T12:03:00Z"),
        ];
        let rows = index(&[h], &[], &units, &alive);
        assert_eq!(
            rows,
            [Row {
                id: "s1".into(),
                harness: "claude-code".into(),
                state: State::Running,
                turn: Some(Turn::Working),
                started: NOON,
                last: NOON + 5 * MIN,
                title: Some("fix login".into()),
                asks: 2,
                pane: None,
                resumable: true,
            }]
        );
        assert!(index(&[], &[], &units, &alive).is_empty(), "no receipt, no pane: no row");
    }

    #[test]
    fn a_session_that_only_said_ready_is_listed_with_nothing_to_resume() {
        let rows = index(&[heard("codex", "c1", NOON, Turn::Ready, NOON)], &[], &[], DEAD);
        assert_eq!((rows[0].state, rows[0].turn), (State::Quiet, Some(Turn::Ready)));
        assert!(!rows[0].resumable);
        assert_eq!((rows[0].last, rows[0].title.as_deref(), rows[0].asks), (NOON, None, 0));
    }

    /// A running pane's session is running whatever its receipts say.
    #[test]
    fn a_running_pane_holds_its_session_running() {
        let h = heard("claude-code", "s1", NOON, Turn::TurnEnded, NOON + MIN);
        let rows = index(std::slice::from_ref(&h), &[pane(3, "claude-code", "s1", true)], &[], DEAD);
        assert_eq!((rows[0].state, rows[0].pane), (State::Running, Some(3)));
        assert_eq!(rows[0].turn, Some(Turn::TurnEnded));
        // A pane bound before any receipt: a row from the pane alone.
        let rows = index(&[], &[pane(4, "codex", "c7", true)], &[], DEAD);
        assert_eq!((ids(&rows), rows[0].state, rows[0].turn), (vec!["c7"], State::Running, None));
        assert_eq!(rows[0].started, NOON - 60 * MIN);
        // A pane that has exited no longer holds it: its harness ended.
        let rows = index(&[h], &[pane(3, "claude-code", "s1", false)], &[], DEAD);
        assert_eq!((rows[0].state, rows[0].pane), (State::Ended, None));
    }

    /// D11: running while the recorded process lives with its start time;
    /// a reused pid is another process; an end on record settles it.
    #[test]
    fn running_or_ended_is_the_recorded_process_or_an_end() {
        let alive = |p: &Process| p.pid == 41 && p.start == "41.1";
        let h = |start: &str| {
            with_process(heard("codex", "c1", NOON, Turn::Working, NOON + MIN), 41, start)
        };
        assert_eq!(index(&[h("41.1")], &[], &[], &alive)[0].state, State::Running);
        assert_eq!(index(&[h("41.9")], &[], &[], &alive)[0].state, State::Ended, "pid reused");
        assert_eq!(index(&[h("41.1")], &[], &[], DEAD)[0].state, State::Ended, "gone");
        let mut ended = h("41.1");
        ended.session.latest = Turn::Ended;
        let row = &index(&[ended], &[], &[], &alive)[0];
        assert_eq!((row.state, row.turn), (State::Ended, None), "the end event settles it");
        let quiet = heard("agy", "a1", NOON, Turn::TurnEnded, NOON + MIN);
        assert_eq!(index(&[quiet], &[], &[], &alive)[0].state, State::Quiet, "nothing to read");
    }

    #[test]
    fn running_rows_come_first_then_the_rest_newest_first_and_the_tree_cuts_at_three() {
        let alive = |p: &Process| p.pid < 10;
        let mut heard_ = Vec::new();
        for (i, id) in ["e1", "e2", "e3", "e4", "e5"].iter().enumerate() {
            heard_.push(with_process(
                heard("codex", id, NOON, Turn::TurnEnded, NOON + i as i64 * MIN),
                100,
                "x",
            ));
        }
        heard_.push(with_process(heard("codex", "r1", NOON, Turn::Working, NOON - DAY), 1, "x"));
        heard_.push(heard("agy", "q1", NOON, Turn::TurnEnded, NOON + 10 * MIN));
        let rows = index(&heard_, &[pane(2, "claude-code", "r2", true)], &[], &alive);
        assert_eq!(ids(&rows), ["r2", "r1", "q1", "e5", "e4", "e3", "e2", "e1"]);
        let (tree, earlier) = split(&rows);
        assert_eq!(
            (ids(tree), ids(earlier)),
            (vec!["r2", "r1", "q1", "e5", "e4"], vec!["e3", "e2", "e1"])
        );
        let (tree, earlier) = split(&rows[..3]);
        assert_eq!((tree.len(), earlier.len()), (3, 0));
    }

    #[test]
    fn a_flag_shaped_directory_is_not_a_row() {
        let rows =
            index(&[heard("codex", "--last --yolo", NOON, Turn::Working, NOON)], &[], &[], DEAD);
        assert!(rows.is_empty());
        let rows = index(&[heard("--x", "c1", NOON, Turn::Working, NOON)], &[], &[], DEAD);
        assert!(rows.is_empty());
    }

    /// Today, yesterday, the seven days before yesterday, and earlier, by
    /// the Mac's local date.
    #[test]
    fn each_date_group_is_by_the_macs_local_date() {
        let seven = 7 * 60 * MIN; // UTC+7: NOON is 19:00 local, Thursday
        assert_eq!(Group::of(NOON, NOON, 0), Group::Today);
        assert_eq!(Group::of(NOON - 12 * 60 * MIN, NOON, 0), Group::Today, "00:00 UTC");
        assert_eq!(Group::of(NOON - 12 * 60 * MIN - MIN, NOON, 0), Group::Yesterday);
        // 23:59 UTC the day before is 06:59 local today.
        assert_eq!(Group::of(NOON - 12 * 60 * MIN - MIN, NOON, seven), Group::Today);
        assert_eq!(Group::of(NOON - 2 * DAY, NOON, 0), Group::Week);
        assert_eq!(Group::of(NOON - 8 * DAY, NOON, 0), Group::Week);
        assert_eq!(Group::of(NOON - 9 * DAY, NOON, 0), Group::Earlier);
        assert_eq!(Group::of(NOON + DAY, NOON, 0), Group::Today, "a clock that moved back");
        assert_eq!(when(NOON + 31 * MIN, NOON + 40 * MIN, 0), "12:31");
        assert_eq!(when(NOON + 31 * MIN, NOON + 40 * MIN, seven), "19:31");
        assert_eq!(when(NOON - DAY, NOON, 0), "12:00");
        assert_eq!(when(NOON - 2 * DAY, NOON, 0), "Tue 12:00");
        assert_eq!(when(NOON - 9 * DAY, NOON, 0), "2026-09-29");
    }

    #[test]
    fn search_finds_a_title_a_harness_or_an_id_and_nothing_else() {
        let units = [asked("ask_1", "Token Refresh", "c41f9a07", "2026-10-08T12:01:00Z")];
        let rows = index(
            &[
                heard("claude-code", "c41f9a07", NOON, Turn::TurnEnded, NOON),
                heard("codex", "5be2d8f4", NOON, Turn::TurnEnded, NOON),
            ],
            &[],
            &units,
            DEAD,
        );
        let found = |q: &str| {
            rows.iter().filter(|r| matches(r, q)).map(|r| r.id.as_str()).collect::<Vec<_>>()
        };
        assert_eq!(found("token"), ["c41f9a07"]);
        assert_eq!(found("CODEX"), ["5be2d8f4"]);
        assert_eq!(found("5be2"), ["5be2d8f4"]);
        assert_eq!(found(""), ["5be2d8f4", "c41f9a07"], "equal times, by id");
        assert!(found("ended").is_empty(), "its state is not searched");
        assert_eq!(short("c41f9a07-1d1f-4e"), "c41f9a07");
    }
}
