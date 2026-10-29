//! Workspace management (design §08 #2): the curated workspace icon set and the Settings >
//! Workspaces page, a manager over every workspace and its groups - rename, remove, pick an
//! icon, add one. The page edits the restored **session layout**, not `config.toml`: a
//! workspace or group is layout with no config key, so Hard rule 1's one-key contract covers
//! the config categories beside it, never this page. Pure state + layout like [`crate::settings`]:
//! the binary owns the workspaces, routes keys and clicks, and applies each change.

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, TextMeasure, Theme};

use crate::settings::{hover_fill, push_row};

/// The icons a workspace can wear. The first eight are the geometric marks every workspace
/// got by position before icons were pickable (so a restored legacy session looks unchanged);
/// the rest are Codicons from the bundled Symbols Nerd Font, drawn with [`FontRole::Icon`].
pub(crate) const ICONS: [char; 24] = [
    '\u{25C6}', // ◆
    '\u{25CF}', // ●
    '\u{25B2}', // ▲
    '\u{25A0}', // ■
    '\u{2605}', // ★
    '\u{25C7}', // ◇
    '\u{25CB}', // ○
    '\u{25A1}', // □
    '\u{EB06}', // cod-home
    '\u{EAAC}', // cod-briefcase
    '\u{EAC4}', // cod-code
    '\u{EA85}', // cod-terminal
    '\u{EA83}', // cod-folder
    '\u{EAA4}', // cod-book
    '\u{EA79}', // cod-beaker
    '\u{EB44}', // cod-rocket
    '\u{EAAF}', // cod-bug
    '\u{EACE}', // cod-database
    '\u{EBAA}', // cod-cloud
    '\u{EB01}', // cod-globe
    '\u{EA61}', // cod-lightbulb
    '\u{EAF2}', // cod-flame
    '\u{EB05}', // cod-heart
    '\u{EC15}', // cod-coffee
];

/// How many of [`ICONS`] are the legacy positional marks.
const POSITIONAL: usize = 8;

/// The font role that draws `icon`: the bundled icon font for a private-use codepoint, else
/// the chrome mono (which carries the geometric marks).
pub(crate) fn icon_role(icon: char) -> FontRole {
    if ('\u{E000}'..='\u{F8FF}').contains(&icon) {
        FontRole::Icon
    } else {
        FontRole::Mono
    }
}

/// The icon a workspace saved before icons were pickable showed at `index`.
pub(crate) fn positional_icon(index: usize) -> char {
    ICONS[index % POSITIONAL]
}

/// A new workspace's icon: the first one no other workspace wears (so chips stay distinct),
/// or the first icon once all are taken.
pub(crate) fn free_icon(taken: &[char]) -> char {
    ICONS
        .iter()
        .copied()
        .find(|icon| !taken.contains(icon))
        .unwrap_or(ICONS[0])
}

/// A new workspace's name, given the `taken` ones: the second gets the guide's "Work" (the
/// first is "Personal"), the rest are numbered from their position - skipping any name in use,
/// so adding after a removal never repeats one.
pub(crate) fn free_name(taken: &[&str]) -> String {
    if taken.len() == 1 && !taken.contains(&"Work") {
        return "Work".to_owned();
    }
    // `taken.len() + 1` candidates cannot all be taken by `taken.len()` names.
    (taken.len() + 1..=2 * taken.len() + 1)
        .map(|n| format!("Space {n}"))
        .find(|name| !taken.contains(&name.as_str()))
        .unwrap_or_default()
}

/// The icon `delta` steps from `icon` in [`ICONS`], wrapping. An icon outside the set (a
/// hand-edited session) steps from the start.
pub(crate) fn step_icon(icon: char, delta: i32) -> char {
    let n = i32::try_from(ICONS.len()).unwrap_or(1);
    let at = ICONS
        .iter()
        .position(|&c| c == icon)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(if delta > 0 { -1 } else { 0 });
    let next = usize::try_from((at + delta).rem_euclid(n)).unwrap_or(0);
    ICONS[next]
}

