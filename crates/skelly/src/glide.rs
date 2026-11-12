//! The sidebar's row glide (design/README.md, 2026-11-12): when the workspace's rows change in
//! place - a tab added, closed, moved or pinned, a group collapsed or expanded, the list
//! scrolled to keep the active tab in view - the rows that stay glide from where they were to
//! where they now rest, the rows that came fade in where they rest, and the rows that went fade
//! out where they were. The active tab's pill glides from the tab it marked to the one it marks
//! now. Only the paint moves: a switch, a new tab or a close takes effect at once.
//!
//! Pure, like the swoosh: the binary keeps the rows as last painted at rest ([`Rest`]) and
//! starts a [`Glide`] when a paint puts its rows anywhere else.

use std::time::{Duration, Instant};

use skelly_render::{ChromeQuad, ProseLabel, Srgb};

use crate::motion::{self, Anim};
use crate::sidebar::{Key, Paint};

/// How long a glide takes: the guide's tab-switch duration.
const GLIDE: Duration = motion::BASE;
/// How long the rows that went take to fade out: quicker than the glide, and quickest at first
/// (decelerate), so the rows that move into their place pass over little of them.
const FADE_OUT: Duration = motion::FAST;
/// The least move that glides, physical px; a smaller one is rounding, not a change.
const MIN_MOVE: f32 = 0.5;

/// One row of the list as painted at rest.
#[derive(Clone)]
struct Row {
    key: Key,
    top: f32,
    quads: Vec<ChromeQuad>,
    labels: Vec<ProseLabel>,
}

impl Row {
    /// The row moved down by `dy`.
    fn shifted(&self, dy: f32) -> Self {
        Self {
            key: self.key,
            top: self.top + dy,
            quads: self.quads.iter().map(|q| shifted(q, dy)).collect(),
            labels: self
                .labels
                .iter()
                .map(|l| ProseLabel {
                    y: l.y + dy,
                    ..l.clone()
                })
                .collect(),
        }
    }
}

/// The rows of the list as last painted at rest, and where the active tab's pill sat.
pub(crate) struct Rest {
    rows: Vec<Row>,
    pill: Option<f32>,
}

impl Rest {
    /// The rows of the list of a sidebar `paint`.
    pub(crate) fn of(paint: &Paint) -> Self {
        Self {
            rows: paint
                .rows
                .iter()
                .map(|span| Row {
                    key: span.key,
                    top: span.top,
                    quads: paint.quads[span.quads.clone()].to_vec(),
                    labels: paint.labels[span.labels.clone()].to_vec(),
                })
                .collect(),
            pill: paint.pill.as_ref().map(|span| span.top),
        }
    }

    /// Whether `paint` puts the same rows, and the pill, where these rest.
    pub(crate) fn holds(&self, paint: &Paint) -> bool {
        let row_at =
            |row: &Row, key: Key, top: f32| row.key == key && row.top.to_bits() == top.to_bits();
        self.rows.len() == paint.rows.len()
            && self
                .rows
                .iter()
                .zip(&paint.rows)
                .all(|(row, span)| row_at(row, span.key, span.top))
            && self.pill.map(f32::to_bits) == paint.pill.as_ref().map(|span| span.top.to_bits())
    }
}

/// A row of `paint` on its way to rest.
struct Moving {
    key: Key,
    /// Its offset from its rest when the glide started, physical px.
    dy: f32,
    /// How opaque it was when the glide started: `0` for a row that came.
    alpha: f32,
}

/// A row that went, fading out where it was shown.
struct Gone {
    row: Row,
    /// How opaque it was when the glide started.
    alpha: f32,
}

/// A change of the list's rows in flight.
pub(crate) struct Glide {
    anim: Anim,
    /// The rows of the paint that are not yet at rest.
    moving: Vec<Moving>,
    /// The rows that went, this change's and those of the change before still fading out.
    gone: Vec<Gone>,
    /// The pill's offset from its rest when the glide started, physical px.
    pill: f32,
}

