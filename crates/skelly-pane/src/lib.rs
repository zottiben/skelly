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
//!
//! Zooming never re-tiles the spiral: it moves a [`Camera`] over it. The resting view is the
//! [`Camera::OVERVIEW`] when unzoomed, or [`PaneTree::framing`] the focused pane when zoomed,
//! and [`Camera::lerp`] moves between two views with every pane still tiled edge to edge, so
//! the binary can animate a zoom by drawing [`PaneTree::view`] at each step.

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

/// The smallest pane extent (viewport-relative) a camera frames, so a pane past `f64`
/// precision deep in the spiral still yields a finite camera.
const MIN_FRAMED_EXTENT: f64 = 1e-12;

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
}

/// A rectangle in viewport-relative units (the viewport spans `0..1` on both axes), in `f64`
/// so panes deep in the spiral, and cameras zoomed far into them, keep their precision.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Unit {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Unit {
    const VIEWPORT: Unit = Unit {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    /// Split into `(first, second)` along `axis`, the first taking `ratio` of the
    /// extent. The second takes the exact remainder, so the two always tile `self`
    /// with no gap or overlap regardless of float rounding.
    fn split(self, axis: Axis, ratio: f64) -> (Unit, Unit) {
        match axis {
            Axis::Row => {
                let w1 = self.w * ratio;
                (
                    Unit { w: w1, ..self },
                    Unit {
                        x: self.x + w1,
                        w: self.w - w1,
                        ..self
                    },
                )
            }
            Axis::Col => {
                let h1 = self.h * ratio;
                (
                    Unit { h: h1, ..self },
                    Unit {
                        y: self.y + h1,
                        h: self.h - h1,
                        ..self
                    },
                )
            }
        }
    }
}

/// Where the view looks over a tab's spiral: a per-axis scale and offset in viewport-relative
/// units. [`Camera::OVERVIEW`] shows every pane at its resting size; [`PaneTree::framing`]
/// fills the viewport with one pane and pushes the rest out past its edges. Relative units
/// keep a camera valid across window resizes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Camera {
    scale_x: f64,
    scale_y: f64,
    x: f64,
    y: f64,
}

impl Camera {
    /// The resting, unzoomed view: the whole spiral filling the viewport.
    pub const OVERVIEW: Camera = Camera {
        scale_x: 1.0,
        scale_y: 1.0,
        x: 0.0,
        y: 0.0,
    };

    /// The camera that maps `target` onto the whole viewport.
    fn framing(target: Unit) -> Camera {
        let scale_x = 1.0 / target.w.max(MIN_FRAMED_EXTENT);
        let scale_y = 1.0 / target.h.max(MIN_FRAMED_EXTENT);
        Camera {
            scale_x,
            scale_y,
            x: -target.x * scale_x,
            y: -target.y * scale_y,
        }
    }

    /// How many times larger than [`Camera::OVERVIEW`] this camera shows the spiral: the
    /// geometric mean of its two axes' scales.
    #[must_use]
    pub fn zoom(self) -> f64 {
        (self.scale_x * self.scale_y).sqrt()
    }

    /// The view `t` of the way from `self` to `to` (`t` clamped to `0..=1`). Every pane moves
    /// along a straight line between its two placements, so the panes stay tiled edge to edge
    /// at every step, and the endpoints are exact.
    #[must_use]
    pub fn lerp(self, to: Camera, t: f32) -> Camera {
        if t <= 0.0 {
            return self;
        }
        if t >= 1.0 {
            return to;
        }
        let t = f64::from(t);
        let mix = |a: f64, b: f64| a + (b - a) * t;
        Camera {
            scale_x: mix(self.scale_x, to.scale_x),
            scale_y: mix(self.scale_y, to.scale_y),
            x: mix(self.x, to.x),
            y: mix(self.y, to.y),
        }
    }

