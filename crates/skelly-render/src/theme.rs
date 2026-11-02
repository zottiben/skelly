//! Semantic theme tokens resolved to GPU-ready colors.
//!
//! M1 resolves the surface background (for the clear) and the primary text color
//! (for glyphs); this grows into the full `category.role.state` resolver in M2/M3.
//! The UI reads these tokens, never raw hex (Hard rule 2), and they are kept
//! distinct from the terminal ANSI palette so any scheme pairs with any theme.

use crate::palettes::{self, Variant};

/// Every selectable theme as `(config name, display name)`, in picker order: Skelly's own
/// Ossein pair, then the ai-team families. Each family is a dark then light pair, which
/// [`toggled`] relies on.
pub const THEMES: &[(&str, &str)] = &[
    ("ossein-dark", "Ossein Dark"),
    ("ossein-light", "Ossein Light"),
    ("vice-city", "Vice City Dark"),
    ("vice-city-light", "Vice City Light"),
    ("gruvbox", "Gruvbox Dark"),
    ("gruvbox-light", "Gruvbox Light"),
    ("tokyonight", "Tokyo Night Dark"),
    ("tokyonight-light", "Tokyo Night Light"),
    ("catppuccin", "Catppuccin Dark"),
    ("catppuccin-light", "Catppuccin Light"),
    ("everforest", "Everforest Dark"),
    ("everforest-light", "Everforest Light"),
    ("kanagawa", "Kanagawa Dark"),
    ("kanagawa-light", "Kanagawa Light"),
];

/// The display names of [`THEMES`], in the same order (a settings picker's options).
pub const THEME_LABELS: [&str; THEMES.len()] = {
    let mut labels = [""; THEMES.len()];
    let mut i = 0;
    while i < THEMES.len() {
        labels[i] = THEMES[i].1;
        i += 1;
    }
    labels
};

/// The theme the light/dark toggle switches `name` to: the other variant of its family, which
/// is its pair in [`THEMES`]. A name outside [`THEMES`] resolves as Ossein Dark, so it toggles
/// to Ossein Light.
#[must_use]
pub fn toggled(name: &str) -> &'static str {
    let index = THEMES.iter().position(|(id, _)| *id == name).unwrap_or(0);
    THEMES[index ^ 1].0
}

/// A resolved color in **linear** RGBA (0.0..=1.0), ready to hand to the GPU as a
/// clear value.
///
/// Colors in the design guide are 8-bit sRGB; [`Theme::resolve`] converts them to
/// linear space here so they appear correct on an sRGB surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    /// Red, linear 0.0..=1.0.
    pub r: f64,
    /// Green, linear 0.0..=1.0.
    pub g: f64,
    /// Blue, linear 0.0..=1.0.
    pub b: f64,
    /// Alpha, 0.0..=1.0.
    pub a: f64,
}

/// An 8-bit sRGB color, as the design guide specifies it. Text rasterizers
/// (glyphon/cosmic-text) take sRGB bytes directly, so glyph colors stay in this
/// form rather than being pre-linearized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Srgb {
    /// Red, 0..=255.
    pub r: u8,
    /// Green, 0..=255.
    pub g: u8,
    /// Blue, 0..=255.
    pub b: u8,
}

impl Srgb {
    /// This color as an opaque linear [`Rgba`] (the GPU clear).
    pub(crate) fn to_rgba(self) -> Rgba {
        srgb_hex(self.r, self.g, self.b)
    }

    /// Convert to linear RGBA (alpha 1.0) for a GPU vertex color on an sRGB surface.
    #[must_use]
    pub(crate) fn to_linear(self) -> [f32; 4] {
        [lin(self.r), lin(self.g), lin(self.b), 1.0]
    }

    /// Composite this color over an opaque `base` at `alpha` (clamped to `0..=1`) in **sRGB
    /// (gamma) space** - exactly how the guide's CSS `rgba()` tints composite, and the opaque
    /// result to fill. A GPU alpha blend of the same tint happens in *linear* space, which over
    /// a dark surface reads noticeably brighter/more saturated than the guide, so chrome tints
    /// on a known solid background (chips, the active-tab pill, palette rows) pre-composite here
    /// instead of relying on [`ChromeQuad::tint`](crate::ChromeQuad::tint).
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the blended channel is a rounded value in 0.0..=255.0"
    )]
    pub fn over(self, base: Srgb, alpha: f32) -> Srgb {
        let a = alpha.clamp(0.0, 1.0);
        let mix = |f: u8, b: u8| (f32::from(f) * a + f32::from(b) * (1.0 - a)).round() as u8;
        Srgb {
            r: mix(self.r, base.r),
            g: mix(self.g, base.g),
            b: mix(self.b, base.b),
        }
    }
}

