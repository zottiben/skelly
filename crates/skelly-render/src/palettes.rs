//! The ai-team palette families as Skelly themes: Vice City, Gruvbox, Tokyo Night, Catppuccin,
//! Everforest and Kanagawa, each dark (`<family>`) and light (`<family>-light`).
//!
//! The colors are copied from ai-team's `ui/src/synth/tokens.css`. That file wires one set of
//! `--p-*` values per variant to its UI and terminal tokens; [`wired`] repeats that wiring, and
//! Vice City (which has its own tokens rather than `--p-*` values) is written out. A variant
//! then resolves to Skelly's semantic UI tokens ([`Theme`](crate::Theme)) and, separately, to
//! its terminal colors ([`AnsiPalette`](crate::AnsiPalette)), as Hard rule 2 keeps them apart.

use crate::theme::Srgb;

/// One variant: its surface ladder, inks, accent, diff hues and terminal colors.
pub(crate) struct Variant {
    /// Whether this is a light variant.
    pub(crate) light: bool,
    /// `--synth-screen`: the terminal and code surface.
    pub(crate) screen: Srgb,
    /// `--synth-floor`: the app floor around the panels.
    pub(crate) floor: Srgb,
    /// `--synth-panel`: panels.
    pub(crate) panel: Srgb,
    /// `--synth-panel-hi`: raised panels and the face of a key.
    pub(crate) raised: Srgb,
    /// `--p-hover`: the most lifted surface of a dark variant (Vice City: `--synth-plate-hi`, as
    /// its key-hover face is a saturated violet, not a surface). Light variants do not use it,
    /// as their `--p-hover` is darker than the screen.
    pub(crate) hover: Srgb,
    /// `--synth-text`.
    pub(crate) text: Srgb,
    /// `--synth-text-muted`, opaque.
    pub(crate) secondary: Srgb,
    /// `--synth-accent`: focus, the terminal cursor and switches.
    pub(crate) accent: Srgb,
    /// `--git-added`.
    pub(crate) added: Srgb,
    /// `--git-deleted`.
    pub(crate) deleted: Srgb,
    /// The terminal blue, for hunk headers.
    pub(crate) hunk: Srgb,
    /// `--synth-term-*`: the 16 ANSI colors, black to bright white.
    pub(crate) ansi: [Srgb; 16],
    /// `--synth-term-fg`.
    pub(crate) term_fg: Srgb,
}

/// The variant a theme name selects, or `None` for a name outside the ai-team families.
pub(crate) fn variant(name: &str) -> Option<&'static Variant> {
    Some(match name {
        "vice-city" => &VICE_CITY,
        "vice-city-light" => &VICE_CITY_LIGHT,
        "gruvbox" => &GRUVBOX,
        "gruvbox-light" => &GRUVBOX_LIGHT,
        "tokyonight" => &TOKYO_NIGHT,
        "tokyonight-light" => &TOKYO_NIGHT_LIGHT,
        "catppuccin" => &CATPPUCCIN,
        "catppuccin-light" => &CATPPUCCIN_LIGHT,
        "everforest" => &EVERFOREST,
        "everforest-light" => &EVERFOREST_LIGHT,
        "kanagawa" => &KANAGAWA,
        "kanagawa-light" => &KANAGAWA_LIGHT,
        _ => return None,
    })
}

/// A family variant's `--p-*` values. A light variant restates only some of them; `deep` is
/// inherited from the dark one.
struct P {
    screen: Srgb,
    floor: Srgb,
    panel: Srgb,
    raised: Srgb,
    hover: Srgb,
    text: Srgb,
    muted: Srgb,
    red: Srgb,
    green: Srgb,
    yellow: Srgb,
    blue: Srgb,
    purple: Srgb,
    cyan: Srgb,
    pink: Srgb,
    deep: Srgb,
}

