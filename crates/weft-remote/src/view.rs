//! What `weft remote` shows in the desktop app's Remote view (remote.md §12),
//! published to the daemon, which relays it to every client.

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// No tunnel is chosen: the adapters to choose from, `[{id, title}]`.
    pub choose: Option<Vec<Value>>,
    /// The chosen tunnel's program was not found: `{id, title, program}`.
    pub missing: Option<Value>,
    /// What the tunnel can see, from its adapter, said wherever it is on.
    pub note: Option<String>,
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
    /// With the program missing: how Install would install it (remote.md
    /// §7a), or why it cannot.
    pub offer: Result<Value, String>,
    /// Installing it: the last line it said.
    pub installing: Option<String>,
    /// The tunnel has no address yet: the last line its program said, or
    /// why it ended.
    pub tunnel_why: Option<String>,
    /// More than one tunnel adapter is installed: the tunnel may be changed.
    pub can_change_tunnel: bool,
    /// Choosing again: the adapter chosen until now.
    pub chosen: Option<String>,
}

impl Default for View {
    fn default() -> Self {
        View {
            choose: None,
            missing: None,
            note: None,
            signing_in: None,
            failed: None,
            login: None,
            address: None,
            up: false,
            devices: Vec::new(),
            asking: None,
            offer: Err(String::new()),
            installing: None,
            tunnel_why: None,
            can_change_tunnel: false,
            chosen: None,
        }
    }
}

impl View {
    pub fn json(&self) -> Value {
        let state = if self.installing.is_some() {
            "installing"
        } else if self.choose.is_some() {
            "choose"
        } else if self.missing.is_some() {
            "not_installed"
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
        if let Some(tunnels) = &self.choose {
            v["tunnels"] = json!(tunnels);
            if let Some(id) = &self.chosen {
                v["chosen"] = json!(id);
            }
        }
        if let (Some(why), false) = (&self.tunnel_why, self.up) {
            v["tunnel_why"] = json!(why);
        }
        if self.can_change_tunnel && matches!(state, "on" | "not_installed" | "signed_out") {
            v["can_change_tunnel"] = json!(true);
        }
        if let (Some(tunnel), "not_installed") = (&self.missing, state) {
            v["tunnel"] = tunnel.clone();
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
            if let Some(note) = &self.note {
                v["note"] = json!(note);
            }
            v["devices"] = json!(self.devices);
        }
        if let Some(asking) = &self.asking {
            v["asking"] = asking.clone();
        }
        v
    }
}
