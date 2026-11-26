//! The desktop's window: its event loop, the threads that connect to the
//! daemon and take what it sends, and the element tree drawn with GPUI.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ::gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, Div,
    ElementId, EntityInputHandler, FocusHandle, Focusable, FollowMode, FontWeight, IntoElement,
    KeyDownEvent, ListAlignment, ListState, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PathPromptOptions, Pixels, Point, Render, Rgba, ScrollHandle,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, StyleRefinement, Styled,
    SystemNotification, UTF16Selection, WeakEntity, Window, WindowBounds as GpuiWindowBounds,
    WindowId, WindowOptions, div as gpui_div, point, prelude::*, px, relative, rgba, size,
};
use anyhow::Result;
use weft_core::state::WindowBounds;
use weft_core::text_keys::System;

use crate::action::{Action, Focus, PaneAct, SheetAct, WindowAct};
use crate::app::WeftDesktopApp;
use crate::daemon::{Again, ProjectClient};
use crate::gpui::{Element, palette};
use crate::keys::{self, Shortcut};
use crate::panes_view::TabDrag;
use crate::terminal_input::{self, Act, Button, Held};
use crate::terminal_view::TerminalView;
use crate::text_field::{Outcome, TextField};
use crate::text_input::{self as field_view, FieldElement, Look, Painted};

/// The text field `which` names, when it is one and is there.
fn field(app: &mut WeftDesktopApp, which: Focus) -> Option<&mut TextField> {
    match which {
        Focus::Search => Some(&mut app.search),
        Focus::Message => Some(app.message_field()),
        Focus::Ask => app.ask_draft_mut(),
        Focus::Setting => app.setting_field_mut(),
        Focus::SealWords => app.seal_words_mut(),
        Focus::Terminal => None,
    }
}

/// The field that has the keyboard, if one has.
fn focused(app: &mut WeftDesktopApp) -> Option<&mut TextField> {
    let which = app.focus;
    field(app, which)
}

/// A key as the text key table names it: `ctrl-alt-shift-cmd-key`.
fn chord(ks: &::gpui::Keystroke) -> String {
    let m = ks.modifiers;
    let mut c = String::new();
    for (on, name) in
        [(m.control, "ctrl-"), (m.alt, "alt-"), (m.shift, "shift-"), (m.platform, "cmd-")]
    {
        if on {
            c.push_str(name);
        }
    }
    c.push_str(&ks.key);
    c
}

pub(crate) struct SharedState {
    pub(crate) app: WeftDesktopApp,
    dirty: bool,
    /// Where each pane's terminal was last painted, and its cell width, to
    /// find the cell under the pointer.
    pub(crate) grids: HashMap<(PathBuf, u32), (Point<Pixels>, f32)>,
    /// Where an app started after this one hands over its project.
    instance: Option<crate::instance::Listening>,
    /// The window is to come in front: another start asked for it.
    raise: bool,
    /// The panes whose terminal had output since the window last looked.
    output: Vec<(PathBuf, u32)>,
}

#[derive(Clone, Copy, PartialEq)]
struct InheritedText {
    color: Rgba,
    font_size: f32,
    font_weight: FontWeight,
    font_mono: bool,
    line_height: Option<f32>,
    nowrap: bool,
    ellipsis: bool,
    pre: bool,
}

struct GpuiDesktopView {
    shared: Arc<Mutex<SharedState>>,
    focus_handle: FocusHandle,
    wheel_accum: f32,
    /// The pane a drag is selecting in.
    selecting: Option<(PathBuf, u32)>,
    /// Where the focused text field was last painted.
    painted: Rc<RefCell<Painted>>,
    /// Each scroll that reads more above its top, and its first child's id.
    scrolls: RefCell<HashMap<String, (ScrollHandle, Option<String>)>>,
    /// The element tree last built from the app.
    last: Option<Element>,
    /// Each conversation's list, by its scroll's key: kept while the app
    /// runs, so each agent's conversation keeps its place.
    lists: RefCell<HashMap<String, Listed>>,
    /// Each scroll with something drawn over it away from its end: whether
    /// it was at its end when last drawn.
    drawn_at_end: RefCell<HashMap<String, bool>>,
    /// `last` was built when the view was told something changed, for the
    /// draw that follows.
    fresh: bool,
    /// The view was told something changed since it last drew.
    stale: bool,
    /// Only terminals were told to draw again, or a list scrolled, since the
    /// view last drew: its tree is as it was.
    output_only: bool,
    /// This draw builds the tree from the app again.
    rebuilding: Cell<bool>,
    /// How many times the tree was built from the app, for a test.
    builds: usize,
    /// Each pane's terminal, a view of its own.
    terminals: RefCell<HashMap<(PathBuf, u32), ::gpui::Entity<TerminalView>>>,
    /// The boxes that hold no terminal, field or page and take their size
    /// from around them, each a view of its own, by their place in the tree.
    parts: RefCell<HashMap<String, ::gpui::Entity<Part>>>,
    /// The terminals and parts this build drew.
    kept: RefCell<Kept>,
    /// The file open in each pane that has one, by its viewer's id.
    #[cfg(target_os = "macos")]
    viewers: RefCell<HashMap<u64, ::gpui::Entity<crate::viewer::ViewerView>>>,
    /// The divider being dragged: where the pointer started, and the
    /// conversation's width then.
    resizing: Option<(f32, f32)>,
}

/// A conversation's list: where it is scrolled and each block's height, the
/// blocks it holds, and the text style they inherit.
struct Listed {
    state: ListState,
    blocks: Rc<RefCell<Vec<Element>>>,
    text: Rc<Cell<InheritedText>>,
}

/// The terminals and parts one build drew; the rest are dropped.
#[derive(Default)]
struct Kept {
    terminals: HashSet<(PathBuf, u32)>,
    parts: HashSet<String>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    viewers: HashSet<u64>,
}

/// A box of the window that holds no terminal and whose size is set by the
/// boxes around it, not by what it holds. It is laid out and drawn again
/// only when the window was told something changed, so a terminal's output
/// leaves it as it was.
struct Part {
    desktop: WeakEntity<GpuiDesktopView>,
    /// The box, filling the place the window gives it.
    el: Element,
    text: InheritedText,
    path: String,
    /// How many times it was drawn, for a test.
    drawn: usize,
}

impl Render for Part {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drawn += 1;
        let Some(desktop) = self.desktop.upgrade() else { return gpui_div().into_any_element() };
        let (el, text, path) = (&self.el, self.text, &self.path);
        desktop.update(cx, |view, cx| view.build_gpui_element(el, text, false, path, None, cx))
    }
}

/// Whether the box an element sits in has a width and a height the layout
/// gives it, and how it lays its children out.
#[derive(Clone, Copy)]
struct Room {
    width: bool,
    height: bool,
    flex: bool,
    row: bool,
}

/// The styles that place a box and size it in its parent; the rest draw it
/// and lay its children out.
const PLACING: &[&str] = &[
    "width",
    "height",
    "min-width",
    "min-height",
    "max-width",
    "max-height",
    "flex",
    "flex-shrink",
    "align-self",
    "position",
    "inset",
    "top",
    "left",
    "right",
    "bottom",
    "z-index",
];

/// Whether an element's width and height are set by the box it sits in and
/// its own styles, never by what it holds: `(width, height)`.
fn sized(el: &Element, parent_is_row: bool, parent: Room) -> (bool, bool) {
    let get = |k: &str| el.styles.iter().find(|(s, _)| s == k).map(|(_, v)| v.as_str());
    let stretched = get("align-self") == Some("stretch");
    // As the window draws it: a stretched box fills its parent across.
    let width_set = |has: bool| match get("width") {
        Some(v) if v.ends_with("px") => true,
        Some("100%") => has,
        Some(_) => false,
        None => stretched && !parent_is_row && has,
    };
    let height_set = |has: bool| match get("height") {
        Some(v) if v.ends_with("px") => true,
        Some("100%") => has,
        Some(_) => false,
        None => stretched && parent_is_row && has,
    };
    let (w, h) = (width_set(parent.width), height_set(parent.height));
    if get("position") == Some("absolute") {
        return match get("inset") {
            Some("0") => (parent.width, parent.height),
            _ => (w, h),
        };
    }
    if !parent.flex {
        // A block fills its parent's width; its height is what it holds.
        return (w || (get("width").is_none() && parent.width), h);
    }
    // `flex: 1` grows from nothing and may shrink to nothing: its share of
    // the room left, whatever it holds.
    let grows = get("flex").is_some_and(|v| v.starts_with('1'));
    let (main, cross) =
        if parent.row { (parent.width, parent.height) } else { (parent.height, parent.width) };
    let main = grows && main;
    let cross = stretched && cross;
    if parent.row { (w || main, h || cross) } else { (w || cross, h || main) }
}

/// The element at a part's path: `0` the root, `0/2/1` its third child's
/// second.
fn at_path<'a>(root: &'a Element, path: &str) -> Option<&'a Element> {
    let mut steps = path.split('/');
    if steps.next() != Some("0") {
        return None;
    }
    steps.try_fold(root, |el, i| el.children.get(i.parse::<usize>().ok()?))
}

/// Whether a box holds what is read when it is drawn, not from its element:
/// a text field's words, what shows only away from a scroll's end, a file's
/// page that hides while a tab is dragged. Such a box is never a part.
fn reads_as_drawn(el: &Element) -> bool {
    el.field.is_some()
        || el.ends.is_some()
        || el.tag == "viewer"
        || el.children.iter().any(reads_as_drawn)
}

/// Whether a pane's terminal is in an element or under it.
fn holds_terminal(el: &Element) -> bool {
    el.tag == "terminal" || el.children.iter().any(holds_terminal)
}

impl Focusable for GpuiDesktopView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// How long a window waits for a daemon to listen again before it ends:
/// long enough for a login service to start a crashed one again.
const LOST_FOR: Duration = Duration::from_secs(30);

