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
/// What the system may hold for a device it has not yet taken. Left to the
/// system, Linux grows it to megabytes, so a device that stops reading fills
/// that rather than its queue, and is never given a whole screen again.
pub const SEND_BUFFER: usize = 64 * 1024;

/// How long a connection has to finish its upgrade. One reaching the tunnel
/// that never does is closed, so it holds no thread or socket open.
pub const HANDSHAKE: Duration = Duration::from_secs(10);

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

/// Set the socket's send buffer to `size`, which also stops the system from
/// growing it. A socket that refuses keeps the system's.
fn bound_send_buffer(stream: &TcpStream, size: usize) {
    use std::os::fd::AsRawFd as _;
    let size = libc::c_int::try_from(size).unwrap_or(libc::c_int::MAX);
    // SAFETY: the descriptor is the stream's own, open for the call, and the
    // value is a c_int of the length given.
    unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&raw const size).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
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
    bound_send_buffer(&stream, SEND_BUFFER);
    let Ok(mut refusal) = stream.try_clone() else { return };
    if stream.set_read_timeout(Some(HANDSHAKE)).is_err() {
        return;
    }
    let mut reading = match tungstenite::accept_hdr_with_config(stream, check, Some(config())) {
        Ok(ws) => ws,
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Io(_))) => return,
        Err(_) => {
            // Not an upgrade at all (a plain GET): nothing here but /ws.
            let _ = refusal.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
            return;
        }
    };
    // Upgraded: a device may be quiet as long as it likes.
    if reading.get_ref().set_read_timeout(None).is_err() {
        return;
    }
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
    items: VecDeque<Item>,
    /// Frames were dropped since it last emptied.
    stale: bool,
    /// Close once what is queued is written.
    finishing: bool,
    closed: bool,
}

enum Item {
    Message(String),
    Screen(Screen, usize),
}

/// An agent's screen frame for a device (remote.md D3): its visible screen
/// whole, or the change from the frame before. It waits as it is, not yet
/// written as a message, so that a newer frame can join it.
pub struct Screen {
    pub agent: String,
    pub seq: u64,
    pub full: bool,
    pub rows: u16,
    pub cols: u16,
    pub bytes: Vec<u8>,
}

impl Screen {
    /// A newer frame of the same agent: a whole screen replaces this one, a
    /// change is drawn after it.
    fn join(&mut self, newer: Screen) {
        if newer.full {
            *self = newer;
        } else {
            self.bytes.extend(newer.bytes);
            self.seq = newer.seq;
        }
    }

    fn message(&self) -> String {
        use base64ct::{Base64, Encoding};
        serde_json::json!({"type": "screen", "agent": self.agent, "seq": self.seq,
            "full": self.full, "rows": self.rows, "cols": self.cols,
            "bytes": Base64::encode_string(&self.bytes)})
        .to_string()
    }
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
            q.items.push_back(Item::Message(text));
        }
        ready.notify_one();
    }

    /// A screen frame. One for an agent whose last frame is still unsent
    /// joins it, up to `QUEUE` frames; past that, or when the queue is full,
    /// it is dropped, which marks the device stale until it drains. Whether
    /// it was taken.
    pub fn frame(&self, screen: Screen) -> bool {
        let (lock, ready) = &*self.0;
        let mut q = lock.lock().expect("outbox");
        if q.stale {
            return false;
        }
        let room = q.items.len() < QUEUE;
        let waiting = q.items.iter_mut().find_map(|item| match item {
            Item::Screen(s, joined) if s.agent == screen.agent => Some((s, joined)),
            _ => None,
        });
        match waiting {
            Some((s, joined)) if *joined < QUEUE => {
                s.join(screen);
                *joined += 1;
                return true;
            }
            Some(_) => {}
            None if room => {
                q.items.push_back(Item::Screen(screen, 0));
                ready.notify_one();
                return true;
            }
            None => {}
        }
        q.stale = true;
        false
    }

    /// How many messages wait to be written.
    pub fn queued(&self) -> usize {
        self.0.0.lock().expect("outbox").items.len()
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
                let text = match q.items.pop_front().expect("an item") {
                    Item::Message(text) => text,
                    Item::Screen(screen, _) => screen.message(),
                };
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
