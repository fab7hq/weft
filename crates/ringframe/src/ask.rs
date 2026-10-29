//! Ask: what is on record of each compiled intent, and the graded
//! observations appended to it (submission, delivery).

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::store::LedgerError;
use crate::workspace::Workspace;
use crate::{config, digest, ids, profiles, schema, sessions, store, workspace};

/// How far back a `PostToolUse` hook may reach for the Ask it belongs to.
const DELIVERY_WINDOW_MS: i64 = 30 * 60 * 1000;

/// The Ask cannot go on without a person: an authorization is missing, and
/// only a person can grant one.
#[derive(Debug)]
pub struct NeedsInput {
    pub reason: String,
}

impl std::fmt::Display for NeedsInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for NeedsInput {}

/// Everything an Ask command can fail with.
#[derive(Debug)]
pub enum AskError {
    Ledger(LedgerError),
    Needs(NeedsInput),
    Workspace(workspace::WorkspaceError),
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AskError::Ledger(e) => write!(f, "{e}"),
            AskError::Needs(e) => write!(f, "{e}"),
            AskError::Workspace(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AskError {}

impl From<LedgerError> for AskError {
    fn from(e: LedgerError) -> Self {
        AskError::Ledger(e)
    }
}

impl From<NeedsInput> for AskError {
    fn from(e: NeedsInput) -> Self {
        AskError::Needs(e)
    }
}

impl From<workspace::WorkspaceError> for AskError {
    fn from(e: workspace::WorkspaceError) -> Self {
        AskError::Workspace(e)
    }
}

impl From<std::io::Error> for AskError {
    fn from(e: std::io::Error) -> Self {
        AskError::Ledger(LedgerError::new_public("ledger.io", e.to_string()))
    }
}

impl From<config::ConfigError> for AskError {
    fn from(e: config::ConfigError) -> Self {
        AskError::Ledger(LedgerError::new_public("config.error", e.0))
    }
}

pub(crate) fn ledger(code: &str, detail: impl Into<String>) -> AskError {
    AskError::Ledger(LedgerError::new_public(code, detail))
}

pub(crate) fn event(type_: &str, id: &str, actor: &Value, data: Value, links: &[Value]) -> Value {
    json!({
        "schema": store::SCHEMA, "event_id": ids::new_id("evt"), "type": type_,
        "time": sessions::now(), "id": id, "actor": actor, "links": links, "data": data
    })
}

fn default_actor(actor: Option<&Value>) -> Value {
    actor
        .cloned()
        .unwrap_or_else(|| json!({"kind": "human", "id": "local-user", "authority": "interactive"}))
}

pub(crate) fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

pub(crate) fn list_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// How this prompt has to reach the host, recorded rather than re-derived.
///
/// `prompt.txt` stays what it has always been — the whole thing, prefix and
/// all, byte for byte what a person submits. What is added here is the mode
/// that prefix names, and where the body starts, so a client can send the two
/// parts separately without cutting the prompt itself or guessing at a length.
///
/// It matters because both hosts fold a long paste into a placeholder and do
/// not scan a folded paste for slash commands: pasted whole, `/plan ...` runs
/// as an ordinary request. `paste_fold_chars` is the measured size at which
/// that starts. A `mode` prefix can be sent on its own first and confirmed; an
/// `inline` one consumes its argument and cannot.
pub(crate) fn delivery_block(profile: &Value, cap: &Value, prompt: &[u8]) -> Value {
    let prefix = cap.get("prompt_prefix").and_then(Value::as_str).filter(|p| !p.is_empty());
    let prefix_bytes = prefix.map_or(0, str::len);
    let fold = profile.get("paste_fold_chars").and_then(Value::as_u64);
    json!({
        "mode": prefix.map(str::trim).filter(|m| !m.is_empty()),
        "mode_kind": prefix.and_then(|_| cap.get("prompt_prefix_kind").cloned()).unwrap_or(Value::Null),
        "mode_active": prefix.and_then(|_| cap.get("prompt_prefix_active").cloned()).unwrap_or(Value::Null),
        "prefix_bytes": prefix_bytes,
        "body_bytes": prompt.len() - prefix_bytes,
        // The size at which this host stops reading a paste as text. Null when
        // the profile does not declare one; a client must then assume nothing.
        "paste_fold_chars": fold,
        "folds": fold.is_some_and(|f| prompt.len() as u64 >= f),
    })
}

/// Refuse an Ask this workspace could never finish, before any work is done.
///
/// The same refusal `compile` makes, available before the person writes an
/// Ask they could not send.
pub fn preflight(ws: &Workspace) -> Result<Value, AskError> {
    workspace::require_git(ws)?;
    workspace::require_initialized(ws)?;
    Ok(json!({"ready": true, "workspace": ws.root.to_string_lossy()}))
}

/// Every link names something this workspace has, of the kind the link means:
/// a revision names an Ask, a follow-up an Ask or an Eval, a remedy a
/// completed Eval. A link to nothing would read as history that never happened.
pub(crate) fn check_links(ws: &Workspace, links: &[Value]) -> Result<(), AskError> {
    let events = store::events(ws)?;
    for l in links {
        let (rel, id) = (str_of(l, "rel"), str_of(l, "id"));
        let (kind, types): (&str, &[&str]) = match rel.as_str() {
            "revises" => ("an Ask", &["ask.compiled"]),
            "follows" => ("an Ask or an Eval", &["ask.compiled", "eval.opened"]),
            "remediates" => ("a completed Eval", &["eval.completed"]),
            _ => {
                return Err(ledger(
                    "ask.link_rel_unknown",
                    format!(
                        "'{rel}' is not a link an Ask carries; use follows, remediates or revises"
                    ),
                ));
            }
        };
        let have: Vec<String> = events
            .iter()
            .filter(|e| types.contains(&str_of(e, "type").as_str()))
            .map(|e| str_of(e, "id"))
            .collect();
        // A voided Eval is not something to follow up or remedy: it is what
        // the person said not to build on.
        if rel != "revises"
            && events.iter().any(|e| e["type"] == "eval.voided" && str_of(e, "id") == id)
        {
            return Err(ledger(
                "ask.link_voided",
                format!("{rel}:{id} names an Eval that was voided; name another, or none"),
            ));
        }
        if !have.contains(&id) {
            let listed = if have.is_empty() { "none".to_string() } else { have.join(", ") };
            return Err(ledger(
                "ask.link_unknown",
                format!("{rel}:{id} is not {kind} in this workspace; it has: {listed}"),
            ));
        }
    }
    Ok(())
}

/// What a request is titled until its compile names it: the first line the
/// person wrote, without the `--override '<json>'` or `[follow-up <id>]` a
/// tool that runs RingFrame may put in front of it, cut to 72 characters.
pub fn requested_title(source: &[u8]) -> String {
    let text = String::from_utf8_lossy(source);
    let mut rest = text.trim_start();
    if let Some(after) = rest.strip_prefix("--override '")
        && let Some(end) = after.find('\'')
    {
        rest = after[end + 1..].trim_start();
    }
    if rest.starts_with("[follow-up ")
        && let Some(end) = rest.find(']')
    {
        rest = rest[end + 1..].trim_start();
    }
    let line = rest.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > 72 {
        line.chars().take(71).chain(['…']).collect()
    } else {
        line.to_string()
    }
}

/// The workspace commit an Ask starts from; Eval's anchor when no Seal
/// precedes it. Null outside Git.
/// The commit the work starts from, none in a repository without one. Read
/// with the Eval's Git helper, which tries a failed read once more: a read
/// that failed for a moment left an Ask with no base, and its Eval with no
/// anchor (q07, Antigravity confirm a3).
pub(crate) fn head(ws: &Workspace) -> Value {
    crate::evaluate::git(&ws.root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map_or(Value::Null, Value::String)
}

fn append_event(
    ws: &Workspace,
    type_: &str,
    ask_id: &str,
    data: Value,
    actor: Option<&Value>,
) -> Result<Value, AskError> {
    let ev = event(type_, ask_id, &default_actor(actor), data.clone(), &[]);
    schema::validate_event(&ev)?;
    store::append(ws, &ev)?;
    let mut out = json!({"ask_id": ask_id});
    for (k, v) in data.as_object().into_iter().flatten() {
        out[k] = v.clone();
    }
    Ok(out)
}

/// A later user prompt whose bytes equal one compiled `prompt.txt`:
/// host-observed submission.
///
/// Three byte forms count as the same prompt, each recorded by name: the file
/// itself (`exact`), the file without its trailing newline
/// (`trailing_newline_dropped`: composers drop it on paste), and the file
/// without the capability's slash-command prefix (`host_prefix_stripped`:
/// Codex hands its hooks the text after `/plan `). When several Asks match,
/// only those already handed off are candidates; a remaining tie records
/// nothing rather than guessing. Once per Ask.
pub fn submission_from_capture(
    ws: &Workspace,
    host: &str,
    session: &str,
    sha256: &str,
    observed_by: &str,
) -> Result<Option<Value>, AskError> {
    submission_as(ws, host, session, sha256, observed_by, None)
}

/// As `submission_from_capture`, for a prompt the caller took out of the
/// host's own wrapping, recorded under that `allowance` as its `match`.
pub fn submission_as(
    ws: &Workspace,
    host: &str,
    session: &str,
    sha256: &str,
    observed_by: &str,
    allowance: Option<&str>,
) -> Result<Option<Value>, AskError> {
    let mut hits: Vec<(String, String, bool)> = Vec::new();
    for (ask_id, v) in by_id(ws)? {
        let Some(compiled) = &v.compiled else { continue };
        if v.submissions.iter().any(|s| s["data"]["state"] == "observed") {
            continue;
        }
        if let Some(m) = prompt_match(ws, &compiled["data"], sha256, host)? {
            hits.push((ask_id, m, v.delivery.is_some()));
        }
    }
    if hits.len() > 1 {
        hits.retain(|h| h.2);
    }
    if hits.len() != 1 {
        return Ok(None);
    }
    let (ask_id, matched, _) = hits.remove(0);
    let matched = allowance.map_or(matched, str::to_string);
    Ok(Some(append_event(
        ws,
        "ask.submission",
        &ask_id,
        json!({
            "state": "observed", "observed_by": observed_by, "attributed_by": null,
            "as_modified": false, "host": {"name": host, "session_ref": session},
            "prompt_sha256": sha256, "match": matched,
        }),
        None,
    )?))
}

fn prompt_match(
    ws: &Workspace,
    compiled: &Value,
    sha256: &str,
    host: &str,
) -> Result<Option<String>, AskError> {
    let reference = &compiled["prompt"];
    if str_of(reference, "sha256") == sha256 {
        return Ok(Some("exact".into()));
    }
    let stored = std::fs::read(ws.rf_dir().join(str_of(reference, "path")))?;
    let data = [unstored_prefix(compiled).as_bytes(), &stored].concat();
    if digest::sha256_bytes(&data) == sha256 {
        return Ok(Some("exact".into()));
    }
    let trimmed: &[u8] = {
        let mut end = data.len();
        while end > 0 && data[end - 1] == b'\n' {
            end -= 1;
        }
        &data[..end]
    };
    if digest::sha256_bytes(trimmed) == sha256 {
        return Ok(Some("trailing_newline_dropped".into()));
    }
    let prefix = profiles::for_host(&compiled["host"])
        .ok()
        .and_then(|p| profiles::capability(&p, &str_of(compiled, "selected_capability")).cloned())
        .map(|c| str_of(&c, "prompt_prefix"))
        .unwrap_or_default();
    if !prefix.is_empty() && data.starts_with(prefix.as_bytes()) {
        let stripped = &data[prefix.len()..];
        let mut end = stripped.len();
        while end > 0 && stripped[end - 1] == b'\n' {
            end -= 1;
        }
        if digest::sha256_bytes(stripped) == sha256
            || digest::sha256_bytes(&stripped[..end]) == sha256
        {
            return Ok(Some("host_prefix_stripped".into()));
        }
    }
    // Sent into another host, the prompt went in that host's own way for the
    // same capability: its command in front of the same body.
    if !host.is_empty() && host != str_of(&compiled["host"], "name") {
        let body = data.strip_prefix(prefix.as_bytes()).unwrap_or(&data);
        let theirs =
            [host_prefix(host, &str_of(compiled, "selected_capability")).as_bytes(), body].concat();
        let mut end = theirs.len();
        while end > 0 && theirs[end - 1] == b'\n' {
            end -= 1;
        }
        if digest::sha256_bytes(&theirs) == sha256 || digest::sha256_bytes(&theirs[..end]) == sha256
        {
            return Ok(Some("receiving_host_prefix".into()));
        }
    }
    Ok(None)
}

/// The command a host puts in front of a prompt for this capability: its
/// profile's prefix, or none when its profile has no such capability or no
/// prefix for it.
fn host_prefix(host: &str, capability: &str) -> String {
    profiles::of(host)
        .ok()
        .and_then(|p| profiles::capability(&p, capability).cloned())
        .map(|c| str_of(&c, "prompt_prefix"))
        .unwrap_or_default()
}

/// The prompt as another host takes it, and how it has to reach that host.
///
/// The wording is the compiled prompt's, never changed: only the command in
/// front of it is that host's for the same capability, or none when it has
/// none. For the host it was compiled for, it is `prompt.txt` as it is.
pub fn prompt_for_host(ws: &Workspace, ask_id: &str, host: &str) -> Result<Value, AskError> {
    let rec = record(ws, ask_id)?;
    let d = &rec.compiled.expect("compiled")["data"];
    if host == str_of(&d["host"], "name") {
        return Ok(json!({
            "ask_id": ask_id, "host": host,
            "prompt": prompt_text(ws, ask_id)?,
            "delivery": d.get("delivery").cloned().unwrap_or(Value::Null),
        }));
    }
    let body = prompt_body(ws, ask_id)?;
    let capability = str_of(d, "selected_capability");
    let profile = profiles::of(host).map_err(|e| ledger("ask.host", e.0))?;
    let cap = profiles::capability(&profile, &capability).cloned().unwrap_or_else(|| json!({}));
    let prompt = format!("{}{body}", str_of(&cap, "prompt_prefix"));
    Ok(json!({
        "ask_id": ask_id, "host": host,
        "delivery": delivery_block(&profile, &cap, prompt.as_bytes()),
        "prompt": prompt,
    }))
}

/// The prompt without the command its prefix names.
///
/// `prompt.txt` stays what it has always been. This is the part a person
/// pastes after typing the command, taken at the byte offset the record
/// already carries rather than by looking for the prefix again.
pub fn prompt_body(ws: &Workspace, ask_id: &str) -> Result<String, AskError> {
    let rec = record(ws, ask_id)?;
    let d = &rec.compiled.expect("compiled")["data"];
    let text = std::fs::read_to_string(ws.rf_dir().join(str_of(&d["prompt"], "path")))?;
    if d["compiler"]["source"] == "verbatim" {
        return Ok(text);
    }
    // A record from before `delivery` existed says nothing about a prefix, so
    // fall back to what the capability declares, and to the whole prompt.
    let prefix = d
        .get("delivery")
        .and_then(|x| x.get("prefix_bytes"))
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or_else(|| {
            profiles::for_host(&d["host"])
                .ok()
                .and_then(|p| profiles::capability(&p, &str_of(d, "selected_capability")).cloned())
                .map(|c| str_of(&c, "prompt_prefix").len())
                .unwrap_or(0)
        });
    Ok(text.get(prefix..).unwrap_or(&text).to_string())
}

/// The prompt as it is typed: `prompt.txt`, with the capability's command
/// in front when the compile stored the words without it.
pub fn prompt_text(ws: &Workspace, ask_id: &str) -> Result<String, AskError> {
    let rec = record(ws, ask_id)?;
    let d = &rec.compiled.expect("compiled")["data"];
    let stored = std::fs::read_to_string(ws.rf_dir().join(str_of(&d["prompt"], "path")))?;
    Ok(format!("{}{stored}", unstored_prefix(d)))
}

/// The command a verbatim compile left out of `prompt.txt`; none for one
/// that stored it.
fn unstored_prefix(d: &Value) -> String {
    if d["compiler"]["source"] != "verbatim" {
        return String::new();
    }
    host_prefix(&str_of(&d["host"], "name"), &str_of(d, "selected_capability"))
}

/// Everything the ledger says about one Ask.
#[derive(Default, Clone)]
pub struct AskRecord {
    pub compiled: Option<Value>,
    pub confirmed: Option<Value>,
    pub cancelled: Option<Value>,
    pub unanswered: Option<Value>,
    pub delivery: Option<Value>,
    pub submissions: Vec<Value>,
}

/// Every Ask in the ledger, in the order they were first written.
fn by_id(ws: &Workspace) -> Result<Vec<(String, AskRecord)>, AskError> {
    let mut out: Vec<(String, AskRecord)> = Vec::new();
    for ev in store::events(ws)? {
        let type_ = str_of(&ev, "type");
        let Some(kind) = type_.strip_prefix("ask.") else { continue };
        let id = str_of(&ev, "id");
        if !out.iter().any(|(k, _)| *k == id) {
            out.push((id.clone(), AskRecord::default()));
        }
        let rec = &mut out.iter_mut().find(|(k, _)| *k == id).expect("just inserted").1;
        match kind {
            "submission" => rec.submissions.push(ev),
            "compiled" => rec.compiled = Some(ev),
            "confirmed" => rec.confirmed = Some(ev),
            "cancelled" => rec.cancelled = Some(ev),
            "unanswered" => rec.unanswered = Some(ev),
            "delivery" => rec.delivery = Some(ev),
            _ => {}
        }
    }
    Ok(out)
}

fn record(ws: &Workspace, ask_id: &str) -> Result<AskRecord, AskError> {
    by_id(ws)?
        .into_iter()
        .find(|(k, v)| k == ask_id && v.compiled.is_some())
        .map(|(_, v)| v)
        .ok_or_else(|| ledger("ask.not_compiled", ask_id))
}

/// One delivery, as the record carries it. They arrive together and are
/// written together, so they are one thing rather than nine.
struct Delivery<'a> {
    mode: &'a str,
    mechanism: Value,
    state: &'a str,
    receipt: Value,
    limitations: Vec<String>,
}

fn append_delivery(
    ws: &Workspace,
    ask_id: &str,
    compiled: &Value,
    delivery: Delivery<'_>,
    actor: Option<&Value>,
) -> Result<Value, AskError> {
    let Delivery { mode, mechanism, state, receipt, limitations } = delivery;
    if by_id(ws)?.iter().any(|(k, v)| k == ask_id && v.delivery.is_some()) {
        return Err(ledger("delivery.duplicate", ask_id));
    }
    let d = &compiled["data"];
    let cap = profiles::for_host(&d["host"])
        .ok()
        .and_then(|p| profiles::capability(&p, &str_of(d, "selected_capability")).cloned())
        .unwrap_or_else(|| json!({}));
    let mut all = vec![
        "native acceptance does not prove instruction following, execution, or completion"
            .to_string(),
    ];
    all.extend(limitations);
    // A profile leaves out an id it does not have; the record always names one.
    let mut qualification = cap.get("qualification").cloned().unwrap_or_else(|| json!({}));
    if qualification.get("id").is_none() {
        qualification["id"] = Value::Null;
    }
    let data = json!({
        "mode": mode, "mechanism": mechanism, "state": state,
        "qualification": qualification,
        "receipt": receipt,
        "submission": "not_applicable",
        "limitations": all,
    });
    let ev = event("ask.delivery", ask_id, &default_actor(actor), data.clone(), &[]);
    schema::validate_event(&ev)?;
    store::append(ws, &ev)?;
    let mut out = json!({"ask_id": ask_id});
    for (k, v) in data.as_object().into_iter().flatten() {
        out[k] = v.clone();
    }
    Ok(out)
}

/// Record `native_accepted` (or `delivery_failed`) when the host's delivery
/// tool was seen to run in a session (`ringframe observe tool`); never raise.
///
/// The Ask it delivers is the one confirmed in that session, routed to
/// native dispatch through this tool, not delivered yet, and compiled within
/// the window. Anything else records nothing, and says why beside the
/// session's prompts. The receipt names who saw the tool run (`by`).
pub fn delivery_observed(
    ws: &Workspace,
    host: &str,
    session: &str,
    tool: &str,
    failed: bool,
    by: &str,
) -> Result<Option<Value>, AskError> {
    if session.is_empty() {
        return Ok(None);
    }
    let cutoff = sessions::now_millis() - DELIVERY_WINDOW_MS;
    let mut candidates: Vec<(String, Value)> = Vec::new();
    for (ask_id, rec) in by_id(ws)? {
        let Some(c) = rec.compiled.clone() else { continue };
        let d = &c["data"];
        if rec.delivery.is_some()
            || rec.cancelled.is_some()
            || str_of(d, "delivery_mode") != "native_dispatch"
            || str_of(&d["host"], "session_ref") != session
        {
            continue;
        }
        let activation_tool = profiles::for_host(&d["host"])
            .ok()
            .and_then(|p| profiles::capability(&p, &str_of(d, "selected_capability")).cloned())
            .map(|cap| str_of(&cap.get("activation").cloned().unwrap_or(Value::Null), "tool"))
            .unwrap_or_default();
        if activation_tool != tool {
            continue;
        }
        if sessions::parse_time(&str_of(&c, "time")).is_some_and(|t| t < cutoff) {
            continue;
        }
        candidates.push((ask_id, c));
    }
    if candidates.len() != 1 {
        let _ = sessions::log(
            ws,
            host,
            session,
            "delivery-skipped.jsonl",
            &json!({
                "reason": if candidates.is_empty() { "no_candidate" } else { "ambiguous" },
                "tool": tool,
                "candidates": candidates.iter().map(|(a, _)| a.clone()).collect::<Vec<_>>(),
            }),
        );
        return Ok(None);
    }
    let (ask_id, compiled) = candidates.remove(0);
    let receipt = json!({"tool": tool, "session_id": session, "captured_by": by});
    Ok(Some(append_delivery(
        ws,
        &ask_id,
        &compiled,
        Delivery {
            mode: "native_dispatch",
            mechanism: json!("capability_activate"),
            state: if failed { "delivery_failed" } else { "native_accepted" },
            receipt,
            limitations: Vec::new(),
        },
        None,
    )?))
}

pub fn submission_grade(rec: &AskRecord) -> &'static str {
    let states: Vec<String> = rec.submissions.iter().map(|s| str_of(&s["data"], "state")).collect();
    if states.iter().any(|s| s == "observed") {
        "observed"
    } else if states.iter().any(|s| s == "attributed") {
        "attributed"
    } else {
        "unobserved"
    }
}

fn sealed_asks(ws: &Workspace) -> Result<BTreeSet<String>, AskError> {
    let mut out = BTreeSet::new();
    for e in store::events(ws)? {
        if str_of(&e, "type") == "seal.created" {
            out.extend(list_of(&e["data"]["basis"], "asks"));
        }
    }
    Ok(out)
}

/// `open` from compile until a Seal names the Ask in its basis; cancelled Asks
/// are never open.
fn state_of(ask_id: &str, rec: &AskRecord, sealed: &BTreeSet<String>) -> &'static str {
    if rec.cancelled.is_some() {
        "cancelled"
    } else if sealed.contains(ask_id) {
        "sealed"
    } else {
        "open"
    }
}

