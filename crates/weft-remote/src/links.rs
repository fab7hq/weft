//! Private links to the files an agent pointed at. A device that opens one
//! is given `/f/<token>/<name>`, which serves that one file on the tunnel's
//! port while it is in use: by range, so a large file, a PDF or a video loads
//! only what is read. The token is the only key; tokens live in memory.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::socket::ORIGIN;

/// How long a link serves its file after it was last used.
pub const LAPSE: Duration = Duration::from_secs(10 * 60);
/// How long a connection may wait between requests.
const IDLE: Duration = Duration::from_secs(30);
/// The most a request's head may hold.
const HEAD_MAX: usize = 16 * 1024;

struct Link {
    file: PathBuf,
    device: String,
    used: Instant,
}

/// The links made, shared by the loop that makes them and the connections
/// that serve them.
#[derive(Clone, Default)]
pub struct Links(Arc<Mutex<HashMap<String, Link>>>);

impl Links {
    /// A new link to `file` for `device`: its path, `/f/<token>/<name>`.
    pub fn make(&self, file: &Path, device: &str) -> String {
        use base64ct::{Base64UrlUnpadded, Encoding};
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw).expect("the system's randomness");
        let token = Base64UrlUnpadded::encode_string(&raw);
        let mut links = self.0.lock().expect("links");
        links.retain(|_, l| l.used.elapsed() < LAPSE);
        let link =
            Link { file: file.to_path_buf(), device: device.to_string(), used: Instant::now() };
        links.insert(token.clone(), link);
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("/f/{token}/{}", escape(&name))
    }

    /// The file a token serves, while it has not lapsed; each use keeps it
    /// for another while, so a long video or document read goes on.
    fn file(&self, token: &str) -> Option<PathBuf> {
        let mut links = self.0.lock().expect("links");
        let link = links.get_mut(token).filter(|l| l.used.elapsed() < LAPSE)?;
        link.used = Instant::now();
        Some(link.file.clone())
    }

    /// A device that is no longer allowed: its links stop serving.
    pub fn forget(&self, device: &str) {
        self.0.lock().expect("links").retain(|_, l| l.device != device);
    }
}

/// Whether a connection's first bytes ask for a link rather than the
/// WebSocket.
pub fn asks_for_link(first: &[u8]) -> bool {
    [&b"GET /f/"[..], b"HEAD /f/", b"OPTIONS /f/"].iter().any(|m| first.starts_with(m))
}

/// Answer a connection's requests for links until it closes, idles, or asks
/// for something else.
pub fn serve(stream: TcpStream, links: &Links) {
    if stream.set_read_timeout(Some(IDLE)).is_err() {
        return;
    }
    let Ok(mut out) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    while let Some(request) = head(&mut reader) {
        let answer = answer(&request, links);
        if write(&mut out, &request, answer).is_err() || !request.keep_alive {
            return;
        }
    }
}

struct Request {
    method: String,
    target: String,
    range: Option<String>,
    keep_alive: bool,
}

/// One request's head; `None` at the end of the connection or on one that
/// is not a link's.
fn head(reader: &mut impl BufRead) -> Option<Request> {
    let mut line = String::new();
    let mut read = 0;
    let mut next = |line: &mut String| -> Option<()> {
        line.clear();
        let n = reader.by_ref().take((HEAD_MAX - read) as u64).read_line(line).ok()?;
        read += n;
        (n > 0 && line.ends_with('\n')).then_some(())
    };
    next(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    let mut request = Request {
        method: method.to_string(),
        target: target.to_string(),
        range: None,
        keep_alive: version == "HTTP/1.1",
    };
    if !asks_for_link(format!("{method} {target}").as_bytes()) {
        return None;
    }
    loop {
        next(&mut line)?;
        let l = line.trim_end();
        if l.is_empty() {
            return Some(request);
        }
        let Some((name, value)) = l.split_once(':') else { continue };
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "range" => request.range = Some(value.to_string()),
            "connection" if value.eq_ignore_ascii_case("close") => request.keep_alive = false,
            "connection" if value.eq_ignore_ascii_case("keep-alive") => request.keep_alive = true,
            _ => {}
        }
    }
}

