//! Native macOS GPU-accelerated desktop window (`gpui` Metal renderer) for
//! `WeftDesktopApp`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ::gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, Div, ElementId, FocusHandle, Focusable,
    FontWeight, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, Rgba, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds as GpuiWindowBounds, WindowOptions,
    div as gpui_div, point, prelude::*, px, rgba, size,
};
use anyhow::Result;
use weft_core::state::WindowBounds;

use crate::app::WeftDesktopApp;
use crate::gpui::{Element, FileDialog, MacFileDialog, palette};
use crate::terminal_input::{self, Act, Button, Held};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveInput {
    Terminal,
    Search,
    MessageRow,
    AskSheet,
}

pub(crate) struct SharedState {
    pub(crate) app: WeftDesktopApp,
    dirty: bool,
    /// Where each pane's terminal was last painted, and its cell width, to
    /// find the cell under the pointer.
    pub(crate) grids: HashMap<(PathBuf, u32), (Point<Pixels>, f32)>,
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
    active_input: ActiveInput,
    wheel_accum: f32,
    /// The pane a drag is selecting in.
    selecting: Option<(PathBuf, u32)>,
}

impl Focusable for GpuiDesktopView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Run the native GPUI Metal desktop window and event loop until the user quits.
pub fn run_interactive(mut app: WeftDesktopApp) -> Result<()> {
    if app.state.open_projects.is_empty()
        && let Ok(cwd) = std::env::current_dir()
        && (cwd.join(".git").exists() || cwd.join(".fab7").exists())
    {
        let p = weft_core::state::normalize_project_path(&cwd);
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("project").to_string();
        app.state.open_projects.push(p.clone());
        let mut c = crate::daemon::ProjectClient::disconnected(p, name);
        c.turbo = app.state.settings.turbo;
        c.start_at_login = app.state.settings.start_at_login;
        app.clients.push(c);
        let _ = app.save_state();
    }

    let initial_bounds = app.state.settings.window_bounds.unwrap_or(WindowBounds {
        x: 120.0,
        y: 80.0,
        width: 1280.0,
        height: 820.0,
    });

    let shared = Arc::new(Mutex::new(SharedState { app, dirty: true, grids: HashMap::new() }));

    // Background connection & reconnection thread: connects any disconnected
    // project to `weftd` outside the UI mutex so startup and GPU rendering
    // never block.
    let connect_shared = Arc::clone(&shared);
    std::thread::spawn(move || {
        let mut first_pass = true;
        loop {
            let targets: Vec<PathBuf> = {
                let Ok(guard) = connect_shared.lock() else {
                    break;
                };
                if guard.app.quit_requested {
                    break;
                }
                if !guard.app.live {
                    Vec::new()
                } else {
                    guard
                        .app
                        .clients
                        .iter()
                        .filter(|c| !c.is_connected())
                        .map(|c| c.root.clone())
                        .collect()
                }
            };

            let mut any_connected = false;
            for root in targets {
                if let Ok(mut client) = crate::daemon::ProjectClient::open(&root, 28, 100) {
                    if first_pass {
                        let _ = client.setup("look", "", "");
                        first_pass = false;
                    }
                    if let Ok(mut guard) = connect_shared.lock() {
                        guard.app.integrate_connected_client(client);
                        let _ = guard.app.save_state();
                        guard.dirty = true;
                        any_connected = true;
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(if any_connected { 250 } else { 1000 }));
        }
    });

    // Background daemon pump thread: polls weftd sockets every 35ms.
    let pump_shared = Arc::clone(&shared);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(35));
            let Ok(mut guard) = pump_shared.lock() else {
                break;
            };
            if guard.app.quit_requested {
                break;
            }
            if guard.app.pump() {
                guard.dirty = true;
            }
        }
    });

    gpui_platform::application().run(move |cx: &mut App| {
        crate::fonts::load(cx);
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
        cx.open_window(
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
                        let width = f32::from(wb.size.width);
                        let height = f32::from(wb.size.height);
                        let x = f32::from(wb.origin.x);
                        let y = f32::from(wb.origin.y);
                        // Each pane's terminal sizes itself from its own rectangle.
                        if let Ok(mut guard) = this.shared.lock() {
                            guard.app.set_window_bounds(x, y, width, height);
                        }
                    })
                    .detach();

                    let poll_shared = Arc::clone(&view_shared);
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor().timer(Duration::from_millis(30)).await;
                            let (dirty, quit) = {
                                if let Ok(mut guard) = poll_shared.lock() {
                                    let d = guard.dirty;
                                    guard.dirty = false;
                                    (d, guard.app.quit_requested)
                                } else {
                                    (false, true)
                                }
                            };
                            if quit {
                                let _ = cx.update(|cx| cx.quit());
                                break;
                            }
                            if dirty {
                                if this.update(cx, |_, cx| cx.notify()).is_err() {
                                    break;
                                }
                            }
                        }
                    })
                    .detach();

                    GpuiDesktopView {
                        shared: view_shared,
                        focus_handle,
                        active_input: ActiveInput::Terminal,
                        wheel_accum: 0.0,
                        selecting: None,
                    }
                })
            },
        )
        .expect("open weft-desktop GPUI window");

        cx.activate(true);
    });

    Ok(())
}

