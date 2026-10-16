//! The tree: one-line rows of projects, agents and their work, each agent
//! led by its status shape, built over `weft_core::tree` and `weft_core::words`.

use weft_core::harness::Harnesses;
use weft_core::sessions::State;
use weft_core::tree::{self, Agent, Ask, Group, Project};
use weft_core::turns::Turn;
use weft_core::words;

use crate::gpui::{Element, div, edge_bar, input_el, palette, span};

/// Which row in the `Tree` is currently selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeSelection {
    Project { project: String },
    Agent { project: String, session: String },
    EarlierSessions { project: String },
    OpenGroup { project: String },
    Ask { project: String, unit: String },
    EvalGroup { project: String, eval: String },
    SealedGroup { project: String },
    Seal { project: String, seal: String },
}

pub struct TreeProps<'a> {
    pub projects: &'a [Project],
    pub selection: Option<&'a TreeSelection>,
    pub search_query: &'a str,
    pub plus_popover_project: Option<&'a str>,
    pub harnesses: &'a Harnesses,
    pub turbo: bool,
    pub unfolded_earlier: &'a std::collections::HashSet<String>,
    pub clients: &'a [crate::daemon::ProjectClient],
}

/// Render the `380px` left `Tree` panel.
pub fn render_tree(props: &TreeProps) -> Element {
    let mut tree_el = div()
        .layer("tree")
        .w(340)
        .shrink_0()
        .self_stretch()
        .flex_col()
        .items_start()
        .style("min-height", "0")
        .style("overflow-y", "auto")
        .style("overflow-x", "hidden")
        .border_right(palette::LINE)
        .child(render_search_box(props.search_query));

    let q = props.search_query.trim();
    if !q.is_empty() {
        let mut any_match = false;
        for proj in props.projects {
            let mut all_agents: Vec<Agent> = proj.agents.rows.clone();
            if let Some(client) = props.clients.iter().find(|c| c.name == proj.name) {
                let (_, earlier_rows) = weft_core::sessions::split(&client.sessions);
                for r in earlier_rows {
                    all_agents.push(Agent {
                        pane: r.pane,
                        session: r.id.clone(),
                        harness: r.harness.clone(),
                        title: r.title.clone(),
                        state: r.state,
                        turn: None,
                        at: r.last,
                        asks: r.asks,
                        ask: None,
                    });
                }
            }
            let matching_agents: Vec<&Agent> =
                all_agents.iter().filter(|a| tree::agent_matches(a, q)).collect();
            let matching_asks: Vec<&Ask> = proj
                .work
                .open
                .asks
                .iter()
                .chain(proj.work.evals.iter().flat_map(|g| g.asks.iter()))
                .filter(|a| tree::ask_matches(a, q))
                .collect();
            let total = matching_agents.len() + matching_asks.len();
            if total == 0 {
                continue;
            }
            any_match = true;
            let turbo =
                props.clients.iter().find(|c| c.name == proj.name).map_or(props.turbo, |c| c.turbo);
            tree_el = tree_el.child(render_search_project_row(proj, total));
            if !matching_agents.is_empty() {
                tree_el = tree_el
                    .child(render_section_header("section AGENTS", words::word("tree.agents")));
                for a in matching_agents {
                    let selected = props.selection
                        == Some(&TreeSelection::Agent {
                            project: proj.name.clone(),
                            session: a.session.clone(),
                        });
                    tree_el = tree_el.child(render_agent_row(&proj.name, a, selected, turbo));
                }
            }
            if !matching_asks.is_empty() {
                tree_el =
                    tree_el.child(render_section_header("section WORK", words::word("tree.work")));
                for ask in matching_asks {
                    let selected = props.selection
                        == Some(&TreeSelection::Ask {
                            project: proj.name.clone(),
                            unit: ask.unit.clone(),
                        });
                    tree_el = tree_el.child(render_ask_row(&proj.name, ask, selected));
                }
            }
        }
        if !any_match {
            tree_el = tree_el.child(
                div()
                    .layer("search empty")
                    .self_stretch()
                    .pad(16, 16)
                    .font_size(13)
                    .color(palette::MUTED)
                    .text(words::fill("tree.search_none", &[("q", q)])),
            );
        }
        return tree_el;
    }

    for proj in props.projects {
        let proj_selected =
            props.selection == Some(&TreeSelection::Project { project: proj.name.clone() });
        tree_el = tree_el.child(render_project_row(proj, proj_selected));
        if proj.folded {
            continue;
        }
        if let Some(needs) = &proj.setup {
            tree_el = tree_el.child(render_setup_checklist(needs));
        }

        let turbo =
            props.clients.iter().find(|c| c.name == proj.name).map_or(props.turbo, |c| c.turbo);
        tree_el =
            tree_el.child(render_section_header("section AGENTS", words::word("tree.agents")));
        for a in &proj.agents.rows {
            let selected = props.selection
                == Some(&TreeSelection::Agent {
                    project: proj.name.clone(),
                    session: a.session.clone(),
                });
            tree_el = tree_el.child(render_agent_row(&proj.name, a, selected, turbo));
        }
        if proj.agents.earlier > 0 {
            let earlier_unfolded = props.unfolded_earlier.contains(&proj.name);
            if !earlier_unfolded {
                let earlier_label = if proj.agents.earlier == 1 {
                    format!("{} ›", words::word("tree.earlier_one"))
                } else {
                    format!(
                        "{} ›",
                        words::fill("tree.earlier", &[("n", &proj.agents.earlier.to_string())])
                    )
                };
                tree_el = tree_el.child(
                    div()
                        .layer("row earlier sessions")
                        .on_click(format!("open_earlier:{}", proj.name))
                        .self_stretch()
                        .shrink_0()
                        .h(30)
                        .flex_row()
                        .style("padding", "0 16px 0 32px")
                        .items_center()
                        .border_bottom(palette::LINE)
                        .child(div().font_size(12).color(palette::BLUE_BORDER).text(earlier_label)),
                );
            } else if let Some(client) = props.clients.iter().find(|c| c.name == proj.name) {
                let (_, earlier_rows) = weft_core::sessions::split(&client.sessions);
                for r in earlier_rows {
                    let a = Agent {
                        pane: r.pane,
                        session: r.id.clone(),
                        harness: r.harness.clone(),
                        title: r.title.clone(),
                        state: r.state,
                        turn: None,
                        at: r.last,
                        asks: r.asks,
                        ask: None,
                    };
                    let selected = props.selection
                        == Some(&TreeSelection::Agent {
                            project: proj.name.clone(),
                            session: a.session.clone(),
                        });
                    tree_el = tree_el.child(render_agent_row(&proj.name, &a, selected, turbo));
                }
            }
        }

        tree_el = tree_el.child(render_section_header("section WORK", words::word("tree.work")));

        let open_selected =
            props.selection == Some(&TreeSelection::OpenGroup { project: proj.name.clone() });
        tree_el = tree_el.child(render_open_group_row(proj, open_selected));
        if !proj.work.open.folded {
            for ask in &proj.work.open.asks {
                let selected = props.selection
                    == Some(&TreeSelection::Ask {
                        project: proj.name.clone(),
                        unit: ask.unit.clone(),
                    });
                tree_el = tree_el.child(render_ask_row(&proj.name, ask, selected));
            }
        }

        for group in &proj.work.evals {
            let eval_selected = props.selection
                == Some(&TreeSelection::EvalGroup {
                    project: proj.name.clone(),
                    eval: group.eval.clone(),
                });
            tree_el = tree_el.child(render_eval_group_row(&proj.name, group, eval_selected));
            if !group.folded {
                for ask in &group.asks {
                    let selected = props.selection
                        == Some(&TreeSelection::Ask {
                            project: proj.name.clone(),
                            unit: ask.unit.clone(),
                        });
                    tree_el = tree_el.child(render_ask_row(&proj.name, ask, selected));
                }
            }
        }

        if proj.work.sealed.seals > 0 {
            let sealed_selected =
                props.selection == Some(&TreeSelection::SealedGroup { project: proj.name.clone() });
            tree_el = tree_el.child(render_sealed_group_row(proj, sealed_selected));
            if !proj.work.sealed.folded {
                for srow in &proj.work.sealed.rows {
                    let selected = props.selection
                        == Some(&TreeSelection::Seal {
                            project: proj.name.clone(),
                            seal: srow.seal.clone(),
                        });
                    tree_el = tree_el.child(render_seal_row(&proj.name, srow, selected));
                }
            }
        }
    }

    tree_el
}

