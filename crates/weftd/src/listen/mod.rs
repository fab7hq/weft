//! Listening to a harness's own event stream: connect to
//! the Unix socket its harness file names, send the table's hello, and hand
//! on each session and state a message gives. Nothing else is ever sent.
//!
//! What a harness's stream says is data (its harness file's `[listen]`); how
//! the stream is framed is a transport, an adapter of its own in this folder.
//! A new transport is one file here, one `Transport` variant in weft-core
//! and one arm in [`open`]; nothing here knows a harness.

mod lines;
mod websocket;

use std::os::unix::net::UnixStream;
use std::path::Path;

use weft_core::listen::{Listen, Transport};

/// One transport: how a stream's messages are framed on its connection.
trait Framed {
    fn send(&mut self, message: &str) -> std::io::Result<()>;
    /// The next message, or none once the server closed the stream.
    fn next(&mut self) -> std::io::Result<Option<String>>;
}

/// The transport a harness file names, over a connected socket.
fn open(transport: Transport, stream: UnixStream) -> std::io::Result<Box<dyn Framed>> {
    Ok(match transport {
        Transport::Websocket => Box::new(websocket::Websocket::open(stream)?),
        Transport::Lines => Box::new(lines::Lines::open(stream)?),
    })
}

/// Read the stream at `socket` until its server closes it, calling `heard`
/// with each session and its state. An error is how it ended.
pub fn run(
    socket: &Path,
    listen: &Listen,
    mut heard: impl FnMut(String, String),
) -> std::io::Result<()> {
    let mut stream = open(listen.transport, UnixStream::connect(socket)?)?;
    let mut message = |text: &str| {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        if let Some((session, state)) = listen.heard(&v) {
            heard(session, state);
        }
        Some(v)
    };
    // A request in the hello (it has an id) is answered before the next is
    // sent: a server may take a message sent before it answers for nothing,
    // and then say nothing more. An error in answer opens no stream.
    for hello in &listen.hello {
        stream.send(&hello.to_string())?;
        let Some(id) = hello.get("id") else { continue };
        loop {
            let Some(text) = stream.next()? else { return Ok(()) };
            let Some(v) = message(&text) else { continue };
            if v.get("id") == Some(id) {
                if let Some(e) = v.get("error") {
                    return Err(std::io::Error::other(format!("the hello was refused: {e}")));
                }
                break;
            }
        }
    }
    while let Some(text) = stream.next()? {
        message(&text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    fn listen(transport: &str) -> Listen {
        Listen::of(&serde_json::json!({
            "socket": "s", "transport": transport, "session": "params.id",
            "hello": [{"method": "hi"}],
            "on": [{"method": "status", "when": {"params.type": "idle"}, "state": "turn_ended"},
                   {"method": "status", "when": {"params.type": "active"}, "state": "working"}],
        }))
        .unwrap()
    }

    const SAID: [&str; 3] = [
        r#"{"method":"status","params":{"id":"t1","type":"active"}}"#,
        r#"{"method":"other","params":{"id":"t1"}}"#,
        r#"{"method":"status","params":{"id":"t1","type":"idle"}}"#,
    ];

    /// A server that takes one client, hands back what it sent first, says
    /// `SAID` and closes.
    fn serve(
        transport: &'static str,
    ) -> (std::path::PathBuf, tempfile::TempDir, mpsc::Receiver<String>) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("server.sock");
        let server = UnixListener::bind(&socket).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = server.accept().unwrap();
            if transport == "websocket" {
                let mut ws = tungstenite::accept(stream).unwrap();
                tx.send(ws.read().unwrap().into_text().unwrap().to_string()).unwrap();
                for s in SAID {
                    ws.send(tungstenite::Message::text(s)).unwrap();
                }
                ws.close(None).unwrap();
                while ws.read().is_ok() {}
            } else {
                let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
                tx.send(lines.next().unwrap().unwrap()).unwrap();
                let mut w = stream;
                for s in SAID {
                    writeln!(w, "{s}").unwrap();
                }
            }
        });
        (socket, dir, rx)
    }

    #[test]
    fn a_websocket_stream_is_greeted_and_read_until_it_closes() {
        let (socket, _dir, hello) = serve("websocket");
        let mut heard = Vec::new();
        run(&socket, &listen("websocket"), |s, st| heard.push((s, st))).expect("closed cleanly");
        assert_eq!(hello.recv().unwrap(), r#"{"method":"hi"}"#);
        assert_eq!(heard, [("t1".into(), "working".into()), ("t1".into(), "turn_ended".into())]);
    }

    #[test]
    fn a_stream_of_lines_is_greeted_and_read_until_it_closes() {
        let (socket, _dir, hello) = serve("lines");
        let mut heard = Vec::new();
        run(&socket, &listen("lines"), |s, st| heard.push((s, st))).expect("closed");
        assert_eq!(hello.recv().unwrap(), r#"{"method":"hi"}"#);
        assert_eq!(heard, [("t1".into(), "working".into()), ("t1".into(), "turn_ended".into())]);
    }

    #[test]
    fn no_socket_is_no_stream() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(&dir.path().join("none.sock"), &listen("lines"), |_, _| {}).is_err());
    }

    /// A hello message that is a request (it has an id) is answered before
    /// the next is sent: a server that has not answered it yet may take the
    /// next for nothing, and then say nothing more.
    #[test]
    fn a_hello_request_is_answered_before_the_next_is_sent() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("server.sock");
        let server = UnixListener::bind(&socket).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = server.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
            let mut lines = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            lines.read_line(&mut first).unwrap();
            // Slow to answer: nothing more may arrive meanwhile.
            std::thread::sleep(std::time::Duration::from_millis(300));
            stream.set_nonblocking(true).unwrap();
            let mut early = String::new();
            let early = lines.read_line(&mut early).map(|n| n > 0).unwrap_or(false);
            stream.set_nonblocking(false).unwrap();
            let mut w = stream;
            writeln!(w, r#"{{"id":1,"result":{{}}}}"#).unwrap();
            let mut second = String::new();
            let _ = lines.read_line(&mut second);
            writeln!(w, "{}", SAID[0]).unwrap();
            tx.send((early, second.trim_end().to_string())).unwrap();
        });
        let l = Listen::of(&serde_json::json!({
            "socket": "s", "transport": "lines", "session": "params.id",
            "hello": [{"id": 1, "method": "hi"}, {"method": "ready"}],
            "on": [{"method": "status", "when": {"params.type": "active"}, "state": "working"}],
        }))
        .unwrap();
        let mut heard = Vec::new();
        let _ = run(&socket, &l, |s, st| heard.push((s, st)));
        let (early, second) = rx.recv().unwrap();
        assert!(!early, "the next hello went before the request was answered");
        assert_eq!(second, r#"{"method":"ready"}"#);
        assert_eq!(heard, [("t1".into(), "working".into())]);
    }

    /// A hello request answered with an error opens no stream.
    #[test]
    fn a_hello_refused_opens_no_stream() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("server.sock");
        let server = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = server.accept().unwrap();
            let mut lines = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            lines.read_line(&mut first).unwrap();
            let mut w = stream;
            writeln!(w, r#"{{"id":1,"error":{{"message":"no"}}}}"#).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(500));
        });
        let l = Listen::of(&serde_json::json!({
            "socket": "s", "transport": "lines", "session": "params.id",
            "hello": [{"id": 1, "method": "hi"}], "on": [],
        }))
        .unwrap();
        assert!(run(&socket, &l, |_, _| {}).is_err());
    }
}