/// Connect every project that is not connected, off the shared lock, until
/// the window quits. One that would not connect waits longer each time, up
/// to half a minute, before it is tried again. One whose daemon went is
/// opened again only on a daemon that listens: never one this window starts,
/// since the person may have stopped it.
fn keep_connected(shared: Arc<Mutex<SharedState>>) {
    let mut waits: HashMap<PathBuf, (std::time::Instant, Duration)> = HashMap::new();
    let mut lost_since: Option<std::time::Instant> = None;
    loop {
        let (targets, own) = match shared.lock() {
            Ok(guard) if !guard.app.quit_requested => {
                (guard.app.disconnected(), guard.app.wants_own_link())
            }
            _ => return,
        };
        // With no project listed, Weft's own link: Updates needs no project.
        if let Some(start) = own
            && let Ok(client) = ProjectClient::machine(&weft_proto::socket_path(), start)
            && let Ok(mut guard) = shared.lock()
        {
            guard.app.integrate_own_link(client);
            guard.dirty = true;
        }
        if !targets.iter().any(|(_, lost)| *lost) {
            lost_since = None;
        }
        let now = std::time::Instant::now();
        for (root, lost) in targets {
            if waits.get(&root).is_some_and(|(next, _)| now < *next) {
                continue;
            }
            let opened = if lost {
                match ProjectClient::again(&weft_proto::socket_path(), &root, 28, 100) {
                    Again::Back(client) => Ok(*client),
                    Again::Waiting => {
                        if lost_since.get_or_insert(now).elapsed() >= LOST_FOR {
                            if let Ok(mut guard) = shared.lock() {
                                guard.app.session_ended();
                            }
                            return;
                        }
                        continue;
                    }
                    Again::Newer(_) if start_newer(&shared) => return,
                    Again::Newer(why) | Again::Refused(why) => Err(why),
                }
            } else {
                ProjectClient::open(&root, 28, 100).map_err(|e| e.to_string())
            };
            match opened {
                Ok(client) => {
                    waits.remove(&root);
                    if let Ok(mut guard) = shared.lock() {
                        guard.app.integrate_connected_client(client);
                        guard.dirty = true;
                    }
                }
                Err(why) => {
                    if let Ok(mut guard) = shared.lock() {
                        guard.app.not_opened(&root, &why);
                    }
                    let wait = waits.get(&root).map_or(Duration::from_secs(1), |(_, w)| {
                        (*w * 2).min(Duration::from_secs(30))
                    });
                    waits.insert(root, (std::time::Instant::now() + wait, wait));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// What a Linux desktop knows the window by: its entry's `StartupWMClass`,
/// which gives the launcher and the dock Weft's icon.
pub const APP_ID: &str = "weft-desktop";

/// A newer Weft took the daemon over: open its app in place of this one,
/// which goes. Whether there was one to open.
fn start_newer(shared: &Mutex<SharedState>) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return false };
    let Some(weft) = crate::daemon::newer_weft(&home) else { return false };
    hand_over(shared, || crate::daemon::open_app_of(&weft).is_ok())
}

/// Go, for the app `open` starts: this one's state written first, so the
/// newer one opens to it. Whether it started.
fn hand_over(shared: &Mutex<SharedState>, open: impl FnOnce() -> bool) -> bool {
    write_state(shared);
    let Ok(mut guard) = shared.lock() else { return false };
    // The newer app listens where this one did.
    guard.instance = None;
    if !open() {
        return false;
    }
    guard.app.quit_requested = true;
    true
}

/// Write the desktop's state when it changed, outside the shared lock.
fn write_state(shared: &Mutex<SharedState>) {
    let unsaved = shared.lock().ok().and_then(|mut g| g.app.unsaved_state());
    if let Some((home, state)) = unsaved {
        let _ = crate::app::save_state_to_home(&home, &state);
    }
}

/// Run the desktop window and its event loop until the person quits.
pub fn run_interactive(mut app: WeftDesktopApp) -> Result<()> {
    if app.state.open_projects.is_empty()
        && let Ok(cwd) = std::env::current_dir()
        && (cwd.join(".git").exists() || cwd.join(".fab7").exists())
    {
        app.open_project_path(cwd);
    }

    let initial_bounds = app.state.settings.window_bounds.unwrap_or(WindowBounds {
        x: 120.0,
        y: 80.0,
        width: 1280.0,
        height: 820.0,
    });

    let shared = Arc::new(Mutex::new(SharedState {
        app,
        dirty: true,
        grids: HashMap::new(),
        instance: None,
        raise: false,
        output: Vec::new(),
    }));

    // An app started after this one hands its project over and ends.
    let handing = Arc::clone(&shared);
    let instance = crate::instance::listen(&crate::instance::socket_path(), move |project| {
        if let Ok(mut guard) = handing.lock() {
            if let Some(dir) = project {
                guard.app.open_project_path(dir);
            }
            guard.raise = true;
            guard.dirty = true;
        }
    });
    if let Ok(mut guard) = shared.lock() {
        guard.instance = instance.ok();
    }

    let connecting = Arc::clone(&shared);
    std::thread::spawn(move || keep_connected(connecting));

    // What the daemon sends, taken every 35 ms: nothing here waits for it.
    let pump_shared = Arc::clone(&shared);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(35));
            {
                let Ok(mut guard) = pump_shared.lock() else { break };
                if guard.app.quit_requested {
                    break;
                }
                let redraw = guard.app.pump();
                guard.dirty |= redraw.window;
                for t in redraw.terminals {
                    if !guard.output.contains(&t) {
                        guard.output.push(t);
                    }
                }
            }
            write_state(&pump_shared);
        }
    });

    let answering = Arc::clone(&shared);
    gpui_platform::application().with_assets(Pictures).run(move |cx: &mut App| {
        crate::fonts::load(cx);
        // A notice the person opens brings its agent in front.
        cx.on_system_notification_response(move |response, cx| {
            if let Ok(mut guard) = answering.lock() {
                guard.app.open_notice(&response.tag);
                guard.dirty = true;
            }
            cx.activate(true);
        });
        // The main window closing ends the app.
        let main_id: Rc<Cell<Option<WindowId>>> = Rc::default();
        let closing = Rc::clone(&main_id);
        cx.on_window_closed(move |cx, id| {
            if cx.windows().is_empty() || closing.get() == Some(id) {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds {
            origin: point(px(initial_bounds.x), px(initial_bounds.y)),
            size: size(px(initial_bounds.width.max(960.0)), px(initial_bounds.height.max(640.0))),
        };

        let view_shared = Arc::clone(&shared);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(GpuiWindowBounds::Windowed(bounds)),
                app_id: Some(APP_ID.into()),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let view = GpuiDesktopView::new(Arc::clone(&view_shared), window, cx);

                    cx.observe_window_bounds(window, |this: &mut GpuiDesktopView, window, _cx| {
                        this.stale = true;
                        let wb = window.bounds();
                        // Each pane's terminal sizes itself from its own rectangle.
                        if let Ok(mut guard) = this.shared.lock() {
                            guard.app.set_window_bounds(
                                f32::from(wb.origin.x),
                                f32::from(wb.origin.y),
                                f32::from(wb.size.width),
                                f32::from(wb.size.height),
                            );
                        }
                    })
                    .detach();

                    // Leaving: each project's queued calls are written before the window goes.
                    cx.on_app_quit(|this: &mut GpuiDesktopView, _cx| {
                        let mut clients = match this.shared.lock() {
                            Ok(mut guard) => {
                                guard.app.quit_requested = true;
                                guard.instance = None;
                                std::mem::take(&mut guard.app.clients)
                            }
                            Err(_) => Vec::new(),
                        };
                        write_state(&this.shared);
                        for c in &mut clients {
                            c.detach();
                        }
                        async {}
                    })
                    .detach();

                    let poll_shared = Arc::clone(&view_shared);
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor().timer(Duration::from_millis(30)).await;
                            let (dirty, output, quit, notices, raise) = match poll_shared.lock() {
                                Ok(mut guard) => (
                                    std::mem::take(&mut guard.dirty),
                                    std::mem::take(&mut guard.output),
                                    guard.app.quit_requested,
                                    guard.app.take_notices(),
                                    std::mem::take(&mut guard.raise),
                                ),
                                Err(_) => (false, Vec::new(), true, Vec::new(), false),
                            };
                            if raise {
                                cx.update(|cx| cx.activate(true));
                            }
                            follow_files(&poll_shared, cx);
                            for n in notices {
                                cx.update(|cx| {
                                    cx.show_system_notification(SystemNotification {
                                        tag: n.tag.into(),
                                        title: n.title.into(),
                                        body: SharedString::default(),
                                        actions: Vec::new(),
                                    })
                                });
                            }
                            if quit {
                                cx.update(|cx| cx.quit());
                                break;
                            }
                            // Output alone draws its panes' terminals, not the window.
                            let drawn = if dirty {
                                this.update(cx, |_, cx| cx.notify())
                            } else if !output.is_empty() {
                                this.update(cx, |view, cx| view.draw_terminals(&output, cx))
                            } else {
                                Ok(())
                            };
                            if drawn.is_err() {
                                break;
                            }
                        }
                    })
                    .detach();

                    view
                })
            },
        );
        match opened {
            Ok(main) => {
                main_id.set(Some(main.window_id()));
                cx.activate(true)
            }
            Err(e) => {
                eprintln!("weft-desktop: {e}");
                cx.quit();
            }
        }
    });

    Ok(())
}

/// What the files the person opened ask of the machine: a file handed to
/// its own app, or shown in its file manager.
fn follow_files(shared: &Arc<Mutex<SharedState>>, cx: &mut ::gpui::AsyncApp) {
    let Ok(mut guard) = shared.lock() else { return };
    let outside = std::mem::take(&mut guard.app.files.outside);
    let reveal = std::mem::take(&mut guard.app.files.reveal);
    drop(guard);
    for path in outside {
        cx.update(|cx| cx.open_with_system(&path));
    }
    for path in reveal {
        cx.update(|cx| cx.reveal_path(&path));
    }
}

impl GpuiDesktopView {
    /// The view over `shared`, with the keyboard.
    fn new(shared: Arc<Mutex<SharedState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        // Told something changed: the tree is built again, and each part of
        // it whose boxes changed is drawn again with it, told before the
        // window draws: a long conversation is not laid out again each time
        // the bar's clock moves.
        cx.observe_self(|this: &mut Self, cx| {
            this.stale = true;
            let tree = this.built();
            let parts: Vec<_> =
                this.parts.borrow().iter().map(|(k, p)| (k.clone(), p.clone())).collect();
            for (path, part) in parts {
                match at_path(&tree, &path).map(filling) {
                    Some(el) if part.read(cx).el == el => {}
                    Some(el) => part.update(cx, |p, cx| {
                        p.el = el;
                        cx.notify();
                    }),
                    None => part.update(cx, |_, cx| cx.notify()),
                }
            }
            this.last = Some(tree);
            this.fresh = true;
        })
        .detach();
        GpuiDesktopView {
            shared,
            focus_handle,
            wheel_accum: 0.0,
            selecting: None,
            painted: Rc::default(),
            scrolls: RefCell::default(),
            last: None,
            lists: RefCell::default(),
            drawn_at_end: RefCell::default(),
            fresh: false,
            stale: true,
            output_only: false,
            rebuilding: Cell::new(true),
            builds: 0,
            terminals: RefCell::default(),
            parts: RefCell::default(),
            kept: RefCell::default(),
            #[cfg(target_os = "macos")]
            viewers: RefCell::default(),
            resizing: None,
        }
    }

    /// The terminals of these panes draw again, and only they: what the
    /// window shows around them did not change.
    fn draw_terminals(&mut self, panes: &[(PathBuf, u32)], cx: &mut Context<Self>) {
        let shown: Vec<_> =
            panes.iter().filter_map(|p| self.terminals.borrow().get(p).cloned()).collect();
        for terminal in &shown {
            terminal.update(cx, |_, cx| cx.notify());
        }
        self.output_only |= !shown.is_empty();
    }

