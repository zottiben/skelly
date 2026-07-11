//! `skelly` - the binary: window, event loop, and the wiring that binds the library
//! crates together.
//!
//! M1c completed the walking skeleton (one shell, one pane). M3 wires the pane tree
//! (`skelly-pane`) into the window: a live terminal (`skelly-term`) per pane, the
//! renderer (`skelly-render`) drawing each pane at its computed rectangle with a
//! divider and a focused-pane ring, input routed to the focused pane, and the
//! split / focus / zoom / resize / close keybindings. The reader thread of each
//! shell wakes the event loop via an `EventLoopProxy` so we repaint only on new
//! output. Errors are contextualized here with `anyhow`; `wgpu`/`alacritty` types
//! never leak up.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Context as _;
use skelly_config::Config;
use skelly_pane::{Dir, PaneId, PaneTree, Rect};
use skelly_render::{AnsiPalette, GridCell, PaneView, PxRect, Renderer, Srgb};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

/// Logical padding (px) around the whole pane area - the window content margin.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset (px) inside each pane, between its border and its cells.
const PANE_INSET: f32 = 6.0;
/// One keyboard resize step, as a fraction of the enclosing split's extent.
const RESIZE_STEP: f32 = 0.04;

/// Event the reader thread sends to wake the UI when a shell produces output.
#[derive(Debug, Clone, Copy)]
struct Wakeup;

fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::load_default().context("loading configuration")?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        theme = %config.appearance.theme,
        "starting skelly"
    );

    let event_loop = EventLoop::<Wakeup>::with_user_event()
        .build()
        .context("creating the event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(config, proxy);
    event_loop
        .run_app(&mut app)
        .context("running the event loop")?;
    Ok(())
}

/// A text selection, in a pane's visible-grid cell coordinates `(column, row)`.
#[derive(Clone, Copy)]
struct Selection {
    anchor: (usize, usize),
    head: (usize, usize),
}

/// A pane operation bound to a keyboard chord (the `⌥`-leader pane bindings).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaneAction {
    /// Split the focused pane, placing the new pane in this direction.
    Split(Dir),
    /// Move focus to the neighbor in this direction.
    Focus(Dir),
    /// Focus the pane at this 0-based index in layout order.
    FocusIndex(usize),
    /// Close the focused pane.
    Close,
    /// Toggle zoom on the focused pane.
    Zoom,
    /// Nudge the focused pane's enclosing divider in this direction.
    Resize(Dir),
    /// Reset every split to an even 50/50.
    EvenOut,
}

/// Application state driven by the winit event loop. The window and renderer are
/// `None` until the platform signals `resumed`; the pane tree exists from the start.
struct App {
    config: Config,
    proxy: EventLoopProxy<Wakeup>,
    palette: AnsiPalette,
    clipboard: Option<arboard::Clipboard>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    /// The tiling model; every leaf maps to a live terminal in `panes`.
    tree: PaneTree,
    /// One live shell per pane.
    panes: HashMap<PaneId, Terminal>,
    /// Each pane's last-applied grid size, so we only resize on a real change.
    dims: HashMap<PaneId, (u16, u16)>,
    /// Current surface size in physical px.
    size: (u32, u32),
    scale: f64,
    modifiers: ModifiersState,
    pointer: (f64, f64),
    /// The active selection and the pane it belongs to.
    selection: Option<(PaneId, Selection)>,
    selecting: bool,
}

impl App {
    fn new(config: Config, proxy: EventLoopProxy<Wakeup>) -> Self {
        let palette = AnsiPalette::resolve(&config.appearance.theme);
        Self {
            config,
            proxy,
            palette,
            clipboard: arboard::Clipboard::new().ok(),
            window: None,
            renderer: None,
            tree: PaneTree::new(),
            panes: HashMap::new(),
            dims: HashMap::new(),
            size: (0, 0),
            scale: 1.0,
            modifiers: ModifiersState::empty(),
            pointer: (0.0, 0.0),
            selection: None,
            selecting: false,
        }
    }

