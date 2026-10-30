//! The git diff dock: a per-repo right dock over the live terminal (AGENTS Hard rule 4 -
//! a layer, the pane tree never unmounts; only one right-dock surface shows at a time).
//! Opened with `⇧⌘G` and dismissed with `Esc`.
//!
//! It is where a person reviews what an agent changed, the way AI Team's review does: every
//! changed file in one scroll against `HEAD` - staged or not, untracked files as added - each
//! under a header that folds it, stages it, and marks it viewed; comment threads under the
//! lines they are about; and a composer that opens under a line from its `+`. The agent reads
//! the threads through `skelly review` and answers them there, and the dock shows the answers
//! as they land. Staging by file or hunk and the commit box stay, the commit box at the foot.
//!
//! This module is pure state + layout: it holds a [`Reading`] of the repository and builds a
//! *proportional* display list (quads + positioned labels - chrome in IBM Plex Sans, code and
//! metadata in the `mono` role so columns stay aligned) with the hit targets for it. The
//! binary owns the git and review-store calls, routing keys and clicks, and the viewport inset.

use std::collections::HashMap;
use std::path::PathBuf;

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};
use skelly_session::review::{Anchor, Author, Location, Review, Side, Status as Open, Thread};
use skelly_session::{
    ChangedFile, DiffLine, FileChange, FileStatus, Hunk, LineKind, Skipped, Status,
};

/// Git-dock layout constants in **logical** px (multiplied by the DPI scale). Tuned to the
/// guide's §10.6: a status bar, the `CHANGED - N` label, the review scroll, and a commit box
/// at the foot.
const PAD_X: f32 = 12.0;
/// Top padding above the status bar.
const PAD_TOP: f32 = 12.0;
/// Status-bar row height.
const STATUS_H: f32 = 26.0;
/// Section-label (`CHANGED - N`) row height.
const LABEL_H: f32 = 24.0;
/// A file header's height.
const FILE_HEAD_H: f32 = 30.0;
/// The height of the line saying why a file's lines are not shown.
const NOTE_H: f32 = 24.0;
/// Diff body line height (compact, code).
const DIFF_ROW_H: f32 = 20.0;
/// A thread or composer card's header height.
const CARD_HEAD_H: f32 = 26.0;
/// A card's text-line height.
const CARD_TEXT_H: f32 = 20.0;
/// A card's author-line height.
const CARD_AUTHOR_H: f32 = 18.0;
/// The composer's button-row height.
const CARD_FOOT_H: f32 = 32.0;
/// The space under a card.
const CARD_GAP_H: f32 = 6.0;
/// A card's inner horizontal padding.
const CARD_PAD: f32 = 10.0;
/// The width of a card's left status bar.
const CARD_BAR: f32 = 2.0;
/// Each commit-band row's height (message input, status line).
const COMMIT_ROW_H: f32 = 24.0;
/// The commit band's total height at the foot (a divider + input + status + padding).
const COMMIT_BAND_H: f32 = 78.0;
/// Gutter width for the diff line-number, in mono chars.
const GUTTER_CHARS: usize = 4;
/// The width of the column the `+` comment button sits in, left of the gutter.
const ADD_W: f32 = 16.0;
/// A small button's height and horizontal padding, the gap between buttons, and its radius.
const BUTTON_H: f32 = 20.0;
const BUTTON_PAD_X: f32 = 7.0;
const BUTTON_GAP: f32 = 4.0;
const BUTTON_RADIUS: f32 = 5.0;
/// The Viewed checkbox's side.
const CHECK: f32 = 12.0;
/// Alpha for an add/del diff-line background (the guide's `diff.*.bg` tokens).
const DIFF_BG_ALPHA: f32 = 0.14;
/// Alpha for a hunk-header background (the lighter `diff.hunk.bg`).
const HUNK_BG_ALPHA: f32 = 0.08;
/// A line longer than this is not for reading: a minified bundle, or generated output. A
/// file with one starts folded, unless a comment is on it.
const UNREADABLE_LINE: usize = 1000;
/// The comment glyph (codicon `comment`), drawn in the bundled Nerd font.
const COMMENT_ICON: &str = "\u{EA6B}";
/// A file header's fold carets (codicon `chevron-down` / `chevron-right`), in the Nerd font.
const CHEVRON_DOWN: &str = "\u{EAB4}";
const CHEVRON_RIGHT: &str = "\u{EAB6}";

/// Which part of the dock has keyboard focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    /// The review scroll (arrows, fold, stage, comment).
    List,
    /// The commit-message box (typing edits the message; Enter commits).
    Commit,
    /// The comment composer (typing edits the comment; Enter sends it).
    Composer,
}

/// Everything the dock shows about a repository, read together so it is one consistent
/// view: the status, every change against `HEAD`, and the review with where each thread is.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Reading {
    /// The repository's root.
    pub(crate) root: PathBuf,
    /// The working status (branch, ahead/behind, and what is staged).
    pub(crate) status: Status,
    /// Every changed file against `HEAD`, untracked files included.
    pub(crate) changes: Vec<FileChange>,
    /// The review's threads and viewed marks, when the review could be read.
    pub(crate) review: Option<Review>,
    /// Where each of the review's threads is now, in the same order as its threads.
    pub(crate) locations: Vec<Location>,
    /// Why the review could not be read, when it could not.
    pub(crate) review_problem: Option<String>,
}

/// A row the keyboard cursor can rest on, by identity, so a reload keeps it in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Spot {
    /// A file's header.
    File(String),
    /// A diff line, by the side and number a comment on it anchors to.
    Line {
        /// The file.
        path: String,
        /// The side the line number counts on.
        side: Side,
        /// The line number on that side.
        line: u32,
    },
    /// A thread's header.
    Thread(u64),
}

/// What a click or a hover lands on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// The empty state's "Init repo" button.
    Init,
    /// A file's stage checkbox.
    Stage(String),
    /// A file header, which folds or unfolds it.
    Fold(String),
    /// A file's Viewed checkbox.
    Viewed(String),
    /// A row the cursor can rest on.
    Row(Spot),
    /// A diff line's `+`, which opens the composer on it.
    Comment(Spot),
    /// A hunk's stage / unstage button.
    Hunk {
        /// The file.
        path: String,
        /// The hunk's index in the file's change.
        hunk: usize,
    },
    /// A thread's Reply button.
    Reply(u64),
    /// A thread's Resolve button.
    Resolve(u64),
    /// A thread's Reopen button.
    Reopen(u64),
    /// A thread's Delete button.
    Delete(u64),
    /// The composer's text.
    Compose,
    /// The composer's send button.
    Send,
    /// The composer's Cancel button.
    Cancel,
    /// The commit-message input.
    Message,
}

impl Target {
    /// Whether clicking it does something (the pointer cursor shows), as opposed to a row
    /// that only takes the keyboard cursor.
    pub(crate) fn clickable(&self) -> bool {
        !matches!(self, Target::Row(_))
    }
}

/// What the composer is writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Compose {
    /// A new comment on these lines.
    Comment(Anchor),
    /// A reply on this thread.
    Reply(u64),
}

/// The open comment composer.
struct Composer {
    target: Compose,
    text: String,
    /// Why the last send failed, shown under the text.
    problem: Option<String>,
}

/// What the next build scrolls into view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reveal {
    Cursor,
    Composer,
}

/// The git diff dock's state: the open flag, the loaded [`Reading`], the review scroll's
/// cursor / folds / scroll, the composer, and the commit box.
pub(crate) struct GitDock {
    /// Whether the dock is showing (captures navigation keys while open and focused).
    pub(crate) open: bool,
    /// Whether a repository was found for the current context (else the empty state).
    repo_present: bool,
    /// The repository as last read.
    data: Reading,
    /// Files folded or unfolded by hand, over the default (folded when viewed).
    folds: HashMap<String, bool>,
    /// The keyboard cursor.
    cursor: Option<Spot>,
    /// The cursor's index among the focusable rows, and the row before it: where it goes when
    /// the row it was on goes away.
    cursor_at: usize,
    cursor_before: Option<Spot>,
    /// The first row of the scroll on screen.
    scroll: usize,
    /// How many rows the last build showed (a page).
    page: usize,
    /// Where the scroll stood, by row identity, so a reload that moves rows above it does
    /// not move the reader: the nearest focusable row at or above the top and its file's
    /// header, each with how many rows below it the top was.
    anchors: Vec<(Spot, usize)>,
    /// Whether the next build resolves [`Self::anchors`] (set by a reload).
    reanchor: bool,
    /// What the next build scrolls into view.
    reveal: Option<Reveal>,
    /// Which part of the dock has keyboard focus.
    focus: Focus,
    /// The comment composer, while one is open.
    composer: Option<Composer>,
    /// The target under the pointer.
    hover: Option<Target>,
    /// The hit targets of the last build, back to front.
    targets: Vec<(PxRect, Target)>,
    /// The commit message being typed in the commit box.
    message: String,
    /// The short SHA of the just-made commit, shown with an Undo hint until the next
    /// action clears it.
    last_commit: Option<String>,
    /// A git error to surface instead of the review, if the last refresh failed.
    error: Option<String>,
}