    /// Ask the system for a folder or a file, then hand it to `then`.
    fn pick(
        &self,
        directories: bool,
        prompt: &str,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut WeftDesktopApp, PathBuf) + 'static,
    ) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: !directories,
            directories,
            multiple: false,
            prompt: Some(SharedString::from(prompt.to_string())),
        });
        let shared = Arc::clone(&self.shared);
        cx.spawn(async move |_, _| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(path) = paths.into_iter().next()
                && let Ok(mut guard) = shared.lock()
            {
                then(&mut guard.app, path);
                guard.dirty = true;
            }
        })
        .detach();
    }

    fn dispatch_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        match &action {
            Action::Window(WindowAct::OpenProject) => {
                if let Ok(mut guard) = self.shared.lock() {
                    guard.app.dispatch(action.clone());
                }
                let prompt = weft_core::words::word("desktop.pick.project");
                return self.pick(true, prompt, cx, WeftDesktopApp::open_project_path);
            }
            // A conversation's scroll is the window's; the terminal's is the app's.
            Action::Pane(PaneAct::ToEnd { project, pane }) => {
                let id = crate::panes_view::conversation_id(project, *pane);
                let key = crate::gpui::scroll_key("pane conversation", &id);
                if let Some(listed) = self.lists.borrow().get(&key) {
                    listed.state.scroll_to_end();
                    listed.state.set_follow_mode(FollowMode::Tail);
                }
                cx.notify();
            }
            // Only a web page: a guide is a tool file's, which the person
            // installed but did not write.
            Action::Sheet(SheetAct::OpenGuide(url)) => {
                if url.starts_with("https://") {
                    cx.open_url(url);
                }
                return;
            }
            Action::Sheet(SheetAct::Locate(harness)) => {
                let harness = harness.clone();
                let prompt =
                    weft_core::words::fill("desktop.pick.program", &[("harness", &harness)]);
                return self.pick(false, &prompt, cx, move |app, path| app.locate(&harness, &path));
            }
            _ => {}
        }
        let mut quit = false;
        if let Ok(mut guard) = self.shared.lock() {
            guard.app.dispatch(action);
            quit = guard.app.quit_requested;
        }
        if quit { cx.quit() } else { cx.notify() }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let mods = ks.modifiers;
        let typing = self.shared.lock().map(|g| g.app.focus != Focus::Terminal).unwrap_or(false);
        let shortcut = keys::shortcut(System::this(), &chord(ks));

        // A key the file's page had and did not use is the page's, not the
        // agent's or a field's; the window's own shortcuts still work.
        #[cfg(target_os = "macos")]
        if !self.viewers.borrow().is_empty() && crate::viewer::page_has_keys(window) {
            cx.stop_propagation();
            if !matches!(
                shortcut,
                Some(
                    Shortcut::Quit
                        | Shortcut::OpenProject
                        | Shortcut::ToggleList
                        | Shortcut::Settings
                )
            ) {
                return;
            }
        }

        // A text field takes its keys from the table; a character goes on to
        // the window's text input, which an input method composes. The
        // window's own shortcuts still work there.
        if typing {
            if self.field_key(event, cx) {
                cx.stop_propagation();
            }
            if !matches!(
                shortcut,
                Some(
                    Shortcut::Quit
                        | Shortcut::OpenProject
                        | Shortcut::ToggleList
                        | Shortcut::Settings
                )
            ) {
                return;
            }
        }

        let action = match shortcut {
            Some(Shortcut::Quit) => Action::from(WindowAct::RequestQuit),
            Some(Shortcut::OpenProject) => Action::from(WindowAct::OpenProject),
            Some(Shortcut::ToggleList) => Action::from(WindowAct::ToggleSidebar),
            Some(Shortcut::Settings) => Action::from(WindowAct::OpenSettings),
            Some(Shortcut::Copy) => {
                let text = self.shared.lock().ok().and_then(|mut guard| {
                    guard.app.active_pane_view_mut().and_then(|p| p.selection_text())
                });
                if let Some(text) = text {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                return;
            }
            Some(Shortcut::Paste) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                    && let Ok(mut guard) = self.shared.lock()
                {
                    let bracketed =
                        guard.app.active_pane_view_mut().is_some_and(|p| p.bracketed_paste());
                    let bytes = terminal_input::paste(&text, bracketed);
                    guard.app.send_active_pane_input(&bytes);
                }
                return;
            }
            None => {
                // Terminal input, as the program's modes say keys are sent.
                let held = Held {
                    shift: mods.shift,
                    alt: mods.alt,
                    control: mods.control,
                    platform: mods.platform,
                };
                if let Ok(mut guard) = self.shared.lock() {
                    let modes =
                        guard.app.active_pane_view_mut().map(|p| p.modes()).unwrap_or_default();
                    if let Some(bytes) =
                        terminal_input::key(&ks.key, ks.key_char.as_deref(), held, modes)
                    {
                        guard.app.send_active_pane_input(&bytes);
                    }
                }
                cx.notify();
                return;
            }
        };
        self.dispatch_action(action, window, cx);
    }

    /// A key in the focused text field, as the key table says; whether it was one.
    fn field_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Ok(mut guard) = self.shared.lock() else { return false };
        let which = guard.app.focus;
        let Some(f) = field(&mut guard.app, which) else { return false };
        // While an input method composes, its keys are its own.
        if f.marked.is_some() {
            return false;
        }
        let Some(outcome) = f.key(&chord(&event.keystroke)) else { return false };
        match outcome {
            Outcome::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Outcome::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    f.paste(&text);
                }
            }
            Outcome::Send => match which {
                Focus::Message => guard.app.dispatch(crate::action::PaneAct::SendMessage),
                Focus::Ask => guard.app.dispatch(SheetAct::SubmitAsk),
                Focus::Setting => guard.app.dispatch(SheetAct::CommitSetting),
                Focus::SealWords => guard.app.dispatch(crate::action::BarAct::SealNext),
                _ => {}
            },
            // Escape leaves the field.
            Outcome::Leave => guard.app.focus = Focus::Terminal,
            Outcome::Changed | Outcome::Nothing => {}
        }
        cx.notify();
        true
    }

    /// The cell under `at` in a pane's terminal, `(row, col)` from 0.
    fn cell_at(
        &self,
        project: &std::path::Path,
        pane: u32,
        at: Point<Pixels>,
    ) -> Option<(u16, u16)> {
        let guard = self.shared.lock().ok()?;
        let (origin, cell_width) = *guard.grids.get(&(project.to_path_buf(), pane))?;
        let row = (f32::from(at.y - origin.y) / crate::terminal_view::LINE_HEIGHT).max(0.0);
        let col = (f32::from(at.x - origin.x) / cell_width).max(0.0);
        Some((row as u16, col as u16))
    }

    /// Report a mouse act to the pane's program if it asked for that kind;
    /// whether it did.
    fn report(
        &mut self,
        project: &std::path::Path,
        pane: u32,
        button: Button,
        act: Act,
        cell: (u16, u16),
    ) -> bool {
        let Ok(mut guard) = self.shared.lock() else { return false };
        let modes = guard.app.pane_view(project, pane).map(|p| p.modes()).unwrap_or_default();
        match terminal_input::mouse(button, act, cell, modes) {
            Some(bytes) => {
                guard.app.send_pane_input(project, pane, &bytes);
                true
            }
            None => false,
        }
    }

    /// The pointer on a terminal: to the program when it asked for the
    /// mouse, else drag selects and the wheel moves through history.
    fn terminal_pointer(&self, d: Div, project: PathBuf, pane: u32, cx: &mut Context<Self>) -> Div {
        let (p1, p2, p3, p4) = (project.clone(), project.clone(), project.clone(), project);
        d.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, e: &MouseDownEvent, _window, cx| {
                let Some(cell) = this.cell_at(&p1, pane, e.position) else { return };
                if let Ok(mut guard) = this.shared.lock() {
                    guard.app.focus = Focus::Terminal;
                }
                if !this.report(&p1, pane, Button::Left, Act::Press, cell) {
                    if let Ok(mut guard) = this.shared.lock()
                        && let Some(view) = guard.app.pane_view_mut(&p1, pane)
                    {
                        view.select_from(cell);
                    }
                    this.selecting = Some((p1.clone(), pane));
                }
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _window, cx| {
            if e.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let Some(cell) = this.cell_at(&p2, pane, e.position) else { return };
            if this.selecting.as_ref() == Some(&(p2.clone(), pane)) {
                if let Ok(mut guard) = this.shared.lock()
                    && let Some(view) = guard.app.pane_view_mut(&p2, pane)
                {
                    view.select_to(cell);
                }
                cx.notify();
            } else {
                this.report(&p2, pane, Button::Left, Act::Drag, cell);
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, e: &MouseUpEvent, _window, _cx| {
                if this.selecting.take().is_none()
                    && let Some(cell) = this.cell_at(&p3, pane, e.position)
                {
                    this.report(&p3, pane, Button::Left, Act::Release, cell);
                }
            }),
        )
        .on_scroll_wheel(cx.listener(move |this, e: &ScrollWheelEvent, _window, cx| {
            let dy = f32::from(e.delta.pixel_delta(px(crate::terminal_view::LINE_HEIGHT)).y);
            this.wheel_accum += dy;
            let lines = (this.wheel_accum / crate::terminal_view::LINE_HEIGHT).trunc() as i32;
            if lines == 0 {
                return;
            }
            this.wheel_accum -= lines as f32 * crate::terminal_view::LINE_HEIGHT;
            let cell = this.cell_at(&p4, pane, e.position).unwrap_or((0, 0));
            let button = if lines > 0 { Button::WheelUp } else { Button::WheelDown };
            let reported =
                (0..lines.unsigned_abs()).all(|_| this.report(&p4, pane, button, Act::Press, cell));
            if !reported
                && let Ok(mut guard) = this.shared.lock()
                && let Some(view) = guard.app.pane_view_mut(&p4, pane)
            {
                view.scroll(lines);
            }
            cx.notify();
        }))
    }

    /// A text field: its words drawn by the field element, the pointer
    /// placing the caret and dragging a selection.
    fn text_field(
        &self,
        el: &Element,
        which: Focus,
        d: Div,
        text_style: InheritedText,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lh = text_style.line_height.unwrap_or(text_style.font_size * 1.4);
        let look = Look {
            line_height: lh,
            color: parse_color(palette::TEXT),
            placeholder: parse_color(palette::DIM),
            caret: parse_color(palette::TEXT),
            selection: parse_color("#388bfd55"),
            max_lines: if el.tag == "input"
                && matches!(which, Focus::Search | Focus::Setting | Focus::SealWords)
            {
                1
            } else {
                6
            },
        };
        let snapshot = self.shared.lock().ok().and_then(|mut g| {
            let is_focused = g.app.focus == which;
            field(&mut g.app, which).map(|f| {
                (
                    is_focused,
                    f.text().to_string(),
                    f.selection(),
                    f.cursor(),
                    f.marked.clone(),
                    f.shape(),
                )
            })
        });
        let Some((is_focused, text, selection, cursor, marked, shape)) = snapshot else {
            return d.into_any_element();
        };
        // Only the focused field's paint is kept for the pointer and the input method.
        let painted = if is_focused { Rc::clone(&self.painted) } else { Rc::default() };
        let element = FieldElement {
            view: cx.entity(),
            focus: self.focus_handle.clone(),
            focused: is_focused,
            text,
            selection,
            cursor,
            marked,
            placeholder: el.placeholder.clone().unwrap_or_default(),
            shape,
            look,
            painted,
        };
        let mut field_box = d
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.focus_handle.focus(window, cx);
                    let Ok(mut guard) = this.shared.lock() else { return };
                    let was = guard.app.focus == which;
                    guard.app.focus = which;
                    let at = if was { this.painted.borrow().index_for(e.position) } else { None };
                    if let Some(f) = field(&mut guard.app, which)
                        && let Some(at) = at
                    {
                        if e.modifiers.shift { f.select_to(at) } else { f.select(at..at) }
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _window, cx| {
                if e.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let Ok(mut guard) = this.shared.lock() else { return };
                if guard.app.focus != which {
                    return;
                }
                let Some(at) = this.painted.borrow().index_for(e.position) else { return };
                if let Some(f) = field(&mut guard.app, which) {
                    f.select_to(at);
                }
                cx.notify();
            }))
            .child(element);
        // What sits inside the field beside its words.
        for (idx, child) in el.children.iter().enumerate() {
            let child_path = format!("{path}/{idx}");
            field_box = field_box.child(self.build_gpui_element(
                child,
                text_style,
                true,
                &child_path,
                None,
                cx,
            ));
        }
        field_box.into_any_element()
    }

    /// An element as GPUI draws it; one with a `z-index` is painted after
    /// everything else, over what follows it, where it is laid out. `room`
    /// is the box it sits in, while its parts are still to be found.
    fn build_gpui_element(
        &self,
        el: &Element,
        parent_text: InheritedText,
        parent_is_row: bool,
        path: &str,
        room: Option<Room>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let built = self.build_gpui_box(el, parent_text, parent_is_row, path, room, cx);
        match el.styles.iter().find(|(k, _)| k == "z-index") {
            Some((_, z)) => {
                ::gpui::deferred(built).with_priority(z.parse().unwrap_or(1)).into_any_element()
            }
            None => built,
        }
    }

    fn build_gpui_box(
        &self,
        el: &Element,
        parent_text: InheritedText,
        parent_is_row: bool,
        path: &str,
        room: Option<Room>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Only while its scroll is away from its end.
        if let Some(key) = &el.ends {
            let end = self.at_its_end(key);
            self.drawn_at_end.borrow_mut().insert(key.clone(), end);
            if end {
                return gpui_div().into_any_element();
            }
        }
        // The size this box gets from around it, when it gets one.
        let own = room.map(|r| sized(el, parent_is_row, r));
        // A box that reads nothing as it is drawn is a part: laid out again
        // only when it changes. A box holding a terminal, a field or a page is
        // built here each time, and its other boxes become parts.
        if own == Some((true, true))
            && el.field.is_none()
            && !el.children.is_empty()
            && !holds_terminal(el)
            && !reads_as_drawn(el)
        {
            return self.part(el, parent_text, parent_is_row, path, cx);
        }
        if el.tag == "img" {
            if el.styles.iter().any(|(k, v)| k == "animation" && v == "spin") {
                return spinning(el, path);
            }
            return picture(el);
        }
        let mut text_style = parent_text;
        let mut has_explicit_width = false;
        let mut has_explicit_height = false;
        let mut is_self_stretch = false;
        let (scroll_x, scroll_y) = scroll_axes(&el.styles);

        for (k, v) in &el.styles {
            match k.as_str() {
                "color" => text_style.color = parse_color(v),
                "font-size" => {
                    if let Some(n) = parse_px(v) {
                        text_style.font_size = n;
                    }
                }
                "font-weight" => {
                    if let Ok(w) = v.parse::<f32>() {
                        text_style.font_weight = FontWeight(w);
                    }
                }
                "font-family" => text_style.font_mono = v.contains("Mono"),
                "line-height" => {
                    if let Some(n) = parse_px(v) {
                        text_style.line_height = Some(n);
                    }
                }
                "white-space" => {
                    text_style.nowrap = v == "nowrap";
                    text_style.pre = v == "pre" || v == "pre-wrap";
                }
                "text-overflow" => text_style.ellipsis = v == "ellipsis",
                "width" => has_explicit_width = true,
                "height" => has_explicit_height = true,
                "align-self" => is_self_stretch = v == "stretch",
                _ => {}
            }
        }

        let mut d: Div = gpui_div()
            .text_color(text_style.color)
            .text_size(px(text_style.font_size))
            .font_weight(text_style.font_weight);
        // A test finds an element's bounds by its layer's name.
        if let Some(name) = &el.layer_name {
            d = d.debug_selector(|| name.clone());
        }
        let mut child_is_row = false;

        if text_style.font_mono {
            d = d.font_family(crate::fonts::MONO);
        }
        if let Some(lh) = text_style.line_height {
            d = d.line_height(px(lh));
        }
        if text_style.nowrap {
            d = d.whitespace_nowrap();
        }
        if text_style.ellipsis {
            d = d.overflow_x_hidden().text_ellipsis();
        }

        // If an element contains inline `span` children and has no explicit flex-direction,
        // lay them out horizontally like HTML inline spans.
        if el.children.iter().any(|c| c.tag == "span")
            && !el.styles.iter().any(|(k, _)| k == "flex-direction")
        {
            d = d.flex().flex_row().items_center();
            child_is_row = true;
        }

        for (k, v) in &el.styles {
            if k == "flex-direction" {
                child_is_row = v == "row";
            }
            // One the window does not draw is left out; a test keeps the views to those it does.
            d = style(d, k, v).unwrap_or_else(|d| d);
        }

        if is_self_stretch {
            if parent_is_row {
                if !has_explicit_height {
                    d = d.h_full();
                }
            } else if !has_explicit_width {
                d = d.w_full();
            }
        }

        if let Some(which) = el.field {
            return self.text_field(el, which, d, text_style, path, cx);
        }

        // The file open in a pane, its page laid over this box.
        #[cfg(target_os = "macos")]
        if el.tag == "viewer" {
            let attr = |k: &str| el.attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
            if let Some(view) = attr("data-id")
                .and_then(|id| id.parse().ok())
                .and_then(|id| self.viewer_view(id, attr("data-covered").is_some(), cx))
            {
                return d.child(view).into_any_element();
            }
        }

        // The divider beside a file: dragged, the conversation's width follows.
        if el.tag == "divider" {
            let width: f32 = el
                .attrs
                .iter()
                .find(|(a, _)| a == "data-width")
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(crate::files::CONVERSATION_WIDTH);
            return d
                .cursor(CursorStyle::ResizeLeftRight)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, _window, cx| {
                        this.resizing = Some((f32::from(e.position.x), width));
                        cx.stop_propagation();
                    }),
                )
                .into_any_element();
        }

        // A pane's terminal, painted from its own cells.
        if el.tag == "terminal" {
            let attr = |k: &str| el.attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
            if let (Some(project), Some(pane)) =
                (attr("data-project"), attr("data-pane").and_then(|p| p.parse().ok()))
            {
                let project = PathBuf::from(project);
                let view = self.terminal_view(&project, pane, cx);
                return self
                    .terminal_pointer(d.size_full(), project, pane, cx)
                    .child(view)
                    .into_any_element();
            }
        }

        // Rich text: one run of words, each range in its own style.
        if el.tag == "rich"
            && let Some(t) = &el.text
        {
            let highlights: Vec<(Range<usize>, ::gpui::HighlightStyle)> = el
                .runs
                .iter()
                .map(|(range, run)| {
                    let style = ::gpui::HighlightStyle {
                        color: run.color.map(|c| parse_color(c).into()),
                        background_color: run.background.map(|c| parse_color(c).into()),
                        font_weight: run.bold.then_some(FontWeight::SEMIBOLD),
                        font_style: run.italic.then_some(::gpui::FontStyle::Italic),
                        underline: run.underline.then(|| ::gpui::UnderlineStyle {
                            thickness: px(1.),
                            ..Default::default()
                        }),
                        strikethrough: run.strike.then(|| ::gpui::StrikethroughStyle {
                            thickness: px(1.),
                            ..Default::default()
                        }),
                        ..Default::default()
                    };
                    (range.clone(), style)
                })
                .collect();
            let mono: Vec<(Range<usize>, SharedString)> = el
                .runs
                .iter()
                .filter(|(_, run)| run.mono)
                .map(|(range, _)| (range.clone(), SharedString::from(crate::fonts::MONO)))
                .collect();
            d = d.child(
                ::gpui::StyledText::new(t.clone())
                    .with_highlights(highlights)
                    .with_font_family_overrides(mono),
            );
        } else if let Some(t) = &el.text
            && !t.is_empty()
        {
            match text_lines(t, text_style.pre, text_style.nowrap) {
                Some(lines) => {
                    let lh = text_style.line_height.unwrap_or(text_style.font_size * 1.4);
                    let mut lines_col = gpui_div().flex().flex_col().w_full();
                    for (line, nowrap) in lines {
                        let row = gpui_div().min_h(px(lh));
                        let row = if nowrap { row.whitespace_nowrap() } else { row };
                        lines_col = lines_col.child(row.child(line));
                    }
                    d = d.child(lines_col);
                }
                None => d = d.child(t.clone()),
            }
        }

        // A conversation: a list that lays out only the blocks in sight.
        if let Some(action) = el.at_top.clone() {
            let key = match (&el.layer_name, &el.id) {
                (Some(ln), Some(id)) => crate::gpui::scroll_key(ln, id),
                _ => format!("node:{path}"),
            };
            let gap = el
                .styles
                .iter()
                .find(|(k, _)| k == "gap")
                .and_then(|(_, v)| v.trim_end_matches("px").parse().ok())
                .unwrap_or(0.0);
            let shown = self.listed(el, &key, text_style, path, action, gap, cx);
            return d.child(shown).into_any_element();
        }

        // The room this box gives its children.
        let flex =
            el.styles.iter().any(|(k, v)| (k == "display" && v == "flex") || k == "flex-direction")
                || el.children.iter().any(|c| c.tag == "span");
        let column = el.styles.iter().any(|(k, v)| k == "flex-direction" && v == "column");
        let inner = own.map(|(width, height)| Room { width, height, flex, row: flex && !column });
        for (idx, child) in el.children.iter().enumerate() {
            let child_path = format!("{path}/{idx}");
            d = d.child(self.build_gpui_element(
                child,
                text_style,
                child_is_row,
                &child_path,
                inner,
                cx,
            ));
        }

        // A tab dragged over: shown where it would go, and taken there.
        if el.drop_glow {
            d = d.drag_over::<TabDrag>(|s, _, _, _| {
                s.bg(parse_color(palette::SELECTED_BG))
                    .border_2()
                    .border_dashed()
                    .border_color(parse_color(palette::BLUE_BORDER))
            });
        }
        if let Some(target) = el.drop {
            d = d.on_drop(cx.listener(move |this, dragged: &TabDrag, window, cx| {
                let act = PaneAct::DropTab { group: dragged.group, tab: dragged.tab, target };
                this.dispatch_action(act.into(), window, cx);
            }));
        }

        // Stateful elements: scrollable containers, clickable actions, dragged tabs
        if scroll_x || scroll_y || el.action.is_some() || el.drag.is_some() {
            // A scroll with an id keeps its place under that id, not its
            // place on screen: one pane shows many agents in turn.
            let id_str = match (&el.layer_name, el.id.as_ref().filter(|_| scroll_x || scroll_y)) {
                (Some(ln), Some(id)) => crate::gpui::scroll_key(ln, id),
                (Some(ln), None) => format!("layer:{ln}:{path}"),
                (None, Some(id)) => format!("id:{id}"),
                (None, None) => format!("node:{path}"),
            };
            let mut stateful = d.id(ElementId::Name(SharedString::from(id_str.clone())));
            if scroll_x {
                stateful = stateful.overflow_x_scroll();
            }
            if scroll_y {
                stateful = stateful.overflow_y_scroll();
            }
            if let Some(at) = el.shows {
                stateful = stateful.track_scroll(&self.shown_scroll(&id_str, at));
            }
            if let Some(tab) = el.drag.clone() {
                stateful = stateful.on_drag(tab, |tab, _, _, cx| cx.new(|_| DragChip(tab.clone())));
            }
            if let Some(action) = el.action.clone() {
                stateful = stateful.cursor_pointer().hover(|s| s.opacity(0.88)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.dispatch_action(action.clone(), window, cx);
                    }),
                );
            }
            return stateful.into_any_element();
        }

        // Under way: it breathes, so a running Eval moves while it runs.
        if el.styles.iter().any(|(k, v)| k == "animation" && v == "pulse") {
            let pulse = Animation::new(Duration::from_millis(1600))
                .repeat()
                .with_easing(::gpui::pulsating_between(0.35, 1.0));
            let id = ElementId::Name(SharedString::from(format!("pulse:{path}")));
            return d.with_animation(id, pulse, |d, t| d.opacity(t)).into_any_element();
        }
        // Still working: its stripe runs along the bar, again and again.
        if el.styles.iter().any(|(k, v)| k == "animation" && v == "slide") {
            let slide = Animation::new(Duration::from_millis(1800))
                .repeat()
                .with_easing(::gpui::ease_in_out);
            let id = ElementId::Name(SharedString::from(format!("slide:{path}")));
            return d
                .with_animation(id, slide, |d, t| d.left(::gpui::relative(t * 1.3 - 0.3)))
                .into_any_element();
        }

        d.into_any_element()
    }

    /// The window's tree, built from the app.
    fn built(&self) -> Element {
        match self.shared.lock() {
            Ok(guard) => {
                let mut tree = guard.app.render();
                keep_whole_in_scrolls(&mut tree);
                tree
            }
            Err(_) => crate::gpui::div(),
        }
    }

    /// The view of a pane's terminal, the same from draw to draw.
    fn terminal_view(
        &self,
        project: &std::path::Path,
        pane: u32,
        cx: &mut Context<Self>,
    ) -> ::gpui::Entity<TerminalView> {
        let key = (project.to_path_buf(), pane);
        self.kept.borrow_mut().terminals.insert(key.clone());
        if let Some(view) = self.terminals.borrow().get(&key) {
            return view.clone();
        }
        let shared = Arc::clone(&self.shared);
        let view = cx.new(|_| TerminalView::new(shared, project.to_path_buf(), pane));
        self.terminals.borrow_mut().insert(key, view.clone());
        view
    }

    /// The view of the file a pane has open, the same from draw to draw;
    /// `None` when the file is no longer open.
    #[cfg(target_os = "macos")]
    fn viewer_view(
        &self,
        id: u64,
        covered: bool,
        cx: &mut Context<Self>,
    ) -> Option<::gpui::Entity<crate::viewer::ViewerView>> {
        use crate::action::FileAct;
        use crate::viewer::{Ask, ViewerView};
        self.kept.borrow_mut().viewers.insert(id);
        let existing = self.viewers.borrow().get(&id).cloned();
        let view = match existing {
            Some(view) => view,
            None => {
                let (page, file) = {
                    let guard = self.shared.lock().ok()?;
                    let v = guard.app.files.get(id)?;
                    (v.page(), v.abs.clone())
                };
                let shared = Arc::clone(&self.shared);
                let asked = move |ask| {
                    if let Ok(mut guard) = shared.lock() {
                        guard.app.dispatch(match ask {
                            Ask::Reveal => FileAct::Reveal(id),
                            Ask::Close => FileAct::Close(id),
                        });
                        guard.dirty = true;
                    }
                };
                let view = cx.new(|_| ViewerView::new(&page, file, asked));
                self.viewers.borrow_mut().insert(id, view.clone());
                view
            }
        };
        view.update(cx, |v, _| v.covered = covered);
        Some(view)
    }

    /// The divider followed to where the pointer is, while it is dragged.
    fn resize(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((from, width)) = self.resizing else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.resizing = None;
            return;
        }
        let to = (width + f32::from(e.position.x) - from).max(0.0);
        if let Ok(mut guard) = self.shared.lock() {
            guard.app.dispatch(crate::action::FileAct::Width(to.round() as u32));
        }
        cx.notify();
    }

    /// A box that holds no terminal and takes its size from around it, as a
    /// view of its own: laid out in its place from its own placing styles,
    /// and drawn inside from the rest.
    fn part(
        &self,
        el: &Element,
        parent_text: InheritedText,
        parent_is_row: bool,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut place = gpui_div();
        for (k, v) in el.styles.iter().filter(|(k, _)| PLACING.contains(&k.as_str())) {
            place = style(place, k, v).unwrap_or_else(|d| d);
        }
        let has = |k: &str| el.styles.iter().any(|(s, _)| s == k);
        if el.styles.iter().any(|(k, v)| k == "align-self" && v == "stretch") {
            if parent_is_row {
                if !has("height") {
                    place = place.h_full();
                }
            } else if !has("width") {
                place = place.w_full();
            }
        }
        let placing: StyleRefinement = Styled::style(&mut place).clone();

        self.kept.borrow_mut().parts.insert(path.to_string());
        let existing = self.parts.borrow().get(path).cloned();
        let part = match existing {
            // Touched on every draw, built or not: the window is told of a
            // part only when its last draw touched it.
            Some(part) => {
                let filled = self.rebuilding.get().then(|| filling(el));
                part.update(cx, |p, _| {
                    if let Some(el) = filled {
                        (p.el, p.text) = (el, parent_text);
                    }
                });
                part
            }
            None => {
                let desktop = cx.weak_entity();
                let (el, path) = (filling(el), path.to_string());
                let part = cx.new(|_| Part {
                    desktop,
                    el,
                    text: parent_text,
                    path: path.clone(),
                    drawn: 0,
                });
                self.parts.borrow_mut().insert(path, part.clone());
                part
            }
        };
        part.cached(placing).into_any_element()
    }

    /// A conversation's list, kept from frame to frame under `key`: it opens
    /// at its end and follows what comes until the person scrolls up, and a
    /// page read in above keeps the place. Only the blocks in sight are laid
    /// out, so a long conversation scrolls as a short one does; a block that
    /// changed is measured again, the others keep their heights.
    #[allow(clippy::too_many_arguments)]
    fn listed(
        &self,
        el: &Element,
        key: &str,
        text: InheritedText,
        path: &str,
        older: Action,
        gap: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut lists = self.lists.borrow_mut();
        let listed = lists.entry(key.to_string()).or_insert_with(|| {
            let state = ListState::new(0, ListAlignment::Top, px(600.0));
            state.set_follow_mode(FollowMode::Tail);
            let (shared, desktop, key) =
                (Arc::clone(&self.shared), cx.weak_entity(), key.to_string());
            state.set_scroll_handler(move |e, _window, cx| {
                // At its top, the page before it is read.
                if e.is_scrolled
                    && e.visible_range.start == 0
                    && let Ok(mut guard) = shared.lock()
                {
                    guard.app.dispatch(older.clone());
                }
                // Scrolled, it draws again, and the window around it with it:
                // the window's tree is as it was.
                let _ = desktop.update(cx, |this, _| this.output_only = true);
                // What is drawn over it changes only at its end, asked once
                // the scroll is done: the list holds its state while it scrolls.
                let (desktop, key) = (desktop.clone(), key.clone());
                cx.defer(move |cx| {
                    let _ = desktop.update(cx, |this, cx| {
                        let was = this.drawn_at_end.borrow().get(&key).copied();
                        if was.is_none_or(|drawn| drawn != this.at_its_end(&key)) {
                            cx.notify();
                        }
                    });
                });
            });
            Listed { state, blocks: Rc::default(), text: Rc::new(Cell::new(text)) }
        });
        listed.text.set(text);
        {
            // The blocks that changed: what lies between the same first ones
            // and the same last ones.
            let mut blocks = listed.blocks.borrow_mut();
            let new = &el.children;
            let first = blocks.iter().zip(new).take_while(|(a, b)| a == b).count();
            let last = blocks[first..]
                .iter()
                .rev()
                .zip(new[first..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            if first < blocks.len().max(new.len()) {
                listed.state.splice(first..blocks.len() - last, new.len() - first - last);
                *blocks = new.clone();
            }
        }
        let (blocks, text, base) =
            (Rc::clone(&listed.blocks), Rc::clone(&listed.text), path.to_string());
        let desktop = cx.weak_entity();
        // Each block as wide as the list, laid out on its own.
        let room = Room { width: true, height: false, flex: true, row: false };
        ::gpui::list(listed.state.clone(), move |ix, _window, cx| {
            let block = blocks.borrow().get(ix).cloned();
            let built = block.zip(desktop.upgrade()).map(|(block, desktop)| {
                let at = format!("{base}/{ix}");
                desktop.update(cx, |view, cx| {
                    view.build_gpui_element(&block, text.get(), false, &at, Some(room), cx)
                })
            });
            let mut row = gpui_div().w_full().flex().flex_col().items_center();
            if ix > 0 {
                row = row.pt(px(gap));
            }
            row.children(built).into_any_element()
        })
        .w_full()
        .flex_1()
        .into_any_element()
    }

    /// Whether a conversation's list, or another scroll, is at its end.
    fn at_its_end(&self, key: &str) -> bool {
        if let Some(listed) = self.lists.borrow().get(key) {
            // Followed, it is at its end; scrolled up, only when it knows it
            // is back there.
            return listed.state.is_following_tail()
                || listed.state.is_scrolled_to_end() == Some(true);
        }
        self.scrolls.borrow().get(key).is_none_or(|(h, _)| at_end(h))
    }

    /// The scroll of an element that keeps its child `at` in view: scrolled
    /// to it when it first shows, whenever another is picked and whenever
    /// the element's width changes, and left where the person scrolls it
    /// otherwise.
    fn shown_scroll(&self, key: &str, at: usize) -> ScrollHandle {
        let mut scrolls = self.scrolls.borrow_mut();
        let (handle, shown) = scrolls.entry(key.to_string()).or_default();
        let now = format!("{at} {:?}", handle.bounds().size.width);
        if shown.as_deref() != Some(now.as_str()) {
            handle.scroll_to_item(at);
            *shown = Some(now);
        }
        handle.clone()
    }
}

/// A part's box as drawn inside its place: every style but those that place
/// it, filling the place.
fn filling(el: &Element) -> Element {
    let mut inner = el.clone();
    inner.styles.retain(|(k, _)| !PLACING.contains(&k.as_str()));
    inner.ends = None;
    inner.style("width", "100%").style("height", "100%")
}

/// A dragged tab, under the pointer: its status and name.
struct DragChip(TabDrag);

impl Render for DragChip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut chip = gpui_div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .bg(parse_color(palette::SELECTED_BG))
            .border_1()
            .border_color(parse_color(palette::BLUE_BORDER))
            .text_color(parse_color(palette::BLUE_BORDER))
            .text_size(px(13.0));
        if let Some((glyph, colour)) = self.0.shape {
            chip = chip.child(
                gpui_div()
                    .font_family(crate::fonts::MONO)
                    .text_size(px(10.0))
                    .text_color(parse_color(colour))
                    .child(glyph),
            );
        }
        chip.child(self.0.label.clone())
    }
}