    /// The pane area within the window (the surface inset by the window margin).
    fn viewport_rect(&self) -> Rect {
        let pad = WINDOW_PAD * scale32(self.scale);
        let w = dim_f32(self.size.0);
        let h = dim_f32(self.size.1);
        Rect::new(pad, pad, (w - 2.0 * pad).max(1.0), (h - 2.0 * pad).max(1.0))
    }

    /// The physical-pixel inset inside each pane (border-to-cells gap).
    fn pane_inset(&self) -> f32 {
        PANE_INSET * scale32(self.scale)
    }

    /// The focused pane's live terminal, if any.
    fn focused_term(&mut self) -> Option<&mut Terminal> {
        let id = self.tree.focused();
        self.panes.get_mut(&id)
    }

    /// The rectangle of pane `id` in the current layout, if it is visible.
    fn pane_rect(&self, id: PaneId) -> Option<Rect> {
        self.tree
            .layout(self.viewport_rect())
            .into_iter()
            .find(|(pid, _)| *pid == id)
            .map(|(_, rect)| rect)
    }

    /// The pane whose rectangle contains the pointer, with that rectangle.
    fn pane_at_pointer(&self) -> Option<(PaneId, Rect)> {
        let (px, py) = point_f32(self.pointer);
        self.tree
            .layout(self.viewport_rect())
            .into_iter()
            .find(|(_, r)| px >= r.x && px < r.x + r.w && py >= r.y && py < r.y + r.h)
    }

    /// Reconcile the live terminals with the pane tree: spawn a shell for any new
    /// pane, resize panes whose grid size changed, and drop shells for closed panes.
    /// Idempotent - safe to call after any layout change.
    fn sync_layout(&mut self) {
        let Some((cell_w, cell_h, _)) = self.renderer.as_ref().map(Renderer::cell_metrics) else {
            return;
        };
        let inset = self.pane_inset();
        let layout = self.tree.layout(self.viewport_rect());

        // Drop shells for panes no longer in the tree (closed panes). Hidden-by-zoom
        // panes stay, since `tree.panes()` still lists them.
        let live: HashSet<PaneId> = self.tree.panes().into_iter().collect();
        self.panes.retain(|id, _| live.contains(id));
        self.dims.retain(|id, _| live.contains(id));

        for (id, rect) in layout {
            let target = pane_dims(rect, cell_w, cell_h, inset);
            if self.panes.contains_key(&id) {
                if self.dims.get(&id) != Some(&target) {
                    if let Some(term) = self.panes.get_mut(&id) {
                        term.resize(target.0, target.1);
                    }
                    self.dims.insert(id, target);
                }
            } else {
                let proxy = self.proxy.clone();
                match Terminal::spawn(target.0, target.1, move || {
                    let _ = proxy.send_event(Wakeup);
                }) {
                    Ok(term) => {
                        self.panes.insert(id, term);
                        self.dims.insert(id, target);
                    }
                    Err(err) => {
                        tracing::error!(%err, "failed to spawn shell for a new pane");
                        // Roll the split back so every live pane still has a shell.
                        self.tree.set_focus(id);
                        self.tree.close();
                    }
                }
            }
        }
    }

