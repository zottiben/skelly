//! The right dock's slide (design/README.md, 2026-11-12): opening the git dock or the session
//! timeline slides it in over the panes from the window's right edge, and closing it slides it
//! back out, over `PANEL` (§03, "Sidebar, timeline slide"), easing out as it opens and in as it
//! closes. The panes follow its edge on screen, but each shell is re-fitted once, to where the
//! panes come to rest, as beside the sidebar's drawer. A dock that closes slides out as it was
//! last painted.
//!
//! Pure, like the drawer: the binary notes where the dock's edge is shown before a change
//! ([`Edge`]), starts a [`Slide`] (with the paint of a dock that closed), and on each frame reads
//! the edge it has reached and shifts the dock's paint to it.

use std::time::{Duration, Instant};

use skelly_render::{ChromeQuad, ProseLabel, PxRect};

use crate::motion::{self, Anim, Bezier};

/// Where the right dock is shown, physical px from the window's right edge.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Edge {
    /// How far in the dock's left edge is: its width, or the window's less the sidebar's while it
    /// spans the window; `0` when no dock is open.
    pub(crate) inset: f32,
    /// The strip it keeps from the panes: its width beside them, even while it spans the window
    /// over them.
    pub(crate) reserve: f32,
}

impl Edge {
    /// The edge `t` of the way from this one to `to`.
    fn toward(self, to: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            inset: mix(self.inset, to.inset),
            reserve: mix(self.reserve, to.reserve),
        }
    }
}

/// Which right dock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Dock {
    Git,
    Timeline,
}

/// A right dock's paint, owned: what the renderer's dock view borrows.
#[derive(Clone)]
pub(crate) struct Frame {
    /// The panel the frame's fill, shadow and divider are drawn on.
    pub(crate) panel: PxRect,
    /// The rect the labels clip to (the panel widened left for the edge-straddling toggle).
    pub(crate) clip: PxRect,
    pub(crate) quads: Vec<ChromeQuad>,
    pub(crate) labels: Vec<ProseLabel>,
}

impl Frame {
    /// Move the whole frame `dx` px to the right.
    pub(crate) fn shift(&mut self, dx: f32) {
        self.panel.x += dx;
        self.clip.x += dx;
        for quad in &mut self.quads {
            quad.rect.x += dx;
        }
        for label in &mut self.labels {
            label.x += dx;
        }
    }
}

/// A dock's frame as painted, with the inset its edge was shown at.
pub(crate) struct Painted {
    pub(crate) dock: Dock,
    pub(crate) frame: Frame,
    pub(crate) inset: f32,
}

/// A move of the right dock's edge in flight.
pub(crate) struct Slide {
    anim: Anim,
    curve: Bezier,
    from: Edge,
    /// The dock that closed, as last painted, sliding out.
    leaving: Option<Painted>,
}

impl Slide {
    /// Move the dock's edge from where it is shown, `from`, toward `to` over `duration` from
    /// `now`, sliding out `leaving`, the dock that closed. The edge eases out as it opens and in
    /// as it closes.
    pub(crate) fn start(
        now: Instant,
        duration: Duration,
        from: Edge,
        to: Edge,
        leaving: Option<Painted>,
    ) -> Self {
        let curve = if to.inset >= from.inset {
            motion::DECELERATE
        } else {
            motion::ACCELERATE
        };
        Self {
            anim: Anim::start(now, duration),
            curve,
            from,
            leaving,
        }
    }

    /// Where the dock's edge is shown at `now`, on its way to `to`, where it rests now. The rest
    /// is read on each frame, so a window resized mid-move still lands where it should.
    pub(crate) fn edge(&self, now: Instant, to: Edge) -> Edge {
        self.from
            .toward(to, self.curve.ease(self.anim.progress(now)))
    }

    /// Whether the move has come to rest at `now`.
    pub(crate) fn done(&self, now: Instant) -> bool {
        self.anim.done(now)
    }