/// Page layout constants in **logical** px (multiplied by the DPI scale).
const ROW_H: f32 = 40.0;
/// A group row sits a step shorter under its workspace.
const GROUP_ROW_H: f32 = 32.0;
/// The workspace chip, matching the sidebar's 26px tiles.
const CHIP: f32 = 26.0;
const CHIP_RADIUS: f32 = 7.0;
/// Gap between the chip and the name; group names indent to line up with workspace names.
const NAME_GAP: f32 = 12.0;
/// Gap between a name and its tab-count meta.
const META_GAP: f32 = 10.0;
/// The compact row buttons (Rename / Remove).
const BUTTON_PAD_X: f32 = 10.0;
const BUTTON_H: f32 = 24.0;
const BUTTON_GAP: f32 = 6.0;
const BUTTON_RADIUS: f32 = 6.0;
/// The icon picker below the list: its caption row, then a grid of tiles.
const PICKER_GAP: f32 = 18.0;
const PICKER_LABEL_H: f32 = 28.0;
const TILE: f32 = 30.0;
const TILE_GAP: f32 = 6.0;
const TILES_PER_ROW: usize = 12;
/// Height of an overflow line (`↑ N more` / `↓ N more`), as in the sidebar.
const IND_H: f32 = 16.0;
/// How many lines the picker's tile grid takes.
const PICKER_LINES: usize = ICONS.len().div_ceil(TILES_PER_ROW);

/// One workspace as the page lists it.
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) icon: char,
    pub(crate) tabs: usize,
    /// Whether it is the workspace on screen.
    pub(crate) active: bool,
    pub(crate) groups: Vec<GroupEntry>,
}

/// One group under its workspace.
pub(crate) struct GroupEntry {
    pub(crate) name: String,
    pub(crate) tabs: usize,
}

/// A row of the page, top to bottom: each workspace followed by its groups, then "New
/// workspace".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Row {
    Workspace(usize),
    Group { workspace: usize, group: usize },
    Add,
}

impl Row {
    /// The workspace this row belongs to (the icon picker edits it).
    pub(crate) fn workspace(self) -> Option<usize> {
        match self {
            Row::Workspace(w) | Row::Group { workspace: w, .. } => Some(w),
            Row::Add => None,
        }
    }
}

/// The page's rows in display order.
pub(crate) fn rows(entries: &[Entry]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (w, entry) in entries.iter().enumerate() {
        rows.push(Row::Workspace(w));
        rows.extend((0..entry.groups.len()).map(|group| Row::Group {
            workspace: w,
            group,
        }));
    }
    rows.push(Row::Add);
    rows
}

/// Whether `row` can be removed: any group, and any workspace but the last one.
pub(crate) fn removable(entries: &[Entry], row: Row) -> bool {
    match row {
        Row::Workspace(_) => entries.len() > 1,
        Row::Group { .. } => true,
        Row::Add => false,
    }
}

/// What a click on the page lands on. Row indices address [`rows`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Target {
    /// A row outside its buttons: focus it.
    Row(usize),
    /// A row's Rename button.
    Rename(usize),
    /// A row's Remove button.
    Remove(usize),
    /// An icon tile: give the focused row's workspace this icon.
    Icon(char),
    /// The "New workspace" row.
    Add,
}

impl Target {
    /// The row this target belongs to, for its hover wash.
    fn row(self) -> Option<usize> {
        match self {
            Target::Row(i) | Target::Rename(i) | Target::Remove(i) => Some(i),
            Target::Icon(_) | Target::Add => None,
        }
    }
}

/// A row's height (logical px): a group row is shorter than a workspace row.
fn row_height(row: Row) -> f32 {
    if matches!(row, Row::Group { .. }) {
        GROUP_ROW_H
    } else {
        ROW_H
    }
}

