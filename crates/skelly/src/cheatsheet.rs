//! The keybinding cheatsheet overlay (design §11, `⌘/`).
//!
//! A reference card listing every keybinding grouped by area (Global / Tabs / Panes / Session &
//! Git / Terminal), laid out in two columns like the guide's §11 "Keybinding reference". Pure
//! layout: the binary gates it (a boolean overlay) and draws the returned display list through
//! the overlay pass (a `bg.elevated` card); `Esc` or `⌘/` dismisses it. The chords shown are the
//! macOS glyph chords, matching the app's other hints.

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, TextMeasure, Theme};

/// One keybinding row: the action and its chord (mac glyphs).
struct Bind {
    action: &'static str,
    chord: &'static str,
}

/// A titled group of bindings, with an optional note under the header.
struct Group {
    title: &'static str,
    note: &'static str,
    binds: &'static [Bind],
}

const fn b(action: &'static str, chord: &'static str) -> Bind {
    Bind { action, chord }
}

const GLOBAL: Group = Group {
    title: "Global",
    note: "",
    binds: &[
        b("Command palette", "\u{2318}K"),
        b("Settings", "\u{2318},"),
        b("Show / hide sidebar", "\u{2318}B"),
        b("Cycle sidebar / rail", "\u{21e7}\u{2318}B"),
        b("Keybinding cheatsheet", "\u{2318}/"),
        b("Find in scrollback", "\u{2318}F"),
        b("Quit", "\u{2318}Q"),
    ],
};
const PANES: Group = Group {
    title: "Panes",
    note: "Leader = \u{2303}A (tmux-style).",
    binds: &[
        b("Split right / down", "\u{2325}| \u{2325}-"),
        b("Move focus", "\u{2325}H J K L"),
        b("Resize pane", "\u{2303}\u{2325} arrows"),
        b("Swap pane", "\u{2325}\u{21e7} arrows"),
        b("Zoom / unzoom", "\u{2325}Z"),
        b("Close pane", "\u{2325}W"),
        b("Cycle layout preset", "\u{2325}Space"),
        b("Even out splits", "\u{2325}="),
    ],
};
const TERMINAL: Group = Group {
    title: "Terminal",
    note: "",
    binds: &[
        b("Copy / paste", "\u{2318}C / V"),
        b("Move by word", "\u{2325}\u{2190} / \u{2192}"),
        b("Start / end of line", "\u{2318}\u{2190} / \u{2192}"),
        b("Newline (no submit)", "\u{21e7}\u{21b5}"),
        b("Clear scrollback", "\u{2318}L"),
        b("Font larger / smaller", "\u{2318}= / -"),
        b("Reset font size", "\u{2318}0"),
    ],
};
const TABS: Group = Group {
    title: "Tabs",
    note: "",
    binds: &[
        b("New tab", "\u{2318}T"),
        b("Close tab", "\u{2318}W"),
        b("Next / prev tab", "\u{2325}\u{21e7} ] / ["),
        b("Go to tab 1-9", "\u{2318}1\u{2026}9"),
        b("Pin / unpin", "\u{21e7}\u{2318}P"),
        b("New group", "\u{21e7}\u{2318}N"),
        b("Rename tab", "F2"),
        b("Reopen closed", "\u{21e7}\u{2318}T"),
    ],
};
const SESSION: Group = Group {
    title: "Session & Git",
    note: "",
    binds: &[
        b("Session timeline", "\u{21e7}\u{2318}H"),
        b("Git diff panel", "\u{21e7}\u{2318}G"),
        b("Rewind one step", "\u{2325}\u{2318}\u{2190}"),
        b("Fast-forward one step", "\u{2325}\u{2318}\u{2192}"),
        b("Return to now (HEAD)", "\u{2325}\u{2318}0"),
        b("Stage hunk (in diff)", "\u{2318}\u{21a9}"),
        b("Full-width dock", "\u{21e7}\u{2318}F"),
    ],
};

