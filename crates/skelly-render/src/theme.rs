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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// `fg.primary` - primary UI/terminal text (sRGB, for glyph rendering).
    pub fg_primary: Srgb,
    /// `fg.secondary` - secondary UI text; also the palette's command labels (sRGB).
    pub fg_secondary: Srgb,
    /// `fg.muted` - muted UI text: key hints, group labels, placeholders (sRGB).
    pub fg_muted: Srgb,
    /// `accent` - brand / terminal cursor color; the focus ring on interactive UI
    /// elements (sRGB).
    pub accent: Srgb,
    /// `bg.elevated` - the surface for menus, the command palette, and modals (sRGB).
    pub bg_elevated: Srgb,
    /// `border` - the subtle divider drawn between tiled panes (sRGB).
    pub border: Srgb,
    /// `border.strong` - the stronger border on the focused pane and on elevated
    /// surfaces (sRGB).
    pub border_strong: Srgb,
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
                fg_primary: srgb(0x4C, 0x4F, 0x69),
                fg_secondary: srgb(0x5C, 0x5F, 0x77),
                fg_muted: srgb(0x8C, 0x8F, 0xA1),
                accent: srgb(0x88, 0x39, 0xEF),
                bg_elevated: srgb(0xFF, 0xFF, 0xFF),
                border: srgb(0xBC, 0xC0, 0xCC),
                border_strong: srgb(0xAC, 0xB0, 0xBE),
            },
            // Ossein Dark (default) - the guide's token table.
            _ => Self {
                bg_base: srgb_hex(0x18, 0x18, 0x25),
                fg_primary: srgb(0xCD, 0xD6, 0xF4),
                fg_secondary: srgb(0xBA, 0xC2, 0xDE),
                fg_muted: srgb(0x7F, 0x84, 0x9C),
                accent: srgb(0xBD, 0x93, 0xF9),
                bg_elevated: srgb(0x38, 0x3A, 0x54),
                border: srgb(0x31, 0x32, 0x44),
                border_strong: srgb(0x6C, 0x6F, 0x93),
            },
        }
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
    fn srgb_endpoints_map_to_linear_endpoints() {
        assert!((srgb_to_linear(0) - 0.0).abs() < 1e-9);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-9);
    }
}
