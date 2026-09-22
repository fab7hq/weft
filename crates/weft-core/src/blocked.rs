//! Reading "this agent is waiting for you" off the screen.
//!
//! This is the one thing Weft infers rather than reads from the record. It is
//! labelled as inference wherever it is shown, and the evidence behind it is
//! always available.
//!
//! Conservative on purpose: only a recognised approval, question, or
//! permission prompt counts. Anything unrecognised is not blocked, because a
//! false "blocked" stops Weft from typing when it could have.

/// Why Weft thinks a pane is waiting. Shown verbatim when asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub rule: &'static str,
    pub line: String,
}

/// Prompts that are waiting for a person, as the harnesses actually draw them.
const RULES: &[(&str, &str)] = &[
    ("selector", "❯ "),
    ("selector", "› 1."),
    ("selector", "› Yes"),
    ("confirm-hint", "Press enter to continue"),
    ("confirm-hint", "Enter to confirm"),
    ("permission", "Allow command?"),
    ("permission", "Do you want to proceed?"),
    ("trust", "Do you trust"),
    ("trust", "trust this folder"),
    ("update", "Update available"),
    ("hook-trust", "Press t to trust"),
];

/// How much of the bottom of the screen counts. A prompt lives at the bottom;
/// the same words scrolled up in transcript history do not mean anything.
///
/// Fourteen lines was too few. A chooser that carries a preview box beside it
/// puts its selector well above the last lines of a tall pane, and Weft missed
/// it — then typed into a pane that was waiting for its person, which is the
/// one thing it must never do. The window is now a share of the screen, so it
/// grows with the pane while still excluding what has scrolled away.
const TAIL_LINES: usize = 14;
const TAIL_SHARE: usize = 3; // three quarters of what is on screen

pub fn looks_blocked(screen: &str) -> Option<Evidence> {
    let lines: Vec<&str> = screen.lines().collect();
    let window = TAIL_LINES.max(lines.len() * TAIL_SHARE / 4);
    let start = lines.len().saturating_sub(window);
    for line in &lines[start..] {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        for (rule, needle) in RULES {
            if trimmed.contains(needle) {
                return Some(Evidence { rule, line: trimmed.to_string() });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from Claude Code 2.1.278 running inside a Weft pane.
    const CLAUDE_TRUST: &str = "\
 Quick safety check: Is this a project you created or one you trust?

 Claude Code'll be able to read, edit, and execute files here.

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel";

    /// Captured from codex-cli 0.154.0 running inside a Weft pane.
    const CODEX_TRUST: &str = "\
  Do you trust the contents of this directory? Working with untrusted contents
  comes with higher risk of prompt injection.

› 1. Yes, continue
  2. No, quit

  Press enter to continue";

    const CODEX_UPDATE: &str = "\
  ✨ Update available! 0.154.0 -> 0.155.1
› 1. Update now (runs `brew upgrade --cask codex`)
  2. Skip
  Press enter to continue";

    const CLAUDE_PERMISSION: &str = "\
 • Editing README.md

 ┌ Allow command? ───────────────────────────────┐
 │ npm test                                      │
 │  › Yes    Yes, and don't ask again    No      │
 └───────────────────────────────────────────────┘";

    const WORKING: &str = "\
 • Reading src/server/routes.ts
 • Editing src/server/routes.ts
 • Running npm test

   PASS  14 passed, 0 failed

 I added the endpoint and it returns the build number from package.json.

 ❯
 ⏵⏵ auto mode on (shift+tab to cycle) · esc to interrupt";

    #[test]
    fn the_claude_trust_prompt_is_recognised() {
        // Which rule fires first is an implementation detail; that the pane is
        // waiting, and that Weft can say why, is the contract.
        let e = looks_blocked(CLAUDE_TRUST).expect("blocked");
        assert!(!e.line.is_empty());
    }

    #[test]
    fn the_codex_trust_prompt_is_recognised() {
        assert!(looks_blocked(CODEX_TRUST).is_some());
    }

    #[test]
    fn the_codex_update_prompt_is_recognised() {
        // The one that let Weft type into a stalled agent before this existed.
        let e = looks_blocked(CODEX_UPDATE).expect("blocked");
        assert_eq!(e.rule, "update");
    }

    #[test]
    fn a_permission_dialog_is_recognised() {
        assert!(looks_blocked(CLAUDE_PERMISSION).is_some());
    }

    /// Captured from codex-cli 0.154.0: the rf plugin's UserPromptSubmit hook
    /// has to be trusted before it will run.
    const CODEX_HOOK_TRUST: &str = "\
  UserPromptSubmit      1           1           0           When the user submits a prompt
  Stop                  0           0           0           Right before Codex ends its turn
  Press t to trust all; enter to review hooks; esc to close";

    #[test]
    fn the_codex_hook_trust_dialog_is_recognised() {
        let e = looks_blocked(CODEX_HOOK_TRUST).expect("blocked");
        assert_eq!(e.rule, "hook-trust");
    }

    #[test]
    fn an_agent_that_is_merely_working_is_not_blocked() {
        assert_eq!(looks_blocked(WORKING), None);
    }

    #[test]
    fn an_empty_screen_is_not_blocked() {
        assert_eq!(looks_blocked(""), None);
    }

    /// Captured from Claude Code 2.1.278 in a Weft pane, asking the Ask
    /// skill's route question. The selector sits well above the bottom of a
    /// 40-row pane because a preview box is drawn beside it.
    const CLAUDE_ROUTE_QUESTION: &str = "\
⏺ Bash(ringframe ask copy --ask ask_01M2XNX85EJGC44B9MENM7A0E8)
  ⎿  Find the `health()` function in this codebase, which currently reports
      a hardcoded literal string \"dev\" as the package version.
     … +18 lines (ctrl+o to expand)
⏺ Now confirming the route with you.
────────────────────────────────────────────────────────────────
 ☐ Ask route
How should I proceed with the health() version-reporting fix?
❯ 1. Proceed (Recommended)        ┌──────────────────────────────┐
  2. Direct execution             │ Find the health() function   │
  3. Cancel                       │ in this codebase, which      │
                                  │ currently reports a          │
                                  │ hardcoded literal string     │
                                  │ \"dev\" as the package        │
                                  │ version. Change it to        │
                                  │ report the real package      │
                                  │ version instead — read it    │
                                  │ from wherever the package's  │
                                  │ canonical version is         │
                                  │ defined, e.g. package        │
                                  │ metadata, a version file,    │
                                  ├─── ✂ ─── 31 lines hidden ────┤
                                  └──────────────────────────────┘";

    #[test]
    fn a_chooser_with_a_preview_beside_it_is_still_a_chooser() {
        // Found by a W4 run: the selector was twelve rows above the bottom of
        // a forty-row pane, the window was fourteen lines, and Weft typed into
        // a pane that was waiting for its person.
        let evidence = looks_blocked(CLAUDE_ROUTE_QUESTION).expect("a chooser");
        assert_eq!(evidence.rule, "selector");
        assert!(evidence.line.contains("1. Proceed"), "{}", evidence.line);
    }

    #[test]
    fn the_words_scrolled_far_up_do_not_count() {
        let mut screen = String::from("❯ Yes, I trust this folder\n");
        for i in 0..40 {
            screen.push_str(&format!("line {i}\n"));
        }
        assert_eq!(looks_blocked(&screen), None, "only the bottom of the screen counts");
    }

    #[test]
    fn the_evidence_names_the_line_it_matched() {
        let e = looks_blocked(CODEX_UPDATE).expect("blocked");
        assert!(e.line.contains("Update available"), "got {:?}", e.line);
    }
}