/// The three columns (design §11), balanced by row count so the card fits the window without
/// clipping: `Global + Session` | `Tabs + Terminal` | `Panes`.
const COLUMNS: [&[Group]; 3] = [&[GLOBAL, SESSION], &[TABS, TERMINAL], &[PANES]];

// --- layout metrics (logical px) ---
const PAD: f32 = 28.0;
const COL_GAP: f32 = 36.0;
const TITLE_H: f32 = 24.0;
const NOTE_H: f32 = 16.0;
const ROW_H: f32 = 26.0;
const GROUP_GAP: f32 = 18.0;
const HEADER_H: f32 = 40.0;

/// The card's natural size (**physical** px) for the binary to center it: two columns of the
/// group stacks plus the header. All values are physical (the measurer's widths are too).
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "the column count is a tiny fixed value"
)]
pub(crate) fn card_size(scale: f32, measure: &mut TextMeasure) -> (f32, f32) {
    let col_w = column_width(scale, measure);
    let cols = COLUMNS.len() as f32;
    let w = cols * col_w + ((cols - 1.0) * COL_GAP + 2.0 * PAD) * scale;
    let tallest = COLUMNS
        .iter()
        .map(|c| column_height(c, scale))
        .fold(0.0_f32, f32::max);
    let h = (HEADER_H + PAD) * scale + tallest + PAD * scale;
    (w, h)
}

/// The physical height of a column's stacked groups.
#[allow(
    clippy::cast_precision_loss,
    reason = "bind counts are tiny fixed values"
)]
fn column_height(groups: &[Group], scale: f32) -> f32 {
    let mut h = 0.0;
    for (i, g) in groups.iter().enumerate() {
        if i > 0 {
            h += GROUP_GAP * scale;
        }
        h += TITLE_H * scale;
        if !g.note.is_empty() {
            h += NOTE_H * scale;
        }
        h += g.binds.len() as f32 * ROW_H * scale;
    }
    h
}

/// The physical width of one column, sized to its widest `action  chord` row (the measurer's
/// widths are already physical).
fn column_width(scale: f32, measure: &mut TextMeasure) -> f32 {
    let mut w = 210.0 * scale;
    for group in COLUMNS.iter().copied().flatten() {
        if !group.note.is_empty() {
            w = w.max(measure.width(group.note, FontRole::Caption, None) + 8.0 * scale);
        }
        for bind in group.binds {
            let aw = measure.width(bind.action, FontRole::Body, None);
            let cw = measure.width(bind.chord, FontRole::Mono, None);
            w = w.max(aw + cw + 32.0 * scale);
        }
    }
    w
}

/// Build the cheatsheet content within the centered card `panel` (physical px): a title, then two
/// columns of grouped binding rows.
pub(crate) fn build(
    panel: PxRect,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
    let mut quads = Vec::new();
    let mut labels = Vec::new();
    let pad = PAD * scale;
    let x0 = panel.x + pad;

    // Header: "Keybindings" + an esc hint.
    let title_line = measure.line_height(FontRole::H2);
    labels.push(ProseLabel {
        text: "Keybindings".to_owned(),
        x: x0,
        y: panel.y + pad,
        role: FontRole::H2,
        color: theme.fg_primary,
        weight: None,
        max_w: f32::MAX,
    });
    let hint = "esc to close";
    let hint_w = measure.width(hint, FontRole::Caption, None);
    labels.push(ProseLabel {
        text: hint.to_owned(),
        x: panel.x + panel.w - pad - hint_w,
        y: panel.y + pad + (title_line - measure.line_height(FontRole::Caption)) * 0.5,
        role: FontRole::Caption,
        color: theme.fg_muted,
        weight: None,
        max_w: f32::MAX,
    });

    let body_top = panel.y + (HEADER_H + PAD) * scale;
    #[allow(
        clippy::cast_precision_loss,
        reason = "the column count is a tiny fixed value"
    )]
    let col_w = (panel.w - 2.0 * pad - (COLUMNS.len() as f32 - 1.0) * COL_GAP * scale)
        / COLUMNS.len() as f32;
    for (i, groups) in COLUMNS.iter().enumerate() {
        #[allow(
            clippy::cast_precision_loss,
            reason = "the column index is a tiny fixed value"
        )]
        let x = x0 + i as f32 * (col_w + COL_GAP * scale);
        push_column(
            &mut quads,
            &mut labels,
            groups,
            x,
            body_top,
            col_w,
            scale,
            theme,
            measure,
        );
    }
    (quads, labels)
}

