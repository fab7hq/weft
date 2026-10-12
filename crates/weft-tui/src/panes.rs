//! Panes, as an editor lays out its tabs: a tree of them, each with its own
//! tab bar. An agent is in exactly one pane, and a split moves it there; it
//! is never shown twice. Panes share the space equally.

use ratatui::layout::Rect;

/// An agent, by its project in this window and the daemon's pane id.
pub type Agent = (usize, u32);

/// The most panes a window shows.
pub const MOST: usize = 4;
/// The least a pane may be, its tab bar and frame included: columns, rows.
pub const LEAST: (u16, u16) = (40, 8);

/// Which way a split goes: a new pane to the right, or below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Right,
    Down,
}

/// A pane's edge, where a dragged tab splits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// Where a dragged tab lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drop {
    /// On a pane's tab bar, before the tab at that place.
    Bar { pane: u32, at: usize },
    /// On a pane's outer fifth: that pane split that way, the tab in the new
    /// pane.
    Edge { pane: u32, side: Side },
    /// In a pane's middle: into that pane, as its last tab.
    Middle { pane: u32 },
}

/// Why a split did not happen. Nothing moves when one is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The pane holds one agent: splitting it would leave it empty.
    NeedsTwo,
    /// There are four panes already.
    Four,
    /// A pane would be smaller than the least.
    NoRoom,
}

impl Refusal {
    /// The word that says why.
    pub fn word(self) -> &'static str {
        match self {
            Refusal::NeedsTwo => "why.split_needs_two",
            Refusal::Four => "why.four_panes",
            Refusal::NoRoom => "why.no_room",
        }
    }
}

/// One pane: its tabs in the person's order, and the one it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: u32,
    pub tabs: Vec<Agent>,
    pub shown: usize,
}

impl Pane {
    /// The agent this pane shows.
    pub fn showing(&self) -> Option<Agent> {
        self.tabs.get(self.shown).copied()
    }

