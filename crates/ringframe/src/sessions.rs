//! Bounded, prunable hook captures under `.fab7/rf/sessions/<host>/<session>/`.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::digest;
use crate::store::canonical;
use crate::workspace::Workspace;

/// Claude Code's slash skill, and Codex's dollar skill.
pub const PREFIXES: [&str; 2] = ["/rf:", "$rf:"];

pub fn now() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is before 1970");
    format_utc(d.as_secs() as i64, d.subsec_millis())
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, which is what the record has always carried.
fn format_utc(secs: i64, millis: u32) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's civil-from-days, which is the short way to do this without
/// a calendar library.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Milliseconds since the epoch from the shape this record writes:
/// `YYYY-MM-DDTHH:MM:SS[.mmm][Z|±HH:MM]`. Anything else is not our timestamp.
pub fn parse_time(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let num = |a: usize, b: usize| text.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let rest = &text[19..];
    let (frac, rest) = match rest.strip_prefix('.') {
        Some(r) => {
            let digits: String = r.chars().take_while(char::is_ascii_digit).collect();
            let ms = format!("{digits:0<3}")[..3].parse::<i64>().ok()?;
            (ms, &r[digits.len()..])
        }
        None => (0, rest),
    };
    let offset = match rest {
        "" | "Z" | "z" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let o = o.trim_start_matches(['+', '-']);
            let (oh, om) = o.split_once(':').unwrap_or((o, "0"));
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
        }
    };
    let days = days_from_civil(y, mo as u32, d as u32);
    Some((days * 86_400 + h * 3600 + mi * 60 + s - offset) * 1000 + frac)
}

/// The inverse of `civil_from_days`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn now_millis() -> i64 {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is before 1970");
    d.as_millis() as i64
}

fn dir(ws: &Workspace, host: &str, session: &str) -> std::io::Result<PathBuf> {
    ws.ensure()?;
    let d = ws.rf_dir().join("sessions").join(host).join(session);
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

pub fn log(
    ws: &Workspace,
    host: &str,
    session: &str,
    name: &str,
    record: &Value,
) -> std::io::Result<()> {
    let mut out = Map::new();
    out.insert("time".into(), Value::String(now()));
    for (k, v) in record.as_object().into_iter().flatten() {
        out.insert(k.clone(), v.clone());
    }
    let mut line = canonical(&Value::Object(out));
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(dir(ws, host, session)?.join(name))?;
    f.write_all(&line)
}

/// Store a `/rf:` or `$rf:` invocation in full; every other prompt as digest
/// and byte count only, never its text.
pub fn capture(
    ws: &Workspace,
    host: &str,
    payload: &Value,
    host_version: Option<&str>,
) -> std::io::Result<Option<Value>> {
    let prompt = payload.get("prompt").and_then(Value::as_str).unwrap_or_default();
    let session = payload.get("session_id").and_then(Value::as_str).unwrap_or_default();
    if prompt.is_empty() || session.is_empty() {
        return Ok(None);
    }
    let data = prompt.as_bytes();
    let version = host_version.map(str::trim).filter(|v| !v.is_empty());
    let mut rec = json!({
        "session_id": session,
        "sha256": digest::sha256_bytes(data),
        "bytes": data.len(),
        "cwd": payload.get("cwd").cloned().unwrap_or(Value::Null),
        "permission_mode": payload.get("permission_mode").cloned().unwrap_or(Value::Null),
        "host_version": version.map_or(Value::Null, |v| Value::String(v.to_string())),
    });
    if PREFIXES.iter().any(|p| prompt.starts_with(p)) {
        rec["prompt"] = Value::String(prompt.to_string());
    }
    log(ws, host, session, "prompts.jsonl", &rec)?;
    Ok(Some(rec))
}

/// Compare source bytes with the argument bytes of a captured `/rf:ask`
/// invocation.
pub fn source_verified(
    ws: &Workspace,
    host: &str,
    session: Option<&str>,
    source: &[u8],
) -> (String, Option<String>) {
    let Some(session) = session.filter(|s| !s.is_empty()) else {
        return ("unverified".into(), Some("no_session_ref".into()));
    };
    let path = ws.rf_dir().join("sessions").join(host).join(session).join("prompts.jsonl");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ("unverified".into(), Some("no_capture".into()));
    };
    let want = String::from_utf8_lossy(source).trim_end_matches('\n').to_string();
    for line in raw.lines() {
        let rec: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if matches(rec.get("prompt").and_then(Value::as_str).unwrap_or_default(), &want) {
            return ("exact".into(), None);
        }
    }
    ("unverified".into(), Some("mismatch".into()))
}

fn matches(prompt: &str, want: &str) -> bool {
    let (head, args) = prompt.split_once(' ').unwrap_or((prompt, ""));
    PREFIXES.iter().any(|p| head == format!("{p}ask")) && args.trim_end_matches('\n') == want
}

