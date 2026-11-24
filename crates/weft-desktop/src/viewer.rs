//! The file open in an agent's pane: a WebView in Weft's window, placed in
//! the pane's box, whose page draws the file. The page and the file come over
//! the WebView's own `weft:` scheme: `weft://app/<path>` the page's files,
//! built into the binary, and `weft://file/0` the one file it opened, read
//! from the disk a requested range at a time.

use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ::gpui::{
    Bounds, Context, IntoElement, Pixels, Render, Rgba, Window, canvas, div, prelude::*, rgb,
};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::{Method, Request, Response, StatusCode, header};
use wry::{Rect, WebView, WebViewBuilder};

use crate::gpui::palette;

/// The viewer's page: each file's path under `viewer/`, and its bytes.
static PAGE: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/viewer.rs"));

pub type Answer = Response<Cow<'static, [u8]>>;

/// What the page asks of the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// The file shown in the machine's file manager.
    Reveal,
    Close,
}

/// A message the page posted: `{"act": "reveal"}` or `{"act": "close"}`.
pub fn ask(message: &str) -> Option<Ask> {
    let m: serde_json::Value = serde_json::from_str(message).ok()?;
    match m["act"].as_str()? {
        "reveal" => Some(Ask::Reveal),
        "close" => Some(Ask::Close),
        _ => None,
    }
}

/// The answer to one of the page's requests; `file` is the one it opened.
pub fn answer(req: &Request<Vec<u8>>, file: &Path) -> Answer {
    match (req.uri().host(), req.uri().path()) {
        (Some("app"), path) => page_file(path),
        (Some("file"), "/0") => served(req, file),
        _ => status(StatusCode::NOT_FOUND),
    }
}

fn status(code: StatusCode) -> Answer {
    let mut r = Response::new(Cow::Borrowed(&[][..]));
    *r.status_mut() = code;
    r
}

/// One of the page's own files.
fn page_file(path: &str) -> Answer {
    let path = match path.trim_start_matches('/') {
        "" => "viewer.html",
        p => p,
    };
    let Some((_, bytes)) = PAGE.iter().find(|(p, _)| *p == path) else {
        return status(StatusCode::NOT_FOUND);
    };
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    let kind = match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" | "map" => "application/json",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => weft_core::files::mime_of(path),
    };
    Response::builder()
        .header(header::CONTENT_TYPE, kind)
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Cow::Borrowed(*bytes))
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR))
}

/// The bytes a `Range` header asks of a file of `total` bytes: `None` when
/// it asks for none this serves (the whole file then), `Err` when they lie
/// past its end.
fn range(asked: &str, total: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = asked.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (from, to) = spec.split_once('-')?;
    let (from, to) = (from.trim(), to.trim());
    let (start, end) = if from.is_empty() {
        // The last `to` bytes.
        let n: u64 = to.parse().ok()?;
        if n == 0 || total == 0 {
            return Some(Err(()));
        }
        (total.saturating_sub(n), total - 1)
    } else {
        let start: u64 = from.parse().ok()?;
        let end: u64 = if to.is_empty() { u64::MAX } else { to.parse().ok()? };
        if end < start {
            return None;
        }
        (start, end.min(total.saturating_sub(1)))
    };
    if start >= total {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

/// The file it opened, or the range of it asked for.
fn served(req: &Request<Vec<u8>>, file: &Path) -> Answer {
    let Ok(mut f) = File::open(file) else { return status(StatusCode::NOT_FOUND) };
    let Ok(meta) = f.metadata() else { return status(StatusCode::NOT_FOUND) };
    if !meta.is_file() {
        return status(StatusCode::NOT_FOUND);
    }
    let total = meta.len();
    let name = file.to_string_lossy();
    let mut head = Response::builder()
        .header(header::CONTENT_TYPE, weft_core::files::mime_of(&name))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Content-Length, Content-Range, Accept-Ranges",
        );
    if let Some(at) =
        meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
    {
        head = head.header(header::LAST_MODIFIED, http_date(at.as_secs()));
    }
    if weft_core::files::kind_of(&name) == Some(weft_core::files::FileKind::Html) {
        // A page the agent wrote runs apart from the viewer's, wherever it opens.
        head = head.header(header::CONTENT_SECURITY_POLICY, "sandbox allow-scripts");
    }
    let asked = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok());
    let (head, start, len) = match asked.and_then(|a| range(a, total)) {
        None => (head.status(StatusCode::OK), 0, total),
        Some(Err(())) => {
            return head
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .body(Cow::Borrowed(&[][..]))
                .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR));
        }
        Some(Ok((a, b))) => (
            head.status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{total}")),
            a,
            b - a + 1,
        ),
    };
    let mut body = Vec::new();
    if req.method() != Method::HEAD {
        body = vec![0; len as usize];
        let read = f.seek(SeekFrom::Start(start)).and_then(|_| f.read_exact(&mut body));
        if read.is_err() {
            return status(StatusCode::NOT_FOUND);
        }
    }
    head.header(header::CONTENT_LENGTH, len)
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR))
}

