//! Weft's own "Start at login" in the TUI (cards T13.2 and
//! T13.3): turned on or off in place from the Weft menu, or turned on from
//! the Remote view. Nothing turns it on for the person.

use super::*;

impl App {
    /// The Weft menu's item `at`: "Start at login" turns in place and the
    /// menu stays open on it; every other item closes the menu first.
    pub(super) fn menu_act(&mut self, at: usize) {
        let act = WEFT_MENU[at].2;
        if act == Act::StartAtLogin {
            self.modal_choice = at;
        } else {
            self.modal = None;
        }
        self.act(act);
    }

    /// Ask the daemon to turn the option the other way. The service runs the
    /// `weft` this window's `PATH` finds, so an update that moves the link
    /// is the one it starts; else this program.
    pub(super) fn toggle_start_at_login(&mut self) {
        let on = !sess!(self).start_at_login;
        let path = std::env::var("PATH").unwrap_or_default();
        let program = weft_on(&path)
            .or_else(|| std::env::current_exe().ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        match sess!(self).set_start_at_login(on, &program, &path) {
            Ok(id) => self.await_answer(id, Then::StartAtLogin { on }),
            Err(e) => self.say(could_not(on, &e.to_string())),
        }
    }

    /// The daemon's answer: every window is told the option by its event as
    /// well; this one also says why, when it did not change.
    pub(super) fn start_at_login_turned(
        &mut self,
        at: usize,
        on: bool,
        answer: std::result::Result<serde_json::Value, String>,
    ) {
        match answer {
            Ok(v) => {
                self.projects[at].session.start_at_login =
                    v.get("start_at_login").and_then(|t| t.as_bool()).unwrap_or(on);
            }
            Err(e) => self.say(could_not(on, &said(&e))),
        }
    }
}

fn could_not(on: bool, why: &str) -> String {
    format!("Could not turn {} Start at login: {why}", if on { "on" } else { "off" })
}

/// The first `weft` on `path` that can be run, as it is named there: a link
/// stays a link.
fn weft_on(path: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    std::env::split_paths(path).map(|dir| dir.join("weft")).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weft_on_path_is_the_first_that_runs_and_keeps_its_link() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = std::env::temp_dir().join(format!("weft-on-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b, c) = (root.join("a"), root.join("b"), root.join("c"));
        for d in [&a, &b, &c] {
            std::fs::create_dir_all(d).unwrap();
        }
        // Not runnable, then a link to a runnable one.
        std::fs::write(a.join("weft"), "").unwrap();
        std::fs::write(c.join("real"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(c.join("real"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(c.join("real"), b.join("weft")).unwrap();
        let path = std::env::join_paths([&a, &b, &c]).unwrap();
        assert_eq!(weft_on(&path.to_string_lossy()), Some(b.join("weft")));
        assert_eq!(weft_on(&a.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
