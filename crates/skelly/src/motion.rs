//! Motion tokens + easing (design §03 "Motion") and a tiny time-driven animation clock.
//!
//! The guide's motion system is a small set of named durations and three cubic-bezier
//! easing curves; the summary is "splits, tab switches, timeline scrubs animate under
//! 200ms", enter/expand *decelerate*, exit/collapse *accelerate*. This module is the pure
//! part: the tokens, a CSS-style cubic-bezier evaluator, and an [`Anim`] that turns a start
//! `Instant` + a duration into an eased 0..1 progress the render reads each frame. The
//! binary owns *which* transitions animate and drives the redraw loop (`ControlFlow::Poll`
//! while any animation is live); kept here so the curves are unit-testable without a window.

use std::time::{Duration, Instant};

/// Fast: hover / press / tooltip (design §03).
#[allow(
    dead_code,
    reason = "token table; wired per transition as the motion slices land"
)]
pub(crate) const FAST: Duration = Duration::from_millis(120);
/// Base: tab switch, menu / palette open.
pub(crate) const BASE: Duration = Duration::from_millis(180);
/// Slow: pane split / close.
#[allow(
    dead_code,
    reason = "token table; wired per transition as the motion slices land"
)]
pub(crate) const SLOW: Duration = Duration::from_millis(260);
/// Panel: sidebar / dock slide.
#[allow(
    dead_code,
    reason = "token table; wired per transition as the motion slices land"
)]
pub(crate) const PANEL: Duration = Duration::from_millis(320);

/// Decelerate `cubic-bezier(0,0,0,1)` - enter / expand (fast in, eases to rest).
pub(crate) const DECELERATE: Bezier = Bezier::new(0.0, 0.0, 0.0, 1.0);
/// Standard `cubic-bezier(.2,0,0,1)` - most transitions.
#[allow(
    dead_code,
    reason = "token table; wired per transition as the motion slices land"
)]
pub(crate) const STANDARD: Bezier = Bezier::new(0.2, 0.0, 0.0, 1.0);
/// Accelerate `cubic-bezier(.3,0,1,1)` - exit / collapse (eases in, fast out).
#[allow(
    dead_code,
    reason = "token table; wired per transition as the motion slices land"
)]
pub(crate) const ACCELERATE: Bezier = Bezier::new(0.3, 0.0, 1.0, 1.0);

/// A CSS-style cubic-bezier easing: control points `(x1,y1)` and `(x2,y2)` with the
/// endpoints fixed at `(0,0)` and `(1,1)`. Maps linear progress `x` in `[0,1]` to an eased
/// output `y`.
#[derive(Clone, Copy)]
pub(crate) struct Bezier {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

impl Bezier {
    const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    /// The eased output for linear input `x` in `[0,1]`. Solves the parametric curve's
    /// `x(t) == x` for `t` by bisection (`x(t)` is monotone for the guide's control points),
    /// then returns `y(t)`. Endpoints map exactly: `ease(0) == 0`, `ease(1) == 1`.
    pub(crate) fn ease(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        // One coordinate of the cubic with fixed 0 / 1 endpoints:
        // B(t) = 3(1-t)^2 t · c1 + 3(1-t) t^2 · c2 + t^3.
        let bezier = |c1: f32, c2: f32, t: f32| {
            let u = 1.0 - t;
            3.0 * u * u * t * c1 + 3.0 * u * t * t * c2 + t * t * t
        };
        let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if bezier(self.x1, self.x2, mid) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        bezier(self.y1, self.y2, 0.5 * (lo + hi))
    }
}

/// A one-shot animation: a start instant plus a duration. [`progress`](Anim::progress) is
/// linear `0..1` (clamped); callers apply an easing. [`done`](Anim::done) once the duration
/// has elapsed. Time is passed in (`now`) so the whole thing stays testable and the redraw
/// loop can read one consistent instant per frame.
#[derive(Clone, Copy)]
pub(crate) struct Anim {
    start: Instant,
    duration: Duration,
}

impl Anim {
    /// Begin an animation at `now` running for `duration`.
    pub(crate) fn start(now: Instant, duration: Duration) -> Self {
        Self {
            start: now,
            duration,
        }
    }

    /// Linear progress in `0..=1` at `now` (clamped; `1.0` for a zero-length duration).
    pub(crate) fn progress(self, now: Instant) -> f32 {
        let secs = self.duration.as_secs_f32();
        if secs <= 0.0 {
            return 1.0;
        }
        (now.saturating_duration_since(self.start).as_secs_f32() / secs).clamp(0.0, 1.0)
    }

    /// Whether `now` is at or past the animation's end.
    pub(crate) fn done(self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::{Anim, ACCELERATE, BASE, DECELERATE};
    use std::time::{Duration, Instant};

    #[test]
    fn easings_pin_their_endpoints() {
        for curve in [DECELERATE, ACCELERATE, super::STANDARD] {
            assert!(curve.ease(0.0).abs() < 1e-3, "ease(0) == 0");
            assert!((curve.ease(1.0) - 1.0).abs() < 1e-3, "ease(1) == 1");
        }
        // Out-of-range inputs clamp to the endpoints.
        assert!(DECELERATE.ease(-1.0).abs() < 1e-3);
        assert!((DECELERATE.ease(2.0) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn easings_are_monotonic_and_shaped() {
        // Monotonic non-decreasing across the unit interval.
        let mut prev = 0.0;
        for i in 0..=20u8 {
            let y = DECELERATE.ease(f32::from(i) / 20.0);
            assert!(y + 1e-4 >= prev, "decelerate is monotonic");
            prev = y;
        }
        // Decelerate is front-loaded (past the midpoint by x=0.5); accelerate lags behind it.
        assert!(
            DECELERATE.ease(0.5) > 0.5,
            "decelerate leads at the midpoint"
        );
        assert!(
            ACCELERATE.ease(0.5) < DECELERATE.ease(0.5),
            "accelerate lags decelerate"
        );
    }

    #[test]
    fn anim_progress_runs_from_zero_to_one_and_reports_done() {
        // Deterministic: a captured base instant plus exact offsets - no sleeping / real time.
        let t0 = Instant::now();
        let anim = Anim::start(t0, BASE);
        assert!(anim.progress(t0).abs() < 1e-3, "starts at 0");
        assert!(!anim.done(t0));
        assert!((anim.progress(t0 + BASE / 2) - 0.5).abs() < 1e-3, "halfway");
        assert!((anim.progress(t0 + BASE) - 1.0).abs() < 1e-3, "ends at 1");
        assert!(anim.done(t0 + BASE));
        // Past the end clamps to 1 and stays done.
        assert!((anim.progress(t0 + BASE + Duration::from_millis(50)) - 1.0).abs() < 1e-3);
        assert!(anim.done(t0 + BASE + Duration::from_millis(50)));
    }
}
