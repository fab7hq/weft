//! Ask: persist a compiled intent, then append graded observations
//! (confirmation, cancellation, submission, delivery).

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::store::LedgerError;
use crate::workspace::Workspace;
use crate::{config, deltas, digest, ids, profiles, schema, sessions, store, workspace};

/// How far back a `PostToolUse` hook may reach for the Ask it belongs to.
const HOOK_WINDOW_MS: i64 = 30 * 60 * 1000;

/// The root is named because a handoff is submitted by hand, and nothing
/// stops it being submitted somewhere else. The prompt's paths are relative
/// to the workspace, so a session started one directory up writes the work
/// outside the record that is meant to describe it — and the Eval then judges
/// a tree where nothing happened.
const HANDOFF: &str = "Prompt prepared for {host} ({capability}):
{path}

Submit it in this directory — the paths in the prompt are relative to it, and
it is where this work is recorded:
{root}
";
/// What to do when the prompt is short enough to go in as it stands. A prompt
/// that folds gets different instructions, and printing both would contradict
/// itself: "copy its complete contents" is the one thing that does not work.
const HANDOFF_WHOLE: &str = "
Open the file, copy its complete contents, and submit them in the
active {host_title} TUI. RingFrame does not observe that submission.
";
const HANDOFF_OBSERVATION: &str = "
RingFrame does not observe that submission.
";

/// A prompt long enough to be folded cannot carry its own command: the host
/// stops reading a folded paste as text, so the command goes in as prose and
/// the mode is never entered. Said here rather than left to be discovered.
const HANDOFF_FOLDS_MODE: &str = "
{host_title} folds a paste of {fold} characters or more into a placeholder and
does not read a folded paste for commands, so pasting all {total} characters
at once would run this as an ordinary request instead of {mode}.

{how}
";
// The body is offered as its own thing rather than as "the rest of the file".
// `{mode} ` is the first few characters of the first line, not a line of its
// own, so "the rest" invites a copy that starts at line two and silently drops
// the first sentence — which is the objective. Nothing here asks anyone to
// split a line by eye.
const HANDOFF_MODE_FIRST: &str = "Submit `{mode}` on its own first. {host_title} shows
`{active}` once it is on. Then copy the body, which is the prompt without its
command:

    ringframe ask copy --ask {ask} --body

Paste that and submit it.";
const HANDOFF_TYPE_PREFIX: &str = "Type `{mode} ` yourself at the start of an empty
composer — typed, the command is read; pasted, it is not. Then copy the body,
which is the prompt without its command:

    ringframe ask copy --ask {ask} --body

Paste that beside what you typed, and submit.";

fn host_title(host: &str) -> &str {
    match host {
        "claude-code" => "Claude Code",
        "codex" => "Codex",
        other => other,
    }
}

