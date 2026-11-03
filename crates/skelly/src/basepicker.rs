//! The drop-down that picks what the git dock's branch review is against: an anchored overlay
//! card under the review's `against` field (AGENTS Hard rule 4 - chrome over the dock, never a
//! route), in the shared overlay card (`bg.elevated` + shadow + `border.strong` ring). It lists
//! the repository's default base branch, then every branch by name, most recently committed
//! first; typing filters them, and the current choice carries a check.
//!
//! This module is pure state + layout: the rows, the filter, the highlight and the scroll, a
//! proportional display list, and the pixel -> row hit-test. The binary owns opening it,
//! routing keys / clicks / the wheel, and keeping the choice in git (`Repo::choose_base`).

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, TextMeasure, Theme};

use crate::palette::{fit_label, fuzzy_match, matched_runs};

/// Layout constants in **logical** px (multiplied by the DPI scale when placed). The card pad,
/// item rows and pill match the tab menu's (§08), so the two overlays read as one family.
const PAD: f32 = 5.0;
/// The filter row's height.
const INPUT_H: f32 = 30.0;
/// An item row's height.
const ITEM_H: f32 = 28.0;
/// Item row inset from the card edge.
const ITEM_PAD_X: f32 = 9.0;
/// Item row corner radius.
const ITEM_RADIUS: f32 = 6.0;
/// The space above and below the rule under the filter.
const DIV_MARGIN: f32 = 4.0;
/// How much of `border.strong` the rule takes over the card.
const DIV_ALPHA: f32 = 0.3;
/// The width of the column the current choice's check sits in.
const CHECK_W: f32 = 20.0;
/// The gap between a row's label and its right-aligned hint.
const HINT_GAP: f32 = 18.0;
/// The most rows shown at once; more scroll.
const MAX_ROWS: usize = 10;
/// The card's width bounds.
const MIN_W: f32 = 240.0;
const MAX_W: f32 = 380.0;
/// The space between the field and the card under it.
const DROP_GAP: f32 = 4.0;
/// The filter's glyph (codicon `search`) and the current choice's (codicon `check`), drawn in
/// the bundled Nerd font.
const SEARCH_ICON: &str = "\u{EA6D}";
const CHECK_ICON: &str = "\u{EAB2}";
/// The filter's placeholder, and the line shown when nothing matches it.
const PLACEHOLDER: &str = "Filter branches";
const NO_MATCH: &str = "No branch matches";
/// The default row's label.
const DEFAULT_LABEL: &str = "Default branch";

/// What choosing a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    /// Review against the repository's default base branch: clear the choice.
    Default,
    /// Review against the branch with this name.
    Branch(String),
}

/// A row the filter shows: what it picks, and which of its label's characters the filter
/// matched.
struct Row {
    pick: Pick,
    positions: Vec<usize>,
}

/// An open base-branch drop-down.
pub(crate) struct BasePicker {
    /// The checked-out branch the choice is for.
    branch: String,
    /// The repository's default base branch, when it has one.
    default: Option<String>,
    /// Every branch it can be against, by name, most recently committed first.
    names: Vec<String>,
    /// What the branch review is against now.
    current: Pick,
    /// The filter as typed.
    query: String,
    /// The rows the filter shows.
    rows: Vec<Row>,
    /// The highlighted row's index into `rows`.
    selected: usize,
    /// The first row on screen.
    scroll: usize,
}

impl BasePicker {
    /// The drop-down for `branch`, whose review is against `current`, offering `default` (the
    /// repository's default base branch, when it has one) and `names`. The current choice is
    /// highlighted and in view.
    pub(crate) fn new(
        branch: String,
        default: Option<String>,
        names: Vec<String>,
        current: Pick,
    ) -> Self {
        let mut picker = Self {
            branch,
            default,
            names,
            current,
            query: String::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
        };
        picker.filter();
        if let Some(i) = picker.rows.iter().position(|r| r.pick == picker.current) {
            picker.selected = i;
            picker.reveal();
        }
        picker
    }

    /// The checked-out branch the choice is for.
    pub(crate) fn branch(&self) -> &str {
        &self.branch
    }

    /// What the branch review is against now.
    pub(crate) fn current(&self) -> &Pick {
        &self.current
    }

    /// Type `text` into the filter.
    pub(crate) fn type_text(&mut self, text: &str) {
        let before = self.query.len();
        self.query.extend(text.chars().filter(|c| !c.is_control()));
        if self.query.len() != before {
            self.filter();
        }
    }

