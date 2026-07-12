//! The command palette: a centered overlay listing runnable commands, filtered by a
//! typed query and navigated by keyboard (AGENTS Hard rule 4 - an overlay over the
//! live terminal, never a route). This module is pure state + layout: it owns the query,
//! the filtered selection, and builds the palette as a *proportional* display list
//! (decorative quads + positioned labels in the guide's fonts, §09 "Command palette row").
//! The binary owns opening it, routing keys, and executing the chosen command.
//!
//! A leading mode prefix narrows the results (design §10.8): `>` restricts to commands, `?`
//! shows the keybinding help, `/` is file search (deferred), and no prefix is the universal
//! mode that also surfaces the open tabs (themes are already commands). The built-in
//! [`COMMANDS`] set is the seed of the keybinding registry; merging user `[keys]` overrides +
//! file search + scrollback search are later slices.

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};

/// Layout constants in **logical** px (multiplied by the DPI scale). Tuned to the guide's
/// §09 palette: an input line, a result count, `accent.subtle` command rows with a
/// right-anchored key hint, and a caption footer.
const PAD: f32 = 14.0;
/// Horizontal inset of a row's text from the padded content edge (leaves room for the pill).
const ROW_INSET: f32 = 10.0;
/// Input row height.
const INPUT_H: f32 = 34.0;
/// Result-count row height.
const COUNT_H: f32 = 22.0;
/// Command row height (the guide's list rows).
const CMD_H: f32 = 30.0;
/// Category-header row height (design §10.8: a small uppercase mono label above each group).
const CAT_H: f32 = 20.0;
/// Gap between a command's icon and its label.
const ICON_GAP: f32 = 12.0;
/// Spacer height between the results and the footer.
const SPACER_H: f32 = 8.0;
/// Footer row height.
const FOOTER_H: f32 = 24.0;
/// Minimum gap between a command label and its right-anchored key hint.
const HINT_GAP: f32 = 24.0;
/// Corner radius (logical px) of the selected-row pill (the guide's `md` radius).
const PILL_RADIUS: f32 = 8.0;

/// A runnable command surfaced in the palette.
pub(crate) struct Command {
    /// The category header this command sits under (design §10.8 grouped list). Consecutive
    /// commands sharing a category render under one header.
    pub(crate) category: &'static str,
    /// The reference glyph shown left of the label (accent when selected, else `fg.muted`),
    /// from the §07 icon set.
    pub(crate) icon: &'static str,
    /// The human label, shown left-aligned.
    pub(crate) label: &'static str,
    /// The default key-chord hint, shown right-aligned in muted text.
    pub(crate) hint: &'static str,
    /// What running it does.
    pub(crate) action: Action,
}

/// What a command does when run. The binary maps each to its existing handlers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Action {
    /// Split the focused pane to the right.
    SplitRight,
    /// Split the focused pane downward.
    SplitDown,
    /// Toggle zoom on the focused pane.
    Zoom,
    /// Reset every split to 50/50.
    EvenOut,
    /// Close the focused pane.
    ClosePane,
    /// Move focus left / down / up / right.
    FocusLeft,
    /// Move focus down.
    FocusDown,
    /// Move focus up.
    FocusUp,
    /// Move focus right.
    FocusRight,
    /// Open a new tab and switch to it.
    NewTab,
    /// Close the active tab.
    CloseTab,
    /// Switch to the next tab.
    NextTab,
    /// Switch to the previous tab.
    PrevTab,
    /// Switch to the tab at this 0-based index (a surfaced tab entry, §10.8).
    GotoTab(usize),
    /// Pin or unpin the active tab (move it in/out of the 3-up pinned grid, §08 #4).
    TogglePin,
    /// Show or hide the left sidebar.
    ToggleSidebar,
    /// Cycle the sidebar between the full panel and the slim icon rail.
    CycleSidebarMode,
    /// Open or close the per-repo git diff dock.
    ShowGitDiff,
    /// Open or close the session-timeline dock.
    ShowTimeline,
    /// Open the full-window settings view.
    OpenSettings,
    /// Switch the UI theme to Ossein Dark.
    ThemeDark,
    /// Switch the UI theme to Ossein Light.
    ThemeLight,
    /// Quit the application.
    Quit,
}