    /// The dock that closed, with its frame shifted to an edge shown at `inset`.
    pub(crate) fn leaving(&self, inset: f32) -> Option<(Dock, Frame)> {
        self.leaving.as_ref().map(|painted| {
            let mut frame = painted.frame.clone();
            frame.shift(painted.inset - inset);
            (painted.dock, frame)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use skelly_render::{
        CapturePane, ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme,
    };
    use skelly_session::{Actor, SessionEvent, Timeline};

    use super::{Dock, Edge, Frame, Painted, Slide};
    use crate::capture::{capture_pane, capture_strip, Shot};
    use crate::motion;
    use crate::timeline::TimelineDock;

    const OPEN: Edge = Edge {
        inset: 420.0,
        reserve: 420.0,
    };
    const CLOSED: Edge = Edge {
        inset: 0.0,
        reserve: 0.0,
    };

    fn frame(x: f32) -> Frame {
        let rect = PxRect {
            x,
            y: 0.0,
            w: 420.0,
            h: 600.0,
        };
        Frame {
            panel: rect,
            clip: rect,
            quads: vec![ChromeQuad::fill(rect, Srgb { r: 1, g: 2, b: 3 })],
            labels: vec![ProseLabel {
                text: "Changes".into(),
                x: x + 16.0,
                y: 12.0,
                role: FontRole::Body,
                color: Srgb { r: 4, g: 5, b: 6 },
                weight: None,
                max_w: f32::MAX,
            }],
        }
    }

    #[test]
    fn an_opening_dock_comes_in_from_the_right_and_eases_out() {
        let now = Instant::now();
        let slide = Slide::start(now, motion::PANEL, CLOSED, OPEN, None);
        assert_eq!(slide.edge(now, OPEN), CLOSED);
        let early = slide.edge(now + motion::PANEL / 4, OPEN);
        assert!(early.inset > OPEN.inset * 0.5, "decelerating: {early:?}");
        assert!((early.reserve - early.inset).abs() < f32::EPSILON);
        assert_eq!(slide.edge(now + motion::PANEL, OPEN), OPEN);
        assert!(slide.done(now + motion::PANEL));
    }

    #[test]
    fn a_closing_dock_eases_in_and_slides_out_as_painted() {
        let now = Instant::now();
        let painted = Painted {
            dock: Dock::Timeline,
            frame: frame(580.0),
            inset: OPEN.inset,
        };
        let slide = Slide::start(now, motion::PANEL, OPEN, CLOSED, Some(painted));
        let early = slide.edge(now + motion::PANEL / 4, CLOSED);
        assert!(early.inset > OPEN.inset * 0.5, "accelerating: {early:?}");
        let (dock, frame) = slide
            .leaving(early.inset)
            .expect("the closed dock slides out");
        assert_eq!(dock, Dock::Timeline);
        let dx = OPEN.inset - early.inset;
        assert!((frame.panel.x - (580.0 + dx)).abs() < 1e-3);
        assert!((frame.clip.x - (580.0 + dx)).abs() < 1e-3);
        assert!((frame.quads[0].rect.x - (580.0 + dx)).abs() < 1e-3);
        assert!((frame.labels[0].x - (596.0 + dx)).abs() < 1e-3);
    }

    #[test]
    fn a_dock_turned_back_mid_slide_moves_on_from_where_it_shows() {
        let now = Instant::now();
        let opening = Slide::start(now, motion::PANEL, CLOSED, OPEN, None);
        let later = now + motion::PANEL / 2;
        let shown = opening.edge(later, OPEN);
        let closing = Slide::start(later, motion::PANEL, shown, CLOSED, None);
        assert_eq!(closing.edge(later, CLOSED), shown);
        assert_eq!(closing.edge(later + motion::PANEL, CLOSED), CLOSED);
    }

    /// The capture's DPI scale.
    const SCALE: f32 = 2.0;
    /// The capture's window, physical px.
    const SIZE: (u32, u32) = (1600, 700);

    fn window() -> (f32, f32) {
        (
            f32::from(u16::try_from(SIZE.0).unwrap()),
            f32::from(u16::try_from(SIZE.1).unwrap()),
        )
    }

    /// The timeline dock over a few events, painted at rest at the window's right edge.
    fn timeline_at_rest(theme: &Theme) -> Frame {
        let mut timeline = Timeline::new();
        timeline.record(
            SessionEvent::new(Actor::System, "0:00", "Session started", "main")
                .restoring("aaaaaaa"),
        );
        timeline.record(SessionEvent::new(
            Actor::Human,
            "1:30",
            "Staged 2 files",
            "a.rs, b.rs",
        ));
        timeline.record(
            SessionEvent::new(
                Actor::Human,
                "3:20",
                "git commit - feat: x",
                "bbbbbbb - main",
            )
            .restoring("bbbbbbb"),
        );
        let mut dock = TimelineDock::new();
        dock.snap_to_newest(timeline.len());
        let (w, h) = window();
        let width = crate::GIT_DOCK_WIDTH * SCALE;
        let panel = PxRect {
            x: w - width,
            y: 0.0,
            w: width,
            h,
        };
        let pad = crate::WINDOW_PAD * SCALE;
        let content = PxRect {
            y: pad,
            h: h - pad,
            ..panel
        };
        let paint = dock.build(
            &timeline,
            content,
            SCALE,
            theme,
            &mut TextMeasure::new(SCALE),
        );
        Frame {
            panel,
            clip: panel,
            quads: paint.quads,
            labels: paint.labels,
        }
    }

    /// A pane beside a dock keeping `reserve` px from the panes.
    fn pane_beside(reserve: f32, theme: &Theme) -> CapturePane {
        let (w, h) = window();
        let pad = crate::WINDOW_PAD * SCALE;
        let rect = PxRect {
            x: pad,
            y: pad,
            w: w - reserve - 2.0 * pad,
            h: h - 2.0 * pad,
        };
        capture_pane(rect, SCALE, theme)
    }

    /// Frames of the timeline sliding in, and back out, over a pane that follows its edge, at
    /// 0, 1, 2, 4, 8 and 16 sixteenths of the slide, in each Ossein theme.
    #[test]
    #[ignore = "manual GPU capture: writes /tmp/skelly-dock-<move>-<theme>.png"]
    fn capture_dock_slide() {
        for name in ["ossein-dark", "ossein-light"] {
            let theme = Theme::resolve(name);
            let rest = timeline_at_rest(&theme);
            let open = Edge {
                inset: rest.panel.w,
                reserve: rest.panel.w,
            };
            let now = Instant::now();
            for (move_name, opening) in [("open", true), ("close", false)] {
                let (from, to) = if opening {
                    (CLOSED, open)
                } else {
                    (open, CLOSED)
                };
                let leaving = (!opening).then(|| Painted {
                    dock: Dock::Timeline,
                    frame: rest.clone(),
                    inset: open.inset,
                });
                let slide = Slide::start(now, motion::PANEL, from, to, leaving);
                let frames: Vec<Shot> = [0, 1, 2, 4, 8, 16]
                    .into_iter()
                    .map(|k: u32| {
                        let edge = slide.edge(now + motion::PANEL * k / 16, to);
                        let dock = if opening {
                            let mut frame = rest.clone();
                            frame.shift(to.inset - edge.inset);
                            Some((Dock::Timeline, frame))
                        } else {
                            slide.leaving(edge.inset)
                        };
                        Shot {
                            dock,
                            panes: vec![pane_beside(edge.reserve, &theme)],
                            ..Shot::default()
                        }
                    })
                    .collect();
                let path = format!("/tmp/skelly-dock-{move_name}-{name}.png");
                capture_strip(&path, name, SIZE, frames);
            }
        }
    }
}
