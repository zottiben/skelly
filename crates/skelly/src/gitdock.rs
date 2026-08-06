//! The git diff dock: a per-repo right dock over the live terminal (AGENTS Hard rule 4 -
//! a layer, the pane tree never unmounts; only one right-dock surface shows at a time).
//! Opened with `⇧⌘G` and dismissed with `Esc`. This module is pure state + layout: it holds
//! the git-derived data ([`Status`] + the selected file's [`FileDiff`]) and builds a
//! *proportional* display list (decorative quads for the diff backgrounds + selection, and
//! positioned labels - chrome in IBM Plex Sans, diff code + metadata in the `mono` role
//! (`JetBrains` Mono) so columns stay aligned). The binary owns the [`Repo`] calls (discover
//! / diff), routing keys, and the viewport inset.
//!
//! v1 is read-only: a status bar (branch, ahead/behind, totals), the changed-file list
//! (status letter, path, `+add`/`-del` counts), and the selected file's unified diff.
//! Per-file / hunk staging, the commit box, split view, and the resizable dock width are
//! later M4 slices; the dock width is fixed at the guide's 420px default for now.
//!
//! [`Repo`]: skelly_session::Repo

use std::path::PathBuf;

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};
use skelly_session::{ChangedFile, FileDiff, FileStatus, Hunk, LineKind, Status};

