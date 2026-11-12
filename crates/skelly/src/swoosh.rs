//! The sidebar's workspace swoosh (design/README.md, 2026-11-03): on a workspace switch the
//! workspace's own rows - the pinned grid, groups, tabs and new-tab action - slide sideways
//! like a carousel. The rows left behind slide out as the new workspace's slide in from the
//! side of its chip (a later workspace from the right), one panel width apart, easing to rest
//! over the guide's panel-slide duration. The chrome around them stays put.
//!
//! A switch during a slide continues it from where the rows are on screen: the rows left
//! behind keep their places beside the live ones, so a fast run of wheel notches reads as one
//! sweep and turning back slides the same rows home. Pure, like `zoom`: the binary snapshots
//! the rows it leaves, starts a [`Swoosh`] and applies it to each sidebar paint.

use std::time::Instant;

use skelly_render::{ChromeQuad, ProseLabel};

use crate::motion::{self, Anim};
use crate::sidebar::Paint;

/// One workspace's rows as painted at rest.
pub(crate) struct Rows {
    quads: Vec<ChromeQuad>,
    labels: Vec<ProseLabel>,
}

impl Rows {
    /// The workspace rows of a sidebar `paint`.
    pub(crate) fn of(paint: &Paint) -> Self {
        Self {
            quads: paint.quads[paint.list_quads.clone()].to_vec(),
            labels: paint.labels[paint.list_labels.clone()].to_vec(),
        }
    }
}

/// Rows a switch left behind, still sliding beside the live ones.
struct Ghost {
    /// The workspace they show; `None` once it is removed, so no switch slides them back in.
    workspace: Option<usize>,
    /// Their offset from the live rows, physical px.
    dx: f32,
    rows: Rows,
}

/// A workspace switch's slide in flight.
pub(crate) struct Swoosh {
    anim: Anim,
    /// The live rows' offset from rest when the slide started, physical px; it eases to `0`.
    from: f32,
    /// The panel width, physical px: rows outside `(-width, width)` are off the panel.
    width: f32,
    ghosts: Vec<Ghost>,
}

impl Swoosh {
    /// Slide from workspace `leaving`, whose rows at rest are `rows`, to workspace `to` across a
    /// panel `width` px wide at `now`, continuing `prev` if a slide is in flight. The new rows
    /// come in where their own rows still are, else one width past the farthest rows on their
    /// side.
    pub(crate) fn start(
        prev: Option<Self>,
        now: Instant,
        leaving: usize,
        rows: Rows,
        to: usize,
        width: f32,
    ) -> Self {
        let (offset, mut ghosts) = prev.map_or((0.0, Vec::new()), |s| (s.offset(now), s.ghosts));
        let side = if to > leaving { 1.0 } else { -1.0 };
        let reach = if let Some(i) = ghosts.iter().position(|g| g.workspace == Some(to)) {
            ghosts.remove(i).dx
        } else {
            let farthest = ghosts.iter().map(|g| g.dx * side).fold(0.0, f32::max);
            side * (farthest + width)
        };
        let from = offset + reach;
        for ghost in &mut ghosts {
            ghost.dx -= reach;
        }
        ghosts.push(Ghost {
            workspace: Some(leaving),
            dx: -reach,
            rows,
        });
        // Rows that stay off the panel for the whole slide are dropped.
        ghosts.retain(|g| {
            let (start, end) = (from + g.dx, g.dx);
            start.min(end) < width && start.max(end) > -width
        });
        Self {
            anim: Anim::start(now, motion::PANEL),
            from,
            width,
            ghosts,
        }
    }

    /// The live rows' offset from rest at `now`, physical px.
    pub(crate) fn offset(&self, now: Instant) -> f32 {
        self.from * (1.0 - motion::DECELERATE.ease(self.anim.progress(now)))
    }

    /// Whether the slide has come to rest at `now`.
    pub(crate) fn done(&self, now: Instant) -> bool {
        self.anim.done(now)
    }

    /// Workspace `w` was removed: rows of it still sliding show no workspace any more, and the
    /// workspaces after it move down one.
    pub(crate) fn forget(&mut self, w: usize) {
        for ghost in &mut self.ghosts {
            ghost.workspace = match ghost.workspace {
                Some(g) if g == w => None,
                Some(g) if g > w => Some(g - 1),
                kept => kept,
            };
        }
    }

    /// Move the live rows of `paint` to where they are at `now`, with the rows left behind
    /// that are on the panel beside them.
    pub(crate) fn apply(&self, paint: &mut Paint, now: Instant) {
        let offset = self.offset(now);
        for quad in &mut paint.quads[paint.list_quads.clone()] {
            quad.rect.x += offset;
        }
        for label in &mut paint.labels[paint.list_labels.clone()] {
            label.x += offset;
        }
        let mut quads = Vec::new();
        let mut labels = Vec::new();
        for ghost in &self.ghosts {
            let x = offset + ghost.dx;
            if x.abs() < self.width {
                quads.extend(ghost.rows.quads.iter().map(|q| {
                    let mut q = *q;
                    q.rect.x += x;
                    q
                }));
                labels.extend(ghost.rows.labels.iter().map(|l| ProseLabel {
                    x: l.x + x,
                    ..l.clone()
                }));
            }
        }
        let (q, l) = (paint.list_quads.end, paint.list_labels.end);
        paint.list_quads.end += quads.len();
        paint.list_labels.end += labels.len();
        paint.quads.splice(q..q, quads);
        paint.labels.splice(l..l, labels);
    }
}