impl Glide {
    /// Glide from the rows of `rest`, shown as `prev` has them at `now`, to those of `paint`;
    /// `None` when every row and the pill are already as `paint` has them.
    pub(crate) fn start(
        prev: Option<&Self>,
        rest: &Rest,
        paint: &Paint,
        now: Instant,
    ) -> Option<Self> {
        let offset = |key: Key| prev.map_or(0.0, |glide| glide.offset(key, now));
        let alpha = |key: Key| prev.map_or(1.0, |glide| glide.alpha(key, now));
        let mut moving = Vec::new();
        for span in &paint.rows {
            let (dy, alpha) = match rest.rows.iter().find(|row| row.key == span.key) {
                Some(row) => (row.top + offset(row.key) - span.top, alpha(row.key)),
                None => (0.0, 0.0),
            };
            if dy.abs() >= MIN_MOVE || alpha < 1.0 {
                moving.push(Moving {
                    key: span.key,
                    dy,
                    alpha,
                });
            }
        }
        let mut gone: Vec<Gone> = prev
            .into_iter()
            .flat_map(|glide| glide.gone.iter().map(move |gone| (glide, gone)))
            .map(|(glide, gone)| Gone {
                row: gone.row.clone(),
                alpha: glide.gone_alpha(gone, now),
            })
            .filter(|gone| gone.alpha > 0.0)
            .collect();
        gone.extend(
            rest.rows
                .iter()
                .filter(|row| paint.rows.iter().all(|span| span.key != row.key))
                .map(|row| Gone {
                    row: row.shifted(offset(row.key)),
                    alpha: alpha(row.key),
                }),
        );
        let pill = match (rest.pill, &paint.pill) {
            (Some(top), Some(span)) => {
                let dy = top + prev.map_or(0.0, |glide| glide.pill_offset(now)) - span.top;
                if dy.abs() >= MIN_MOVE {
                    dy
                } else {
                    0.0
                }
            }
            _ => 0.0,
        };
        let still = moving.is_empty() && gone.is_empty() && pill == 0.0;
        (!still).then(|| Self {
            anim: Anim::start(now, GLIDE),
            moving,
            gone,
            pill,
        })
    }

    /// Whether the glide has come to rest at `now`.
    pub(crate) fn done(&self, now: Instant) -> bool {
        self.anim.done(now)
    }

    /// The share of each start offset and start transparency still left at `now`, `1..=0`.
    fn left(&self, now: Instant) -> f32 {
        1.0 - motion::STANDARD.ease(self.anim.progress(now))
    }

    /// Row `key`'s offset from its rest at `now`, physical px.
    fn offset(&self, key: Key, now: Instant) -> f32 {
        self.moving
            .iter()
            .find(|row| row.key == key)
            .map_or(0.0, |row| row.dy * self.left(now))
    }

    /// How opaque row `key` is at `now`.
    fn alpha(&self, key: Key, now: Instant) -> f32 {
        self.moving
            .iter()
            .find(|row| row.key == key)
            .map_or(1.0, |row| 1.0 - (1.0 - row.alpha) * self.left(now))
    }

    /// How opaque a row that went is at `now`: it fades out over [`FADE_OUT`].
    fn gone_alpha(&self, gone: &Gone, now: Instant) -> f32 {
        let out = self.anim.progress(now) * GLIDE.as_secs_f32() / FADE_OUT.as_secs_f32();
        gone.alpha * (1.0 - motion::DECELERATE.ease(out.min(1.0)))
    }

    /// The pill's offset from its rest at `now`, physical px.
    fn pill_offset(&self, now: Instant) -> f32 {
        self.pill * self.left(now)
    }

