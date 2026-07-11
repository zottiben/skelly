//! `skelly-term` - the terminal core.
//!
//! Spawns the login shell in a PTY (`portable-pty`, ADR-0002), parses its output
//! with `alacritty_terminal` (ADR-0001) into a cell grid, and forwards input. The
//! `alacritty_terminal` types are kept internal - callers see only [`Terminal`] and
//! a plain-text grid snapshot, so the engine stays swappable and no UI/GPU
//! dependency leaks in.
//!
//! M1c: a single shell, monochrome text snapshot (no scrollback, colors, or
//! selection yet - those arrive with the real cell grid in M2). A background thread
//! reads the PTY and advances the parser; the UI thread snapshots and writes input.

#![doc(test(attr(deny(warnings))))]

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Processor};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

type SharedTerm = Arc<Mutex<Term<VoidListener>>>;

/// A terminal color, independent of any palette: the default foreground, a palette
/// index (ANSI 16 or the 256-color cube), or a 24-bit truecolor. Resolving an index
/// to concrete RGB is a theming concern the renderer owns (Hard rule 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellColor {
    /// The palette's default foreground.
    Default,
    /// A palette index (0..=255).
    Indexed(u8),
    /// A 24-bit truecolor.
    Rgb(u8, u8, u8),
}

bitflags::bitflags! {
    /// The SGR text attributes set on a cell, as a compact flag set. Palette
    /// independent: `BOLD`/`ITALIC`/`UNDERLINE` are font-level effects the renderer
    /// applies directly, while `INVERSE` (reverse video) and `DIM` are resolved
    /// against a concrete palette (swap fg/bg, reduce intensity) per Hard rule 2.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct CellAttrs: u8 {
        /// Bold weight (`ESC[1m`).
        const BOLD = 1 << 0;
        /// Italic style (`ESC[3m`).
        const ITALIC = 1 << 1;
        /// Underline (`ESC[4m`, plus the double / curly / dotted / dashed variants).
        const UNDERLINE = 1 << 2;
        /// Reverse video (`ESC[7m`): swap foreground and background.
        const INVERSE = 1 << 3;
        /// Dim / faint (`ESC[2m`): reduce the foreground intensity.
        const DIM = 1 << 4;
    }
}

/// One grid cell: its character, colors, and SGR text attributes.
#[derive(Clone, Copy, Debug)]
pub struct TermCell {
    /// The cell's character (a space if empty).
    pub c: char,
    /// The cell's foreground color.
    pub fg: CellColor,
    /// The cell's background color ([`CellColor::Default`] means the terminal's
    /// default background - the renderer draws no fill for those).
    pub bg: CellColor,
    /// The cell's SGR text attributes.
    pub attrs: CellAttrs,
}

/// A live terminal: a shell in a PTY, parsed into a cell grid.
///
/// The shell runs until it exits or the `Terminal` is dropped. New output sets a
/// dirty flag ([`Terminal::take_dirty`]) so the UI only repaints when something
/// changed.
pub struct Terminal {
    term: SharedTerm,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    dirty: Arc<AtomicBool>,
    _child: Box<dyn Child + Send + Sync>,
    _reader: JoinHandle<()>,
}

impl Terminal {
    /// Spawn the login shell (`$SHELL`, else `/bin/bash`) in a `cols` x `rows` PTY.
    ///
    /// `wakeup` is invoked from the reader thread whenever new output arrives, so
    /// the caller can request a repaint (e.g. via a winit `EventLoopProxy`) instead
    /// of polling. Pass `|| {}` when no wakeup is needed.
    ///
    /// # Errors
    /// Returns an error if the PTY cannot be opened or the shell cannot be spawned.
    pub fn spawn<W>(cols: u16, rows: u16, wakeup: W) -> std::io::Result<Self>
    where
        W: Fn() + Send + 'static,
    {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(to_io)?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        let child = pair.slave.spawn_command(cmd).map_err(to_io)?;
        drop(pair.slave); // close the parent's slave handle so the master sees EOF.

        let reader = pair.master.try_clone_reader().map_err(to_io)?;
        let writer = pair.master.take_writer().map_err(to_io)?;

        let dims = GridSize::new(cols, rows);
        let term: SharedTerm = Arc::new(Mutex::new(Term::new(
            Config::default(),
            &dims,
            VoidListener,
        )));
        let dirty = Arc::new(AtomicBool::new(true));

        let reader_term = Arc::clone(&term);
        let reader_dirty = Arc::clone(&dirty);
        let handle = thread::spawn(move || read_loop(reader, &reader_term, &reader_dirty, wakeup));

        Ok(Self {
            term,
            master: pair.master,
            writer,
            dirty,
            _child: child,
            _reader: handle,
        })
    }