    /// Repaint every visible pane from its terminal grid, resolving cell colors and
    /// overlaying the selection and the focused-pane ring.
    fn redraw(&mut self) {
        let inset = self.pane_inset();
        let layout = self.tree.layout(self.viewport_rect());
        let focused = self.tree.focused();

        let frames: Vec<PaneFrame> = layout
            .into_iter()
            .filter_map(|(id, rect)| {
                let term = self.panes.get(&id)?;
                let rows: Vec<Vec<GridCell>> = term
                    .cells()
                    .iter()
                    .map(|row| row.iter().map(|c| resolve_cell(c, &self.palette)).collect())
                    .collect();
                let cols = rows.first().map_or(0, Vec::len);
                let selection = match self.selection {
                    Some((sid, sel)) if sid == id => selection_cells(sel, rows.len(), cols),
                    _ => Vec::new(),
                };
                Some(PaneFrame {
                    rect: PxRect {
                        x: rect.x,
                        y: rect.y,
                        w: rect.w,
                        h: rect.h,
                    },
                    origin: (rect.x + inset, rect.y + inset),
                    rows,
                    cursor: term.cursor(),
                    selection,
                    focused: id == focused,
                })
            })
            .collect();

        let views: Vec<PaneView> = frames
            .iter()
            .map(|f| PaneView {
                rect: f.rect,
                origin: f.origin,
                rows: &f.rows,
                cursor: f.cursor,
                selection: &f.selection,
                focused: f.focused,
            })
            .collect();

        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_panes(&views);
            if let Err(err) = renderer.render() {
                tracing::error!(%err, "frame render failed");
            }
        }
    }

    /// Copy the current selection to the clipboard.
    fn copy_selection(&mut self) {
        let Some((id, sel)) = self.selection else {
            return;
        };
        let Some(term) = self.panes.get(&id) else {
            return;
        };
        let text = selection_text(sel, &term.cells());
        if text.is_empty() {
            return;
        }
        if let Some(clipboard) = self.clipboard.as_mut() {
            if let Err(err) = clipboard.set_text(text) {
                tracing::warn!(%err, "clipboard copy failed");
            }
        }
    }

    /// Paste the clipboard contents into the focused pane's shell.
    fn paste(&mut self) {
        let Some(text) = self.clipboard.as_mut().and_then(|c| c.get_text().ok()) else {
            return;
        };
        if let Some(term) = self.focused_term() {
            term.scroll_to_bottom();
            term.write(text.as_bytes());
        }
    }

    /// Apply a pane-tree operation, then reconcile terminals and request a repaint.
    fn apply_pane_action(&mut self, action: PaneAction) {
        let changed = match action {
            PaneAction::Split(dir) => {
                let cap = usize::from(self.config.panes.max).min(skelly_pane::MAX_PANES);
                self.tree.count() < cap && self.tree.split(dir).is_some()
            }
            PaneAction::Focus(dir) => self.tree.focus(dir),
            PaneAction::FocusIndex(index) => self
                .tree
                .panes()
                .get(index)
                .is_some_and(|&id| self.tree.set_focus(id)),
            PaneAction::Close => self.tree.close(),
            PaneAction::Zoom => {
                self.tree.zoom_toggle();
                true
            }
            PaneAction::Resize(dir) => self.tree.resize(dir, RESIZE_STEP),
            PaneAction::EvenOut => {
                self.tree.even_out();
                true
            }
        };
        if changed {
            self.selection = None;
            self.sync_layout();
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
    }

    /// Handle a key press: platform combos (quit/copy/paste), pane chords, scrollback
    /// keys, then terminal input to the focused pane.
    fn on_key(&mut self, event_loop: &ActiveEventLoop, key_event: &KeyEvent) {
        if key_event.state != ElementState::Pressed {
            return;
        }
        // Platform combos (Cmd/Super + Q/C/V). The terminal owns every other key -
        // Ctrl+C etc. still reach the shell.
        if self.modifiers.super_key() {
            if let Key::Character(ch) = key_event.logical_key.as_ref() {
                if ch.eq_ignore_ascii_case("q") {
                    event_loop.exit();
                    return;
                }
                if ch.eq_ignore_ascii_case("c") {
                    self.copy_selection();
                    return;
                }
                if ch.eq_ignore_ascii_case("v") {
                    self.paste();
                    return;
                }
            }
        }
        // Pane control (the `⌥` leader chords). Matched on the physical key so macOS
        // Option-key glyph remapping does not get in the way.
        if let PhysicalKey::Code(code) = key_event.physical_key {
            if let Some(action) = pane_action(code, self.modifiers) {
                self.apply_pane_action(action);
                return;
            }
        }
        // Shift + PageUp/PageDown scrolls the focused pane's scrollback.
        if self.modifiers.shift_key() {
            match key_event.logical_key.as_ref() {
                Key::Named(NamedKey::PageUp) => {
                    if let Some(term) = self.focused_term() {
                        term.scroll_page(true);
                    }
                    return;
                }
                Key::Named(NamedKey::PageDown) => {
                    if let Some(term) = self.focused_term() {
                        term.scroll_page(false);
                    }
                    return;
                }
                _ => {}
            }
        }
        if let Some(bytes) = key_to_bytes(key_event, self.modifiers) {
            if let Some(term) = self.focused_term() {
                // Typing jumps back to the live prompt.
                term.scroll_to_bottom();
                term.write(&bytes);
            }
            self.selection = None; // typing clears the selection
        }
    }
}