impl GpuiDesktopView {
    fn dispatch_action(&mut self, action: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);

        if action == "open_project_dialog" {
            let bg_shared = Arc::clone(&self.shared);
            std::thread::spawn(move || {
                if let Some(folder) = MacFileDialog.pick_folder()
                    && let Ok(mut guard) = bg_shared.lock()
                {
                    guard.app.open_project_path(folder);
                    guard.dirty = true;
                }
            });
            return;
        }

        if action == "attach_file_dialog" {
            let bg_shared = Arc::clone(&self.shared);
            std::thread::spawn(move || {
                if let Some(file) = MacFileDialog.pick_file("Attach file")
                    && let Ok(mut guard) = bg_shared.lock()
                {
                    let token = format!(" @{} ", file.display());
                    if let Some(draft) = guard.app.ask_draft_mut() {
                        draft.push_str(&token);
                    }
                    guard.dirty = true;
                }
            });
            return;
        }

        if action == "clear_search" {
            if let Ok(mut guard) = self.shared.lock() {
                guard.app.dispatch("clear_search");
            }
            self.active_input = ActiveInput::Terminal;
            cx.notify();
            return;
        }

        if action.starts_with("open_ask:") || action.starts_with("open_follow_up:") {
            self.active_input = ActiveInput::AskSheet;
        } else if action == "close_sheet" || action == "submit_ask_sheet" {
            self.active_input = ActiveInput::Terminal;
        } else if action.starts_with("focus_pane_group:") || action.starts_with("select_tab:") {
            self.active_input = ActiveInput::Terminal;
        }

