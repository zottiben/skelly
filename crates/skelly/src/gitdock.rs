//! The git diff dock: a per-repo right dock over the live terminal (AGENTS Hard rule 4 -
//! a layer, the pane tree never unmounts; only one right-dock surface shows at a time).
//! Opened with `⇧⌘G` and dismissed with `Esc`. This module is pure state + view-building:
//! it holds the git-derived data ([`Status`] + the selected file's [`FileDiff`]) and turns
//! it into a monospace grid of UI-token-colored cells plus the add/del/hunk row metadata
//! the renderer needs for the `diff.*` backgrounds. The binary owns the [`Repo`] calls
//! (discover / status / diff), routing keys, and the viewport inset.
//!
//! v1 is read-only: a status bar (branch, ahead/behind, totals), the changed-file list
//! (status letter, path, `+add`/`-del` counts), and the selected file's unified diff.
//! Per-file / hunk staging, the commit box, split view, and the resizable dock width are
//! later M4 slices; the dock width is fixed at the guide's 420px default for now.
//!
//! [`Repo`]: skelly_session::Repo

use skelly_render::{GridCell, Srgb, Theme};
use skelly_session::{ChangedFile, FileDiff, FileStatus, LineKind, Status};

/// Grid row of the status bar (branch / ahead-behind / totals).
const STATUS_ROW: usize = 0;
/// Grid row of the `CHANGED - N` file-list section label.
const LABEL_ROW: usize = 2;
/// First grid row of the changed-file list.
const FILE_START: usize = 3;
/// Most file rows to show at once; a longer list scrolls to keep the selection visible.
const FILE_ROWS_MAX: usize = 12;
/// Rows the diff section always reserves below the file list (blank + header + a little
/// body), so the file list never crowds the diff out entirely.
const DIFF_RESERVE: usize = 4;
/// Cells reserved for the diff line-number gutter (right-aligned), then a space, the
/// `+`/`-`/` ` sign, a space, and the code text at [`DIFF_TEXT_COL`].
const GUTTER_COLS: usize = 4;
/// Column where a diff line's code text begins (`gutter(4) + space + sign + space`).
const DIFF_TEXT_COL: usize = GUTTER_COLS + 3;
/// Column of the per-file stage checkbox (`[x]`/`[ ]`) at the start of a file row.
const FILE_CHECK_COL: usize = 0;
/// Column of the status letter in a file row (after the checkbox).
const FILE_LETTER_COL: usize = 4;
/// Column where a file row's path begins.
const FILE_PATH_COL: usize = 6;
/// Rows the commit box occupies at the foot of the dock: a divider, the message input,
/// and a status line. Only shown when there is room and the tree has changes.
const COMMIT_ROWS: usize = 3;
/// Column where the commit message begins (after the `> ` prompt).
const COMMIT_TEXT_COL: usize = 2;

/// Which part of the dock has keyboard focus.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    /// The changed-file list (arrows / Space / `a` / PageUp-Down).
    List,
    /// The commit-message box (typing edits the message; Enter commits).
    Commit,
}

/// The git diff dock's state: the open flag, the loaded git data, and the selection /
/// scroll positions. The binary refreshes [`Self::status`]/[`Self::diff`] via the setters
/// whenever it opens the dock or moves the selection.
pub(crate) struct GitDock {
    /// Whether the dock is showing (captures navigation keys while open).
    pub(crate) open: bool,
    /// Whether a repository was found for the current context (else the empty state).
    repo_present: bool,
    /// The working status (branch, ahead/behind, changed files).
    status: Status,
    /// The selected file's unified diff (empty when none / untracked / binary).
    diff: FileDiff,
    /// Index of the selected file in `status.files`.
    selected: usize,
    /// Top line offset into the selected file's flattened diff (scrolled with PageUp/Dn).
    diff_scroll: usize,
    /// Which part of the dock has keyboard focus (the list or the commit box).
    focus: Focus,
    /// The commit message being typed in the commit box.
    message: String,
    /// The short SHA of the just-made commit, shown with an Undo hint until the next
    /// action clears it.
    last_commit: Option<String>,
    /// A git error to surface instead of the file list, if the last refresh failed.
    error: Option<String>,
}

