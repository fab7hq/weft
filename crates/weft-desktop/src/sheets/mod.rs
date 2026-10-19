//! The sheets that take the workspace's place, one file per screen: a work's,
//! an Eval's, a Seal's and a session's Details, the Eval view, Updates and
//! Remote; the Ask docked under the panes; and the `☰` menu.

mod ask;
mod eval;
mod menu;
mod project;
mod remote;
mod ringframe;
mod seal;
pub mod send_to;
mod session;
mod settings;
mod unit;
mod updates;
mod work;

use std::path::Path;

use weft_core::harness::Harnesses;

use crate::app::{ActiveSheet, BottomBarState};
use crate::daemon::ProjectClient;
use crate::gpui::Element;
use crate::tree_view::Listed;

pub use menu::{MenuProps, render_menu_popover};
pub use updates::behind;

pub struct SheetProps<'a> {
    pub sheet: &'a ActiveSheet,
    pub projects: &'a [Listed],
    pub clients: &'a [ProjectClient],
    pub harnesses: &'a Harnesses,
    pub bottom_bar: &'a BottomBarState,
    /// What the open Ask's field holds.
    pub ask_draft: &'a str,
    /// The tunnel picked in Remote, not yet used.
    pub remote_pick: Option<&'a str>,
    /// The open Ask's SEND TO.
    pub send_to: Option<&'a send_to::SendTo>,
    /// The Details this one was opened from, by name.
    pub back: Option<&'a str>,
    /// The setting whose choices are open, by its dotted key, or `add`.
    pub choice: Option<&'a str>,
    /// The setting being typed, and its text.
    pub setting_editing: Option<&'a (Option<std::path::PathBuf>, Vec<String>)>,
    pub setting_text: &'a str,
    /// The RingFrame preview's `<host>|<route>|<task>`.
    pub preview: &'a str,
}

impl SheetProps<'_> {
    fn client(&self, root: &Path) -> Option<&ProjectClient> {
        self.clients.iter().find(|c| c.root == root)
    }

    fn listed(&self, root: &Path) -> Option<&Listed> {
        self.projects.iter().find(|p| p.root == root)
    }
}

/// The open sheet; a stage opened from other Details leads back to them.
pub fn render_sheet(props: &SheetProps) -> Element {
    let sheet = sheet_of(props);
    let leads_back = matches!(
        props.sheet,
        ActiveSheet::WorkDetail { .. }
            | ActiveSheet::EvalDetail { .. }
            | ActiveSheet::SealDetail { .. }
            | ActiveSheet::SessionDetail { .. }
    );
    match props.back.filter(|_| leads_back) {
        Some(name) => crate::gpui::div()
            .flex_1()
            .self_stretch()
            .flex_col()
            .child(
                crate::gpui::div()
                    .layer("sheet back")
                    .on_click(crate::action::SheetAct::Back)
                    .self_stretch()
                    .pad(10, 20)
                    .border_bottom(crate::gpui::palette::LINE)
                    .font_size(13)
                    .color(crate::gpui::palette::BLUE_BORDER)
                    .text(format!("‹ {name}")),
            )
            .child(sheet),
        None => sheet,
    }
}

fn sheet_of(props: &SheetProps) -> Element {
    match props.sheet {
        ActiveSheet::Ask { follow_up_unit, .. } => ask::render(props, follow_up_unit.as_deref()),
        ActiveSheet::Unit { project, unit } => unit::render(props, project, unit),
        ActiveSheet::WorkDetail { project, unit } => work::render(props, project, unit),
        ActiveSheet::EvalDetail { project, eval } => eval::render_detail(props, project, eval),
        ActiveSheet::EvalView { project, eval, by_file, at } => {
            eval::render_view(props, project, eval, *by_file, at)
        }
        ActiveSheet::SealDetail { project, seal } => seal::render(props, project, seal),
        ActiveSheet::SessionDetail { project, session } => session::render(props, project, session),
        ActiveSheet::Updates => updates::render(props),
        ActiveSheet::Remote => remote::render(props),
        ActiveSheet::Settings { tab } => settings::render(props, *tab),
        ActiveSheet::ProjectSettings { project } => project::render(props, project),
    }
}
