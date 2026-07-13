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

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};
use skelly_session::{Actor, SessionEvent, Timeline};

/// Timeline dock layout constants in **logical** px (multiplied by the DPI scale). Tuned to
/// the guide's §10.5 timeline: a status banner, a section label, `accent.subtle` event rows
/// (time · title · actor tag), and a foot band (legend + session summary).
const PAD_X: f32 = 14.0;
/// Top padding above the status banner.
const PAD_TOP: f32 = 12.0;
/// Status-banner row height.
const STATUS_H: f32 = 26.0;
/// Section-label row height.
const LABEL_H: f32 = 24.0;
/// Event row height.
const EVENT_H: f32 = 30.0;
/// The foot band height (a divider + the legend + the summary + bottom padding).
const FOOT_H: f32 = 60.0;
/// Gap (logical px) between an event's title and the badge / actor tag on its right.
const RIGHT_GAP: f32 = 10.0;

/// The session-timeline dock's **view state**: the open flag, the scrub cursor, and the summary
/// context. The event log itself lives on the binary (per repo, keyed by repo root), so every
/// query/view method takes the active repo's `&Timeline`; there is only ever one visible dock, so
/// a single cursor suffices (it resets to now when the active repo changes).
pub(crate) struct TimelineDock {
    /// Whether the dock is showing (captures navigation keys while open).
    pub(crate) open: bool,
    /// Index of the selected event in the active repo's timeline (the cursor; newest = now).
    selected: usize,
    /// The current branch, for the session-summary line (set when the dock opens).
    branch: Option<String>,
    /// The active repo timeline's total elapsed at its last event, for the summary duration.
    duration: Duration,
}