impl GitDock {
    /// A closed, empty dock.
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            repo_present: false,
            status: Status::default(),
            diff: FileDiff::default(),
            selected: 0,
            diff_scroll: 0,
            focus: Focus::List,
            message: String::new(),
            last_commit: None,
            error: None,
        }
    }

    /// Open the dock on the file list (the binary loads the repo status right after).
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.focus = Focus::List;
        self.last_commit = None;
    }

    /// Close the dock.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// Whether the commit box currently has focus.
    pub(crate) fn commit_focused(&self) -> bool {
        self.focus == Focus::Commit
    }

    /// Move focus to the commit box (so typing edits the message).
    pub(crate) fn focus_commit(&mut self) {
        self.focus = Focus::Commit;
    }

    /// Move focus back to the file list.
    pub(crate) fn focus_list(&mut self) {
        self.focus = Focus::List;
    }

    /// Append a typed character to the commit message.
    pub(crate) fn push_char(&mut self, c: char) {
        self.message.push(c);
    }

    /// Delete the last character of the commit message.
    pub(crate) fn backspace(&mut self) {
        self.message.pop();
    }

    /// The commit message being edited.
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// How many changed files have something staged (the commit gate + status line).
    pub(crate) fn staged_count(&self) -> usize {
        self.status.files.iter().filter(|f| f.staged).count()
    }

    /// Whether a commit is allowed: at least one file staged and a non-blank message.
    pub(crate) fn can_commit(&self) -> bool {
        self.staged_count() > 0 && !self.message.trim().is_empty()
    }

    /// Record a successful commit: clear the message, return focus to the list, and keep
    /// the short SHA for the Undo hint.
    pub(crate) fn set_committed(&mut self, short_sha: String) {
        self.message.clear();
        self.focus = Focus::List;
        self.last_commit = Some(short_sha);
    }

    /// The short SHA of the just-made commit, if the Undo hint is still showing.
    pub(crate) fn last_commit(&self) -> Option<&str> {
        self.last_commit.as_deref()
    }

    /// Clear the just-committed Undo hint (after an undo, or any other change).
    pub(crate) fn clear_last_commit(&mut self) {
        self.last_commit = None;
    }

    /// Record that the current context is not inside a git repository (the empty state).
    pub(crate) fn set_no_repo(&mut self) {
        self.repo_present = false;
        self.status = Status::default();
        self.diff = FileDiff::default();
        self.selected = 0;
        self.diff_scroll = 0;
        self.focus = Focus::List;
        self.message.clear();
        self.last_commit = None;
        self.error = None;
    }

    /// Load a fresh repository status, clamping the selection and resetting the diff
    /// scroll. Call [`Self::selected_file`] afterwards to fetch the selected file's diff.
    pub(crate) fn load(&mut self, status: Status) {
        self.repo_present = true;
        self.error = None;
        self.selected = self.selected.min(status.files.len().saturating_sub(1));
        self.status = status;
        self.diff = FileDiff::default();
        self.diff_scroll = 0;
    }

    /// Set the selected file's diff (from `Repo::diff`), resetting the diff scroll.
    pub(crate) fn set_diff(&mut self, diff: FileDiff) {
        self.diff = diff;
        self.diff_scroll = 0;
    }

    /// Surface a git error in place of the file list.
    pub(crate) fn set_error(&mut self, message: String) {
        self.repo_present = true;
        self.error = Some(message);
    }

    /// The selected changed file, if any (the binary uses its path + staged flag to fetch
    /// the diff).
    pub(crate) fn selected_file(&self) -> Option<&ChangedFile> {
        self.status.files.get(self.selected)
    }

    /// Move the file selection by `delta`, clamped to the file list. Returns `true` when
    /// the selection actually moved (so the binary reloads the diff); resets the scroll.
    pub(crate) fn move_selection(&mut self, delta: i32) -> bool {
        let count = self.status.files.len();
        if count == 0 {
            return false;
        }
        let last = i32::try_from(count - 1).unwrap_or(0);
        let cur = i32::try_from(self.selected).unwrap_or(0);
        let next = usize::try_from((cur + delta).clamp(0, last)).unwrap_or(0);
        if next == self.selected {
            return false;
        }
        self.selected = next;
        self.diff_scroll = 0;
        true
    }

    /// Scroll the selected file's diff by `delta` lines (clamped in [`Self::view`]).
    pub(crate) fn scroll_diff(&mut self, delta: i32) {
        let next =
            i64::from(i32::try_from(self.diff_scroll).unwrap_or(i32::MAX)) + i64::from(delta);
        self.diff_scroll = usize::try_from(next.max(0)).unwrap_or(0);
    }

    /// Store the clamped diff scroll [`Self::view`] settled on, so paging past the end of a
    /// diff does not leave a large value that then needs many pages back to undo.
    pub(crate) fn set_scroll(&mut self, scroll: usize) {
        self.diff_scroll = scroll;
    }

    /// Build the dock grid `cols` cells wide and `rows` cells tall, in `theme`'s UI
    /// tokens. Returns the grid plus the row metadata the renderer needs (selected file,
    /// and which rows are additions / deletions / hunk headers) and the clamped diff
    /// scroll actually used (the binary writes it back so repeated paging settles).
    pub(crate) fn view(&self, cols: usize, rows: usize, theme: &Theme) -> View {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut grid: Vec<Vec<GridCell>> =
            (0..rows).map(|_| blank_row(cols, theme.fg_muted)).collect();

        // Empty / error states.
        if !self.repo_present {
            center(&mut grid, "No repository here", theme.fg_muted);
            return View::empty(grid);
        }
        if let Some(error) = &self.error {
            center(&mut grid, "git error", theme.diff_del);
            if rows > 1 {
                let msg: String = error
                    .lines()
                    .next()
                    .unwrap_or(error)
                    .chars()
                    .take(cols)
                    .collect();
                write_centered(
                    &mut grid[(rows / 2 + 1).min(rows - 1)],
                    cols,
                    &msg,
                    theme.fg_muted,
                );
            }
            return View::empty(grid);
        }

        self.write_status_bar(&mut grid[STATUS_ROW], cols, theme);

        if self.status.files.is_empty() {
            // After committing everything the tree is clean; keep the Undo hint visible.
            let message = match &self.last_commit {
                Some(sha) => format!("committed {sha} - press u to undo"),
                None => "Working tree clean".to_owned(),
            };
            if LABEL_ROW < rows {
                write_centered(
                    &mut grid[LABEL_ROW.max(rows / 2)],
                    cols,
                    &message,
                    theme.fg_muted,
                );
            }
            return View::empty(grid);
        }

        // The commit box occupies a fixed band at the foot (when there is room); the
        // file list + diff lay out above it, in `content_rows`.
        let commit_rows = if rows >= FILE_START + DIFF_RESERVE + COMMIT_ROWS {
            COMMIT_ROWS
        } else {
            0
        };
        let content_rows = rows.saturating_sub(commit_rows);

        // The file list (label + rows), then the diff, then the commit band.
        let (file_visible, selected_file_row) =
            self.write_file_list(&mut grid, content_rows, cols, theme);

        // The diff section: a blank, the selected file's header, then its scrolled body.
        let header_row = FILE_START + file_visible + 1;
        let mut view = View {
            rows: Vec::new(),
            selected_file_row,
            add_rows: Vec::new(),
            del_rows: Vec::new(),
            hunk_rows: Vec::new(),
            diff_scroll: 0,
            caret: None,
        };
        if let (Some(file), true) = (self.selected_file(), header_row < content_rows) {
            let path = file.path.to_string_lossy();
            let (added, removed) = self.diff.stats();
            write(&mut grid[header_row], 0, &path, theme.fg_secondary);
            write_counts(&mut grid[header_row], cols, added, removed, theme);
        }
        let body_start = header_row + 1;
        let body_rows = content_rows.saturating_sub(body_start);
        view.diff_scroll =
            self.write_diff_body(&mut grid, body_start, body_rows, cols, theme, &mut view);

        // The commit box at the foot.
        if commit_rows > 0 {
            view.caret = self.write_commit_band(&mut grid, content_rows, cols, theme);
        }

        view.rows = grid;
        view
    }

    /// Render the changed-file list (its `CHANGED - N` label + a key hint, then the file
    /// rows) into the first `content_rows` of `grid`, scrolled to keep the selection
    /// visible. Returns the number of file rows drawn and the selected file's grid row.
    fn write_file_list(
        &self,
        grid: &mut [Vec<GridCell>],
        content_rows: usize,
        cols: usize,
        theme: &Theme,
    ) -> (usize, Option<usize>) {
        write(
            &mut grid[LABEL_ROW],
            0,
            &format!("CHANGED - {}", self.status.files.len()),
            theme.fg_muted,
        );
        // A quiet key hint on the far right of the label row (staging is keyboard-driven).
        write_before(
            &mut grid[LABEL_ROW],
            cols,
            "space stage  a all",
            theme.fg_muted,
        );
        let avail = content_rows.saturating_sub(FILE_START);
        let file_visible = self
            .status
            .files
            .len()
            .min(FILE_ROWS_MAX)
            .min(avail.saturating_sub(DIFF_RESERVE))
            .max(1);
        let file_offset = scroll_window(self.status.files.len(), file_visible, self.selected);
        let mut selected_file_row = None;
        for visible in 0..file_visible {
            let index = file_offset + visible;
            let Some(file) = self.status.files.get(index) else {
                break;
            };
            let row = FILE_START + visible;
            if row >= content_rows {
                break;
            }
            if index == self.selected {
                selected_file_row = Some(row);
            }
            file_row(&mut grid[row], cols, file, index == self.selected, theme);
        }
        (file_visible, selected_file_row)
    }

    /// Render the commit box at the foot of the dock (a divider, the message input, and a
    /// status line), starting at grid row `top`. Returns the caret cell when the commit
    /// box has focus.
    fn write_commit_band(
        &self,
        grid: &mut [Vec<GridCell>],
        top: usize,
        cols: usize,
        theme: &Theme,
    ) -> Option<(usize, usize)> {
        let focused = self.focus == Focus::Commit;
        // Divider.
        if let Some(row) = grid.get_mut(top) {
            write(row, 0, &"-".repeat(cols), theme.border_strong);
        }

        // The message input line: a prompt + the message (tail-truncated to fit).
        let input_row = top + 1;
        let caret = grid.get_mut(input_row).map(|row| {
            let prompt_fg = if focused {
                theme.accent
            } else {
                theme.fg_muted
            };
            write(row, 0, "> ", prompt_fg);
            let max = cols.saturating_sub(COMMIT_TEXT_COL + 1);
            let count = self.message.chars().count();
            let shown: String = if count > max {
                self.message.chars().skip(count - max).collect()
            } else {
                self.message.clone()
            };
            if shown.is_empty() && !focused {
                write(row, COMMIT_TEXT_COL, "commit message", theme.fg_muted);
            } else {
                write(row, COMMIT_TEXT_COL, &shown, theme.fg_primary);
            }
            (COMMIT_TEXT_COL + shown.chars().count(), input_row)
        });

        // The status line: the just-committed Undo hint, else the staged count + a hint.
        if let Some(row) = grid.get_mut(top + 2) {
            let staged = self.staged_count();
            if let Some(sha) = &self.last_commit {
                write(row, 0, &format!("committed {sha}"), theme.diff_add);
                write_before(row, cols, "u undo", theme.fg_muted);
            } else {
                write(row, 0, &format!("{staged} staged"), theme.fg_muted);
                let hint = if focused {
                    if self.can_commit() {
                        "enter commit  esc back"
                    } else {
                        "esc back"
                    }
                } else {
                    "tab to write a message"
                };
                write_before(row, cols, hint, theme.fg_muted);
            }
        }

        focused.then_some(caret).flatten()
    }

    /// Write the status bar: branch (in `diff.hunk`), ahead/behind (muted), the added/
    /// removed totals (`diff.add`/`diff.del`), and an `esc` hint anchored right.
    fn write_status_bar(&self, row: &mut [GridCell], cols: usize, theme: &Theme) {
        let branch = self.status.branch.as_deref().unwrap_or("(detached)");
        write(row, 0, branch, theme.diff_hunk);
        let mut col = branch.chars().count() + 3;
        if self.status.ahead > 0 || self.status.behind > 0 {
            let text = format!("ahead {} behind {}", self.status.ahead, self.status.behind);
            write(row, col, &text, theme.fg_muted);
            col += text.chars().count();
        }
        let _ = col;
        let (added, removed): (u32, u32) = self
            .status
            .files
            .iter()
            .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        // Right-anchored, right to left: esc, then -removed, then +added.
        let mut end = cols;
        end = write_before(row, end, "esc", theme.fg_muted);
        end = write_before(row, end, &format!("-{removed}"), theme.diff_del);
        write_before(row, end, &format!("+{added}"), theme.diff_add);
    }

    /// Render the selected file's flattened diff into `grid[body_start..]`, recording the
    /// add/del/hunk grid rows into `view`. Returns the clamped scroll actually used. Shows
    /// a placeholder when there is nothing to diff (untracked / binary / unchanged).
    fn write_diff_body(
        &self,
        grid: &mut [Vec<GridCell>],
        body_start: usize,
        body_rows: usize,
        cols: usize,
        theme: &Theme,
        view: &mut View,
    ) -> usize {
        if body_rows == 0 {
            return 0;
        }
        let lines = self.flatten_diff();
        if lines.is_empty() {
            let note = match self.selected_file().map(|f| f.status) {
                Some(FileStatus::Untracked) => "Untracked file",
                _ => "No textual changes",
            };
            write(&mut grid[body_start], 0, note, theme.fg_muted);
            return 0;
        }
        let max_scroll = lines.len().saturating_sub(body_rows);
        let scroll = self.diff_scroll.min(max_scroll);
        for (visible, line) in lines.iter().skip(scroll).take(body_rows).enumerate() {
            let row = body_start + visible;
            match line.kind {
                DiffRowKind::Hunk => {
                    view.hunk_rows.push(row);
                    write(&mut grid[row], 0, &line.text, theme.diff_hunk);
                }
                DiffRowKind::Add => {
                    view.add_rows.push(row);
                    diff_line(&mut grid[row], cols, line, theme.diff_add, theme);
                }
                DiffRowKind::Del => {
                    view.del_rows.push(row);
                    diff_line(&mut grid[row], cols, line, theme.diff_del, theme);
                }
                DiffRowKind::Context => {
                    diff_line(&mut grid[row], cols, line, theme.fg_secondary, theme);
                }
            }
        }
        scroll
    }

    /// Flatten the selected file's [`FileDiff`] into display lines (a header line per
    /// hunk, then its context/add/del lines with the right gutter number and sign).
    fn flatten_diff(&self) -> Vec<DiffRow> {
        let mut lines = Vec::new();
        for hunk in &self.diff.hunks {
            let (old_count, new_count) = hunk_counts(hunk);
            let mut header = format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start, old_count, hunk.new_start, new_count
            );
            if !hunk.heading.is_empty() {
                header.push(' ');
                header.push_str(&hunk.heading);
            }
            lines.push(DiffRow {
                kind: DiffRowKind::Hunk,
                gutter: None,
                sign: ' ',
                text: header,
            });
            for line in &hunk.lines {
                let (kind, sign, gutter) = match line.kind {
                    LineKind::Context => (DiffRowKind::Context, ' ', line.new_no),
                    LineKind::Add => (DiffRowKind::Add, '+', line.new_no),
                    LineKind::Del => (DiffRowKind::Del, '-', line.old_no),
                };
                lines.push(DiffRow {
                    kind,
                    gutter,
                    sign,
                    text: line.text.clone(),
                });
            }
        }
        lines
    }
}