/// Every Ask in `ask_id`'s unit of work, oldest first: the root Ask (one
/// that follows, revises or remediates nothing) and every Ask whose links
/// reach it. A link to an Eval reaches the Asks that Eval covered.
pub fn unit_of(ws: &Workspace, ask_id: &str) -> Result<Vec<String>, AskError> {
    let asks = by_id(ws)?;
    let events = store::events(ws)?;
    let parent = |id: &str| -> Option<String> {
        let (_, rec) = asks.iter().find(|(k, _)| k == id)?;
        let link = rec.compiled.as_ref()?["links"]
            .as_array()?
            .iter()
            .find(|l| matches!(str_of(l, "rel").as_str(), "follows" | "revises" | "remediates"))?;
        let target = str_of(link, "id");
        if target.starts_with("evl_") {
            let opened =
                events.iter().find(|e| e["type"] == "eval.opened" && str_of(e, "id") == target)?;
            list_of(&opened["data"]["basis"], "asks").into_iter().next()
        } else {
            Some(target)
        }
    };
    let root = |id: &str| -> String {
        let mut at = id.to_string();
        let mut seen = vec![at.clone()];
        while let Some(up) = parent(&at).filter(|u| !seen.contains(u)) {
            seen.push(up.clone());
            at = up;
        }
        at
    };
    if !asks.iter().any(|(k, rec)| k == ask_id && rec.compiled.is_some()) {
        return Err(ledger(
            "ask.unknown",
            format!("{ask_id} is not a compiled Ask in this workspace"),
        ));
    }
    let mine = root(ask_id);
    Ok(asks
        .iter()
        .filter(|(k, rec)| rec.compiled.is_some() && root(k) == mine)
        .map(|(k, _)| k.clone())
        .collect())
}

