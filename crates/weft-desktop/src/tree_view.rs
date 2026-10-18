//! The tree: one-line rows of projects, agents and their work, each agent
//! led by its status shape, built over `weft_core::tree` and `weft_core::words`.

use std::path::{Path, PathBuf};

use weft_core::harness::Harnesses;
use weft_core::sessions::State;
use weft_core::tree::{self, Agent, Project, WorkRow};
use weft_core::turns::{self, Turn};
use weft_core::words;
use weft_core::work::Status;

use crate::action::{Focus, TreeAct, WindowAct};
use crate::gpui::{Element, div, edge_bar, input_el, palette, span};

/// A project as the tree lists it, and the folder it is.
#[derive(Debug, Clone)]
pub struct Listed {
    pub root: PathBuf,
    pub project: Project,
    /// Where the project sends its acts, as `(act, harness)`; none when it
    /// sets none.
    pub routes: Vec<(&'static str, String)>,
    /// Every unit of work's row, folded away or not: what an act on one
    /// says when it cannot run.
    pub work: Vec<tree::WorkRow>,
}

impl Listed {
    /// A unit of work's row, by its root Ask, whatever the tree folds.
    pub fn work_row(&self, unit: &str) -> Option<&tree::WorkRow> {
        self.work.iter().find(|w| w.unit == unit)
    }
}

/// Which row in the `Tree` is currently selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeSelection {
    Project {
        project: PathBuf,
    },
    Agent {
        project: PathBuf,
        session: String,
    },
    EarlierSessions {
        project: PathBuf,
    },
    /// A unit of work, by its root Ask.
    Work {
        project: PathBuf,
        unit: String,
    },
    SealedEarlier {
        project: PathBuf,
    },
}