/// A time as HTTP writes it: `Sun, 22 Nov 2026 09:20:00 GMT`.
fn http_date(secs: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // The civil date of a day count since 1970-01-01.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        DAYS[(days % 7) as usize],
        MONTHS[(month - 1) as usize],
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// The canvas's colour, behind the page while it loads.
fn canvas_color() -> (u8, u8, u8) {
    let hex = |at: usize| u8::from_str_radix(&palette::CANVAS[at..at + 2], 16).unwrap_or(0);
    (hex(1), hex(3), hex(5))
}

/// What a viewer's WebView is made from, until the window first draws it.
struct Making {
    page: String,
    file: PathBuf,
    asked: Box<dyn Fn(Ask)>,
}

/// The file open in a pane: its WebView, laid over the box the window gives
/// it. Dropped, the WebView goes with it.
pub struct ViewerView {
    making: Option<Making>,
    /// `None` until first drawn, or when the system would not make one.
    webview: Option<Rc<WebView>>,
    /// Something is drawn over the box, which the WebView would hide.
    pub covered: bool,
    /// Where the WebView was last put, and whether it showed: told to it
    /// again only when that changes, so a scroll beside it does not make
    /// WebKit lay its page out each frame.
    placed: Rc<std::cell::Cell<Placed>>,
}

/// Where a WebView was put, and whether it showed.
type Placed = Option<(Bounds<Pixels>, bool)>;

impl ViewerView {
    /// The view loading `page`, serving `file`; what the page asks goes to
    /// `asked`.
    pub fn new(page: &str, file: PathBuf, asked: impl Fn(Ask) + 'static) -> Self {
        let making = Making { page: page.to_string(), file, asked: Box::new(asked) };
        ViewerView { making: Some(making), webview: None, covered: false, placed: Rc::default() }
    }

    /// The WebView in `window`, made the first time it is drawn.
    fn made(&mut self, window: &Window) -> Option<Rc<WebView>> {
        if let Some(Making { page, file, asked }) = self.making.take() {
            let (r, g, b) = canvas_color();
            let webview = WebViewBuilder::new()
                .with_asynchronous_custom_protocol("weft".into(), move |_, req, responder| {
                    let file = file.clone();
                    std::thread::spawn(move || responder.respond(answer(&req, &file)));
                })
                .with_ipc_handler(move |req: Request<String>| {
                    if let Some(a) = ask(req.body()) {
                        asked(a);
                    }
                })
                // The page stays: a link out of it does not take the pane.
                .with_navigation_handler(|url| {
                    ["weft:", "about:", "blob:", "data:"].iter().any(|s| url.starts_with(s))
                })
                .with_background_color((r, g, b, 255))
                .with_url(&page)
                .build_as_child(window);
            if let Err(e) = &webview {
                eprintln!("weft-desktop: the viewer: {e}");
            }
            self.webview = webview.ok().map(Rc::new);
        }
        self.webview.clone()
    }

    /// Out of sight: its pane shows something else, or the file is gone.
    pub fn hide(&self) {
        if let Some(webview) = &self.webview {
            let _ = webview.set_visible(false);
            self.placed.set(None);
        }
    }
}

/// Whether a viewer's page has the keyboard: the window's first responder is
/// a view inside Weft's own, not Weft's. The keys it does not use come on to
/// Weft's view, and are the page's, not Weft's.
pub fn page_has_keys(window: &Window) -> bool {
    use objc2_app_kit::NSView;
    let Some(weft) = weft_view(window) else { return false };
    let Some(responder) = weft.window().and_then(|w| w.firstResponder()) else { return false };
    let Ok(view) = responder.downcast::<NSView>() else { return false };
    !std::ptr::eq(&*view, weft) && view.isDescendantOf(weft)
}

/// Weft's own view, the one GPUI draws into and takes keys in.
fn weft_view(window: &Window) -> Option<&objc2_app_kit::NSView> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return None };
    // SAFETY: the handle's view is the window's own, alive while the window is.
    Some(unsafe { handle.ns_view.cast().as_ref() })
}

/// The keyboard to Weft's own view, from whatever has it: a page, or the
/// window itself once a page that had it is gone. Held by the window, each
/// key reaches Weft twice.
pub fn take_keys(window: &Window) {
    let Some(weft) = weft_view(window) else { return };
    let Some(w) = weft.window() else { return };
    let has = w.firstResponder().is_some_and(|r| {
        std::ptr::eq(
            (&*r as *const objc2_app_kit::NSResponder).cast::<u8>(),
            (weft as *const objc2_app_kit::NSView).cast::<u8>(),
        )
    });
    if !has {
        w.makeFirstResponder(Some(weft));
    }
}