impl Rgba {
    /// As a linear `[f32; 4]` GPU vertex color (this color is already linear).
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "linear channels are in 0.0..=1.0; the f64->f32 narrowing is lossless enough for a fill"
    )]
    pub(crate) fn to_array(self) -> [f32; 4] {
        [self.r as f32, self.g as f32, self.b as f32, self.a as f32]
    }

    /// The opaque 8-bit sRGB view of this (linear) color, for compositing chrome tints over
    /// `bg.base` in sRGB space via [`Srgb::over`] (alpha is dropped). The inverse of the
    /// sRGB->linear conversion [`Theme::resolve`] applies, so it recovers the source hex.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "each channel maps to a rounded value in 0.0..=255.0"
    )]
    pub fn to_srgb(self) -> Srgb {
        let enc = |c: f64| {
            let s = if c <= 0.003_130_8 {
                c * 12.92
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round().clamp(0.0, 255.0) as u8
        };
        Srgb {
            r: enc(self.r),
            g: enc(self.g),
            b: enc(self.b),
        }
    }
}

/// Convert one 8-bit sRGB channel to a linear `f32` (0.0..=1.0).
fn lin(channel: u8) -> f32 {
    let c = f32::from(channel) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The active UI theme's resolved surface tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// `bg.base` - the app and terminal background (linear, for the GPU clear).
    pub bg_base: Rgba,
    /// `bg.sidebar` - the tab sidebar surface, one step off `bg.base` (sRGB).
    pub bg_sidebar: Srgb,
    /// `bg.surface` - the base for panels, cards, and popovers - the docks (sRGB).
    pub bg_surface: Srgb,
    /// `bg.elevated` - the surface for menus, the command palette, and modals (sRGB).
    pub bg_elevated: Srgb,
    /// `bg.inset` - the recessed surface for inputs, wells, and code blocks (sRGB).
    pub bg_inset: Srgb,
    /// `fg.primary` - primary UI/terminal text (sRGB, for glyph rendering).
    pub fg_primary: Srgb,
    /// `fg.secondary` - secondary UI text; also the palette's command labels (sRGB).
    pub fg_secondary: Srgb,
    /// `fg.muted` - muted UI text: key hints, group labels, placeholders (sRGB).
    pub fg_muted: Srgb,
    /// `fg.faint` - disabled text and the empty-state watermark (sRGB).
    pub fg_faint: Srgb,
    /// `accent` - brand / terminal cursor color; the focus ring on interactive UI
    /// elements (sRGB).
    pub accent: Srgb,
    /// `accent.hover` - the brighter accent for hover / pressed states (sRGB).
    pub accent_hover: Srgb,
    /// `accent.subtle` alpha (§03: 0.14 dark / 0.12 light) - the accent tint weight for
    /// selected-row / active-chip fills, composited over their surface by [`Theme::accent_subtle_on`].
    pub accent_subtle_alpha: f32,
    /// `border.subtle` - hairline dividers and inner separators (sRGB).
    pub border_subtle: Srgb,
    /// `border` (`border.default`) - card and pane edges (sRGB).
    pub border: Srgb,
    /// `border.strong` - the stronger border on the focused pane and on elevated
    /// surfaces (sRGB).
    pub border_strong: Srgb,
    /// `diff.add` - added-line text/gutter in the git diff dock; its background is the
    /// same hue drawn translucent (sRGB). Separate from the ANSI palette (Hard rule 2).
    pub diff_add: Srgb,
    /// `diff.del` - removed-line text/gutter in the git diff dock; its background is the
    /// same hue drawn translucent (sRGB).
    pub diff_del: Srgb,
    /// `diff.hunk` - the `@@` hunk-header color (and the diff dock's branch label);
    /// its background is the same hue drawn translucent (sRGB).
    pub diff_hunk: Srgb,
}

