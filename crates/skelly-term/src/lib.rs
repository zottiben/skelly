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
//!
//! [`Parser`] is the headless twin of [`Terminal`]: the same `alacritty_terminal`
//! grid and `Processor`, driven directly by bytes with no PTY, shell, or thread.
//! It lets conformance tests and fuzzers exercise the exact parse path
//! deterministically (see `tests/conformance.rs` and `tests/robustness.rs`).

#![doc(test(attr(deny(warnings))))]

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape as VteCursorShape, Processor};
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

type SharedTerm = Arc<Mutex<Term<TitleListener>>>;
/// The latest OS window title the running program set (via `OSC 0/2`), shared between the
/// reader thread's parser and the UI. Editors set it to the open file's name, which the status
/// line reads for the filetype (design §10.4).
type SharedTitle = Arc<Mutex<Option<String>>>;

/// A terminal [`EventListener`] that keeps only the latest title the program set (`OSC 0/2`),
/// discarding every other event. `send_event` takes `&self`, so the title lives behind a
/// shared `Mutex` the [`Terminal`] also holds to read it. A default (empty-handle) instance
/// discards titles - used by the headless [`Parser`], which has no UI to show one.
#[derive(Clone, Default)]
struct TitleListener {
    title: SharedTitle,
}

impl EventListener for TitleListener {
    fn send_event(&self, event: Event) {
        if let Event::Title(title) = event {
            *self.title.lock().expect("title mutex poisoned") = Some(title);
        }
    }
}

/// The narrowest grid we allow. `alacritty_terminal`'s reflow degenerates into a
/// pathological (multi-second) loop at exactly one column - a size no usable
/// terminal reaches, but reachable by dragging the window to a sliver, which would
/// let hostile output wedge the reflow. Two columns and up reflow in microseconds,
/// so we clamp the floor here, at the single point where dimensions enter the core,
/// keeping the PTY size and the grid in lockstep. See `tests/robustness.rs`.
const MIN_COLS: u16 = 2;
/// The shortest grid we allow: a zero-row grid is degenerate and panic-prone.
const MIN_ROWS: u16 = 1;

/// Clamp requested dimensions to the usable floor ([`MIN_COLS`] x [`MIN_ROWS`]).
fn clamp_dims(cols: u16, rows: u16) -> (u16, u16) {
    (cols.max(MIN_COLS), rows.max(MIN_ROWS))
}

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

/// The cursor shape a running program has requested (via `DECSCUSR`), reported by
/// [`Terminal::cursor_shape`]. Modal editors set it per mode, which the status line maps to an
/// editor-mode label (design §10.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    /// A full-cell block (editors: normal mode; also the default at a shell prompt).
    Block,
    /// A thin vertical bar (editors: insert mode).
    Bar,
    /// A thin underline (editors: replace mode).
    Underline,
    /// An invisible cursor.
    Hidden,
}

/// A scrollback search hit ([`Terminal::find`], the guide's `⌘F`): where the match sits in the
/// visible grid after it is scrolled into view, plus the buffer line to continue searching from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FindHit {
    /// The match's row in the visible grid (`0` = top visible row).
    pub row: usize,
    /// The match's starting column in that row.
    pub col: usize,
    /// The match length in columns (clamped to the row end for multi-line matches).
    pub len: usize,
    /// The match's absolute buffer line, to pass back as `from` for the next/prev search.
    pub line: i32,
}

