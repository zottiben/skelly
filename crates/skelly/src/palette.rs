//! The command palette: a centered overlay listing runnable commands, filtered by a
//! typed query and navigated by keyboard (AGENTS Hard rule 4 - an overlay over the
//! live terminal, never a route). This module is pure state + layout: it owns the query,
//! the filtered selection, and builds the palette as a *proportional* display list
//! (decorative quads + positioned labels in the guide's fonts, §09 "Command palette row").
//! The binary owns opening it, routing keys, and executing the chosen command.
//!
//! The built-in [`COMMANDS`] set is the seed of the keybinding registry; merging user
//! `[keys]` overrides + surfacing tabs/themes/files is a later slice.

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

/// The built-in command set. Order is the display order.
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        label: "Split pane right",
        hint: "opt |",
        action: Action::SplitRight,
    },
    Command {
        label: "Split pane down",
        hint: "opt -",
        action: Action::SplitDown,
    },
    Command {
        label: "Zoom / unzoom pane",
        hint: "opt Z",
        action: Action::Zoom,
    },
    Command {
        label: "Even out splits",
        hint: "opt =",
        action: Action::EvenOut,
    },
    Command {
        label: "Close pane",
        hint: "opt W",
        action: Action::ClosePane,
    },
    Command {
        label: "Focus pane left",
        hint: "opt H",
        action: Action::FocusLeft,
    },
    Command {
        label: "Focus pane down",
        hint: "opt J",
        action: Action::FocusDown,
    },
    Command {
        label: "Focus pane up",
        hint: "opt K",
        action: Action::FocusUp,
    },
    Command {
        label: "Focus pane right",
        hint: "opt L",
        action: Action::FocusRight,
    },
    Command {
        label: "New tab",
        hint: "cmd T",
        action: Action::NewTab,
    },
    Command {
        label: "Close tab",
        hint: "cmd W",
        action: Action::CloseTab,
    },
    Command {
        label: "Next tab",
        hint: "opt shift ]",
        action: Action::NextTab,
    },
    Command {
        label: "Previous tab",
        hint: "opt shift [",
        action: Action::PrevTab,
    },
    Command {
        label: "Toggle sidebar",
        hint: "cmd B",
        action: Action::ToggleSidebar,
    },
    Command {
        label: "Cycle sidebar mode",
        hint: "shift cmd B",
        action: Action::CycleSidebarMode,
    },
    Command {
        label: "Show git diff",
        hint: "shift cmd G",
        action: Action::ShowGitDiff,
    },
    Command {
        label: "Show session timeline",
        hint: "shift cmd H",
        action: Action::ShowTimeline,
    },
    Command {
        label: "Open settings",
        hint: "cmd ,",
        action: Action::OpenSettings,
    },
    Command {
        label: "Theme: Ossein Dark",
        hint: "",
        action: Action::ThemeDark,
    },
    Command {
        label: "Theme: Ossein Light",
        hint: "",
        action: Action::ThemeLight,
    },
    Command {
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

/// Palette state: whether it is open, the query, and the selected match.
pub(crate) struct Palette {
    /// Whether the palette overlay is showing.
    pub(crate) open: bool,
    query: String,
    selected: usize,
}

impl Palette {
    /// A closed, empty palette.
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
        }
    }

    /// Open the palette fresh (empty query, first match selected).
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    /// Close the palette.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// The commands whose label fuzzy-matches the query, best first. Each carries the
    /// matched character positions (for accent highlighting). An empty query matches
    /// everything in [`COMMANDS`] order.
    pub(crate) fn results(&self) -> Vec<Match> {
        let query = self.query.trim();
        let mut scored: Vec<(i32, usize, Vec<usize>)> = COMMANDS
            .iter()
            .enumerate()
            .filter_map(|(index, cmd)| {
                fuzzy_match(query, cmd.label).map(|(score, positions)| (score, index, positions))
            })
            .collect();
        // Best score first; ties keep COMMANDS order (a stable, predictable list).
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored
            .into_iter()
            .map(|(_, index, positions)| Match { index, positions })
            .collect()
    }

    /// The matched command indices, best first (positions dropped).
    pub(crate) fn matches(&self) -> Vec<usize> {
        self.results().into_iter().map(|m| m.index).collect()
    }

    /// Move the selection by `delta`, clamped to the current match count.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        let count = self.matches().len();
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

    /// The action of the currently selected match, if any.
    pub(crate) fn selected_action(&self) -> Option<Action> {
        self.results()
            .get(self.selected)
            .map(|m| COMMANDS[m.index].action)
    }

    /// The palette's natural panel size in **physical** px (including the card padding),
    /// for the binary to center + animate the card. Width is the widest content row (a
    /// command's label + gap + hint, the footer, or the input) plus insets; height is the
    /// sum of the fixed row heights.
    pub(crate) fn natural_size(&self, scale: f32, measure: &mut TextMeasure) -> (f32, f32) {
        let results = self.results();
        let inset = (PAD + ROW_INSET) * scale;
        let mut content_w = measure.width(FOOTER, FontRole::Caption, None);
        // The input line ("> " + query or placeholder).
        let query = if self.query.is_empty() {
            PLACEHOLDER
        } else {
            &self.query
        };
        content_w = content_w.max(
            measure.width("> ", FontRole::Body, None) + measure.width(query, FontRole::Body, None),
        );
        for hit in &results {
            let cmd = &COMMANDS[hit.index];
            let row = measure.width(cmd.label, FontRole::Body, None)
                + HINT_GAP * scale
                + measure.width(cmd.hint, FontRole::Micro, None);
            content_w = content_w.max(row);
        }
        let width = content_w + 2.0 * inset;
        #[allow(
            clippy::cast_precision_loss,
            reason = "the match count is a small, exact value"
        )]
        let rows_h = INPUT_H + COUNT_H + results.len() as f32 * CMD_H + SPACER_H + FOOTER_H;
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

        // Result count.
        let results = self.results();
        let count = if results.len() == 1 {
            "1 result".to_owned()
        } else {
            format!("{} results", results.len())
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

        // Command rows.
        for (index, hit) in results.iter().enumerate() {
            let cmd = &COMMANDS[hit.index];
            if index == self.selected {
                let inset = ROW_INSET * 0.5 * scale;
                quads.push(ChromeQuad::tint(
                    PxRect {
                        x: cx + inset,
                        y,
                        w: (cw - 2.0 * inset).max(0.0),
                        h: CMD_H * scale,
                    },
                    theme.accent,
                    0.14,
                    PILL_RADIUS * scale,
                ));
            }
            push_command(
                &mut labels,
                cmd,
                &hit.positions,
                cx,
                cw,
                y,
                scale,
                theme,
                measure,
            );
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
        push_line(
            labels,
            ">",
            FontRole::Body,
            theme.accent,
            prompt_x,
            y,
            INPUT_H,
            scale,
            measure,
        );
        let query_x = prompt_x + measure.width("> ", FontRole::Body, None);
        let (query_text, query_color) = if self.query.is_empty() {
            (PLACEHOLDER, theme.fg_muted)
        } else {
            (self.query.as_str(), theme.fg_primary)
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
        // Caret after the query text (or at the input start when empty).
        let caret_x = query_x
            + if self.query.is_empty() {
                0.0
            } else {
                measure.width(&self.query, FontRole::Body, None)
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

/// A fuzzy match: a command index plus the label positions the query matched.
pub(crate) struct Match {
    pub(crate) index: usize,
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

/// A command row: the `label` left-aligned with matched `positions` in `accent` (the rest
/// `fg.primary`), and the key `hint` right-anchored in muted `micro` mono.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_command(
    labels: &mut Vec<ProseLabel>,
    cmd: &Command,
    positions: &[usize],
    cx: f32,
    cw: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) {
    let line_h = measure.line_height(FontRole::Body);
    let y = top + (CMD_H * scale - line_h) * 0.5;
    // The label, split into matched / unmatched runs so matched chars draw in accent.
    let mut x = cx + ROW_INSET * scale;
    for (text, matched) in matched_runs(cmd.label, positions) {
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
    if !cmd.hint.is_empty() {
        let hint_w = measure.width(cmd.hint, FontRole::Micro, None);
        let hint_x = cx + cw - ROW_INSET * scale - hint_w;
        let hint_line = measure.line_height(FontRole::Micro);
        labels.push(ProseLabel {
            text: cmd.hint.to_owned(),
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
    use super::{matched_runs, Action, Palette, PxRect, TextMeasure, Theme, COMMANDS};

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
        p.open();
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
        let p = Palette::new();
        assert_eq!(p.matches().len(), COMMANDS.len());
    }

    #[test]
    fn query_filters_by_label_substring_case_insensitively() {
        let mut p = Palette::new();
        p.open();
        for c in "zoom".chars() {
            p.push_char(c);
        }
        let matches = p.matches();
        assert_eq!(matches.len(), 1);
        assert_eq!(COMMANDS[matches[0]].action, Action::Zoom);
        assert_eq!(p.selected_action(), Some(Action::Zoom));
    }

    #[test]
    fn selection_clamps_within_matches() {
        let mut p = Palette::new();
        p.open();
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
        p.open();
        p.move_selection(3);
        for c in "focus".chars() {
            p.push_char(c);
        }
        // "focus" matches the four focus commands; selection reset to the first.
        assert_eq!(p.matches().len(), 4);
        assert_eq!(p.selected_action(), Some(Action::FocusLeft));
    }

    #[test]
    fn a_no_match_query_has_no_action() {
        let mut p = Palette::new();
        p.open();
        for c in "zzzz".chars() {
            p.push_char(c);
        }
        assert!(p.matches().is_empty());
        assert_eq!(p.selected_action(), None);
    }

    #[test]
    fn fuzzy_matches_a_subsequence_and_records_matched_positions() {
        let mut p = Palette::new();
        p.open();
        for c in "zm".chars() {
            p.push_char(c);
        }
        let results = p.results();
        assert_eq!(results.len(), 1, "'zm' fuzzy-matches only Zoom");
        assert_eq!(COMMANDS[results[0].index].action, Action::Zoom);
        // "Zoom / unzoom pane": z at 0, m at 3.
        assert_eq!(results[0].positions, vec![0, 3]);
    }

    #[test]
    fn earlier_first_match_ranks_ahead() {
        // 's' starts "Split pane right" (position 0) and appears mid-word elsewhere;
        // the earliest first-match wins, and ties keep COMMANDS order.
        let mut p = Palette::new();
        p.open();
        p.push_char('s');
        let results = p.results();
        assert_eq!(COMMANDS[results[0].index].action, Action::SplitRight);
    }

    #[test]
    fn theme_query_surfaces_both_theme_commands() {
        let mut p = Palette::new();
        p.open();
        for c in "theme".chars() {
            p.push_char(c);
        }
        let actions: Vec<Action> = p.matches().iter().map(|&i| COMMANDS[i].action).collect();
        assert_eq!(actions, vec![Action::ThemeDark, Action::ThemeLight]);
    }

    #[test]
    fn backspace_widens_the_match_set_again() {
        let mut p = Palette::new();
        p.open();
        for c in "zoomx".chars() {
            p.push_char(c);
        }
        assert!(p.matches().is_empty());
        p.backspace();
        assert_eq!(p.matches().len(), 1);
    }
}
