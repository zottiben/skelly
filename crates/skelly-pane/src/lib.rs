//! `skelly-pane` - a tab's panes and their Fibonacci-spiral tiling.
//!
//! This is pure geometry + state logic with no UI, GPU, or terminal dependency -
//! a leaf crate the `skelly` binary drives, mirroring `skelly-config`. The binary
//! owns the *wiring* (mapping a [`PaneId`] to a live terminal, rendering each pane at
//! its computed rectangle, and binding keys to these operations); this crate owns the
//! *model*: which panes exist, which one is focused, and how they tile the viewport.
//!
//! Panes form an ordered spiral with no cap on their number. [`PaneTree::open`] appends
//! a pane to the end of the spiral, and the layout follows from the order alone: each pane
//! cuts its share off the space the earlier panes left, turning clockwise (left, top,
//! right, bottom, left, ...), and with `k` panes still to place the next one takes
//! `F(k) / F(k+1)` of that space (`F` being the Fibonacci sequence). On a viewport sized
//! `F(n+1) x F(n)` the `n` panes are exactly the squares of the Fibonacci spiral. Exactly
//! one pane is always focused, and the focused pane can be zoomed to fill the viewport.

#![doc(test(attr(deny(warnings))))]

use serde::{Deserialize, Serialize};

/// A stable pane identifier. Allocated monotonically and never reused within a tree's
/// lifetime, so an id the binary maps to a live terminal stays valid until that pane
/// is closed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PaneId(u32);

/// A direction - the argument to [`PaneTree::focus`].
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Axis {
    Row,
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

/// A serializable snapshot of a tab's panes for launch-time session restore (layout only -
/// shells are never re-run): which pane is focused and whether it is zoomed. The spiral
/// itself follows from the pane count, which the caller already records as its per-pane
/// payload (one cwd per pane, in `panes()` order). Round-trips via [`PaneTree::to_layout`] /
/// [`PaneTree::from_layout`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaneLayout {
    focused: usize,
    zoomed: bool,
}

/// A tab's panes in spiral order. Always holds at least one pane and exactly one focused
/// pane.
#[derive(Debug)]
pub struct PaneTree {
    panes: Vec<PaneId>,
    focused: PaneId,
    zoomed: bool,
    next_id: u32,
}

impl PaneTree {
    /// A fresh tree: a single, focused pane. Its id is [`PaneTree::focused`].
    #[must_use]
    pub fn new() -> Self {
        let root = PaneId(0);
        Self {
            panes: vec![root],
            focused: root,
            zoomed: false,
            next_id: 1,
        }
    }

    /// The number of panes - always at least 1.
    #[must_use]
    pub fn count(&self) -> usize {
        self.panes.len()
    }