/// Git-dock layout constants in **logical** px (multiplied by the DPI scale). Tuned to the
/// guide's §10.6: a status bar, a `CHANGED - N` file list at `bg.inset`, the selected file's
/// diff (code in `mono`), and a commit box at the foot.
const PAD_X: f32 = 12.0;
/// Top padding above the status bar.
const PAD_TOP: f32 = 12.0;
/// Status-bar row height.
const STATUS_H: f32 = 26.0;
/// Section-label (`CHANGED - N`) row height.
const LABEL_H: f32 = 24.0;
/// Changed-file row height.
const FILE_ROW_H: f32 = 26.0;
/// Diff header (file path + counts) row height.
const DIFF_HEADER_H: f32 = 26.0;
/// Diff body line height (compact, code).
const DIFF_ROW_H: f32 = 20.0;
/// Each commit-band row's height (message input, status line).
const COMMIT_ROW_H: f32 = 24.0;
/// The commit band's total height at the foot (a divider + input + status + padding).
const COMMIT_BAND_H: f32 = 78.0;
/// Most file rows to show at once; a longer list scrolls to keep the selection visible.
const FILE_ROWS_MAX: usize = 10;
/// Gutter width for the diff line-number, in mono chars.
const GUTTER_CHARS: usize = 4;
/// Alpha for an add/del diff-line background (the guide's `diff.*.bg` tokens).
const DIFF_BG_ALPHA: f32 = 0.14;
/// Alpha for a hunk-header background (the lighter `diff.hunk.bg`).
const HUNK_BG_ALPHA: f32 = 0.08;

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
    /// Whether `diff` is the staged (index-vs-HEAD) diff rather than the working-tree
    /// one; determines whether `⌘↵` stages or unstages the focused hunk.
    diff_is_staged: bool,
    /// The path `diff` was read for, so a live refresh can tell a reload of the file on
    /// screen (keep the scroll) from a move to another file (reset it).
    diff_path: Option<PathBuf>,
    /// Index of the focused hunk in `diff.hunks` (the target of `⌘↵`).
    focused_hunk: usize,
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
            diff_is_staged: false,
            diff_path: None,
            focused_hunk: 0,
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

    /// Whether the current context is not inside a git repository (the empty state, where
    /// `Enter` runs `Init repo`). True only after a refresh found no repo.
    pub(crate) fn no_repo(&self) -> bool {
        !self.repo_present
    }

    /// Record that the current context is not inside a git repository (the empty state).
    pub(crate) fn set_no_repo(&mut self) {
        self.repo_present = false;
        self.status = Status::default();
        self.diff = FileDiff::default();
        self.diff_path = None;
        self.selected = 0;
        self.diff_scroll = 0;
        self.focus = Focus::List;
        self.message.clear();
        self.last_commit = None;
        self.error = None;
    }

    /// Whether `status` is what the dock is already showing - the guard that keeps a live
    /// refresh (design §10.4) free when the working tree has not moved.
    pub(crate) fn matches(&self, status: &Status) -> bool {
        self.repo_present && self.error.is_none() && self.status == *status
    }

    /// Load a fresh repository status, keeping the user's place: the selection follows the
    /// selected **path** rather than its index, so a refresh that reorders or adds files
    /// (the dock re-reads the working tree while it is open) never jumps the cursor to
    /// another file. Call [`Self::selected_file`] afterwards to fetch the selected diff.
    pub(crate) fn load(&mut self, status: Status) {
        self.repo_present = true;
        self.error = None;
        let previous = self.selected_file().map(|f| f.path.clone());
        self.selected = previous
            .and_then(|path| status.files.iter().position(|f| f.path == path))
            .unwrap_or_else(|| self.selected.min(status.files.len().saturating_sub(1)));
        self.status = status;
    }

    /// Set the selected file's diff (from `Repo::diff`), recording whether it is the
    /// staged side. Scroll and hunk focus reset when this is a different file (or a
    /// different side of the same one) and are kept when it is a refresh of the one on
    /// screen, so a live reload does not scroll the diff out from under the reader.
    pub(crate) fn set_diff(&mut self, diff: FileDiff, staged: bool) {
        let path = self.selected_file().map(|f| f.path.clone());
        if path != self.diff_path || staged != self.diff_is_staged {
            self.focused_hunk = 0;
            self.diff_scroll = 0;
        }
        self.diff_path = path;
        self.diff = diff;
        self.diff_is_staged = staged;
        self.focused_hunk = self
            .focused_hunk
            .min(self.diff.hunks.len().saturating_sub(1));
    }

    /// Whether the shown diff is the staged side (so `⌘↵` unstages rather than stages).
    pub(crate) fn diff_is_staged(&self) -> bool {
        self.diff_is_staged
    }

    /// The focused hunk of the shown diff, if any (the target of `⌘↵`).
    pub(crate) fn focused_hunk(&self) -> Option<&Hunk> {
        self.diff.hunks.get(self.focused_hunk)
    }

    /// Move the focused hunk by `delta`, clamped, and scroll the diff so its header is at
    /// the top of the body. A no-op when the diff has no hunks.
    pub(crate) fn focus_hunk(&mut self, delta: i32) {
        let count = self.diff.hunks.len();
        if count == 0 {
            return;
        }
        let last = i32::try_from(count - 1).unwrap_or(0);
        let cur = i32::try_from(self.focused_hunk).unwrap_or(0);
        self.focused_hunk = usize::try_from((cur + delta).clamp(0, last)).unwrap_or(0);
        self.diff_scroll = self.hunk_line_offset(self.focused_hunk);
    }

    /// The flattened-diff display-line index of hunk `index`'s header (each earlier hunk
    /// contributes its header line plus its body lines).
    fn hunk_line_offset(&self, index: usize) -> usize {
        self.diff
            .hunks
            .iter()
            .take(index)
            .map(|h| 1 + h.lines.len())
            .sum()
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

    /// Scroll the selected file's diff by `delta` lines (clamped in [`Self::build`]).
    pub(crate) fn scroll_diff(&mut self, delta: i32) {
        let next =
            i64::from(i32::try_from(self.diff_scroll).unwrap_or(i32::MAX)) + i64::from(delta);
        self.diff_scroll = usize::try_from(next.max(0)).unwrap_or(0);
    }

    /// Store the clamped diff scroll [`Self::build`] settled on, so paging past the end of a
    /// diff does not leave a large value that then needs many pages back to undo.
    pub(crate) fn set_scroll(&mut self, scroll: usize) {
        self.diff_scroll = scroll;
    }

    /// Build the dock's proportional display list within `panel` (physical px) at DPI
    /// `scale`, in `theme`'s UI tokens: the status bar, the `CHANGED - N` file list (the
    /// selected file filled), the selected file's diff (add/del/hunk backgrounds + `mono`
    /// code), and the commit box. The renderer draws the dock frame (shadow + divider).
    /// Returns the clamped diff scroll actually used (the binary writes it back).
    pub(crate) fn build(
        &self,
        panel: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> Paint {
        let ctx = GCtx {
            panel,
            cx: panel.x + PAD_X * scale,
            cr: panel.x + panel.w - PAD_X * scale,
            scale,
            theme,
        };
        let mid = panel.y + panel.h * 0.5;
        // Empty / error states short-circuit with their own centered message.
        if let Some(paint) = self.empty_paint(&ctx, measure, mid) {
            return paint;
        }
        let mut quads = Vec::new();
        let mut labels = Vec::new();
        self.push_status_bar(&mut labels, &ctx, measure, panel.y + PAD_TOP * scale);

        // The commit box occupies a fixed band at the foot; the file list + diff lay out above.
        let content_bottom = panel.y + panel.h - COMMIT_BAND_H * scale;
        let mut y = panel.y + PAD_TOP * scale + STATUS_H * scale;
        // File-list section label + a key hint.
        push_row(
            &mut labels,
            measure,
            &format!("CHANGED - {}", self.status.files.len()),
            FontRole::Micro,
            theme.fg_muted,
            ctx.cx,
            y,
            LABEL_H,
            scale,
        );
        push_right(
            &mut labels,
            measure,
            "space stage  a all",
            FontRole::Caption,
            theme.fg_muted,
            ctx.cr,
            y,
            LABEL_H,
            scale,
        );
        y += LABEL_H * scale;

        let files_bottom =
            self.push_file_list(&mut quads, &mut labels, &ctx, measure, y, content_bottom);
        let diff_scroll = self.push_diff(
            &mut quads,
            &mut labels,
            &ctx,
            measure,
            files_bottom,
            content_bottom,
        );
        self.push_commit(&mut quads, &mut labels, &ctx, measure, content_bottom);
        Paint {
            quads,
            labels,
            diff_scroll,
        }
    }

    /// The empty / error states, each a centered message: no repo (with an Init button), a
    /// git error, or a clean tree (with the status bar). `Some` short-circuits [`Self::build`].
    fn empty_paint(&self, ctx: &GCtx, measure: &mut TextMeasure, mid: f32) -> Option<Paint> {
        let (panel, scale, theme) = (ctx.panel, ctx.scale, ctx.theme);
        if !self.repo_present {
            let mut quads = Vec::new();
            let mut labels = Vec::new();
            push_centered(
                &mut labels,
                measure,
                "No repository here",
                FontRole::Body,
                theme.fg_muted,
                panel,
                mid,
            );
            // The "Init repo" button (design §12 "Not a git repo"): an accent affordance.
            let by = mid + 34.0 * scale;
            let w = measure.width("Init repo  \u{21a9}", FontRole::Body, None);
            let bx = panel.x + (panel.w - w) * 0.5;
            quads.push(ChromeQuad::rounded(
                PxRect {
                    x: bx - 10.0 * scale,
                    y: by,
                    w: w + 20.0 * scale,
                    h: 28.0 * scale,
                },
                theme.accent_subtle_on(theme.bg_base.to_srgb()),
                6.0 * scale,
            ));
            push_row(
                &mut labels,
                measure,
                "Init repo  \u{21a9}",
                FontRole::Body,
                theme.accent,
                bx,
                by,
                28.0,
                scale,
            );
            return Some(Paint {
                quads,
                labels,
                diff_scroll: 0,
            });
        }
        if let Some(error) = &self.error {
            let mut labels = Vec::new();
            push_centered(
                &mut labels,
                measure,
                "git error",
                FontRole::Body,
                theme.diff_del,
                panel,
                mid - 14.0 * scale,
            );
            let msg: String = error
                .lines()
                .next()
                .unwrap_or(error)
                .chars()
                .take(60)
                .collect();
            push_centered(
                &mut labels,
                measure,
                &msg,
                FontRole::Caption,
                theme.fg_muted,
                panel,
                mid + 14.0 * scale,
            );
            return Some(Paint {
                quads: Vec::new(),
                labels,
                diff_scroll: 0,
            });
        }
        if self.status.files.is_empty() {
            let mut labels = Vec::new();
            self.push_status_bar(&mut labels, ctx, measure, panel.y + PAD_TOP * scale);
            let message = match &self.last_commit {
                Some(sha) => format!("committed {sha} - press u to undo"),
                None => "Working tree clean".to_owned(),
            };
            push_centered(
                &mut labels,
                measure,
                &message,
                FontRole::Body,
                theme.fg_muted,
                panel,
                mid,
            );
            return Some(Paint {
                quads: Vec::new(),
                labels,
                diff_scroll: 0,
            });
        }
        None
    }

    /// The status bar: branch (`diff.hunk`), ahead/behind (muted), the `+added`/`-removed`
    /// totals right-anchored, and an `esc` hint.
    fn push_status_bar(
        &self,
        labels: &mut Vec<ProseLabel>,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        top: f32,
    ) {
        let theme = ctx.theme;
        let branch = self.status.branch.as_deref().unwrap_or("(detached)");
        let mut x = ctx.cx;
        push_row(
            labels,
            measure,
            &format!("\u{2442} {branch}"),
            FontRole::Mono,
            theme.diff_hunk,
            x,
            top,
            STATUS_H,
            ctx.scale,
        );
        x += measure.width(&format!("\u{2442} {branch}"), FontRole::Mono, None) + 10.0 * ctx.scale;
        if self.status.ahead > 0 || self.status.behind > 0 {
            let text = format!(
                "\u{2191}{} \u{2193}{}",
                self.status.ahead, self.status.behind
            );
            push_row(
                labels,
                measure,
                &text,
                FontRole::Mono,
                theme.fg_muted,
                x,
                top,
                STATUS_H,
                ctx.scale,
            );
        }
        let (added, removed): (u32, u32) = self
            .status
            .files
            .iter()
            .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        // Right-anchored right-to-left: esc, then -removed, then +added.
        let mut end = ctx.cr;
        end = push_right(
            labels,
            measure,
            "esc",
            FontRole::Caption,
            theme.fg_muted,
            end,
            top,
            STATUS_H,
            ctx.scale,
        ) - 10.0 * ctx.scale;
        end = push_right(
            labels,
            measure,
            &format!("-{removed}"),
            FontRole::Mono,
            theme.diff_del,
            end,
            top,
            STATUS_H,
            ctx.scale,
        ) - 8.0 * ctx.scale;
        push_right(
            labels,
            measure,
            &format!("+{added}"),
            FontRole::Mono,
            theme.diff_add,
            end,
            top,
            STATUS_H,
            ctx.scale,
        );
    }

    /// The changed-file list (windowed to keep the selection visible): each row's stage
    /// checkbox, status letter, path (Plex, clipped before the counts), and `+add`/`-del`
    /// counts, the selected row behind an `accent.subtle` fill. Returns the y below the list.
    fn push_file_list(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        top: f32,
        bottom: f32,
    ) -> f32 {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let row_h = FILE_ROW_H * scale;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "small non-negative count"
        )]
        let capacity =
            (((bottom - top) * 0.45 / row_h).floor().max(1.0) as usize).min(FILE_ROWS_MAX);
        let visible = self.status.files.len().min(capacity);
        let offset = scroll_window(self.status.files.len(), visible, self.selected);
        let mut y = top;
        for slot in 0..visible {
            let index = offset + slot;
            let Some(file) = self.status.files.get(index) else {
                break;
            };
            let selected = index == self.selected;
            if selected {
                // accent.subtle selected-row band, sRGB-composited over the dock's bg.base
                // backing so it reads at the guide's weight (not the brighter linear blend).
                quads.push(ChromeQuad::fill(
                    PxRect {
                        x: ctx.panel.x,
                        y,
                        w: ctx.panel.w,
                        h: row_h,
                    },
                    theme.accent_subtle_on(theme.bg_base.to_srgb()),
                ));
            }
            push_file_row(labels, measure, ctx, file, selected, y);
            y += row_h;
        }
        y
    }

    /// The selected file's diff: a header (path + counts), then the flattened body scrolled
    /// into the space above the commit band, with add/del/hunk backgrounds, the focused-hunk
    /// fill + stage hint, and `mono` code. Returns the clamped scroll used.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the visible-row count + slot index are small, non-negative values"
    )]
    fn push_diff(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        top: f32,
        bottom: f32,
    ) -> usize {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let mut y = top + 6.0 * scale;
        if let Some(file) = self.selected_file() {
            let (added, removed) = self.diff.stats();
            push_row(
                labels,
                measure,
                &file.path.to_string_lossy(),
                FontRole::Label,
                theme.fg_secondary,
                ctx.cx,
                y,
                DIFF_HEADER_H,
                scale,
            );
            push_counts(labels, measure, ctx, added, removed, y, DIFF_HEADER_H);
        }
        y += DIFF_HEADER_H * scale;
        let body_top = y;
        let row_h = DIFF_ROW_H * scale;
        let body_rows = ((bottom - body_top) / row_h).floor().max(0.0) as usize;
        if body_rows == 0 {
            return 0;
        }
        let lines = self.flatten_diff();
        if lines.is_empty() {
            let note = match self.selected_file().map(|f| f.status) {
                Some(FileStatus::Untracked) => "Untracked file",
                _ => "No textual changes",
            };
            push_row(
                labels,
                measure,
                note,
                FontRole::Caption,
                theme.fg_muted,
                ctx.cx,
                body_top,
                DIFF_ROW_H,
                scale,
            );
            return 0;
        }
        let scroll = self.diff_scroll.min(lines.len().saturating_sub(body_rows));
        let gutter_w = measure.width(&"0".repeat(GUTTER_CHARS), FontRole::Mono, None);
        for (i, line) in lines.iter().skip(scroll).take(body_rows).enumerate() {
            let ry = body_top + i as f32 * row_h;
            push_diff_line(
                quads,
                labels,
                measure,
                ctx,
                line,
                ry,
                row_h,
                gutter_w,
                self.diff_is_staged,
                line.hunk_index == Some(self.focused_hunk),
            );
        }
        scroll
    }

    /// The commit box at the foot: a divider, the `> message` input (with an accent caret when
    /// focused), and a status line (the just-committed Undo hint, or the staged count + hint).
    fn push_commit(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        top: f32,
    ) {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let focused = self.focus == Focus::Commit;
        quads.push(ChromeQuad::fill(
            PxRect {
                x: ctx.panel.x,
                y: top,
                w: ctx.panel.w,
                h: scale.max(1.0),
            },
            theme.border,
        ));
        // Message input.
        let iy = top + 10.0 * scale;
        let prompt_fg = if focused {
            theme.accent
        } else {
            theme.fg_muted
        };
        push_row(
            labels,
            measure,
            "\u{203a}",
            FontRole::Mono,
            prompt_fg,
            ctx.cx,
            iy,
            COMMIT_ROW_H,
            scale,
        );
        let msg_x = ctx.cx + measure.width("\u{203a} ", FontRole::Mono, None);
        if self.message.is_empty() && !focused {
            push_row(
                labels,
                measure,
                "commit message",
                FontRole::Body,
                theme.fg_muted,
                msg_x,
                iy,
                COMMIT_ROW_H,
                scale,
            );
        } else {
            push_row(
                labels,
                measure,
                &self.message,
                FontRole::Body,
                theme.fg_primary,
                msg_x,
                iy,
                COMMIT_ROW_H,
                scale,
            );
        }
        if focused {
            let caret_x = msg_x + measure.width(&self.message, FontRole::Body, None);
            let line_h = measure.line_height(FontRole::Body);
            quads.push(ChromeQuad::fill(
                PxRect {
                    x: caret_x,
                    y: iy + (COMMIT_ROW_H * scale - line_h) * 0.5,
                    w: (2.0 * scale).max(1.0),
                    h: line_h,
                },
                theme.accent,
            ));
        }
        self.push_commit_status(labels, ctx, measure, iy + COMMIT_ROW_H * scale, focused);
    }

    /// The commit box's status line: the just-committed Undo hint, or the staged count + a
    /// context hint.
    fn push_commit_status(
        &self,
        labels: &mut Vec<ProseLabel>,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        sy: f32,
        focused: bool,
    ) {
        let (scale, theme) = (ctx.scale, ctx.theme);
        if let Some(sha) = &self.last_commit {
            push_row(
                labels,
                measure,
                &format!("committed {sha}"),
                FontRole::Mono,
                theme.diff_add,
                ctx.cx,
                sy,
                COMMIT_ROW_H,
                scale,
            );
            push_right(
                labels,
                measure,
                "u undo",
                FontRole::Caption,
                theme.fg_muted,
                ctx.cr,
                sy,
                COMMIT_ROW_H,
                scale,
            );
        } else {
            push_row(
                labels,
                measure,
                &format!("{} staged", self.staged_count()),
                FontRole::Caption,
                theme.fg_muted,
                ctx.cx,
                sy,
                COMMIT_ROW_H,
                scale,
            );
            let hint = if focused {
                if self.can_commit() {
                    "enter commit  esc back"
                } else {
                    "esc back"
                }
            } else {
                "tab to write a message"
            };
            push_right(
                labels,
                measure,
                hint,
                FontRole::Caption,
                theme.fg_muted,
                ctx.cr,
                sy,
                COMMIT_ROW_H,
                scale,
            );
        }
    }

    /// Flatten the selected file's [`FileDiff`] into display lines (a header line per
    /// hunk, then its context/add/del lines with the right gutter number and sign).
    fn flatten_diff(&self) -> Vec<DiffRow> {
        let mut lines = Vec::new();
        for (index, hunk) in self.diff.hunks.iter().enumerate() {
            let (old_count, new_count) = hunk.counts();
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
                hunk_index: Some(index),
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
                    hunk_index: None,
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

/// The dock's proportional display list: the content quads (diff backgrounds, selection /
/// hunk fills, commit caret) + the positioned labels, and the clamped diff scroll used.
pub(crate) struct Paint {
    /// The content quads over the dock frame.
    pub(crate) quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub(crate) labels: Vec<ProseLabel>,
    /// The clamped diff scroll actually used (the binary writes it back).
    pub(crate) diff_scroll: usize,
}

/// Shared geometry for the dock builders: the panel, the content left/right x, scale, theme.
struct GCtx<'a> {
    panel: PxRect,
    cx: f32,
    cr: f32,
    scale: f32,
    theme: &'a Theme,
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
    /// The hunk this row belongs to, for a hunk header (`None` for a body line).
    hunk_index: Option<usize>,
}

/// The role of a flattened diff line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffRowKind {
    Context,
    Add,
    Del,
    Hunk,
}