/// Every open Ask, oldest first: the basis of an Eval and of a Seal.
pub fn open_asks(ws: &Workspace) -> Result<Vec<Value>, AskError> {
    let sealed = sealed_asks(ws)?;
    Ok(by_id(ws)?
        .into_iter()
        .filter(|(k, v)| v.compiled.is_some() && state_of(k, v, &sealed) == "open")
        .map(|(k, v)| summary(ws, &k, &v, &sealed))
        .collect())
}

fn summary(ws: &Workspace, ask_id: &str, rec: &AskRecord, sealed: &BTreeSet<String>) -> Value {
    let ev = rec.compiled.as_ref().expect("a summary needs a compiled event");
    let d = &ev["data"];
    let outcome = if rec.cancelled.is_some() {
        "cancelled"
    } else if rec.confirmed.is_some() {
        "confirmed"
    } else {
        "compiled"
    };
    let time_of = |e: &Option<Value>| {
        e.as_ref().map_or(Value::Null, |e| e.get("time").cloned().unwrap_or(Value::Null))
    };
    json!({
        "id": ask_id, "ask_id": ask_id, "title": d["title"], "time": ev["time"],
        "outcome": outcome, "state": state_of(ask_id, rec, sealed),
        "confirmed_at": time_of(&rec.confirmed),
        "base_commit": d.get("base_commit").cloned().unwrap_or(Value::Null),
        "baselines": d.get("baselines").cloned().unwrap_or(Value::Null),
        // Asked and not answered. Still open, still confirmable; said so a
        // client can put it back in front of the person.
        "unanswered_at": time_of(&rec.unanswered),
        "links": ev["links"],
        "capability": d["selected_capability"], "session_ref": d["host"]["session_ref"],
        "source_verified": d["source_verified"], "source": d["source"], "prompt": d["prompt"],
        "prompt_path": ws.rf_dir().join(str_of(&d["prompt"], "path")).to_string_lossy(),
        "delivery": rec.delivery.as_ref().map_or(Value::Null, |e| e["data"]["state"].clone()),
        "submission": submission_grade(rec),
    })
}