impl TimelineDock {
    /// A closed dock.
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            selected: 0,
            branch: None,
            duration: Duration::ZERO,
        }
    }

    /// Open the dock over `timeline` (the active repo's log), snapping the selection to now and
    /// recording the branch for the summary line.
    pub(crate) fn open(&mut self, timeline: &Timeline, branch: Option<String>) {
        self.open = true;
        self.branch = branch;
        self.selected = timeline.newest().unwrap_or(0);
    }

    /// Close the dock.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// The scrub cursor index (the binary reads it to re-pin after appending an event).
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// Set the summary duration (the active repo timeline's elapsed at its last event).
    pub(crate) fn set_duration(&mut self, duration: Duration) {
        self.duration = duration;
    }

    /// Pin the cursor to the newest event of a `len`-event timeline (used after an append when the
    /// cursor was already at now). No-op keeps it where the user scrubbed to.
    pub(crate) fn snap_to_newest(&mut self, len: usize) {
        self.selected = len.saturating_sub(1);
    }

    /// Reset the dock to now for a newly-active repo's `timeline` (snap to its newest, adopt its
    /// branch for the summary). Called when the active repo changes so each repo shows at HEAD.
    pub(crate) fn reset_to_now(&mut self, timeline: &Timeline, branch: Option<String>) {
        self.branch = branch;
        self.selected = timeline.newest().unwrap_or(0);
    }

    /// Move the selection by `delta` within `timeline`, clamped. Returns `true` when it actually
    /// moved (so the binary reconciles the shadow worktree to the new selection).
    pub(crate) fn move_selection(&mut self, timeline: &Timeline, delta: i32) -> bool {
        let count = timeline.len();
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

    /// Select the newest event of `timeline` (return to now). Returns `true` if it moved.
    pub(crate) fn select_now(&mut self, timeline: &Timeline) -> bool {
        let newest = timeline.newest().unwrap_or(0);
        if newest == self.selected {
            return false;
        }
        self.selected = newest;
        true
    }

    /// The restorable commit for the current selection in `timeline` (the event's own SHA, or the
    /// nearest earlier commit's - see [`Timeline::effective_restore`]). `None` when nothing at or
    /// before the selection is restorable.
    pub(crate) fn selected_restore(&self, timeline: &Timeline) -> Option<String> {
        timeline.effective_restore(self.selected).map(str::to_owned)
    }

    /// Whether the current selection in `timeline` is "now" (the newest restorable state == HEAD),
    /// so the binary discards any shadow worktree rather than checking HEAD out again.
    pub(crate) fn selection_is_now(&self, timeline: &Timeline) -> bool {
        timeline.is_now(self.selected)
    }

    /// Build the dock's proportional display list within `panel` (physical px) at DPI
    /// `scale`, in `theme`'s UI tokens: the status banner, the `TIMELINE - N` label, the
    /// scrolled event list (time · title · actor tag, the selected row filled + the viewed
    /// one barred), and the foot band (legend + session summary).
    pub(crate) fn build(
        &self,
        timeline: &Timeline,
        panel: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> Paint {
        let mut quads = Vec::new();
        let mut labels = Vec::new();
        let cx = panel.x + PAD_X * scale;
        let cr = panel.x + panel.w - PAD_X * scale;

        if timeline.is_empty() {
            push_centered(
                &mut labels,
                measure,
                "No session events yet",
                FontRole::Body,
                theme.fg_muted,
                panel,
                panel.y + panel.h * 0.5 - EVENT_H * scale,
                EVENT_H,
                scale,
            );
            return Paint { quads, labels };
        }

        let mut y = panel.y + PAD_TOP * scale;
        // Status banner.
        let (banner, banner_fg) = self.status_banner(timeline, theme);
        push_row(
            &mut labels,
            measure,
            &banner,
            FontRole::Label,
            banner_fg,
            cx,
            y,
            STATUS_H,
            scale,
        );
        push_right(
            &mut labels,
            measure,
            "esc",
            FontRole::Caption,
            theme.fg_muted,
            cr,
            y,
            STATUS_H,
            scale,
        );
        y += STATUS_H * scale;

        // Section label.
        push_row(
            &mut labels,
            measure,
            &format!("TIMELINE - {} EVENTS", timeline.len()),
            FontRole::Micro,
            theme.fg_muted,
            cx,
            y,
            LABEL_H,
            scale,
        );
        push_right(
            &mut labels,
            measure,
            "up down move",
            FontRole::Caption,
            theme.fg_muted,
            cr,
            y,
            LABEL_H,
            scale,
        );
        y += LABEL_H * scale;

        self.push_events(
            timeline,
            &mut quads,
            &mut labels,
            panel,
            y,
            scale,
            theme,
            measure,
        );
        self.push_foot(
            timeline,
            &mut quads,
            &mut labels,
            panel,
            scale,
            theme,
            measure,
        );
        Paint { quads, labels }
    }

    /// Lay out the scrolled event list between the header (`top`) and the foot band. Each row
    /// draws the time, the title (clipped before the badge/actor), the actor tag, and a badge;
    /// the selected row gets an `accent.subtle` fill and, when rewound, an `accent` bar.
    #[allow(clippy::too_many_arguments, reason = "one focused list builder")]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "the visible-row count + slot index are small, non-negative values"
    )]
    fn push_events(
        &self,
        timeline: &Timeline,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        panel: PxRect,
        top: f32,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) {
        let cx = panel.x + PAD_X * scale;
        let cr = panel.x + panel.w - PAD_X * scale;
        let events_bottom = panel.y + panel.h - FOOT_H * scale;
        let row_h = EVENT_H * scale;
        let visible = (((events_bottom - top) / row_h).floor().max(0.0)) as usize;
        if visible == 0 {
            return;
        }
        let offset = scroll_window(timeline.len(), visible, self.selected);
        let newest = timeline.newest();
        let viewing_past = !self.selection_is_now(timeline);
        let ctx = EventCtx {
            panel,
            cx,
            cr,
            row_h,
            time_w: measure.width("00:00", FontRole::Micro, None) + 8.0 * scale,
            scale,
            theme,
        };

        for slot in 0..visible {
            let index = offset + slot;
            let Some(event) = timeline.events().get(index) else {
                break;
            };
            let is_selected = index == self.selected;
            let title_fg = if index > self.selected {
                theme.fg_muted
            } else if is_selected {
                theme.fg_primary
            } else {
                theme.fg_secondary
            };
            // Rewind-state badges (design §10.7 timeline events): the newest restorable state is
            // `HEAD`; while rewound, the viewed row is `VIEWING` and each later row is `future`.
            let badge = if Some(index) == newest {
                Some(("HEAD", theme.diff_hunk))
            } else if is_selected && viewing_past {
                Some(("VIEWING", theme.accent))
            } else if viewing_past && index > self.selected {
                Some(("FUTURE", theme.accent))
            } else {
                None
            };
            push_event(
                quads,
                labels,
                measure,
                &ctx,
                &Ev {
                    event,
                    row_top: top + slot as f32 * row_h,
                    selected: is_selected && viewing_past,
                    fill: is_selected,
                    title_fg,
                    badge,
                },
            );
        }
    }

    /// The status banner text + color: at HEAD (now) or viewing a past state.
    fn status_banner(&self, timeline: &Timeline, theme: &Theme) -> (String, Srgb) {
        if self.selection_is_now(timeline) {
            ("At HEAD - now".to_owned(), theme.diff_hunk)
        } else {
            let time = timeline
                .events()
                .get(self.selected)
                .map_or("", |e| e.time.as_str());
            let short: String = self
                .selected_restore(timeline)
                .unwrap_or_default()
                .chars()
                .take(7)
                .collect();
            (format!("Viewing {time} - {short}"), theme.accent)
        }
    }

    /// The foot band: a `border` divider, the actor legend (each count in its actor color),
    /// and the session summary.
    #[allow(clippy::too_many_arguments, reason = "one focused foot-band builder")]
    fn push_foot(
        &self,
        timeline: &Timeline,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        panel: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) {
        let cx = panel.x + PAD_X * scale;
        let foot_top = panel.y + panel.h - FOOT_H * scale;
        quads.push(ChromeQuad::fill(
            PxRect {
                x: panel.x,
                y: foot_top,
                w: panel.w,
                h: scale.max(1.0),
            },
            theme.border,
        ));
        // Legend.
        let (human, agent, system) = timeline.counts();
        let mut lx = cx;
        let legend_y = foot_top + 8.0 * scale;
        for (actor, count) in [
            (Actor::Human, human),
            (Actor::Agent, agent),
            (Actor::System, system),
        ] {
            let label = format!("{} {count}", actor.label());
            let w = measure.width(&label, FontRole::Caption, None);
            push_row(
                labels,
                measure,
                &label,
                FontRole::Caption,
                actor_color(actor, theme),
                lx,
                legend_y,
                20.0,
                scale,
            );
            lx += w + 12.0 * scale;
        }
        // Summary.
        let branch = self.branch.as_deref().unwrap_or("(detached)");
        let summary = format!(
            "Session - {} - {} events - {branch}",
            fmt_duration(self.duration),
            timeline.len()
        );
        push_row(
            labels,
            measure,
            &summary,
            FontRole::Caption,
            theme.fg_muted,
            cx,
            legend_y + 22.0 * scale,
            20.0,
            scale,
        );
    }
}

