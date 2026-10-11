//! A push when an agent needs the person and they are away (remote.md §10).
//! The machine sends Web Push itself, encrypted to each device,
//! with its own VAPID key; nothing goes through Fab7. A push says nothing the
//! app would not show, and is never taken as an answer.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use base64ct::{Base64UrlUnpadded, Encoding};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use serde_json::{Value, json};
use web_push_native::{Auth, WebPushBuilder};

/// No key pressed on the machine for this long, and the person is away.
pub const AWAY: Duration = Duration::from_secs(120);
/// Who to write to about this sender.
const CONTACT: &str = "mailto:push@getfab7.com";
/// How long a push waits for a device that is off.
const TTL: Duration = Duration::from_secs(3600);
/// One JWT per push service, renewed at most hourly.
const JWT_FOR: Duration = Duration::from_secs(3600);
const JWT_RENEW: Duration = Duration::from_secs(50 * 60);

/// The machine's VAPID key: ES256 (P-256), the only signature Web Push asks of it.
pub type Vapid = SigningKey;

/// The VAPID key pair, made on first start and kept in `vapid.pem` (0600,
/// PKCS#8).
pub fn vapid(dir: &Path) -> anyhow::Result<Vapid> {
    let path = dir.join("vapid.pem");
    if let Ok(pem) = std::fs::read_to_string(&path) {
        return Vapid::from_pkcs8_pem(&pem).map_err(|e| anyhow::anyhow!("{e}"));
    }
    let key = loop {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)?;
        // Nearly every 32 bytes is a scalar; the rest are drawn again.
        if let Ok(key) = Vapid::from_slice(&bytes) {
            break key;
        }
    };
    let pem = key.to_pkcs8_pem(LineEnding::LF).map_err(|e| anyhow::anyhow!("{e}"))?;
    crate::store::write_secret(&path, &pem)?;
    Ok(key)
}

/// The public key a browser subscribes with: base64url of the uncompressed
/// point.
pub fn public(key: &Vapid) -> String {
    Base64UrlUnpadded::encode_string(key.verifying_key().to_encoded_point(false).as_bytes())
}

/// The `Authorization` a push service asks of a sender (RFC 8292): a JWT
/// signed with ES256 for the push service's origin, and the public key.
pub fn authorization(key: &Vapid, audience: &str, now: u64) -> String {
    let part = |v: Value| Base64UrlUnpadded::encode_string(v.to_string().as_bytes());
    let claims = json!({"aud": audience, "exp": now + JWT_FOR.as_secs(), "sub": CONTACT});
    let signed = format!("{}.{}", part(json!({"typ": "JWT", "alg": "ES256"})), part(claims));
    let signature: Signature = key.sign(signed.as_bytes());
    let token = format!("{signed}.{}", Base64UrlUnpadded::encode_string(&signature.to_bytes()));
    format!("vapid t={token}, k={}", public(key))
}

/// When to push: once per change to `waiting`, while the person is away and
/// no device has the agent open in the foreground.
#[derive(Default)]
pub struct Rules {
    /// Agents waiting now, and whether this wait was pushed.
    waiting: HashMap<String, bool>,
}

impl Rules {
    /// The agents' turns as they stand; those newly waiting are due.
    pub fn turns(&mut self, turns: &[(String, &str)]) {
        let now: HashSet<&str> =
            turns.iter().filter(|(_, t)| *t == "waiting").map(|(a, _)| a.as_str()).collect();
        self.waiting.retain(|a, _| now.contains(a.as_str()));
        for a in now {
            self.waiting.entry(a.to_string()).or_insert(false);
        }
    }

    /// How many agents need the person: the badge.
    pub fn count(&self) -> usize {
        self.waiting.len()
    }

    /// The agents to push for now, each taken once.
    pub fn due(&mut self, away: bool, in_front: &HashSet<String>) -> Vec<String> {
        if !away {
            return Vec::new();
        }
        let mut due = Vec::new();
        for (agent, pushed) in &mut self.waiting {
            if !*pushed && !in_front.contains(agent) {
                *pushed = true;
                due.push(agent.clone());
            }
        }
        due.sort();
        due
    }

    /// A device has it open in front of the person: this wait needs no push.
    pub fn seen(&mut self, agent: &str) {
        if let Some(pushed) = self.waiting.get_mut(agent) {
            *pushed = true;
        }
    }
}

/// One push to send.
pub struct Job {
    pub device: String,
    pub subscription: Value,
    pub agent: String,
    pub payload: Value,
}

/// What a push service said, when it matters: the subscription is gone.
pub struct Gone {
    pub device: String,
}