/// The one session whose recent captured `/rf:ask` invocation carries exactly
/// these source bytes.
///
/// The model never knows its own session id; the hook does. Ambiguity resolves
/// to nothing.
pub fn resolve_session(ws: &Workspace, host: &str, source: &[u8], window: Duration) -> Option<Value> {
    let base = ws.rf_dir().join("sessions").join(host);
    let want = String::from_utf8_lossy(source).trim_end_matches('\n').to_string();
    let cutoff = SystemTime::now().checked_sub(window)?;
    let mut sessions: Vec<PathBuf> =
        std::fs::read_dir(&base).into_iter().flatten().flatten().map(|e| e.path()).collect();
    sessions.sort();
    let mut hits = Vec::new();
    for session in sessions {
        let path = session.join("prompts.jsonl");
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.modified().is_ok_and(|m| m < cutoff) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        for line in raw.lines() {
            let rec: Value = serde_json::from_str(line).unwrap_or(Value::Null);
            if matches(rec.get("prompt").and_then(Value::as_str).unwrap_or_default(), &want) {
                hits.push(json!({
                    "session_ref": session.file_name().unwrap_or_default().to_string_lossy(),
                    "host_version": rec.get("host_version").cloned().unwrap_or(Value::Null),
                }));
                break;
            }
        }
    }
    (hits.len() == 1).then(|| hits.remove(0))
}

/// The window `resolve_session` looks back over when nothing says otherwise.
pub const WINDOW: Duration = Duration::from_secs(1800);

pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let bad = || "duration must look like 7d or 36h".to_string();
    let (digits, unit) = text.split_at(text.len().checked_sub(1).ok_or_else(bad)?);
    let n: u64 = digits.parse().map_err(|_| bad())?;
    match unit {
        "d" => Ok(Duration::from_secs(n * 86_400)),
        "h" => Ok(Duration::from_secs(n * 3_600)),
        _ => Err(bad()),
    }
}