    /// Where the relative rectangle `r` lands within `viewport`.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "on-screen coordinates are pixel-scale values well inside f32"
    )]
    fn place(self, r: Unit, viewport: Rect) -> Rect {
        let (vx, vy) = (f64::from(viewport.x), f64::from(viewport.y));
        let (vw, vh) = (f64::from(viewport.w), f64::from(viewport.h));
        Rect {
            x: (vx + (r.x * self.scale_x + self.x) * vw) as f32,
            y: (vy + (r.y * self.scale_y + self.y) * vh) as f32,
            w: (r.w * self.scale_x * vw) as f32,
            h: (r.h * self.scale_y * vh) as f32,
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
    /// changing anything if `id` isn't a live pane. A zoomed view moves to the new pane.
    pub fn set_focus(&mut self, id: PaneId) -> bool {
        if self.contains(id) {
            self.focused = id;
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
    /// moved (it will not at the edge of the layout). A zoomed view moves to the new pane.
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

    /// Each visible pane's rectangle within `viewport` at rest. When a pane is zoomed, only
    /// that pane is returned, filling exactly the whole viewport; otherwise the panes tile
    /// `viewport` as the Fibonacci spiral, with no gaps or overlaps. The same panes as
    /// [`view`](Self::view) through [`camera`](Self::camera), without its rounding.
    #[must_use]
    pub fn layout(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
        if self.zoomed {
            vec![(self.focused, viewport)]
        } else {
            self.tiled_layout(viewport)
        }
    }

    /// Every pane with the rectangle its terminal is sized for: its slot in the spiral, or
    /// `viewport` for the zoomed pane. Unlike [`layout`](Self::layout) it also lists the panes
    /// the zoom hides, so each is ready for a camera move to show it (ADR-0011).
    #[must_use]
    pub fn resting_layout(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
        let mut layout = self.tiled_layout(viewport);
        if self.zoomed {
            for (id, rect) in &mut layout {
                if *id == self.focused {
                    *rect = viewport;
                }
            }
        }
        layout
    }

    /// The resting camera: [`Camera::OVERVIEW`], or the focused pane framed when zoomed.
    #[must_use]
    pub fn camera(&self) -> Camera {
        if self.zoomed {
            self.framing(self.focused)
        } else {
            Camera::OVERVIEW
        }
    }

    /// The camera that fills the viewport with pane `id`, or [`Camera::OVERVIEW`] when `id`
    /// isn't a live pane.
    #[must_use]
    pub fn framing(&self, id: PaneId) -> Camera {
        self.panes
            .iter()
            .zip(spiral(self.panes.len()))
            .find(|(p, _)| **p == id)
            .map_or(Camera::OVERVIEW, |(_, r)| Camera::framing(r))
    }

    /// Each pane `camera` shows within `viewport`, with its rectangle there - which may reach
    /// past the viewport's edges. Panes the camera leaves wholly outside are omitted.
    #[must_use]
    pub fn view(&self, viewport: Rect, camera: Camera) -> Vec<(PaneId, Rect)> {
        self.panes
            .iter()
            .copied()
            .zip(spiral(self.panes.len()))
            .map(|(id, r)| (id, camera.place(r, viewport)))
            .filter(|(_, r)| shows(viewport, *r))
            .collect()
    }

    fn tiled_layout(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
        self.panes
            .iter()
            .copied()
            .zip(spiral(self.panes.len()))
            .map(|(id, r)| (id, Camera::OVERVIEW.place(r, viewport)))
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

/// Tile the relative viewport with `n` panes as a clockwise Fibonacci spiral, in spiral order.
fn spiral(n: usize) -> Vec<Unit> {
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }
    let mut rest = Unit::VIEWPORT;
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
fn fibonacci_shares(n: usize) -> Vec<f64> {
    let mut shares = Vec::with_capacity(n.saturating_sub(1));
    let mut share = 0.5_f64;
    for _ in 1..n {
        shares.push(share);
        share = 1.0 / (1.0 + share);
    }
    shares.reverse();
    shares
}

/// Whether `r` covers any of `viewport` (a sliver under a thousandth of a pixel-scale unit, from
/// rounding at a pane edge, does not count).
fn shows(viewport: Rect, r: Rect) -> bool {
    let eps = 1e-3;
    r.x < viewport.x + viewport.w - eps
        && viewport.x < r.x + r.w - eps
        && r.y < viewport.y + viewport.h - eps
        && viewport.y < r.y + r.h - eps
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
    fn zoom_follows_focus_to_another_pane() {
        let mut t = PaneTree::new();
        let first = t.focused();
        t.open();
        let third = t.open();
        t.zoom_toggle();

        assert!(t.focus(Dir::Left));
        assert!(t.is_zoomed(), "moving focus keeps the zoom");
        assert_eq!(t.layout(VIEWPORT), vec![(first, VIEWPORT)]);

        assert!(t.set_focus(third));
        assert!(t.is_zoomed());
        assert_eq!(t.layout(VIEWPORT), vec![(third, VIEWPORT)]);
    }

    #[test]
    fn the_resting_camera_shows_the_resting_layout() {
        for n in 1..=9 {
            let mut t = tree_with(n);
            for _ in 0..2 {
                assert_view_matches_layout(&t);
                t.zoom_toggle();
            }
        }
    }

    #[test]
    fn a_zoomed_tab_keeps_its_hidden_panes_sized_to_their_slots() {
        let mut t = tree_with(4);
        let tiled = t.layout(VIEWPORT);
        assert_eq!(t.resting_layout(VIEWPORT), tiled);

        let zoomed = t.panes()[1];
        t.set_focus(zoomed);
        t.zoom_toggle();
        for ((id, rect), (slot_id, slot)) in t.resting_layout(VIEWPORT).iter().zip(&tiled) {
            assert_eq!(id, slot_id);
            assert_eq!(*rect, if *id == zoomed { VIEWPORT } else { *slot });
        }
    }

    #[test]
    fn framing_a_pane_fills_the_viewport_with_it_alone() {
        let t = tree_with(6);
        for id in t.panes() {
            let view = t.view(VIEWPORT, t.framing(id));
            assert_eq!(view.len(), 1, "{view:?}");
            assert_eq!(view[0].0, id);
            assert!(approx_rect(view[0].1, VIEWPORT), "{view:?}");
        }
        assert_eq!(t.framing(PaneId(99)), Camera::OVERVIEW);
        assert!((Camera::OVERVIEW.zoom() - 1.0).abs() < 1e-12);
        let half = tree_with(2).framing(t.panes()[0]);
        assert!(
            (half.zoom() - 2.0_f64.sqrt()).abs() < 1e-9,
            "a half-width pane"
        );
    }

    #[test]
    fn a_camera_move_slides_each_pane_in_a_straight_line() {
        let t = tree_with(5);
        let focused = t.focused();
        let to = t.framing(focused);
        assert_eq!(Camera::OVERVIEW.lerp(to, -1.0), Camera::OVERVIEW);
        assert_eq!(Camera::OVERVIEW.lerp(to, 2.0), to);

        let start = rect_of(&t, focused);
        let half = t
            .view(VIEWPORT, Camera::OVERVIEW.lerp(to, 0.5))
            .into_iter()
            .find(|(p, _)| *p == focused)
            .map(|(_, r)| r)
            .expect("the zooming pane stays in view");
        assert!(approx_rect(
            half,
            Rect::new(
                f32::midpoint(start.x, VIEWPORT.x),
                f32::midpoint(start.y, VIEWPORT.y),
                f32::midpoint(start.w, VIEWPORT.w),
                f32::midpoint(start.h, VIEWPORT.h),
            )
        ));
    }

    #[test]
    fn framing_a_pane_deep_in_a_huge_spiral_stays_finite() {
        let t = tree_with(2000);
        let last = *t.panes().last().expect("panes");
        for t_ in [0.0, 0.5, 1.0] {
            for (_, r) in t.view(VIEWPORT, Camera::OVERVIEW.lerp(t.framing(last), t_)) {
                assert!(
                    r.x.is_finite() && r.y.is_finite() && r.w.is_finite() && r.h.is_finite(),
                    "{r:?}"
                );
            }
        }
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

        assert_view_matches_layout(t);
        let resting: Vec<PaneId> = t
            .resting_layout(VIEWPORT)
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(
            resting, panes,
            "every pane rests somewhere, in spiral order"
        );
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

    fn assert_view_matches_layout(t: &PaneTree) {
        let view = t.view(VIEWPORT, t.camera());
        let layout = t.layout(VIEWPORT);
        assert_eq!(view.len(), layout.len(), "{view:?} != {layout:?}");
        for ((vid, vr), (lid, lr)) in view.iter().zip(&layout) {
            assert_eq!(vid, lid);
            assert!(approx_rect(*vr, *lr), "{vr:?} != {lr:?}");
        }
    }

    fn clipped_area(r: Rect) -> f32 {
        let w = (r.x + r.w).min(VIEWPORT.x + VIEWPORT.w) - r.x.max(VIEWPORT.x);
        let h = (r.y + r.h).min(VIEWPORT.y + VIEWPORT.h) - r.y.max(VIEWPORT.y);
        w.max(0.0) * h.max(0.0)
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(400))]

        /// Every step of a camera move between two resting views (the overview or any pane
        /// framed) still covers the viewport exactly, so a zoom animation never shows a gap
        /// or a doubled pane.
        #[test]
        fn a_camera_between_resting_views_still_tiles_the_viewport(
            n in 1_usize..24,
            from in proptest::prelude::any::<u8>(),
            to in proptest::prelude::any::<u8>(),
            step in 0.0_f32..=1.0,
        ) {
            let t = tree_with(n);
            let panes = t.panes();
            let resting = |i: u8| {
                let i = usize::from(i) % (panes.len() + 1);
                panes.get(i).map_or(Camera::OVERVIEW, |&id| t.framing(id))
            };
            let view = t.view(VIEWPORT, resting(from).lerp(resting(to), step));
            proptest::prop_assert!(!view.is_empty());
            for i in 0..view.len() {
                for j in (i + 1)..view.len() {
                    proptest::prop_assert!(!overlaps(view[i].1, view[j].1), "{view:?}");
                }
            }
            let area: f32 = view.iter().map(|(_, r)| clipped_area(*r)).sum();
            let vp_area = VIEWPORT.w * VIEWPORT.h;
            proptest::prop_assert!((area - vp_area).abs() <= 1e-2 * vp_area, "{area} != {vp_area}");
        }

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