        let mut should_quit = false;
        if let Ok(mut guard) = self.shared.lock() {
            guard.app.dispatch(action);
            should_quit = guard.app.quit_requested;
        }
        if should_quit {
            cx.quit();
        } else {
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let key = ks.key.as_str();
        let mods = ks.modifiers;

        // Global shortcuts: Cmd+Q, Cmd+O, Cmd+B, Cmd+V
        if mods.platform {
            match key.to_ascii_lowercase().as_str() {
                "q" => {
                    let mut should_quit = false;
                    if let Ok(mut guard) = self.shared.lock() {
                        guard.app.dispatch("request_quit");
                        should_quit = guard.app.quit_requested;
                    }
                    if should_quit {
                        cx.quit();
                    } else {
                        cx.notify();
                    }
                    return;
                }
                "o" => {
                    let bg_shared = Arc::clone(&self.shared);
                    std::thread::spawn(move || {
                        if let Some(folder) = MacFileDialog.pick_folder()
                            && let Ok(mut guard) = bg_shared.lock()
                        {
                            guard.app.open_project_path(folder);
                            guard.dirty = true;
                        }
                    });
                    return;
                }
                "b" => {
                    if let Ok(mut guard) = self.shared.lock() {
                        guard.app.dispatch("toggle_sidebar");
                    }
                    cx.notify();
                    return;
                }
                "c" if self.active_input == ActiveInput::Terminal => {
                    let text = self.shared.lock().ok().and_then(|mut guard| {
                        guard.app.active_pane_view_mut().and_then(|p| p.selection_text())
                    });
                    if let Some(text) = text {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    return;
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        if let Ok(mut guard) = self.shared.lock() {
                            match self.active_input {
                                ActiveInput::Search => {
                                    guard.app.search_query.push_str(&text.replace('\n', " "));
                                }
                                ActiveInput::MessageRow => {
                                    guard.app.message_draft.push_str(&text.replace('\n', " "));
                                }
                                ActiveInput::AskSheet => {
                                    if let Some(draft) = guard.app.ask_draft_mut() {
                                        draft.push_str(&text);
                                    }
                                }
                                ActiveInput::Terminal => {
                                    let bracketed = guard
                                        .app
                                        .active_pane_view_mut()
                                        .is_some_and(|p| p.bracketed_paste());
                                    let bytes = terminal_input::paste(&text, bracketed);
                                    guard.app.send_active_pane_input(&bytes);
                                    let _ = guard.app.pump();
                                }
                            }
                        }
                        cx.notify();
                    }
                    return;
                }
                _ => {}
            }
        }

        // Handle text fields when Search, MessageRow, or AskSheet is focused
        if self.active_input != ActiveInput::Terminal {
            match key {
                "escape" => {
                    self.active_input = ActiveInput::Terminal;
                    cx.notify();
                    return;
                }
                "backspace" => {
                    if let Ok(mut guard) = self.shared.lock() {
                        match self.active_input {
                            ActiveInput::Search => {
                                if mods.platform || mods.alt {
                                    guard.app.search_query.clear();
                                } else {
                                    guard.app.search_query.pop();
                                }
                            }
                            ActiveInput::MessageRow => {
                                if mods.platform || mods.alt {
                                    guard.app.message_draft.clear();
                                } else {
                                    guard.app.message_draft.pop();
                                }
                            }
                            ActiveInput::AskSheet => {
                                if let Some(draft) = guard.app.ask_draft_mut() {
                                    draft.pop();
                                }
                            }
                            ActiveInput::Terminal => {}
                        }
                    }
                    cx.notify();
                    return;
                }
                "enter" => {
                    if let Ok(mut guard) = self.shared.lock() {
                        match self.active_input {
                            ActiveInput::Search => {}
                            ActiveInput::MessageRow => {
                                guard.app.dispatch("send_message_row");
                            }
                            ActiveInput::AskSheet => {
                                if mods.platform || mods.control {
                                    guard.app.dispatch("submit_ask_sheet");
                                    self.active_input = ActiveInput::Terminal;
                                } else if let Some(draft) = guard.app.ask_draft_mut() {
                                    draft.push('\n');
                                }
                            }
                            ActiveInput::Terminal => {}
                        }
                    }
                    cx.notify();
                    return;
                }
                "space" => {
                    if !mods.control && !mods.platform {
                        if let Ok(mut guard) = self.shared.lock() {
                            match self.active_input {
                                ActiveInput::Search => guard.app.search_query.push(' '),
                                ActiveInput::MessageRow => guard.app.message_draft.push(' '),
                                ActiveInput::AskSheet => {
                                    if let Some(draft) = guard.app.ask_draft_mut() {
                                        draft.push(' ');
                                    }
                                }
                                ActiveInput::Terminal => {}
                            }
                        }
                        cx.notify();
                    }
                    return;
                }
                _ => {
                    if !mods.control && !mods.platform {
                        let typed = ks
                            .key_char
                            .as_deref()
                            .or_else(|| (key.chars().count() == 1).then_some(key));
                        if let Some(ch) = typed {
                            if let Ok(mut guard) = self.shared.lock() {
                                match self.active_input {
                                    ActiveInput::Search => guard.app.search_query.push_str(ch),
                                    ActiveInput::MessageRow => guard.app.message_draft.push_str(ch),
                                    ActiveInput::AskSheet => {
                                        if let Some(draft) = guard.app.ask_draft_mut() {
                                            draft.push_str(ch);
                                        }
                                    }
                                    ActiveInput::Terminal => {}
                                }
                            }
                            cx.notify();
                        }
                    }
                    return;
                }
            }
        }

        // Terminal input, as the program's modes say keys are sent.
        let held = Held {
            shift: mods.shift,
            alt: mods.alt,
            control: mods.control,
            platform: mods.platform,
        };
        if let Ok(mut guard) = self.shared.lock() {
            let modes = guard.app.active_pane_view_mut().map(|p| p.modes()).unwrap_or_default();
            if let Some(bytes) = terminal_input::key(key, ks.key_char.as_deref(), held, modes) {
                guard.app.send_active_pane_input(&bytes);
                let _ = guard.app.pump();
            }
        }
        cx.notify();
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
                this.active_input = ActiveInput::Terminal;
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

    fn build_gpui_element(
        &self,
        el: &Element,
        parent_text: InheritedText,
        parent_is_row: bool,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut text_style = parent_text;
        let mut has_explicit_width = false;
        let mut has_explicit_height = false;
        let mut has_horiz_margin = false;
        let mut has_vert_margin = false;
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
                "font-family" => {
                    if v.contains("Mono") {
                        text_style.font_mono = true;
                    }
                }
                "line-height" => {
                    if let Some(n) = parse_px(v) {
                        text_style.line_height = Some(n);
                    }
                }
                "white-space" => match v.as_str() {
                    "nowrap" => {
                        text_style.nowrap = true;
                        text_style.pre = false;
                    }
                    "pre" | "pre-wrap" => {
                        text_style.pre = true;
                        text_style.nowrap = false;
                    }
                    _ => {
                        text_style.nowrap = false;
                        text_style.pre = false;
                    }
                },
                "text-overflow" => {
                    if v == "ellipsis" {
                        text_style.ellipsis = true;
                    }
                }
                "width" => has_explicit_width = true,
                "height" => has_explicit_height = true,
                "margin" => {
                    let (t, r, b, l) = parse_box_edges(v);
                    if l != 0.0 || r != 0.0 {
                        has_horiz_margin = true;
                    }
                    if t != 0.0 || b != 0.0 {
                        has_vert_margin = true;
                    }
                }
                "margin-left" | "margin-right" => has_horiz_margin = true,
                "margin-top" | "margin-bottom" => has_vert_margin = true,
                "align-self" => {
                    if v == "stretch" {
                        is_self_stretch = true;
                    }
                }
                "overflow-y" => {
                    if v == "auto" || v == "scroll" {
                        is_scroll_y = true;
                    }
                }
                _ => {}
            }
        }