pub struct TreeProps<'a> {
    pub projects: &'a [Listed],
    pub selection: Option<&'a TreeSelection>,
    pub search_query: &'a str,
    /// The search field has the keyboard.
    pub searching: bool,
    pub plus_popover_project: Option<&'a Path>,
    pub harnesses: &'a Harnesses,
    pub turbo: bool,
    pub unfolded_earlier: &'a std::collections::HashSet<PathBuf>,
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
        .child(render_search_box(props.search_query, props.searching));

    let q = props.search_query.trim();
    if !q.is_empty() {
        let mut any_match = false;
        for Listed { root, project: proj, .. } in props.projects {
            let mut all_agents: Vec<Agent> = proj.agents.rows.clone();
            if let Some(client) = props.clients.iter().find(|c| c.root == *root) {
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
            let matching_asks: Vec<&WorkRow> =
                proj.work.units.iter().filter(|w| tree::work_matches(w, q)).collect();
            let total = matching_agents.len() + matching_asks.len();
            if total == 0 {
                continue;
            }
            any_match = true;
            let client = props.clients.iter().find(|c| c.root == *root);
            let turbo = client.map_or(props.turbo, |c| c.turbo);
            let zone = client.map_or(0, |c| c.zone);
            tree_el = tree_el.child(render_search_project_row(proj, total));
            if !matching_agents.is_empty() {
                tree_el = tree_el
                    .child(render_section_header("section AGENTS", words::word("tree.agents")));
                for a in matching_agents {
                    let selected = props.selection
                        == Some(&TreeSelection::Agent {
                            project: root.clone(),
                            session: a.session.clone(),
                        });
                    tree_el = tree_el.child(render_agent_row(root, a, selected, turbo, zone));
                }
            }
            if !matching_asks.is_empty() {
                tree_el =
                    tree_el.child(render_section_header("section WORK", words::word("tree.work")));
                for ask in matching_asks {
                    let selected = props.selection
                        == Some(&TreeSelection::Work {
                            project: root.clone(),
                            unit: ask.unit.clone(),
                        });
                    tree_el = tree_el.child(render_work_row(root, ask, selected));
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

    for Listed { root, project: proj, routes, .. } in props.projects {
        let proj_selected =
            props.selection == Some(&TreeSelection::Project { project: root.clone() });
        tree_el = tree_el.child(render_project_row(root, proj, proj_selected));
        // Selected, a project says where it sends its acts, in plain words.
        if proj_selected && !routes.is_empty() {
            let said: Vec<String> = routes
                .iter()
                .map(|(act, h)| words::fill(&format!("desktop.routes.{act}"), &[("harness", h)]))
                .collect();
            tree_el = tree_el.child(
                div()
                    .layer("routing line")
                    .self_stretch()
                    .style("padding", "6px 16px 6px 32px")
                    .border_bottom(palette::LINE)
                    .font_size(12)
                    .color(palette::MUTED)
                    .text(format!("{}.", said.join("; "))),
            );
        }
        if proj.folded {
            continue;
        }
        if let Some(needs) = &proj.setup {
            tree_el = tree_el.child(render_setup_checklist(needs));
        }

        let client = props.clients.iter().find(|c| c.root == *root);
        let turbo = client.map_or(props.turbo, |c| c.turbo);
        let zone = client.map_or(0, |c| c.zone);
        tree_el =
            tree_el.child(render_section_header("section AGENTS", words::word("tree.agents")));
        for a in &proj.agents.rows {
            let selected = props.selection
                == Some(&TreeSelection::Agent {
                    project: root.clone(),
                    session: a.session.clone(),
                });
            tree_el = tree_el.child(render_agent_row(root, a, selected, turbo, zone));
        }
        if proj.agents.earlier > 0 {
            let earlier_unfolded = props.unfolded_earlier.contains(root);
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
                        .on_click(TreeAct::OpenEarlier(root.clone()))
                        .self_stretch()
                        .shrink_0()
                        .h(30)
                        .flex_row()
                        .style("padding", "0 16px 0 32px")
                        .items_center()
                        .border_bottom(palette::LINE)
                        .child(div().font_size(12).color(palette::BLUE_BORDER).text(earlier_label)),
                );
            } else if let Some(client) = props.clients.iter().find(|c| c.root == *root) {
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
                            project: root.clone(),
                            session: a.session.clone(),
                        });
                    tree_el = tree_el.child(render_agent_row(root, &a, selected, turbo, zone));
                }
            }
        }

        tree_el = tree_el.child(render_section_header("section WORK", words::word("tree.work")));

        for w in &proj.work.units {
            let selected = props.selection
                == Some(&TreeSelection::Work { project: root.clone(), unit: w.unit.clone() });
            tree_el = tree_el.child(render_work_row(root, w, selected));
        }
        if proj.work.earlier > 0 {
            let selected =
                props.selection == Some(&TreeSelection::SealedEarlier { project: root.clone() });
            tree_el = tree_el.child(
                row("row sealed earlier", 36, 32, selected)
                    .on_click(TreeAct::ToggleSealed(root.to_path_buf()))
                    .child(div().font_size(12).color(palette::BLUE_BORDER).text(format!(
                        "{} ›",
                        words::fill(
                            "tree.sealed_earlier",
                            &[("n", &proj.work.earlier.to_string())]
                        )
                    ))),
            );
        }
    }

    tree_el
}

