//! The session-timeline dock: a right dock over the live terminal (AGENTS Hard rule 4 -
//! a layer, the pane tree never unmounts; only one right-dock surface shows at a time, so
//! it is mutually exclusive with the git diff dock). Opened with `⇧⌘H` and dismissed with
//! `Esc`. This module is pure state + view-building over a [`Timeline`]: it holds the
//! event log + the selection, and turns them into a monospace grid of UI-token-colored
//! cells plus the row metadata the renderer needs (the selected row's fill and the
//! "viewing" accent bar). The binary owns recording events, the shadow-worktree rewind
//! ([`skelly_session::Repo::shadow_checkout`]), and key routing.
//!
//! v1 is a read-only inspection (ADR-0007): selecting a restorable event rewinds to it in
//! a shadow worktree (HEAD untouched, Hard rule 3); `⌥⌘0` returns to now. Event times are
//! **session-relative** elapsed labels (`M:SS` into the session) - dependency-free and
//! honest, in place of the mockup's wall-clock times (which would need a date dependency a
//! minimal terminal should not carry).

use std::time::Duration;

use skelly_render::{GridCell, Srgb, Theme};
use skelly_session::{Actor, SessionEvent, Timeline};

/// Grid row of the status / "viewing" banner.
const STATUS_ROW: usize = 0;
/// Grid row of the `TIMELINE - N` section label.
const LABEL_ROW: usize = 2;
/// First grid row of the event list.
const EVENT_START: usize = 3;
/// Rows the foot band reserves (a divider, the legend, the session summary).
const FOOT_ROWS: usize = 3;
/// Column where an event's title begins (after the `M:SS ` time column).
const TITLE_COL: usize = 7;

/// The session-timeline dock's state: the open flag, the event log, and the selected
/// event. The binary records events into [`Self::record`] and drives the rewind from the
/// selection.
pub(crate) struct TimelineDock {
    /// Whether the dock is showing (captures navigation keys while open).
    pub(crate) open: bool,
    /// The append-only session event log.
    timeline: Timeline,
    /// Index of the selected event (the cursor; defaults to the newest = now).
    selected: usize,
    /// The current branch, for the session-summary line (set when the dock opens).
    branch: Option<String>,
    /// The session's total elapsed at the last event, for the summary duration.
    duration: Duration,
}

