//! The desktop's window: its event loop, the threads that connect to the
//! daemon and take what it sends, and the element tree drawn with GPUI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ::gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, Div, ElementId, EntityInputHandler,
    FocusHandle, Focusable, FontWeight, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, PathPromptOptions, Pixels, Point, Render, Rgba,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, SystemNotification,
    UTF16Selection, Window, WindowBounds as GpuiWindowBounds, WindowOptions, div as gpui_div,
    point, prelude::*, px, relative, rgba, size,
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
use crate::text_field::{Outcome, TextField};
use crate::text_input::{self as field_view, FieldElement, Look, Painted};

/// The text field `which` names, when it is one and is there.
fn field(app: &mut WeftDesktopApp, which: Focus) -> Option<&mut TextField> {
    match which {
        Focus::Search => Some(&mut app.search),
        Focus::Message => Some(app.message_field()),
        Focus::Ask => app.ask_draft_mut(),
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
}

#[derive(Clone, Copy)]
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
        let targets = match shared.lock() {
            Ok(guard) if !guard.app.quit_requested => guard.app.disconnected(),
            _ => return,
        };
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

/// A newer Weft took the daemon over: open its app in place of this one,
/// which goes. Whether there was one to open.
fn start_newer(shared: &Mutex<SharedState>) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return false };
    let Some(weft) = crate::daemon::newer_weft(&home) else { return false };
    let Ok(mut guard) = shared.lock() else { return false };
    // The newer app listens where this one did.
    guard.instance = None;
    if crate::daemon::open_app_of(&weft).is_err() {
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
                if guard.app.pump() {
                    guard.dirty = true;
                }
            }
            write_state(&pump_shared);
        }
    });

    let answering = Arc::clone(&shared);
    gpui_platform::application().run(move |cx: &mut App| {
        crate::fonts::load(cx);
        // A notice the person opens brings its agent in front.
        cx.on_system_notification_response(move |response, cx| {
            if let Ok(mut guard) = answering.lock() {
                guard.app.open_notice(&response.tag);
                guard.dirty = true;
            }
            cx.activate(true);
        });
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
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
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window, cx);

                    cx.observe_window_bounds(window, |this: &mut GpuiDesktopView, window, _cx| {
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
                            let (dirty, quit, notices, raise) = match poll_shared.lock() {
                                Ok(mut guard) => (
                                    std::mem::take(&mut guard.dirty),
                                    guard.app.quit_requested,
                                    guard.app.take_notices(),
                                    std::mem::take(&mut guard.raise),
                                ),
                                Err(_) => (false, true, Vec::new(), false),
                            };
                            if raise {
                                cx.update(|cx| cx.activate(true));
                            }
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
                            if dirty && this.update(cx, |_, cx| cx.notify()).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();

                    GpuiDesktopView {
                        shared: view_shared,
                        focus_handle,
                        wheel_accum: 0.0,
                        selecting: None,
                        painted: Rc::default(),
                    }
                })
            },
        );
        match opened {
            Ok(_) => cx.activate(true),
            Err(e) => {
                eprintln!("weft-desktop: {e}");
                cx.quit();
            }
        }
    });

    Ok(())
}

impl GpuiDesktopView {
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

        // A text field takes its keys from the table; a character goes on to
        // the window's text input, which an input method composes. The
        // window's own shortcuts still work there.
        if typing {
            if self.field_key(event, cx) {
                cx.stop_propagation();
            }
            if !matches!(
                shortcut,
                Some(Shortcut::Quit | Shortcut::OpenProject | Shortcut::ToggleList)
            ) {
                return;
            }
        }