/// The built-in command set. Order is the display order; consecutive commands share a
/// category header (design §10.8). Icons are §07 reference glyphs.
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        category: "Panes",
        icon: "\u{25AF}", // ▯ split vertical
        label: "Split pane right",
        hint: "opt |",
        action: Action::SplitRight,
    },
    Command {
        category: "Panes",
        icon: "\u{25AD}", // ▭ split horizontal
        label: "Split pane down",
        hint: "opt -",
        action: Action::SplitDown,
    },
    Command {
        category: "Panes",
        icon: "\u{2922}", // ⤢ zoom
        label: "Zoom / unzoom pane",
        hint: "opt Z",
        action: Action::Zoom,
    },
    Command {
        category: "Panes",
        icon: "\u{229E}", // ⊞ even out
        label: "Even out splits",
        hint: "opt =",
        action: Action::EvenOut,
    },
    Command {
        category: "Panes",
        icon: "\u{2715}", // ✕ close
        label: "Close pane",
        hint: "opt W",
        action: Action::ClosePane,
    },
    Command {
        category: "Panes",
        icon: "\u{2190}", // ←
        label: "Focus pane left",
        hint: "opt H",
        action: Action::FocusLeft,
    },
    Command {
        category: "Panes",
        icon: "\u{2193}", // ↓
        label: "Focus pane down",
        hint: "opt J",
        action: Action::FocusDown,
    },
    Command {
        category: "Panes",
        icon: "\u{2191}", // ↑
        label: "Focus pane up",
        hint: "opt K",
        action: Action::FocusUp,
    },
    Command {
        category: "Panes",
        icon: "\u{2192}", // →
        label: "Focus pane right",
        hint: "opt L",
        action: Action::FocusRight,
    },
    Command {
        category: "Tabs",
        icon: "+",
        label: "New tab",
        hint: "cmd T",
        action: Action::NewTab,
    },
    Command {
        category: "Tabs",
        icon: "\u{2715}", // ✕
        label: "Close tab",
        hint: "cmd W",
        action: Action::CloseTab,
    },
    Command {
        category: "Tabs",
        icon: "\u{25C8}", // ◈ pin
        label: "Pin / unpin tab",
        hint: "shift cmd P",
        action: Action::TogglePin,
    },
    Command {
        category: "Tabs",
        icon: "\u{203A}", // › next
        label: "Next tab",
        hint: "opt shift ]",
        action: Action::NextTab,
    },
    Command {
        category: "Tabs",
        icon: "\u{2039}", // ‹ previous
        label: "Previous tab",
        hint: "opt shift [",
        action: Action::PrevTab,
    },
    Command {
        category: "View",
        icon: "\u{25A4}", // ▤ sidebar
        label: "Toggle sidebar",
        hint: "cmd B",
        action: Action::ToggleSidebar,
    },
    Command {
        category: "View",
        icon: "\u{25A4}", // ▤ sidebar
        label: "Cycle sidebar mode",
        hint: "shift cmd B",
        action: Action::CycleSidebarMode,
    },
    Command {
        category: "View",
        icon: "\u{00B1}", // ± diff
        label: "Show git diff",
        hint: "shift cmd G",
        action: Action::ShowGitDiff,
    },
    Command {
        category: "View",
        icon: "\u{27F2}", // ⟲ timeline
        label: "Show session timeline",
        hint: "shift cmd H",
        action: Action::ShowTimeline,
    },
    Command {
        category: "View",
        icon: "\u{2699}", // ⚙ settings
        label: "Open settings",
        hint: "cmd ,",
        action: Action::OpenSettings,
    },
    Command {
        category: "Appearance",
        icon: "\u{25D0}", // ◐ theme
        label: "Theme: Ossein Dark",
        hint: "",
        action: Action::ThemeDark,
    },
    Command {
        category: "Appearance",
        icon: "\u{25D0}", // ◐ theme
        label: "Theme: Ossein Light",
        hint: "",
        action: Action::ThemeLight,
    },
    Command {
        category: "Session",
        icon: "\u{23FB}", // ⏻ power
        label: "Quit skelly",
        hint: "cmd Q",
        action: Action::Quit,
    },
];

