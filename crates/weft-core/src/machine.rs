//! What this machine is called: the name the person gave it in Settings,
//! else the one its system gives. Where each system keeps its own name is
//! data here; asking it is the daemon's, since nothing here runs a program.

use crate::words;

/// One place a system keeps its own name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A program's answer: its output's first line.
    Program(&'static str, &'static [&'static str]),
    /// A file's `KEY=value` line, the value quoted or not.
    FileKey(&'static str, &'static str),
    /// A file's first line: a host name.
    File(&'static str),
    /// The host name the kernel gives.
    HostName,
}

impl Source {
    /// Whether it gives a host name, whose domain is cut, rather than a name
    /// for people.
    pub fn is_host(&self) -> bool {
        matches!(self, Source::File(_) | Source::HostName)
    }
}

/// Each system's places, best first. A Mac's own name is its Computer Name,
/// set in System Settings › General › About; Linux has a pretty name, then
/// its static host name.
const SOURCES: &[(&str, &[Source])] = &[
    ("macos", &[Source::Program("scutil", &["--get", "ComputerName"]), Source::HostName]),
    (
        "linux",
        &[
            Source::Program("hostnamectl", &["--pretty"]),
            Source::FileKey("/etc/machine-info", "PRETTY_HOSTNAME"),
            Source::File("/etc/hostname"),
            Source::HostName,
        ],
    ),
];

/// Where `os` (as Rust names it) keeps its name, best first; the host name
/// on any other.
pub fn sources(os: &str) -> &'static [Source] {
    SOURCES.iter().find(|(o, _)| *o == os).map_or(&[Source::HostName], |(_, s)| s)
}

/// A name the system said, as Weft shows it: a host name without its domain.
/// None when it names nothing a person would know the machine by: empty, an
/// address, or a bare number.
pub fn usable(said: &str, host: bool) -> Option<String> {
    let said = said.trim().trim_end_matches('.');
    if said.parse::<core::net::IpAddr>().is_ok() {
        return None;
    }
    let name = if host { said.split('.').next().unwrap_or_default() } else { said }.trim();
    let bare = name.chars().all(|c| c.is_ascii_digit() || ".-:_ ".contains(c));
    (!bare).then(|| name.to_string())
}

/// The first name a place gives that Weft can show, asking each in turn
/// with `ask`; "this machine" when none does.
pub fn first(sources: &[Source], mut ask: impl FnMut(&Source) -> Option<String>) -> String {
    sources
        .iter()
        .find_map(|s| ask(s).and_then(|said| usable(&said, s.is_host())))
        .unwrap_or_else(|| words::word("desktop.this_machine").to_string())
}

/// The value of `key` in a `KEY=value` file.
pub fn file_key(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix(key)?.strip_prefix('=')?;
        Some(value.trim().trim_matches('"').trim_matches('\'').to_string())
    })
}

/// The name Weft and the person's devices use: the one set in Settings,
/// else the system's.
pub fn effective(set: Option<&str>, system: &str) -> String {
    set.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(system).to_string()
}

/// What a name typed in Settings keeps in `config.toml`: nothing when it is
/// empty or the system's own name again, which is what nothing set means.
pub fn to_keep(typed: &str, system: &str) -> Option<String> {
    let typed = typed.trim();
    (!typed.is_empty() && typed != system.trim()).then(|| typed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_or_a_bare_number_is_never_a_name() {
        assert_eq!(usable("192.168.1.97", true), None, "the host name a network handed out");
        assert_eq!(usable("192.168.1.97", false), None);
        assert_eq!(usable("fe80::1", true), None);
        assert_eq!(usable("192", true), None);
        assert_eq!(usable("192-168-1-97", true), None);
        assert_eq!(usable("  ", false), None);
        assert_eq!(usable("", true), None);
    }

    #[test]
    fn a_host_name_loses_its_domain_and_a_name_for_people_keeps_every_dot() {
        assert_eq!(usable("studio.local", true).as_deref(), Some("studio"));
        assert_eq!(usable("studio.local.", true).as_deref(), Some("studio"));
        assert_eq!(usable("build-box\n", true).as_deref(), Some("build-box"));
        assert_eq!(usable("Ada’s MacBook Pro", false).as_deref(), Some("Ada’s MacBook Pro"));
        assert_eq!(usable("v2.0 rig", false).as_deref(), Some("v2.0 rig"));
        assert_eq!(usable("1.lab", true), None, "a number once its domain is cut");
    }

    #[test]
    fn the_first_place_with_a_name_wins_and_this_machine_is_last() {
        let mac = sources("macos");
        let said = |computer: Option<&str>, host: Option<&str>| {
            first(mac, |s| match s {
                Source::Program(..) => computer.map(String::from),
                _ => host.map(String::from),
            })
        };
        assert_eq!(said(Some("Ada’s MacBook Pro"), Some("192.168.1.97")), "Ada’s MacBook Pro");
        assert_eq!(said(None, Some("studio.local")), "studio");
        assert_eq!(said(Some(""), Some("192.168.1.97")), words::word("desktop.this_machine"));
        assert_eq!(said(None, None), words::word("desktop.this_machine"));
    }

    #[test]
    fn each_system_asks_its_own_places_and_any_other_its_host_name() {
        assert_eq!(sources("macos")[0], Source::Program("scutil", &["--get", "ComputerName"]));
        assert_eq!(sources("linux")[0], Source::Program("hostnamectl", &["--pretty"]));
        assert_eq!(sources("freebsd"), &[Source::HostName]);
        for (_, places) in SOURCES {
            assert_eq!(places.last(), Some(&Source::HostName), "the host name is last");
        }
    }

    #[test]
    fn a_key_is_read_from_its_line_quoted_or_not() {
        let text = "DEPLOYMENT=lab\nPRETTY_HOSTNAME=\"Ada's Box\"\n";
        assert_eq!(file_key(text, "PRETTY_HOSTNAME").as_deref(), Some("Ada's Box"));
        assert_eq!(file_key("PRETTY_HOSTNAME=Box\n", "PRETTY_HOSTNAME").as_deref(), Some("Box"));
        assert_eq!(file_key("PRETTY=Box\n", "PRETTY_HOSTNAME"), None);
    }

    #[test]
    fn the_name_set_wins_and_nothing_set_is_the_system_s() {
        assert_eq!(effective(Some("Studio"), "Ada’s MacBook Pro"), "Studio");
        assert_eq!(effective(None, "Ada’s MacBook Pro"), "Ada’s MacBook Pro");
        assert_eq!(effective(Some("  "), "Ada’s MacBook Pro"), "Ada’s MacBook Pro");
    }

    #[test]
    fn an_empty_name_or_the_system_s_own_keeps_nothing() {
        let system = "Ada’s MacBook Pro";
        assert_eq!(to_keep("", system), None);
        assert_eq!(to_keep("   ", system), None);
        assert_eq!(to_keep(" Ada’s MacBook Pro ", system), None);
        assert_eq!(to_keep(" Studio ", system).as_deref(), Some("Studio"));
    }
}