impl Default for GitDock {
    fn default() -> Self {
        Self::new()
    }
}

/// The rendered dock grid plus the renderer's row metadata.
pub(crate) struct View {
    /// The dock's lines as a grid of UI-colored cells.
    pub(crate) rows: Vec<Vec<GridCell>>,
    /// Grid row of the selected file (for the `accent.subtle` highlight quad).
    pub(crate) selected_file_row: Option<usize>,
    /// Grid rows that are diff additions (for the `diff.add` background quads).
    pub(crate) add_rows: Vec<usize>,
    /// Grid rows that are diff deletions (for the `diff.del` background quads).
    pub(crate) del_rows: Vec<usize>,
    /// Grid rows that are `@@` hunk headers (for the `diff.hunk` background quads).
    pub(crate) hunk_rows: Vec<usize>,
    /// The clamped diff scroll actually used (the binary writes it back).
    pub(crate) diff_scroll: usize,
    /// The commit-message caret `(column, row)`, when the commit box has focus.
    pub(crate) caret: Option<(usize, usize)>,
}

impl View {
    /// A view carrying just a grid (empty / error / clean states have no highlights).
    fn empty(rows: Vec<Vec<GridCell>>) -> Self {
        Self {
            rows,
            selected_file_row: None,
            add_rows: Vec::new(),
            del_rows: Vec::new(),
            hunk_rows: Vec::new(),
            diff_scroll: 0,
            caret: None,
        }
    }
}

