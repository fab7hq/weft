//! The remote protocol (remote.md §4, §6): §14 test 5, and a `req` always
//! answered.

mod common;

use std::time::Duration;

use common::Harness;
use serde_json::{Value, json};

#[test]
fn nothing_the_never_sent_list_names_reaches_the_daemon() {
    let mut h = Harness::start("never");
    let mut shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    let attempts = [
        json!({"type": "act", "agent": agent, "act": "send", "req": 1}),
        json!({"type": "act", "agent": agent, "act": "follow_up", "text": "x", "req": 2}),
        json!({"type": "act", "agent": agent, "act": "start", "req": 3}),
        json!({"type": "act", "agent": agent, "act": "confirm", "req": 4}),
        json!({"type": "resolve", "pending": "pnd_1", "yes": true, "force": true, "req": 5}),
        json!({"type": "read", "agent": agent, "what": "wording", "req": 6}),
        json!({"type": "start", "harness": "sh", "spec": "/bin/sh", "req": 7}),
        json!({"type": "agent.start", "harness": "sh", "req": 8}),
        json!({"type": "close", "agent": agent, "req": 9}),
        json!({"type": "stage", "agent": agent, "text": "rm -rf", "req": 10}),
        json!({"type": "setup", "req": 11}),
        json!({"type": "turbo", "on": true, "req": 12}),
        json!({"type": "shutdown", "req": 13}),
        json!({"type": "confirm", "unit": "ask_1", "req": 14}),
        // A resume names a session's id, never "the latest" (remote.md §6).
        json!({"type": "start", "project": "work", "harness": "sh", "resume": true, "req": 15}),
    ];
    for (n, attempt) in attempts.iter().enumerate() {
        d.send(attempt.clone());
        let answer = d.answer(n as i64 + 1);
        assert_eq!(answer["type"], "error", "{attempt}");
        assert!(
            answer["code"] == "not_allowed" || answer["code"] == "bad_request",
            "{attempt}: {answer}"
        );
    }
    // The daemon is as it was: one agent, still running, and nothing added.
    d.send(json!({"type": "ping", "t": 7}));
    assert_eq!(d.next_of("pong")["t"], 7);
    let _ = shell.app.views(Duration::from_millis(300));
    let (mut fresh, welcome) = h.allowed("d_2");
    let _ = &mut fresh;
    assert_eq!(welcome["agents"].as_array().map(Vec::len), Some(1), "no agent started or closed");
    assert_eq!(welcome["agents"][0]["turn"], "ready", "still running");
}

#[test]
fn the_tree_comes_in_welcome_and_again_when_an_agent_ends() {
    let mut h = Harness::start("tree");
    let mut shell = h.shell("work");
    let (mut first, _) = h.allowed("d_1");
    first.agents(1);
    // Once the project is open, a device is welcomed with its tree.
    let (mut d, welcome) = h.allowed("d_2");
    let agent = welcome["agents"][0]["agent"].clone();
    let tree = &welcome["tree"];
    assert_eq!(tree.as_array().map(Vec::len), Some(1), "{tree}");
    assert_eq!((tree[0]["project"].clone(), tree[0]["open"].clone()), (json!("work"), json!(0)));
    assert!(tree[0].get("setup").is_some(), "whether RingFrame is set up here: {tree}");
    // No session named yet: a row all the same, with no id, its agent the pane's.
    let row = &tree[0]["agents"]["rows"][0];
    assert_eq!(
        (row["session"].clone(), row["state"].clone(), row["agent"].clone()),
        (json!(""), json!("running"), agent.clone()),
        "{tree}"
    );
    assert!(tree[0].get("harnesses").is_none(), "no harness rows (remote.md §6)");

    // The shell's hook names its session: it is a row, its agent the pane's.
    let told = std::fs::read_to_string(shell.project.with_extension("pane")).unwrap();
    let (token, socket) = told.split_once('\n').unwrap();
    let sent = weft_core::signal::Sent {
        harness: "sh".into(),
        hook: "Hook".into(),
        session: "s1".into(),
        workspace: Some(shell.project.to_string_lossy().into_owned()),
        state: Some("ready".into()),
        pane: Some(0),
        token: Some(token.into()),
        ..Default::default()
    };
    let line = weft_proto::Line::Event(weft_proto::Event::Signal(Box::new(sent))).encode();
    use std::io::Write as _;
    std::os::unix::net::UnixStream::connect(socket.trim()).unwrap().write_all(&line).unwrap();
    let named = loop {
        let m = d.next_of("tree");
        if m["tree"][0]["agents"]["rows"][0]["session"] == json!("s1") {
            break m;
        }
    };
    let row = &named["tree"][0]["agents"]["rows"][0];
    assert_eq!(
        (row["session"].clone(), row["state"].clone(), row["agent"].clone()),
        (json!("s1"), json!("running"), agent.clone())
    );

    let pane = agent.as_str().unwrap().rsplit_once('-').unwrap().1.parse().unwrap();
    shell.app.call(weft_proto::Call::Input { pane, bytes: b"exit\r".to_vec() });
    let ended = loop {
        let m = d.next_of("tree");
        if m["tree"][0]["agents"]["rows"][0]["state"] == "ended" {
            break m;
        }
    };
    let a = &ended["tree"][0]["agents"];
    assert_eq!(
        (a["running"].clone(), a["ended"].clone(), a["rows"][0]["agent"].clone()),
        (json!(0), json!(1), Value::Null),
        "its row stays, ended: {ended}"
    );
}

