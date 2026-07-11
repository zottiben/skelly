//! `skelly-render` - the GPU cell-grid renderer.
//!
//! Rasterizes the terminal grid on the GPU: glyph shaping + fallback, a glyph
//! texture atlas, batched quad draws, cursor, and selection. Also resolves the
//! active theme's semantic tokens (`category.role.state`) to concrete colors - the
//! UI never references raw hex (Hard rule 2), and the UI token set is kept separate
//! from the terminal ANSI palette so any scheme pairs with any theme.
//!
//! Renders off a snapshot of terminal state so I/O and input never block frames
//! (the Ghostty model). Depends on `skelly-config` for tokens and metrics; never on
//! the binary.
//!
//! Status: M1a - opens a `wgpu` surface on the window and clears it to the resolved
//! theme background each frame. Text shaping + the cell grid land in the next slice
//! (ADR-0003).

#![doc(test(attr(deny(warnings))))]

mod ansi;
mod capture;
mod cells;
mod error;
mod renderer;
mod text;
mod theme;

pub use ansi::AnsiPalette;
pub use capture::{capture_cells_rgba, capture_panes_rgba, capture_rgba, CapturePane};
pub use error::RenderError;
pub use renderer::Renderer;
pub use text::{measure_cell, TextLayer};
pub use theme::{Rgba, Srgb, Theme};

/// A rectangle in physical pixels, positioning a pane on the render surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PxRect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// One pane to draw this frame.
///
/// The renderer tiles any number of these onto the surface: it fills each pane's
/// cell backgrounds/underlines/selection, draws the glyphs clipped to `rect`, and -
/// when more than one pane is present - a subtle `border` divider around each with
/// an `accent` ring around the focused one. The cursor is drawn only in the focused
/// pane. The binary owns the geometry (`rect` and the cell `origin`); the renderer
/// just paints where told.
pub struct PaneView<'a> {
    /// The pane's rectangle on the surface (border + text clip), physical px.
    pub rect: PxRect,
    /// Pixel position of cell `(0, 0)`'s top-left corner, physical px.
    pub origin: (f32, f32),
    /// The pane's cell grid, top to bottom.
    pub rows: &'a [Vec<GridCell>],
    /// Cursor position `(column, row)` - drawn only when `focused`.
    pub cursor: (usize, usize),
    /// Selected cells `(column, row)` in this pane.
    pub selection: &'a [(usize, usize)],
    /// Whether this is the focused pane (accent ring + drawn cursor).
    pub focused: bool,
}

/// One cell to render: its character, foreground, optional background fill, and
/// resolved text attributes.
///
/// Colors are already resolved against the ANSI palette (reverse video and dim
/// folded in), so the renderer only needs the font-level attributes: `bold` and
/// `italic` pick the face; `underline` draws a rule beneath the glyph.
#[derive(Clone, Copy, Debug)]
pub struct GridCell {
    /// The cell's character.
    pub c: char,
    /// Foreground (glyph) color.
    pub fg: Srgb,
    /// Background fill; `None` means the terminal's default background (no fill).
    pub bg: Option<Srgb>,
    /// Render the glyph with a bold weight.
    pub bold: bool,
    /// Render the glyph with an italic style.
    pub italic: bool,
    /// Draw an underline rule beneath the cell in the foreground color.
    pub underline: bool,
}