    /// Send bytes to the shell (keyboard input, pastes).
    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Resize the PTY and the grid to `cols` x `rows`.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut term) = self.term.lock() {
            term.resize(GridSize::new(cols, rows));
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Take the dirty flag: `true` if new output has arrived since the last call.
    #[must_use]
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    /// Snapshot the visible grid as trimmed text lines (top to bottom).
    ///
    /// # Panics
    /// Panics if the terminal mutex is poisoned (i.e. the reader thread panicked
    /// while holding it).
    #[must_use]
    pub fn snapshot(&self) -> Vec<String> {
        let term = self.term.lock().expect("terminal mutex poisoned");
        let grid = term.grid();
        let columns = grid.columns();
        let lines = grid.screen_lines();
        let offset = i32::try_from(grid.display_offset()).unwrap_or(0);

        let mut out = Vec::with_capacity(lines);
        for line in 0..lines {
            let row = Line(i32::try_from(line).unwrap_or(0) - offset);
            let mut text = String::with_capacity(columns);
            for column in 0..columns {
                text.push(grid[row][Column(column)].c);
            }
            out.push(text.trim_end().to_owned());
        }
        out
    }

    /// Snapshot the visible grid as cells with per-cell foreground color (top to
    /// bottom, left to right). Used by the renderer to draw a colored grid.
    ///
    /// # Panics
    /// Panics if the terminal mutex is poisoned (i.e. the reader thread panicked
    /// while holding it).
    #[must_use]
    pub fn cells(&self) -> Vec<Vec<TermCell>> {
        let term = self.term.lock().expect("terminal mutex poisoned");
        let grid = term.grid();
        let columns = grid.columns();
        let lines = grid.screen_lines();
        let offset = i32::try_from(grid.display_offset()).unwrap_or(0);

        let mut out = Vec::with_capacity(lines);
        for line in 0..lines {
            let row = Line(i32::try_from(line).unwrap_or(0) - offset);
            let mut cells = Vec::with_capacity(columns);
            for column in 0..columns {
                let cell = &grid[row][Column(column)];
                cells.push(TermCell {
                    c: cell.c,
                    fg: map_color(cell.fg),
                    bg: map_color(cell.bg),
                    attrs: map_attrs(cell.flags),
                });
            }
            out.push(cells);
        }
        out
    }

    /// The cursor's position as `(column, row)` in the visible grid.
    ///
    /// # Panics
    /// Panics if the terminal mutex is poisoned.
    #[must_use]
    pub fn cursor(&self) -> (usize, usize) {
        let term = self.term.lock().expect("terminal mutex poisoned");
        let grid = term.grid();
        let offset = i32::try_from(grid.display_offset()).unwrap_or(0);
        let point = grid.cursor.point;
        (
            point.column.0,
            usize::try_from(point.line.0 + offset).unwrap_or(0),
        )
    }

    /// Scroll the view by `delta` lines (positive scrolls up into history).
    pub fn scroll_lines(&mut self, delta: i32) {
        self.scroll(Scroll::Delta(delta));
    }

    /// Scroll one page up (`true`) or down (`false`).
    pub fn scroll_page(&mut self, up: bool) {
        self.scroll(if up { Scroll::PageUp } else { Scroll::PageDown });
    }

    /// Jump back to the live bottom of the buffer.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll(Scroll::Bottom);
    }

    fn scroll(&mut self, scroll: Scroll) {
        if let Ok(mut term) = self.term.lock() {
            term.scroll_display(scroll);
        }
        self.dirty.store(true, Ordering::Relaxed);
    }
}

/// Map an `alacritty_terminal` cell's flags to our SGR [`CellAttrs`]. The various
/// underline styles all collapse to a single underline for now.
fn map_attrs(flags: Flags) -> CellAttrs {
    let mut attrs = CellAttrs::empty();
    attrs.set(CellAttrs::BOLD, flags.contains(Flags::BOLD));
    attrs.set(CellAttrs::ITALIC, flags.contains(Flags::ITALIC));
    attrs.set(
        CellAttrs::UNDERLINE,
        flags.intersects(Flags::ALL_UNDERLINES),
    );
    attrs.set(CellAttrs::INVERSE, flags.contains(Flags::INVERSE));
    attrs.set(CellAttrs::DIM, flags.contains(Flags::DIM));
    attrs
}

/// Map an `alacritty_terminal` cell color to a palette-independent [`CellColor`].
fn map_color(color: AnsiColor) -> CellColor {
    match color {
        AnsiColor::Spec(rgb) => CellColor::Rgb(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(index) => CellColor::Indexed(index),
        // The 16 ANSI names map to indices 0..=15; everything else (Foreground,
        // Background, Cursor, Dim*) falls back to the default foreground for now.
        AnsiColor::Named(named) => u8::try_from(named as usize)
            .ok()
            .filter(|&index| index < 16)
            .map_or(CellColor::Default, CellColor::Indexed),
    }
}

/// Grid dimensions with no scrollback history (M1c).
#[derive(Clone, Copy)]
struct GridSize {
    columns: usize,
    screen_lines: usize,
}

impl GridSize {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            columns: usize::from(cols),
            screen_lines: usize::from(rows),
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Read PTY bytes and advance the parser until EOF (shell exit) or a read error.
fn read_loop<W: Fn()>(
    mut reader: Box<dyn Read + Send>,
    term: &Mutex<Term<VoidListener>>,
    dirty: &AtomicBool,
    wakeup: W,
) {
    // `Processor`'s default sync handler (`StdSyncHandler`) is fine; annotate so the
    // type parameter resolves.
    let mut processor: Processor = Processor::new();
    let mut buf = [0_u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Ok(mut term) = term.lock() {
                    processor.advance(&mut *term, &buf[..n]);
                }
                dirty.store(true, Ordering::Relaxed);
                wakeup();
            }
        }
    }
}

/// Map any displayable error into an `io::Error` so callers need not depend on the
/// PTY crate's error type.
fn to_io<E: std::fmt::Display>(err: E) -> std::io::Error {
    std::io::Error::other(err.to_string())
}
