//! The first-run onboarding modal (design §10.1).
//!
//! Shown once on a fresh install (no `config.toml` yet): a centered card that picks the shell
//! and UI theme, shows the key chords that matter, and offers **Skip** (accept defaults - the
//! login shell + Ossein Dark) or **Start** (apply the picks). This module is pure state +
//! layout: it owns the selection/focus and turns the card rect into a proportional display list
//! ([`build`]) plus a hit-test ([`hit`]); the binary owns first-run detection, live theme
//! preview, and writing the config on dismiss.
//!
//! The guide's theme picker shows a third `kana / + 8 presets` card for themes Skelly does not
//! ship - omitted here rather than fabricated (Hard rule 5); the two real Ossein themes remain.
#![allow(
    clippy::cast_precision_loss,
    reason = "control/index counts are tiny exact values (few segments, cards, chips)"
)]

use skelly_render::{
    logo_chrome_quads, ChromeQuad, FontRole, ProseLabel, PxRect, TextMeasure, Theme,
};

/// The shells the picker offers (design §10.1). The string is the `[shell] program` value.
pub(crate) const SHELLS: [&str; 3] = ["zsh", "bash", "fish"];
/// The UI themes offered: `(config name, display label)`. Two real Ossein themes (see the
/// module note on the omitted third card).
pub(crate) const THEMES: [(&str, &str); 2] = [
    ("ossein-dark", "Ossein Dark"),
    ("ossein-light", "Ossein Light"),
];
/// The key-chord hints shown above the buttons (chord, label).
const HINTS: [(&str, &str); 3] = [
    ("\u{2318}K", "palette"),
    ("\u{2318}T", "new tab"),
    ("\u{2325}|", "split"),
];

// --- card + section metrics (logical px, from the guide's §10.1 markup) ---
const CARD_W: f32 = 460.0;
const PAD_X: f32 = 32.0;
const PAD_TOP: f32 = 30.0;
const PAD_BOTTOM: f32 = 26.0;
const MARK: f32 = 38.0;
const MARK_GAP: f32 = 13.0;
const HEADER_GAP: f32 = 22.0;
const SECTION_LABEL_H: f32 = 14.0;
const SECTION_LABEL_GAP: f32 = 8.0;
const SEG_H: f32 = 34.0;
const SEG_GAP: f32 = 8.0;
const SEG_RADIUS: f32 = 8.0;
const SHELL_BLOCK_GAP: f32 = 18.0;
const CARD_SWATCH_H: f32 = 32.0;
const CARD_STRIP_H: f32 = 24.0;
const THEME_BLOCK_GAP: f32 = 22.0;
const CHIP_ROW_H: f32 = 22.0;
const CHIP_BLOCK_GAP: f32 = 22.0;
const BTN_H: f32 = 36.0;
const BTN_GAP: f32 = 10.0;

/// The focusable control groups, in Tab order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Field {
    Shell,
    Theme,
    Skip,
    Start,
}

/// What a click landed on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Hit {
    Shell(usize),
    Theme(usize),
    Skip,
    Start,
}

/// The onboarding modal's live selection + keyboard focus.
pub(crate) struct Onboarding {
    shell: usize,
    theme: usize,
    focus: Field,
}

impl Onboarding {
    /// Fresh state: zsh + Ossein Dark, focus on the shell row.
    pub(crate) fn new() -> Self {
        Self {
            shell: 0,
            theme: 0,
            focus: Field::Shell,
        }
    }

