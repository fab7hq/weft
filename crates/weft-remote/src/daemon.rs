//! The daemon's socket, as `weft remote` uses it: one client per open
//! project, because a client watches one project, opened with `output:
//! "none"` so no raw bytes ever come this way (remote.md §5, D4). Each reads
//! on its own thread into the loop's channel, so nothing here waits on the
//! network and a slow device never slows the daemon.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use serde_json::Value;
use weft_proto::{Call, Event, Line, Lines, PROTOCOL};

/// What a project's client hears, for the loop.
#[derive(Debug)]
pub enum Heard {
    Event(usize, Event),
    /// The answer to a call this client made: its result, or its error's
    /// code and sentence.
    Answer(usize, u64, Result<Value, (String, String)>),
    /// The daemon went away.
    Gone(usize),
}

/// One project, open on its own client.
pub struct Project {
    pub path: PathBuf,
    /// What `project.open` answered.
    pub opened: Value,
    out: UnixStream,
    next: u64,
}

/// The number the control client's events come with.
pub const CONTROL: usize = usize::MAX;

/// The size a project is opened at. A device never resizes by watching; it
/// says its own size before it types (remote.md §9).
const ROWS: u16 = 24;
const COLS: u16 = 80;

impl Project {
    /// Say hello as `weft-remote`, open `path` with no raw output, and read
    /// from then on into `tx`, as project number `at`.
    pub fn open<T: From<Heard> + Send + 'static>(
        socket: &Path,
        path: &Path,
        at: usize,
        tx: Sender<T>,
    ) -> anyhow::Result<Self> {
        Self::connect(socket, Some(path), at, tx)
    }

    /// The client that watches no project: it publishes the remote view and
    /// hears the acts for it and presence. Numbered [`CONTROL`].
    pub fn control<T: From<Heard> + Send + 'static>(
        socket: &Path,
        tx: Sender<T>,
    ) -> anyhow::Result<Self> {
        Self::connect(socket, None, CONTROL, tx)
    }

    fn connect<T: From<Heard> + Send + 'static>(
        socket: &Path,
        path: Option<&Path>,
        at: usize,
        tx: Sender<T>,
    ) -> anyhow::Result<Self> {
        let out = UnixStream::connect(socket)?;
        let mut lines = Lines::new(out.try_clone()?);
        let mut project = Project {
            path: path.map(Path::to_path_buf).unwrap_or_default(),
            opened: Value::Null,
            out,
            next: 1,
        };
        let mut calls = vec![Call::Hello { client: client_name(), protocol: PROTOCOL }];
        if let Some(path) = path {
            let path = path.to_string_lossy().into_owned();
            calls.push(Call::Open { path, rows: ROWS, cols: COLS, raw: false });
        }
        for call in calls {
            let id = project.call(call)?;
            project.opened = loop {
                match lines.next()? {
                    Some(Line::Result { id: got, result }) if got == id => break result,
                    Some(Line::Error { id: got, code, message }) if got == id => {
                        anyhow::bail!("{code}: {message}")
                    }
                    Some(_) => continue,
                    None => anyhow::bail!("the daemon closed the socket"),
                }
            };
        }
        std::thread::spawn(move || {
            while let Ok(Some(line)) = lines.next() {
                let heard = match line {
                    Line::Event(e) => Heard::Event(at, e),
                    Line::Result { id, result } => Heard::Answer(at, id, Ok(result)),
                    Line::Error { id, code, message } => {
                        Heard::Answer(at, id, Err((code, message)))
                    }
                    Line::Call { .. } => continue,
                };
                if tx.send(heard.into()).is_err() {
                    return;
                }
            }
            let _ = tx.send(Heard::Gone(at).into());
        });
        Ok(project)
    }

    /// Send a call; its answer comes back through the channel with this id.
    /// A call `weft remote` may not make is refused here, before the socket
    /// (remote.md §4).
    pub fn call(&mut self, call: Call) -> std::io::Result<u64> {
        if !allowed(&call) {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not allowed"));
        }
        let id = self.next;
        self.next += 1;
        self.out.write_all(&Line::Call { id, call }.encode())?;
        self.out.flush()?;
        Ok(id)
    }
}

/// The calls `weft remote` may make (remote.md §4), and its own handshake
/// and view. Never `agent.start`, `agent.close`, `stage`, `setup`,
/// `project.turbo`, `shutdown`, `unit.confirm`, or `force: true`.
pub fn allowed(call: &Call) -> bool {
    match call {
        Call::Hello { .. } | Call::RemoteView { .. } => true,
        Call::Open { raw, .. } => !raw,
        Call::Attach { form, .. } => form == "screen" || form == "none",
        Call::Input { .. } | Call::Resize { .. } => true,
        Call::Act { act, .. } => matches!(act.as_str(), "ask" | "check" | "decide"),
        Call::Resolve { force, .. } => !force,
        Call::Read { what, .. } => what == "eval" || what == "history",
        _ => false,
    }
}

/// How `weft remote` names itself to the daemon: only this client may publish
/// the remote view, and its typing is never the person's presence.
pub fn client_name() -> String {
    concat!("weft-remote/", env!("CARGO_PKG_VERSION")).to_string()
}

/// The projects the daemon has open, from its `projects.json`.
pub fn projects(file: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    weft_core::projects::read(&text).into_iter().map(|p| p.path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use weft_core::inject::Handoff;

    /// remote.md §14 test 5, at the socket: every call in §4's never-sent
    /// list is refused before it is written.
    #[test]
    fn only_the_calls_remote_md_allows_are_made() {
        let never = [
            Call::StartAgent { harness: "sh".into(), spec: "sh".into(), session: None },
            Call::CloseAgent { pane: 0 },
            Call::Stage {
                pane: 0,
                bytes: vec![],
                how: Handoff::Whole,
                what: String::new(),
                why: String::new(),
            },
            Call::Setup { act: "run".into(), harness: String::new(), path: String::new() },
            Call::Turbo { on: true },
            Call::Shutdown,
            Call::ConfirmAsk { unit: "ask_1".into() },
            Call::Resolve { pending: "pnd_1".into(), yes: true, force: true },
            Call::Act { act: "send".into(), unit: Some("ask_1".into()), pane: None, text: None },
            Call::Act { act: "follow_up".into(), unit: None, pane: Some(0), text: None },
            Call::Read { what: "wording".into(), unit: "ask_1".into(), pane: None, lines: None },
            Call::Open { path: "/p".into(), rows: 24, cols: 80, raw: true },
            Call::Remote { act: serde_json::json!({"act": "on"}) },
            Call::Available,
            Call::Detach,
        ];
        for call in never {
            assert!(!allowed(&call), "{call:?}");
        }
        let may = [
            Call::Attach { pane: 0, form: "screen".into() },
            Call::Input { pane: 0, bytes: b"1".to_vec() },
            Call::Resize { pane: 0, rows: 40, cols: 50 },
            Call::Act { act: "ask".into(), unit: None, pane: Some(0), text: Some("hi".into()) },
            Call::Act { act: "check".into(), unit: Some("ask_1".into()), pane: None, text: None },
            Call::Act { act: "decide".into(), unit: Some("ask_1".into()), pane: None, text: None },
            Call::Resolve { pending: "pnd_1".into(), yes: true, force: false },
            Call::Read {
                what: "history".into(),
                unit: String::new(),
                pane: Some(0),
                lines: Some(2000),
            },
            Call::Read { what: "eval".into(), unit: "ev_1".into(), pane: None, lines: None },
        ];
        for call in may {
            assert!(allowed(&call), "{call:?}");
        }
    }
}
