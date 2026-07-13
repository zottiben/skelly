//! `skelly-pane` - the pane tree: a tab's tiling layout of terminal panes.
//!
//! This is pure geometry + state logic with no UI, GPU, or terminal dependency -
//! a leaf crate the `skelly` binary drives, mirroring `skelly-config`. The binary
//! owns the *wiring* (mapping a [`PaneId`] to a live terminal, rendering each pane at
//! its computed rectangle, and binding keys to these operations); this crate owns the
//! *model*: how splits nest, how focus moves, and how the viewport tiles.
//!
//! A tab holds at most [`MAX_PANES`] panes (AGENTS Hard rule 4). Panes tile by
//! nesting binary splits, so splits need not be even; each split carries a `ratio` a
//! divider drag adjusts. Exactly one pane is always focused, and the focused pane can
//! be zoomed to fill the viewport.

#![doc(test(attr(deny(warnings))))]

/// The most panes a single tab may hold (AGENTS Hard rule 4).
pub const MAX_PANES: usize = 8;

/// The smallest fraction a split's first child may shrink to, so resizing can never
/// collapse a pane to nothing.
const MIN_RATIO: f32 = 0.05;

/// A stable pane identifier. Allocated monotonically and never reused within a tree's
/// lifetime, so an id the binary maps to a live terminal stays valid until that pane
/// is closed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PaneId(u32);

/// A direction - the argument to [`PaneTree::split`], [`PaneTree::focus`], and
/// [`PaneTree::resize`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    /// Left / west.
    Left,
    /// Right / east.
    Right,
    /// Up / north.
    Up,
    /// Down / south.
    Down,
}

impl Dir {
    /// The split axis this direction implies.
    fn axis(self) -> Axis {
        match self {
            Dir::Left | Dir::Right => Axis::Row,
            Dir::Up | Dir::Down => Axis::Col,
        }
    }

    /// Whether a split in this direction places the new pane after (to the right of /
    /// below) the pane being split.
    fn new_pane_is_second(self) -> bool {
        matches!(self, Dir::Right | Dir::Down)
    }
}

/// How a split arranges its two children.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Axis {
    /// Side by side, left then right (a vertical divider) - a `Left`/`Right` split.
    Row,
    /// Stacked, top then bottom (a horizontal divider) - an `Up`/`Down` split.
    Col,
}

/// A rectangle in the caller's coordinate space (e.g. physical pixels). Layout is
/// resolution-independent: pass whatever viewport you render into and read back each
/// pane's rectangle within it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle at `(x, y)` sized `w` x `h`.
    #[must_use]
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Split into `(first, second)` along `axis`, the first taking `ratio` of the
    /// extent. The second takes the exact remainder, so the two always tile `self`
    /// with no gap or overlap regardless of float rounding.
    fn split(self, axis: Axis, ratio: f32) -> (Rect, Rect) {
        match axis {
            Axis::Row => {
                let w1 = self.w * ratio;
                (
                    Rect { w: w1, ..self },
                    Rect {
                        x: self.x + w1,
                        w: self.w - w1,
                        ..self
                    },
                )
            }
            Axis::Col => {
                let h1 = self.h * ratio;
                (
                    Rect { h: h1, ..self },
                    Rect {
                        y: self.y + h1,
                        h: self.h - h1,
                        ..self
                    },
                )
            }
        }
    }
}

/// A node in the pane tree: either a single pane (leaf) or a split of two subtrees.
#[derive(Debug)]
enum Node {
    Leaf(PaneId),
    Split {
        axis: Axis,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    /// The left-/top-most leaf of this subtree.
    fn first_leaf(&self) -> PaneId {
        match self {
            Node::Leaf(p) => *p,
            Node::Split { first, .. } => first.first_leaf(),
        }
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(p) => out.push(*p),
            Node::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf(p) => *p == id,
            Node::Split { first, second, .. } => first.contains(id) || second.contains(id),
        }
    }

    /// Exchange the leaves holding `a` and `b` (swapping their positions in the tiling). Each
    /// id occupies exactly one leaf, so a single traversal suffices.
    fn swap_leaves(&mut self, a: PaneId, b: PaneId) {
        match self {
            Node::Leaf(p) => {
                if *p == a {
                    *p = b;
                } else if *p == b {
                    *p = a;
                }
            }
            Node::Split { first, second, .. } => {
                first.swap_leaves(a, b);
                second.swap_leaves(a, b);
            }
        }
    }