    /// Every pane id, in spiral order (largest first, newest last).
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        self.panes.clone()
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
        self.panes.contains(&id)
    }

    /// Focus pane `id` directly (e.g. on a mouse click). Returns `false` without
    /// changing anything if `id` isn't a live pane. Cancels zoom.
    pub fn set_focus(&mut self, id: PaneId) -> bool {
        if self.contains(id) {
            self.focused = id;
            self.zoomed = false;
            true
        } else {
            false
        }
    }

    /// Open a new pane at the end of the spiral and focus it; the existing panes re-flow
    /// to their Fibonacci shares. Returns the new pane's id. Cancels zoom.
    pub fn open(&mut self) -> PaneId {
        let new = PaneId(self.next_id);
        self.next_id += 1;
        self.panes.push(new);
        self.focused = new;
        self.zoomed = false;
        new
    }

    /// Close the focused pane; the later panes move up the spiral to fill its place, and
    /// focus goes to the pane that takes its slot (or the previous one when it was last).
    /// Returns `false` without changing anything if this is the last pane - the caller
    /// decides what closing the final pane means (e.g. close the tab). Cancels zoom.
    pub fn close(&mut self) -> bool {
        if self.panes.len() <= 1 {
            return false;
        }
        let Some(index) = self.panes.iter().position(|&p| p == self.focused) else {
            return false;
        };
        self.panes.remove(index);
        self.focused = self.panes[index.min(self.panes.len() - 1)];
        self.zoomed = false;
        true
    }

    /// Move focus to the nearest pane in `dir` (by the tiled layout). Returns whether focus
    /// moved (it will not at the edge of the layout). Cancels zoom.
    pub fn focus(&mut self, dir: Dir) -> bool {
        let layout = self.tiled_layout(Rect::new(0.0, 0.0, 1.0, 1.0));
        let Some(cur) = layout
            .iter()
            .find(|(id, _)| *id == self.focused)
            .map(|&(_, r)| r)
        else {
            return false;
        };
        let target = layout
            .iter()
            .filter(|(id, _)| *id != self.focused)
            .filter_map(|&(id, r)| candidate_score(cur, r, dir).map(|score| (score, id)))
            .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, id)| id);
        if let Some(id) = target {
            self.focused = id;
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

    /// Each visible pane's rectangle within `viewport`. When a pane is zoomed, only
    /// that pane is returned, filling the whole viewport; otherwise the panes tile
    /// `viewport` exactly as the Fibonacci spiral, with no gaps or overlaps.
    #[must_use]
    pub fn layout(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
        if self.zoomed {
            vec![(self.focused, viewport)]
        } else {
            self.tiled_layout(viewport)
        }
    }

    fn tiled_layout(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
        self.panes
            .iter()
            .copied()
            .zip(spiral(viewport, self.panes.len()))
            .collect()
    }

    /// A serializable snapshot of the focus and zoom state for session persistence. The
    /// caller records one payload entry per pane in `panes()` order alongside it, and passes
    /// that count back to [`from_layout`](Self::from_layout).
    #[must_use]
    pub fn to_layout(&self) -> PaneLayout {
        PaneLayout {
            focused: self
                .panes
                .iter()
                .position(|&p| p == self.focused)
                .unwrap_or(0),
            zoomed: self.zoomed,
        }
    }

    /// Rebuild a tree of `count` panes (at least one) from a persisted [`PaneLayout`],
    /// allocating fresh ids in `panes()` order so the nth pane pairs with the nth saved
    /// payload. An out-of-range focus falls back to the first pane.
    #[must_use]
    pub fn from_layout(count: usize, layout: &PaneLayout) -> Self {
        let panes: Vec<PaneId> = (0..).map(PaneId).take(count.max(1)).collect();
        let focused = panes.get(layout.focused).copied().unwrap_or(panes[0]);
        let next_id = panes.last().map_or(0, |p| p.0 + 1);
        Self {
            panes,
            focused,
            zoomed: layout.zoomed,
            next_id,
        }
    }
}

impl Default for PaneTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Tile `viewport` with `n` panes as a clockwise Fibonacci spiral, in spiral order.
fn spiral(viewport: Rect, n: usize) -> Vec<Rect> {
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }
    let mut rest = viewport;
    for (i, ratio) in fibonacci_shares(n).into_iter().enumerate() {
        let (pane, remainder) = match i % 4 {
            0 => rest.split(Axis::Row, ratio),
            1 => rest.split(Axis::Col, ratio),
            2 => {
                let (remainder, pane) = rest.split(Axis::Row, 1.0 - ratio);
                (pane, remainder)
            }
            _ => {
                let (remainder, pane) = rest.split(Axis::Col, 1.0 - ratio);
                (pane, remainder)
            }
        };
        out.push(pane);
        rest = remainder;
    }
    out.push(rest);
    out
}

