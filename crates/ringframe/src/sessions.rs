//! Bounded, prunable observed prompts under `.fab7/rf/sessions/<host>/<session>/`.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
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

/// Store a prompt someone observed (`ringframe observe prompt`): its text,
/// digest and byte count, in the local ledger only. `by` is whoever observed it.
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
        "text": prompt,
        "host_version": version.map_or(Value::Null, |v| Value::String(v.to_string())),
        "observed_by": by,
    });
    log(ws, host, session, "prompts.jsonl", &rec)?;
    Ok(Some(rec))
}

/// One observed prompt. `text` is None for a row from before prompts kept
/// their text: such a prompt is counted, never read.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub host: String,
    pub session: String,
    pub time: String,
    pub sha256: String,
    pub text: Option<String>,
}

/// Every session's `prompts.jsonl`, with its host and session, in order.
fn prompt_files(ws: &Workspace) -> Vec<(String, String, PathBuf)> {
    let mut paths = Vec::new();
    let base = ws.rf_dir().join("sessions");
    for host in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        for session in std::fs::read_dir(host.path()).into_iter().flatten().flatten() {
            let p = session.path().join("prompts.jsonl");
            if p.is_file() {
                let name = |e: &std::fs::DirEntry| e.file_name().to_string_lossy().to_string();
                paths.push((name(&host), name(&session), p));
            }
        }
    }
    paths.sort();
    paths
}

/// One row of a `prompts.jsonl`, if it reads.
fn prompt_of(host: &str, session: &str, line: &str) -> Option<Prompt> {
    let rec = serde_json::from_str::<Value>(line).ok()?;
    let field = |k: &str| rec[k].as_str().unwrap_or_default().to_string();
    Some(Prompt {
        host: host.to_string(),
        session: session.to_string(),
        time: field("time"),
        sha256: field("sha256"),
        text: rec["text"].as_str().map(str::to_string),
    })
}

/// Every prompt observed in this workspace, session by session, each
/// session's in the order they were observed.
pub fn read_prompts(ws: &Workspace) -> Vec<Prompt> {
    let mut out = Vec::new();
    for (host, session, path) in prompt_files(ws) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        out.extend(text.lines().filter_map(|l| prompt_of(&host, &session, l)));
    }
    out
}

/// Each session's first prompt that says something, titled as an Ask is: its
/// first line, cut to 72 characters, without a leading command such as
/// `/goal`. A prompt that is only a command, or kept without its text, names
/// nothing.
///
/// Kept for the sessions asked about: each one's file is read once, up to that
/// prompt, and again only when its time or size changed.
#[derive(Debug, Default)]
pub struct FirstPrompts {
    read: HashMap<(String, String), (SystemTime, u64, Option<String>)>,
}

impl FirstPrompts {
    /// The first prompts of these sessions, each named by its host and id,
    /// by session id. Only their own files are looked at; a session no
    /// longer asked about is forgotten.
    pub fn of(
        &mut self,
        ws: &Workspace,
        sessions: &[(String, String)],
    ) -> BTreeMap<String, String> {
        let base = ws.rf_dir().join("sessions");
        let mut kept = HashMap::with_capacity(sessions.len());
        let mut out = BTreeMap::new();
        for (host, session) in sessions {
            if !segment(host) || !segment(session) {
                continue;
            }
            let path = base.join(host).join(session).join("prompts.jsonl");
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let stamp = (meta.modified().unwrap_or(UNIX_EPOCH), meta.len());
            let key = (host.clone(), session.clone());
            let title = match self.read.remove(&key) {
                Some((at, len, title)) if (at, len) == stamp => title,
                _ => first_prompt(host, session, &path),
            };
            if let Some(title) = &title {
                out.insert(session.clone(), title.clone());
            }
            kept.insert(key, (stamp.0, stamp.1, title));
        }
        self.read = kept;
        out
    }
}

/// A name that is one folder of the path: never `..`, never a separator.
fn segment(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// One `prompts.jsonl`'s first prompt with words, read only up to it.
fn first_prompt(host: &str, session: &str, path: &Path) -> Option<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let words = std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| prompt_of(host, session, &l)?.text)
        .find_map(|t| words_of(&t))?;
    Some(crate::ask::requested_title(words.as_bytes()))
}

