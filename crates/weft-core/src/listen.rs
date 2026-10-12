//! A harness file's `[listen]`: the event stream a harness's
//! own server keeps, and what each message on it means to Weft. Weft only
//! listens: it sends the table's hello and nothing else.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How the stream is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// A WebSocket over the Unix socket, one JSON message per text frame.
    Websocket,
    /// One JSON message per line.
    Lines,
}

/// A harness file's `[listen]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listen {
    /// The Unix socket, under the harness's configuration home.
    pub socket: String,
    pub transport: Transport,
    /// Sent once, in order, when the stream opens.
    #[serde(default)]
    pub hello: Vec<Value>,
    /// The dotted path of the session id in a message.
    pub session: String,
    pub on: Vec<Row>,
}

/// One meaning: a message with this method, whose `when` fields match, puts
/// its session in `state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub method: String,
    /// Dotted paths and the value each must hold: a field equal to it, or a
    /// list that holds it.
    #[serde(default)]
    pub when: serde_json::Map<String, Value>,
    pub state: String,
}

impl Listen {
    pub fn of(v: &Value) -> Option<Self> {
        serde_json::from_value(v.clone()).ok()
    }

    /// The session a message names and the state the first matching row
    /// gives it; none for a message no row is for.
    pub fn heard(&self, message: &Value) -> Option<(String, String)> {
        let method = message.get("method")?.as_str()?;
        let row = self.on.iter().find(|r| {
            r.method == method
                && r.when.iter().all(|(path, want)| match crate::signal::value_at(message, path) {
                    Some(Value::Array(items)) => items.contains(want),
                    Some(got) => got == want,
                    None => false,
                })
        })?;
        let session = crate::signal::value_at(message, &self.session)?.as_str()?;
        Some((session.to_string(), row.state.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn listen() -> Listen {
        let table: toml::Table = r#"
            socket = "server/control.sock"
            transport = "websocket"
            hello = [{ id = 1, method = "initialize" }, { method = "initialized" }]
            session = "params.threadId"
            on = [
              { method = "status", when = { "params.status.flags" = "asking" }, state = "waiting" },
              { method = "status", when = { "params.status.type" = "active" },  state = "working" },
              { method = "status", when = { "params.status.type" = "idle" },    state = "turn_ended" },
            ]
        "#
        .parse()
        .unwrap();
        Listen::of(&serde_json::to_value(table).unwrap()).expect("a [listen]")
    }

    fn status(kind: &str, flags: &[&str]) -> Value {
        json!({"method": "status", "params": {"threadId": "t1", "status": {"type": kind, "flags": flags}}})
    }

    #[test]
    fn the_first_matching_row_gives_the_state() {
        let l = listen();
        assert_eq!(l.transport, Transport::Websocket);
        assert_eq!(l.hello.len(), 2);
        assert_eq!(l.heard(&status("active", &[])), Some(("t1".into(), "working".into())));
        assert_eq!(
            l.heard(&status("active", &["other", "asking"])),
            Some(("t1".into(), "waiting".into())),
            "a list that holds it"
        );
        assert_eq!(l.heard(&status("idle", &[])), Some(("t1".into(), "turn_ended".into())));
    }

    #[test]
    fn a_message_no_row_is_for_changes_nothing() {
        let l = listen();
        assert_eq!(l.heard(&status("notLoaded", &[])), None);
        assert_eq!(l.heard(&json!({"method": "other", "params": {"threadId": "t1"}})), None);
        assert_eq!(l.heard(&json!({"id": 1, "result": {}})), None, "an answer to the hello");
        assert_eq!(
            l.heard(&json!({"method": "status", "params": {"status": {"type": "idle"}}})),
            None,
            "no session"
        );
    }

    #[test]
    fn a_table_missing_a_field_is_no_stream() {
        assert_eq!(
            Listen::of(&json!({"socket": "s", "transport": "websocket", "session": "id"})),
            None
        );
        assert_eq!(
            Listen::of(&json!({"socket": "s", "transport": "pigeon", "session": "id", "on": []})),
            None
        );
        assert_eq!(Listen::of(&Value::Null), None);
    }
}
