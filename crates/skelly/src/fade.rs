//! The settings view's fade (design/README.md, 2026-11-12): opening the view fades through to
//! it - its page covers the terminal first, then its content comes in over the page - and
//! closing it runs the same back, quicker, as an exit is (§03). Only paint fades: the view takes
//! input from the moment it opens, and the terminal from the moment it closes.
//!
//! Pure: the binary starts a [`Fade`] as the view opens or closes and reads, on each frame, how
//! much of its page and its content show ([`alphas`]).

use std::time::{Duration, Instant};

use crate::motion::{self, Anim};

/// How far through the fade (`0..1`) the page has covered the terminal; the content comes in
/// over the rest of the way.
const FILL_PART: f32 = 0.35;

/// The view fading in or out.
pub(crate) struct Fade {
    anim: Anim,
    from: f32,
    to: f32,
}

impl Fade {
    /// Fade the view from showing `from` of the way (`0` hidden, `1` shown) to `to`, from
    /// `now`, at the pace of `whole` for the whole way: a fade turned back part way takes only
    /// as long as the way back.
    pub(crate) fn start(now: Instant, from: f32, to: f32, whole: Duration) -> Self {
        Self {
            anim: Anim::start(now, whole.mul_f32((to - from).abs().min(1.0))),
            from,
            to,
        }
    }

    /// How much of the view shows at `now`, `0..=1`.
    pub(crate) fn shown(&self, now: Instant) -> f32 {
        self.from + (self.to - self.from) * self.anim.progress(now)
    }

    /// Whether the fade has come to rest at `now`.
    pub(crate) fn done(&self, now: Instant) -> bool {
        self.anim.done(now)
    }
}

/// How much of the page and of its content show, `(fill, content)`, when `shown` of the view
/// shows: the page first, then the content over it, each on the standard curve.
pub(crate) fn alphas(shown: f32) -> (f32, f32) {
    let part = |t: f32| motion::STANDARD.ease(t.clamp(0.0, 1.0));
    (
        part(shown / FILL_PART),
        part((shown - FILL_PART) / (1.0 - FILL_PART)),
    )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use skelly_config::Config;
    use skelly_render::{CaptureSettings, PxRect, TextMeasure, Theme};

    use super::{alphas, Fade, FILL_PART};
    use crate::capture::{capture_pane, capture_strip, Shot};
    use crate::motion;
    use crate::settings::Settings;

    #[test]
    fn the_page_covers_the_terminal_before_the_content_shows() {
        assert_eq!(alphas(0.0), (0.0, 0.0));
        let (fill, content) = alphas(FILL_PART * 0.5);
        assert!(fill > 0.0 && fill < 1.0, "the page is coming in: {fill}");
        assert!(content.abs() < f32::EPSILON, "no content yet: {content}");
        let (fill, content) = alphas(FILL_PART + (1.0 - FILL_PART) * 0.5);
        assert!((fill - 1.0).abs() < f32::EPSILON, "the page is in: {fill}");
        assert!(
            content > 0.0 && content < 1.0,
            "the content is coming in: {content}"
        );
        assert_eq!(alphas(1.0), (1.0, 1.0));
    }

    #[test]
    fn opening_runs_from_hidden_to_shown() {
        let now = Instant::now();
        let fade = Fade::start(now, 0.0, 1.0, motion::BASE);
        assert!(fade.shown(now).abs() < f32::EPSILON);
        let half = fade.shown(now + motion::BASE / 2);
        assert!((half - 0.5).abs() < 0.01, "linear in time: {half}");
        assert!((fade.shown(now + motion::BASE) - 1.0).abs() < f32::EPSILON);
        assert!(!fade.done(now + motion::BASE / 2));
        assert!(fade.done(now + motion::BASE));
    }

    #[test]
    fn a_fade_turned_back_takes_only_the_way_back() {
        let now = Instant::now();
        let opening = Fade::start(now, 0.0, 1.0, motion::BASE);
        let later = now + motion::BASE / 4;
        let was = opening.shown(later);
        let closing = Fade::start(later, was, 0.0, motion::FAST);
        assert!((closing.shown(later) - was).abs() < f32::EPSILON, "no jump");
        let way_back = motion::FAST.mul_f32(was);
        assert!(!closing.done(later + way_back.saturating_sub(Duration::from_millis(1))));
        assert!(closing.done(later + way_back));
        assert!(closing.shown(later + way_back).abs() < f32::EPSILON);
    }

    /// Frames of the settings view fading in over a pane, by how much of it shows (0, 0.15,
    /// 0.3, 0.5, 0.75 and 1), in each Ossein theme.
    #[test]
    #[ignore = "manual GPU capture: writes /tmp/skelly-settings-fade-<theme>.png"]
    fn capture_settings_fade() {
        const SCALE: f32 = 2.0;
        const SIZE: (u32, u32) = (1600, 1000);
        let (w, h) = (
            f32::from(u16::try_from(SIZE.0).unwrap()),
            f32::from(u16::try_from(SIZE.1).unwrap()),
        );
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w,
            h,
        };
        let pad = crate::WINDOW_PAD * SCALE;
        let pane = PxRect {
            x: pad,
            y: pad,
            w: w - 2.0 * pad,
            h: h - 2.0 * pad,
        };
        for name in ["ossein-dark", "ossein-light"] {
            let theme = Theme::resolve(name);
            let mut config = Config::default();
            config.appearance.theme = name.into();
            let paint = Settings::new().build(
                panel,
                SCALE,
                &config,
                &[],
                &theme,
                &mut TextMeasure::new(SCALE),
            );
            let frames: Vec<Shot> = [0.0, 0.15, 0.3, 0.5, 0.75, 1.0]
                .into_iter()
                .map(|shown| {
                    let (fill_alpha, content_alpha) = alphas(shown);
                    Shot {
                        settings: Some(CaptureSettings {
                            panel,
                            nav_divider_x: paint.nav_divider_x,
                            quads: paint.quads.clone(),
                            labels: paint.labels.clone(),
                            fill_alpha,
                            content_alpha,
                        }),
                        panes: vec![capture_pane(pane, SCALE, &theme)],
                        ..Shot::default()
                    }
                })
                .collect();
            capture_strip(
                &format!("/tmp/skelly-settings-fade-{name}.png"),
                name,
                SIZE,
                frames,
            );
        }
    }
}