impl GitDock {
    /// A closed, empty dock.
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            repo_present: false,
            data: Reading::default(),
            folds: HashMap::new(),
            cursor: None,
            cursor_at: 0,
            cursor_before: None,
            scroll: 0,
            page: 0,
            anchors: Vec::new(),
            reanchor: false,
            reveal: None,
            focus: Focus::List,
            composer: None,
            hover: None,
            targets: Vec::new(),
            message: String::new(),
            last_commit: None,
            error: None,
        }
    }

    /// Open the dock on the review (the binary loads the repository right after).
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.focus = Focus::List;
        self.last_commit = None;
    }

    /// Close the dock.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.hover = None;
        if self.focus == Focus::Composer {
            self.focus = Focus::List;
        }
    }

    /// Forget the place in the review - cursor, folds, scroll, composer, message - for a
    /// different repository, whose files the old place does not name.
    fn reset_view(&mut self) {
        self.folds.clear();
        self.cursor = None;
        self.cursor_at = 0;
        self.cursor_before = None;
        self.scroll = 0;
        self.anchors.clear();
        self.reveal = None;
        self.composer = None;
        self.focus = Focus::List;
        self.message.clear();
        self.last_commit = None;
    }

    /// Whether the commit box currently has focus.
    pub(crate) fn commit_focused(&self) -> bool {
        self.focus == Focus::Commit
    }

    /// Whether the composer currently has focus.
    pub(crate) fn composer_focused(&self) -> bool {
        self.focus == Focus::Composer && self.composer.is_some()
    }

    /// Move focus to the commit box (so typing edits the message).
    pub(crate) fn focus_commit(&mut self) {
        self.focus = Focus::Commit;
    }

    /// Move focus back to the review scroll.
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
        self.data.status.files.iter().filter(|f| f.staged).count()
    }

    /// Whether a commit is allowed: at least one file staged and a non-blank message.
    pub(crate) fn can_commit(&self) -> bool {
        self.staged_count() > 0 && !self.message.trim().is_empty()
    }

    /// Record a successful commit: clear the message, return focus to the review, and keep
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
        self.reset_view();
        self.repo_present = false;
        self.data = Reading::default();
        self.error = None;
    }

    /// Whether `reading` is what the dock is already showing - the guard that keeps a live
    /// refresh (design §10.4) free when nothing moved.
    pub(crate) fn matches(&self, reading: &Reading) -> bool {
        self.repo_present && self.error.is_none() && self.data == *reading
    }

    /// Load a fresh reading of the repository, keeping the reader's place: the cursor and
    /// the scroll follow the rows they were on, by identity, rather than their positions.
    pub(crate) fn load(&mut self, reading: Reading) {
        if reading.root != self.data.root {
            self.reset_view();
        }
        self.repo_present = true;
        self.error = None;
        self.data = reading;
        self.reanchor = true;
    }

    /// Surface a git error in place of the review.
    pub(crate) fn set_error(&mut self, message: String) {
        self.repo_present = true;
        self.error = Some(message);
    }

    /// The review's threads (none when it could not be read).
    fn threads(&self) -> &[Thread] {
        self.data.review.as_ref().map_or(&[], Review::threads)
    }

    /// Where thread `index` is now.
    fn location(&self, index: usize) -> Location {
        self.data
            .locations
            .get(index)
            .copied()
            .unwrap_or(Location::Outdated)
    }

    /// Thread `id`, if the review has it.
    pub(crate) fn thread(&self, id: u64) -> Option<&Thread> {
        self.threads().iter().find(|t| t.id == id)
    }

    /// Whether comments can be written: the review was read (a damaged file, or one from a
    /// newer Skelly, is never written over).
    pub(crate) fn can_comment(&self) -> bool {
        self.data.review.is_some()
    }

    /// The change to `path`, if it changed.
    fn change(&self, path: &str) -> Option<&FileChange> {
        self.data
            .changes
            .iter()
            .find(|c| c.path.to_string_lossy() == path)
    }

    /// Whether the change to `path` is marked viewed as it is now.
    pub(crate) fn is_viewed(&self, path: &str) -> bool {
        self.change(path).is_some_and(|c| self.viewed(c))
    }

    /// Whether `change` is marked viewed as it is now.
    fn viewed(&self, change: &FileChange) -> bool {
        self.data
            .review
            .as_ref()
            .is_some_and(|r| r.is_viewed(&change.path.to_string_lossy(), &change.fingerprint))
    }

    /// The fingerprint a viewed mark on `path` is kept against.
    pub(crate) fn fingerprint(&self, path: &str) -> Option<&str> {
        self.change(path).map(|c| c.fingerprint.as_str())
    }

    /// Every changed path (what viewed marks are kept for).
    pub(crate) fn changed_paths(&self) -> Vec<String> {
        self.data
            .changes
            .iter()
            .map(|c| c.path.to_string_lossy().into_owned())
            .collect()
    }

    /// What `git status` says about `path`: its own entry, or the untracked directory it is in.
    pub(crate) fn status_of(&self, path: &str) -> Option<&ChangedFile> {
        let files = &self.data.status.files;
        files
            .iter()
            .find(|f| f.path.to_string_lossy() == path)
            .or_else(|| {
                files.iter().find(|f| {
                    f.status == FileStatus::Untracked
                        && std::path::Path::new(path).starts_with(&f.path)
                })
            })
    }

    /// Hunk `index` of the change to `path`.
    pub(crate) fn hunk(&self, path: &str, index: usize) -> Option<&Hunk> {
        self.change(path)?.diff.hunks.get(index)
    }

    /// Whether `path` is untracked (staging a hunk of it stages the file).
    pub(crate) fn is_untracked(&self, path: &str) -> bool {
        self.change(path)
            .is_some_and(|c| c.status == FileStatus::Untracked)
    }

    /// Fold or unfold `path`, with the cursor on its header.
    pub(crate) fn toggle_fold(&mut self, path: &str) {
        let folded = self
            .outline()
            .sections
            .iter()
            .find(|s| s.path == path)
            .is_some_and(|s| s.folded);
        self.folds.insert(path.to_owned(), !folded);
        self.set_cursor(Spot::File(path.to_owned()));
    }

    /// Drop the hand-set fold on `path`, so it folds by its viewed mark again - called after
    /// marking it, the way AI Team folds a file as it is marked viewed.
    pub(crate) fn unfold_by_hand(&mut self, path: &str) {
        self.folds.remove(path);
        if self.cursor_file().as_deref() == Some(path) {
            self.cursor = Some(Spot::File(path.to_owned()));
            self.reveal = Some(Reveal::Cursor);
        }
    }

    /// The keyboard cursor.
    pub(crate) fn cursor(&self) -> Option<&Spot> {
        self.cursor.as_ref()
    }

    /// Put the keyboard cursor on `spot` and scroll it into view.
    pub(crate) fn set_cursor(&mut self, spot: Spot) {
        self.cursor = Some(spot);
        self.reveal = Some(Reveal::Cursor);
    }

    /// The file the cursor is in.
    pub(crate) fn cursor_file(&self) -> Option<String> {
        match self.cursor.as_ref()? {
            Spot::File(path) | Spot::Line { path, .. } => Some(path.clone()),
            Spot::Thread(id) => self.thread(*id)?.anchor.as_ref().map(|a| a.path.clone()),
        }
    }

    /// The hunk the cursor is in: its file and index.
    pub(crate) fn cursor_hunk(&self) -> Option<(String, usize)> {
        let Some(Spot::Line { path, side, line }) = &self.cursor else {
            return None;
        };
        let change = self.change(path)?;
        let index = change.diff.hunks.iter().position(|hunk| {
            hunk.lines
                .iter()
                .any(|l| line_spot(l) == Some((*side, *line)))
        })?;
        Some((path.clone(), index))
    }

    /// Move the cursor `delta` focusable rows, clamped; from no cursor, onto the first.
    pub(crate) fn move_cursor(&mut self, delta: i32) {
        let spots = self.spots();
        if spots.is_empty() {
            return;
        }
        let next = match self.cursor_index(&spots.iter().collect::<Vec<_>>()) {
            Some(at) => step(at, delta, spots.len()),
            None => 0,
        };
        self.set_cursor(spots[next].clone());
    }

    /// Move the cursor to the next (`delta > 0`) or previous file header.
    pub(crate) fn move_file(&mut self, delta: i32) {
        self.jump(delta, |spot| matches!(spot, Spot::File(_)));
    }

    /// Move the cursor to the first line of the next (`delta > 0`) or previous hunk.
    pub(crate) fn move_hunk(&mut self, delta: i32) {
        let outline = self.outline();
        let starts: Vec<Spot> = outline
            .items
            .windows(2)
            .filter(|pair| matches!(pair[0], Item::Hunk(..)))
            .filter_map(|pair| self.item_spot(&outline, pair[1]))
            .collect();
        self.jump(delta, |spot| starts.contains(spot));
    }

    /// Move the cursor to the nearest focusable row past it in `delta`'s direction that
    /// `accept` takes.
    fn jump(&mut self, delta: i32, accept: impl Fn(&Spot) -> bool) {
        let spots = self.spots();
        let at = self.cursor_index(&spots.iter().collect::<Vec<_>>());
        let found = if delta > 0 {
            let from = at.map_or(0, |i| i + 1);
            (from..spots.len()).find(|&i| accept(&spots[i]))
        } else {
            let to = at.unwrap_or(spots.len());
            (0..to).rev().find(|&i| accept(&spots[i]))
        };
        if let Some(index) = found {
            self.set_cursor(spots[index].clone());
        }
    }

    /// Where the cursor is among `spots`: on its own row; or, when that row has gone (the line
    /// was edited away, the file folded), on the row after the one that was before it; or
    /// failing that, where it last was.
    fn cursor_index(&self, spots: &[&Spot]) -> Option<usize> {
        let cursor = self.cursor.as_ref()?;
        let last = spots.len().checked_sub(1)?;
        spots
            .iter()
            .position(|s| *s == cursor)
            .or_else(|| {
                let before = self.cursor_before.as_ref()?;
                let at = spots.iter().position(|s| *s == before)?;
                Some((at + 1).min(last))
            })
            .or(Some(self.cursor_at.min(last)))
    }

    /// Scroll the review by `delta` rows (clamped in [`Self::build`]).
    pub(crate) fn scroll_rows(&mut self, delta: i32) {
        self.scroll = step_unclamped(self.scroll, delta);
        self.reveal = None;
    }

    /// Scroll the review by `delta` pages.
    pub(crate) fn page(&mut self, delta: i32) {
        let page = i32::try_from(self.page.max(1)).unwrap_or(1);
        self.scroll_rows(delta.saturating_mul(page));
    }

    /// Open the composer on the diff line at `spot`, keeping any text already written: there is
    /// one composer, and a draft moves with it rather than being lost. `false` when `spot` is
    /// not a line.
    pub(crate) fn start_comment(&mut self, spot: &Spot) -> bool {
        let Spot::Line { path, side, line } = spot else {
            return false;
        };
        let Some(text) = self.change(path).and_then(|c| {
            c.diff
                .hunks
                .iter()
                .flat_map(|h| &h.lines)
                .find(|l| line_spot(l) == Some((*side, *line)))
                .map(|l| l.text.clone())
        }) else {
            return false;
        };
        let anchor = Anchor {
            path: path.clone(),
            side: *side,
            line: *line,
            line_end: *line,
            text,
        };
        self.compose(Compose::Comment(anchor));
        self.cursor = Some(spot.clone());
        true
    }

    /// Open the composer for a reply on thread `id`.
    pub(crate) fn start_reply(&mut self, id: u64) {
        self.compose(Compose::Reply(id));
        self.cursor = Some(Spot::Thread(id));
    }

    fn compose(&mut self, target: Compose) {
        let text = self.composer.take().map(|c| c.text).unwrap_or_default();
        self.composer = Some(Composer {
            target,
            text,
            problem: None,
        });
        self.focus = Focus::Composer;
        self.reveal = Some(Reveal::Composer);
    }

    /// Give the open composer focus again.
    pub(crate) fn focus_composer(&mut self) {
        if self.composer.is_some() {
            self.focus = Focus::Composer;
        }
    }

    /// Type `text` into the composer (a paste, or a keystroke). Control characters other
    /// than a newline are dropped.
    pub(crate) fn compose_text(&mut self, text: &str) {
        if let Some(composer) = self.composer.as_mut() {
            composer
                .text
                .extend(text.chars().filter(|c| *c == '\n' || !c.is_control()));
            composer.problem = None;
        }
    }

    /// Delete the composer's last character.
    pub(crate) fn compose_backspace(&mut self) {
        if let Some(composer) = self.composer.as_mut() {
            composer.text.pop();
        }
    }

    /// Leave the composer (`Esc`): an empty one closes; one with a draft stays where it is,
    /// unfocused, so the draft is never lost to a stray key.
    pub(crate) fn leave_composer(&mut self) {
        if self
            .composer
            .as_ref()
            .is_some_and(|c| c.text.trim().is_empty())
        {
            self.composer = None;
        }
        self.focus = Focus::List;
    }

    /// Close the composer and discard what it held (its Cancel, or after a send).
    pub(crate) fn close_composer(&mut self) {
        self.composer = None;
        if self.focus == Focus::Composer {
            self.focus = Focus::List;
        }
    }

    /// What the composer would send: its target and text, when there is text.
    pub(crate) fn composed(&self) -> Option<(Compose, String)> {
        let composer = self.composer.as_ref()?;
        let text = composer.text.trim();
        (!text.is_empty()).then(|| (composer.target.clone(), text.to_owned()))
    }

    /// Show why the last send failed, under the composer's text.
    pub(crate) fn set_composer_problem(&mut self, problem: String) {
        if let Some(composer) = self.composer.as_mut() {
            composer.problem = Some(problem);
        }
    }

    /// Track the target under the pointer at (`px`, `py`); `true` when it changed (repaint).
    pub(crate) fn hover_at(&mut self, px: f32, py: f32) -> bool {
        let hover = self.target_at(px, py);
        let changed = hover != self.hover;
        self.hover = hover;
        changed
    }

    /// The target under the pointer.
    pub(crate) fn hovered(&self) -> Option<&Target> {
        self.hover.as_ref()
    }

    /// Why the review could not be read, when it could not.
    pub(crate) fn review_problem(&self) -> Option<&str> {
        self.data.review_problem.as_deref()
    }

    /// Forget the hovered target (the pointer left the dock); `true` when there was one.
    pub(crate) fn clear_hover(&mut self) -> bool {
        self.hover.take().is_some()
    }

    /// The target at (`px`, `py`) in the last build, the frontmost first.
    pub(crate) fn target_at(&self, px: f32, py: f32) -> Option<Target> {
        self.targets
            .iter()
            .rev()
            .find(|(r, _)| px >= r.x && px < r.x + r.w && py >= r.y && py < r.y + r.h)
            .map(|(_, target)| target.clone())
    }

    /// The scroll's structure: its sections and items, folds applied.
    fn outline(&self) -> Outline {
        let threads = self.threads();
        let composer_path = self.composer_path();
        let mut sections: Vec<Section> = self
            .data
            .changes
            .iter()
            .enumerate()
            .map(|(i, c)| Section {
                path: c.path.to_string_lossy().into_owned(),
                change: Some(i),
                folded: false,
            })
            .collect();
        let mut extra: Vec<String> = threads
            .iter()
            .filter(|t| t.status == Open::Open)
            .filter_map(|t| t.anchor.as_ref().map(|a| a.path.clone()))
            .filter(|path| !sections.iter().any(|s| &s.path == path))
            .collect();
        extra.sort();
        extra.dedup();
        sections.extend(extra.into_iter().map(|path| Section {
            path,
            change: None,
            folded: false,
        }));
        for section in &mut sections {
            section.folded = self.folded(section, composer_path.as_deref());
        }
        let mut items = Vec::new();
        // A resolved thread shows while what it is about is still changed, as on a file.
        let changed = !self.data.changes.is_empty();
        let general: Vec<usize> = (0..threads.len())
            .filter(|&t| {
                threads[t].anchor.is_none() && (changed || threads[t].status == Open::Open)
            })
            .collect();
        if !general.is_empty() {
            items.push(Item::General);
            for t in general {
                self.push_thread(&mut items, t, true);
            }
        }
        for (si, section) in sections.iter().enumerate() {
            items.push(Item::File(si));
            if !section.folded {
                self.push_section(&mut items, si, section);
            }
        }
        Outline { sections, items }
    }

    /// The file the composer is on, which stays unfolded while it is open.
    fn composer_path(&self) -> Option<String> {
        match &self.composer.as_ref()?.target {
            Compose::Comment(anchor) => Some(anchor.path.clone()),
            Compose::Reply(id) => self.thread(*id)?.anchor.as_ref().map(|a| a.path.clone()),
        }
    }

    /// Whether `section` is folded: as set by hand, else when it is viewed, or unreadable
    /// (a minified line) with nothing to read on it.
    fn folded(&self, section: &Section, composer_path: Option<&str>) -> bool {
        if composer_path == Some(section.path.as_str()) {
            return false;
        }
        if let Some(&folded) = self.folds.get(&section.path) {
            return folded;
        }
        let Some(change) = section.change.map(|i| &self.data.changes[i]) else {
            return false;
        };
        let commented = self
            .threads()
            .iter()
            .any(|t| t.anchor.as_ref().is_some_and(|a| a.path == section.path));
        self.viewed(change) || (longest_line(change) > UNREADABLE_LINE && !commented)
    }

    /// A thread card, and the composer under it when it is replying there.
    fn push_thread(&self, items: &mut Vec<Item>, thread: usize, attached: bool) {
        items.push(Item::Thread(thread, attached));
        let id = self.threads()[thread].id;
        if self
            .composer
            .as_ref()
            .is_some_and(|c| c.target == Compose::Reply(id))
        {
            items.push(Item::Composer);
        }
    }

    /// An unfolded file's rows: its hunks and lines with each thread under the line it is
    /// on, then the threads that are on no line shown, then the composer if it is here.
    fn push_section(&self, items: &mut Vec<Item>, si: usize, section: &Section) {
        let threads = self.threads();
        let mut pending: Vec<usize> = (0..threads.len())
            .filter(|&t| {
                threads[t]
                    .anchor
                    .as_ref()
                    .is_some_and(|a| a.path == section.path)
                    && (section.change.is_some() || threads[t].status == Open::Open)
            })
            .collect();
        let mut composer = self.composer.as_ref().and_then(|c| match &c.target {
            Compose::Comment(a) if a.path == section.path => Some((a.side, a.line)),
            _ => None,
        });
        let change = section.change.map(|i| &self.data.changes[i]);
        match change {
            Some(change) if change.skipped.is_none() && !change.diff.hunks.is_empty() => {
                for (hi, hunk) in change.diff.hunks.iter().enumerate() {
                    items.push(Item::Hunk(si, hi));
                    for (li, line) in hunk.lines.iter().enumerate() {
                        items.push(Item::Line(si, hi, li));
                        let Some(spot) = line_spot(line) else {
                            continue;
                        };
                        pending.retain(|&t| {
                            let here = self.thread_spot(t) == Some(spot);
                            if here {
                                self.push_thread(items, t, true);
                            }
                            !here
                        });
                        if composer == Some(spot) {
                            items.push(Item::Composer);
                            composer = None;
                        }
                    }
                }
            }
            _ => items.push(Item::Note(si)),
        }
        for t in pending {
            self.push_thread(items, t, false);
        }
        if composer.is_some() {
            items.push(Item::Composer);
        }
    }

    /// The side and line thread `index` is on now, when it is on one.
    fn thread_spot(&self, index: usize) -> Option<(Side, u32)> {
        let anchor = self.threads()[index].anchor.as_ref()?;
        match self.location(index) {
            Location::At(line) => Some((anchor.side, line)),
            Location::Whole | Location::Outdated => None,
        }
    }

    /// The focusable rows, top to bottom.
    fn spots(&self) -> Vec<Spot> {
        let outline = self.outline();
        outline
            .items
            .iter()
            .filter_map(|&item| self.item_spot(&outline, item))
            .collect()
    }

    /// The spot of `item`, when the cursor can rest on it.
    fn item_spot(&self, outline: &Outline, item: Item) -> Option<Spot> {
        match item {
            Item::File(si) => Some(Spot::File(outline.sections[si].path.clone())),
            Item::Line(si, hi, li) => {
                let line = self.diff_line(outline, si, hi, li)?;
                let (side, number) = line_spot(line)?;
                Some(Spot::Line {
                    path: outline.sections[si].path.clone(),
                    side,
                    line: number,
                })
            }
            Item::Thread(t, _) => Some(Spot::Thread(self.threads()[t].id)),
            Item::General | Item::Note(_) | Item::Hunk(..) | Item::Composer => None,
        }
    }

    /// Diff line `li` of hunk `hi` of section `si`.
    fn diff_line(&self, outline: &Outline, si: usize, hi: usize, li: usize) -> Option<&DiffLine> {
        let change = &self.data.changes[outline.sections[si].change?];
        change.diff.hunks.get(hi)?.lines.get(li)
    }

    /// Build the dock's display list within `panel` (physical px) at DPI `scale`, in
    /// `theme`'s UI tokens: the status bar, the review scroll, and the commit box. Keeps the
    /// hit targets for [`Self::target_at`]. The renderer draws the dock frame (shadow +
    /// divider).
    pub(crate) fn build(
        &mut self,
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
        let mut out = Out::default();
        let outline = self.outline();
        if !self.empty_paint(&ctx, measure, &outline, &mut out) {
            self.push_status_bar(&mut out, &ctx, measure, panel.y + PAD_TOP * scale);
            let top = panel.y + (PAD_TOP + STATUS_H) * scale;
            self.push_label_row(&mut out, &ctx, measure, top);
            let body = (
                top + LABEL_H * scale,
                panel.y + panel.h - COMMIT_BAND_H * scale,
            );
            let rows = self.rows(&outline, &ctx, measure);
            self.settle(&outline, &rows, body.1 - body.0, scale);
            self.paint_rows(&outline, &rows, &ctx, measure, body, &mut out);
            self.push_commit(&mut out, &ctx, measure, body.1);
        }
        self.targets = out.targets;
        Paint {
            quads: out.quads,
            labels: out.labels,
        }
    }

    /// The empty / error states, each a centered message: no repo (with an Init button), a
    /// git error, or nothing to review (with the status bar). `true` when it painted one.
    fn empty_paint(
        &self,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        outline: &Outline,
        out: &mut Out,
    ) -> bool {
        let (panel, scale, theme) = (ctx.panel, ctx.scale, ctx.theme);
        let mid = panel.y + panel.h * 0.5;
        if !self.repo_present {
            push_centered(
                &mut out.labels,
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
            let rect = PxRect {
                x: bx - 10.0 * scale,
                y: by,
                w: w + 20.0 * scale,
                h: 28.0 * scale,
            };
            out.quads.push(ChromeQuad::rounded(
                rect,
                theme.accent_subtle_on(theme.bg_base.to_srgb()),
                6.0 * scale,
            ));
            push_row(
                &mut out.labels,
                measure,
                "Init repo  \u{21a9}",
                FontRole::Body,
                theme.accent,
                bx,
                by,
                28.0,
                scale,
            );
            out.targets.push((rect, Target::Init));
            return true;
        }
        if let Some(error) = &self.error {
            push_centered(
                &mut out.labels,
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
                &mut out.labels,
                measure,
                &msg,
                FontRole::Caption,
                theme.fg_muted,
                panel,
                mid + 14.0 * scale,
            );
            return true;
        }
        if !outline.items.is_empty() {
            return false;
        }
        self.push_status_bar(out, ctx, measure, panel.y + PAD_TOP * scale);
        let message = match &self.last_commit {
            Some(sha) => format!("committed {sha} - press u to undo"),
            None => "Working tree clean".to_owned(),
        };
        push_centered(
            &mut out.labels,
            measure,
            &message,
            FontRole::Body,
            theme.fg_muted,
            panel,
            mid,
        );
        true
    }

    /// The status bar: branch (`diff.hunk`), ahead/behind (muted), the `+added`/`-removed`
    /// totals right-anchored, and an `esc` hint.
    fn push_status_bar(&self, out: &mut Out, ctx: &GCtx, measure: &mut TextMeasure, top: f32) {
        let theme = ctx.theme;
        let labels = &mut out.labels;
        let status = &self.data.status;
        let branch = format!(
            "\u{2442} {}",
            status.branch.as_deref().unwrap_or("(detached)")
        );
        push_row(
            labels,
            measure,
            &branch,
            FontRole::Mono,
            theme.diff_hunk,
            ctx.cx,
            top,
            STATUS_H,
            ctx.scale,
        );
        if status.ahead > 0 || status.behind > 0 {
            let x = ctx.cx + measure.width(&branch, FontRole::Mono, None) + 10.0 * ctx.scale;
            let text = format!("\u{2191}{} \u{2193}{}", status.ahead, status.behind);
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
        let (added, removed) = self.data.changes.iter().fold((0, 0), |(a, r), c| {
            let (ca, cr) = c.diff.stats();
            (a + ca, r + cr)
        });
        // Right-anchored right-to-left: esc, then -removed, then +added.
        let end = push_right(
            labels,
            measure,
            "esc",
            FontRole::Caption,
            theme.fg_muted,
            ctx.cr,
            top,
            STATUS_H,
            ctx.scale,
        ) - 10.0 * ctx.scale;
        push_counts(labels, measure, ctx, (added, removed), end, top, STATUS_H);
    }

    /// The label over the scroll: `CHANGED - N` and how many are viewed, with the open
    /// comment count (or the key hints, when nothing is open) right-anchored.
    fn push_label_row(&self, out: &mut Out, ctx: &GCtx, measure: &mut TextMeasure, top: f32) {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let total = self.data.changes.len();
        let label = format!("CHANGED - {total}");
        push_row(
            &mut out.labels,
            measure,
            &label,
            FontRole::Micro,
            theme.fg_muted,
            ctx.cx,
            top,
            LABEL_H,
            scale,
        );
        let viewed = self.data.changes.iter().filter(|c| self.viewed(c)).count();
        if total > 0 {
            let x = ctx.cx + measure.width(&label, FontRole::Micro, None) + 10.0 * scale;
            push_row(
                &mut out.labels,
                measure,
                &format!("{viewed}/{total} viewed"),
                FontRole::Caption,
                theme.fg_muted,
                x,
                top,
                LABEL_H,
                scale,
            );
        }
        let open = self
            .threads()
            .iter()
            .filter(|t| t.status == Open::Open)
            .count();
        let (text, role, color) = if let Some(problem) = &self.data.review_problem {
            (
                format!("comments unavailable: {problem}"),
                FontRole::Caption,
                theme.diff_del,
            )
        } else if open > 0 {
            (
                format!("{open} open comment{}", if open == 1 { "" } else { "s" }),
                FontRole::Caption,
                theme.accent,
            )
        } else {
            (
                "c comment  v viewed".to_owned(),
                FontRole::Caption,
                theme.fg_muted,
            )
        };
        push_right(
            &mut out.labels,
            measure,
            &text,
            role,
            color,
            ctx.cr,
            top,
            LABEL_H,
            scale,
        );
    }

    /// Expand the outline into drawn rows, wrapping card text to the card's width.
    fn rows(&self, outline: &Outline, ctx: &GCtx, measure: &mut TextMeasure) -> Vec<Row> {
        let text_w = (ctx.cr - ctx.cx - 2.0 * CARD_PAD * ctx.scale).max(1.0);
        let mut rows = Vec::new();
        let mut section = None;
        for (index, &item) in outline.items.iter().enumerate() {
            let mut push = |h: f32, part: Part| {
                rows.push(Row {
                    item: index,
                    section,
                    h,
                    part,
                });
            };
            match item {
                Item::General => {
                    section = None;
                    rows.push(Row {
                        item: index,
                        section,
                        h: LABEL_H,
                        part: Part::Whole,
                    });
                }
                Item::File(si) => {
                    section = Some(si);
                    rows.push(Row {
                        item: index,
                        section,
                        h: FILE_HEAD_H,
                        part: Part::Whole,
                    });
                }
                Item::Note(_) => push(NOTE_H, Part::Whole),
                Item::Hunk(..) | Item::Line(..) => push(DIFF_ROW_H, Part::Whole),
                Item::Thread(t, attached) => {
                    let thread = &self.threads()[t];
                    push(CARD_HEAD_H, Part::Head);
                    if !attached && thread.anchor.is_some() {
                        push(CARD_TEXT_H, Part::Quote);
                    }
                    for (m, message) in thread.messages.iter().enumerate() {
                        push(CARD_AUTHOR_H, Part::Author(m));
                        for line in wrap(&message.body, FontRole::Body, text_w, measure) {
                            push(CARD_TEXT_H, Part::Text(line));
                        }
                    }
                    push(CARD_GAP_H, Part::Gap);
                }
                Item::Composer => {
                    let Some(composer) = self.composer.as_ref() else {
                        continue;
                    };
                    push(CARD_HEAD_H, Part::Head);
                    let lines = wrap(&composer.text, FontRole::Body, text_w, measure);
                    let count = lines.len();
                    for (i, text) in lines.into_iter().enumerate() {
                        let last = i + 1 == count;
                        push(CARD_TEXT_H, Part::Draft { text, last });
                    }
                    if composer.problem.is_some() {
                        push(CARD_TEXT_H, Part::Problem);
                    }
                    push(CARD_FOOT_H, Part::Foot);
                    push(CARD_GAP_H, Part::Gap);
                }
            }
        }
        rows
    }

    /// Settle the scroll for this build: follow the rows it was on across a reload, settle
    /// the cursor when its row went away, bring the cursor or composer into view when asked,
    /// clamp to the content, and remember the rows it now stands on.
    fn settle(&mut self, outline: &Outline, rows: &[Row], body_h: f32, scale: f32) {
        let spots: Vec<Option<Spot>> = outline
            .items
            .iter()
            .map(|&item| self.item_spot(outline, item))
            .collect();
        let first_row = |spot: &Spot| {
            rows.iter()
                .position(|r| spots[r.item].as_ref() == Some(spot))
        };
        if std::mem::take(&mut self.reanchor) {
            if let Some(at) = self
                .anchors
                .iter()
                .find_map(|(spot, offset)| first_row(spot).map(|r| r + offset))
            {
                self.scroll = at;
            }
        }
        let focusable: Vec<&Spot> = spots.iter().flatten().collect();
        if let Some(at) = self.cursor_index(&focusable) {
            self.cursor = Some(focusable[at].clone());
            self.cursor_at = at;
            self.cursor_before = at.checked_sub(1).map(|b| focusable[b].clone());
        }
        let fit = |last: usize| top_for(outline, rows, last, body_h, scale);
        let span = match self.reveal.take() {
            Some(Reveal::Cursor) => self.cursor.as_ref().and_then(first_row).map(|r| (r, r)),
            Some(Reveal::Composer) => {
                let mut on = rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| outline.items[r.item] == Item::Composer && r.part != Part::Gap)
                    .map(|(i, _)| i);
                on.next()
                    .map(|first| (first, on.next_back().unwrap_or(first)))
            }
            None => None,
        };
        if let Some((first, last)) = span {
            self.scroll = self.scroll.max(fit(last).min(first)).min(first);
        }
        self.scroll = self.scroll.min(fit(rows.len().saturating_sub(1)));
        self.anchors = self.anchors_at(outline, rows, &spots);
    }

    /// Where the scroll stands, by identity: the nearest focusable row at or above the top,
    /// and the header of the file the top is in, each with how many rows below it the top is.
    fn anchors_at(
        &self,
        outline: &Outline,
        rows: &[Row],
        spots: &[Option<Spot>],
    ) -> Vec<(Spot, usize)> {
        let Some(top) = rows.get(self.scroll) else {
            return Vec::new();
        };
        // Offsets count from an item's first row, which is where a reload finds it again.
        let mut anchors: Vec<(Spot, usize)> = (0..=self.scroll)
            .rev()
            .find(|&r| spots[rows[r].item].is_some())
            .and_then(|r| {
                let item = rows[r].item;
                let first = (0..=r).rev().take_while(|&f| rows[f].item == item).last()?;
                Some((spots[item].clone()?, self.scroll - first))
            })
            .into_iter()
            .collect();
        if let Some(si) = top.section {
            let head = (0..=self.scroll)
                .rev()
                .find(|&r| outline.items[rows[r].item] == Item::File(si));
            if let Some(head) = head {
                let path = outline.sections[si].path.clone();
                anchors.push((Spot::File(path), self.scroll - head));
            }
        }
        anchors
    }

    /// Paint the rows from the scroll down that fit in `body` (top, bottom), under the
    /// header of the file the top row is in when that header has scrolled away.
    fn paint_rows(
        &mut self,
        outline: &Outline,
        rows: &[Row],
        ctx: &GCtx,
        measure: &mut TextMeasure,
        body: (f32, f32),
        out: &mut Out,
    ) {
        let scale = ctx.scale;
        let mut y = body.0;
        if let Some(si) = sticky_section(outline, rows, self.scroll) {
            self.paint_file_head(out, ctx, measure, &outline.sections[si], y);
            y += FILE_HEAD_H * scale;
        }
        let mut shown = 0;
        for row in rows.iter().skip(self.scroll) {
            let h = row.h * scale;
            if y + h > body.1 + 0.5 {
                break;
            }
            self.paint_row(outline, row, ctx, measure, y, out);
            y += h;
            shown += 1;
        }
        self.page = shown;
    }

    /// Paint one row at `y`.
    fn paint_row(
        &self,
        outline: &Outline,
        row: &Row,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        y: f32,
        out: &mut Out,
    ) {
        match outline.items[row.item] {
            Item::General => push_row(
                &mut out.labels,
                measure,
                "ON THE WHOLE CHANGE",
                FontRole::Micro,
                ctx.theme.fg_muted,
                ctx.cx,
                y,
                LABEL_H,
                ctx.scale,
            ),
            Item::File(si) => self.paint_file_head(out, ctx, measure, &outline.sections[si], y),
            Item::Note(si) => self.paint_note(out, ctx, measure, &outline.sections[si], y),
            Item::Hunk(si, hi) => self.paint_hunk(out, ctx, measure, outline, (si, hi), y),
            Item::Line(si, hi, li) => {
                if let Some(line) = self.diff_line(outline, si, hi, li) {
                    let path = &outline.sections[si].path;
                    self.paint_line(out, ctx, measure, path, line, y);
                }
            }
            Item::Thread(t, attached) => {
                let card = Card {
                    y,
                    h: row.h * ctx.scale,
                };
                self.paint_thread_row(out, ctx, measure, (t, attached), &row.part, card);
            }
            Item::Composer => {
                let card = Card {
                    y,
                    h: row.h * ctx.scale,
                };
                self.paint_composer_row(out, ctx, measure, &row.part, card);
            }
        }
    }

    /// A file header: the stage checkbox, the fold caret, the status letter and path, then
    /// right-anchored the Viewed checkbox, the `+add -del` counts, and the comment count.
    fn paint_file_head(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        section: &Section,
        y: f32,
    ) {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let h = FILE_HEAD_H * scale;
        let full = PxRect {
            x: ctx.panel.x,
            y,
            w: ctx.panel.w,
            h,
        };
        let cursor = self.cursor == Some(Spot::File(section.path.clone()));
        let surface = if cursor {
            theme.accent_subtle_on(theme.bg_surface)
        } else {
            theme.bg_surface
        };
        out.quads.push(ChromeQuad::fill(full, surface));
        out.quads.push(ChromeQuad::fill(
            PxRect {
                h: scale.max(1.0),
                ..full
            },
            theme.border_subtle,
        ));
        if cursor {
            push_cursor_bar(out, ctx, y, h);
        }
        // The whole header folds; the checkboxes on it are pushed after, so they are on top.
        out.targets.push((full, Target::Fold(section.path.clone())));
        let change = section.change.map(|i| &self.data.changes[i]);
        let mut x = ctx.cx;
        if change.is_some() {
            let staged = self.status_of(&section.path).is_some_and(|f| f.staged);
            let (check, color) = if staged {
                ("[x]", theme.diff_add)
            } else {
                ("[ ]", theme.fg_muted)
            };
            push_row(
                &mut out.labels,
                measure,
                check,
                FontRole::Mono,
                color,
                x,
                y,
                FILE_HEAD_H,
                scale,
            );
            let w = measure.width("[x]", FontRole::Mono, None);
            let target = Target::Stage(section.path.clone());
            let rect = PxRect {
                x: x - 3.0 * scale,
                y,
                w: w + 6.0 * scale,
                h,
            };
            out.targets.push((rect, target));
            x += measure.width("[x] ", FontRole::Mono, None);
        }
        let caret = if section.folded {
            CHEVRON_RIGHT
        } else {
            CHEVRON_DOWN
        };
        push_row(
            &mut out.labels,
            measure,
            caret,
            FontRole::Icon,
            theme.fg_muted,
            x,
            y,
            FILE_HEAD_H,
            scale,
        );
        x += measure.width(CHEVRON_RIGHT, FontRole::Icon, None) + 6.0 * scale;
        if let Some(change) = change {
            let code = change.status.code().to_string();
            push_row(
                &mut out.labels,
                measure,
                &code,
                FontRole::Mono,
                status_color(theme, change.status),
                x,
                y,
                FILE_HEAD_H,
                scale,
            );
            x += measure.width("M ", FontRole::Mono, None);
        }
        let right = self.paint_file_controls(out, ctx, measure, section, change, y);
        let role = FontRole::Label;
        out.labels.push(ProseLabel {
            text: section.path.clone(),
            x,
            y: y + (h - measure.line_height(role)) * 0.5,
            role,
            color: theme.fg_primary,
            weight: None,
            max_w: (right - 8.0 * scale - x).max(1.0),
        });
    }

    /// A file header's right-anchored controls; returns where they start, for the path to
    /// stop before.
    fn paint_file_controls(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        section: &Section,
        change: Option<&FileChange>,
        y: f32,
    ) -> f32 {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let mut right = ctx.cr;
        if let Some(change) = change {
            let bx = self.push_viewed(out, ctx, measure, section, change, y);
            right = bx - 12.0 * scale;
            if change.skipped.is_none() {
                right = push_counts(
                    &mut out.labels,
                    measure,
                    ctx,
                    change.diff.stats(),
                    right,
                    y,
                    FILE_HEAD_H,
                ) - 10.0 * scale;
            }
        }
        let on = self
            .threads()
            .iter()
            .filter(|t| t.anchor.as_ref().is_some_and(|a| a.path == section.path));
        let (count, open) = on.fold((0, 0), |(n, o), t| {
            (n + 1, o + usize::from(t.status == Open::Open))
        });
        if count > 0 {
            let color = if open > 0 {
                theme.accent
            } else {
                theme.fg_muted
            };
            let left = push_right(
                &mut out.labels,
                measure,
                &count.to_string(),
                FontRole::Caption,
                color,
                right,
                y,
                FILE_HEAD_H,
                scale,
            );
            right = push_right(
                &mut out.labels,
                measure,
                COMMENT_ICON,
                FontRole::Icon,
                color,
                left - 4.0 * scale,
                y,
                FILE_HEAD_H,
                scale,
            ) - 10.0 * scale;
        }
        right
    }

    /// A file header's Viewed checkbox and label, right-anchored; returns the box's left edge.
    fn push_viewed(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        section: &Section,
        change: &FileChange,
        y: f32,
    ) -> f32 {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let viewed = self.viewed(change);
        let target = Target::Viewed(section.path.clone());
        let hover = self.hover.as_ref() == Some(&target);
        let color = if viewed || hover {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        let text_x = push_right(
            &mut out.labels,
            measure,
            "Viewed",
            FontRole::Caption,
            color,
            ctx.cr,
            y,
            FILE_HEAD_H,
            scale,
        );
        let side = CHECK * scale;
        let rect = PxRect {
            x: text_x - 6.0 * scale - side,
            y: y + (FILE_HEAD_H * scale - side) * 0.5,
            w: side,
            h: side,
        };
        if viewed {
            out.quads
                .push(ChromeQuad::rounded(rect, theme.accent, 3.0 * scale));
            let tick = "\u{2713}";
            let tw = measure.width(tick, FontRole::Mono, None);
            out.labels.push(ProseLabel {
                text: tick.to_owned(),
                x: rect.x + (side - tw) * 0.5,
                y: rect.y + (side - measure.line_height(FontRole::Mono)) * 0.5,
                role: FontRole::Mono,
                color: theme.bg_base.to_srgb(),
                weight: None,
                max_w: f32::MAX,
            });
        } else {
            let ring = if hover {
                theme.border_strong
            } else {
                theme.border
            };
            push_ringed(
                &mut out.quads,
                rect,
                ring,
                theme.bg_surface,
                3.0 * scale,
                scale,
            );
        }
        let hit = PxRect {
            x: rect.x - 4.0 * scale,
            y,
            w: ctx.cr - rect.x + 4.0 * scale,
            h: FILE_HEAD_H * scale,
        };
        out.targets.push((hit, target));
        rect.x
    }

    /// The line under a file header saying why its lines are not shown.
    fn paint_note(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        section: &Section,
        y: f32,
    ) {
        let change = section.change.map(|i| &self.data.changes[i]);
        let note = match change {
            None => "Not changed - comments on the file as it is",
            Some(c) => match c.skipped {
                Some(Skipped::Binary) => "Binary file - not shown",
                Some(Skipped::TooLarge) => "Large file - not shown",
                Some(Skipped::Unreadable) => "The file could not be read",
                None if matches!(c.status, FileStatus::Added | FileStatus::Untracked) => {
                    "Empty file"
                }
                None if c.status == FileStatus::Renamed => "Renamed without changes",
                None => "No line changes",
            },
        };
        push_row(
            &mut out.labels,
            measure,
            note,
            FontRole::Caption,
            ctx.theme.fg_muted,
            ctx.cx + ADD_W * ctx.scale,
            y,
            NOTE_H,
            ctx.scale,
        );
    }

    /// A hunk's `@@` header: a `diff.hunk` wash, the header text, and - when hovered or the
    /// cursor is in it - its stage / unstage button.
    fn paint_hunk(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        outline: &Outline,
        (si, hi): (usize, usize),
        y: f32,
    ) {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let section = &outline.sections[si];
        let Some(hunk) = section
            .change
            .and_then(|i| self.data.changes[i].diff.hunks.get(hi))
        else {
            return;
        };
        let h = DIFF_ROW_H * scale;
        let full = PxRect {
            x: ctx.panel.x,
            y,
            w: ctx.panel.w,
            h,
        };
        let base = theme.bg_base.to_srgb();
        out.quads.push(ChromeQuad::fill(
            full,
            theme.diff_hunk.over(base, HUNK_BG_ALPHA),
        ));
        let (old_count, new_count) = hunk.counts();
        let mut header = format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, old_count, hunk.new_start, new_count
        );
        if !hunk.heading.is_empty() {
            header.push(' ');
            header.push_str(&hunk.heading);
        }
        let target = Target::Hunk {
            path: section.path.clone(),
            hunk: hi,
        };
        let here = self.hover.as_ref() == Some(&target)
            || self.cursor_hunk() == Some((section.path.clone(), hi));
        let staged = self
            .status_of(&section.path)
            .is_some_and(|f| f.staged && !f.unstaged);
        let hint = if staged {
            "unstage \u{2318}\u{21a9}"
        } else {
            "stage \u{2318}\u{21a9}"
        };
        let hint_w = measure.width(hint, FontRole::Micro, None);
        let hint_x = ctx.cr - hint_w;
        out.labels.push(ProseLabel {
            text: header,
            x: ctx.cx,
            y: y + (h - measure.line_height(FontRole::Mono)) * 0.5,
            role: FontRole::Mono,
            color: theme.diff_hunk,
            weight: None,
            max_w: (if here { hint_x - 8.0 * scale } else { ctx.cr } - ctx.cx).max(1.0),
        });
        if here {
            push_row(
                &mut out.labels,
                measure,
                hint,
                FontRole::Micro,
                theme.accent,
                hint_x,
                y,
                DIFF_ROW_H,
                scale,
            );
        }
        let rect = PxRect {
            x: hint_x - 6.0 * scale,
            y,
            w: ctx.cr - hint_x + 6.0 * scale,
            h,
        };
        out.targets.push((rect, target));
    }

    /// One diff line: its add/del background, the `+` comment button when hovered or under
    /// the cursor, the right-aligned gutter number, and the code.
    fn paint_line(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        path: &str,
        line: &DiffLine,
        y: f32,
    ) {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let h = DIFF_ROW_H * scale;
        let full = PxRect {
            x: ctx.panel.x,
            y,
            w: ctx.panel.w,
            h,
        };
        // Diff-row backgrounds pre-composite their translucent hue over the dock's bg.base
        // backing in sRGB space (the guide's CSS weight), not the brighter linear-space blend.
        let base = theme.bg_base.to_srgb();
        let (fg, sign, bg) = match line.kind {
            LineKind::Add => (theme.diff_add, '+', Some(theme.diff_add)),
            LineKind::Del => (theme.diff_del, '-', Some(theme.diff_del)),
            LineKind::Context => (theme.fg_secondary, ' ', None),
        };
        if let Some(bg) = bg {
            out.quads
                .push(ChromeQuad::fill(full, bg.over(base, DIFF_BG_ALPHA)));
        }
        let spot = line_spot(line).map(|(side, number)| Spot::Line {
            path: path.to_owned(),
            side,
            line: number,
        });
        let cursor = spot.is_some() && self.cursor == spot;
        if cursor {
            push_cursor_bar(out, ctx, y, h);
        }
        let gutter_x = ctx.cx + ADD_W * scale;
        let gutter_w = measure.width(&"0".repeat(GUTTER_CHARS), FontRole::Mono, None);
        let number = match line.kind {
            LineKind::Del => line.old_no,
            LineKind::Add | LineKind::Context => line.new_no,
        };
        if let Some(number) = number {
            let color = if sign == ' ' { theme.fg_muted } else { fg };
            push_right(
                &mut out.labels,
                measure,
                &number.to_string(),
                FontRole::Mono,
                color,
                gutter_x + gutter_w,
                y,
                DIFF_ROW_H,
                scale,
            );
        }
        let code_x = gutter_x + gutter_w + measure.width("  ", FontRole::Mono, None);
        out.labels.push(ProseLabel {
            text: format!("{sign} {}", line.text),
            x: code_x,
            y: y + (h - measure.line_height(FontRole::Mono)) * 0.5,
            role: FontRole::Mono,
            color: fg,
            weight: None,
            max_w: (ctx.cr - code_x).max(1.0),
        });
        let Some(spot) = spot else {
            return;
        };
        out.targets.push((full, Target::Row(spot.clone())));
        if !self.can_comment() {
            return;
        }
        let add = Target::Comment(spot.clone());
        let hovered =
            matches!(&self.hover, Some(Target::Row(s) | Target::Comment(s)) if *s == spot);
        if hovered || cursor {
            let side = (ADD_W - 2.0) * scale;
            let rect = PxRect {
                x: ctx.cx,
                y: y + (h - side) * 0.5,
                w: side,
                h: side,
            };
            let button = if self.hover.as_ref() == Some(&add) {
                theme.accent_hover
            } else {
                theme.accent
            };
            out.quads
                .push(ChromeQuad::rounded(rect, button, 4.0 * scale));
            let pw = measure.width("+", FontRole::Mono, None);
            out.labels.push(ProseLabel {
                text: "+".to_owned(),
                x: rect.x + (side - pw) * 0.5,
                y: rect.y + (side - measure.line_height(FontRole::Mono)) * 0.5,
                role: FontRole::Mono,
                color: theme.bg_base.to_srgb(),
                weight: Some(700),
                max_w: f32::MAX,
            });
        }
        let hit = PxRect {
            x: ctx.panel.x,
            y,
            w: gutter_x - ctx.panel.x,
            h,
        };
        out.targets.push((hit, add));
    }

    /// One row of a thread card.
    fn paint_thread_row(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        (t, attached): (usize, bool),
        part: &Part,
        card: Card,
    ) {
        let theme = ctx.theme;
        let thread = &self.threads()[t];
        let resolved = thread.status == Open::Resolved;
        if *part == Part::Gap {
            return;
        }
        let cursor = *part == Part::Head && self.cursor == Some(Spot::Thread(thread.id));
        let fill = if cursor {
            theme.accent_subtle_on(theme.bg_surface)
        } else {
            theme.bg_surface
        };
        let bar = if resolved { theme.border } else { theme.accent };
        push_card(out, ctx, card, fill, bar);
        let x = ctx.cx + CARD_PAD * ctx.scale;
        match part {
            Part::Head => self.paint_thread_head(out, ctx, measure, (t, attached), card),
            Part::Quote => {
                let text = thread
                    .anchor
                    .as_ref()
                    .map_or_else(String::new, |a| format!("> {}", a.text));
                push_clipped(
                    out,
                    measure,
                    &text,
                    FontRole::Mono,
                    theme.fg_muted,
                    x,
                    ctx.cr - CARD_PAD * ctx.scale,
                    card,
                );
            }
            Part::Author(m) => {
                let author = thread.messages.get(*m).map_or(Author::Human, |m| m.author);
                let (name, color) = match author {
                    Author::Human => ("You", theme.fg_secondary),
                    Author::Agent => ("Agent", theme.accent),
                };
                let line_h = measure.line_height(FontRole::Caption);
                out.labels.push(ProseLabel {
                    text: name.to_owned(),
                    x,
                    y: card.y + card.h - line_h,
                    role: FontRole::Caption,
                    color,
                    weight: Some(600),
                    max_w: f32::MAX,
                });
            }
            Part::Text(text) => {
                let color = if resolved {
                    theme.fg_secondary
                } else {
                    theme.fg_primary
                };
                push_clipped(
                    out,
                    measure,
                    text,
                    FontRole::Body,
                    color,
                    x,
                    ctx.cr - CARD_PAD * ctx.scale,
                    card,
                );
            }
            Part::Draft { .. } | Part::Problem | Part::Foot | Part::Gap | Part::Whole => {}
        }
    }

    /// A thread card's header: its number, where it is, whether it is resolved, and its
    /// Reply / Resolve-or-Reopen / Delete buttons.
    fn paint_thread_head(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        (t, attached): (usize, bool),
        card: Card,
    ) {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let thread = &self.threads()[t];
        let id = thread.id;
        // The header takes the cursor; its buttons are pushed after, so they are on top.
        let row = PxRect {
            x: ctx.cx,
            y: card.y,
            w: ctx.cr - ctx.cx,
            h: card.h,
        };
        out.targets.push((row, Target::Row(Spot::Thread(id))));
        let buttons = if self.can_comment() {
            let toggle = if thread.status == Open::Open {
                ("Resolve", Target::Resolve(id), ButtonStyle::Plain)
            } else {
                ("Reopen", Target::Reopen(id), ButtonStyle::Plain)
            };
            vec![
                ("Reply", Target::Reply(id), ButtonStyle::Plain),
                toggle,
                ("Delete", Target::Delete(id), ButtonStyle::Danger),
            ]
        } else {
            Vec::new()
        };
        let mut right = ctx.cr - CARD_PAD * scale * 0.5;
        for (text, target, style) in buttons.into_iter().rev() {
            right = self.push_button(out, ctx, measure, (text, target, style), right, card)
                - BUTTON_GAP * scale;
        }
        let mut x = ctx.cx + CARD_PAD * scale;
        let number = format!("#{id}");
        push_row(
            &mut out.labels,
            measure,
            &number,
            FontRole::Mono,
            theme.fg_muted,
            x,
            card.y,
            CARD_HEAD_H,
            scale,
        );
        x += measure.width(&number, FontRole::Mono, None) + 8.0 * scale;
        let place = match (&thread.anchor, self.location(t)) {
            // Under the "on the whole change" heading, which already says where it is.
            (None, _) => String::new(),
            (Some(_), Location::Outdated) => "Outdated - the line has changed".to_owned(),
            (Some(a), location) => {
                let line = match location {
                    Location::At(line) => line,
                    Location::Whole | Location::Outdated => a.line,
                };
                let side = if a.side == Side::Old {
                    "old line"
                } else {
                    "line"
                };
                if attached {
                    format!("{side} {line}")
                } else {
                    format!("{side} {line}, outside the diff")
                }
            }
        };
        let place = if thread.status == Open::Resolved {
            format!("{place} \u{b7} Resolved")
        } else {
            place
        };
        let color = if thread.status == Open::Resolved {
            theme.diff_add
        } else {
            theme.fg_secondary
        };
        let role = FontRole::Caption;
        out.labels.push(ProseLabel {
            text: place,
            x,
            y: card.y + (card.h - measure.line_height(role)) * 0.5,
            role,
            color,
            weight: None,
            max_w: (right - x).max(1.0),
        });
    }

    /// One row of the composer card.
    fn paint_composer_row(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        part: &Part,
        card: Card,
    ) {
        let Some(composer) = self.composer.as_ref() else {
            return;
        };
        if *part == Part::Gap {
            return;
        }
        let (scale, theme) = (ctx.scale, ctx.theme);
        let focused = self.focus == Focus::Composer;
        push_card(
            out,
            ctx,
            card,
            theme.bg_inset,
            if focused {
                theme.accent
            } else {
                theme.border_strong
            },
        );
        let x = ctx.cx + CARD_PAD * scale;
        let right = ctx.cr - CARD_PAD * scale;
        match part {
            Part::Head => {
                let head = match &composer.target {
                    Compose::Comment(a) if a.side == Side::Old => {
                        format!("Comment on old line {}", a.line)
                    }
                    Compose::Comment(a) => format!("Comment on line {}", a.line),
                    Compose::Reply(id) => format!("Reply to #{id}"),
                };
                push_row(
                    &mut out.labels,
                    measure,
                    &head,
                    FontRole::Caption,
                    theme.fg_secondary,
                    x,
                    card.y,
                    CARD_HEAD_H,
                    scale,
                );
                push_right(
                    &mut out.labels,
                    measure,
                    "for the agent",
                    FontRole::Caption,
                    theme.fg_muted,
                    right,
                    card.y,
                    CARD_HEAD_H,
                    scale,
                );
            }
            Part::Draft { text, last } => {
                self.paint_draft(out, ctx, measure, (text, *last), card);
            }
            Part::Problem => {
                let problem = composer.problem.as_deref().unwrap_or_default();
                push_clipped(
                    out,
                    measure,
                    problem,
                    FontRole::Caption,
                    theme.diff_del,
                    x,
                    right,
                    card,
                );
            }
            Part::Foot => self.paint_composer_foot(out, ctx, measure, card),
            Part::Quote | Part::Author(_) | Part::Text(_) | Part::Gap | Part::Whole => {}
        }
    }

    /// One line of the composer's draft (or its placeholder), with the caret after the last
    /// line while it has focus.
    fn paint_draft(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        (text, last): (&str, bool),
        card: Card,
    ) {
        let Some(composer) = self.composer.as_ref() else {
            return;
        };
        let (scale, theme) = (ctx.scale, ctx.theme);
        let focused = self.focus == Focus::Composer;
        let x = ctx.cx + CARD_PAD * scale;
        let right = ctx.cr - CARD_PAD * scale;
        if composer.text.is_empty() {
            let hint = match composer.target {
                Compose::Comment(_) => "Ask the agent for a change",
                Compose::Reply(_) => "Write a reply",
            };
            push_clipped(
                out,
                measure,
                hint,
                FontRole::Body,
                theme.fg_muted,
                x,
                right,
                card,
            );
        } else {
            push_clipped(
                out,
                measure,
                text,
                FontRole::Body,
                theme.fg_primary,
                x,
                right,
                card,
            );
        }
        if focused && last {
            let line_h = measure.line_height(FontRole::Body);
            let caret_x = x + measure.width(text, FontRole::Body, None);
            out.quads.push(ChromeQuad::fill(
                PxRect {
                    x: caret_x.min(right),
                    y: card.y + (card.h - line_h) * 0.5,
                    w: (2.0 * scale).max(1.0),
                    h: line_h,
                },
                theme.accent,
            ));
        }
        out.targets.push((
            PxRect {
                x: ctx.cx,
                y: card.y,
                w: ctx.cr - ctx.cx,
                h: card.h,
            },
            Target::Compose,
        ));
    }

    /// The composer's foot: the new-line hint, and its send and Cancel buttons.
    fn paint_composer_foot(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        card: Card,
    ) {
        let Some(composer) = self.composer.as_ref() else {
            return;
        };
        let (scale, theme) = (ctx.scale, ctx.theme);
        let x = ctx.cx + CARD_PAD * scale;
        let right = ctx.cr - CARD_PAD * scale;
        push_row(
            &mut out.labels,
            measure,
            "\u{21a9} send  \u{21e7}\u{21a9} new line",
            FontRole::Micro,
            theme.fg_muted,
            x,
            card.y,
            CARD_FOOT_H,
            scale,
        );
        let send = match composer.target {
            Compose::Comment(_) => "Comment",
            Compose::Reply(_) => "Reply",
        };
        let style = if composer.text.trim().is_empty() {
            ButtonStyle::Disabled
        } else {
            ButtonStyle::Primary
        };
        let left = self.push_button(
            out,
            ctx,
            measure,
            (send, Target::Send, style),
            right + CARD_PAD * scale * 0.5,
            card,
        );
        self.push_button(
            out,
            ctx,
            measure,
            ("Cancel", Target::Cancel, ButtonStyle::Plain),
            left - BUTTON_GAP * scale,
            card,
        );
    }

    /// A small button ending at `right`, vertically centered in `card`; returns its left
    /// edge.
    fn push_button(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        (text, target, style): (&str, Target, ButtonStyle),
        right: f32,
        card: Card,
    ) -> f32 {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let role = FontRole::Caption;
        let w = measure.width(text, role, None) + 2.0 * BUTTON_PAD_X * scale;
        let rect = PxRect {
            x: right - w,
            y: card.y + (card.h - BUTTON_H * scale) * 0.5,
            w,
            h: BUTTON_H * scale,
        };
        let hover = self.hover.as_ref() == Some(&target);
        let radius = BUTTON_RADIUS * scale;
        let color = match style {
            ButtonStyle::Primary => {
                let fill = if hover {
                    theme.accent_hover
                } else {
                    theme.accent
                };
                out.quads.push(ChromeQuad::rounded(rect, fill, radius));
                theme.bg_base.to_srgb()
            }
            ButtonStyle::Disabled => {
                push_ringed(
                    &mut out.quads,
                    rect,
                    theme.border_subtle,
                    theme.bg_inset,
                    radius,
                    scale,
                );
                theme.fg_faint
            }
            ButtonStyle::Plain | ButtonStyle::Danger => {
                let fill = if hover {
                    theme.bg_elevated
                } else {
                    theme.bg_surface
                };
                push_ringed(
                    &mut out.quads,
                    rect,
                    theme.border_subtle,
                    fill,
                    radius,
                    scale,
                );
                match (style, hover) {
                    (ButtonStyle::Danger, _) => theme.diff_del,
                    (_, true) => theme.fg_primary,
                    _ => theme.fg_secondary,
                }
            }
        };
        out.labels.push(ProseLabel {
            text: text.to_owned(),
            x: rect.x + BUTTON_PAD_X * scale,
            y: rect.y + (rect.h - measure.line_height(role)) * 0.5,
            role,
            color,
            weight: None,
            max_w: f32::MAX,
        });
        if style != ButtonStyle::Disabled {
            out.targets.push((rect, target));
        }
        rect.x
    }

    /// The commit box at the foot: a divider, the `> message` input (with an accent caret when
    /// focused), and a status line (the just-committed Undo hint, or the staged count + hint).
    fn push_commit(&self, out: &mut Out, ctx: &GCtx, measure: &mut TextMeasure, top: f32) {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let focused = self.focus == Focus::Commit;
        out.quads.push(ChromeQuad::fill(
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
            &mut out.labels,
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
        let (text, color) = if self.message.is_empty() && !focused {
            ("commit message", theme.fg_muted)
        } else {
            (self.message.as_str(), theme.fg_primary)
        };
        push_row(
            &mut out.labels,
            measure,
            text,
            FontRole::Body,
            color,
            msg_x,
            iy,
            COMMIT_ROW_H,
            scale,
        );
        if focused {
            let caret_x = msg_x + measure.width(&self.message, FontRole::Body, None);
            let line_h = measure.line_height(FontRole::Body);
            out.quads.push(ChromeQuad::fill(
                PxRect {
                    x: caret_x,
                    y: iy + (COMMIT_ROW_H * scale - line_h) * 0.5,
                    w: (2.0 * scale).max(1.0),
                    h: line_h,
                },
                theme.accent,
            ));
        }
        out.targets.push((
            PxRect {
                x: ctx.panel.x,
                y: iy,
                w: ctx.panel.w,
                h: COMMIT_ROW_H * scale,
            },
            Target::Message,
        ));
        self.push_commit_status(out, ctx, measure, iy + COMMIT_ROW_H * scale, focused);
    }

    /// The commit box's status line: the just-committed Undo hint, or the staged count + a
    /// context hint.
    fn push_commit_status(
        &self,
        out: &mut Out,
        ctx: &GCtx,
        measure: &mut TextMeasure,
        sy: f32,
        focused: bool,
    ) {
        let (scale, theme) = (ctx.scale, ctx.theme);
        let labels = &mut out.labels;
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
            return;
        }
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
        let hint = match (focused, self.can_commit()) {
            (true, true) => "enter commit  esc back",
            (true, false) => "esc back",
            (false, _) => "tab to write a message",
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

impl Default for GitDock {
    fn default() -> Self {
        Self::new()
    }
}

/// The dock's proportional display list: the content quads + the positioned labels.
pub(crate) struct Paint {
    /// The content quads over the dock frame.
    pub(crate) quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub(crate) labels: Vec<ProseLabel>,
}

/// What a build collects: quads, labels, and hit targets.
#[derive(Default)]
struct Out {
    quads: Vec<ChromeQuad>,
    labels: Vec<ProseLabel>,
    targets: Vec<(PxRect, Target)>,
}

/// Shared geometry for the dock builders: the panel, the content left/right x, scale, theme.
struct GCtx<'a> {
    panel: PxRect,
    cx: f32,
    cr: f32,
    scale: f32,
    theme: &'a Theme,
}

/// One file's part of the scroll.
struct Section {
    /// The file, as a review anchor names it.
    path: String,
    /// Its change, as an index into the reading's changes; `None` for a file that is only
    /// here because an open thread is on it.
    change: Option<usize>,
    /// Whether its lines are hidden.
    folded: bool,
}

/// The scroll's structure, before any text is wrapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    /// The heading over comments on the change as a whole.
    General,
    /// A file's header.
    File(usize),
    /// The line saying why a file's lines are not shown.
    Note(usize),
    /// A hunk's `@@` header: section, hunk.
    Hunk(usize, usize),
    /// A diff line: section, hunk, line.
    Line(usize, usize, usize),
    /// A thread card, and whether it sits under the line it is on.
    Thread(usize, bool),
    /// The composer card.
    Composer,
}

/// A section list and the items in scroll order.
struct Outline {
    sections: Vec<Section>,
    items: Vec<Item>,
}

/// One drawn row of the scroll.
struct Row {
    /// The outline item it draws (part of).
    item: usize,
    /// The section it is in (`None` above the first file).
    section: Option<usize>,
    /// Its height, logical px.
    h: f32,
    /// Which part of the item.
    part: Part,
}

/// Which part of an item a row draws.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    /// All of it (a header, note, hunk header, or diff line).
    Whole,
    /// A card's header.
    Head,
    /// The line an off-diff thread is about, quoted.
    Quote,
    /// Who wrote message `n`.
    Author(usize),
    /// One wrapped line of a message.
    Text(String),
    /// One wrapped line of the composer's draft, and whether the caret ends it.
    Draft {
        /// The line.
        text: String,
        /// Whether it is the draft's last line.
        last: bool,
    },
    /// Why the composer's last send failed.
    Problem,
    /// The composer's buttons.
    Foot,
    /// The space under a card.
    Gap,
}

/// A card row's vertical extent, physical px.
#[derive(Clone, Copy)]
struct Card {
    y: f32,
    h: f32,
}

/// How a button draws.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonStyle {
    Plain,
    Danger,
    Primary,
    Disabled,
}