/// The height the picker takes under the rows (physical px), reserved even when the focused
/// row has none so the list does not jump as focus moves.
#[allow(
    clippy::cast_precision_loss,
    reason = "the picker's line count is a tiny constant"
)]
fn picker_height(scale: f32) -> f32 {
    let lines = PICKER_LINES as f32;
    (PICKER_GAP + PICKER_LABEL_H + lines * (TILE + TILE_GAP) - TILE_GAP) * scale
}

/// The first row to draw for rows of `heights` in a `viewport` that tall: `first` clamped so
/// the list never scrolls past its end, then, when `reveal`, moved just enough to show
/// `selected`.
fn first_row(heights: &[f32], viewport: f32, first: usize, selected: usize, reveal: bool) -> usize {
    let mut max_first = heights.len();
    let mut tail = 0.0;
    while max_first > 0 && tail + heights[max_first - 1] <= viewport {
        max_first -= 1;
        tail += heights[max_first];
    }
    let mut first = first.min(max_first.min(heights.len().saturating_sub(1)));
    if reveal {
        first = first.min(selected);
        while first < selected && heights[first..=selected].iter().sum::<f32>() > viewport {
            first += 1;
        }
    }
    first
}

/// The page's view state for one paint.
pub(crate) struct Page<'a> {
    pub(crate) entries: &'a [Entry],
    /// The focused row (clamped to the rows).
    pub(crate) selected: usize,
    /// The focused row's rename draft, while editing.
    pub(crate) editing: Option<&'a str>,
    pub(crate) hover: Option<Target>,
    /// The first row to draw (the list's scroll), clamped so the list never scrolls past its
    /// end.
    pub(crate) first: usize,
    /// Whether to scroll the focused row into view (after a keyboard move).
    pub(crate) reveal: bool,
}

/// Where the page lays out (physical px): the content column, plus the band the row fills
/// span (from the nav divider to the panel's right edge, like the settings rows).
pub(crate) struct Area {
    pub(crate) x: f32,
    pub(crate) right: f32,
    pub(crate) band_x: f32,
    pub(crate) band_right: f32,
    pub(crate) top: f32,
    /// The lowest y the page draws to; the rows scroll within what the picker leaves.
    pub(crate) bottom: f32,
}

/// The display list the page draws into, plus every clickable rect in draw order.
pub(crate) struct Out<'a> {
    pub(crate) quads: &'a mut Vec<ChromeQuad>,
    pub(crate) labels: &'a mut Vec<ProseLabel>,
    pub(crate) targets: Vec<(PxRect, Target)>,
}