        let action = match shortcut {
            Some(Shortcut::Quit) => Action::from(WindowAct::RequestQuit),
            Some(Shortcut::OpenProject) => Action::from(WindowAct::OpenProject),
            Some(Shortcut::ToggleList) => Action::from(WindowAct::ToggleSidebar),
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
                _ => {}
            },
            // Tab takes the suggestion shown in the message field; Escape drops
            // it, and with none shown leaves the field.
            Outcome::Take => {
                if which == Focus::Message {
                    guard.app.take_suggestion();
                }
            }
            Outcome::Drop => {
                if which != Focus::Message || !guard.app.drop_suggestion() {
                    guard.app.focus = Focus::Terminal;
                }
            }
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
            max_lines: if el.tag == "input" && which == Focus::Search { 1 } else { 6 },
        };
        let snapshot = self.shared.lock().ok().and_then(|mut g| {
            let is_focused = g.app.focus == which;
            let ghost =
                (which == Focus::Message).then(|| g.app.ghost().map(str::to_string)).flatten();
            field(&mut g.app, which).map(|f| {
                (
                    is_focused,
                    f.text().to_string(),
                    f.selection(),
                    f.cursor(),
                    f.marked.clone(),
                    f.shape(),
                    ghost,
                )
            })
        });
        let Some((is_focused, text, selection, cursor, marked, shape, ghost)) = snapshot else {
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
            ghost,
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
        // What sits inside the field beside its words, such as the key that takes a suggestion.
        for (idx, child) in el.children.iter().enumerate() {
            let child_path = format!("{path}/{idx}");
            field_box =
                field_box.child(self.build_gpui_element(child, text_style, true, &child_path, cx));
        }
        field_box.into_any_element()
    }

    fn build_gpui_element(
        &self,
        el: &Element,
        parent_text: InheritedText,
        parent_is_row: bool,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if el.tag == "img" {
            return picture(el);
        }
        let mut text_style = parent_text;
        let mut has_explicit_width = false;
        let mut has_explicit_height = false;
        let mut is_self_stretch = false;
        let mut is_scroll_y = false;

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
                "overflow-y" => is_scroll_y = v == "auto" || v == "scroll",
                _ => {}
            }
        }

        let mut d: Div = gpui_div()
            .text_color(text_style.color)
            .text_size(px(text_style.font_size))
            .font_weight(text_style.font_weight);
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

        // A pane's terminal, painted from its own cells.
        if el.tag == "terminal" {
            let attr = |k: &str| el.attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
            if let (Some(project), Some(pane)) =
                (attr("data-project"), attr("data-pane").and_then(|p| p.parse().ok()))
            {
                let project = PathBuf::from(project);
                let shared = Arc::clone(&self.shared);
                let view = crate::terminal_view::terminal(shared, project.clone(), pane);
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

        for (idx, child) in el.children.iter().enumerate() {
            let child_path = format!("{path}/{idx}");
            d = d.child(self.build_gpui_element(child, text_style, child_is_row, &child_path, cx));
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
        if is_scroll_y || el.action.is_some() || el.drag.is_some() {
            let id_str = match &el.layer_name {
                Some(ln) => format!("layer:{ln}:{path}"),
                None => format!("node:{path}"),
            };
            let mut stateful = d.id(ElementId::Name(SharedString::from(id_str)));
            if is_scroll_y {
                stateful = stateful.overflow_y_scroll();
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

        d.into_any_element()
    }
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
    "background",
    "border",
    "border-bottom",
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
    "max-width",
    "min-height",
    "min-width",
    "overflow",
    "overflow-x",
    "overflow-y",
    "padding",
    "position",
    "right",
    "text-align",
    "text-overflow",
    "top",
    "white-space",
    "width",
];

/// One style on a div; the div untouched, as an error, for one the window
/// does not draw.
#[allow(clippy::result_large_err)]
fn style(d: Div, k: &str, v: &str) -> std::result::Result<Div, Div> {
    let n = parse_px(v);
    Ok(match k {
        // The text's own, taken before the box is built.
        "color" | "font-size" | "font-weight" | "font-family" | "line-height" | "white-space"
        | "text-overflow" | "align-self" => d,
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
        "padding" => {
            let (t, r, b, l) = parse_box_edges(v);
            d.pt(px(t)).pr(px(r)).pb(px(b)).pl(px(l))
        }
        "background" if v == "transparent" => d,
        "background" => d.bg(parse_color(v)),
        "border-radius" => match n {
            Some(n) => d.rounded(px(n)),
            None => d,
        },
        "border" => match parse_border_color(v) {
            Some(c) => {
                if v.starts_with("2px") { d.border_2() } else { d.border_1() }.border_color(c)
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root_el = match self.shared.lock() {
            Ok(guard) => guard.app.render(),
            Err(_) => crate::gpui::div(),
        };
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
        gpui_div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(parse_color(palette::CANVAS))
            .text_color(parse_color(palette::TEXT))
            .font_family(crate::fonts::SANS)
            .child(self.build_gpui_element(&root_el, root_text, false, "0", cx))
    }
}

/// One of Weft's own pictures, drawn from its SVG at the size its styles give.
fn picture(el: &Element) -> AnyElement {
    let named = |k: &str| el.attrs.iter().chain(&el.styles).find(|(n, _)| n == k).map(|(_, v)| v);
    let Some(bytes) = named("src").and_then(|n| crate::gpui::asset(n)) else {
        return gpui_div().into_any_element();
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
    use ::gpui::{TestAppContext, VisualTestContext};
    use weft_core::text_keys::Edit;

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
        }));
        let view_shared = Arc::clone(&shared);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window, cx);
            GpuiDesktopView {
                shared: view_shared,
                focus_handle,
                wheel_accum: 0.0,
                selecting: None,
                painted: Rc::default(),
            }
        });
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

    /// A suggestion shown for the draft the field holds now.
    fn suggested(shared: &Arc<Mutex<SharedState>>, wire: &mut Wire, text: &str) {
        let mut g = shared.lock().unwrap();
        let t0 = std::time::Instant::now();
        g.app.suggest_tick(t0);
        g.app.suggest_tick(t0 + std::time::Duration::from_millis(150));
        let (id, call) = wire.call();
        assert!(matches!(call, weft_proto::Call::Suggest { .. }), "{call:?}");
        wire.answer(id, serde_json::json!({"text": text, "score": 0.8}));
        crate::app::tests::pumped(&mut g.app, |a| a.ghost().is_some());
        assert_eq!(g.app.ghost(), Some(text));
    }

    #[test]
    fn tab_takes_the_suggestion_and_escape_drops_it_keeping_the_field() {
        windowed(|cx| {
            let (_tmp, shared, _view, mut wire, cx) = opened_view(cx);
            wire.calls();
            cx.simulate_input("run the");
            suggested(&shared, &mut wire, " login tests");
            cx.simulate_keystrokes("tab");
            assert_eq!(message(&shared), "run the login tests");
            cx.simulate_input(" now");
            suggested(&shared, &mut wire, " please");
            cx.simulate_keystrokes("escape");
            assert_eq!(shared.lock().unwrap().app.ghost(), None);
            cx.simulate_input("!");
            assert_eq!(message(&shared), "run the login tests now!", "still in the field");
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

    #[test]
    fn every_style_in_the_list_is_drawn() {
        for k in STYLES {
            assert!(style(gpui_div(), k, "0").is_ok(), "{k}");
        }
        assert!(style(gpui_div(), "letter-spacing", "0").is_err(), "one it cannot draw");
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
