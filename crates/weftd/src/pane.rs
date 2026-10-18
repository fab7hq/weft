//! A pane: a real terminal running a harness.
//!
//! Weft launches the harness with the person's own environment and never
//! sandboxes it or alters its permissions.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use weft_core::inject::{self, Handoff, PaneState, Refusal, Step};
use weft_term::{Snapshot, Term};

/// Where a pane's output goes, numbered, once someone asks for the stream.
type Tap = Arc<Mutex<Option<Sender<(u64, Vec<u8>)>>>>;

/// The live screen as `weft-sense` reads it: the alternate screen or not,
/// `(rows, cols)`, the cursor's `(row, col)`, the last non-blank lines, and
/// the output sequence it was taken at.
pub struct Surface {
    pub alt: bool,
    pub size: (u16, u16),
    pub cursor: (u16, u16),
    pub bottom: Vec<String>,
    pub seq: u64,
}

pub struct Pane {
    pub title: String,
    term: Arc<Mutex<Term>>,
    /// The number of the last chunk the terminal took, moved only while its
    /// lock is held: a screen read under that lock holds every chunk up to
    /// this number and none after (remote.md D1).
    seq: Arc<AtomicU64>,
    /// Installed only when someone asks for the stream, so a pane nobody is
    /// listening to never accumulates output it will not be asked for.
    tap: Tap,
    master: Box<dyn MasterPty + Send>,
    /// Shared with the terminal, which answers what the harness asks about
    /// it (where the cursor is, what it is, its colours) as a terminal would.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    injecting: Arc<AtomicBool>,
}

/// What types into a pane, apart from the pane: its terminal, read to see
/// what the agent shows, and its writer. Typing waits on the agent (a paste
/// landing, a mode entered), so it runs off the daemon's loop.
#[derive(Clone)]
pub struct Typist {
    term: Arc<Mutex<Term>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    injecting: Arc<AtomicBool>,
    /// Whether the agent was running when this was taken.
    running: bool,
}

impl Pane {
    pub fn spawn(
        title: impl Into<String>,
        program: &str,
        cwd: &str,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        Self::spawn_args(title, program, &[], cwd, &[], rows, cols)
    }

