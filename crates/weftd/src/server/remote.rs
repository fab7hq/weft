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
    /// Turning it on, told as a job until the address answers, and whether
    /// it signed in on the way.
    turning_on: Option<(jobs::Handle, bool)>,
}

/// Turning Remote on, step by step: starting Weft's remote, signing in,
/// starting the tunnel, waiting until the address answers.
const STARTING: usize = 0;
const SIGNING_IN: usize = 1;
const TUNNEL: usize = 2;
const ADDRESS: usize = 3;

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
    pub(super) fn turn_remote(&mut self, on: bool, asked: (u64, u64)) -> Answer {
        let text = std::fs::read_to_string(&self.config).unwrap_or_default();
        if let Err(why) = weft_core::config::set_remote(&text, on) {
            return Answer::No("config", why);
        }
        if let Err(why) =
            crate::routing::change(&self.config, |t| weft_core::config::set_remote(t, on))
        {
            return Answer::No("failed", why);
        }
        self.supervisor.on.store(on, Ordering::SeqCst);
        if let Some((job, _)) = self.supervisor.turning_on.take() {
            job.end(Err("turned off".into()));
        }
        if on {
            let steps = vec![
                jobs::step("step.starting_remote", &[], None),
                jobs::step("step.signing_in", &[], None),
                jobs::step("step.starting_tunnel", &[], None),
                jobs::step("step.waiting_address", &[], None),
            ];
            let job = self.jobs.begin("remote", Default::default(), None, Some(asked), steps);
            job.now(STARTING);
            self.supervisor.turning_on = Some((job, false));
        }
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

    /// `weft remote` said where it is: turning it on moves on a step, or
    /// ends once the address answers, or when it needs the person instead.
    pub(super) fn remote_moved(&mut self, view: &serde_json::Value) {
        let Some((job, signed)) = &mut self.supervisor.turning_on else { return };
        let state = view["state"].as_str().unwrap_or_default();
        let ended = match state {
            "starting" => {
                job.now(STARTING);
                None
            }
            "signing_in" | "signed_out" => {
                *signed = true;
                job.now(SIGNING_IN);
                None
            }
            "on" => {
                if !*signed {
                    // Already signed in: there was nothing to sign in to.
                    job.skip(SIGNING_IN);
                    *signed = true;
                }
                match (view["tunnel"].as_str(), view["answers"] == true) {
                    (Some("up"), true) => Some(Ok(())),
                    (Some("up"), false) => {
                        job.now(ADDRESS);
                        None
                    }
                    _ => {
                        job.now(TUNNEL);
                        None
                    }
                }
            }
            // A tunnel to choose, or one to install: the person's to do.
            "choose" | "not_installed" => Some(Err(state.to_string())),
            _ => None,
        };
        if let Some(result) = ended {
            job.end(result);
            self.supervisor.turning_on = None;
        }
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
