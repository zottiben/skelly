//! The bundled proportional chrome fonts and the guide's type scale (§05).
//!
//! Skelly's terminal grid uses the user's configured monospace font, but the *chrome*
//! (sidebar, palette, settings, docks, status line) is proportional, drawn in the exact
//! fonts the design guide specifies - IBM Plex Sans, Space Grotesk, and `JetBrains` Mono.
//! Those are unlikely to be installed, so they ship *inside* the binary (bundled here and
//! loaded into every `FontSystem` via [`load_bundled`]); this way the chrome renders
//! byte-identically on any machine and the layout measurer (the binary) and the renderer
//! always agree on glyph widths. All three families are OFL-licensed and redistributable
//! (the license texts sit alongside the `.ttf`s in `assets/fonts/`).
//!
//! [`FontRole`] enumerates the guide's type tokens (`display`/`h1`/`h2`/`title`/`body`/
//! `label`/`caption`/`micro`) with the exact family, weight, size, line height, and
//! letter-spacing transcribed from the §05 scale, in *logical* px (multiply by the DPI
//! scale for physical px).

use std::sync::OnceLock;

use glyphon::cosmic_text::{Fallback, PlatformFallback};
use glyphon::{fontdb, FontSystem};
use unicode_script::Script;

/// The bundled font files, embedded in the binary so chrome renders identically anywhere.
const IBM_PLEX_SANS_REGULAR: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf");
const IBM_PLEX_SANS_MEDIUM: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf");
const IBM_PLEX_SANS_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf");
const SPACE_GROTESK_VARIABLE: &[u8] = include_bytes!("../assets/fonts/SpaceGrotesk-Variable.ttf");
const JETBRAINS_MONO_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const JETBRAINS_MONO_MEDIUM: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf");
const JETBRAINS_MONO_BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");
/// Symbols Nerd Font Mono - the icons-only Nerd Font, bundled so terminal programs (lazyvim,
/// starship, …) render their glyphs without the user installing a patched font. It carries the
/// Nerd Font private-use-area glyphs at fixed (mono) advance so they align to the cell grid; the
/// terminal falls back to it for any glyph its primary font lacks.
const SYMBOLS_NERD_FONT: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

/// The bundled font family names, as registered in the font database (see the `fc-scan`
/// output the assets were verified against).
pub(crate) const FAMILY_SANS: &str = "IBM Plex Sans";
pub(crate) const FAMILY_DISPLAY: &str = "Space Grotesk";
pub(crate) const FAMILY_MONO: &str = "JetBrains Mono";
/// The bundled Nerd Font fallback family (icons for terminal programs).
pub(crate) const FAMILY_NERD: &str = "Symbols Nerd Font Mono";

/// Load the bundled chrome fonts into `db` if they are not already present. Idempotent:
/// called on every `FontSystem` the crate builds (renderer, capture, measurer) so the
/// guide fonts are always available for chrome without disturbing the user's system fonts
/// (which stay loaded for the terminal). The system faces are still enumerated, so a
/// user font of the same name would coexist; the bundled copy guarantees the family
/// exists at all.
pub(crate) fn load_bundled(db: &mut fontdb::Database) {
    // Guard against double-loading into a shared database (the same `FontSystem` is never
    // loaded twice today, but keep it cheap and safe if that changes).
    let already = db
        .faces()
        .any(|face| face.families.iter().any(|(name, _)| name == FAMILY_SANS));
    if already {
        return;
    }
    for data in [
        IBM_PLEX_SANS_REGULAR,
        IBM_PLEX_SANS_MEDIUM,
        IBM_PLEX_SANS_SEMIBOLD,
        SPACE_GROTESK_VARIABLE,
        JETBRAINS_MONO_REGULAR,
        JETBRAINS_MONO_MEDIUM,
        JETBRAINS_MONO_BOLD,
        SYMBOLS_NERD_FONT,
    ] {
        db.load_font_data(data.to_vec());
    }
}

/// The terminal's font fallback: the platform defaults, but with the bundled `Symbols Nerd Font
/// Mono` inserted first in the common list so a glyph missing from the primary font (a Nerd Font
/// icon in lazyvim/starship/…) resolves to it instead of showing tofu. Script + forbidden
/// fallbacks defer to the platform.
#[derive(Debug)]
struct NerdFallback;

impl Fallback for NerdFallback {
    fn common_fallback(&self) -> &[&'static str] {
        static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
        LIST.get_or_init(|| {
            let mut list = vec![FAMILY_NERD];
            list.extend_from_slice(PlatformFallback.common_fallback());
            list
        })
        .as_slice()
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        PlatformFallback.forbidden_fallback()
    }

    fn script_fallback(&self, script: Script, locale: &str) -> &[&'static str] {
        PlatformFallback.script_fallback(script, locale)
    }
}

/// Build a `FontSystem` for the terminal grid: the system fonts plus Skelly's bundled families,
/// with the bundled `JetBrains Mono` as the generic monospace default (so an unconfigured or
/// uninstalled terminal font still renders in a known-good mono) and the [`NerdFallback`] wired so
/// Nerd Font icon glyphs resolve to the bundled Symbols font. Mirrors `FontSystem::new`'s db setup
/// but swaps in our fallback + monospace default.
#[must_use]
pub(crate) fn new_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    load_bundled(&mut db);
    db.set_monospace_family(FAMILY_MONO);
    db.set_sans_serif_family(FAMILY_SANS);
    FontSystem::new_with_locale_and_db_and_fallback(locale(), db, NerdFallback)
}

