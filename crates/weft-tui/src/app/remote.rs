//! Weft from another device, in the TUI (remote.md §12): `weft remote`'s
//! view under `W` → `R`, and the question a device asking to be allowed
//! puts in front of the person, by itself, on any screen.

use super::*;

impl App {
    fn remote_state(&self) -> &str {
        self.remote_view().and_then(|v| v["state"].as_str()).unwrap_or("off")
    }

    /// The devices allowed here, as the view lists them.
    pub fn remote_devices(&self) -> Vec<serde_json::Value> {
        self.remote_view().and_then(|v| v["devices"].as_array().cloned()).unwrap_or_default()
    }

    fn remote_act(&mut self, act: serde_json::Value) {
        let _ = sess!(self).remote_act(act);
    }

    pub(super) fn on_remote_key(&mut self, key: KeyEvent) {
        let state = self.remote_state().to_string();
        let devices = self.remote_devices();
        // "Run this?" is answered first, and nothing runs without a yes.
        if let Some(how) = self.remote_run.clone() {
            match key.code {
                event::KeyCode::Char('y' | 'Y') => {
                    self.remote_act(serde_json::json!({"act": "install", "how": how}));
                    self.remote_run = None;
                }
                event::KeyCode::Char('n' | 'N') | event::KeyCode::Esc => self.remote_run = None,
                _ => {}
            }
            return;
        }
        let offer = self.remote_view().map(|v| v["offer"].clone()).unwrap_or_default();
        match key.code {
            event::KeyCode::Char('i' | 'I') if state == "no_cloudflared" => {
                if let Some(how) = offer["how"].as_str() {
                    self.remote_run = Some(how.to_string());
                }
            }
            event::KeyCode::Char('d' | 'D')
                if state == "no_cloudflared" && offer["download"] == true =>
            {
                self.remote_run = Some("download".into());
            }
            event::KeyCode::Up => self.modal_choice = self.modal_choice.saturating_sub(1),
            event::KeyCode::Down => {
                self.modal_choice = (self.modal_choice + 1).min(devices.len().saturating_sub(1))
            }
            event::KeyCode::Enter => match state.as_str() {
                "off" => self.remote_act(serde_json::json!({"act": "on"})),
                "no_cloudflared" => self.remote_act(serde_json::json!({"act": "look"})),
                "signed_out" => self.remote_act(serde_json::json!({"act": "sign_in"})),
                _ => {}
            },
            event::KeyCode::Char('o' | 'O') if state != "off" => {
                self.remote_act(serde_json::json!({"act": "off"}))
            }
            event::KeyCode::Char('d' | 'D') if state == "on" => {
                if let Some(id) = devices.get(self.modal_choice).and_then(|d| d["id"].as_str()) {
                    self.remote_act(serde_json::json!({"act": "remove", "device": id}));
                    self.modal_choice = self.modal_choice.saturating_sub(1);
                }
            }
            event::KeyCode::Esc if state == "signing_in" => {
                self.remote_act(serde_json::json!({"act": "cancel"}))
            }
            event::KeyCode::Left | event::KeyCode::Esc => self.modal = None,
            _ => {}
        }
    }

    pub(super) fn on_allow_key(&mut self, key: KeyEvent, device: &str) {
        let yes = match key.code {
            event::KeyCode::Char('y' | 'Y') => true,
            event::KeyCode::Char('n' | 'N') => false,
            _ => return,
        };
        self.remote_act(serde_json::json!({"act": "allow", "device": device, "yes": yes}));
        self.modal = self.before_allow.take();
    }

    /// A device asking to be allowed opens the question by itself, over
    /// whatever is in front; when the request goes (answered elsewhere, or
    /// expired) the question goes with it, and what was in front comes back.
    pub(super) fn offer_allow(&mut self) {
        let asking = self.remote_view().map(|v| v["asking"].clone()).filter(|a| a.is_object());
        let shown = match &self.modal {
            Some(Modal::Allow { device, .. }) => Some(device.clone()),
            _ => None,
        };
        let wanted = asking.as_ref().and_then(|a| a["device"].as_str()).map(str::to_string);
        if shown == wanted {
            return;
        }
        if shown.is_some() {
            self.modal = self.before_allow.take();
        }
        if let (Some(a), Some(device)) = (asking, wanted) {
            let s = |k: &str| a[k].as_str().unwrap_or_default().to_string();
            self.before_allow = self.modal.take();
            self.modal = Some(Modal::Allow { device, name: s("name"), login: s("login") });
        }
    }
}

/// How long ago, said plainly.
pub fn ago(millis: i64, now: i64) -> String {
    let minutes = (now - millis).max(0) / 60_000;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes} min ago"),
        60..=2879 => format!("{} h ago", minutes / 60),
        _ => format!("{} days ago", minutes / 1440),
    }
}