pub fn prune(ws: &Workspace, older_than: &str) -> Result<Vec<String>, String> {
    let window = parse_duration(older_than)?;
    let cutoff = SystemTime::now().checked_sub(window).ok_or("the window is too long")?;
    let mut removed = Vec::new();
    let base = ws.rf_dir().join("sessions");
    let mut hosts: Vec<PathBuf> =
        std::fs::read_dir(&base).into_iter().flatten().flatten().map(|e| e.path()).collect();
    hosts.sort();
    for host in hosts {
        let mut kids: Vec<PathBuf> =
            std::fs::read_dir(&host).into_iter().flatten().flatten().map(|e| e.path()).collect();
        kids.sort();
        for session in kids {
            let newest = std::fs::read_dir(&session)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| e.metadata().ok()?.modified().ok())
                .max()
                .or_else(|| std::fs::metadata(&session).ok()?.modified().ok());
            if newest.is_some_and(|n| n < cutoff) {
                std::fs::remove_dir_all(&session).map_err(|e| e.to_string())?;
                removed.push(format!(
                    "{}/{}",
                    host.file_name().unwrap_or_default().to_string_lossy(),
                    session.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{repo, ws_for};

    fn payload(prompt: &str, session: &str) -> Value {
        json!({"hook_event_name": "UserPromptSubmit", "session_id": session, "cwd": "/w",
               "permission_mode": "default", "prompt": prompt})
    }

    fn prompts(ws: &Workspace, host: &str, session: &str) -> String {
        std::fs::read_to_string(
            ws.rf_dir().join("sessions").join(host).join(session).join("prompts.jsonl"),
        )
        .unwrap()
    }

    #[test]
    fn capture_stores_only_rf_invocations() {
        let repo = repo();
        let ws = ws_for(repo.path());
        let other =
            capture(&ws, "claude-code", &payload("hello there", "s1"), None).unwrap().unwrap();
        assert!(other["sha256"].is_string());
        assert_eq!(other["bytes"], 11);
        assert!(other.get("prompt").is_none(), "the text must never be stored");
        assert!(!prompts(&ws, "claude-code", "s1").contains("hello there"));

        let rec = capture(&ws, "claude-code", &payload("/rf:ask fix the login bug", "s1"), None)
            .unwrap()
            .unwrap();
        assert!(rec["sha256"].is_string());
        assert_eq!(rec["bytes"], "/rf:ask fix the login bug".len());
        let text = prompts(&ws, "claude-code", "s1");
        let stored: Vec<&str> = text.lines().collect();
        assert_eq!(stored.len(), 2);
        let second: Value = serde_json::from_str(stored[1]).unwrap();
        assert_eq!(second["prompt"], "/rf:ask fix the login bug");
    }

    #[test]
    fn find_invocation_matches_argument_bytes() {
        let repo = repo();
        let ws = ws_for(repo.path());
        capture(&ws, "claude-code", &payload("/rf:ask fix the login bug", "s1"), None).unwrap();
        let got = |s: Option<&str>, b: &[u8]| source_verified(&ws, "claude-code", s, b);
        assert_eq!(got(Some("s1"), b"fix the login bug\n"), ("exact".into(), None));
        assert_eq!(
            got(Some("s1"), b"fix login"),
            ("unverified".into(), Some("mismatch".into()))
        );
        assert_eq!(got(Some("nope"), b"x"), ("unverified".into(), Some("no_capture".into())));
        assert_eq!(got(None, b"x"), ("unverified".into(), Some("no_session_ref".into())));
    }

    #[test]
    fn capture_records_host_version_and_resolves_the_session() {
        let repo = repo();
        let ws = ws_for(repo.path());
        let rec = capture(
            &ws,
            "claude-code",
            &payload("/rf:ask fix the login bug", "sA"),
            Some("2.1.263 (Claude Code)\n"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(rec["host_version"], "2.1.263 (Claude Code)");
        assert_eq!(
            resolve_session(&ws, "claude-code", b"fix the login bug\n", WINDOW),
            Some(json!({"session_ref": "sA", "host_version": "2.1.263 (Claude Code)"}))
        );
        assert_eq!(resolve_session(&ws, "claude-code", b"something else", WINDOW), None);
        capture(&ws, "claude-code", &payload("/rf:ask fix the login bug", "sB"), None).unwrap();
        // Two sessions carry the same intent, so neither can be claimed.
        assert_eq!(resolve_session(&ws, "claude-code", b"fix the login bug", WINDOW), None);
    }

    #[test]
    fn prune_removes_old_sessions() {
        let repo = repo();
        let ws = ws_for(repo.path());
        capture(&ws, "claude-code", &payload("/rf:ask a", "old"), None).unwrap();
        capture(&ws, "claude-code", &payload("/rf:ask b", "new"), None).unwrap();
        let old = ws.rf_dir().join("sessions/claude-code/old");
        let ten_days_ago = format!("{}", (std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap().as_secs()) - 10 * 86_400);
        for path in [old.join("prompts.jsonl"), old.clone()] {
            crate::testing::run(&["touch", "-t",
                &epoch_to_touch(&ten_days_ago), &path.to_string_lossy()]);
        }
        assert_eq!(prune(&ws, "7d").unwrap(), ["claude-code/old"]);
        assert!(!old.exists());
        assert_eq!(parse_duration("36h").unwrap(), Duration::from_secs(36 * 3600));
        assert!(parse_duration("7").is_err());
        assert!(parse_duration("").is_err());
    }

    /// `touch -t` wants `[[CC]YY]MMDDhhmm[.ss]`.
    fn epoch_to_touch(secs: &str) -> String {
        let secs: i64 = secs.parse().unwrap();
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        let rem = secs.rem_euclid(86_400);
        format!("{y:04}{m:02}{d:02}{:02}{:02}.{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
    }

    #[test]
    fn the_codex_dollar_prefix_is_an_invocation_too() {
        let repo = repo();
        let ws = ws_for(repo.path());
        let rec = capture(
            &ws,
            "codex",
            &payload("$rf:ask fix the login bug", "c1"),
            Some("codex-cli 0.153.4"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(rec["prompt"], "$rf:ask fix the login bug");
        assert_eq!(
            source_verified(&ws, "codex", Some("c1"), b"fix the login bug\n"),
            ("exact".into(), None)
        );
        assert_eq!(
            resolve_session(&ws, "codex", b"fix the login bug", WINDOW),
            Some(json!({"session_ref": "c1", "host_version": "codex-cli 0.153.4"}))
        );
        let plain = capture(&ws, "codex", &payload("$rfx not ours", "c1"), None).unwrap().unwrap();
        assert!(plain.get("prompt").is_none());
    }

    #[test]
    fn a_recorded_time_reads_back_as_the_instant_it_named() {
        assert_eq!(parse_time("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_time("2026-01-01T00:00:00.007Z"), Some(1_767_225_600_007));
        assert_eq!(parse_time("2024-02-29T00:00:00.999Z"), Some(1_709_164_800_999));
        // Offsets and a missing fraction, which a grant file may carry.
        assert_eq!(parse_time("2026-01-01T00:00:00+00:00"), Some(1_767_225_600_000));
        assert_eq!(parse_time("2026-01-01T01:00:00+01:00"), Some(1_767_225_600_000));
        assert_eq!(parse_time("2025-12-31T23:00:00-01:00"), Some(1_767_225_600_000));
        assert_eq!(parse_time("nonsense"), None);
        assert_eq!(parse_time(""), None);
        // Every timestamp this record writes round-trips.
        let n = now();
        assert_eq!(parse_time(&n).map(|m| m / 1000), Some(now_millis() / 1000));
    }

    #[test]
    fn the_clock_renders_the_way_the_record_has_always_carried_it() {
        assert_eq!(format_utc(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_utc(1_767_225_600, 7), "2026-01-01T00:00:00.007Z");
        assert_eq!(format_utc(1_709_164_800, 999), "2024-02-29T00:00:00.999Z");
        let n = now();
        assert_eq!(n.len(), 24, "{n}");
        assert!(n.ends_with('Z') && n.contains('T'), "{n}");
    }
}
