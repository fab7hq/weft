//! Who gets in (remote.md §8, ADR-0020). A device is let in only after the
//! person allows it on this machine, and asks only with Fab7's assertion that
//! it belongs to the machine's owner; anything else is refused silently. The
//! machine issues the device's token and keeps only its SHA-256
//! (`devices.toml`).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64ct::{Base64UrlUnpadded, Encoding};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Fab7's public key for assertions, built in: rotating it is a Weft
/// release (account.md §7). The private half is the account service's.
pub const FAB7_KEY: &str = "Wv894MwG4Iu2RQmFq0oo43N32BK5PykMJ2FBmiOQt2g";

pub fn fab7_key() -> VerifyingKey {
    key(FAB7_KEY).expect("the built-in key is a key")
}

/// A base64url Ed25519 public key.
pub fn key(text: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = Base64UrlUnpadded::decode_vec(text).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// What Fab7 signed: this GitHub user may ask this machine to allow this
/// device, until `exp` (seconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub sub: u64,
    pub login: String,
    pub machine: String,
    pub device: String,
    pub exp: i64,
}

/// `base64url(json) "." base64url(signature)`, the signature over the first
/// part as sent. Checked against the key, the clock, this machine and its
/// owner; why not, if not.
pub fn check(
    key: &VerifyingKey,
    text: &str,
    now: i64,
    machine: &str,
    owner: u64,
) -> Result<Assertion, &'static str> {
    let (payload, signature) = text.split_once('.').ok_or("not an assertion")?;
    let signature: [u8; 64] = Base64UrlUnpadded::decode_vec(signature)
        .ok()
        .and_then(|s| s.try_into().ok())
        .ok_or("not a signature")?;
    key.verify_strict(payload.as_bytes(), &Signature::from_bytes(&signature))
        .map_err(|_| "not signed by Fab7")?;
    let json: Value = Base64UrlUnpadded::decode_vec(payload)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("not JSON")?;
    let s = |k: &str| json[k].as_str().map(str::to_string);
    let a = Assertion {
        sub: json["sub"].as_u64().ok_or("no subject")?,
        login: s("login").ok_or("no login")?,
        machine: s("machine").ok_or("no machine")?,
        device: s("device").ok_or("no device")?,
        exp: json["exp"].as_i64().ok_or("no expiry")?,
    };
    if json["iss"] != "fab7" {
        return Err("not issued by fab7");
    }
    if a.exp <= now {
        return Err("expired");
    }
    if a.machine != machine {
        return Err("another machine");
    }
    if a.sub != owner {
        return Err("another person");
    }
    Ok(a)
}

/// At most three requests to be allowed, from all devices together, in ten
/// minutes: more are refused as busy without asking the person.
#[derive(Default)]
pub struct Gate(VecDeque<Instant>);

const ASKS: usize = 3;
const ASKS_WITHIN: Duration = Duration::from_secs(10 * 60);

impl Gate {
    pub fn may_ask(&mut self, now: Instant) -> bool {
        while self.0.front().is_some_and(|t| now.duration_since(*t) >= ASKS_WITHIN) {
            self.0.pop_front();
        }
        if self.0.len() >= ASKS {
            return false;
        }
        self.0.push_back(now);
        true
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// 32 random bytes, base64url.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system's randomness");
    Base64UrlUnpadded::encode_string(&bytes)
}

/// A device the person allowed here.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub login: String,
    pub token_sha256: String,
    pub allowed_at: i64,
    pub last_seen: i64,
    /// Its Web Push subscription, when it turned pushes on.
    pub push: Option<Value>,
}

/// `devices.toml`.
pub struct Devices {
    path: PathBuf,
    pub list: Vec<Device>,
}

