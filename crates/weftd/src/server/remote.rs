//! Keeping `weft remote` running while remote access is turned on (remote.md
//! D9, D10): started through the `Outside` seam when the daemon starts
//! serving, and again with a 2, 4, 8 … 60 second backoff whenever it ends.
//! Turned on and off from a client; the choice is kept in `config.toml`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::*;

/// The first wait before starting it again, and the longest.
const FIRST_WAIT: Duration = Duration::from_secs(2);
const LONGEST_WAIT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(super) struct Supervisor {
    /// Remote access is turned on.
    on: Arc<AtomicBool>,
    /// A thread is keeping it running.
    running: Arc<AtomicBool>,
}

impl Session {
    /// Turned on in `config.toml` when the daemon starts: start it.
    pub(super) fn remote_from_config(&mut self) {
        let config = crate::routing::read_at(&self.config, &Default::default());
        if config.remote().enabled {
            self.supervisor.on.store(true, Ordering::SeqCst);
            self.keep_remote_running();
        }
    }

    /// `remote {act: "on" | "off"}` from a client: kept in `config.toml`, and
    /// `weft remote` started, or told to stop. Any other act is its own.
    pub(super) fn turn_remote(&mut self, on: bool) -> Answer {
        let text = std::fs::read_to_string(&self.config).unwrap_or_default();
        match weft_core::config::set_remote(&text, on) {
            Ok(text) => {
                if let Some(dir) = self.config.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&self.config, text) {
                    return Answer::No("failed", format!("config.toml was not written: {e}"));
                }
            }
            Err(why) => return Answer::No("config", why),
        }
        self.supervisor.on.store(on, Ordering::SeqCst);
        let publisher = self.clients.remote.as_ref().map(|(to, _)| *to);
        if on {
            self.keep_remote_running();
        } else if let Some(to) = publisher {
            let stop = Out::RemoteAct { act: serde_json::json!({"act": "stop"}) };
            self.clients.send_to(to, &Line::Event(stop));
        }
        // Until `weft remote` says otherwise, the view is the daemon's: it is
        // starting, or it is off and its last view is no longer true.
        let view = serde_json::json!({"state": if on { "starting" } else { "off" }});
        self.clients.remote = Some((publisher.unwrap_or(u64::MAX), view.clone()));
        self.clients.broadcast_all(&Out::Remote { view });
        Answer::nothing()
    }

    /// Whether remote access is turned on.
    pub(super) fn remote_wanted(&self) -> bool {
        self.supervisor.on.load(Ordering::SeqCst)
    }

    fn keep_remote_running(&mut self) {
        if self.supervisor.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let (on, running) = (self.supervisor.on.clone(), self.supervisor.running.clone());
        let out = self.outside.clone();
        std::thread::spawn(move || {
            let mut wait = FIRST_WAIT;
            while on.load(Ordering::SeqCst) {
                let started = Instant::now();
                let _ = out.remote();
                if !on.load(Ordering::SeqCst) {
                    break;
                }
                // A run that lasted is not a crash loop: start the count again.
                if started.elapsed() >= LONGEST_WAIT {
                    wait = FIRST_WAIT;
                }
                std::thread::sleep(wait);
                wait = (wait * 2).min(LONGEST_WAIT);
            }
            running.store(false, Ordering::SeqCst);
        });
    }
}
