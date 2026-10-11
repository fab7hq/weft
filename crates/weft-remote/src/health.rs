//! The tunnel seen from outside, and the machine's own sleep and network
//! (remote.md §7, §11a; ADR-0034 D3). Only rules: the loop gives the time and
//! what each look answered, and does what these say.
//!
//! Sleep is found from the wall clock against the tick: the monotonic clock
//! (`Instant`) does not advance while a Mac sleeps or a Linux machine is
//! suspended, so a tick that comes late by it never shows a sleep. The wall
//! clock does advance.

use std::time::{Duration, Instant, SystemTime};

use crate::account::{Probe, Reported};

/// The wall clock ran this far ahead of the tick: the machine slept.
pub const SLEPT: Duration = Duration::from_secs(15);
/// A new name is not looked at from outside for this long: until it is known
/// everywhere, a lookup of it can say it does not exist.
pub const NEW_NAME: Duration = Duration::from_secs(120);
/// A new name is told to the service this long after it appears. Resolvers
/// learn it a moment after the tunnel prints it, and one asked sooner answers
/// that it does not exist and keeps that answer for a minute or more: a phone
/// told at once looked it up within 2 s and could not reach it for 90 s
/// (Gate W-durable, 2026-10-11). Asked 3 s and more after, every lookup found it.
pub const PUBLISHED: Duration = Duration::from_secs(5);
/// How often the tunnel is looked at while it answers: often enough that one
/// Cloudflare dropped is found, ended and replaced within the 60 s target
/// (15 + 2 × 10 s of looks, then a new tunnel's few seconds).
pub const LOOK_EVERY: Duration = Duration::from_secs(15);
/// How often the network is looked at while it does not answer.
pub const OFFLINE_EVERY: Duration = Duration::from_secs(5);
/// Between looks that find the tunnel gone.
pub const GONE_EVERY: Duration = Duration::from_secs(10);
/// Looks in a row that find it gone before it is started again.
pub const GONE_TIMES: u32 = 3;

/// What the loop is to look at now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    Nothing,
    /// The tunnel's address, from outside.
    Tunnel,
    /// Whether the network answers at all: there is no tunnel up to look at.
    Network,
}

/// What a look changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Seen {
    /// The network answers again after it did not: tell the service the
    /// address again, and start the tunnel without waiting.
    pub back: bool,
    /// The tunnel runs here but nothing reaches it: end it, so it starts
    /// again with a new address.
    pub recycle: bool,
}

pub struct Health {
    mono: Instant,
    wall: SystemTime,
    next_look: Instant,
    looking: bool,
    gone: u32,
    offline: bool,
    /// When the tunnel last gave a new name.
    named: Option<Instant>,
}

impl Health {
    pub fn new(mono: Instant, wall: SystemTime) -> Self {
        Health {
            mono,
            wall,
            next_look: mono + NEW_NAME,
            looking: false,
            gone: 0,
            offline: false,
            named: None,
        }
    }

    /// A tick at these times: whether the machine slept since the last one.
    /// Awake again, perhaps on another network, the next look is now.
    pub fn tick(&mut self, mono: Instant, wall: SystemTime) -> bool {
        let ran = mono.saturating_duration_since(self.mono);
        let passed = wall.duration_since(self.wall).unwrap_or_default();
        (self.mono, self.wall) = (mono, wall);
        let slept = passed.saturating_sub(ran) > SLEPT;
        if slept {
            self.next_look = self.next_look.min(mono);
        }
        slept
    }

    /// What to look at now, if anything; noted as on its way when there is.
    pub fn due(&mut self, mono: Instant, up: bool) -> Due {
        if self.looking || mono < self.next_look {
            return Due::Nothing;
        }
        self.looking = true;
        if up { Due::Tunnel } else { Due::Network }
    }

    /// A new address: looked at once it is known everywhere.
    pub fn new_name(&mut self, mono: Instant) {
        self.next_look = mono + NEW_NAME;
        self.gone = 0;
        self.named = Some(mono);
    }

    /// Whether resolvers know the tunnel's name by now (`PUBLISHED`).
    pub fn published(&self, mono: Instant) -> bool {
        self.named.is_none_or(|at| mono >= at + PUBLISHED)
    }

    /// What a look at the tunnel answered.
    pub fn seen(&mut self, mono: Instant, probe: Probe) -> Seen {
        self.looking = false;
        let back = probe != Probe::Offline && std::mem::take(&mut self.offline);
        let mut seen = Seen { back, recycle: false };
        match probe {
            Probe::Up => {
                self.gone = 0;
                self.next_look = mono + LOOK_EVERY;
            }
            Probe::Offline => {
                self.gone = 0;
                self.offline = true;
                self.next_look = mono + OFFLINE_EVERY;
            }
            Probe::Gone => {
                self.gone += 1;
                self.next_look = mono + GONE_EVERY;
                if self.gone >= GONE_TIMES {
                    self.gone = 0;
                    seen.recycle = true;
                }
            }
        }
        seen
    }