    /// Show `paint`'s rows as they are at `now`: on their way to rest and fading in over the
    /// sidebar's `bg`, the pill on its way, and beneath them all the rows that went, fading out.
    pub(crate) fn apply(&self, paint: &mut Paint, now: Instant, bg: Srgb) {
        for row in &self.moving {
            let Some(span) = paint.rows.iter().find(|span| span.key == row.key) else {
                continue;
            };
            let (quads, labels) = (span.quads.clone(), span.labels.clone());
            let (dy, alpha) = (self.offset(row.key, now), self.alpha(row.key, now));
            for quad in &mut paint.quads[quads] {
                quad.rect.y += dy;
                quad.alpha *= alpha;
            }
            for label in &mut paint.labels[labels] {
                label.y += dy;
                label.color = label.color.over(bg, alpha);
            }
        }
        if let Some(span) = &paint.pill {
            let dy = self.pill_offset(now);
            for quad in &mut paint.quads[span.quads.clone()] {
                quad.rect.y += dy;
            }
        }
        let mut quads = Vec::new();
        let mut labels = Vec::new();
        for gone in &self.gone {
            let alpha = self.gone_alpha(gone, now);
            if alpha <= 0.0 {
                continue;
            }
            quads.extend(gone.row.quads.iter().map(|q| ChromeQuad {
                alpha: q.alpha * alpha,
                ..*q
            }));
            labels.extend(gone.row.labels.iter().map(|l| ProseLabel {
                color: l.color.over(bg, alpha),
                ..l.clone()
            }));
        }
        if !quads.is_empty() || !labels.is_empty() {
            insert_beneath(paint, quads, labels);
        }
    }
}

/// `quad` moved down by `dy`.
fn shifted(quad: &ChromeQuad, dy: f32) -> ChromeQuad {
    let mut quad = *quad;
    quad.rect.y += dy;
    quad
}

/// Insert `quads` and `labels` at the start of `paint`'s list, beneath its rows, keeping every
/// row's ranges and the list's in step.
fn insert_beneath(paint: &mut Paint, quads: Vec<ChromeQuad>, labels: Vec<ProseLabel>) {
    let (q, l) = (paint.list_quads.start, paint.list_labels.start);
    let (nq, nl) = (quads.len(), labels.len());
    paint.quads.splice(q..q, quads);
    paint.labels.splice(l..l, labels);
    paint.list_quads.end += nq;
    paint.list_labels.end += nl;
    for span in paint.rows.iter_mut().chain(paint.pill.as_mut()) {
        span.quads = span.quads.start + nq..span.quads.end + nq;
        span.labels = span.labels.start + nl..span.labels.end + nl;
    }
}