    /// The selected shell program (for `[shell] program`).
    pub(crate) fn shell_program(&self) -> &'static str {
        SHELLS[self.shell]
    }

    /// The selected UI theme name (for `[appearance] theme`).
    pub(crate) fn theme_name(&self) -> &'static str {
        THEMES[self.theme].0
    }

    /// Move keyboard focus to the next (`forward`) or previous group, wrapping.
    pub(crate) fn cycle_focus(&mut self, forward: bool) {
        let order = [Field::Shell, Field::Theme, Field::Skip, Field::Start];
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let n = order.len();
        self.focus = order[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }];
    }

    /// Handle `←`/`→`: change the selection within the shell/theme row, else move focus between
    /// the Skip/Start buttons. Returns `true` when the THEME selection changed (so the binary
    /// live-applies it for preview).
    pub(crate) fn horizontal(&mut self, forward: bool) -> bool {
        match self.focus {
            Field::Shell => {
                self.shell = step(self.shell, SHELLS.len(), forward);
                false
            }
            Field::Theme => {
                self.theme = step(self.theme, THEMES.len(), forward);
                true
            }
            Field::Skip if forward => {
                self.focus = Field::Start;
                false
            }
            Field::Start if !forward => {
                self.focus = Field::Skip;
                false
            }
            _ => false,
        }
    }

    /// Whether Enter should confirm (Start) - true unless focus is on Skip.
    pub(crate) fn enter_is_start(&self) -> bool {
        self.focus != Field::Skip
    }

    /// Apply a click: select a shell/theme (theme selection is reported so the binary previews
    /// it) or focus a button. Returns the hit for the binary to act on.
    pub(crate) fn click(&mut self, hit: Hit) {
        match hit {
            Hit::Shell(i) => {
                self.shell = i;
                self.focus = Field::Shell;
            }
            Hit::Theme(i) => {
                self.theme = i;
                self.focus = Field::Theme;
            }
            Hit::Skip => self.focus = Field::Skip,
            Hit::Start => self.focus = Field::Start,
        }
    }

    /// The card's natural size (physical px), centered by the binary.
    pub(crate) fn card_size(scale: f32) -> (f32, f32) {
        let h = PAD_TOP
            + MARK
            + HEADER_GAP
            + SECTION_LABEL_H
            + SECTION_LABEL_GAP
            + SEG_H
            + SHELL_BLOCK_GAP
            + SECTION_LABEL_H
            + SECTION_LABEL_GAP
            + CARD_SWATCH_H
            + CARD_STRIP_H
            + THEME_BLOCK_GAP
            + CHIP_ROW_H
            + CHIP_BLOCK_GAP
            + BTN_H
            + PAD_BOTTOM;
        (CARD_W * scale, h * scale)
    }
}

/// The card's laid-out hit/draw rectangles (physical px), shared by [`build`] and [`hit`] so a
/// click lands on exactly what is drawn.
struct Layout {
    shells: [PxRect; 3],
    themes: [PxRect; 2],
    skip: PxRect,
    start: PxRect,
    /// The y (physical) of each section's content top, threaded as the card is walked.
    shell_label_y: f32,
    theme_label_y: f32,
    chip_y: f32,
    header_y: f32,
}

fn layout(panel: PxRect, scale: f32) -> Layout {
    let pad = PAD_X * scale;
    let inner_x = panel.x + pad;
    let inner_w = panel.w - 2.0 * pad;
    let mut y = panel.y + PAD_TOP * scale;
    let header_y = y;
    y += (MARK + HEADER_GAP) * scale;

    let shell_label_y = y;
    y += (SECTION_LABEL_H + SECTION_LABEL_GAP) * scale;
    let seg_w = (inner_w - 2.0 * SEG_GAP * scale) / 3.0;
    let shells = std::array::from_fn(|i| PxRect {
        x: inner_x + i as f32 * (seg_w + SEG_GAP * scale),
        y,
        w: seg_w,
        h: SEG_H * scale,
    });
    y += (SEG_H + SHELL_BLOCK_GAP) * scale;

    let theme_label_y = y;
    y += (SECTION_LABEL_H + SECTION_LABEL_GAP) * scale;
    let card_w = (inner_w - SEG_GAP * scale) / 2.0;
    let card_h = (CARD_SWATCH_H + CARD_STRIP_H) * scale;
    let themes = std::array::from_fn(|i| PxRect {
        x: inner_x + i as f32 * (card_w + SEG_GAP * scale),
        y,
        w: card_w,
        h: card_h,
    });
    y += card_h + THEME_BLOCK_GAP * scale;

    let chip_y = y;
    y += (CHIP_ROW_H + CHIP_BLOCK_GAP) * scale;

    let skip_w = (inner_w - BTN_GAP * scale) / 3.0; // Skip flex:1, Start flex:2
    let skip = PxRect {
        x: inner_x,
        y,
        w: skip_w,
        h: BTN_H * scale,
    };
    let start = PxRect {
        x: inner_x + skip_w + BTN_GAP * scale,
        y,
        w: inner_w - skip_w - BTN_GAP * scale,
        h: BTN_H * scale,
    };
    Layout {
        shells,
        themes,
        skip,
        start,
        shell_label_y,
        theme_label_y,
        chip_y,
        header_y,
    }
}

/// Hit-test a click at `(px, py)` (physical) against the card at `panel`.
pub(crate) fn hit(panel: PxRect, scale: f32, px: f32, py: f32) -> Option<Hit> {
    let l = layout(panel, scale);
    let inside = |r: &PxRect| px >= r.x && px < r.x + r.w && py >= r.y && py < r.y + r.h;
    for (i, r) in l.shells.iter().enumerate() {
        if inside(r) {
            return Some(Hit::Shell(i));
        }
    }
    for (i, r) in l.themes.iter().enumerate() {
        if inside(r) {
            return Some(Hit::Theme(i));
        }
    }
    if inside(&l.skip) {
        return Some(Hit::Skip);
    }
    if inside(&l.start) {
        return Some(Hit::Start);
    }
    None
}