    pub fn spawn_args(
        title: impl Into<String>,
        program: &str,
        args: &[&str],
        cwd: &str,
        env: &[(&str, Option<String>)],
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        let pair =
            native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        // `None` keeps a variable the daemon inherited out of the pane.
        for (k, v) in env {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            }
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let answer = Arc::clone(&writer);
        // A harness that asks about its terminal waits for the answer, and
        // some give up and quit without it.
        let term = Arc::new(Mutex::new(Term::new(rows, cols, 10_000, move |reply| {
            if let Ok(mut w) = answer.lock() {
                let _ = w.write_all(reply);
                let _ = w.flush();
            }
        })));
        let seq = Arc::new(AtomicU64::new(0));
        let tap: Tap = Arc::new(Mutex::new(None));
        let mut reader = pair.master.try_clone_reader()?;
        let sink = Arc::clone(&term);
        let tap_reader = Arc::clone(&tap);
        let numbering = Arc::clone(&seq);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let bytes = buf[..n].to_vec();
                let mut number = 0;
                if let Ok(mut t) = sink.lock() {
                    t.feed(&bytes);
                    number = numbering.fetch_add(1, Ordering::SeqCst) + 1;
                }
                // The same bytes go to whoever is streaming this pane, so a
                // client rebuilds the screen from the harness's own output.
                if let Ok(mut t) = tap_reader.lock()
                    && let Some(sender) = t.as_ref()
                    && sender.send((number, bytes)).is_err()
                {
                    *t = None;
                }
            }
            if let Ok(mut t) = tap_reader.lock() {
                *t = None;
            }
        });

        Ok(Self {
            title: title.into(),
            term,
            seq,
            tap,
            master: pair.master,
            writer,
            child,
            injecting: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Every byte this pane prints, from now on, each chunk with its number.
    /// One listener at a time.
    pub fn stream_output(&mut self) -> Receiver<(u64, Vec<u8>)> {
        let (tx, rx) = channel();
        *self.tap.lock().expect("pane tap") = Some(tx);
        rx
    }

    /// End this agent. Called when the person closes its pane: the pane is
    /// the only way to see the process, so it must not outlive it.
    pub fn stop(&mut self) {
        if self.running() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// The terminal, for reading. Held briefly; the reader thread is writing.
    pub fn with_term<T>(&self, f: impl FnOnce(&Term) -> T) -> T {
        let term = self.term.lock().expect("pane terminal");
        f(&term)
    }

    /// The screen as escape codes that redraw it on a fresh terminal of its
    /// size, with up to `history` lines behind it, the number of the last
    /// chunk in it and that size, read under one lock.
    pub fn screen_at(&self, history: usize) -> (Vec<u8>, u64, (u16, u16)) {
        let term = self.term.lock().expect("pane terminal");
        (term.formatted(history), self.seq.load(Ordering::SeqCst), term.size())
    }

    /// The next frame for a device that last drew `last`: the change from
    /// it, or the whole visible screen (`full`) when there is no last, the
    /// size changed or the alternate screen switched. Read under one lock
    /// with the number of the last chunk in it.
    pub fn frame(&self, last: Option<&Snapshot>) -> Frame {
        let term = self.term.lock().expect("pane terminal");
        let (full, bytes) = match last.and_then(|l| term.diff(l)) {
            Some(change) => (false, change),
            None => (true, term.formatted(0)),
        };
        Frame { full, bytes, shown: term.snapshot(), seq: self.seq.load(Ordering::SeqCst) }
    }

    pub fn current_seq(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }

    /// Snapshot of the live screen for `weft-sense` Head 1.
    pub fn surface_snapshot(&self) -> Surface {
        let term = self.term.lock().expect("pane terminal");
        let (rows, cols) = term.size();
        let cursor = term.cursor();
        let alt = term.alt_screen();
        let mut lines: Vec<String> =
            term.rows().into_iter().filter(|l| !l.trim().is_empty()).collect();
        if lines.len() > 15 {
            lines.drain(..lines.len() - 15);
        }
        let seq = self.seq.load(Ordering::SeqCst);
        Surface { alt, size: (rows, cols), cursor, bottom: lines, seq }
    }

    /// The last `lines` of what the pane printed, scrollback then screen, as
    /// plain text: the history a device scrolls back through (remote.md D7).
    pub fn history(&self, lines: usize) -> String {
        self.term.lock().expect("pane terminal").history(lines)
    }

    /// Move back through what the pane has already printed.
    ///
    /// A harness that asks for no mouse reporting and no alternate screen
    /// leaves Weft — which has no terminal behind it to keep a scrollback —
    /// to keep one itself and move through it here.
    ///
    /// That only reaches what the harness let scroll off. One that prints
    /// inline has its whole history here. One that repaints its viewport
    /// instead sends nothing here, so this moves nothing: its history lives
    /// inside it, behind the key its harness file names (`transcript`).
    pub fn scroll(&mut self, delta: i32) {
        self.term.lock().expect("pane terminal").scroll(delta);
    }

    pub fn scroll_offset(&self) -> usize {
        self.term.lock().expect("pane terminal").scroll_offset()
    }

    /// Jump back to the live output.
    pub fn scroll_to_bottom(&mut self) {
        self.term.lock().expect("pane terminal").scroll_to_bottom();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        self.term.lock().expect("pane terminal").resize(rows, cols);
        Ok(())
    }

    /// A keystroke the person typed while focused on this pane. Passed through
    /// untouched — Weft reserves only its toggle key.
    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().expect("pane writer");
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub fn state(&mut self) -> PaneState {
        PaneState { running: self.running(), injecting: self.injecting.load(Ordering::SeqCst) }
    }

    /// What types into this pane, for a thread of its own.
    pub fn typist(&mut self) -> Typist {
        Typist {
            term: Arc::clone(&self.term),
            writer: Arc::clone(&self.writer),
            injecting: Arc::clone(&self.injecting),
            running: self.running(),
        }
    }

    /// Type a prompt on the person's behalf.
    ///
    /// Returns once the bytes are written. That is all it proves: the caller
    /// must never report this as delivery.
    pub fn inject(&mut self, payload: &[u8]) -> Result<Attempt, Refusal> {
        self.inject_as(payload, &Handoff::Whole)
    }

    /// Type a prompt the way its command requires: see [`Typist::inject_as`].
    pub fn inject_as(&mut self, payload: &[u8], how: &Handoff) -> Result<Attempt, Refusal> {
        self.typist().inject_as(payload, how)
    }

    /// Whether the pane has drawn something and then left it alone for
    /// `still`. One look: `last` is the screen when it last changed, kept by
    /// the caller between looks, so nothing waits here.
    pub fn gone_quiet(
        &self,
        last: &mut (std::time::Instant, String),
        still: std::time::Duration,
    ) -> bool {
        gone_quiet(&self.term, last, still)
    }
}

/// See [`Pane::gone_quiet`].
fn gone_quiet(
    term: &Mutex<Term>,
    last: &mut (std::time::Instant, String),
    still: std::time::Duration,
) -> bool {
    let screen = term.lock().expect("pane terminal").contents();
    if screen != last.1 {
        *last = (std::time::Instant::now(), screen);
        return false;
    }
    !screen.trim().is_empty() && last.0.elapsed() >= still
}

impl Typist {
    fn send(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut writer = self.writer.lock().expect("pane writer");
        writer.write_all(bytes)?;
        writer.flush()
    }

    fn contents(&self) -> String {
        self.term.lock().expect("pane terminal").contents()
    }

    /// Type a prompt the way its command requires.
    ///
    /// The bytes that reach the host are the same whichever way this goes; what
    /// changes is whether the host reads the command. Typed, it does; inside a
    /// folded paste, it may not, so a command is never pasted.
    pub fn inject_as(&self, payload: &[u8], how: &Handoff) -> Result<Attempt, Refusal> {
        // Only what can be said is typed: a prompt read from a project's files
        // could otherwise end its own paste and press keys (`!`, Enter).
        let command = match how {
            Handoff::EnterMode { command, .. } | Handoff::TypeCommand { command, .. } => {
                Some(command)
            }
            Handoff::Whole => None,
        };
        if !inject::sayable(payload) || command.is_some_and(|c| !inject::is_command(c)) {
            return Err(Refusal::ControlCharacters);
        }
        if let Handoff::EnterMode { command, active, .. } = how {
            // The mode first, on its own, and only believed when the host says
            // so. A mode switch is handled by the harness's TUI and submits no prompt, so
            // this adds nothing to the record.
            self.send(command.as_bytes()).map_err(|_| Refusal::NoProcess)?;
            // Enter goes in its own write, after the composer has been left
            // alone: written together they are one burst, and a host that
            // suppresses Enter during a burst takes it as a newline.
            std::thread::sleep(inject::SUBMIT_SETTLE);
            if !self.wait_for(active, inject::MODE_TIMEOUT)? {
                return Err(Refusal::ModeNotEntered);
            }
        }
        let typed = match how {
            Handoff::TypeCommand { command, .. } => Some(format!("{command} ")),
            _ => None,
        };
        if let Some(text) = &typed {
            // Typed, not pasted: a pasted command is not read as one.
            self.send(text.as_bytes()).map_err(|_| Refusal::NoProcess)?;
            std::thread::sleep(inject::ENTER_DELAY);
        }
        self.write_and_submit(how.body(payload))
    }

    /// Press Enter and wait for the mode it enters. A host may already name
    /// the mode in a popup for the command typed (`/plan  switch to Plan
    /// mode`), and may keep part of its screen moving (a spinner), so the mode
    /// counts once the lines that name it are not the ones that did before
    /// the Enter, and the body then waits [`inject::SUBMIT_SETTLE`]: input
    /// that arrives while the host is still switching is dropped.
    fn wait_for(&self, active: &str, timeout: std::time::Duration) -> Result<bool, Refusal> {
        let naming = |screen: String| -> Vec<String> {
            screen.lines().filter(|l| l.contains(active)).map(|l| l.trim().to_string()).collect()
        };
        let before = naming(self.contents());
        self.send(b"\r").map_err(|_| Refusal::NoProcess)?;
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let now = naming(self.contents());
            if !now.is_empty() && now != before {
                std::thread::sleep(inject::SUBMIT_SETTLE);
                return Ok(true);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(false)
    }

    fn write_and_submit(&self, payload: &[u8]) -> Result<Attempt, Refusal> {
        let state =
            PaneState { running: self.running, injecting: self.injecting.load(Ordering::SeqCst) };
        inject::check(&state)?;
        self.injecting.store(true, Ordering::SeqCst);
        let bracketed = self.term.lock().expect("pane terminal").bracketed_paste();
        let steps = inject::compose(payload, bracketed);
        // The screen before the paste, to tell whether the paste changed it.
        let mut before = (std::time::Instant::now(), String::new());
        gone_quiet(&self.term, &mut before, inject::COMPOSER_SETTLE);
        for step in &steps {
            match step {
                Step::Write(bytes) => {
                    if self.send(bytes).is_err() {
                        self.injecting.store(false, Ordering::SeqCst);
                        return Err(Refusal::NoProcess);
                    }
                }
                Step::Wait(d) => {
                    // A fixed pause is not enough: a harness that is still
                    // settling drops the Enter and the prompt sits unsent in
                    // its composer. Wait for the paste to land instead.
                    std::thread::sleep(*d);
                    if !self.paste_landed(&before.1, inject::PASTE_TIMEOUT) {
                        // The screen never changed, so the paste did not land
                        // where Enter would send it. Never a blind retry.
                        self.injecting.store(false, Ordering::SeqCst);
                        return Ok(Attempt { bytes: payload.len(), bracketed, submitted: false });
                    }
                }
            }
        }
        self.injecting.store(false, Ordering::SeqCst);
        Ok(Attempt { bytes: payload.len(), bracketed, submitted: true })
    }

    /// Whether the screen changed from `before` and then held still for
    /// [`inject::COMPOSER_SETTLE`], so Enter lands on a composer that took
    /// the paste rather than one still taking it. Only whether it changed is
    /// compared, never what it says: a harness may show a long
    /// paste whole, folded or scrolled. Changed but never still (a spinner)
    /// is taken as landed when the time is up; never changed is not.
    fn paste_landed(&self, before: &str, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut last = (std::time::Instant::now(), before.to_string());
        let mut changed = false;
        while std::time::Instant::now() < deadline {
            let quiet = gone_quiet(&self.term, &mut last, inject::COMPOSER_SETTLE);
            changed |= last.1 != before;
            if changed && quiet {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        changed
    }
}

/// A screen frame for a device: see [`Pane::frame`].
pub struct Frame {
    pub full: bool,
    pub bytes: Vec<u8>,
    /// What the device shows once it draws this frame.
    pub shown: Snapshot,
    pub seq: u64,
}

/// An injection that was *attempted*. Never evidence that it arrived: only
/// what the harness reports it received is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub bytes: usize,
    pub bracketed: bool,
    /// Whether Enter was sent at all. It is not when the paste never changed
    /// the screen: the text may be in the composer, and the person is told.
    pub submitted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Pane {
        let mut p = Pane::spawn("test", "/bin/sh", "/tmp", 24, 80).expect("spawn");
        p.send(format!("{script}\n").as_bytes()).expect("send");
        std::thread::sleep(std::time::Duration::from_millis(400));
        p
    }

    #[test]
    fn a_pane_runs_a_real_process_and_renders_its_output() {
        let pane = sh("printf 'hello from a pane'");
        let text = pane.with_term(|t| t.contents());
        assert!(text.contains("hello from a pane"), "screen was: {text:?}");
    }

    /// A harness that asks where the cursor is gets the answer a terminal
    /// gives, from the pane's own screen; without it, one quits ("The cursor
    /// position could not be read within a normal duration").
    #[test]
    fn a_pane_answers_a_harness_that_asks_where_the_cursor_is() {
        let script =
            "printf 'abc\\033[6n'; IFS= read -r -t 3 -d R reply; printf ' got=%s;' \"${reply#*[}\"";
        let mut pane =
            Pane::spawn_args("test", "bash", &["-c", script], "/tmp", &[], 24, 80).expect("spawn");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let text = pane.with_term(|t| t.contents());
        assert!(text.contains("got=1;4;"), "the cursor sat after `abc`, row 1 column 4: {text:?}");
        pane.stop();
    }

    #[test]
    fn a_pane_does_not_inherit_a_variable_given_as_none() {
        let script = "printf 'home=%s pane=%s;' \"${HOME-unset}\" \"$WEFT_PANE\"";
        let env = [("HOME", None), ("WEFT_PANE", Some("7".to_string()))];
        let mut pane =
            Pane::spawn_args("test", "bash", &["-c", script], "/tmp", &env, 24, 80).expect("spawn");
        std::thread::sleep(std::time::Duration::from_millis(500));
        let text = pane.with_term(|t| t.contents());
        assert!(text.contains("home=unset pane=7;"), "screen was: {text:?}");
        pane.stop();
    }

    /// What Sense reads off a pane (the bottom rows, the cursor, the
    /// alternate screen) is what vt100 showed for the same recorded output:
    /// the model was trained on that text. Rows end at their last character.
    #[test]
    fn sense_sees_the_rows_it_was_trained_on() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/screens");
        for (name, rows, cols) in [("claude", 30, 100), ("codex", 30, 100), ("less", 24, 80)] {
            let golden: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.join(format!("{name}.text.json"))).unwrap(),
            )
            .unwrap();
            let want = golden["frames"].as_array().unwrap().last().unwrap();
            // The recorded bytes exactly as recorded: no newline translation.
            let script = format!(
                "stty -opost -echo; cat '{}'; sleep 5",
                dir.join(format!("{name}.bin")).display()
            );
            let mut pane =
                Pane::spawn_args("test", "sh", &["-c", &script], "/tmp", &[], rows, cols)
                    .expect("spawn");
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let Surface { alt, cursor, bottom, .. } = pane.surface_snapshot();
            let want_bottom: Vec<&str> = want["bottom"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().trim_end_matches(' '))
                .collect();
            assert_eq!(bottom, want_bottom, "{name}");
            assert_eq!(alt, want["alternate"].as_bool().unwrap(), "{name}");
            assert_eq!(
                [cursor.0 as u64, cursor.1 as u64],
                [want["cursor"][0].as_u64().unwrap(), want["cursor"][1].as_u64().unwrap()],
                "{name}"
            );
            pane.stop();
        }
    }

    /// A harness asking what its terminal is (DA1) and what its background
    /// is (OSC 11) gets one answer to each, and nothing more.
    #[test]
    fn a_pane_answers_what_it_is_and_its_background_once() {
        let script = "stty -echo; printf '\\033[c'; IFS= read -r -t 3 -d c da; \
             printf '\\033]11;?\\007'; IFS= read -r -t 3 -d $'\\a' bg; \
             IFS= read -r -t 1 -n 1 more; \
             printf ' da=%s; bg=%s; more=%d;' \"${da#*[}\" \"${bg#*;}\" \"${#more}\"";
        let mut pane =
            Pane::spawn_args("test", "bash", &["-c", script], "/tmp", &[], 24, 200).expect("spawn");
        std::thread::sleep(std::time::Duration::from_millis(2500));
        let text = pane.with_term(|t| t.contents());
        assert!(text.contains("da=?6;"), "a VT102's identity: {text:?}");
        assert!(text.contains("bg=rgb:0505/0808/0f0f;"), "the Atlas background: {text:?}");
        assert!(text.contains("more=0;"), "one answer each: {text:?}");
        pane.stop();
    }

    #[test]
    fn closing_a_pane_ends_its_process() {
        let mut pane = sh("sleep 30");
        assert!(pane.running());
        pane.stop();
        assert!(!pane.running(), "nothing is left running behind a closed pane");
    }

    #[test]
    fn a_pane_reports_when_its_process_is_gone() {
        let mut pane = sh("exit 0");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!pane.running());
    }

    /// A host that, as Codex 0.161 does, shows its slash-command popup (which
    /// names the mode) as soon as `/plan` is typed, drops input that arrives
    /// before it has handled the Enter, and keeps a footer spinner turning so
    /// its screen is never still: the body goes in only once the mode is shown
    /// after the Enter.
    #[test]
    fn a_mode_named_by_the_popup_is_not_taken_for_the_mode_entered() {
        let host = HOST_DROPPING_EARLY_INPUT;
        let mut pane =
            Pane::spawn_args("test", "python3", &["-c", host], "/tmp", &[], 24, 80).expect("spawn");
        std::thread::sleep(std::time::Duration::from_millis(800));
        let how = Handoff::EnterMode {
            command: "/plan".into(),
            at: "/plan ".len(),
            active: "Plan mode".into(),
        };
        let attempt = pane.inject_as(b"/plan ship it", &how).expect("typed");
        assert!(attempt.submitted, "{attempt:?}");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let text = pane.with_term(|t| t.contents());
        assert!(text.contains("SUBMITTED[ship it]"), "the body was dropped: {text:?}");
        pane.stop();
    }

    const HOST_DROPPING_EARLY_INPUT: &str = r#"import os, sys, termios, tty, time, threading
def spin():
    frames = '|/-\\'
    i = 0
    while True:
        os.write(1, ('\x1b7\x1b[24;1H' + frames[i % 4] + '\x1b8').encode())
        i += 1
        time.sleep(0.1)
threading.Thread(target=spin, daemon=True).start()
fd = sys.stdin.fileno()
tty.setraw(fd)
out = lambda t: (os.write(1, t.encode()), None)
buf, mode = '', False
out('ready\r\n')
while True:
    c = os.read(fd, 1).decode()
    if not mode:
        if c == '\r' and buf == '/plan':
            time.sleep(0.3)
            termios.tcflush(fd, termios.TCIFLUSH)
            mode, buf = True, ''
            out('\r\nPlan mode on\r\n')
            continue
        buf += c
        out(c)
        if buf == '/plan':
            out('\r\n/plan  switch to Plan mode\r\n')
    else:
        if c == '\r':
            out('\r\nSUBMITTED[' + buf + ']\r\n')
            buf = ''
        else:
            buf += c
            out(c)
"#;

    /// A made-up composer: it takes a bracketed paste and draws it as
    /// `style` says, then, on Enter, writes what it took to `out` and says
    /// `SUBMITTED`. `scroll` shows only the last two lines under a line that
    /// counts the rest, as a composer that scrolls a long input; `fold` shows
    /// a placeholder in place of a paste over 200 characters; `plain` shows
    /// it all. Nothing here is any harness's own screen.
    const COMPOSER: &str = r#"import os, sys, tty
style, out = sys.argv[1], sys.argv[2]
fd = sys.stdin.fileno()
tty.setraw(fd)
w = lambda t: os.write(1, t.encode())
w('\x1b[?2004h> ')
buf, raw = '', b''
def draw():
    lines = buf.split('\n')
    if style == 'scroll' and len(lines) > 2:
        shown = ['(%d hidden above)' % (len(lines) - 2)] + lines[-2:]
    elif style == 'fold' and len(buf) > 200:
        shown = ['<<%d folded>>' % len(buf)]
    else:
        shown = lines
    w('\x1b[2J\x1b[H> ' + '\r\n  '.join(shown))
while True:
    raw += os.read(fd, 4096)
    while raw:
        if raw.startswith(b'\x1b[200~'):
            end = raw.find(b'\x1b[201~')
            if end < 0:
                break
            buf += raw[6:end].decode()
            raw = raw[end + 6:]
            draw()
        elif raw[:1] == b'\r':
            open(out, 'w').write(buf)
            w('\r\nSUBMITTED\r\n')
            buf, raw = '', raw[1:]
        elif raw[:1] == b'\x1b':
            break
        else:
            buf += raw[:1].decode()
            raw = raw[1:]
            draw()
"#;

    /// A prompt as long as a compiled one: 3500 bytes over 60 lines.
    fn long_prompt() -> Vec<u8> {
        let line = "Keep the change to the health endpoint and its tests; no more.";
        let mut text = String::from("Ship the health endpoint.\n");
        while text.len() < 3440 {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str("End of the Ask.");
        text.into_bytes()
    }

    /// Each composer gets Enter, and takes the prompt whole: none of their
    /// screens is read, only whether it changed and then held still.
    fn composer_takes(style: &str, how: &Handoff, payload: &[u8]) -> (Attempt, String) {
        let dir = tempfile::tempdir().expect("dir");
        let out = dir.path().join("took");
        let args = ["-c", COMPOSER, style, out.to_str().expect("path")];
        let mut pane =
            Pane::spawn_args("test", "python3", &args, "/tmp", &[], 24, 80).expect("spawn");
        std::thread::sleep(std::time::Duration::from_millis(600));
        let attempt = pane.inject_as(payload, how).expect("allowed");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !out.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        pane.stop();
        (attempt, std::fs::read_to_string(&out).unwrap_or_default())
    }

    /// A composer that scrolls a long prompt shows
    /// only its last lines, so the start of the prompt is never on screen.
    /// Weft used to look for it there and withheld Enter on a 1523-byte
    /// prompt. It now gets Enter, and what arrived is judged by the receipt.
    #[test]
    fn a_scrolling_composer_gets_enter_for_a_long_prompt() {
        let prompt = long_prompt();
        let (attempt, took) = composer_takes("scroll", &Handoff::Whole, &prompt);
        assert!(attempt.submitted, "{attempt:?}");
        assert_eq!(took.as_bytes(), prompt.as_slice());
    }

    #[test]
    fn a_folding_composer_gets_enter_for_a_long_prompt() {
        let prompt = long_prompt();
        let (attempt, took) = composer_takes("fold", &Handoff::Whole, &prompt);
        assert!(attempt.submitted, "{attempt:?}");
        assert_eq!(took.as_bytes(), prompt.as_slice());
    }

    #[test]
    fn a_plain_composer_gets_enter_for_a_short_prompt() {
        let (attempt, took) = composer_takes("plain", &Handoff::Whole, b"Ship it.");
        assert!(attempt.submitted, "{attempt:?}");
        assert_eq!(took, "Ship it.");
    }

    /// A command is typed in front of the body, never pasted, whatever the
    /// length and however the composer draws the body.
    #[test]
    fn a_command_is_typed_in_front_of_a_short_body_and_a_long_one() {
        let how = Handoff::TypeCommand { command: "/goal".into(), at: "/goal ".len() };
        let mut long = b"/goal ".to_vec();
        long.extend(long_prompt());
        for (style, payload) in [("plain", b"/goal ship it".to_vec()), ("scroll", long.clone())] {
            let (attempt, took) = composer_takes(style, &how, &payload);
            assert!(attempt.submitted, "{style}: {attempt:?}");
            assert_eq!(took.as_bytes(), payload.as_slice(), "{style}");
        }
    }

    /// A paste that never changes the screen did not land where Enter would
    /// send it: Enter is withheld, and the attempt says so.
    #[test]
    fn enter_is_withheld_when_the_paste_never_changes_the_screen() {
        let mut pane = sh("stty -echo; printf 'ready'; cat > /dev/null");
        std::thread::sleep(std::time::Duration::from_millis(400));
        let attempt = pane.inject(b"Return the real build number").expect("allowed to write");
        assert!(!attempt.submitted, "Enter withheld: {attempt:?}");
    }

    #[test]
    fn injected_bytes_reach_the_process_exactly() {
        let mut pane = sh("cat > /tmp/weft-inject-test.txt");
        let payload = "/plan Return the real build number";
        pane.inject(payload.as_bytes()).expect("allowed");
        std::thread::sleep(std::time::Duration::from_millis(200));
        pane.send(&[4]).expect("eof"); // Ctrl-D closes cat's stdin
        std::thread::sleep(std::time::Duration::from_millis(300));
        let written = std::fs::read_to_string("/tmp/weft-inject-test.txt").unwrap_or_default();
        assert_eq!(written.trim_end_matches(['\r', '\n']), payload);
        let _ = std::fs::remove_file("/tmp/weft-inject-test.txt");
    }

    #[test]
    fn a_pane_streams_its_output_to_a_listener() {
        let mut pane = Pane::spawn("test", "/bin/sh", "/tmp", 24, 80).expect("spawn");
        let stream = pane.stream_output();
        pane.send(b"printf 'streamed-to-the-server'\n").expect("send");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            if let Ok((_, chunk)) = stream.recv_timeout(std::time::Duration::from_millis(200)) {
                seen.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&seen).contains("streamed-to-the-server") {
                    break;
                }
            }
        }
        assert!(
            String::from_utf8_lossy(&seen).contains("streamed-to-the-server"),
            "the listener sees what the pane printed"
        );
        // And the pane's own screen still has it: the tap is a copy, not a move.
        assert!(pane.with_term(|t| t.contents()).contains("streamed-to-the-server"));
    }

    /// Each chunk is numbered as the terminal takes it, so a screen taken at
    /// one number holds every chunk up to it and none after (remote.md D1).
    #[test]
    fn each_chunk_a_pane_prints_is_numbered_in_order() {
        let mut pane = Pane::spawn("test", "/bin/sh", "/tmp", 24, 80).expect("spawn");
        let stream = pane.stream_output();
        pane.send(b"for i in $(seq 1 200); do echo numbered-$i; done\n").expect("send");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (mut seqs, mut seen) = (Vec::new(), Vec::new());
        while std::time::Instant::now() < deadline
            && !String::from_utf8_lossy(&seen).contains("numbered-200")
        {
            if let Ok((seq, chunk)) = stream.recv_timeout(std::time::Duration::from_millis(200)) {
                seqs.push(seq);
                seen.extend_from_slice(&chunk);
            }
        }
        assert!(seqs.len() > 1, "more than one chunk: {seqs:?}");
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "strictly increasing: {seqs:?}");
        let (_, at, _) = pane.screen_at(0);
        // The shell's prompt may follow as one more chunk, so at least.
        assert!(at >= *seqs.last().unwrap(), "the screen has taken every chunk sent: {at}");
        pane.stop();
    }

    #[test]
    fn a_pane_scrolls_back_through_what_it_printed() {
        let mut pane = sh("for i in $(seq 1 60); do echo line-$i; done");
        assert_eq!(pane.scroll_offset(), 0, "starts at the live output");
        assert!(pane.with_term(|t| t.contents()).contains("line-60"));

        pane.scroll(30);
        assert_eq!(pane.scroll_offset(), 30);
        let scrolled = pane.with_term(|t| t.contents());
        assert!(scrolled.contains("line-20"), "older output is reachable: {scrolled:?}");

        pane.scroll_to_bottom();
        assert_eq!(pane.scroll_offset(), 0);
        assert!(pane.with_term(|t| t.contents()).contains("line-60"));
    }

    #[test]
    fn scrolling_past_the_live_output_stops_at_it() {
        let mut pane = sh("echo only-one-line");
        pane.scroll(-50);
        assert_eq!(pane.scroll_offset(), 0, "never scrolls below the bottom");
    }

    #[test]
    fn a_pane_resizes_without_losing_its_screen() {
        let mut pane = sh("printf 'resize me'");
        pane.resize(30, 100).expect("resize");
        assert!(pane.with_term(|t| t.contents()).contains("resize me"));
        assert_eq!(pane.with_term(|t| t.size()), (30, 100));
    }
}