/// What a request is answered: a status, its headers, and the span of the
/// file sent.
#[derive(Debug, PartialEq)]
struct Answer {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Option<(PathBuf, u64, u64)>,
}

fn answer(request: &Request, links: &Links) -> Answer {
    let cors =
        vec![("Access-Control-Allow-Origin", ORIGIN.to_string()), ("Vary", "Origin".to_string())];
    let bare = |status, mut headers: Vec<(&'static str, String)>| {
        headers.push(("Content-Length", "0".into()));
        Answer { status, headers, body: None }
    };
    if request.method == "OPTIONS" {
        let mut h = cors;
        h.push(("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS".into()));
        h.push(("Access-Control-Allow-Headers", "Range".into()));
        h.push(("Access-Control-Max-Age", "600".into()));
        return bare(204, h);
    }
    let path = request.target.split('?').next().unwrap_or_default();
    let token = path.strip_prefix("/f/").and_then(|r| r.split('/').next()).unwrap_or("");
    let found = links.file(token).and_then(|f| std::fs::metadata(&f).ok().map(|m| (f, m)));
    let Some((file, meta)) = found.filter(|(_, m)| m.is_file()) else { return bare(404, cors) };
    let size = meta.len();
    let name = file.to_string_lossy();
    let mut h = cors;
    h.push((
        "Access-Control-Expose-Headers",
        "Content-Length, Content-Range, Accept-Ranges, Last-Modified".into(),
    ));
    h.push(("Content-Type", weft_core::files::mime_of(&name).to_string()));
    h.push(("Accept-Ranges", "bytes".into()));
    h.push(("Cache-Control", "no-store".into()));
    h.push(("X-Content-Type-Options", "nosniff".into()));
    if let Ok(t) = meta.modified() {
        h.push(("Last-Modified", http_date(t)));
    }
    if weft_core::files::kind_of(&name) == Some(weft_core::files::FileKind::Html) {
        // Opened anywhere, the page runs in an origin of its own.
        h.push(("Content-Security-Policy", "sandbox allow-scripts".into()));
    }
    let (status, from, len) = match request.range.as_deref().map(|r| span(r, size)) {
        None => (200, 0, size),
        Some(Some((from, to))) => {
            h.push(("Content-Range", format!("bytes {from}-{to}/{size}")));
            (206, from, to - from + 1)
        }
        Some(None) => {
            h.push(("Content-Range", format!("bytes */{size}")));
            return bare(416, h);
        }
    };
    h.push(("Content-Length", len.to_string()));
    let body = (request.method == "GET").then_some((file, from, len));
    Answer { status, headers: h, body }
}

/// The bytes `Range` asks for, first and last, inclusive: one range only.
/// `None` when it cannot be met.
fn span(range: &str, size: u64) -> Option<(u64, u64)> {
    let spec = range.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let (from, to) = match (a.is_empty(), b.is_empty()) {
        (true, false) => {
            let n = b.parse::<u64>().ok()?.min(size);
            (size.checked_sub(n)?, size.checked_sub(1)?)
        }
        (false, _) => {
            let from = a.parse::<u64>().ok()?;
            let to = if b.is_empty() { size.saturating_sub(1) } else { b.parse::<u64>().ok()? };
            (from, to.min(size.saturating_sub(1)))
        }
        (true, true) => return None,
    };
    (from <= to && from < size).then_some((from, to))
}

fn write(out: &mut TcpStream, request: &Request, a: Answer) -> std::io::Result<()> {
    let reason = match a.status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        404 => "Not Found",
        _ => "Range Not Satisfiable",
    };
    let mut head = format!("HTTP/1.1 {} {reason}\r\n", a.status);
    for (k, v) in &a.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(if request.keep_alive {
        "Connection: keep-alive\r\n\r\n"
    } else {
        "Connection: close\r\n\r\n"
    });
    out.write_all(head.as_bytes())?;
    if let Some((file, from, len)) = a.body {
        let mut f = std::fs::File::open(file)?;
        f.seek(SeekFrom::Start(from))?;
        let sent = std::io::copy(&mut f.take(len), out)?;
        if sent < len {
            // The file shrank while it was sent: the length promised is a lie.
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
    }
    out.flush()
}

