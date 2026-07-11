//! Semantic theme tokens resolved to GPU-ready colors.
//!
//! M1a resolves only the surface background we need to clear to; this grows into
//! the full `category.role.state` resolver in M2/M3. The UI reads these tokens,
//! never raw hex (Hard rule 2), and they are kept distinct from the terminal ANSI
//! palette so any scheme pairs with any theme.

/// A resolved color in **linear** RGBA (0.0..=1.0), ready to hand to the GPU.
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

/// The active UI theme's resolved surface tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// `bg.base` - the app and terminal background.
    pub bg_base: Rgba,
}

impl Theme {
    /// Resolve a theme by its config name. Unknown or dark-only names fall back to
    /// Ossein Dark, matching the design guide's fallback behavior.
    #[must_use]
    pub fn resolve(name: &str) -> Self {
        match name {
            // Ossein Light `bg.base` = #EFF1F5.
            "ossein-light" => Self {
                bg_base: srgb_hex(0xEF, 0xF1, 0xF5),
            },
            // Ossein Dark `bg.base` = #181825 (default).
            _ => Self {
                bg_base: srgb_hex(0x18, 0x18, 0x25),
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
        // Unknown names fall back to Ossein Dark (#181825).
        assert_eq!(Theme::resolve("nope"), Theme::resolve("ossein-dark"));
    }

    #[test]
    fn dark_and_light_backgrounds_differ() {
        assert_ne!(
            Theme::resolve("ossein-dark"),
            Theme::resolve("ossein-light")
        );
    }

    #[test]
    fn srgb_endpoints_map_to_linear_endpoints() {
        assert!((srgb_to_linear(0) - 0.0).abs() < 1e-9);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-9);
    }
}
