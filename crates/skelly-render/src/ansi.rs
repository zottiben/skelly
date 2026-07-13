//! The terminal ANSI palette and 256-color resolution.
//!
//! This is the color set programs *inside* the terminal use - deliberately separate
//! from the UI semantic tokens (Hard rule 2), so a user can pair any ANSI scheme
//! with any UI theme. M2a ships the Ossein Dark / Light 16-color palettes from the
//! design guide; importable schemes come later.

use crate::theme::Srgb;

/// A resolved terminal palette: the 16 ANSI base colors plus the default
/// foreground and background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnsiPalette {
    base: [Srgb; 16],
    default_fg: Srgb,
    default_bg: Srgb,
}

impl AnsiPalette {
    /// Resolve the palette for a theme name. Unknown / dark-only names fall back to
    /// Ossein Dark (matching the design guide's fallback).
    #[must_use]
    pub fn resolve(name: &str) -> Self {
        match name {
            "ossein-light" => Self {
                base: OSSEIN_LIGHT,
                default_fg: rgb(0x4C, 0x4F, 0x69),
                default_bg: rgb(0xEF, 0xF1, 0xF5),
            },
            "tokyonight" => Self {
                base: TOKYONIGHT,
                default_fg: rgb(0xC0, 0xCA, 0xF5),
                default_bg: rgb(0x1A, 0x1B, 0x26),
            },
            "kanagawa" => Self {
                base: KANAGAWA,
                default_fg: rgb(0xDC, 0xD7, 0xBA),
                default_bg: rgb(0x1F, 0x1F, 0x28),
            },
            _ => Self {
                base: OSSEIN_DARK,
                default_fg: rgb(0xCD, 0xD6, 0xF4),
                default_bg: rgb(0x18, 0x18, 0x25),
            },
        }
    }

    /// The default foreground (`fg.primary`), used for cells with no explicit color.
    #[must_use]
    pub fn default_fg(&self) -> Srgb {
        self.default_fg
    }

    /// The default background, used as the solid fill when reverse video inverts a
    /// cell that has no explicit background.
    #[must_use]
    pub fn default_bg(&self) -> Srgb {
        self.default_bg
    }

    /// Resolve a palette index to a concrete color: 0..=15 are the ANSI base colors,
    /// 16..=231 the 6x6x6 color cube, 232..=255 the 24-step grayscale ramp.
    #[must_use]
    pub fn indexed(&self, index: u8) -> Srgb {
        match index {
            0..=15 => self.base[usize::from(index)],
            16..=231 => {
                let n = index - 16;
                rgb(cube(n / 36), cube((n % 36) / 6), cube(n % 6))
            }
            232..=255 => {
                let value = 8 + (index - 232) * 10;
                rgb(value, value, value)
            }
        }
    }
}