/// The push thread: sends one at a time, and tells of subscriptions a push
/// service says are gone (404 or 410).
pub fn sender<T: From<Gone> + Send + 'static>(key: Vapid, gone: Sender<T>) -> Sender<Job> {
    let (tx, rx) = channel::<Job>();
    std::thread::spawn(move || {
        let http: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        let mut jwts: HashMap<String, (Instant, String)> = HashMap::new();
        while let Ok(job) = rx.recv() {
            let Some(request) = request(&key, &mut jwts, &job) else { continue };
            if let Ok(response) = http.run(request)
                && matches!(response.status().as_u16(), 404 | 410)
                && gone.send(Gone { device: job.device }.into()).is_err()
            {
                return;
            }
        }
    });
    tx
}

/// The encrypted request for one push, with its headers.
pub fn request(
    key: &Vapid,
    jwts: &mut HashMap<String, (Instant, String)>,
    job: &Job,
) -> Option<http::Request<Vec<u8>>> {
    let endpoint: http::Uri = job.subscription["endpoint"].as_str()?.parse().ok()?;
    let decode =
        |k: &str| Base64UrlUnpadded::decode_vec(job.subscription["keys"][k].as_str()?).ok();
    let ua_public = web_push_native::p256::PublicKey::from_sec1_bytes(&decode("p256dh")?).ok()?;
    let auth_bytes = decode("auth")?;
    if auth_bytes.len() != 16 {
        return None;
    }
    let auth = Auth::clone_from_slice(&auth_bytes);
    let origin = format!("{}://{}", endpoint.scheme_str()?, endpoint.authority()?);
    let fresh = jwts.get(&origin).is_some_and(|(made, _)| made.elapsed() < JWT_RENEW);
    if !fresh {
        // The audience is the push service's scheme and host, without a port.
        let audience = format!("{}://{}", endpoint.scheme_str()?, endpoint.host()?);
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        jwts.insert(origin.clone(), (Instant::now(), authorization(key, &audience, now)));
    }
    let vapid = jwts[&origin].1.clone();
    let mut request = WebPushBuilder::new(endpoint, ua_public, auth)
        .with_valid_duration(TTL)
        .build(job.payload.to_string())
        .ok()?;
    let headers = request.headers_mut();
    headers.insert("authorization", vapid.parse().ok()?);
    headers.insert("urgency", "high".parse().ok()?);
    // A newer push for the same agent replaces an older one.
    headers.insert("topic", job.agent.parse().ok()?);
    Some(request)
}

/// What a push says: the same words as the terminal notice, nothing from the
/// conversation.
pub fn payload(agent: &str, harness: &str, project: &str, count: usize) -> Value {
    json!({"agent": agent, "title": format!("{harness} in {project}"),
           "body": "needs your input", "count": count})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vapid_jwt_verifies_with_the_key_it_names() {
        use p256::ecdsa::VerifyingKey;
        use p256::ecdsa::signature::Verifier;
        let dir = std::env::temp_dir().join(format!("weft-vapid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = vapid(&dir).unwrap();
        assert_eq!(vapid(&dir).unwrap(), key, "kept in vapid.pem");
        let header = authorization(&key, "https://push.example", 1_000);
        let (token, k) = header.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
        assert_eq!(k, public(&key));
        let [head, claims, signature]: [&str; 3] =
            token.split('.').collect::<Vec<_>>().try_into().unwrap();
        let read = |s: &str| {
            serde_json::from_slice::<Value>(&Base64UrlUnpadded::decode_vec(s).unwrap()).unwrap()
        };
        assert_eq!(read(head), json!({"typ": "JWT", "alg": "ES256"}));
        assert_eq!(
            read(claims),
            json!({"aud": "https://push.example", "exp": 4_600, "sub": CONTACT})
        );
        let point =
            VerifyingKey::from_sec1_bytes(&Base64UrlUnpadded::decode_vec(k).unwrap()).unwrap();
        let signature =
            Signature::from_slice(&Base64UrlUnpadded::decode_vec(signature).unwrap()).unwrap();
        point.verify(format!("{head}.{claims}").as_bytes(), &signature).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn once_per_change_while_away_and_out_of_sight() {
        let mut rules = Rules::default();
        let none = HashSet::new();
        rules.turns(&[("a".into(), "waiting"), ("b".into(), "working")]);
        assert_eq!(rules.count(), 1);
        assert!(rules.due(false, &none).is_empty(), "held while the person is at the machine");
        let front: HashSet<String> = ["a".to_string()].into();
        assert!(rules.due(true, &front).is_empty(), "not while a device shows it");
        assert_eq!(rules.due(true, &none), ["a"]);
        assert!(rules.due(true, &none).is_empty(), "once per change");
        rules.turns(&[("a".into(), "working")]);
        rules.turns(&[("a".into(), "waiting")]);
        assert_eq!(rules.due(true, &none), ["a"], "a new change, a new push");
        rules.turns(&[("a".into(), "working")]);
        rules.turns(&[("a".into(), "waiting")]);
        rules.seen("a");
        assert!(rules.due(true, &none).is_empty(), "seen in front since");
    }
}
