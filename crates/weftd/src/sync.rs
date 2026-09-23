//! The RingFrame view's two jobs that need a process: asking what is behind,
//! and running the commands that catch it up.

use std::process::Command;

use serde_json::Value;
use weft_core::readiness::Readiness;
use weft_core::sync::{Mark, View, view};

use crate::harness::{OnThisMachine, SUPPORTED};

/// What the view shows, and each harness's readiness as found on the way.
pub fn look() -> (View, Vec<(String, Readiness)>) {
    let check = Command::new("ringframe")
        .args(["sync", "--check"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok());
    let cli = crate::ringframe::installed();
    let found: Vec<_> = SUPPORTED
        .iter()
        .filter(|h| h.on_path())
        .map(|h| {
            let (state, version) = crate::readiness::look(h, cli);
            (h, state, version)
        })
        .collect();
    let states = found.iter().map(|(h, s, _)| (h.name.to_string(), *s)).collect();
    (view(check.as_ref(), &found), states)
}

/// Run each step in order, showing every change, and stop at the first that
/// fails. A zero exit proves nothing, so the caller looks again afterwards.
pub fn proceed(v: &mut View, mut show: impl FnMut(&View)) {
    v.running = true;
    for i in 0..v.steps.len() {
        v.steps[i].mark = Mark::Running;
        show(v);
        let step = &mut v.steps[i];
        match Command::new(&step.program).args(&step.args).output() {
            Ok(o) if o.status.success() => step.mark = Mark::Done,
            Ok(o) => {
                step.mark = Mark::Failed;
                let said = if o.stderr.is_empty() { o.stdout } else { o.stderr };
                let said = String::from_utf8_lossy(&said);
                let lines: Vec<&str> = said.trim().lines().collect();
                step.said = lines[lines.len().saturating_sub(3)..].join("\n");
            }
            Err(e) => {
                step.mark = Mark::Failed;
                step.said = e.to_string();
            }
        }
        if step.mark == Mark::Failed {
            break;
        }
    }
    v.running = false;
    show(v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use weft_core::sync::Step;

    fn step(program: &str) -> Step {
        Step { program: program.into(), args: vec![], mark: Mark::Waiting, said: String::new() }
    }

    #[test]
    fn a_failure_stops_the_run_and_what_follows_stays_waiting() {
        let mut v =
            View { steps: vec![step("true"), step("false"), step("true")], ..View::default() };
        let mut shown = 0;
        proceed(&mut v, |_| shown += 1);
        let marks: Vec<Mark> = v.steps.iter().map(|s| s.mark).collect();
        assert_eq!(marks, [Mark::Done, Mark::Failed, Mark::Waiting]);
        assert!(!v.running);
        assert_eq!(shown, 3, "each start, then the end");
    }
}
