//! The WebSocket a device connects to (remote.md §5, §6): on a loopback port,
//! which `cloudflared` carries; only `GET /ws` with the app's Origin
//! upgrades, and everything else is refused at the upgrade. Each connection
//! has a reader thread and a writer thread with a bounded queue, so a slow
//! device never slows the loop or another device.

use std::collections::VecDeque;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::{Role, WebSocketConfig};
use tungstenite::{Message, WebSocket};

/// The one page a device may connect from.
pub const ORIGIN: &str = "https://weft.getfab7.com";
/// The largest message a device may send.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// The most messages a device may send in a second.
pub const PER_SECOND: u32 = 50;
/// How many messages may wait for a device before its screen frames are
/// dropped; twice this, and it is closed.
pub const QUEUE: usize = 64;

/// What a connection tells the loop.
pub enum Socket {
    Opened(u64, Outbox),
    Message(u64, String),
    Closed(u64),
    /// Its queue emptied after frames were dropped: it is ready for a whole
    /// screen again.
    Drained(u64),
}

/// Listen on `127.0.0.1` at a port the system picks; answers the port.
pub fn listen<T: From<Socket> + Send + 'static>(tx: Sender<T>) -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for (id, stream) in (1u64..).zip(listener.incoming().flatten()) {
            let tx = tx.clone();
            std::thread::spawn(move || serve(id, stream, tx));
        }
    });
    Ok(port)
}

fn config() -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE))
}

/// Upgrade `GET /ws` from the app's Origin; refuse anything else.
// The shape is tungstenite's handshake callback's, error and all.
#[allow(clippy::result_large_err)]
fn check(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    let refuse = |status: u16| {
        let mut r = ErrorResponse::new(None);
        *r.status_mut() = http::StatusCode::from_u16(status).expect("a status");
        r
    };
    if request.uri().path() != "/ws" {
        return Err(refuse(404));
    }
    let origin = request.headers().get("origin").and_then(|o| o.to_str().ok());
    if origin != Some(ORIGIN) {
        return Err(refuse(403));
    }
    Ok(response)
}

fn serve<T: From<Socket> + Send + 'static>(id: u64, stream: TcpStream, tx: Sender<T>) {
    let Ok(mut refusal) = stream.try_clone() else { return };
    let mut reading = match tungstenite::accept_hdr_with_config(stream, check, Some(config())) {
        Ok(ws) => ws,
        Err(_) => {
            // Not an upgrade at all (a plain GET): nothing here but /ws.
            let _ = refusal.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
            return;
        }
    };
    let Ok(raw) = reading.get_ref().try_clone() else { return };
    let writing = WebSocket::from_raw_socket(raw, Role::Server, Some(config()));
    let outbox = Outbox::default();
    let drained = {
        let tx = tx.clone();
        move || tx.send(Socket::Drained(id).into()).is_ok()
    };
    {
        let outbox = outbox.clone();
        std::thread::spawn(move || outbox.write(writing, drained));
    }
    if tx.send(Socket::Opened(id, outbox.clone()).into()).is_err() {
        return;
    }
    let (mut window, mut count) = (Instant::now(), 0u32);
    loop {
        let text = match reading.read() {
            Ok(Message::Text(text)) => text.as_str().to_string(),
            Ok(Message::Ping(_) | Message::Pong(_)) => continue,
            // Text only; a close, a binary message, an oversized one or a
            // broken socket ends it.
            _ => break,
        };
        if window.elapsed() >= Duration::from_secs(1) {
            (window, count) = (Instant::now(), 0);
        }
        count += 1;
        if count > PER_SECOND || tx.send(Socket::Message(id, text).into()).is_err() {
            break;
        }
    }
    outbox.close();
    let _ = tx.send(Socket::Closed(id).into());
}

#[derive(Default)]
struct Queue {
    items: VecDeque<String>,
    /// Frames were dropped since it last emptied.
    stale: bool,
    /// Close once what is queued is written.
    finishing: bool,
    closed: bool,
}

/// What waits to be written to one device. Pushing never blocks.
#[derive(Clone, Default)]
pub struct Outbox(Arc<(Mutex<Queue>, Condvar)>);

impl Outbox {
    /// A message the device must have: queued, unless the device has stopped
    /// reading altogether, which closes it.
    pub fn send(&self, text: String) {
        let (lock, ready) = &*self.0;
        let mut q = lock.lock().expect("outbox");
        if q.items.len() >= 2 * QUEUE {
            q.closed = true;
        } else {
            q.items.push_back(text);
        }
        ready.notify_one();
    }

    /// A screen frame: dropped when the queue is full, which marks the device
    /// stale until it drains. Whether it was queued.
    pub fn frame(&self, text: String) -> bool {
        let (lock, ready) = &*self.0;
        let mut q = lock.lock().expect("outbox");
        if q.stale || q.items.len() >= QUEUE {
            q.stale = true;
            return false;
        }
        q.items.push_back(text);
        ready.notify_one();
        true
    }

    pub fn is_stale(&self) -> bool {
        self.0.0.lock().expect("outbox").stale
    }

    /// Close the connection once what is queued has been written: a refusal,
    /// then the end.
    pub fn finish(&self) {
        let (lock, ready) = &*self.0;
        lock.lock().expect("outbox").finishing = true;
        ready.notify_one();
    }

    /// Close the connection at once: what is queued is dropped.
    pub fn close(&self) {
        let (lock, ready) = &*self.0;
        lock.lock().expect("outbox").closed = true;
        ready.notify_one();
    }

    pub fn is_closed(&self) -> bool {
        self.0.0.lock().expect("outbox").closed
    }

    /// The writer thread: one message at a time, until closed.
    fn write<S>(self, mut ws: WebSocket<S>, drained: impl Fn() -> bool)
    where
        S: std::io::Read + Write + AsShutdown,
    {
        let (lock, ready) = &*self.0;
        loop {
            let (text, emptied) = {
                let mut q = lock.lock().expect("outbox");
                while q.items.is_empty() && !q.closed && !q.finishing {
                    q = ready.wait(q).expect("outbox");
                }
                if q.closed || (q.finishing && q.items.is_empty()) {
                    break;
                }
                let text = q.items.pop_front().expect("an item");
                let emptied = q.items.is_empty() && q.stale;
                if emptied {
                    q.stale = false;
                }
                (text, emptied)
            };
            if ws.send(Message::text(text)).is_err() {
                break;
            }
            if emptied && !drained() {
                break;
            }
        }
        lock.lock().expect("outbox").closed = true;
        let _ = ws.close(None);
        let _ = ws.flush();
        ws.get_ref().shutdown();
    }
}

/// Ending a connection for both of its threads.
pub trait AsShutdown {
    fn shutdown(&self);
}

impl AsShutdown for TcpStream {
    fn shutdown(&self) {
        let _ = TcpStream::shutdown(self, std::net::Shutdown::Both);
    }
}
