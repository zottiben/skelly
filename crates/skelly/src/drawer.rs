//! The sidebar's drawer motion (design/README.md, 2026-11-12): showing or hiding the sidebar
//! slides it in or out at the window's left edge, and switching between the full panel and the
//! slim rail - or hovering the rail open - moves its edge, uncovering or covering the full
//! panel in place. The panes follow the edge on screen, so nothing jumps, but each shell is
//! re-fitted once, to where the panes come to rest: a terminal never reflows frame by frame.
//! An edge that opens eases out (decelerate) and one that closes eases in (accelerate), §03.
//! Between the rail and the full panel the rail's content fades out near the rail's width and
//! the full panel's fades in beyond it, never both at once, so the move starts and ends on the
//! rail as it shows at rest.
//!
//! Pure, like the swoosh: the binary notes where the sidebar is shown before a change
//! ([`Spot`]) and what it shows on the way ([`Look`]), starts a [`Drawer`], and on each frame
//! reads the spot it has reached and fits the sidebar's paint to it.

use std::time::{Duration, Instant};

use skelly_render::{ProseLabel, Srgb};

use crate::motion::{self, Anim, Bezier};
use crate::sidebar::Paint;

/// How far from the rail's width toward the full panel's (`0..1`) the rail has faded out.
const RAIL_GONE: f32 = 0.2;
/// How far the full panel has to come before it starts to show, and before it fully shows: not
/// before the rail has gone.
const FULL_SHOWS: (f32, f32) = (RAIL_GONE, 0.6);

/// Where the sidebar is shown, physical px.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Spot {
    /// The panel's right edge: its painted width, `0` when hidden.
    pub(crate) edge: f32,
    /// The strip it keeps from the panes: the rail's width while the rail is hovered open over
    /// them.
    pub(crate) footprint: f32,
    /// The pane viewport's top, lower while a narrow sidebar leaves the traffic lights over the
    /// panes.
    pub(crate) top: f32,
}

impl Spot {
    /// The spot `t` of the way from this one to `to`.
    fn toward(self, to: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            edge: mix(self.edge, to.edge),
            footprint: mix(self.footprint, to.footprint),
            top: mix(self.top, to.top),
        }
    }
}

/// What the sidebar shows at rest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    Hidden,
    Rail,
    Full,
}

/// What the panel shows while its edge moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Look {
    /// Whether it shows the rail; else the full panel.
    pub(crate) rail: bool,
    /// Whether its content slides with the edge (showing or hiding it); else the edge uncovers
    /// it in place (the rail opening to the full panel, or closing to it).
    pub(crate) slides: bool,
    /// Whether the full panel cross-fades with the rail as the edge nears the rail's width (a
    /// move to or from the rail).
    pub(crate) fades: bool,
}

impl Look {
    /// The look of a move from `from` to `to`, where `prev` is the look of a move still in
    /// flight. The full panel shows if either end shows it, so the rail is never stretched.
    pub(crate) fn between(prev: Option<Self>, from: Mode, to: Mode) -> Self {
        let full_before = prev.map_or(from == Mode::Full, |look| !look.rail);
        let slides_before = prev.map_or(from == Mode::Hidden, |look| look.slides);
        let fades_before = prev.map_or(from == Mode::Rail, |look| look.fades);
        let rail = !(full_before || to == Mode::Full);
        Self {
            rail,
            slides: slides_before || to == Mode::Hidden,
            fades: !rail && (fades_before || to == Mode::Rail),
        }
    }
}

/// A move of the sidebar's edge in flight.
pub(crate) struct Drawer {
    anim: Anim,
    curve: Bezier,
    from: Spot,
    look: Look,
}

impl Drawer {
    /// Move the sidebar from the spot it is shown at, `from`, toward `to` over `duration` from
    /// `now`, showing `look` on the way. The edge eases out when it opens and in when it closes.
    pub(crate) fn start(
        now: Instant,
        duration: Duration,
        from: Spot,
        to: Spot,
        look: Look,
    ) -> Self {
        let curve = if to.edge >= from.edge {
            motion::DECELERATE
        } else {
            motion::ACCELERATE
        };
        Self {
            anim: Anim::start(now, duration),
            curve,
            from,
            look,
        }
    }