/// The scroll offset that keeps `anchor` visible in a `visible`-row window over `len`
/// items (centered on the anchor, clamped to the ends).
fn scroll_window(len: usize, visible: usize, anchor: usize) -> usize {
    if len <= visible {
        return 0;
    }
    anchor.saturating_sub(visible / 2).min(len - visible)
}

/// Push one file row: the stage checkbox (mono), status letter (mono, colored by kind), the
/// path (Plex, clipped before the counts), and its `+add`/`-del` counts (mono).
fn push_file_row(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    ctx: &GCtx,
    file: &ChangedFile,
    selected: bool,
    top: f32,
) {
    let (scale, theme) = (ctx.scale, ctx.theme);
    let mut x = ctx.cx;
    let check = if file.staged { "[x]" } else { "[ ]" };
    let check_fg = if file.staged {
        theme.diff_add
    } else {
        theme.fg_muted
    };
    push_row(
        labels,
        measure,
        check,
        FontRole::Mono,
        check_fg,
        x,
        top,
        FILE_ROW_H,
        scale,
    );
    x += measure.width("[x] ", FontRole::Mono, None);
    let letter_fg = match file.status {
        FileStatus::Added | FileStatus::Untracked => theme.diff_add,
        FileStatus::Deleted => theme.diff_del,
        FileStatus::Modified | FileStatus::Renamed | FileStatus::Copied => theme.diff_hunk,
        FileStatus::TypeChange | FileStatus::Unmerged => theme.fg_secondary,
    };
    push_row(
        labels,
        measure,
        &file.status.code().to_string(),
        FontRole::Mono,
        letter_fg,
        x,
        top,
        FILE_ROW_H,
        scale,
    );
    x += measure.width("M  ", FontRole::Mono, None);
    // Counts right-anchored; the path clips before them.
    let counts_left = push_counts(
        labels,
        measure,
        ctx,
        file.added,
        file.removed,
        top,
        FILE_ROW_H,
    );
    let name_fg = if selected {
        theme.fg_primary
    } else {
        theme.fg_secondary
    };
    labels.push(ProseLabel {
        text: file.path.to_string_lossy().into_owned(),
        x,
        y: top + (FILE_ROW_H * scale - measure.line_height(FontRole::Body)) * 0.5,
        role: FontRole::Body,
        color: name_fg,
        weight: None,
        max_w: (counts_left - 8.0 * scale - x).max(1.0),
    });
}

