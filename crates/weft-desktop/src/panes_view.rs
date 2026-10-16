//! Editor-style 4-pane split layout (`D15.1`–`D15.4`), draggable tab bars,
//! layout menu, `vt100` terminal view, `Terminal | Conversation` switch
//! (`D12.1`–`D12.4`), `D3.4` `NEEDS YOU` (`↑ ↓ ⏎ Esc`) strip, and `MessageRow`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use weft_core::conversation::{Item, Kind};
use weft_core::state::{PaneViewMode, SavedPaneTab, SavedPanesLayout, SavedTabGroup, SplitLayout};
use weft_core::turns::Turn;
use weft_core::words;

use crate::daemon::ProjectClient;
use crate::gpui::{Element, div, input_el, palette, span};

/// Where a dragged tab is dropped (`D15.4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    TabBar { group: usize, index: usize },
    EdgeRight { group: usize },
    EdgeDown { group: usize },
    Center { group: usize },
}

/// Split `group_idx` to the right, moving its active tab into the new pane (`D15.1`–`D15.3`).
pub fn split_pane_right(
    panes: &mut SavedPanesLayout,
    group_idx: usize,
) -> std::result::Result<(), &'static str> {
    split_pane(panes, group_idx, true)
}

/// Split `group_idx` downward, moving its active tab into the new pane (`D15.1`–`D15.3`).
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

/// Move the active tab of `group_idx` to the next pane (`D15.2`).
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

/// Join all panes into a single pane holding every tab (`D15.2`).
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

/// Drag a tab from `(from_group, from_tab)` and drop it at `target` (`D15.4`).
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
}

/// Render the right-side multi-pane workspace (`D15.1`–`D15.4`, `D12.1`–`D12.4`, `D3.4`).
pub fn render_panes_workspace(props: &PanesProps) -> Element {
    let mut container =
        div().layer("workspace").flex_1().self_stretch().flex_col().style("min-height", "0");

    if props.panes.groups.is_empty() {
        let woven = "╸┃╺╸┃╺╸┃╺╸┃╺\n┃╺╸┃╺╸┃╺╸┃╺╸\n╺╸┃╺╸┃╺╸┃╺╸┃\n╸┃╺╸┃╺╸┃╺╸┃╺\n┃╺╸┃╺╸┃╺╸┃╺╸\n╺╸┃╺╸┃╺╸┃╺╸┃\n╸┃╺╸┃╺╸┃╺╸┃╺";
        return container.child(
            div()
                .layer("empty panes")
                .flex_1()
                .self_stretch()
                .flex_col()
                .gap(24)
                .items_center()
                .justify_center()
                .child(
                    div()
                        .layer("woven mark")
                        .font_mono()
                        .font_weight(700)
                        .font_size(22)
                        .style("line-height", "26px")
                        .color(palette::TEAL)
                        .style("white-space", "pre")
                        .style("user-select", "none")
                        .text(woven),
                )
                .child(
                    div()
                        .layer("btn Start an agent")
                        .on_click("open_start_sheet")
                        .flex_row()
                        .gap(8)
                        .pad(10, 18)
                        .items_center()
                        .justify_center()
                        .bg(palette::SURFACE)
                        .rounded(8)
                        .border(palette::LINE)
                        .child(
                            div().font_weight(600).font_size(14).text(words::word("start.title")),
                        ),
                ),
        );
    }

    let group_els: Vec<Element> = props
        .panes
        .groups
        .iter()
        .enumerate()
        .map(|(g_idx, g)| render_pane_group(props, g_idx, g))
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

    // Focused pane's D3.4 NEEDS YOU strip (when in Terminal mode and Waiting)
    // and bottom MessageRow (`the message row`).
    let focused_idx = props.panes.focused_group.min(props.panes.groups.len().saturating_sub(1));
    if let Some(group) = props.panes.groups.get(focused_idx)
        && let Some(active_tab) =
            group.tabs.get(group.active_tab.min(group.tabs.len().saturating_sub(1)))
        && let Some((proj_name, harness, turn, _turbo, _running, _contents)) =
            resolve_tab_info(props.clients, active_tab)
    {
        if active_tab.view == PaneViewMode::Terminal && turn == Some(Turn::Waiting) {
            container = container.child(render_needs_you_strip(focused_idx));
        }
        container = container.child(render_message_row(&proj_name, &harness, props.message_draft));
    }

    container
}