/// The `+` popover: a new agent of each harness in this project, in one click.
pub fn render_plus_popover(root: &Path, harnesses: &Harnesses, turbo: bool) -> Element {
    let project_name = crate::daemon::ProjectClient::name_of(root);
    let mut pop = div()
        .layer("plus popover")
        .style("position", "absolute")
        .style("top", "84px")
        .style("left", "190px")
        .w(210)
        .flex_col()
        .bg(palette::SURFACE_ALT)
        .border(palette::BLUE_BORDER)
        .panel_corners()
        .child(
            div()
                .pad(6, 12)
                .border_bottom(palette::LINE)
                .font_mono()
                .font_size(10)
                .font_weight(600)
                .color(palette::DIM)
                .text(
                    words::fill("desktop.plus.title", &[("project", &project_name)]).to_uppercase(),
                ),
        );

    for (idx, h) in harnesses.0.iter().enumerate() {
        let mut row = div()
            .layer(format!("plus harness {}", h.name))
            .on_click(TreeAct::StartAgent { project: root.to_path_buf(), harness: h.name.clone() })
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

fn render_search_box(query: &str, searching: bool) -> Element {
    let active = !query.trim().is_empty() || searching;
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
        .on_click(WindowAct::Focus(Focus::Search))
        .font_size(13)
        .color(if active { palette::TEXT } else { palette::DIM })
        .child(span("⌕").shrink_0().font_mono().color(palette::MUTED))
        .child(
            input_el(Focus::Search)
                .flex_1()
                .style("white-space", "nowrap")
                .style("overflow", "hidden")
                .placeholder(words::word("tree.search_placeholder"))
                .text(query.to_string())
                .font_size(13)
                .color(palette::TEXT),
        );

    if !query.trim().is_empty() {
        box_el = box_el.child(
            span("✕")
                .layer("clear search")
                .on_click(TreeAct::ClearSearch)
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

fn render_project_row(root: &Path, proj: &Project, selected: bool) -> Element {
    let mut el = row(format!("row {}", proj.name), 40, 16, selected)
        .on_click(TreeAct::SelectProject(root.to_path_buf()))
        .child(
            div()
                .on_click(TreeAct::ToggleProject(root.to_path_buf()))
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
            .on_click(TreeAct::TogglePlus(root.to_path_buf()))
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
                .on_click(TreeAct::CloseProject(root.to_path_buf()))
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
        .child(div().font_mono().font_weight(600).font_size(10).color(palette::DIM).text(title))
}

/// The status an agent's row leads with: a circle while it runs, a square
/// while it waits for you, a hollow square once it has ended.
pub fn status_shape(state: State, turn: Option<Turn>) -> (&'static str, &'static str) {
    match (state, turn) {
        (State::Running, turn) if turns::is_asking(turn) => ("■", palette::AMBER),
        (State::Running, _) => ("●", palette::TEAL),
        _ => ("□", palette::DIM),
    }
}

fn render_agent_row(project: &Path, a: &Agent, selected: bool, turbo: bool, zone: i64) -> Element {
    let layer_name = match &a.title {
        Some(t) => format!("row agent {} {t}", a.harness),
        None => format!("row agent {}", a.harness),
    };
    let (shape, shape_color) = status_shape(a.state, a.turn);
    let running = a.state == State::Running;

    let mut title =
        div().flex_1().style("overflow", "hidden").flex_row().gap(6).items_baseline().child(
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
        .on_click(TreeAct::SelectAgent {
            project: project.to_path_buf(),
            session: a.session.clone(),
            pane: a.pane,
        })
        .child({
            let mark =
                div().shrink_0().w(10).font_mono().font_size(11).color(shape_color).text(shape);
            // A waiting agent's ■ says why it waits.
            if running && turns::is_asking(a.turn) {
                mark.layer(format!("why waiting {}", a.session)).on_click(TreeAct::WhyWaiting {
                    project: project.to_path_buf(),
                    session: a.session.clone(),
                })
            } else {
                mark
            }
        })
        .child(title);

    if selected && !running && !a.session.is_empty() {
        el = el.child(
            div()
                .layer("btn Resume (row)")
                .on_click(crate::action::SheetAct::Resume {
                    project: project.to_path_buf(),
                    harness: a.harness.clone(),
                    session: a.session.clone(),
                })
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
            .text(clock(a.at, zone)),
    )
}

/// The colour a unit's status is said in.
pub fn status_colour(s: &Status) -> &'static str {
    match s {
        Status::Asking | Status::Asked => palette::MUTED,
        Status::Judging => palette::BLUE_BORDER,
        Status::Judged { verdict: weft_core::ledger::Verdict::Matches, .. } => palette::TEAL,
        Status::Judged { .. } => palette::AMBER,
        Status::Sealed { .. } => palette::DIM,
    }
}

/// A unit of work: its title, its harness, and its one status.
fn render_work_row(project: &Path, w: &WorkRow, selected: bool) -> Element {
    let colour = status_colour(&w.status);
    row(format!("row unit {}", w.title), 36, 32, selected)
        .on_click(TreeAct::SelectWork { project: project.to_path_buf(), unit: w.unit.clone() })
        .child(
            div()
                .flex_1()
                .style("min-width", "0")
                .flex_row()
                .gap(4)
                .items_baseline()
                .child(line(w.title.clone()).font_weight(500).font_size(13).color(palette::TEXT))
                .child(
                    div()
                        .shrink_0()
                        .font_size(13)
                        .color(palette::DIM)
                        .text(format!("· {}", w.harness)),
                ),
        )
        .child(
            div()
                .shrink_0()
                .font_mono()
                .font_size(11)
                .color(colour)
                .text(tree::status_word(&w.status)),
        )
}

/// A time of day, `HH:MM`, at the Mac's distance from UTC; nothing when no
/// time was given.
pub fn clock(ms: i64, zone: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let total_mins = ((ms + zone) / 60_000).rem_euclid(24 * 60);
    format!("{:02}:{:02}", total_mins / 60, total_mins % 60)
}