/// Owned per-pane frame data the borrowed [`PaneView`]s point at during a repaint.
struct PaneFrame {
    rect: PxRect,
    origin: (f32, f32),
    rows: Vec<Vec<GridCell>>,
    cursor: (usize, usize),
    selection: Vec<(usize, usize)>,
    focused: bool,
}

/// Resolve a terminal cell into a render cell against the active ANSI palette,
/// folding in the palette-dependent SGR effects: *dim* reduces the foreground
/// intensity and *reverse video* swaps foreground and background (using the
/// palette's default background when the cell has none). Bold/italic/underline pass
/// through for the renderer to apply.
fn resolve_cell(cell: &TermCell, palette: &AnsiPalette) -> GridCell {
    let mut fg = resolve_fg(cell.fg, palette);
    let mut bg = resolve_bg(cell.bg, palette);
    if cell.attrs.contains(CellAttrs::DIM) {
        fg = dim(fg);
    }
    if cell.attrs.contains(CellAttrs::INVERSE) {
        let fill = fg;
        fg = bg.unwrap_or_else(|| palette.default_bg());
        bg = Some(fill);
    }
    GridCell {
        c: cell.c,
        fg,
        bg,
        bold: cell.attrs.contains(CellAttrs::BOLD),
        italic: cell.attrs.contains(CellAttrs::ITALIC),
        underline: cell.attrs.contains(CellAttrs::UNDERLINE),
    }
}

/// Resolve a cell's foreground color against the active ANSI palette.
fn resolve_fg(color: CellColor, palette: &AnsiPalette) -> Srgb {
    match color {
        CellColor::Default => palette.default_fg(),
        CellColor::Indexed(index) => palette.indexed(index),
        CellColor::Rgb(r, g, b) => Srgb { r, g, b },
    }
}

/// Resolve a cell's background; the default background gets no fill (`None`).
fn resolve_bg(color: CellColor, palette: &AnsiPalette) -> Option<Srgb> {
    match color {
        CellColor::Default => None,
        CellColor::Indexed(index) => Some(palette.indexed(index)),
        CellColor::Rgb(r, g, b) => Some(Srgb { r, g, b }),
    }
}

/// Reduce a foreground color's intensity to ~60% for the SGR *dim* attribute.
fn dim(c: Srgb) -> Srgb {
    let faint = |v: u8| u8::try_from(u16::from(v) * 3 / 5).unwrap_or(v);
    Srgb {
        r: faint(c.r),
        g: faint(c.g),
        b: faint(c.b),
    }
}