fn render_pane_group(props: &PanesProps, g_idx: usize, group: &SavedTabGroup) -> Element {
    let active_idx = group.active_tab.min(group.tabs.len().saturating_sub(1));
    let active_tab = group.tabs.get(active_idx);

    let mut tabs_row = div()
        .layer("tabs, scrolling")
        .flex_1()
        .flex_row()
        .gap(6)
        .items_center()
        .style("overflow", "hidden");

    for (t_idx, tab) in group.tabs.iter().enumerate() {
        let is_active = t_idx == active_idx;
        let (proj_name, harness, turn, turbo, running, _) = resolve_tab_info(props.clients, tab)
            .unwrap_or_else(|| {
                (
                    tab.project
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("project")
                        .to_string(),
                    "agent".to_string(),
                    None,
                    false,
                    false,
                    String::new(),
                )
            });
        let label = format!("{proj_name} · {harness}");
        let mut pill = div()
            .layer(format!("tab {label}"))
            .on_click(format!("select_tab:{g_idx}:{t_idx}"))
            .attr("draggable", "true")
            .flex_row()
            .gap(6)
            .pad(6, 12)
            .items_center()
            .rounded(16)
            .bg(if is_active { palette::SELECTED_BG } else { palette::SURFACE })
            .border(if is_active { palette::BLUE_BORDER } else { palette::LINE })
            .font_size(13)
            .color(if is_active { palette::TEXT } else { palette::MUTED });

        if turn == Some(Turn::Waiting) {
            pill = pill.child(div().font_size(11).color(palette::AMBER).text("●"));
        } else if !running {
            pill = pill.child(div().font_size(11).color(palette::DIM).text("○"));
        }
        pill = pill.child(
            div()
                .font_weight(if is_active { 600 } else { 400 })
                .style("white-space", "nowrap")
                .text(label),
        );
        if turbo {
            pill = pill.child(div().font_size(12).color(palette::AMBER).text("⚡"));
        }
        pill = pill.child(
            span("✕")
                .on_click(format!("close_tab:{g_idx}:{t_idx}"))
                .font_size(11)
                .color(palette::DIM),
        );
        tabs_row = tabs_row.child(pill);
    }

    let current_view = active_tab.map(|t| t.view).unwrap_or(PaneViewMode::Terminal);
    let term_selected = current_view == PaneViewMode::Terminal;
    let conv_selected = current_view == PaneViewMode::Conversation;

    let view_switch = div()
        .layer("view switch")
        .flex_row()
        .pad(2, 2)
        .items_center()
        .rounded(8)
        .bg(palette::SURFACE)
        .border(palette::LINE)
        .font_size(12)
        .font_weight(600)
        .child(
            div()
                .layer(if term_selected { "seg Terminal selected" } else { "seg Terminal" })
                .on_click(format!("set_tab_view:{g_idx}:{active_idx}:terminal"))
                .pad(4, 10)
                .rounded(6)
                .bg(if term_selected { palette::SELECTED_BG } else { "transparent" })
                .color(if term_selected { palette::TEXT } else { palette::MUTED })
                .text(words::word("web.view.terminal")),
        )
        .child(
            div()
                .layer(if conv_selected { "seg Conversation selected" } else { "seg Conversation" })
                .on_click(format!("set_tab_view:{g_idx}:{active_idx}:conversation"))
                .pad(4, 10)
                .rounded(6)
                .bg(if conv_selected { palette::SELECTED_BG } else { "transparent" })
                .color(if conv_selected { palette::TEXT } else { palette::MUTED })
                .text(words::word("web.view.conversation")),
        );

    let layout_btn = div()
        .layer("layout button")
        .role("button")
        .aria_label(words::word("act.layout"))
        .on_click(format!("toggle_layout_menu:{g_idx}"))
        .flex_row()
        .gap(5)
        .pad(6, 10)
        .items_center()
        .rounded(8)
        .bg(palette::SURFACE)
        .border(palette::LINE)
        .font_size(13)
        .font_weight(600)
        .child(span("⊞").font_size(14).color(palette::TEXT))
        .child(span("▾").font_size(11).color(palette::MUTED));

    let tab_strip = div()
        .layer("tab strip")
        .self_stretch()
        .flex_row()
        .gap(8)
        .pad(8, 14)
        .items_center()
        .border_bottom(palette::LINE)
        .child(tabs_row)
        .child(view_switch)
        .child(layout_btn);

    let body = if let Some(tab) = active_tab {
        match tab.view {
            PaneViewMode::Terminal => render_terminal_viewport(props, g_idx, tab),
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
        .rounded(10)
        .style("box-shadow", "0 10px 28px rgba(0,0,0,0.45)")
        .font_size(14)
        .child(
            div()
                .layer("menu Split right")
                .on_click(format!("split_right:{g_idx}"))
                .pad(8, 14)
                .bg(palette::SELECTED_BG)
                .text(words::word("web.layout.split_right")),
        )
        .child(
            div()
                .layer("menu Split down")
                .on_click(format!("split_down:{g_idx}"))
                .pad(8, 14)
                .text(words::word("web.layout.split_down")),
        )
        .child(
            div()
                .layer("menu Move tab to the next pane")
                .on_click(format!("move_tab_next:{g_idx}"))
                .pad(8, 14)
                .text(words::word("web.layout.next_pane")),
        )
        .child(
            div()
                .layer("menu Join all panes")
                .on_click("join_all_panes")
                .pad(8, 14)
                .text(words::word("web.layout.join")),
        )
}

fn render_terminal_viewport(props: &PanesProps, g_idx: usize, tab: &SavedPaneTab) -> Element {
    let contents =
        resolve_tab_info(props.clients, tab).map(|(_, _, _, _, _, c)| c).unwrap_or_default();
    let is_focused = props.panes.focused_group == g_idx;

    div()
        .self_stretch()
        .flex_1()
        .flex_col()
        .style("min-height", "0")
        .gap(8)
        .pad(12, 14)
        .items_start()
        .child(
            div()
                .layer(if is_focused { "pane" } else { "pane inactive" })
                .on_click(format!("focus_pane_group:{g_idx}"))
                .self_stretch()
                .flex_1()
                .flex_col()
                .pad(12, 12)
                .items_start()
                .style("min-height", "0")
                .style("overflow", "hidden")
                .bg(palette::TERMINAL_BG)
                .rounded(10)
                .border(if is_focused { palette::BLUE_BORDER } else { palette::LINE })
                .child(
                    // The window paints the pane's own terminal here; the text
                    // stays for what reads this tree.
                    div()
                        .layer("terminal")
                        .attr("data-project", tab.project.to_string_lossy())
                        .attr("data-pane", tab.pane.to_string())
                        .self_stretch()
                        .flex_1()
                        .font_mono()
                        .font_size(12)
                        .style("line-height", "18px")
                        .style("white-space", "pre-wrap")
                        .text(contents),
                ),
        )
}

fn render_conversation_viewport(
    props: &PanesProps,
    g_idx: usize,
    t_idx: usize,
    tab: &SavedPaneTab,
) -> Element {
    let (_, harness, turn, _, _, _) = resolve_tab_info(props.clients, tab)
        .unwrap_or_else(|| ("project".into(), "agent".into(), None, false, false, String::new()));

    let has_items = props.conversations.contains_key(&(tab.project.clone(), tab.pane));
    let supports_conversation = has_items
        || props
            .clients
            .iter()
            .find(|c| c.root == tab.project)
            .and_then(|c| c.harnesses.find(&harness))
            .map(|h| h.conversation.is_some())
            .unwrap_or(true);

    let mut chat_col = div().layer("chat column").w(680).flex_col().gap(16).items_start();

    if !supports_conversation {
        chat_col = chat_col.child(
            div()
                .layer("conversation unreadable")
                .font_size(14)
                .color(palette::MUTED)
                .text(words::say("web.conversation.unreadable", &harness)),
        );
    } else if let Some(items) = props.conversations.get(&(tab.project.clone(), tab.pane)) {
        chat_col = append_conversation_items(
            chat_col,
            items,
            &harness,
            &tab.project,
            tab.pane,
            props.unfolded_steps,
        );
    }

    if turn == Some(Turn::Waiting) {
        let waiting_prefix = words::say("web.waiting.for_you", &harness);
        let waiting_cta = words::word("web.waiting.answer");
        chat_col = chat_col.child(
            div()
                .layer("waiting line")
                .on_click(format!("set_tab_view:{g_idx}:{t_idx}:terminal"))
                .font_size(13)
                .style("line-height", "19px")
                .color(palette::AMBER)
                .style("white-space", "nowrap")
                .child(span(waiting_prefix))
                .child(span(waiting_cta).font_weight(600)),
        );
    }

    div()
        .layer("pane conversation")
        .self_stretch()
        .flex_1()
        .flex_col()
        .pad(20, 24)
        .items_center()
        .style("min-height", "0")
        .style("overflow-y", "auto")
        .style("overflow-x", "hidden")
        .child(chat_col)
}

fn append_conversation_items(
    mut chat_col: Element,
    items: &[Item],
    harness: &str,
    project: &PathBuf,
    pane: u32,
    unfolded_steps: &HashSet<(PathBuf, u32, usize)>,
) -> Element {
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
                let clock = format_item_clock(items[i].at);
                chat_col = chat_col.child(
                    div()
                        .layer("msg you")
                        .style("align-self", "flex-end")
                        .style("max-width", "540px")
                        .flex_col()
                        .gap(4)
                        .items_start()
                        .pad(10, 14)
                        .bg(palette::SELECTED_BG)
                        .rounded(12)
                        .border(palette::BLUE_BORDER)
                        .child(
                            div()
                                .flex_row()
                                .gap(8)
                                .items_baseline()
                                .child(div().font_size(12).color(palette::BLUE_BORDER).text("you"))
                                .child(div().font_size(11).color(palette::MUTED).text(clock)),
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
                let clock = format_item_clock(items[i].at);
                chat_col = chat_col.child(
                    div()
                        .layer(layer)
                        .style("align-self", "flex-start")
                        .style("max-width", "640px")
                        .flex_col()
                        .gap(4)
                        .child(
                            div()
                                .flex_row()
                                .gap(8)
                                .items_baseline()
                                .child(
                                    div()
                                        .font_size(12)
                                        .color(palette::MUTED)
                                        .text(harness.to_string()),
                                )
                                .child(div().font_size(11).color(palette::MUTED).text(clock)),
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
                let is_open = unfolded_steps.contains(&(project.clone(), pane, block_idx));
                chat_col = chat_col.child(render_step_block(
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
    project: &PathBuf,
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
    let proj_str = project.to_string_lossy();
    let toggle_act = format!("toggle_steps:{proj_str}:{pane}:{block_idx}");

    let mut box_el = div()
        .layer(if is_open { "tool steps open" } else { "tool steps folded" })
        .on_click(toggle_act)
        .style("align-self", "flex-start")
        .w(560)
        .flex_col()
        .gap(3)
        .style("padding", "7px 12px 9px 12px")
        .bg(palette::SURFACE_ALT)
        .rounded(8)
        .border(palette::LINE)
        .font_mono()
        .font_size(12)
        .style("line-height", "18px")
        .child(
            div()
                .color(palette::MUTED)
                .style("white-space", "nowrap")
                .text(format!("{caret} {count_label}")),
        );

    if is_open {
        for s in steps {
            let tool = s.tool.as_deref().unwrap_or("Tool");
            let arg = s.arg.as_deref().unwrap_or("");
            let failed =
                s.call.as_deref().and_then(|c| outcomes.get(c).copied().flatten()).or(s.failed);
            let (badge, badge_color) = match failed {
                Some(true) => ("✗", palette::AMBER),
                Some(false) => ("✓", palette::TEAL),
                None => (words::word("web.step.running"), palette::MUTED),
            };
            box_el = box_el.child(
                div()
                    .layer(format!("step {tool}"))
                    .flex_row()
                    .gap(12)
                    .style("padding-left", "16px")
                    .items_baseline()
                    .child(div().w(72).shrink_0().color(palette::TEXT).text(tool))
                    .child(
                        div()
                            .flex_1()
                            .color(palette::MUTED)
                            .style("white-space", "nowrap")
                            .style("overflow", "hidden")
                            .style("text-overflow", "ellipsis")
                            .text(arg),
                    )
                    .child(div().color(badge_color).text(badge)),
            );
        }
    }

    box_el
}

fn render_needs_you_strip(group_idx: usize) -> Element {
    let key_btn = |label: &str, code: &str| {
        div()
            .layer(format!("needs key {label}"))
            .on_click(format!("send_choice_key:{group_idx}:{code}"))
            .flex_row()
            .pad(7, 12)
            .items_center()
            .justify_center()
            .bg(palette::SURFACE)
            .rounded(7)
            .border(palette::LINE)
            .child(div().font_mono().font_size(12).style("line-height", "18px").text(label))
    };

    div()
        .layer("NEEDS YOU strip")
        .self_stretch()
        .flex_row()
        .gap(8)
        .pad(8, 14)
        .items_center()
        .bg(palette::SURFACE_ALT)
        .border_top(palette::LINE)
        .child(
            div()
                .font_weight(600)
                .font_size(11)
                .style("letter-spacing", "0.08em")
                .color(palette::AMBER)
                .text("NEEDS YOU"),
        )
        .child(div().flex_1().flex_row().items_center())
        .child(key_btn("↑", "up"))
        .child(key_btn("↓", "down"))
        .child(key_btn("⏎", "enter"))
        .child(key_btn("Esc", "esc"))
}

fn render_message_row(project: &str, harness: &str, draft: &str) -> Element {
    let placeholder = format!("Message {project} · {harness}…");
    div()
        .layer("the message row")
        .self_stretch()
        .flex_row()
        .gap(8)
        .pad(10, 14)
        .items_center()
        .bg(palette::SURFACE_ALT)
        .border_top(palette::LINE)
        .child(
            input_el()
                .flex_1()
                .pad(9, 14)
                .bg(palette::SURFACE)
                .rounded(20)
                .border(palette::LINE)
                .placeholder(placeholder)
                .font_size(13)
                .color(palette::TEXT)
                .style("outline", "none")
                .text(draft.to_string()),
        )
        .child(
            div()
                .layer("send message button")
                .on_click("send_message_row")
                .flex_row()
                .pad(8, 13)
                .items_center()
                .bg(palette::BLUE)
                .rounded(20)
                .child(div().font_size(13).color("#ffffff").text("▶")),
        )
}

fn resolve_tab_info(
    clients: &[ProjectClient],
    tab: &SavedPaneTab,
) -> Option<(String, String, Option<Turn>, bool, bool, String)> {
    let client = clients.iter().find(|c| c.root == tab.project)?;
    if let Some(p) = client.view(tab.pane) {
        return Some((
            client.name.clone(),
            p.harness.clone(),
            p.turn,
            p.turbo,
            p.running,
            p.contents(),
        ));
    }
    if let Some(s) = client.sessions.iter().find(|s| s.pane == Some(tab.pane)) {
        return Some((
            client.name.clone(),
            s.harness.clone(),
            s.turn,
            client.turbo,
            s.state == weft_core::sessions::State::Running,
            String::new(),
        ));
    }
    None
}

fn format_item_clock(at: Option<i64>) -> String {
    let Some(ms) = at else {
        return "11:20".to_string();
    };
    if ms < 2400 && ms > 0 {
        let h = (ms / 100).rem_euclid(24);
        let m = ms.rem_euclid(100).min(59);
        return format!("{h:02}:{m:02}");
    }
    let total_mins = (ms / 60_000).rem_euclid(24 * 60);
    format!("{:02}:{:02}", total_mins / 60, total_mins % 60)
}