/// A `--p-*` family variant through `tokens.css`'s shared wiring: accent `--p-cyan`, git
/// added / deleted `--p-green` / `--p-red`, and the terminal from black `--p-deep` to bright
/// white `--p-text`.
const fn wired(light: bool, p: &P) -> Variant {
    Variant {
        light,
        screen: p.screen,
        floor: p.floor,
        panel: p.panel,
        raised: p.raised,
        hover: p.hover,
        text: p.text,
        secondary: p.muted,
        accent: p.cyan,
        added: p.green,
        deleted: p.red,
        hunk: p.blue,
        ansi: [
            p.deep, p.red, p.green, p.yellow, p.blue, p.purple, p.cyan, p.text, p.muted, p.red,
            p.green, p.yellow, p.blue, p.pink, p.cyan, p.text,
        ],
        term_fg: p.text,
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Srgb {
    Srgb { r, g, b }
}

/// Vice City (neon, ai-team's original palette).
static VICE_CITY: Variant = Variant {
    light: false,
    screen: rgb(0x0A, 0x01, 0x19),
    floor: rgb(0x0C, 0x02, 0x1C),
    panel: rgb(0x1A, 0x06, 0x33),
    raised: rgb(0x26, 0x10, 0x49),
    hover: rgb(0x37, 0x14, 0x70),
    text: rgb(0xFD, 0xF4, 0xFF),
    // rgba(253,244,255,.62) over the screen.
    secondary: rgb(0xA1, 0x98, 0xA8),
    accent: rgb(0x2D, 0xE2, 0xE6),
    added: rgb(0x2D, 0xE2, 0xE6),
    deleted: rgb(0xFF, 0x6B, 0x35),
    hunk: rgb(0xB9, 0x67, 0xFF),
    ansi: [
        rgb(0x07, 0x00, 0x12), // black
        rgb(0xFF, 0x6B, 0x35), // red
        rgb(0x2D, 0xE2, 0xE6), // green
        rgb(0xFF, 0xD2, 0x3F), // yellow
        rgb(0xB9, 0x67, 0xFF), // blue
        rgb(0xFF, 0x79, 0xC6), // magenta
        rgb(0xB5, 0xF2, 0xFF), // cyan
        rgb(0xFD, 0xF4, 0xFF), // white
        rgb(0x8A, 0x6A, 0xA8), // bright black
        rgb(0xFF, 0x6B, 0x35), // bright red
        rgb(0xE0, 0xC3, 0xFF), // bright green
        rgb(0xFF, 0xD2, 0x3F), // bright yellow
        rgb(0xB5, 0xF2, 0xFF), // bright blue
        rgb(0xFF, 0x79, 0xC6), // bright magenta
        rgb(0xB5, 0xF2, 0xFF), // bright cyan
        rgb(0xFF, 0xFF, 0xFF), // bright white
    ],
    term_fg: rgb(0xE0, 0xC3, 0xFF),
};

/// Vice City's daylight faceplate: the same hues as ink.
static VICE_CITY_LIGHT: Variant = Variant {
    light: true,
    screen: rgb(0xFF, 0xFB, 0xFE),
    floor: rgb(0xFF, 0xF4, 0xFB),
    panel: rgb(0xF4, 0xEA, 0xFA),
    raised: rgb(0xFF, 0xF7, 0xFD),
    hover: rgb(0xF4, 0xD7, 0xED),
    text: rgb(0x29, 0x12, 0x3F),
    secondary: rgb(0x61, 0x47, 0x75),
    accent: rgb(0x00, 0x61, 0x68),
    added: rgb(0x00, 0x61, 0x68),
    deleted: rgb(0x96, 0x3A, 0x1A),
    hunk: rgb(0x71, 0x34, 0xA5),
    ansi: [
        rgb(0x29, 0x12, 0x3F), // black
        rgb(0x96, 0x3A, 0x1A), // red
        rgb(0x00, 0x61, 0x68), // green
        rgb(0x79, 0x51, 0x00), // yellow
        rgb(0x71, 0x34, 0xA5), // blue
        rgb(0x9B, 0x20, 0x5F), // magenta
        rgb(0x24, 0x5B, 0x83), // cyan
        rgb(0x61, 0x47, 0x75), // white
        rgb(0x70, 0x5A, 0x82), // bright black
        rgb(0x96, 0x3A, 0x1A), // bright red
        rgb(0x00, 0x4B, 0x51), // bright green
        rgb(0x5E, 0x3E, 0x00), // bright yellow
        rgb(0x1D, 0x4A, 0x6B), // bright blue
        rgb(0x7D, 0x1A, 0x4C), // bright magenta
        rgb(0x1D, 0x4A, 0x6B), // bright cyan
        rgb(0x29, 0x12, 0x3F), // bright white
    ],
    term_fg: rgb(0x29, 0x12, 0x3F),
};

/// morhetz/gruvbox: warm charcoal, cream, ochre and sage.
static GRUVBOX: Variant = wired(
    false,
    &P {
        screen: rgb(0x1D, 0x20, 0x21),
        floor: rgb(0x24, 0x23, 0x23),
        panel: rgb(0x28, 0x28, 0x28),
        raised: rgb(0x32, 0x30, 0x2F),
        hover: rgb(0x3A, 0x36, 0x32),
        text: rgb(0xFB, 0xF1, 0xC7),
        muted: rgb(0xD5, 0xC4, 0xA1),
        red: rgb(0xFB, 0x97, 0x88),
        green: rgb(0xC7, 0xD8, 0x78),
        yellow: rgb(0xF5, 0xD5, 0x80),
        blue: rgb(0xB5, 0xCB, 0xBF),
        purple: rgb(0xDC, 0xB2, 0xCE),
        cyan: rgb(0xA6, 0xD4, 0xBF),
        pink: rgb(0xEE, 0xB7, 0xBC),
        deep: rgb(0x28, 0x28, 0x28),
    },
);

static GRUVBOX_LIGHT: Variant = wired(
    true,
    &P {
        screen: rgb(0xFF, 0xF9, 0xE8),
        floor: rgb(0xFB, 0xF1, 0xC7),
        panel: rgb(0xF2, 0xE7, 0xCB),
        raised: rgb(0xFF, 0xF5, 0xDC),
        hover: rgb(0xE9, 0xDC, 0xC0),
        text: rgb(0x28, 0x28, 0x28),
        muted: rgb(0x57, 0x4A, 0x3D),
        red: rgb(0x8C, 0x2B, 0x25),
        green: rgb(0x33, 0x42, 0x14),
        yellow: rgb(0x5A, 0x3C, 0x05),
        blue: rgb(0x20, 0x43, 0x42),
        purple: rgb(0x59, 0x2E, 0x4E),
        cyan: rgb(0x1D, 0x46, 0x35),
        pink: rgb(0x80, 0x3C, 0x4E),
        deep: rgb(0x28, 0x28, 0x28),
    },
);

/// folke/tokyonight.nvim: blue-black and electric violet.
static TOKYO_NIGHT: Variant = wired(
    false,
    &P {
        screen: rgb(0x16, 0x16, 0x1E),
        floor: rgb(0x1A, 0x1B, 0x26),
        panel: rgb(0x20, 0x22, 0x31),
        raised: rgb(0x29, 0x2D, 0x40),
        hover: rgb(0x30, 0x36, 0x4D),
        text: rgb(0xD9, 0xE3, 0xFF),
        muted: rgb(0xAF, 0xBE, 0xDF),
        red: rgb(0xFF, 0x9D, 0xB0),
        green: rgb(0xB3, 0xD9, 0x90),
        yellow: rgb(0xE6, 0xCE, 0x91),
        blue: rgb(0xA6, 0xC4, 0xFF),
        purple: rgb(0xD7, 0xB8, 0xFF),
        cyan: rgb(0x9E, 0xE4, 0xED),
        pink: rgb(0xF4, 0xB5, 0xE7),
        deep: rgb(0x20, 0x22, 0x31),
    },
);

static TOKYO_NIGHT_LIGHT: Variant = wired(
    true,
    &P {
        screen: rgb(0xF4, 0xF6, 0xFF),
        floor: rgb(0xE6, 0xEA, 0xFA),
        panel: rgb(0xE1, 0xE6, 0xF5),
        raised: rgb(0xEE, 0xF2, 0xFF),
        hover: rgb(0xD8, 0xDE, 0xF0),
        text: rgb(0x24, 0x33, 0x5B),
        muted: rgb(0x46, 0x54, 0x78),
        red: rgb(0x92, 0x2C, 0x50),
        green: rgb(0x26, 0x44, 0x1E),
        yellow: rgb(0x56, 0x3D, 0x12),
        blue: rgb(0x20, 0x3F, 0x7D),
        purple: rgb(0x4D, 0x2A, 0x7A),
        cyan: rgb(0x11, 0x42, 0x4F),
        pink: rgb(0x84, 0x39, 0x76),
        deep: rgb(0x20, 0x22, 0x31),
    },
);

/// catppuccin/nvim: Mocha / Latte-inspired neutral pastels.
static CATPPUCCIN: Variant = wired(
    false,
    &P {
        screen: rgb(0x18, 0x18, 0x25),
        floor: rgb(0x1E, 0x1E, 0x2E),
        panel: rgb(0x25, 0x25, 0x36),
        raised: rgb(0x30, 0x30, 0x43),
        hover: rgb(0x39, 0x39, 0x4D),
        text: rgb(0xCD, 0xD6, 0xF4),
        muted: rgb(0xB5, 0xBF, 0xDC),
        red: rgb(0xF3, 0x8B, 0xA8),
        green: rgb(0xA6, 0xE3, 0xA1),
        yellow: rgb(0xF9, 0xE2, 0xAF),
        blue: rgb(0x89, 0xB4, 0xFA),
        purple: rgb(0xCB, 0xA6, 0xF7),
        cyan: rgb(0x94, 0xE2, 0xD5),
        pink: rgb(0xF5, 0xC2, 0xE7),
        deep: rgb(0x25, 0x25, 0x36),
    },
);

static CATPPUCCIN_LIGHT: Variant = wired(
    true,
    &P {
        screen: rgb(0xFC, 0xFC, 0xFF),
        floor: rgb(0xEF, 0xF1, 0xF5),
        panel: rgb(0xE6, 0xE9, 0xF1),
        raised: rgb(0xF6, 0xF7, 0xFB),
        hover: rgb(0xDC, 0xE0, 0xEB),
        text: rgb(0x30, 0x34, 0x46),
        muted: rgb(0x51, 0x57, 0x6D),
        red: rgb(0xA3, 0x23, 0x47),
        green: rgb(0x22, 0x4D, 0x20),
        yellow: rgb(0x5E, 0x40, 0x09),
        blue: rgb(0x1C, 0x3F, 0x82),
        purple: rgb(0x53, 0x2C, 0x88),
        cyan: rgb(0x11, 0x4B, 0x4E),
        pink: rgb(0x87, 0x35, 0x82),
        deep: rgb(0x25, 0x25, 0x36),
    },
);

/// sainnhe/everforest: eucalyptus, sage and warm paper.
static EVERFOREST: Variant = wired(
    false,
    &P {
        screen: rgb(0x20, 0x28, 0x2B),
        floor: rgb(0x27, 0x2F, 0x32),
        panel: rgb(0x2D, 0x35, 0x3B),
        raised: rgb(0x33, 0x3F, 0x40),
        hover: rgb(0x3A, 0x45, 0x43),
        text: rgb(0xE4, 0xE1, 0xC5),
        muted: rgb(0xC2, 0xC8, 0xAF),
        red: rgb(0xF1, 0xA5, 0xA1),
        green: rgb(0xBD, 0xD9, 0xA0),
        yellow: rgb(0xE6, 0xD3, 0x9B),
        blue: rgb(0xA9, 0xD3, 0xDB),
        purple: rgb(0xD5, 0xB6, 0xD5),
        cyan: rgb(0xA8, 0xDC, 0xC2),
        pink: rgb(0xE5, 0xB7, 0xC6),
        deep: rgb(0x29, 0x35, 0x31),
    },
);

static EVERFOREST_LIGHT: Variant = wired(
    true,
    &P {
        screen: rgb(0xFD, 0xF9, 0xEC),
        floor: rgb(0xF3, 0xF0, 0xDC),
        panel: rgb(0xE9, 0xEC, 0xDA),
        raised: rgb(0xF7, 0xF7, 0xE9),
        hover: rgb(0xDC, 0xE3, 0xCD),
        text: rgb(0x30, 0x3D, 0x38),
        muted: rgb(0x50, 0x60, 0x57),
        red: rgb(0x92, 0x3C, 0x37),
        green: rgb(0x30, 0x47, 0x1F),
        yellow: rgb(0x5A, 0x40, 0x10),
        blue: rgb(0x21, 0x47, 0x52),
        purple: rgb(0x5A, 0x36, 0x52),
        cyan: rgb(0x20, 0x4A, 0x3A),
        pink: rgb(0x83, 0x46, 0x5C),
        deep: rgb(0x29, 0x35, 0x31),
    },
);

/// rebelot/kanagawa.nvim: charcoal ink, wave blue and aged paper.
static KANAGAWA: Variant = wired(
    false,
    &P {
        screen: rgb(0x16, 0x16, 0x1D),
        floor: rgb(0x1F, 0x1F, 0x28),
        panel: rgb(0x25, 0x25, 0x31),
        raised: rgb(0x2D, 0x2D, 0x3C),
        hover: rgb(0x36, 0x36, 0x49),
        text: rgb(0xEE, 0xE5, 0xC4),
        muted: rgb(0xCB, 0xC3, 0xA7),
        red: rgb(0xE9, 0xA0, 0x9C),
        green: rgb(0xB9, 0xC8, 0x9A),
        yellow: rgb(0xE5, 0xC7, 0x8C),
        blue: rgb(0xA3, 0xBE, 0xDA),
        purple: rgb(0xC7, 0xB3, 0xDA),
        cyan: rgb(0xA2, 0xD3, 0xCE),
        pink: rgb(0xDD, 0xB1, 0xC4),
        deep: rgb(0x25, 0x25, 0x31),
    },
);

static KANAGAWA_LIGHT: Variant = wired(
    true,
    &P {
        screen: rgb(0xFA, 0xF5, 0xE5),
        floor: rgb(0xEE, 0xE8, 0xD5),
        panel: rgb(0xE6, 0xE0, 0xCD),
        raised: rgb(0xF6, 0xF0, 0xDF),
        hover: rgb(0xDA, 0xD5, 0xC4),
        text: rgb(0x35, 0x35, 0x47),
        muted: rgb(0x57, 0x56, 0x6A),
        red: rgb(0x88, 0x3D, 0x41),
        green: rgb(0x36, 0x42, 0x22),
        yellow: rgb(0x5A, 0x40, 0x16),
        blue: rgb(0x27, 0x44, 0x5E),
        purple: rgb(0x51, 0x35, 0x5C),
        cyan: rgb(0x1C, 0x47, 0x43),
        pink: rgb(0x7A, 0x41, 0x5B),
        deep: rgb(0x25, 0x25, 0x31),
    },
);
