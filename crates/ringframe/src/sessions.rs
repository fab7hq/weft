//! Bounded, prunable observed prompts under `.fab7/rf/sessions/<host>/<session>/`.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::digest;
use crate::store::canonical;
use crate::workspace::Workspace;

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
    if !ws.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            crate::workspace::uninitialized(ws),
        ));
    }
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

/// Store a prompt someone observed (`ringframe observe prompt`) as its digest
/// and byte count only, never its text. `by` is whoever observed it.
pub fn observe_prompt(
    ws: &Workspace,
    host: &str,
    session: &str,
    prompt: &str,
    host_version: Option<&str>,
    by: &str,
) -> std::io::Result<Option<Value>> {
    if prompt.is_empty() || session.is_empty() {
        return Ok(None);
    }
    let data = prompt.as_bytes();
    let version = host_version.map(str::trim).filter(|v| !v.is_empty());
    let rec = json!({
        "session_id": session,
        "sha256": digest::sha256_bytes(data),
        "bytes": data.len(),
        "host_version": version.map_or(Value::Null, |v| Value::String(v.to_string())),
        "observed_by": by,
    });
    log(ws, host, session, "prompts.jsonl", &rec)?;
    Ok(Some(rec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;
    use crate::testing::{repo, ws_for};

    fn capture(
        ws: &Workspace,
        host: &str,
        prompt: &str,
        session: &str,
        version: Option<&str>,
    ) -> std::io::Result<Option<Value>> {
        observe_prompt(ws, host, session, prompt, version, "weft")
    }

    fn prompts(ws: &Workspace, host: &str, session: &str) -> String {
        std::fs::read_to_string(
            ws.rf_dir().join("sessions").join(host).join(session).join("prompts.jsonl"),
        )
        .unwrap()
    }

    #[test]
    fn a_prompt_is_kept_as_its_digest_size_and_host_version_never_its_text() {
        // Profiles are read from the config home, which is per process.
        crate::testing::with_config_home(|_| {
            let repo = repo();
            let ws = ws_for(repo.path());
            for (n, prompt) in ["hello there", "/rf:ask fix the login bug"].into_iter().enumerate()
            {
                let rec =
                    capture(&ws, "claude-code", prompt, "s1", Some("2.1.263 (Claude Code)\n"))
                        .unwrap()
                        .unwrap();
                assert!(rec["sha256"].is_string());
                assert_eq!(rec["bytes"], prompt.len());
                assert_eq!(rec["host_version"], "2.1.263 (Claude Code)");
                assert!(rec.get("prompt").is_none(), "the text must never be stored");
                let kept = prompts(&ws, "claude-code", "s1");
                assert_eq!(kept.lines().count(), n + 1);
                assert!(!kept.contains(prompt), "{kept}");
            }
            assert!(store::events(&ws).unwrap().is_empty(), "a prompt records no Ask");
        });
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