    /// Delete the filter's last character.
    pub(crate) fn backspace(&mut self) {
        if self.query.pop().is_some() {
            self.filter();
        }
    }

    /// Move the highlight `delta` rows, stopping at the first and the last.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        let last = self.rows.len().saturating_sub(1);
        let step = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
        self.selected = if delta < 0 {
            self.selected.saturating_sub(step)
        } else {
            self.selected.saturating_add(step).min(last)
        };
        self.reveal();
    }

    /// Move the highlight a page.
    pub(crate) fn page(&mut self, delta: i32) {
        let rows = i32::try_from(MAX_ROWS).unwrap_or(1);
        self.move_selection(delta.saturating_mul(rows));
    }

    /// Scroll the list `delta` rows (the wheel), leaving the highlight where it is.
    pub(crate) fn scroll_rows(&mut self, delta: i32) {
        let top = self.rows.len().saturating_sub(MAX_ROWS);
        let step = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
        self.scroll = if delta < 0 {
            self.scroll.saturating_sub(step)
        } else {
            self.scroll.saturating_add(step).min(top)
        };
    }

    /// What the highlighted row picks (for `Enter`); `None` when nothing matches the filter.
    pub(crate) fn selected_pick(&self) -> Option<Pick> {
        self.rows.get(self.selected).map(|r| r.pick.clone())
    }

    /// What row `i` picks, highlighting it (for a click).
    pub(crate) fn pick_at(&mut self, i: usize) -> Option<Pick> {
        let pick = self.rows.get(i)?.pick.clone();
        self.selected = i;
        Some(pick)
    }

    /// Highlight the row under the pointer (for hover); whether the highlight moved.
    pub(crate) fn hover(&mut self, panel: PxRect, scale: f32, px: f32, py: f32) -> bool {
        match self.hit(panel, scale, px, py) {
            Some(i) if i != self.selected => {
                self.selected = i;
                true
            }
            _ => false,
        }
    }

    /// The card's natural size in **physical** px: wide enough for the longest branch name
    /// (within bounds, so it holds still while the filter narrows the rows), and tall enough
    /// for the rows on screen.
    pub(crate) fn natural_size(&self, scale: f32, measure: &mut TextMeasure) -> (f32, f32) {
        let mut widest = measure.width(PLACEHOLDER, FontRole::Body, None)
            + measure.width(SEARCH_ICON, FontRole::Icon, None)
            + ITEM_PAD_X * scale;
        let default_w = measure.width(DEFAULT_LABEL, FontRole::Body, None)
            + HINT_GAP * scale
            + measure.width(self.default_hint(), FontRole::Mono, None);
        widest = widest.max(default_w);
        for name in &self.names {
            widest = widest.max(measure.width(name, FontRole::Mono, None));
        }
        let w = (widest + (CHECK_W + 2.0 * ITEM_PAD_X + 2.0 * PAD) * scale)
            .clamp(MIN_W * scale, MAX_W * scale);
        let shown = self.rows.len().clamp(1, MAX_ROWS);
        let h = (Self::list_top() + rows_h(shown) + PAD) * scale;
        (w, h)
    }

    /// Place the card of `size` under `field` (physical px), right edges aligned as the field is
    /// right-anchored, shifted back inside the `surface` when it would spill past an edge.
    pub(crate) fn place(
        field: PxRect,
        size: (f32, f32),
        surface: (f32, f32),
        scale: f32,
    ) -> PxRect {
        let (w, h) = size;
        let x = (field.x + field.w - w).min(surface.0 - w).max(0.0);
        let y = (field.y + field.h + DROP_GAP * scale)
            .min(surface.1 - h)
            .max(0.0);
        PxRect { x, y, w, h }
    }

    /// The row under physical `(px, py)`, if any.
    pub(crate) fn hit(&self, panel: PxRect, scale: f32, px: f32, py: f32) -> Option<usize> {
        if px < panel.x || px >= panel.x + panel.w {
            return None;
        }
        let top = panel.y + Self::list_top() * scale;
        (0..MAX_ROWS)
            .find(|&k| {
                let y = top + rows_h(k) * scale;
                py >= y && py < y + ITEM_H * scale
            })
            .map(|k| self.scroll + k)
            .filter(|&i| i < self.rows.len())
    }

    /// Build the card's content within `panel` (physical px; the renderer draws the card
    /// itself): the filter with its caret, the rule under it, the highlighted row's
    /// `accent.subtle` pill, and each row's check, label (matched characters in `accent`) and
    /// hint.
    pub(crate) fn build(
        &self,
        panel: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
        let mut quads = Vec::new();
        let mut labels = Vec::new();
        let left = panel.x + (PAD + ITEM_PAD_X) * scale;
        let right = panel.x + panel.w - (PAD + ITEM_PAD_X) * scale;
        self.push_filter(&mut quads, &mut labels, panel, scale, theme, measure);
        quads.push(ChromeQuad::fill(
            PxRect {
                x: panel.x + PAD * scale,
                y: panel.y + (PAD + INPUT_H + DIV_MARGIN) * scale,
                w: panel.w - 2.0 * PAD * scale,
                h: scale.max(1.0),
            },
            theme.border_strong.over(theme.bg_elevated, DIV_ALPHA),
        ));
        let list_top = panel.y + Self::list_top() * scale;
        let row_h = ITEM_H * scale;
        if self.rows.is_empty() {
            push_centered_in_row(
                &mut labels,
                measure,
                NO_MATCH,
                FontRole::Body,
                theme.fg_muted,
                left,
                list_top,
                row_h,
            );
            return (quads, labels);
        }
        let shown = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(MAX_ROWS);
        for (offset, (i, row)) in shown.enumerate() {
            let top = list_top + rows_h(offset) * scale;
            let selected = i == self.selected;
            if selected {
                quads.push(ChromeQuad::rounded(
                    PxRect {
                        x: panel.x + PAD * scale,
                        y: top + 1.0 * scale,
                        w: panel.w - 2.0 * PAD * scale,
                        h: row_h - 2.0 * scale,
                    },
                    theme.accent_subtle_on(theme.bg_elevated),
                    ITEM_RADIUS * scale,
                ));
            }
            let content = PxRect {
                x: left,
                y: top,
                w: right - left,
                h: row_h,
            };
            self.push_row(&mut labels, row, selected, content, scale, theme, measure);
        }
        (quads, labels)
    }

    /// One row's text within `content` (physical px): the check on the current choice, then the
    /// label, `fg.primary` while `selected`, with the characters the filter matched in `accent`.
    /// The default row's hint, right-aligned, names what the default is.
    #[allow(clippy::too_many_arguments, reason = "one focused row builder")]
    fn push_row(
        &self,
        labels: &mut Vec<ProseLabel>,
        row: &Row,
        selected: bool,
        content: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) {
        let (top, row_h) = (content.y, content.h);
        let right = content.x + content.w;
        if row.pick == self.current {
            push_centered_in_row(
                labels,
                measure,
                CHECK_ICON,
                FontRole::Icon,
                theme.accent,
                content.x,
                top,
                row_h,
            );
        }
        let text_color = if selected {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        let label_x = content.x + CHECK_W * scale;
        match &row.pick {
            Pick::Default => {
                let hint = self.default_hint();
                let hint_w = measure.width(hint, FontRole::Mono, None);
                push_centered_in_row(
                    labels,
                    measure,
                    DEFAULT_LABEL,
                    FontRole::Body,
                    text_color,
                    label_x,
                    top,
                    row_h,
                );
                let hint_x = (right - hint_w)
                    .max(label_x + measure.width(DEFAULT_LABEL, FontRole::Body, None));
                push_centered_in_row(
                    labels,
                    measure,
                    hint,
                    FontRole::Mono,
                    theme.fg_muted,
                    hint_x,
                    top,
                    row_h,
                );
            }
            Pick::Branch(name) => {
                let (text, positions) = fit_label(
                    name,
                    &row.positions,
                    (right - label_x).max(0.0),
                    FontRole::Mono,
                    measure,
                );
                let mut x = label_x;
                for (run, matched) in matched_runs(&text, &positions) {
                    let color = if matched { theme.accent } else { text_color };
                    let w = measure.width(&run, FontRole::Mono, None);
                    push_centered_in_row(
                        labels,
                        measure,
                        &run,
                        FontRole::Mono,
                        color,
                        x,
                        top,
                        row_h,
                    );
                    x += w;
                }
            }
        }
    }

    /// The filter row: the search glyph, then the filter (or its placeholder) and a caret.
    fn push_filter(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        panel: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) {
        let top = panel.y + PAD * scale;
        let row_h = INPUT_H * scale;
        let x = panel.x + (PAD + ITEM_PAD_X) * scale;
        push_centered_in_row(
            labels,
            measure,
            SEARCH_ICON,
            FontRole::Icon,
            theme.fg_muted,
            x,
            top,
            row_h,
        );
        let text_x = x + CHECK_W * scale;
        let (text, color) = if self.query.is_empty() {
            (PLACEHOLDER, theme.fg_muted)
        } else {
            (self.query.as_str(), theme.fg_primary)
        };
        push_centered_in_row(
            labels,
            measure,
            text,
            FontRole::Body,
            color,
            text_x,
            top,
            row_h,
        );
        let caret_x = if self.query.is_empty() {
            text_x
        } else {
            text_x + measure.width(&self.query, FontRole::Body, None)
        };
        let line_h = measure.line_height(FontRole::Body);
        quads.push(ChromeQuad::fill(
            PxRect {
                x: caret_x,
                y: top + (row_h - line_h) * 0.5,
                w: (2.0 * scale).max(1.0),
                h: line_h,
            },
            theme.accent,
        ));
    }

    /// What the default row says it is: the default base branch, or `HEAD` without one.
    fn default_hint(&self) -> &str {
        self.default.as_deref().unwrap_or("HEAD")
    }

    /// The list's top within the card, in logical px.
    fn list_top() -> f32 {
        PAD + INPUT_H + 2.0 * DIV_MARGIN + 1.0
    }

    /// Rebuild the rows for the filter: with none, the default then every branch as listed;
    /// with one, the branches it matches, best match first. The highlight goes to the first.
    fn filter(&mut self) {
        self.rows = if self.query.trim().is_empty() {
            std::iter::once(Row {
                pick: Pick::Default,
                positions: Vec::new(),
            })
            .chain(self.names.iter().map(|name| Row {
                pick: Pick::Branch(name.clone()),
                positions: Vec::new(),
            }))
            .collect()
        } else {
            let mut matched: Vec<(i32, Row)> = self
                .names
                .iter()
                .filter_map(|name| {
                    let (score, positions) = fuzzy_match(&self.query, name)?;
                    Some((
                        score,
                        Row {
                            pick: Pick::Branch(name.clone()),
                            positions,
                        },
                    ))
                })
                .collect();
            // Stable, so equal matches keep the most recently committed first.
            matched.sort_by_key(|m| std::cmp::Reverse(m.0));
            matched.into_iter().map(|(_, row)| row).collect()
        };
        self.selected = 0;
        self.scroll = 0;
    }

    /// Scroll the highlighted row into view.
    fn reveal(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + MAX_ROWS {
            self.scroll = self.selected + 1 - MAX_ROWS;
        }
    }
}

