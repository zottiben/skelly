//! `skelly` - the binary: window, event loop, and the wiring that binds the library
//! crates together.
//!
//! M1c completes the walking skeleton: load the config, open a native window, spawn
//! the login shell in a PTY (`skelly-term`), and paint its live grid on the GPU
//! (`skelly-render`), forwarding keystrokes to the shell. The reader thread wakes
//! the event loop via an `EventLoopProxy` so we repaint only on new output. Errors
//! are contextualized here with `anyhow`; `wgpu`/`alacritty` types never leak up.

use std::sync::Arc;

use anyhow::Context as _;
use skelly_config::{Appearance, Config};
use skelly_render::{AnsiPalette, GridCell, Renderer, Srgb};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

/// Event the reader thread sends to wake the UI when the shell produces output.
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

/// A text selection, in visible-grid cell coordinates `(column, row)`.
#[derive(Clone, Copy)]
struct Selection {
    anchor: (usize, usize),
    head: (usize, usize),
}

/// Application state driven by the winit event loop. The window, renderer, and
/// terminal are `None` until the platform signals `resumed`.
struct App {
    config: Config,
    proxy: EventLoopProxy<Wakeup>,
    palette: AnsiPalette,
    clipboard: Option<arboard::Clipboard>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    terminal: Option<Terminal>,
    scale: f64,
    modifiers: ModifiersState,
    pointer: (f64, f64),
    selection: Option<Selection>,
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
            terminal: None,
            scale: 1.0,
            modifiers: ModifiersState::empty(),
            pointer: (0.0, 0.0),
            selection: None,
            selecting: false,
        }
    }

    /// Repaint from the current terminal grid, resolving each cell's colors and
    /// overlaying the selection highlight.
    fn redraw(&mut self) {
        let (rows, cursor) = self.terminal.as_ref().map_or_else(
            || (Vec::new(), (0, 0)),
            |term| {
                let rows: Vec<Vec<GridCell>> = term
                    .cells()
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| resolve_cell(cell, &self.palette))
                            .collect()
                    })
                    .collect();
                (rows, term.cursor())
            },
        );
        let cols = rows.first().map_or(0, Vec::len);
        let selection = self
            .selection
            .map(|sel| selection_cells(sel, rows.len(), cols))
            .unwrap_or_default();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_grid(&rows, cursor, &selection);
            if let Err(err) = renderer.render() {
                tracing::error!(%err, "frame render failed");
            }
        }
    }

    /// Map the last pointer position to a grid cell `(column, row)`.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "pointer and cell metrics are small, non-negative pixel values"
    )]
    fn pointer_cell(&self) -> (usize, usize) {
        let Some(renderer) = self.renderer.as_ref() else {
            return (0, 0);
        };
        let (cell_w, cell_h, pad) = renderer.cell_metrics();
        let col = ((self.pointer.0 as f32 - pad) / cell_w).floor().max(0.0) as usize;
        let row = ((self.pointer.1 as f32 - pad) / cell_h).floor().max(0.0) as usize;
        (col, row)
    }

    /// Copy the current selection to the clipboard.
    fn copy_selection(&mut self) {
        let Some(sel) = self.selection else { return };
        let Some(term) = self.terminal.as_ref() else {
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

    /// Paste the clipboard contents into the shell.
    fn paste(&mut self) {
        let Some(text) = self.clipboard.as_mut().and_then(|c| c.get_text().ok()) else {
            return;
        };
        if let Some(term) = self.terminal.as_mut() {
            term.scroll_to_bottom();
            term.write(text.as_bytes());
        }
    }

    /// Handle a key press: platform combos (quit/copy/paste), scrollback keys, then
    /// terminal input.
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
        // Shift + PageUp/PageDown scrolls the scrollback (not sent to the shell).
        if self.modifiers.shift_key() {
            match key_event.logical_key.as_ref() {
                Key::Named(NamedKey::PageUp) => {
                    if let Some(terminal) = self.terminal.as_mut() {
                        terminal.scroll_page(true);
                    }
                    return;
                }
                Key::Named(NamedKey::PageDown) => {
                    if let Some(terminal) = self.terminal.as_mut() {
                        terminal.scroll_page(false);
                    }
                    return;
                }
                _ => {}
            }
        }
        if let Some(bytes) = key_to_bytes(key_event, self.modifiers) {
            if let Some(terminal) = self.terminal.as_mut() {
                // Typing jumps back to the live prompt.
                terminal.scroll_to_bottom();
                terminal.write(&bytes);
            }
            self.selection = None; // typing clears the selection
        }
    }
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
        let renderer = Renderer::new(
            window.clone(),
            size.width,
            size.height,
            self.scale,
            &self.config.appearance,
        );

        let (cols, rows) = grid_dims(size.width, size.height, &self.config.appearance, self.scale);
        let proxy = self.proxy.clone();
        let terminal = match Terminal::spawn(cols, rows, move || {
            let _ = proxy.send_event(Wakeup);
        }) {
            Ok(terminal) => terminal,
            Err(err) => {
                tracing::error!(%err, "failed to spawn shell");
                event_loop.exit();
                return;
            }
        };

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.terminal = Some(terminal);
        tracing::info!(cols, rows, "window, GPU, and shell ready");
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
                    let cell = self.pointer_cell();
                    if let Some(selection) = self.selection.as_mut() {
                        selection.head = cell;
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
                        let cell = self.pointer_cell();
                        self.selection = Some(Selection {
                            anchor: cell,
                            head: cell,
                        });
                        self.selecting = true;
                    }
                    ElementState::Released => {
                        self.selecting = false;
                        // A click with no drag clears the (single-cell) selection.
                        if self.selection.is_some_and(|sel| sel.anchor == sel.head) {
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
                    if let Some(terminal) = self.terminal.as_mut() {
                        terminal.scroll_lines(lines);
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                let (cols, rows) =
                    grid_dims(size.width, size.height, &self.config.appearance, self.scale);
                if let Some(terminal) = self.terminal.as_mut() {
                    terminal.resize(cols, rows);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
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

/// Estimate the terminal grid size (cols, rows) from the window's physical size and
/// the cell metrics. Approximate for now - the exact cell size comes from font
/// shaping in M2.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "cols/rows are clamped to a small positive range before the cast"
)]
fn grid_dims(width: u32, height: u32, appearance: &Appearance, scale: f64) -> (u16, u16) {
    let font_px = f64::from(appearance.font_size) * scale;
    let cell_w = (font_px * 0.6).max(1.0);
    let cell_h = (font_px * f64::from(appearance.line_height)).max(1.0);
    let cols = (f64::from(width) / cell_w).floor().clamp(1.0, 1000.0) as u16;
    let rows = (f64::from(height) / cell_h).floor().clamp(1.0, 1000.0) as u16;
    (cols, rows)
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
    use super::{dim, order, resolve_cell, selection_cells, selection_text, Selection};
    use skelly_render::{AnsiPalette, Srgb};
    use skelly_term::{CellAttrs, CellColor, TermCell};

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
}