impl Render for ViewerView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let webview = self.made(window);
        let (covered, placed) = (self.covered, Rc::clone(&self.placed));
        let (r, g, b) = canvas_color();
        let bg: Rgba = rgb(u32::from_be_bytes([0, r, g, b]));
        div().size_full().bg(bg).child(
            // The WebView follows this box on every frame, out of sight while
            // something is drawn over it or a tab is dragged across it.
            canvas(
                move |bounds, _, cx| {
                    let Some(webview) = webview else { return };
                    let shown = !covered && !cx.has_active_drag();
                    let was = placed.replace(Some((bounds, shown)));
                    if was.is_some_and(|(b, s)| s == shown && (!shown || b == bounds)) {
                        return;
                    }
                    let _ = webview.set_visible(shown);
                    if !shown {
                        return;
                    }
                    let _ = webview.set_bounds(Rect {
                        position: LogicalPosition::new(
                            f32::from(bounds.origin.x),
                            f32::from(bounds.origin.y),
                        )
                        .into(),
                        size: LogicalSize::new(
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                        )
                        .into(),
                    });
                },
                |_, _, _, _| {},
            )
            .size_full(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(uri: &str, range: Option<&str>) -> Request<Vec<u8>> {
        let mut req = Request::builder().uri(uri);
        if let Some(r) = range {
            req = req.header(header::RANGE, r);
        }
        req.body(Vec::new()).unwrap()
    }

    fn said(r: &Answer, name: header::HeaderName) -> &str {
        r.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or("")
    }

    /// The file whole, a range of it from the bytes asked for, a range past
    /// its end refused, and a file gone not found.
    #[test]
    fn the_file_is_served_whole_or_a_range_at_a_time() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("q2.csv");
        let bytes: Vec<u8> = (0..10_000u32).map(|n| (n % 251) as u8).collect();
        std::fs::write(&file, &bytes).unwrap();

        let whole = answer(&get("weft://file/0", None), &file);
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(whole.body().as_ref(), &bytes[..]);
        assert_eq!(said(&whole, header::CONTENT_LENGTH), "10000");
        assert_eq!(said(&whole, header::CONTENT_TYPE), "text/csv; charset=utf-8");
        assert_eq!(said(&whole, header::ACCEPT_RANGES), "bytes");
        assert_eq!(said(&whole, header::CACHE_CONTROL), "no-store");
        assert!(said(&whole, header::LAST_MODIFIED).ends_with(" GMT"));

        let part = answer(&get("weft://file/0", Some("bytes=1000-1099")), &file);
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.body().as_ref(), &bytes[1000..1100]);
        assert_eq!(said(&part, header::CONTENT_RANGE), "bytes 1000-1099/10000");
        assert_eq!(said(&part, header::CONTENT_LENGTH), "100");
        let rest = answer(&get("weft://file/0", Some("bytes=9990-")), &file);
        assert_eq!(rest.body().as_ref(), &bytes[9990..]);
        let last = answer(&get("weft://file/0", Some("bytes=-10")), &file);
        assert_eq!(said(&last, header::CONTENT_RANGE), "bytes 9990-9999/10000");
        let over = answer(&get("weft://file/0", Some("bytes=9000-20000")), &file);
        assert_eq!(said(&over, header::CONTENT_RANGE), "bytes 9000-9999/10000");

        let past = answer(&get("weft://file/0", Some("bytes=10000-")), &file);
        assert_eq!(past.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(said(&past, header::CONTENT_RANGE), "bytes */10000");
        assert!(past.body().is_empty());

        let gone = answer(&get("weft://file/0", None), &tmp.path().join("gone.csv"));
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
        // Only the window's file: no other number, no other host.
        assert_eq!(answer(&get("weft://file/1", None), &file).status(), StatusCode::NOT_FOUND);
        assert_eq!(answer(&get("weft://x/0", None), &file).status(), StatusCode::NOT_FOUND);
    }

    /// The page's own files come from the binary; one it does not have is
    /// not found.
    #[test]
    fn the_page_is_served_from_the_binary() {
        let nowhere = Path::new("/no/such/file");
        for uri in ["weft://app/viewer.html", "weft://app/"] {
            let page = answer(&get(uri, None), nowhere);
            assert_eq!(page.status(), StatusCode::OK, "{uri}");
            assert_eq!(said(&page, header::CONTENT_TYPE), "text/html; charset=utf-8");
            assert!(String::from_utf8_lossy(page.body()).contains("<html"));
        }
        let missing = answer(&get("weft://app/no-such.js", None), nowhere);
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn the_page_asks_to_reveal_or_close() {
        assert_eq!(ask(r#"{"act":"reveal"}"#), Some(Ask::Reveal));
        assert_eq!(ask(r#"{"act":"close"}"#), Some(Ask::Close));
        for other in [r#"{"act":"reply"}"#, r#"{"act":"open"}"#, "{}", "close", ""] {
            assert_eq!(ask(other), None, "{other}");
        }
    }

    #[test]
    fn times_read_as_http_writes_them() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(1_795_339_200), "Sun, 22 Nov 2026 09:20:00 GMT");
        assert_eq!(http_date(951_782_400), "Tue, 29 Feb 2000 00:00:00 GMT");
    }
}
