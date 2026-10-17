//! The workspace: up to four panes split edge to edge, their flush tab
//! strips, the layout menu, the `Terminal | Conversation` switch and the
//! message row.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use weft_core::conversation::{Item, Kind};
use weft_core::sessions::State;
use weft_core::state::{PaneViewMode, SavedPaneTab, SavedPanesLayout, SavedTabGroup, SplitLayout};
use weft_core::turns::Turn;
use weft_core::words;

use crate::action::{Focus, PaneAct};
use crate::daemon::ProjectClient;
use crate::gpui::{Element, div, edge_bar, img_el, input_el, palette, span, terminal_el};

/// Where a dragged tab is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    TabBar { group: usize, index: usize },
    EdgeRight { group: usize },
    EdgeDown { group: usize },
    Center { group: usize },
}

/// Split `group_idx` to the right, moving its active tab into the new pane.
pub fn split_pane_right(
    panes: &mut SavedPanesLayout,
    group_idx: usize,
) -> std::result::Result<(), &'static str> {
    split_pane(panes, group_idx, true)
}

/// Split `group_idx` downward, moving its active tab into the new pane.
pub fn split_pane_down(
    panes: &mut SavedPanesLayout,
    group_idx: usize,
) -> std::result::Result<(), &'static str> {
    split_pane(panes, group_idx, false)
}

fn split_pane(
    panes: &mut SavedPanesLayout,
    group_idx: usize,
    right: bool,
) -> std::result::Result<(), &'static str> {
    if panes.groups.len() >= 4 {
        return Err("web.why.four_panes");
    }
    let Some(group) = panes.groups.get_mut(group_idx) else {
        return Err("why.split_needs_two");
    };
    if group.tabs.len() < 2 {
        return Err("why.split_needs_two");
    }
    let take_idx = group.active_tab.min(group.tabs.len() - 1);
    let moved_tab = group.tabs.remove(take_idx);
    if group.active_tab >= group.tabs.len() {
        group.active_tab = group.tabs.len().saturating_sub(1);
    }

    let insert_at = (group_idx + 1).min(panes.groups.len());
    panes.groups.insert(insert_at, SavedTabGroup { active_tab: 0, tabs: vec![moved_tab] });
    panes.focused_group = insert_at;
    panes.layout = match (panes.groups.len(), right) {
        (1, _) => SplitLayout::Single,
        (2, true) => SplitLayout::TwoColumns,
        (2, false) => SplitLayout::TwoRows,
        (3, _) => SplitLayout::ThreeRight,
        _ => SplitLayout::FourGrid,
    };
    Ok(())
}

/// Move the active tab of `group_idx` to the next pane.
pub fn move_tab_to_next_pane(
    panes: &mut SavedPanesLayout,
    group_idx: usize,
) -> std::result::Result<(), &'static str> {
    if panes.groups.len() < 2 {
        return Err("why.split_needs_two");
    }
    let Some(group) = panes.groups.get_mut(group_idx) else {
        return Ok(());
    };
    if group.tabs.is_empty() {
        return Ok(());
    }
    let take_idx = group.active_tab.min(group.tabs.len() - 1);
    let tab = group.tabs.remove(take_idx);
    if group.active_tab >= group.tabs.len() {
        group.active_tab = group.tabs.len().saturating_sub(1);
    }
    let next_group = (group_idx + 1) % panes.groups.len();
    panes.groups[next_group].tabs.push(tab);
    panes.groups[next_group].active_tab = panes.groups[next_group].tabs.len() - 1;
    panes.focused_group = next_group;

    if panes.groups[group_idx].tabs.is_empty() {
        panes.groups.remove(group_idx);
        if panes.focused_group > group_idx {
            panes.focused_group -= 1;
        }
    }
    normalize_layout_for_group_count(panes);
    Ok(())
}

/// Join all panes into a single pane holding every tab.
pub fn join_all_panes(panes: &mut SavedPanesLayout) {
    if panes.groups.len() <= 1 {
        panes.layout = SplitLayout::Single;
        panes.focused_group = 0;
        return;
    }
    let mut all_tabs = Vec::new();
    for g in panes.groups.drain(..) {
        for t in g.tabs {
            if !all_tabs.iter().any(|e: &SavedPaneTab| e.project == t.project && e.pane == t.pane) {
                all_tabs.push(t);
            }
        }
    }
    let active = all_tabs.len().saturating_sub(1);
    panes.groups.push(SavedTabGroup { active_tab: active, tabs: all_tabs });
    panes.layout = SplitLayout::Single;
    panes.focused_group = 0;
}