#[test]
fn a_device_starts_only_a_harness_the_machine_offers() {
    let mut h = Harness::start("start");
    let _shell = h.shell("work");
    let (mut first, _) = h.allowed("d_1");
    first.agents(1);
    let (mut d, welcome) = h.allowed("d_2");
    // No harness file here is set up for RingFrame, so none can be started.
    assert_eq!(welcome["harnesses"], json!([]), "{welcome}");
    for (n, (start, code)) in [
        (json!({"type": "start", "project": "work", "harness": "sh", "req": 1}), "no_harness"),
        (json!({"type": "start", "project": "elsewhere", "harness": "sh", "req": 2}), "no_project"),
        (json!({"type": "start", "harness": "sh", "req": 3}), "bad_request"),
    ]
    .into_iter()
    .enumerate()
    {
        d.send(start.clone());
        let answer = d.answer(n as i64 + 1);
        assert_eq!(
            (answer["type"].clone(), answer["code"].clone()),
            (json!("error"), json!(code)),
            "{start}"
        );
    }
}

#[test]
fn every_req_gets_ok_or_error() {
    let mut h = Harness::start("reqs");
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    let requests = [
        (json!({"type": "keys", "agent": agent, "bytes": "MQ==", "req": 1}), "ok"),
        (json!({"type": "keys", "agent": agent, "bytes": "not base64!", "req": 2}), "error"),
        (json!({"type": "keys", "agent": "000000000000-9", "bytes": "MQ==", "req": 3}), "error"),
        (json!({"type": "text", "agent": agent, "text": "echo hi", "req": 4}), "ok"),
        (json!({"type": "text", "agent": agent, "text": "", "req": 5}), "error"),
        (json!({"type": "act", "agent": agent, "act": "eval", "req": 6}), "error"),
        (json!({"type": "act", "agent": agent, "act": "ask", "req": 7}), "error"),
        (json!({"type": "resolve", "pending": "pnd_404", "yes": false, "req": 8}), "error"),
        (json!({"type": "read", "agent": agent, "what": "history", "req": 9}), "ok"),
        (json!({"type": "read", "agent": agent, "what": "eval", "req": 10}), "error"),
        (json!({"type": "nonsense", "req": 11}), "error"),
        (json!({"type": "read", "what": "detail", "unit": "ask_404", "req": 12}), "error"),
        (json!({"type": "read", "what": "eval_view", "req": 13}), "error"),
        (json!({"type": "read", "what": "sessions", "project": "work", "q": "x", "req": 14}), "ok"),
        (json!({"type": "read", "what": "sealed", "project": "work", "req": 15}), "ok"),
        (
            json!({"type": "read", "what": "session", "project": "work", "session": "s_9", "req": 16}),
            "error",
        ),
        (
            json!({"type": "read", "what": "eval", "project": "work", "eval": "evl_9", "req": 17}),
            "error",
        ),
        (
            json!({"type": "read", "what": "seal", "project": "work", "seal": "sel_9", "req": 18}),
            "error",
        ),
        (
            json!({"type": "start", "project": "work", "harness": "sh", "resume": "s_9", "req": 19}),
            "error",
        ),
        (json!({"type": "act", "act": "eval", "project": "nowhere", "req": 20}), "error"),
    ];
    for (n, (request, want)) in requests.iter().enumerate() {
        d.send(request.clone());
        let answer = d.answer(n as i64 + 1);
        assert_eq!(answer["type"], *want, "{request}: {answer}");
    }
    // Exactly one answer each: nothing more for any of them.
    d.send(json!({"type": "ping", "t": 1}));
    loop {
        let m = d.next();
        assert!(m["type"] != "ok" && m["type"] != "error", "a second answer: {m}");
        if m["type"] == "pong" {
            break;
        }
    }
    // Without a req, a message the protocol does not allow ends the socket.
    d.send(json!({"type": "nonsense"}));
    assert!(d.closed_within(Duration::from_secs(1)));
}