/// One flattened diff display line.
struct DiffRow {
    kind: DiffRowKind,
    /// The gutter line number (`None` for a hunk header).
    gutter: Option<u32>,
    /// The leading `+`/`-`/` ` sign (unused for a hunk header).
    sign: char,
    /// The code text (or the whole `@@ ... @@` header line).
    text: String,
}

/// The role of a flattened diff line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffRowKind {
    Context,
    Add,
    Del,
    Hunk,
}

/// A hunk's `(old_count, new_count)` line spans, for reconstructing its `@@` header.
fn hunk_counts(hunk: &skelly_session::Hunk) -> (u32, u32) {
    let mut old = 0;
    let mut new = 0;
    for line in &hunk.lines {
        match line.kind {
            LineKind::Context => {
                old += 1;
                new += 1;
            }
            LineKind::Add => new += 1,
            LineKind::Del => old += 1,
        }
    }
    (old, new)
}

/// The scroll offset that keeps `anchor` visible in a `visible`-row window over `len`
/// items (centered on the anchor, clamped to the ends).
fn scroll_window(len: usize, visible: usize, anchor: usize) -> usize {
    if len <= visible {
        return 0;
    }
    anchor.saturating_sub(visible / 2).min(len - visible)
}

/// Write one file row: the stage checkbox, the status letter (colored by kind), the
/// path, and its `+add`/`-del` counts. The selected row's path is drawn in the primary
/// color; a staged file shows a ticked checkbox.
fn file_row(row: &mut [GridCell], cols: usize, file: &ChangedFile, selected: bool, theme: &Theme) {
    // `[x]` when anything is staged, `[ ]` otherwise (the tick in `diff.add`).
    write(row, FILE_CHECK_COL, "[", theme.fg_muted);
    write(
        row,
        FILE_CHECK_COL + 1,
        if file.staged { "x" } else { " " },
        theme.diff_add,
    );
    write(row, FILE_CHECK_COL + 2, "]", theme.fg_muted);

    let letter_fg = match file.status {
        FileStatus::Added | FileStatus::Untracked => theme.diff_add,
        FileStatus::Deleted => theme.diff_del,
        FileStatus::Modified | FileStatus::Renamed | FileStatus::Copied => theme.diff_hunk,
        FileStatus::TypeChange | FileStatus::Unmerged => theme.fg_secondary,
    };
    write(
        row,
        FILE_LETTER_COL,
        &file.status.code().to_string(),
        letter_fg,
    );

    let name_fg = if selected {
        theme.fg_primary
    } else {
        theme.fg_secondary
    };
    // The counts are right-anchored; clip the path so it never runs under them.
    let count_start = counts_start(cols, file.added, file.removed);
    let path = file.path.to_string_lossy();
    let max_path = count_start.saturating_sub(FILE_PATH_COL + 1);
    write_clipped(row, FILE_PATH_COL, &path, max_path, name_fg);
    write_counts(row, cols, file.added, file.removed, theme);
}