/// Push one diff body line: the add/del/hunk background (+ focused-hunk fill), then the
/// content - a hunk header (mono, `diff.hunk`, with a stage/unstage hint when focused) or a
/// body line (right-aligned mono gutter + mono code, colored by kind).
#[allow(clippy::too_many_arguments, reason = "one focused diff-line builder")]
fn push_diff_line(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    ctx: &GCtx,
    line: &DiffRow,
    top: f32,
    row_h: f32,
    gutter_w: f32,
    staged: bool,
    focused_hunk: bool,
) {
    let (scale, theme, panel) = (ctx.scale, ctx.theme, ctx.panel);
    let full = PxRect {
        x: panel.x,
        y: top,
        w: panel.w,
        h: row_h,
    };
    // Diff-row backgrounds pre-composite their translucent hue over the dock's bg.base backing
    // in sRGB space (the guide's CSS weight), not the brighter linear-space GPU blend.
    let base = theme.bg_base.to_srgb();
    if line.kind == DiffRowKind::Hunk {
        // The `@@` hunk header: a diff.hunk wash, deepened to an accent wash when it is focused.
        let hunk_bg = theme.diff_hunk.over(base, HUNK_BG_ALPHA);
        let row_bg = if focused_hunk {
            theme.accent.over(hunk_bg, DIFF_BG_ALPHA)
        } else {
            hunk_bg
        };
        quads.push(ChromeQuad::fill(full, row_bg));
        let role_h = DIFF_ROW_H;
        push_row(
            labels,
            measure,
            &line.text,
            FontRole::Mono,
            theme.diff_hunk,
            ctx.cx,
            top,
            role_h,
            scale,
        );
        if focused_hunk {
            let hint = if staged {
                "unstage \u{2318}\u{21a9}"
            } else {
                "stage \u{2318}\u{21a9}"
            };
            push_right(
                labels,
                measure,
                hint,
                FontRole::Micro,
                theme.accent,
                ctx.cr,
                top,
                role_h,
                scale,
            );
        }
    } else {
        let fg = match line.kind {
            DiffRowKind::Add => theme.diff_add,
            DiffRowKind::Del => theme.diff_del,
            _ => theme.fg_secondary,
        };
        let alpha_token = match line.kind {
            DiffRowKind::Add => Some(theme.diff_add),
            DiffRowKind::Del => Some(theme.diff_del),
            _ => None,
        };
        if let Some(bg) = alpha_token {
            quads.push(ChromeQuad::fill(full, bg.over(base, DIFF_BG_ALPHA)));
        }
        if let Some(number) = line.gutter {
            let gutter_fg = if line.sign == ' ' { theme.fg_muted } else { fg };
            push_right(
                labels,
                measure,
                &number.to_string(),
                FontRole::Mono,
                gutter_fg,
                ctx.cx + gutter_w,
                top,
                DIFF_ROW_H,
                scale,
            );
        }
        let code_x = ctx.cx + gutter_w + measure.width("  ", FontRole::Mono, None);
        let code = format!("{} {}", line.sign, line.text);
        labels.push(ProseLabel {
            text: code,
            x: code_x,
            y: top + (row_h - measure.line_height(FontRole::Mono)) * 0.5,
            role: FontRole::Mono,
            color: fg,
            weight: None,
            max_w: (ctx.cr - code_x).max(1.0),
        });
    }
}

