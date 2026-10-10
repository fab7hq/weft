//! The recovery targets (remote.md §11a, ADR-0034 D6), each a scenario on a
//! clock of the test's own: the machine's own rules (`Recovery`, the tunnel's
//! `Backoff` and its time to give an address) against a modelled tunnel
//! program, Cloudflare, the network and Weft's service. A target missed is a
//! failed test.
//!
//! "The phone works" is the machine's side done: the service holds an address
//! that Cloudflare carries to this machine's running tunnel. The phone then
//! has `PHONE` to reach it (it asks the service again at least every 10 s,
//! web `machine.ts`), so the machine's budget is each target less that.
//! What a real tunnel and network do is Gate W-durable's (HostLab).

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime};

use weft_remote::account::Probe;
use weft_remote::health::{Act, Recovery};
use weft_remote::tunnel::{ADDRESS_WITHIN, Backoff};

const PHONE: Duration = Duration::from_secs(10);
/// A tunnel's program gives its address this long after it starts.
const ADDRESS_AFTER: Duration = Duration::from_secs(3);
/// A look or a report is answered this long after it is made.
const ANSWER: Duration = Duration::from_secs(1);
/// weftd starts `weft remote` again this long after it crashes (its first
/// wait, `server/remote.rs`; `the_daemon_keeps_weft_remote_running…`).
const REMOTE_AGAIN: Duration = Duration::from_secs(2);
/// launchd starts a crashed agent again within this (its throttle); systemd
/// with `RestartSec=2`, sooner. The platform's, not tested here (D7).
const SERVICE_AGAIN: Duration = Duration::from_secs(10);

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// What the tunnel's thread is doing (`tunnel.rs`).
#[derive(Debug, Clone, PartialEq)]
enum Program {
    /// Running since `since`, with the address it gave, if it gave one.
    Running { since: Duration, address: Option<String> },
    /// Ended; started again at `until` unless the network's return is told.
    Waiting { until: Duration },
}

struct World {
    t: Duration,
    /// Time the machine slept: the monotonic clock stood still through it.
    slept: Duration,
    mono: Instant,
    wall: SystemTime,
    network: bool,
    /// What Cloudflare carries to this machine.
    carried: HashSet<String>,
    program: Program,
    backoff: Backoff,
    /// The program ends this long after the network goes, if it does.
    exits_offline: Option<Duration>,
    offline_since: Option<Duration>,
    names: u32,
    recovery: Recovery,
    /// Answers on their way: when they arrive, and what they answer.
    answers: Vec<(Duration, Answer)>,
    /// The address the service holds.
    service: Option<String>,
}

#[derive(Debug, Clone)]
enum Answer {
    Probe(String),
    Network,
    Report(String),
}

impl World {
    /// `weft remote` starting now, its tunnel with it, on a network or not.
    fn start(network: bool) -> Self {
        let (mono, wall) = (Instant::now(), SystemTime::now());
        let mut w = World {
            t: Duration::ZERO,
            slept: Duration::ZERO,
            mono,
            wall,
            network,
            carried: HashSet::new(),
            program: Program::Running { since: Duration::ZERO, address: None },
            backoff: Backoff::default(),
            exits_offline: None,
            offline_since: None,
            names: 0,
            recovery: Recovery::new(mono, wall),
            answers: Vec::new(),
            service: None,
        };
        w.tick();
        w
    }

    /// Up, reported and carried, as it is long after a start.
    fn settled() -> Self {
        let mut w = World::start(true);
        w.run_for(secs(300));
        assert!(w.works(), "it never settled");
        w
    }

    fn mono(&self) -> Instant {
        self.mono + self.t - self.slept
    }

    fn wall(&self) -> SystemTime {
        self.wall + self.t
    }

    fn address(&self) -> Option<&str> {
        match &self.program {
            Program::Running { address, .. } => address.as_deref(),
            Program::Waiting { .. } => None,
        }
    }