/// A name as it may stand in a URL's path.
fn escape(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A time as HTTP writes it: `Sun, 22 Nov 2026 09:20:00 GMT`.
fn http_date(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days % 7) as usize];
    let months =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!(
        "{weekday}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        months[(month - 1) as usize],
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(target: &str, range: Option<&str>) -> Request {
        Request {
            method: "GET".into(),
            target: target.into(),
            range: range.map(str::to_string),
            keep_alive: true,
        }
    }

    fn header<'a>(a: &'a Answer, name: &str) -> Option<&'a str> {
        a.headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_range_is_one_span_inside_the_file() {
        assert_eq!(span("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(span("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(span("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(span("bytes=990-2000", 1000), Some((990, 999)));
        assert_eq!(span("bytes=1000-", 1000), None);
        assert_eq!(span("bytes=5-1", 1000), None);
        assert_eq!(span("bytes=0-1,5-6", 1000), None);
        assert_eq!(span("items=0-1", 1000), None);
        assert_eq!(span("bytes=0-0", 0), None);
    }

    #[test]
    fn a_link_serves_its_file_whole_or_by_range_and_nothing_else() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("q3 report.html");
        std::fs::write(&file, vec![b'x'; 1000]).unwrap();
        let links = Links::default();
        let link = links.make(&file, "d_1");
        assert!(link.starts_with("/f/") && link.ends_with("/q3%20report.html"), "{link}");

        let whole = answer(&get(&link, None), &links);
        assert_eq!((whole.status, header(&whole, "Content-Length")), (200, Some("1000")));
        assert_eq!(header(&whole, "Content-Type"), Some("text/html; charset=utf-8"));
        assert_eq!(header(&whole, "Content-Security-Policy"), Some("sandbox allow-scripts"));
        assert_eq!(header(&whole, "Cache-Control"), Some("no-store"));
        assert_eq!(header(&whole, "Access-Control-Allow-Origin"), Some(ORIGIN));
        assert!(header(&whole, "Last-Modified").is_some_and(|d| d.ends_with(" GMT")));

        let part = answer(&get(&link, Some("bytes=10-19")), &links);
        assert_eq!(part.status, 206);
        assert_eq!(header(&part, "Content-Range"), Some("bytes 10-19/1000"));
        assert_eq!(part.body, Some((file.clone(), 10, 10)));

        let past = answer(&get(&link, Some("bytes=5000-")), &links);
        assert_eq!((past.status, header(&past, "Content-Range")), (416, Some("bytes */1000")));

        assert_eq!(answer(&get("/f/forged/q3.html", None), &links).status, 404);
        links.forget("d_1");
        assert_eq!(answer(&get(&link, None), &links).status, 404, "its device was forgotten");
    }

    #[test]
    fn a_gone_file_is_not_found_and_a_preflight_allows_range() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a.csv");
        std::fs::write(&file, "a,b\n").unwrap();
        let links = Links::default();
        let link = links.make(&file, "d_1");
        let pre = Request { method: "OPTIONS".into(), ..get(&link, None) };
        let a = answer(&pre, &links);
        assert_eq!((a.status, header(&a, "Access-Control-Allow-Headers")), (204, Some("Range")));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(answer(&get(&link, None), &links).status, 404);
    }

    #[test]
    fn only_link_requests_are_taken_from_the_websocket() {
        assert!(asks_for_link(b"GET /f/abc/x.pdf HTTP/1.1\r\n"));
        assert!(asks_for_link(b"OPTIONS /f/abc/x.pdf HTTP/1.1\r\n"));
        assert!(!asks_for_link(b"GET /ws HTTP/1.1\r\n"));
        assert!(!asks_for_link(b"GET /fx HTTP/1.1\r\n"));
    }

    #[test]
    fn dates_are_written_as_http_writes_them() {
        let t = UNIX_EPOCH + Duration::from_secs(1_795_339_200);
        assert_eq!(http_date(t), "Sun, 22 Nov 2026 09:20:00 GMT");
        assert_eq!(http_date(UNIX_EPOCH), "Thu, 01 Jan 1970 00:00:00 GMT");
    }
}