/// Render the `D9.9` 1-click `+` new-agent popover for `project_name`.
pub fn render_plus_popover(project_name: &str, harnesses: &Harnesses, turbo: bool) -> Element {
    let mut pop = div()
        .layer("plus popover")
        .style("position", "absolute")
        .style("top", "84px")
        .style("left", "190px")
        .w(210)
        .flex_col()
        .bg(palette::SURFACE_ALT)
        .border(palette::BLUE_BORDER)
        .child(
            div()
                .pad(6, 12)
                .border_bottom(palette::LINE)
                .font_mono()
                .font_size(10)
                .font_weight(600)
                .style("letter-spacing", "0.08em")
                .color(palette::DIM)
                .text(format!("NEW AGENT IN {}", project_name.to_uppercase())),
        );

    for (idx, h) in harnesses.0.iter().enumerate() {
        let mut row = div()
            .layer(format!("plus harness {}", h.name))
            .on_click(format!("start_agent:{}:{}", project_name, h.name))
            .flex_row()
            .pad(8, 12)
            .items_center()
            .justify_between();
        if idx == 0 {
            row = row
                .style("position", "relative")
                .bg(palette::SELECTED_BG)
                .child(edge_bar("left", 2, palette::BLUE_BORDER))
                .child(
                    div()
                        .font_size(13)
                        .color(palette::BLUE_BORDER)
                        .font_weight(600)
                        .text(h.title.clone()),
                );
        } else {
            row = row.child(div().font_size(13).text(h.title.clone()));
        }
        if turbo {
            row = row.child(div().font_size(12).color(palette::AMBER).text("⚡"));
        }
        pop = pop.child(row);
    }

    pop
}

