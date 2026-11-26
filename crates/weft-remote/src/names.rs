//! Whether a tunnel's new name is published yet, asked of the name's own
//! name servers (remote.md §11a). They never keep an answer that a name does
//! not exist, so the machine may ask as early as it likes: a resolver asked
//! too soon (a phone's, the router's) keeps that answer for a minute or more.
//!
//! The zone's name servers are found through the machine's resolver (an NS
//! lookup of the name's parent), then the name itself is asked of them
//! without recursion. Plain DNS over UDP, the few parts of a message this
//! needs.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

/// What the name's own name servers said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    /// It answers: resolvers asked now learn it.
    Found,
    /// It does not exist there yet.
    NotYet,
    /// No name server could be asked (no resolver, no network, no answer).
    CannotAsk,
}

const A: u16 = 1;
const NS: u16 = 2;
const WAIT: Duration = Duration::from_secs(2);

/// Ask whether `address`'s name is published at its zone's name servers.
pub fn published(address: &str) -> Lookup {
    let Some(host) = host_of(address) else { return Lookup::CannotAsk };
    let Some(resolver) = resolver() else { return Lookup::CannotAsk };
    // The zone is the nearest parent with name servers of its own: a
    // tailnet's name has none, its `ts.net` does.
    let mut parent = host;
    let ns = loop {
        let Some((_, up)) = parent.split_once('.').filter(|(_, up)| up.contains('.')) else {
            return Lookup::CannotAsk;
        };
        parent = up;
        let Some(answer) = ask(resolver, parent, NS, true) else { return Lookup::CannotAsk };
        if answer.count(NS) > 0 {
            break answer;
        }
    };
    let servers: Vec<SocketAddr> = ns
        .names(NS)
        .iter()
        .filter_map(|ns| (ns.as_str(), 53).to_socket_addrs().ok()?.next())
        .collect();
    for server in servers {
        if let Some(answer) = ask(server, host, A, false) {
            return if answer.rcode == 0 && answer.count(A) > 0 {
                Lookup::Found
            } else {
                Lookup::NotYet
            };
        }
    }
    Lookup::CannotAsk
}

/// The host of an `https://host[:port]/…` address.
fn host_of(address: &str) -> Option<&str> {
    let rest = address.split_once("://").map_or(address, |(_, r)| r);
    let host = rest.split(['/', ':']).next()?;
    (!host.is_empty()).then_some(host)
}

/// The machine's first resolver, as `/etc/resolv.conf` names it.
fn resolver() -> Option<SocketAddr> {
    let text = std::fs::read_to_string("/etc/resolv.conf").ok()?;
    text.lines().find_map(|l| {
        let ip: IpAddr = l.trim().strip_prefix("nameserver")?.trim().parse().ok()?;
        Some(SocketAddr::new(ip, 53))
    })
}

/// One question to one server; its answer, or none in `WAIT`.
fn ask(server: SocketAddr, name: &str, kind: u16, recurse: bool) -> Option<Answer> {
    let bind: SocketAddr =
        if server.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { ([0u16; 8], 0).into() };
    let socket = UdpSocket::bind(bind).ok()?;
    socket.set_read_timeout(Some(WAIT)).ok()?;
    let id = (std::process::id() as u16) ^ (name.len() as u16).rotate_left(7) ^ kind;
    socket.send_to(&query(id, name, kind, recurse)?, server).ok()?;
    let mut buf = [0u8; 1500];
    let (n, from) = socket.recv_from(&mut buf).ok()?;
    (from.ip() == server.ip()).then(|| Answer::parse(&buf[..n], id)).flatten()
}

/// A question: header, then the name as labels, its type and class IN.
fn query(id: u16, name: &str, kind: u16, recurse: bool) -> Option<Vec<u8>> {
    let mut m = Vec::with_capacity(32 + name.len());
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&(if recurse { 0x0100u16 } else { 0 }).to_be_bytes());
    m.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.trim_end_matches('.').split('.') {
        let len = u8::try_from(label.len()).ok().filter(|l| (1..64).contains(l))?;
        m.push(len);
        m.extend_from_slice(label.as_bytes());
    }
    m.push(0);
    m.extend_from_slice(&kind.to_be_bytes());
    m.extend_from_slice(&1u16.to_be_bytes());
    Some(m)
}

/// The parts of an answer this reads: its code, and each record of the
/// answer section with its type and, for NS, the name it holds.
#[derive(Debug, Default)]
struct Answer {
    rcode: u8,
    records: Vec<(u16, Option<String>)>,
}