/// The share each of the first `n - 1` panes takes of the space left to it: with `k` panes
/// still to place, `F(k) / F(k+1)`. Built from the innermost split (`k = 2`, an even half)
/// outward via `F(k+1) / F(k+2) = 1 / (1 + F(k) / F(k+1))`, so no Fibonacci number overflows.
fn fibonacci_shares(n: usize) -> Vec<f32> {
    let mut shares = Vec::with_capacity(n.saturating_sub(1));
    let mut share = 0.5_f32;
    for _ in 1..n {
        shares.push(share);
        share = 1.0 / (1.0 + share);
    }
    shares.reverse();
    shares
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
    const EPS: f32 = 0.5;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }

    fn approx_rect(a: Rect, b: Rect) -> bool {
        approx(a.x, b.x) && approx(a.y, b.y) && approx(a.w, b.w) && approx(a.h, b.h)
    }

    fn rect_of(tree: &PaneTree, id: PaneId) -> Rect {
        tree.layout(VIEWPORT)
            .into_iter()
            .find(|(p, _)| *p == id)
            .map(|(_, r)| r)
            .expect("pane should be in the layout")
    }

    fn tree_with(count: usize) -> PaneTree {
        let mut t = PaneTree::new();
        for _ in 1..count {
            t.open();
        }
        t
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
    fn fibonacci_shares_are_consecutive_fibonacci_ratios() {
        let expected = [5.0 / 8.0, 3.0 / 5.0, 2.0 / 3.0, 1.0 / 2.0];
        let shares = fibonacci_shares(5);
        assert_eq!(shares.len(), expected.len());
        for (share, want) in shares.iter().zip(expected) {
            assert!((share - want).abs() < 1e-6, "{shares:?}");
        }
        assert!(fibonacci_shares(1).is_empty());
        let golden = fibonacci_shares(60)[0];
        assert!((golden - 0.618_034).abs() < 1e-5, "converges to 1/phi");
    }

    #[test]
    fn opening_panes_spirals_clockwise_left_top_right_bottom() {
        let viewport = Rect::new(0.0, 0.0, 800.0, 500.0);
        let t = tree_with(5);
        let rects: Vec<Rect> = t.layout(viewport).into_iter().map(|(_, r)| r).collect();
        let expected = [
            Rect::new(0.0, 0.0, 500.0, 500.0),
            Rect::new(500.0, 0.0, 300.0, 300.0),
            Rect::new(600.0, 300.0, 200.0, 200.0),
            Rect::new(500.0, 400.0, 100.0, 100.0),
            Rect::new(500.0, 300.0, 100.0, 100.0),
        ];
        assert_eq!(rects.len(), expected.len());
        for (got, want) in rects.iter().zip(expected) {
            assert!(approx_rect(*got, want), "{got:?} != {want:?}");
        }
    }

    #[test]
    fn a_fibonacci_shaped_viewport_tiles_into_the_fibonacci_squares() {
        // fib[k] = F(k + 1): 1, 1, 2, 3, 5, 8, ...
        let fib: Vec<f32> =
            std::iter::successors(Some((1.0_f32, 1.0_f32)), |&(a, b)| Some((b, a + b)))
                .map(|(a, _)| a)
                .take(16)
                .collect();
        for n in 2..=14 {
            let viewport = Rect::new(0.0, 0.0, fib[n] * 10.0, fib[n - 1] * 10.0);
            let rects: Vec<Rect> = tree_with(n)
                .layout(viewport)
                .into_iter()
                .map(|(_, r)| r)
                .collect();
            for (i, r) in rects.iter().enumerate() {
                let side = fib[n - 1 - i] * 10.0;
                assert!(
                    (r.w - side).abs() < 1e-3 * side && (r.h - side).abs() < 1e-3 * side,
                    "n={n} pane {i}: {r:?}, expected a {side} square"
                );
            }
        }
    }

    #[test]
    fn two_panes_split_evenly_side_by_side() {
        let mut t = PaneTree::new();
        let first = t.focused();
        let second = t.open();
        assert_eq!(t.count(), 2);
        assert_eq!(t.focused(), second, "the new pane takes focus");
        assert!(approx_rect(
            rect_of(&t, first),
            Rect::new(10.0, 20.0, 400.0, 600.0)
        ));
        assert!(approx_rect(
            rect_of(&t, second),
            Rect::new(410.0, 20.0, 400.0, 600.0)
        ));
    }

    #[test]
    fn opening_has_no_pane_cap() {
        let t = tree_with(200);
        assert_eq!(t.count(), 200);
        assert_invariants(&t);
    }

    #[test]
    fn open_cancels_zoom() {
        let mut t = tree_with(2);
        t.zoom_toggle();
        t.open();
        assert!(!t.is_zoomed());
        assert_eq!(t.layout(VIEWPORT).len(), 3);
    }

    #[test]
    fn close_reflows_the_spiral_and_focuses_the_pane_taking_its_slot() {
        let mut t = PaneTree::new();
        let a = t.focused();
        let b = t.open();
        let c = t.open();
        assert!(t.set_focus(b));

        assert!(t.close());
        assert_eq!(t.panes(), vec![a, c]);
        assert_eq!(t.focused(), c, "the next pane moves into the closed slot");
        assert!(approx_rect(
            rect_of(&t, c),
            Rect::new(410.0, 20.0, 400.0, 600.0)
        ));

        assert!(t.close());
        assert_eq!(
            t.focused(),
            a,
            "closing the last pane focuses the previous one"
        );
        assert_eq!(rect_of(&t, a), VIEWPORT, "the survivor reclaims the space");
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
        let a = t.open();
        assert!(t.close());
        let b = t.open();
        assert_ne!(a, b, "a fresh pane gets a brand-new id");
    }

    #[test]
    fn zoom_fills_the_viewport_and_restores() {
        let mut t = tree_with(2);
        let focused = t.focused();

        assert!(t.zoom_toggle(), "now zoomed");
        assert_eq!(t.layout(VIEWPORT), vec![(focused, VIEWPORT)]);

        assert!(!t.zoom_toggle(), "unzoomed");
        assert_eq!(t.layout(VIEWPORT).len(), 2, "tiling restored");
    }

    #[test]
    fn directional_focus_follows_the_spiral_and_stops_at_edges() {
        let mut t = PaneTree::new();
        let first = t.focused();
        let second = t.open();
        let third = t.open();

        assert!(!t.focus(Dir::Right), "already at the right edge");
        assert!(t.focus(Dir::Up));
        assert_eq!(t.focused(), second);
        assert!(t.focus(Dir::Left));
        assert_eq!(t.focused(), first);
        assert!(!t.focus(Dir::Left), "already at the left edge");
        assert!(t.focus(Dir::Right));
        assert!([second, third].contains(&t.focused()));
    }

    #[test]
    fn layout_snapshot_round_trips_count_focus_and_zoom() {
        let mut t = tree_with(4);
        let second = t.panes()[1];
        t.set_focus(second);
        t.zoom_toggle();

        let restored = PaneTree::from_layout(t.count(), &t.to_layout());
        assert_eq!(restored.count(), 4);
        assert!(restored.is_zoomed());
        assert_eq!(restored.panes()[1], restored.focused());

        let mut grown = PaneTree::from_layout(4, &t.to_layout());
        let new = grown.open();
        assert!(
            !restored.contains(new),
            "ids continue after the restored ones"
        );
    }

    #[test]
    fn from_layout_tolerates_a_degenerate_count_or_focus() {
        let layout = PaneLayout {
            focused: 9,
            zoomed: false,
        };
        let t = PaneTree::from_layout(0, &layout);
        assert_eq!(t.count(), 1);
        assert_eq!(t.focused(), t.panes()[0]);
    }

    // ----- property-based tiling invariants -----------------------------------

    #[derive(Clone, Copy, Debug)]
    enum Op {
        Open,
        Close,
        Focus(Dir),
        Zoom,
        SetFocus(u8),
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w - EPS
            && b.x < a.x + a.w - EPS
            && a.y < b.y + b.h - EPS
            && b.y < a.y + a.h - EPS
    }

    fn assert_invariants(t: &PaneTree) {
        let n = t.count();
        assert!(n >= 1, "a tree always holds a pane");

        let panes = t.panes();
        let unique: std::collections::HashSet<_> = panes.iter().collect();
        assert_eq!(unique.len(), n, "duplicate pane ids: {panes:?}");
        assert!(t.contains(t.focused()), "focus points at a missing pane");

        let layout = t.layout(VIEWPORT);
        if t.is_zoomed() {
            assert_eq!(layout, vec![(t.focused(), VIEWPORT)]);
            return;
        }

        assert_eq!(layout.len(), n, "every pane must get exactly one rectangle");
        for (_, r) in &layout {
            assert!(r.w >= 0.0 && r.h >= 0.0, "negative pane rect {r:?}");
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
                    Op::Open => { t.open(); }
                    Op::Close => { t.close(); }
                    Op::Focus(d) => { t.focus(d); }
                    Op::Zoom => { t.zoom_toggle(); }
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
            Just(Op::Open),
            Just(Op::Close),
            dir_strategy().prop_map(Op::Focus),
            Just(Op::Zoom),
            proptest::prelude::any::<u8>().prop_map(Op::SetFocus),
        ]
    }
}
