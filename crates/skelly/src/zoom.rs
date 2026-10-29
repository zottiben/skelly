//! Pane zoom motion (ADR-0011): a tab's camera glides between its resting views, and a
//! trackpad pinch drags it by hand.
//!
//! A tab rests on [`PaneTree::camera`] - the whole spiral, or the focused pane filling the
//! viewport when zoomed. A [`Motion`] is the camera on its way there: a [`Motion::Glide`] eases
//! from wherever the view was shown when the tree changed, and a [`Motion::Pinch`] follows the
//! fingers until it is released, then glides to the view that fits best. The tree itself only
//! changes on commit, so a pinch the user lets go of springs back without touching a terminal.

use std::time::Instant;

use skelly_pane::{Camera, PaneId, PaneTree};

use crate::motion::{self, Anim, Bezier};

/// The finger scale that carries a pinch the whole way to a pane far smaller than the viewport.
/// Panes needing less magnification than this track the fingers one to one.
const PINCH_SPAN: f64 = 2.0;

/// The pinch progress past which a release settles on the far view.
const PINCH_COMMIT: f32 = 0.5;

/// The pinch progress past which a release still moving toward the far view settles there.
const PINCH_FLICK: f32 = 0.1;

/// What a tab shows: where its camera looks, and how strongly the pane dividers and focus ring
/// show - opaque over the spiral, gone with one pane filling the view.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Shot {
    pub(crate) camera: Camera,
    pub(crate) dividers: f32,
}

impl Shot {
    /// The whole spiral.
    const OVERVIEW: Shot = Shot {
        camera: Camera::OVERVIEW,
        dividers: 1.0,
    };

    /// The view `tree` rests on.
    pub(crate) fn resting(tree: &PaneTree) -> Self {
        if tree.is_zoomed() {
            Self::filling(tree, tree.focused())
        } else {
            Self::OVERVIEW
        }
    }

    /// Pane `id` filling the viewport.
    fn filling(tree: &PaneTree, id: PaneId) -> Self {
        Self {
            camera: tree.framing(id),
            dividers: 0.0,
        }
    }

    fn lerp(self, to: Shot, t: f32) -> Shot {
        let t = t.clamp(0.0, 1.0);
        Shot {
            camera: self.camera.lerp(to.camera, t),
            dividers: self.dividers + (to.dividers - self.dividers) * t,
        }
    }
}

/// A tab's camera on the move. A tab at rest has none.
#[derive(Clone, Copy)]
pub(crate) enum Motion {
    /// Easing from `from` to the tab's resting view.
    Glide {
        from: Shot,
        anim: Anim,
        curve: Bezier,
    },
    /// Following a pinch.
    Pinch(Pinch),
}

impl Motion {
    /// A glide from `from` to rest over the guide's pane duration (design §03).
    fn glide(from: Shot, now: Instant) -> Self {
        Self::Glide {
            from,
            anim: Anim::start(now, motion::SLOW),
            curve: motion::STANDARD,
        }
    }

    /// The glide after a pinch is let go at `from`, with `remaining` of the way left: quicker
    /// the less is left, and decelerating as the fingers hand over to the motion.
    fn release(from: Shot, remaining: f32, now: Instant) -> Self {
        let duration = motion::SLOW
            .mul_f32(remaining.clamp(0.0, 1.0))
            .max(motion::FAST);
        Self::Glide {
            from,
            anim: Anim::start(now, duration),
            curve: motion::DECELERATE,
        }
    }

    /// What the tab shows at `now`, given the view it rests on.
    pub(crate) fn shot(&self, rest: Shot, now: Instant) -> Shot {
        match self {
            Self::Glide { from, anim, curve } => from.lerp(rest, curve.ease(anim.progress(now))),
            Self::Pinch(pinch) => pinch.shot(),
        }
    }