/// Drag a tab from `(from_group, from_tab)` and drop it at `target`.
pub fn drag_tab(
    panes: &mut SavedPanesLayout,
    from_group: usize,
    from_tab: usize,
    target: DropTarget,
) -> std::result::Result<(), &'static str> {
    let Some(g) = panes.groups.get_mut(from_group) else {
        return Ok(());
    };
    if from_tab >= g.tabs.len() {
        return Ok(());
    }
    g.active_tab = from_tab;
    match target {
        DropTarget::EdgeRight { group } => {
            if group == from_group {
                split_pane_right(panes, from_group)
            } else {
                move_tab_between_groups(panes, from_group, from_tab, group, usize::MAX);
                Ok(())
            }
        }
        DropTarget::EdgeDown { group } => {
            if group == from_group {
                split_pane_down(panes, from_group)
            } else {
                move_tab_between_groups(panes, from_group, from_tab, group, usize::MAX);
                Ok(())
            }
        }
        DropTarget::TabBar { group, index } => {
            move_tab_between_groups(panes, from_group, from_tab, group, index);
            Ok(())
        }
        DropTarget::Center { .. } => {
            join_all_panes(panes);
            Ok(())
        }
    }
}

fn move_tab_between_groups(
    panes: &mut SavedPanesLayout,
    from_group: usize,
    from_tab: usize,
    to_group: usize,
    to_index: usize,
) {
    if from_group == to_group {
        let g = &mut panes.groups[from_group];
        if from_tab < g.tabs.len() {
            let tab = g.tabs.remove(from_tab);
            let idx = to_index.min(g.tabs.len());
            g.tabs.insert(idx, tab);
            g.active_tab = idx;
        }
        return;
    }
    let tab = panes.groups[from_group].tabs.remove(from_tab);
    if panes.groups[from_group].active_tab >= panes.groups[from_group].tabs.len() {
        panes.groups[from_group].active_tab = panes.groups[from_group].tabs.len().saturating_sub(1);
    }
    let dest = &mut panes.groups[to_group];
    let idx = to_index.min(dest.tabs.len());
    dest.tabs.insert(idx, tab);
    dest.active_tab = idx;
    panes.focused_group = to_group;

    if panes.groups[from_group].tabs.is_empty() {
        panes.groups.remove(from_group);
        if panes.focused_group > from_group {
            panes.focused_group -= 1;
        }
    }
    normalize_layout_for_group_count(panes);
}

fn normalize_layout_for_group_count(panes: &mut SavedPanesLayout) {
    panes.layout = match panes.groups.len() {
        0 | 1 => SplitLayout::Single,
        2 => match panes.layout {
            SplitLayout::TwoRows => SplitLayout::TwoRows,
            _ => SplitLayout::TwoColumns,
        },
        3 => SplitLayout::ThreeRight,
        _ => SplitLayout::FourGrid,
    };
}

/// Props for rendering the multi-pane workspace (`panes_view`).
pub struct PanesProps<'a> {
    pub panes: &'a SavedPanesLayout,
    pub clients: &'a [ProjectClient],
    pub layout_menu_pane: Option<usize>,
    pub conversations: &'a HashMap<(PathBuf, u32), Vec<Item>>,
    pub unfolded_steps: &'a HashSet<(PathBuf, u32, usize)>,
    pub message_draft: &'a str,
    /// The message field has the keyboard.
    pub typing: bool,
    /// What the agent in front's field shows after its caret.
    pub ghost: Option<&'a str>,
    /// What sits under the panes: the docked Ask in place of the message row.
    pub dock: Option<Element>,
}