/// The card's content as a proportional display list. The binary draws the card frame (shadow +
/// border + fill) via the overlay pass; this fills in the mark, sections, controls, and buttons.
#[allow(clippy::too_many_lines, reason = "one straight-line card builder")]
pub(crate) fn build(
    onb: &Onboarding,
    panel: PxRect,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
    let l = layout(panel, scale);
    let mut quads = Vec::new();
    let mut labels = Vec::new();
    let inner_x = panel.x + PAD_X * scale;

    // Header: the vertebra mark + welcome title/subtitle.
    quads.extend(logo_chrome_quads(
        inner_x,
        l.header_y,
        MARK * scale,
        theme,
        1.0,
    ));
    let text_x = inner_x + (MARK + MARK_GAP) * scale;
    let title_h = measure.line_height(FontRole::H2);
    labels.push(ProseLabel {
        text: "Welcome to skelly".to_owned(),
        x: text_x,
        y: l.header_y + (MARK * scale - title_h - measure.line_height(FontRole::Caption)) * 0.5,
        role: FontRole::H2,
        color: theme.fg_primary,
        weight: None,
        max_w: f32::MAX,
    });
    labels.push(ProseLabel {
        text: "Barebones. Let\u{2019}s set two things.".to_owned(),
        x: text_x,
        y: l.header_y
            + (MARK * scale - title_h - measure.line_height(FontRole::Caption)) * 0.5
            + title_h,
        role: FontRole::Caption,
        color: theme.fg_muted,
        weight: None,
        max_w: f32::MAX,
    });

    // Section labels (micro, letterspaced, fg.muted).
    push_section_label(
        &mut labels,
        measure,
        "SHELL",
        inner_x,
        l.shell_label_y,
        scale,
        theme,
    );
    push_section_label(
        &mut labels,
        measure,
        "THEME",
        inner_x,
        l.theme_label_y,
        scale,
        theme,
    );

    // Shell segmented control.
    for (i, r) in l.shells.iter().enumerate() {
        let selected = i == onb.shell;
        push_segment(&mut quads, r, selected, scale, theme);
        center_label(
            &mut labels,
            measure,
            SHELLS[i],
            *r,
            FontRole::Mono,
            seg_fg(selected, theme),
        );
    }

    // Theme cards: a bg.base swatch header with "❯ ossein" in the theme's accent, then a
    // bg.surface label strip. The active theme card is accent-bordered.
    for (i, r) in l.themes.iter().enumerate() {
        push_theme_card(
            &mut quads,
            &mut labels,
            measure,
            r,
            i,
            i == onb.theme,
            scale,
            theme,
        );
    }

    // Key-chord hints, centered.
    push_hints(
        &mut quads,
        &mut labels,
        measure,
        panel,
        l.chip_y,
        scale,
        theme,
    );

    // Buttons: Skip (outline) + Start (accent).
    push_skip(
        &mut quads,
        &mut labels,
        measure,
        l.skip,
        onb.focus == Field::Skip,
        scale,
        theme,
    );
    push_start(
        &mut quads,
        &mut labels,
        measure,
        l.start,
        onb.focus == Field::Start,
        scale,
        theme,
    );

    (quads, labels)
}

// --- section drawing helpers ---

fn push_section_label(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    text: &str,
    x: f32,
    y: f32,
    scale: f32,
    theme: &Theme,
) {
    let h = measure.line_height(FontRole::Micro);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x,
        y: y + (SECTION_LABEL_H * scale - h) * 0.5,
        role: FontRole::Micro,
        color: theme.fg_muted,
        weight: None,
        max_w: f32::MAX,
    });
}

fn seg_fg(selected: bool, theme: &Theme) -> skelly_render::Srgb {
    if selected {
        theme.fg_primary
    } else {
        theme.fg_secondary
    }
}