/// The palette's proportional display list for a [`skelly_render::OverlayView`]: the
/// content quads (the selected-row pill + the input caret) and the positioned labels.
pub(crate) struct Paint {
    /// The content quads over the card, in draw order.
    pub(crate) quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub(crate) labels: Vec<ProseLabel>,
}

/// A search mode selected by the query's leading prefix (design §10.8).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// No prefix: commands + surfaced tabs (themes are commands).
    All,
    /// `>`: commands only.
    Commands,
    /// `?`: the keybinding help (the command list as a reference).
    Help,
    /// `/`: file search - deferred (shows an empty hint).
    Files,
}

impl Mode {
    /// The prompt glyph shown at the input's left for this mode.
    fn prompt(self) -> &'static str {
        match self {
            Mode::All | Mode::Commands | Mode::Help => ">",
            Mode::Files => "/",
        }
    }
}

/// One thing the palette can surface and run.
#[derive(Clone, Copy)]
enum Entry {
    /// A built-in command, by [`COMMANDS`] index.
    Command(usize),
    /// Switch to the open tab at this 0-based index.
    Tab(usize),
}

/// Palette state: whether it is open, the query, the selected match, and a snapshot of the
/// open tab titles (captured at open time, since the palette captures all input while up).
pub(crate) struct Palette {
    /// Whether the palette overlay is showing.
    pub(crate) open: bool,
    query: String,
    selected: usize,
    tabs: Vec<String>,
}