/// Escape a query so a `RegexSearch` matches it literally (backslash-prefix the regex metachars).
fn escape_regex(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for c in query.chars() {
        if ".^$*+?()[]{}|\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// How the shell ended, reported once the child process has exited.
///
/// The shell runs until the user exits it (`exit`, Ctrl-D), it is killed, or it
/// crashes; when that happens the reader thread reaps the child and records this. The
/// binary reads it via [`Terminal::exit_status`] to draw the shell-exit overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitStatus {
    /// The process exit code (`0` is a clean exit).
    pub code: u32,
    /// The terminating signal's name (Unix), if the shell was killed by one.
    pub signal: Option<String>,
}

impl ExitStatus {
    /// Whether the shell ended cleanly (exit code `0`, no terminating signal).
    #[must_use]
    pub fn success(&self) -> bool {
        self.signal.is_none() && self.code == 0
    }
}

/// The shell's exit status, shared between the reader thread (which records it once
/// the child is reaped) and the UI thread (which polls it via [`Terminal::exit_status`]).
type SharedExit = Arc<Mutex<Option<ExitStatus>>>;

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
/// changed; when the shell itself ends, its [`exit_status`](Terminal::exit_status)
/// becomes `Some` and one final wakeup fires so the UI can draw the exit overlay.
///
/// Dropping a `Terminal` kills the shell (via a cloned [`ChildKiller`]) so the reader
/// thread's `wait()` reaps it and exits - no lingering process or thread.
pub struct Terminal {
    term: SharedTerm,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    dirty: Arc<AtomicBool>,
    /// Set once the shell exits (the reader thread reaps the child and records it).
    exit: SharedExit,
    /// Kills the shell on drop so the reader thread unblocks, `wait()`s, and exits.
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// The shell's own pid, captured at spawn - it is the leader of its process group, so
    /// comparing it to the PTY's foreground process group reveals a running foreground job.
    shell_pid: Option<u32>,
    /// The latest window title the running program set (`OSC 0/2`); the status line reads it
    /// for an editor's open-file name / filetype (design §10.4).
    title: SharedTitle,
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
        Self::spawn_shell(cols, rows, None, wakeup)
    }

    /// Spawn a specific shell `program` (e.g. `zsh`) in a `cols` x `rows` PTY, or the login
    /// shell when `program` is `None` or empty. Backs the `[shell] program` config key set by
    /// the settings view / first-run onboarding (design §10.1). `wakeup` behaves as in
    /// [`spawn`](Terminal::spawn).
    ///
    /// # Errors
    /// Returns an error if the PTY cannot be opened or the shell cannot be spawned.
    pub fn spawn_shell<W>(
        cols: u16,
        rows: u16,
        program: Option<&str>,
        wakeup: W,
    ) -> std::io::Result<Self>
    where
        W: Fn() + Send + 'static,
    {
        let (cols, rows) = clamp_dims(cols, rows);
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(to_io)?;

        // A configured, non-empty program wins; otherwise the login shell ($SHELL, else bash).
        let shell = program
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map_or_else(
                || std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned()),
                ToOwned::to_owned,
            );
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        let child = pair.slave.spawn_command(cmd).map_err(to_io)?;
        drop(pair.slave); // close the parent's slave handle so the master sees EOF.
                          // A killer we keep on the UI side so dropping the `Terminal` can stop the shell
                          // even mid-job; the child itself moves into the reader thread to be reaped.
        let killer = child.clone_killer();
        // Capture the shell's pid before the child moves into the reader thread; the shell
        // is its own process-group leader, so this identifies "no foreground job running".
        let shell_pid = child.process_id();

        let reader = pair.master.try_clone_reader().map_err(to_io)?;
        let writer = pair.master.take_writer().map_err(to_io)?;

        let dims = GridSize::new(cols, rows);
        let title: SharedTitle = Arc::new(Mutex::new(None));
        let term: SharedTerm = Arc::new(Mutex::new(Term::new(
            Config::default(),
            &dims,
            TitleListener {
                title: Arc::clone(&title),
            },
        )));
        let dirty = Arc::new(AtomicBool::new(true));
        let exit: SharedExit = Arc::new(Mutex::new(None));

        let reader_term = Arc::clone(&term);
        let reader_dirty = Arc::clone(&dirty);
        let reader_exit = Arc::clone(&exit);
        let handle = thread::spawn(move || {
            read_loop(
                reader,
                &reader_term,
                &reader_dirty,
                &reader_exit,
                child,
                wakeup,
            );
        });

        Ok(Self {
            term,
            master: pair.master,
            writer,
            dirty,
            exit,
            killer,
            shell_pid,
            title,
            _reader: handle,
        })
    }

    /// The pid of the foreground job running in this pane, if any - i.e. a process the
    /// shell has put in the foreground that is not the shell itself (e.g. `vim`, `cargo`).
    /// Returns `None` when the shell is idle at its prompt (its own process group is in the
    /// foreground) or when the foreground group can't be read. The binary uses this to warn
    /// before closing a pane that would kill a running job (design §12 "Process running on
    /// close").
    #[must_use]
    pub fn foreground_job_pid(&self) -> Option<u32> {
        let foreground = u32::try_from(self.master.process_group_leader()?).ok()?;
        let shell = self.shell_pid?;
        (foreground != shell).then_some(foreground)
    }

    /// Send bytes to the shell (keyboard input, pastes).
    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Resize the PTY and the grid to `cols` x `rows` (clamped to the usable floor).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = clamp_dims(cols, rows);
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

    /// The shell's exit status, or `None` while it is still running. Becomes `Some`
    /// once the shell exits (or is killed / crashes); the reader thread fires one final
    /// wakeup at that point so the UI repaints and can show the shell-exit overlay.
    #[must_use]
    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.exit.lock().ok().and_then(|status| status.clone())
    }

    /// Snapshot the visible grid as trimmed text lines (top to bottom).
    ///
    /// # Panics
    /// Panics if the terminal mutex is poisoned (i.e. the reader thread panicked
    /// while holding it).
    #[must_use]
    pub fn snapshot(&self) -> Vec<String> {
        let term = self.term.lock().expect("terminal mutex poisoned");
        grid_lines(&term)
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
        grid_cells(&term)
    }

    /// The cursor's position as `(column, row)` in the visible grid.
    ///
    /// # Panics
    /// Panics if the terminal mutex is poisoned.
    #[must_use]
    pub fn cursor(&self) -> (usize, usize) {
        let term = self.term.lock().expect("terminal mutex poisoned");
        grid_cursor(&term)
    }

    /// The cursor shape the running program has set (via `DECSCUSR`). Modal editors change it
    /// per mode - block in normal, bar in insert, underline in replace - so the status line can
    /// report the editor mode (design §10.4) from a real terminal signal rather than guessing.
    ///
    /// The latest window title the running program set (via `OSC 0/2`), or `None` if it never
    /// set one. Editors set it to the open file (e.g. `main.rs (…) - NVIM`), which the status
    /// line parses for the filetype (design §10.4).
    ///
    /// # Panics
    /// Panics if the title mutex is poisoned.
    #[must_use]
    pub fn title(&self) -> Option<String> {
        self.title.lock().expect("title mutex poisoned").clone()
    }

    /// # Panics
    /// Panics if the terminal mutex is poisoned.
    #[must_use]
    pub fn cursor_shape(&self) -> CursorShape {
        let term = self.term.lock().expect("terminal mutex poisoned");
        match term.cursor_style().shape {
            VteCursorShape::Beam => CursorShape::Bar,
            VteCursorShape::Underline => CursorShape::Underline,
            VteCursorShape::Hidden => CursorShape::Hidden,
            // Block / HollowBlock both read as a block cursor for mode purposes.
            VteCursorShape::Block | VteCursorShape::HollowBlock => CursorShape::Block,
        }
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

    /// Search the scrollback for `query` (a literal, case-sensitive match) and scroll the found
    /// match into view (the guide's `⌘F` "Find in scrollback"). `from` is the line of the current
    /// match to continue from (`None` starts fresh - the newest match searching up, or the oldest
    /// searching down); `forward` searches toward newer output. Returns the match's visible
    /// position, or `None` when there is no match.
    ///
    /// # Panics
    /// Panics if the terminal mutex is poisoned.
    pub fn find(&mut self, query: &str, from: Option<i32>, forward: bool) -> Option<FindHit> {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Direction, Line, Point, Side};
        use alacritty_terminal::term::search::RegexSearch;

        if query.is_empty() {
            return None;
        }
        let mut regex = RegexSearch::new(&escape_regex(query)).ok()?;
        let mut term = self.term.lock().expect("terminal mutex poisoned");
        let cols = term.columns();
        let screen = term.screen_lines();
        let history = term.history_size();
        let dir = if forward {
            Direction::Right
        } else {
            Direction::Left
        };
        // Step one line off the current match so we advance to the next one; a fresh search starts
        // at the bottom (searching up) or the top of history (searching down).
        let origin = match from {
            Some(line) if forward => Point::new(Line(line + 1), Column(0)),
            Some(line) => Point::new(Line(line - 1), Column(cols.saturating_sub(1))),
            None if forward => Point::new(Line(-(i32::try_from(history).unwrap_or(0))), Column(0)),
            None => Point::new(
                Line(i32::try_from(screen).unwrap_or(1) - 1),
                Column(cols.saturating_sub(1)),
            ),
        };
        let m = term.search_next(&mut regex, origin, dir, Side::Left, None)?;
        let (start, end) = (*m.start(), *m.end());
        term.scroll_to_point(start);
        let offset = i32::try_from(term.grid().display_offset()).unwrap_or(0);
        let row = usize::try_from((start.line.0 + offset).max(0)).unwrap_or(0);
        let col = start.column.0.min(cols.saturating_sub(1));
        // A match may span lines; highlight to the row end in that case.
        let len = if end.line == start.line {
            (end.column.0 + 1).saturating_sub(col)
        } else {
            cols - col
        };
        self.dirty.store(true, Ordering::Relaxed);
        Some(FindHit {
            row,
            col,
            len,
            line: start.line.0,
        })
    }

    /// Clear the saved scrollback history (the guide's `⌘L` "Clear scrollback"), leaving the
    /// visible screen and the shell untouched. Snaps the view back to the bottom.
    pub fn clear_scrollback(&mut self) {
        use alacritty_terminal::vte::ansi::{ClearMode, Handler};
        if let Ok(mut term) = self.term.lock() {
            term.clear_screen(ClearMode::Saved);
            term.scroll_display(Scroll::Bottom);
        }
        self.dirty.store(true, Ordering::Relaxed);
    }
}