impl ApplicationHandler<Wakeup> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("skelly")
            .with_inner_size(LogicalSize::new(960.0, 600.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                tracing::error!(%err, "failed to create window");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        self.scale = window.scale_factor();
        self.size = (size.width, size.height);
        let renderer = Renderer::new(
            window.clone(),
            size.width,
            size.height,
            self.scale,
            &self.config.appearance,
        );

        self.window = Some(window);
        self.renderer = Some(renderer);
        // Spawn the shell for the initial pane (and size it to the viewport).
        self.sync_layout();
        if self.panes.is_empty() {
            tracing::error!("failed to spawn the initial shell");
            event_loop.exit();
            return;
        }
        tracing::info!(panes = self.panes.len(), "window, GPU, and shell ready");
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: Wakeup) {
        // New shell output arrived; ask the window to repaint.
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                self.on_key(event_loop, &key_event);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x, position.y);
                if self.selecting {
                    if let Some((id, _)) = self.selection {
                        if let Some(rect) = self.pane_rect(id) {
                            let (cell_w, cell_h) = self.cell_size();
                            let cell = pointer_cell_in(
                                rect,
                                cell_w,
                                cell_h,
                                self.pane_inset(),
                                self.pointer,
                            );
                            if let Some((_, sel)) = self.selection.as_mut() {
                                sel.head = cell;
                            }
                        }
                    }
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                match state {
                    ElementState::Pressed => {
                        if let Some((id, rect)) = self.pane_at_pointer() {
                            self.tree.set_focus(id);
                            let (cell_w, cell_h) = self.cell_size();
                            let cell = pointer_cell_in(
                                rect,
                                cell_w,
                                cell_h,
                                self.pane_inset(),
                                self.pointer,
                            );
                            self.selection = Some((
                                id,
                                Selection {
                                    anchor: cell,
                                    head: cell,
                                },
                            ));
                            self.selecting = true;
                        }
                    }
                    ElementState::Released => {
                        self.selecting = false;
                        // A click with no drag clears the (single-cell) selection.
                        if self
                            .selection
                            .is_some_and(|(_, sel)| sel.anchor == sel.head)
                        {
                            self.selection = None;
                        }
                    }
                }
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => wheel_lines(f64::from(y)),
                    MouseScrollDelta::PixelDelta(pos) => wheel_lines(pos.y / 20.0),
                };
                if lines != 0 {
                    if let Some((id, _)) = self.pane_at_pointer() {
                        if let Some(term) = self.panes.get_mut(&id) {
                            term.scroll_lines(lines);
                        }
                    }
                }
            }
            WindowEvent::Resized(size) => {
                self.size = (size.width, size.height);
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                self.sync_layout();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

impl App {
    /// The current cell size `(width, height)` in physical px, or a `1 x 1` fallback
    /// before the renderer exists.
    fn cell_size(&self) -> (f32, f32) {
        self.renderer.as_ref().map_or((1.0, 1.0), |r| {
            let (w, h, _) = r.cell_metrics();
            (w, h)
        })
    }
}

/// Decode a physical key + modifiers into a pane action. Pane control uses `Alt`
/// (`⌥`) as its leader-less modifier, matching the design guide's shown chords
/// (`⌥|` split right, `⌥-` split down, `⌥Z` zoom, `⌥1..⌥8` focus by number), plus
/// `⌥h/j/k/l` directional focus, `⌥⇧h/j/k/l` resize, `⌥w` close, and `⌥=` even out.
/// Returns `None` for anything else (which then reaches the shell).
fn pane_action(code: KeyCode, mods: ModifiersState) -> Option<PaneAction> {
    if !mods.alt_key() {
        return None;
    }
    let shift = mods.shift_key();
    Some(match code {
        KeyCode::Backslash => PaneAction::Split(Dir::Right),
        KeyCode::Minus => PaneAction::Split(Dir::Down),
        KeyCode::Equal => PaneAction::EvenOut,
        KeyCode::KeyZ => PaneAction::Zoom,
        KeyCode::KeyW => PaneAction::Close,
        KeyCode::KeyH if shift => PaneAction::Resize(Dir::Left),
        KeyCode::KeyJ if shift => PaneAction::Resize(Dir::Down),
        KeyCode::KeyK if shift => PaneAction::Resize(Dir::Up),
        KeyCode::KeyL if shift => PaneAction::Resize(Dir::Right),
        KeyCode::KeyH => PaneAction::Focus(Dir::Left),
        KeyCode::KeyJ => PaneAction::Focus(Dir::Down),
        KeyCode::KeyK => PaneAction::Focus(Dir::Up),
        KeyCode::KeyL => PaneAction::Focus(Dir::Right),
        KeyCode::Digit1 => PaneAction::FocusIndex(0),
        KeyCode::Digit2 => PaneAction::FocusIndex(1),
        KeyCode::Digit3 => PaneAction::FocusIndex(2),
        KeyCode::Digit4 => PaneAction::FocusIndex(3),
        KeyCode::Digit5 => PaneAction::FocusIndex(4),
        KeyCode::Digit6 => PaneAction::FocusIndex(5),
        KeyCode::Digit7 => PaneAction::FocusIndex(6),
        KeyCode::Digit8 => PaneAction::FocusIndex(7),
        _ => return None,
    })
}

/// Convert a wheel delta (in lines, or approximated from pixels) to a line count.
/// Positive scrolls up into history, matching winit's convention.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a wheel step is a small number of lines"
)]
fn wheel_lines(delta: f64) -> i32 {
    delta.round() as i32
}