#[cfg(test)]
mod tests {
    use super::{Glide, Rest, FADE_OUT, GLIDE};
    use crate::sidebar::{Key, Paint, Span};
    use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb};
    use std::time::{Duration, Instant};

    const ROW: f32 = 60.0;
    const INK: Srgb = Srgb {
        r: 200,
        g: 200,
        b: 200,
    };
    const BG: Srgb = Srgb { r: 0, g: 0, b: 0 };

    fn quad(y: f32) -> ChromeQuad {
        ChromeQuad::fill(
            PxRect {
                x: 0.0,
                y,
                w: 4.0,
                h: 4.0,
            },
            INK,
        )
    }

    fn label(text: &str, y: f32) -> ProseLabel {
        ProseLabel {
            text: text.to_owned(),
            x: 20.0,
            y,
            role: FontRole::Label,
            color: INK,
            weight: None,
            max_w: 200.0,
        }
    }

    /// A sidebar paint listing a row per key from the top, a row apart, with the pill on the
    /// row of `active`: the panel fill, the pill, then each row's quad and label, the divider.
    fn paint(keys: &[Key], active: Option<Key>) -> Paint {
        let mut quads = vec![quad(0.0)];
        let mut labels = vec![label("chrome", 0.0)];
        let list = (quads.len(), labels.len());
        let mut rows = Vec::new();
        let mut pill = None;
        for (i, &key) in keys.iter().enumerate() {
            #[allow(clippy::cast_precision_loss, reason = "a tiny test row count")]
            let top = 100.0 + i as f32 * ROW;
            if Some(key) == active {
                quads.push(quad(top));
                pill = Some(Span {
                    key,
                    top,
                    quads: quads.len() - 1..quads.len(),
                    labels: labels.len()..labels.len(),
                });
            }
            quads.push(quad(top));
            labels.push(label(&format!("{key:?}"), top));
            rows.push(Span {
                key,
                top,
                quads: quads.len() - 1..quads.len(),
                labels: labels.len() - 1..labels.len(),
            });
        }
        let list_quads = list.0..quads.len();
        let list_labels = list.1..labels.len();
        quads.push(quad(900.0));
        Paint {
            panel: PxRect {
                x: 0.0,
                y: 0.0,
                w: 480.0,
                h: 1000.0,
            },
            quads,
            labels,
            chip_first: None,
            list_quads,
            list_labels,
            rows,
            pill,
        }
    }

    /// Each list label's text, y and color at `now`, top to bottom.
    fn shown(
        glide: &Glide,
        keys: &[Key],
        active: Option<Key>,
        now: Instant,
    ) -> Vec<(String, f32, Srgb)> {
        let mut p = paint(keys, active);
        glide.apply(&mut p, now, BG);
        let mut list: Vec<_> = p.labels[p.list_labels.clone()]
            .iter()
            .map(|l| (l.text.clone(), l.y, l.color))
            .collect();
        list.sort_by(|a, b| a.1.total_cmp(&b.1));
        list
    }

    fn y(list: &[(String, f32, Srgb)], key: Key) -> f32 {
        list.iter()
            .find(|(t, ..)| *t == format!("{key:?}"))
            .unwrap()
            .1
    }

    const A: Key = Key::Tab(1);
    const B: Key = Key::Tab(2);
    const C: Key = Key::Tab(3);

    #[test]
    fn a_paint_with_its_rows_where_they_rest_does_not_glide() {
        let rest = Rest::of(&paint(&[A, B, Key::NewTab], Some(A)));
        let same = paint(&[A, B, Key::NewTab], Some(A));
        assert!(rest.holds(&same));
        assert!(Glide::start(None, &rest, &same, Instant::now()).is_none());
        assert!(!rest.holds(&paint(&[A, B, Key::NewTab], Some(B))));
        assert!(!rest.holds(&paint(&[B, A, Key::NewTab], Some(A))));
    }

    #[test]
    fn a_closed_row_fades_out_while_the_rows_under_it_glide_up() {
        let t0 = Instant::now();
        let rest = Rest::of(&paint(&[A, B, C], None));
        let after = [A, C];
        let glide = Glide::start(None, &rest, &paint(&after, None), t0).unwrap();
        // At the start every row is where it was, the closed one included.
        let start = shown(&glide, &after, None, t0);
        assert_eq!(start.len(), 3);
        assert!((y(&start, C) - (100.0 + 2.0 * ROW)).abs() < 1e-3);
        assert!((y(&start, B) - (100.0 + ROW)).abs() < 1e-3);
        assert_eq!(start[1].2, INK, "the closed row starts opaque");
        // Once the closed row has faded out, the moved one is still on its way.
        let mid = shown(&glide, &after, None, t0 + FADE_OUT);
        assert!(y(&mid, C) > 100.0 + ROW && y(&mid, C) < 100.0 + 2.0 * ROW);
        assert_eq!(mid.len(), 2, "the closed row is gone once faded");
        // At rest every row is home.
        let end = t0 + GLIDE;
        assert!(glide.done(end));
        let end = shown(&glide, &after, None, end);
        assert!((y(&end, C) - (100.0 + ROW)).abs() < 1e-3);
        assert!((y(&end, A) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn a_new_row_fades_in_where_it_rests() {
        let t0 = Instant::now();
        let rest = Rest::of(&paint(&[A, Key::NewTab], None));
        let after = [A, B, Key::NewTab];
        let glide = Glide::start(None, &rest, &paint(&after, None), t0).unwrap();
        let start = shown(&glide, &after, None, t0);
        assert!(
            (y(&start, B) - (100.0 + ROW)).abs() < 1e-3,
            "it never moves"
        );
        let color = start.iter().find(|(t, ..)| t == "Tab(2)").unwrap().2;
        assert_eq!(color, BG, "it starts faded out");
        // The new-tab action makes room from where it was.
        assert!((y(&start, Key::NewTab) - (100.0 + ROW)).abs() < 1e-3);
        let end = shown(&glide, &after, None, t0 + GLIDE);
        assert_eq!(end.iter().find(|(t, ..)| t == "Tab(2)").unwrap().2, INK);
        assert!((y(&end, Key::NewTab) - (100.0 + 2.0 * ROW)).abs() < 1e-3);
    }

    #[test]
    fn the_pill_glides_to_the_new_active_tab() {
        let t0 = Instant::now();
        let rows = [A, B, C];
        let rest = Rest::of(&paint(&rows, Some(A)));
        let glide = Glide::start(None, &rest, &paint(&rows, Some(C)), t0).unwrap();
        let pill_y = |at: Instant| {
            let mut p = paint(&rows, Some(C));
            glide.apply(&mut p, at, BG);
            p.quads[p.pill.unwrap().quads][0].rect.y
        };
        assert!(
            (pill_y(t0) - 100.0).abs() < 1e-3,
            "it starts on the tab it left"
        );
        let mid = pill_y(t0 + GLIDE / 2);
        assert!(mid > 100.0 && mid < 100.0 + 2.0 * ROW);
        assert!((pill_y(t0 + GLIDE) - (100.0 + 2.0 * ROW)).abs() < 1e-3);
        // The rows themselves stay put.
        let list = shown(&glide, &rows, Some(C), t0 + GLIDE / 2);
        assert!((y(&list, B) - (100.0 + ROW)).abs() < 1e-3);
    }

    #[test]
    fn a_change_mid_glide_continues_from_where_the_rows_are() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(40);
        let first = [A, B, C];
        let second = [C, A, B];
        let rest = Rest::of(&paint(&first, None));
        let glide = Glide::start(None, &rest, &paint(&second, None), t0).unwrap();
        let before = shown(&glide, &second, None, t1);
        // The rows move back before the first glide ends: nothing jumps.
        let rest = Rest::of(&paint(&second, None));
        let back = Glide::start(Some(&glide), &rest, &paint(&first, None), t1).unwrap();
        let after = shown(&back, &first, None, t1);
        for key in first {
            assert!(
                (y(&before, key) - y(&after, key)).abs() < 1e-3,
                "{key:?} jumped"
            );
        }
    }

    #[test]
    fn a_change_mid_fade_fades_on_from_where_the_rows_are() {
        let t0 = Instant::now();
        let t1 = t0 + FADE_OUT / 2;
        let alpha = |list: &[(String, f32, Srgb)], key: Key| {
            list.iter()
                .find(|(t, ..)| *t == format!("{key:?}"))
                .map(|(.., c)| c.r)
        };
        // B is closed, and C is added, then A is closed while B fades out and C fades in.
        let rest = Rest::of(&paint(&[A, B], None));
        let first = Glide::start(None, &rest, &paint(&[A, C], None), t0).unwrap();
        let before = shown(&first, &[A, C], None, t1);
        let rest = Rest::of(&paint(&[A, C], None));
        let second = Glide::start(Some(&first), &rest, &paint(&[C], None), t1).unwrap();
        let after = shown(&second, &[C], None, t1);
        assert_eq!(alpha(&after, B), alpha(&before, B), "B keeps fading out");
        assert_eq!(alpha(&after, C), alpha(&before, C), "C keeps fading in");
        assert!((y(&after, C) - y(&before, C)).abs() < 1e-3);
        let end = shown(&second, &[C], None, t1 + GLIDE);
        assert_eq!(end.len(), 1);
        assert_eq!(alpha(&end, C), Some(INK.r));
    }

    #[test]
    fn the_rows_that_went_draw_beneath_the_list_and_keep_its_ranges() {
        let t0 = Instant::now();
        let rest = Rest::of(&paint(&[A, B, C], Some(C)));
        let glide = Glide::start(None, &rest, &paint(&[A, C], Some(C)), t0).unwrap();
        let mut p = paint(&[A, C], Some(C));
        let chrome = (p.quads[0], p.quads[p.quads.len() - 1]);
        glide.apply(&mut p, t0, BG);
        assert_eq!((p.quads[0], p.quads[p.quads.len() - 1]), chrome);
        // The row that went is first in the list, and every span still covers its own row.
        assert_eq!(p.labels[p.list_labels.start].text, "Tab(2)");
        for span in &p.rows {
            assert_eq!(
                p.labels[span.labels.clone()][0].text,
                format!("{:?}", span.key)
            );
        }
        let pill = p.pill.unwrap();
        assert!(pill.quads.end <= p.list_quads.end);
    }

    /// Frames of a tab closed, a tab added, a switch and a group folded, in each Ossein theme.
    #[test]
    #[ignore = "manual GPU capture: writes /tmp/skelly-glide-<change>-<theme>.png"]
    fn capture_row_glide() {
        use crate::sidebar::{self, GroupSpan, View};
        use skelly_render::{TextMeasure, Theme};

        const SCALE: f32 = 2.0;
        /// The tabs' titles and keys, the active one, and the group of the last two, folded or
        /// not.
        type State = (&'static [&'static str], &'static [u64], usize, Option<bool>);
        let changes: [(&str, State, State); 4] = [
            (
                "close",
                (&["nvim", "server", "tests", "logs"], &[0, 1, 2, 3], 1, None),
                (&["nvim", "tests", "logs"], &[0, 2, 3], 1, None),
            ),
            (
                "add",
                (&["nvim", "tests"], &[0, 2], 0, None),
                (&["nvim", "tests", "build"], &[0, 2, 4], 2, None),
            ),
            (
                "switch",
                (&["nvim", "server", "tests", "logs"], &[0, 1, 2, 3], 0, None),
                (&["nvim", "server", "tests", "logs"], &[0, 1, 2, 3], 3, None),
            ),
            (
                "fold",
                (
                    &["nvim", "server", "tests", "logs"],
                    &[0, 1, 2, 3],
                    0,
                    Some(false),
                ),
                (
                    &["nvim", "server", "tests", "logs"],
                    &[0, 1, 2, 3],
                    0,
                    Some(true),
                ),
            ),
        ];
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w: 240.0 * SCALE,
            h: 420.0 * SCALE,
        };
        let chips = ['\u{25c6}', '\u{25cf}'];
        for name in ["ossein-dark", "ossein-light"] {
            let theme = Theme::resolve(name);
            let mut measure = TextMeasure::new(SCALE);
            for (label, before, after) in changes {
                let mut paint = |(titles, keys, active, folded): State| {
                    let titles: Vec<String> = titles.iter().map(|t| (*t).to_owned()).collect();
                    let groups: Vec<GroupSpan> = folded
                        .map(|collapsed| GroupSpan {
                            key: 10,
                            name: "skelly \u{b7} main",
                            collapsed,
                            start: 2,
                            len: 2,
                        })
                        .into_iter()
                        .collect();
                    let view = View {
                        tab_count: titles.len(),
                        active_tab: active,
                        chips: &chips,
                        active_chip: 0,
                        chip_first: 0,
                        reveal_chip: false,
                        pinned: &[],
                        active_pinned: None,
                        groups: &groups,
                        tab_running: &[],
                        tab_titles: &titles,
                        tab_keys: keys,
                        rail: false,
                        top_inset: crate::TITLE_STRIP,
                    };
                    sidebar::build(&view, panel, SCALE, &theme, &mut measure)
                };
                let rest = Rest::of(&paint(before));
                let t0 = Instant::now();
                let glide = Glide::start(None, &rest, &paint(after), t0).unwrap();
                let frames = [0, 30, 60, 90, 120, 180]
                    .into_iter()
                    .map(|ms| {
                        let mut live = paint(after);
                        glide.apply(&mut live, t0 + Duration::from_millis(ms), theme.bg_sidebar);
                        (Some(live), Vec::new())
                    })
                    .collect();
                let path = format!("/tmp/skelly-glide-{label}-{name}.png");
                sidebar::capture_strip(&path, name, (520, 840), frames);
            }
        }
    }
}
