//! The frames a client and its server exchange.
//!
//! Spec: `plans/weft/spec/runtime.md`. The server owns the pane processes and
//! outlives any client; a client renders and sends input. Raw harness output
//! crosses the socket unchanged, so a client that attaches later reconstructs
//! exactly what a client that never left would be showing.
//!
//! This transport is internal. The public NDJSON socket is Phase 5, and
//! freezing one here would commit to an API before anyone has used it.

use std::io::{self, Read, Write};

/// `[kind:u8][len:u32 be][payload]`, and for pane traffic the payload opens
/// with `[pane:u32 be]`.
pub const HEADER: usize = 5;
/// Refuse anything absurd rather than allocating on a corrupt length.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToServer {
    /// A client is here. The server replies with `Hello` and a replay.
    Attach { rows: u16, cols: u16 },
    /// Keystrokes for a pane, passed through untouched.
    Input { pane: u32, bytes: Vec<u8> },
    /// Start an agent. `spec` is the command line, arguments and all.
    Spawn { harness: String, spec: String },
    Resize { pane: u32, rows: u16, cols: u16 },
    /// Type a prompt on the person's behalf, with the ordered submission and
    /// the echo wait. The server does it, because the server owns the pane.
    Inject { pane: u32, bytes: Vec<u8> },
    /// Leave, without stopping anything.
    Detach,
    /// Stop every agent and end the session.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToClient {
    /// The panes that exist, in order, followed by a replay of each.
    Hello { panes: Vec<PaneInfo> },
    Output { pane: u32, bytes: Vec<u8> },
    Added { pane: u32, harness: String },
    Exited { pane: u32 },
    /// An injection was attempted, or refused before anything was written.
    Injected { pane: u32, refusal: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane: u32,
    pub harness: String,
    pub running: bool,
}

const ATTACH: u8 = 1;
const INPUT: u8 = 2;
const SPAWN: u8 = 3;
const RESIZE: u8 = 4;
const INJECT: u8 = 5;
const DETACH: u8 = 6;
const SHUTDOWN: u8 = 7;
const HELLO: u8 = 128;
const OUTPUT: u8 = 129;
const ADDED: u8 = 130;
const EXITED: u8 = 131;
const INJECTED: u8 = 132;

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn pane_payload(pane: u32, bytes: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(4 + bytes.len());
    p.extend_from_slice(&pane.to_be_bytes());
    p.extend_from_slice(bytes);
    p
}

fn split_pane(payload: &[u8]) -> Option<(u32, Vec<u8>)> {
    if payload.len() < 4 {
        return None;
    }
    let pane = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    Some((pane, payload[4..].to_vec()))
}

fn text(payload: &[u8]) -> String {
    String::from_utf8_lossy(payload).into_owned()
}

impl ToServer {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ToServer::Attach { rows, cols } => frame(ATTACH, format!("{rows} {cols}").as_bytes()),
            ToServer::Input { pane, bytes } => frame(INPUT, &pane_payload(*pane, bytes)),
            ToServer::Spawn { harness, spec } => {
                frame(SPAWN, format!("{harness}\u{1f}{spec}").as_bytes())
            }
            ToServer::Resize { pane, rows, cols } => {
                frame(RESIZE, format!("{pane} {rows} {cols}").as_bytes())
            }
            ToServer::Inject { pane, bytes } => frame(INJECT, &pane_payload(*pane, bytes)),
            ToServer::Detach => frame(DETACH, &[]),
            ToServer::Shutdown => frame(SHUTDOWN, &[]),
        }
    }

    pub fn decode(kind: u8, payload: &[u8]) -> Option<Self> {
        match kind {
            ATTACH => {
                let t = text(payload);
                let mut it = t.split_whitespace();
                Some(ToServer::Attach {
                    rows: it.next()?.parse().ok()?,
                    cols: it.next()?.parse().ok()?,
                })
            }
            INPUT => split_pane(payload).map(|(pane, bytes)| ToServer::Input { pane, bytes }),
            SPAWN => {
                let t = text(payload);
                let (harness, spec) = t.split_once('\u{1f}')?;
                Some(ToServer::Spawn { harness: harness.into(), spec: spec.into() })
            }
            RESIZE => {
                let t = text(payload);
                let mut it = t.split_whitespace();
                Some(ToServer::Resize {
                    pane: it.next()?.parse().ok()?,
                    rows: it.next()?.parse().ok()?,
                    cols: it.next()?.parse().ok()?,
                })
            }
            INJECT => split_pane(payload).map(|(pane, bytes)| ToServer::Inject { pane, bytes }),
            DETACH => Some(ToServer::Detach),
            SHUTDOWN => Some(ToServer::Shutdown),
            _ => None,
        }
    }
}