#[test]
fn watching_sends_the_screen_whole_then_as_it_changes() {
    let mut h = Harness::start("watch");
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    d.send(json!({"type": "watch", "agent": agent}));
    let first = d.next_of("screen");
    assert_eq!(first["full"], true);
    assert_eq!(first["agent"], agent.as_str());
    assert_eq!((first["rows"].as_u64(), first["cols"].as_u64()), (Some(24), Some(80)));
    d.send(json!({"type": "keys", "agent": agent, "bytes": "ZWNobyB3YXRjaGVkLWZyb20tYS1waG9uZQ0=", "req": 1}));
    let mut seen = String::new();
    while !seen.contains("watched-from-a-phone") {
        let frame = d.next_of("screen");
        assert_eq!(frame["full"], false);
        use base64ct::Encoding;
        let bytes = base64ct::Base64::decode_vec(frame["bytes"].as_str().unwrap()).unwrap();
        seen.push_str(&String::from_utf8_lossy(&bytes));
    }
}

/// A device with four panes watches four agents: a whole screen of each, at
/// the agent's own size, whatever the device's panes are; a fifth is ignored.
/// `welcome` says `panes`.
#[test]
fn a_device_watches_four_agents_each_at_the_agents_size() {
    let mut h = Harness::start("watch-four");
    let _shells: Vec<_> = ["a", "b", "c", "d", "e"].iter().map(|n| h.shell(n)).collect();
    let (mut d, welcome) = h.allowed("d_1");
    assert_eq!((welcome["panes"].clone(), welcome["split"].clone()), (json!(4), json!(true)));
    let agents = d.agents(5);
    let id = |i: usize| agents[i]["agent"].as_str().unwrap().to_string();
    let also: Vec<Value> =
        (1..5).map(|i| json!({"agent": id(i), "rows": 20 + i, "cols": 40 + i})).collect();
    d.send(json!({"type": "watch", "agent": id(0), "rows": 20, "cols": 40, "also": also}));
    let mut whole = std::collections::HashMap::new();
    while whole.len() < 4 {
        let frame = d.next_of("screen");
        let agent = frame["agent"].as_str().unwrap().to_string();
        assert_ne!(agent, id(4), "a fifth agent is not watched");
        if frame["full"] == true {
            whole.insert(agent, (frame["rows"].clone(), frame["cols"].clone()));
        }
    }
    for i in 0..4 {
        assert_eq!(whole[&id(i)], (json!(24), json!(80)), "agent {i}");
    }
    // The fifth's output never comes; the first's does.
    d.send(json!({"type": "keys", "agent": id(4), "bytes": "ZWNobyBub3Qtd2F0Y2hlZA0=", "req": 1}));
    d.send(
        json!({"type": "keys", "agent": id(0), "bytes": "ZWNobyBzdGlsbC13YXRjaGVkDQ==", "req": 2}),
    );
    let mut seen = String::new();
    while !seen.contains("still-watched") {
        let frame = d.next_of("screen");
        assert_ne!(frame["agent"], id(4).as_str(), "a fifth agent is not watched");
        if frame["agent"] == id(0).as_str() {
            use base64ct::Encoding;
            let bytes = base64ct::Base64::decode_vec(frame["bytes"].as_str().unwrap()).unwrap();
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
}

/// A device may watch a second agent beside the first, each at the
/// agent's own size: both screens come, each whole first, and the second is let go when
/// the device watches one again. A machine says it can, in its welcome.
#[test]
fn a_device_watches_two_agents_side_by_side() {
    let mut h = Harness::start("watch-two");
    let _work = h.shell("work");
    let _more = h.shell("more");
    let (mut d, welcome) = h.allowed("d_1");
    assert_eq!(welcome["split"], true, "{welcome}");
    let agents = d.agents(2);
    let (a, b) = (agents[0]["agent"].as_str().unwrap(), agents[1]["agent"].as_str().unwrap());
    d.send(json!({"type": "watch", "agent": a, "rows": 30, "cols": 40,
                  "also": {"agent": b, "rows": 30, "cols": 41}}));
    let mut whole = std::collections::HashSet::new();
    while whole.len() < 2 {
        let frame = d.next_of("screen");
        if frame["full"] == true {
            whole.insert(frame["agent"].as_str().unwrap().to_string());
        }
    }
    assert!(whole.contains(a) && whole.contains(b), "{whole:?}");
    // One again: only the first agent's frames come.
    d.send(json!({"type": "watch", "agent": a}));
    d.send(json!({"type": "keys", "agent": b, "bytes": "ZWNobyBub3Qtd2F0Y2hlZA0=", "req": 1}));
    d.send(json!({"type": "keys", "agent": a, "bytes": "ZWNobyBzdGlsbC13YXRjaGVkDQ==", "req": 2}));
    let mut seen = String::new();
    while !seen.contains("still-watched") {
        let frame = d.next_of("screen");
        assert_eq!(frame["agent"], a, "the second agent was let go");
        use base64ct::Encoding;
        let bytes = base64ct::Base64::decode_vec(frame["bytes"].as_str().unwrap()).unwrap();
        seen.push_str(&String::from_utf8_lossy(&bytes));
    }
}

/// The machine's settings, read and changed from a device (weft-after-desktop
/// ADR-0003 D6): every device is told what changed, Turbo included.
#[test]
fn a_device_reads_and_changes_the_machine_s_settings() {
    let mut h = Harness::with("settings", |dir| {
        std::fs::create_dir_all(dir.join("harnesses")).unwrap();
        let grok = weft_core::harness::fixture::text("grok");
        std::fs::write(dir.join("harnesses/grok.toml"), grok).unwrap();
    });
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].clone();
    let (mut other, _) = h.allowed("d_2");
    other.agents(1);
    // An Ask sheet read names the agent it goes to: its plans are those of
    // the folder that agent works in.
    let unit = json!({"harness": "claude-code", "words": "Fix it.", "stage": "full"}).to_string();
    for (req, to) in [(40, agent), (41, json!("nope-9"))] {
        d.send(json!({"type": "settings", "project": "work", "what": "ask", "unit": unit,
                      "agent": to, "req": req}));
        let unknown = d.answer(req)["code"] == "no_agent";
        assert_eq!(unknown, req == 41, "only an agent not here is refused before the daemon");
    }
    d.send(json!({"type": "settings", "project": "work", "req": 1}));
    let got = d.answer(1);
    assert_eq!(got["type"], "ok", "{got}");
    assert_eq!(got["settings"]["effective"]["turbo"], false);
    let set = json!({"type": "set", "project": "work", "scope": "machine", "key": ["turbo"],
                     "value": true, "req": 2});
    d.send(set);
    let changed = d.answer(2);
    assert_eq!(changed["settings"]["effective"]["turbo"], true, "{changed}");
    let told = other.next_of("settings");
    assert_eq!(told["project"], "work");
    assert_eq!(told["settings"]["machine"]["turbo"], true);
    d.send(json!({"type": "set", "project": "work", "scope": "machine", "key": ["turbo"],
                  "value": "yes", "req": 3}));
    assert_eq!(d.answer(3)["type"], "error", "what Weft would not read is refused");
    // RingFrame's rules, a preview and a harness's profile reach the daemon
    // too, which asks RingFrame.
    for (n, (what, unit)) in
        [("rules", "machine"), ("preview", "codex|native_plan|implement"), ("profile", "codex")]
            .into_iter()
            .enumerate()
    {
        let req = 20 + n as i64;
        d.send(
            json!({"type": "settings", "project": "work", "what": what, "unit": unit, "req": req}),
        );
        let answer = d.answer(req);
        // Whatever RingFrame on this machine says: never refused before the
        // daemon.
        assert!(answer["type"] == "ok" || answer["code"] == "refused", "{what}: {answer}");
    }
    // Every harness this Weft supports, each with how it stands as the
    // machine last said it: answered by the remote from what it holds.
    d.send(json!({"type": "settings", "project": "work", "what": "harnesses", "req": 30}));
    let listed = d.answer(30);
    let grok = &listed["settings"][0];
    assert_eq!(
        (&grok["harness"], &grok["title"]),
        (&json!("grok"), &json!("Grok Build")),
        "{listed}"
    );
    for field in ["verified", "mark", "state"] {
        assert!(grok[field].is_string(), "{field}: {grok}");
    }
    // Which program runs, for a harness or the tunnel, is set on the machine
    // only: a device naming one is refused before the daemon hears it.
    for (n, key) in
        [json!(["harnesses", "codex", "program"]), json!(["remote", "program"]), json!(["remote"])]
            .into_iter()
            .enumerate()
    {
        let req = 10 + n as i64;
        d.send(json!({"type": "set", "project": "work", "scope": "machine", "key": key,
                      "value": "/tmp/anything", "req": req}));
        let answer = d.answer(req);
        assert_eq!(answer["code"], "not_allowed", "{key}: {answer}");
    }
}

/// A job in a project, as weftd sends it: one step, its count, how it ended.
fn job(kind: &str, state: weft_proto::StepState, done: u64, end: Option<&str>) -> weft_proto::Job {
    weft_proto::Job {
        id: "job_1".into(),
        kind: kind.into(),
        on: weft_proto::JobOn { work: Some("ask_1".into()), ..Default::default() },
        call: None,
        steps: vec![weft_proto::JobStep {
            key: "step.reading_change".into(),
            args: Default::default(),
            state,
            ms: Some(500),
            done: Some(done),
            total: Some(100),
            unit: Some("files".into()),
        }],
        share: None,
        age_ms: 500,
        end: end.map(|why| weft_proto::JobEnd { ok: false, why: Some(why.into()) }),
    }
}

#[test]
fn a_device_is_told_each_job_and_the_jobs_there_when_it_connects() {
    use weft_proto::{Event, StepState};
    let mut h = Harness::proxied("jobs");
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    d.agents(1);

    h.said.event(Event::Progress { job: job("seal", StepState::Now, 1, None) });
    let told = d.next_of("progress");
    assert_eq!(
        (told["project"].clone(), told["job"]["id"].clone()),
        (json!("work"), json!("job_1"))
    );
    assert!(told.get("req").is_none(), "no device asked for it: {told}");

    // A device that connects while it runs is told it right after its welcome.
    let (mut late, _) = h.allowed("d_2");
    let jobs = late.next_of("jobs");
    assert_eq!(
        (jobs["project"].clone(), jobs["jobs"][0]["id"].clone()),
        (json!("work"), json!("job_1"))
    );

    // Counts within a second are held back; a step change is not.
    h.said.event(Event::Progress { job: job("seal", StepState::Now, 2, None) });
    h.said.event(Event::Progress { job: job("seal", StepState::Done, 100, None) });
    let next = d.next_of("progress");
    assert_eq!(next["job"]["steps"][0]["state"], "done", "the count was held back: {next}");
}

#[test]
fn the_device_that_asked_is_told_its_job_and_an_eval_that_could_not_open_is_kept() {
    use weft_proto::{Event, StepState};
    let mut h = Harness::proxied("refused");
    let _shell = h.shell("work");
    let (mut asking, _) = h.allowed("d_1");
    asking.agents(1);
    let (mut other, _) = h.allowed("d_2");
    other.agents(1);
    let reads = json!({"since": "ask", "when": "2026-11-19T09:12:00.000Z", "commits": 3,
                       "files": 14, "skipped": {"generated": 1, "vendored": 0, "binary": 1, "lock": 0},
                       "folder": "src/", "folder_files": 9, "large": false});
    let eval = Event::Pending {
        id: "pnd_e".into(),
        pane: u32::MAX,
        what: "Eval".into(),
        why: String::new(),
        payload: json!({ "reads": reads }).to_string().into_bytes(),
        sends: None,
        go: Some(weft_proto::Go { word: "act.eval".into(), warn: false }),
    };
    h.said.event(eval);
    let pending = asking.next_of("pending");
    assert_eq!((pending["reads"].clone(), pending["text"].clone()), (reads, json!("")));

    // The yes: the opening's job ends failed, then RingFrame's refusal, as
    // weftd does; the job carries the call it began with.
    let mut failed = job("eval_open", StepState::Failed, 3, Some("git exited 128"));
    failed.call = Some(common::Said::THIS_CALL);
    h.said.refuse(
        "pending.resolve",
        vec![Event::Resolved { id: "pnd_e".into(), yes: true }, Event::Progress { job: failed }],
        "refused",
        "RingFrame could not read the change",
    );
    asking.send(json!({"type": "resolve", "pending": "pnd_e", "yes": true, "req": 1}));
    let own = asking.next_of("progress");
    assert_eq!((own["req"].clone(), own["job"]["end"]["ok"].clone()), (json!(1), json!(false)));
    assert!(own["job"].get("call").is_none(), "the daemon's call is not the device's");
    assert_eq!(asking.answer(1)["code"], "refused");
    let theirs = other.next_of("progress");
    assert!(theirs.get("req").is_none(), "{theirs}");
    assert_eq!(theirs["job"]["end"]["why"], "git exited 128");

    // The asking phone went to the background and came back: its welcome is
    // followed by the Eval that could not open, kept until acted on again.
    let token = asking.token.clone();
    drop(asking);
    std::thread::sleep(Duration::from_millis(1100));
    let (mut back, _) = h.again("d_1", &token);
    let kept = back.next_of("jobs");
    assert_eq!(kept["jobs"][0]["end"]["why"], "git exited 128", "{kept}");
    back.send(json!({"type": "act", "act": "eval", "project": "work", "unit": "ask_1", "req": 2}));
    let cleared = other.next_of("jobs");
    assert_eq!(cleared["jobs"], json!([]), "{cleared}");
}

/// Which files an Eval should skip reach every device: after its welcome,
/// and each time the daemon's check moves. A device checks again and takes a
/// line through the daemon, which writes only a line its own check proposed;
/// an Eval dialog said again under its id replaces the one devices hold.
#[test]
fn a_device_is_told_what_to_skip_and_takes_only_a_proposed_line() {
    use weft_proto::Event;
    let mut h = Harness::proxied("skip");
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    // After its welcome, or once the project opens here.
    let first = d.next_of("skip");
    assert_eq!((first["project"].clone(), first["findings"].clone()), (json!("work"), json!([])));

    let finding = json!({"kind": "untracked_bulk", "path": "runs/", "files": 30, "bytes": 2000000,
                         "commits": 0, "kinds_of_files": [["log", 30]],
                         "fix": {"file": ".gitignore", "line": "/runs/"}});
    h.said.event(Event::Skip { findings: vec![serde_json::from_value(finding.clone()).unwrap()] });
    let told = d.next_of("skip");
    assert_eq!(told["findings"], json!([finding]), "{told}");

    // The daemon checks again; a line it did not propose is refused there.
    d.send(json!({"type": "skip_check", "project": "work", "req": 1}));
    assert_eq!(d.answer(1)["findings"], json!([]), "the project has nothing to skip");
    d.send(json!({"type": "skip_add", "project": "work", "file": ".gitignore",
                  "line": "/src/", "req": 2}));
    assert_eq!(d.answer(2)["code"], "not_proposed");
    d.send(json!({"type": "skip_add", "project": "work", "file": ".gitignore", "req": 3}));
    assert_eq!(d.answer(3)["code"], "bad_request");

    // An Eval dialog said again: the same id, its reads as they are now.
    let dialog = |files: u64, added: bool| {
        let mut f = finding.clone();
        if added {
            f["added"] = json!(true);
        }
        let reads = json!({"since": "ask", "files": files, "findings": [f]});
        Event::Pending {
            id: "pnd_e".into(),
            pane: u32::MAX,
            what: "Eval".into(),
            why: String::new(),
            payload: json!({ "reads": reads }).to_string().into_bytes(),
            sends: None,
            go: Some(weft_proto::Go { word: "act.eval".into(), warn: false }),
        }
    };
    h.said.event(dialog(31, false));
    assert_eq!(d.next_of("pending")["reads"]["files"], 31);
    h.said.event(dialog(1, true));
    let again = d.next_of("pending");
    assert_eq!(
        (again["id"].clone(), again["reads"]["findings"][0]["added"].clone()),
        (json!("pnd_e"), json!(true))
    );
    let (_, welcome) = h.allowed("d_2");
    let held = welcome["pendings"].as_array().unwrap();
    assert_eq!(held.len(), 1, "one dialog, said again: {welcome}");
    assert_eq!(held[0]["reads"]["files"], 1);
}

/// A project whose ledger holds one Ask, the Eval that judged it and the Seal
/// that accepted it, `sel_1`.
fn sealed(project: &std::path::Path) {
    let rf = project.join(".fab7/rf");
    std::fs::create_dir_all(&rf).unwrap();
    let events = [
        json!({"type": "ask.compiled", "id": "ask_1", "time": "2026-10-08T09:00:00Z",
               "data": {"title": "build number", "delivery_mode": "native_dispatch",
                        "host": {"name": "sh"}}}),
        json!({"type": "eval.opened", "id": "evl_1", "time": "2026-10-08T10:00:00Z",
               "data": {"basis": {"asks": ["ask_1"]}}}),
        json!({"type": "eval.completed", "id": "evl_1", "time": "2026-10-08T10:05:00Z",
               "data": {"basis": {"asks": ["ask_1"]}, "report": {"faults": 0}}}),
        json!({"type": "seal.created", "id": "sel_1", "time": "2026-10-08T10:06:00Z",
               "actor": {"kind": "human", "id": "local-user"},
               "links": [{"rel": "seals", "id": "ask_1"}, {"rel": "seals", "id": "evl_1"}],
               "data": {"basis": {"asks": ["ask_1"]}, "disposition": "accepted"}}),
    ];
    let lines: String = events.iter().map(|e| format!("{e}\n")).collect();
    std::fs::write(rf.join("ledger.jsonl"), lines).unwrap();
}

/// A Seal's Details come at once, from what weft remote holds, with nothing
/// said yet of its receipt; whether the receipt holds is its own read, which
/// the daemon answers once RingFrame has checked it. Its count reaches the
/// device that asked, and a check that failed carries RingFrame's reason.
#[test]
fn a_seals_facts_come_at_once_and_a_failed_receipt_check_says_why() {
    use weft_proto::{Event, StepState};
    let mut h = Harness::proxied("receipt");
    sealed(&h.dir.join("work"));
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    d.agents(1);
    // The daemon's next read is not heard: RingFrame's count, then its refusal.
    let mut check = job("seal_check", StepState::Now, 40, None);
    check.on = weft_proto::JobOn { seal: Some("sel_1".into()), ..Default::default() };
    check.call = Some(common::Said::THIS_CALL);
    let why = "RingFrame could not read the Seal's record (seals/sel_1.json is missing)";
    h.said.refuse("read", vec![Event::Progress { job: check }], "refused", why);

    d.send(json!({"type": "read", "what": "seal", "project": "work", "seal": "sel_1", "req": 1}));
    let facts = d.answer(1);
    assert_eq!(facts["type"], "ok", "{facts}");
    let seal = &facts["seal"];
    assert_eq!(
        (seal["n"].clone(), seal["decision"].clone(), seal["eval"]["eval"].clone()),
        (json!(1), json!("accepted"), json!("evl_1"))
    );
    assert_eq!((seal["receipt"].clone(), seal["holds"].clone()), (json!("sel_1"), Value::Null));
    assert_eq!(
        (seal["total"].clone(), seal["asks"][0]["unit"].clone()),
        (json!(1), json!("ask_1"))
    );

    d.send(
        json!({"type": "read", "what": "receipt", "project": "work", "seal": "sel_1", "req": 2}),
    );
    let told = d.next_of("progress");
    assert_eq!(
        (told["req"].clone(), told["job"]["kind"].clone(), told["job"]["on"]["seal"].clone()),
        (json!(2), json!("seal_check"), json!("sel_1")),
        "{told}"
    );
    let failed = d.answer(2);
    assert_eq!(
        (failed["type"].clone(), failed["code"].clone(), failed["message"].clone()),
        (json!("error"), json!("refused"), json!(why))
    );

    // Through to the daemon: RingFrame checks it, and finds no receipt on disk.
    d.send(
        json!({"type": "read", "what": "receipt", "project": "work", "seal": "sel_1", "req": 3}),
    );
    let checked = d.answer(3);
    assert_eq!(
        checked["receipt"],
        json!({"receipt": "sel_1", "holds": false, "broken": ["the receipt is not on disk"]}),
        "{checked}"
    );

    // A Seal that is not here is refused before anything is checked.
    d.send(
        json!({"type": "read", "what": "receipt", "project": "work", "seal": "sel_9", "req": 4}),
    );
    assert_eq!(d.answer(4)["code"], "no_seal");
}
