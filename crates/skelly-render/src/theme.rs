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
    /// `accent` - brand / cursor color (sRGB). Also the focused pane's `border.strong`
    /// ring per the design guide.
    pub accent: Srgb,
    /// `border` - the subtle divider drawn between tiled panes (sRGB).
    pub border: Srgb,
}

impl Theme {
    /// Resolve a theme by its config name. Unknown or dark-only names fall back to
    /// Ossein Dark, matching the design guide's fallback behavior.
    #[must_use]
    pub fn resolve(name: &str) -> Self {
        match name {
            // Ossein Light: bg.base #EFF1F5, fg.primary #4C4F69, accent #8839EF,
            // border #BCC0CC (the shared Catppuccin-Latte surface the palette derives
            // from - see design/README.md).
            "ossein-light" => Self {
                bg_base: srgb_hex(0xEF, 0xF1, 0xF5),
                fg_primary: Srgb {
                    r: 0x4C,
                    g: 0x4F,
                    b: 0x69,
                },
                accent: Srgb {
                    r: 0x88,
                    g: 0x39,
                    b: 0xEF,
                },
                border: Srgb {
                    r: 0xBC,
                    g: 0xC0,
                    b: 0xCC,
                },
            },
            // Ossein Dark (default): bg.base #181825, fg.primary #CDD6F4, accent
            // #BD93F9, border #313244 (an Ossein surface color used in the guide).
            _ => Self {
                bg_base: srgb_hex(0x18, 0x18, 0x25),
                fg_primary: Srgb {
                    r: 0xCD,
                    g: 0xD6,
                    b: 0xF4,
                },
                accent: Srgb {
                    r: 0xBD,
                    g: 0x93,
                    b: 0xF9,
                },
                border: Srgb {
                    r: 0x31,
                    g: 0x32,
                    b: 0x44,
                },
            },
        }
    }
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