/// Cast a DPI scale factor to `f32`. Sub-pixel precision loss is irrelevant here.
#[allow(
    clippy::cast_possible_truncation,
    reason = "DPI scale precision loss is irrelevant for layout"
)]
fn scale32(scale: f64) -> f32 {
    scale as f32
}

/// Cast a pixel dimension to `f32` for layout math.
#[allow(
    clippy::cast_precision_loss,
    reason = "window pixel dimensions are far within f32's exact-integer range"
)]
fn dim_f32(value: u32) -> f32 {
    value as f32
}

/// Cast a pointer position to `f32`.
#[allow(
    clippy::cast_possible_truncation,
    reason = "pointer coordinates are small pixel values"
)]
fn point_f32(pointer: (f64, f64)) -> (f32, f32) {
    (pointer.0 as f32, pointer.1 as f32)
}

/// The `(cols, rows)` that fit a pane rect, given cell metrics and the inner content
/// inset (all physical px). Clamped to a small positive range; `skelly-term` applies
/// its own 2-column floor.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "cols/rows are clamped to a small positive range before the cast"
)]
fn pane_dims(rect: Rect, cell_w: f32, cell_h: f32, inset: f32) -> (u16, u16) {
    let cols = ((rect.w - 2.0 * inset) / cell_w).floor().clamp(1.0, 1000.0) as u16;
    let rows = ((rect.h - 2.0 * inset) / cell_h).floor().clamp(1.0, 1000.0) as u16;
    (cols, rows)
}

/// Map a pointer position (physical px) to a `(column, row)` cell within `rect`'s
/// content area.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pointer and cell metrics are small, non-negative pixel values"
)]
fn pointer_cell_in(
    rect: Rect,
    cell_w: f32,
    cell_h: f32,
    inset: f32,
    pointer: (f64, f64),
) -> (usize, usize) {
    let (px, py) = point_f32(pointer);
    let col = ((px - rect.x - inset) / cell_w).floor().max(0.0) as usize;
    let row = ((py - rect.y - inset) / cell_h).floor().max(0.0) as usize;
    (col, row)
}

/// Order a selection's endpoints in reading order (row-major).
fn order(sel: Selection) -> ((usize, usize), (usize, usize)) {
    let (a, h) = (sel.anchor, sel.head);
    if (a.1, a.0) <= (h.1, h.0) {
        (a, h)
    } else {
        (h, a)
    }
}

/// The cells covered by a linear selection, clamped to a `rows` x `cols` grid.
fn selection_cells(sel: Selection, rows: usize, cols: usize) -> Vec<(usize, usize)> {
    if rows == 0 || cols == 0 {
        return Vec::new();
    }
    let ((start_col, start_row), (end_col, end_row)) = order(sel);
    if start_row >= rows {
        return Vec::new();
    }
    let end_row = end_row.min(rows - 1);
    let mut cells = Vec::new();
    for row in start_row..=end_row {
        let first = if row == start_row { start_col } else { 0 };
        let last = if row == end_row { end_col } else { cols - 1 };
        for col in first..=last.min(cols - 1) {
            cells.push((col, row));
        }
    }
    cells
}

/// The text of a linear selection, row by row (trailing spaces trimmed).
fn selection_text(sel: Selection, cells: &[Vec<TermCell>]) -> String {
    if cells.is_empty() {
        return String::new();
    }
    let ((start_col, start_row), (end_col, end_row)) = order(sel);
    if start_row >= cells.len() {
        return String::new();
    }
    let end_row = end_row.min(cells.len() - 1);
    let mut lines = Vec::new();
    for (row, cols) in cells.iter().enumerate().take(end_row + 1).skip(start_row) {
        let last_col = cols.len().saturating_sub(1);
        let first = if row == start_row { start_col } else { 0 };
        let last = if row == end_row {
            end_col.min(last_col)
        } else {
            last_col
        };
        let text: String = (first..=last)
            .filter_map(|c| cols.get(c).map(|cell| cell.c))
            .collect();
        lines.push(text.trim_end().to_owned());
    }
    lines.join("\n")
}