impl TimelineDock {
    /// A closed, empty dock.
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            timeline: Timeline::new(),
            selected: 0,
            branch: None,
            duration: Duration::ZERO,
        }
    }

    /// Open the dock, snapping the selection to the newest event (now) and recording the
    /// branch for the summary line.
    pub(crate) fn open(&mut self, branch: Option<String>) {
        self.open = true;
        self.branch = branch;
        self.selected = self.timeline.newest().unwrap_or(0);
    }

    /// Close the dock.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// Record an event into the session log at `elapsed` into the session, keeping the
    /// selection pinned to the newest event unless the user has scrubbed to the past.
    pub(crate) fn record(&mut self, elapsed: Duration, event: SessionEvent) {
        self.duration = elapsed;
        let was_at_newest = self.timeline.newest().is_none_or(|n| self.selected == n);
        self.timeline.record(event);
        if was_at_newest {
            self.selected = self.timeline.newest().unwrap_or(0);
        }
    }

    /// Move the selection by `delta`, clamped to the event list. Returns `true` when it
    /// actually moved (so the binary reconciles the shadow worktree to the new selection).
    pub(crate) fn move_selection(&mut self, delta: i32) -> bool {
        let count = self.timeline.len();
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
        true
    }

    /// Select the newest event (return to now). Returns `true` if the selection moved.
    pub(crate) fn select_now(&mut self) -> bool {
        let newest = self.timeline.newest().unwrap_or(0);
        if newest == self.selected {
            return false;
        }
        self.selected = newest;
        true
    }

    /// The restorable commit for the current selection (the event's own SHA, or the nearest
    /// earlier commit's - see [`Timeline::effective_restore`]). `None` when nothing at or
    /// before the selection is restorable.
    pub(crate) fn selected_restore(&self) -> Option<String> {
        self.timeline
            .effective_restore(self.selected)
            .map(str::to_owned)
    }

    /// Whether the current selection is "now" (the newest restorable state == HEAD), so the
    /// binary discards any shadow worktree rather than checking HEAD out again.
    pub(crate) fn selection_is_now(&self) -> bool {
        self.timeline.is_now(self.selected)
    }

    /// Build the dock grid `cols` cells wide and `rows` cells tall, in `theme`'s UI tokens.
    /// Returns the grid plus the renderer's row metadata (the selected row's fill and, when
    /// viewing the past, the "viewing" accent bar row).
    pub(crate) fn view(&self, cols: usize, rows: usize, theme: &Theme) -> View {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut grid: Vec<Vec<GridCell>> =
            (0..rows).map(|_| blank_row(cols, theme.fg_muted)).collect();

        if self.timeline.is_empty() {
            center(&mut grid, "No session events yet", theme.fg_muted);
            return View::empty(grid);
        }

        self.write_status_bar(&mut grid[STATUS_ROW], cols, theme);
        if LABEL_ROW < rows {
            write(
                &mut grid[LABEL_ROW],
                0,
                &format!("TIMELINE - {} events", self.timeline.len()),
                theme.fg_muted,
            );
            write_before(&mut grid[LABEL_ROW], cols, "up down move", theme.fg_muted);
        }

        let (selected_row, viewing_row) = self.write_events(&mut grid, rows, cols, theme);
        self.write_foot(&mut grid, rows, cols, theme);

        View {
            rows: grid,
            selected_row,
            viewing_row,
        }
    }

    /// Write the event list (scrolled to keep the selection visible) into the body between
    /// [`EVENT_START`] and the foot band. Returns the selected event's grid row and, when
    /// viewing a past state, the viewing event's grid row (for the accent bar).
    fn write_events(
        &self,
        grid: &mut [Vec<GridCell>],
        rows: usize,
        cols: usize,
        theme: &Theme,
    ) -> (Option<usize>, Option<usize>) {
        let body_end = rows.saturating_sub(FOOT_ROWS);
        let visible = body_end.saturating_sub(EVENT_START);
        if visible == 0 {
            return (None, None);
        }
        let offset = scroll_window(self.timeline.len(), visible, self.selected);
        let newest = self.timeline.newest();
        let viewing_past = !self.selection_is_now();
        let mut selected_row = None;
        let mut viewing_row = None;

        for slot in 0..visible {
            let index = offset + slot;
            let Some(event) = self.timeline.events().get(index) else {
                break;
            };
            let row = EVENT_START + slot;
            let is_selected = index == self.selected;
            // Events newer than the selection are the dimmed "future" (guide); the selected
            // one is primary, the rest secondary.
            let title_fg = if index > self.selected {
                theme.fg_muted
            } else if is_selected {
                theme.fg_primary
            } else {
                theme.fg_secondary
            };
            // Badges: the newest event is HEAD/now; the selected past event is "viewing".
            let badge = if Some(index) == newest {
                Some(("now", theme.diff_hunk))
            } else if is_selected && viewing_past {
                Some(("view", theme.accent))
            } else {
                None
            };
            event_row(
                &mut grid[row],
                cols,
                event,
                title_fg,
                actor_color(event.actor, theme),
                badge,
            );
            if is_selected {
                selected_row = Some(row);
                if viewing_past {
                    viewing_row = Some(row);
                }
            }
        }
        (selected_row, viewing_row)
    }

    /// Write the status banner: whether we are at HEAD (now) or viewing a past state, plus
    /// an `esc` hint.
    fn write_status_bar(&self, row: &mut [GridCell], cols: usize, theme: &Theme) {
        if self.selection_is_now() {
            write(row, 0, "At HEAD - now", theme.diff_hunk);
        } else {
            let time = self
                .timeline
                .events()
                .get(self.selected)
                .map_or("", |e| e.time.as_str());
            let sha = self.selected_restore().unwrap_or_default();
            let short: String = sha.chars().take(7).collect();
            write(row, 0, &format!("Viewing {time} - {short}"), theme.accent);
        }
        write_before(row, cols, "esc", theme.fg_muted);
    }

    /// Write the foot band: a divider, the actor legend, and the session summary.
    fn write_foot(&self, grid: &mut [Vec<GridCell>], rows: usize, cols: usize, theme: &Theme) {
        if rows < FOOT_ROWS + EVENT_START {
            return;
        }
        let top = rows - FOOT_ROWS;
        write(&mut grid[top], 0, &"-".repeat(cols), theme.border_strong);

        // Legend: `you N  agent N  system N`, each label in its actor color.
        let (human, agent, system) = self.timeline.counts();
        let legend = &mut grid[top + 1];
        let mut col = 0;
        for (actor, count) in [
            (Actor::Human, human),
            (Actor::Agent, agent),
            (Actor::System, system),
        ] {
            let label = format!("{} {count}", actor.label());
            write(legend, col, &label, actor_color(actor, theme));
            col += label.chars().count() + 2;
        }

        // Session summary: duration, event count, and branch.
        let branch = self.branch.as_deref().unwrap_or("(detached)");
        let summary = format!(
            "Session - {} - {} events - {branch}",
            fmt_duration(self.duration),
            self.timeline.len()
        );
        write(&mut grid[top + 2], 0, &summary, theme.fg_muted);
    }
}