impl Page<'_> {
    /// Lay the page out: every workspace row (chip, name, tab count) with its group rows
    /// indented beneath, "New workspace", then the icon picker for the focused row's
    /// workspace. The focused and hovered rows show their Rename / Remove buttons. Rows that
    /// do not fit above the picker scroll; returns the first row drawn, the scroll to keep.
    pub(crate) fn build(
        &self,
        area: &Area,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
        out: &mut Out<'_>,
    ) -> usize {
        let rows = rows(self.entries);
        let selected = self.selected.min(rows.len() - 1);
        let heights: Vec<f32> = rows.iter().map(|&row| row_height(row) * scale).collect();
        // A list taller than its room scrolls between `↑ N more` / `↓ N more` lines.
        let indicator = IND_H * scale;
        let (mut top, mut bottom) = (area.top, area.bottom - picker_height(scale));
        let overflow = heights.iter().sum::<f32>() > bottom - top;
        if overflow {
            top += indicator;
            bottom -= indicator;
        }
        let first = first_row(&heights, bottom - top, self.first, selected, self.reveal);
        let mut y = top;
        let mut last = first;
        for (i, row) in rows.iter().copied().enumerate().skip(first) {
            let row_h = row_height(row);
            // Rows past the viewport are scrolled out; the first one always draws.
            if i > first && y + row_h * scale > bottom {
                break;
            }
            last = i;
            self.push_list_row(i, row, y, selected, area, scale, theme, measure, out);
            y += row_h * scale;
        }
        if overflow {
            let below = rows.len() - 1 - last;
            for (hidden, arrow, at) in [(first, '\u{2191}', area.top), (below, '\u{2193}', y)] {
                if hidden > 0 {
                    let text = format!("{arrow} {hidden} more");
                    let (role, color) = (FontRole::Caption, theme.fg_muted);
                    push_row(
                        out.labels, measure, &text, role, color, area.x, at, IND_H, scale,
                    );
                }
            }
            y += indicator;
        }
        if let Some(w) = rows[selected].workspace() {
            self.push_picker(w, y + PICKER_GAP * scale, area, scale, theme, measure, out);
        }
        first
    }

    /// Row `i` (`row`) at `y`: its band (filled when focused or hovered, and its click
    /// target), then its content, with Rename / Remove on the focused or hovered row.
    #[allow(clippy::too_many_arguments, reason = "one focused row builder")]
    fn push_list_row(
        &self,
        i: usize,
        row: Row,
        y: f32,
        selected: usize,
        area: &Area,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
        out: &mut Out<'_>,
    ) {
        let row_h = row_height(row);
        let hovered = self.hover.and_then(Target::row);
        let band = PxRect {
            x: area.band_x,
            y,
            w: area.band_right - area.band_x,
            h: row_h * scale,
        };
        let focused = i == selected;
        if focused {
            out.quads.push(ChromeQuad::fill(
                band,
                theme.accent_subtle_on(theme.bg_elevated),
            ));
        } else if hovered == Some(i) || (row == Row::Add && self.hover == Some(Target::Add)) {
            out.quads
                .push(ChromeQuad::fill(band, hover_fill(theme, theme.bg_elevated)));
        }
        out.targets.push((
            band,
            if row == Row::Add {
                Target::Add
            } else {
                Target::Row(i)
            },
        ));
        let editing = self.editing.filter(|_| focused);
        let buttons = (focused || hovered == Some(i)) && editing.is_none();
        match row {
            Row::Workspace(w) => {
                self.push_workspace(w, i, y, editing, buttons, area, scale, theme, measure, out);
            }
            Row::Group { workspace, group } => {
                let entry = &self.entries[workspace].groups[group];
                let x = area.x + (CHIP + NAME_GAP) * scale;
                let right = if buttons {
                    push_buttons(i, true, area.right, y, row_h, scale, theme, measure, out)
                } else {
                    area.right
                };
                push_name(
                    &entry.name,
                    editing,
                    FontRole::Micro,
                    theme.fg_secondary,
                    &tabs_label(entry.tabs),
                    (x, right, y, row_h),
                    scale,
                    theme,
                    measure,
                    out,
                );
            }
            Row::Add => {
                let chip = chip_rect(area.x, y, ROW_H, scale);
                out.quads.push(ChromeQuad::rounded(
                    chip,
                    theme.bg_surface,
                    CHIP_RADIUS * scale,
                ));
                push_glyph('+', chip, theme.fg_muted, measure, out);
                push_row(
                    out.labels,
                    measure,
                    "New workspace",
                    FontRole::Label,
                    theme.fg_secondary,
                    chip.x + chip.w + NAME_GAP * scale,
                    y,
                    ROW_H,
                    scale,
                );
            }
        }
    }

    /// One workspace row: its chip (the active workspace's ringed in accent), its name (or the
    /// rename draft), its tab count, and the row buttons when shown.
    #[allow(clippy::too_many_arguments, reason = "one focused row builder")]
    fn push_workspace(
        &self,
        w: usize,
        row: usize,
        y: f32,
        editing: Option<&str>,
        buttons: bool,
        area: &Area,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
        out: &mut Out<'_>,
    ) {
        let entry = &self.entries[w];
        let chip = chip_rect(area.x, y, ROW_H, scale);
        push_tile(
            entry.icon,
            chip,
            entry.active,
            false,
            scale,
            theme,
            measure,
            out,
        );
        let right = if buttons {
            let removable = removable(self.entries, Row::Workspace(w));
            push_buttons(
                row, removable, area.right, y, ROW_H, scale, theme, measure, out,
            )
        } else {
            area.right
        };
        let mut meta = tabs_label(entry.tabs);
        if entry.active {
            meta.push_str(" \u{b7} current");
        }
        push_name(
            &entry.name,
            editing,
            FontRole::Label,
            theme.fg_primary,
            &meta,
            (chip.x + chip.w + NAME_GAP * scale, right, y, ROW_H),
            scale,
            theme,
            measure,
            out,
        );
    }

    /// The icon picker for workspace `w`: a caption naming it, then every icon as a tile, the
    /// one it wears ringed in accent.
    #[allow(
        clippy::too_many_arguments,
        clippy::cast_precision_loss,
        reason = "one focused picker builder; the tile grid is a tiny fixed count"
    )]
    fn push_picker(
        &self,
        w: usize,
        top: f32,
        area: &Area,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
        out: &mut Out<'_>,
    ) {
        let entry = &self.entries[w];
        push_row(
            out.labels,
            measure,
            &format!("Icon \u{b7} {}", entry.name),
            FontRole::Caption,
            theme.fg_muted,
            area.x,
            top,
            PICKER_LABEL_H,
            scale,
        );
        let grid_top = top + PICKER_LABEL_H * scale;
        for (i, icon) in ICONS.iter().copied().enumerate() {
            let (col, line) = (i % TILES_PER_ROW, i / TILES_PER_ROW);
            let tile = PxRect {
                x: area.x + col as f32 * (TILE + TILE_GAP) * scale,
                y: grid_top + line as f32 * (TILE + TILE_GAP) * scale,
                w: TILE * scale,
                h: TILE * scale,
            };
            let hovered = self.hover == Some(Target::Icon(icon));
            push_tile(
                icon,
                tile,
                icon == entry.icon,
                hovered,
                scale,
                theme,
                measure,
                out,
            );
            out.targets.push((tile, Target::Icon(icon)));
        }
    }
}

