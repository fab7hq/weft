//! `weft signal [--reply <json>] <harness> <hook>`: every hook of every
//! harness (ADR-0030 D1). It prints the hook's reply before anything else can
//! fail, takes from the payload only what the harness file's `[signal]`
//! names, finds the harness process, and sends weftd one `signal` message,
//! giving up after 200 ms. It writes no file, never runs `ringframe`, and
//! always exits 0: a hook must never fail the harness's turn. Without weftd
//! it is silent and records nothing. Where `[signal]` keeps the prompt in a
//! file the payload names, it reads only that file's end, for its last line.

use std::io::{Read, Write};
use std::path::PathBuf;

/// Where `weft signal` looks: weftd's socket (`$WEFT_SOCKET`, else the
/// machine's), the harness files, and the pane Weft started the harness in.
#[derive(Debug, Clone)]
pub struct Where {
    pub socket: PathBuf,
    pub harnesses: PathBuf,
    pub pane: Option<String>,
    pub token: Option<String>,
}

impl Where {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Where {
            socket: var("WEFT_SOCKET").map(PathBuf::from).unwrap_or_else(weft_proto::socket_path),
            harnesses: crate::routing::file().with_file_name("harnesses"),
            pane: var("WEFT_PANE"),
            token: var("WEFT_PANE_TOKEN"),
        }
    }
}

/// How long the message may take to reach weftd.
pub const GIVE_UP: std::time::Duration = std::time::Duration::from_millis(200);

/// One hook. Whatever happens, the reply is printed and nothing fails.
pub fn run(args: &[String], stdin: &mut impl Read, out: &mut impl Write, at: &Where) {
    let (reply, rest) = match args {
        [flag, reply, rest @ ..] if flag == "--reply" => (Some(reply.as_str()), rest),
        rest => (None, rest),
    };
    let mut payload = Vec::new();
    let _ = stdin.read_to_end(&mut payload);
    if let Some(reply) = reply {
        let _ = writeln!(out, "{reply}");
    }
    let _ = out.flush();
    let [harness, hook, ..] = rest else { return };
    if let Some(sent) = sent(harness, hook, &payload, at) {
        send(&at.socket, &sent);
    }
}

/// The message for this hook, or nothing: no harness file, no `[signal]`,
/// a payload that is not JSON, no row for the hook, or no session.
fn sent(harness: &str, hook: &str, payload: &[u8], at: &Where) -> Option<weft_core::signal::Sent> {
    if !weft_core::sessions::is_id(harness) {
        return None;
    }
    let text = std::fs::read_to_string(at.harnesses.join(format!("{harness}.toml"))).ok()?;
    let table = serde_json::to_value(text.parse::<toml::Table>().ok()?).ok()?;
    let signal = weft_core::signal::Signal::of(&table["signal"])?;
    let payload: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let mut sent = signal.sent(harness, hook, &payload)?;
    if let Some(file) = signal.prompt_file(hook, &payload) {
        sent.prompt = tail(std::path::Path::new(&file)).and_then(|t| signal.prompt_from(&t));
    }
    if let Some(program) = table["program"].as_str()
        && let Some(found) = crate::process::harness(program)
    {
        sent.pid = Some(found.pid);
        sent.pid_start = Some(found.start);
    }
    sent.pane = at.pane.as_deref().and_then(|p| p.parse().ok());
    sent.token = at.token.clone();
    Some(sent)
}

/// How much of the end of a file holding the prompt is read: its last line
/// must fit.
const TAIL: u64 = 4 << 20;

