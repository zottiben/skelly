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
mod fonts;
mod prose;
mod renderer;
mod text;
mod theme;

pub use ansi::AnsiPalette;
pub use capture::{
    capture_cells_rgba, capture_panes_rgba, capture_rgba, capture_settings_rgba, CaptureGitDock,
    CaptureOverlay, CapturePane, CaptureSettings, CaptureSidebar, CaptureTimeline, Chrome,
    PaneOverlay,
};
pub use cells::logo_chrome_quads;
pub use error::RenderError;
pub use fonts::FontRole;
pub use prose::{ProseLabel, TextMeasure};
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

/// One decorative chrome quad the binary hands the renderer to paint: a physical-pixel
/// rectangle, a resolved UI-token color at `alpha`, and an optional corner `radius`
/// (physical px; `0` = a sharp fill). This is how proportional chrome expresses its
/// surfaces, pills, bars, and dividers - the binary owns the layout, the renderer just
/// paints where told (the same split as [`ProseLabel`] for text).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromeQuad {
    /// The rectangle to fill, physical px. For a `diamond`, this is the disc's square
    /// bounding box (side = the disc's size) centered where the disc sits.
    pub rect: PxRect,
    /// The fill color (a resolved UI token; theme-correct).
    pub color: Srgb,
    /// Fill alpha (`1.0` opaque; `< 1.0` for translucent tints like `accent.subtle`).
    pub alpha: f32,
    /// Corner radius in physical px (`0.0` = sharp corners).
    pub radius: f32,
    /// When true, the quad is a rounded square rotated 45° (a vertebra-logo "diamond" disc,
    /// §02) rather than an axis-aligned rect - so overlays can draw the brand mark.
    pub diamond: bool,
}

impl ChromeQuad {
    /// A sharp, opaque fill.
    #[must_use]
    pub fn fill(rect: PxRect, color: Srgb) -> Self {
        Self {
            rect,
            color,
            alpha: 1.0,
            radius: 0.0,
            diamond: false,
        }
    }

    /// An opaque fill with rounded corners of `radius` physical px.
    #[must_use]
    pub fn rounded(rect: PxRect, color: Srgb, radius: f32) -> Self {
        Self {
            rect,
            color,
            alpha: 1.0,
            radius,
            diamond: false,
        }
    }

    /// A translucent fill (e.g. an `accent.subtle` selected-row pill) at `alpha`, with an
    /// optional corner `radius`.
    #[must_use]
    pub fn tint(rect: PxRect, color: Srgb, alpha: f32, radius: f32) -> Self {
        Self {
            rect,
            color,
            alpha,
            radius,
            diamond: false,
        }
    }

    /// A rounded-square "diamond" disc of the vertebra brand mark (§02): a square of side
    /// `size` centered at `(cx, cy)` (physical px), rotated 45°, corners rounded by `radius`,
    /// at `alpha`. Lets overlays (the first-run modal, §10.1) draw the mark.
    #[must_use]
    pub fn diamond(cx: f32, cy: f32, size: f32, color: Srgb, alpha: f32, radius: f32) -> Self {
        Self {
            rect: PxRect {
                x: cx - size * 0.5,
                y: cy - size * 0.5,
                w: size,
                h: size,
            },
            color,
            alpha,
            radius,
            diamond: true,
        }
    }
}

/// One pane to draw this frame.
///
/// The renderer tiles any number of these onto the surface: it fills each pane's
/// cell backgrounds/underlines/selection, draws the glyphs clipped to `rect`, and -
/// when more than one pane is present - a subtle `border` divider around each with
/// a `border.strong` ring around the focused one. The cursor is drawn only in the
/// focused pane. The binary owns the geometry (`rect` and the cell `origin`); the
/// renderer just paints where told.
pub struct PaneView<'a> {
    /// The pane's rectangle on the surface (border + text clip), physical px.
    pub rect: PxRect,
    /// Pixel position of cell `(0, 0)`'s top-left corner, physical px.
    pub origin: (f32, f32),
    /// The pane's cell grid, top to bottom.
    pub rows: &'a [Vec<GridCell>],
    /// Cursor position `(column, row)` - drawn only when `focused`.
    pub cursor: (usize, usize),
    /// The cursor's shape (block / bar / underline / hidden), honoring what the running program
    /// requested via `DECSCUSR` (e.g. vim's per-mode cursor).
    pub cursor_shape: CursorShape,
    /// Selected cells `(column, row)` in this pane.
    pub selection: &'a [(usize, usize)],
    /// Whether this is the focused pane (accent ring + drawn cursor).
    pub focused: bool,
    /// The empty-state brand watermark's square bounding box (physical px), when this pane
    /// is a pristine empty-state tab (design §10.2). The renderer paints the vertebra logo
    /// mark there, beneath the glyphs; `None` for every ordinary pane.
    pub logo: Option<PxRect>,
}

