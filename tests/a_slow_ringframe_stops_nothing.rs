//! A slow RingFrame holds up only what waits on it: while the crate's set-up,
//! preflight, Eval open, Seal check, Ask draft and Send compile wait on a slow
//! Git, the daemon goes on answering every other call. What waits is told as
//! a job once it has run a moment, with its call to the window that asked.
//!
//! Its own test binary because it puts a stand-in first on PATH and sets
//! `HOME`, which are global: one test, one process.

mod support;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

/// How long the stand-in takes over what it is slow at, unless told otherwise.
const SLOW: &str = "2";

/// A `git` that takes its time over `rev-parse --show-prefix`, which the
/// crate's preflight, Eval open and Seal check each ask: as many seconds as
/// the file `slow` beside it says. While a file `fail` is there, it fails it.
fn stand_in(dir: &Path) {
    let d = dir.display();
    let git = format!(
        "#!/bin/sh\ncase \"$*\" in *--show-prefix*) [ -e {d}/fail ] && exit 1; sleep $(cat {d}/slow) ;; esac\nexec /usr/bin/git \"$@\"\n"
    );
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("git");
    std::fs::write(&path, git).expect("stand-in");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    slow(dir, SLOW);
}

/// How many seconds the stand-in takes from now on.
fn slow(dir: &Path, seconds: &str) {
    std::fs::write(dir.join("slow"), seconds).expect("slow");
}

/// A job of `kind`, as `progress` says it.
fn job_of(kind: &'static str) -> impl FnMut(Event) -> Option<weft::protocol::Job> {
    move |e| match e {
        Event::Progress { job } if job.kind == kind => Some(job),
        _ => None,
    }
}

struct Client {
    stream: UnixStream,
    lines: Lines<UnixStream>,
    id: u64,
    /// The events read while waiting for an answer.
    seen: Vec<Event>,
}

impl Client {
    fn connect(socket: &Path) -> Client {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "no daemon");
            std::thread::sleep(Duration::from_millis(20));
        };
        let lines = Lines::new(stream.try_clone().expect("clone"));
        Client { stream, lines, id: 0, seen: Vec::new() }
    }

    fn call(&mut self, call: Call) -> u64 {
        self.id += 1;
        self.stream.write_all(&Line::Call { id: self.id, call }.encode()).expect("write");
        self.id
    }

    /// The answer to call `id`; the events before it are kept in `seen`.
    fn answer(&mut self, id: u64, within: Duration) -> serde_json::Value {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
            match self.lines.next() {
                Ok(Some(Line::Result { id: got, result })) if got == id => return result,
                Ok(Some(Line::Error { id: got, message, .. })) if got == id => {
                    panic!("call {id} refused: {message}")
                }
                Ok(Some(Line::Event(e))) => self.seen.push(e),
                _ => {}
            }
        }
        panic!("no answer to call {id} in {within:?}");
    }

    /// A call's answer, or its refusal's code.
    fn reply(&mut self, id: u64, within: Duration) -> Result<serde_json::Value, String> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
            match self.lines.next() {
                Ok(Some(Line::Result { id: got, result })) if got == id => return Ok(result),
                Ok(Some(Line::Error { id: got, code, .. })) if got == id => return Err(code),
                Ok(Some(Line::Event(e))) => self.seen.push(e),
                _ => {}
            }
        }
        panic!("no answer to call {id} in {within:?}");
    }

    #[track_caller]
    fn wait<T>(&mut self, mut pick: impl FnMut(Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
            if let Ok(Some(Line::Event(e))) = self.lines.next()
                && let Some(found) = pick(e)
            {
                return found;
            }
        }
        panic!("nothing arrived");
    }
}

/// How long the daemon takes to answer a new client's hello.
fn probe(socket: &Path) -> Duration {
    let mut c = Client::connect(socket);
    let asked = Instant::now();
    let id = c.call(Call::Hello { client: "probe".into(), protocol: PROTOCOL });
    c.answer(id, Duration::from_secs(10));
    asked.elapsed()
}

