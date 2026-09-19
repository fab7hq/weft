//! Drive Weft through the whole loop against a real harness.
//!
//! Verification aid for gate W4. It types the way a person would and reads the
//! screen to decide when to move on. The ledger is the ground truth.
//!
//!   cargo run --example loop_drive -- /path/to/project claude

use std::process::Command;
use std::time::{Duration, Instant};
use weft::pane::Pane;

/// The agent configurations this probe runs against. **Test settings only.**
/// Weft chooses no model and carries no default: in the product the person's
/// own harness configuration decides, and Weft passes their arguments through
/// untouched (ADR-0005).
const CLAUDE_UNDER_TEST: &str = "claude --model sonnet --effort medium";
#[allow(dead_code)]
const CODEX_UNDER_TEST: &str = "codex -m gpt-5.6-luna -c model_reasoning_effort=medium";

const TOGGLE: &[u8] = &[0x1d]; // Ctrl+], what a bare PTY negotiates down to
const ENTER: &[u8] = b"\r";

struct Drive {
    pane: Pane,
    project: String,
}

impl Drive {
    fn screen(&self) -> String {
        self.pane.with_screen(|s| s.contents())
    }

    fn send(&mut self, bytes: &[u8]) {
        self.pane.send(bytes).expect("send");
        std::thread::sleep(Duration::from_millis(400));
    }

    fn wait_screen(&mut self, needle: &str, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if self.screen().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        false
    }

    fn rf(&self, args: &[&str]) -> String {
        let out = Command::new("ringframe")
            .args(["--workspace", &self.project])
            .args(args)
            .output()
            .expect("ringframe");
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// Wait for the ledger itself to say something happened.
    fn wait_ledger(&self, kind: &str, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            let path = std::path::Path::new(&self.project).join(".fab7/rf/ledger.jsonl");
            if let Ok(text) = std::fs::read_to_string(&path) {
                if text.contains(&format!("\"type\":\"{kind}\"")) {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        false
    }

    /// Answer whatever chooser the harness has put up, the way a person would.
    fn answer_chooser(&mut self, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            let s = self.screen();
            if s.contains("Press t to trust") {
                self.send(b"t");
                std::thread::sleep(Duration::from_secs(1));
                if self.screen().contains("Press t to trust") {
                    self.send(b"\x1b");
                }
                return true;
            }
            if s.contains("Proceed") || s.contains("Accept") || s.contains("Seal") || s.contains("❯ 1.") {
                self.send(ENTER);
                return true;
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        false
    }

    /// Select the row the board says is ready, then send it. v2 dims `[S]END`
    /// for a row with nothing to send and names the right row in the hint,
    /// which is the affordance this uses rather than guessing at the layout.
    fn send_the_ready_one(&mut self, rows: usize) -> bool {
        for _ in 0..rows.max(1) {
            self.send(b"s");
            std::thread::sleep(Duration::from_millis(400));
            let s = self.screen();
            if s.contains("[Enter] DO IT") {
                return true;
            }
            if !s.contains("is the one ready to send") {
                return false;
            }
            self.send(b"\x1b[B"); // down, to the row the hint named
        }
        false
    }

    fn report(&mut self, label: &str) {
        println!("\n═══ {label} ═══\n{}", self.screen());
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = args.next().expect("a project directory");
    let harness = args.next().unwrap_or_else(|| CLAUDE_UNDER_TEST.into());
    let intent = args.next().unwrap_or_else(|| {
        "make health() report the real package version instead of the literal \"dev\"".into()
    });

    let bin = concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/weft");
    // Weft takes the project and the agents to run as arguments.
    let pane = Pane::spawn_args("weft", bin, &[&project, &harness], &project, 40, 120)?;
    let mut d = Drive { pane, project: project.clone() };

    d.wait_screen("in Weft", 25);

    // Startup questions belong to the agent, so they are answered there: an
    // update offer, then the folder-trust question. A real person would do the
    // same; Weft itself never answers either.
    d.send(TOGGLE);
    if d.wait_screen("Update available", 20) {
        d.send(b"\x1b[B"); // 2. Skip
        d.send(ENTER);
        std::thread::sleep(Duration::from_secs(2));
    }
    // Codex shows a hooks panel at startup. RingFrame's prompt capture is
    // already trusted; an unrelated user hook may still want review, and the
    // panel holds focus until it is closed.
    if d.wait_screen("Press t to trust", 25) {
        d.send(b"t");
        std::thread::sleep(Duration::from_secs(2));
        if d.screen().contains("Press t to trust") {
            d.send(b"\x1b"); // esc to close
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    if d.wait_screen("trust", 25) {
        // Claude Code defaults to "No, exit"; Codex defaults to "Yes, continue".
        if d.screen().contains("No, exit") {
            d.send(b"\x1b[B");
        }
        d.send(ENTER);
    }
    std::thread::sleep(Duration::from_secs(8));
    d.report("agent ready");
    d.send(TOGGLE);

    // 1. Ask.
    d.send(b"a");
    d.wait_screen("WHAT DO YOU WANT DONE?", 10);
    d.send(intent.as_bytes());
    d.send(ENTER);
    d.wait_screen("[Enter] DO IT", 10);
    d.report("confirmation before typing");
    d.send(ENTER);

    println!("\n── waiting for the skill to compile an Ask ──");
    let compiled = d.wait_ledger("ask.compiled", 420);
    println!("ask.compiled: {compiled}");
    d.answer_chooser(120);
    let confirmed = d.wait_ledger("ask.confirmed", 240);
    println!("ask.confirmed: {confirmed}");
    d.report("after the ask");

    // On a handoff route the board offers to type the prompt for the person.
    d.send(TOGGLE);
    if d.wait_screen("READY TO SEND", 20) {
        println!("\n── the board offers to send it ──");
        if d.send_the_ready_one(8) {
            d.report("ready to send");
            d.send(ENTER);
        }
    } else {
        d.send(TOGGLE);
    }

    // The harness now does the work. Give it room, answering anything it asks.
    println!("\n── the agent is working ──");
    for _ in 0..40 {
        d.answer_chooser(15);
        if d.screen().contains("esc to interrupt") {
            std::thread::sleep(Duration::from_secs(10));
        } else {
            break;
        }
    }
    d.report("work settled");

    // 2. Check.
    d.send(TOGGLE);
    d.send(b"c");
    if d.wait_screen("[Enter] DO IT", 10) {
        d.report("check confirmation");
        d.send(ENTER);
    }
    println!("\n── waiting for judges ──");
    let evaluated = d.wait_ledger("eval.completed", 900);
    println!("eval.completed: {evaluated}");
    for _ in 0..30 {
        d.answer_chooser(10);
        if !d.screen().contains("esc to interrupt") {
            break;
        }
        std::thread::sleep(Duration::from_secs(10));
    }
    d.report("after the check");

    // 3. Decide.
    d.send(TOGGLE);
    d.send(b"d");
    if d.wait_screen("[Enter] DO IT", 10) {
        d.send(ENTER);
    }
    d.answer_chooser(180);
    let sealed = d.wait_ledger("seal.created", 300);
    println!("seal.created: {sealed}");
    d.report("after the decision");

    println!("\n═══ ledger ═══");
    println!("{}", d.rf(&["ask", "list", "--minimal"]));
    println!("{}", d.rf(&["eval", "list", "--minimal"]));
    println!("{}", d.rf(&["ledger", "verify", "--minimal"]));
    println!("harness: {harness}");
    Ok(())
}