fn render_search_box(query: &str) -> Element {
    let active = !query.trim().is_empty();
    let mut box_el = div()
        .layer("search")
        .self_stretch()
        .h(38)
        .shrink_0()
        .flex_row()
        .gap(8)
        .items_center()
        .pad(0, 16)
        .bg(palette::SURFACE_ALT)
        .border_bottom(if active { palette::BLUE_BORDER } else { palette::LINE })
        .font_size(13)
        .color(if active { palette::TEXT } else { palette::DIM })
        .child(span("⌕").shrink_0().font_mono().color(palette::MUTED))
        .child(
            input_el()
                .id("tree-search")
                .flex_1()
                .style("min-width", "0")
                .style("white-space", "nowrap")
                .style("overflow", "hidden")
                .placeholder(words::word("tree.search_placeholder"))
                .text(query.to_string())
                .style("background", "transparent")
                .style("border", "none")
                .style("outline", "none")
                .font_size(13)
                .color(palette::TEXT),
        );

    if active {
        box_el = box_el.child(
            span("✕")
                .layer("clear search")
                .on_click("clear_search")
                .shrink_0()
                .color(palette::MUTED),
        );
    }
    box_el
}

/// A tree row: one line, `indent` from the left, a line below it, and the
/// selection's fill and bar when it is selected.
fn row(layer: impl Into<String>, height: u32, indent: u32, selected: bool) -> Element {
    let mut el = div()
        .layer(layer)
        .self_stretch()
        .shrink_0()
        .h(height)
        .flex_row()
        .gap(10)
        .items_center()
        .style("overflow", "hidden")
        .border_bottom(palette::LINE);
    el = el.style("padding", format!("0 16px 0 {indent}px"));
    if selected {
        el = el.style("position", "relative").bg(palette::SELECTED_BG).child(edge_bar(
            "left",
            3,
            palette::BLUE_BORDER,
        ));
    }
    el
}