/// The command-palette / modal overlay to draw over the live terminal (proportional chrome).
///
/// The renderer draws the floating card (the `e4` shadow, `border.strong` ring, and
/// `bg.elevated` fill) from `panel`, then paints the binary's display list on top: the
/// content `quads` (the selected-row `accent.subtle` pill, the input caret) and the
/// positioned prose `labels` (the query, results, key hints, footer), clipped to the panel.
/// Drawn over the terminal, so it never unmounts the panes beneath (AGENTS Hard rule 4).
pub struct OverlayView<'a> {
    /// The centered panel rectangle on the surface, physical px.
    pub panel: PxRect,
    /// The content quads (selected-row pill, caret), in draw order over the card.
    pub quads: &'a [ChromeQuad],
    /// The positioned proportional text labels.
    pub labels: &'a [ProseLabel],
}

/// The persistent left sidebar to draw as base-layer chrome (design §08).
///
/// It sits in the left strip of the surface, beneath any overlay and never over the panes
/// (the pane viewport is inset to its right). Proportional chrome: the binary lays the
/// sidebar out in the guide's fonts and hands over the finished display list - the
/// decorative `quads` (the surface fill, the active tab's rounded `accent.subtle` pill +
/// `accent` bar, chips, the input well, the right-edge divider) and the positioned text
/// `labels` (header, tab titles, group headers, the utility bar). The renderer paints them,
/// clipping to `panel`.
pub struct SidebarView<'a> {
    /// The sidebar rectangle on the surface (`x = 0`, full height), physical px.
    pub panel: PxRect,
    /// The decorative quads (surface, pills, bars, dividers), in draw order.
    pub quads: &'a [ChromeQuad],
    /// The positioned proportional text labels.
    pub labels: &'a [ProseLabel],
}

/// The per-repo git diff dock to draw as base-layer chrome on the right edge.
///
/// Like [`SidebarView`] it sits over the surface (the pane viewport insets to its left)
/// and never over the panes; the palette overlay and settings view still draw on top of
/// it (AGENTS Hard rule 4 - a layer, the terminal never unmounts). The renderer draws a
/// `border` divider on its left edge, the selected file's `accent.subtle` highlight, and
/// the translucent add/del/hunk line backgrounds (from the `diff.*` tokens), then paints
/// `rows` as a monospace grid at `text_origin` (clipped to `panel`). The caller bakes the
/// UI-token text colors into `rows` and reports which grid rows are additions, deletions,
/// and hunk headers; the renderer owns only the decorative quads.
pub struct GitDockView<'a> {
    /// The dock rectangle on the surface (right edge, full height), physical px.
    pub panel: PxRect,
    /// The content quads over the dock frame (diff add/del/hunk line backgrounds, the
    /// selected-file + focused-hunk fills, the commit caret).
    pub quads: &'a [ChromeQuad],
    /// The positioned proportional text labels (status bar, file list, diff lines, commit
    /// box); mono content (code, branch, counts) uses the `mono` font role.
    pub labels: &'a [ProseLabel],
}