    /// What the panel shows on the way.
    pub(crate) fn look(&self) -> Look {
        self.look
    }

    /// Where the sidebar is shown at `now`, on its way to `to`, where it rests now. The rest is
    /// read on each frame, so a window resized mid-move still lands where it should.
    pub(crate) fn spot(&self, now: Instant, to: Spot) -> Spot {
        self.from
            .toward(to, self.curve.ease(self.anim.progress(now)))
    }

    /// Whether the move has come to rest at `now`.
    pub(crate) fn done(&self, now: Instant) -> bool {
        self.anim.done(now)
    }

    /// Fit `paint`, laid out at the look's width, to the `edge` it has reached: slid so its
    /// right edge rides on `edge` (or left in place), cut off at `edge`, with the divider on it.
    pub(crate) fn fit(&self, paint: &mut Paint, edge: f32) {
        let dx = if self.look.slides {
            edge - paint.panel.w
        } else {
            0.0
        };
        for quad in &mut paint.quads {
            quad.rect.x += dx;
        }
        for label in &mut paint.labels {
            label.x += dx;
        }
        if let Some(divider) = paint.quads.last_mut() {
            divider.rect.x = paint.panel.x + edge - divider.rect.w;
        }
        paint.panel.w = edge;
    }

    /// Fade the `rail`'s content into `paint`, the full panel fitted to `edge`, by how near `edge`
    /// is to the rail's width `narrow` from the full panel's `wide`: the rail fades out over the
    /// first stretch, then the full panel fades in, over the sidebar's `bg`. The full panel's fill
    /// and divider stay opaque beneath and above both.
    pub(crate) fn blend(
        &self,
        paint: &mut Paint,
        rail: Paint,
        edge: f32,
        (narrow, wide): (f32, f32),
        bg: Srgb,
    ) {
        let open = ((edge - narrow) / (wide - narrow).max(1.0)).clamp(0.0, 1.0);
        let (from, to) = FULL_SHOWS;
        let full = ((open - from) / (to - from)).clamp(0.0, 1.0);
        let shown = (1.0 - open / RAIL_GONE).max(0.0);
        // A sliding rail rides the edge below its own width.
        let dx = if self.look.slides {
            (edge - narrow).min(0.0)
        } else {
            0.0
        };
        let content = 1..paint.quads.len().saturating_sub(1);
        for quad in &mut paint.quads[content] {
            quad.alpha *= full;
        }
        paint.labels.retain(|_| full > 0.0);
        for label in &mut paint.labels {
            label.color = label.color.over(bg, full);
        }
        let Paint {
            quads: rail_quads,
            labels: rail_labels,
            ..
        } = rail;
        let rail_content = 1..rail_quads.len().saturating_sub(1);
        let at = paint.quads.len().saturating_sub(1);
        paint.quads.splice(
            at..at,
            rail_quads[rail_content].iter().map(|quad| {
                let mut quad = *quad;
                quad.rect.x += dx;
                quad.alpha *= shown;
                quad
            }),
        );
        if shown > 0.0 {
            paint
                .labels
                .extend(rail_labels.into_iter().map(|label| ProseLabel {
                    x: label.x + dx,
                    color: label.color.over(bg, shown),
                    ..label
                }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Drawer, Look, Mode, Spot};
    use crate::motion;
    use crate::sidebar::{self, Paint, View};
    use skelly_render::{
        CapturePane, ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme,
    };
    use std::time::{Duration, Instant};

    /// The scale of the tests and the capture, and the panel widths (physical px).
    const SCALE: f32 = 2.0;
    const FULL: f32 = 240.0 * SCALE;
    const RAIL: f32 = crate::RAIL_WIDTH * SCALE;
    const INK: Srgb = Srgb { r: 0, g: 0, b: 0 };

    fn spot(edge: f32) -> Spot {
        Spot {
            edge,
            footprint: edge,
            top: if edge > 160.0 { 16.0 } else { 76.0 },
        }
    }

    /// A paint `w` wide: the panel fill, a row quad and label at x 20, the divider.
    fn paint(w: f32) -> Paint {
        let rect = |x: f32, w: f32| PxRect {
            x,
            y: 0.0,
            w,
            h: 10.0,
        };
        Paint {
            panel: rect(0.0, w),
            quads: vec![
                ChromeQuad::fill(rect(0.0, w), INK),
                ChromeQuad::fill(rect(20.0, 40.0), INK),
                ChromeQuad::fill(rect(w - 2.0, 2.0), INK),
            ],
            labels: vec![ProseLabel {
                text: "tab".into(),
                x: 20.0,
                y: 0.0,
                role: FontRole::Label,
                color: INK,
                weight: None,
                max_w: 100.0,
            }],
            chip_first: None,
            list_quads: 1..2,
            list_labels: 0..1,
            rows: Vec::new(),
            pill: None,
        }
    }

    #[test]
    fn showing_eases_the_edge_out_and_hiding_eases_it_in() {
        let t0 = Instant::now();
        let look = Look::between(None, Mode::Hidden, Mode::Full);
        let show = Drawer::start(t0, motion::PANEL, spot(0.0), spot(FULL), look);
        assert!(show.spot(t0, spot(FULL)).edge.abs() < 1e-3);
        let mid = show.spot(t0 + motion::PANEL / 2, spot(FULL));
        assert!(
            mid.edge > FULL * 0.5,
            "an opening edge is mostly out by halfway"
        );
        assert!(
            mid.top < 76.0 && mid.top > 16.0,
            "the panes' top moves with it"
        );
        assert_eq!(show.spot(t0 + motion::PANEL, spot(FULL)), spot(FULL));
        assert!(show.done(t0 + motion::PANEL));

        let look = Look::between(None, Mode::Full, Mode::Hidden);
        let hide = Drawer::start(t0, motion::PANEL, spot(FULL), spot(0.0), look);
        let mid = hide.spot(t0 + motion::PANEL / 2, spot(0.0));
        assert!(
            mid.edge > FULL * 0.5,
            "a closing edge has gone less than halfway by then"
        );
        assert!(hide.spot(t0 + motion::PANEL, spot(0.0)).edge.abs() < 1e-3);
    }

    #[test]
    fn the_rest_is_read_on_every_frame() {
        let t0 = Instant::now();
        let look = Look::between(None, Mode::Hidden, Mode::Full);
        let show = Drawer::start(t0, motion::PANEL, spot(0.0), spot(FULL), look);
        // The panel was widened mid-move: it lands at the new width.
        let end = show.spot(t0 + motion::PANEL, spot(FULL + 40.0));
        assert!((end.edge - (FULL + 40.0)).abs() < 1e-3);
    }

    #[test]
    fn the_full_panel_shows_when_either_end_shows_it() {
        let look = |rail, slides, fades| Look {
            rail,
            slides,
            fades,
        };
        assert_eq!(
            Look::between(None, Mode::Hidden, Mode::Full),
            look(false, true, false)
        );
        assert_eq!(
            Look::between(None, Mode::Rail, Mode::Hidden),
            look(true, true, false)
        );
        assert_eq!(
            Look::between(None, Mode::Rail, Mode::Full),
            look(false, false, true)
        );
        assert_eq!(
            Look::between(None, Mode::Full, Mode::Rail),
            look(false, false, true)
        );
        // A move turned around keeps what it showed, so nothing changes on screen.
        let opening = Look::between(None, Mode::Hidden, Mode::Full);
        assert_eq!(
            Look::between(Some(opening), Mode::Full, Mode::Rail),
            look(false, true, true)
        );
    }

    #[test]
    fn the_rail_fades_out_before_the_full_panel_fades_in() {
        let t0 = Instant::now();
        let look = Look::between(None, Mode::Rail, Mode::Full);
        let open = Drawer::start(t0, motion::PANEL, spot(RAIL), spot(FULL), look);
        let blended = |edge: f32| {
            let mut p = paint(FULL);
            open.fit(&mut p, edge);
            let mut rail = paint(RAIL);
            rail.labels[0].text = "rail".into();
            open.blend(&mut p, rail, edge, (RAIL, FULL), Srgb { r: 0, g: 0, b: 0 });
            p
        };
        let at_rail = blended(RAIL);
        let texts: Vec<&str> = at_rail.labels.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["rail"]);
        assert!(
            at_rail.quads[1].alpha.abs() < 1e-3,
            "the full panel's row is hidden"
        );
        assert!(
            (at_rail.quads[2].alpha - 1.0).abs() < 1e-3,
            "the rail's row shows"
        );
        for step in 0..=40_u8 {
            let edge = RAIL + (FULL - RAIL) * f32::from(step) / 40.0;
            let p = blended(edge);
            let both = ["rail", "tab"]
                .iter()
                .all(|text| p.labels.iter().any(|l| l.text == *text));
            assert!(!both, "the rail and the full panel both show at {edge}");
        }
        let out = blended(FULL * 0.9);
        let texts: Vec<&str> = out.labels.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["tab"]);
        // The fill stays first and opaque, and the divider last, on the edge.
        assert!((out.quads[0].alpha - 1.0).abs() < 1e-3);
        let divider = out.quads[out.quads.len() - 1];
        assert!((divider.rect.x + divider.rect.w - FULL * 0.9).abs() < 1e-3);
    }

    #[test]
    fn a_sliding_panel_rides_its_edge_and_an_uncovered_one_stays_put() {
        let t0 = Instant::now();
        let look = Look::between(None, Mode::Hidden, Mode::Full);
        let show = Drawer::start(t0, motion::PANEL, spot(0.0), spot(FULL), look);
        let mut p = paint(FULL);
        show.fit(&mut p, 300.0);
        assert!((p.panel.w - 300.0).abs() < 1e-3, "cut off at the edge");
        assert!(
            (p.labels[0].x - (20.0 - 180.0)).abs() < 1e-3,
            "slid by what is hidden"
        );
        assert!(
            (p.quads[2].rect.x - 298.0).abs() < 1e-3,
            "the divider is on the edge"
        );

        let look = Look::between(None, Mode::Rail, Mode::Full);
        let open = Drawer::start(t0, motion::PANEL, spot(RAIL), spot(FULL), look);
        let mut p = paint(FULL);
        open.fit(&mut p, 300.0);
        assert!((p.labels[0].x - 20.0).abs() < 1e-3, "left in place");
        assert!((p.quads[2].rect.x - 298.0).abs() < 1e-3);
    }

    /// The capture's size (physical px).
    const SIZE: (u32, u32) = (840, 560);

    /// Where a sidebar `edge` wide, keeping `footprint` from the panes, is shown in the capture.
    fn at(edge: f32, footprint: f32) -> Spot {
        let pad = crate::WINDOW_PAD * SCALE;
        Spot {
            edge,
            footprint,
            top: if footprint < crate::TRAFFIC_LIGHT_WIDTH * SCALE {
                pad.max(crate::TITLE_STRIP * SCALE)
            } else {
                pad
            },
        }
    }

    /// The shares of `duration` at which `drawer`'s edge has gone each sixth of the way from
    /// `from` to `to`, wherever its curve puts them.
    fn even_steps(
        drawer: &Drawer,
        t0: Instant,
        duration: Duration,
        from: Spot,
        to: Spot,
    ) -> Vec<f32> {
        let travel = |t: f32| {
            let edge = drawer.spot(t0 + duration.mul_f32(t), to).edge;
            (edge - from.edge) / (to.edge - from.edge)
        };
        (0..7_u8)
            .map(|k| {
                let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
                for _ in 0..24 {
                    let mid = f32::midpoint(lo, hi);
                    if travel(mid) < f32::from(k) / 6.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                hi
            })
            .collect()
    }

    /// The sidebar `drawer` shows with its edge at `edge`, `None` when it is out of sight.
    fn sidebar_at(
        drawer: &Drawer,
        edge: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> Option<Paint> {
        let chips = ['\u{25c6}', '\u{25cf}'];
        let titles = ["nvim", "server", "tests"].map(String::from);
        let h = f32::from(u16::try_from(SIZE.1).unwrap());
        let look = drawer.look();
        let mut build = |rail: bool| {
            let view = View {
                tab_count: titles.len(),
                active_tab: 1,
                chips: &chips,
                active_chip: 0,
                chip_first: 0,
                reveal_chip: false,
                pinned: &[],
                active_pinned: None,
                groups: &[],
                tab_running: &[],
                tab_titles: &titles,
                tab_keys: &[],
                rail,
                top_inset: crate::TITLE_STRIP,
            };
            let w = if rail { RAIL } else { FULL };
            let panel = PxRect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            };
            sidebar::build(&view, panel, SCALE, theme, measure)
        };
        (edge > 0.0).then(|| {
            let mut paint = build(look.rail);
            drawer.fit(&mut paint, edge);
            if look.fades {
                drawer.blend(
                    &mut paint,
                    build(true),
                    edge,
                    (RAIL, FULL),
                    theme.bg_sidebar,
                );
            }
            paint
        })
    }

    /// A pane of a few lines beside a sidebar shown at `spot`.
    fn pane_beside(spot: Spot, theme: &Theme) -> CapturePane {
        let pad = crate::WINDOW_PAD * SCALE;
        let (w, h) = (
            f32::from(u16::try_from(SIZE.0).unwrap()),
            f32::from(u16::try_from(SIZE.1).unwrap()),
        );
        let rect = PxRect {
            x: spot.footprint + pad,
            y: spot.top,
            w: w - spot.footprint - 2.0 * pad,
            h: h - spot.top - pad,
        };
        crate::capture::capture_pane(rect, SCALE, theme)
    }

    /// Frames of the sidebar showing, the full panel closing to the rail, and the rail hovered
    /// open over the panes, in each Ossein theme, with a pane beside the sidebar.
    #[test]
    #[ignore = "manual GPU capture: writes /tmp/skelly-drawer-<move>-<theme>.png"]
    fn capture_drawer() {
        let moves = [
            (
                "show",
                Mode::Hidden,
                at(0.0, 0.0),
                Mode::Full,
                at(FULL, FULL),
                motion::PANEL,
            ),
            (
                "rail",
                Mode::Full,
                at(FULL, FULL),
                Mode::Rail,
                at(RAIL, RAIL),
                motion::PANEL,
            ),
            (
                "hover",
                Mode::Rail,
                at(RAIL, RAIL),
                Mode::Full,
                at(FULL, RAIL),
                motion::BASE,
            ),
        ];
        for name in ["ossein-dark", "ossein-light"] {
            let theme = Theme::resolve(name);
            let mut measure = TextMeasure::new(SCALE);
            for (label, was, from, is, to, duration) in moves {
                let t0 = Instant::now();
                let drawer = Drawer::start(t0, duration, from, to, Look::between(None, was, is));
                let frames = even_steps(&drawer, t0, duration, from, to)
                    .into_iter()
                    .map(|t| {
                        let spot = drawer.spot(t0 + duration.mul_f32(t), to);
                        let paint = sidebar_at(&drawer, spot.edge, &theme, &mut measure);
                        (paint, vec![pane_beside(spot, &theme)])
                    })
                    .collect();
                let path = format!("/tmp/skelly-drawer-{label}-{name}.png");
                crate::capture::capture_strip(&path, name, SIZE, frames);
            }
        }
    }
}