/// One line of text that ends in an ellipsis when it does not fit.
fn line(t: impl Into<String>) -> Element {
    div()
        .style("white-space", "nowrap")
        .style("overflow", "hidden")
        .style("text-overflow", "ellipsis")
        .text(t)
}

fn caret(folded: bool) -> &'static str {
    if folded { "▸" } else { "▾" }
}

fn count(n: usize) -> Element {
    div().shrink_0().font_mono().font_size(11).color(palette::MUTED).text(n.to_string())
}

fn chevron() -> Element {
    div().shrink_0().font_weight(600).font_size(14).color(palette::DIM).text("›")
}

fn render_search_project_row(proj: &Project, matches: usize) -> Element {
    div()
        .layer(format!("row {}", proj.name))
        .self_stretch()
        .shrink_0()
        .h(46)
        .flex_row()
        .gap(10)
        .pad(0, 16)
        .items_center()
        .child(div().font_size(13).color(palette::MUTED).text("▾"))
        .child(div().flex_1().font_weight(600).font_size(15).text(proj.name.clone()))
        .child(
            div()
                .font_size(12)
                .color(palette::MUTED)
                .text(words::fill("tree.match", &[("n", &matches.to_string())])),
        )
}

fn render_project_row(proj: &Project, selected: bool) -> Element {
    let mut el = row(format!("row {}", proj.name), 40, 16, selected)
        .on_click(format!("select_project:{}", proj.name))
        .child(
            div()
                .on_click(format!("toggle_project:{}", proj.name))
                .font_size(12)
                .color(palette::MUTED)
                .text(caret(proj.folded)),
        )
        .child(div().flex_1().font_weight(600).font_size(14).text(proj.name.clone()));
    // Folded, a project still says what waits in it.
    if proj.folded && proj.needs > 0 {
        el = el.child(
            div().font_mono().font_size(11).color(palette::AMBER).text(format!("■ {}", proj.needs)),
        );
    }
    el = el.child(
        div()
            .layer(format!("plus {}", proj.name))
            .role("button")
            .aria_label(words::fill("web.start.in", &[("project", &proj.name)]))
            .on_click(format!("toggle_plus:{}", proj.name))
            .flex_row()
            .pad(2, 7)
            .bg(palette::SURFACE)
            .border(palette::LINE)
            .font_mono()
            .font_size(13)
            .color(palette::MUTED)
            .text("+"),
    );
    if selected {
        el = el.child(
            div()
                .layer(format!("close {}", proj.name))
                .role("button")
                .aria_label(words::word("act.close"))
                .on_click(format!("close_project:{}", proj.name))
                .font_size(13)
                .color(palette::DIM)
                .text("⌫"),
        );
    }
    el
}

fn render_setup_checklist(needs: &weft_core::workspace::Needs) -> Element {
    let mut el = div()
        .layer("setup checklist")
        .self_stretch()
        .flex_col()
        .gap(4)
        .pad(8, 32)
        .font_size(12)
        .color(palette::MUTED)
        .child(div().font_weight(600).color(palette::AMBER).text(words::word("tree.not_set_up")));
    for line in needs.lines() {
        let text = match line.how() {
            Some(how) => format!("{} {} ({how})", line.mark, line.say()),
            None => format!("{} {}", line.mark, line.say()),
        };
        el = el.child(div().text(text));
    }
    el
}