    /// Whether this is a glide still under way at `now`, so the loop must keep drawing frames.
    /// A pinch draws on its own events instead.
    pub(crate) fn gliding(&self, now: Instant) -> bool {
        matches!(self, Self::Glide { anim, .. } if !anim.done(now))
    }

    /// Whether this is a glide that has reached its resting view by `now`.
    pub(crate) fn settled(&self, now: Instant) -> bool {
        matches!(self, Self::Glide { anim, .. } if anim.done(now))
    }
}

/// What a tab with `tree`, moving by `motion`, shows at `now`.
pub(crate) fn shown(tree: &PaneTree, motion: Option<Motion>, now: Instant) -> Shot {
    let rest = Shot::resting(tree);
    motion.map_or(rest, |m| m.shot(rest, now))
}

/// A tab's pane tree just before a change, with what it showed then.
#[derive(Clone, Copy)]
pub(crate) struct Before {
    shown: Shot,
    rest: Shot,
    count: usize,
    focused: PaneId,
    zoomed: bool,
}

impl Before {
    pub(crate) fn capture(tree: &PaneTree, shown: Shot) -> Self {
        Self {
            shown,
            rest: Shot::resting(tree),
            count: tree.count(),
            focused: tree.focused(),
            zoomed: tree.is_zoomed(),
        }
    }
}

/// The motion for `tree` after a change from `before`, which was moving by `current`. A new
/// resting view glides in from what was shown. When the pane count changed, what was shown
/// belongs to the old spiral: a zoomed tab glides out from the pane that filled the view (or
/// the pane taking its slot), and an unzoomed one re-tiles at once.
pub(crate) fn after_change(
    tree: &PaneTree,
    before: Before,
    current: Option<Motion>,
    now: Instant,
) -> Option<Motion> {
    let rest = Shot::resting(tree);
    let from = if tree.count() == before.count {
        if rest == before.rest {
            return current;
        }
        before.shown
    } else if before.zoomed {
        let anchor = if tree.contains(before.focused) {
            before.focused
        } else {
            tree.focused()
        };
        Shot::filling(tree, anchor)
    } else {
        return None;
    };
    (from != rest).then(|| Motion::glide(from, now))
}

/// A pinch in progress: zooming into a pane from the spiral, or out of the zoomed pane.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pinch {
    from: Shot,
    to: Shot,
    target: PaneId,
    zoom_in: bool,
    /// The log of the finger scale that completes the pinch.
    span: f64,
    /// The fingers' accumulated scale, `1` when they touched down.
    scale: f64,
    progress: f32,
    /// The progress the latest finger movement made, signed toward `to`.
    step: f32,
}

impl Pinch {
    /// Begin a pinch on `tree`, which shows `from`: from the spiral into `under` (the pane
    /// beneath the fingers, else the focused one), or from the zoomed pane out to the spiral.
    /// `None` for a lone pane, which has nowhere to zoom.
    pub(crate) fn begin(tree: &PaneTree, under: Option<PaneId>, from: Shot) -> Option<Self> {
        let zoom_in = !tree.is_zoomed();
        let target = if zoom_in {
            under
                .filter(|&id| tree.contains(id))
                .unwrap_or(tree.focused())
        } else {
            tree.focused()
        };
        let to = if zoom_in {
            Shot::filling(tree, target)
        } else {
            Shot::OVERVIEW
        };
        let magnification = tree.framing(target).zoom();
        (magnification > 1.0 + 1e-6).then(|| Self {
            from,
            to,
            target,
            zoom_in,
            span: magnification.min(PINCH_SPAN).ln(),
            scale: 1.0,
            progress: 0.0,
            step: 0.0,
        })
    }