/// "1 tab" / "3 tabs".
fn tabs_label(n: usize) -> String {
    if n == 1 {
        "1 tab".to_owned()
    } else {
        format!("{n} tabs")
    }
}

/// A chip-sized tile at the content inset, vertically centered in a `row_h` row.
fn chip_rect(x: f32, top: f32, row_h: f32, scale: f32) -> PxRect {
    PxRect {
        x,
        y: top + (row_h - CHIP) * 0.5 * scale,
        w: CHIP * scale,
        h: CHIP * scale,
    }
}

/// An icon tile: `accent`@0.4 ring over an `accent.subtle` fill with an accent glyph when
/// `chosen` (the sidebar's active-chip look), else a `bg.surface` tile (washed when
/// `hovered`) with a muted glyph.
#[allow(clippy::too_many_arguments, reason = "one focused tile builder")]
fn push_tile(
    icon: char,
    tile: PxRect,
    chosen: bool,
    hovered: bool,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
    out: &mut Out<'_>,
) {
    let radius = CHIP_RADIUS * scale;
    if chosen {
        push_ringed(
            out.quads,
            tile,
            theme.accent.over(theme.bg_elevated, 0.4),
            theme.accent_subtle_on(theme.bg_elevated),
            radius,
            scale,
        );
    } else {
        let fill = if hovered {
            hover_fill(theme, theme.bg_surface)
        } else {
            theme.bg_surface
        };
        out.quads.push(ChromeQuad::rounded(tile, fill, radius));
    }
    let color = if chosen { theme.accent } else { theme.fg_muted };
    push_glyph(icon, tile, color, measure, out);
}

