//! A pane: a real terminal running a harness.
//!
//! Spec: `plans/weft/spec/runtime.md`. Weft launches the harness with the
//! person's own environment and never sandboxes it or alters its permissions.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::inject::{self, PaneState, Refusal, Step};

pub struct Pane {
    pub title: String,
    parser: Arc<Mutex<vt100::Parser>>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    injecting: bool,
}

impl Pane {
    pub fn spawn(title: impl Into<String>, program: &str, cwd: &str, rows: u16, cols: u16) -> Result<Self> {
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
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10_000)));
        let mut reader = pair.master.try_clone_reader()?;
        let sink = Arc::clone(&parser);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut p) = sink.lock() {
                    p.process(&buf[..n]);
                }
            }
        });

        let writer = pair.master.take_writer()?;
        Ok(Self {
            title: title.into(),
            parser,
            master: pair.master,
            writer,
            child,
            injecting: false,
        })
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Screen state for rendering. Held briefly; the reader thread is writing.
    pub fn with_screen<T>(&self, f: impl FnOnce(&vt100::Screen) -> T) -> T {
        let parser = self.parser.lock().expect("pane parser");
        f(parser.screen())
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
                    // its composer. Wait for the text to appear instead, and
                    // fall back to the pause if it never does.
                    std::thread::sleep(*d);
                    echoed = self.wait_for_echo(payload, inject::ECHO_TIMEOUT);
                }
            }
        }
        self.injecting = false;
        Ok(Attempt { bytes: payload.len(), bracketed, echoed })
    }

    fn bracketed_paste(&self) -> bool {
        self.with_screen(|s| s.bracketed_paste())
    }

    /// Wait until the pane shows the tail of what was written, so Enter lands
    /// on a composer that has the text rather than one still catching up.
    fn wait_for_echo(&self, payload: &[u8], timeout: std::time::Duration) -> bool {
        let Some(tail) = inject::echo_tail(payload) else {
            return false;
        };
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let screen = self.with_screen(|s| s.contents());
            if inject::squeeze(&screen).contains(&tail) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }
}

/// An injection that was *attempted*. Never evidence that it arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt {
    pub bytes: usize,
    pub bracketed: bool,
    /// Whether the pane was seen to show the text before Enter was sent.
    /// Still not evidence it was received — only the hook receipt is that.
    pub echoed: bool,
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
    fn a_pane_reports_when_its_process_is_gone() {
        let mut pane = sh("exit 0");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!pane.running());
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
        let attempt = pane
            .inject(b"$rf:eval the distinctive tail", false)
            .expect("allowed");
        assert!(attempt.echoed, "Enter is sent once the text is visible");
        assert!(attempt.bytes > 0);
    }

    #[test]
    fn a_pane_resizes_without_losing_its_screen() {
        let mut pane = sh("printf 'resize me'");
        pane.resize(30, 100).expect("resize");
        assert!(pane.with_screen(|s| s.contents()).contains("resize me"));
        assert_eq!(pane.with_screen(|s| s.size()), (30, 100));
    }
}