/// A prompt's words: without a leading `/command`; none when nothing is left.
fn words_of(text: &str) -> Option<String> {
    let text = text.trim();
    let words = match text.strip_prefix('/') {
        Some(command) => command.split_once(char::is_whitespace).map_or("", |(_, rest)| rest),
        None => text,
    };
    let words = words.trim();
    (!words.is_empty()).then(|| words.to_string())
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
    fn a_prompt_is_kept_with_its_text_digest_size_and_host_version() {
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
                assert_eq!(rec["text"], prompt);
                assert_eq!(rec["host_version"], "2.1.263 (Claude Code)");
                assert_eq!(prompts(&ws, "claude-code", "s1").lines().count(), n + 1);
            }
            let read = read_prompts(&ws);
            assert_eq!(read.len(), 2);
            assert_eq!(read[0].text.as_deref(), Some("hello there"));
            assert_eq!((read[0].host.as_str(), read[0].session.as_str()), ("claude-code", "s1"));
            assert_eq!(read[0].sha256, digest::sha256_bytes(b"hello there"));
            assert!(store::events(&ws).unwrap().is_empty(), "a prompt records no Ask");
        });
    }

    #[test]
    fn an_older_row_without_text_reads_as_counted_only() {
        crate::testing::with_config_home(|_| {
            let repo = repo();
            let ws = ws_for(repo.path());
            let old =
                json!({"session_id": "s1", "sha256": "ab", "bytes": 2, "observed_by": "weft"});
            log(&ws, "codex", "s1", "prompts.jsonl", &old).unwrap();
            let read = read_prompts(&ws);
            assert_eq!(read.len(), 1);
            assert_eq!(read[0].text, None);
            assert_eq!(read[0].sha256, "ab");
        });
    }

    #[test]
    fn each_sessions_first_prompt_with_words_names_it() {
        crate::testing::with_config_home(|_| {
            let repo = repo();
            let ws = ws_for(repo.path());
            capture(&ws, "claude-code", "/clear", "s1", None).unwrap();
            capture(&ws, "claude-code", "fix the login redirect\nand its test", "s1", None)
                .unwrap();
            capture(&ws, "claude-code", "then the docs", "s1", None).unwrap();
            capture(&ws, "codex", "/goal why does CI fail on main", "s2", None).unwrap();
            capture(&ws, "grok", &"word ".repeat(30), "s4", None).unwrap();
            let old =
                json!({"session_id": "s3", "sha256": "ab", "bytes": 2, "observed_by": "weft"});
            log(&ws, "agy", "s3", "prompts.jsonl", &old).unwrap();
            let listed = [("claude-code", "s1"), ("codex", "s2"), ("agy", "s3"), ("grok", "s4")]
                .map(|(h, s)| (h.to_string(), s.to_string()));
            let firsts = FirstPrompts::default().of(&ws, &listed);
            assert_eq!(firsts["s1"], "fix the login redirect", "a command alone is skipped");
            assert_eq!(firsts["s2"], "why does CI fail on main", "its command dropped");
            assert!(!firsts.contains_key("s3"), "a prompt kept without its text names nothing");
            assert_eq!(firsts["s4"].chars().count(), 72, "cut as an Ask's title is");
            assert!(firsts["s4"].ends_with('…'));
        });
    }

    #[test]
    fn a_first_prompt_is_read_once_and_again_only_when_its_file_changes() {
        crate::testing::with_config_home(|_| {
            let repo = repo();
            let ws = ws_for(repo.path());
            capture(&ws, "claude-code", "fix the login redirect", "s1", None).unwrap();
            capture(&ws, "codex", "not asked about", "s2", None).unwrap();
            let path = ws.rf_dir().join("sessions/claude-code/s1/prompts.jsonl");
            let listed = vec![("claude-code".to_string(), "s1".to_string())];
            let mut firsts = FirstPrompts::default();
            let read = firsts.of(&ws, &listed);
            assert_eq!(read.len(), 1, "only the sessions asked about: {read:?}");
            assert_eq!(read["s1"], "fix the login redirect");
            // Other words in a file of the same time and size: it is not read again.
            let was = std::fs::metadata(&path).unwrap().modified().unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, text.replace("login", "LOGIN")).unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(was).unwrap();
            assert_eq!(firsts.of(&ws, &listed)["s1"], "fix the login redirect");
            // Its size changed: read again.
            std::fs::write(&path, text.replace("login", "sign-in")).unwrap();
            assert_eq!(firsts.of(&ws, &listed)["s1"], "fix the sign-in redirect");
            // Gone, it names nothing.
            std::fs::remove_file(&path).unwrap();
            assert!(firsts.of(&ws, &listed).is_empty());
            // A name that is not one folder names no file.
            let outside = vec![("..".to_string(), "s1".to_string())];
            assert!(firsts.of(&ws, &outside).is_empty());
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