/// The current locale for script-fallback ordering (mirrors cosmic-text's own detection), e.g.
/// `en-US`; falls back to `en-US` when unset.
fn locale() -> String {
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_CTYPE"))
        .or_else(|_| std::env::var("LANG"))
        .ok()
        .and_then(|raw| raw.split('.').next().map(|s| s.replace('_', "-")))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "en-US".to_owned())
}

/// A chrome type token from the guide's §05 scale. Each maps 1:1 to a `(family, weight,
/// size, line-height, tracking)` tuple; the renderer and the layout measurer both resolve
/// through here so proportional text is sized identically everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontRole {
    /// `display` - Space Grotesk 700, 88px. Splash & brand only.
    Display,
    /// `h1` - Space Grotesk 600, 34px. Screen / section titles.
    H1,
    /// `h2` - Space Grotesk 600, 20px. Panel & dialog titles.
    H2,
    /// `title` - IBM Plex Sans 600, 16px. Card & list headers.
    Title,
    /// `body` - IBM Plex Sans 400, 14px. Default UI text.
    Body,
    /// `label` - IBM Plex Sans 500, 13px. Controls, tabs, buttons.
    Label,
    /// `caption` - IBM Plex Sans 400, 12px. Metadata, hints.
    Caption,
    /// `micro` - `JetBrains` Mono 500, 10.5px, +1.5 tracking. Uppercase tags & badges.
    Micro,
    /// `mono` - `JetBrains` Mono 400, 12px, no tracking. Terminal-adjacent code + metadata
    /// in the chrome (diff lines, branch names, counts) where columns must stay aligned; not
    /// one of the §05 display tokens but the guide's stated mono use ("code, and every token
    /// / keybinding label").
    Mono,
}

impl FontRole {
    /// The font family this role renders in.
    #[must_use]
    pub(crate) fn family(self) -> &'static str {
        match self {
            Self::Display | Self::H1 | Self::H2 => FAMILY_DISPLAY,
            Self::Title | Self::Body | Self::Label | Self::Caption => FAMILY_SANS,
            Self::Micro | Self::Mono => FAMILY_MONO,
        }
    }

    /// The role's default weight (100..900), overridable per label.
    #[must_use]
    pub fn weight(self) -> u16 {
        match self {
            Self::Display => 700,
            Self::H1 | Self::H2 | Self::Title => 600,
            Self::Body | Self::Caption | Self::Mono => 400,
            Self::Label | Self::Micro => 500,
        }
    }

    /// The role's font size in **logical** px (from the §05 scale).
    #[must_use]
    pub fn size_px(self) -> f32 {
        match self {
            Self::Display => 88.0,
            Self::H1 => 34.0,
            Self::H2 => 20.0,
            Self::Title => 16.0,
            Self::Body => 14.0,
            Self::Label => 13.0,
            Self::Caption | Self::Mono => 12.0,
            Self::Micro => 10.5,
        }
    }

    /// The role's line height as a multiple of the font size (from the §05 scale).
    #[must_use]
    pub fn line_height_ratio(self) -> f32 {
        match self {
            Self::Display => 0.9,
            Self::H1 => 1.1,
            Self::H2 => 1.3,
            Self::Title | Self::Label | Self::Caption | Self::Mono => 1.4,
            Self::Body => 1.6,
            Self::Micro => 1.0,
        }
    }

    /// The role's letter-spacing (tracking) in **logical** px (from the §05 scale).
    #[must_use]
    pub fn tracking_px(self) -> f32 {
        match self {
            Self::Display => -3.0,
            Self::H1 => -1.0,
            Self::H2 => -0.4,
            Self::Micro => 1.5,
            Self::Title | Self::Body | Self::Label | Self::Caption | Self::Mono => 0.0,
        }
    }

    /// The line-box height in physical px at DPI `scale` (`size · line-height ratio`).
    #[must_use]
    pub fn line_height_px(self, scale: f32) -> f32 {
        self.size_px() * self.line_height_ratio() * scale
    }
}

#[cfg(test)]
mod tests {
    use super::{load_bundled, FontRole, FAMILY_DISPLAY, FAMILY_MONO, FAMILY_SANS};
    use glyphon::fontdb;

    #[test]
    fn bundled_families_load_and_are_queryable() {
        let mut db = fontdb::Database::new();
        load_bundled(&mut db);
        for family in [FAMILY_SANS, FAMILY_DISPLAY, FAMILY_MONO] {
            assert!(
                db.faces()
                    .any(|face| face.families.iter().any(|(name, _)| name == family)),
                "bundled family {family:?} should be registered"
            );
        }
        // A second load is a no-op (idempotent), leaving the face count unchanged.
        let count = db.faces().count();
        load_bundled(&mut db);
        assert_eq!(db.faces().count(), count);
    }

    #[test]
    fn role_metrics_match_the_guide_type_scale() {
        // A spot-check of the §05 tokens the chrome is built against.
        assert!((FontRole::Label.size_px() - 13.0).abs() < 1e-4);
        assert_eq!(FontRole::Label.family(), FAMILY_SANS);
        assert_eq!(FontRole::Label.weight(), 500);
        assert_eq!(FontRole::Micro.family(), FAMILY_MONO);
        assert!((FontRole::Micro.tracking_px() - 1.5).abs() < 1e-4);
        assert_eq!(FontRole::H2.family(), FAMILY_DISPLAY);
        assert_eq!(FontRole::H2.weight(), 600);
        // Line-box height folds size · ratio · scale.
        assert!((FontRole::Body.line_height_px(2.0) - 14.0 * 1.6 * 2.0).abs() < 1e-4);
    }
}