impl Palette {
    /// A closed, empty palette.
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            tabs: Vec::new(),
        }
    }

    /// Open the palette fresh (empty query, first match selected), snapshotting the open tab
    /// titles so they can be surfaced as entries.
    pub(crate) fn open(&mut self, tabs: Vec<String>) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.tabs = tabs;
    }

    /// The active mode + the search term (the query minus any leading mode prefix).
    fn mode_and_term(&self) -> (Mode, &str) {
        let q = self.query.as_str();
        match q.as_bytes().first() {
            Some(b'>') => (Mode::Commands, q[1..].trim_start()),
            Some(b'?') => (Mode::Help, q[1..].trim_start()),
            Some(b'/') => (Mode::Files, q[1..].trim_start()),
            _ => (Mode::All, q.trim()),
        }
    }

    /// Close the palette.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// The rows the palette shows for the current query, in a stable category-grouped order
    /// (so the §10.8 headers stay coherent - a match never jumps out of its group): the
    /// fuzzy-matched commands, then in the universal mode the matched open tabs. An empty term
    /// matches everything in the active mode; the deferred file mode yields nothing.
    fn rows(&self) -> Vec<Row> {
        let (mode, term) = self.mode_and_term();
        if mode == Mode::Files {
            return Vec::new();
        }
        let mut rows: Vec<Row> = COMMANDS
            .iter()
            .enumerate()
            .filter_map(|(index, cmd)| {
                fuzzy_match(term, cmd.label).map(|(_, positions)| Row {
                    entry: Entry::Command(index),
                    category: cmd.category,
                    icon: cmd.icon,
                    label: cmd.label.to_owned(),
                    hint: cmd.hint,
                    positions,
                })
            })
            .collect();
        // The universal mode also surfaces the open tabs (under their own group).
        if mode == Mode::All {
            for (index, title) in self.tabs.iter().enumerate() {
                if let Some((_, positions)) = fuzzy_match(term, title) {
                    rows.push(Row {
                        entry: Entry::Tab(index),
                        category: "Tabs",
                        icon: "\u{276f}", // ❯
                        label: title.clone(),
                        hint: "",
                        positions,
                    });
                }
            }
        }
        rows
    }

    /// Move the selection by `delta`, clamped to the current row count.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        let count = self.rows().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let current = i32::try_from(self.selected).unwrap_or(i32::MAX);
        let last = i32::try_from(count - 1).unwrap_or(i32::MAX);
        let next = (current + delta).clamp(0, last);
        self.selected = usize::try_from(next).unwrap_or(0);
    }

    /// Append a typed character to the query and reset the selection.
    pub(crate) fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
    }

    /// Delete the last query character and reset the selection.
    pub(crate) fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// The action of the currently selected row, if any (run a command, or switch to a tab).
    pub(crate) fn selected_action(&self) -> Option<Action> {
        self.rows().get(self.selected).map(|row| match row.entry {
            Entry::Command(index) => COMMANDS[index].action,
            Entry::Tab(index) => Action::GotoTab(index),
        })
    }

    /// The palette's natural panel size in **physical** px (including the card padding),
    /// for the binary to center + animate the card. Width is the widest content row (a
    /// command's label + gap + hint, the footer, or the input) plus insets; height is the
    /// sum of the fixed row heights.
    pub(crate) fn natural_size(&self, scale: f32, measure: &mut TextMeasure) -> (f32, f32) {
        let rows = self.rows();
        let inset = (PAD + ROW_INSET) * scale;
        let mut content_w = measure.width(FOOTER, FontRole::Caption, None);
        // The input line (prompt + term or placeholder).
        let (_, term) = self.mode_and_term();
        let shown = if term.is_empty() { PLACEHOLDER } else { term };
        content_w = content_w.max(
            measure.width("> ", FontRole::Body, None) + measure.width(shown, FontRole::Body, None),
        );
        let mut current_category = "";
        let mut categories = 0_usize;
        for row in &rows {
            if row.category != current_category {
                current_category = row.category;
                categories += 1;
            }
            let w = measure.width(row.icon, FontRole::Body, None)
                + ICON_GAP * scale
                + measure.width(&row.label, FontRole::Body, None)
                + HINT_GAP * scale
                + measure.width(row.hint, FontRole::Micro, None);
            content_w = content_w.max(w);
        }
        let width = content_w + 2.0 * inset;
        #[allow(
            clippy::cast_precision_loss,
            reason = "the match + category counts are small, exact values"
        )]
        let rows_h = INPUT_H
            + COUNT_H
            + categories as f32 * CAT_H
            + rows.len() as f32 * CMD_H
            + SPACER_H
            + FOOTER_H;
        let height = rows_h * scale + 2.0 * PAD * scale;
        (width, height)
    }

    /// Build the palette's content display list within `panel` (the renderer draws the card
    /// itself): the selected-row `accent.subtle` pill + the input caret as quads, and the
    /// prompt / query / count / command rows (matched chars in `accent`, key hints right) /
    /// footer as proportional labels.
    pub(crate) fn build(
        &self,
        panel: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> Paint {
        let mut quads = Vec::new();
        let mut labels = Vec::new();
        let cx = panel.x + PAD * scale;
        let cw = panel.w - 2.0 * PAD * scale;
        let prompt_x = cx + ROW_INSET * scale;
        let mut y = panel.y + PAD * scale;

        self.push_input(&mut quads, &mut labels, prompt_x, y, scale, theme, measure);
        y += INPUT_H * scale;

        // Result count (or the deferred-file-search hint).
        let (mode, _) = self.mode_and_term();
        let rows = self.rows();
        let count = if mode == Mode::Files {
            "file search is coming soon".to_owned()
        } else if rows.len() == 1 {
            "1 result".to_owned()
        } else {
            format!("{} results", rows.len())
        };
        push_line(
            &mut labels,
            &count,
            FontRole::Caption,
            theme.fg_muted,
            prompt_x,
            y,
            COUNT_H,
            scale,
            measure,
        );
        y += COUNT_H * scale;

        // Rows, grouped under a category header whenever the category changes.
        let mut current_category = "";
        for (index, row) in rows.iter().enumerate() {
            if row.category != current_category {
                current_category = row.category;
                push_line(
                    &mut labels,
                    &current_category.to_uppercase(),
                    FontRole::Micro,
                    theme.fg_faint,
                    prompt_x,
                    y,
                    CAT_H,
                    scale,
                    measure,
                );
                y += CAT_H * scale;
            }
            let selected = index == self.selected;
            if selected {
                let inset = ROW_INSET * 0.5 * scale;
                // accent.subtle (§03) selected-row pill, composited in sRGB over the palette
                // card (bg.elevated) so it reads at the guide's weight, not the brighter blend.
                quads.push(ChromeQuad::rounded(
                    PxRect {
                        x: cx + inset,
                        y,
                        w: (cw - 2.0 * inset).max(0.0),
                        h: CMD_H * scale,
                    },
                    theme.accent_subtle_on(theme.bg_elevated),
                    PILL_RADIUS * scale,
                ));
            }
            push_command(&mut labels, row, selected, cx, cw, y, scale, theme, measure);
            y += CMD_H * scale;
        }

        y += SPACER_H * scale;
        push_line(
            &mut labels,
            FOOTER,
            FontRole::Caption,
            theme.fg_muted,
            prompt_x,
            y,
            FOOTER_H,
            scale,
            measure,
        );

        Paint { quads, labels }
    }

    /// The input line: "> " (accent) then the query (primary) or placeholder (muted), with a
    /// caret bar after the text. `prompt_x` is the left edge; the row top is physical `y`.
    #[allow(clippy::too_many_arguments, reason = "one focused input-row builder")]
    fn push_input(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        prompt_x: f32,
        y: f32,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) {
        // The mode prompt (accent), then the search term (the query minus its mode prefix).
        let (mode, term) = self.mode_and_term();
        push_line(
            labels,
            mode.prompt(),
            FontRole::Body,
            theme.accent,
            prompt_x,
            y,
            INPUT_H,
            scale,
            measure,
        );
        let query_x = prompt_x
            + measure.width(mode.prompt(), FontRole::Body, None)
            + measure.width(" ", FontRole::Body, None);
        let (query_text, query_color) = if term.is_empty() {
            (PLACEHOLDER, theme.fg_muted)
        } else {
            (term, theme.fg_primary)
        };
        push_line(
            labels,
            query_text,
            FontRole::Body,
            query_color,
            query_x,
            y,
            INPUT_H,
            scale,
            measure,
        );
        // Caret after the term (or at the input start when empty).
        let caret_x = query_x
            + if term.is_empty() {
                0.0
            } else {
                measure.width(term, FontRole::Body, None)
            };
        let line_h = measure.line_height(FontRole::Body);
        quads.push(ChromeQuad::fill(
            PxRect {
                x: caret_x,
                y: y + (INPUT_H * scale - line_h) * 0.5,
                w: (2.0 * scale).max(1.0),
                h: line_h,
            },
            theme.accent,
        ));
    }
}

