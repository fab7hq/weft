//! Files an Eval should skip: RingFrame's read check, run off the run loop
//! when a project opens, when an Eval dialog is made and when a client asks
//! again, and the one line a person takes, written only when the latest
//! check proposed it.

use super::*;

use weft_core::skip::{Finding, Fix};

/// What a project's read check last found.
#[derive(Default)]
pub(super) struct Skip {
    /// The project's findings from the latest check kept, nested
    /// repositories among them.
    pub(super) findings: Vec<Finding>,
    /// The number of the last check begun, and of the last one kept: one
    /// that ends after a later one began comes too late to keep.
    begun: u64,
    kept: u64,
    /// How many checks run now.
    running: usize,
}

impl Skip {
    /// The findings a project's clients are told: no nested repository.
    pub(super) fn shown(&self) -> Vec<Finding> {
        self.findings.iter().filter(|f| f.in_settings()).cloned().collect()
    }

    /// Whether the latest check of the project proposed this line.
    fn proposes(&self, fix: &Fix) -> bool {
        self.findings.iter().any(|f| f.fix.as_ref() == Some(fix))
    }

    /// A check begun now: its number.
    pub(super) fn begin(&mut self) -> u64 {
        self.begun += 1;
        self.running += 1;
        self.begun
    }
}

