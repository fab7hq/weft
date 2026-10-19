//! "Start at login", Weft's own option: kept in `config.toml`
//! under `[service]`, and the service written or removed when a window turns
//! it on or off. Remote never changes it.

use super::*;

impl Session {
    /// Whether `config.toml` says Weft starts at login.
    pub(super) fn start_at_login(&self) -> bool {
        crate::routing::read_at(&self.config, &Default::default()).start_at_login()
    }

    /// `service.login {on, program, path}` from a window: the service first,
    /// then the option, so the file never says on while nothing would start.
    pub(super) fn turn_start_at_login(&mut self, on: bool, program: &str, path: &str) -> Answer {
        let Some(service) = self.outside.service() else {
            return Answer::No("no_service", "Weft has no login service on this system".into());
        };
        let managed = crate::service::managed();
        let done = if on {
            if program.is_empty() {
                return Answer::No("bad_request", "which weft to start was not said".into());
            }
            service.install(Path::new(program), path, managed)
        } else {
            service.remove(managed)
        };
        if let Err(why) = done {
            return Answer::No("failed", why);
        }
        let text = std::fs::read_to_string(&self.config).unwrap_or_default();
        if let Err(why) = weft_core::config::set_start_at_login(&text, on) {
            return Answer::No("config", why);
        }
        let changed =
            crate::routing::change(&self.config, |t| weft_core::config::set_start_at_login(t, on));
        if let Err(why) = changed {
            return Answer::No("failed", why);
        }
        self.clients.broadcast_all(&Out::StartAtLogin { on });
        Answer::Ok(serde_json::json!({"start_at_login": on}))
    }
}