#[allow(clippy::too_many_arguments, reason = "one focused column builder")]
fn push_column(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    groups: &[Group],
    x: f32,
    top: f32,
    col_w: f32,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) {
    let mut y = top;
    for (i, group) in groups.iter().enumerate() {
        if i > 0 {
            y += GROUP_GAP * scale;
        }
        // Group title, with a hairline divider under it spanning the column.
        labels.push(ProseLabel {
            text: group.title.to_owned(),
            x,
            y,
            role: FontRole::Title,
            color: theme.fg_primary,
            weight: None,
            max_w: f32::MAX,
        });
        y += TITLE_H * scale;
        quads.push(ChromeQuad::fill(
            PxRect {
                x,
                y: y - 6.0 * scale,
                w: col_w,
                h: scale.max(1.0),
            },
            theme.border_subtle,
        ));
        if !group.note.is_empty() {
            labels.push(ProseLabel {
                text: group.note.to_owned(),
                x,
                y,
                role: FontRole::Caption,
                color: theme.fg_muted,
                weight: None,
                max_w: f32::MAX,
            });
            y += NOTE_H * scale;
        }
        let row_line = measure.line_height(FontRole::Body);
        for bind in group.binds {
            let cy = y + (ROW_H * scale - row_line) * 0.5;
            labels.push(ProseLabel {
                text: bind.action.to_owned(),
                x,
                y: cy,
                role: FontRole::Body,
                color: theme.fg_secondary,
                weight: None,
                max_w: f32::MAX,
            });
            let cw = measure.width(bind.chord, FontRole::Mono, None);
            labels.push(ProseLabel {
                text: bind.chord.to_owned(),
                x: x + col_w - cw,
                y: cy,
                role: FontRole::Mono,
                color: theme.fg_primary,
                weight: None,
                max_w: f32::MAX,
            });
            y += ROW_H * scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build, card_size, COLUMNS};
    use skelly_render::{PxRect, TextMeasure, Theme};

    #[test]
    fn covers_every_group_and_renders_bindings() {
        // All five §11 groups are present across the columns.
        let titles: Vec<&str> = COLUMNS.iter().copied().flatten().map(|g| g.title).collect();
        for expected in ["Global", "Tabs", "Panes", "Session & Git", "Terminal"] {
            assert!(titles.contains(&expected), "missing group {expected}");
        }
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let (w, h) = card_size(2.0, &mut m);
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w,
            h,
        };
        let (_, labels) = build(panel, 2.0, &theme, &mut m);
        let joined: String = labels.iter().map(|l| l.text.clone()).collect();
        assert!(joined.contains("Keybindings"));
        assert!(joined.contains("Command palette"));
        assert!(joined.contains("Cycle layout preset"));
        assert!(joined.contains("Reopen closed"));

        // Layout sanity: every label starts inside the card, and the bottom-most row stays within
        // the card height (so nothing is clipped or overflows).
        let line = m.line_height(super::FontRole::Body);
        for l in &labels {
            assert!(l.x >= panel.x - 0.5, "label {:?} left of the card", l.text);
            let w = m.width(&l.text, l.role, None);
            assert!(
                l.x + w <= panel.x + panel.w + 0.5,
                "label {:?} overflows",
                l.text
            );
            assert!(
                l.y + line <= panel.y + panel.h + 0.5,
                "label {:?} below the card",
                l.text
            );
        }
    }
}