/// A matched, renderable palette row: what it runs plus everything [`build`] needs to draw it.
struct Row {
    entry: Entry,
    category: &'static str,
    icon: &'static str,
    label: String,
    hint: &'static str,
    positions: Vec<usize>,
}

/// Fuzzy subsequence match: `Some((score, matched positions))` if every non-space
/// character of `query` appears in order in `label` (ASCII case-insensitive), else
/// `None`. An empty query matches with no highlights. Higher score is better - an
/// earlier first match and fewer gaps between matched characters win.
fn fuzzy_match(query: &str, label: &str) -> Option<(i32, Vec<usize>)> {
    let mut wanted = query.chars().filter(|c| !c.is_whitespace()).peekable();
    if wanted.peek().is_none() {
        return Some((0, Vec::new()));
    }
    let mut positions = Vec::new();
    let mut current = wanted.next();
    for (i, lc) in label.chars().enumerate() {
        let Some(q) = current else { break };
        if lc.eq_ignore_ascii_case(&q) {
            positions.push(i);
            current = wanted.next();
        }
    }
    if current.is_some() {
        return None; // ran out of label before matching every query character
    }
    let first = i32::try_from(positions.first().copied().unwrap_or(0)).unwrap_or(0);
    let gaps =
        i32::try_from(positions.windows(2).filter(|w| w[1] != w[0] + 1).count()).unwrap_or(0);
    Some((-first - gaps * 2, positions))
}

/// The footer hint line.
const FOOTER: &str = "up/down navigate    enter run    esc close";
/// The empty-input placeholder.
const PLACEHOLDER: &str = "search commands";

impl Default for Palette {
    fn default() -> Self {
        Self::new()
    }
}

/// Push one proportional label, vertically centered in a row of `row_h` logical px whose
/// top is physical `top`.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_line(
    labels: &mut Vec<ProseLabel>,
    text: &str,
    role: FontRole,
    color: Srgb,
    x: f32,
    top: f32,
    row_h: f32,
    scale: f32,
    measure: &mut TextMeasure,
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