impl Default for TimelineDock {
    fn default() -> Self {
        Self::new()
    }
}

/// The rendered dock grid plus the renderer's row metadata.
pub(crate) struct View {
    /// The dock's lines as a grid of UI-colored cells.
    pub(crate) rows: Vec<Vec<GridCell>>,
    /// Grid row of the selected event (for the `accent.subtle` fill).
    pub(crate) selected_row: Option<usize>,
    /// Grid row of the event being viewed in the past (for the `accent` bar), when rewound.
    pub(crate) viewing_row: Option<usize>,
}

impl View {
    /// A view carrying just a grid (the empty state has no highlights).
    fn empty(rows: Vec<Vec<GridCell>>) -> Self {
        Self {
            rows,
            selected_row: None,
            viewing_row: None,
        }
    }
}

/// The accent color for an event's actor tag: human = `accent`, agent = `diff.hunk`
/// (reserved; unused in v1), system = muted.
fn actor_color(actor: Actor, theme: &Theme) -> Srgb {
    match actor {
        Actor::Human => theme.accent,
        Actor::Agent => theme.diff_hunk,
        Actor::System => theme.fg_muted,
    }
}

/// Write one event row: the session-relative time (col 0), a right-anchored actor label,
/// an optional `now`/`view` badge just left of it, and the title in between - clipped so it
/// always keeps a gap before the badge/actor region. Placing all the right-side segments
/// here keeps the title clip and the badge from ever colliding.
fn event_row(
    row: &mut [GridCell],
    cols: usize,
    event: &SessionEvent,
    title_fg: Srgb,
    actor_fg: Srgb,
    badge: Option<(&str, Srgb)>,
) {
    write(row, 0, &format!("{:>5}", event.time), title_fg);
    let actor = event.actor.label();
    let actor_start = cols.saturating_sub(actor.chars().count() + 1);
    write(row, actor_start, actor, actor_fg);
    // The badge (if any) sits just left of the actor; the title ends a gap before whichever
    // of them is leftmost.
    let right = match badge {
        Some((text, fg)) => write_before(row, actor_start, text, fg),
        None => actor_start,
    };
    let title_max = right.saturating_sub(TITLE_COL + 1);
    write_clipped(row, TITLE_COL, &event.title, title_max, title_fg);
}

/// Format a session duration as `H:MM` (or `M:SS` under an hour) for the summary line.
fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m {s:02}s")
    }
}

/// The scroll offset that keeps `anchor` visible in a `visible`-row window over `len`
/// items (centered on the anchor, clamped to the ends).
fn scroll_window(len: usize, visible: usize, anchor: usize) -> usize {
    if len <= visible {
        return 0;
    }
    anchor.saturating_sub(visible / 2).min(len - visible)
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
    let clipped: String = text.chars().take(max).collect();
    write(row, col, &clipped, fg);
}