/// The side and number a comment on `line` anchors to: the old side for a removed line, the
/// new side otherwise - one gutter number per line, as in AI Team.
fn line_spot(line: &DiffLine) -> Option<(Side, u32)> {
    match line.kind {
        LineKind::Del => line.old_no.map(|n| (Side::Old, n)),
        LineKind::Add | LineKind::Context => line.new_no.map(|n| (Side::New, n)),
    }
}

/// The longest line in a change's diff, in bytes.
fn longest_line(change: &FileChange) -> usize {
    change
        .diff
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .map(|l| l.text.len())
        .max()
        .unwrap_or(0)
}

/// The section whose header pins over the scroll when row `from` is at the top: the one it
/// is in, unless it is that section's own header.
fn sticky_section(outline: &Outline, rows: &[Row], from: usize) -> Option<usize> {
    let row = rows.get(from)?;
    let si = row.section?;
    (outline.items[row.item] != Item::File(si)).then_some(si)
}

/// The first row the scroll can start at with every row through `last` in `body_h`
/// (physical px), counting the file header pinned over them; `last` itself when not even it
/// fits. Walks up from `last` only as far as a page.
fn top_for(outline: &Outline, rows: &[Row], last: usize, body_h: f32, scale: f32) -> usize {
    let mut top = last;
    let mut sum = 0.0;
    for from in (0..=last.min(rows.len().saturating_sub(1))).rev() {
        sum += rows[from].h;
        let pinned = if sticky_section(outline, rows, from).is_some() {
            FILE_HEAD_H
        } else {
            0.0
        };
        if (sum + pinned) * scale > body_h + 0.5 {
            break;
        }
        top = from;
    }
    top
}

