//! The WebSocket listener (remote.md §5, §6): loopback, `/ws` only, the
//! Origin checked at the upgrade, size and rate limits, and a bounded writer
//! per device (§14 test 2).

use std::net::TcpStream;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};
use weft_remote::socket::{self, Outbox, Socket};

fn listening() -> (u16, Receiver<Socket>) {
    let (tx, rx) = channel();
    (socket::listen(tx).unwrap(), rx)
}

fn connect(port: u16, path: &str, origin: &str) -> Result<WebSocket<TcpStream>, String> {
    let mut request = format!("ws://127.0.0.1:{port}{path}").into_client_request().unwrap();
    request.headers_mut().insert("origin", origin.parse().unwrap());
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    match tungstenite::client(request, stream) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(r))) => {
            Err(r.status().as_u16().to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn opened(rx: &Receiver<Socket>) -> (u64, Outbox) {
    match rx.recv_timeout(Duration::from_secs(5)).expect("opened") {
        Socket::Opened(id, outbox) => (id, outbox),
        _ => panic!("expected an open"),
    }
}

#[test]
fn only_get_ws_from_the_apps_origin_upgrades() {
    let (port, rx) = listening();
    assert_eq!(connect(port, "/ws", "https://evil.example").err().as_deref(), Some("403"));
    assert_eq!(connect(port, "/ws", "null").err().as_deref(), Some("403"));
    assert_eq!(connect(port, "/other", socket::ORIGIN).err().as_deref(), Some("404"));
    // A plain request is not an upgrade, and finds nothing.
    use std::io::{Read, Write};
    let mut plain = TcpStream::connect(("127.0.0.1", port)).unwrap();
    plain.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n").unwrap();
    let mut answer = String::new();
    plain.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 404"), "{answer}");
    assert!(rx.try_recv().is_err(), "nothing refused reached the loop");
    assert!(connect(port, "/ws", socket::ORIGIN).is_ok());
}

#[test]
fn a_message_over_64_kib_closes_the_socket() {
    let (port, rx) = listening();
    let mut ws = connect(port, "/ws", socket::ORIGIN).unwrap();
    let (id, _) = opened(&rx);
    ws.send(Message::text("x".repeat(64 * 1024 - 100))).unwrap();
    assert!(
        matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Socket::Message(got, _) if got == id)
    );
    let _ = ws.send(Message::text("x".repeat(65 * 1024)));
    assert!(
        matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Socket::Closed(got) if got == id)
    );
}

#[test]
fn more_than_fifty_messages_in_a_second_closes_the_socket() {
    let (port, rx) = listening();
    let mut ws = connect(port, "/ws", socket::ORIGIN).unwrap();
    let (id, _) = opened(&rx);
    for _ in 0..51 {
        let _ = ws.send(Message::text("{}"));
    }
    let mut heard = 0;
    loop {
        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            Socket::Message(..) => heard += 1,
            Socket::Closed(got) => {
                assert_eq!(got, id);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(heard, 50);
}

/// §14 test 2, at the socket: a device that stops reading never delays
/// another's frames; its frames are dropped instead, and when it reads
/// again it says so, ready for a whole screen.
#[test]
fn a_device_that_stops_reading_never_delays_another() {
    let (port, rx) = listening();
    let mut reading = connect(port, "/ws", socket::ORIGIN).unwrap();
    let (_, to_reading) = opened(&rx);
    let mut stopped = connect(port, "/ws", socket::ORIGIN).unwrap();
    let (stopped_id, to_stopped) = opened(&rx);
    reading.get_mut().set_read_timeout(Some(Duration::from_secs(2))).unwrap();

    let frame = "f".repeat(32 * 1024);
    let mut slowest_push = Duration::ZERO;
    for n in 0..200 {
        let pushed = Instant::now();
        to_stopped.frame(format!("{n} {frame}"));
        to_reading.frame(format!("{n} {frame}"));
        slowest_push = slowest_push.max(pushed.elapsed());
        let got = reading.read().unwrap();
        assert!(got.to_text().unwrap().starts_with(&format!("{n} ")), "in order, at once");
    }
    assert!(slowest_push < Duration::from_millis(50), "the loop never waited: {slowest_push:?}");
    assert!(to_stopped.is_stale(), "frames for the stopped device were dropped");

    // It reads again: what was queued arrives, then it says it is drained.
    stopped.get_mut().set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut caught_up = 0;
    let drained = loop {
        match rx.try_recv() {
            Ok(Socket::Drained(id)) => break id,
            _ => {
                stopped.read().unwrap();
                caught_up += 1;
            }
        }
    };
    assert_eq!(drained, stopped_id);
    assert!(caught_up < 200, "frames were dropped, not all queued: {caught_up}");
    assert!(!to_stopped.is_stale());
    assert!(to_stopped.frame("full".into()), "it takes frames again");
    loop {
        if stopped.read().unwrap().to_text().unwrap() == "full" {
            break;
        }
    }
}