impl Devices {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("devices.toml");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let table: toml::Table = text.parse().unwrap_or_default();
        let list = table
            .get("device")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|d| {
                let s = |k: &str| d.get(k)?.as_str().map(str::to_string);
                let n = |k: &str| d.get(k).and_then(toml::Value::as_integer).unwrap_or(0);
                Some(Device {
                    id: s("id")?,
                    name: s("name")?,
                    login: s("login")?,
                    token_sha256: s("token_sha256")?,
                    allowed_at: n("allowed_at"),
                    last_seen: n("last_seen"),
                    push: s("push").and_then(|p| serde_json::from_str(&p).ok()),
                })
            })
            .collect();
        Devices { path, list }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let devices: Vec<toml::Value> = self
            .list
            .iter()
            .map(|d| {
                let mut t = toml::Table::new();
                t.insert("id".into(), d.id.clone().into());
                t.insert("name".into(), d.name.clone().into());
                t.insert("login".into(), d.login.clone().into());
                t.insert("token_sha256".into(), d.token_sha256.clone().into());
                t.insert("allowed_at".into(), d.allowed_at.into());
                t.insert("last_seen".into(), d.last_seen.into());
                if let Some(p) = &d.push {
                    t.insert("push".into(), p.to_string().into());
                }
                toml::Value::Table(t)
            })
            .collect();
        let mut file = toml::Table::new();
        file.insert("device".into(), toml::Value::Array(devices));
        crate::store::write_secret(&self.path, &file.to_string())
    }

    /// Whether this device is allowed here and this is its token.
    pub fn admits(&self, id: &str, token: &str) -> bool {
        let hash = sha256_hex(token.as_bytes());
        self.list.iter().any(|d| d.id == id && d.token_sha256 == hash)
    }

    /// Allow it: its token, which is kept only as a hash.
    pub fn allow(&mut self, id: &str, name: &str, login: &str, now: i64) -> String {
        let token = random_token();
        self.list.retain(|d| d.id != id);
        self.list.push(Device {
            id: id.into(),
            name: name.into(),
            login: login.into(),
            token_sha256: sha256_hex(token.as_bytes()),
            allowed_at: now,
            last_seen: now,
            push: None,
        });
        token
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|d| d.id != id);
        before != self.list.len()
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Device> {
        self.list.iter_mut().find(|d| d.id == id)
    }

    /// What the Remote view lists: name, login, last seen.
    pub fn shown(&self) -> Vec<Value> {
        self.list
            .iter()
            .map(|d| {
                serde_json::json!({"id": d.id, "name": d.name, "login": d.login,
                                   "last_seen": d.last_seen})
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed(key: &SigningKey, json: &Value) -> String {
        let payload = Base64UrlUnpadded::encode_string(json.to_string().as_bytes());
        let signature = key.sign(payload.as_bytes());
        format!("{payload}.{}", Base64UrlUnpadded::encode_string(&signature.to_bytes()))
    }

    #[test]
    fn only_fab7s_word_for_this_machines_owner_and_in_time_is_taken() {
        let fab7 = SigningKey::from_bytes(&[7; 32]);
        let other = SigningKey::from_bytes(&[8; 32]);
        let key = fab7.verifying_key();
        let good = serde_json::json!({"iss": "fab7", "sub": 42, "login": "octo",
                                      "machine": "m_1", "device": "d_1", "exp": 1060});
        let ok = check(&key, &signed(&fab7, &good), 1000, "m_1", 42).unwrap();
        assert_eq!((ok.login.as_str(), ok.device.as_str()), ("octo", "d_1"));
        assert_eq!(check(&key, &signed(&other, &good), 1000, "m_1", 42), Err("not signed by Fab7"));
        assert_eq!(check(&key, &signed(&fab7, &good), 1060, "m_1", 42), Err("expired"));
        assert_eq!(check(&key, &signed(&fab7, &good), 1000, "m_2", 42), Err("another machine"));
        assert_eq!(check(&key, &signed(&fab7, &good), 1000, "m_1", 43), Err("another person"));
        let mut forged = signed(&fab7, &good);
        forged.insert(3, 'x');
        assert!(check(&key, &forged, 1000, "m_1", 42).is_err(), "changed after signing");
        assert!(check(&key, "nonsense", 1000, "m_1", 42).is_err());
        assert!(super::key(FAB7_KEY).is_some(), "the built-in key reads");
    }

    #[test]
    fn three_asks_in_ten_minutes_then_busy() {
        let mut gate = Gate::default();
        let t = Instant::now();
        assert!(gate.may_ask(t) && gate.may_ask(t) && gate.may_ask(t));
        assert!(!gate.may_ask(t + Duration::from_secs(599)));
        assert!(gate.may_ask(t + Duration::from_secs(600)), "ten minutes on, one more");
    }

    #[test]
    fn a_device_is_kept_by_the_hash_of_its_token() {
        let dir = std::env::temp_dir().join(format!("weft-devices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut devices = Devices::load(&dir);
        let token = devices.allow("d_1", "iPhone", "octo", 5);
        devices.save().unwrap();
        let text = std::fs::read_to_string(dir.join("devices.toml")).unwrap();
        assert!(!text.contains(&token), "only its hash is kept");
        let devices = Devices::load(&dir);
        assert!(devices.admits("d_1", &token));
        assert!(!devices.admits("d_1", "another"));
        assert!(!devices.admits("d_2", &token));
        std::fs::remove_dir_all(&dir).ok();
    }
}