fn render_section_header(layer: &str, title: &str) -> Element {
    div()
        .layer(layer)
        .self_stretch()
        .shrink_0()
        .h(22)
        .flex_row()
        .items_center()
        .style("padding", "0 16px 0 32px")
        .bg(palette::SURFACE_ALT)
        .border_bottom(palette::LINE)
        .child(
            div()
                .font_mono()
                .font_weight(600)
                .font_size(10)
                .style("letter-spacing", "0.08em")
                .color(palette::DIM)
                .text(title),
        )
}

/// The status an agent's row leads with: a circle while it runs, a square
/// while it waits for you, a hollow square once it has ended.
pub fn status_shape(state: State, turn: Option<Turn>) -> (&'static str, &'static str) {
    match (state, turn) {
        (State::Running, Some(Turn::Waiting)) => ("■", palette::AMBER),
        (State::Running, _) => ("●", palette::TEAL),
        _ => ("□", palette::DIM),
    }
}

fn render_agent_row(project: &str, a: &Agent, selected: bool, turbo: bool) -> Element {
    let layer_name = match &a.title {
        Some(t) => format!("row agent {} {t}", a.harness),
        None => format!("row agent {}", a.harness),
    };
    let (shape, shape_color) = status_shape(a.state, a.turn);
    let running = a.state == State::Running;
    let sess_key = if a.session.is_empty() {
        match a.pane {
            Some(pid) => format!("pane:{pid}"),
            None => String::new(),
        }
    } else {
        a.session.clone()
    };

    let mut title = div()
        .flex_1()
        .style("min-width", "0")
        .style("overflow", "hidden")
        .flex_row()
        .gap(6)
        .items_baseline()
        .child(
            div()
                .shrink_0()
                .font_size(13)
                .font_weight(if selected { 600 } else { 500 })
                .style("white-space", "nowrap")
                .color(if running { palette::TEXT } else { palette::MUTED })
                .text(a.harness.clone()),
        );
    if turbo && running {
        title = title.child(div().shrink_0().font_size(11).color(palette::AMBER).text("⚡"));
    }
    if let Some(t) = &a.title {
        title = title.child(div().shrink_0().color(palette::DIM).text("·")).child(
            line(t.clone()).font_size(13).color(if running {
                palette::MUTED
            } else {
                palette::DIM
            }),
        );
    }

    let mut el = row(layer_name, 36, 32, selected)
        .on_click(format!("select_agent:{}:{}", project, sess_key))
        .child(div().shrink_0().w(10).font_mono().font_size(11).color(shape_color).text(shape))
        .child(title);

    if selected && !running && !a.session.is_empty() {
        el = el.child(
            div()
                .layer("btn Resume (row)")
                .on_click(format!("resume_session:{}:{}:{}", project, a.harness, a.session))
                .shrink_0()
                .pad(2, 8)
                .bg(palette::SURFACE)
                .border(palette::LINE)
                .font_weight(600)
                .font_size(12)
                .text(words::word("act.resume")),
        );
    }

    el.child(
        div()
            .shrink_0()
            .font_mono()
            .font_size(11)
            .color(if selected { palette::MUTED } else { palette::DIM })
            .text(format_clock(a.at)),
    )
}

fn render_open_group_row(proj: &Project, selected: bool) -> Element {
    row("row group Open", 36, 32, selected)
        .gap(8)
        .on_click(format!("select_open:{}", proj.name))
        .child(
            div()
                .on_click(format!("toggle_open:{}", proj.name))
                .shrink_0()
                .font_size(12)
                .color(palette::MUTED)
                .text(caret(proj.work.open.folded)),
        )
        .child(div().flex_1().font_weight(600).font_size(13).text(words::word("tree.open")))
        .child(count(proj.work.open.count))
}