/// The color a status letter draws in.
fn status_color(theme: &Theme, status: FileStatus) -> Srgb {
    match status {
        FileStatus::Added | FileStatus::Untracked => theme.diff_add,
        FileStatus::Deleted => theme.diff_del,
        FileStatus::Modified | FileStatus::Renamed | FileStatus::Copied => theme.diff_hunk,
        FileStatus::TypeChange | FileStatus::Unmerged => theme.fg_secondary,
    }
}

/// `at` moved by `delta`, clamped to `0..len`.
fn step(at: usize, delta: i32, len: usize) -> usize {
    step_unclamped(at, delta).min(len.saturating_sub(1))
}

/// `at` moved by `delta`, floored at zero.
fn step_unclamped(at: usize, delta: i32) -> usize {
    let magnitude = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
    if delta < 0 {
        at.saturating_sub(magnitude)
    } else {
        at.saturating_add(magnitude)
    }
}

/// `text` broken into lines no wider than `width` in `role`: at its newlines, then between
/// words, and inside a word too long for a line of its own. Spaces are kept, so a caret after
/// the last line lands where the text ends. Always at least one line.
fn wrap(text: &str, role: FontRole, width: f32, measure: &mut TextMeasure) -> Vec<String> {
    let space = measure.width(" ", role, None);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut line_w = 0.0;
        for (i, word) in paragraph.split(' ').enumerate() {
            let word_w = measure.width(word, role, None);
            let gap = if i == 0 { 0.0 } else { space };
            if i > 0 && line_w + gap + word_w > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                line_w = 0.0;
            } else if i > 0 {
                line.push(' ');
                line_w += gap;
            }
            if word_w <= width {
                line.push_str(word);
                line_w += word_w;
                continue;
            }
            // A word wider than a line: break it wherever it runs out of room.
            for c in word.chars() {
                let c_w = measure.width(c.encode_utf8(&mut [0; 4]), role, None);
                if line_w + c_w > width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    line_w = 0.0;
                }
                line.push(c);
                line_w += c_w;
            }
        }
        lines.push(line);
    }
    lines
}

