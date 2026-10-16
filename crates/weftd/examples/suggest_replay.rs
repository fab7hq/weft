//! Replay this machine's own transcripts through Weft's suggestions and say
//! how they would have done: for the owner to set the bar they ship at.
//!
//! Every transcript the installed harness files find is read, the person's
//! messages taken in the order they were sent, and each suggested from those
//! before it only. Only numbers are printed; no message leaves this program.
//!
//! cargo run --release -p weftd --example suggest_replay -- [harnesses dir] [Sense bundle dir]

use std::path::{Path, PathBuf};
use std::time::Instant;

use weft_core::suggest::{self, Entry, Index};
use weftd::harness::OnThisMachine;

/// The confidence floors each score is reported at.
const FLOORS: [f64; 5] = [0.0, 0.3, 0.5, 0.7, 0.9];

fn main() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let mut args = std::env::args().skip(1);
    let harnesses_dir = args.next().map_or_else(|| home.join(".fab7/weft/harnesses"), PathBuf::from);
    let bundle = args.next().map(PathBuf::from);
    let (harnesses, _) = weftd::harness::installed(&harnesses_dir);

    let mut entries: Vec<Entry> = Vec::new();
    let mut files = 0;
    let read_started = Instant::now();
    for h in harnesses.iter() {
        let Some(mapping) = h.conversation.as_ref() else { continue };
        let Some(find) = mapping.find.as_deref() else { continue };
        let home = h.config_home();
        for path in matching(&home, &find.replace("{session}", "*")) {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Ok(read) = mapping.read(&bytes, 0) else { continue };
            files += 1;
            let session = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            // A transcript's folder stands for its project: Claude Code keeps one per project.
            let project = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let at = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            entries.extend(suggest::from_conversation(&read.items, &h.name, &project, &session, at));
        }
    }
    let read_secs = read_started.elapsed().as_secs_f64();
    entries.retain(|e| suggest::keep(&e.text));

    // Each reply embedded by Sense, as the daemon does in the background.
    let model = bundle.as_deref().and_then(|b| weft_sense::SenseModel::load(b).ok());
    let embed_started = Instant::now();
    if let Some(m) = &model {
        for e in entries.iter_mut().filter(|e| !e.reply.trim().is_empty()) {
            e.near = m.embed(&e.reply);
        }
    }
    let embed_secs = embed_started.elapsed().as_secs_f64();

    let replay_started = Instant::now();
    let (next, rest) = suggest::replay(&entries, &FLOORS);
    let replay_secs = replay_started.elapsed().as_secs_f64();

    // How fast a completion is with everything indexed: the index built and
    // asked, as the daemon does for each pane.suggest.
    let mut times: Vec<f64> = Vec::new();
    for e in entries.iter().step_by((entries.len() / 200).max(1)) {
        let draft: String = e.text.chars().take(e.text.chars().count() / 3).collect();
        if draft.trim().is_empty() {
            continue;
        }
        let t = Instant::now();
        let index = Index::new(&entries, &e.project);
        let _ = index.complete(&draft, &e.harness, e.at);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let p95 = times.get(times.len() * 95 / 100).copied().unwrap_or_default();

    let kept = suggest::Stored::new("replay", entries.clone()).to_json().len();
    let projects: std::collections::BTreeSet<&str> = entries.iter().map(|e| e.project.as_str()).collect();
    println!("Transcripts read: {files}, in {read_secs:.1} s");
    println!("Messages kept: {} in {} projects", entries.len(), projects.len());
    println!(
        "Embedded: {}",
        if model.is_some() { format!("yes, in {embed_secs:.1} s") } else { "no (BM25 only)".into() }
    );
    println!("Replay: {replay_secs:.1} s");
    println!("Completion speed: p95 {p95:.1} ms over {} asks, the whole index each time", times.len());
    println!("Index memory: {:.1} MB as kept on disk", kept as f64 / 1e6);
    for (name, scores) in [("Next prompt (empty field)", &next), ("Rest of the message (from its first third)", &rest)] {
        println!("\n{name}: {} messages", scores[0].eligible);
        println!("| floor | shown | coverage | hit rate | wrong rate |");
        println!("| --- | --- | --- | --- | --- |");
        for (floor, s) in FLOORS.iter().zip(scores.iter()) {
            println!(
                "| {floor:.1} | {} | {:.1}% | {:.1}% | {:.1}% |",
                s.shown,
                s.coverage() * 100.0,
                s.hit_rate() * 100.0,
                s.wrong_rate() * 100.0
            );
        }
    }
}

/// Every file under `home` whose path matches `pattern`, its parts holding `*`.
fn matching(home: &Path, pattern: &str) -> Vec<PathBuf> {
    let parts: Vec<&str> = pattern.split('/').filter(|p| !p.is_empty()).collect();
    let mut found = vec![home.to_path_buf()];
    for (n, part) in parts.iter().enumerate() {
        let last = n + 1 == parts.len();
        found = found
            .iter()
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(|name| glob(part, name)))
            .filter(|e| e.file_type().is_ok_and(|t| if last { t.is_file() } else { t.is_dir() }))
            .map(|e| e.path())
            .collect();
    }
    found
}

/// Whether `name` matches `pattern`, where `*` stands for any run of characters.
fn glob(pattern: &str, name: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    if pieces.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (pieces[0], pieces[pieces.len() - 1]);
    if !name.starts_with(first) || !name.ends_with(last) || name.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for piece in &pieces[1..pieces.len() - 1] {
        match rest.find(piece) {
            Some(i) => rest = &rest[i + piece.len()..],
            None => return false,
        }
    }
    true
}