fn fill(template: &str, pairs: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (k, v) in pairs {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// The Ask cannot go on without a person: either an authorization is missing,
/// or a choice has to be made between candidates.
#[derive(Debug)]
pub struct NeedsInput {
    pub reason: String,
    pub candidates: Vec<Value>,
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

fn ledger(code: &str, detail: impl Into<String>) -> AskError {
    AskError::Ledger(LedgerError::new_public(code, detail))
}

fn event(type_: &str, id: &str, actor: &Value, data: Value, links: &[Value]) -> Value {
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

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn list_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Interactive humans act by being present. Any other actor needs a
/// pre-authorization record.
fn authorized(
    ws: &Workspace,
    actor: &Value,
    capability: &str,
    effects: &[String],
) -> Result<Value, AskError> {
    let authority = {
        let a = str_of(actor, "authority");
        if a.is_empty() { "interactive".to_string() } else { a }
    };
    if str_of(actor, "kind") == "human" && authority == "interactive" {
        return Ok(actor.clone());
    }
    let id = str_of(actor, "id");
    let kind = str_of(actor, "kind");
    let path = ws.rf_dir().join("authorizations").join(format!("{id}.json"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Err(NeedsInput {
            reason: format!(
                "authorization required: no record at authorizations/{id}.json for {kind}:{id}"
            ),
            candidates: Vec::new(),
        }
        .into());
    };
    let grant: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let allowed = grant.get("allowed").cloned().unwrap_or_else(|| json!({}));
    let expired = grant
        .get("expires")
        .and_then(Value::as_str)
        .and_then(sessions::parse_time)
        .is_some_and(|e| e <= sessions::now_millis());
    let granted: BTreeSet<String> = list_of(&allowed, "effects").into_iter().collect();
    let ok = grant.get("actor").and_then(Value::as_str) == Some(&format!("{kind}:{id}"))
        && list_of(&allowed, "capabilities").iter().any(|c| c == capability)
        && effects.iter().all(|e| granted.contains(e))
        && !expired;
    if !ok {
        let mut shown: Vec<&String> = effects.iter().collect();
        shown.sort();
        return Err(NeedsInput {
            reason: format!(
                "authorization does not cover {capability} with effects [{}] for {kind}:{id}",
                shown.iter().map(|e| format!("'{e}'")).collect::<Vec<_>>().join(", ")
            ),
            candidates: Vec::new(),
        }
        .into());
    }
    let mut out = actor.clone();
    out["authority"] = json!(format!("preauthorized:authorizations/{id}.json"));
    Ok(out)
}

/// Domain names are file names, so their hyphens are significant and must
/// survive normalization.
const VERBATIM: [&str; 1] = ["domains"];

/// Accept `approval-gated` for `approval_gated`; the vocabulary itself is
/// unchanged.
fn normalize_classification(c: &Value) -> Value {
    let Some(map) = c.as_object() else { return c.clone() };
    let fix = |v: &Value| match v.as_str() {
        Some(s) => Value::String(s.replace('-', "_")),
        None => v.clone(),
    };
    let mut out = Map::new();
    for (k, v) in map {
        let next = if VERBATIM.contains(&k.as_str()) {
            v.clone()
        } else {
            match v.as_array() {
                Some(items) => Value::Array(items.iter().map(fix).collect()),
                None => fix(v),
            }
        };
        out.insert(k.clone(), next);
    }
    Value::Object(out)
}

/// `source.txt` plus exactly one of `prompt.txt` (legacy: the model wrote
/// everything), `body.txt` (the CLI renders the directives after it) or
/// `composed.txt` (the model applied the CLI-selected directives; the CLI adds
/// the prefix only).
fn staged_form(names: &[String]) -> Option<&'static str> {
    match names.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["prompt.txt", "source.txt"] => Some("prompt"),
        ["body.txt", "source.txt"] => Some("body"),
        ["composed.txt", "source.txt"] => Some("composed"),
        _ => None,
    }
}

fn read_staged(staged: &Path) -> Result<(Vec<u8>, &'static str, Vec<u8>), AskError> {
    let mut names: Vec<String> = std::fs::read_dir(staged)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    let Some(form) = staged_form(&names) else {
        return Err(ledger(
            "ask.staged_dir",
            "must contain exactly source.txt and one of composed.txt, body.txt or prompt.txt",
        ));
    };
    let read = |name: &str| -> Result<Vec<u8>, AskError> {
        let data = std::fs::read(staged.join(name))?;
        if data.is_empty()
            || data.starts_with(b"\xef\xbb\xbf")
            || String::from_utf8(data.clone()).is_err()
        {
            return Err(ledger(
                "ask.staged_file",
                format!("{name} must be non-empty UTF-8 without BOM"),
            ));
        }
        Ok(data)
    };
    let source = read("source.txt")?;
    let text = read(&names[0])?;
    Ok((source, form, text))
}

fn without(block: &Value, drop: &[&str]) -> Value {
    let mut out = Map::new();
    for (k, v) in block.as_object().into_iter().flatten() {
        if !drop.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// prompt = capability prefix + text (+ rendered directives when the form is
/// `body`). The selection is always the CLI's; a composed prompt records what
/// was supplied, recomputed from the classification.
fn render_prompt(
    ws: &Workspace,
    profile: &Value,
    cap: &Value,
    capability: &str,
    classification: &Value,
    text_in: &[u8],
    form: &str,
) -> Result<(Vec<u8>, Value), AskError> {
    let rendered = deltas::render(
        Some(ws),
        profile,
        capability,
        classification,
        &["qualified".to_string()],
        deltas::DEFAULT_DOMAIN,
    )
    .map_err(|e| ledger("ask.classification", e.0))?;
    let body = String::from_utf8_lossy(text_in).trim_end_matches('\n').to_string();
    let mut text = format!("{}{body}\n", str_of(cap, "prompt_prefix"));
    let rendered_text = str_of(&rendered, "text");
    if form == "body" && !rendered_text.is_empty() {
        text.push_str(rendered_text.trim_end_matches('\n'));
        text.push('\n');
    }
    let mut provenance = json!({
        "source": form,
        "host": without(&rendered["host"], &["text", "entries"]),
        "practice": without(&rendered["practice"], &["text", "entries"]),
    });
    if form == "composed" {
        let mut supplied: Vec<Value> =
            rendered["host"]["entries"].as_array().cloned().unwrap_or_default();
        supplied.extend(rendered["practice"]["entries"].as_array().cloned().unwrap_or_default());
        let (applied, omitted) =
            deltas::audit_composed(&String::from_utf8_lossy(text_in), &supplied)
                .map_err(|e| ledger("ask.composed_rules", e.0))?;
        provenance["applied"] = json!(applied);
        provenance["omitted"] = json!(omitted);
    }
    Ok((text.into_bytes(), provenance))
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
fn delivery_block(profile: &Value, cap: &Value, prompt: &[u8]) -> Value {
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
/// `compile` already requires Git, but it runs at the end: by then the skill
/// has read the profile, classified the intent, composed the prompt and staged
/// it, and the person has waited through a model turn to be told the workspace
/// was never usable. This is the same refusal, available first.
pub fn preflight(ws: &Workspace) -> Result<Value, AskError> {
    workspace::require_git(ws)?;
    Ok(json!({"ready": true, "workspace": ws.root.to_string_lossy()}))
}

pub struct Compile<'a> {
    pub staged: &'a Path,
    pub title: &'a str,
    pub capability: &'a str,
    pub classification: Value,
    pub route: Value,
    pub host: Value,
    pub links: Vec<Value>,
    pub limitations: Vec<String>,
    pub actor: Option<Value>,
}

/// The only Ask operation that writes artifacts. Appends `ask.compiled`.
pub fn compile(ws: &Workspace, args: Compile<'_>) -> Result<Value, AskError> {
    workspace::require_git(ws)?;
    // Staging is read and then deleted, so it has to be inside the directory
    // RingFrame was opened in. The skill already stages under `.fab7/rf/tmp/`.
    let staged = workspace::within(ws, args.staged)?;
    let (source, form, mut prompt) = read_staged(&staged)?;
    let classification = normalize_classification(&args.classification);
    let mut host = args.host.clone();
    let mut provenance: Vec<(&str, Value)> = Vec::new();
    if str_of(&host, "session_ref").is_empty()
        && let Some(found) =
            sessions::resolve_session(ws, &str_of(&host, "name"), &source, sessions::WINDOW)
    {
        host["session_ref"] = found["session_ref"].clone();
        provenance.push(("session_ref_source", json!("capture")));
        if str_of(&host, "version").is_empty() && found["host_version"].is_string() {
            host["version"] = found["host_version"].clone();
            provenance.push(("version_source", json!("capture")));
        }
    }
    let profile = profiles::for_host(&host)?;
    let Some(cap) = profiles::capability(&profile, args.capability).cloned() else {
        return Err(ledger(
            "ask.capability",
            format!("'{}' is not in profile {}", args.capability, str_of(&profile, "profile_id")),
        ));
    };
    let effects = list_of(&classification, "effects");
    let gated: Vec<String> = {
        let mut g: Vec<String> = list_of(&cap, "requires_explicit_request_for_effects")
            .into_iter()
            .filter(|e| effects.contains(e))
            .collect();
        g.sort();
        g
    };
    if !gated.is_empty() && args.route.get("explicit_direct_request") != Some(&json!(true)) {
        return Err(ledger(
            "ask.route_policy",
            format!(
                "{} with effects [{}] requires route.explicit_direct_request=true, which is only \
                 true when the source intent itself asks to skip planning or act immediately; \
                 otherwise select native_plan",
                args.capability,
                gated.iter().map(|e| format!("'{e}'")).collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    let domains = deltas::selected_domains(&classification, Some(ws))
        .map_err(|e| ledger("ask.classification", e.0))?;
    deltas::validate_concerns(&list_of(&classification, "concerns"), &domains, Some(ws))
        .map_err(|e| ledger("ask.classification", e.0))?;
    let mut compiler = json!({"source": "prompt"});
    if form != "prompt" {
        let (rendered, prov) =
            render_prompt(ws, &profile, &cap, args.capability, &classification, &prompt, form)?;
        prompt = rendered;
        compiler = prov;
    }
    let text = String::from_utf8_lossy(&prompt).to_string();
    let prefix = str_of(&cap, "prompt_prefix");
    if !prefix.is_empty() && !text.starts_with(&prefix) {
        return Err(ledger(
            "ask.prompt_prefix",
            format!(
                "{} on {} requires prompt.txt to begin with '{prefix}'",
                args.capability,
                str_of(&host, "name")
            ),
        ));
    }
    if let Some(max) = cap.get("max_prompt_chars").and_then(Value::as_u64)
        && text.trim_end_matches('\n').chars().count() as u64 > max
    {
        return Err(ledger(
            "ask.prompt_too_long",
            format!(
                "{} on {} allows at most {max} characters",
                args.capability,
                str_of(&host, "name")
            ),
        ));
    }
    let actor = authorized(ws, &default_actor(args.actor.as_ref()), args.capability, &effects)?;
    let mut limitations = args.limitations.clone();
    limitations.extend(list_of(&cap, "limitations"));
    if str_of(&profile, "profile_id") == "unknown" {
        limitations.push("qualification gap: no profile for this host".into());
    }
    let ask_id = ids::new_id("ask");
    // Provisional references let the event be validated before anything is written.
    let source_ref = json!({"role": "source_intent", "path": format!("asks/{ask_id}/source.txt"),
                            "bytes": source.len(), "sha256": digest::sha256_bytes(&source)});
    let prompt_ref = json!({"role": "generated_prompt", "path": format!("asks/{ask_id}/prompt.txt"),
                            "bytes": prompt.len(), "sha256": digest::sha256_bytes(&prompt)});
    let session_ref = host.get("session_ref").and_then(Value::as_str).map(str::to_string);
    let (verified, reason) =
        sessions::source_verified(ws, &str_of(&host, "name"), session_ref.as_deref(), &source);
    if let Some(reason) = reason {
        limitations.push(format!("source_verified unverified: {reason}"));
    }
    let mut host_block = json!({
        "name": host.get("name").cloned().unwrap_or(Value::Null),
        "version": host.get("version").cloned().unwrap_or(Value::Null),
        "surface": host.get("surface").cloned().unwrap_or(Value::Null),
        "session_ref": host.get("session_ref").cloned().unwrap_or(Value::Null),
        "workspace": ws.describe(),
        "profile_id": str_of(&profile, "profile_id"),
        "profile_sha256": profiles::sha256(
            profile.get("host").and_then(Value::as_str).unwrap_or("unknown"))?,
    });
    for (k, v) in provenance {
        host_block[k] = v;
    }
    let data = json!({
        "title": args.title, "classification": classification,
        "selected_capability": args.capability, "route_explanation": args.route,
        "host": host_block, "source": source_ref, "prompt": prompt_ref,
        "source_verified": verified, "limitations": limitations,
        "delivery_mode": cap["delivery_mode"],
        "delivery": delivery_block(&profile, &cap, &prompt),
        "compiler": compiler, "base_commit": head(ws),
    });
    let ev = event("ask.compiled", &ask_id, &actor, data.clone(), &args.links);
    schema::validate_event(&ev)?;
    let wrote_source = store::publish(ws, &str_of(&source_ref, "path"), &source, "source_intent")?;
    let wrote_prompt =
        store::publish(ws, &str_of(&prompt_ref, "path"), &prompt, "generated_prompt")?;
    // The references went into a validated event before the bytes existed; if
    // they now disagree, the record would describe a file that is not there.
    assert_eq!(wrote_source, source_ref, "the published source is not what was recorded");
    assert_eq!(wrote_prompt, prompt_ref, "the published prompt is not what was recorded");
    store::append(ws, &ev)?;
    std::fs::remove_dir_all(&staged)?;
    Ok(json!({
        "ask_id": ask_id, "source": source_ref, "prompt": prompt_ref,
        "prompt_path": ws.rf_dir().join(str_of(&prompt_ref, "path")).to_string_lossy(),
        "source_verified": verified, "delivery_mode": data["delivery_mode"],
    }))
}

/// The workspace commit an Ask starts from; Eval's anchor when no Seal
/// precedes it. Null outside Git.
fn head(ws: &Workspace) -> Value {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&ws.root)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { Value::Null } else { Value::String(s) }
        }
        _ => Value::Null,
    }
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

/// The confirmation surface a profile declares. None under the unknown
/// profile: never invent a surface.
fn confirmation_surface(host: &Value) -> Value {
    profiles::for_host(host)
        .ok()
        .and_then(|p| p.get("confirmation").and_then(|c| c.get("tool")).cloned())
        .unwrap_or(Value::Null)
}

/// Record the skill-reported chooser answer. Evidence grade: observed by the
/// skill, not by the host.
pub fn confirm(ws: &Workspace, ask_id: &str, actor: Option<&Value>) -> Result<Value, AskError> {
    let rec = record(ws, ask_id)?;
    if rec.confirmed.is_some() {
        return Err(ledger("ask.already_confirmed", ask_id));
    }
    let d = &rec.compiled.as_ref().expect("a record has a compiled event")["data"];
    let actor = authorized(
        ws,
        &default_actor(actor),
        &str_of(d, "selected_capability"),
        &list_of(&d["classification"], "effects"),
    )?;
    append_event(
        ws,
        "ask.confirmed",
        ask_id,
        json!({"confirmation": {"observed_by": "skill", "surface": confirmation_surface(&d["host"])}}),
        Some(&actor),
    )
}

pub fn cancel(
    ws: &Workspace,
    ask_id: &str,
    reason: Option<&str>,
    actor: Option<&Value>,
    attributed: bool,
) -> Result<Value, AskError> {
    let rec = record(ws, ask_id)?;
    if rec.cancelled.is_some() {
        return Err(ledger("ask.already_cancelled", ask_id));
    }
    let a = default_actor(actor);
    let grade = if attributed {
        json!({"attributed_by": format!("{}:{}", str_of(&a, "kind"), str_of(&a, "id"))})
    } else {
        json!({"observed_by": "skill"})
    };
    let mut data = json!({"cancellation": grade});
    if let Some(reason) = reason.filter(|r| !r.is_empty()) {
        data["reason"] = json!(reason);
    }
    append_event(ws, "ask.cancelled", ask_id, data, Some(&a))
}

/// The confirmation surface gave no answer. An observation, not an outcome.
///
/// Dismissed, timed out and closed-without-choosing are indistinguishable in
/// what the surface returns, so this records what was seen — nothing — and
/// says which surface it was seen on. It deliberately does **not** end the
/// Ask: no answer is not a no, and treating it as one throws away a candidate
/// the person may still want, while writing a refusal they never made.
///
/// The Ask stays open and confirmable. Whoever asks next — the same skill, or
/// Weft's board — can still put it to the person.
pub fn unanswered(
    ws: &Workspace,
    ask_id: &str,
    reason: Option<&str>,
    actor: Option<&Value>,
) -> Result<Value, AskError> {
    let rec = record(ws, ask_id)?;
    if rec.confirmed.is_some() {
        return Err(ledger("ask.already_confirmed", ask_id));
    }
    if rec.cancelled.is_some() {
        return Err(ledger("ask.already_cancelled", ask_id));
    }
    let a = default_actor(actor);
    let d = &rec.compiled.as_ref().expect("a record has a compiled event")["data"];
    let mut data = json!({"unanswered": {"observed_by": "skill", "surface": confirmation_surface(&d["host"])}});
    if let Some(reason) = reason.filter(|r| !r.is_empty()) {
        data["reason"] = json!(reason);
    }
    append_event(ws, "ask.unanswered", ask_id, data, Some(&a))
}

/// The person attests that they submitted the prompt, possibly edited. Human
/// attestation, not host evidence.
pub fn submitted(
    ws: &Workspace,
    ask_id: &str,
    as_modified: bool,
    actor: Option<&Value>,
) -> Result<Value, AskError> {
    let rec = record(ws, ask_id)?;
    let a = default_actor(actor);
    let d = &rec.compiled.as_ref().expect("a record has a compiled event")["data"];
    append_event(
        ws,
        "ask.submission",
        ask_id,
        json!({
            "state": "attributed", "observed_by": null,
            "attributed_by": format!("{}:{}", str_of(&a, "kind"), str_of(&a, "id")),
            "as_modified": as_modified,
            "host": {"name": d["host"]["name"], "session_ref": d["host"]["session_ref"]},
            "prompt_sha256": d["prompt"]["sha256"],
        }),
        Some(&a),
    )
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
) -> Result<Option<Value>, AskError> {
    let mut hits: Vec<(String, String, bool)> = Vec::new();
    for (ask_id, v) in by_id(ws)? {
        let Some(compiled) = &v.compiled else { continue };
        if v.submissions.iter().any(|s| s["data"]["state"] == "observed") {
            continue;
        }
        if let Some(m) = prompt_match(ws, &compiled["data"], sha256)? {
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
    Ok(Some(append_event(
        ws,
        "ask.submission",
        &ask_id,
        json!({
            "state": "observed", "observed_by": "hook:UserPromptSubmit", "attributed_by": null,
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
) -> Result<Option<String>, AskError> {
    let reference = &compiled["prompt"];
    if str_of(reference, "sha256") == sha256 {
        return Ok(Some("exact".into()));
    }
    let data = std::fs::read(ws.rf_dir().join(str_of(reference, "path")))?;
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
    Ok(None)
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

pub fn prompt_text(ws: &Workspace, ask_id: &str) -> Result<String, AskError> {
    let rec = record(ws, ask_id)?;
    let path = str_of(&rec.compiled.expect("compiled")["data"]["prompt"], "path");
    Ok(std::fs::read_to_string(ws.rf_dir().join(path))?)
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
    let data = json!({
        "mode": mode, "mechanism": mechanism, "state": state,
        "qualification": cap.get("qualification").cloned().unwrap_or_else(|| json!({"id": null})),
        "receipt": receipt,
        "submission": if mode == "human_handoff" { "unobserved" } else { "not_applicable" },
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

/// Record `native_accepted` (or `delivery_failed`) from a `PostToolUse`
/// payload; never raise.
pub fn delivery_from_hook(ws: &Workspace, payload: &Value) -> Result<Option<Value>, AskError> {
    let session = str_of(payload, "session_id");
    let host = "claude-code";
    if str_of(payload, "hook_event_name") != "PostToolUse" || session.is_empty() {
        return Ok(None);
    }
    let tool = str_of(payload, "tool_name");
    let cutoff = sessions::now_millis() - HOOK_WINDOW_MS;
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
            &session,
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
    let response = payload.get("tool_response").cloned().unwrap_or(Value::Null);
    let error = response.get("error").filter(|v| !v.is_null()).cloned();
    let failed =
        response.is_object() && (error.is_some() || response.get("is_error") == Some(&json!(true)));
    let receipt = json!({
        "tool": tool, "tool_use_id": payload.get("tool_use_id").cloned().unwrap_or(Value::Null),
        "session_id": session, "response_sha256": digest::sha256_bytes(&store::canonical(&response)),
        "captured_by": "hook:PostToolUse",
    });
    let limitations = if failed {
        vec![format!(
            "tool error: {}",
            error
                .as_ref()
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| { error.map_or("None".to_string(), |e| e.to_string()) })
        )]
    } else {
        Vec::new()
    };
    Ok(Some(append_delivery(
        ws,
        &ask_id,
        &compiled,
        Delivery {
            mode: "native_dispatch",
            mechanism: json!("capability_activate"),
            state: if failed { "delivery_failed" } else { "native_accepted" },
            receipt,
            limitations,
        },
        None,
    )?))
}

pub fn delivery_handoff(ws: &Workspace, ask_id: &str) -> Result<(String, Value), AskError> {
    let compiled = record(ws, ask_id)?.compiled.expect("compiled");
    let d = &compiled["data"];
    let path = ws.rf_dir().join(str_of(&d["prompt"], "path"));
    let host = str_of(&d["host"], "name");
    let title = host_title(&host).to_string();
    let mut text = fill(
        HANDOFF,
        &[
            ("host", host.clone()),
            ("capability", str_of(d, "selected_capability")),
            ("path", path.to_string_lossy().to_string()),
            ("root", ws.root.to_string_lossy().to_string()),
        ],
    );
    // One instruction, never two: a prompt that folds cannot be pasted whole,
    // so saying "copy its complete contents" beside the fold advice would
    // contradict it.
    let folded = handoff_mode(d.get("delivery").unwrap_or(&Value::Null), &title, ask_id);
    if folded.is_empty() {
        text.push_str(&fill(HANDOFF_WHOLE, &[("host_title", title.clone())]));
    } else {
        text.push_str(HANDOFF_OBSERVATION);
        text.push_str(&folded);
    }
    let rec = append_delivery(
        ws,
        ask_id,
        &compiled,
        Delivery {
            mode: "human_handoff",
            mechanism: Value::Null,
            state: "handoff_ready",
            receipt: json!({"path": str_of(&d["prompt"], "path"), "emitted_by": "cli"}),
            limitations: vec!["submission unobserved".into()],
        },
        None,
    )?;
    Ok((text, rec))
}

/// What the person has to do differently because this prompt carries a mode.
///
/// Nothing, unless the prompt is long enough for the host to fold it. A record
/// from before `delivery` existed has no mode here and says nothing extra.
fn handoff_mode(delivery: &Value, title: &str, ask_id: &str) -> String {
    let mode = delivery.get("mode").and_then(Value::as_str).unwrap_or_default();
    if mode.is_empty() || delivery.get("folds") != Some(&json!(true)) {
        return String::new();
    }
    let active = str_of(delivery, "mode_active");
    let how = if str_of(delivery, "mode_kind") == "mode" && !active.is_empty() {
        fill(
            HANDOFF_MODE_FIRST,
            &[
                ("mode", mode.to_string()),
                ("host_title", title.to_string()),
                ("active", active),
                ("ask", ask_id.to_string()),
            ],
        )
    } else {
        fill(HANDOFF_TYPE_PREFIX, &[("mode", mode.to_string()), ("ask", ask_id.to_string())])
    };
    let total = delivery.get("prefix_bytes").and_then(Value::as_u64).unwrap_or(0)
        + delivery.get("body_bytes").and_then(Value::as_u64).unwrap_or(0);
    fill(
        HANDOFF_FOLDS_MODE,
        &[
            ("host_title", title.to_string()),
            ("fold", delivery.get("paste_fold_chars").map_or(String::new(), |v| v.to_string())),
            ("total", total.to_string()),
            ("mode", mode.to_string()),
            ("how", how),
        ],
    )
}

pub fn delivery_state(
    ws: &Workspace,
    ask_id: &str,
    state: &str,
    reason: &str,
) -> Result<Value, AskError> {
    let compiled = record(ws, ask_id)?.compiled.expect("compiled");
    let declared = str_of(&compiled["data"], "delivery_mode");
    let mode = if declared == "unsupported" { "human_handoff".to_string() } else { declared };
    append_delivery(
        ws,
        ask_id,
        &compiled,
        Delivery {
            mode: &mode,
            mechanism: Value::Null,
            state,
            receipt: Value::Null,
            limitations: vec![reason.to_string()],
        },
        None,
    )
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

/// Ordered resolution; never picks the newest for being newest.
pub fn resolve(
    ws: &Workspace,
    session: Option<&str>,
    reference: Option<&str>,
) -> Result<Value, AskError> {
    let sealed = sealed_asks(ws)?;
    let summaries: Vec<Value> = by_id(ws)?
        .into_iter()
        .filter(|(_, v)| v.compiled.is_some())
        .map(|(k, v)| summary(ws, &k, &v, &sealed))
        .collect();
    if let Some(reference) = reference.filter(|r| !r.is_empty()) {
        let exact: Vec<Value> =
            summaries.iter().filter(|s| str_of(s, "ask_id") == reference).cloned().collect();
        if !exact.is_empty() {
            return Ok(json!({"candidates": exact, "rule_applied": "explicit_id"}));
        }
        let needle = reference.to_lowercase();
        let hits: Vec<Value> = summaries
            .iter()
            .filter(|s| str_of(s, "title").to_lowercase().contains(&needle))
            .cloned()
            .collect();
        if hits.len() == 1 {
            return Ok(json!({"candidates": hits, "rule_applied": "explicit_title"}));
        }
        let candidates = if hits.is_empty() { summaries } else { hits };
        return Ok(json!({"candidates": candidates, "rule_applied": "chooser"}));
    }
    if let Some(session) = session.filter(|s| !s.is_empty()) {
        let same: Vec<Value> =
            summaries.iter().filter(|s| str_of(s, "session_ref") == session).cloned().collect();
        if !same.is_empty() {
            return Ok(json!({"candidates": same, "rule_applied": "same_session"}));
        }
    }
    let rule = if summaries.len() == 1 { "unique_in_workspace" } else { "chooser" };
    Ok(json!({"candidates": summaries, "rule_applied": rule}))
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

pub fn show(
    ws: &Workspace,
    ask_id: Option<&str>,
    session: Option<&str>,
) -> Result<Value, AskError> {
    let res = resolve(ws, session, ask_id)?;
    let candidates = res["candidates"].as_array().cloned().unwrap_or_default();
    if candidates.len() != 1 {
        return Err(NeedsInput {
            reason: if candidates.is_empty() { "no_record".into() } else { "chooser".into() },
            candidates,
        }
        .into());
    }
    Ok(candidates[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{repo, with_config_home, ws_for};

    fn host() -> Value {
        json!({"name": "claude-code", "version": "2.1.260", "surface": "native-tui",
               "session_ref": "s1"})
    }

    fn cls() -> Value {
        json!({"task": ["plan"], "result": "plan", "interaction": "approval_gated",
               "horizon": "session", "effects": ["read"]})
    }

    fn route() -> Value {
        json!({"fits": "bounded",
               "alternatives": [{"capability": "native_direct", "reason": "no review"}],
               "continuation": "plan review", "effects": "reads", "gaps": []})
    }

    /// A staging directory holding `source.txt` and one prompt form.
    fn stage_named(ws: &Workspace, name: &str, source: &[u8], prompt: &[u8]) -> std::path::PathBuf {
        let mut d = ws.rf_dir().join("tmp").join("stage-1");
        let mut n = 1;
        while d.exists() {
            n += 1;
            d = ws.rf_dir().join("tmp").join(format!("stage-{n}"));
        }
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("source.txt"), source).unwrap();
        std::fs::write(d.join(name), prompt).unwrap();
        d
    }

    fn stage(ws: &Workspace) -> std::path::PathBuf {
        stage_named(ws, "prompt.txt", b"fix the login bug\n", b"Fix the login bug.\n")
    }

    fn staged_prompt(ws: &Workspace, prompt: &[u8]) -> std::path::PathBuf {
        stage_named(ws, "prompt.txt", b"fix the login bug\n", prompt)
    }

    /// The Python suite's `compile_` helper: defaults with keyword overrides.
    struct Args {
        staged: Option<std::path::PathBuf>,
        title: String,
        capability: String,
        classification: Value,
        route: Value,
        host: Value,
        actor: Option<Value>,
    }

    impl Default for Args {
        fn default() -> Self {
            Args {
                staged: None,
                title: "Login fix".into(),
                capability: "native_plan".into(),
                classification: cls(),
                route: route(),
                host: host(),
                actor: None,
            }
        }
    }

    fn compile_with(ws: &Workspace, args: Args) -> Result<Value, AskError> {
        let staged = args.staged.unwrap_or_else(|| stage(ws));
        compile(
            ws,
            Compile {
                staged: &staged,
                title: &args.title,
                capability: &args.capability,
                classification: args.classification,
                route: args.route,
                host: args.host,
                links: Vec::new(),
                limitations: Vec::new(),
                actor: args.actor,
            },
        )
    }

    fn compiled(ws: &Workspace) -> Value {
        compile_with(ws, Args::default()).unwrap()
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

    fn types(ws: &Workspace) -> Vec<String> {
        events(ws).iter().map(|e| str_of(e, "type")).collect()
    }

    fn capture(
        ws: &Workspace,
        host: &str,
        session: &str,
        prompt: &str,
        version: Option<&str>,
    ) -> Value {
        sessions::capture(ws, host, &json!({"session_id": session, "prompt": prompt}), version)
            .unwrap()
            .unwrap()
    }

    fn bench<T>(body: impl FnOnce(&Workspace) -> T) -> T {
        with_config_home(|_| {
            let repo = repo();
            let ws = ws_for(repo.path());
            body(&ws)
        })
    }

    #[test]
    fn compile_publishes_two_artifacts_and_one_event() {
        bench(|ws| {
            capture(ws, "claude-code", "s1", "/rf:ask fix the login bug", None);
            let out = compiled(ws);
            assert!(str_of(&out, "ask_id").starts_with("ask_"));
            assert_eq!(out["delivery_mode"], "native_dispatch");
            assert_eq!(out["source_verified"], "exact");
            assert_eq!(
                std::fs::read(ws.rf_dir().join(str_of(&out["prompt"], "path"))).unwrap(),
                b"Fix the login bug.\n"
            );
            assert!(!ws.rf_dir().join("tmp/stage-1").exists());
            let evs = events(ws);
            assert_eq!(evs.len(), 1);
            assert_eq!(evs[0]["type"], "ask.compiled");
            assert_eq!(evs[0]["id"], out["ask_id"]);
            assert_eq!(evs[0]["data"]["host"]["profile_id"], "claude-code");
            assert_eq!(evs[0]["data"]["host"]["workspace"]["rule"], "cwd");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
            let shown = show(ws, None, None).unwrap();
            assert_eq!(shown["outcome"], "compiled");
            assert_eq!(shown["submission"], "unobserved");
        });
    }

    fn codex() -> Value {
        json!({"name": "codex", "version": "0.155.1", "surface": "native-tui",
               "session_ref": "c1"})
    }

    fn goal_cls() -> Value {
        json!({"task": ["implement"], "result": "continuing_objective",
               "interaction": "approval_gated", "horizon": "persistent", "effects": ["read"]})
    }

    #[test]
    fn a_compiled_ask_records_the_mode_beside_the_prompt() {
        // prompt.txt stays the whole thing; the mode it carries is recorded
        // too. Both hosts fold a long paste and stop reading it for commands,
        // so a client has to be able to send the command and the body apart.
        // It can only do that if the record says which prefix is a mode and
        // where the body starts.
        bench(|ws| {
            let out = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"/plan Ship the endpoint.\n")),
                    host: codex(),
                    ..Default::default()
                },
            )
            .unwrap();
            let evs = events(ws);
            let d = &evs[0]["data"]["delivery"];
            assert_eq!(d["mode"], "/plan");
            assert_eq!(d["mode_kind"], "mode");
            assert_eq!(d["mode_active"], "Plan mode");
            // The prompt itself is untouched, and the split is exact.
            let prompt = std::fs::read(ws.rf_dir().join(str_of(&out["prompt"], "path"))).unwrap();
            assert_eq!(prompt, b"/plan Ship the endpoint.\n");
            let prefix = d["prefix_bytes"].as_u64().unwrap() as usize;
            assert_eq!(&prompt[prefix..], b"Ship the endpoint.\n");
            assert_eq!(prefix as u64 + d["body_bytes"].as_u64().unwrap(), prompt.len() as u64);
            assert_eq!(d["paste_fold_chars"], 1001);
            assert_eq!(d["folds"], false, "short enough to paste whole");
        });
    }

    #[test]
    fn a_prompt_past_the_fold_is_recorded_as_folding() {
        bench(|ws| {
            let mut long = b"/plan ".to_vec();
            long.extend(std::iter::repeat_n(b'x', 1200));
            long.push(b'\n');
            compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, &long)),
                    host: codex(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(events(ws)[0]["data"]["delivery"]["folds"], true);
        });
    }

    #[test]
    fn an_inline_command_is_recorded_as_one_and_not_a_mode() {
        // `/goal` takes its objective as the argument: sent alone it only
        // views the goal, so there is no two-step to offer and the record
        // says so.
        bench(|ws| {
            let mut r = route();
            r["explicit_direct_request"] = json!(true);
            compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"/goal Keep the suite green.\n")),
                    capability: "native_goal".into(),
                    host: codex(),
                    classification: goal_cls(),
                    route: r,
                    ..Default::default()
                },
            )
            .unwrap();
            let d = &events(ws)[0]["data"]["delivery"];
            assert_eq!(d["mode"], "/goal");
            assert_eq!(d["mode_kind"], "inline");
            assert_eq!(d["mode_active"], json!(null));
        });
    }

    #[test]
    fn a_capability_with_no_prefix_records_no_mode() {
        bench(|ws| {
            compiled(ws);
            let d = &events(ws)[0]["data"]["delivery"];
            assert_eq!(d["mode"], json!(null));
            assert_eq!(d["mode_kind"], json!(null));
            assert_eq!(d["prefix_bytes"], 0);
        });
    }

    #[test]
    fn compile_will_not_read_or_delete_a_staging_directory_outside_the_workspace() {
        // Compile removes the staging directory when it is done, so a path
        // that escaped would have RingFrame delete something nobody opened.
        bench(|ws| {
            let elsewhere = crate::testing::tmp_dir();
            let staged = elsewhere.path().join("stage-1");
            std::fs::create_dir_all(&staged).unwrap();
            std::fs::write(staged.join("source.txt"), b"do it\n").unwrap();
            std::fs::write(staged.join("prompt.txt"), b"/plan Do it.\n").unwrap();

            let e = compile_with(ws, Args { staged: Some(staged.clone()), ..Default::default() })
                .unwrap_err();
            assert!(matches!(&e, AskError::Workspace(w) if w.code == "workspace.outside"), "{e}");
            assert!(staged.exists(), "and it is still there");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    #[test]
    fn the_handoff_says_which_directory_to_submit_it_in() {
        // A handoff is submitted by hand and nothing stops it being submitted
        // one directory up. Observed: a plan and its build both landed in the
        // parent repository while the record sat in the workspace, so the
        // Eval would have judged a tree where nothing had happened.
        bench(|ws| {
            let out = compile_with(ws, Args::default()).unwrap();
            confirm(ws, &str_of(&out, "ask_id"), None).unwrap();
            let (text, _) = delivery_handoff(ws, &str_of(&out, "ask_id")).unwrap();
            assert!(
                text.contains(&ws.root.to_string_lossy().to_string()),
                "the root the paths are relative to is named: {text}"
            );
        });
    }

    #[test]
    fn the_handoff_says_how_to_submit_a_mode_it_cannot_paste_whole() {
        bench(|ws| {
            let mut long = b"/plan ".to_vec();
            long.extend(std::iter::repeat_n(b'x', 1200));
            long.push(b'\n');
            let out = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, &long)),
                    host: codex(),
                    ..Default::default()
                },
            )
            .unwrap();
            confirm(ws, &str_of(&out, "ask_id"), None).unwrap();
            let (text, _) = delivery_handoff(ws, &str_of(&out, "ask_id")).unwrap();
            assert!(text.contains("1001 characters or more"), "{text}");
            assert!(text.contains("Submit `/plan` on its own first"), "{text}");
            assert!(text.contains("Plan mode"));

            // A prompt short enough to paste whole says nothing extra.
            let short = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"/plan Ship it.\n")),
                    host: codex(),
                    ..Default::default()
                },
            )
            .unwrap();
            confirm(ws, &str_of(&short, "ask_id"), None).unwrap();
            let (plain, _) = delivery_handoff(ws, &str_of(&short, "ask_id")).unwrap();
            assert!(!plain.contains("characters or more"), "{plain}");
        });
    }

    #[test]
    fn the_handoff_never_asks_anyone_to_split_a_line_by_eye() {
        // `/plan ` is the first few characters of the first line, not a line
        // of its own. Telling someone to paste "the rest of the file" invites
        // a copy that starts at line two and silently drops the first
        // sentence — which is the objective.
        bench(|ws| {
            let mut long = b"/plan ".to_vec();
            long.extend(std::iter::repeat_n(b'x', 1200));
            long.push(b'\n');
            let out = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, &long)),
                    host: codex(),
                    ..Default::default()
                },
            )
            .unwrap();
            let id = str_of(&out, "ask_id");
            confirm(ws, &id, None).unwrap();
            let (text, _) = delivery_handoff(ws, &id).unwrap();
            assert!(!text.contains("rest of the file"), "{text}");
            assert!(!text.contains("everything after"), "{text}");
            // It names the command that produces exactly what to paste.
            assert!(text.contains(&format!("ringframe ask copy --ask {id} --body")), "{text}");
            // And it does not also say to copy the whole thing, which is the
            // one thing that cannot work here.
            assert!(!text.contains("complete contents"), "{text}");
        });
    }

    #[test]
    fn the_body_is_the_prompt_without_its_command() {
        bench(|ws| {
            let out = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"/plan Ship the endpoint.\n")),
                    host: codex(),
                    ..Default::default()
                },
            )
            .unwrap();
            let id = str_of(&out, "ask_id");
            let whole = prompt_text(ws, &id).unwrap();
            let body = prompt_body(ws, &id).unwrap();
            assert_eq!(whole, "/plan Ship the endpoint.\n");
            assert_eq!(body, "Ship the endpoint.\n");
            // Typing the command and pasting the body reproduces the prompt
            // byte for byte, which is the whole point.
            assert_eq!(format!("/plan {body}"), whole);

            // A capability with no prefix has nothing to drop.
            let plain = compiled(ws);
            let id = str_of(&plain, "ask_id");
            assert_eq!(prompt_body(ws, &id).unwrap(), prompt_text(ws, &id).unwrap());
        });
    }

    #[test]
    fn a_prompt_that_fits_is_told_to_go_in_whole() {
        bench(|ws| {
            let out = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"/plan Ship it.\n")),
                    host: codex(),
                    ..Default::default()
                },
            )
            .unwrap();
            let id = str_of(&out, "ask_id");
            confirm(ws, &id, None).unwrap();
            let (text, _) = delivery_handoff(ws, &id).unwrap();
            assert!(text.contains("copy its complete contents"), "{text}");
            assert!(!text.contains("--body"), "{text}");
            assert!(text.contains("RingFrame does not observe"), "{text}");
        });
    }

    #[test]
    fn no_answer_leaves_the_ask_open_rather_than_refusing_it() {
        // Codex's chooser returns the same empty answer whether it was
        // dismissed, closed or timed out after its fixed two minutes. None of
        // those is a no, so none of them ends the Ask: the candidate stays
        // open for the person.
        bench(|ws| {
            let out = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"/plan Ship the endpoint.\n")),
                    host: codex(),
                    ..Default::default()
                },
            )
            .unwrap();
            let id = str_of(&out, "ask_id");
            unanswered(ws, &id, Some("no answer within the chooser's limit"), None).unwrap();

            let shown = show(ws, None, None).unwrap();
            assert_eq!(shown["outcome"], "compiled", "not cancelled");
            assert_eq!(shown["state"], "open", "and still usable");
            assert!(
                shown["unanswered_at"].is_string(),
                "the record says it was asked and not answered"
            );
            assert!(open_asks(ws).unwrap().iter().any(|a| str_of(a, "ask_id") == id));

            let last = events(ws).pop().unwrap();
            assert_eq!(last["type"], "ask.unanswered");
            assert_eq!(last["data"]["unanswered"]["observed_by"], "skill");
            // Named, so the record says where the question went unanswered.
            assert_eq!(last["data"]["unanswered"]["surface"], "request_user_input");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    #[test]
    fn an_unanswered_ask_can_still_be_confirmed_later() {
        // The whole point: the person gets to say yes afterwards, from
        // wherever they are — the same skill asking again, or Weft's board.
        bench(|ws| {
            let out = compiled(ws);
            let id = str_of(&out, "ask_id");
            unanswered(ws, &id, Some("chooser closed"), None).unwrap();
            confirm(ws, &id, None).unwrap();
            assert_eq!(show(ws, None, None).unwrap()["outcome"], "confirmed");
        });
    }

    #[test]
    fn an_ask_that_was_answered_or_refused_is_not_unanswered() {
        bench(|ws| {
            let yes = str_of(&compiled(ws), "ask_id");
            confirm(ws, &yes, None).unwrap();
            assert_eq!(
                ledger_error(unanswered(ws, &yes, None, None).unwrap_err()).code,
                "ask.already_confirmed"
            );

            let no = str_of(&compiled(ws), "ask_id");
            cancel(ws, &no, Some("not what I meant"), None, false).unwrap();
            assert_eq!(
                ledger_error(unanswered(ws, &no, None, None).unwrap_err()).code,
                "ask.already_cancelled"
            );
        });
    }

    #[test]
    fn cancel_still_means_the_person_said_no() {
        // Unanswered is not a softer cancel. An actual Cancel still ends it.
        bench(|ws| {
            let id = str_of(&compiled(ws), "ask_id");
            cancel(ws, &id, Some("wrong route"), None, false).unwrap();
            let shown = show(ws, None, None).unwrap();
            assert_eq!(shown["outcome"], "cancelled");
            assert_eq!(shown["state"], "cancelled");
            assert!(open_asks(ws).unwrap().is_empty());
        });
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
    fn preflight_refuses_what_compile_would_refuse_at_the_end() {
        // The same refusal, before the skill classifies, composes and stages.
        // `compile` has always required Git. It runs last, so the person
        // waited through a model turn to learn the workspace was never usable.
        let dir = crate::testing::tmp_dir();
        let plain = workspace::resolve(Some(dir.path()), None).unwrap();
        let e = preflight(&plain).unwrap_err();
        let AskError::Workspace(e) = e else { panic!("wanted a workspace error") };
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
    fn confirm_and_cancel_are_appended_graded_events() {
        bench(|ws| {
            let id = str_of(&compiled(ws), "ask_id");
            let rec = confirm(ws, &id, None).unwrap();
            // The Claude profile has no top-level confirmation surface.
            assert_eq!(rec["confirmation"], json!({"observed_by": "skill", "surface": null}));
            assert_eq!(
                ledger_error(confirm(ws, &id, None).unwrap_err()).code,
                "ask.already_confirmed"
            );
            let later = cancel(ws, &id, Some("changed my mind"), None, true).unwrap();
            assert_eq!(later["cancellation"], json!({"attributed_by": "human:local-user"}));
            assert_eq!(types(ws), ["ask.compiled", "ask.confirmed", "ask.cancelled"]);
            assert_eq!(show(ws, None, None).unwrap()["outcome"], "cancelled");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
            assert_eq!(
                ledger_error(confirm(ws, "ask_nope", None).unwrap_err()).code,
                "ask.not_compiled"
            );
        });
    }

    #[test]
    fn submission_observed_from_capture_and_attributed_by_human() {
        bench(|ws| {
            let codex_host = json!({"name": "codex", "surface": "native-tui"});
            let out = compile_with(
                ws,
                Args {
                    capability: "native_direct".into(),
                    host: codex_host.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
            let other = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"Something else.\n")),
                    title: "Other".into(),
                    capability: "native_direct".into(),
                    host: codex_host.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
            // The person pastes the exact prompt; the hook captures its digest.
            let rec = capture(ws, "codex", "c1", "Fix the login bug.\n", None);
            let sub = submission_from_capture(ws, "codex", "c1", &str_of(&rec, "sha256"))
                .unwrap()
                .unwrap();
            assert_eq!(sub["ask_id"], out["ask_id"]);
            assert_eq!(sub["state"], "observed");
            assert_eq!(sub["observed_by"], "hook:UserPromptSubmit");
            assert_eq!(sub["match"], "exact");
            // Once only.
            assert!(
                submission_from_capture(ws, "codex", "c1", &str_of(&rec, "sha256"))
                    .unwrap()
                    .is_none()
            );
            // Composers drop the trailing newline of a pasted file; that is
            // still the same prompt.
            let pasted = capture(ws, "codex", "c1", "Something else.", None);
            let sub2 = submission_from_capture(ws, "codex", "c1", &str_of(&pasted, "sha256"))
                .unwrap()
                .unwrap();
            assert_eq!(sub2["ask_id"], other["ask_id"]);
            assert_eq!(sub2["match"], "trailing_newline_dropped");
            assert_eq!(
                show(ws, Some(&str_of(&other, "ask_id")), None).unwrap()["submission"],
                "observed"
            );
            let third = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"Third.\n")),
                    title: "Third".into(),
                    capability: "native_direct".into(),
                    host: codex_host,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(submission_from_capture(ws, "codex", "c1", &"f".repeat(64)).unwrap().is_none());
            assert_eq!(
                show(ws, Some(&str_of(&out, "ask_id")), None).unwrap()["submission"],
                "observed"
            );
            let att = submitted(ws, &str_of(&third, "ask_id"), true, None).unwrap();
            assert_eq!(att["state"], "attributed");
            assert_eq!(att["as_modified"], true);
            assert_eq!(
                show(ws, Some(&str_of(&third, "ask_id")), None).unwrap()["submission"],
                "attributed"
            );
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    #[test]
    fn prompt_text_for_copy() {
        bench(|ws| {
            let id = str_of(&compiled(ws), "ask_id");
            assert_eq!(prompt_text(ws, &id).unwrap(), "Fix the login bug.\n");
        });
    }

    fn hook(session: &str, error: bool, tool: &str) -> Value {
        json!({
            "hook_event_name": "PostToolUse", "session_id": session, "tool_name": tool,
            "tool_use_id": "toolu_1", "tool_input": {},
            "tool_response": if error { json!({"error": "denied"}) } else { json!({"ok": true}) }
        })
    }

    #[test]
    fn confirm_resolves_session_and_version_from_capture() {
        bench(|ws| {
            capture(
                ws,
                "claude-code",
                "hook-session",
                "/rf:ask fix the login bug",
                Some("2.1.263 (Claude Code)"),
            );
            let out = compile_with(
                ws,
                Args {
                    host: json!({"name": "claude-code", "surface": "native-tui"}),
                    ..Default::default()
                },
            )
            .unwrap();
            confirm(ws, &str_of(&out, "ask_id"), None).unwrap();
            assert_eq!(out["source_verified"], "exact");
            let host = events(ws)[0]["data"]["host"].clone();
            assert_eq!(host["session_ref"], "hook-session");
            assert_eq!(host["session_ref_source"], "capture");
            assert_eq!(host["version"], "2.1.263 (Claude Code)");
            assert_eq!(host["version_source"], "capture");
            assert_eq!(host["profile_id"], "claude-code");
            let rec = delivery_from_hook(ws, &hook("hook-session", false, "EnterPlanMode"))
                .unwrap()
                .unwrap();
            assert_eq!(rec["state"], "native_accepted");
        });
    }

    #[test]
    fn compile_without_capture_is_unverified() {
        bench(|ws| {
            let out = compiled(ws);
            assert_eq!(out["source_verified"], "unverified");
            assert_eq!(events(ws)[0]["data"]["source_verified"], "unverified");
        });
    }

    #[test]
    fn compile_rejects_bad_staging_and_unknown_capability() {
        bench(|ws| {
            let d = stage(ws);
            std::fs::write(d.join("extra"), "x").unwrap();
            let e = compile_with(ws, Args { staged: Some(d.clone()), ..Default::default() })
                .unwrap_err();
            assert_eq!(ledger_error(e).code, "ask.staged_dir");
            std::fs::remove_file(d.join("extra")).unwrap();
            let e = compile_with(
                ws,
                Args {
                    staged: Some(d.clone()),
                    capability: "native_review".into(),
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert_eq!(ledger_error(e).code, "ask.capability");
            assert!(events(ws).is_empty());
            assert!(d.join("source.txt").exists());
        });
    }

    #[test]
    fn a_direct_route_with_effects_needs_an_explicit_request() {
        bench(|ws| {
            let mut writes = cls();
            writes["effects"] = json!(["write"]);
            let e = compile_with(
                ws,
                Args {
                    capability: "native_direct".into(),
                    classification: writes.clone(),
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert_eq!(ledger_error(e).code, "ask.route_policy");
            assert!(events(ws).is_empty());
            assert_eq!(std::fs::read_dir(ws.rf_dir().join("asks")).unwrap().count(), 0);

            // The staging survived the refusal.
            let mut explicit = route();
            explicit["explicit_direct_request"] = json!(true);
            let out = compile_with(
                ws,
                Args {
                    staged: Some(ws.rf_dir().join("tmp/stage-1")),
                    capability: "native_direct".into(),
                    classification: writes,
                    route: explicit,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(out["delivery_mode"], "native_dispatch");

            let mut question = cls();
            question["task"] = json!(["question"]);
            question["result"] = json!("answer");
            compile_with(
                ws,
                Args {
                    staged: Some(stage_named(
                        ws,
                        "prompt.txt",
                        b"what does auth.ts do?",
                        b"Explain auth.ts.",
                    )),
                    title: "Q".into(),
                    capability: "native_direct".into(),
                    classification: question,
                    ..Default::default()
                },
            )
            .unwrap();
        });
    }

    #[test]
    fn an_invalid_classification_publishes_nothing() {
        bench(|ws| {
            let mut bad = cls();
            bad["task"] = json!(["add-endpoint"]);
            let e =
                compile_with(ws, Args { classification: bad, ..Default::default() }).unwrap_err();
            assert!(ledger_error(e).detail.contains("classification.task"));
            assert_eq!(std::fs::read_dir(ws.rf_dir().join("asks")).unwrap().count(), 0);
            assert!(events(ws).is_empty());
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
            // Staging survives for a retry.
            assert!(ws.rf_dir().join("tmp/stage-1/source.txt").exists());
        });
    }

    #[test]
    fn hyphenated_vocabulary_is_accepted() {
        bench(|ws| {
            let mut c = cls();
            c["interaction"] = json!("approval-gated");
            c["horizon"] = json!("one-turn");
            compile_with(ws, Args { classification: c, ..Default::default() }).unwrap();
            let recorded = events(ws)[0]["data"]["classification"].clone();
            assert_eq!(recorded["interaction"], "approval_gated");
            assert_eq!(recorded["horizon"], "one_turn");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    #[test]
    fn an_unknown_host_falls_back_to_handoff() {
        bench(|ws| {
            let out = compile_with(
                ws,
                Args {
                    host: json!({"name": "cursor", "version": "1.0", "surface": "cli",
                             "session_ref": null}),
                    capability: "human_handoff".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(out["delivery_mode"], "human_handoff");
            let limitations = events(ws)[0]["data"]["limitations"].to_string().to_lowercase();
            assert!(limitations.contains("qualification"), "{limitations}");
        });
    }

    #[test]
    fn cancel_after_compile_keeps_both_artifacts() {
        bench(|ws| {
            let out = compiled(ws);
            let rec =
                cancel(ws, &str_of(&out, "ask_id"), Some("changed mind"), None, false).unwrap();
            assert_eq!(types(ws), ["ask.compiled", "ask.cancelled"]);
            assert_eq!(events(ws)[1]["data"]["reason"], "changed mind");
            assert_eq!(rec["cancellation"], json!({"observed_by": "skill"}));
            assert!(ws.rf_dir().join(str_of(&out["source"], "path")).exists());
            assert!(ws.rf_dir().join(str_of(&out["prompt"], "path")).exists());
        });
    }

    #[test]
    fn delivery_from_hook_records_native_accepted_once() {
        bench(|ws| {
            let out = compiled(ws);
            confirm(ws, &str_of(&out, "ask_id"), None).unwrap();
            let rec = delivery_from_hook(ws, &hook("s1", false, "EnterPlanMode")).unwrap().unwrap();
            assert_eq!(rec["state"], "native_accepted");
            assert_eq!(rec["ask_id"], out["ask_id"]);
            let last = events(ws).pop().unwrap();
            assert_eq!(last["type"], "ask.delivery");
            assert_eq!(last["data"]["receipt"]["tool_use_id"], "toolu_1");
            assert_eq!(last["data"]["receipt"]["captured_by"], "hook:PostToolUse");
            assert_eq!(last["data"]["qualification"]["id"], "ringframe-ask-plan-q04");
            // Already delivered: skip, and log why.
            assert!(delivery_from_hook(ws, &hook("s1", false, "EnterPlanMode")).unwrap().is_none());
            let skipped = std::fs::read_to_string(
                ws.rf_dir().join("sessions/claude-code/s1/delivery-skipped.jsonl"),
            )
            .unwrap();
            assert!(skipped.contains("no_candidate"), "{skipped}");
            assert_eq!(types(ws).iter().filter(|t| *t == "ask.delivery").count(), 1);
        });
    }

    #[test]
    fn delivery_from_hook_error_records_failed() {
        bench(|ws| {
            let out = compiled(ws);
            confirm(ws, &str_of(&out, "ask_id"), None).unwrap();
            let rec = delivery_from_hook(ws, &hook("s1", true, "EnterPlanMode")).unwrap().unwrap();
            assert_eq!(rec["state"], "delivery_failed");
        });
    }

    #[test]
    fn delivery_from_hook_ignores_other_sessions_tools_and_ambiguity() {
        bench(|ws| {
            let out = compiled(ws);
            confirm(ws, &str_of(&out, "ask_id"), None).unwrap();
            assert!(
                delivery_from_hook(ws, &hook("other", false, "EnterPlanMode")).unwrap().is_none()
            );
            assert!(delivery_from_hook(ws, &hook("s1", false, "Write")).unwrap().is_none());
            compile_with(
                ws,
                Args {
                    staged: Some(stage_named(ws, "prompt.txt", b"a", b"b")),
                    title: "second".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(delivery_from_hook(ws, &hook("s1", false, "EnterPlanMode")).unwrap().is_none());
            let skipped = std::fs::read_to_string(
                ws.rf_dir().join("sessions/claude-code/s1/delivery-skipped.jsonl"),
            )
            .unwrap();
            assert!(skipped.contains("ambiguous"), "{skipped}");
        });
    }

    #[test]
    fn handoff_emits_text_and_records_once() {
        bench(|ws| {
            let out = compiled(ws);
            let id = str_of(&out, "ask_id");
            confirm(ws, &id, None).unwrap();
            let (text, rec) = delivery_handoff(ws, &id).unwrap();
            let path = ws.rf_dir().join(str_of(&out["prompt"], "path"));
            assert!(text.contains(&path.to_string_lossy().to_string()), "{text}");
            assert!(text.contains("active Claude Code TUI"));
            assert_eq!(rec["state"], "handoff_ready");
            assert_eq!(events(ws).pop().unwrap()["data"]["submission"], "unobserved");
            let e = delivery_state(ws, &id, "unavailable", "x").unwrap_err();
            assert_eq!(ledger_error(e).code, "delivery.duplicate");
        });
    }

    #[test]
    fn show_and_resolve() {
        bench(|ws| {
            let a = compiled(ws);
            let a_id = str_of(&a, "ask_id");
            confirm(ws, &a_id, None).unwrap();
            let shown = show(ws, Some(&a_id), None).unwrap();
            assert_eq!(shown["title"], "Login fix");
            assert_eq!(shown["delivery"], json!(null));
            assert!(str_of(&shown, "prompt_path").ends_with("prompt.txt"));
            assert_eq!(show(ws, None, Some("s1")).unwrap()["ask_id"], a["ask_id"]);
            // A unique title substring, then unique in the workspace.
            assert_eq!(show(ws, Some("login"), None).unwrap()["ask_id"], a["ask_id"]);
            assert_eq!(show(ws, None, None).unwrap()["ask_id"], a["ask_id"]);

            let mut other = host();
            other["session_ref"] = json!("s2");
            let b = compile_with(
                ws,
                Args {
                    staged: Some(stage_named(ws, "prompt.txt", b"a", b"b")),
                    title: "Logout".into(),
                    capability: "native_direct".into(),
                    host: other,
                    ..Default::default()
                },
            )
            .unwrap();
            let AskError::Needs(e) = show(ws, None, None).unwrap_err() else {
                panic!("wanted a chooser")
            };
            let ids: BTreeSet<String> = e.candidates.iter().map(|c| str_of(c, "id")).collect();
            assert_eq!(ids, [str_of(&a, "ask_id"), str_of(&b, "ask_id")].into());
            assert!(matches!(show(ws, Some("Log"), None), Err(AskError::Needs(_))));
            let cands = resolve(ws, Some("s2"), None).unwrap();
            assert_eq!(
                cands["candidates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| str_of(c, "id"))
                    .collect::<Vec<_>>(),
                [str_of(&b, "ask_id")]
            );
            assert_eq!(cands["rule_applied"], "same_session");
        });
    }

    #[test]
    fn the_codex_handoff_has_a_prompt_prefix_and_a_length_policy() {
        bench(|ws| {
            let host =
                json!({"name": "codex", "version": "codex-cli 0.153.1", "surface": "native-tui"});
            let mut goal = cls();
            goal["result"] = json!("continuing_objective");
            goal["horizon"] = json!("persistent");
            let out = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"/goal Keep the checkout p95 under 800 ms.\n")),
                    capability: "native_goal".into(),
                    host: host.clone(),
                    classification: goal.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(out["delivery_mode"], "human_handoff");

            let e = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"Keep the checkout fast.\n")),
                    capability: "native_goal".into(),
                    host: host.clone(),
                    classification: goal.clone(),
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert_eq!(ledger_error(e).code, "ask.prompt_prefix");

            let mut long = b"/goal ".to_vec();
            long.extend(std::iter::repeat_n(b'x', 4000));
            long.push(b'\n');
            let e = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, &long)),
                    capability: "native_goal".into(),
                    host,
                    classification: goal,
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert_eq!(ledger_error(e).code, "ask.prompt_too_long");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    #[test]
    fn a_goal_too_long_for_claude_code_is_refused_before_anybody_submits_it() {
        bench(|ws| {
            let host = json!({"name": "claude-code", "version": "2.1.278",
                              "surface": "native-tui"});
            let mut goal = cls();
            goal["result"] = json!("continuing_objective");
            goal["horizon"] = json!("persistent");

            let mut long = b"/goal ".to_vec();
            long.extend(std::iter::repeat_n(b'x', 4000));
            long.push(b'\n');
            let e = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, &long)),
                    capability: "native_goal".into(),
                    host: host.clone(),
                    classification: goal.clone(),
                    ..Default::default()
                },
            )
            .unwrap_err();
            let e = ledger_error(e);
            assert_eq!(e.code, "ask.prompt_too_long");
            assert!(e.detail.contains("4000"), "the budget is named: {}", e.detail);
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());

            // One that fits still compiles.
            compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"/goal Keep the suite green.\n")),
                    capability: "native_goal".into(),
                    host,
                    classification: goal,
                    ..Default::default()
                },
            )
            .unwrap();
        });
    }

    #[test]
    fn a_non_human_actor_needs_authorization_to_compile_or_confirm() {
        bench(|ws| {
            let agent = json!({"kind": "agent", "id": "ci", "authority": "preauthorized"});
            let e = compile_with(ws, Args { actor: Some(agent.clone()), ..Default::default() })
                .unwrap_err();
            assert!(matches!(&e, AskError::Needs(n) if n.reason.contains("authorization")), "{e}");

            std::fs::create_dir_all(ws.rf_dir().join("authorizations")).unwrap();
            std::fs::write(
                ws.rf_dir().join("authorizations/ci.json"),
                serde_json::to_string(&json!({
                    "schema": "ringframe.authorization/1", "actor": "agent:ci",
                    "granted_by": "human:owner", "time": "t",
                    "allowed": {"capabilities": ["native_plan"], "effects": ["read"],
                                "dispositions": [], "eval_verdicts": [], "subject_kinds": []},
                    "expires": "2999-01-01T00:00:00Z"
                }))
                .unwrap(),
            )
            .unwrap();
            let out = compile_with(ws, Args { actor: Some(agent.clone()), ..Default::default() })
                .unwrap();
            confirm(ws, &str_of(&out, "ask_id"), Some(&agent)).unwrap();
            assert_eq!(
                events(ws)[0]["actor"],
                json!({"kind": "agent", "id": "ci",
                       "authority": "preauthorized:authorizations/ci.json"})
            );
            let mut question = cls();
            question["task"] = json!(["question"]);
            question["result"] = json!("answer");
            let e = compile_with(
                ws,
                Args {
                    actor: Some(agent),
                    capability: "native_direct".into(),
                    classification: question,
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert!(matches!(e, AskError::Needs(_)), "{e}");
        });
    }

    #[test]
    fn the_confirmation_surface_comes_from_the_profile_or_is_unknown() {
        bench(|ws| {
            // An unknown host must not acquire a native confirmation surface.
            let out = compile_with(
                ws,
                Args {
                    host: json!({"name": "unknown-host", "surface": "native-tui"}),
                    capability: "human_handoff".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let rec = confirm(ws, &str_of(&out, "ask_id"), None).unwrap();
            assert_eq!(rec["confirmation"]["surface"], json!(null));

            capture(ws, "codex", "cx", "$rf:ask fix the login bug", Some("codex-cli 0.153.4"));
            let mut writes = cls();
            writes["task"] = json!(["implement"]);
            writes["result"] = json!("workspace_change");
            writes["effects"] = json!(["write"]);
            let out2 = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"/plan Fix.\n")),
                    title: "second".into(),
                    host: json!({"name": "codex", "surface": "native-tui"}),
                    classification: writes,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                confirm(ws, &str_of(&out2, "ask_id"), None).unwrap()["confirmation"]["surface"],
                "request_user_input"
            );
        });
    }

    /// Each Ask comes from its own hook-identified Codex session: the hook,
    /// not the model, knows the session.
    fn codex_plan(ws: &Workspace, title: &str, prompt: &[u8]) -> Value {
        capture(
            ws,
            "codex",
            &format!("s-{title}"),
            "$rf:ask fix the login bug",
            Some("codex-cli 0.153.4"),
        );
        let mut writes = cls();
        writes["task"] = json!(["implement"]);
        writes["result"] = json!("workspace_change");
        writes["effects"] = json!(["write"]);
        compile_with(
            ws,
            Args {
                staged: Some(staged_prompt(ws, prompt)),
                title: title.into(),
                host: json!({"name": "codex", "version": "codex-cli 0.153.4",
                         "surface": "native-tui", "session_ref": format!("s-{title}")}),
                classification: writes,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn a_submission_matches_when_the_host_strips_its_slash_command_prefix() {
        bench(|ws| {
            let out = codex_plan(ws, "one", b"/plan Fix the login bug.\n");
            // Codex hands the UserPromptSubmit hook the text after "/plan ",
            // without the file's trailing newline.
            let rec = capture(ws, "codex", "s-one", "Fix the login bug.", None);
            let sub = submission_from_capture(ws, "codex", "s-one", &str_of(&rec, "sha256"))
                .unwrap()
                .unwrap();
            assert_eq!(sub["ask_id"], out["ask_id"]);
            assert_eq!(sub["match"], "host_prefix_stripped");
            assert_eq!(
                show(ws, Some(&str_of(&out, "ask_id")), None).unwrap()["submission"],
                "observed"
            );
        });
    }

    #[test]
    fn an_ambiguous_submission_prefers_the_delivered_ask_and_otherwise_records_nothing() {
        bench(|ws| {
            let first = codex_plan(ws, "one", b"/plan Fix the login bug.\n");
            let second = codex_plan(ws, "two", b"/plan Fix the login bug.\n");
            confirm(ws, &str_of(&second, "ask_id"), None).unwrap();
            delivery_handoff(ws, &str_of(&second, "ask_id")).unwrap();
            let rec = capture(ws, "codex", "s-two", "Fix the login bug.", None);
            let sub = submission_from_capture(ws, "codex", "s-two", &str_of(&rec, "sha256"))
                .unwrap()
                .unwrap();
            // The Ask that was handed off is the one a paste is expected for.
            assert_eq!(sub["ask_id"], second["ask_id"]);

            let third = codex_plan(ws, "three", b"/plan Fix the login bug.\n");
            for a in [&first, &third] {
                confirm(ws, &str_of(a, "ask_id"), None).unwrap();
                delivery_handoff(ws, &str_of(a, "ask_id")).unwrap();
            }
            let rec2 = capture(ws, "codex", "s-three", "Fix the login bug.", None);
            // Two delivered candidates: no attribution.
            assert!(
                submission_from_capture(ws, "codex", "s-three", &str_of(&rec2, "sha256"))
                    .unwrap()
                    .is_none()
            );
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    #[test]
    fn compile_renders_a_prompt_from_a_body_and_records_compiler_provenance() {
        bench(|ws| {
            capture(ws, "codex", "cg", "$rf:ask keep checkout fast", Some("codex-cli 0.153.4"));
            let d = stage_named(
                ws,
                "body.txt",
                b"keep checkout fast\n",
                b"Run plans/checkout-perf, every item in order.\n",
            );
            let cls = json!({"task": ["implement"], "result": "continuing_objective",
                             "interaction": "approval_gated", "horizon": "persistent",
                             "effects": ["write"], "concerns": ["performance"]});
            let out = compile_with(
                ws,
                Args {
                    staged: Some(d),
                    title: "Checkout goal".into(),
                    capability: "native_goal".into(),
                    host: json!({"name": "codex", "surface": "native-tui"}),
                    classification: cls,
                    ..Default::default()
                },
            )
            .unwrap();
            let text = prompt_text(ws, &str_of(&out, "ask_id")).unwrap();
            // Host prefix + body: the CLI added the prefix.
            assert!(
                text.starts_with("/goal Run plans/checkout-perf, every item in order.\n"),
                "{text}"
            );
            assert!(text.contains("\nRules:\n"), "{text}");
            // A labelled directive, selected by the performance concern.
            assert!(
                text.contains("- Knuth: Do not optimize on suspicion; measure first"),
                "{text}"
            );
            let ev = events(ws).into_iter().rfind(|e| e["type"] == "ask.compiled").unwrap();
            let comp = &ev["data"]["compiler"];
            assert_eq!(comp["source"], "body");
            assert_eq!(comp["host"]["deltas"], json!([]));
            assert!(list_of(&comp["practice"], "selected").contains(&"practice.knuth".to_string()));
            assert_eq!(comp["practice"]["matched_concerns"], json!(["performance"]));
            assert_eq!(str_of(&comp["practice"], "shipped_sha256").len(), 64);
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());

            // prompt.txt and body.txt together are ambiguous.
            let d2 = stage(ws);
            std::fs::write(d2.join("body.txt"), b"x\n").unwrap();
            let e = compile_with(ws, Args { staged: Some(d2), ..Default::default() }).unwrap_err();
            assert_eq!(ledger_error(e).code, "ask.staged_dir");
        });
    }

    #[test]
    fn compile_rejects_an_unknown_concern_before_writing() {
        bench(|ws| {
            let mut c = cls();
            c["concerns"] = json!(["telepathy"]);
            let e = compile_with(ws, Args { classification: c, ..Default::default() }).unwrap_err();
            assert_eq!(ledger_error(e).code, "ask.classification");
            assert_eq!(std::fs::read_dir(ws.rf_dir().join("asks")).unwrap().count(), 0);
        });
    }

    #[test]
    fn a_composed_prompt_records_the_cli_selection_not_the_models_claim() {
        bench(|ws| {
            capture(
                ws,
                "codex",
                "cc",
                "$rf:ask add the health endpoint",
                Some("codex-cli 0.153.4"),
            );
            let d = stage_named(
                ws,
                "composed.txt",
                b"add the health endpoint\n",
                b"Add GET /health returning uptime. Reuse the existing bearer check.\n\nRules:\n\
                  - Hyrum: keep every current endpoint's behaviour unchanged.\n\
                  - KISS, YAGNI: return only uptime; no options.\n",
            );
            let cls = json!({"task": ["implement"], "result": "workspace_change",
                             "interaction": "approval_gated", "horizon": "session",
                             "effects": ["write"], "concerns": ["api_surface"]});
            let out = compile_with(
                ws,
                Args {
                    staged: Some(d),
                    title: "Health".into(),
                    host: json!({"name": "codex", "surface": "native-tui"}),
                    classification: cls,
                    ..Default::default()
                },
            )
            .unwrap();
            let text = prompt_text(ws, &str_of(&out, "ask_id")).unwrap();
            // Prefix added, nothing appended.
            assert!(
                text.starts_with(
                    "/plan Add GET /health returning uptime. Reuse the existing bearer check.\n"
                ),
                "{text}"
            );
            assert!(text.trim_end_matches('\n').ends_with("no options."), "{text}");
            let ev = events(ws).into_iter().rfind(|e| e["type"] == "ask.compiled").unwrap();
            let comp = &ev["data"]["compiler"];
            assert_eq!(comp["source"], "composed");
            let selected = list_of(&comp["practice"], "selected");
            assert!(selected.contains(&"practice.hyrum".to_string()));
            assert!(selected.contains(&"practice.kiss".to_string()));
            assert_eq!(comp["practice"]["matched_concerns"], json!(["api_surface"]));
            assert_eq!(comp["host"]["deltas"], json!([]));
        });
    }

    fn composed_stage(ws: &Workspace, rules: &str) -> std::path::PathBuf {
        stage_named(
            ws,
            "composed.txt",
            b"add the health endpoint\n",
            format!(
                "Add GET /health/details returning uptime and version behind the existing bearer check.\n\nRules:\n{rules}"
            )
            .as_bytes(),
        )
    }

    #[test]
    fn composed_rules_are_audited_against_the_supplied_directives() {
        bench(|ws| {
            let cls = json!({"task": ["implement"], "result": "workspace_change",
                             "interaction": "approval_gated", "horizon": "session",
                             "effects": ["write"], "concerns": ["api_surface"]});
            let host = json!({"name": "codex", "version": "codex-cli 0.153.4",
                              "surface": "native-tui", "session_ref": "cx"});
            let good = composed_stage(
                ws,
                "- KISS, YAGNI: Expose only uptime and version; no new options or abstractions.\n\
                 - Hyrum: Keep every existing endpoint's observable behaviour unchanged.\n",
            );
            let out = compile_with(
                ws,
                Args {
                    staged: Some(good),
                    title: "Health".into(),
                    host: host.clone(),
                    classification: cls.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
            let ev = events(ws).into_iter().rfind(|e| e["type"] == "ask.compiled").unwrap();
            let comp = &ev["data"]["compiler"];
            assert_eq!(comp["source"], "composed");
            let applied: BTreeSet<String> = list_of(comp, "applied").into_iter().collect();
            assert_eq!(
                applied,
                ["practice.kiss".to_string(), "practice.yagni".into(), "practice.hyrum".into()]
                    .into()
            );
            let selected: BTreeSet<String> =
                list_of(&comp["practice"], "selected").into_iter().collect();
            let omitted: BTreeSet<String> = list_of(comp, "omitted").into_iter().collect();
            assert_eq!(omitted, selected.difference(&applied).cloned().collect());
            assert!(omitted.contains("practice.testing_pyramid"));
            assert!(prompt_text(ws, &str_of(&out, "ask_id")).unwrap().starts_with("/plan Add GET"));

            let bad =
                composed_stage(ws, "- KISS: Keep it small.\n- Telepathy: Read the user's mind.\n");
            let e = compile_with(
                ws,
                Args {
                    staged: Some(bad),
                    title: "Health".into(),
                    host: host.clone(),
                    classification: cls.clone(),
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert_eq!(ledger_error(e).code, "ask.composed_rules");

            let norules = stage_named(
                ws,
                "composed.txt",
                b"add the health endpoint\n",
                b"Add GET /health/details.\n",
            );
            let e = compile_with(
                ws,
                Args {
                    staged: Some(norules),
                    title: "Health".into(),
                    host,
                    classification: cls,
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert_eq!(ledger_error(e).code, "ask.composed_rules");
        });
    }

    #[test]
    fn ask_list_enumerates_every_compiled_ask_oldest_first() {
        bench(|ws| {
            assert!(list_asks(ws).unwrap().is_empty());
            let a = compiled(ws);
            let b = compile_with(
                ws,
                Args {
                    staged: Some(staged_prompt(ws, b"Other.\n")),
                    title: "Other".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            confirm(ws, &str_of(&b, "ask_id"), None).unwrap();
            let listed = list_asks(ws).unwrap();
            assert_eq!(
                listed.iter().map(|x| str_of(x, "ask_id")).collect::<Vec<_>>(),
                [str_of(&a, "ask_id"), str_of(&b, "ask_id")]
            );
            assert_eq!(listed[0]["outcome"], "compiled");
            assert_eq!(listed[1]["outcome"], "confirmed");
            assert_eq!(listed[1]["title"], "Other");
        });
    }

    #[test]
    fn compile_records_the_base_commit_and_asks_stay_open_until_sealed_or_cancelled() {
        bench(|ws| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&ws.root)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let a = compiled(ws);
            let b =
                compile_with(ws, Args { title: "Second".into(), ..Default::default() }).unwrap();
            assert_eq!(events(ws)[0]["data"]["base_commit"], head);
            let states = |ws: &Workspace| {
                list_asks(ws).unwrap().iter().map(|x| str_of(x, "state")).collect::<Vec<_>>()
            };
            assert_eq!(states(ws), ["open", "open"]);
            assert_eq!(
                open_asks(ws).unwrap().iter().map(|x| str_of(x, "ask_id")).collect::<Vec<_>>(),
                [str_of(&a, "ask_id"), str_of(&b, "ask_id")]
            );
            cancel(ws, &str_of(&b, "ask_id"), None, None, false).unwrap();
            assert_eq!(states(ws), ["open", "cancelled"]);
            assert_eq!(
                open_asks(ws).unwrap().iter().map(|x| str_of(x, "ask_id")).collect::<Vec<_>>(),
                [str_of(&a, "ask_id")]
            );
            // A seal event naming the Ask in its basis closes it.
            store::append(ws, &json!({
                "schema": store::SCHEMA, "event_id": ids::new_id("evt"), "type": "seal.created",
                "time": sessions::now(), "id": "sel_x",
                "actor": {"kind": "human", "id": "local-user"}, "links": [],
                "data": {
                    "basis": {"asks": [str_of(&a, "ask_id")]}, "eval": null,
                    "subject": {"kind": "git_commit", "ref": head}, "disposition": "accepted",
                    "authority": {"kind": "human", "id": "local-user", "authority": "interactive"},
                    "artifact": {"role": "seal_receipt", "path": "seals/sel_x.json", "bytes": 0,
                                 "sha256": "0".repeat(64)}
                }
            })).unwrap();
            assert_eq!(states(ws), ["sealed", "cancelled"]);
            assert!(open_asks(ws).unwrap().is_empty());
        });
    }

    #[test]
    fn compile_outside_git_is_refused() {
        // Git is a hard requirement: an Ask that could never be evaluated is
        // not recorded.
        with_config_home(|_| {
            let dir = crate::testing::tmp_dir();
            let ws = workspace::resolve(None, Some(dir.path())).unwrap();
            ws.ensure().unwrap();
            let e = compile_with(&ws, Args::default()).unwrap_err();
            let AskError::Workspace(e) = e else { panic!("wanted a workspace error") };
            assert_eq!(e.code, "workspace.no_git");
            assert!(events(&ws).is_empty());
        });
    }

    #[test]
    fn compile_keeps_the_observed_version_and_the_profile_digest() {
        // One workspace per version: the host carries no session_ref, so the
        // capture is what has to supply both it and the version, and two
        // captures of the same intent would be ambiguous.
        for version in [Some("codex-cli 1.0.0"), Some("development"), None] {
            bench(|ws| {
                capture(ws, "codex", "new", "$rf:ask fix the login bug", version);
                let out = compile_with(
                    ws,
                    Args {
                        staged: Some(staged_prompt(ws, b"/plan Fix.\n")),
                        host: json!({"name": "codex"}),
                        ..Default::default()
                    },
                )
                .unwrap();
                let recorded = events(ws)[0]["data"]["host"].clone();
                assert_eq!(recorded["version"], json!(version));
                assert_eq!(recorded["profile_id"], "codex");
                assert_eq!(recorded["profile_sha256"], profiles::sha256("codex").unwrap());
                assert_eq!(out["source_verified"], "exact");
                assert_eq!(
                    confirm(ws, &str_of(&out, "ask_id"), None).unwrap()["confirmation"]["surface"],
                    "request_user_input"
                );
            });
        }
    }

    #[test]
    fn codex_review_compiles_and_hands_off_from_the_profile() {
        bench(|ws| {
            let d = stage_named(
                ws,
                "body.txt",
                b"fix the login bug\n",
                b"Review the current diff for regressions.\n",
            );
            let mut review = cls();
            review["task"] = json!(["review"]);
            review["result"] = json!("evidence");
            let out = compile_with(
                ws,
                Args {
                    staged: Some(d),
                    capability: "native_review".into(),
                    host: json!({"name": "codex", "surface": "native-tui"}),
                    classification: review,
                    ..Default::default()
                },
            )
            .unwrap();
            let text = prompt_text(ws, &str_of(&out, "ask_id")).unwrap();
            assert!(text.starts_with("/review Review the current diff for regressions."), "{text}");
            confirm(ws, &str_of(&out, "ask_id"), None).unwrap();
            let (_, delivery) = delivery_handoff(ws, &str_of(&out, "ask_id")).unwrap();
            assert_eq!(delivery["state"], "handoff_ready");
            let ev = events(ws).into_iter().rfind(|e| e["type"] == "ask.compiled").unwrap();
            assert_eq!(ev["data"]["host"]["profile_sha256"], profiles::sha256("codex").unwrap());
            assert_eq!(ev["data"]["selected_capability"], "native_review");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    #[test]
    fn classification_normalization_keeps_domain_names_verbatim() {
        // `approval-gated` is normalized, but a domain name is a file name:
        // its hyphens are significant.
        let c = normalize_classification(
            &json!({"interaction": "approval-gated", "domains": ["autonomous-trading"]}),
        );
        assert_eq!(c["interaction"], "approval_gated");
        assert_eq!(c["domains"], json!(["autonomous-trading"]));
    }
}
