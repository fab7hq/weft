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

/// What the service did with an address a machine reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    Taken,
    /// The machine was removed from its account (on the web, D1.6).
    NotOnAnAccount,
}

#[derive(Clone)]
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
    pub fn report(&self, machine: &Machine, address: &str) -> Result<Report, String> {
        let url = format!("{}/api/machine/address", self.service);
        match self.json("PUT", &url, Some(&machine.token), Some(json!({"address": address})))? {
            (200 | 204, _) => Ok(Report::Taken),
            (401, said) if said["code"] == "unknown_machine" => Ok(Report::NotOnAnAccount),
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

impl Account {
    /// Ask the tunnel's public address for something only Weft answers: a
    /// plain GET of `/ws` is Weft's 404. Anything else came from Cloudflare.
    pub fn probe(&self, address: &str) -> Probe {
        let get = |url: &str| {
            let request =
                http::Request::builder().method("GET").uri(url).body(String::new()).ok()?;
            self.http.run(request).ok().map(|r| r.status().as_u16())
        };
        match get(&format!("{}/ws", address.trim_end_matches('/'))) {
            Some(404) => Probe::Up,
            Some(_) => Probe::Gone,
            // No answer at all: a name Cloudflare dropped, or no network. The
            // service tells which.
            None if get(&self.service).is_some() => Probe::Gone,
            None => Probe::Offline,
        }
    }
}

impl Default for Account {
    fn default() -> Self {
        Self::new()
    }
}

/// How long an address the service took stands before it is told again, so a
/// service that lost it, or never got it, has it back.
pub const REPORT_AGAIN: Duration = Duration::from_secs(10 * 60);
/// The longest wait between tries when the service cannot be reached.
const REPORT_WAIT_MOST: Duration = Duration::from_secs(60);

/// What the service holds of this machine's address: the last one it took,
/// one on its way, and when to try again after a report that did not arrive
/// (no network yet after a laptop wakes, the service down).
#[derive(Default)]
pub struct Reported {
    taken: Option<(String, Instant)>,
    sending: bool,
    retry_at: Option<Instant>,
    failures: u32,
}

impl Reported {
    /// Whether to send `address` now: one the service has not taken, or took
    /// a while ago, with nothing on its way and no wait to keep. Noted as on
    /// its way when it is.
    pub fn due(&mut self, address: &str, now: Instant) -> bool {
        let fresh = self
            .taken
            .as_ref()
            .is_some_and(|(a, at)| a == address && now.duration_since(*at) < REPORT_AGAIN);
        if self.sending || fresh || self.retry_at.is_some_and(|t| now < t) {
            return false;
        }
        self.sending = true;
        true
    }

    /// The service took `address`.
    pub fn taken(&mut self, address: &str, now: Instant) {
        *self = Reported { taken: Some((address.to_string(), now)), ..Default::default() };
    }

    /// It did not arrive: try again, waiting longer each time, up to a minute.
    pub fn failed(&mut self, now: Instant) {
        self.sending = false;
        self.failures += 1;
        let wait = Duration::from_secs(2u64.saturating_pow(self.failures.min(6)));
        self.retry_at = Some(now + wait.min(REPORT_WAIT_MOST));
    }

    /// Tell it again at once: the machine was asleep, and the network it woke
    /// on may be another.
    pub fn again(&mut self) {
        self.taken = None;
        self.retry_at = None;
        self.failures = 0;
    }
}

/// What the tunnel's public address answered from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// Weft's own answer came back through it.
    Up,
    /// The internet answers, but not through the tunnel: Cloudflare no longer
    /// has it (a quick tunnel dropped while the machine slept).
    Gone,
    /// Nothing answers: this machine is offline, and a new tunnel would not help.
    Offline,
}