/// Text of more than one line, or kept as written: each line on its own, and
/// whether it stays on one line rather than wrapping. `None` for text that
/// flows as it is.
fn text_lines(t: &str, pre: bool, nowrap: bool) -> Option<Vec<(String, bool)>> {
    (pre || t.contains('\n')).then(|| {
        t.split('\n')
            .map(|line| (if line.is_empty() { " ".to_string() } else { line.to_string() }, nowrap))
            .collect()
    })
}

/// Every style the views may use. Each is drawn by [`style`]; a view that
/// uses another fails a test.
#[cfg(test)]
pub(crate) const STYLES: &[&str] = &[
    "align-items",
    "align-self",
    "animation",
    "background",
    "border",
    "border-bottom",
    "border-color",
    "border-left",
    "border-radius",
    "border-right",
    "border-top",
    "bottom",
    "color",
    "display",
    "flex",
    "flex-direction",
    "flex-shrink",
    "flex-wrap",
    "font-family",
    "font-size",
    "font-weight",
    "gap",
    "height",
    "inset",
    "justify-content",
    "left",
    "line-height",
    "max-height",
    "max-width",
    "min-height",
    "min-width",
    "opacity",
    "overflow",
    "overflow-x",
    "overflow-y",
    "padding",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "position",
    "right",
    "text-align",
    "text-overflow",
    "top",
    "white-space",
    "width",
    "z-index",
];