/// Translate a key press into the bytes a terminal expects, or `None` if it has no
/// terminal representation.
fn key_to_bytes(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    match &event.logical_key {
        Key::Named(NamedKey::Enter) => Some(vec![b'\r']),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
        Key::Named(NamedKey::Tab) => Some(vec![b'\t']),
        Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
        Key::Named(NamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
        Key::Named(NamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
        Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
        Key::Named(NamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
        // Ctrl + letter -> the corresponding control byte (Ctrl+A = 0x01, etc.).
        Key::Character(ch) if modifiers.control_key() => {
            let upper = ch.chars().next()?.to_ascii_uppercase();
            if upper.is_ascii_alphabetic() {
                u8::try_from(upper).ok().map(|byte| vec![byte - b'@'])
            } else {
                event.text.as_ref().map(|text| text.as_bytes().to_vec())
            }
        }
        _ => event.text.as_ref().map(|text| text.as_bytes().to_vec()),
    }
}

/// Initialize `tracing` with an env filter (`SKELLY_LOG`, default `info`), writing
/// structured logs to stderr.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("SKELLY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

#[cfg(test)]
mod tests {
    use super::{
        dim, order, pane_action, pane_dims, pointer_cell_in, resolve_cell, selection_cells,
        selection_text, PaneAction, Selection,
    };
    use skelly_pane::{Dir, Rect};
    use skelly_render::{AnsiPalette, Srgb};
    use skelly_term::{CellAttrs, CellColor, TermCell};
    use winit::keyboard::{KeyCode, ModifiersState};

    fn plain(c: char) -> TermCell {
        TermCell {
            c,
            fg: CellColor::Default,
            bg: CellColor::Default,
            attrs: CellAttrs::empty(),
        }
    }

    fn grid(lines: &[&str]) -> Vec<Vec<TermCell>> {
        lines
            .iter()
            .map(|line| line.chars().map(plain).collect())
            .collect()
    }

    #[test]
    fn single_row_selection_text() {
        let g = grid(&["hello world"]);
        let sel = Selection {
            anchor: (0, 0),
            head: (4, 0),
        };
        assert_eq!(selection_text(sel, &g), "hello");
    }

    #[test]
    fn multi_row_selection_reads_in_order() {
        let g = grid(&["abcde", "fghij", "klmno"]);
        let sel = Selection {
            anchor: (2, 0),
            head: (2, 2),
        };
        assert_eq!(selection_text(sel, &g), "cde\nfghij\nklm");
    }

    #[test]
    fn reversed_endpoints_are_ordered() {
        let g = grid(&["abcde"]);
        let sel = Selection {
            anchor: (4, 0),
            head: (1, 0),
        };
        let (start, end) = order(sel);
        assert_eq!((start, end), ((1, 0), (4, 0)));
        assert_eq!(selection_text(sel, &g), "bcde");
    }

    #[test]
    fn selection_cells_clamp_to_the_grid() {
        let cells = selection_cells(
            Selection {
                anchor: (0, 0),
                head: (100, 100),
            },
            3,
            5,
        );
        assert_eq!(cells.len(), 15); // 3 rows x 5 cols
        assert!(cells.contains(&(4, 2)));
        assert!(!cells.iter().any(|&(c, r)| c >= 5 || r >= 3));
    }

    #[test]
    fn reverse_video_swaps_fg_and_bg() {
        let palette = AnsiPalette::resolve("ossein-dark");
        let mut cell = plain('x');
        cell.fg = CellColor::Indexed(1); // red
        cell.attrs.insert(CellAttrs::INVERSE);
        let resolved = resolve_cell(&cell, &palette);
        // The red foreground becomes the fill; the (defaulted) background becomes the
        // glyph color, drawn as the palette's default background.
        assert_eq!(resolved.bg, Some(palette.indexed(1)));
        assert_eq!(resolved.fg, palette.default_bg());
    }

    #[test]
    fn reverse_video_with_an_explicit_background() {
        let palette = AnsiPalette::resolve("ossein-dark");
        let mut cell = plain('x');
        cell.fg = CellColor::Indexed(2); // green
        cell.bg = CellColor::Indexed(4); // blue
        cell.attrs.insert(CellAttrs::INVERSE);
        let resolved = resolve_cell(&cell, &palette);
        assert_eq!(resolved.fg, palette.indexed(4));
        assert_eq!(resolved.bg, Some(palette.indexed(2)));
    }

    #[test]
    fn dim_darkens_the_foreground() {
        let bright = Srgb {
            r: 200,
            g: 100,
            b: 50,
        };
        let faint = dim(bright);
        assert_eq!(
            faint,
            Srgb {
                r: 120,
                g: 60,
                b: 30
            }
        );
        assert_eq!(dim(Srgb { r: 0, g: 0, b: 0 }), Srgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn attributes_pass_through_to_the_render_cell() {
        let palette = AnsiPalette::resolve("ossein-dark");
        let mut cell = plain('b');
        cell.attrs
            .insert(CellAttrs::BOLD | CellAttrs::ITALIC | CellAttrs::UNDERLINE);
        let resolved = resolve_cell(&cell, &palette);
        assert!(resolved.bold && resolved.italic && resolved.underline);
    }

    // ----- pane wiring geometry + keybindings ---------------------------------

    #[test]
    fn pane_dims_fit_cells_inside_the_inset() {
        // 800 wide, 12px inset each side, 10px cells -> floor(776 / 10) = 77 cols.
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let (cols, rows) = pane_dims(rect, 10.0, 20.0, 12.0);
        assert_eq!(cols, 77);
        assert_eq!(rows, 28); // floor((600 - 24) / 20)
    }

    #[test]
    fn pointer_maps_to_a_cell_relative_to_the_pane_origin() {
        // Pane at (100, 200), 6px inset, 10x20 cells. A pointer 3.5 cells in.
        let rect = Rect::new(100.0, 200.0, 400.0, 300.0);
        let cell = pointer_cell_in(
            rect,
            10.0,
            20.0,
            6.0,
            (100.0 + 6.0 + 35.0, 200.0 + 6.0 + 50.0),
        );
        assert_eq!(cell, (3, 2));
    }

    #[test]
    fn pointer_above_the_content_clamps_to_the_first_cell() {
        let rect = Rect::new(100.0, 200.0, 400.0, 300.0);
        // Pointer in the inset gutter maps to cell (0, 0), never negative.
        let cell = pointer_cell_in(rect, 10.0, 20.0, 6.0, (100.0, 200.0));
        assert_eq!(cell, (0, 0));
    }

    #[test]
    fn alt_chords_decode_to_pane_actions() {
        let alt = ModifiersState::ALT;
        let alt_shift = ModifiersState::ALT | ModifiersState::SHIFT;
        assert_eq!(
            pane_action(KeyCode::Backslash, alt),
            Some(PaneAction::Split(Dir::Right))
        );
        assert_eq!(
            pane_action(KeyCode::Minus, alt),
            Some(PaneAction::Split(Dir::Down))
        );
        assert_eq!(pane_action(KeyCode::KeyZ, alt), Some(PaneAction::Zoom));
        assert_eq!(pane_action(KeyCode::KeyW, alt), Some(PaneAction::Close));
        assert_eq!(
            pane_action(KeyCode::KeyL, alt),
            Some(PaneAction::Focus(Dir::Right))
        );
        assert_eq!(
            pane_action(KeyCode::KeyL, alt_shift),
            Some(PaneAction::Resize(Dir::Right))
        );
        assert_eq!(
            pane_action(KeyCode::Digit3, alt),
            Some(PaneAction::FocusIndex(2))
        );
    }

    #[test]
    fn keys_without_alt_are_not_pane_actions() {
        // Without the Alt leader the key belongs to the shell.
        assert_eq!(pane_action(KeyCode::KeyL, ModifiersState::empty()), None);
        assert_eq!(pane_action(KeyCode::KeyA, ModifiersState::ALT), None);
    }
}