/// The empty workspace's hairline grid: a line every 32 px, enough for any
/// window, cut to the workspace.
fn hairline_grid() -> Element {
    let line = "#25355038";
    let mut grid = div()
        .layer("hairline grid")
        .style("position", "absolute")
        .style("inset", "0")
        .style("overflow", "hidden");
    for i in 0..100 {
        grid = grid
            .child(
                div()
                    .style("position", "absolute")
                    .style("left", format!("{}px", i * 32))
                    .style("top", "0")
                    .w(1)
                    .style("height", "100%")
                    .bg(line),
            )
            .child(
                div()
                    .style("position", "absolute")
                    .style("top", format!("{}px", i * 32))
                    .style("left", "0")
                    .h(1)
                    .style("width", "100%")
                    .bg(line),
            );
    }
    grid
}

/// Render the workspace: the panes edge to edge with 1 px lines between
/// them, or the blueprint grid when no agent is open.
pub fn render_panes_workspace(props: &PanesProps) -> Element {
    let mut container =
        div().layer("workspace").flex_1().self_stretch().flex_col().style("min-height", "0");

    if props.panes.groups.is_empty() {
        return container.child(
            div()
                .layer("empty panes")
                .flex_1()
                .self_stretch()
                .style("position", "relative")
                .bg(palette::TERMINAL_BG)
                .child(hairline_grid())
                .child(
                    div()
                        .style("position", "absolute")
                        .style("inset", "0")
                        .flex_col()
                        .gap(16)
                        .items_center()
                        .justify_center()
                        // Weft's banner, as the web draws it.
                        .child(img_el("weft-banner").layer("woven mark").w(189).h(60))
                        .child(
                            div()
                                .layer("woven said")
                                .font_size(13)
                                .color(palette::MUTED)
                                .text(words::word("workspace.empty")),
                        ),
                ),
        );
    }

    let split = props.panes.groups.len() > 1;
    let focused_idx = props.panes.focused_group.min(props.panes.groups.len().saturating_sub(1));
    if split && let Some(group) = props.panes.groups.get(focused_idx) {
        container = container.child(render_split_toolbar(props, focused_idx, group));
    }

    let group_els: Vec<Element> = props
        .panes
        .groups
        .iter()
        .enumerate()
        .map(|(g_idx, g)| render_pane_group(props, g_idx, g, split))
        .collect();

    let grid = match (props.panes.layout, group_els.len()) {
        (_, 1) => {
            div().flex_1().self_stretch().flex_col().style("min-height", "0").children(group_els)
        }
        (SplitLayout::TwoRows, 2) => {
            let mut it = group_els.into_iter();
            let top = it.next().unwrap().border_bottom(palette::LINE);
            let bot = it.next().unwrap();
            div().flex_1().self_stretch().flex_col().child(top).child(bot)
        }
        (_, 2) => {
            let mut it = group_els.into_iter();
            let left = it.next().unwrap().border_right(palette::LINE);
            let right = it.next().unwrap();
            div().flex_1().self_stretch().flex_row().child(left).child(right)
        }
        (_, 3) => {
            let mut it = group_els.into_iter();
            let left = it.next().unwrap().border_right(palette::LINE);
            let top_right = it.next().unwrap().border_bottom(palette::LINE);
            let bot_right = it.next().unwrap();
            div()
                .flex_1()
                .self_stretch()
                .flex_row()
                .child(left)
                .child(div().flex_1().self_stretch().flex_col().child(top_right).child(bot_right))
        }
        _ => {
            let mut it = group_els.into_iter();
            let tl = it.next().unwrap().border_right(palette::LINE);
            let tr = it.next().unwrap();
            let bl = it.next().unwrap().border_right(palette::LINE);
            let br = it.next().unwrap();
            div()
                .flex_1()
                .self_stretch()
                .flex_col()
                .child(
                    div()
                        .flex_1()
                        .self_stretch()
                        .flex_row()
                        .border_bottom(palette::LINE)
                        .child(tl)
                        .child(tr),
                )
                .child(div().flex_1().self_stretch().flex_row().child(bl).child(br))
        }
    };

    container = container.child(grid);

    // The docked Ask, or the focused pane's message row.
    if let Some(dock) = &props.dock {
        container = container.child(dock.clone());
    } else if let Some(group) = props.panes.groups.get(focused_idx)
        && let Some(active_tab) =
            group.tabs.get(group.active_tab.min(group.tabs.len().saturating_sub(1)))
        && let Some(info) = resolve_tab_info(props.clients, active_tab)
    {
        container = container.child(render_message_row(
            &info.project,
            &info.harness,
            props.message_draft,
            props.ghost,
            props.typing,
        ));
    }

    container
}