/// The height of `n` rows, in logical px.
#[allow(clippy::cast_precision_loss, reason = "a row count is a small value")]
fn rows_h(n: usize) -> f32 {
    n as f32 * ITEM_H
}

/// Push `text` at `x`, centered in a row of physical height `row_h` whose top is `top`.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_centered_in_row(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: skelly_render::Srgb,
    x: f32,
    top: f32,
    row_h: f32,
) {
    let line_h = measure.line_height(role);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x,
        y: top + (row_h - line_h) * 0.5,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

#[cfg(test)]
mod tests {
    use super::{BasePicker, Pick, PxRect, MAX_ROWS};

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("feat-{i}")).collect()
    }

    fn picker(current: Pick) -> BasePicker {
        BasePicker::new(
            "stack/top".to_owned(),
            Some("origin/main".to_owned()),
            vec!["feat-a".into(), "main".into(), "fix/feat-ab".into()],
            current,
        )
    }

    fn shown(p: &BasePicker) -> Vec<Pick> {
        p.rows.iter().map(|r| r.pick.clone()).collect()
    }

    #[test]
    fn the_default_comes_first_then_every_branch_as_listed() {
        let p = picker(Pick::Default);
        assert_eq!(
            shown(&p),
            [
                Pick::Default,
                Pick::Branch("feat-a".into()),
                Pick::Branch("main".into()),
                Pick::Branch("fix/feat-ab".into()),
            ]
        );
        assert_eq!(p.selected_pick(), Some(Pick::Default), "the current choice");
        assert_eq!(p.branch(), "stack/top");
    }

    #[test]
    fn it_opens_on_the_current_choice() {
        let p = picker(Pick::Branch("main".into()));
        assert_eq!(p.selected_pick(), Some(Pick::Branch("main".into())));

        let mut many = BasePicker::new(
            "top".into(),
            None,
            names(40),
            Pick::Branch("feat-30".into()),
        );
        assert_eq!(many.selected_pick(), Some(Pick::Branch("feat-30".into())));
        assert!(
            (many.scroll..many.scroll + MAX_ROWS).contains(&many.selected),
            "in view"
        );
        many.move_selection(-100);
        assert_eq!((many.selected, many.scroll), (0, 0));
    }

    #[test]
    fn typing_filters_the_branches_best_match_first() {
        let mut p = picker(Pick::Default);
        p.type_text("feat-a");
        assert_eq!(
            shown(&p),
            [
                Pick::Branch("feat-a".into()),
                Pick::Branch("fix/feat-ab".into())
            ],
            "the default is not a branch to filter"
        );
        assert_eq!(p.selected_pick(), Some(Pick::Branch("feat-a".into())));
        p.type_text("zzz");
        assert!(shown(&p).is_empty());
        assert_eq!(p.selected_pick(), None, "Enter picks nothing");
        for _ in 0..3 {
            p.backspace();
        }
        p.type_text("\u{7f}\n");
        assert_eq!(p.query, "feat-a", "control characters are not typed");
        for _ in 0..6 {
            p.backspace();
        }
        assert_eq!(shown(&p).len(), 4, "an empty filter shows everything again");
    }

    #[test]
    fn the_highlight_and_the_wheel_stay_inside_the_list() {
        let mut p = BasePicker::new("top".into(), None, names(25), Pick::Default);
        p.move_selection(-1);
        assert_eq!(p.selected, 0);
        p.page(1);
        assert_eq!(p.selected, MAX_ROWS);
        assert_eq!(p.scroll, 1, "the highlight is kept in view");
        p.move_selection(100);
        assert_eq!(p.selected, 25, "the last row (the default is row 0)");
        assert_eq!(p.scroll, 26 - MAX_ROWS);
        p.scroll_rows(-100);
        assert_eq!(p.scroll, 0);
        p.scroll_rows(100);
        assert_eq!(p.scroll, 26 - MAX_ROWS, "no further than the last page");
        assert_eq!(p.selected, 25, "the wheel leaves the highlight");
    }

    #[test]
    fn a_click_lands_on_the_row_drawn_under_it() {
        let mut p = BasePicker::new("top".into(), None, names(25), Pick::Default);
        p.scroll_rows(3);
        let scale = 2.0;
        let panel = PxRect {
            x: 100.0,
            y: 50.0,
            w: 500.0,
            h: 800.0,
        };
        let top = panel.y + BasePicker::list_top() * scale;
        let row = super::ITEM_H * scale;
        assert_eq!(p.hit(panel, scale, 200.0, top - 1.0), None, "the filter");
        assert_eq!(p.hit(panel, scale, 200.0, top + 1.0), Some(3));
        assert_eq!(p.hit(panel, scale, 200.0, top + 2.5 * row), Some(5));
        assert_eq!(
            p.hit(panel, scale, 99.0, top + 1.0),
            None,
            "left of the card"
        );
        assert_eq!(
            p.hit(panel, scale, 200.0, top + 10.5 * row),
            None,
            "below the last row on screen"
        );
        assert_eq!(p.pick_at(5), Some(Pick::Branch("feat-4".into())));
        assert_eq!(p.selected, 5);
        assert!(
            !p.hover(panel, scale, 200.0, top + 2.5 * row),
            "already there"
        );
        assert!(p.hover(panel, scale, 200.0, top + 0.5 * row));
        assert_eq!(p.selected, 3);
    }

    #[test]
    fn the_card_drops_down_under_the_field_inside_the_window() {
        let field = PxRect {
            x: 700.0,
            y: 40.0,
            w: 120.0,
            h: 24.0,
        };
        let placed = BasePicker::place(field, (300.0, 200.0), (1000.0, 800.0), 1.0);
        assert_eq!(
            (placed.x, placed.y),
            (520.0, 68.0),
            "right edges aligned, just under the field"
        );
        let placed = BasePicker::place(field, (300.0, 200.0), (1000.0, 240.0), 1.0);
        assert!(
            (placed.y - 40.0).abs() < f32::EPSILON,
            "shifted up to stay in a short window"
        );
        let narrow = PxRect { x: 10.0, ..field };
        let placed = BasePicker::place(narrow, (300.0, 200.0), (1000.0, 800.0), 1.0);
        assert!(placed.x.abs() < f32::EPSILON, "never past the left edge");
    }
}