    /// What a look at the network answered, with no tunnel up to look at.
    pub fn network(&mut self, mono: Instant, online: bool) -> Seen {
        self.looking = false;
        let back = online && std::mem::take(&mut self.offline);
        self.offline = !online;
        self.next_look = mono + OFFLINE_EVERY;
        Seen { back, recycle: false }
    }

    /// No answer came back at all (the look's thread was lost): look again.
    pub fn lost(&mut self, mono: Instant) {
        self.looking = false;
        self.next_look = mono + OFFLINE_EVERY;
    }
}

/// What the loop is to do, by these rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Look at the tunnel's address from outside.
    LookAtTunnel(String),
    /// Look at whether the network answers.
    LookAtNetwork,
    /// Tell the service this address.
    Report(String),
    /// Start a tunnel waiting to start again now.
    PokeTunnel,
    /// End the tunnel's program: it carries nothing.
    Recycle,
}

/// The machine's side of every recovery (remote.md §11a): when to look,
/// when to tell the service, and what a look or a wake changes. The loop
/// does each act; the scenario tests drive the same rules on a clock of
/// their own.
pub struct Recovery {
    pub health: Health,
    pub reported: Reported,
}

impl Recovery {
    pub fn new(mono: Instant, wall: SystemTime) -> Self {
        Recovery { health: Health::new(mono, wall), reported: Reported::default() }
    }

    /// A tick. `address` is the tunnel's while it is up; `tunnel` whether a
    /// tunnel is started at all; `joined` whether this machine is on an
    /// account to report to.
    pub fn tick(
        &mut self,
        mono: Instant,
        wall: SystemTime,
        address: Option<&str>,
        tunnel: bool,
        joined: bool,
    ) -> Vec<Act> {
        let mut acts = Vec::new();
        if self.health.tick(mono, wall) {
            // Awake again, perhaps on another network.
            acts.extend(self.back());
        }
        if tunnel {
            match (self.health.due(mono, address.is_some()), address) {
                (Due::Tunnel, Some(a)) => acts.push(Act::LookAtTunnel(a.to_string())),
                (Due::Network, _) => acts.push(Act::LookAtNetwork),
                _ => {}
            }
        }
        acts.extend(self.report(mono, address, joined));
        acts
    }

    /// Tell the service `address` if it is due.
    pub fn report(&mut self, mono: Instant, address: Option<&str>, joined: bool) -> Option<Act> {
        let a = address.filter(|_| joined && self.health.published(mono))?;
        self.reported.due(a, mono).then(|| Act::Report(a.to_string()))
    }

    /// What a look at the tunnel answered.
    pub fn seen(&mut self, mono: Instant, probe: Probe) -> Vec<Act> {
        let seen = self.health.seen(mono, probe);
        let mut acts = if seen.back { self.back() } else { Vec::new() };
        if seen.recycle {
            acts.push(Act::Recycle);
        }
        acts
    }

    /// What a look at the network answered.
    pub fn network(&mut self, mono: Instant, online: bool) -> Vec<Act> {
        if self.health.network(mono, online).back { self.back() } else { Vec::new() }
    }

