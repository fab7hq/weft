//! Running the commands that set a harness up or catch it up.

use weft_core::sync::{Mark, Step};

use crate::outside::Outside;

/// Run each step in order, showing every change, and stop at the first that
/// fails, unless that step may fail: then it is done, with what it said kept,
/// and the next runs. A zero exit proves nothing, so the caller looks again
/// afterwards.
pub fn proceed(out: &dyn Outside, steps: &mut [Step], mut show: impl FnMut(&[Step])) {
    for i in 0..steps.len() {
        steps[i].mark = Mark::Running;
        show(steps);
        let step = &mut steps[i];
        let args: Vec<&str> = step.args.iter().map(String::as_str).collect();
        match out.run(&step.program, &args, None) {
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
            if !step.may_fail {
                break;
            }
            step.mark = Mark::Done;
        }
    }
    show(steps);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(program: &str) -> Step {
        Step::new(program, &[] as &[&str])
    }

    #[test]
    fn a_failure_stops_the_run_and_what_follows_stays_waiting() {
        let mut steps = vec![step("true"), step("false"), step("true")];
        let mut shown = 0;
        proceed(&crate::outside::Machine, &mut steps, |_| shown += 1);
        let marks: Vec<Mark> = steps.iter().map(|s| s.mark).collect();
        assert_eq!(marks, [Mark::Done, Mark::Failed, Mark::Waiting]);
        assert_eq!(shown, 3, "each start, then the end");
    }

    #[test]
    fn a_step_that_may_fail_lets_the_next_one_run() {
        let mut steps = vec![step("false").may_fail(), step("true")];
        proceed(&crate::outside::Machine, &mut steps, |_| {});
        let marks: Vec<Mark> = steps.iter().map(|s| s.mark).collect();
        assert_eq!(marks, [Mark::Done, Mark::Done], "the install still ran");
        let mut steps = vec![step("false").may_fail(), step("false")];
        proceed(&crate::outside::Machine, &mut steps, |_| {});
        let marks: Vec<Mark> = steps.iter().map(|s| s.mark).collect();
        assert_eq!(marks, [Mark::Done, Mark::Failed], "and its own failure is the one that counts");
    }
}