/// Every compiled Ask in this workspace, oldest first: what Eval and a person
/// choose from.
pub fn list_asks(ws: &Workspace) -> Result<Vec<Value>, AskError> {
    let sealed = sealed_asks(ws)?;
    Ok(by_id(ws)?
        .into_iter()
        .filter(|(_, v)| v.compiled.is_some())
        .map(|(k, v)| summary(ws, &k, &v, &sealed))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{repo, with_config_home, ws_for};

    fn bench<T>(body: impl FnOnce(&Workspace) -> T) -> T {
        with_config_home(|_| {
            let repo = repo();
            let ws = ws_for(repo.path());
            body(&ws)
        })
    }

    /// An Ask of these words, as Weft compiles and confirms one.
    fn ask(
        ws: &Workspace,
        words: &[u8],
        host: &str,
        capability: &str,
        links: Vec<Value>,
    ) -> Result<Value, AskError> {
        let classification = if capability == "native_plan" {
            json!({"task": ["plan"], "result": "plan", "effects": ["read"]})
        } else {
            json!({"task": ["implement"], "result": "workspace_change", "effects": ["write"]})
        };
        crate::compile::compile(
            ws,
            crate::compile::Request {
                source: words,
                host: json!({"name": host, "session_ref": "s1"}),
                capability,
                classification,
                plan: None,
                practices: Vec::new(),
                classified_by: json!({"by": "person"}),
                selected_by: json!({"by": "person"}),
                links,
                overrides: &config::Overrides::default(),
                via: "weft",
            },
        )
    }

    fn plan(ws: &Workspace, words: &[u8]) -> Value {
        ask(ws, words, "claude-code", "native_plan", Vec::new()).unwrap()
    }

    fn codex_plan(ws: &Workspace, words: &[u8]) -> Value {
        ask(ws, words, "codex", "native_plan", Vec::new()).unwrap()
    }

    fn link(rel: &str, id: &str) -> Vec<Value> {
        vec![json!({"rel": rel, "id": id})]
    }

    fn ledger_error(e: AskError) -> LedgerError {
        match e {
            AskError::Ledger(e) => e,
            other => panic!("wanted a ledger error, got {other}"),
        }
    }

    fn events(ws: &Workspace) -> Vec<Value> {
        store::events(ws).unwrap()
    }

    fn capture(ws: &Workspace, host: &str, session: &str, prompt: &str) -> String {
        let rec = sessions::observe_prompt(ws, host, session, prompt, None, "weft").unwrap();
        str_of(&rec.unwrap(), "sha256")
    }

    fn submitted(ws: &Workspace, host: &str, session: &str, prompt: &str) -> Option<Value> {
        let sha = capture(ws, host, session, prompt);
        submission_from_capture(ws, host, session, &sha, "hook:UserPromptSubmit").unwrap()
    }

    /// An Eval over the bench's two open Asks, opened and settled.
    fn completed_eval(ws: &Workspace) -> String {
        let o = crate::testing::opened(ws);
        let eval_id = str_of(&o.out, "eval_id");
        crate::testing::settled(ws, &eval_id, false);
        eval_id
    }

    #[test]
    fn a_requests_title_is_what_was_asked_without_what_weft_adds() {
        assert_eq!(requested_title(b"fix the login bug\nand more"), "fix the login bug");
        assert_eq!(
            requested_title(b"--override '{\"a\": 1}' [follow-up evl_1] tidy the tests"),
            "tidy the tests"
        );
        let long = "x".repeat(100);
        assert_eq!(requested_title(long.as_bytes()).chars().count(), 72);
    }

    #[test]
    fn preflight_clears_a_workspace_an_ask_could_finish() {
        bench(|ws| {
            assert_eq!(
                preflight(ws).unwrap(),
                json!({"ready": true, "workspace": ws.root.to_string_lossy()})
            );
        });
    }

    #[test]
    fn preflight_refuses_what_compile_would_refuse() {
        let dir = crate::testing::tmp_dir();
        let plain = workspace::resolve(Some(dir.path()), None).unwrap();
        let AskError::Workspace(e) = preflight(&plain).unwrap_err() else {
            panic!("wanted a workspace error")
        };
        assert_eq!(e.code, "workspace.no_git");

        crate::testing::run(&["git", "init", "-q", &dir.path().to_string_lossy()]);
        let empty = workspace::resolve(Some(dir.path()), None).unwrap();
        let AskError::Workspace(e) = preflight(&empty).unwrap_err() else {
            panic!("wanted a workspace error")
        };
        assert_eq!(e.code, "workspace.no_commit");
        assert!(e.detail.contains("first commit"), "{}", e.detail);
    }

    #[test]
    fn a_link_must_name_something_this_workspace_has() {
        bench(|ws| {
            let first = str_of(&plan(ws, b"Fix the login bug.\n"), "ask_id");
            let open_eval = str_of(
                &crate::evaluate::open_eval(ws, crate::evaluate::Open::default()).unwrap(),
                "eval_id",
            );
            let before = events(ws).len();
            let refused = |links: Vec<Value>, code: &str| {
                let e = ledger_error(
                    ask(ws, b"More.\n", "claude-code", "native_plan", links).unwrap_err(),
                );
                assert_eq!(e.code, code, "{e}");
                e.to_string()
            };
            refused(link("blocks", &first), "ask.link_rel_unknown");
            refused(link("seals", &first), "ask.link_rel_unknown");
            assert!(refused(link("revises", "ask_gone"), "ask.link_unknown").contains("ask_gone"));
            refused(link("follows", "evl_gone"), "ask.link_unknown");
            refused(link("remediates", &first), "ask.link_unknown");
            let unfinished = refused(link("remediates", &open_eval), "ask.link_unknown");
            assert!(unfinished.contains(&open_eval), "{unfinished}");
            assert_eq!(events(ws).len(), before, "a refused link writes nothing");

            for links in
                [link("revises", &first), link("follows", &first), link("follows", &open_eval)]
            {
                ask(ws, b"More.\n", "claude-code", "native_plan", links).unwrap();
            }
        });
    }

    #[test]
    fn a_voided_eval_is_neither_followed_nor_remedied() {
        bench(|ws| {
            let eval_id = completed_eval(ws);
            crate::evaluate::void_eval(ws, &eval_id, "the judges were scripts", None).unwrap();
            let before = events(ws).len();
            for rel in ["remediates", "follows"] {
                let e = ledger_error(
                    ask(ws, b"Fix it.\n", "claude-code", "native_plan", link(rel, &eval_id))
                        .unwrap_err(),
                );
                assert_eq!(e.code, "ask.link_voided", "{e}");
                assert!(e.to_string().contains(&eval_id), "{e}");
            }
            assert_eq!(events(ws).len(), before, "a refused link writes nothing");
        });
    }

    #[test]
    fn a_fix_is_told_to_read_the_eval_it_fixes_first() {
        bench(|ws| {
            let eval_id = completed_eval(ws);
            let read_first = format!(
                "Read .fab7/rf/evals/{eval_id}/eval.md first: it lists what the last Eval found."
            );
            let fix =
                ask(ws, b"Fix it.\n", "claude-code", "native_plan", link("remediates", &eval_id))
                    .unwrap();
            assert!(str_of(&fix, "prompt").contains(&read_first), "{}", fix["prompt"]);
            let follow =
                ask(ws, b"Fix it.\n", "claude-code", "native_plan", link("follows", &eval_id))
                    .unwrap();
            assert!(!str_of(&follow, "prompt").contains(&read_first), "{}", follow["prompt"]);
        });
    }

    #[test]
    fn a_submission_is_the_prompt_as_typed_or_without_its_trailing_newline_once() {
        bench(|ws| {
            let one = plan(ws, b"Fix the login bug.\n");
            let other = plan(ws, b"Something else.\n");
            let typed = prompt_text(ws, &str_of(&one, "ask_id")).unwrap();
            let sub = submitted(ws, "claude-code", "s1", &typed).unwrap();
            assert_eq!(sub["ask_id"], one["ask_id"]);
            assert_eq!(sub["state"], "observed");
            assert_eq!(sub["observed_by"], "hook:UserPromptSubmit");
            assert_eq!(sub["match"], "exact");
            assert!(submitted(ws, "claude-code", "s1", &typed).is_none(), "once only");
            // Composers drop the trailing newline of a pasted file.
            let typed = prompt_text(ws, &str_of(&other, "ask_id")).unwrap();
            let sub = submitted(ws, "claude-code", "s1", typed.trim_end()).unwrap();
            assert_eq!(sub["ask_id"], other["ask_id"]);
            assert_eq!(sub["match"], "trailing_newline_dropped");
            assert!(submitted(ws, "claude-code", "s1", "Not an Ask.").is_none());
            let listed = list_asks(ws).unwrap();
            assert!(listed.iter().all(|a| a["submission"] == "observed"), "{listed:?}");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    #[test]
    fn a_submission_matches_when_the_host_strips_its_slash_command_prefix() {
        bench(|ws| {
            let out = codex_plan(ws, b"Fix the login bug.\n");
            let typed = prompt_text(ws, &str_of(&out, "ask_id")).unwrap();
            let body = typed.strip_prefix("/plan ").expect("codex plans by /plan");
            // Codex hands its hook the text after "/plan ", without the newline.
            let sub = submitted(ws, "codex", "c1", body.trim_end()).unwrap();
            assert_eq!(sub["ask_id"], out["ask_id"]);
            assert_eq!(sub["match"], "host_prefix_stripped");
        });
    }

    #[test]
    fn another_host_takes_the_same_wording_with_its_own_command_or_none() {
        bench(|ws| {
            let out = codex_plan(ws, b"Fix the login bug.\n");
            let id = str_of(&out, "ask_id");
            let own = prompt_for_host(ws, &id, "codex").unwrap();
            let body = prompt_body(ws, &id).unwrap();
            assert_eq!(own["prompt"], format!("/plan {body}"));
            assert_eq!(own["delivery"]["mode"], "/plan");
            // Antigravity plans with its own /plan, a mode it shows.
            let agy = prompt_for_host(ws, &id, "antigravity").unwrap();
            assert_eq!(agy["prompt"], format!("/plan {body}"));
            assert_eq!(agy["delivery"]["prefix_bytes"], 6);
            // Claude Code has no command to plan by: the wording alone.
            let claude = prompt_for_host(ws, &id, "claude-code").unwrap();
            assert_eq!(claude["prompt"], body);
            assert_eq!(claude["delivery"]["mode"], Value::Null);
        });
    }

    #[test]
    fn a_prompt_sent_into_another_host_is_its_submission_there() {
        bench(|ws| {
            // A host that plans by another command.
            let dir = config::config_dir().join("harnesses");
            let codex = std::fs::read_to_string(dir.join("codex.toml")).unwrap();
            let other = codex
                .replace("host = \"codex\"", "host = \"other\"")
                .replace("profile_id = \"codex\"", "profile_id = \"other\"")
                .replace("prompt_prefix = \"/plan \"", "prompt_prefix = \"/think \"");
            std::fs::write(dir.join("other.toml"), other).unwrap();
            let out = codex_plan(ws, b"Fix the login bug.\n");
            let id = str_of(&out, "ask_id");
            let theirs = str_of(&prompt_for_host(ws, &id, "other").unwrap(), "prompt");
            assert!(theirs.starts_with("/think "), "{theirs}");
            let sub = submitted(ws, "other", "s-9", theirs.trim_end()).unwrap();
            assert_eq!(sub["ask_id"], out["ask_id"]);
            assert_eq!(sub["match"], "receiving_host_prefix");
            assert_eq!(sub["host"]["name"], "other");
        });
    }

    #[test]
    fn two_asks_with_one_prompt_and_no_delivery_record_no_submission() {
        bench(|ws| {
            let a = plan(ws, b"Fix the login bug.\n");
            plan(ws, b"Fix the login bug.\n");
            let typed = prompt_text(ws, &str_of(&a, "ask_id")).unwrap();
            assert!(submitted(ws, "claude-code", "s1", &typed).is_none());
        });
    }

    #[test]
    fn an_observed_delivery_tool_records_native_accepted_once() {
        bench(|ws| {
            let out = plan(ws, b"Fix the login bug.\n");
            let rec = delivery_observed(ws, "claude-code", "s1", "EnterPlanMode", false, "weft")
                .unwrap()
                .unwrap();
            assert_eq!(rec["state"], "native_accepted");
            assert_eq!(rec["ask_id"], out["ask_id"]);
            let last = events(ws).pop().unwrap();
            assert_eq!(last["type"], "ask.delivery");
            assert_eq!(
                last["data"]["receipt"],
                json!({"tool": "EnterPlanMode", "session_id": "s1", "captured_by": "weft"})
            );
            // Already delivered: skipped, and why is logged.
            assert!(
                delivery_observed(ws, "claude-code", "s1", "EnterPlanMode", false, "weft")
                    .unwrap()
                    .is_none()
            );
            let skipped = std::fs::read_to_string(
                ws.rf_dir().join("sessions/claude-code/s1/delivery-skipped.jsonl"),
            )
            .unwrap();
            assert!(skipped.contains("no_candidate"), "{skipped}");
        });
    }

    #[test]
    fn an_observed_failed_delivery_tool_records_failed() {
        bench(|ws| {
            plan(ws, b"Fix the login bug.\n");
            let rec = delivery_observed(ws, "claude-code", "s1", "EnterPlanMode", true, "weft")
                .unwrap()
                .unwrap();
            assert_eq!(rec["state"], "delivery_failed");
        });
    }

    #[test]
    fn an_observed_delivery_tool_ignores_other_sessions_tools_and_ambiguity() {
        bench(|ws| {
            plan(ws, b"Fix the login bug.\n");
            let seen = |session: &str, tool: &str| {
                delivery_observed(ws, "claude-code", session, tool, false, "weft").unwrap()
            };
            assert!(seen("other", "EnterPlanMode").is_none());
            assert!(seen("s1", "Write").is_none());
            plan(ws, b"Something else.\n");
            assert!(seen("s1", "EnterPlanMode").is_none());
            let skipped = std::fs::read_to_string(
                ws.rf_dir().join("sessions/claude-code/s1/delivery-skipped.jsonl"),
            )
            .unwrap();
            assert!(skipped.contains("ambiguous"), "{skipped}");
        });
    }

    #[test]
    fn asks_are_listed_oldest_first_and_stay_open_until_sealed() {
        bench(|ws| {
            assert!(list_asks(ws).unwrap().is_empty());
            let head = crate::evaluate::git(&ws.root, &["rev-parse", "HEAD"]).unwrap();
            let a = plan(ws, b"Fix the login bug.\n");
            let b = plan(ws, b"Other.\n");
            assert_eq!(events(ws)[0]["data"]["base_commit"], head.trim());
            let ids =
                |asks: Vec<Value>| asks.iter().map(|x| str_of(x, "ask_id")).collect::<Vec<_>>();
            let (a, b) = (str_of(&a, "ask_id"), str_of(&b, "ask_id"));
            assert_eq!(ids(list_asks(ws).unwrap()), [a.clone(), b.clone()]);
            assert_eq!(list_asks(ws).unwrap()[1]["title"], "Other.");
            assert_eq!(ids(open_asks(ws).unwrap()), [a.clone(), b.clone()]);
            // A seal event naming the Ask in its basis closes it.
            store::append(ws, &json!({
                "schema": store::SCHEMA, "event_id": ids::new_id("evt"), "type": "seal.created",
                "time": sessions::now(), "id": "sel_x",
                "actor": {"kind": "human", "id": "local-user"}, "links": [],
                "data": {
                    "basis": {"asks": [a]}, "eval": null,
                    "subject": {"kind": "git_commit", "ref": head.trim()}, "disposition": "accepted",
                    "authority": {"kind": "human", "id": "local-user", "authority": "interactive"},
                    "artifact": {"role": "seal_receipt", "path": "seals/sel_x.json", "bytes": 0,
                                 "sha256": "0".repeat(64)}
                }
            })).unwrap();
            let states =
                list_asks(ws).unwrap().iter().map(|x| str_of(x, "state")).collect::<Vec<_>>();
            assert_eq!(states, ["sealed", "open"]);
            assert_eq!(ids(open_asks(ws).unwrap()), [b]);
        });
    }

    /// Nothing writes a cancellation any more; one an older RingFrame wrote
    /// still closes its Ask.
    #[test]
    fn an_ask_an_older_ringframe_cancelled_stays_cancelled() {
        bench(|ws| {
            let a = str_of(&plan(ws, b"Fix the login bug.\n"), "ask_id");
            let actor = json!({"kind": "human", "id": "local-user", "authority": "interactive"});
            let cancelled = json!({"cancellation": {"observed_by": "skill", "surface": null}});
            let ev = event("ask.cancelled", &a, &actor, cancelled, &[]);
            schema::validate_event(&ev).unwrap();
            store::append(ws, &ev).unwrap();
            assert_eq!(str_of(&list_asks(ws).unwrap()[0], "state"), "cancelled");
            assert!(open_asks(ws).unwrap().is_empty());
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }
}