/// Write one diff body line: the right-aligned gutter number, the sign, and the code
/// text, in `fg` (context lines use a muted gutter).
fn diff_line(row: &mut [GridCell], cols: usize, line: &DiffRow, fg: Srgb, theme: &Theme) {
    if let Some(number) = line.gutter {
        let gutter_fg = if line.sign == ' ' { theme.fg_muted } else { fg };
        write(row, 0, &format!("{number:>GUTTER_COLS$}"), gutter_fg);
    }
    write(row, GUTTER_COLS + 1, &line.sign.to_string(), fg);
    let max_text = cols.saturating_sub(DIFF_TEXT_COL);
    write_clipped(row, DIFF_TEXT_COL, &line.text, max_text, fg);
}

/// The column where a right-anchored `+add -del` count pair begins.
fn counts_start(cols: usize, added: u32, removed: u32) -> usize {
    let del = format!("-{removed}");
    let add = format!("+{added}");
    // ` +add -del ` occupies: 1 margin + add + 1 gap + del.
    cols.saturating_sub(1 + add.chars().count() + 1 + del.chars().count())
}

/// Write a right-anchored `+add -del` count pair (add in `diff.add`, del in `diff.del`).
fn write_counts(row: &mut [GridCell], cols: usize, added: u32, removed: u32, theme: &Theme) {
    let del = format!("-{removed}");
    let del_start = cols.saturating_sub(1 + del.chars().count());
    write(row, del_start, &del, theme.diff_del);
    let add = format!("+{added}");
    let add_start = del_start.saturating_sub(1 + add.chars().count());
    write(row, add_start, &add, theme.diff_add);
}