impl Default for TimelineDock {
    fn default() -> Self {
        Self::new()
    }
}

/// The dock's proportional display list: the content quads (selected-event fill + the
/// viewing accent bar) and the positioned labels. The renderer draws the dock frame (left
/// shadow + divider) itself.
pub(crate) struct Paint {
    /// The content quads over the dock frame.
    pub(crate) quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub(crate) labels: Vec<ProseLabel>,
}

/// Shared geometry for laying out event rows (everything but the row itself).
struct EventCtx<'a> {
    panel: PxRect,
    cx: f32,
    cr: f32,
    row_h: f32,
    time_w: f32,
    scale: f32,
    theme: &'a Theme,
}

/// One event row's per-row inputs.
struct Ev<'a> {
    event: &'a SessionEvent,
    row_top: f32,
    /// Draw the `accent` viewing bar (selected AND rewound).
    selected: bool,
    /// Draw the `accent.subtle` row fill (selected).
    fill: bool,
    title_fg: Srgb,
    badge: Option<(&'a str, Srgb)>,
}

/// Render one event row: its selection marks (fill + viewing bar), the time, the actor tag,
/// a badge, and the title clipped to the space before them.
fn push_event(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    ctx: &EventCtx,
    ev: &Ev,
) {
    let (panel, theme, scale, row_h) = (ctx.panel, ctx.theme, ctx.scale, ctx.row_h);
    if ev.fill {
        // accent.subtle selected-row band, sRGB-composited over the dock's bg.base backing so
        // it reads at the guide's weight (not the brighter linear-space blend).
        quads.push(ChromeQuad::fill(
            PxRect {
                x: panel.x,
                y: ev.row_top,
                w: panel.w,
                h: row_h,
            },
            theme.accent_subtle_on(theme.bg_base.to_srgb()),
        ));
        if ev.selected {
            quads.push(ChromeQuad::fill(
                PxRect {
                    x: panel.x,
                    y: ev.row_top,
                    w: (2.0 * scale).max(1.0),
                    h: row_h,
                },
                theme.accent,
            ));
        }
    }
    // Time (mono, muted).
    push_row(
        labels,
        measure,
        &ev.event.time,
        FontRole::Micro,
        theme.fg_muted,
        ctx.cx,
        ev.row_top,
        EVENT_H,
        scale,
    );
    // Actor tag (right-anchored); the badge sits just left of it.
    let actor_x = push_right(
        labels,
        measure,
        ev.event.actor.label(),
        FontRole::Micro,
        actor_color(ev.event.actor, theme),
        ctx.cr,
        ev.row_top,
        EVENT_H,
        scale,
    );
    let right = match ev.badge {
        Some((text, fg)) => push_right(
            labels,
            measure,
            text,
            FontRole::Micro,
            fg,
            actor_x - RIGHT_GAP * scale,
            ev.row_top,
            EVENT_H,
            scale,
        ),
        None => actor_x,
    };
    // Title, clipped to the space before the badge/actor.
    let title_x = ctx.cx + ctx.time_w;
    labels.push(ProseLabel {
        text: ev.event.title.clone(),
        x: title_x,
        y: ev.row_top + (EVENT_H * scale - measure.line_height(FontRole::Body)) * 0.5,
        role: FontRole::Body,
        color: ev.title_fg,
        weight: None,
        max_w: (right - RIGHT_GAP * scale - title_x).max(1.0),
    });
}