impl Drop for Terminal {
    /// Stop the shell so it can't outlive its pane. Killing it closes the PTY slave,
    /// which unblocks the reader thread's `read` (EOF); the thread then `wait()`s -
    /// reaping the child - and exits.
    ///
    /// Only kill while the shell is still unreaped (no recorded exit): once the reader
    /// thread has `wait()`ed, the pid is free to be reused, so signalling it could hit an
    /// unrelated process. An unreaped pid (running or zombie) is still reserved, so killing
    /// it is safe; an already-exited shell needs no killing anyway.
    fn drop(&mut self) {
        if self.exit_status().is_none() {
            let _ = self.killer.kill();
        }
    }
}

/// A headless VT parser: an `alacritty_terminal` grid driven directly by bytes,
/// with no PTY, shell, or reader thread.
///
/// [`Parser`] shares the exact parse path the live [`Terminal`] uses - the same
/// `Processor` and `Term`, built from the same [`Config`] (so scrollback history
/// matches) - which makes it the harness for deterministic conformance tests and
/// fuzzing: feed known (or arbitrary) bytes with [`advance`](Parser::advance), then
/// read the grid back with the same [`snapshot`](Parser::snapshot),
/// [`cells`](Parser::cells), and [`cursor`](Parser::cursor) methods.
pub struct Parser {
    term: Term<TitleListener>,
    processor: Processor,
}