/// What a tab shows: the agent's status shape when it is not plainly
/// running, its name, and ⚡ in Turbo.
fn tab_label(
    props: &PanesProps,
    tab: &SavedPaneTab,
) -> (String, Option<(&'static str, &'static str)>, bool) {
    match resolve_tab_info(props.clients, tab) {
        Some(i) => {
            let state = if i.running { State::Running } else { State::Ended };
            let shape = crate::tree_view::status_shape(state, i.turn);
            (format!("{} · {}", i.project, i.harness), Some(shape), i.turbo && i.running)
        }
        // An agent no longer known here has ended, as far as Weft can tell.
        None => {
            let proj = ProjectClient::name_of(&tab.project);
            let shape = crate::tree_view::status_shape(State::Ended, None);
            (proj, Some(shape), false)
        }
    }
}

/// The view switch and the layout button, for the focused pane.
fn render_view_controls(g_idx: usize, active_idx: usize, view: PaneViewMode) -> Vec<Element> {
    let seg = |selected: bool, layer: &str, mode: PaneViewMode, word: &str| {
        let mut el = div()
            .layer(if selected { format!("{layer} selected") } else { layer.to_string() })
            .on_click(PaneAct::SetView { group: g_idx, tab: active_idx, view: mode })
            .pad(4, 12)
            .font_size(12)
            .text(word.to_string());
        if selected {
            el = el.bg(palette::SELECTED_BG).color(palette::BLUE_BORDER).font_weight(600)
        } else {
            el = el.bg(palette::SURFACE).color(palette::MUTED)
        }
        el
    };
    let term = view == PaneViewMode::Terminal;
    let mut first =
        seg(term, "seg Terminal", PaneViewMode::Terminal, words::word("web.view.terminal"));
    let second = seg(
        !term,
        "seg Conversation",
        PaneViewMode::Conversation,
        words::word("web.view.conversation"),
    );
    first = first.border_right(if term { palette::BLUE_BORDER } else { palette::LINE });
    let switch =
        div().layer("view switch").flex_row().border(palette::LINE).child(first).child(second);
    let layout_btn = div()
        .layer("layout button")
        .role("button")
        .aria_label(words::word("act.layout"))
        .on_click(PaneAct::ToggleLayoutMenu(g_idx))
        .flex_row()
        .gap(6)
        .pad(4, 8)
        .items_center()
        .bg(palette::SURFACE)
        .border(palette::LINE)
        .child(span("⊞").font_size(14).color(palette::TEXT))
        .child(span("▾").font_size(10).color(palette::MUTED));
    vec![switch, layout_btn]
}

/// Split, one bar above the panes carries the view switch and the layout
/// button once, for the focused pane.
fn render_split_toolbar(props: &PanesProps, g_idx: usize, group: &SavedTabGroup) -> Element {
    let active_idx = group.active_tab.min(group.tabs.len().saturating_sub(1));
    let tab = group.tabs.get(active_idx);
    let view = tab.map(|t| t.view).unwrap_or(PaneViewMode::Terminal);
    let focused = tab
        .map(|t| tab_label(props, t))
        .map(|(label, _, turbo)| if turbo { format!("{label} ⚡") } else { label });
    let mut info = div()
        .flex_row()
        .gap(6)
        .font_mono()
        .font_size(11)
        .color(palette::MUTED)
        .child(span(words::fill("panes.n", &[("n", &props.panes.groups.len().to_string())])));
    if let Some(f) = focused {
        info = info
            .child(span(words::word("panes.focused")))
            .child(span(f).color(palette::BLUE_BORDER).font_weight(600));
    }
    div()
        .layer("split toolbar")
        .self_stretch()
        .h(36)
        .shrink_0()
        .flex_row()
        .pad(0, 12)
        .items_center()
        .justify_between()
        .bg(palette::SURFACE_ALT)
        .border_bottom(palette::LINE)
        .child(info)
        .child(
            div()
                .flex_row()
                .gap(8)
                .items_center()
                .children(render_view_controls(g_idx, active_idx, view)),
        )
}

