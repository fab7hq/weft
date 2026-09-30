//! Running the commands that set a harness up or catch it up.

use std::process::Command;

use weft_core::sync::{Mark, Step};

/// Run each step in order, showing every change, and stop at the first that
/// fails. A zero exit proves nothing, so the caller looks again afterwards.
pub fn proceed(steps: &mut [Step], mut show: impl FnMut(&[Step])) {
    for i in 0..steps.len() {
        steps[i].mark = Mark::Running;
        show(steps);
        let step = &mut steps[i];
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
    show(steps);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(program: &str) -> Step {
        Step { program: program.into(), args: vec![], mark: Mark::Waiting, said: String::new() }
    }

    #[test]
    fn a_failure_stops_the_run_and_what_follows_stays_waiting() {
        let mut steps = vec![step("true"), step("false"), step("true")];
        let mut shown = 0;
        proceed(&mut steps, |_| shown += 1);
        let marks: Vec<Mark> = steps.iter().map(|s| s.mark).collect();
        assert_eq!(marks, [Mark::Done, Mark::Failed, Mark::Waiting]);
        assert_eq!(shown, 3, "each start, then the end");
    }
}