        let mut d: Div = gpui_div();
        let mut child_is_row = false;

        d = d
            .text_color(text_style.color)
            .text_size(px(text_style.font_size))
            .font_weight(text_style.font_weight);

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
            match k.as_str() {
                "display" => match v.as_str() {
                    "flex" => d = d.flex(),
                    "none" => d = d.hidden(),
                    _ => {}
                },
                "flex-direction" => match v.as_str() {
                    "row" => {
                        d = d.flex().flex_row();
                        child_is_row = true;
                    }
                    "column" => {
                        d = d.flex().flex_col();
                        child_is_row = false;
                    }
                    _ => {}
                },
                "align-items" => match v.as_str() {
                    "center" => d = d.items_center(),
                    "flex-start" => d = d.items_start(),
                    "baseline" => d = d.items_baseline(),
                    _ => {}
                },
                "justify-content" => match v.as_str() {
                    "space-between" => d = d.justify_between(),
                    "center" => d = d.justify_center(),
                    "flex-end" => d = d.justify_end(),
                    _ => {}
                },
                "flex" => {
                    if v.starts_with('1') {
                        d = d.flex_1().min_w_0().min_h_0();
                    }
                }
                "flex-shrink" => {
                    if v == "0" {
                        d = d.flex_shrink_0();
                    }
                }
                "flex-wrap" => {
                    if v == "wrap" {
                        d = d.flex_wrap();
                    }
                }
                "gap" => {
                    if let Some(n) = parse_px(v) {
                        d = d.gap(px(n));
                    }
                }
                "width" => {
                    if v == "100%" {
                        d = d.w_full();
                    } else if let Some(n) = parse_px(v) {
                        d = d.w(px(n));
                    }
                }
                "height" => {
                    if v == "100%" {
                        d = d.h_full();
                    } else if let Some(n) = parse_px(v) {
                        d = d.h(px(n));
                    }
                }
                "min-width" => {
                    if let Some(n) = parse_px(v) {
                        d = d.min_w(px(n));
                    }
                }
                "min-height" => {
                    if let Some(n) = parse_px(v) {
                        d = d.min_h(px(n));
                    }
                }
                "max-width" => {
                    if let Some(n) = parse_px(v) {
                        d = d.max_w(px(n));
                    }
                }
                "max-height" => {
                    if let Some(n) = parse_px(v) {
                        d = d.max_h(px(n));
                    }
                }
                "padding" => {
                    let (t, r, b, l) = parse_box_edges(v);
                    d = d.pt(px(t)).pr(px(r)).pb(px(b)).pl(px(l));
                }
                "margin" => {
                    let (t, r, b, l) = parse_box_edges(v);
                    d = d.mt(px(t)).mr(px(r)).mb(px(b)).ml(px(l));
                }
                "margin-left" => {
                    if v == "auto" {
                        d = d.ml_auto();
                    } else if let Some(n) = parse_px(v) {
                        d = d.ml(px(n));
                    }
                }
                "margin-top" => {
                    if v == "auto" {
                        d = d.mt_auto();
                    } else if let Some(n) = parse_px(v) {
                        d = d.mt(px(n));
                    }
                }
                "background" => {
                    if v != "transparent" {
                        d = d.bg(parse_color(v));
                    }
                }
                "border-radius" => {
                    if let Some(n) = parse_px(v) {
                        d = d.rounded(px(n));
                    }
                }
                "border" => {
                    if let Some(c) = parse_border_color(v) {
                        d = if v.starts_with("2px") { d.border_2() } else { d.border_1() };
                        d = d.border_color(c);
                    }
                }
                // The guideline's bevel, `inset 0 -2px 0`: the nearest a GPUI
                // div draws is a 2 px bottom border in the border's colour.
                "box-shadow" if v.starts_with("inset 0 -2px") => d = d.border_b_2(),
                "border-top" => {
                    if let Some(c) = parse_border_color(v) {
                        d = if v.starts_with("2px") { d.border_t_2() } else { d.border_t_1() };
                        d = d.border_color(c);
                    }
                }
                "border-bottom" => {
                    if let Some(c) = parse_border_color(v) {
                        d = d.border_b_1().border_color(c);
                    }
                }
                "border-left" => {
                    if let Some(c) = parse_border_color(v) {
                        d = d.border_l_1().border_color(c);
                    }
                }
                "border-right" => {
                    if let Some(c) = parse_border_color(v) {
                        d = d.border_r_1().border_color(c);
                    }
                }
                "position" => match v.as_str() {
                    "absolute" => d = d.absolute(),
                    "relative" => d = d.relative(),
                    _ => {}
                },
                "inset" => {
                    if v == "0" {
                        d = d.inset_0();
                    }
                }
                "top" => {
                    if let Some(n) = parse_px(v) {
                        d = d.top(px(n));
                    }
                }
                "left" => {
                    if let Some(n) = parse_px(v) {
                        d = d.left(px(n));
                    }
                }
                "right" => {
                    if let Some(n) = parse_px(v) {
                        d = d.right(px(n));
                    }
                }
                "bottom" => {
                    if let Some(n) = parse_px(v) {
                        d = d.bottom(px(n));
                    }
                }
                "overflow" => {
                    if v == "hidden" {
                        d = d.overflow_hidden();
                    }
                }
                "overflow-x" => {
                    if v == "hidden" {
                        d = d.overflow_x_hidden();
                    }
                }
                "opacity" => {
                    if let Ok(op) = v.parse::<f32>() {
                        d = d.opacity(op);
                    }
                }
                _ => {}
            }
        }

        if is_self_stretch {
            if parent_is_row {
                if !has_explicit_height && !has_vert_margin {
                    d = d.h_full();
                }
            } else if !has_explicit_width && !has_horiz_margin {
                d = d.w_full();
            }
        }

        // Root window layer fills the entire native GPUI window
        if el.layer_name.as_deref() == Some("window") {
            d = d.size_full().flex_1();
        }

        // Highlight search box border when Search input is active
        if el.layer_name.as_deref() == Some("search") && self.active_input == ActiveInput::Search {
            d = d.border_1().border_color(parse_color(palette::BLUE_BORDER));
        }

        // Handle input & textarea elements natively
        if el.tag == "input" || el.tag == "textarea" {
            let target_input = if el.id.as_deref() == Some("tree-search") {
                ActiveInput::Search
            } else if el.tag == "textarea" {
                ActiveInput::AskSheet
            } else {
                ActiveInput::MessageRow
            };
            let is_focused = self.active_input == target_input;
            if is_focused && target_input != ActiveInput::Search {
                d = d.border_1().border_color(parse_color(palette::BLUE_BORDER));
            }
            let val = el.text.as_deref().unwrap_or("");
            let placeholder = el.placeholder.as_deref().unwrap_or("");
            let (display_str, display_color) = if is_focused {
                (format!("{val}▏"), parse_color(palette::TEXT))
            } else if !val.is_empty() {
                (val.to_string(), parse_color(palette::TEXT))
            } else {
                (placeholder.to_string(), parse_color(palette::DIM))
            };
            d = d
                .cursor_text()
                .text_color(display_color)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.focus_handle.focus(window, cx);
                        this.active_input = target_input;
                        cx.notify();
                    }),
                )
                .child(display_str);
            return d.into_any_element();
        }

        // Clicking anywhere on the search pill focuses Search
        if el.layer_name.as_deref() == Some("search") {
            d = d.cursor_text().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.focus_handle.focus(window, cx);
                    this.active_input = ActiveInput::Search;
                    cx.notify();
                }),
            );
        }

        // A pane's terminal, painted from its own cells.
        if el.layer_name.as_deref() == Some("terminal") {
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

        // Text content (handling multi-line pre / terminal text cleanly)
        if let Some(t) = &el.text
            && !t.is_empty()
        {
            if text_style.pre || t.contains('\n') {
                let lh = text_style.line_height.unwrap_or(text_style.font_size * 1.4);
                let mut lines_col = gpui_div().flex().flex_col().w_full();
                for line in t.split('\n') {
                    let display_line =
                        if line.is_empty() { " ".to_string() } else { line.to_string() };
                    lines_col = lines_col
                        .child(gpui_div().min_h(px(lh)).whitespace_nowrap().child(display_line));
                }
                d = d.child(lines_col);
            } else {
                d = d.child(t.clone());
            }
        }

        // Children (skipping synthetic `titlebar` layer since GPUI renders native macOS titlebar)
        for (idx, child) in el.children.iter().enumerate() {
            if child.layer_name.as_deref() == Some("titlebar") {
                continue;
            }
            let child_path = format!("{path}/{idx}");
            d = d.child(self.build_gpui_element(child, text_style, child_is_row, &child_path, cx));
        }

        // Stateful elements: scrollable containers or clickable actions
        if is_scroll_y || el.action.is_some() {
            let id_str = if let Some(ln) = &el.layer_name {
                format!("layer:{ln}:{path}")
            } else if let Some(act) = &el.action {
                format!("act:{act}:{path}")
            } else {
                format!("node:{path}")
            };
            let mut stateful = d.id(ElementId::Name(SharedString::from(id_str)));
            if is_scroll_y {
                stateful = stateful.overflow_y_scroll();
            }
            if let Some(action) = el.action.clone() {
                stateful = stateful.cursor_pointer().hover(|s| s.opacity(0.88)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.dispatch_action(&action, window, cx);
                    }),
                );
            }
            return stateful.into_any_element();
        }

        d.into_any_element()
    }
}

impl Render for GpuiDesktopView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root_el = if let Ok(guard) = self.shared.lock() {
            guard.app.render()
        } else {
            crate::gpui::div()
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

fn parse_px(s: &str) -> Option<f32> {
    let trimmed = s.trim().trim_end_matches("px");
    trimmed.parse::<f32>().ok()
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
    if let Some(inner) = s.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')) {
        let comps: Vec<&str> = inner.split(',').map(str::trim).collect();
        if comps.len() == 4 {
            let r = comps[0].parse::<u32>().unwrap_or(0).min(255);
            let g = comps[1].parse::<u32>().unwrap_or(0).min(255);
            let b = comps[2].parse::<u32>().unwrap_or(0).min(255);
            let a = (comps[3].parse::<f32>().unwrap_or(1.0).clamp(0.0, 1.0) * 255.0) as u32;
            return rgba((r << 24) | (g << 16) | (b << 8) | a);
        }
    }
    rgba(0x00000000)
}