    /// The network answers again, or the machine woke: tell the service the
    /// address again at once, and start a tunnel waiting to start now
    /// (ADR-0034 D3).
    fn back(&mut self) -> Vec<Act> {
        self.reported.again();
        vec![Act::PokeTunnel]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> (Instant, SystemTime) {
        (Instant::now(), SystemTime::now())
    }

    /// The check that never fired on macOS: the tick's own clock stood still
    /// through ten minutes of sleep, and only the wall clock moved.
    #[test]
    fn a_ten_minute_wall_jump_within_one_tick_is_a_wake() {
        let (mono, wall) = at();
        let mut h = Health::new(mono, wall);
        let one = Duration::from_secs(1);
        assert!(!h.tick(mono + one, wall + one), "an ordinary tick");
        let woke = h.tick(mono + one * 2, wall + one + Duration::from_secs(600));
        assert!(woke, "the wall clock ran ten minutes ahead of the tick");
        assert_eq!(h.due(mono + one * 2, true), Due::Tunnel, "looked at at once");
    }

    /// A tick that comes late because the machine was busy is not a sleep:
    /// both clocks moved together.
    #[test]
    fn a_late_tick_on_a_busy_machine_is_not_a_wake() {
        let (mono, wall) = at();
        let mut h = Health::new(mono, wall);
        let late = Duration::from_secs(40);
        assert!(!h.tick(mono + late, wall + late));
        // And a wall clock set back is no sleep either.
        assert!(!h.tick(mono + late * 2, wall));
    }

    /// Offline, then answering: back once, which the loop turns into one
    /// report and a tunnel started without waiting; looked at every 5 s
    /// while offline.
    #[test]
    fn the_network_coming_back_is_noticed_once() {
        let (mono, wall) = at();
        let mut h = Health::new(mono, wall);
        let t = mono + NEW_NAME;
        assert_eq!(h.due(t, true), Due::Tunnel);
        assert_eq!(h.seen(t, Probe::Offline), Seen::default());
        assert_eq!(h.due(t + Duration::from_secs(4), true), Due::Nothing);
        let t = t + OFFLINE_EVERY;
        assert_eq!(h.due(t, true), Due::Tunnel, "looked at again 5 s later");
        assert_eq!(h.seen(t, Probe::Up), Seen { back: true, recycle: false });
        let t = t + LOOK_EVERY;
        assert_eq!(h.due(t, true), Due::Tunnel);
        assert_eq!(h.seen(t, Probe::Up), Seen::default(), "back only once");
    }

    /// Back, the loop tells the service the address it already took once
    /// more, at once, and only once.
    #[test]
    fn coming_back_reports_the_address_once() {
        use crate::account::Reported;
        let now = Instant::now();
        let mut r = Reported::default();
        assert!(r.due("https://a.example", now));
        r.taken("https://a.example", now);
        assert!(!r.due("https://a.example", now), "taken: not sent again");
        r.again();
        assert!(r.due("https://a.example", now), "back: sent again");
        assert!(!r.due("https://a.example", now), "once: it is on its way");
    }

    /// The address the service took is told again every 5 minutes, so the
    /// service's last-heard time is never older than that (ADR-0034 D4).
    #[test]
    fn the_service_hears_from_the_machine_every_five_minutes() {
        use crate::account::Reported;
        let now = Instant::now();
        let mut r = Reported::default();
        assert!(r.due("https://a.example", now));
        r.taken("https://a.example", now);
        let almost = now + Duration::from_secs(5 * 60 - 1);
        assert!(!r.due("https://a.example", almost));
        assert!(r.due("https://a.example", now + Duration::from_secs(5 * 60)));
    }

    /// With no tunnel up, the network itself is looked at, so its return
    /// still starts the tunnel without waiting.
    #[test]
    fn with_no_tunnel_up_the_network_is_looked_at() {
        let (mono, wall) = at();
        let mut h = Health::new(mono, wall);
        let t = mono + NEW_NAME;
        assert_eq!(h.due(t, false), Due::Network);
        assert_eq!(h.network(t, false), Seen::default());
        let t = t + OFFLINE_EVERY;
        assert_eq!(h.due(t, false), Due::Network);
        assert_eq!(h.network(t, true), Seen { back: true, recycle: false });
        assert_eq!(h.network(t + OFFLINE_EVERY, true), Seen::default());
    }

    /// Three looks in a row that find it gone, 10 s apart, end the tunnel.
    #[test]
    fn a_tunnel_gone_three_looks_in_a_row_is_started_again() {
        let (mono, wall) = at();
        let mut h = Health::new(mono, wall);
        let mut t = mono + NEW_NAME;
        for n in 1..=GONE_TIMES {
            assert_eq!(h.due(t, true), Due::Tunnel, "look {n}");
            let seen = h.seen(t, Probe::Gone);
            assert_eq!(seen.recycle, n == GONE_TIMES, "look {n}");
            t += GONE_EVERY;
        }
        // An answer between resets the count.
        h.seen(t, Probe::Gone);
        h.seen(t, Probe::Up);
        assert!(!h.seen(t, Probe::Gone).recycle && !h.seen(t, Probe::Gone).recycle);
    }

    /// A new name is told to the service only once resolvers know it: a
    /// device that looks it up sooner is told it does not exist, and keeps
    /// that answer for a minute or more (Gate W-durable, 2026-10-11).
    #[test]
    fn a_new_name_is_reported_only_once_it_is_published() {
        let (mono, wall) = at();
        let mut r = Recovery::new(mono, wall);
        r.health.new_name(mono);
        let name = "https://new.example";
        assert_eq!(r.report(mono, Some(name), true), None, "not as it appears");
        let soon = mono + PUBLISHED - Duration::from_millis(1);
        assert!(
            r.tick(soon, wall, Some(name), true, true).iter().all(|a| !matches!(a, Act::Report(_)))
        );
        let acts = r.tick(mono + PUBLISHED, wall, Some(name), true, true);
        assert!(acts.contains(&Act::Report(name.into())), "{acts:?}");
    }
}
