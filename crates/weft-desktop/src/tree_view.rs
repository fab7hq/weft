//! Collapsible `380px` `Tree` (`D2.6`, `D2.9`, `D9.9`, `D11.1`, `D11.8`, `D15.7`)
//! built over `weft_core::tree` and `weft_core::words`.

use weft_core::harness::Harnesses;
use weft_core::sessions::State;
use weft_core::tree::{self, Agent, Ask, Group, Project};
use weft_core::turns::Turn;
use weft_core::words;

use crate::gpui::{Element, div, input_el, palette, span};

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
        .w(380)
        .shrink_0()
        .self_stretch()
        .flex_col()
        .style("padding", "2px 0")
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
            tree_el = tree_el.child(render_search_project_row(proj, total));
            if !matching_agents.is_empty() {
                tree_el = tree_el.child(render_section_header(
                    "section AGENTS",
                    words::word("tree.agents"),
                    "",
                ));
                for a in matching_agents {
                    let selected = props.selection
                        == Some(&TreeSelection::Agent {
                            project: proj.name.clone(),
                            session: a.session.clone(),
                        });
                    tree_el = tree_el.child(render_agent_row(&proj.name, a, selected));
                }
            }
            if !matching_asks.is_empty() {
                tree_el = tree_el.child(render_section_header(
                    "section WORK",
                    words::word("tree.work"),
                    "",
                ));
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

        // AGENTS section
        let counts = words::fill(
            "tree.counts",
            &[("n", &proj.agents.running.to_string()), ("m", &proj.agents.ended.to_string())],
        );
        tree_el = tree_el.child(render_section_header(
            "section AGENTS",
            words::word("tree.agents"),
            &counts,
        ));
        for a in &proj.agents.rows {
            let selected = props.selection
                == Some(&TreeSelection::Agent {
                    project: proj.name.clone(),
                    session: a.session.clone(),
                });
            tree_el = tree_el.child(render_agent_row(&proj.name, a, selected));
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
                        .h(28)
                        .flex_row()
                        .gap(8)
                        .style("padding", "0 16px 0 52px")
                        .items_center()
                        .child(div().font_size(13).color(palette::BLUE_BORDER).text(earlier_label)),
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
                    tree_el = tree_el.child(render_agent_row(&proj.name, &a, selected));
                }
            }
        }

        // WORK section
        tree_el = tree_el.child(render_section_header(
            "section WORK",
            words::word("tree.work"),
            words::word("tree.by_round"),
        ));

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
        .style("top", "92px")
        .style("left", "190px")
        .w(210)
        .flex_col()
        .pad(6, 0)
        .bg(palette::SURFACE)
        .rounded(12)
        .border(palette::LINE)
        .style("box-shadow", "0 12px 32px rgba(0,0,0,0.65)")
        .child(
            div()
                .pad(5, 14)
                .font_size(11)
                .font_weight(600)
                .style("letter-spacing", "0.06em")
                .color(palette::DIM)
                .text(format!("NEW AGENT IN {}", project_name.to_uppercase())),
        );

    for (idx, h) in harnesses.0.iter().enumerate() {
        let mut row = div()
            .layer(format!("plus harness {}", h.name))
            .on_click(format!("start_agent:{}:{}", project_name, h.name))
            .flex_row()
            .pad(8, 14)
            .items_center()
            .justify_between();
        if idx == 0 {
            row = row.bg(palette::SELECTED_BG).child(
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
        .w(348)
        .h(36)
        .shrink_0()
        .flex_row()
        .gap(8)
        .items_center()
        .style("position", "sticky")
        .style("top", "0")
        .style("z-index", "2")
        .style("margin", "8px 16px 6px")
        .pad(0, 14)
        .bg(palette::SURFACE)
        .border(if active { palette::BLUE_BORDER } else { palette::LINE })
        .rounded(20)
        .font_size(13)
        .color(if active { palette::TEXT } else { palette::MUTED })
        .child(span("⌕").shrink_0().color(palette::MUTED))
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
    let caret = if proj.folded { "▸" } else { "▾" };
    let mut counts = div().font_size(12).flex_row().style("flex-wrap", "wrap");
    if proj.open > 0 && proj.needs > 0 {
        counts = counts
            .child(div().style("white-space", "pre-wrap").color(palette::MUTED).text(format!(
                "{} {} · ",
                proj.open,
                words::word("title.open")
            )))
            .child(
                div()
                    .style("white-space", "pre-wrap")
                    .color(palette::AMBER)
                    .text(format!("{} ●", proj.needs)),
            );
    } else if proj.open > 0 {
        counts = counts.child(div().color(palette::MUTED).text(format!(
            "{} {}",
            proj.open,
            words::word("title.open")
        )));
    } else if proj.needs > 0 {
        counts = counts.child(div().color(palette::AMBER).text(format!("{} ●", proj.needs)));
    }

    let mut row = div()
        .layer(format!("row {}", proj.name))
        .on_click(format!("select_project:{}", proj.name))
        .self_stretch()
        .shrink_0()
        .h(46)
        .flex_row()
        .gap(10)
        .pad(0, 16)
        .items_center();

    if selected {
        row = row
            .bg(palette::SELECTED_BG)
            .style("border-left", format!("3px solid {}", palette::BLUE_BORDER));
    }

    row.child(
        div()
            .on_click(format!("toggle_project:{}", proj.name))
            .font_size(13)
            .color(palette::MUTED)
            .text(caret),
    )
    .child(div().flex_1().font_weight(600).font_size(15).text(proj.name.clone()))
    .child(counts)
    .child(
        div()
            .layer(format!("plus {}", proj.name))
            .role("button")
            .aria_label(words::fill("web.start.in", &[("project", &proj.name)]))
            .on_click(format!("toggle_plus:{}", proj.name))
            .flex_row()
            .pad(2, 7)
            .bg(palette::SELECTED_BG)
            .rounded(6)
            .border(palette::BLUE_BORDER)
            .font_size(14)
            .color(palette::BLUE_BORDER)
            .font_weight(600)
            .text("+"),
    )
    .child(
        div()
            .layer(format!("close {}", proj.name))
            .role("button")
            .aria_label(words::word("act.close"))
            .on_click(format!("close_project:{}", proj.name))
            .font_size(13)
            .color(palette::DIM)
            .text("⌫"),
    )
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

fn render_section_header(layer: &str, title: &str, right: &str) -> Element {
    let mut el = div()
        .layer(layer)
        .self_stretch()
        .shrink_0()
        .h(20)
        .flex_row()
        .style("padding", "0 16px 0 32px")
        .style("align-items", "flex-end")
        .child(
            div()
                .flex_1()
                .font_weight(600)
                .font_size(11)
                .style("letter-spacing", "0.08em")
                .color(palette::DIM)
                .text(title),
        );
    if !right.is_empty() {
        el = el.child(div().font_size(12).color(palette::DIM).text(right));
    }
    el
}

fn render_agent_row(project: &str, a: &Agent, selected: bool) -> Element {
    let subtitle = a.title.as_deref().unwrap_or(words::word("tree.no_ask"));
    let layer_name = format!("row agent {} {}", a.harness, subtitle);
    let (dot, dot_color) = match (a.state, a.turn) {
        (State::Running, Some(Turn::Waiting)) => ("●", palette::AMBER),
        (State::Running, _) => ("●", palette::TEAL),
        _ => ("○", palette::DIM),
    };
    let clock = format_clock(a.at);
    let state_str = tree::state_word(a, &clock);
    let state_color = if a.turn == Some(Turn::Waiting) { palette::AMBER } else { palette::MUTED };

    let sess_key = if a.session.is_empty() {
        match a.pane {
            Some(pid) => format!("pane:{pid}"),
            None => String::new(),
        }
    } else {
        a.session.clone()
    };

    let mut row = div()
        .layer(layer_name)
        .on_click(format!("select_agent:{}:{}", project, sess_key))
        .self_stretch()
        .shrink_0()
        .h(44)
        .flex_row()
        .gap(8)
        .style("padding", "0 16px 0 32px")
        .style("overflow", "hidden")
        .items_center();

    if selected {
        row = row
            .bg(palette::SELECTED_BG)
            .style("border-left", format!("3px solid {}", palette::BLUE_BORDER));
    }

    row = row.child(div().shrink_0().font_size(12).color(dot_color).w(10).text(dot)).child(
        div()
            .flex_1()
            .style("min-width", "0")
            .style("overflow", "hidden")
            .flex_col()
            .gap(1)
            .child(
                div()
                    .flex_row()
                    .gap(6)
                    .items_baseline()
                    .style("min-width", "0")
                    .style("overflow", "hidden")
                    .child(
                        div()
                            .font_size(13)
                            .font_weight(500)
                            .style("white-space", "nowrap")
                            .color(if a.state == State::Running {
                                palette::TEXT
                            } else {
                                palette::MUTED
                            })
                            .text(a.harness.clone()),
                    )
                    .child(
                        div()
                            .font_size(12)
                            .color(state_color)
                            .style("white-space", "nowrap")
                            .style("overflow", "hidden")
                            .style("text-overflow", "ellipsis")
                            .text(state_str),
                    ),
            )
            .child(
                div()
                    .font_size(12)
                    .color(if a.title.is_some() { palette::MUTED } else { palette::DIM })
                    .style("white-space", "nowrap")
                    .style("overflow", "hidden")
                    .style("text-overflow", "ellipsis")
                    .text(subtitle),
            ),
    );

    if selected && a.state == State::Running && a.pane.is_some() {
        row = row.child(
            div()
                .layer("btn Ask (row)")
                .on_click(format!("open_ask:{}:{}", project, sess_key))
                .shrink_0()
                .flex_row()
                .gap(6)
                .pad(4, 10)
                .items_center()
                .justify_center()
                .bg(palette::SURFACE)
                .rounded(7)
                .border(palette::LINE)
                .child(div().font_weight(600).font_size(12).text(words::word("act.ask"))),
        );
    } else if selected && !a.session.is_empty() {
        row = row.child(
            div()
                .layer("btn Resume (row)")
                .on_click(format!("resume_session:{}:{}:{}", project, a.harness, a.session))
                .shrink_0()
                .flex_row()
                .gap(6)
                .pad(4, 10)
                .items_center()
                .justify_center()
                .bg(palette::SURFACE)
                .rounded(7)
                .border(palette::LINE)
                .child(div().font_weight(600).font_size(12).text(words::word("act.resume"))),
        );
    }

    row.child(div().shrink_0().font_size(12).color(palette::DIM).text(clock))
}

fn render_open_group_row(proj: &Project, selected: bool) -> Element {
    let caret = if proj.work.open.folded { "▸" } else { "▾" };
    let mut label = div()
        .flex_1()
        .style("min-width", "0")
        .flex_row()
        .gap(6)
        .items_baseline()
        .style("overflow", "hidden")
        .child(
            div()
                .font_weight(600)
                .font_size(13)
                .style("white-space", "nowrap")
                .text(words::word("tree.open")),
        );
    if let Some(since) = proj.work.open.since {
        label = label.child(
            div()
                .font_size(12)
                .color(palette::MUTED)
                .style("white-space", "nowrap")
                .style("overflow", "hidden")
                .style("text-overflow", "ellipsis")
                .text(format!("· {}", words::fill("tree.since", &[("n", &since.to_string())]))),
        );
    }

    let mut row = div()
        .layer("row group Open")
        .on_click(format!("select_open:{}", proj.name))
        .self_stretch()
        .shrink_0()
        .h(38)
        .flex_row()
        .gap(8)
        .style("padding", "0 16px 0 32px")
        .style("overflow", "hidden")
        .items_center();

    if selected {
        row = row
            .bg(palette::SELECTED_BG)
            .style("border-left", format!("3px solid {}", palette::BLUE_BORDER));
    }

    row = row
        .child(
            div()
                .on_click(format!("toggle_open:{}", proj.name))
                .shrink_0()
                .font_size(13)
                .color(palette::MUTED)
                .text(caret),
        )
        .child(label);

    if proj.work.open.count > 0 {
        row = row.child(
            div()
                .layer("btn Eval (row)")
                .on_click(format!("act_eval:{}", proj.name))
                .shrink_0()
                .flex_row()
                .gap(6)
                .pad(4, 10)
                .items_center()
                .justify_center()
                .bg(palette::SURFACE)
                .rounded(7)
                .border(palette::LINE)
                .child(div().font_weight(600).font_size(12).text(words::word("act.eval"))),
        );
    }

    row.child(
        div()
            .shrink_0()
            .font_size(12)
            .color(palette::MUTED)
            .style("min-width", "16px")
            .style("text-align", "right")
            .text(proj.work.open.count.to_string()),
    )
}

fn render_ask_row(project: &str, ask: &Ask, selected: bool) -> Element {
    let meta = match ask.word {
        Some(w) => format!("{} · {}", ask.harness, w.say()),
        None => ask.harness.clone(),
    };
    let mut row = div()
        .layer(format!("row ask {}", ask.title))
        .on_click(format!("select_ask:{}:{}", project, ask.unit))
        .self_stretch()
        .shrink_0()
        .h(44)
        .flex_row()
        .gap(8)
        .style("padding", "0 16px 0 48px")
        .style("overflow", "hidden")
        .items_center();

    if selected {
        row = row
            .bg(palette::SELECTED_BG)
            .style("border-left", format!("3px solid {}", palette::BLUE_BORDER));
    }

    row.child(
        div()
            .flex_1()
            .style("min-width", "0")
            .style("overflow", "hidden")
            .flex_col()
            .gap(1)
            .child(
                div()
                    .font_size(13)
                    .font_weight(500)
                    .color(palette::TEXT)
                    .style("white-space", "nowrap")
                    .style("overflow", "hidden")
                    .style("text-overflow", "ellipsis")
                    .text(ask.title.clone()),
            )
            .child(
                div()
                    .font_size(12)
                    .color(palette::MUTED)
                    .style("white-space", "nowrap")
                    .style("overflow", "hidden")
                    .style("text-overflow", "ellipsis")
                    .text(meta),
            ),
    )
    .child(div().shrink_0().font_weight(600).font_size(14).color(palette::DIM).text("›"))
}

fn render_eval_group_row(project: &str, g: &Group, selected: bool) -> Element {
    let caret = if g.folded { "▸" } else { "▾" };
    let title = words::fill("tree.eval", &[("n", &g.n.to_string())]);
    let mut summary = div()
        .font_size(12)
        .style("white-space", "nowrap")
        .style("min-width", "0")
        .style("overflow", "hidden")
        .flex_row();

    if g.judging {
        summary = summary.child(div().color(palette::AMBER).text(words::word("tree.judging")));
    } else if let Some(v) = g.verdict {
        let (v_word, v_color) = match v {
            "aligned" => (words::word("verdict.aligned"), palette::TEAL),
            "drifted" => (words::word("verdict.drifted"), palette::AMBER),
            _ => (words::word("verdict.incomplete"), palette::AMBER),
        };
        summary = summary.child(div().style("white-space", "pre").color(v_color).text(v_word));
        if let Some(agr) = &g.agreement {
            summary = summary.child(
                div()
                    .style("white-space", "pre")
                    .color(palette::MUTED)
                    .text(format!(" · {agr} · ")),
            );
        } else {
            summary =
                summary.child(div().style("white-space", "pre").color(palette::MUTED).text(" · "));
        }
        summary = summary.child(
            div()
                .style("white-space", "pre")
                .color(palette::AMBER)
                .text(words::word("tree.not_sealed")),
        );
    }

    let mut row = div()
        .layer(format!("row group {title}"))
        .on_click(format!("select_eval:{}:{}", project, g.eval))
        .self_stretch()
        .shrink_0()
        .h(38)
        .flex_row()
        .gap(8)
        .style("padding", "0 16px 0 32px")
        .style("overflow", "hidden")
        .items_center();

    if selected {
        row = row
            .bg(palette::SELECTED_BG)
            .style("border-left", format!("3px solid {}", palette::BLUE_BORDER));
    }

    row = row
        .child(
            div()
                .on_click(format!("toggle_eval:{}:{}", project, g.eval))
                .shrink_0()
                .font_size(13)
                .color(palette::MUTED)
                .text(caret),
        )
        .child(
            div()
                .flex_1()
                .style("min-width", "0")
                .flex_row()
                .gap(6)
                .items_baseline()
                .style("overflow", "hidden")
                .child(
                    div().font_weight(600).font_size(13).style("white-space", "nowrap").text(title),
                )
                .child(div().font_size(12).color(palette::DIM).text("·"))
                .child(summary),
        );

    if !g.judging {
        row = row.child(
            div()
                .layer("btn Seal (row)")
                .on_click(format!("open_seal:{}:{}", project, g.eval))
                .shrink_0()
                .flex_row()
                .gap(6)
                .pad(4, 10)
                .items_center()
                .justify_center()
                .bg(palette::SURFACE)
                .rounded(7)
                .border(palette::LINE)
                .child(div().font_weight(600).font_size(12).text(words::word("act.seal"))),
        );
    }

    row.child(
        div()
            .shrink_0()
            .font_size(12)
            .color(palette::MUTED)
            .style("min-width", "16px")
            .style("text-align", "right")
            .text(g.count.to_string()),
    )
}

fn render_sealed_group_row(proj: &Project, selected: bool) -> Element {
    let caret = if proj.work.sealed.folded { "▸" } else { "▾" };
    let seals_word = if proj.work.sealed.seals == 1 {
        words::word("tree.seals_one").to_string()
    } else {
        words::fill("tree.seals", &[("n", &proj.work.sealed.seals.to_string())])
    };
    let mut row = div()
        .layer("row group Sealed")
        .on_click(format!("toggle_sealed:{}", proj.name))
        .self_stretch()
        .shrink_0()
        .h(38)
        .flex_row()
        .gap(8)
        .style("padding", "0 16px 0 32px")
        .style("overflow", "hidden")
        .items_center();

    if selected {
        row = row
            .bg(palette::SELECTED_BG)
            .style("border-left", format!("3px solid {}", palette::BLUE_BORDER));
    }

    row.child(div().shrink_0().font_size(13).color(palette::MUTED).text(caret))
        .child(
            div()
                .flex_1()
                .style("min-width", "0")
                .flex_row()
                .gap(6)
                .items_baseline()
                .style("overflow", "hidden")
                .child(
                    div()
                        .font_weight(600)
                        .font_size(13)
                        .style("white-space", "nowrap")
                        .text(words::word("tree.sealed")),
                )
                .child(
                    div()
                        .font_size(12)
                        .color(palette::MUTED)
                        .style("white-space", "nowrap")
                        .style("overflow", "hidden")
                        .style("text-overflow", "ellipsis")
                        .text(format!("· {seals_word}")),
                ),
        )
        .child(
            div()
                .shrink_0()
                .font_size(12)
                .color(palette::MUTED)
                .style("min-width", "16px")
                .style("text-align", "right")
                .text(proj.work.sealed.asks.to_string()),
        )
}

fn render_seal_row(project: &str, srow: &tree::SealRow, selected: bool) -> Element {
    let title = words::fill("tree.seal", &[("n", &srow.n.to_string())]);
    let meta = match srow.eval {
        Some(en) => format!("{} · Eval {en}", srow.decision),
        None => srow.decision.clone(),
    };
    let mut row = div()
        .layer(format!("row seal {title}"))
        .on_click(format!("select_seal:{}:{}", project, srow.seal))
        .self_stretch()
        .shrink_0()
        .h(44)
        .flex_row()
        .gap(8)
        .style("padding", "0 16px 0 48px")
        .style("overflow", "hidden")
        .items_center();

    if selected {
        row = row
            .bg(palette::SELECTED_BG)
            .style("border-left", format!("3px solid {}", palette::BLUE_BORDER));
    }

    row.child(
        div()
            .flex_1()
            .style("min-width", "0")
            .style("overflow", "hidden")
            .flex_col()
            .gap(1)
            .child(
                div()
                    .font_size(13)
                    .font_weight(500)
                    .color(palette::TEXT)
                    .style("white-space", "nowrap")
                    .style("overflow", "hidden")
                    .style("text-overflow", "ellipsis")
                    .text(title),
            )
            .child(
                div()
                    .font_size(12)
                    .color(palette::MUTED)
                    .style("white-space", "nowrap")
                    .style("overflow", "hidden")
                    .style("text-overflow", "ellipsis")
                    .text(meta),
            ),
    )
    .child(div().shrink_0().font_size(12).color(palette::DIM).text(srow.asks.to_string()))
    .child(div().shrink_0().font_weight(600).font_size(14).color(palette::DIM).text("›"))
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