impl ToClient {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ToClient::Hello { panes } => {
                let body = panes
                    .iter()
                    .map(|p| format!("{} {} {}", p.pane, u8::from(p.running), p.harness))
                    .collect::<Vec<_>>()
                    .join("\u{1f}");
                frame(HELLO, body.as_bytes())
            }
            ToClient::Output { pane, bytes } => frame(OUTPUT, &pane_payload(*pane, bytes)),
            ToClient::Added { pane, harness } => {
                frame(ADDED, format!("{pane} {harness}").as_bytes())
            }
            ToClient::Exited { pane } => frame(EXITED, pane.to_string().as_bytes()),
            ToClient::Injected { pane, refusal } => frame(
                INJECTED,
                format!("{pane} {}", refusal.clone().unwrap_or_default()).as_bytes(),
            ),
        }
    }

    pub fn decode(kind: u8, payload: &[u8]) -> Option<Self> {
        match kind {
            HELLO => {
                let t = text(payload);
                let panes = if t.is_empty() {
                    Vec::new()
                } else {
                    t.split('\u{1f}')
                        .filter_map(|row| {
                            let mut it = row.splitn(3, ' ');
                            Some(PaneInfo {
                                pane: it.next()?.parse().ok()?,
                                running: it.next()? == "1",
                                harness: it.next()?.to_string(),
                            })
                        })
                        .collect()
                };
                Some(ToClient::Hello { panes })
            }
            OUTPUT => split_pane(payload).map(|(pane, bytes)| ToClient::Output { pane, bytes }),
            ADDED => {
                let t = text(payload);
                let (pane, harness) = t.split_once(' ')?;
                Some(ToClient::Added { pane: pane.parse().ok()?, harness: harness.into() })
            }
            EXITED => Some(ToClient::Exited { pane: text(payload).trim().parse().ok()? }),
            INJECTED => {
                let t = text(payload);
                let (pane, rest) = t.split_once(' ')?;
                Some(ToClient::Injected {
                    pane: pane.parse().ok()?,
                    refusal: if rest.is_empty() { None } else { Some(rest.to_string()) },
                })
            }
            _ => None,
        }
    }
}

/// Reads whole frames from a stream, holding a partial one until it completes.
pub struct Frames<R> {
    inner: R,
    buf: Vec<u8>,
}

impl<R: Read> Frames<R> {
    pub fn new(inner: R) -> Self {
        Self { inner, buf: Vec::new() }
    }

    /// The next complete frame, or `None` when the peer has gone.
    pub fn next(&mut self) -> io::Result<Option<(u8, Vec<u8>)>> {
        loop {
            if let Some(f) = self.take() {
                return Ok(Some(f?));
            }
            let mut chunk = [0u8; 8192];
            match self.inner.read(&mut chunk)? {
                0 => return Ok(None),
                n => self.buf.extend_from_slice(&chunk[..n]),
            }
        }
    }

    fn take(&mut self) -> Option<io::Result<(u8, Vec<u8>)>> {
        if self.buf.len() < HEADER {
            return None;
        }
        let len =
            u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len > MAX_FRAME {
            return Some(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame larger than the limit",
            )));
        }
        if self.buf.len() < HEADER + len {
            return None;
        }
        let kind = self.buf[0];
        let payload = self.buf[HEADER..HEADER + len].to_vec();
        self.buf.drain(..HEADER + len);
        Some(Ok((kind, payload)))
    }
}