#[cfg(test)]
mod tests {
    use super::{Rows, Swoosh};
    use crate::motion;
    use crate::sidebar::Paint;
    use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb};
    use std::time::{Duration, Instant};

    const W: f32 = 480.0;
    const INK: Srgb = Srgb { r: 0, g: 0, b: 0 };

    fn label(text: &str) -> ProseLabel {
        ProseLabel {
            text: text.to_owned(),
            x: 20.0,
            y: 100.0,
            role: FontRole::Label,
            color: INK,
            weight: None,
            max_w: 200.0,
        }
    }

    fn quad(x: f32) -> ChromeQuad {
        ChromeQuad::fill(
            PxRect {
                x,
                y: 0.0,
                w: 4.0,
                h: 4.0,
            },
            INK,
        )
    }

    /// A sidebar paint: the panel fill, a list of one quad and one label `text`, the divider.
    fn paint(text: &str) -> Paint {
        Paint {
            panel: PxRect {
                x: 0.0,
                y: 0.0,
                w: W,
                h: 800.0,
            },
            quads: vec![quad(0.0), quad(18.0), quad(W - 1.0)],
            labels: vec![label("chrome"), label(text), label("utility")],
            chip_first: None,
            list_quads: 1..2,
            list_labels: 1..2,
            rows: Vec::new(),
            pill: None,
        }
    }

    fn rows(text: &str) -> Rows {
        Rows::of(&paint(text))
    }

    /// Where each list label of a live paint `text` lands at `now`, sorted by x.
    fn shown(swoosh: &Swoosh, text: &str, now: Instant) -> Vec<(String, f32)> {
        let mut p = paint(text);
        swoosh.apply(&mut p, now);
        let mut list: Vec<(String, f32)> = p.labels[p.list_labels.clone()]
            .iter()
            .map(|l| (l.text.clone(), l.x - 20.0))
            .collect();
        list.sort_by(|a, b| a.1.total_cmp(&b.1));
        list
    }

    #[test]
    fn a_later_workspace_slides_in_from_the_right_and_settles() {
        let t0 = Instant::now();
        let s = Swoosh::start(None, t0, 0, rows("one"), 1, W);
        // At the start the old rows sit at rest and the new ones a width to their right.
        assert_eq!(
            shown(&s, "two", t0),
            vec![("one".into(), 0.0), ("two".into(), W)]
        );
        // Halfway the decelerating slide is mostly done, both rows moving left.
        let mid = shown(&s, "two", t0 + motion::PANEL / 2);
        assert!(mid[0].1 < -W * 0.5 && mid[1].1 < W * 0.5 && mid[1].1 > 0.0);
        assert!((mid[1].1 - mid[0].1 - W).abs() < 1e-3, "one width apart");
        // At rest the old rows are off the panel and the new ones are home.
        let end = t0 + motion::PANEL;
        assert!(s.done(end));
        assert_eq!(shown(&s, "two", end), vec![("two".into(), 0.0)]);
        // An earlier workspace comes in from the left.
        let back = Swoosh::start(None, t0, 3, rows("four"), 1, W);
        assert!((back.offset(t0) + W).abs() < 1e-3);
    }

    #[test]
    fn the_chrome_around_the_list_never_moves() {
        let t0 = Instant::now();
        let s = Swoosh::start(None, t0, 0, rows("one"), 1, W);
        let mut p = paint("two");
        s.apply(&mut p, t0 + Duration::from_millis(40));
        let rest = paint("two");
        assert_eq!(p.quads.first(), rest.quads.first());
        assert_eq!(p.quads.last(), rest.quads.last());
        assert_eq!(p.labels.first(), rest.labels.first());
        assert_eq!(p.labels.last(), rest.labels.last());
        // The ghost rows join the list, between the chrome before and after it.
        assert_eq!((p.list_quads.clone(), p.list_labels.clone()), (1..3, 1..3));
    }

    #[test]
    fn a_switch_mid_slide_continues_from_where_the_rows_are() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(30);
        let first = Swoosh::start(None, t0, 0, rows("one"), 1, W);
        let before = shown(&first, "two", t1);
        // On to a third workspace: nothing on screen jumps, and the third comes in a width
        // right of the second.
        let on = Swoosh::start(Some(first), t1, 1, rows("two"), 2, W);
        let after = shown(&on, "three", t1);
        let x = |list: &[(String, f32)], text: &str| {
            list.iter().find(|(t, _)| t == text).map(|(_, x)| *x)
        };
        for text in ["one", "two"] {
            assert!((x(&before, text).unwrap() - x(&after, text).unwrap()).abs() < 1e-3);
        }
        assert!((x(&after, "three").unwrap() - x(&after, "two").unwrap() - W).abs() < 1e-3);

        // Turning back slides the second workspace's rows home from where they are.
        let first = Swoosh::start(None, t0, 0, rows("one"), 1, W);
        let back = Swoosh::start(Some(first), t1, 1, rows("two"), 0, W);
        let after = shown(&back, "one", t1);
        assert!((x(&before, "one").unwrap() - x(&after, "one").unwrap()).abs() < 1e-3);
        assert!((x(&before, "two").unwrap() - x(&after, "two").unwrap()).abs() < 1e-3);
        assert_eq!(after.len(), 2, "the first workspace is not drawn twice");
    }

    #[test]
    fn new_rows_never_land_on_rows_still_sliding() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(30);
        // From the second workspace back to the first leaves the second's rows on the right;
        // jumping on to the third puts it past them, not on top of them.
        let back = Swoosh::start(None, t0, 1, rows("two"), 0, W);
        let on = Swoosh::start(Some(back), t1, 0, rows("one"), 2, W);
        let list = shown(&on, "three", t1);
        let names: Vec<&str> = list.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(names, ["one", "two", "three"]);
        assert!((list[1].1 - list[0].1 - W).abs() < 1e-3);
        assert!((list[2].1 - list[1].1 - W).abs() < 1e-3);
    }

    #[test]
    fn rows_off_the_panel_for_the_whole_slide_are_dropped() {
        let t0 = Instant::now();
        let first = Swoosh::start(None, t0, 0, rows("one"), 1, W);
        // A switch as the first slide comes to rest: the first workspace's rows are a width
        // off the panel and only move further off, so they go.
        let on = Swoosh::start(Some(first), t0 + motion::PANEL, 1, rows("two"), 2, W);
        assert_eq!(on.ghosts.len(), 1);
        assert_eq!(on.ghosts[0].workspace, Some(1));
        // In one instant every workspace passed stays: the sweep crosses them all.
        let mut s = Swoosh::start(None, t0, 0, rows("w0"), 1, W);
        for w in 1..5 {
            s = Swoosh::start(Some(s), t0, w, rows(&format!("w{w}")), w + 1, W);
        }
        assert_eq!(s.ghosts.len(), 5);
    }

    #[test]
    fn a_removed_workspace_s_rows_slide_out_but_never_back_in() {
        let t0 = Instant::now();
        // Removing workspace 1 while on it shows workspace 2, which then becomes workspace 1.
        let mut s = Swoosh::start(None, t0, 1, rows("removed"), 2, W);
        s.forget(1);
        assert_eq!(s.ghosts[0].workspace, None);
        assert_eq!(shown(&s, "next", t0).len(), 2, "its rows still slide out");
        let mut s = Swoosh::start(None, t0, 3, rows("four"), 2, W);
        s.forget(1);
        assert_eq!(s.ghosts[0].workspace, Some(2));
    }

    /// Frames of a switch from the first workspace to the second, side by side, in each Ossein
    /// theme.
    #[test]
    #[ignore = "manual GPU capture: writes /tmp/skelly-swoosh-<theme>.png"]
    fn capture_workspace_swoosh() {
        use crate::sidebar::{self, View};
        use skelly_render::{TextMeasure, Theme};

        const SCALE: f32 = 2.0;
        const FRAME: (u32, u32) = (680, 900);
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w: 240.0 * SCALE,
            h: 450.0 * SCALE,
        };
        let chips = ['\u{25c6}', '\u{25cf}', '\u{25b2}'];
        let first = ["nvim", "server", "tests"].map(String::from);
        let second = ["docs", "api"].map(String::from);
        for name in ["ossein-dark", "ossein-light"] {
            let theme = Theme::resolve(name);
            let mut measure = TextMeasure::new(SCALE);
            let mut paint = |titles: &[String], chip: usize, pinned: &[char]| {
                let view = View {
                    tab_count: titles.len(),
                    active_tab: 1,
                    chips: &chips,
                    active_chip: chip,
                    chip_first: 0,
                    reveal_chip: false,
                    pinned,
                    active_pinned: None,
                    groups: &[],
                    tab_running: &[],
                    tab_titles: titles,
                    tab_keys: &[],
                    rail: false,
                    top_inset: 38.0,
                };
                sidebar::build(&view, panel, SCALE, &theme, &mut measure)
            };
            let first_rows = Rows::of(&paint(&first, 0, &['N', 'S']));
            let t0 = Instant::now();
            let swoosh = Swoosh::start(None, t0, 0, first_rows, 1, panel.w);
            let frames = [0, 16, 33, 66, 120, 320]
                .into_iter()
                .map(|ms| {
                    let mut live = paint(&second, 1, &[]);
                    swoosh.apply(&mut live, t0 + Duration::from_millis(ms));
                    (Some(live), Vec::new())
                })
                .collect();
            let path = format!("/tmp/skelly-swoosh-{name}.png");
            crate::capture::capture_strip(&path, name, FRAME, frames);
        }
    }
}