impl Answer {
    fn parse(m: &[u8], id: u16) -> Option<Answer> {
        let word = |at: usize| Some(u16::from_be_bytes([*m.get(at)?, *m.get(at + 1)?]));
        if word(0)? != id || m.get(2)? & 0x80 == 0 {
            return None;
        }
        let rcode = m.get(3)? & 0x0f;
        let (questions, answers) = (word(4)?, word(6)?);
        let mut at = 12;
        for _ in 0..questions {
            at = name(m, at)?.1 + 4;
        }
        let mut records = Vec::new();
        for _ in 0..answers {
            at = name(m, at)?.1;
            let (kind, len) = (word(at)?, word(at + 8)? as usize);
            let data = at + 10;
            let held = if kind == NS { Some(name(m, data)?.0) } else { None };
            records.push((kind, held));
            at = data + len;
        }
        Some(Answer { rcode, records })
    }

    fn count(&self, kind: u16) -> usize {
        self.records.iter().filter(|(k, _)| *k == kind).count()
    }

    fn names(&self, kind: u16) -> Vec<String> {
        self.records.iter().filter(|(k, _)| *k == kind).filter_map(|(_, n)| n.clone()).collect()
    }
}

/// The name at `at`, following compression pointers, and where the record
/// goes on after it.
fn name(m: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut after = None;
    for _ in 0..64 {
        let len = *m.get(at)? as usize;
        if len == 0 {
            return Some((labels.join("."), after.unwrap_or(at + 1)));
        }
        if len & 0xc0 == 0xc0 {
            after.get_or_insert(at + 2);
            at = ((len & 0x3f) << 8) | *m.get(at + 1)? as usize;
            continue;
        }
        labels.push(String::from_utf8_lossy(m.get(at + 1..at + 1 + len)?).into_owned());
        at += 1 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_is_its_labels_type_and_class() {
        let q = query(0xabcd, "a.example.com", NS, true).unwrap();
        assert_eq!(&q[..4], &[0xab, 0xcd, 0x01, 0x00], "id, then recursion asked");
        assert_eq!(&q[4..12], &[0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&q[12..], b"\x01a\x07example\x03com\x00\x00\x02\x00\x01");
        assert_eq!(query(1, "x.example", A, false).unwrap()[2], 0, "no recursion");
        assert!(query(1, &"a".repeat(64), A, false).is_none(), "a label is at most 63");
    }

    /// An answer as a server writes one: the question, then two NS records
    /// whose names point back into the question.
    fn ns_answer(id: u16, rcode: u8) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend_from_slice(&id.to_be_bytes());
        m.extend_from_slice(&[0x81, 0x80 | rcode]);
        m.extend_from_slice(&[0, 1, 0, 2, 0, 0, 0, 0]);
        m.extend_from_slice(b"\x07example\x03com\x00\x00\x02\x00\x01");
        for ns in [b"\x05kevin".as_slice(), b"\x07marjory".as_slice()] {
            m.extend_from_slice(&[0xc0, 12, 0, 2, 0, 1, 0, 0, 0, 60]);
            let data = [ns, &[0xc0, 12]].concat();
            m.extend_from_slice(&(data.len() as u16).to_be_bytes());
            m.extend_from_slice(&data);
        }
        m
    }

    #[test]
    fn an_answer_gives_its_code_and_the_names_it_holds() {
        let a = Answer::parse(&ns_answer(7, 0), 7).unwrap();
        assert_eq!(a.rcode, 0);
        assert_eq!(a.names(NS), ["kevin.example.com", "marjory.example.com"]);
        assert_eq!(a.count(A), 0);
        assert!(Answer::parse(&ns_answer(7, 0), 8).is_none(), "another question's answer");
        assert_eq!(Answer::parse(&ns_answer(9, 3), 9).unwrap().rcode, 3, "no such name");
    }

    #[test]
    fn a_name_that_points_at_itself_is_refused() {
        let mut m = vec![0u8; 12];
        m.extend_from_slice(&[0xc0, 12]);
        assert!(name(&m, 12).is_none());
        assert!(name(&[3, b'a'], 0).is_none(), "cut short");
    }

    #[test]
    fn the_host_is_read_from_an_address() {
        assert_eq!(host_of("https://a-b.trycloudflare.com"), Some("a-b.trycloudflare.com"));
        assert_eq!(host_of("https://n.tail.ts.net:443/ws"), Some("n.tail.ts.net"));
        assert_eq!(host_of("https://"), None);
    }
}