impl Theme {
    /// Resolve a theme by its config name (see [`THEMES`]). Unknown names fall back to
    /// Ossein Dark, matching the design guide's fallback behavior.
    #[must_use]
    pub fn resolve(name: &str) -> Self {
        if let Some(variant) = palettes::variant(name) {
            return Self::from_variant(variant);
        }
        match name {
            // Ossein Light (see the guide's token table; border #BCC0CC is the shared
            // Catppuccin-Latte surface the palette derives from - see design/README).
            "ossein-light" => Self {
                bg_base: srgb_hex(0xEF, 0xF1, 0xF5),
                bg_sidebar: srgb(0xE6, 0xE9, 0xEF),
                bg_surface: srgb(0xF7, 0xF8, 0xFC),
                bg_elevated: srgb(0xFF, 0xFF, 0xFF),
                bg_inset: srgb(0xE6, 0xE9, 0xEF),
                fg_primary: srgb(0x4C, 0x4F, 0x69),
                fg_secondary: srgb(0x5C, 0x5F, 0x77),
                fg_muted: srgb(0x8C, 0x8F, 0xA1),
                fg_faint: srgb(0xBC, 0xC0, 0xCC),
                accent: srgb(0x88, 0x39, 0xEF),
                accent_hover: srgb(0x94, 0x50, 0xF0),
                accent_subtle_alpha: 0.12,
                border_subtle: srgb(0xDC, 0xE0, 0xE8),
                border: srgb(0xCC, 0xD0, 0xDA),
                border_strong: srgb(0xAC, 0xB0, 0xBE),
                diff_add: srgb(0x40, 0xA0, 0x2B),
                diff_del: srgb(0xD2, 0x0F, 0x39),
                diff_hunk: srgb(0x1E, 0x66, 0xF5),
            },
            // Ossein Dark (default) - the guide's token table.
            _ => Self {
                bg_base: srgb_hex(0x18, 0x18, 0x25),
                bg_sidebar: srgb(0x1E, 0x1E, 0x2E),
                bg_surface: srgb(0x31, 0x32, 0x44),
                bg_elevated: srgb(0x38, 0x3A, 0x54),
                bg_inset: srgb(0x1A, 0x1A, 0x2B),
                fg_primary: srgb(0xCD, 0xD6, 0xF4),
                fg_secondary: srgb(0xBA, 0xC2, 0xDE),
                fg_muted: srgb(0x7F, 0x84, 0x9C),
                fg_faint: srgb(0x52, 0x52, 0x6A),
                accent: srgb(0xBD, 0x93, 0xF9),
                accent_hover: srgb(0xD6, 0xBB, 0xFC),
                accent_subtle_alpha: 0.14,
                border_subtle: srgb(0x2E, 0x2E, 0x44),
                border: srgb(0x3A, 0x3A, 0x54),
                border_strong: srgb(0x6C, 0x6F, 0x93),
                diff_add: srgb(0xA6, 0xE3, 0xA1),
                diff_del: srgb(0xF3, 0x8B, 0xA8),
                diff_hunk: srgb(0x89, 0xB4, 0xFA),
            },
        }
    }

    /// Map an ai-team variant onto the semantic tokens. The terminal surface is `bg.base`, the
    /// app floor is the sidebar, and a key's face is `bg.surface`. `bg.elevated` is the most
    /// lifted surface (dark: `--p-hover`; light: the screen) and `bg.inset` the most recessed
    /// (dark: the screen; light: a panel). The muted / faint inks and the borders are `text`
    /// over the screen at the same weights Ossein's tokens sit at.
    fn from_variant(v: &Variant) -> Self {
        let ink = |alpha: f32| v.text.over(v.screen, alpha);
        Self {
            bg_base: v.screen.to_rgba(),
            bg_sidebar: v.floor,
            bg_surface: v.raised,
            bg_elevated: if v.light { v.screen } else { v.hover },
            bg_inset: if v.light { v.panel } else { v.screen },
            fg_primary: v.text,
            fg_secondary: v.secondary,
            fg_muted: ink(0.58),
            fg_faint: ink(0.32),
            accent: v.accent,
            accent_hover: v.text.over(v.accent, 0.3),
            accent_subtle_alpha: if v.light { 0.12 } else { 0.16 },
            border_subtle: ink(0.12),
            border: ink(0.2),
            border_strong: ink(0.45),
            diff_add: v.added,
            diff_del: v.deleted,
            diff_hunk: v.hunk,
        }
    }

    /// The opaque fill for an `accent.subtle` (§03) tint over the opaque `base` surface -
    /// the accent composited at this theme's [`accent_subtle_alpha`](Self::accent_subtle_alpha)
    /// in sRGB space (see [`Srgb::over`]). Used for selected-row / active-chip / active-tab
    /// fills so they read at the guide's weight instead of the brighter GPU linear-space blend.
    #[must_use]
    pub fn accent_subtle_on(&self, base: Srgb) -> Srgb {
        self.accent.over(base, self.accent_subtle_alpha)
    }
}