impl Parser {
    /// Create a `cols` x `rows` headless grid, configured identically to a live
    /// [`Terminal`] (same 10k-line scrollback history).
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        let (cols, rows) = clamp_dims(cols, rows);
        let dims = GridSize::new(cols, rows);
        Self {
            term: Term::new(Config::default(), &dims, TitleListener::default()),
            processor: Processor::new(),
        }
    }

    /// Feed bytes through the VT parser into the grid - the same call the live
    /// reader thread makes on PTY output.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    /// Snapshot the visible grid as trimmed text lines (top to bottom).
    #[must_use]
    pub fn snapshot(&self) -> Vec<String> {
        grid_lines(&self.term)
    }

    /// Snapshot the visible grid as cells with color + SGR attributes.
    #[must_use]
    pub fn cells(&self) -> Vec<Vec<TermCell>> {
        grid_cells(&self.term)
    }

    /// The cursor's position as `(column, row)` in the visible grid.
    #[must_use]
    pub fn cursor(&self) -> (usize, usize) {
        grid_cursor(&self.term)
    }

    /// Resize the grid to `cols` x `rows` (clamped to the usable floor).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = clamp_dims(cols, rows);
        self.term.resize(GridSize::new(cols, rows));
    }

    /// Scroll the view by `delta` lines (positive scrolls up into history).
    pub fn scroll_lines(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }
}

/// Snapshot a grid's visible lines as trimmed text (top to bottom). Shared by the
/// live [`Terminal`] and the headless [`Parser`] so both read the grid identically.
fn grid_lines(term: &Term<TitleListener>) -> Vec<String> {
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

/// Snapshot a grid's visible cells with per-cell color + SGR attributes (top to
/// bottom, left to right). Shared by [`Terminal`] and [`Parser`].
fn grid_cells(term: &Term<TitleListener>) -> Vec<Vec<TermCell>> {
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

/// The cursor position as `(column, row)` in the visible grid. Shared by
/// [`Terminal`] and [`Parser`].
fn grid_cursor(term: &Term<TitleListener>) -> (usize, usize) {
    let grid = term.grid();
    let offset = i32::try_from(grid.display_offset()).unwrap_or(0);
    let point = grid.cursor.point;
    (
        point.column.0,
        usize::try_from(point.line.0 + offset).unwrap_or(0),
    )
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

/// Read PTY bytes and advance the parser until EOF (shell exit) or a read error, then
/// reap the child and record its exit status.
///
/// Owning `child` here means the shell is `wait()`ed exactly once, when its output ends;
/// that reaps it (no zombie) and yields the exit code. A final wakeup after that lets the
/// UI repaint and show the exit overlay even though no more output followed.
fn read_loop<W: Fn()>(
    mut reader: Box<dyn Read + Send>,
    term: &Mutex<Term<TitleListener>>,
    dirty: &AtomicBool,
    exit: &Mutex<Option<ExitStatus>>,
    mut child: Box<dyn Child + Send + Sync>,
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
    // Output has ended: reap the shell and publish how it exited. `wait` returns promptly
    // since the child closed its PTY, and an errored wait still counts as a failed exit.
    let status = child.wait().map_or(
        ExitStatus {
            code: 1,
            signal: None,
        },
        |status| ExitStatus {
            code: status.exit_code(),
            signal: status.signal().map(str::to_owned),
        },
    );
    if let Ok(mut slot) = exit.lock() {
        *slot = Some(status);
    }
    dirty.store(true, Ordering::Relaxed);
    wakeup();
}

/// Map any displayable error into an `io::Error` so callers need not depend on the
/// PTY crate's error type.
fn to_io<E: std::fmt::Display>(err: E) -> std::io::Error {
    std::io::Error::other(err.to_string())
}