/// A command row: the `icon` (accent when `selected`, else `fg.muted`), then the `label`
/// left-aligned with matched `positions` in `accent` (the rest `fg.primary`), and the key
/// `hint` right-anchored in muted `micro` mono.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_command(
    labels: &mut Vec<ProseLabel>,
    row: &Row,
    selected: bool,
    cx: f32,
    cw: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) {
    let line_h = measure.line_height(FontRole::Body);
    let y = top + (CMD_H * scale - line_h) * 0.5;
    // The reference-glyph icon (accent when selected, else muted), then the label after a gap.
    let icon_x = cx + ROW_INSET * scale;
    labels.push(ProseLabel {
        text: row.icon.to_owned(),
        x: icon_x,
        y,
        role: FontRole::Body,
        color: if selected {
            theme.accent
        } else {
            theme.fg_muted
        },
        weight: None,
        max_w: f32::MAX,
    });
    // The label, split into matched / unmatched runs so matched chars draw in accent.
    let mut x = icon_x + measure.width(row.icon, FontRole::Body, None) + ICON_GAP * scale;
    for (text, matched) in matched_runs(&row.label, &row.positions) {
        let color = if matched {
            theme.accent
        } else {
            theme.fg_primary
        };
        let w = measure.width(&text, FontRole::Body, None);
        labels.push(ProseLabel {
            text,
            x,
            y,
            role: FontRole::Body,
            color,
            weight: None,
            max_w: f32::MAX,
        });
        x += w;
    }
    // The right-anchored key hint (micro mono, muted).
    if !row.hint.is_empty() {
        let hint_w = measure.width(row.hint, FontRole::Micro, None);
        let hint_x = cx + cw - ROW_INSET * scale - hint_w;
        let hint_line = measure.line_height(FontRole::Micro);
        labels.push(ProseLabel {
            text: row.hint.to_owned(),
            x: hint_x,
            y: top + (CMD_H * scale - hint_line) * 0.5,
            role: FontRole::Micro,
            color: theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
        });
    }
}

