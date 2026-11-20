//! What an Eval would read, as its dialog says it, and which files an Eval
//! should skip: RingFrame's preview of a work's range and its read check,
//! asked off the run loop and kept so the dialog can be said again once a
//! line is written.

use std::path::Path;

use serde_json::{Value, json};
use weft_core::skip::{Finding, Fix};

/// One check: the project's findings, and for each work asked about, its
/// preview (none when RingFrame could not say) and its findings.
#[derive(Debug, Default)]
pub struct Checked {
    pub project: Vec<Finding>,
    pub works: Vec<(String, Option<Value>, Vec<Finding>)>,
}

/// Check the project at `root`, and the work of each Ask of `asks`. Each
/// work's check also looks at the whole project: the first one's is kept.
pub fn check(root: &Path, asks: &[String]) -> Checked {
    let mut out = Checked::default();
    if asks.is_empty() {
        out.project = crate::ringframe::read_check(root, None).0;
        return out;
    }
    for (i, ask) in asks.iter().enumerate() {
        let preview = crate::ringframe::eval_preview(root, ask).ok();
        let (project, work) = crate::ringframe::read_check(root, Some(ask));
        if i == 0 {
            out.project = project;
        }
        out.works.push((ask.clone(), preview, work));
    }
    out
}

/// What an Eval dialog says it would read, and the findings in its work's
/// range, kept while the dialog is open.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalReads {
    /// An Ask of the work the dialog is of.
    pub ask: String,
    /// RingFrame's preview, as `eval_preview` says it; none when it could
    /// not say.
    pub preview: Option<Value>,
    /// How many files the range held when the dialog was made.
    pub files_before: Option<u64>,
    /// The work's findings, as the latest check found them.
    pub findings: Vec<Finding>,
    /// The findings whose line was written while the dialog was open, as
    /// they were found.
    pub added: Vec<Finding>,
}

impl EvalReads {
    pub fn new(ask: &str, preview: Option<Value>, findings: Vec<Finding>) -> Self {
        let files_before = preview.as_ref().and_then(|p| p["files"].as_u64());
        EvalReads { ask: ask.to_string(), preview, files_before, findings, added: Vec::new() }
    }

    /// The dialog's payload: `{"reads": {<preview>, findings, files_before?}}`,
    /// where `findings` are those already added (`added: true`), then the
    /// rest, and `files_before` is said only once the count has moved.
    /// Empty when RingFrame could not say what it would read.
    pub fn payload(&self) -> Vec<u8> {
        let Some(mut reads) = self.preview.clone() else { return Vec::new() };
        let findings: Vec<&Finding> = self.added.iter().chain(&self.findings).collect();
        reads["findings"] = serde_json::to_value(findings).unwrap_or_default();
        if let Some(before) = self.files_before.filter(|b| Some(*b) != reads["files"].as_u64()) {
            reads["files_before"] = json!(before);
        }
        json!({ "reads": reads }).to_string().into_bytes()
    }

    /// Whether the latest check proposed this line for the work.
    pub fn proposes(&self, fix: &Fix) -> bool {
        self.findings.iter().any(|f| f.fix.as_ref() == Some(fix))
    }

    /// The line was written: the findings it keeps out are added.
    pub fn took(&mut self, fix: &Fix) {
        let (taken, kept): (Vec<Finding>, Vec<Finding>) = std::mem::take(&mut self.findings)
            .into_iter()
            .partition(|f| f.fix.as_ref() == Some(fix));
        self.findings = kept;
        self.added.extend(taken.into_iter().map(|f| Finding { added: true, ..f }));
    }

    /// The work checked again: what it would read now, and what is still
    /// found, but for what was added.
    pub fn checked(&mut self, preview: Option<Value>, findings: Vec<Finding>) {
        if preview.is_some() {
            self.preview = preview;
        }
        let added: Vec<&str> = self.added.iter().map(|f| f.path.as_str()).collect();
        self.findings =
            findings.into_iter().filter(|f| !added.contains(&f.path.as_str())).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(kind: &str, path: &str, files: usize, fix: Option<(&str, &str)>) -> Finding {
        Finding {
            kind: kind.into(),
            path: path.into(),
            files,
            bytes: 2 << 20,
            commits: 0,
            kinds_of_files: Vec::new(),
            fix: fix.map(|(file, line)| Fix { file: file.into(), line: line.into() }),
            added: false,
        }
    }

    fn said(r: &EvalReads) -> Value {
        serde_json::from_slice::<Value>(&r.payload()).unwrap()["reads"].clone()
    }

    /// A written line turns its finding into one added, said first; the
    /// count of files moves, and what it was is said beside it.
    #[test]
    fn a_line_written_is_said_added_and_the_files_before_it() {
        let output = finding("untracked_bulk", "output/", 329, Some((".gitignore", "/output/")));
        let data = finding(
            "data_bulk",
            "evidence/",
            13708,
            Some((".gitattributes", "evidence/** linguist-generated=true")),
        );
        let sub = finding("sub_repo", "homepage/", 12, None);
        let preview = json!({"since": "ask", "files": 14181, "commits": 52});
        let mut r =
            EvalReads::new("ask_1", Some(preview), vec![output.clone(), data.clone(), sub.clone()]);
        let first = said(&r);
        assert_eq!(first["files"], 14181);
        assert_eq!(first["files_before"], Value::Null, "said only once it moved");
        assert_eq!(first["findings"].as_array().unwrap().len(), 3);

        let line = Fix { file: ".gitignore".into(), line: "/output/".into() };
        assert!(r.proposes(&line));
        assert!(!r.proposes(&Fix { file: ".gitignore".into(), line: "/src/".into() }));
        r.took(&line);
        // Checked again, the folder ignored is found no more, and if it were
        // it is still said once, as added.
        r.checked(
            Some(json!({"since": "ask", "files": 13852, "commits": 52})),
            vec![output.clone(), data.clone(), sub.clone()],
        );
        let after = said(&r);
        assert_eq!(
            (after["files"].clone(), after["files_before"].clone()),
            (json!(13852), json!(14181))
        );
        let paths: Vec<(&str, bool)> = after["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["path"].as_str().unwrap(), f["added"].as_bool().unwrap_or(false)))
            .collect();
        assert_eq!(paths, [("output/", true), ("evidence/", false), ("homepage/", false)]);
        assert!(!r.proposes(&line), "a line written is proposed no more");
    }

    #[test]
    fn a_dialog_ringframe_could_not_preview_carries_nothing() {
        assert!(EvalReads::new("ask_1", None, Vec::new()).payload().is_empty());
    }
}
