//! A WebSocket over the Unix socket, one JSON message per text frame.

use std::os::unix::net::UnixStream;

use tungstenite::{Message, WebSocket};

pub(super) struct Websocket(WebSocket<UnixStream>);

impl Websocket {
    pub(super) fn open(stream: UnixStream) -> std::io::Result<Self> {
        let (ws, _) =
            tungstenite::client("ws://localhost/", stream).map_err(std::io::Error::other)?;
        Ok(Websocket(ws))
    }
}

impl super::Framed for Websocket {
    fn send(&mut self, message: &str) -> std::io::Result<()> {
        self.0.send(Message::text(message)).map_err(std::io::Error::other)
    }

    fn next(&mut self) -> std::io::Result<Option<String>> {
        loop {
            match self.0.read().map_err(std::io::Error::other)? {
                Message::Text(text) => return Ok(Some(text.to_string())),
                Message::Close(_) => return Ok(None),
                _ => {}
            }
        }
    }
}