    /// Replace the leaf `target` with `replacement` (taken on the first match).
    /// Returns whether the target was found.
    fn replace_leaf(&mut self, target: PaneId, replacement: &mut Option<Node>) -> bool {
        match self {
            Node::Leaf(p) => {
                if *p == target {
                    *self = replacement.take().expect("replacement taken exactly once");
                    true
                } else {
                    false
                }
            }
            Node::Split { first, second, .. } => {
                first.replace_leaf(target, replacement) || second.replace_leaf(target, replacement)
            }
        }
    }

    /// Remove the leaf `target`, collapsing its parent split into `target`'s sibling.
    /// Returns a leaf of the promoted sibling (a sensible new focus), or `None` if
    /// `target` isn't directly under a split in this subtree.
    fn remove_leaf(&mut self, target: PaneId) -> Option<PaneId> {
        let Node::Split { first, second, .. } = self else {
            return None;
        };
        if matches!(&**first, Node::Leaf(p) if *p == target) {
            let sibling = std::mem::replace(&mut **second, Node::Leaf(target));
            let focus = sibling.first_leaf();
            *self = sibling;
            return Some(focus);
        }
        if matches!(&**second, Node::Leaf(p) if *p == target) {
            let sibling = std::mem::replace(&mut **first, Node::Leaf(target));
            let focus = sibling.first_leaf();
            *self = sibling;
            return Some(focus);
        }
        first
            .remove_leaf(target)
            .or_else(|| second.remove_leaf(target))
    }

    /// Nudge the ratio of the deepest split of `axis` on the path to `target` by
    /// `delta` (clamped). Returns whether such an ancestor existed.
    fn resize_ancestor(&mut self, target: PaneId, axis: Axis, delta: f32) -> bool {
        let Node::Split {
            axis: a,
            ratio,
            first,
            second,
        } = self
        else {
            return false;
        };
        let a = *a;
        let child = if first.contains(target) {
            first.as_mut()
        } else if second.contains(target) {
            second.as_mut()
        } else {
            return false;
        };
        // Recurse first so the *deepest* (nearest) matching divider wins.
        if child.resize_ancestor(target, axis, delta) {
            return true;
        }
        if a == axis {
            *ratio = (*ratio + delta).clamp(MIN_RATIO, 1.0 - MIN_RATIO);
            return true;
        }
        false
    }

    fn even_out(&mut self) {
        if let Node::Split {
            ratio,
            first,
            second,
            ..
        } = self
        {
            *ratio = 0.5;
            first.even_out();
            second.even_out();
        }
    }

    fn layout_into(&self, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf(p) => out.push((*p, rect)),
            Node::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (r1, r2) = rect.split(*axis, *ratio);
                first.layout_into(r1, out);
                second.layout_into(r2, out);
            }
        }
    }
}

/// A tab's tiling tree of panes. Always holds at least one pane and exactly one
/// focused pane.
#[derive(Debug)]
pub struct PaneTree {
    root: Node,
    focused: PaneId,
    zoomed: bool,
    next_id: u32,
    /// Which preset [`cycle_layout`](Self::cycle_layout) last applied, so repeated presses walk
    /// through the arrangements.
    layout_preset: u8,
}

impl PaneTree {
    /// A fresh tree: a single, focused pane. Its id is [`PaneTree::focused`].
    #[must_use]
    pub fn new() -> Self {
        let root = PaneId(0);
        Self {
            root: Node::Leaf(root),
            focused: root,
            zoomed: false,
            next_id: 1,
            layout_preset: 0,
        }
    }

    /// The number of panes (leaves) - always `1..=MAX_PANES`.
    #[must_use]
    pub fn count(&self) -> usize {
        self.panes().len()
    }