    /// Follow the fingers by a magnification `delta` (positive spreads them, zooming in).
    #[allow(
        clippy::cast_possible_truncation,
        reason = "progress is a 0..1 fraction; f32 is ample"
    )]
    pub(crate) fn update(&mut self, delta: f64) {
        if !delta.is_finite() {
            return;
        }
        self.scale = (self.scale + delta).max(0.05);
        let pulled = if self.zoom_in {
            self.scale.ln()
        } else {
            -self.scale.ln()
        };
        let progress = (pulled / self.span).clamp(0.0, 1.0) as f32;
        let step = progress - self.progress;
        if step.abs() > f32::EPSILON {
            self.step = step;
        }
        self.progress = progress;
    }

    /// What the pinch shows.
    pub(crate) fn shot(&self) -> Shot {
        self.from.lerp(self.to, self.progress)
    }

    /// Whether letting go now settles on the far view: past halfway, or past a nudge with the
    /// fingers still heading there.
    fn commits(&self) -> bool {
        self.progress >= PINCH_COMMIT || (self.progress > PINCH_FLICK && self.step > 0.0)
    }

    /// Let go of the pinch (`ended`; a cancelled pinch never commits): apply the view it
    /// settles on to `tree` and return the glide there, plus whether `tree` changed.
    pub(crate) fn release(self, tree: &mut PaneTree, ended: bool, now: Instant) -> (Motion, bool) {
        let commit = ended && self.commits();
        if commit {
            if self.zoom_in {
                tree.set_focus(self.target);
            }
            if tree.is_zoomed() != self.zoom_in {
                tree.zoom_toggle();
            }
        }
        let remaining = if commit {
            1.0 - self.progress
        } else {
            self.progress
        };
        (Motion::release(self.shot(), remaining, now), commit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_with(count: usize) -> PaneTree {
        let mut tree = PaneTree::new();
        for _ in 1..count {
            tree.open();
        }
        tree
    }

    #[test]
    fn a_zoom_glides_from_the_spiral_and_lands_exactly_on_the_pane() {
        let mut tree = tree_with(3);
        let t0 = Instant::now();
        let before = Before::capture(&tree, Shot::resting(&tree));
        tree.zoom_toggle();
        let glide = after_change(&tree, before, None, t0).expect("zooming glides");

        assert_eq!(shown(&tree, Some(glide), t0), Shot::OVERVIEW);
        let mid = shown(&tree, Some(glide), t0 + motion::SLOW / 2);
        assert!(mid.dividers > 0.0 && mid.dividers < 1.0, "{mid:?}");
        assert!(glide.gliding(t0 + motion::SLOW / 2));
        assert!(!glide.gliding(t0 + motion::SLOW));
        assert_eq!(
            shown(&tree, Some(glide), t0 + motion::SLOW),
            Shot::resting(&tree)
        );
    }

    #[test]
    fn a_change_that_keeps_the_resting_view_keeps_the_motion() {
        let mut tree = tree_with(3);
        let t0 = Instant::now();
        let before = Before::capture(&tree, Shot::resting(&tree));
        tree.zoom_toggle();
        let glide = after_change(&tree, before, None, t0);

        let mid = shown(&tree, glide, t0 + motion::SLOW / 2);
        let before = Before::capture(&tree, mid);
        let first = tree.panes()[0];
        tree.set_focus(tree.focused());
        assert!(
            after_change(&tree, before, glide, t0 + motion::SLOW / 2)
                .is_some_and(|m| m.shot(Shot::resting(&tree), t0) == Shot::OVERVIEW),
            "the running glide carries on"
        );

        let before = Before::capture(&tree, mid);
        tree.set_focus(first);
        let pan = after_change(&tree, before, glide, t0 + motion::SLOW / 2)
            .expect("moving focus while zoomed pans");
        assert_eq!(
            pan.shot(Shot::resting(&tree), t0 + motion::SLOW / 2),
            mid,
            "the pan starts where the view was, without a jump"
        );
    }

    #[test]
    fn opening_a_pane_while_zoomed_glides_out_from_the_zoomed_pane() {
        let mut tree = tree_with(2);
        let zoomed = tree.focused();
        tree.zoom_toggle();
        let t0 = Instant::now();
        let before = Before::capture(&tree, Shot::resting(&tree));
        tree.open();
        let glide = after_change(&tree, before, None, t0).expect("glides out");
        assert_eq!(
            glide.shot(Shot::resting(&tree), t0),
            Shot::filling(&tree, zoomed)
        );
        assert_eq!(Shot::resting(&tree), Shot::OVERVIEW);
    }

    #[test]
    fn a_new_pane_in_the_spiral_re_tiles_at_once() {
        let mut tree = tree_with(2);
        let before = Before::capture(&tree, Shot::resting(&tree));
        tree.open();
        assert!(after_change(&tree, before, None, Instant::now()).is_none());
    }

    #[test]
    fn a_lone_pane_has_nothing_to_pinch() {
        let tree = tree_with(1);
        assert!(Pinch::begin(&tree, None, Shot::resting(&tree)).is_none());
    }

    #[test]
    fn a_pinch_tracks_the_fingers_and_commits_past_halfway() {
        let mut tree = tree_with(4);
        let under = tree.panes()[2];
        let mut pinch = Pinch::begin(&tree, Some(under), Shot::OVERVIEW).expect("pinch");
        pinch.update(-0.2);
        assert_eq!(
            pinch.shot(),
            Shot::OVERVIEW,
            "pinching in at rest goes nowhere"
        );

        pinch.update(0.2);
        pinch.update(0.6);
        let shot = pinch.shot();
        assert!(shot.dividers < 0.5, "{shot:?}");
        let t0 = Instant::now();
        let (glide, changed) = pinch.release(&mut tree, true, t0);
        assert!(changed);
        assert!(tree.is_zoomed());
        assert_eq!(
            tree.focused(),
            under,
            "the pane under the fingers takes focus"
        );
        assert_eq!(
            glide.shot(Shot::resting(&tree), t0),
            shot,
            "no jump on release"
        );
        assert!(!glide.gliding(t0 + motion::SLOW));
        assert!(glide.settled(t0 + motion::SLOW));
    }

    #[test]
    fn a_pinch_during_a_glide_starts_from_what_is_shown() {
        let mut tree = tree_with(3);
        let t0 = Instant::now();
        let before = Before::capture(&tree, Shot::resting(&tree));
        tree.zoom_toggle();
        let glide = after_change(&tree, before, None, t0);
        let mid = shown(&tree, glide, t0 + motion::SLOW / 4);

        let pinch = Pinch::begin(&tree, None, mid).expect("pinch out of the zoom");
        assert_eq!(pinch.shot(), mid, "no jump when the fingers land");
    }

    #[test]
    fn a_short_pinch_springs_back_unless_it_is_still_heading_there() {
        let mut tree = tree_with(3);
        let mut pinch = Pinch::begin(&tree, None, Shot::OVERVIEW).expect("pinch");
        pinch.update(0.3);
        pinch.update(-0.15);
        let (_, changed) = pinch.release(&mut tree, true, Instant::now());
        assert!(!changed, "pulled back below halfway");
        assert!(!tree.is_zoomed());

        let mut flick = Pinch::begin(&tree, None, Shot::OVERVIEW).expect("pinch");
        flick.update(0.15);
        let (_, changed) = flick.release(&mut tree, true, Instant::now());
        assert!(changed, "a flick toward the pane carries through");
        assert!(tree.is_zoomed());

        let mut out =
            Pinch::begin(&tree, None, Shot::resting(&tree)).expect("pinch out of the zoom");
        out.update(-0.9);
        let (_, changed) = out.release(&mut tree, false, Instant::now());
        assert!(!changed, "a cancelled pinch never commits");
        assert!(tree.is_zoomed());
        out.update(f64::NAN);
        let (_, changed) = out.release(&mut tree, true, Instant::now());
        assert!(changed);
        assert!(!tree.is_zoomed(), "pinching in zooms back out");
    }
}