#[test]
fn a_slow_ringframe_holds_up_only_what_waits_on_it() {
    let tmp = std::env::temp_dir().join(format!("weft-slow-rf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (bin, fresh, root) = (tmp.join("bin"), tmp.join("fresh"), tmp.join("project"));
    let home = tmp.join("home");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::create_dir_all(&fresh).expect("fresh");
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("HOME", &home);
    }
    let ask = support::real_ask(&root, &home, "sh");
    let _ = support::git_repo(&fresh);
    stand_in(&bin);
    // SAFETY: as above.
    unsafe {
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    }

    let socket = protocol::private_socket("weft-slow-rf");
    let agent = support::agent(&support::agents_dir(&socket));
    let harnesses = support::stub_harness_proving(&socket.with_extension("judges"), "sh");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let config = listening.with_extension("no-config.toml");
        let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
    });
    let mut a = Client::connect(&socket);
    let hello = a.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
    a.answer(hello, Duration::from_secs(5));
    let open = |a: &mut Client, path: &Path| {
        a.call(Call::Open { rows: 24, cols: 80, path: path.to_string_lossy().into(), raw: true })
    };
    let quick = Duration::from_millis(500);

    // Opening a project RingFrame is not set up in: its set-up, then preflight.
    let opening = open(&mut a, &fresh);
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on init");
    let opened = a.answer(opening, Duration::from_secs(10));
    assert_eq!(opened["setup"], serde_json::Value::Null, "set up by its init: {opened}");

    // A set-up project: preflight only. A window already in it stages
    // a send meanwhile: the opening window gets it in its answer, and not as
    // an event before the answer too.
    let mut b = Client::connect(&socket);
    let hello = b.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
    b.answer(hello, Duration::from_secs(5));
    let opening = open(&mut b, &root);
    b.answer(opening, Duration::from_secs(10));
    let opening = open(&mut a, &root);
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on preflight");
    let staged = b.call(Call::Stage {
        pane: 0,
        bytes: b"hello".to_vec(),
        how: weft_core::inject::Handoff::Whole,
        what: "a send".into(),
        why: String::new(),
    });
    b.answer(staged, Duration::from_secs(5));
    a.seen.clear();
    let opened = a.answer(opening, Duration::from_secs(10));
    assert_eq!(opened["pendings"].as_array().map(Vec::len), Some(1), "{opened}");
    assert!(
        !a.seen.iter().any(|e| matches!(e, Event::Pending { .. })),
        "the send came before the answer too: {:?}",
        a.seen
    );

    // Eval: its dialog is staged.
    a.call(Call::StartAgent { harness: "sh".into(), spec: agent, session: None });
    a.wait(|e| matches!(e, Event::Added { .. }).then_some(()));
    support::pane_says(&socket, &support::agents_dir(&socket), &root, "sh", 0, "s1", "ready");
    a.wait(|e| match e {
        Event::Agents { panes, .. }
            if panes.first().copied().flatten() == Some(weft::turns::Turn::Ready) =>
        {
            Some(())
        }
        _ => None,
    });
    let check = a.call(Call::Act {
        act: "check".into(),
        unit: Some(ask.clone()),
        pane: None,
        text: None,
        eval: None,
        start: None,
        draft: None,
    });
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on eval open");
    let staged = a.answer(check, Duration::from_secs(10));
    assert!(staged["pending"].is_string(), "the Eval staged: {staged}");

    // A second yes to the same work's Eval while the first is still opening
    // opens nothing more: it is refused, and says why.
    let eval_act = || Call::Act {
        act: "check".into(),
        unit: Some(ask.clone()),
        pane: None,
        text: None,
        eval: None,
        start: None,
        draft: None,
    };
    let yes = |pending: &serde_json::Value| Call::Resolve {
        pending: pending.as_str().unwrap().to_string(),
        yes: true,
        to: None,
        start: None,
    };
    //
    // An open that fails at once is told all the same, failed, to every
    // window; the one that asked gets its call with it.
    std::fs::write(bin.join("fail"), "").expect("fail");
    let failed = a.call(yes(&staged["pending"]));
    let told = b.wait(|e| job_of("eval_open")(e).filter(|j| j.end.is_some()));
    assert_eq!(told.on.work.as_deref(), Some(ask.as_str()));
    assert_eq!((told.call, told.end.map(|e| e.ok)), (None, Some(false)), "a failed open ends");
    a.seen.clear();
    assert!(a.reply(failed, Duration::from_secs(30)).is_err(), "the open failed");
    let own = a.seen.iter().find_map(|e| match e {
        Event::Progress { job } if job.kind == "eval_open" => Some(job.clone()),
        _ => None,
    });
    assert_eq!(own.and_then(|j| j.call), Some(failed), "the asker's copy carries its call");
    std::fs::remove_file(bin.join("fail")).expect("fail");

    let staged = a.call(eval_act());
    let staged = a.answer(staged, Duration::from_secs(10));
    slow(&bin, "4");
    let first = a.call(yes(&staged["pending"]));
    let began = Instant::now();
    let running = b.wait(job_of("eval_open"));
    assert!(began.elapsed() >= Duration::from_millis(350), "sent only once it has run a moment");
    assert_eq!(
        (running.end.is_none(), running.steps[0].key.as_str()),
        (true, "step.reading_change")
    );
    let again = a.call(eval_act());
    let again = a.answer(again, Duration::from_secs(10));
    let second = a.call(yes(&again["pending"]));
    assert_eq!(a.reply(second, Duration::from_secs(5)), Err("opening".to_string()));

    // A window that opens the project meanwhile finds it opening in its answer.
    std::thread::sleep(Duration::from_millis(300));
    slow(&bin, "0");
    let mut c = Client::connect(&socket);
    let hello = c.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
    c.answer(hello, Duration::from_secs(5));
    let joined = open(&mut c, &root);
    let joined = c.answer(joined, Duration::from_secs(10));
    let jobs = joined["jobs"].as_array().cloned().unwrap_or_default();
    let opening: Vec<_> =
        jobs.iter().filter(|j| j["kind"] == "eval_open" && j["end"].is_null()).collect();
    assert_eq!(opening.len(), 1, "{joined}");
    assert_eq!(opening[0]["on"]["work"], serde_json::json!(ask));
    assert!(
        !jobs.iter().any(|j| j["end"]["ok"] == false),
        "the open that failed went when its Eval was pressed again: {joined}"
    );

    assert!(a.reply(first, Duration::from_secs(30)).is_ok(), "the open succeeded");
    let starting = |j: &weft::protocol::Job| j.steps[3].state == weft::protocol::StepState::Now;
    let opened = b.wait(|e| job_of("eval_open")(e).filter(starting));
    assert_eq!(opened.steps[3].key, "step.starting_readers", "RingFrame's part is done");
    slow(&bin, SLOW);

    // An Ask: Sense and RingFrame draft it off the run loop, and its Send
    // records it off the run loop too. The window that asked is told the
    // job with its call; the other, without.
    let draft = weft_core::ask::Draft {
        route: Some("human_handoff".into()),
        task: Some("implement".into()),
        rules: Some(Vec::new()),
        ..Default::default()
    };
    let asked = a.call(Call::Act {
        act: "ask".into(),
        unit: None,
        pane: Some(0),
        text: Some("Ship more.".into()),
        eval: None,
        start: None,
        draft: Some(draft),
    });
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on the Ask's draft");
    a.seen.clear();
    let drafted = a.answer(asked, Duration::from_secs(10));
    assert!(drafted["pending"].is_string(), "the Ask staged: {drafted}");
    let own = a.seen.iter().find_map(|e| match e {
        Event::Progress { job } if job.kind == "ask" => Some(job.clone()),
        _ => None,
    });
    let own = own.expect("the draft ran past a moment: its job was told");
    assert_eq!(own.call, Some(asked));
    assert_eq!(own.steps[0].key, "step.reading_words");
    let sent = a.call(yes(&drafted["pending"]));
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on the Send's record");
    let theirs = b.wait(job_of("send"));
    assert_eq!(theirs.call, None, "only the asker's copy carries its call");
    assert_eq!(theirs.steps[0].key, "step.recording_ask");
    a.answer(sent, Duration::from_secs(10));

    // A receipt's check.
    let ws = ringframe::api::workspace(&root).expect("workspace");
    let sealed = ringframe::api::seal_create(
        &ws,
        ringframe::api::SealCreate {
            disposition: "accepted",
            eval_id: None,
            reason: None,
            note: None,
            answers: None,
        },
    )
    .expect("sealed");
    let read = a.call(Call::Read {
        what: "seal".into(),
        unit: sealed.seal_id,
        pane: None,
        lines: None,
        before: None,
        after: None,
        q: None,
    });
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on seal check");
    let checked = a.answer(read, Duration::from_secs(10));
    assert_eq!(checked["fresh"], true, "{checked}");

    a.call(Call::Shutdown);
    std::fs::remove_dir_all(&tmp).ok();
}