/// The session-timeline dock to draw as base-layer chrome on the right edge.
///
/// Like [`GitDockView`] it sits over the surface (the pane viewport insets to its left)
/// and never over the panes; only one right-dock surface is open at a time (AGENTS Hard
/// rule 4 - a layer, the terminal never unmounts). The renderer draws a `border` divider
/// on its left edge, the selected event's `accent.subtle` row fill, and - when rewound to
/// a past state - an `accent` bar on the viewed event's row. The caller bakes the UI-token
/// text colors into `rows`; the renderer owns only the decorative quads.
pub struct TimelineView<'a> {
    /// The dock rectangle on the surface (right edge, full height), physical px.
    pub panel: PxRect,
    /// The content quads over the dock frame (selected-event fill, the viewing accent bar).
    pub quads: &'a [ChromeQuad],
    /// The positioned proportional text labels (banner, events, legend, summary).
    pub labels: &'a [ProseLabel],
}

/// The full-window settings view to draw over the live terminal (AGENTS Hard rule
/// 4 - a layer over the always-present pane tree, dismissed with Esc, never a route).
///
/// It splits into a left category nav and a right control panel, both baked into one
/// monospace `rows` grid: the nav occupies the first `nav_cols` cells of each row and
/// the controls occupy the rest. The renderer fills the panel with `bg.elevated`, the
/// nav strip with `bg.base`, draws a `border` divider between them, marks the active
/// category (`nav_active_row`: `accent` bar + `accent.subtle` fill) and the focused
/// control (`selected_row`: translucent `accent` fill), then paints `rows` on top. The
/// caller bakes the UI-token colors into `rows`; the renderer owns only the quads.
pub struct SettingsView<'a> {
    /// The settings panel rectangle on the surface (usually the whole window).
    pub panel: PxRect,
    /// The x of the nav/content divider (physical px); the nav strip fills to its left.
    pub nav_divider_x: f32,
    /// The content quads over the frame (active-category fill + bar, focused-control fill).
    pub quads: &'a [ChromeQuad],
    /// The positioned proportional text labels (nav categories, control label/value, footer).
    pub labels: &'a [ProseLabel],
}

/// The shape to draw the focused pane's cursor as, honoring the running program's `DECSCUSR`
/// request (vim sets a block in normal mode, a bar in insert, an underline in replace).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// A full-cell block (the terminal default; vim normal mode).
    #[default]
    Block,
    /// A thin vertical bar at the cell's left edge (vim insert mode).
    Bar,
    /// A thin underline along the cell's bottom (vim replace mode).
    Underline,
    /// No cursor drawn (the program hid it).
    Hidden,
}

/// One cell to render: its character, foreground, optional background fill, and
/// resolved text attributes.
///
/// Colors are already resolved against the ANSI palette (reverse video and dim
/// folded in), so the renderer only needs the font-level attributes: `bold` and
/// `italic` pick the face; `underline` draws a rule beneath the glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// Bench-only entry points for the pure, per-frame CPU builders on the render hot path
/// (playbook §4). These wrap the internal `grid_quads` / `text_runs` - which turn a grid
/// snapshot into GPU instance / shaping data every frame - so `criterion` can measure them
/// with no GPU or window. They take and return only public types (the internal `Quad` /
/// `Run` never leak, and glyphon's `Color` inside `Run` stays hidden); the real output is
/// black-boxed internally so the optimizer cannot elide the work, and the count is returned
/// as something to observe. Not a stable API - hidden from the docs, exists solely for the
/// `benches/render.rs` harness.
#[doc(hidden)]
pub mod bench_support {
    use crate::{GridCell, Srgb};

    /// Build one pane's background / underline / selection / cursor quads for `rows` (a
    /// focused pane: cursor at `(0, 0)`, no selection) and return how many were produced.
    #[must_use]
    pub fn grid_quads_len(cell_w: f32, cell_h: f32, rows: &[Vec<GridCell>], accent: Srgb) -> usize {
        let quads = crate::cells::grid_quads(
            cell_w,
            cell_h,
            (0.0, 0.0),
            rows,
            Some((0, 0)),
            crate::CursorShape::Block,
            accent,
            &[],
        );
        std::hint::black_box(&quads).len()
    }

    /// Merge `rows` into shaping runs (same color / weight / style coalesced) and return
    /// how many runs resulted.
    #[must_use]
    pub fn text_runs_len(rows: &[Vec<GridCell>]) -> usize {
        let runs = crate::text::text_runs(rows);
        std::hint::black_box(&runs).len()
    }
}