    /// Every pane id, in left-to-right, top-to-bottom tree order.
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        let mut v = Vec::new();
        self.root.collect_leaves(&mut v);
        v
    }

    /// The focused pane's id.
    #[must_use]
    pub fn focused(&self) -> PaneId {
        self.focused
    }

    /// Whether the focused pane is zoomed to fill the viewport.
    #[must_use]
    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    /// Whether `id` is a live pane in this tree.
    #[must_use]
    pub fn contains(&self, id: PaneId) -> bool {
        self.root.contains(id)
    }

    /// Focus pane `id` directly (e.g. on a mouse click). Returns `false` without
    /// changing anything if `id` isn't a live pane. Cancels zoom.
    pub fn set_focus(&mut self, id: PaneId) -> bool {
        if self.root.contains(id) {
            self.focused = id;
            self.zoomed = false;
            true
        } else {
            false
        }
    }

    /// Split the focused pane in `dir`, giving the new pane focus. Returns the new
    /// pane's id, or `None` if the tab is already at [`MAX_PANES`]. Cancels zoom.
    pub fn split(&mut self, dir: Dir) -> Option<PaneId> {
        if self.count() >= MAX_PANES {
            return None;
        }
        let new = PaneId(self.next_id);
        let old = Node::Leaf(self.focused);
        let created = Node::Leaf(new);
        let (first, second) = if dir.new_pane_is_second() {
            (old, created)
        } else {
            (created, old)
        };
        let mut replacement = Some(Node::Split {
            axis: dir.axis(),
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        });
        let found = self.root.replace_leaf(self.focused, &mut replacement);
        debug_assert!(found, "the focused pane must exist in the tree");
        self.next_id += 1;
        self.focused = new;
        self.zoomed = false;
        Some(new)
    }

    /// Close the focused pane, collapsing its split so the sibling takes the space and
    /// receives focus. Returns `false` without changing anything if this is the last
    /// pane - the caller decides what closing the final pane means (e.g. close the
    /// tab). Cancels zoom.
    pub fn close(&mut self) -> bool {
        if self.count() <= 1 {
            return false;
        }
        match self.root.remove_leaf(self.focused) {
            Some(new_focus) => {
                self.focused = new_focus;
                self.zoomed = false;
                true
            }
            None => false,
        }
    }

    /// The nearest pane to the focused one in `dir` (by the tiled layout), or `None` at the
    /// edge of the layout. Shared by [`focus`](Self::focus) and [`swap`](Self::swap).
    fn neighbor(&self, dir: Dir) -> Option<PaneId> {
        // Adjacency is defined by the tiled layout, even when zoomed.
        let layout = self.tiled_layout(Rect::new(0.0, 0.0, 1.0, 1.0));
        let cur = layout
            .iter()
            .find(|(id, _)| *id == self.focused)
            .map(|&(_, r)| r)?;
        layout
            .iter()
            .filter(|(id, _)| *id != self.focused)
            .filter_map(|&(id, r)| candidate_score(cur, r, dir).map(|score| (score, id)))
            .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, id)| id)
    }

    /// Move focus to the nearest pane in `dir`. Returns whether focus moved (it will
    /// not at the edge of the layout). Cancels zoom.
    pub fn focus(&mut self, dir: Dir) -> bool {
        if let Some(id) = self.neighbor(dir) {
            self.focused = id;
            self.zoomed = false;
            true
        } else {
            false
        }
    }

    /// Swap the focused pane with its nearest neighbor in `dir` (exchanging their positions in
    /// the tiling); focus follows the moved pane. Returns whether a neighbor existed to swap
    /// with (the guide's §11 "Swap pane", `⌥⇧arrows`). Cancels zoom.
    pub fn swap(&mut self, dir: Dir) -> bool {
        if let Some(neighbor) = self.neighbor(dir) {
            self.root.swap_leaves(self.focused, neighbor);
            self.zoomed = false;
            true
        } else {
            false
        }
    }

    /// Toggle zoom on the focused pane (fill the viewport, or restore the tiling).
    /// Returns the new zoom state.
    pub fn zoom_toggle(&mut self) -> bool {
        self.zoomed = !self.zoomed;
        self.zoomed
    }

    /// Nudge the divider immediately enclosing the focused pane along `dir` by `delta`
    /// (a fraction of that split's extent, e.g. `0.05`) - the keyboard equivalent of
    /// dragging that divider. Widens the pane on the `dir`-near side and narrows the
    /// other. Returns whether a divider along `dir` existed to move. Cancels zoom.
    pub fn resize(&mut self, dir: Dir, delta: f32) -> bool {
        self.zoomed = false;
        let signed = if dir.new_pane_is_second() {
            delta
        } else {
            -delta
        };
        self.root.resize_ancestor(self.focused, dir.axis(), signed)
    }

    /// Reset every split to an even 50/50, undoing manual resizes. Cancels zoom.
    pub fn even_out(&mut self) {
        self.zoomed = false;
        self.root.even_out();
    }

    /// Cycle the panes through preset layouts (the guide's §11 `⌥Space` "Cycle layout preset"):
    /// even columns -> even rows -> main-vertical (one large pane left, the rest stacked right),
    /// keeping the same panes (and the focused one). A no-op with fewer than two panes. Cancels
    /// zoom. Returns whether the layout changed.
    pub fn cycle_layout(&mut self) -> bool {
        let mut ids = Vec::new();
        self.root.collect_leaves(&mut ids);
        if ids.len() < 2 {
            return false;
        }
        // Apply the current preset, then advance - so the first press gives even columns.
        self.root = match self.layout_preset {
            0 => build_chain(Axis::Row, &ids),
            1 => build_chain(Axis::Col, &ids),
            _ => build_main_vertical(&ids),
        };
        self.layout_preset = (self.layout_preset + 1) % 3;
        self.zoomed = false;
        true
    }

    /// Each visible pane's rectangle within `viewport`. When a pane is zoomed, only
    /// that pane is returned, filling the whole viewport; otherwise the panes tile
    /// `viewport` exactly, with no gaps or overlaps.
    #[must_use]
    pub fn layout(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
        if self.zoomed {
            vec![(self.focused, viewport)]
        } else {
            self.tiled_layout(viewport)
        }
    }

    /// The full tiling, ignoring zoom - the geometry focus/adjacency is defined over.
    fn tiled_layout(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.root.layout_into(viewport, &mut out);
        out
    }
}