fn render_pane_group(
    props: &PanesProps,
    g_idx: usize,
    group: &SavedTabGroup,
    split: bool,
) -> Element {
    let active_idx = group.active_tab.min(group.tabs.len().saturating_sub(1));
    let active_tab = group.tabs.get(active_idx);
    let focused = props.panes.focused_group == g_idx;

    let mut tab_strip = div()
        .layer("tab strip")
        .self_stretch()
        .h(if split { 30 } else { 38 })
        .shrink_0()
        .flex_row()
        .style("padding", "0 12px 0 0")
        .bg(palette::SURFACE_ALT)
        .border_bottom(palette::LINE);

    for (t_idx, tab) in group.tabs.iter().enumerate() {
        let is_active = t_idx == active_idx;
        let (label, shape, turbo) = tab_label(props, tab);
        let mut el = div()
            .layer(format!("tab {label}"))
            .on_click(PaneAct::SelectTab { group: g_idx, tab: t_idx })
            .attr("draggable", "true")
            .self_stretch()
            .flex_row()
            .gap(6)
            .pad(0, if split { 12 } else { 16 })
            .items_center()
            .border_right(palette::LINE)
            .font_size(if split { 12 } else { 13 });
        el = if is_active && (focused || !split) {
            el.style("position", "relative")
                .bg(palette::SELECTED_BG)
                .child(edge_bar("top", 2, palette::BLUE_BORDER))
                .color(palette::BLUE_BORDER)
                .font_weight(500)
        } else if is_active {
            el.bg(palette::SURFACE).color(palette::TEXT).font_weight(500)
        } else {
            el.color(palette::MUTED)
        };
        // Every tab leads with its agent's status.
        if let Some((glyph, colour)) = shape {
            el = el.child(span(glyph).font_mono().font_size(10).color(colour));
        }
        el = el.child(div().style("white-space", "nowrap").text(label));
        if turbo {
            el = el.child(span("⚡").font_size(11).color(palette::AMBER));
        }
        // Its close shows on the tab in front, in the pane you are in.
        if is_active && (focused || !split) {
            el = el.child(
                span("✕")
                    .layer("close tab")
                    .on_click(PaneAct::CloseTab { group: g_idx, tab: t_idx })
                    .font_size(11)
                    .color(palette::DIM),
            );
        }
        tab_strip = tab_strip.child(el);
    }
    tab_strip = tab_strip.child(div().flex_1());
    if !split {
        let view = active_tab.map(|t| t.view).unwrap_or(PaneViewMode::Terminal);
        tab_strip = tab_strip.child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .items_center()
                .children(render_view_controls(g_idx, active_idx, view)),
        );
    }

    let body = if let Some(tab) = active_tab {
        match tab.view {
            PaneViewMode::Terminal => render_terminal_viewport(props, g_idx, tab, split),
            PaneViewMode::Conversation => {
                render_conversation_viewport(props, g_idx, active_idx, tab)
            }
        }
    } else {
        div().flex_1().self_stretch()
    };

    let mut pane_col = div()
        .layer(format!("pane group {g_idx}"))
        .flex_1()
        .self_stretch()
        .flex_col()
        .style("min-height", "0")
        .style("position", "relative")
        .child(tab_strip)
        .child(body);

    if props.layout_menu_pane == Some(g_idx) {
        pane_col = pane_col.child(render_layout_menu(g_idx));
    }

    pane_col
}

fn render_layout_menu(g_idx: usize) -> Element {
    div()
        .layer("layout menu")
        .style("position", "absolute")
        .style("top", "46px")
        .style("right", "12px")
        .w(240)
        .pad(6, 0)
        .bg(palette::SURFACE)
        .border(palette::LINE)
        .panel_corners()
        .font_size(14)
        .child(
            div()
                .layer("menu Split right")
                .on_click(PaneAct::SplitRight(g_idx))
                .pad(8, 14)
                .bg(palette::SELECTED_BG)
                .text(words::word("web.layout.split_right")),
        )
        .child(
            div()
                .layer("menu Split down")
                .on_click(PaneAct::SplitDown(g_idx))
                .pad(8, 14)
                .text(words::word("web.layout.split_down")),
        )
        .child(
            div()
                .layer("menu Move tab to the next pane")
                .on_click(PaneAct::MoveTabNext(g_idx))
                .pad(8, 14)
                .text(words::word("web.layout.next_pane")),
        )
        .child(
            div()
                .layer("menu Join all panes")
                .on_click(PaneAct::JoinAll)
                .pad(8, 14)
                .text(words::word("web.layout.join")),
        )
}