fn render_ask_row(project: &str, ask: &Ask, selected: bool) -> Element {
    let meta = match ask.word {
        Some(w) => format!("{} · {}", w.say(), ask.harness),
        None => ask.harness.clone(),
    };
    row(format!("row ask {}", ask.title), 34, 48, selected)
        .on_click(format!("select_ask:{}:{}", project, ask.unit))
        .child(line(ask.title.clone()).flex_1().font_size(13).color(palette::TEXT))
        .child(div().shrink_0().font_mono().font_size(11).color(palette::MUTED).text(meta))
        .child(chevron())
}

fn render_eval_group_row(project: &str, g: &Group, selected: bool) -> Element {
    let title = words::fill("tree.eval", &[("n", &g.n.to_string())]);
    let mut label = div()
        .flex_1()
        .style("min-width", "0")
        .style("overflow", "hidden")
        .flex_row()
        .gap(6)
        .items_baseline()
        .child(div().shrink_0().font_weight(600).font_size(13).text(title.clone()));
    let verdict = if g.judging {
        Some((words::word("tree.judging").to_string(), palette::AMBER))
    } else {
        g.verdict.map(|v| {
            let (word, colour) = match v {
                "aligned" => (words::word("verdict.aligned"), palette::TEAL),
                "drifted" => (words::word("verdict.drifted"), palette::AMBER),
                _ => (words::word("verdict.incomplete"), palette::AMBER),
            };
            match &g.agreement {
                Some(agr) => (format!("{word} {agr}"), colour),
                None => (word.to_string(), colour),
            }
        })
    };
    if let Some((v, colour)) = verdict {
        label = label
            .child(div().shrink_0().font_size(12).color(palette::DIM).text("·"))
            .child(line(v).font_size(12).color(colour));
    }
    row(format!("row group {title}"), 36, 32, selected)
        .gap(8)
        .on_click(format!("select_eval:{}:{}", project, g.eval))
        .child(
            div()
                .on_click(format!("toggle_eval:{}:{}", project, g.eval))
                .shrink_0()
                .font_size(12)
                .color(palette::MUTED)
                .text(caret(g.folded)),
        )
        .child(label)
        .child(count(g.count))
}

fn render_sealed_group_row(proj: &Project, selected: bool) -> Element {
    row("row group Sealed", 36, 32, selected)
        .gap(8)
        .on_click(format!("toggle_sealed:{}", proj.name))
        .child(
            div()
                .shrink_0()
                .font_size(12)
                .color(palette::MUTED)
                .text(caret(proj.work.sealed.folded)),
        )
        .child(div().flex_1().font_weight(600).font_size(13).text(words::word("tree.sealed")))
        .child(count(proj.work.sealed.asks))
}

fn render_seal_row(project: &str, srow: &tree::SealRow, selected: bool) -> Element {
    let title = words::fill("tree.seal", &[("n", &srow.n.to_string())]);
    let meta = match srow.eval {
        Some(en) => format!("{} · Eval {en}", srow.decision),
        None => srow.decision.clone(),
    };
    row(format!("row seal {title}"), 34, 48, selected)
        .on_click(format!("select_seal:{}:{}", project, srow.seal))
        .child(line(title).flex_1().font_size(13).color(palette::TEXT))
        .child(div().shrink_0().font_mono().font_size(11).color(palette::MUTED).text(meta))
        .child(count(srow.asks))
        .child(chevron())
}

fn format_clock(ms: i64) -> String {
    if ms <= 0 {
        return "00:00".to_string();
    }
    if ms < 2400 {
        let h = (ms / 100).rem_euclid(24);
        let m = ms.rem_euclid(100).min(59);
        return format!("{h:02}:{m:02}");
    }
    let total_mins = (ms / 60_000).rem_euclid(24 * 60);
    let h = total_mins / 60;
    let m = total_mins % 60;
    format!("{h:02}:{m:02}")
}