/// A card row's slice of its card: the fill across the content width and the status bar
/// down its left edge.
fn push_card(out: &mut Out, ctx: &GCtx, card: Card, fill: Srgb, bar: Srgb) {
    let rect = PxRect {
        x: ctx.cx,
        y: card.y,
        w: ctx.cr - ctx.cx,
        h: card.h,
    };
    out.quads.push(ChromeQuad::fill(rect, fill));
    out.quads.push(ChromeQuad::fill(
        PxRect {
            w: CARD_BAR * ctx.scale,
            ..rect
        },
        bar,
    ));
}

/// The keyboard cursor's accent bar down the dock's left edge.
fn push_cursor_bar(out: &mut Out, ctx: &GCtx, y: f32, h: f32) {
    out.quads.push(ChromeQuad::fill(
        PxRect {
            x: ctx.panel.x,
            y,
            w: 2.0 * ctx.scale,
            h,
        },
        ctx.theme.accent,
    ));
}

/// A label vertically centered in `card`, clipped at `right`.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_clipped(
    out: &mut Out,
    measure: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    x: f32,
    right: f32,
    card: Card,
) {
    out.labels.push(ProseLabel {
        text: text.to_owned(),
        x,
        y: card.y + (card.h - measure.line_height(role)) * 0.5,
        role,
        color,
        weight: None,
        max_w: (right - x).max(1.0),
    });
}

