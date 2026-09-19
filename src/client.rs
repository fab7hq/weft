//! The client side of a session.
//!
//! A client is one view of panes the server owns. It keeps its own terminal
//! emulator per pane, fed by the harness's own bytes off the socket, so what
//! it draws is what the harness printed — and scrollback, being a property of
//! the view, stays here rather than crossing the wire.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::protocol::{self, Frames, ToClient, ToServer};
use crate::server;

/// One pane, as this client sees it.
pub struct PaneView {
    pub harness: String,
    pub running: bool,
    parser: vt100::Parser,
    rows: u16,
    cols: u16,
}

impl PaneView {
    fn new(harness: String) -> Self {
        Self {
            harness,
            running: true,
            parser: vt100::Parser::new(24, 80, 10_000),
            rows: 24,
            cols: 80,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn with_screen<T>(&self, f: impl FnOnce(&vt100::Screen) -> T) -> T {
        f(self.parser.screen())
    }

    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    /// Scrollback belongs to the view, not the session: two clients may be
    /// looking at different parts of the same pane.
    pub fn scroll(&mut self, delta: i32) {
        let screen = self.parser.screen_mut();
        let at = screen.scrollback() as i32;
        screen.set_scrollback((at + delta).max(0) as usize);
    }

    pub fn scroll_offset(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn scroll_to_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) != (self.rows, self.cols) {
            self.parser.screen_mut().set_size(rows, cols);
            self.rows = rows;
            self.cols = cols;
        }
    }
}

pub struct Session {
    stream: UnixStream,
    inbox: Receiver<ToClient>,
    pub panes: Vec<PaneView>,
    /// The last refusal the server reported, for the UI to show.
    pub last_refusal: Option<String>,
}

impl Session {
    /// Attach to this project's session, starting one if none is listening.
    pub fn open(root: &Path, rows: u16, cols: u16) -> Result<Self> {
        let socket = server::socket_path(root);
        if !server::is_live(&socket) {
            server::clear_dead(&socket);
            start_server(root)?;
        }
        Self::connect(&socket, rows, cols)
    }

    pub fn connect(socket: &Path, rows: u16, cols: u16) -> Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match UnixStream::connect(socket) {
                Ok(s) => break s,
                Err(e) if Instant::now() >= deadline => {
                    return Err(anyhow!("no session at {}: {e}", socket.display()))
                }
                Err(_) => std::thread::sleep(Duration::from_millis(25)),
            }
        };

        let reading = stream.try_clone()?;
        let (tx, inbox) = channel();
        std::thread::spawn(move || {
            let mut frames = Frames::new(reading);
            while let Ok(Some((kind, payload))) = frames.next() {
                let Some(message) = ToClient::decode(kind, &payload) else { continue };
                if tx.send(message).is_err() {
                    return;
                }
            }
        });

        let mut session = Session { stream, inbox, panes: Vec::new(), last_refusal: None };
        session.tell(ToServer::Attach { rows, cols })?;
        Ok(session)
    }

    fn tell(&mut self, m: ToServer) -> Result<()> {
        self.stream.write_all(&m.encode())?;
        self.stream.flush()?;
        Ok(())
    }

    /// Take in whatever has arrived. Returns true when anything changed.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        while let Ok(message) = self.inbox.try_recv() {
            changed = true;
            match message {
                ToClient::Hello { panes } => {
                    self.panes = panes
                        .into_iter()
                        .map(|p| {
                            let mut v = PaneView::new(p.harness);
                            v.running = p.running;
                            v
                        })
                        .collect();
                }
                ToClient::Output { pane, bytes } => {
                    if let Some(p) = self.panes.get_mut(pane as usize) {
                        p.feed(&bytes);
                    }
                }
                ToClient::Added { pane, harness } => {
                    while self.panes.len() <= pane as usize {
                        self.panes.push(PaneView::new(harness.clone()));
                    }
                }
                ToClient::Exited { pane } => {
                    if let Some(p) = self.panes.get_mut(pane as usize) {
                        p.running = false;
                    }
                }
                ToClient::Injected { refusal, .. } => self.last_refusal = refusal,
            }
        }
        changed
    }

    pub fn spawn(&mut self, harness: &str, spec: &str) -> Result<()> {
        self.tell(ToServer::Spawn { harness: harness.into(), spec: spec.into() })
    }

    pub fn input(&mut self, pane: usize, bytes: &[u8]) -> Result<()> {
        self.tell(ToServer::Input { pane: pane as u32, bytes: bytes.to_vec() })
    }

    /// Ask the server to type a prompt. It owns the pane, so it owns the
    /// ordered submission and the wait for the text to land.
    pub fn inject(&mut self, pane: usize, bytes: &[u8]) -> Result<()> {
        self.last_refusal = None;
        self.tell(ToServer::Inject { pane: pane as u32, bytes: bytes.to_vec() })
    }

    pub fn resize(&mut self, pane: usize, rows: u16, cols: u16) -> Result<()> {
        if let Some(p) = self.panes.get_mut(pane) {
            p.resize(rows, cols);
        }
        self.tell(ToServer::Resize { pane: pane as u32, rows, cols })
    }

    /// Leave without stopping anything. This is the ordinary way out.
    pub fn detach(&mut self) {
        let _ = self.tell(ToServer::Detach);
    }

    pub fn shutdown(&mut self) {
        let _ = self.tell(ToServer::Shutdown);
    }
}

/// Start a session server for this project, detached from this process so it
/// survives the client that asked for it.
fn start_server(root: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--serve")
        .arg(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Its own session, so closing the terminal does not take the agents with it.
    unsafe {
        cmd.pre_exec(|| {
            libc_setsid();
            Ok(())
        });
    }
    cmd.spawn()?;

    let socket = server::socket_path(root);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if server::is_live(&socket) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(anyhow!("the session server did not come up"))
}

fn libc_setsid() {
    unsafe extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        setsid();
    }
}

pub fn socket_for(root: &Path) -> PathBuf {
    server::socket_path(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_rebuilds_the_screen_from_the_harnesses_own_bytes() {
        let mut v = PaneView::new("codex".into());
        v.feed(b"hello from the pane");
        assert!(v.contents().contains("hello from the pane"));
    }

    #[test]
    fn scrollback_is_the_views_own_business() {
        let mut v = PaneView::new("sh".into());
        for i in 1..=60 {
            v.feed(format!("line-{i}\r\n").as_bytes());
        }
        assert_eq!(v.scroll_offset(), 0);
        assert!(v.contents().contains("line-60"));

        v.scroll(30);
        assert_eq!(v.scroll_offset(), 30);
        assert!(v.contents().contains("line-20"), "older output is reachable");

        v.scroll_to_bottom();
        assert!(v.contents().contains("line-60"));
    }

    #[test]
    fn two_views_of_one_pane_scroll_independently() {
        let mut a = PaneView::new("sh".into());
        let mut b = PaneView::new("sh".into());
        for i in 1..=60 {
            let line = format!("line-{i}\r\n");
            a.feed(line.as_bytes());
            b.feed(line.as_bytes());
        }
        a.scroll(25);
        assert_eq!(a.scroll_offset(), 25);
        assert_eq!(b.scroll_offset(), 0, "one client scrolling does not move another");
    }

    #[test]
    fn a_view_resizes_without_losing_what_is_on_it() {
        let mut v = PaneView::new("sh".into());
        v.feed(b"resize me");
        v.resize(30, 100);
        assert_eq!(v.with_screen(|s| s.size()), (30, 100));
        assert!(v.contents().contains("resize me"));
    }
}