fn push_segment(
    quads: &mut Vec<ChromeQuad>,
    r: &PxRect,
    selected: bool,
    scale: f32,
    theme: &Theme,
) {
    let radius = SEG_RADIUS * scale;
    let stroke = scale.max(1.0);
    if selected {
        // accent@0.4 border over an accent.subtle fill, composited in sRGB over the card.
        quads.push(ChromeQuad::rounded(
            *r,
            theme.accent.over(theme.bg_elevated, 0.4),
            radius,
        ));
        let inner = inset(*r, stroke);
        quads.push(ChromeQuad::rounded(
            inner,
            theme.accent_subtle_on(theme.bg_elevated),
            radius - stroke,
        ));
    } else {
        quads.push(ChromeQuad::rounded(*r, theme.border_subtle, radius));
        let inner = inset(*r, stroke);
        quads.push(ChromeQuad::rounded(
            inner,
            theme.bg_surface,
            radius - stroke,
        ));
    }
}

#[allow(clippy::too_many_arguments, reason = "one focused theme-card builder")]
fn push_theme_card(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    r: &PxRect,
    index: usize,
    selected: bool,
    scale: f32,
    theme: &Theme,
) {
    let radius = SEG_RADIUS * scale;
    let stroke = scale.max(1.0);
    // The card border (accent when selected, else border.subtle) with the two-band interior.
    let border = if selected {
        theme.accent.over(theme.bg_elevated, 0.4)
    } else {
        theme.border_subtle
    };
    quads.push(ChromeQuad::rounded(*r, border, radius));
    let inner = inset(*r, stroke);
    // The swatch shows the theme's own bg.base; the strip uses bg.surface.
    let card_theme = Theme::resolve(THEMES[index].0);
    let swatch_h = CARD_SWATCH_H * scale - stroke;
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: inner.x,
            y: inner.y,
            w: inner.w,
            h: swatch_h,
        },
        card_theme.bg_base.to_srgb(),
        radius - stroke,
    ));
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: inner.x,
            y: inner.y + swatch_h,
            w: inner.w,
            h: inner.h - swatch_h,
        },
        theme.bg_surface,
        radius - stroke,
    ));
    // "❯ ossein" in the theme's accent, on the swatch.
    let sw_line = measure.line_height(FontRole::Mono);
    labels.push(ProseLabel {
        text: "\u{276f} ossein".to_owned(),
        x: inner.x + 9.0 * scale,
        y: inner.y + (swatch_h - sw_line) * 0.5,
        role: FontRole::Mono,
        color: card_theme.accent,
        weight: None,
        max_w: f32::MAX,
    });
    // The theme name on the strip.
    let strip_line = measure.line_height(FontRole::Caption);
    labels.push(ProseLabel {
        text: THEMES[index].1.to_owned(),
        x: inner.x + 9.0 * scale,
        y: inner.y + swatch_h + (inner.h - swatch_h - strip_line) * 0.5,
        role: FontRole::Caption,
        color: if selected {
            theme.fg_primary
        } else {
            theme.fg_secondary
        },
        weight: None,
        max_w: f32::MAX,
    });
}

fn push_hints(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    panel: PxRect,
    y: f32,
    scale: f32,
    theme: &Theme,
) {
    let gap = 16.0 * scale;
    let kbd_pad = 6.0 * scale;
    let kbd_gap = 6.0 * scale;
    // Measure the whole row to center it.
    let widths: Vec<(f32, f32, f32)> = HINTS
        .iter()
        .map(|(chord, label)| {
            let kw = measure.width(chord, FontRole::Micro, None) + 2.0 * kbd_pad;
            let lw = measure.width(label, FontRole::Caption, None);
            (kw, lw, kw + kbd_gap + lw)
        })
        .collect();
    let total: f32 = widths.iter().map(|(_, _, w)| w).sum::<f32>() + gap * (HINTS.len() - 1) as f32;
    let mut x = panel.x + (panel.w - total) * 0.5;
    let kbd_h = CHIP_ROW_H * scale;
    let kbd_line = measure.line_height(FontRole::Micro);
    let lbl_line = measure.line_height(FontRole::Caption);
    for (i, (chord, label)) in HINTS.iter().enumerate() {
        let (kw, _, w) = widths[i];
        quads.push(ChromeQuad::rounded(
            PxRect {
                x,
                y,
                w: kw,
                h: kbd_h,
            },
            theme.bg_surface,
            4.0 * scale,
        ));
        labels.push(ProseLabel {
            text: (*chord).to_owned(),
            x: x + kbd_pad,
            y: y + (kbd_h - kbd_line) * 0.5,
            role: FontRole::Micro,
            color: theme.fg_primary,
            weight: None,
            max_w: f32::MAX,
        });
        labels.push(ProseLabel {
            text: (*label).to_owned(),
            x: x + kw + kbd_gap,
            y: y + (kbd_h - lbl_line) * 0.5,
            role: FontRole::Caption,
            color: theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
        });
        x += w + gap;
    }
}