/// A rounded rect with a 1px ring: the ring color filled, then the fill inset by a stroke.
fn push_ringed(
    quads: &mut Vec<ChromeQuad>,
    rect: PxRect,
    ring: Srgb,
    fill: Srgb,
    radius: f32,
    scale: f32,
) {
    let stroke = scale.max(1.0);
    quads.push(ChromeQuad::rounded(rect, ring, radius));
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: rect.x + stroke,
            y: rect.y + stroke,
            w: (rect.w - 2.0 * stroke).max(0.0),
            h: (rect.h - 2.0 * stroke).max(0.0),
        },
        fill,
        (radius - stroke).max(0.0),
    ));
}

/// Push a right-anchored `+add -del` count pair ending at `right` (add in `diff.add`, del in
/// `diff.del`); returns the left edge of the pair.
fn push_counts(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    ctx: &GCtx,
    (added, removed): (u32, u32),
    right: f32,
    top: f32,
    row_h: f32,
) -> f32 {
    let end = push_right(
        labels,
        measure,
        &format!("-{removed}"),
        FontRole::Mono,
        ctx.theme.diff_del,
        right,
        top,
        row_h,
        ctx.scale,
    );
    push_right(
        labels,
        measure,
        &format!("+{added}"),
        FontRole::Mono,
        ctx.theme.diff_add,
        end - 8.0 * ctx.scale,
        top,
        row_h,
        ctx.scale,
    )
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

// Spelled out so the module resolves the same when the capture example includes this file.
#[cfg(test)]
#[path = "gitdock/tests.rs"]
mod tests;