pub fn send(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    w.write_all(bytes)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_server(m: ToServer) {
        let wire = m.encode();
        let (kind, payload) = (wire[0], &wire[HEADER..]);
        assert_eq!(ToServer::decode(kind, payload), Some(m));
    }

    fn roundtrip_client(m: ToClient) {
        let wire = m.encode();
        let (kind, payload) = (wire[0], &wire[HEADER..]);
        assert_eq!(ToClient::decode(kind, payload), Some(m));
    }

    #[test]
    fn every_message_to_the_server_survives_the_wire() {
        roundtrip_server(ToServer::Attach { rows: 40, cols: 120 });
        roundtrip_server(ToServer::Input { pane: 2, bytes: b"hello".to_vec() });
        roundtrip_server(ToServer::Spawn {
            harness: "claude-code".into(),
            spec: "claude --model sonnet --effort medium".into(),
        });
        roundtrip_server(ToServer::Resize { pane: 1, rows: 24, cols: 80 });
        roundtrip_server(ToServer::Inject { pane: 0, bytes: b"/rf:eval".to_vec() });
        roundtrip_server(ToServer::Detach);
        roundtrip_server(ToServer::Shutdown);
    }

    #[test]
    fn every_message_to_a_client_survives_the_wire() {
        roundtrip_client(ToClient::Hello {
            panes: vec![
                PaneInfo { pane: 0, harness: "codex".into(), running: true },
                PaneInfo { pane: 1, harness: "claude-code".into(), running: false },
            ],
        });
        roundtrip_client(ToClient::Hello { panes: vec![] });
        roundtrip_client(ToClient::Output { pane: 1, bytes: vec![0x1b, b'[', b'A'] });
        roundtrip_client(ToClient::Added { pane: 3, harness: "codex".into() });
        roundtrip_client(ToClient::Exited { pane: 2 });
        roundtrip_client(ToClient::Injected { pane: 0, refusal: None });
        roundtrip_client(ToClient::Injected { pane: 0, refusal: Some("PaneBlocked".into()) });
    }

    #[test]
    fn harness_output_crosses_unchanged_including_invalid_utf8() {
        // A pane's bytes are the harness's, not text Weft may normalise.
        let raw = vec![0x1b, 0x5b, 0x33, 0x31, 0x6d, 0xff, 0xfe, 0x00, b'x'];
        let m = ToClient::Output { pane: 7, bytes: raw.clone() };
        let wire = m.encode();
        let back = ToClient::decode(wire[0], &wire[HEADER..]).expect("decode");
        assert_eq!(back, ToClient::Output { pane: 7, bytes: raw });
    }

    #[test]
    fn a_frame_split_across_reads_is_held_until_it_completes() {
        let wire = ToClient::Output { pane: 1, bytes: b"abcdefgh".to_vec() }.encode();
        let (head, tail) = wire.split_at(7);
        // A reader that hands over the first part, then the rest.
        let mut frames = Frames::new(std::io::Cursor::new([head, tail].concat()));
        let (kind, payload) = frames.next().expect("read").expect("a frame");
        assert_eq!(ToClient::decode(kind, &payload).unwrap().encode(), wire);
    }

    #[test]
    fn frames_arriving_together_are_read_one_at_a_time() {
        let mut stream = Vec::new();
        stream.extend(ToClient::Output { pane: 0, bytes: b"one".to_vec() }.encode());
        stream.extend(ToClient::Output { pane: 1, bytes: b"two".to_vec() }.encode());
        stream.extend(ToClient::Exited { pane: 1 }.encode());
        let mut frames = Frames::new(std::io::Cursor::new(stream));
        let mut seen = Vec::new();
        while let Some((kind, payload)) = frames.next().expect("read") {
            seen.push(ToClient::decode(kind, &payload).expect("decode"));
        }
        assert_eq!(
            seen,
            vec![
                ToClient::Output { pane: 0, bytes: b"one".to_vec() },
                ToClient::Output { pane: 1, bytes: b"two".to_vec() },
                ToClient::Exited { pane: 1 },
            ]
        );
    }

    #[test]
    fn a_closed_peer_reads_as_gone_not_as_an_error() {
        let mut frames = Frames::new(std::io::Cursor::new(Vec::new()));
        assert_eq!(frames.next().expect("read"), None);
    }

    #[test]
    fn an_absurd_length_is_refused_rather_than_allocated() {
        let mut bad = vec![OUTPUT];
        bad.extend_from_slice(&u32::MAX.to_be_bytes());
        let mut frames = Frames::new(std::io::Cursor::new(bad));
        assert!(frames.next().is_err());
    }

    #[test]
    fn an_unknown_kind_is_ignored_rather_than_guessed_at() {
        assert_eq!(ToServer::decode(200, b"whatever"), None);
        assert_eq!(ToClient::decode(0, b"whatever"), None);
    }
}