    /// The machine's side is done: the phone can reach it.
    fn works(&self) -> bool {
        let up = self.address();
        self.network
            && up.is_some()
            && self.service.as_deref() == up
            && self.carried.contains(up.unwrap())
    }

    fn ended(&mut self) {
        let wait = self.backoff.wait();
        self.program = Program::Waiting { until: self.t + wait };
    }

    /// One second of the world, then the loop's tick.
    fn tick(&mut self) {
        // The tunnel's program.
        match self.program.clone() {
            Program::Running { since, address: None } => {
                if self.network && self.t >= since + ADDRESS_AFTER {
                    self.names += 1;
                    let name = format!("https://run-{}.trycloudflare.com", self.names);
                    self.carried.insert(name.clone());
                    self.program = Program::Running { since, address: Some(name) };
                    self.backoff.reset();
                    self.recovery.health.new_name(self.mono());
                    let (mono, address) = (self.mono(), self.address().map(str::to_string));
                    let act = self.recovery.report(mono, address.as_deref(), true);
                    self.perform(act.into_iter().collect());
                } else if self.t >= since + ADDRESS_WITHIN {
                    self.ended();
                }
            }
            Program::Running { .. } => {
                let gone_for = self.offline_since.map(|s| self.t - s);
                if self.exits_offline.zip(gone_for).is_some_and(|(after, gone)| gone >= after) {
                    self.ended();
                }
            }
            Program::Waiting { until } if self.t >= until => {
                self.program = Program::Running { since: self.t, address: None };
            }
            Program::Waiting { .. } => {}
        }
        // Answers that arrived.
        let (now, later): (Vec<_>, Vec<_>) =
            self.answers.drain(..).partition(|(at, _)| *at <= self.t);
        self.answers = later;
        for (_, answer) in now {
            let acts = match answer {
                Answer::Probe(address) => {
                    let probe = if !self.network {
                        Probe::Offline
                    } else if self.carried.contains(&address) && self.address() == Some(&address) {
                        Probe::Up
                    } else {
                        Probe::Gone
                    };
                    self.recovery.seen(self.mono(), probe)
                }
                Answer::Network => self.recovery.network(self.mono(), self.network),
                Answer::Report(address) => {
                    if self.network {
                        self.service = Some(address.clone());
                        self.recovery.reported.taken(&address, self.mono());
                    } else {
                        self.recovery.reported.failed(self.mono());
                    }
                    Vec::new()
                }
            };
            self.perform(acts);
        }
        let address = self.address().map(str::to_string);
        let acts = self.recovery.tick(self.mono(), self.wall(), address.as_deref(), true, true);
        self.perform(acts);
    }

    fn perform(&mut self, acts: Vec<Act>) {
        for act in acts {
            let at = self.t + ANSWER;
            match act {
                Act::LookAtTunnel(a) => self.answers.push((at, Answer::Probe(a))),
                Act::LookAtNetwork => self.answers.push((at, Answer::Network)),
                Act::Report(a) => self.answers.push((at, Answer::Report(a))),
                // A poke wakes a thread waiting to start again; one that runs
                // drops pokes from before it ended.
                Act::PokeTunnel => {
                    if let Program::Waiting { .. } = self.program {
                        self.backoff.reset();
                        self.program = Program::Running { since: self.t, address: None };
                    }
                }
                Act::Recycle => {
                    if let Program::Running { .. } = self.program {
                        self.ended();
                    }
                }
            }
        }
    }

    fn run_for(&mut self, d: Duration) {
        let until = self.t + d;
        while self.t < until {
            self.t += secs(1);
            self.tick();
        }
    }

    /// How long until the phone could work again, from now; panics past
    /// `most`, the machine's budget.
    fn works_within(&mut self, most: Duration) -> Duration {
        let from = self.t;
        while !self.works() {
            assert!(
                self.t - from <= most,
                "not working {:?} after; the budget is {most:?}",
                self.t - from
            );
            self.t += secs(1);
            self.tick();
        }
        self.t - from
    }

