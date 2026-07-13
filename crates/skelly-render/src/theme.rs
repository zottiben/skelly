//! Semantic theme tokens resolved to GPU-ready colors.
//!
//! M1 resolves the surface background (for the clear) and the primary text color
//! (for glyphs); this grows into the full `category.role.state` resolver in M2/M3.
//! The UI reads these tokens, never raw hex (Hard rule 2), and they are kept
//! distinct from the terminal ANSI palette so any scheme pairs with any theme.

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
    /// Resolve a theme by its config name. Unknown or dark-only names fall back to
    /// Ossein Dark, matching the design guide's fallback behavior.
    #[must_use]
    pub fn resolve(name: &str) -> Self {
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
            // Tokyo Night (the canonical "night" palette; the guide's config lists it as a
            // preset). Mapped from tokyonight.nvim's published colors to Skelly's semantic tokens.
            "tokyonight" => Self {
                bg_base: srgb_hex(0x1A, 0x1B, 0x26),
                bg_sidebar: srgb(0x16, 0x16, 0x1E),
                bg_surface: srgb(0x29, 0x2E, 0x42),
                bg_elevated: srgb(0x2F, 0x35, 0x49),
                bg_inset: srgb(0x16, 0x16, 0x1E),
                fg_primary: srgb(0xC0, 0xCA, 0xF5),
                fg_secondary: srgb(0xA9, 0xB1, 0xD6),
                fg_muted: srgb(0x56, 0x5F, 0x89),
                fg_faint: srgb(0x41, 0x48, 0x68),
                accent: srgb(0x7A, 0xA2, 0xF7),
                accent_hover: srgb(0x9E, 0xB8, 0xF9),
                accent_subtle_alpha: 0.16,
                border_subtle: srgb(0x1F, 0x23, 0x35),
                border: srgb(0x29, 0x2E, 0x42),
                border_strong: srgb(0x54, 0x5C, 0x7E),
                diff_add: srgb(0x9E, 0xCE, 0x6A),
                diff_del: srgb(0xF7, 0x76, 0x8E),
                diff_hunk: srgb(0x7A, 0xA2, 0xF7),
            },
            // Kanagawa (the "wave" palette; the guide's config lists it as a preset). Mapped from
            // kanagawa.nvim's published colors.
            "kanagawa" => Self {
                bg_base: srgb_hex(0x1F, 0x1F, 0x28),
                bg_sidebar: srgb(0x16, 0x16, 0x1D),
                bg_surface: srgb(0x2A, 0x2A, 0x37),
                bg_elevated: srgb(0x36, 0x36, 0x46),
                bg_inset: srgb(0x16, 0x16, 0x1D),
                fg_primary: srgb(0xDC, 0xD7, 0xBA),
                fg_secondary: srgb(0xC8, 0xC0, 0x93),
                fg_muted: srgb(0x72, 0x71, 0x69),
                fg_faint: srgb(0x54, 0x54, 0x64),
                accent: srgb(0x7E, 0x9C, 0xD8),
                accent_hover: srgb(0x7F, 0xB4, 0xCA),
                accent_subtle_alpha: 0.16,
                border_subtle: srgb(0x22, 0x22, 0x2C),
                border: srgb(0x2A, 0x2A, 0x37),
                border_strong: srgb(0x54, 0x54, 0x6D),
                diff_add: srgb(0x76, 0x94, 0x6A),
                diff_del: srgb(0xC3, 0x40, 0x43),
                diff_hunk: srgb(0x7E, 0x9C, 0xD8),
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
    fn preset_themes_resolve_to_distinct_palettes() {
        // Tokyo Night + Kanagawa are real, distinct themes (not the Ossein fallback).
        let ossein = Theme::resolve("ossein-dark");
        let tokyo = Theme::resolve("tokyonight");
        let kana = Theme::resolve("kanagawa");
        assert_ne!(tokyo.bg_base, ossein.bg_base);
        assert_ne!(kana.bg_base, ossein.bg_base);
        assert_ne!(tokyo.bg_base, kana.bg_base);
        assert_ne!(tokyo.accent, kana.accent);
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