/// Whether a scroll is at its end, or was never drawn: within a few pixels,
/// as a wheel leaves it.
pub(crate) fn at_end(handle: &ScrollHandle) -> bool {
    -handle.offset().y >= handle.max_offset().y - px(24.0)
}

/// What a scroll holds is never squeezed to fit it: each child of a column
/// that scrolls down, or of a row that scrolls across, keeps its size on that
/// axis, so it runs past the scroll's end and the scroll moves it. A child
/// clipped with `overflow: hidden` would otherwise shrink to fit and lose
/// its end.
fn keep_whole_in_scrolls(el: &mut Element) {
    let (x, y) = scroll_axes(&el.styles);
    let direction = el.styles.iter().find(|(k, _)| k == "flex-direction").map(|(_, v)| v.as_str());
    let keep = (y && direction == Some("column")) || (x && direction == Some("row"));
    for child in &mut el.children {
        if keep && !child.styles.iter().any(|(k, _)| k == "flex-shrink") {
            child.styles.push(("flex-shrink".into(), "0".into()));
        }
        keep_whole_in_scrolls(child);
    }
}

/// The axes an element scrolls on, `(x, y)`: its `overflow-x` and
/// `overflow-y` of `auto` or `scroll`.
fn scroll_axes(styles: &[(String, String)]) -> (bool, bool) {
    let scrolls =
        |axis: &str| styles.iter().any(|(k, v)| k == axis && (v == "auto" || v == "scroll"));
    (scrolls("overflow-x"), scrolls("overflow-y"))
}

/// One style on a div; the div untouched, as an error, for one the window
/// does not draw.
#[allow(clippy::result_large_err)]
fn style(d: Div, k: &str, v: &str) -> std::result::Result<Div, Div> {
    let n = parse_px(v);
    Ok(match k {
        // The text's own, taken before the box is built.
        "color" | "font-size" | "font-weight" | "font-family" | "line-height" | "white-space"
        | "text-overflow" | "align-self" | "z-index" => d,
        // Drawn once the box is built: it pulses.
        "animation" => d,
        "text-align" => match v {
            "center" => d.text_center(),
            "right" => d.text_right(),
            _ => d,
        },
        "display" => match v {
            "flex" => d.flex(),
            "none" => d.hidden(),
            _ => d,
        },
        "flex-direction" => match v {
            "row" => d.flex().flex_row(),
            "column" => d.flex().flex_col(),
            _ => d,
        },
        "align-items" => match v {
            "center" => d.items_center(),
            "flex-start" => d.items_start(),
            "baseline" => d.items_baseline(),
            _ => d,
        },
        "justify-content" => match v {
            "space-between" => d.justify_between(),
            "center" => d.justify_center(),
            "flex-end" => d.justify_end(),
            _ => d,
        },
        "flex" if v.starts_with('1') => d.flex_1().min_w_0().min_h_0(),
        "flex" => d,
        "flex-shrink" if v == "0" => d.flex_shrink_0(),
        "flex-shrink" => d,
        "flex-wrap" if v == "wrap" => d.flex_wrap(),
        "flex-wrap" => d,
        "gap" => match n {
            Some(n) => d.gap(px(n)),
            None => d,
        },
        "width" if v == "100%" => d.w_full(),
        "width" => match n {
            Some(n) => d.w(px(n)),
            None => d,
        },
        "height" if v == "100%" => d.h_full(),
        "height" => match n {
            Some(n) => d.h(px(n)),
            None => d,
        },
        "min-width" => match n {
            Some(n) => d.min_w(px(n)),
            None => d,
        },
        "min-height" => match n {
            Some(n) => d.min_h(px(n)),
            None => d,
        },
        "max-width" => match n {
            Some(n) => d.max_w(px(n)),
            None => d,
        },
        "max-height" => match n {
            Some(n) => d.max_h(px(n)),
            None => d,
        },
        "padding" => {
            let (t, r, b, l) = parse_box_edges(v);
            d.pt(px(t)).pr(px(r)).pb(px(b)).pl(px(l))
        }
        "padding-top" => d.pt(px(n.unwrap_or(0.0))),
        "padding-right" => d.pr(px(n.unwrap_or(0.0))),
        "padding-bottom" => d.pb(px(n.unwrap_or(0.0))),
        "padding-left" => d.pl(px(n.unwrap_or(0.0))),
        "opacity" => match v.parse::<f32>() {
            Ok(o) => d.opacity(o.clamp(0.0, 1.0)),
            Err(_) => d,
        },
        "border-color" => match parse_border_color(v) {
            Some(c) => d.border_color(c),
            None => d,
        },
        "background" if v == "transparent" => d,
        "background" => d.bg(parse_color(v)),
        "border-radius" => match n {
            Some(n) => d.rounded(px(n)),
            None => d,
        },
        "border" => match parse_border_color(v) {
            Some(c) => {
                let d = if v.starts_with("2px") { d.border_2() } else { d.border_1() };
                let d = d.border_color(c);
                if v.contains("dashed") { d.border_dashed() } else { d }
            }
            None => d,
        },
        "border-top" => match parse_border_color(v) {
            Some(c) => {
                if v.starts_with("2px") { d.border_t_2() } else { d.border_t_1() }.border_color(c)
            }
            None => d,
        },
        "border-bottom" => match parse_border_color(v) {
            Some(c) => d.border_b_1().border_color(c),
            None => d,
        },
        "border-right" => match parse_border_color(v) {
            Some(c) => d.border_r_1().border_color(c),
            None => d,
        },
        "border-left" => match parse_border_color(v) {
            Some(c) => {
                if v.starts_with("2px") { d.border_l_2() } else { d.border_l_1() }.border_color(c)
            }
            None => d,
        },
        "position" => match v {
            "absolute" => d.absolute(),
            "relative" => d.relative(),
            _ => d,
        },
        "inset" if v == "0" => d.inset_0(),
        "inset" => d,
        "top" => match (n, percent(v)) {
            (Some(n), _) => d.top(px(n)),
            (None, Some(f)) => d.top(relative(f)),
            _ => return Err(d),
        },
        "left" => match (n, percent(v)) {
            (Some(n), _) => d.left(px(n)),
            (None, Some(f)) => d.left(relative(f)),
            _ => return Err(d),
        },
        "right" => match (n, percent(v)) {
            (Some(n), _) => d.right(px(n)),
            (None, Some(f)) => d.right(relative(f)),
            _ => return Err(d),
        },
        "bottom" => match (n, percent(v)) {
            (Some(n), _) => d.bottom(px(n)),
            (None, Some(f)) => d.bottom(relative(f)),
            _ => return Err(d),
        },
        "overflow" if v == "hidden" => d.overflow_hidden(),
        "overflow-x" if v == "hidden" => d.overflow_x_hidden(),
        "overflow" | "overflow-x" | "overflow-y" => d,
        _ => return Err(d),
    })
}