/// One channel of the 6x6x6 color cube (0, 95, 135, 175, 215, 255).
fn cube(step: u8) -> u8 {
    if step == 0 {
        0
    } else {
        55 + step * 40
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Srgb {
    Srgb { r, g, b }
}

/// Ossein Dark ANSI 16 (design guide, section 04).
const OSSEIN_DARK: [Srgb; 16] = [
    rgb(0x18, 0x18, 0x25), // black
    rgb(0xF3, 0x8B, 0xA8), // red
    rgb(0xA6, 0xE3, 0xA1), // green
    rgb(0xBD, 0x93, 0xF9), // yellow
    rgb(0x89, 0xB4, 0xFA), // blue
    rgb(0xF5, 0xC2, 0xE7), // magenta
    rgb(0x94, 0xE2, 0xD5), // cyan
    rgb(0xBA, 0xC2, 0xDE), // white
    rgb(0x52, 0x52, 0x6A), // bright black
    rgb(0xFF, 0x8F, 0xA8), // bright red
    rgb(0xB8, 0xED, 0xB0), // bright green
    rgb(0xD6, 0xBB, 0xFC), // bright yellow
    rgb(0xA6, 0xC8, 0xFF), // bright blue
    rgb(0xFB, 0xD0, 0xEF), // bright magenta
    rgb(0xA8, 0xED, 0xE0), // bright cyan
    rgb(0xCD, 0xD6, 0xF4), // bright white
];

/// Ossein Light ANSI 16 (design guide, section 04).
const OSSEIN_LIGHT: [Srgb; 16] = [
    rgb(0x4C, 0x4F, 0x69), // black
    rgb(0xD2, 0x0F, 0x39), // red
    rgb(0x40, 0xA0, 0x2B), // green
    rgb(0x88, 0x39, 0xEF), // yellow
    rgb(0x1E, 0x66, 0xF5), // blue
    rgb(0xEA, 0x76, 0xCB), // magenta
    rgb(0x17, 0x92, 0x99), // cyan
    rgb(0x5C, 0x5F, 0x77), // white
    rgb(0x8C, 0x8F, 0xA1), // bright black
    rgb(0xE6, 0x45, 0x53), // bright red
    rgb(0x5A, 0x9E, 0x2F), // bright green
    rgb(0x94, 0x50, 0xF0), // bright yellow
    rgb(0x3B, 0x7D, 0xF5), // bright blue
    rgb(0xC0, 0x63, 0xA8), // bright magenta
    rgb(0x2A, 0xA5, 0x98), // bright cyan
    rgb(0x4C, 0x4F, 0x69), // bright white
];

/// Tokyo Night ANSI 16 (tokyonight.nvim's published terminal colors).
const TOKYONIGHT: [Srgb; 16] = [
    rgb(0x15, 0x16, 0x1E), // black
    rgb(0xF7, 0x76, 0x8E), // red
    rgb(0x9E, 0xCE, 0x6A), // green
    rgb(0xE0, 0xAF, 0x68), // yellow
    rgb(0x7A, 0xA2, 0xF7), // blue
    rgb(0xBB, 0x9A, 0xF7), // magenta
    rgb(0x7D, 0xCF, 0xFF), // cyan
    rgb(0xA9, 0xB1, 0xD6), // white
    rgb(0x41, 0x48, 0x68), // bright black
    rgb(0xF7, 0x76, 0x8E), // bright red
    rgb(0x9E, 0xCE, 0x6A), // bright green
    rgb(0xE0, 0xAF, 0x68), // bright yellow
    rgb(0x7A, 0xA2, 0xF7), // bright blue
    rgb(0xBB, 0x9A, 0xF7), // bright magenta
    rgb(0x7D, 0xCF, 0xFF), // bright cyan
    rgb(0xC0, 0xCA, 0xF5), // bright white
];

/// Kanagawa (wave) ANSI 16 (kanagawa.nvim's published terminal colors).
const KANAGAWA: [Srgb; 16] = [
    rgb(0x09, 0x06, 0x18), // black
    rgb(0xC3, 0x40, 0x43), // red
    rgb(0x76, 0x94, 0x6A), // green
    rgb(0xC0, 0xA3, 0x6E), // yellow
    rgb(0x7E, 0x9C, 0xD8), // blue
    rgb(0x95, 0x7F, 0xB8), // magenta
    rgb(0x6A, 0x95, 0x89), // cyan
    rgb(0xC8, 0xC0, 0x93), // white
    rgb(0x72, 0x71, 0x69), // bright black
    rgb(0xE8, 0x24, 0x24), // bright red
    rgb(0x98, 0xBB, 0x6C), // bright green
    rgb(0xE6, 0xC3, 0x84), // bright yellow
    rgb(0x7F, 0xB4, 0xCA), // bright blue
    rgb(0x93, 0x8A, 0xA9), // bright magenta
    rgb(0x7A, 0xA8, 0x9F), // bright cyan
    rgb(0xDC, 0xD7, 0xBA), // bright white
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_indices_match_the_palette() {
        let p = AnsiPalette::resolve("ossein-dark");
        assert_eq!(p.indexed(1), rgb(0xF3, 0x8B, 0xA8)); // red
        assert_eq!(p.indexed(4), rgb(0x89, 0xB4, 0xFA)); // blue
    }

    #[test]
    fn cube_and_grayscale_endpoints() {
        let p = AnsiPalette::resolve("ossein-dark");
        assert_eq!(p.indexed(16), rgb(0, 0, 0)); // cube origin
        assert_eq!(p.indexed(231), rgb(255, 255, 255)); // cube max
        assert_eq!(p.indexed(232), rgb(8, 8, 8)); // darkest gray
        assert_eq!(p.indexed(255), rgb(238, 238, 238)); // lightest gray
    }
}