fn push_skip(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    r: PxRect,
    focused: bool,
    scale: f32,
    theme: &Theme,
) {
    let radius = SEG_RADIUS * scale;
    let stroke = scale.max(1.0);
    // Outline button (transparent fill = the card surface); a focus ring uses the accent border.
    let border = if focused { theme.accent } else { theme.border };
    quads.push(ChromeQuad::rounded(r, border, radius));
    quads.push(ChromeQuad::rounded(
        inset(r, stroke),
        theme.bg_elevated,
        radius - stroke,
    ));
    center_label(
        labels,
        measure,
        "Skip",
        r,
        FontRole::Body,
        theme.fg_secondary,
    );
}

fn push_start(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    r: PxRect,
    focused: bool,
    scale: f32,
    theme: &Theme,
) {
    let radius = SEG_RADIUS * scale;
    // Accent-filled primary button; a focus ring brightens it to accent.hover.
    let fill = if focused {
        theme.accent_hover
    } else {
        theme.accent
    };
    quads.push(ChromeQuad::rounded(r, fill, radius));
    center_label(
        labels,
        measure,
        "Start  \u{276f}",
        r,
        FontRole::Body,
        theme.bg_base.to_srgb(),
    );
}

fn center_label(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    text: &str,
    r: PxRect,
    role: FontRole,
    color: skelly_render::Srgb,
) {
    let w = measure.width(text, role, None);
    let line = measure.line_height(role);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x: r.x + (r.w - w) * 0.5,
        y: r.y + (r.h - line) * 0.5,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

/// A rect inset on every side by `d` (physical px), clamped non-negative.
fn inset(r: PxRect, d: f32) -> PxRect {
    PxRect {
        x: r.x + d,
        y: r.y + d,
        w: (r.w - 2.0 * d).max(0.0),
        h: (r.h - 2.0 * d).max(0.0),
    }
}

/// Step an index `+1`/`-1` with wraparound over `len`.
fn step(i: usize, len: usize, forward: bool) -> usize {
    if forward {
        (i + 1) % len
    } else {
        (i + len - 1) % len
    }
}

#[cfg(test)]
mod tests {
    use super::{build, hit, Field, Hit, Onboarding, SHELLS, THEMES};
    use skelly_render::{PxRect, TextMeasure, Theme};

    fn card() -> PxRect {
        let (w, h) = Onboarding::card_size(2.0);
        PxRect {
            x: 100.0,
            y: 80.0,
            w,
            h,
        }
    }

    #[test]
    fn defaults_are_zsh_and_ossein_dark() {
        let o = Onboarding::new();
        assert_eq!(o.shell_program(), "zsh");
        assert_eq!(o.theme_name(), "ossein-dark");
    }

    #[test]
    fn horizontal_changes_selection_and_reports_theme_preview() {
        let mut o = Onboarding::new();
        // Focus starts on Shell: left/right change the shell, no preview.
        assert!(!o.horizontal(true));
        assert_eq!(o.shell_program(), SHELLS[1]);
        // Move focus to Theme: horizontal now reports a preview change.
        o.cycle_focus(true);
        assert!(o.horizontal(true), "theme change asks for a live preview");
        assert_eq!(o.theme_name(), THEMES[1].0);
    }

    #[test]
    fn enter_starts_unless_focused_on_skip() {
        let mut o = Onboarding::new();
        assert!(o.enter_is_start());
        o.click(Hit::Skip);
        assert_eq!(o.focus, Field::Skip);
        assert!(!o.enter_is_start(), "Enter on Skip does not start");
    }

    #[test]
    fn hit_maps_clicks_to_controls_and_build_draws_them() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let mut o = Onboarding::new();
        let panel = card();
        // A click in the middle shell segment selects bash.
        let mid = &super::layout(panel, 2.0).shells[1];
        let h = hit(panel, 2.0, mid.x + mid.w * 0.5, mid.y + mid.h * 0.5);
        assert_eq!(h, Some(Hit::Shell(1)));
        o.click(Hit::Shell(1));
        assert_eq!(o.shell_program(), "bash");
        // Build emits the buttons + selected controls.
        let (quads, labels) = build(&o, panel, 2.0, &theme, &mut m);
        assert!(!quads.is_empty());
        let joined: String = labels.iter().map(|l| l.text.clone()).collect();
        assert!(joined.contains("Welcome to skelly"));
        assert!(joined.contains("Start"));
        assert!(joined.contains("Skip"));
        assert!(joined.contains("Ossein Dark"));
        // The vertebra mark contributes diamond quads.
        assert!(quads.iter().any(|q| q.diamond), "the brand mark is drawn");
    }
}