/// Push a right-anchored `+add -del` count pair (add in `diff.add`, del in `diff.del`);
/// returns the left edge of the pair (so a path can clip before it).
fn push_counts(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    ctx: &GCtx,
    added: u32,
    removed: u32,
    top: f32,
    row_h: f32,
) -> f32 {
    let mut end = push_right(
        labels,
        measure,
        &format!("-{removed}"),
        FontRole::Mono,
        ctx.theme.diff_del,
        ctx.cr,
        top,
        row_h,
        ctx.scale,
    );
    end = push_right(
        labels,
        measure,
        &format!("+{added}"),
        FontRole::Mono,
        ctx.theme.diff_add,
        end - 8.0 * ctx.scale,
        top,
        row_h,
        ctx.scale,
    );
    end
}

/// Push one left-anchored label vertically centered in a row of `row_h` logical px.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_row(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    x: f32,
    top: f32,
    row_h: f32,
    scale: f32,
) {
    let line_h = measure.line_height(role);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x,
        y: top + (row_h * scale - line_h) * 0.5,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

/// Push a right-anchored label ending at `right`, vertically centered; returns its left edge.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_right(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    right: f32,
    top: f32,
    row_h: f32,
    scale: f32,
) -> f32 {
    let x = right - measure.width(text, role, None);
    push_row(labels, measure, text, role, color, x, top, row_h, scale);
    x
}

