//! What `weft remote` shows in the TUI's Remote view (remote.md §12),
//! published to the daemon, which relays it to every client.

use serde_json::{Value, json};

/// Said wherever remote access is on (ADR-0021).
pub const DECRYPTED: &str = "traffic is decrypted at Cloudflare's edge";

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// `cloudflared` was not found.
    pub no_cloudflared: bool,
    /// Signing in: the code and where to enter it.
    pub signing_in: Option<(String, String)>,
    /// Why the last sign-in did not finish.
    pub failed: Option<String>,
    /// Joined to this login.
    pub login: Option<String>,
    pub address: Option<String>,
    /// The tunnel is up.
    pub up: bool,
    /// The devices allowed here.
    pub devices: Vec<Value>,
    /// A device asking to be allowed: `{device, name, login}`.
    pub asking: Option<Value>,
    /// With no `cloudflared`: how Weft would install it (remote.md §7a), or
    /// why it cannot.
    pub offer: Result<Value, String>,
    /// Installing it: the last line it said.
    pub installing: Option<String>,
}

impl Default for View {
    fn default() -> Self {
        View {
            no_cloudflared: false,
            signing_in: None,
            failed: None,
            login: None,
            address: None,
            up: false,
            devices: Vec::new(),
            asking: None,
            offer: Err(String::new()),
            installing: None,
        }
    }
}

impl View {
    pub fn json(&self) -> Value {
        let state = if self.installing.is_some() {
            "installing"
        } else if self.no_cloudflared {
            "no_cloudflared"
        } else if self.signing_in.is_some() {
            "signing_in"
        } else if self.login.is_none() {
            "signed_out"
        } else {
            "on"
        };
        let mut v = json!({"state": state});
        if let Some((code, url)) = &self.signing_in {
            v["code"] = json!(code);
            v["url"] = json!(url);
        }
        if let Some(why) = &self.failed {
            v["failed"] = json!(why);
        }
        if let Some(line) = &self.installing {
            v["line"] = json!(line);
        }
        if state == "no_cloudflared" {
            match &self.offer {
                Ok(offer) => v["offer"] = offer.clone(),
                Err(why) => {
                    v["offer"] = Value::Null;
                    v["why"] = json!(why);
                }
            }
        }
        if state == "on" {
            v["login"] = json!(self.login);
            v["address"] = json!(self.address);
            v["tunnel"] = json!(if self.up { "up" } else { "starting" });
            v["note"] = json!(DECRYPTED);
            v["devices"] = json!(self.devices);
        }
        if let Some(asking) = &self.asking {
            v["asking"] = asking.clone();
        }
        v
    }
}