impl Session {
    /// Check the project, and the work of each Eval dialog open in it, off
    /// the run loop. `call` is answered with the project's findings once
    /// the check is done.
    pub(super) fn check_skip(&mut self, at: usize, call: Option<(u64, u64)>) {
        let p = &mut self.projects[at];
        let check = p.skip.begin();
        let asks: Vec<String> =
            p.waiting.iter().filter_map(|w| w.reads.as_ref()).map(|r| r.ask.clone()).collect();
        let (root, tx) = (p.root.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let checked = Box::new(crate::reads::check(&root, &asks));
            let _ = tx.send(Wake::SkipChecked { project: at, check, checked, call });
        });
    }

    /// A project opened: checked unless a check runs already.
    pub(super) fn check_skip_on_open(&mut self, at: usize) {
        if self.projects[at].skip.running == 0 {
            self.check_skip(at, None);
        }
    }

    /// A check ended: kept when no later one has been, every client told
    /// when what it found moved, and each open Eval dialog of a work it
    /// checked sent again with its reads, under the same id.
    pub(super) fn skip_checked(
        &mut self,
        at: usize,
        check: u64,
        checked: crate::reads::Checked,
        call: Option<(u64, u64)>,
    ) {
        let p = &mut self.projects[at];
        p.skip.running = p.skip.running.saturating_sub(1);
        if check > p.skip.kept && !p.closed {
            p.skip.kept = check;
            let moved = p.skip.findings != checked.project;
            p.skip.findings = checked.project;
            let mut again = Vec::new();
            for (ask, preview, work) in checked.works {
                for w in p.waiting.iter_mut() {
                    let Some(reads) = w.reads.as_mut().filter(|r| r.ask == ask) else { continue };
                    reads.checked(preview.clone(), work.clone());
                    w.payload = reads.payload();
                    again.push(Out::Pending {
                        id: w.id.clone(),
                        pane: w.pane,
                        what: w.what.clone(),
                        why: w.why.clone(),
                        payload: w.payload.clone(),
                        sends: w.sends.clone(),
                        go: w.go.clone(),
                    });
                }
            }
            if moved {
                let findings = self.projects[at].skip.shown();
                self.clients.broadcast(at, &Out::Skip { findings });
            }
            for message in again {
                self.clients.broadcast(at, &message);
            }
        }
        if let Some((client, call_id)) = call {
            let findings = self.projects[at].skip.shown();
            let answer = Answer::Ok(serde_json::json!({ "findings": findings }));
            self.clients.send_to(client, &answer.line(call_id));
        }
    }

    /// Write the line a finding proposed, then check again. A line no
    /// check of this project or of an open Eval dialog's work proposed last
    /// is refused, and nothing is written.
    pub(super) fn add_skip_line(
        &mut self,
        (client, call_id): (u64, u64),
        at: usize,
        fix: Fix,
    ) -> Answer {
        let p = &mut self.projects[at];
        let proposed = p.skip.proposes(&fix)
            || p.waiting.iter().filter_map(|w| w.reads.as_ref()).any(|r| r.proposes(&fix));
        if !proposed {
            return Answer::No(
                "not_proposed",
                format!("Weft's latest check did not propose {} in {}", fix.line, fix.file),
            );
        }
        if let Err(crate::ringframe::Error::Refused { message, .. }) =
            crate::ringframe::add_skip_line(&p.root, &fix)
        {
            return Answer::No("refused", message);
        }
        for reads in p.waiting.iter_mut().filter_map(|w| w.reads.as_mut()) {
            reads.took(&fix);
        }
        self.check_skip(at, Some((client, call_id)));
        Answer::Later
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project opened is checked and its clients told what an Eval should
    /// skip, never a nested repository; only the line the check proposed is
    /// written, and once it is, the check runs again and finds nothing.
    #[test]
    fn a_project_opened_is_checked_and_only_a_proposed_line_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let (weft_dir, proj) = (tmp.path().join("weft"), tmp.path().join("proj"));
        std::fs::create_dir_all(&weft_dir).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(ok.status.success(), "git {args:?}");
        };
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::write(proj.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&proj, &["init", "-q"]);
        git(&proj, &["add", "-A"]);
        git(&proj, &["commit", "-qm", "init"]);
        // Output nobody committed, over what an Eval reads whole.
        std::fs::create_dir_all(proj.join("runs/a")).unwrap();
        let text = "a line of output, written by a run\n".repeat(1200);
        for i in 0..30 {
            std::fs::write(proj.join(format!("runs/a/{i}.log")), &text).unwrap();
        }
        // A repository of its own, with a change the project cannot see.
        let nested = proj.join("site");
        std::fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q"]);
        std::fs::write(nested.join("index.html"), "<p>new</p>\n").unwrap();

        let (sock, cfg) = (weft_dir.join("weft.sock"), weft_dir.join("config.toml"));
        std::fs::write(&cfg, "").unwrap();
        let world = crate::outside::Fixed::new()
            .with_release(Err("no release in this world".into()))
            .shared();
        let harnesses = weft_core::harness::fixture::harnesses();
        let (sock_srv, cfg_srv) = (sock.clone(), cfg.clone());
        std::thread::spawn(move || {
            let _ = Session::serve_in(&sock_srv, &cfg_srv, Some(harnesses), world);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !weft_proto::is_live(&sock) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let stream = UnixStream::connect(&sock).unwrap();
        let mut reader = Lines::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut skips: Vec<Vec<Finding>> = Vec::new();
        let mut ask = |id: u64, call: Call, skips: &mut Vec<Vec<Finding>>| {
            weft_proto::send(&mut writer, &Line::Call { id, call }.encode()).unwrap();
            loop {
                match reader.next() {
                    Ok(Some(Line::Result { id: got, result })) if got == id => return Ok(result),
                    Ok(Some(Line::Error { id: got, code, .. })) if got == id => return Err(code),
                    Ok(Some(Line::Event(Out::Skip { findings }))) => skips.push(findings),
                    Ok(Some(_)) => {}
                    other => panic!("no answer to {id}: {other:?}"),
                }
            }
        };
        ask(1, Call::Hello { client: "test".into(), protocol: PROTOCOL }, &mut skips).unwrap();
        let path = proj.display().to_string();
        let opened =
            ask(2, Call::Open { path, rows: 24, cols: 80, raw: true }, &mut skips).unwrap();
        let found = ask(3, Call::SkipCheck, &mut skips).unwrap();
        let found: Vec<Finding> = serde_json::from_value(found["findings"].clone()).unwrap();
        let said: Vec<(&str, &str)> =
            found.iter().map(|f| (f.kind.as_str(), f.path.as_str())).collect();
        assert_eq!(said, [("untracked_bulk", "runs/")], "no nested repository: {found:?}");
        // Checked as it opened: in its answer, or told once the check ended.
        let told = serde_json::from_value::<Vec<Finding>>(opened["skip"].clone()).unwrap();
        assert!(told == found || skips.last() == Some(&found), "{opened} {skips:?}");
        let line = found[0].fix.clone().unwrap();
        assert_eq!((line.file.as_str(), line.line.as_str()), (".gitignore", "/runs/"));

        let other = Call::SkipAdd { file: ".gitignore".into(), line: "/src/".into() };
        assert_eq!(ask(4, other, &mut skips), Err("not_proposed".into()));
        assert!(!proj.join(".gitignore").exists(), "nothing written");

        let add = Call::SkipAdd { file: line.file, line: line.line };
        let after = ask(5, add, &mut skips).unwrap();
        assert_eq!(after["findings"], serde_json::json!([]));
        assert_eq!(skips.last(), Some(&Vec::new()), "told it is found no more");
        let ignore = std::fs::read_to_string(proj.join(".gitignore")).unwrap();
        assert!(ignore.lines().any(|l| l == "/runs/"), "{ignore}");
        // The settings' decisions are read off the run loop: none yet.
        let decisions = ask(6, Call::Decisions, &mut skips).unwrap();
        assert_eq!(decisions["title"], "YOUR DECISIONS", "{decisions}");
        assert_eq!(decisions["list"], serde_json::json!([]), "{decisions}");
        let _ = UnixStream::connect(&sock).map(|mut s| {
            weft_proto::send(&mut s, &Line::Call { id: 1, call: Call::Shutdown }.encode())
        });
    }
}