/// Split `label` into consecutive `(text, matched)` runs by whether each character's index
/// is in `positions` (the fuzzy-matched positions), so matched runs can draw in accent.
fn matched_runs(label: &str, positions: &[usize]) -> Vec<(String, bool)> {
    let mut runs: Vec<(String, bool)> = Vec::new();
    for (i, ch) in label.chars().enumerate() {
        let matched = positions.contains(&i);
        match runs.last_mut() {
            Some((text, m)) if *m == matched => text.push(ch),
            _ => runs.push((ch.to_string(), matched)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::{matched_runs, Action, Entry, Palette, PxRect, TextMeasure, Theme, COMMANDS};

    impl Palette {
        /// Test-only: the action each visible row would run, in display order.
        fn match_actions(&self) -> Vec<Action> {
            self.rows()
                .iter()
                .map(|r| match r.entry {
                    Entry::Command(i) => COMMANDS[i].action,
                    Entry::Tab(i) => Action::GotoTab(i),
                })
                .collect()
        }
    }

    #[test]
    fn matched_characters_render_in_accent_and_the_rest_in_primary() {
        // "Zoom / unzoom pane" matched by "zm": z at 0, m at 3. matched_runs splits into
        // ["Z"(matched), "oo"(not), "m"(matched), " / unzoom pane"(not)], so the label draws
        // the matched runs in accent.
        let runs = matched_runs("Zoom / unzoom pane", &[0, 3]);
        assert_eq!(runs[0], ("Z".to_owned(), true));
        assert_eq!(runs[1], ("oo".to_owned(), false));
        assert_eq!(runs[2], ("m".to_owned(), true));
        assert!(runs[3].0.starts_with(" / "));
        assert!(!runs[3].1);
    }

    #[test]
    fn build_emits_a_selected_pill_and_accent_matched_label_runs() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let mut p = Palette::new();
        p.open(Vec::new());
        for c in "zm".chars() {
            p.push_char(c);
        }
        let (w, h) = p.natural_size(2.0, &mut m);
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w,
            h,
        };
        let paint = p.build(panel, 2.0, &theme, &mut m);
        // The sole match is selected, so a pill quad plus the input caret are emitted.
        assert!(paint.quads.len() >= 2);
        // Some label draws in accent (the matched characters).
        assert!(paint.labels.iter().any(|l| l.color == theme.accent));
    }

    #[test]
    fn empty_query_matches_every_command() {
        let p = Palette::new(); // no tabs, so the universal mode lists just the commands
        assert_eq!(p.match_actions().len(), COMMANDS.len());
    }

    #[test]
    fn query_filters_by_label_substring_case_insensitively() {
        let mut p = Palette::new();
        p.open(Vec::new());
        for c in "zoom".chars() {
            p.push_char(c);
        }
        assert_eq!(p.match_actions(), vec![Action::Zoom]);
        assert_eq!(p.selected_action(), Some(Action::Zoom));
    }

    #[test]
    fn selection_clamps_within_matches() {
        let mut p = Palette::new();
        p.open(Vec::new());
        p.move_selection(-1); // cannot go below 0
        assert_eq!(p.selected_action(), Some(COMMANDS[0].action));
        p.move_selection(1000); // clamps to the last match
        assert_eq!(
            p.selected_action(),
            Some(COMMANDS[COMMANDS.len() - 1].action)
        );
    }

    #[test]
    fn filtering_resets_selection_and_narrows() {
        let mut p = Palette::new();
        p.open(Vec::new());
        p.move_selection(3);
        for c in "focus".chars() {
            p.push_char(c);
        }
        // "focus" matches the four focus commands; selection reset to the first.
        assert_eq!(p.match_actions().len(), 4);
        assert_eq!(p.selected_action(), Some(Action::FocusLeft));
    }

    #[test]
    fn a_no_match_query_has_no_action() {
        let mut p = Palette::new();
        p.open(Vec::new());
        for c in "zzzz".chars() {
            p.push_char(c);
        }
        assert!(p.match_actions().is_empty());
        assert_eq!(p.selected_action(), None);
    }

    #[test]
    fn fuzzy_matches_a_subsequence_and_records_matched_positions() {
        let mut p = Palette::new();
        p.open(Vec::new());
        for c in "zm".chars() {
            p.push_char(c);
        }
        let rows = p.rows();
        assert_eq!(rows.len(), 1, "'zm' fuzzy-matches only Zoom");
        assert_eq!(p.selected_action(), Some(Action::Zoom));
        // "Zoom / unzoom pane": z at 0, m at 3.
        assert_eq!(rows[0].positions, vec![0, 3]);
    }

    #[test]
    fn theme_query_surfaces_both_theme_commands() {
        let mut p = Palette::new();
        p.open(Vec::new());
        for c in "theme".chars() {
            p.push_char(c);
        }
        assert_eq!(
            p.match_actions(),
            vec![Action::ThemeDark, Action::ThemeLight]
        );
    }

    #[test]
    fn backspace_widens_the_match_set_again() {
        let mut p = Palette::new();
        p.open(Vec::new());
        for c in "zoomx".chars() {
            p.push_char(c);
        }
        assert!(p.match_actions().is_empty());
        p.backspace();
        assert_eq!(p.match_actions().len(), 1);
    }

    #[test]
    fn universal_mode_surfaces_open_tabs_and_the_prefix_filters_them_out() {
        let mut p = Palette::new();
        p.open(vec!["Tab 1".to_owned(), "Tab 2".to_owned()]);
        // No prefix: commands + the two tabs.
        assert_eq!(p.match_actions().len(), COMMANDS.len() + 2);
        // A tab entry runs GotoTab.
        for c in "Tab 2".chars() {
            p.push_char(c);
        }
        assert_eq!(p.match_actions(), vec![Action::GotoTab(1)]);
        assert_eq!(p.selected_action(), Some(Action::GotoTab(1)));
    }

    #[test]
    fn the_commands_prefix_restricts_to_commands_and_hides_tabs() {
        let mut p = Palette::new();
        p.open(vec!["Tab 1".to_owned()]);
        p.push_char('>'); // commands-only mode
                          // Every match is a command, never a tab.
        assert_eq!(p.match_actions().len(), COMMANDS.len());
        assert!(p
            .match_actions()
            .iter()
            .all(|a| !matches!(a, Action::GotoTab(_))));
    }

    #[test]
    fn the_files_prefix_is_deferred_and_yields_nothing() {
        let mut p = Palette::new();
        p.open(Vec::new());
        p.push_char('/');
        assert!(p.match_actions().is_empty());
        assert_eq!(p.selected_action(), None);
    }
}