/// The end of a file, as text: up to `TAIL` bytes.
fn tail(path: &std::path::Path) -> Option<String> {
    use std::io::Seek;
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(std::io::SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// One line to weftd, or nothing when no weftd answers in time.
fn send(socket: &std::path::Path, sent: &weft_core::signal::Sent) {
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket) else { return };
    let _ = stream.set_write_timeout(Some(GIVE_UP));
    let line = weft_proto::Line::Event(weft_proto::Event::Signal(Box::new(sent.clone()))).encode();
    let _ = stream.write_all(&line);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use weft_core::signal::Sent;
    use weft_proto::{Event, Line, Lines};

    struct Bench {
        _dir: tempfile::TempDir,
        at: Where,
    }

    fn bench() -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let harnesses = dir.path().join("harnesses");
        std::fs::create_dir_all(&harnesses).unwrap();
        for name in ["claude-code", "antigravity"] {
            std::fs::write(
                harnesses.join(format!("{name}.toml")),
                weft_core::harness::fixture::text(name),
            )
            .unwrap();
        }
        let socket = weft_proto::private_socket("signal");
        let at = Where { socket, harnesses, pane: None, token: None };
        Bench { _dir: dir, at }
    }

    fn hook(at: &Where, args: &[&str], payload: &str) -> String {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let mut out = Vec::new();
        run(&args, &mut payload.as_bytes(), &mut out, at);
        String::from_utf8(out).unwrap()
    }

    /// A test daemon: the one line that arrives, decoded.
    fn heard(at: &Where, args: &[&str], payload: &str) -> (String, Option<Sent>) {
        std::fs::create_dir_all(at.socket.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&at.socket);
        let listener = UnixListener::bind(&at.socket).unwrap();
        let printed = hook(at, args, payload);
        listener.set_nonblocking(true).unwrap();
        let sent = listener.accept().ok().map(|(stream, _)| {
            stream.set_nonblocking(false).unwrap();
            match Lines::new(stream).next() {
                Ok(Some(Line::Event(Event::Signal(sent)))) => *sent,
                other => panic!("not a signal: {other:?}"),
            }
        });
        let _ = std::fs::remove_file(&at.socket);
        (printed, sent)
    }

    #[test]
    fn the_reply_is_printed_whatever_weftd_does() {
        let b = bench();
        let allow = r#"{"decision":"allow"}"#;
        // No weftd listening, a payload that is not JSON, a harness with no
        // file, a hook no row is for, and no arguments at all.
        assert_eq!(
            hook(&b.at, &["--reply", allow, "antigravity", "PreToolUse"], "{}"),
            format!("{allow}\n")
        );
        assert_eq!(hook(&b.at, &["--reply", "{}", "antigravity", "Stop"], "not json"), "{}\n");
        assert_eq!(hook(&b.at, &["--reply", "{}", "nobody", "Stop"], "{}"), "{}\n");
        assert_eq!(hook(&b.at, &["--reply", "{}"], ""), "{}\n");
        assert_eq!(hook(&b.at, &["claude-code", "Stop"], r#"{"session_id":"s1"}"#), "");
        assert_eq!(hook(&b.at, &[], ""), "");
    }

    #[test]
    fn the_event_reaches_a_daemon_with_only_what_signal_names() {
        let mut b = bench();
        b.at.pane = Some("3".into());
        b.at.token = Some("ab".repeat(16));
        let payload = r#"{"session_id":"s1","cwd":"/w","transcript_path":"/secret","tool_name":"Bash",
            "tool_input":{"command":"npm test"},"tool_response":{"stdout":"SECRET"}}"#;
        let (printed, sent) = heard(&b.at, &["claude-code", "PostToolUse"], payload);
        assert_eq!(printed, "");
        let sent = sent.expect("weftd heard it");
        assert_eq!(
            (sent.harness.as_str(), sent.hook.as_str(), sent.session.as_str()),
            ("claude-code", "PostToolUse", "s1")
        );
        assert_eq!(sent.workspace.as_deref(), Some("/w"));
        assert_eq!(sent.state.as_deref(), Some("working"));
        assert_eq!(
            (sent.tool.as_deref(), sent.command.as_deref(), sent.outcome.as_deref()),
            (Some("Bash"), Some("npm test"), Some("succeeded"))
        );
        assert_eq!((sent.pane, sent.token.as_deref()), (Some(3), Some("ab".repeat(16).as_str())));
        let wire = serde_json::to_string(&sent).unwrap();
        assert!(!wire.contains("SECRET") && !wire.contains("secret"), "{wire}");
    }

    #[test]
    fn a_hook_no_row_is_for_sends_nothing() {
        let b = bench();
        let (_, sent) = heard(
            &b.at,
            &["claude-code", "PreToolUse"],
            r#"{"session_id":"s1","tool_name":"Bash"}"#,
        );
        assert_eq!(sent, None);
        let (_, sent) = heard(&b.at, &["claude-code", "Stop"], r#"{"cwd":"/w"}"#);
        assert_eq!(sent, None, "no session");
    }

    /// A `[signal]` whose prompt is in a file the payload names: `weft signal`
    /// reads that file's last line and sends its text as the prompt.
    #[test]
    fn a_prompt_kept_in_a_file_the_payload_names_reaches_weftd() {
        let b = bench();
        std::fs::write(
            b.at.harnesses.join("made-up.toml"),
            "title = \"Made up\"\nprogram = \"made-up\"\n[signal]\nsession = \"id\"\nworkspace = \"dir\"\n\
             prompt = { file = \"log\", last = { kind = \"typed\" }, field = \"text\" }\n\
             on = [ { hook = \"Began\", state = \"working\", forward = \"prompt\" } ]\n",
        )
        .unwrap();
        let log = b.at.harnesses.join("log.jsonl");
        let payload = format!(r#"{{"id":"s1","dir":"/w","log":"{}"}}"#, log.display());
        let older = "x".repeat(1 << 20);
        std::fs::write(&log, format!("{{\"kind\":\"answer\",\"text\":\"{older}\"}}\n{{\"kind\":\"typed\",\"text\":\"/rf:ask go\"}}\n")).unwrap();
        let (_, sent) = heard(&b.at, &["made-up", "Began"], &payload);
        let sent = sent.expect("weftd heard it");
        assert_eq!(
            (sent.prompt.as_deref(), sent.state.as_deref()),
            (Some("/rf:ask go"), Some("working"))
        );
        std::fs::write(
            &log,
            "{\"kind\":\"typed\",\"text\":\"/rf:ask go\"}\n{\"kind\":\"tool\",\"text\":\"ran\"}\n",
        )
        .unwrap();
        let (_, sent) = heard(&b.at, &["made-up", "Began"], &payload);
        let sent = sent.expect("the state still goes");
        assert_eq!((sent.prompt, sent.state.as_deref()), (None, Some("working")), "no new prompt");
        std::fs::remove_file(&log).unwrap();
        let (_, sent) = heard(&b.at, &["made-up", "Began"], &payload);
        assert_eq!(sent.expect("a missing file sends the state").prompt, None);
    }

    #[test]
    fn a_daemon_that_never_reads_is_given_up_on() {
        let b = bench();
        std::fs::create_dir_all(b.at.socket.parent().unwrap()).unwrap();
        let _listener = UnixListener::bind(&b.at.socket).unwrap();
        // A large payload, so a write would block if nothing gave up.
        let big = format!(r#"{{"session_id":"s1","cwd":"/w","prompt":"{}"}}"#, "x".repeat(4 << 20));
        let started = std::time::Instant::now();
        hook(&b.at, &["claude-code", "UserPromptSubmit"], &big);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        let _ = std::fs::remove_file(&b.at.socket);
    }
}