/// One UI cell: a character in `fg`, no background or attributes.
fn cell(c: char, fg: Srgb) -> GridCell {
    GridCell {
        c,
        fg,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
    }
}

/// A blank row of `cols` spaces.
fn blank_row(cols: usize, fg: Srgb) -> Vec<GridCell> {
    vec![cell(' ', fg); cols]
}

/// Overwrite `text` into `row` starting at `col`, clipped to the row width.
fn write(row: &mut [GridCell], col: usize, text: &str, fg: Srgb) {
    for (i, ch) in text.chars().enumerate() {
        if let Some(slot) = row.get_mut(col + i) {
            *slot = cell(ch, fg);
        }
    }
}

/// Like [`write()`], but truncate `text` to at most `max` cells first.
fn write_clipped(row: &mut [GridCell], col: usize, text: &str, max: usize, fg: Srgb) {
    if max == 0 {
        return;
    }
    if text.chars().count() <= max {
        write(row, col, text, fg);
    } else {
        let clipped: String = text.chars().take(max).collect();
        write(row, col, &clipped, fg);
    }
}

/// Write `text` so its last cell sits just before `end`, returning the column one cell
/// before it starts (for chaining right-anchored segments right to left).
fn write_before(row: &mut [GridCell], end: usize, text: &str, fg: Srgb) -> usize {
    let len = text.chars().count();
    let start = end.saturating_sub(len + 1); // +1 leaves a one-cell margin/gap
    write(row, start, text, fg);
    start
}

/// Write `text` centered on a single `row` of `cols`.
fn write_centered(row: &mut [GridCell], cols: usize, text: &str, fg: Srgb) {
    let start = cols.saturating_sub(text.chars().count()) / 2;
    write(row, start, text, fg);
}

/// Write `text` centered on the middle row of `grid`.
fn center(grid: &mut [Vec<GridCell>], text: &str, fg: Srgb) {
    if grid.is_empty() {
        return;
    }
    let mid = grid.len() / 2;
    let cols = grid[mid].len();
    write_centered(&mut grid[mid], cols, text, fg);
}