/// Write `text` so its last cell sits just before `end`, returning the start column (for
/// chaining right-anchored segments right to left).
fn write_before(row: &mut [GridCell], end: usize, text: &str, fg: Srgb) -> usize {
    let len = text.chars().count();
    let start = end.saturating_sub(len + 1);
    write(row, start, text, fg);
    start
}

/// Write `text` centered on the middle row of `grid`.
fn center(grid: &mut [Vec<GridCell>], text: &str, fg: Srgb) {
    if grid.is_empty() {
        return;
    }
    let mid = grid.len() / 2;
    let cols = grid[mid].len();
    let start = cols.saturating_sub(text.chars().count()) / 2;
    write(&mut grid[mid], start, text, fg);
}

#[cfg(test)]
mod tests {
    use super::{TimelineDock, EVENT_START};
    use skelly_render::Theme;
    use skelly_session::{Actor, SessionEvent};
    use std::time::Duration;

    fn recorded() -> TimelineDock {
        let mut dock = TimelineDock::new();
        dock.record(
            Duration::ZERO,
            SessionEvent::new(Actor::System, "0:00", "Session started", "main")
                .restoring("aaaaaaa"),
        );
        dock.record(
            Duration::from_secs(90),
            SessionEvent::new(Actor::Human, "1:30", "Staged 2 files", "a.rs, b.rs"),
        );
        dock.record(
            Duration::from_secs(200),
            SessionEvent::new(
                Actor::Human,
                "3:20",
                "git commit - feat: x",
                "bbbbbbb - main",
            )
            .restoring("bbbbbbb"),
        );
        dock
    }

    fn joined(dock: &TimelineDock, cols: usize, rows: usize) -> String {
        let theme = Theme::resolve("ossein-dark");
        dock.view(cols, rows, &theme)
            .rows
            .iter()
            .map(|r| r.iter().map(|c| c.c).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn recording_keeps_the_selection_pinned_to_now() {
        let dock = recorded();
        // Three events recorded; the selection tracks the newest, so we are at "now".
        assert!(dock.selection_is_now());
        assert_eq!(dock.selected_restore().as_deref(), Some("bbbbbbb"));
    }

    #[test]
    fn moving_up_rewinds_to_a_past_commit_and_now_returns() {
        let mut dock = recorded();
        dock.open(Some("main".to_owned()));
        // Up to the stage event: inherits the launch HEAD, so it is a past state.
        assert!(dock.move_selection(-1));
        assert!(!dock.selection_is_now());
        assert_eq!(dock.selected_restore().as_deref(), Some("aaaaaaa"));
        // Back to now.
        assert!(dock.select_now());
        assert!(dock.selection_is_now());
    }

    #[test]
    fn view_shows_the_events_status_banner_and_legend() {
        let dock = recorded();
        let text = joined(&dock, 46, 24);
        assert!(text.contains("At HEAD - now"), "at-now banner");
        assert!(text.contains("TIMELINE - 3 events"));
        assert!(text.contains("Session started"));
        assert!(text.contains("git commit - feat: x"));
        assert!(text.contains("Session -"), "summary line");
        assert!(text.contains("you 2"), "legend human count");
    }

    #[test]
    fn view_marks_the_selected_row_and_a_viewing_bar_when_rewound() {
        let mut dock = recorded();
        dock.open(Some("main".to_owned()));
        dock.move_selection(-2); // to the oldest (session start), a past state
        let theme = Theme::resolve("ossein-dark");
        let view = dock.view(46, 24, &theme);
        assert_eq!(view.selected_row, Some(EVENT_START));
        assert_eq!(view.viewing_row, Some(EVENT_START), "rewound -> accent bar");
        let banner = view.rows[0].iter().map(|c| c.c).collect::<String>();
        assert!(banner.contains("Viewing 0:00"), "viewing banner: {banner}");
    }

    #[test]
    fn empty_timeline_shows_a_placeholder() {
        let dock = TimelineDock::new();
        let text = joined(&dock, 46, 20);
        assert!(text.contains("No session events yet"));
    }
}