/// A rounded `fill` inside a 1px `ring` (the ring drawn behind, the fill inset by the stroke).
fn push_ringed(
    quads: &mut Vec<ChromeQuad>,
    rect: PxRect,
    ring: skelly_render::Srgb,
    fill: skelly_render::Srgb,
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

/// A glyph centered in `rect`, in the role that carries it.
fn push_glyph(
    glyph: char,
    rect: PxRect,
    color: skelly_render::Srgb,
    measure: &mut TextMeasure,
    out: &mut Out<'_>,
) {
    let role = icon_role(glyph);
    let text = glyph.to_string();
    let w = measure.width(&text, role, None);
    let line = measure.line_height(role);
    out.labels.push(ProseLabel {
        text,
        x: rect.x + (rect.w - w) * 0.5,
        y: rect.y + (rect.h - line) * 0.5,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

/// A row's name (or its rename draft with a caret, in accent) followed by its muted meta,
/// clipped short of `right` (where the buttons start). `at` is `(x, right, top, row_h)`.
#[allow(clippy::too_many_arguments, reason = "one focused name builder")]
fn push_name(
    name: &str,
    editing: Option<&str>,
    role: FontRole,
    color: skelly_render::Srgb,
    meta: &str,
    at: (f32, f32, f32, f32),
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
    out: &mut Out<'_>,
) {
    let (x, right, top, row_h) = at;
    let room = (right - x - META_GAP * scale).max(1.0);
    if let Some(draft) = editing {
        push_row(
            out.labels,
            measure,
            &format!("{draft}\u{258f}"),
            FontRole::Label,
            theme.accent,
            x,
            top,
            row_h,
            scale,
        );
        if let Some(label) = out.labels.last_mut() {
            label.max_w = room;
        }
        return;
    }
    let name_w = measure.width(name, role, None).min(room);
    push_row(out.labels, measure, name, role, color, x, top, row_h, scale);
    if let Some(label) = out.labels.last_mut() {
        label.max_w = room;
    }
    let meta_x = x + name_w + META_GAP * scale;
    if meta_x + measure.width(meta, FontRole::Caption, None) <= right {
        push_row(
            out.labels,
            measure,
            meta,
            FontRole::Caption,
            theme.fg_muted,
            meta_x,
            top,
            row_h,
            scale,
        );
    }
}

/// The row buttons, right-anchored at `right`: Rename, then Remove (danger) when `removable`.
/// Returns the left edge of the leftmost button, where the name must stop.
#[allow(clippy::too_many_arguments, reason = "one focused button-row builder")]
fn push_buttons(
    row: usize,
    removable: bool,
    right: f32,
    top: f32,
    row_h: f32,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
    out: &mut Out<'_>,
) -> f32 {
    let mut buttons = vec![("Rename", Target::Rename(row), theme.fg_secondary)];
    if removable {
        buttons.push(("Remove", Target::Remove(row), theme.diff_del));
    }
    let mut x = right;
    for (text, target, color) in buttons.into_iter().rev() {
        let w = measure.width(text, FontRole::Caption, None) + 2.0 * BUTTON_PAD_X * scale;
        x -= w;
        let rect = PxRect {
            x,
            y: top + (row_h - BUTTON_H) * 0.5 * scale,
            w,
            h: BUTTON_H * scale,
        };
        push_ringed(
            out.quads,
            rect,
            theme.border_subtle,
            theme.bg_surface,
            BUTTON_RADIUS * scale,
            scale,
        );
        push_row(
            out.labels,
            measure,
            text,
            FontRole::Caption,
            color,
            x + BUTTON_PAD_X * scale,
            top,
            row_h,
            scale,
        );
        out.targets.push((rect, target));
        x -= BUTTON_GAP * scale;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::{
        first_row, free_icon, free_name, icon_role, picker_height, positional_icon, removable,
        rows, step_icon, Area, Entry, GroupEntry, Out, Page, Row, Target, ICONS, IND_H, ROW_H,
    };
    use skelly_render::{FontRole, TextMeasure, Theme};

    fn entries() -> Vec<Entry> {
        vec![
            Entry {
                name: "Personal".into(),
                icon: ICONS[0],
                tabs: 3,
                active: true,
                groups: vec![GroupEntry {
                    name: "skelly \u{b7} main".into(),
                    tabs: 2,
                }],
            },
            Entry {
                name: "Work".into(),
                icon: ICONS[9],
                tabs: 1,
                active: false,
                groups: Vec::new(),
            },
        ]
    }

    #[test]
    fn rows_list_each_workspace_then_its_groups_then_add() {
        assert_eq!(
            rows(&entries()),
            vec![
                Row::Workspace(0),
                Row::Group {
                    workspace: 0,
                    group: 0
                },
                Row::Workspace(1),
                Row::Add,
            ]
        );
        let e = entries();
        assert!(removable(&e, Row::Workspace(1)));
        assert!(removable(
            &e,
            Row::Group {
                workspace: 0,
                group: 0
            }
        ));
        assert!(
            !removable(&e[..1], Row::Workspace(0)),
            "the last workspace stays"
        );
        assert!(!removable(&e, Row::Add));
    }

    #[test]
    fn icons_default_distinct_step_with_wrap_and_pick_their_font() {
        assert_eq!(positional_icon(0), ICONS[0]);
        assert_eq!(positional_icon(9), ICONS[1]);
        assert_eq!(free_icon(&[ICONS[0], ICONS[1]]), ICONS[2]);
        assert_eq!(free_icon(&ICONS), ICONS[0]);
        assert_eq!(step_icon(ICONS[0], -1), ICONS[ICONS.len() - 1]);
        assert_eq!(step_icon(ICONS[ICONS.len() - 1], 1), ICONS[0]);
        assert_eq!(step_icon('x', 1), ICONS[0]);
        assert_eq!(icon_role(ICONS[0]), FontRole::Mono);
        assert_eq!(icon_role(ICONS[8]), FontRole::Icon);
    }

    #[test]
    fn new_workspace_names_follow_the_guide_and_never_repeat() {
        assert_eq!(free_name(&["Personal"]), "Work");
        assert_eq!(free_name(&["Personal", "Work"]), "Space 3");
        // After removing "Work" from [Personal, Work, Space 3], "Space 3" is still taken.
        assert_eq!(free_name(&["Personal", "Space 3"]), "Space 4");
        // A lone workspace renamed "Work" moves the next one on to a number.
        assert_eq!(free_name(&["Work"]), "Space 2");
    }

    fn build(page: &Page<'_>) -> Vec<(skelly_render::PxRect, Target)> {
        let theme = Theme::resolve("ossein-dark");
        let mut measure = TextMeasure::new(1.0);
        let (mut quads, mut labels) = (Vec::new(), Vec::new());
        let mut out = Out {
            quads: &mut quads,
            labels: &mut labels,
            targets: Vec::new(),
        };
        let area = Area {
            x: 234.0,
            right: 1180.0,
            band_x: 210.0,
            band_right: 1200.0,
            top: 64.0,
            bottom: 900.0,
        };
        page.build(&area, 1.0, &theme, &mut measure, &mut out);
        out.targets
    }

    #[test]
    fn the_focused_row_shows_its_buttons_and_the_picker_for_its_workspace() {
        let e = entries();
        let targets = build(&Page {
            entries: &e,
            selected: 2, // Work
            editing: None,
            hover: None,
            first: 0,
            reveal: false,
        });
        let has = |t: Target| targets.iter().any(|(_, x)| *x == t);
        assert!(has(Target::Rename(2)) && has(Target::Remove(2)));
        assert!(!has(Target::Rename(0)), "unfocused rows hide their buttons");
        assert!(has(Target::Add));
        assert_eq!(
            targets
                .iter()
                .filter(|(_, t)| matches!(t, Target::Icon(_)))
                .count(),
            ICONS.len()
        );
        // The buttons sit over their row, so the topmost hit is the button.
        let (rect, _) = targets
            .iter()
            .find(|(_, t)| *t == Target::Remove(2))
            .unwrap();
        let (cx, cy) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
        let hit = targets
            .iter()
            .rev()
            .find(|(r, _)| cx >= r.x && cx < r.x + r.w && cy >= r.y && cy < r.y + r.h)
            .map(|(_, t)| *t);
        assert_eq!(hit, Some(Target::Remove(2)));
    }

    #[test]
    fn rows_scroll_to_reveal_focus_and_never_past_the_end() {
        // Five 40px rows in a 100px viewport: two fit.
        let h = [40.0; 5];
        assert_eq!(first_row(&h, 100.0, 0, 0, true), 0);
        // Revealing row 3 scrolls just enough to show it last.
        assert_eq!(first_row(&h, 100.0, 0, 3, true), 2);
        // Revealing a row above the scroll brings it to the top.
        assert_eq!(first_row(&h, 100.0, 3, 1, true), 1);
        // A wheel scroll (no reveal) keeps its place, but not past the last full page.
        assert_eq!(first_row(&h, 100.0, 2, 0, false), 2);
        assert_eq!(first_row(&h, 100.0, 9, 0, false), 3);
        // Everything fits: never scrolled.
        assert_eq!(first_row(&h, 400.0, 2, 4, true), 0);
        // A viewport too short for one row still shows the focused one.
        assert_eq!(first_row(&h, 10.0, 0, 2, true), 2);
    }

    #[test]
    fn a_short_page_draws_only_the_rows_that_fit_above_the_picker() {
        let e = entries();
        let theme = Theme::resolve("ossein-dark");
        let mut measure = TextMeasure::new(1.0);
        let (mut quads, mut labels) = (Vec::new(), Vec::new());
        let mut out = Out {
            quads: &mut quads,
            labels: &mut labels,
            targets: Vec::new(),
        };
        let area = Area {
            x: 234.0,
            right: 1180.0,
            band_x: 210.0,
            band_right: 1200.0,
            top: 0.0,
            bottom: picker_height(1.0) + 2.0 * ROW_H + 2.0 * IND_H,
        };
        let page = Page {
            entries: &e,
            selected: 3,
            editing: None,
            hover: None,
            first: 0,
            reveal: true,
        };
        let first = page.build(&area, 1.0, &theme, &mut measure, &mut out);
        let drawn: Vec<usize> = out
            .targets
            .iter()
            .filter_map(|(_, t)| t.row().or((*t == Target::Add).then_some(3)))
            .collect();
        assert_eq!(first, 2, "focus on the last row scrolls it into view");
        assert!(drawn.contains(&2) && drawn.contains(&3));
        assert!(
            !drawn.contains(&0),
            "the rows above the scroll are not drawn"
        );
        assert!(labels.iter().any(|l| l.text == "\u{2191} 2 more"));
        assert!(!labels.iter().any(|l| l.text.starts_with('\u{2193}')));
    }

    #[test]
    fn the_add_row_has_no_picker_and_editing_hides_the_buttons() {
        let e = entries();
        let on_add = build(&Page {
            entries: &e,
            selected: 3,
            editing: None,
            hover: None,
            first: 0,
            reveal: false,
        });
        assert!(!on_add.iter().any(|(_, t)| matches!(t, Target::Icon(_))));
        let editing = build(&Page {
            entries: &e,
            selected: 0,
            editing: Some("Pers"),
            hover: None,
            first: 0,
            reveal: false,
        });
        assert!(!editing.iter().any(|(_, t)| *t == Target::Rename(0)));
        // The only workspace cannot be removed: its row offers Rename alone.
        let single = build(&Page {
            entries: &e[..1],
            selected: 0,
            editing: None,
            hover: None,
            first: 0,
            reveal: false,
        });
        assert!(single.iter().any(|(_, t)| *t == Target::Rename(0)));
        assert!(!single.iter().any(|(_, t)| *t == Target::Remove(0)));
    }
}