#[cfg(test)]
mod tests {
    use super::GitDock;
    use skelly_render::Theme;
    use skelly_session::{ChangedFile, DiffLine, FileDiff, FileStatus, Hunk, LineKind, Status};

    fn changed(path: &str, status: FileStatus, added: u32, removed: u32) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            orig_path: None,
            status,
            staged: false,
            unstaged: true,
            added,
            removed,
        }
    }

    fn sample_status() -> Status {
        Status {
            branch: Some("main".to_owned()),
            ahead: 2,
            behind: 1,
            files: vec![
                changed("src/pane/tree.rs", FileStatus::Modified, 42, 11),
                changed("src/session/timeline.rs", FileStatus::Added, 80, 0),
                changed("old/legacy.rs", FileStatus::Deleted, 0, 34),
            ],
        }
    }

    fn sample_diff() -> FileDiff {
        FileDiff {
            hunks: vec![Hunk {
                old_start: 18,
                new_start: 18,
                heading: "impl PaneTree".to_owned(),
                lines: vec![
                    DiffLine {
                        kind: LineKind::Context,
                        old_no: Some(18),
                        new_no: Some(18),
                        text: "fn split(&mut self) {".to_owned(),
                    },
                    DiffLine {
                        kind: LineKind::Del,
                        old_no: Some(19),
                        new_no: None,
                        text: "    node.grow(dir);".to_owned(),
                    },
                    DiffLine {
                        kind: LineKind::Add,
                        old_no: None,
                        new_no: Some(19),
                        text: "    guard();".to_owned(),
                    },
                ],
            }],
        }
    }

    fn row_text(row: &[skelly_render::GridCell]) -> String {
        row.iter()
            .map(|c| c.c)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    /// A dock loaded with one file, staged iff `staged`.
    fn dock_with_one(staged: bool) -> GitDock {
        let mut file = changed("s.rs", FileStatus::Modified, 1, 0);
        file.staged = staged;
        file.unstaged = !staged;
        let mut dock = GitDock::new();
        dock.load(Status {
            branch: Some("main".to_owned()),
            files: vec![file],
            ..Status::default()
        });
        dock
    }

    #[test]
    fn commit_box_edits_the_message_and_gates_on_a_staged_file() {
        let mut dock = dock_with_one(true);
        assert!(!dock.can_commit(), "a staged file but no message yet");
        dock.focus_commit();
        assert!(dock.commit_focused());
        for c in "fix".chars() {
            dock.push_char(c);
        }
        assert_eq!(dock.message(), "fix");
        assert!(dock.can_commit(), "staged file + non-blank message");
        dock.backspace();
        assert_eq!(dock.message(), "fi");

        // A committed result clears the message, returns to the list, and keeps the SHA.
        dock.set_committed("abc1234".to_owned());
        assert_eq!(dock.message(), "");
        assert!(!dock.commit_focused());
        assert_eq!(dock.last_commit(), Some("abc1234"));
        dock.clear_last_commit();
        assert_eq!(dock.last_commit(), None);
    }

    #[test]
    fn commit_is_blocked_without_a_staged_file() {
        let mut dock = dock_with_one(false); // unstaged only
        dock.focus_commit();
        for c in "msg".chars() {
            dock.push_char(c);
        }
        assert!(!dock.can_commit(), "nothing is staged");
    }

    #[test]
    fn view_renders_the_commit_band_with_a_caret_when_focused() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = dock_with_one(true);
        dock.focus_commit();
        for c in "hi".chars() {
            dock.push_char(c);
        }
        let view = dock.view(46, 24, &theme);
        assert!(
            view.caret.is_some(),
            "the caret shows when the box has focus"
        );
        let joined = view
            .rows
            .iter()
            .map(|r| row_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("> hi"), "the message input line");
        assert!(joined.contains("1 staged"), "the staged-count status line");
    }

    #[test]
    fn no_repo_shows_the_empty_state() {
        let theme = Theme::resolve("ossein-dark");
        let dock = GitDock::new(); // repo_present is false until loaded
        let view = dock.view(46, 20, &theme);
        let joined = view
            .rows
            .iter()
            .map(|r| row_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("No repository here"));
        assert!(view.selected_file_row.is_none());
    }

    #[test]
    fn clean_tree_shows_the_branch_and_a_clean_note() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        dock.load(Status {
            branch: Some("main".to_owned()),
            ..Status::default()
        });
        let view = dock.view(46, 20, &theme);
        let joined = view
            .rows
            .iter()
            .map(|r| row_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("main"));
        assert!(joined.contains("Working tree clean"));
    }

    #[test]
    fn status_bar_shows_branch_ahead_behind_and_totals() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        dock.load(sample_status());
        let view = dock.view(60, 30, &theme);
        let status = row_text(&view.rows[0]);
        assert!(status.starts_with("main"));
        assert!(status.contains("ahead 2 behind 1"));
        // Totals: 42+80+0 added, 11+0+34 removed.
        assert!(status.contains("+122"));
        assert!(status.contains("-45"));
        assert!(status.contains("esc"));
    }

    #[test]
    fn file_list_marks_the_selected_row_and_lists_every_file() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        dock.load(sample_status());
        let view = dock.view(60, 30, &theme);
        assert_eq!(view.selected_file_row, Some(super::FILE_START));
        let joined = view
            .rows
            .iter()
            .map(|r| row_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("src/pane/tree.rs"));
        assert!(joined.contains("src/session/timeline.rs"));
        assert!(joined.contains("old/legacy.rs"));
        assert!(joined.contains("CHANGED - 3"));
    }

    #[test]
    fn file_rows_show_a_stage_checkbox_reflecting_the_staged_flag() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        let mut staged = changed("staged.rs", FileStatus::Modified, 1, 0);
        staged.staged = true;
        staged.unstaged = false;
        dock.load(Status {
            branch: Some("main".to_owned()),
            files: vec![staged, changed("unstaged.rs", FileStatus::Modified, 1, 0)],
            ..Status::default()
        });
        let view = dock.view(60, 30, &theme);
        // First file row is staged -> "[x]"; second is unstaged -> "[ ]".
        assert_eq!(&row_text(&view.rows[super::FILE_START])[..3], "[x]");
        assert_eq!(
            &view.rows[super::FILE_START + 1]
                .iter()
                .map(|c| c.c)
                .collect::<String>()[..3],
            "[ ]"
        );
    }

    #[test]
    fn diff_body_classifies_hunk_add_del_context_rows() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        dock.load(sample_status());
        dock.set_diff(sample_diff());
        let view = dock.view(60, 30, &theme);
        assert_eq!(view.hunk_rows.len(), 1, "one hunk header");
        assert_eq!(view.add_rows.len(), 1, "one addition");
        assert_eq!(view.del_rows.len(), 1, "one deletion");
        // The hunk header reconstructs its counts (1 context + 1 del = 2 old; 1+1 = 2 new).
        let hunk_row = view.hunk_rows[0];
        assert!(row_text(&view.rows[hunk_row]).contains("@@ -18,2 +18,2 @@ impl PaneTree"));
    }

    #[test]
    fn untracked_selection_shows_a_placeholder_not_an_empty_diff() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        dock.load(Status {
            branch: Some("main".to_owned()),
            files: vec![changed("new.txt", FileStatus::Untracked, 0, 0)],
            ..Status::default()
        });
        // No diff set (untracked files have none).
        let view = dock.view(46, 20, &theme);
        let joined = view
            .rows
            .iter()
            .map(|r| row_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Untracked file"));
    }

    #[test]
    fn move_selection_clamps_and_reports_change() {
        let mut dock = GitDock::new();
        dock.load(sample_status());
        assert!(!dock.move_selection(-1), "already at the top: no change");
        assert!(dock.move_selection(1));
        assert_eq!(
            dock.selected_file().unwrap().path.to_string_lossy(),
            "src/session/timeline.rs"
        );
        assert!(dock.move_selection(100)); // clamps to the last file
        assert_eq!(
            dock.selected_file().unwrap().path.to_string_lossy(),
            "old/legacy.rs"
        );
        assert!(!dock.move_selection(5), "already at the bottom: no change");
    }

    #[test]
    fn diff_scroll_clamps_to_the_content_height() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        dock.load(sample_status());
        dock.set_diff(sample_diff());
        dock.scroll_diff(1000); // far past the end
                                // With plenty of rows the whole (3-line) diff fits, so the used scroll clamps to 0.
        let view = dock.view(60, 40, &theme);
        assert_eq!(view.diff_scroll, 0);
    }

    #[test]
    fn long_file_list_keeps_the_selection_visible() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        let files: Vec<ChangedFile> = (0..40)
            .map(|i| changed(&format!("file{i}.rs"), FileStatus::Modified, 1, 0))
            .collect();
        dock.load(Status {
            branch: Some("main".to_owned()),
            files,
            ..Status::default()
        });
        for _ in 0..30 {
            dock.move_selection(1);
        }
        let view = dock.view(46, 24, &theme);
        // The selected file (file30.rs) must appear and be highlighted somewhere on screen.
        assert!(view.selected_file_row.is_some());
        let joined = view
            .rows
            .iter()
            .map(|r| row_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("file30.rs"),
            "selected file scrolled into view"
        );
    }
}