    fn network_down(&mut self) {
        self.network = false;
        self.offline_since = Some(self.t);
    }

    fn network_up(&mut self) {
        self.network = true;
        self.offline_since = None;
    }

    /// The machine sleeps for `d`: nothing runs, the monotonic clock stands
    /// still, the wall clock does not.
    fn sleep(&mut self, d: Duration) {
        self.t += d;
        self.slept += d;
    }
}

fn budget(target: u64) -> Duration {
    secs(target) - PHONE
}

/// Network drops, then returns: within 60 s of its return. A tunnel that
/// lived through it, one whose program gave up, and one Cloudflare dropped.
#[test]
fn the_network_returning_after_30_s_or_10_min_works_within_60_s() {
    for (outage, exits, dropped) in [
        (secs(30), None, false),
        (secs(600), None, false),
        (secs(600), Some(secs(10)), false),
        (secs(600), None, true),
        (secs(30), Some(secs(10)), true),
    ] {
        let mut w = World::settled();
        w.exits_offline = exits;
        w.network_down();
        w.run_for(outage);
        if dropped {
            w.carried.clear();
        }
        w.network_up();
        let took = w.works_within(budget(60));
        eprintln!("outage {outage:?}, exits {exits:?}, dropped {dropped}: {took:?}");
    }
}

/// cloudflared exits, or Cloudflare drops its tunnel while the network is
/// fine: within 60 s.
#[test]
fn the_tunnel_ending_or_dropped_works_within_60_s() {
    let mut w = World::settled();
    w.ended();
    w.works_within(budget(60));
    // Dropped at any moment between two looks.
    for at in 0..20 {
        let mut w = World::settled();
        w.run_for(secs(at));
        w.carried.clear();
        w.works_within(budget(60));
    }
}

/// The Mac wakes: within 60 s, whether Cloudflare kept the tunnel or not,
/// and whether the network is there at once or a little later.
#[test]
fn waking_works_within_60_s_of_waking() {
    for (dropped, network_after) in [(false, 0), (true, 0), (true, 20)] {
        let mut w = World::settled();
        w.network_down();
        w.sleep(secs(600));
        if dropped {
            w.carried.clear();
        }
        w.run_for(secs(network_after));
        w.network_up();
        w.works_within(budget(60));
    }
}

/// `weft remote` crashes: weftd starts it again after its first wait, and a
/// new tunnel is up and reported, within 30 s.
#[test]
fn weft_remote_crashing_works_within_30_s() {
    let mut w = World::start(true);
    w.works_within(budget(30) - REMOTE_AGAIN);
}

/// weftd crashes with Start at login on: the service starts it, which
/// starts `weft remote`: within 30 s.
#[test]
fn weftd_crashing_with_start_at_login_works_within_30_s() {
    let mut w = World::start(true);
    w.works_within(budget(30) - SERVICE_AGAIN);
}

/// After a restart, the service starts Weft at login, often before the
/// network is up: within 60 s of login, with the network up in its first
/// 20 s.
#[test]
fn logging_in_works_within_60_s() {
    for network_after in [0, 5, 20] {
        let mut w = World::start(network_after == 0);
        w.run_for(secs(network_after));
        w.network_up();
        w.works_within(budget(60) - secs(network_after));
    }
}

/// The scenarios fail when a target is missed: a tunnel never carried again
/// is never "working".
#[test]
#[should_panic(expected = "not working")]
fn a_missed_target_is_a_failed_test() {
    let mut w = World::settled();
    w.carried.clear();
    w.recovery = Recovery::new(w.mono(), w.wall());
    // Nothing looks: the tunnel is never found gone.
    w.recovery.health.new_name(w.mono() + secs(3600));
    w.works_within(budget(60));
}
