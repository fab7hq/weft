//! Fab7's account service, from this machine (account.md §3, §5): joining
//! the person's account once through GitHub's device flow, and telling the
//! service each new address. The GitHub token is used once and dropped; the
//! machine keeps its own token. Nothing else goes this way.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::store::Machine;

/// The Fab7 Weft GitHub App's client id: public, built in (account.md §3).
pub const CLIENT_ID: &str = "Iv23liun7wYzjax5d6p3";
pub const SERVICE: &str = "https://weft.getfab7.com";
pub const GITHUB: &str = "https://github.com";

/// The longest a device-flow sign-in is waited for.
const SIGN_IN_WITHIN: Duration = Duration::from_secs(15 * 60);

pub struct Account {
    service: String,
    github: String,
    http: ureq::Agent,
}

/// What signing in tells the loop as it goes.
#[derive(Debug, Clone, PartialEq)]
pub enum SignIn {
    /// Enter this code at this address.
    Code {
        code: String,
        url: String,
    },
    Joined(Machine),
    Failed(String),
}

impl Account {
    pub fn new() -> Self {
        Self::at(SERVICE, GITHUB)
    }

    /// The service and GitHub at other addresses: a test's.
    pub fn at(service: &str, github: &str) -> Self {
        let http = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        Account {
            service: service.trim_end_matches('/').into(),
            github: github.trim_end_matches('/').into(),
            http,
        }
    }

    /// Join the person's account as `name`: GitHub's device flow with the
    /// public client id, then `POST /api/machine/enrol`. Each step is told;
    /// `cancelled` stops it between polls.
    pub fn sign_in(&self, name: &str, cancelled: &AtomicBool, mut told: impl FnMut(SignIn)) {
        let outcome = self.join(name, cancelled, &mut told);
        told(match outcome {
            Ok(machine) => SignIn::Joined(machine),
            Err(why) => SignIn::Failed(why),
        });
    }

    fn join(
        &self,
        name: &str,
        cancelled: &AtomicBool,
        told: &mut impl FnMut(SignIn),
    ) -> Result<Machine, String> {
        let (status, code) =
            self.form(&format!("{}/login/device/code", self.github), &[("client_id", CLIENT_ID)])?;
        let device_code =
            code["device_code"].as_str().filter(|_| status == 200).ok_or_else(|| {
                format!(
                    "GitHub would not start a sign-in ({status}): {}",
                    code["error"].as_str().unwrap_or("")
                )
            })?;
        let user_code = code["user_code"].as_str().unwrap_or_default().to_string();
        let url = code["verification_uri"].as_str().unwrap_or("https://github.com/login/device");
        told(SignIn::Code { code: user_code, url: url.trim_start_matches("https://").to_string() });
        let mut every = Duration::from_secs(code["interval"].as_u64().unwrap_or(5));
        let until = Instant::now() + SIGN_IN_WITHIN;
        let token = loop {
            if cancelled.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            if Instant::now() > until {
                return Err("the code expired".into());
            }
            std::thread::sleep(every);
            if cancelled.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let (_, said) = self.form(
                &format!("{}/login/oauth/access_token", self.github),
                &[
                    ("client_id", CLIENT_ID),
                    ("device_code", device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ],
            )?;
            if let Some(token) = said["access_token"].as_str() {
                break token.to_string();
            }
            match said["error"].as_str() {
                Some("authorization_pending") => {}
                // GitHub asks for five seconds more between polls.
                Some("slow_down") => every += Duration::from_secs(5),
                Some(other) => return Err(format!("GitHub said {other}")),
                None => return Err("GitHub's answer had no token".into()),
            }
        };
        let (status, joined) = self.json(
            "POST",
            &format!("{}/api/machine/enrol", self.service),
            None,
            Some(json!({"github_token": token, "name": name})),
        )?;
        drop(token);
        let s = |k: &str| joined[k].as_str().map(str::to_string);
        match (
            status,
            s("machine_id"),
            s("machine_token"),
            joined["github_id"].as_u64(),
            s("login"),
        ) {
            (200, Some(id), Some(token), Some(github_id), Some(login)) => {
                Ok(Machine { id, name: name.to_string(), github_id, login, token })
            }
            _ => Err(format!(
                "Fab7 would not enrol this machine ({status}): {}",
                joined["message"].as_str().unwrap_or("")
            )),
        }
    }

    /// `PUT /api/machine/address`.
    pub fn report(&self, machine: &Machine, address: &str) -> Result<(), String> {
        let url = format!("{}/api/machine/address", self.service);
        match self.json("PUT", &url, Some(&machine.token), Some(json!({"address": address})))? {
            (200 | 204, _) => Ok(()),
            (status, said) => Err(format!("the address was not taken ({status}): {said}")),
        }
    }

    /// `DELETE /api/machine`: this machine leaves the account.
    pub fn leave(&self, machine: &Machine) -> Result<(), String> {
        let url = format!("{}/api/machine", self.service);
        match self.json("DELETE", &url, Some(&machine.token), None)? {
            (200 | 204, _) => Ok(()),
            (status, said) => Err(format!("Fab7 did not remove it ({status}): {said}")),
        }
    }

    fn form(&self, url: &str, fields: &[(&str, &str)]) -> Result<(u16, Value), String> {
        let mut response = self
            .http
            .post(url)
            .header("accept", "application/json")
            .send_form(fields.iter().copied())
            .map_err(|e| format!("{url}: {e}"))?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }

    fn json(
        &self,
        method: &str,
        url: &str,
        bearer: Option<&str>,
        body: Option<Value>,
    ) -> Result<(u16, Value), String> {
        let mut request = http::Request::builder()
            .method(method)
            .uri(url)
            .header("accept", "application/json")
            .header("content-type", "application/json");
        if let Some(token) = bearer {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let request = request.body(body).map_err(|e| e.to_string())?;
        let mut response = self.http.run(request).map_err(|e| format!("{url}: {e}"))?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }
}

impl Default for Account {
    fn default() -> Self {
        Self::new()
    }
}

/// The last address the service was told, so only a new one is sent.
#[derive(Default)]
pub struct Reported(Option<String>);

impl Reported {
    /// Whether `address` is new, noting it if so.
    pub fn is_new(&mut self, address: &str) -> bool {
        if self.0.as_deref() == Some(address) {
            return false;
        }
        self.0 = Some(address.to_string());
        true
    }
}