fn render_terminal_viewport(
    props: &PanesProps,
    g_idx: usize,
    tab: &SavedPaneTab,
    split: bool,
) -> Element {
    let is_focused = props.panes.focused_group == g_idx;

    let mut pane = div()
        .layer(if is_focused { "pane" } else { "pane inactive" })
        .on_click(PaneAct::Focus(g_idx))
        .self_stretch()
        .flex_1()
        .flex_col()
        .items_start()
        .style("min-height", "0")
        .style("overflow", "hidden")
        .bg(palette::TERMINAL_BG);
    pane = if split { pane.pad(12, 14) } else { pane.pad(16, 20) };
    // Split, the focused pane is framed; alone, it needs no frame.
    if split && is_focused {
        pane = pane.style("border", format!("2px solid {}", palette::BLUE_BORDER));
    }
    // The window paints the pane's own terminal here.
    pane.child(terminal_el(&tab.project, tab.pane).layer("terminal").self_stretch().flex_1())
}

fn render_conversation_viewport(
    props: &PanesProps,
    g_idx: usize,
    t_idx: usize,
    tab: &SavedPaneTab,
) -> Element {
    let info = resolve_tab_info(props.clients, tab);
    let harness = info.as_ref().map(|i| i.harness.clone()).unwrap_or_default();
    let turn = info.as_ref().and_then(|i| i.turn);
    let zone = props.clients.iter().find(|c| c.root == tab.project).map_or(0, |c| c.zone);

    let has_items = props.conversations.contains_key(&(tab.project.clone(), tab.pane));
    let supports_conversation = has_items
        || props
            .clients
            .iter()
            .find(|c| c.root == tab.project)
            .and_then(|c| c.harnesses.find(&harness))
            .map(|h| h.conversation.is_some())
            .unwrap_or(true);

    let mut chat_col = div()
        .layer("chat column")
        .self_stretch()
        .style("max-width", "740px")
        .flex_col()
        .gap(16)
        .items_start();

    if !supports_conversation {
        chat_col = chat_col.child(
            div()
                .layer("conversation unreadable")
                .font_size(14)
                .color(palette::MUTED)
                .text(words::say("web.conversation.unreadable", &harness)),
        );
    } else if let Some(items) = props.conversations.get(&(tab.project.clone(), tab.pane)) {
        chat_col = chat_col.children(conversation_blocks(
            items,
            &harness,
            &tab.project,
            tab.pane,
            props.unfolded_steps,
            turn == Some(Turn::Waiting),
            zone,
        ));
    }

    if turn == Some(Turn::Waiting) {
        let waiting_prefix = words::say("web.waiting.for_you", &harness);
        let waiting_cta = words::word("web.waiting.answer");
        chat_col = chat_col.child(
            div()
                .layer("waiting line")
                .on_click(PaneAct::SetView {
                    group: g_idx,
                    tab: t_idx,
                    view: PaneViewMode::Terminal,
                })
                .self_stretch()
                .flex_row()
                .pad(8, 14)
                .items_center()
                .justify_between()
                .bg(palette::AMBER_BG)
                .border(palette::AMBER_BORDER)
                .font_size(12)
                .color(palette::AMBER)
                .child(span(waiting_prefix))
                .child(span(waiting_cta).font_weight(600)),
        );
    }

    div()
        .layer("pane conversation")
        .self_stretch()
        .flex_1()
        .flex_col()
        .pad(24, 20)
        .items_center()
        .bg(palette::TERMINAL_BG)
        .style("min-height", "0")
        .style("overflow-y", "auto")
        .style("overflow-x", "hidden")
        .child(chat_col)
}