/// Push a label horizontally centered in `panel` at physical `top`.
fn push_centered(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    panel: PxRect,
    top: f32,
) {
    let w = measure.width(text, role, None);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x: panel.x + (panel.w - w) * 0.5,
        y: top,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

#[cfg(test)]
mod tests {
    use super::{GitDock, Paint};
    use skelly_render::{PxRect, TextMeasure, Theme};
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

    fn long_diff() -> FileDiff {
        FileDiff {
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                heading: "large change".to_owned(),
                lines: (0..100)
                    .map(|i| DiffLine {
                        kind: LineKind::Context,
                        old_no: Some(i + 1),
                        new_no: Some(i + 1),
                        text: format!("large diff line {i}"),
                    })
                    .collect(),
            }],
        }
    }

    /// A tall dock panel (the 420px dock) at 2x DPI.
    fn panel() -> PxRect {
        PxRect {
            x: 0.0,
            y: 0.0,
            w: 420.0 * 2.0,
            h: 900.0 * 2.0,
        }
    }

    /// Build the dock's paint at a representative panel.
    fn built(dock: &GitDock) -> Paint {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        dock.build(panel(), 2.0, &theme, &mut m)
    }

    /// The joined text of every label the dock builds (for content assertions).
    fn texts(paint: &Paint) -> String {
        paint
            .labels
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
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

        dock.set_committed("abc1234".to_owned());
        assert_eq!(dock.message(), "");
        assert!(!dock.commit_focused());
        assert_eq!(dock.last_commit(), Some("abc1234"));
        dock.clear_last_commit();
        assert_eq!(dock.last_commit(), None);
    }

    #[test]
    fn commit_is_blocked_without_a_staged_file() {
        let mut dock = dock_with_one(false);
        dock.focus_commit();
        for c in "msg".chars() {
            dock.push_char(c);
        }
        assert!(!dock.can_commit(), "nothing is staged");
    }

    #[test]
    fn build_renders_the_commit_band_with_a_caret_when_focused() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = dock_with_one(true);
        dock.focus_commit();
        for c in "hi".chars() {
            dock.push_char(c);
        }
        let paint = built(&dock);
        let joined = texts(&paint);
        assert!(joined.contains("hi"), "the message input line");
        assert!(joined.contains("1 staged"), "the staged-count status line");
        // The caret (a thin accent bar) shows when the box has focus.
        assert!(
            paint
                .quads
                .iter()
                .any(|q| (q.alpha - 1.0).abs() < 1e-6 && q.color == theme.accent),
            "the commit caret"
        );
    }

    #[test]
    fn no_repo_shows_the_empty_state_with_an_init_button() {
        let dock = GitDock::new(); // repo_present is false until loaded
        assert!(dock.no_repo());
        let paint = built(&dock);
        let joined = texts(&paint);
        assert!(joined.contains("No repository here"));
        assert!(joined.contains("Init repo"));
        // The Init button has an accent highlight (so Enter has a visible target).
        assert!(!paint.quads.is_empty(), "init button highlight");
    }

    #[test]
    fn a_live_refresh_keeps_the_selected_file_and_its_scroll() {
        // The dock re-loads itself from the git poll while it is open (design §10.4), so a refresh
        // must not move the reader: it follows the selected path, not its index.
        let mut dock = GitDock::new();
        dock.load(sample_status());
        assert!(dock.move_selection(1), "select the second file");
        let selected = dock.selected_file().expect("a selection").path.clone();
        dock.set_diff(long_diff(), false);
        dock.scroll_diff(30);

        // A poll where a *new* file appeared first in git's order, and the file after ours went
        // clean - both of which would shift a plain index.
        let mut refreshed = sample_status();
        refreshed.files.remove(2);
        refreshed
            .files
            .insert(0, changed("fresh.rs", FileStatus::Added, 3, 0));
        dock.load(refreshed);
        dock.set_diff(long_diff(), false);

        assert_eq!(
            dock.selected_file().expect("still selected").path,
            selected,
            "the selection follows the file, not the row"
        );
        assert_eq!(
            dock.diff_scroll, 30,
            "and stays scrolled where the reader was"
        );
    }

    #[test]
    fn selecting_another_file_resets_the_diff_scroll() {
        let mut dock = GitDock::new();
        dock.load(sample_status());
        dock.set_diff(long_diff(), false);
        dock.scroll_diff(30);
        assert!(dock.move_selection(1));
        dock.set_diff(long_diff(), false);
        assert_eq!(dock.diff_scroll, 0, "a different file starts at the top");
    }

    #[test]
    fn matches_reports_whether_a_polled_status_is_already_on_screen() {
        let mut dock = GitDock::new();
        assert!(
            !dock.matches(&sample_status()),
            "nothing is loaded yet, so there is always work to do"
        );
        dock.load(sample_status());
        assert!(
            dock.matches(&sample_status()),
            "an idle poll changes nothing"
        );

        let mut edited = sample_status();
        edited.files[0].added += 1;
        assert!(!dock.matches(&edited), "a line count moved");

        dock.set_error("git exploded".to_owned());
        assert!(
            !dock.matches(&sample_status()),
            "an errored dock always reloads, so a transient failure clears itself"
        );
    }

    #[test]
    fn clean_tree_shows_the_branch_and_a_clean_note() {
        let mut dock = GitDock::new();
        dock.load(Status {
            branch: Some("main".to_owned()),
            ..Status::default()
        });
        let joined = texts(&built(&dock));
        assert!(joined.contains("main"));
        assert!(joined.contains("Working tree clean"));
    }

    #[test]
    fn status_bar_shows_branch_ahead_behind_and_totals() {
        let mut dock = GitDock::new();
        dock.load(sample_status());
        let joined = texts(&built(&dock));
        assert!(joined.contains("main"));
        assert!(joined.contains("\u{2191}2 \u{2193}1"), "ahead/behind");
        // Totals: 42+80+0 added, 11+0+34 removed.
        assert!(joined.contains("+122"));
        assert!(joined.contains("-45"));
        assert!(joined.contains("esc"));
    }

    #[test]
    fn file_list_marks_the_selected_file_and_lists_every_file() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        dock.load(sample_status());
        let paint = built(&dock);
        let joined = texts(&paint);
        assert!(joined.contains("src/pane/tree.rs"));
        assert!(joined.contains("src/session/timeline.rs"));
        assert!(joined.contains("old/legacy.rs"));
        assert!(joined.contains("CHANGED - 3"));
        // The selected file sits behind an accent.subtle fill, pre-composited opaque over bg.base.
        let selected_fill = theme.accent_subtle_on(theme.bg_base.to_srgb());
        assert!(
            paint
                .quads
                .iter()
                .any(|q| q.color == selected_fill && (q.alpha - 1.0).abs() < 1e-6),
            "selected-file fill"
        );
    }

    #[test]
    fn file_rows_show_a_stage_checkbox_reflecting_the_staged_flag() {
        let mut dock = GitDock::new();
        let mut staged = changed("staged.rs", FileStatus::Modified, 1, 0);
        staged.staged = true;
        staged.unstaged = false;
        dock.load(Status {
            branch: Some("main".to_owned()),
            files: vec![staged, changed("unstaged.rs", FileStatus::Modified, 1, 0)],
            ..Status::default()
        });
        let joined = texts(&built(&dock));
        assert!(joined.contains("[x]"), "a staged file's ticked checkbox");
        assert!(joined.contains("[ ]"), "an unstaged file's empty checkbox");
    }

    #[test]
    fn diff_body_classifies_hunk_add_del_context_rows() {
        let theme = Theme::resolve("ossein-dark");
        let mut dock = GitDock::new();
        dock.load(sample_status());
        dock.set_diff(sample_diff(), false);
        let paint = built(&dock);
        // Diff-row backgrounds are now opaque, pre-composited over bg.base in sRGB space.
        let base = theme.bg_base.to_srgb();
        let count = |color| {
            paint
                .quads
                .iter()
                .filter(|q| q.color == color && (q.alpha - 1.0).abs() < 1e-6)
                .count()
        };
        // The (focused) hunk header is an accent wash over a diff.hunk wash over bg.base.
        let hunk_bg = theme.accent.over(theme.diff_hunk.over(base, 0.08), 0.14);
        assert_eq!(count(hunk_bg), 1, "one focused hunk-header background");
        assert_eq!(
            count(theme.diff_add.over(base, 0.14)),
            1,
            "one addition background"
        );
        assert_eq!(
            count(theme.diff_del.over(base, 0.14)),
            1,
            "one deletion background"
        );
        let joined = texts(&paint);
        // The hunk header reconstructs its counts (1 context + 1 del = 2 old; 1+1 = 2 new).
        assert!(joined.contains("@@ -18,2 +18,2 @@ impl PaneTree"));
        // The first (only) hunk is focused, and its header shows the stage affordance.
        assert!(joined.contains("stage"));
    }

    #[test]
    fn focus_hunk_moves_between_hunks_and_reports_the_target() {
        let mut two = sample_diff();
        two.hunks.push(Hunk {
            old_start: 40,
            new_start: 41,
            heading: String::new(),
            lines: vec![DiffLine {
                kind: LineKind::Add,
                old_no: None,
                new_no: Some(41),
                text: "added tail".to_owned(),
            }],
        });
        let mut dock = GitDock::new();
        dock.load(sample_status());
        dock.set_diff(two, false);
        assert_eq!(
            dock.focused_hunk().unwrap().old_start,
            18,
            "starts on hunk 0"
        );
        dock.focus_hunk(1);
        assert_eq!(
            dock.focused_hunk().unwrap().old_start,
            40,
            "moved to hunk 1"
        );
        dock.focus_hunk(5);
        assert_eq!(dock.focused_hunk().unwrap().old_start, 40);
        dock.focus_hunk(-5);
        assert_eq!(dock.focused_hunk().unwrap().old_start, 18);
    }

    #[test]
    fn untracked_selection_shows_a_placeholder_not_an_empty_diff() {
        let mut dock = GitDock::new();
        dock.load(Status {
            branch: Some("main".to_owned()),
            files: vec![changed("new.txt", FileStatus::Untracked, 0, 0)],
            ..Status::default()
        });
        assert!(texts(&built(&dock)).contains("Untracked file"));
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
        assert!(dock.move_selection(100));
        assert_eq!(
            dock.selected_file().unwrap().path.to_string_lossy(),
            "old/legacy.rs"
        );
        assert!(!dock.move_selection(5), "already at the bottom: no change");
    }

    #[test]
    fn diff_scroll_moves_through_large_files_and_clamps_to_content() {
        let mut dock = GitDock::new();
        dock.load(sample_status());
        dock.set_diff(long_diff(), false);

        dock.scroll_diff(10);
        let scrolled = built(&dock);
        assert_eq!(scrolled.diff_scroll, 10);
        let visible = texts(&scrolled);
        assert!(visible.contains("large diff line 9"));
        assert!(!visible.contains("large diff line 0"));

        dock.set_scroll(scrolled.diff_scroll);
        dock.scroll_diff(1000); // far past the end
        let end = built(&dock).diff_scroll;
        assert!(end > 10, "a large diff has a real scroll range");
        assert!(end < 1000, "the scroll clamps to the content height");
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
        let paint = built(&dock);
        // The selected file (file30.rs) must appear and be highlighted.
        assert!(
            texts(&paint).contains("file30.rs"),
            "selected file scrolled into view"
        );
        let selected_fill = theme.accent_subtle_on(theme.bg_base.to_srgb());
        assert!(
            paint
                .quads
                .iter()
                .any(|q| q.color == selected_fill && (q.alpha - 1.0).abs() < 1e-6),
            "selection fill"
        );
    }
}