    fn take(&mut self, agent: Agent) -> bool {
        let Some(i) = self.tabs.iter().position(|&a| a == agent) else { return false };
        self.tabs.remove(i);
        if i < self.shown || self.shown >= self.tabs.len() {
            self.shown = self.shown.saturating_sub(1);
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Pane(Pane),
    Split(Axis, Vec<Node>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panes {
    root: Node,
    /// The pane with the keys.
    active: u32,
    next: u32,
}

impl Default for Panes {
    fn default() -> Self {
        Panes { root: Node::Pane(Pane { id: 0, tabs: Vec::new(), shown: 0 }), active: 0, next: 1 }
    }
}

impl Panes {
    /// Every pane, left to right, top to bottom.
    pub fn panes(&self) -> Vec<&Pane> {
        let mut out = Vec::new();
        leaves(&self.root, &mut out);
        let at = self.rects(Rect::new(0, 0, 1200, 1200));
        out.sort_by_key(|p| at.iter().find(|(id, _)| *id == p.id).map(|(_, r)| (r.y, r.x)));
        out
    }

    pub fn count(&self) -> usize {
        self.panes().len()
    }

    /// The pane with the keys.
    pub fn active(&self) -> &Pane {
        self.pane(self.active).expect("the pane with the keys is in the tree")
    }

    pub fn pane(&self, id: u32) -> Option<&Pane> {
        self.panes().into_iter().find(|p| p.id == id)
    }

    /// Every agent, pane by pane, in each pane's order: what the numbers
    /// count.
    pub fn order(&self) -> Vec<Agent> {
        self.panes().iter().flat_map(|p| p.tabs.iter().copied()).collect()
    }

    /// The pane holding an agent.
    pub fn pane_of(&self, agent: Agent) -> Option<u32> {
        self.panes().iter().find(|p| p.tabs.contains(&agent)).map(|p| p.id)
    }

    /// Where each pane is drawn in an area, left to right, top to bottom.
    /// Side by side, a column is left between panes; one below another, none.
    pub fn rects(&self, area: Rect) -> Vec<(u32, Rect)> {
        let mut out = Vec::new();
        place(&self.root, area, &mut out);
        out.sort_by_key(|(_, r)| (r.y, r.x));
        out
    }

    /// Every pane is at least the least size in this area.
    pub fn fits(&self, area: Rect) -> bool {
        self.rects(area).iter().all(|(_, r)| r.width >= LEAST.0 && r.height >= LEAST.1)
    }

    /// Keep the tabs to the agents running: one that ended goes, a pane left
    /// empty closes, and a new one opens as the last tab of the pane with the
    /// keys.
    pub fn sync(&mut self, running: &[Agent]) {
        for agent in self.order() {
            if !running.contains(&agent) {
                self.remove(agent);
            }
        }
        let known = self.order();
        let active = self.active;
        for &agent in running.iter().filter(|a| !known.contains(a)) {
            self.with_pane(active, |p| p.tabs.push(agent));
        }
    }

    /// Show an agent in the pane that holds it, and give that pane the keys.
    pub fn show(&mut self, agent: Agent) -> bool {
        let Some(id) = self.pane_of(agent) else { return false };
        self.with_pane(id, |p| p.shown = p.tabs.iter().position(|&a| a == agent).unwrap_or(0));
        self.active = id;
        true
    }

    /// Give a pane the keys.
    pub fn focus(&mut self, pane: u32) {
        if self.pane(pane).is_some() {
            self.active = pane;
        }
    }

    /// The tab with the keys into a new pane that way, which gets the keys.
    pub fn split(&mut self, axis: Axis, area: Rect) -> Result<(), Refusal> {
        let pane = self.active;
        let side = if axis == Axis::Right { Side::Right } else { Side::Down };
        let agent = self.active().showing().ok_or(Refusal::NeedsTwo)?;
        self.drop_tab(agent, Drop::Edge { pane, side }, area)
    }

    /// The tab with the keys one place along its bar.
    pub fn move_tab(&mut self, by: isize) {
        let active = self.active;
        self.with_pane(active, |p| {
            let to = p.shown as isize + by;
            if (0..p.tabs.len() as isize).contains(&to) {
                p.tabs.swap(p.shown, to as usize);
                p.shown = to as usize;
            }
        });
    }

    /// The tab with the keys into the next pane, as its last tab. False with
    /// one pane, where there is no next.
    pub fn to_next_pane(&mut self) -> bool {
        let panes: Vec<u32> = self.panes().iter().map(|p| p.id).collect();
        let Some(agent) = self.active().showing() else { return false };
        if panes.len() < 2 {
            return false;
        }
        let at = panes.iter().position(|&id| id == self.active).unwrap_or(0);
        let next = panes[(at + 1) % panes.len()];
        let to = self.pane(next).map_or(0, |p| p.tabs.len());
        self.drop_tab(agent, Drop::Bar { pane: next, at: to }, Rect::default()).is_ok()
    }

    /// One pane again, every tab in it in the order they were numbered.
    pub fn join(&mut self) {
        let shown = self.active().showing();
        let tabs = self.order();
        let id = self.fresh();
        let at = shown.and_then(|s| tabs.iter().position(|&a| a == s)).unwrap_or(0);
        self.root = Node::Pane(Pane { id, tabs, shown: at });
        self.active = id;
    }

    /// A dragged tab, dropped. A split this way is refused as `split` is,
    /// and then nothing moves.
    pub fn drop_tab(&mut self, agent: Agent, to: Drop, area: Rect) -> Result<(), Refusal> {
        let Some(from) = self.pane_of(agent) else { return Ok(()) };
        let mut next = self.clone();
        match to {
            Drop::Bar { pane, mut at } => {
                if next.pane(pane).is_none() {
                    return Ok(());
                }
                if pane == from {
                    let was = next.pane(pane).and_then(|p| p.tabs.iter().position(|&a| a == agent));
                    if was.is_some_and(|w| w < at) {
                        at -= 1;
                    }
                }
                next.remove(agent);
                next.with_pane(pane, |p| {
                    let at = at.min(p.tabs.len());
                    p.tabs.insert(at, agent);
                    p.shown = at;
                });
                next.active = pane;
            }
            Drop::Middle { pane } => {
                if pane == from || next.pane(pane).is_none() {
                    return Ok(());
                }
                next.remove(agent);
                next.with_pane(pane, |p| {
                    p.tabs.push(agent);
                    p.shown = p.tabs.len() - 1;
                });
                next.active = pane;
            }
            Drop::Edge { pane, side } => {
                if next.pane(pane).is_none() {
                    return Ok(());
                }
                if pane == from && next.pane(pane).is_some_and(|p| p.tabs.len() < 2) {
                    return Err(Refusal::NeedsTwo);
                }
                next.remove(agent);
                if next.count() >= MOST {
                    return Err(Refusal::Four);
                }
                let id = next.fresh();
                let new = Node::Pane(Pane { id, tabs: vec![agent], shown: 0 });
                let (axis, after) = match side {
                    Side::Left => (Axis::Right, false),
                    Side::Right => (Axis::Right, true),
                    Side::Up => (Axis::Down, false),
                    Side::Down => (Axis::Down, true),
                };
                beside(&mut next.root, pane, axis, after, new);
                next.active = id;
                if !next.fits(area) {
                    return Err(Refusal::NoRoom);
                }
            }
        }
        *self = next;
        Ok(())
    }

    /// A project leaves the window: its agents go, and the projects after it
    /// move up one place.
    pub fn forget_project(&mut self, at: usize) {
        for agent in self.order() {
            if agent.0 == at {
                self.remove(agent);
            }
        }
        each_pane(&mut self.root, &mut |p| {
            for a in &mut p.tabs {
                if a.0 > at {
                    a.0 -= 1;
                }
            }
        });
    }

    /// Take an agent out of its pane; a pane left empty closes, unless it is
    /// the last.
    fn remove(&mut self, agent: Agent) {
        let Some(id) = self.pane_of(agent) else { return };
        self.with_pane(id, |p| {
            p.take(agent);
        });
        if self.pane(id).is_some_and(|p| p.tabs.is_empty()) && self.count() > 1 {
            close(&mut self.root, id);
            if self.active == id {
                self.active = self.panes()[0].id;
            }
        }
    }

    fn with_pane(&mut self, id: u32, f: impl FnOnce(&mut Pane)) {
        let mut f = Some(f);
        each_pane(&mut self.root, &mut |p| {
            if p.id == id
                && let Some(f) = f.take()
            {
                f(p)
            }
        });
    }

    fn fresh(&mut self) -> u32 {
        self.next += 1;
        self.next - 1
    }
}

fn leaves<'a>(node: &'a Node, out: &mut Vec<&'a Pane>) {
    match node {
        Node::Pane(p) => out.push(p),
        Node::Split(_, kids) => kids.iter().for_each(|k| leaves(k, out)),
    }
}

fn each_pane(node: &mut Node, f: &mut impl FnMut(&mut Pane)) {
    match node {
        Node::Pane(p) => f(p),
        Node::Split(_, kids) => kids.iter_mut().for_each(|k| each_pane(k, f)),
    }
}

/// Equal shares: side by side, the width less a column between each two;
/// one below another, the height. What does not divide goes to the first.
fn place(node: &Node, area: Rect, out: &mut Vec<(u32, Rect)>) {
    let kids = match node {
        Node::Pane(p) => return out.push((p.id, area)),
        Node::Split(_, kids) => kids,
    };
    let n = kids.len() as u16;
    let across = matches!(node, Node::Split(Axis::Right, _));
    let (whole, gap) = if across { (area.width, 1) } else { (area.height, 0) };
    let room = whole.saturating_sub(gap * (n - 1));
    let mut at = if across { area.x } else { area.y };
    for (i, kid) in kids.iter().enumerate() {
        let size = room / n + u16::from((i as u16) < room % n);
        let r = if across {
            Rect { x: at, width: size, ..area }
        } else {
            Rect { y: at, height: size, ..area }
        };
        place(kid, r, out);
        at += size + gap;
    }
}

/// Put a new pane beside another, that way: among its siblings when they
/// already go that way, else in a split of its own.
fn beside(node: &mut Node, target: u32, axis: Axis, after: bool, new: Node) -> Option<Node> {
    match node {
        Node::Pane(p) if p.id == target => {
            let old = std::mem::replace(node, Node::Split(axis, Vec::new()));
            let Node::Split(_, kids) = node else { unreachable!() };
            *kids = if after { vec![old, new] } else { vec![new, old] };
            None
        }
        Node::Pane(_) => Some(new),
        Node::Split(way, kids) => {
            let here = kids.iter().position(|k| matches!(k, Node::Pane(p) if p.id == target));
            if let (Some(i), true) = (here, *way == axis) {
                kids.insert(if after { i + 1 } else { i }, new);
                return None;
            }
            let mut new = Some(new);
            for kid in kids.iter_mut() {
                new = beside(kid, target, axis, after, new.take()?);
            }
            new
        }
    }
}

/// Take a pane out of the tree; a split left with one pane becomes it, and a
/// split left inside one going its own way joins it.
fn close(node: &mut Node, id: u32) {
    let Node::Split(axis, kids) = node else { return };
    kids.retain(|k| !matches!(k, Node::Pane(p) if p.id == id));
    for kid in kids.iter_mut() {
        close(kid, id);
    }
    let axis = *axis;
    let mut flat = Vec::new();
    for kid in std::mem::take(kids) {
        match kid {
            Node::Split(a, inner) if a == axis => flat.extend(inner),
            other => flat.push(other),
        }
    }
    *kids = flat;
    if kids.len() == 1 {
        *node = kids.pop().expect("one");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen's body on the cards: 120 columns by 28 rows.
    const BODY: Rect = Rect { x: 0, y: 1, width: 120, height: 28 };

    fn five() -> Panes {
        let mut p = Panes::default();
        p.sync(&[(0, 1), (0, 2), (1, 1), (2, 1), (2, 2)]);
        p
    }

    fn ids(p: &Panes) -> Vec<Vec<Agent>> {
        p.panes().iter().map(|p| p.tabs.clone()).collect()
    }

    #[test]
    fn one_pane_holds_every_agent_in_the_order_they_came() {
        let p = five();
        assert_eq!(p.count(), 1);
        assert_eq!(p.order(), vec![(0, 1), (0, 2), (1, 1), (2, 1), (2, 2)]);
        assert_eq!(p.active().showing(), Some((0, 1)));
    }

    #[test]
    fn a_split_moves_the_tab_with_the_keys_to_a_new_pane_that_gets_them() {
        let mut p = five();
        p.show((1, 1));
        p.split(Axis::Right, BODY).expect("split");
        assert_eq!(ids(&p), vec![vec![(0, 1), (0, 2), (2, 1), (2, 2)], vec![(1, 1)]]);
        assert_eq!(p.active().showing(), Some((1, 1)));
        assert_eq!(p.order()[4], (1, 1), "numbers count pane by pane");
        // The pane it left shows its neighbour.
        assert_eq!(p.panes()[0].showing(), Some((2, 1)));
    }

    #[test]
    fn a_pane_with_one_agent_does_not_split() {
        let mut p = Panes::default();
        p.sync(&[(0, 1)]);
        assert_eq!(p.split(Axis::Right, BODY), Err(Refusal::NeedsTwo));
        assert_eq!(p.count(), 1);
    }

    #[test]
    fn a_fifth_pane_is_refused_and_nothing_moves() {
        let mut p = four();
        assert_eq!(p.count(), 4);
        p.show((2, 1));
        let before = p.clone();
        assert_eq!(p.split(Axis::Right, Rect::new(0, 0, 400, 100)), Err(Refusal::Four));
        assert_eq!(p, before);
    }

    #[test]
    fn a_split_that_leaves_a_pane_under_forty_by_eight_is_refused() {
        let mut p = five();
        p.split(Axis::Right, BODY).expect("two of 60 and 59");
        p.show((0, 2));
        assert_eq!(p.split(Axis::Right, BODY), Err(Refusal::NoRoom), "three of 39");
        let mut p = five();
        assert_eq!(p.split(Axis::Down, Rect::new(0, 0, 120, 15)), Err(Refusal::NoRoom));
        assert_eq!(p.count(), 1);
    }

    /// As the four-pane card draws it: gl · codex, openbox · claude above;
    /// gl · claude, and docs' two below.
    fn four() -> Panes {
        let mut p = five();
        p.show((1, 1));
        p.split(Axis::Right, BODY).expect("right");
        p.show((0, 2));
        p.split(Axis::Down, BODY).expect("left down");
        let right = p.pane_of((1, 1)).expect("right");
        p.drop_tab((2, 1), Drop::Edge { pane: right, side: Side::Down }, BODY).expect("right down");
        let below = p.pane_of((2, 1)).expect("below");
        p.drop_tab((2, 2), Drop::Middle { pane: below }, BODY).expect("into it");
        p
    }

    #[test]
    fn panes_share_the_space_equally() {
        let mut p = five();
        p.split(Axis::Right, BODY).expect("right");
        let r: Vec<Rect> = p.rects(BODY).into_iter().map(|(_, r)| r).collect();
        assert_eq!(r, vec![Rect::new(0, 1, 60, 28), Rect::new(61, 1, 59, 28)]);
        let mut p = five();
        p.split(Axis::Down, BODY).expect("down");
        let r: Vec<Rect> = p.rects(BODY).into_iter().map(|(_, r)| r).collect();
        assert_eq!(r, vec![Rect::new(0, 1, 120, 14), Rect::new(0, 15, 120, 14)]);
        // A split halves its pane; panes one way share their row equally.
        let wide = Rect::new(0, 0, 200, 40);
        let mut p = five();
        p.split(Axis::Right, wide).expect("right");
        p.show((0, 2));
        p.split(Axis::Right, wide).expect("right again");
        let w: Vec<u16> = p.rects(wide).iter().map(|(_, r)| r.width).collect();
        assert_eq!(w, vec![66, 66, 66]);
    }

    #[test]
    fn four_panes_are_a_quarter_each_and_numbered_left_to_right_top_to_bottom() {
        let p = four();
        let r: Vec<Rect> = p.rects(BODY).into_iter().map(|(_, r)| r).collect();
        assert_eq!(
            r,
            vec![
                Rect::new(0, 1, 60, 14),
                Rect::new(61, 1, 59, 14),
                Rect::new(0, 15, 60, 14),
                Rect::new(61, 15, 59, 14)
            ]
        );
        assert_eq!(p.order(), vec![(0, 1), (1, 1), (0, 2), (2, 1), (2, 2)]);
    }

    #[test]
    fn brackets_move_the_tab_with_the_keys_along_its_bar() {
        let mut p = five();
        p.show((0, 2));
        p.move_tab(-1);
        assert_eq!(p.order()[..2], [(0, 2), (0, 1)]);
        assert_eq!(p.active().showing(), Some((0, 2)));
        p.move_tab(-1);
        assert_eq!(p.order()[0], (0, 2), "the first stays first");
        p.move_tab(1);
        p.move_tab(1);
        assert_eq!(p.order()[2], (0, 2));
    }

    #[test]
    fn tab_moves_the_tab_to_the_next_pane_and_an_empty_pane_closes() {
        let mut p = five();
        assert!(!p.to_next_pane(), "one pane has no next");
        p.show((2, 2));
        p.split(Axis::Right, BODY).expect("right");
        p.show((0, 1));
        assert!(p.to_next_pane());
        assert_eq!(ids(&p), vec![vec![(0, 2), (1, 1), (2, 1)], vec![(2, 2), (0, 1)]]);
        assert_eq!(p.active().showing(), Some((0, 1)));
        // From the last pane it wraps to the first; a pane left empty closes.
        p.show((2, 2));
        assert!(p.to_next_pane());
        p.show((0, 1));
        assert!(p.to_next_pane());
        assert_eq!(p.count(), 1);
        assert_eq!(p.order(), vec![(0, 2), (1, 1), (2, 1), (2, 2), (0, 1)]);
    }

    #[test]
    fn join_puts_every_tab_in_one_pane_in_their_numbered_order() {
        let mut p = four();
        p.show((0, 2));
        let order = p.order();
        p.join();
        assert_eq!(p.count(), 1);
        assert_eq!(p.order(), order);
        assert_eq!(p.active().showing(), Some((0, 2)));
    }

    #[test]
    fn an_ended_agent_goes_and_a_new_one_opens_in_the_pane_with_the_keys() {
        let mut p = five();
        p.show((2, 2));
        p.split(Axis::Right, BODY).expect("right");
        p.show((0, 1));
        p.sync(&[(0, 1), (0, 2), (1, 1), (2, 1), (3, 1)]);
        assert_eq!(p.count(), 1, "the pane whose last tab left closed");
        assert_eq!(p.order().last(), Some(&(3, 1)));
        p.sync(&[]);
        assert_eq!(p.count(), 1, "the last pane stays");
        assert!(p.order().is_empty());
    }

    #[test]
    fn a_dragged_tab_reorders_moves_splits_or_joins() {
        let mut p = five();
        // Its place on its own bar: moved there.
        p.drop_tab((2, 2), Drop::Bar { pane: p.active().id, at: 0 }, BODY).expect("reorder");
        assert_eq!(p.order()[0], (2, 2));
        p.drop_tab((2, 2), Drop::Bar { pane: p.active().id, at: 2 }, BODY).expect("reorder");
        assert_eq!(p.order()[..3], [(0, 1), (2, 2), (0, 2)]);
        // A pane's lower fifth: split below, the tab in the new pane.
        let first = p.active().id;
        p.drop_tab((0, 1), Drop::Edge { pane: first, side: Side::Down }, BODY).expect("edge");
        assert_eq!(ids(&p), vec![vec![(2, 2), (0, 2), (1, 1), (2, 1)], vec![(0, 1)]]);
        // Its left edge: a new pane before it.
        let below = p.active().id;
        p.drop_tab((1, 1), Drop::Edge { pane: below, side: Side::Left }, BODY).expect("left");
        assert_eq!(ids(&p)[1..], [vec![(1, 1)], vec![(0, 1)]]);
        // Another pane's middle: into it; the pane it left, empty, closes.
        p.drop_tab((1, 1), Drop::Middle { pane: first }, BODY).expect("middle");
        assert_eq!(ids(&p), vec![vec![(2, 2), (0, 2), (2, 1), (1, 1)], vec![(0, 1)]]);
        // Onto another bar, at a place.
        p.drop_tab((0, 1), Drop::Bar { pane: first, at: 1 }, BODY).expect("bar");
        assert_eq!(p.count(), 1);
        assert_eq!(p.order()[..2], [(2, 2), (0, 1)]);
        // A lone tab onto its own pane's edge: nothing to split.
        let mut one = Panes::default();
        one.sync(&[(0, 1)]);
        let id = one.active().id;
        assert_eq!(
            one.drop_tab((0, 1), Drop::Edge { pane: id, side: Side::Right }, BODY),
            Err(Refusal::NeedsTwo)
        );
    }

    #[test]
    fn a_lone_tab_dragged_to_another_panes_edge_keeps_the_count() {
        let mut p = four();
        let lone = p.panes()[1].clone();
        assert_eq!(lone.tabs.len(), 1);
        let target = p.panes()[0].id;
        let wide = Rect::new(0, 0, 300, 80);
        p.drop_tab(lone.tabs[0], Drop::Edge { pane: target, side: Side::Up }, wide)
            .expect("its pane closes, so this is still four");
        assert_eq!(p.count(), 4);
    }

    #[test]
    fn a_layout_fits_or_not() {
        let mut p = five();
        p.split(Axis::Right, BODY).expect("right");
        assert!(p.fits(BODY));
        assert!(!p.fits(Rect::new(0, 0, 80, 28)));
    }

    #[test]
    fn a_project_closed_takes_its_agents_and_the_rest_move_up() {
        let mut p = five();
        p.forget_project(1);
        assert_eq!(p.order(), vec![(0, 1), (0, 2), (1, 1), (1, 2)]);
    }
}