/// The accent color for an event's actor tag: human = `accent`, agent = `session.ai`
/// (`diff.hunk` token, reserved), system = muted.
fn actor_color(actor: Actor, theme: &Theme) -> Srgb {
    match actor {
        Actor::Human => theme.accent,
        Actor::Agent => theme.diff_hunk,
        Actor::System => theme.fg_muted,
    }
}

/// Push one left-anchored label vertically centered in a row of `row_h` logical px whose top
/// is physical `top`.
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

/// Push a right-anchored label ending at `right`, vertically centered in the row; returns
/// its left edge (for chaining right-to-left segments).
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
    let w = measure.width(text, role, None);
    let x = right - w;
    push_row(labels, measure, text, role, color, x, top, row_h, scale);
    x
}

/// Push a horizontally-centered label (the empty-state placeholder).
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_centered(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    panel: PxRect,
    top: f32,
    row_h: f32,
    scale: f32,
) {
    let w = measure.width(text, role, None);
    push_row(
        labels,
        measure,
        text,
        role,
        color,
        panel.x + (panel.w - w) * 0.5,
        top,
        row_h,
        scale,
    );
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

#[cfg(test)]
mod tests {
    use super::TimelineDock;
    use skelly_render::{PxRect, TextMeasure, Theme};
    use skelly_session::{Actor, SessionEvent, Timeline};

    /// A representative dock panel (the 420px right dock) at 2x DPI.
    fn panel() -> PxRect {
        PxRect {
            x: 0.0,
            y: 0.0,
            w: 420.0 * 2.0,
            h: 700.0 * 2.0,
        }
    }

    /// A dock pinned to now over a three-event timeline (the event log lives beside the dock now).
    fn recorded() -> (TimelineDock, Timeline) {
        let mut timeline = Timeline::new();
        timeline.record(
            SessionEvent::new(Actor::System, "0:00", "Session started", "main")
                .restoring("aaaaaaa"),
        );
        timeline.record(SessionEvent::new(
            Actor::Human,
            "1:30",
            "Staged 2 files",
            "a.rs, b.rs",
        ));
        timeline.record(
            SessionEvent::new(
                Actor::Human,
                "3:20",
                "git commit - feat: x",
                "bbbbbbb - main",
            )
            .restoring("bbbbbbb"),
        );
        let mut dock = TimelineDock::new();
        dock.snap_to_newest(timeline.len());
        (dock, timeline)
    }

    /// The joined text of every label the dock builds over `timeline` (for content assertions).
    fn labels_text(dock: &TimelineDock, timeline: &Timeline) -> String {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        dock.build(timeline, panel(), 2.0, &theme, &mut m)
            .labels
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn recording_keeps_the_selection_pinned_to_now() {
        let (dock, tl) = recorded();
        // Three events recorded; the selection tracks the newest, so we are at "now".
        assert!(dock.selection_is_now(&tl));
        assert_eq!(dock.selected_restore(&tl).as_deref(), Some("bbbbbbb"));
    }

    #[test]
    fn moving_up_rewinds_to_a_past_commit_and_now_returns() {
        let (mut dock, tl) = recorded();
        dock.open(&tl, Some("main".to_owned()));
        // Up to the stage event: inherits the launch HEAD, so it is a past state.
        assert!(dock.move_selection(&tl, -1));
        assert!(!dock.selection_is_now(&tl));
        assert_eq!(dock.selected_restore(&tl).as_deref(), Some("aaaaaaa"));
        // Back to now.
        assert!(dock.select_now(&tl));
        assert!(dock.selection_is_now(&tl));
    }

    #[test]
    fn build_shows_the_events_status_banner_and_legend() {
        let (dock, tl) = recorded();
        let text = labels_text(&dock, &tl);
        assert!(text.contains("At HEAD - now"), "at-now banner");
        assert!(text.contains("TIMELINE - 3 EVENTS"));
        assert!(text.contains("Session started"));
        assert!(text.contains("git commit - feat: x"));
        assert!(text.contains("Session -"), "summary line");
        assert!(text.contains("you 2"), "legend human count");
    }

    #[test]
    fn build_marks_the_selected_row_and_a_viewing_bar_when_rewound() {
        let (mut dock, tl) = recorded();
        dock.open(&tl, Some("main".to_owned()));
        dock.move_selection(&tl, -2); // to the oldest (session start), a past state
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let paint = dock.build(&tl, panel(), 2.0, &theme, &mut m);
        // Rewound: a selected-row accent.subtle fill (opaque, pre-composited over bg.base) plus
        // a solid accent viewing bar.
        let selected_fill = theme.accent_subtle_on(theme.bg_base.to_srgb());
        assert!(
            paint
                .quads
                .iter()
                .any(|q| q.color == selected_fill && (q.alpha - 1.0).abs() < 1e-6),
            "selected fill"
        );
        assert!(
            paint
                .quads
                .iter()
                .any(|q| (q.alpha - 1.0).abs() < 1e-6 && q.color == theme.accent),
            "viewing bar"
        );
        assert!(
            paint
                .labels
                .iter()
                .any(|l| l.text.starts_with("Viewing 0:00")),
            "viewing banner"
        );
    }

    #[test]
    fn empty_timeline_shows_a_placeholder() {
        let dock = TimelineDock::new();
        let tl = Timeline::new();
        assert!(labels_text(&dock, &tl).contains("No session events yet"));
    }
}