impl Default for PaneTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Build an evenly-split chain of `ids` along `axis` (each pane an equal share) - a left-deep
/// tree of splits. `ids` must be non-empty.
#[allow(
    clippy::cast_precision_loss,
    reason = "pane counts are tiny (<=8, the hard cap)"
)]
fn build_chain(axis: Axis, ids: &[PaneId]) -> Node {
    match ids {
        [] => unreachable!("build_chain needs at least one id"),
        [id] => Node::Leaf(*id),
        [id, rest @ ..] => Node::Split {
            axis,
            // The first pane takes `1/n`; the recursive remainder splits the rest evenly.
            ratio: 1.0 / ids.len() as f32,
            first: Box::new(Node::Leaf(*id)),
            second: Box::new(build_chain(axis, rest)),
        },
    }
}

/// Build a "main-vertical" layout: the first pane fills the left half, the rest stack evenly in
/// the right half. `ids` must be non-empty.
fn build_main_vertical(ids: &[PaneId]) -> Node {
    match ids {
        [] => unreachable!("build_main_vertical needs at least one id"),
        [id] => Node::Leaf(*id),
        [id, rest @ ..] => Node::Split {
            axis: Axis::Row,
            ratio: 0.5,
            first: Box::new(Node::Leaf(*id)),
            second: Box::new(build_chain(Axis::Col, rest)),
        },
    }
}