/// An 8-bit sRGB color from its channels - a terse constructor for the token table.
fn srgb(r: u8, g: u8, b: u8) -> Srgb {
    Srgb { r, g, b }
}

/// Build an opaque [`Rgba`] from an 8-bit sRGB triple, converting to linear space.
fn srgb_hex(r: u8, g: u8, b: u8) -> Rgba {
    Rgba {
        r: srgb_to_linear(r),
        g: srgb_to_linear(g),
        b: srgb_to_linear(b),
        a: 1.0,
    }
}

/// Convert one 8-bit sRGB channel to a linear 0.0..=1.0 value.
fn srgb_to_linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_ossein_dark_by_default() {
        // Unknown names fall back to Ossein Dark (#181825 / #CDD6F4).
        assert_eq!(Theme::resolve("nope"), Theme::resolve("ossein-dark"));
    }

    #[test]
    fn dark_and_light_differ() {
        let dark = Theme::resolve("ossein-dark");
        let light = Theme::resolve("ossein-light");
        assert_ne!(dark.bg_base, light.bg_base);
        assert_ne!(dark.fg_primary, light.fg_primary);
    }

    #[test]
    fn every_listed_theme_resolves_to_its_own_tokens() {
        // No two listed themes share a surface + accent, so none is the Ossein fallback in
        // disguise (only `ossein-dark` itself equals it).
        let fallback = Theme::resolve("nope");
        for (i, (a, _)) in THEMES.iter().enumerate() {
            let ta = Theme::resolve(a);
            assert_eq!(ta == fallback, *a == "ossein-dark", "{a}");
            for (b, _) in &THEMES[i + 1..] {
                let tb = Theme::resolve(b);
                assert!(
                    ta.bg_base != tb.bg_base || ta.accent != tb.accent,
                    "{a} == {b}"
                );
            }
        }
    }

    #[test]
    fn light_themes_put_dark_ink_on_light_surfaces() {
        let luma = |c: Srgb| 2 * u32::from(c.r) + 7 * u32::from(c.g) + u32::from(c.b);
        for (name, label) in THEMES {
            let t = Theme::resolve(name);
            let (bg, fg) = (luma(t.bg_base.to_srgb()), luma(t.fg_primary));
            assert_eq!(bg > fg, label.ends_with("Light"), "{name}");
            // The ink steps keep their order on every theme: primary, muted, faint, base.
            let order = [fg, luma(t.fg_muted), luma(t.fg_faint), bg];
            assert!(
                order.windows(2).all(|w| (w[0] < w[1]) == (fg < bg)),
                "{name}: {order:?}"
            );
        }
    }

    #[test]
    fn the_toggle_flips_between_a_familys_dark_and_light_variants() {
        assert_eq!(toggled("ossein-dark"), "ossein-light");
        assert_eq!(toggled("ossein-light"), "ossein-dark");
        assert_eq!(toggled("gruvbox"), "gruvbox-light");
        assert_eq!(toggled("vice-city-light"), "vice-city");
        // An unknown name resolves as Ossein Dark, so it toggles to Ossein Light.
        assert_eq!(toggled("nope"), "ossein-light");
        assert_eq!(toggled("nope-light"), "ossein-light");
        // Every theme pairs with the other variant of its own family.
        let family = |label: &str| label.rsplit_once(' ').unwrap().0.to_owned();
        for (i, (name, label)) in THEMES.iter().enumerate() {
            let (partner, partner_label) = THEMES[i ^ 1];
            assert_eq!(toggled(name), partner);
            assert_eq!(family(label), family(partner_label), "{name}");
            assert_ne!(label.ends_with("Light"), partner_label.ends_with("Light"));
        }
    }

    #[test]
    fn vice_city_maps_its_synth_tokens() {
        let t = Theme::resolve("vice-city");
        assert_eq!(t.bg_base.to_srgb(), srgb(0x0A, 0x01, 0x19)); // --synth-screen
        assert_eq!(t.bg_sidebar, srgb(0x0C, 0x02, 0x1C)); // --synth-floor
        assert_eq!(t.bg_surface, srgb(0x26, 0x10, 0x49)); // --synth-panel-hi
        assert_eq!(t.fg_primary, srgb(0xFD, 0xF4, 0xFF)); // --synth-text
        assert_eq!(t.fg_secondary, srgb(0xA1, 0x98, 0xA8)); // --synth-text-muted, opaque
        assert_eq!(t.accent, srgb(0x2D, 0xE2, 0xE6)); // --synth-accent
        assert_eq!(t.diff_del, srgb(0xFF, 0x6B, 0x35)); // --git-deleted
        assert_eq!(THEME_LABELS[2], "Vice City Dark");
    }

    #[test]
    fn dark_foreground_matches_the_spec() {
        assert_eq!(
            Theme::resolve("ossein-dark").fg_primary,
            Srgb {
                r: 0xCD,
                g: 0xD6,
                b: 0xF4
            }
        );
    }

    #[test]
    fn diff_tokens_match_the_spec_and_differ_by_theme() {
        // The guide's token table: diff.add / diff.del / diff.hunk, dark vs light.
        let dark = Theme::resolve("ossein-dark");
        assert_eq!(dark.diff_add, srgb(0xA6, 0xE3, 0xA1));
        assert_eq!(dark.diff_del, srgb(0xF3, 0x8B, 0xA8));
        assert_eq!(dark.diff_hunk, srgb(0x89, 0xB4, 0xFA));
        let light = Theme::resolve("ossein-light");
        assert_eq!(light.diff_add, srgb(0x40, 0xA0, 0x2B));
        assert_ne!(dark.diff_add, light.diff_add);
    }

    #[test]
    fn srgb_endpoints_map_to_linear_endpoints() {
        assert!((srgb_to_linear(0) - 0.0).abs() < 1e-9);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-9);
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test: exact token alphas + a reference mix mirroring `over`"
    )]
    fn over_composites_in_srgb_space_like_the_guide_css() {
        // `Srgb::over` reproduces the guide's CSS `rgba()` compositing (gamma-space per-channel
        // mix), NOT the brighter linear-space GPU blend. accent #BD93F9 at 0.14 over the sidebar
        // bg #1E1E2E: round(0.14*fg + 0.86*bg) per channel.
        let dark = Theme::resolve("ossein-dark");
        let got = dark.accent.over(dark.bg_sidebar, 0.14);
        let mix = |f: u8, b: u8| (0.14_f32 * f32::from(f) + 0.86 * f32::from(b)).round() as u8;
        assert_eq!(
            got,
            srgb(mix(0xBD, 0x1E), mix(0x93, 0x1E), mix(0xF9, 0x2E),)
        );
        // The opaque result is far dimmer than the bare accent (the whole point of the fix).
        assert!(got.r < dark.accent.r && got.g < dark.accent.g && got.b < dark.accent.b);
        // The endpoints: alpha 0 is the base, alpha 1 is the fg.
        assert_eq!(dark.accent.over(dark.bg_sidebar, 0.0), dark.bg_sidebar);
        assert_eq!(dark.accent.over(dark.bg_sidebar, 1.0), dark.accent);
        // `accent_subtle_on` uses the per-theme token alpha (0.14 dark / 0.12 light).
        assert_eq!(dark.accent_subtle_alpha, 0.14);
        assert_eq!(dark.accent_subtle_on(dark.bg_sidebar), got);
        assert_eq!(Theme::resolve("ossein-light").accent_subtle_alpha, 0.12);
        // `Rgba::to_srgb` recovers bg.base's source hex (#181825) so it can be a composite base.
        assert_eq!(dark.bg_base.to_srgb(), srgb(0x18, 0x18, 0x25));
    }

    #[test]
    fn surface_and_border_tokens_match_the_guide() {
        // The guide's token table (§03): distinct surface + border tokens the UI reads.
        let dark = Theme::resolve("ossein-dark");
        assert_eq!(dark.bg_sidebar, srgb(0x1E, 0x1E, 0x2E)); // one step off bg.base
        assert_eq!(dark.bg_surface, srgb(0x31, 0x32, 0x44));
        assert_eq!(dark.bg_inset, srgb(0x1A, 0x1A, 0x2B));
        assert_eq!(dark.border_subtle, srgb(0x2E, 0x2E, 0x44));
        assert_eq!(dark.border, srgb(0x3A, 0x3A, 0x54)); // border.default, corrected
        assert_eq!(dark.fg_faint, srgb(0x52, 0x52, 0x6A));
        assert_eq!(dark.accent_hover, srgb(0xD6, 0xBB, 0xFC));
        let light = Theme::resolve("ossein-light");
        assert_eq!(light.bg_sidebar, srgb(0xE6, 0xE9, 0xEF));
        assert_eq!(light.border, srgb(0xCC, 0xD0, 0xDA));
        assert_eq!(light.fg_faint, srgb(0xBC, 0xC0, 0xCC));
    }
}