/// The conversation's blocks, in order.
fn conversation_blocks(
    items: &[Item],
    harness: &str,
    project: &std::path::Path,
    pane: u32,
    unfolded_steps: &HashSet<(PathBuf, u32, usize)>,
    waiting: bool,
    zone: i64,
) -> Vec<Element> {
    // The last thing the agent said leads with its state: the square while it
    // waits for you, the circle otherwise.
    let mut chat_col = Vec::new();
    let last_agent = items.iter().rposition(|it| it.kind == Kind::Agent);
    // Map call IDs to settled outcomes
    let mut outcomes: HashMap<&str, Option<bool>> = HashMap::new();
    for item in items {
        if item.kind == Kind::Outcome
            && let Some(call) = item.call.as_deref()
        {
            outcomes.insert(call, item.failed);
        }
    }

    let mut i = 0;
    let mut block_idx = 0usize;
    let mut agent_msg_count = 0usize;

    while i < items.len() {
        match items[i].kind {
            Kind::Person => {
                let text = items[i].text.as_deref().unwrap_or("");
                let clock =
                    items[i].at.map_or_else(String::new, |at| crate::tree_view::clock(at, zone));
                chat_col.push(
                    div()
                        .layer("msg you")
                        .self_stretch()
                        .style("position", "relative")
                        .flex_col()
                        .gap(4)
                        .style("padding", "12px 16px 12px 19px")
                        .bg(palette::SURFACE)
                        .border(palette::LINE)
                        .child(edge_bar("left", 3, palette::BLUE_BORDER))
                        .child(
                            div()
                                .self_stretch()
                                .flex_row()
                                .justify_between()
                                .font_mono()
                                .font_size(11)
                                .child(span("YOU").font_weight(600).color(palette::BLUE_BORDER))
                                .child(span(clock).color(palette::DIM)),
                        )
                        .child(
                            div().font_size(14).style("line-height", "21px").text(text.to_string()),
                        ),
                );
                i += 1;
            }
            Kind::Agent => {
                agent_msg_count += 1;
                let layer = if agent_msg_count == 1 {
                    format!("msg {harness}")
                } else {
                    format!("msg {harness} {agent_msg_count}")
                };
                let text = items[i].text.as_deref().unwrap_or("");
                let clock =
                    items[i].at.map_or_else(String::new, |at| crate::tree_view::clock(at, zone));
                let (shape, colour) = if waiting && last_agent == Some(i) {
                    ("■", palette::AMBER)
                } else {
                    ("●", palette::TEAL)
                };
                chat_col.push(
                    div()
                        .layer(layer)
                        .self_stretch()
                        .style("position", "relative")
                        .flex_col()
                        .gap(4)
                        .style("padding", "4px 0 4px 16px")
                        .child(edge_bar("left", 2, palette::LINE))
                        .child(
                            div()
                                .flex_row()
                                .gap(8)
                                .items_baseline()
                                .font_mono()
                                .font_size(11)
                                .child(
                                    span(format!("{shape} {harness}"))
                                        .font_weight(600)
                                        .color(colour),
                                )
                                .child(span(clock).color(palette::DIM)),
                        )
                        .child(
                            div().font_size(14).style("line-height", "21px").text(text.to_string()),
                        ),
                );
                i += 1;
            }
            Kind::Step => {
                let mut steps_group = Vec::new();
                while i < items.len() {
                    if items[i].kind == Kind::Step {
                        steps_group.push(&items[i]);
                        i += 1;
                    } else if items[i].kind == Kind::Outcome {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let is_open = unfolded_steps.contains(&(project.to_path_buf(), pane, block_idx));
                chat_col.push(render_step_block(
                    &steps_group,
                    &outcomes,
                    project,
                    pane,
                    block_idx,
                    is_open,
                ));
                block_idx += 1;
            }
            Kind::Outcome => {
                i += 1;
            }
        }
    }

    chat_col
}

fn render_step_block(
    steps: &[&Item],
    outcomes: &HashMap<&str, Option<bool>>,
    project: &std::path::Path,
    pane: u32,
    block_idx: usize,
    is_open: bool,
) -> Element {
    let n = steps.len();
    let count_label = if n == 1 {
        words::word("web.steps.one").to_string()
    } else {
        words::fill("web.steps.many", &[("n", &n.to_string())])
    };
    let caret = if is_open { "▾" } else { "▸" };
    let toggle_act =
        PaneAct::ToggleSteps { project: project.to_path_buf(), pane, block: block_idx };

    let mut box_el = div()
        .layer(if is_open { "tool steps open" } else { "tool steps folded" })
        .on_click(toggle_act)
        .self_stretch()
        .style("position", "relative")
        .flex_col()
        .gap(4)
        .style("padding", "8px 14px 8px 16px")
        .bg(palette::SURFACE_ALT)
        .border(palette::LINE)
        .child(edge_bar("left", 2, palette::TEAL))
        .font_mono()
        .font_size(12)
        .style("line-height", "18px");
    let mut head = div()
        .color(palette::MUTED)
        .style("white-space", "nowrap")
        .text(format!("{caret} {count_label}"));
    if is_open {
        head = head.style("padding", "0 0 4px 0").border_bottom(palette::LINE);
    }
    box_el = box_el.child(head);

    if is_open {
        for s in steps {
            let tool = s.tool.as_deref().unwrap_or("Tool");
            let arg = s.arg.as_deref().unwrap_or("");
            let failed =
                s.call.as_deref().and_then(|c| outcomes.get(c).copied().flatten()).or(s.failed);
            let (badge, badge_color) = match failed {
                Some(true) => ("✗", palette::RED),
                Some(false) => ("✓", palette::TEAL),
                None => (words::word("web.step.running"), palette::MUTED),
            };
            box_el = box_el.child(
                div()
                    .layer(format!("step {tool}"))
                    .flex_row()
                    .gap(12)
                    .items_baseline()
                    .child(div().shrink_0().color(badge_color).text(badge))
                    .child(div().w(64).shrink_0().color(palette::TEXT).text(tool))
                    .child(
                        div()
                            .flex_1()
                            .color(palette::MUTED)
                            .style("white-space", "nowrap")
                            .style("overflow", "hidden")
                            .style("text-overflow", "ellipsis")
                            .text(arg),
                    ),
            );
        }
    }

    box_el
}

fn render_message_row(
    project: &str,
    harness: &str,
    draft: &str,
    ghost: Option<&str>,
    typing: bool,
) -> Element {
    let placeholder =
        words::fill("desktop.message.placeholder", &[("project", project), ("harness", harness)]);
    div()
        .layer("the message row")
        .self_stretch()
        .flex_row()
        .gap(8)
        .pad(10, 16)
        .items_center()
        .bg(palette::SURFACE_ALT)
        .border_top(palette::LINE)
        .child(
            input_el(Focus::Message)
                .flex_1()
                .pad(9, 12)
                .bg(palette::SURFACE)
                .border(if typing { palette::BLUE_BORDER } else { palette::LINE })
                .placeholder(placeholder)
                .font_size(13)
                .color(palette::TEXT)
                .text(draft.to_string())
                .children(ghost.map(|_| {
                    // The key that takes the suggestion, at the field's right.
                    div()
                        .layer("take suggestion")
                        .on_click(PaneAct::TakeSuggestion)
                        .shrink_0()
                        .pad(0, 5)
                        .font_mono()
                        .font_size(10)
                        .color(palette::DIM)
                        .border(palette::LINE)
                        .text(words::word("suggest.take"))
                })),
        )
        .child(
            div()
                .layer("send message button")
                .on_click(PaneAct::SendMessage)
                .flex_row()
                .pad(9, 14)
                .items_center()
                .bg(palette::BLUE)
                .border(palette::BLUE_BORDER)
                .child(div().font_size(13).font_weight(700).color(palette::CANVAS).text("▶")),
        )
}

/// What a tab's agent is, as its project last said.
struct TabInfo {
    project: String,
    harness: String,
    turn: Option<Turn>,
    turbo: bool,
    running: bool,
}

fn resolve_tab_info(clients: &[ProjectClient], tab: &SavedPaneTab) -> Option<TabInfo> {
    let client = clients.iter().find(|c| c.root == tab.project)?;
    if let Some(p) = client.view(tab.pane) {
        return Some(TabInfo {
            project: client.name.clone(),
            harness: p.harness.clone(),
            turn: p.turn,
            turbo: p.turbo,
            running: p.running,
        });
    }
    let s = client.sessions.iter().find(|s| s.pane == Some(tab.pane))?;
    Some(TabInfo {
        project: client.name.clone(),
        harness: s.harness.clone(),
        turn: s.turn,
        turbo: client.turbo,
        running: s.state == weft_core::sessions::State::Running,
    })
}