/// Score `cand` as a focus target from `cur` moving `dir`: `Some((primary,
/// secondary))` to minimize if `cand` lies in that direction and overlaps
/// perpendicularly, else `None`. `primary` is the gap along `dir`; `secondary` is the
/// perpendicular center offset, breaking ties toward the best-aligned neighbor.
fn candidate_score(cur: Rect, cand: Rect, dir: Dir) -> Option<(f32, f32)> {
    let eps = 1e-4;
    let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| a0 < b1 - eps && b0 < a1 - eps;
    // Perpendicular center offset - breaks ties toward the best-aligned neighbor.
    let perp = match dir {
        Dir::Left | Dir::Right => ((cand.y + cand.h / 2.0) - (cur.y + cur.h / 2.0)).abs(),
        Dir::Up | Dir::Down => ((cand.x + cand.w / 2.0) - (cur.x + cur.w / 2.0)).abs(),
    };
    match dir {
        Dir::Right => (cand.x + eps >= cur.x + cur.w
            && overlap(cur.y, cur.y + cur.h, cand.y, cand.y + cand.h))
        .then_some((cand.x - (cur.x + cur.w), perp)),
        Dir::Left => (cand.x + cand.w <= cur.x + eps
            && overlap(cur.y, cur.y + cur.h, cand.y, cand.y + cand.h))
        .then_some((cur.x - (cand.x + cand.w), perp)),
        Dir::Down => (cand.y + eps >= cur.y + cur.h
            && overlap(cur.x, cur.x + cur.w, cand.x, cand.x + cand.w))
        .then_some((cand.y - (cur.y + cur.h), perp)),
        Dir::Up => (cand.y + cand.h <= cur.y + eps
            && overlap(cur.x, cur.x + cur.w, cand.x, cand.x + cand.w))
        .then_some((cur.y - (cand.y + cand.h), perp)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Rect = Rect {
        x: 10.0,
        y: 20.0,
        w: 800.0,
        h: 600.0,
    };
    /// A generous pixel tolerance: real gaps/overlaps are tens of pixels, while float
    /// rounding over <=8 nested splits stays well under one.
    const EPS: f32 = 0.5;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }

    fn rect_of(tree: &PaneTree, id: PaneId) -> Rect {
        tree.layout(VIEWPORT)
            .into_iter()
            .find(|(p, _)| *p == id)
            .map(|(_, r)| r)
            .expect("pane should be in the layout")
    }

    #[test]
    fn new_tree_has_one_focused_pane_filling_the_viewport() {
        let t = PaneTree::new();
        assert_eq!(t.count(), 1);
        assert!(t.contains(t.focused()));
        assert!(!t.is_zoomed());
        let layout = t.layout(VIEWPORT);
        assert_eq!(layout.len(), 1);
        assert_eq!(layout[0].1, VIEWPORT);
    }

    #[test]
    fn split_right_tiles_into_two_side_by_side_halves() {
        let mut t = PaneTree::new();
        let left = t.focused();
        let right = t.split(Dir::Right).expect("under the cap");
        assert_eq!(t.count(), 2);
        assert_eq!(t.focused(), right, "the new pane takes focus");

        let lr = rect_of(&t, left);
        let rr = rect_of(&t, right);
        assert!(
            approx(lr.w, 400.0) && approx(rr.w, 400.0),
            "even 50/50 split"
        );
        assert!(approx(lr.x, 10.0) && approx(rr.x, 410.0), "left then right");
        assert!(
            approx(lr.h, 600.0) && approx(rr.h, 600.0),
            "full height each"
        );
    }

    #[test]
    fn split_down_stacks_top_over_bottom() {
        let mut t = PaneTree::new();
        let top = t.focused();
        let bottom = t.split(Dir::Down).expect("under the cap");
        let tr = rect_of(&t, top);
        let br = rect_of(&t, bottom);
        assert!(approx(tr.h, 300.0) && approx(br.h, 300.0));
        assert!(approx(tr.y, 20.0) && approx(br.y, 320.0), "top then bottom");
        assert!(
            approx(tr.w, 800.0) && approx(br.w, 800.0),
            "full width each"
        );
    }

    #[test]
    fn split_is_capped_at_max_panes() {
        let mut t = PaneTree::new();
        for _ in 1..MAX_PANES {
            assert!(t.split(Dir::Right).is_some());
        }
        assert_eq!(t.count(), MAX_PANES);
        assert!(t.split(Dir::Right).is_none(), "the 9th pane is refused");
        assert_eq!(t.count(), MAX_PANES);
    }

    #[test]
    fn close_collapses_the_split_and_refocuses_the_sibling() {
        let mut t = PaneTree::new();
        let left = t.focused();
        let right = t.split(Dir::Right).unwrap();
        assert_eq!(t.focused(), right);

        assert!(t.close(), "closed the focused (right) pane");
        assert_eq!(t.count(), 1);
        assert_eq!(t.focused(), left, "focus falls back to the sibling");
        assert_eq!(rect_of(&t, left), VIEWPORT, "sibling reclaims the space");
    }

    #[test]
    fn closing_the_last_pane_is_a_no_op() {
        let mut t = PaneTree::new();
        assert!(!t.close());
        assert_eq!(t.count(), 1);
    }

    #[test]
    fn pane_ids_are_never_reused() {
        let mut t = PaneTree::new();
        let a = t.split(Dir::Right).unwrap();
        assert!(t.close()); // closes `a`, focus back to pane 0
        let b = t.split(Dir::Right).unwrap();
        assert_ne!(a, b, "a fresh split gets a brand-new id");
    }

    #[test]
    fn zoom_fills_the_viewport_and_restores() {
        let mut t = PaneTree::new();
        let _left = t.focused();
        let right = t.split(Dir::Right).unwrap();

        assert!(t.zoom_toggle(), "now zoomed");
        let layout = t.layout(VIEWPORT);
        assert_eq!(layout.len(), 1);
        assert_eq!(layout[0], (right, VIEWPORT));

        assert!(!t.zoom_toggle(), "unzoomed");
        assert_eq!(t.layout(VIEWPORT).len(), 2, "tiling restored");
    }

    #[test]
    fn directional_focus_moves_between_neighbors_and_stops_at_edges() {
        let mut t = PaneTree::new();
        let left = t.focused();
        let right = t.split(Dir::Right).unwrap(); // focus is on `right`

        assert!(!t.focus(Dir::Right), "already at the right edge");
        assert!(t.focus(Dir::Left), "moves to the left pane");
        assert_eq!(t.focused(), left);
        assert!(!t.focus(Dir::Left), "already at the left edge");
        assert!(t.focus(Dir::Right));
        assert_eq!(t.focused(), right);
    }

    #[test]
    #[allow(
        clippy::many_single_char_names,
        reason = "test: a/b/c/t name the panes and the tree tersely"
    )]
    fn cycle_layout_rearranges_panes_through_presets_keeping_them_all() {
        let mut t = PaneTree::new();
        let a = t.focused();
        let b = t.split(Dir::Right).unwrap();
        let c = t.split(Dir::Down).unwrap(); // 3 panes, some arrangement
        let all = |t: &PaneTree| {
            let mut v = t.panes();
            v.sort();
            v
        };
        let mut sorted = vec![a, b, c];
        sorted.sort();

        // Preset 0: even columns (all side by side) - three equal-width panes at y=0.
        assert!(t.cycle_layout());
        assert_eq!(all(&t), sorted, "no pane lost");
        let cols = t.layout(Rect::new(0.0, 0.0, 900.0, 300.0));
        assert!(
            cols.iter().all(|(_, r)| approx(r.h, 300.0)),
            "even columns share the full height"
        );
        assert!(cols.iter().all(|(_, r)| approx(r.w, 300.0)), "equal widths");

        // Preset 1: even rows (stacked).
        assert!(t.cycle_layout());
        let rows = t.layout(Rect::new(0.0, 0.0, 300.0, 900.0));
        assert!(rows.iter().all(|(_, r)| approx(r.w, 300.0)), "full width");
        assert!(
            rows.iter().all(|(_, r)| approx(r.h, 300.0)),
            "equal heights"
        );

        // Preset 2: main-vertical (first pane fills the left half).
        assert!(t.cycle_layout());
        let main = t.layout(Rect::new(0.0, 0.0, 800.0, 600.0));
        let first = main.iter().find(|(id, _)| *id == a).unwrap().1;
        assert!(approx(first.w, 400.0), "main pane takes the left half");
    }

    #[test]
    fn swap_exchanges_the_focused_pane_with_its_neighbor() {
        let mut t = PaneTree::new();
        let first = t.focused();
        let second = t.split(Dir::Right).unwrap(); // focus on `second` (the right half)
        assert!(
            rect_of(&t, second).x > rect_of(&t, first).x,
            "second starts on the right"
        );

        // Swapping the focused (right) pane left exchanges the two panes' positions; focus
        // follows the moved pane, which now sits on the left.
        assert!(t.swap(Dir::Left));
        assert_eq!(t.focused(), second, "focus follows the moved pane");
        assert!(
            rect_of(&t, second).x < rect_of(&t, first).x,
            "second is now on the left"
        );
        // At the edge there is no neighbor to swap with.
        assert!(!t.swap(Dir::Left), "no neighbor further left");
    }

    #[test]
    fn resize_moves_the_divider_and_even_out_restores_it() {
        let mut t = PaneTree::new();
        let left = t.focused();
        let right = t.split(Dir::Right).unwrap();

        // Focused is `right`; nudging the divider right widens the left pane.
        assert!(t.resize(Dir::Right, 0.1));
        assert!(approx(rect_of(&t, left).w, 480.0), "left grew to 60%");
        assert!(approx(rect_of(&t, right).w, 320.0), "right shrank to 40%");

        t.even_out();
        assert!(approx(rect_of(&t, left).w, 400.0), "back to 50/50");
    }

    #[test]
    fn resize_without_a_matching_divider_is_a_no_op() {
        let mut t = PaneTree::new();
        assert!(!t.resize(Dir::Right, 0.1), "a single pane has no divider");
    }

    // ----- property-based tiling invariants -----------------------------------

    #[derive(Clone, Copy, Debug)]
    enum Op {
        Split(Dir),
        Close,
        Focus(Dir),
        Resize(Dir),
        Zoom,
        Even,
        SetFocus(u8),
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        let eps = EPS;
        a.x < b.x + b.w - eps
            && b.x < a.x + a.w - eps
            && a.y < b.y + b.h - eps
            && b.y < a.y + a.h - eps
    }

    fn assert_invariants(t: &PaneTree) {
        let n = t.count();
        assert!((1..=MAX_PANES).contains(&n), "pane count {n} out of range");

        let panes = t.panes();
        assert_eq!(panes.len(), n);
        let unique: std::collections::HashSet<_> = panes.iter().collect();
        assert_eq!(unique.len(), n, "duplicate pane ids: {panes:?}");
        assert!(t.contains(t.focused()), "focus points at a missing pane");

        let layout = t.layout(VIEWPORT);
        if t.is_zoomed() {
            assert_eq!(layout.len(), 1);
            assert_eq!(layout[0].0, t.focused());
            assert_eq!(layout[0].1, VIEWPORT);
            return;
        }

        assert_eq!(layout.len(), n, "every pane must get exactly one rectangle");
        for (_, r) in &layout {
            assert!(r.w > EPS && r.h > EPS, "degenerate pane rect {r:?}");
            assert!(
                r.x >= VIEWPORT.x - EPS
                    && r.y >= VIEWPORT.y - EPS
                    && r.x + r.w <= VIEWPORT.x + VIEWPORT.w + EPS
                    && r.y + r.h <= VIEWPORT.y + VIEWPORT.h + EPS,
                "pane rect {r:?} escapes the viewport"
            );
        }
        for i in 0..layout.len() {
            for j in (i + 1)..layout.len() {
                assert!(
                    !overlaps(layout[i].1, layout[j].1),
                    "panes overlap: {:?} and {:?}",
                    layout[i].1,
                    layout[j].1
                );
            }
        }
        // Non-overlapping rects inside the viewport whose areas sum to the viewport's
        // area prove an exact, gap-free tiling.
        let area: f32 = layout.iter().map(|(_, r)| r.w * r.h).sum();
        let vp_area = VIEWPORT.w * VIEWPORT.h;
        assert!(
            (area - vp_area).abs() <= 1e-2 * vp_area,
            "tiling area {area} != viewport area {vp_area}"
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(400))]

        /// Any sequence of pane operations keeps the tree well-formed and its layout
        /// an exact tiling of the viewport - the invariants the renderer relies on.
        #[test]
        fn random_ops_preserve_the_tiling_invariants(
            ops in proptest::collection::vec(op_strategy(), 0..64)
        ) {
            let mut t = PaneTree::new();
            assert_invariants(&t);
            for op in ops {
                match op {
                    Op::Split(d) => { t.split(d); }
                    Op::Close => { t.close(); }
                    Op::Focus(d) => { t.focus(d); }
                    Op::Resize(d) => { t.resize(d, 0.1); }
                    Op::Zoom => { t.zoom_toggle(); }
                    Op::Even => { t.even_out(); }
                    Op::SetFocus(i) => {
                        let panes = t.panes();
                        let id = panes[usize::from(i) % panes.len()];
                        t.set_focus(id);
                    }
                }
                assert_invariants(&t);
            }
        }
    }

    fn dir_strategy() -> impl proptest::strategy::Strategy<Value = Dir> {
        proptest::prop_oneof![
            proptest::strategy::Just(Dir::Left),
            proptest::strategy::Just(Dir::Right),
            proptest::strategy::Just(Dir::Up),
            proptest::strategy::Just(Dir::Down),
        ]
    }

    fn op_strategy() -> impl proptest::strategy::Strategy<Value = Op> {
        use proptest::strategy::{Just, Strategy};
        proptest::prop_oneof![
            dir_strategy().prop_map(Op::Split),
            Just(Op::Close),
            dir_strategy().prop_map(Op::Focus),
            dir_strategy().prop_map(Op::Resize),
            Just(Op::Zoom),
            Just(Op::Even),
            proptest::prelude::any::<u8>().prop_map(Op::SetFocus),
        ]
    }
}
