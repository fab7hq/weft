//! A pane: a real terminal running a harness.
//!
//! Spec: `plans/weft/spec/runtime.md`. Weft launches the harness with the
//! person's own environment and never sandboxes it or alters its permissions.

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use weft_core::inject::{self, Handoff, PaneState, Refusal, Step};

pub struct Pane {
    pub title: String,
    parser: Arc<Mutex<vt100::Parser>>,
    /// Installed only when someone asks for the stream, so a pane nobody is
    /// listening to never accumulates output it will not be asked for.
    tap: Arc<Mutex<Option<Sender<Vec<u8>>>>>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    injecting: bool,
}

impl Pane {
    pub fn spawn(
        title: impl Into<String>,
        program: &str,
        cwd: &str,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        Self::spawn_args(title, program, &[], cwd, rows, cols)
    }

    pub fn spawn_args(
        title: impl Into<String>,
        program: &str,
        args: &[&str],
        cwd: &str,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        let pair =
            native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10_000)));
        let tap: Arc<Mutex<Option<Sender<Vec<u8>>>>> = Arc::new(Mutex::new(None));
        let mut reader = pair.master.try_clone_reader()?;
        let sink = Arc::clone(&parser);
        let tap_reader = Arc::clone(&tap);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut p) = sink.lock() {
                    p.process(&buf[..n]);
                }
                // The same bytes go to whoever is streaming this pane, so a
                // client rebuilds the screen from the harness's own output.
                if let Ok(mut t) = tap_reader.lock()
                    && let Some(sender) = t.as_ref()
                    && sender.send(buf[..n].to_vec()).is_err()
                {
                    *t = None;
                }
            }
            if let Ok(mut t) = tap_reader.lock() {
                *t = None;
            }
        });

        let writer = pair.master.take_writer()?;
        Ok(Self {
            title: title.into(),
            parser,
            tap,
            master: pair.master,
            writer,
            child,
            injecting: false,
        })
    }

    /// Every byte this pane prints, from now on. One listener at a time.
    pub fn stream_output(&mut self) -> Receiver<Vec<u8>> {
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

    /// Screen state for rendering. Held briefly; the reader thread is writing.
    pub fn with_screen<T>(&self, f: impl FnOnce(&vt100::Screen) -> T) -> T {
        let parser = self.parser.lock().expect("pane parser");
        f(parser.screen())
    }

    /// Move back through what the pane has already printed.
    ///
    /// Neither supported harness asks for mouse reporting or uses the
    /// alternate screen, so Weft — which has no terminal behind it to keep a
    /// scrollback — keeps one itself and moves through it here.
    ///
    /// That only reaches what the harness let scroll off. Claude Code prints
    /// inline, so this is its whole history. Codex repaints its viewport
    /// instead, so nothing ever arrives here and this moves nothing: its
    /// history lives inside Codex, behind the key `Harness::transcript` names.
    pub fn scroll(&mut self, delta: i32) {
        let mut parser = self.parser.lock().expect("pane parser");
        let screen = parser.screen_mut();
        let at = screen.scrollback() as i32;
        let next = (at + delta).max(0) as usize;
        screen.set_scrollback(next);
    }

    pub fn scroll_offset(&self) -> usize {
        self.parser.lock().expect("pane parser").screen().scrollback()
    }

    /// Jump back to the live output.
    pub fn scroll_to_bottom(&mut self) {
        self.parser.lock().expect("pane parser").screen_mut().set_scrollback(0);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        self.parser.lock().expect("pane parser").screen_mut().set_size(rows, cols);
        Ok(())
    }

    /// A keystroke the person typed while focused on this pane. Passed through
    /// untouched — Weft reserves only its toggle key.
    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn state(&mut self, blocked: bool) -> PaneState {
        PaneState { running: self.running(), blocked, injecting: self.injecting }
    }

    /// Type a prompt on the person's behalf.
    ///
    /// Returns once the bytes are written. That is all it proves: the caller
    /// must never report this as delivery.
    pub fn inject(&mut self, payload: &[u8], blocked: bool) -> Result<Attempt, Refusal> {
        self.inject_as(payload, blocked, &Handoff::Whole)
    }

    /// Type a prompt the way its command requires.
    ///
    /// The bytes that reach the host are the same whichever way this goes; what
    /// changes is whether the host reads the command. Typed, it does; inside a
    /// folded paste, it does not. See `spec/injection.md`.
    pub fn inject_as(
        &mut self,
        payload: &[u8],
        blocked: bool,
        how: &Handoff,
    ) -> Result<Attempt, Refusal> {
        if let Handoff::EnterMode { command, active, .. } = how {
            // The mode first, on its own, and only believed when the host says
            // so. A mode switch is handled by the TUI and submits no prompt, so
            // this adds nothing to the record.
            self.send(command.as_bytes()).map_err(|_| Refusal::NoProcess)?;
            // Enter goes in its own write, after the composer has been left
            // alone: written together they are one burst, and a host that
            // suppresses Enter during a burst takes it as a newline.
            std::thread::sleep(inject::SUBMIT_SETTLE);
            self.send(b"\r").map_err(|_| Refusal::NoProcess)?;
            if !self.wait_for(active, inject::MODE_TIMEOUT) {
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
        let body = how.body(payload);
        self.write_and_submit(body, blocked, typed.is_some())
    }

    /// Wait for the host to show something, rather than assuming it did.
    fn wait_for(&self, needle: &str, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.with_screen(|s| s.contents()).contains(needle) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    fn write_and_submit(
        &mut self,
        payload: &[u8],
        blocked: bool,
        command_typed: bool,
    ) -> Result<Attempt, Refusal> {
        let state = self.state(blocked);
        inject::check(&state)?;
        self.injecting = true;
        let bracketed = self.bracketed_paste();
        let steps = inject::compose(payload, bracketed);
        let mut echoed = false;
        for step in &steps {
            match step {
                Step::Write(bytes) => {
                    if self.send(bytes).is_err() {
                        self.injecting = false;
                        return Err(Refusal::NoProcess);
                    }
                }
                Step::Wait(d) => {
                    // A fixed pause is not enough: a harness that is still
                    // settling drops the Enter and the prompt sits unsent in
                    // its composer. Wait for the text to appear instead.
                    std::thread::sleep(*d);
                    echoed = self.wait_for_echo(payload, inject::ECHO_TIMEOUT);
                    if !echoed {
                        // The composer is not showing what was written, so
                        // Enter would submit something other than the Ask. The
                        // text stays where it is, unsent, for the person to
                        // see. Never a blind retry.
                        self.injecting = false;
                        return Ok(Attempt {
                            bytes: payload.len(),
                            bracketed,
                            echoed,
                            submitted: false,
                            folded_command: None,
                        });
                    }
                    // The text is there, but folded into a placeholder — and a
                    // folded paste is not scanned for slash commands. Enter
                    // here runs the prompt as an ordinary request instead of
                    // the mode it asked for, which is a different act. Unless
                    // the command was typed in front of it, which is the whole
                    // point of doing that.
                    if let Some(command) =
                        (!command_typed).then(|| self.folded_command(payload)).flatten()
                    {
                        self.injecting = false;
                        return Ok(Attempt {
                            bytes: payload.len(),
                            bracketed,
                            echoed,
                            submitted: false,
                            folded_command: Some(command),
                        });
                    }
                }
            }
        }
        self.injecting = false;
        Ok(Attempt {
            bytes: payload.len(),
            bracketed,
            echoed,
            submitted: true,
            folded_command: None,
        })
    }

    /// The command this payload opens with, when the pane has folded the paste
    /// away so that the host will never see it as a command.
    ///
    /// Measured: Codex 0.155.1 folds a paste from 1000 characters, Claude Code
    /// 2.1.278 from somewhere between 500 and 800. A folded paste submits as
    /// ordinary text — the model does not even change — so a prompt carrying
    /// `/plan` gets an ordinary answer.
    fn folded_command(&self, payload: &[u8]) -> Option<String> {
        let command = inject::leading_command(payload)?;
        let screen = self.with_screen(|s| s.contents());
        inject::collapsed_paste(&screen).then_some(command)
    }

    fn bracketed_paste(&self) -> bool {
        self.with_screen(|s| s.bracketed_paste())
    }

    /// Wait until the pane shows both ends of what was written, so Enter
    /// lands on a composer holding the whole prompt rather than one still
    /// catching up — or one that swallowed the start of the paste.
    fn wait_for_echo(&self, payload: &[u8], timeout: std::time::Duration) -> bool {
        let (Some(head), Some(tail)) = (inject::echo_head(payload), inject::echo_tail(payload))
        else {
            return false;
        };
        let deadline = std::time::Instant::now() + timeout;
        // Seen, and then unchanged for a moment: a composer still taking the
        // paste is still redrawing, and Enter sent into that is dropped.
        let mut settled: Option<(std::time::Instant, String)> = None;
        while std::time::Instant::now() < deadline {
            let screen = inject::squeeze(&self.with_screen(|s| s.contents()));
            // Past a size the host folds the paste into a placeholder instead
            // of showing it, and then the placeholder is the echo. It is the
            // length that decides, not the line count: a single long line is
            // folded just the same.
            let folded = inject::collapsed_paste_of(&screen, payload.len());
            let showing = folded || (screen.contains(&head) && screen.contains(&tail));
            settled = match (settled, showing) {
                (_, false) => None,
                (None, true) => Some((std::time::Instant::now(), screen)),
                (Some((since, before)), true) if before == screen => {
                    if since.elapsed() >= inject::COMPOSER_SETTLE {
                        return true;
                    }
                    Some((since, before))
                }
                (Some(_), true) => Some((std::time::Instant::now(), screen)),
            };
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // On screen but never still: say so rather than pretend it settled.
        settled.is_some()
    }
}

/// An injection that was *attempted*. Never evidence that it arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub bytes: usize,
    pub bracketed: bool,
    /// Whether the pane was seen to show the text before Enter was sent.
    /// Still not evidence it was received — only the hook receipt is that.
    pub echoed: bool,
    /// Whether Enter was sent at all. It is not, when the composer never
    /// showed the whole prompt: submitting a mangled Ask is worse than
    /// leaving the text sitting there for the person to see.
    pub submitted: bool,
    /// The slash command this prompt opens with, when the harness folded the
    /// paste and would therefore run it as ordinary text. Enter is withheld:
    /// `/plan` that does not enter Plan mode is not the Ask that was confirmed.
    pub folded_command: Option<String>,
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
        let text = pane.with_screen(|s| s.contents());
        assert!(text.contains("hello from a pane"), "screen was: {text:?}");
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

    #[test]
    fn enter_is_withheld_when_the_pane_never_shows_the_prompt() {
        // Found by a W4 run against Codex: the composer swallowed the first
        // characters of the paste, the tail still matched, and Weft submitted
        // a prompt the person had not asked for. With nothing echoed at all,
        // Enter must not be sent and the attempt must say so.
        let mut pane = sh("stty -echo; cat > /dev/null");
        std::thread::sleep(std::time::Duration::from_millis(400));
        let attempt =
            pane.inject(b"/plan Return the real build number", false).expect("allowed to write");
        assert!(!attempt.echoed, "nothing was echoed");
        assert!(!attempt.submitted, "so Enter was withheld");
    }

    #[test]
    fn enter_is_withheld_when_the_paste_folds_a_command_away() {
        // Found against Codex 0.155.1: a paste of 1000 characters or more is
        // folded into `[Pasted Content N chars]`, and a folded paste is not
        // scanned for slash commands. Enter there submits `/plan …` as an
        // ordinary request — the model does not even change — which is not
        // the Ask that was confirmed.
        let payload = b"/plan ship the health endpoint\nwith rules\nand a tail";
        let mut pane = sh(&format!(
            "stty -echo; printf '[Pasted Content {} chars]\\n'; cat > /dev/null",
            payload.len()
        ));
        std::thread::sleep(std::time::Duration::from_millis(400));
        let attempt = pane.inject(payload, false).expect("allowed to write");
        assert!(attempt.echoed, "the whole prompt is there, folded");
        assert!(!attempt.submitted, "so Enter is withheld");
        assert_eq!(attempt.folded_command.as_deref(), Some("/plan"), "and it says which command");
    }

    #[test]
    fn a_folded_paste_with_no_command_is_still_sent() {
        // Folding is only a problem for a command. Ordinary prompt text reads
        // the same folded or not, so there is nothing to withhold Enter for.
        let payload = b"ship the health endpoint\nwith rules\nand a tail";
        let mut pane = sh(&format!(
            "stty -echo; printf '[Pasted Content {} chars]\\n'; cat > /dev/null",
            payload.len()
        ));
        std::thread::sleep(std::time::Duration::from_millis(400));
        let attempt = pane.inject(payload, false).expect("allowed to write");
        assert!(attempt.submitted, "nothing here needs a command to run");
        assert_eq!(attempt.folded_command, None);
    }

    #[test]
    fn injected_bytes_reach_the_process_exactly() {
        let mut pane = sh("cat > /tmp/weft-inject-test.txt");
        let payload = "/plan Return the real build number";
        pane.inject(payload.as_bytes(), false).expect("allowed");
        std::thread::sleep(std::time::Duration::from_millis(200));
        pane.send(&[4]).expect("eof"); // Ctrl-D closes cat's stdin
        std::thread::sleep(std::time::Duration::from_millis(300));
        let written = std::fs::read_to_string("/tmp/weft-inject-test.txt").unwrap_or_default();
        assert_eq!(written.trim_end_matches(['\r', '\n']), payload);
        let _ = std::fs::remove_file("/tmp/weft-inject-test.txt");
    }

    #[test]
    fn injection_into_a_blocked_pane_is_refused_before_anything_is_written() {
        let mut pane = sh("cat > /dev/null");
        assert_eq!(pane.inject(b"anything", true), Err(Refusal::PaneBlocked));
    }

    #[test]
    fn an_injection_waits_for_the_pane_to_show_the_text() {
        let mut pane = sh("cat");
        let attempt = pane.inject(b"$rf:eval the distinctive tail", false).expect("allowed");
        assert!(attempt.echoed, "Enter is sent once the text is visible");
        assert!(attempt.bytes > 0);
    }

    #[test]
    fn a_pane_streams_its_output_to_a_listener() {
        let mut pane = Pane::spawn("test", "/bin/sh", "/tmp", 24, 80).expect("spawn");
        let stream = pane.stream_output();
        pane.send(b"printf 'streamed-to-the-server'\n").expect("send");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            if let Ok(chunk) = stream.recv_timeout(std::time::Duration::from_millis(200)) {
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
        assert!(pane.with_screen(|s| s.contents()).contains("streamed-to-the-server"));
    }

    #[test]
    fn a_pane_scrolls_back_through_what_it_printed() {
        let mut pane = sh("for i in $(seq 1 60); do echo line-$i; done");
        assert_eq!(pane.scroll_offset(), 0, "starts at the live output");
        assert!(pane.with_screen(|s| s.contents()).contains("line-60"));

        pane.scroll(30);
        assert_eq!(pane.scroll_offset(), 30);
        let scrolled = pane.with_screen(|s| s.contents());
        assert!(scrolled.contains("line-20"), "older output is reachable: {scrolled:?}");

        pane.scroll_to_bottom();
        assert_eq!(pane.scroll_offset(), 0);
        assert!(pane.with_screen(|s| s.contents()).contains("line-60"));
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
        assert!(pane.with_screen(|s| s.contents()).contains("resize me"));
        assert_eq!(pane.with_screen(|s| s.size()), (30, 100));
    }
}