/// The window's text input, for the field that has the keyboard: typing,
/// what an input method composes and commits, and where it puts its window.
impl EntityInputHandler for GpuiDesktopView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let mut guard = self.shared.lock().ok()?;
        let text = focused(&mut guard.app)?.text().to_string();
        let bytes = field_view::range_from_utf16(&text, &range);
        adjusted.replace(field_view::range_to_utf16(&text, &bytes));
        Some(text[bytes].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let mut guard = self.shared.lock().ok()?;
        let f = focused(&mut guard.app)?;
        Some(UTF16Selection {
            range: field_view::range_to_utf16(f.text(), &f.selection()),
            reversed: f.reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let mut guard = self.shared.lock().ok()?;
        let f = focused(&mut guard.app)?;
        f.marked.as_ref().map(|m| field_view::range_to_utf16(f.text(), m))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Ok(mut guard) = self.shared.lock()
            && let Some(f) = focused(&mut guard.app)
        {
            f.marked = None;
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Ok(mut guard) = self.shared.lock()
            && let Some(f) = focused(&mut guard.app)
        {
            let bytes = range.map(|r| field_view::range_from_utf16(f.text(), &r));
            match bytes {
                Some(r) => f.replace(Some(r), text, true),
                None if f.marked.is_some() => f.replace(None, text, false),
                None => f.type_text(text),
            }
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Ok(mut guard) = self.shared.lock()
            && let Some(f) = focused(&mut guard.app)
        {
            let bytes = range.map(|r| field_view::range_from_utf16(f.text(), &r));
            let inside = selected.map(|r| field_view::range_from_utf16(text, &r));
            f.mark(bytes, text, inside);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let text = {
            let mut guard = self.shared.lock().ok()?;
            focused(&mut guard.app)?.text().to_string()
        };
        self.painted.borrow().bounds_for(field_view::range_from_utf16(&text, &range))
    }

    fn character_index_for_point(
        &mut self,
        at: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let ix = self.painted.borrow().index_for(at)?;
        let mut guard = self.shared.lock().ok()?;
        Some(field_view::to_utf16(focused(&mut guard.app)?.text(), ix))
    }
}

impl Render for GpuiDesktopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The window is needed only for the file pages, which only macOS draws.
        #[cfg(not(target_os = "macos"))]
        let _ = &window;
        // Drawn for terminals' output alone, the tree and its parts are as
        // they were; told anything else, both are built again.
        let output_only = std::mem::take(&mut self.output_only);
        let rebuild = std::mem::take(&mut self.stale) || !output_only || self.last.is_none();
        if rebuild {
            if !std::mem::take(&mut self.fresh) {
                self.last = Some(self.built());
            }
            self.builds += 1;
            *self.kept.borrow_mut() = Default::default();
        }
        self.rebuilding.set(rebuild);
        let root_text = InheritedText {
            color: parse_color(palette::TEXT),
            font_size: 13.0,
            font_weight: FontWeight(400.0),
            font_mono: false,
            line_height: None,
            nowrap: false,
            ellipsis: false,
            pre: false,
        };
        let drawn = gpui_div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _window, cx| this.resize(e, cx)))
            // A click anywhere on Weft's own view takes the keyboard back
            // from a file's page, or from the window once a page is gone.
            .capture_any_mouse_down(|_: &MouseDownEvent, _window, _cx| {
                #[cfg(target_os = "macos")]
                crate::viewer::take_keys(_window);
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, _cx| this.resizing = None),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(parse_color(palette::CANVAS))
            .text_color(parse_color(palette::TEXT))
            .font_family(crate::fonts::SANS);
        let room = Room { width: true, height: true, flex: true, row: false };
        let root_el = self.last.as_ref().expect("built above");
        let root = self.build_gpui_element(root_el, root_text, false, "0", Some(room), cx);
        if rebuild {
            // What this build did not draw is gone.
            let kept = self.kept.borrow();
            self.terminals.borrow_mut().retain(|k, _| kept.terminals.contains(k));
            self.parts.borrow_mut().retain(|k, _| kept.parts.contains(k));
            // A file still open but not drawn keeps its page, out of sight;
            // a file closed takes its page with it.
            #[cfg(target_os = "macos")]
            if let Ok(guard) = self.shared.lock() {
                let files = &guard.app.files;
                let before = self.viewers.borrow().len();
                self.viewers.borrow_mut().retain(|id, view| {
                    let open = files.get(*id).is_some();
                    if open && !kept.viewers.contains(id) {
                        view.read(cx).hide();
                    }
                    open
                });
                // A page closed takes the keyboard with it, if it had it:
                // Weft's view takes it first.
                if self.viewers.borrow().len() < before {
                    crate::viewer::take_keys(window);
                }
            }
        }
        drawn.child(root)
    }
}

/// One of Weft's own pictures, drawn from its SVG at the size its styles give.
fn picture(el: &Element) -> AnyElement {
    let named = |k: &str| el.attrs.iter().chain(&el.styles).find(|(n, _)| n == k).map(|(_, v)| v);
    let bytes = match named("svg") {
        Some(svg) => svg.as_bytes(),
        None => match named("src").and_then(|n| crate::gpui::asset(n)) {
            Some(bytes) => bytes,
            None => return gpui_div().into_any_element(),
        },
    };
    let image = Arc::new(::gpui::Image::from_bytes(::gpui::ImageFormat::Svg, bytes.to_vec()));
    let mut picture = ::gpui::img(image).flex_none();
    if let Some(w) = named("width").and_then(|v| parse_px(v)) {
        picture = picture.w(px(w));
    }
    if let Some(h) = named("height").and_then(|v| parse_px(v)) {
        picture = picture.h(px(h));
    }
    picture.into_any_element()
}

/// One of Weft's own pictures turning round its centre, as a spinner does:
/// drawn in the accent, from the asset source.
fn spinning(el: &Element, path: &str) -> AnyElement {
    let named = |k: &str| el.attrs.iter().chain(&el.styles).find(|(n, _)| n == k).map(|(_, v)| v);
    let Some(src) = named("src") else { return gpui_div().into_any_element() };
    let side = named("width").and_then(|v| parse_px(v)).unwrap_or(16.0);
    let turn = Animation::new(Duration::from_millis(2400)).repeat();
    ::gpui::svg()
        .path(SharedString::from(src.clone()))
        .size(px(side))
        .text_color(parse_color(palette::BLUE_BORDER))
        .with_animation(
            ElementId::Name(SharedString::from(format!("spin:{path}"))),
            turn,
            |s, t| s.with_transformation(::gpui::Transformation::rotate(::gpui::percentage(t))),
        )
        .into_any_element()
}

/// Weft's own pictures, for the elements that draw from a path.
struct Pictures;

impl ::gpui::AssetSource for Pictures {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        Ok(crate::gpui::asset(path).map(std::borrow::Cow::Borrowed))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// `50%` as a fraction.
fn percent(s: &str) -> Option<f32> {
    s.trim().strip_suffix('%')?.parse::<f32>().ok().map(|p| p / 100.0)
}

fn parse_px(s: &str) -> Option<f32> {
    s.trim().trim_end_matches("px").parse::<f32>().ok()
}

fn parse_box_edges(s: &str) -> (f32, f32, f32, f32) {
    let parts: Vec<f32> = s.split_whitespace().map(|p| parse_px(p).unwrap_or(0.0)).collect();
    match parts.as_slice() {
        [all] => (*all, *all, *all, *all),
        [v, h] => (*v, *h, *v, *h),
        [t, h, b] => (*t, *h, *b, *h),
        [t, r, b, l, ..] => (*t, *r, *b, *l),
        [] => (0.0, 0.0, 0.0, 0.0),
    }
}

fn parse_border_color(s: &str) -> Option<Rgba> {
    let color_str = s.split_whitespace().last()?;
    if color_str == "none" || color_str == "transparent" {
        return None;
    }
    Some(parse_color(color_str))
}

fn parse_color(s: &str) -> Rgba {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6
            && let Ok(val) = u32::from_str_radix(hex, 16)
        {
            return rgba((val << 8) | 0xff);
        }
        if hex.len() == 8
            && let Ok(val) = u32::from_str_radix(hex, 16)
        {
            return rgba(val);
        }
    }
    rgba(0x00000000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{Wire, start, wired_app};
    use crate::daemon::PaneView;
    use ::gpui::{TestAppContext, VisualTestContext};
    use weft_core::state::SavedPaneTab;
    use weft_core::text_keys::Edit;

    /// A Linux desktop binds the window to Weft's entry, and its icon, by the
    /// window's app id.
    #[test]
    fn the_window_is_known_by_the_desktop_entrys_name() {
        let entry = include_str!("../../../assets/linux/weft.desktop");
        assert!(entry.lines().any(|l| l == format!("StartupWMClass={APP_ID}")), "{entry}");
    }

    #[test]
    fn a_newer_app_opens_to_this_one_s_state_written() {
        let (tmp, mut app, _wire) = wired_app();
        app.state.folded_projects.push(PathBuf::from("/work/folded"));
        let _ = app.save_state();
        let want = app.state.clone();
        let shared = Mutex::new(SharedState {
            app,
            dirty: false,
            grids: HashMap::new(),
            instance: None,
            raise: false,
            output: Vec::new(),
        });
        let opened = hand_over(&shared, || {
            assert_eq!(crate::app::load_state_from_home(tmp.path()), want, "written before");
            true
        });
        assert!(opened && shared.lock().unwrap().app.quit_requested);
    }

    /// The desktop's view in a window without a screen, a pane open and the message row focused.
    fn opened(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Arc<Mutex<SharedState>>, &mut VisualTestContext) {
        let (tmp, shared, _view, _wire, cx) = opened_view(cx);
        (tmp, shared, cx)
    }

    /// The same, with the view, for what the window's text input hands it.
    fn opened_view(
        cx: &mut TestAppContext,
    ) -> (
        tempfile::TempDir,
        Arc<Mutex<SharedState>>,
        ::gpui::Entity<GpuiDesktopView>,
        Wire,
        &mut VisualTestContext,
    ) {
        let (tmp, mut app, mut wire) = wired_app();
        start(&mut app, &mut wire, "claude");
        app.focus = Focus::Message;
        let shared = Arc::new(Mutex::new(SharedState {
            app,
            dirty: true,
            grids: HashMap::new(),
            instance: None,
            raise: false,
            output: Vec::new(),
        }));
        let view_shared = Arc::clone(&shared);
        let (view, cx) =
            cx.add_window_view(|window, cx| GpuiDesktopView::new(view_shared, window, cx));
        cx.run_until_parked();
        (tmp, shared, view, wire, cx)
    }

    /// Run `test` with a window context, on a thread with a main thread's stack:
    /// the screen's element tree is deep.
    fn windowed(test: impl FnOnce(&mut TestAppContext) + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(move || test(&mut TestAppContext::single()))
            .unwrap()
            .join()
            .unwrap();
    }

    /// This system's key for `edit`, as a test types it.
    fn key(edit: Edit) -> &'static str {
        weft_core::text_keys::keys(System::this()).find(|(_, e)| *e == edit).expect("a key").0
    }

    /// Take what the daemon sent until something is to be drawn.
    fn pumped(shared: &Arc<Mutex<SharedState>>) -> crate::daemon::Redraw {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let redraw = shared.lock().unwrap().app.pump();
            if redraw.window || !redraw.terminals.is_empty() {
                return redraw;
            }
            assert!(std::time::Instant::now() < deadline, "nothing came");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A pane's output draws its terminal again and nothing else: the tree
    /// is not built again and no part of the window is laid out or drawn
    /// again. Anything else builds and draws all of it, as before.
    #[test]
    fn a_panes_output_draws_its_terminal_and_not_the_window() {
        windowed(|cx| {
            let (_tmp, shared, view, mut wire, cx) = opened_view(cx);
            let key = {
                let g = shared.lock().unwrap();
                let tab = &g.app.state.panes.groups[0].tabs[0];
                (tab.project.clone(), tab.pane)
            };
            let counts = |cx: &mut VisualTestContext| {
                view.read_with(cx, |v, cx| {
                    let parts: usize = v.parts.borrow().values().map(|p| p.read(cx).drawn).sum();
                    let terminal = v.terminals.borrow()[&key].read(cx).drawn;
                    (v.builds, parts, terminal)
                })
            };
            let (builds, parts, terminal) = counts(cx);
            assert!(parts > 0, "the tree and the bars are parts of their own");

            wire.event(weft_proto::Event::Output {
                pane: key.1,
                seq: 1_000,
                bytes: b"hello from the agent".to_vec(),
            });
            let redraw = pumped(&shared);
            assert_eq!(
                redraw,
                crate::daemon::Redraw { window: false, terminals: vec![key.clone()] },
                "output alone redraws its terminal"
            );
            view.update(cx, |v, cx| v.draw_terminals(&redraw.terminals, cx));
            cx.run_until_parked();
            let after = counts(cx);
            assert_eq!((after.0, after.1), (builds, parts), "nothing built or laid out again");
            assert!(after.2 > terminal, "the terminal drew again");
            let shown = shared.lock().unwrap().app.pane_view(&key.0, key.1).unwrap().contents();
            assert!(shown.contains("hello from the agent"), "{shown}");

            // Told of anything else, the tree is built again; a part drawn
            // as it was is not drawn again, and one that changed is.
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            let told = counts(cx);
            assert!(told.0 > after.0, "built again: {after:?} then {told:?}");
            assert_eq!(told.1, after.1, "no part changed, none drawn again");
            let each = |cx: &mut VisualTestContext| {
                view.read_with(cx, |v, cx| {
                    v.parts
                        .borrow()
                        .iter()
                        .map(|(k, p)| (k.clone(), p.read(cx).drawn))
                        .collect::<HashMap<_, _>>()
                })
            };
            let before = each(cx);
            shared.lock().unwrap().app.state.settings.sidebar_open = false;
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            let again = each(cx)
                .into_iter()
                .filter(|(k, n)| before.get(k).is_some_and(|was| n > was))
                .count();
            assert!(again > 0, "the parts it moved drawn again");
        });
    }

    /// Output that takes a pane scrolled back to its end hides the way back
    /// to the end, so it redraws the window.
    #[test]
    fn output_that_ends_a_panes_scroll_back_redraws_the_window() {
        let (_tmp, mut app, mut wire) = wired_app();
        let pane = start(&mut app, &mut wire, "claude");
        let root = app.clients[0].root.clone();
        let lines: String = (0..100).map(|i| format!("line {i}\r\n")).collect();
        wire.event(weft_proto::Event::Output { pane, seq: 1, bytes: lines.into_bytes() });
        let shared = Arc::new(Mutex::new(SharedState {
            app,
            dirty: false,
            grids: HashMap::new(),
            instance: None,
            raise: false,
            output: Vec::new(),
        }));
        assert_eq!(pumped(&shared).terminals, vec![(root.clone(), pane)]);
        shared.lock().unwrap().app.pane_view_mut(&root, pane).unwrap().scroll(5);
        // The program takes the other screen, which has no history.
        wire.event(weft_proto::Event::Output { pane, seq: 2, bytes: b"\x1b[?1049h".to_vec() });
        let redraw = pumped(&shared);
        assert!(redraw.window, "the button over the pane goes");
        assert_eq!(redraw.terminals, vec![(root, pane)]);
    }

    fn message(shared: &Arc<Mutex<SharedState>>) -> String {
        shared.lock().unwrap().app.message_field().text().to_string()
    }

    #[test]
    fn typing_goes_into_the_message_row_shift_enter_adds_a_line_and_enter_sends() {
        windowed(|cx| {
            let (_tmp, shared, cx) = opened(cx);
            cx.simulate_input("fix 日本");
            cx.simulate_keystrokes("shift-enter");
            cx.simulate_input("then test it");
            assert_eq!(message(&shared), "fix 日本\nthen test it");
            cx.simulate_keystrokes(&format!(
                "{} {}",
                key(Edit::WordLeft),
                key(Edit::DeleteWordBack)
            ));
            assert_eq!(message(&shared), "fix 日本\nthen it");
            cx.simulate_keystrokes(key(Edit::Undo));
            assert_eq!(message(&shared), "fix 日本\nthen test it");
            cx.simulate_keystrokes("enter");
            assert_eq!(message(&shared), "", "sent");
        });
    }

    #[test]
    fn escape_leaves_the_field_and_keys_go_to_the_agent_again() {
        windowed(|cx| {
            let (_tmp, shared, cx) = opened(cx);
            cx.simulate_input("ab");
            cx.simulate_keystrokes("escape");
            cx.simulate_input("c");
            assert_eq!(message(&shared), "ab");
        });
    }

    #[test]
    fn a_pasted_message_keeps_its_lines_and_a_pasted_search_does_not() {
        windowed(|cx| {
            let (_tmp, shared, cx) = opened(cx);
            cx.write_to_clipboard(ClipboardItem::new_string("one\ntwo".into()));
            cx.simulate_keystrokes(key(Edit::Paste));
            assert_eq!(message(&shared), "one\ntwo");
            shared.lock().unwrap().app.search.paste("one\ntwo");
            assert_eq!(shared.lock().unwrap().app.search.text(), "one two");
        });
    }

    #[test]
    fn escape_leaves_the_message_field_and_tab_types_nothing_into_it() {
        windowed(|cx| {
            let (_tmp, shared, _view, _wire, cx) = opened_view(cx);
            cx.simulate_input("run the");
            cx.simulate_keystrokes("tab");
            assert_eq!(message(&shared), "run the");
            cx.simulate_keystrokes("escape");
            assert_eq!(shared.lock().unwrap().app.focus, Focus::Terminal);
        });
    }

    /// An input method asks where its window goes as soon as it marks a
    /// draft, before the field is painted again: past what was painted, it is
    /// told the end of the last painted line, never nothing.
    #[test]
    fn an_input_method_is_told_a_place_past_what_was_painted() {
        windowed(|cx| {
            let (_tmp, _shared, view, _wire, cx) = opened_view(cx);
            cx.simulate_input("say ");
            cx.run_until_parked();
            let (end, past) = view.update_in(cx, |v, window, cx| {
                let any = Bounds::default();
                let end = v.bounds_for_range(4..4, any, window, cx);
                v.replace_and_mark_text_in_range(None, "nihao", None, window, cx);
                (end, v.bounds_for_range(9..9, any, window, cx))
            });
            assert!(end.is_some(), "the field was painted");
            assert_eq!(past, end, "the end of what was painted");
        });
    }

    /// What an input method does, as macOS hands it over: a composing draft,
    /// marked and replaced as it changes, then the committed characters.
    #[test]
    fn an_input_method_composes_in_the_message_field_and_enter_meanwhile_is_its_own() {
        windowed(|cx| {
            let (_tmp, shared, view, mut wire, cx) = opened_view(cx);
            cx.simulate_input("say ");
            view.update_in(cx, |v, window, cx| {
                v.replace_and_mark_text_in_range(None, "n", None, window, cx);
                v.replace_and_mark_text_in_range(None, "ni", None, window, cx);
                v.replace_and_mark_text_in_range(None, "nihao", None, window, cx);
            });
            assert_eq!(message(&shared), "say nihao");
            let marked = view.update_in(cx, |v, window, cx| v.marked_text_range(window, cx));
            assert_eq!(marked, Some(4..9), "the draft is marked, in UTF-16 units");
            // The commit takes the draft's place, unmarked.
            view.update_in(cx, |v, window, cx| v.replace_text_in_range(None, "你好", window, cx));
            assert_eq!(message(&shared), "say 你好");
            assert_eq!(view.update_in(cx, |v, window, cx| v.marked_text_range(window, cx)), None);
            cx.simulate_input("!");
            assert_eq!(message(&shared), "say 你好!");
            // Enter while composing is the input method's: Weft sends nothing.
            wire.calls();
            view.update_in(cx, |v, window, cx| {
                v.replace_and_mark_text_in_range(None, "a", None, window, cx)
            });
            cx.simulate_keystrokes("enter");
            let sent = wire
                .calls()
                .into_iter()
                .filter(|(_, c)| matches!(c, weft_proto::Call::Input { .. }));
            assert_eq!(sent.count(), 0, "not sent while composing");
        });
    }

    /// Two agents' conversations in one pane: each opens at its end and
    /// keeps where the person left it while the other is shown.
    #[test]
    fn each_tab_keeps_its_own_place_in_its_conversation() {
        windowed(|cx| {
            let (_tmp, shared, view, _wire, cx) = opened_view(cx);
            let panes = {
                let mut g = shared.lock().unwrap();
                let app = &mut g.app;
                let root = app.clients[0].root.clone();
                app.state.panes.groups[0].tabs.clear();
                let mut panes = Vec::new();
                for n in 0..2u32 {
                    let pane = 200 + n;
                    app.clients[0].panes.push(PaneView::new(pane, "claude".into(), "sh".into()));
                    app.ensure_tab_in_active_pane(SavedPaneTab {
                        project: root.clone(),
                        pane,
                        view: weft_core::state::PaneViewMode::Conversation,
                    });
                    let items = (0..40)
                        .map(|i| weft_core::conversation::Item {
                            id: format!("{n}.{i}"),
                            kind: weft_core::conversation::Kind::Person,
                            at: None,
                            text: Some(format!("message {i} of agent {n}")),
                            tool: None,
                            arg: None,
                            call: None,
                            failed: None,
                            refs: Vec::new(),
                            mentions: Vec::new(),
                        })
                        .collect();
                    app.conversations.insert((root.clone(), pane), items);
                    panes.push(pane);
                }
                g.dirty = true;
                panes
            };
            let show = |cx: &mut VisualTestContext, tab: Option<usize>| {
                if let Some(tab) = tab {
                    let mut g = shared.lock().unwrap();
                    g.app.dispatch(PaneAct::SelectTab { group: 0, tab });
                    g.dirty = true;
                }
                for _ in 0..3 {
                    cx.update(|window, _| window.refresh());
                    cx.run_until_parked();
                }
            };
            let key = |pane: u32| {
                let root = shared.lock().unwrap().app.clients[0].root.clone();
                let id = crate::panes_view::conversation_id(&root, pane);
                crate::gpui::scroll_key("pane conversation", &id)
            };
            let handle = |cx: &mut VisualTestContext, pane: u32| {
                view.read_with(cx, |v, _| {
                    let lists = v.lists.borrow();
                    let mine = lists.get(&key(pane));
                    assert!(mine.is_some(), "a list for pane {pane}: {:?}", lists.keys());
                    mine.unwrap().state.clone()
                })
            };
            let at_end = |cx: &mut VisualTestContext, pane: u32| {
                view.read_with(cx, |v, _| v.at_its_end(&key(pane)))
            };

            show(cx, None);
            let (first, second) = (panes[0], panes[1]);
            assert_eq!(shared.lock().unwrap().app.state.panes.groups[0].active_tab, 1);
            assert!(at_end(cx, second), "the conversation opens at its end");
            assert!(cx.debug_bounds("scroll to end").is_none(), "at its end, no button");
            // Read further up the second, then show the first.
            let wheel = |cx: &mut VisualTestContext, dy: f32| {
                let delta = ::gpui::ScrollDelta::Pixels(point(px(0.0), px(dy)));
                let position = point(px(640.0), px(300.0));
                cx.simulate_event(ScrollWheelEvent { position, delta, ..Default::default() });
                cx.run_until_parked();
            };
            wheel(cx, 200.0);
            assert!(!at_end(cx, second), "the wheel scrolls it up");
            assert!(cx.debug_bounds("scroll to end").is_some(), "scrolled up, the button");
            let up = ::gpui::ListOffset { item_ix: 10, offset_in_item: px(0.0) };
            handle(cx, second).scroll_to(up);
            show(cx, Some(0));
            assert!(at_end(cx, first), "the first opens at its own end");
            show(cx, Some(1));
            assert_eq!(
                handle(cx, second).logical_scroll_top().item_ix,
                10,
                "the second is where it was left"
            );
            // Away from its end, the way back to it; one press, and it is gone.
            assert!(cx.debug_bounds("scroll to end").is_some(), "scrolled up, the button");
            let project = shared.lock().unwrap().app.clients[0].root.clone();
            view.update_in(cx, |v, window, cx| {
                v.dispatch_action(PaneAct::ToEnd { project, pane: second }.into(), window, cx)
            });
            shared.lock().unwrap().dirty = true;
            show(cx, None);
            assert!(at_end(cx, second), "at its end");
            assert!(cx.debug_bounds("scroll to end").is_none(), "at its end, no button");
        });
    }

    /// Twelve agents in one pane of a 960-px window: the strip scrolls
    /// rather than squeezing them, and the tab in front is in view.
    #[test]
    fn twelve_tabs_keep_their_labels_whole_and_the_controls_inside_the_pane() {
        windowed(|cx| {
            let (_tmp, shared, cx) = opened(cx);
            let labels: Vec<String> = {
                let mut g = shared.lock().unwrap();
                let app = &mut g.app;
                let root = app.clients[0].root.clone();
                for n in 1..=11 {
                    let pane = 100 + n;
                    let harness = format!("agent-{n:02}");
                    app.clients[0].panes.push(PaneView::new(pane, harness, "sh".into()));
                    app.ensure_tab_in_active_pane(SavedPaneTab {
                        project: root.clone(),
                        pane,
                        view: Default::default(),
                    });
                }
                g.dirty = true;
                let strip = g.app.render().find_layer("tabs").expect("the tabs").clone();
                strip.children.iter().filter_map(|t| t.layer_name.clone()).collect()
            };
            assert_eq!(labels.len(), 12);
            cx.simulate_resize(size(px(960.0), px(640.0)));
            for _ in 0..2 {
                cx.update(|window, _| window.refresh());
                cx.run_until_parked();
            }
            let mut at = |name: &str| {
                let name: &'static str = Box::leak(name.to_string().into_boxed_str());
                cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} was not drawn"))
            };
            let (pane, tabs, controls) = (at("pane group 0"), at("tabs"), at("view controls"));
            assert!(controls.right() <= pane.right(), "{controls:?} inside {pane:?}");
            assert!(controls.left() >= tabs.right(), "the tabs end where the controls begin");
            let mut width = px(0.0);
            for label in &labels {
                let tab = at(label);
                let close = at(&label.replacen("tab ", "close ", 1));
                assert!(
                    close.right() <= tab.right(),
                    "{label}: its ✕ is inside it, its label whole"
                );
                width += tab.size.width;
            }
            assert!(width > tabs.size.width, "twelve tabs are wider than the strip");
            let front = at(labels.last().unwrap());
            assert!(
                front.left() >= tabs.left() && front.right() <= tabs.right(),
                "the tab in front {front:?} is in view in {tabs:?}"
            );
        });
    }

    /// Lay the window out at 960 by 640 and give `name`'s bounds.
    fn laid_out(cx: &mut VisualTestContext, name: &'static str) -> Bounds<Pixels> {
        cx.simulate_resize(size(px(960.0), px(640.0)));
        for _ in 0..2 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} was not drawn"))
    }

    /// A change in the Eval view with more to say than the window holds: its
    /// detail ends inside the window, so it scrolls instead of running off.
    #[test]
    fn a_long_change_scrolls_inside_the_eval_view() {
        windowed(|cx| {
            let (_tmp, shared, _view, mut wire, cx) = opened_view(cx);
            {
                let mut g = shared.lock().unwrap();
                let mut view = crate::app::tests::an_eval_view();
                view.items[0].why = (0..80).map(|n| format!("Reason {n} it is here.")).collect();
                crate::app::tests::open_view(&mut g.app, &mut wire, &view);
                g.dirty = true;
            }
            let detail = laid_out(cx, "eval detail");
            assert!(detail.bottom() <= px(640.0), "{detail:?} ends inside the window");
            let list = laid_out(cx, "eval items");
            assert!(list.bottom() <= px(640.0), "{list:?} ends inside the window");
        });
    }

    /// A change's lines, more than the window holds: they scroll inside the
    /// window instead of being cut off.
    #[test]
    fn a_changes_long_lines_scroll_inside_the_eval_view() {
        windowed(|cx| {
            let (_tmp, shared, _view, mut wire, cx) = opened_view(cx);
            let block = {
                let mut g = shared.lock().unwrap();
                let mut view = crate::app::tests::an_eval_view();
                let at = view.items.iter().position(|i| !i.windows.is_empty()).expect("lines");
                let window = view.items[at].windows[0].clone();
                view.items[at].windows.truncate(1);
                let key = view.items[at].key.clone();
                crate::app::tests::open_view(&mut g.app, &mut wire, &view);
                let text: String = (1..=200).map(|n| format!("line {n}\n")).collect();
                let then =
                    crate::daemon::Then::Window { eval: "evl_3".into(), window: window.clone() };
                g.app.clients[0].read.insert(then, serde_json::json!({ "text": text }));
                if let Some(crate::app::ActiveSheet::EvalView { at, .. }) = &mut g.app.active_sheet
                {
                    *at = vec![key, "lines".into()];
                }
                g.dirty = true;
                Box::leak(format!("lines {window}").into_boxed_str()) as &'static str
            };
            let pane = laid_out(cx, "eval lines");
            assert!(pane.bottom() <= px(640.0), "{pane:?} ends inside the window");
            let code = cx.debug_bounds(block).expect("the lines drawn");
            assert!(code.size.height >= px(200.0 * 22.0), "every line keeps its height: {code:?}");
        });
    }

    /// Every sheet in a window too short for what it says: each of its
    /// scrolls ends inside the window, so what does not fit scrolls instead
    /// of running off its foot.
    #[test]
    fn every_sheets_scrolls_end_inside_a_short_window() {
        use crate::action::SettingsTab;
        use crate::app::ActiveSheet;
        windowed(|cx| {
            let (_tmp, shared, _view, mut wire, cx) = opened_view(cx);
            let root = shared.lock().unwrap().app.clients[0].root.clone();
            {
                let mut g = shared.lock().unwrap();
                let view = crate::app::tests::an_eval_view();
                crate::app::tests::open_view(&mut g.app, &mut wire, &view);
            }
            let p = || root.clone();
            let sheets = vec![
                ActiveSheet::Settings { tab: SettingsTab::General },
                ActiveSheet::Settings { tab: SettingsTab::Eval },
                ActiveSheet::Settings { tab: SettingsTab::Rules },
                ActiveSheet::Settings { tab: SettingsTab::Apps },
                ActiveSheet::ProjectSettings { project: p() },
                ActiveSheet::Ask { project: p(), send_to: None, follow_up_unit: None },
                ActiveSheet::Unit { project: p(), unit: "ask_4".into() },
                ActiveSheet::WorkDetail { project: p(), unit: "ask_4".into() },
                ActiveSheet::EvalDetail { project: p(), eval: "evl_2".into() },
                ActiveSheet::EvalView { project: p(), eval: "evl_3".into(), at: Vec::new() },
                ActiveSheet::SealDetail { project: p(), seal: "sel_4".into() },
                ActiveSheet::SessionDetail { project: p(), session: "s1".into() },
                ActiveSheet::Updates,
                ActiveSheet::Remote,
            ];
            for sheet in sheets {
                let scrolls = {
                    let mut g = shared.lock().unwrap();
                    g.app.active_sheet = Some(sheet.clone());
                    g.dirty = true;
                    let mut names = Vec::new();
                    scrolls_in(&g.app.render(), &mut names);
                    names
                };
                cx.simulate_resize(size(px(960.0), px(360.0)));
                for _ in 0..2 {
                    cx.update(|window, _| window.refresh());
                    cx.run_until_parked();
                }
                let mut measured = 0;
                for name in scrolls {
                    let name: &'static str = Box::leak(name.into_boxed_str());
                    let Some(b) = cx.debug_bounds(name) else { continue };
                    measured += 1;
                    assert!(b.bottom() <= px(360.0), "{sheet:?}: {name} {b:?} ends inside");
                }
                assert!(measured > 0, "{sheet:?}: a scroll of it was drawn");
            }
        });
    }

    /// The dialogs over the window, each with more than a short window
    /// holds, by the scroll each has: the Seal's, an Eval's with many
    /// findings, an Ask's long prompt. Each of their scrolls ends inside the
    /// window.
    #[test]
    fn every_dialogs_scrolls_end_inside_a_short_window() {
        windowed(|cx| {
            let (_tmp, shared, _view, mut wire, cx) = opened_view(cx);
            let findings: Vec<_> = (0..40)
                .map(|n| {
                    serde_json::json!({
                        "kind": "data_bulk", "path": format!("data/set-{n}/"), "files": 900,
                        "kinds_of_files": [["json", 900]],
                        "fix": {"file": ".gitattributes",
                                "line": format!("data/set-{n}/** linguist-generated=true")},
                    })
                })
                .collect();
            let reads = serde_json::json!({"reads": {
                "since": "ask", "when": "2026-11-08T09:12:00Z", "commits": 52,
                "files": 13_852, "skipped": {}, "large": false, "findings": findings,
            }});
            let staged = |id: &str, what: &str, payload: Vec<u8>| crate::daemon::Staged {
                id: id.into(),
                pane: u32::MAX,
                what: what.into(),
                why: String::new(),
                payload,
                sends: None,
                go: None,
            };
            let long_prompt: String =
                (0..120).map(|n| format!("- rule {n}: keep it plain\n")).collect();
            type Open = Box<dyn Fn(&mut crate::app::WeftDesktopApp, &mut crate::app::tests::Wire)>;
            let dialogs: Vec<(&str, Open)> = vec![
                ("Seal dialog", Box::new(crate::app::seal_tests::many_questions)),
                (
                    "findings",
                    Box::new(move |app, _| {
                        app.clients[0].waiting = vec![staged(
                            "p1",
                            "Eval this work?",
                            serde_json::to_vec(&reads).unwrap(),
                        )]
                    }),
                ),
                (
                    "payload",
                    Box::new(move |app, _| {
                        app.clients[0].waiting =
                            vec![staged("p2", "Ask Codex?", long_prompt.clone().into_bytes())]
                    }),
                ),
            ];
            for (dialog, open) in dialogs {
                let scrolls = {
                    let mut g = shared.lock().unwrap();
                    g.app.bottom_bar = crate::app::BottomBarState::None;
                    g.app.clients[0].waiting.clear();
                    open(&mut g.app, &mut wire);
                    g.dirty = true;
                    let mut names = Vec::new();
                    scrolls_in(&g.app.render(), &mut names);
                    names
                };
                cx.simulate_resize(size(px(960.0), px(360.0)));
                for _ in 0..2 {
                    cx.update(|window, _| window.refresh());
                    cx.run_until_parked();
                }
                assert!(scrolls.iter().any(|s| s == dialog), "{dialog} open: {scrolls:?}");
                for name in scrolls {
                    let name: &'static str = Box::leak(name.into_boxed_str());
                    let Some(b) = cx.debug_bounds(name) else {
                        assert_ne!(name, dialog, "{dialog} was drawn");
                        continue;
                    };
                    assert!(b.bottom() <= px(360.0), "{dialog}: {name} {b:?} ends inside");
                }
            }
        });
    }

    /// The names of the scrolls in `el`, every one named.
    fn scrolls_in(el: &Element, names: &mut Vec<String>) {
        if scroll_axes(&el.styles).1 {
            let name = el.layer_name.clone();
            names.push(name.unwrap_or_else(|| panic!("a scroll with no name: {:?}", el.styles)));
        }
        for c in &el.children {
            scrolls_in(c, names);
        }
    }

    /// A finding with a very long path and line, in the Eval dialog of a
    /// 960-px window: its words and its line end inside its box, and its
    /// button stays whole within it.
    #[test]
    fn a_long_finding_stays_inside_the_eval_dialog() {
        windowed(|cx| {
            let (_tmp, shared, cx) = opened(cx);
            let long = format!("{}/", "a-folder-with-a-long-name".repeat(20));
            {
                let mut g = shared.lock().unwrap();
                let finding = serde_json::json!({
                    "kind": "data_bulk", "path": long, "files": 13_708,
                    "kinds_of_files": [["json", 13_708]],
                    "fix": {"file": ".gitattributes",
                            "line": format!("{long}** linguist-generated=true")},
                });
                let reads = serde_json::json!({"reads": {
                    "since": "ask", "when": "2026-11-08T09:12:00Z", "commits": 52,
                    "files": 13_852, "skipped": {}, "large": false, "findings": [finding],
                }});
                g.app.clients[0].waiting.push(crate::daemon::Staged {
                    id: "p1".into(),
                    pane: u32::MAX,
                    what: "Eval this work?".into(),
                    why: String::new(),
                    payload: serde_json::to_vec(&reads).unwrap(),
                    sends: None,
                    go: None,
                });
                g.dirty = true;
            }
            cx.simulate_resize(size(px(960.0), px(640.0)));
            for _ in 0..2 {
                cx.update(|window, _| window.refresh());
                cx.run_until_parked();
            }
            let mut at = |name: &'static str| {
                cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} was not drawn"))
            };
            let (list, said, button) =
                (at("findings"), at("finding said"), at("btn Mark as generated"));
            assert!(list.right() <= px(960.0), "{list:?} inside the window");
            assert!(said.right() <= button.left(), "{said:?} ends before {button:?}");
            assert!(button.right() <= list.right(), "{button:?} inside {list:?}");
            assert!(button.size.width > px(100.0), "the button whole: {button:?}");
        });
    }

    /// Each child of a scrolling column keeps its height, one clipped with
    /// `overflow: hidden` among them; one that says its own shrink keeps it,
    /// and a column that does not scroll squeezes as before.
    #[test]
    fn what_a_scroll_holds_keeps_its_size() {
        let clipped = || crate::gpui::div().style("overflow", "hidden").text("x");
        let mut tree = crate::gpui::div()
            .flex_col()
            .child(
                crate::gpui::div()
                    .flex_col()
                    .style("overflow-y", "auto")
                    .child(clipped())
                    .child(clipped().style("flex-shrink", "1")),
            )
            .child(clipped());
        keep_whole_in_scrolls(&mut tree);
        let shrink = |el: &Element| {
            el.styles
                .iter()
                .filter(|(k, _)| k == "flex-shrink")
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>()
        };
        let scroll = &tree.children[0];
        assert_eq!(shrink(&scroll.children[0]), ["0"], "kept whole");
        assert_eq!(shrink(&scroll.children[1]), ["1"], "its own say stands");
        assert!(shrink(&tree.children[1]).is_empty(), "not in a scroll");
        assert!(shrink(scroll).is_empty(), "the scroll itself is the column's to size");
    }

    #[test]
    fn overflow_auto_or_scroll_scrolls_on_its_axis() {
        let axes = |k: &str, v: &str| scroll_axes(&[(k.to_string(), v.to_string())]);
        assert_eq!(axes("overflow-x", "auto"), (true, false), "a horizontal scroll");
        assert_eq!(axes("overflow-x", "scroll"), (true, false));
        assert_eq!(axes("overflow-y", "auto"), (false, true));
        assert_eq!(axes("overflow-x", "hidden"), (false, false));
    }

    #[test]
    fn every_style_in_the_list_is_drawn() {
        for k in STYLES {
            assert!(style(gpui_div(), k, "0").is_ok(), "{k}");
        }
        assert!(style(gpui_div(), "letter-spacing", "0").is_err(), "one it cannot draw");
    }

    /// One side's padding, a resting control's opacity and a border's own
    /// colour are drawn, not dropped.
    #[test]
    fn a_sides_padding_opacity_and_a_border_colour_are_drawn() {
        for (k, v) in [
            ("padding-top", "14px"),
            ("padding-right", "8px"),
            ("padding-bottom", "2px"),
            ("padding-left", "28px"),
            ("opacity", "0.45"),
            ("border-color", crate::gpui::palette::AMBER),
        ] {
            assert!(style(gpui_div(), k, v).is_ok(), "{k}: {v}");
            assert!(STYLES.contains(&k), "{k} is listed");
        }
    }

    #[test]
    fn prose_of_several_lines_wraps_each_line() {
        assert_eq!(text_lines("one line", false, false), None);
        assert_eq!(
            text_lines("first\n\nthird", false, false),
            Some(vec![("first".into(), false), (" ".into(), false), ("third".into(), false)])
        );
        assert_eq!(
            text_lines("a\nb", false, true),
            Some(vec![("a".into(), true), ("b".into(), true)])
        );
    }
}
