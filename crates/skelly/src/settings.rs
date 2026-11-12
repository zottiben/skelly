//! The settings view: a full in-window view over `config.toml`, opened with `⌘,`
//! and dismissed with Esc (AGENTS Hard rule 4 - a layer over the always-present pane
//! tree, never a route). This module is pure state + view-building + the control model
//! that edits a [`Config`]; the binary owns opening it, routing keys, applying live
//! effects, and persisting the file.
//!
//! Every control maps to **exactly one** `config.toml` key (AGENTS Hard rule 1): the
//! file is the single source of truth and this view is a typed editor over it. A
//! control declares its key path (for the round-trip contract) and a getter/setter
//! pair, so reading and writing always go through the config, never a shadow copy.
//!
//! Keyboard- and mouse-driven: a left category nav, and a content column of named sections,
//! each a card of control rows. `↑/↓` moves between controls, `←/→` (and Enter) changes a
//! value, and a click lands on any nav pill, row or widget. A row shows its label over the
//! `config.toml` key it owns, and its §09 widget at the right - a toggle switch, a segmented
//! control, a slider, or a text box (see [`push_control`]). The theme picker is the guide's
//! §10.9 grid of preview cards ([`ThemeCard`]). [`Settings::build`] records a [`Target`] for
//! every clickable rect it draws, so a click lands on exactly what is on screen. Deferred: the
//! keybindings / advanced categories (they need the `[keys]` registry).
//!
//! The nav ends with the Workspaces page ([`crate::workspace`]): a manager over the session
//! layout rather than the config, so it sits outside [`CATEGORIES`] and the one-key contract.

use skelly_config::{Config, CursorStyle, DiffAgainst, DiffView, SidebarMode, TabTitle};
use skelly_render::{
    ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme, THEMES, THEME_LABELS,
};

use crate::workspace::{self, push_ringed};

/// Settings-view layout constants in **logical** px (multiplied by the DPI scale), after the
/// guide's §10.9 screen: a 184px category nav beside a content column of section cards.
const NAV_WIDTH: f32 = 184.0;
/// Pad from the panel's top, right and bottom edges.
const PAD: f32 = 20.0;
/// The nav pills' inset from the nav's edges.
const NAV_INSET: f32 = 8.0;
/// A nav pill's height, the gap between pills, and its corner radius.
const NAV_ROW_H: f32 = 32.0;
const NAV_ROW_GAP: f32 = 2.0;
const NAV_RADIUS: f32 = 7.0;
/// A nav pill's icon inset, and the icon column (the 16px glyph plus its 9px gap).
const NAV_ICON_X: f32 = 11.0;
const NAV_ICON_W: f32 = 25.0;
/// The gap on each side of the hairline that sets the Workspaces page apart.
const NAV_SEP_GAP: f32 = 8.0;
/// The content column's inset from the nav divider, and its widest; a wider area centers it.
const CONTENT_PAD: f32 = 32.0;
const COLUMN_MAX_W: f32 = 720.0;
/// The header block (the category name over its summary line), and the gap below it, which
/// holds a scrolled page's `↑ N more` line.
const HEADER_H: f32 = 50.0;
const HEADER_GAP: f32 = 22.0;
/// The header's first row, which centers the category name, the esc hint, the close button and
/// the nav's "Settings" title on one line.
const NAME_ROW_H: f32 = 28.0;
/// The gap between the name row and the summary line under it.
const SUMMARY_GAP: f32 = 2.0;
/// The gap between the nav's title and its first pill.
const NAV_TITLE_GAP: f32 = 14.0;
/// The header close button: a square of this side, on the name row.
const CLOSE_SIDE: f32 = 28.0;
/// The close button's corner radius.
const CLOSE_RADIUS: f32 = 6.0;
/// Gap between the esc hint and the close button.
const CLOSE_GAP: f32 = 8.0;
/// The close button's glyph: the guide's `close` icon (`✕`), as the bundled Nerd Font's codicon
/// so it reads at icon size (the mono face's `✕` is too small to fill the button).
const CLOSE_ICON: &str = "\u{ea76}";
/// The gap above a section (except a page's first), and its label line.
const SECTION_GAP: f32 = 22.0;
const SECTION_LABEL_H: f32 = 24.0;
/// A section card's corner radius.
pub(crate) const CARD_RADIUS: f32 = 10.0;
/// A control row in a card: its height, its side pad, and the inset + radius of the focus or
/// hover pill drawn inside it.
const ROW_H: f32 = 56.0;
pub(crate) const ROW_PAD_X: f32 = 16.0;
pub(crate) const PILL_INSET: f32 = 4.0;
pub(crate) const PILL_RADIUS: f32 = 7.0;
/// The gap between a row's label and the config key beneath it, and the least gap between
/// either and the widget at the row's right.
const KEY_GAP: f32 = 1.0;
const LABEL_GAP: f32 = 16.0;
/// The theme grid: the gap between cards, a card's height and its name strip's, and the
/// narrowest a card gets before the grid drops from four columns to two.
const THEME_GAP: f32 = 9.0;
const THEME_CARD_H: f32 = 66.0;
const THEME_STRIP_H: f32 = 26.0;
const THEME_CARD_MIN_W: f32 = 140.0;
/// A theme card's corner radius.
const THEME_RADIUS: f32 = 9.0;
/// A text value's input box: its widest, its height, its text inset, and its corner radius.
const INPUT_W: f32 = 300.0;
const INPUT_H: f32 = 30.0;
const INPUT_PAD_X: f32 = 10.0;
const INPUT_RADIUS: f32 = 6.0;
/// Toggle-switch dimensions (design §09: 38x22 pill, 18px knob, 2px inset).
const TOGGLE_W: f32 = 38.0;
const TOGGLE_H: f32 = 22.0;
const TOGGLE_KNOB: f32 = 18.0;
const TOGGLE_INSET: f32 = 2.0;
/// Segmented-control dimensions (§09: 3px container pad, per-segment 4x11 pad, radii 7/5).
const SEG_PAD: f32 = 3.0;
const SEG_ITEM_PAD_X: f32 = 11.0;
const SEG_H: f32 = 24.0;
const SEG_RADIUS: f32 = 7.0;
const SEG_ITEM_RADIUS: f32 = 5.0;
/// Slider dimensions (§09: a 6px track with a 14px knob, plus a mono value readout).
const SLIDER_W: f32 = 116.0;
const SLIDER_TRACK_H: f32 = 6.0;
const SLIDER_KNOB: f32 = 14.0;
const SLIDER_VALUE_GAP: f32 = 12.0;
/// A key-hint row: a keycap's height, its side pad, the gap to its action, and the gap
/// between hints.
const HINT_H: f32 = 22.0;
const HINT_KEY_PAD: f32 = 6.0;
const HINT_KEY_GAP: f32 = 6.0;
const HINT_GAP: f32 = 16.0;
/// The gap between the page body and the footer's key hints.
const FOOTER_GAP: f32 = 14.0;
/// The footer's key hints on a config page.
const FOOTER_HINTS: &[(&str, &str)] = &[
    ("\u{2191}\u{2193}", "move"),
    ("\u{2190}\u{2192}", "change"),
    ("enter", "edit"),
    ("tab", "category"),
    ("esc", "close"),
];
/// The footer's key hints on the Workspaces page.
const WORKSPACES_HINTS: &[(&str, &str)] = &[
    ("\u{2191}\u{2193}", "move"),
    ("\u{2190}\u{2192}", "icon"),
    ("enter", "rename"),
    ("delete", "remove"),
    ("tab", "category"),
    ("esc", "close"),
];
/// The Workspaces page's nav entry (codicon `multiple-windows`), after every config category.
const WORKSPACES_NAV: (&str, &str) = ("\u{eb23}", "Workspaces");
/// The Workspaces page's summary line.
const WORKSPACES_ABOUT: &str = "Workspaces and their groups, kept in the session layout";
/// The theme card's selected mark (codicon `check`).
const CHECK_ICON: &str = "\u{eab2}";

/// One editable control. It maps to exactly one `config.toml` key (Hard rule 1):
/// `key` is the dotted TOML path (also what the round-trip test asserts against), and
/// [`Kind`] carries the getter/setter that read and write only that key.
struct Control {
    /// The human label, shown at the left of its row.
    label: &'static str,
    /// The section the control sits in: a page's controls run in section order, and a new
    /// section name starts a new card under its own label.
    section: &'static str,
    /// The `config.toml` key path this control owns, e.g. `appearance.font_size`.
    key: &'static str,
    /// How the value is displayed and changed.
    kind: Kind,
}

/// The shape of a control's value, with the getter/setter bound to its config key.
/// All closures are non-capturing so they coerce to `fn` pointers in the static table.
enum Kind {
    /// A bounded string edited explicitly with Enter; validated before committing.
    Text {
        get: fn(&Config) -> &str,
        set: fn(&mut Config, String),
    },
    /// An on/off switch.
    Toggle {
        /// Read the current state.
        get: fn(&Config) -> bool,
        /// Write the new state.
        set: fn(&mut Config, bool),
    },
    /// A choice among named options, addressed by index.
    Choice {
        /// The option labels, in order.
        options: &'static [&'static str],
        /// Read the current option index.
        get: fn(&Config) -> usize,
        /// Write the chosen option index (already clamped to `options`).
        set: fn(&mut Config, usize),
        /// How the options draw.
        look: Look,
    },
    /// An integer (or scaled-fraction) value stepped between `min..=max`.
    Range {
        /// Inclusive lower bound (in raw units).
        min: i32,
        /// Inclusive upper bound (in raw units).
        max: i32,
        /// One `←/→` increment (in raw units).
        step: i32,
        /// Divisor for display: `1` shows the integer, `10`/`100` show a fraction.
        divisor: i32,
        /// Unit suffix appended to the displayed value (e.g. `px`).
        suffix: &'static str,
        /// Read the current value (raw units).
        get: fn(&Config) -> i32,
        /// Write the new value (raw units, already clamped).
        set: fn(&mut Config, i32),
    },
}

/// How a [`Kind::Choice`] draws its options.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    /// A §09 segmented control holding every option.
    Segments,
    /// The §10.9 theme grid: the options are [`THEME_LABELS`], each drawn as a preview card
    /// of the [`THEMES`] entry at its index.
    Themes,
}

impl Kind {
    /// The value as displayed to the right of the label.
    fn value(&self, c: &Config) -> String {
        match self {
            Kind::Text { get, .. } => get(c).to_owned(),
            Kind::Toggle { get, .. } => if get(c) { "On" } else { "Off" }.to_owned(),
            Kind::Choice { options, get, .. } => {
                let i = get(c).min(options.len().saturating_sub(1));
                (*options.get(i).unwrap_or(&"")).to_owned()
            }
            Kind::Range {
                get,
                divisor,
                suffix,
                ..
            } => range_text(get(c), *divisor, suffix),
        }
    }

    /// The widest readout a range control can show (its min or max, formatted), for
    /// reserving a fixed readout column; `None` for other kinds.
    fn readout_width(&self, measure: &mut TextMeasure) -> Option<f32> {
        let Kind::Range {
            min,
            max,
            divisor,
            suffix,
            ..
        } = self
        else {
            return None;
        };
        let lo = measure.width(&range_text(*min, *divisor, suffix), FontRole::Mono, None);
        let hi = measure.width(&range_text(*max, *divisor, suffix), FontRole::Mono, None);
        Some(lo.max(hi))
    }

    /// Change the value by a signed `←/→` nudge, staying within bounds.
    fn adjust(&self, c: &mut Config, delta: i32) {
        match self {
            Kind::Text { .. } => {}
            // `→`/`←` map to On/Off so direction reads intuitively.
            Kind::Toggle { set, .. } => set(c, delta > 0),
            Kind::Choice {
                options, get, set, ..
            } => {
                let last = i32::try_from(options.len().saturating_sub(1)).unwrap_or(0);
                let cur = i32::try_from(get(c)).unwrap_or(0);
                let next = (cur + delta).clamp(0, last);
                set(c, usize::try_from(next).unwrap_or(0));
            }
            Kind::Range {
                min,
                max,
                step,
                get,
                set,
                ..
            } => set(c, (get(c) + delta * step).clamp(*min, *max)),
        }
    }

    /// Enter/Space: flip a toggle, cycle a choice forward (wrapping), or step a range up.
    fn activate(&self, c: &mut Config) {
        match self {
            Kind::Toggle { get, set } => set(c, !get(c)),
            Kind::Choice {
                options, get, set, ..
            } => {
                let n = options.len().max(1);
                set(c, (get(c) + 1) % n);
            }
            Kind::Range { .. } => self.adjust(c, 1),
            Kind::Text { .. } => {}
        }
    }
}

/// A settings category, shown in the left nav with its icon and label.
struct Category {
    /// Its nav glyph: a codicon from the bundled Nerd font, drawn with [`FontRole::Icon`].
    icon: &'static str,
    /// The category name, shown in the nav and as the content heading.
    label: &'static str,
    /// The one-line summary under the heading.
    about: &'static str,
    /// The controls listed when this category is active.
    controls: &'static [Control],
}

/// The category nav + their controls. Order is the display order. Each category groups
/// the config keys it owns; every control below round-trips exactly one key.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "range getters/setters convert small, already-clamped values between the \
              config's typed fields (u8/u16/f32) and the i32 slider units"
)]
static CATEGORIES: &[Category] = &[
    Category {
        icon: "\u{eac6}", // color-mode
        label: "Appearance",
        about: "Theme, text and the terminal window",
        controls: &[
            Control {
                label: "Theme",
                section: "Theme",
                key: "appearance.theme",
                kind: Kind::Choice {
                    options: &THEME_LABELS,
                    // An unknown name resolves as Ossein Dark, the first option.
                    get: |c| {
                        THEMES
                            .iter()
                            .position(|(name, _)| *name == c.appearance.theme)
                            .unwrap_or(0)
                    },
                    set: |c, i| THEMES[i].0.clone_into(&mut c.appearance.theme),
                    look: Look::Themes,
                },
            },
            Control {
                label: "Font size",
                section: "Text",
                key: "appearance.font_size",
                kind: Kind::Range {
                    min: 8,
                    max: 32,
                    step: 1,
                    divisor: 1,
                    suffix: "px",
                    get: |c| i32::from(c.appearance.font_size),
                    set: |c, v| c.appearance.font_size = v.clamp(8, 32) as u16,
                },
            },
            Control {
                label: "Line height",
                section: "Text",
                key: "appearance.line_height",
                kind: Kind::Range {
                    min: 8,
                    max: 30,
                    step: 1,
                    divisor: 10,
                    suffix: "",
                    get: |c| (c.appearance.line_height * 10.0).round() as i32,
                    set: |c, v| c.appearance.line_height = v as f32 / 10.0,
                },
            },
            Control {
                label: "Font ligatures",
                section: "Text",
                key: "appearance.ligatures",
                kind: Kind::Toggle {
                    get: |c| c.appearance.ligatures,
                    set: |c, v| c.appearance.ligatures = v,
                },
            },
            Control {
                label: "Bold uses bright colors",
                section: "Text",
                key: "appearance.bold_is_bright",
                kind: Kind::Toggle {
                    get: |c| c.appearance.bold_is_bright,
                    set: |c, v| c.appearance.bold_is_bright = v,
                },
            },
            Control {
                label: "Cursor style",
                section: "Terminal",
                key: "appearance.cursor",
                kind: Kind::Choice {
                    options: &["Block", "Bar", "Underline"],
                    get: |c| match c.appearance.cursor {
                        CursorStyle::Block => 0,
                        CursorStyle::Bar => 1,
                        CursorStyle::Underline => 2,
                    },
                    set: |c, i| {
                        c.appearance.cursor = match i {
                            1 => CursorStyle::Bar,
                            2 => CursorStyle::Underline,
                            _ => CursorStyle::Block,
                        };
                    },
                    look: Look::Segments,
                },
            },
            Control {
                label: "Show pane status line",
                section: "Terminal",
                key: "appearance.show_status_line",
                kind: Kind::Toggle {
                    get: |c| c.appearance.show_status_line,
                    set: |c, v| c.appearance.show_status_line = v,
                },
            },
            Control {
                label: "Window opacity",
                section: "Window",
                key: "appearance.opacity",
                kind: Kind::Range {
                    min: 0,
                    max: 100,
                    step: 5,
                    divisor: 100,
                    suffix: "",
                    get: |c| (c.appearance.opacity * 100.0).round() as i32,
                    set: |c, v| c.appearance.opacity = v as f32 / 100.0,
                },
            },
            Control {
                label: "Background blur",
                section: "Window",
                key: "appearance.bg_blur",
                kind: Kind::Range {
                    min: 0,
                    max: 100,
                    step: 2,
                    divisor: 1,
                    suffix: "",
                    get: |c| i32::from(c.appearance.bg_blur),
                    set: |c, v| c.appearance.bg_blur = v.clamp(0, 100) as u8,
                },
            },
        ],
    },
    Category {
        icon: "\u{ebf3}", // layout-sidebar-left
        label: "Sidebar",
        about: "How the sidebar shows, and how wide it is",
        controls: &[
            Control {
                label: "Mode",
                section: "Display",
                key: "sidebar.mode",
                kind: Kind::Choice {
                    options: &["Fixed", "Auto-hide", "Hidden"],
                    get: |c| match c.sidebar.mode {
                        SidebarMode::Fixed => 0,
                        SidebarMode::Autohide => 1,
                        SidebarMode::Hidden => 2,
                    },
                    set: |c, i| {
                        c.sidebar.mode = match i {
                            1 => SidebarMode::Autohide,
                            2 => SidebarMode::Hidden,
                            _ => SidebarMode::Fixed,
                        };
                    },
                    look: Look::Segments,
                },
            },
            Control {
                label: "Width",
                section: "Display",
                key: "sidebar.width",
                kind: Kind::Range {
                    min: 180,
                    max: 360,
                    step: 8,
                    divisor: 1,
                    suffix: "px",
                    get: |c| i32::from(c.sidebar.width),
                    set: |c, v| c.sidebar.width = v.clamp(180, 360) as u16,
                },
            },
            Control {
                label: "Show pinned grid",
                section: "Pinned tabs",
                key: "sidebar.show_pinned",
                kind: Kind::Toggle {
                    get: |c| c.sidebar.show_pinned,
                    set: |c, v| c.sidebar.show_pinned = v,
                },
            },
        ],
    },
    Category {
        icon: "\u{eaae}", // browser
        label: "Tabs",
        about: "What names a tab",
        controls: &[
            Control {
                label: "Title source",
                section: "Titles",
                key: "tabs.title",
                kind: Kind::Choice {
                    options: &["Directory", "Command", "Custom"],
                    get: |c| match c.tabs.title {
                        TabTitle::Cwd => 0,
                        TabTitle::Command => 1,
                        TabTitle::Custom => 2,
                    },
                    set: |c, i| {
                        c.tabs.title = match i {
                            1 => TabTitle::Command,
                            2 => TabTitle::Custom,
                            _ => TabTitle::Cwd,
                        };
                    },
                    look: Look::Segments,
                },
            },
            Control {
                label: "Follow directory",
                section: "Titles",
                key: "tabs.follow_cwd",
                kind: Kind::Toggle {
                    get: |c| c.tabs.follow_cwd,
                    set: |c, v| c.tabs.follow_cwd = v,
                },
            },
        ],
    },
    Category {
        icon: "\u{ebeb}", // layout
        label: "Panes",
        about: "Where a new pane starts",
        controls: &[Control {
            label: "New panes inherit cwd",
            section: "New panes",
            key: "panes.split_inherits_cwd",
            kind: Kind::Toggle {
                get: |c| c.panes.split_inherits_cwd,
                set: |c, v| c.panes.split_inherits_cwd = v,
            },
        }],
    },
    Category {
        icon: "\u{ea85}", // terminal
        label: "Shell & env",
        about: "The shell each new pane runs",
        controls: &[Control {
            label: "Shell",
            section: "Program",
            key: "shell.program",
            kind: Kind::Choice {
                // Index 0 = the login shell (empty `program`); the rest set it explicitly.
                options: &["Login shell", "zsh", "bash", "fish"],
                get: |c| match c.shell.program.as_str() {
                    "zsh" => 1,
                    "bash" => 2,
                    "fish" => 3,
                    _ => 0,
                },
                set: |c, i| {
                    let program = match i {
                        1 => "zsh",
                        2 => "bash",
                        3 => "fish",
                        _ => "",
                    };
                    program.clone_into(&mut c.shell.program);
                },
                look: Look::Segments,
            },
        }],
    },
    Category {
        icon: "\u{ea82}", // history
        label: "Session",
        about: "The session timeline, rewind and layout restore",
        controls: &[
            Control {
                label: "Record timeline",
                section: "Timeline",
                key: "session.timeline",
                kind: Kind::Toggle {
                    get: |c| c.session.timeline,
                    set: |c, v| c.session.timeline = v,
                },
            },
            Control {
                label: "Shadow worktree",
                section: "Timeline",
                key: "session.shadow_worktree",
                kind: Kind::Toggle {
                    get: |c| c.session.shadow_worktree,
                    set: |c, v| c.session.shadow_worktree = v,
                },
            },
            Control {
                label: "Persist layout",
                section: "Restore",
                key: "session.persist",
                kind: Kind::Toggle {
                    get: |c| c.session.persist,
                    set: |c, v| c.session.persist = v,
                },
            },
        ],
    },
    Category {
        icon: "\u{ea68}", // source-control
        label: "Git",
        about: "What the git dock reviews, and how",
        controls: &[
            Control {
                label: "Diff layout",
                section: "Git dock",
                key: "git.diff_view",
                kind: Kind::Choice {
                    options: &["Unified", "Split"],
                    get: |c| match c.git.diff_view {
                        DiffView::Unified => 0,
                        DiffView::Split => 1,
                    },
                    set: |c, i| {
                        c.git.diff_view = if i == 1 {
                            DiffView::Split
                        } else {
                            DiffView::Unified
                        };
                    },
                    look: Look::Segments,
                },
            },
            Control {
                label: "Review",
                section: "Git dock",
                key: "git.diff_against",
                kind: Kind::Choice {
                    options: &["Uncommitted", "Branch"],
                    get: |c| match c.git.diff_against {
                        DiffAgainst::Head => 0,
                        DiffAgainst::Branch => 1,
                    },
                    set: |c, i| {
                        c.git.diff_against = if i == 1 {
                            DiffAgainst::Branch
                        } else {
                            DiffAgainst::Head
                        };
                    },
                    look: Look::Segments,
                },
            },
        ],
    },
    Category {
        icon: "\u{ec12}", // mic
        label: "Voice",
        about: "Local dictation and the Pi voice bridge",
        controls: &[
            Control {
                label: "Pi voice bridge (new panes)",
                section: "Bridge",
                key: "voice.enabled",
                kind: Kind::Toggle {
                    get: |c| c.voice.enabled,
                    set: |c, v| c.voice.enabled = v,
                },
            },
            Control {
                label: "Whisper executable",
                section: "Transcription",
                key: "voice.whisper_program",
                kind: Kind::Text {
                    get: |c| &c.voice.whisper_program,
                    set: |c, v| c.voice.whisper_program = v,
                },
            },
            Control {
                label: "Model path",
                section: "Transcription",
                key: "voice.model_path",
                kind: Kind::Text {
                    get: |c| &c.voice.model_path,
                    set: |c, v| c.voice.model_path = v,
                },
            },
            Control {
                label: "Language (code or auto)",
                section: "Transcription",
                key: "voice.language",
                kind: Kind::Text {
                    get: |c| &c.voice.language,
                    set: |c, v| c.voice.language = v,
                },
            },
            Control {
                label: "Dictation shortcut",
                section: "Recording",
                key: "voice.dictation_shortcut",
                kind: Kind::Text {
                    get: |c| &c.voice.dictation_shortcut,
                    set: |c, v| c.voice.dictation_shortcut = v,
                },
            },
            Control {
                label: "Recording limit",
                section: "Recording",
                key: "voice.max_recording_seconds",
                kind: Kind::Range {
                    min: 5,
                    max: 120,
                    step: 5,
                    divisor: 1,
                    suffix: "s",
                    get: |c| i32::from(c.voice.max_recording_seconds),
                    set: |c, v| c.voice.max_recording_seconds = u16::try_from(v).unwrap_or(60),
                },
            },
        ],
    },
    Category {
        icon: "\u{eb75}", // unmute
        label: "Speech",
        about: "Spoken replies in a voice conversation",
        controls: &[
            Control {
                label: "Spoken replies (voice mode)",
                section: "Replies",
                key: "voice.spoken_replies",
                kind: Kind::Toggle {
                    get: |c| c.voice.spoken_replies,
                    set: |c, v| c.voice.spoken_replies = v,
                },
            },
            Control {
                label: "Speech voice (optional)",
                section: "Replies",
                key: "voice.speech_voice",
                kind: Kind::Text {
                    get: |c| &c.voice.speech_voice,
                    set: |c, v| c.voice.speech_voice = v,
                },
            },
            Control {
                label: "Speech rate",
                section: "Replies",
                key: "voice.speech_rate",
                kind: Kind::Range {
                    min: 80,
                    max: 400,
                    step: 10,
                    divisor: 1,
                    suffix: " wpm",
                    get: |c| i32::from(c.voice.speech_rate),
                    set: |c, v| c.voice.speech_rate = u16::try_from(v).unwrap_or(180),
                },
            },
            Control {
                label: "Record/send shortcut",
                section: "Turns",
                key: "voice.turn_shortcut",
                kind: Kind::Text {
                    get: |c| &c.voice.turn_shortcut,
                    set: |c, v| c.voice.turn_shortcut = v,
                },
            },
            Control {
                label: "Busy Pi delivery",
                section: "Turns",
                key: "voice.busy_delivery",
                kind: Kind::Choice {
                    options: &["Refuse", "Steer", "Follow up"],
                    look: Look::Segments,
                    get: |c| match c.voice.busy_delivery {
                        skelly_config::VoiceDelivery::Idle => 0,
                        skelly_config::VoiceDelivery::Steer => 1,
                        skelly_config::VoiceDelivery::FollowUp => 2,
                    },
                    set: |c, i| {
                        c.voice.busy_delivery = match i {
                            1 => skelly_config::VoiceDelivery::Steer,
                            2 => skelly_config::VoiceDelivery::FollowUp,
                            _ => skelly_config::VoiceDelivery::Idle,
                        }
                    },
                },
            },
        ],
    },
];

/// A range value as its readout: `v` scaled down by `divisor` (to its decimals) plus `suffix`.
fn range_text(v: i32, divisor: i32, suffix: &str) -> String {
    if divisor <= 1 {
        format!("{v}{suffix}")
    } else {
        format!(
            "{:.*}{}",
            decimals_for(divisor),
            f64::from(v) / f64::from(divisor),
            suffix
        )
    }
}

/// Digits after the decimal point implied by a display `divisor` (10 -> 1, 100 -> 2).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "divisor is a small power of ten; log10 is a tiny non-negative value"
)]
fn decimals_for(divisor: i32) -> usize {
    f64::from(divisor.max(1)).log10().round() as usize
}

/// The rendered settings view's display list, for a [`skelly_render::SettingsView`].
pub(crate) struct Paint {
    /// The x of the nav/content divider (physical px); the nav strip fills to its left.
    pub(crate) nav_divider_x: f32,
    /// The quads over the frame: nav pills, section cards, row washes and widgets.
    pub(crate) quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub(crate) labels: Vec<ProseLabel>,
    /// Every clickable rect drawn this frame (physical px), in draw order: a widget's own
    /// target follows its row's, so [`target_at`] (which searches last-first) prefers it.
    pub(crate) targets: Vec<(PxRect, Target)>,
    /// The active page's first drawn row, to keep as its scroll ([`Settings::settle_rows`]).
    pub(crate) rows_first: usize,
}

/// What a click on the settings view lands on. Indices address the active category's
/// controls (or the nav's categories).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Target {
    /// The header close button (or the `esc to close` hint beside it): close the view.
    Close,
    /// A nav row: switch to this category.
    Category(usize),
    /// A control row outside its widget: focus the control.
    Row(usize),
    /// A toggle switch: flip it.
    Toggle(usize),
    /// One option of a choice - a segment, or a theme card: choose it.
    Choose {
        /// The control's index.
        index: usize,
        /// The option's index.
        option: usize,
    },
    /// A slider: set the value from the pointer's x along the track `[x, x + w)`.
    Slider {
        /// The control's index.
        index: usize,
        /// The track's left edge, physical px.
        x: f32,
        /// The track's width, physical px.
        w: f32,
    },
    /// A text value: start editing it.
    Text(usize),
    /// Something on the Workspaces page.
    Workspaces(workspace::Target),
}

impl Target {
    /// The control this target belongs to, if it is part of a control row.
    fn control(self) -> Option<usize> {
        match self {
            Target::Row(i) | Target::Toggle(i) | Target::Text(i) => Some(i),
            Target::Choose { index, .. } | Target::Slider { index, .. } => Some(index),
            Target::Close | Target::Category(_) | Target::Workspaces(_) => None,
        }
    }
}

/// The topmost target under physical `(px, py)`: the last one recorded whose rect contains it.
pub(crate) fn target_at(targets: &[(PxRect, Target)], px: f32, py: f32) -> Option<Target> {
    targets
        .iter()
        .rev()
        .find(|(r, _)| px >= r.x && px < r.x + r.w && py >= r.y && py < r.y + r.h)
        .map(|(_, t)| *t)
}

/// Settings-view state: whether it is open, the active category, and the focused
/// control within that category.
pub(crate) struct Settings {
    /// Whether the settings view is showing (captures all input while open).
    pub(crate) open: bool,
    category: usize,
    selected: usize,
    pub(crate) editing: Option<String>,
    select_all: bool,
    /// The target under the pointer, for the hover fill on nav and control rows.
    pub(crate) hover: Option<Target>,
    /// The slider being dragged (its control index + track), between press and release.
    pub(crate) drag: Option<Target>,
    /// The active page's scroll: its first drawn row.
    rows_first: usize,
    /// Whether the next paint scrolls the focused row into view.
    reveal_row: bool,
}

impl Settings {
    /// A closed settings view, at the first category and control.
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            category: 0,
            selected: 0,
            editing: None,
            select_all: false,
            hover: None,
            drag: None,
            rows_first: 0,
            reveal_row: false,
        }
    }

    /// Open the settings view fresh (first category, first control).
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.category = 0;
        self.selected = 0;
        self.rows_first = 0;
    }

    /// Jump from a pane's voice controls to their existing config-backed setup.
    pub(crate) fn open_voice(&mut self) {
        self.open();
        self.category = CATEGORIES
            .iter()
            .position(|c| c.label == "Voice")
            .unwrap_or(0);
        self.editing = None;
    }

    /// Close the settings view.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.editing = None;
        self.hover = None;
        self.drag = None;
    }

    /// Enter starts a text edit without changing the live configuration.
    pub(crate) fn start_text_edit(&mut self, config: &Config) -> bool {
        let Some(Control {
            kind: Kind::Text { get, .. },
            ..
        }) = self.controls().get(self.selected)
        else {
            return false;
        };
        self.editing = Some(get(config).into());
        self.select_all = false;
        true
    }

    /// Only validated edits reach the shared config; errors preserve the editable draft.
    pub(crate) fn commit_text(
        &mut self,
        config: &mut Config,
    ) -> Result<Option<&'static str>, skelly_config::ConfigError> {
        let Some(value) = &self.editing else {
            return Ok(None);
        };
        let Some(Control {
            key,
            kind: Kind::Text { set, .. },
            ..
        }) = self.controls().get(self.selected)
        else {
            return Ok(None);
        };
        let mut candidate = config.clone();
        set(&mut candidate, value.clone());
        candidate.validate()?;
        *config = candidate;
        self.editing = None;
        Ok(Some(key))
    }

    pub(crate) fn select_text(&mut self) {
        self.select_all = true;
    }

    pub(crate) fn backspace_text(&mut self) {
        if let Some(text) = &mut self.editing {
            if self.select_all {
                text.clear();
            } else {
                text.pop();
            }
        }
        self.select_all = false;
    }

    pub(crate) fn append_text(&mut self, text: &str) {
        if let Some(draft) = &mut self.editing {
            if self.select_all && text.chars().any(|c| !c.is_control()) {
                draft.clear();
                self.select_all = false;
            }
            for c in text.chars().filter(|c| !c.is_control()) {
                if draft.len() + c.len_utf8() > 4096 {
                    break;
                }
                draft.push(c);
            }
        }
    }

    /// The controls of the active category (none on the Workspaces page).
    fn controls(&self) -> &'static [Control] {
        CATEGORIES.get(self.category).map_or(&[], |c| c.controls)
    }

    /// Whether the Workspaces page is showing (its rows, not config controls, take input).
    pub(crate) fn on_workspaces_page(&self) -> bool {
        self.category == CATEGORIES.len()
    }

    /// Open the view on the Workspaces page with row `row` focused, optionally straight into
    /// renaming it from `name`.
    pub(crate) fn open_workspaces(&mut self, row: usize, name: Option<String>) {
        self.open();
        self.category = CATEGORIES.len();
        self.selected = row;
        self.editing = name;
        self.select_all = self.editing.is_some();
        self.reveal_row = true;
    }

    /// The focused row on the Workspaces page.
    pub(crate) fn selected_row(&self) -> usize {
        self.selected
    }

    /// Move the Workspaces-page focus by `delta` over `count` rows, clamped.
    pub(crate) fn move_row(&mut self, delta: i32, count: usize) {
        let last = i32::try_from(count.saturating_sub(1)).unwrap_or(0);
        let cur = i32::try_from(self.selected).unwrap_or(0);
        self.selected = usize::try_from((cur + delta).clamp(0, last)).unwrap_or(0);
        self.reveal_row = true;
    }

    /// Focus Workspaces-page row `row`, dropping an open rename of another row.
    pub(crate) fn select_row(&mut self, row: usize) {
        if row != self.selected {
            self.editing = None;
        }
        self.selected = row;
        self.reveal_row = true;
    }

    /// Scroll the active page's rows by wheel `lines` (positive = up), leaving focus put.
    pub(crate) fn scroll_rows(&mut self, lines: i32) {
        let by = usize::try_from(lines.unsigned_abs()).unwrap_or(usize::MAX);
        self.rows_first = if lines > 0 {
            self.rows_first.saturating_sub(by)
        } else {
            self.rows_first.saturating_add(by)
        };
        self.reveal_row = false;
    }

    /// Keep the scroll the last paint settled on (see [`Paint::rows_first`]).
    pub(crate) fn settle_rows(&mut self, first: usize) {
        self.rows_first = first;
        self.reveal_row = false;
    }

    /// Start renaming the focused Workspaces-page row from its current `name`, all selected
    /// so typing replaces it.
    pub(crate) fn start_rename(&mut self, name: String) {
        self.editing = Some(name);
        self.select_all = true;
    }

    /// Switch to the category at nav index `index` (a click), resetting the control selection.
    pub(crate) fn select_category(&mut self, index: usize) {
        if index <= CATEGORIES.len() && index != self.category {
            self.category = index;
            self.selected = 0;
            self.rows_first = 0;
            self.editing = None;
        }
    }

    /// Focus the control at `index` in the active category (a click), clamped.
    pub(crate) fn select(&mut self, index: usize) {
        self.selected = index.min(self.controls().len().saturating_sub(1));
    }

    /// The index of the text control being edited, if an edit is open.
    pub(crate) fn editing_index(&self) -> Option<usize> {
        self.editing.as_ref().map(|_| self.selected)
    }

    /// Choose option `option` of the choice control at `index` (a segment or theme-card click),
    /// returning the config key it wrote.
    pub(crate) fn choose(
        &mut self,
        config: &mut Config,
        index: usize,
        option: usize,
    ) -> Option<&'static str> {
        self.select(index);
        let control = self.controls().get(index)?;
        let Kind::Choice { options, set, .. } = &control.kind else {
            return None;
        };
        set(config, option.min(options.len().saturating_sub(1)));
        Some(control.key)
    }

    /// Set the range control at `index` to the value at `fraction` (0..=1) along its track,
    /// snapped to its step (a slider click or drag), returning the config key it wrote - or
    /// `None` when the value did not move, so a drag only applies and saves real steps.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "range bounds are small config values; the rounded step count fits an i32"
    )]
    pub(crate) fn slide(
        &mut self,
        config: &mut Config,
        index: usize,
        fraction: f32,
    ) -> Option<&'static str> {
        self.select(index);
        let control = self.controls().get(index)?;
        let Kind::Range {
            min,
            max,
            step,
            get,
            set,
            ..
        } = &control.kind
        else {
            return None;
        };
        let span = (max - min) as f32;
        let steps = (fraction.clamp(0.0, 1.0) * span / *step as f32).round() as i32;
        let value = (min + steps * step).clamp(*min, *max);
        if value == get(config) {
            return None;
        }
        set(config, value);
        Some(control.key)
    }

    /// Switch category by `delta` (wrapping, through the Workspaces page), resetting the
    /// control selection.
    pub(crate) fn cycle_category(&mut self, forward: bool) {
        let n = CATEGORIES.len() + 1;
        self.category = if forward {
            (self.category + 1) % n
        } else {
            (self.category + n - 1) % n
        };
        self.selected = 0;
        self.rows_first = 0;
    }

    /// Move the control selection by `delta`, clamped to the active category.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        let count = self.controls().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let last = i32::try_from(count - 1).unwrap_or(0);
        let cur = i32::try_from(self.selected).unwrap_or(0);
        self.selected = usize::try_from((cur + delta).clamp(0, last)).unwrap_or(0);
        self.reveal_row = true;
    }

    /// Change the focused control's value by a `←/→` nudge, returning the config key it
    /// wrote so the binary can apply the matching live effect and persist the file.
    pub(crate) fn adjust(&self, config: &mut Config, delta: i32) -> Option<&'static str> {
        let control = self.controls().get(self.selected)?;
        control.kind.adjust(config, delta);
        Some(control.key)
    }

    /// Activate the focused control (Enter/Space): flip a toggle, cycle a choice, or
    /// step a range up. Returns the config key it wrote.
    pub(crate) fn activate(&self, config: &mut Config) -> Option<&'static str> {
        let control = self.controls().get(self.selected)?;
        control.kind.activate(config);
        Some(control.key)
    }

    /// Build the settings view's display list within `panel` (physical px) at DPI `scale`,
    /// reading current values from `config` in `theme`'s UI tokens: the category nav, the
    /// header (the category's name over its summary, the esc hint and the close button), the
    /// active page - a config category's section cards, or the Workspaces page - and the
    /// footer's key hints. The renderer fills the panel and draws the nav divider beneath.
    pub(crate) fn build(
        &self,
        panel: PxRect,
        scale: f32,
        config: &Config,
        workspaces: &[workspace::Entry],
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> Paint {
        let at = Layout::new(panel, scale);
        let mut paint = Paint {
            nav_divider_x: at.divider,
            quads: Vec::new(),
            labels: Vec::new(),
            targets: Vec::new(),
            rows_first: 0,
        };
        self.push_header(&mut paint, measure, &at, scale, theme);
        self.push_nav(&mut paint, measure, &at, scale, theme);
        paint.rows_first = if self.on_workspaces_page() {
            self.push_workspaces(&mut paint, measure, workspaces, &at, scale, theme)
        } else {
            let ctx = PageCtx::new(self.controls(), config, &at, scale, theme, measure);
            self.push_page(&mut paint, measure, &ctx, &at)
        };
        let hints = if self.on_workspaces_page() {
            WORKSPACES_HINTS
        } else {
            FOOTER_HINTS
        };
        push_key_hints(
            &mut paint.quads,
            &mut paint.labels,
            measure,
            hints,
            (at.left, at.footer_y),
            scale,
            theme,
        );
        paint
    }

    /// Draw the header: the active page's name (heading) over its summary, the `✕` close button
    /// at the panel's right on the name's line, and an esc hint before it, which also closes
    /// unless a text edit is open. The button stays live during an edit: the click commits the
    /// draft first, as any click does.
    fn push_header(
        &self,
        paint: &mut Paint,
        measure: &mut TextMeasure,
        at: &Layout,
        scale: f32,
        theme: &Theme,
    ) {
        let (name, about) = CATEGORIES
            .get(self.category)
            .map_or((WORKSPACES_NAV.1, WORKSPACES_ABOUT), |c| (c.label, c.about));
        let labels = &mut paint.labels;
        let (h2, caption) = (FontRole::H2, FontRole::Caption);
        push_row(
            labels,
            measure,
            name,
            h2,
            theme.fg_primary,
            at.left,
            at.top,
            NAME_ROW_H,
            scale,
        );
        labels.push(ProseLabel {
            text: about.to_owned(),
            x: at.left,
            y: at.top + (NAME_ROW_H + SUMMARY_GAP) * scale,
            role: caption,
            color: theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
        });
        let side = CLOSE_SIDE * scale;
        let button = PxRect {
            x: at.edge - side,
            y: at.top + (NAME_ROW_H - CLOSE_SIDE) * 0.5 * scale,
            w: side,
            h: side,
        };
        let hovered = self.hover == Some(Target::Close);
        if hovered {
            paint.quads.push(ChromeQuad::rounded(
                button,
                hover_fill(theme, page_fill(theme)),
                CLOSE_RADIUS * scale,
            ));
        }
        let icon_w = measure.width(CLOSE_ICON, FontRole::Icon, None);
        let color = if hovered {
            theme.fg_primary
        } else {
            theme.fg_muted
        };
        let icon_x = button.x + (side - icon_w) * 0.5;
        let icon = FontRole::Icon;
        push_row(
            labels, measure, CLOSE_ICON, icon, color, icon_x, button.y, CLOSE_SIDE, scale,
        );
        paint.targets.push((button, Target::Close));
        let editing = self.editing.is_some();
        let hint = if editing {
            "enter save · esc cancel"
        } else {
            "esc to close"
        };
        let hint_right = button.x - CLOSE_GAP * scale;
        let muted = theme.fg_muted;
        push_right(
            labels, measure, hint, caption, muted, hint_right, button.y, CLOSE_SIDE, scale,
        );
        if !editing {
            let w = measure.width(hint, caption, None);
            let rect = PxRect {
                x: hint_right - w,
                y: button.y,
                w,
                h: side,
            };
            paint.targets.push((rect, Target::Close));
        }
    }

    /// Draw the Workspaces page: the session's workspaces and groups, focused + hovered from
    /// this view's state, in the content column. Returns the first row drawn (its scroll).
    fn push_workspaces(
        &self,
        paint: &mut Paint,
        measure: &mut TextMeasure,
        entries: &[workspace::Entry],
        at: &Layout,
        scale: f32,
        theme: &Theme,
    ) -> usize {
        let hover = match self.hover {
            Some(Target::Workspaces(t)) => Some(t),
            _ => None,
        };
        let page = workspace::Page {
            entries,
            selected: self.selected,
            editing: self.editing.as_deref(),
            hover,
            first: self.rows_first,
            reveal: self.reveal_row,
        };
        let area = workspace::Area {
            x: at.left,
            right: at.right,
            top: at.body_top,
            bottom: at.body_bottom,
        };
        let mut out = workspace::Out {
            quads: &mut paint.quads,
            labels: &mut paint.labels,
            targets: Vec::new(),
        };
        let first = page.build(&area, scale, theme, measure, &mut out);
        paint.targets.extend(
            out.targets
                .into_iter()
                .map(|(rect, t)| (rect, Target::Workspaces(t))),
        );
        first
    }

    /// Draw the left nav: a "Settings" title on the heading's line, then a pill per page - its
    /// codicon and name - the active one on an `accent.subtle` fill with an `accent` icon, a
    /// hovered one on a lighter wash. A hairline sets the Workspaces page apart from the config
    /// categories above it.
    fn push_nav(
        &self,
        paint: &mut Paint,
        measure: &mut TextMeasure,
        at: &Layout,
        scale: f32,
        theme: &Theme,
    ) {
        let page = page_fill(theme);
        let inset = NAV_INSET * scale;
        let x = at.panel.x + inset;
        let w = at.divider - at.panel.x - 2.0 * inset;
        let title_x = x + NAV_ICON_X * scale;
        let (title, primary) = (FontRole::Title, theme.fg_primary);
        let labels = &mut paint.labels;
        push_row(
            labels, measure, "Settings", title, primary, title_x, at.top, NAME_ROW_H, scale,
        );
        let mut y = at.top + (NAME_ROW_H + NAV_TITLE_GAP) * scale;
        for (index, (icon, name)) in nav_entries().enumerate() {
            if index == CATEGORIES.len() {
                let stroke = scale.max(1.0);
                y += (NAV_SEP_GAP - NAV_ROW_GAP) * scale;
                let rule = PxRect {
                    x: title_x,
                    y,
                    w: x + w - title_x,
                    h: stroke,
                };
                paint
                    .quads
                    .push(ChromeQuad::fill(rule, theme.border_subtle));
                y += stroke + NAV_SEP_GAP * scale;
            }
            let pill = PxRect {
                x,
                y,
                w,
                h: NAV_ROW_H * scale,
            };
            let active = index == self.category;
            let radius = NAV_RADIUS * scale;
            if active {
                let fill = theme.accent_subtle_on(page);
                paint.quads.push(ChromeQuad::rounded(pill, fill, radius));
            } else if self.hover == Some(Target::Category(index)) {
                let fill = hover_fill(theme, page);
                paint.quads.push(ChromeQuad::rounded(pill, fill, radius));
            }
            let row = PxRect {
                x: at.panel.x,
                w: at.divider - at.panel.x,
                ..pill
            };
            paint.targets.push((row, Target::Category(index)));
            let (icon_color, color) = if active {
                (theme.accent, theme.fg_primary)
            } else {
                (theme.fg_muted, theme.fg_secondary)
            };
            let icon_w = measure.width(icon, FontRole::Icon, None);
            let icon_x = x + NAV_ICON_X * scale + (16.0 * scale - icon_w) * 0.5;
            let labels = &mut paint.labels;
            let role = FontRole::Icon;
            push_row(
                labels, measure, icon, role, icon_color, icon_x, y, NAV_ROW_H, scale,
            );
            let label_x = x + (NAV_ICON_X + NAV_ICON_W) * scale;
            let role = FontRole::Label;
            push_row(
                labels, measure, name, role, color, label_x, y, NAV_ROW_H, scale,
            );
            y += (NAV_ROW_H + NAV_ROW_GAP) * scale;
        }
    }

    /// Draw the active category's page: its controls as section cards, each section under its
    /// label, the theme picker as a grid of preview cards. A page taller than the body scrolls
    /// by whole [`Unit`]s, with a `↑ N more` line in the header gap and a `↓ N more` line under
    /// the last unit, as the Workspaces page does; returns the first unit drawn, the scroll to
    /// keep.
    fn push_page(
        &self,
        paint: &mut Paint,
        measure: &mut TextMeasure,
        ctx: &PageCtx<'_>,
        at: &Layout,
    ) -> usize {
        let scale = ctx.scale;
        let units = units(ctx.controls, ctx.cols, scale);
        let focus_line = match ctx.controls.get(self.selected).map(|c| &c.kind) {
            Some(Kind::Choice {
                get,
                look: Look::Themes,
                ..
            }) => get(ctx.config) / ctx.cols,
            _ => 0,
        };
        let selected = units
            .iter()
            .position(|u| u.control == self.selected && u.line == focus_line)
            .unwrap_or(0);
        let heights: Vec<f32> = units.iter().map(|u| u.h).collect();
        let indicator = workspace::IND_H * scale;
        let (top, mut bottom) = (at.body_top, at.body_bottom);
        let overflow = heights.iter().sum::<f32>() > bottom - top;
        if overflow {
            bottom -= indicator;
        }
        let first = workspace::first_row(
            &heights,
            bottom - top,
            self.rows_first,
            selected,
            self.reveal_row,
        );
        let mut placed = Vec::new();
        let mut y = top;
        for (i, unit) in units.iter().enumerate().skip(first) {
            // Units past the viewport are scrolled out; the first one always draws.
            if i > first && y + unit.h > bottom {
                break;
            }
            placed.push((*unit, y));
            y += unit.h;
        }
        push_section_cards(&mut paint.quads, ctx, &placed);
        for (n, &(unit, y)) in placed.iter().enumerate() {
            if unit.head > 0.0 {
                push_section_label(paint, measure, ctx, unit, y);
            }
            let top = y + unit.head;
            if unit.cards {
                self.push_theme_line(paint, measure, ctx, unit, top);
            } else {
                let rule = n > 0 && unit.head == 0.0;
                self.push_control_row(paint, measure, ctx, unit.control, (top, rule));
            }
        }
        if overflow {
            // A control counts as hidden when any of its units is: a theme grid scrolled
            // part-way is still "more".
            let shown = first + placed.len();
            let hidden = (controls_in(&units[..first]), controls_in(&units[shown..]));
            let above = top - indicator;
            workspace::push_more(
                &mut paint.labels,
                measure,
                hidden,
                at.left,
                (above, y),
                scale,
                ctx.theme,
            );
        }
        first
    }

    /// One control's row at `at = (top, rule)` inside its section card: the focus or hover pill,
    /// a hairline above it when `rule` (unless it or the row above is lit), the label over its
    /// config key at the left, clipped short of the widget at the right.
    fn push_control_row(
        &self,
        paint: &mut Paint,
        measure: &mut TextMeasure,
        ctx: &PageCtx<'_>,
        index: usize,
        at: (f32, bool),
    ) {
        let (top, rule) = at;
        let (scale, theme) = (ctx.scale, ctx.theme);
        let rect = PxRect {
            x: ctx.left,
            y: top,
            w: ctx.right - ctx.left,
            h: ROW_H * scale,
        };
        paint.targets.push((rect, Target::Row(index)));
        let hovered = self.hover.and_then(Target::control);
        let lit = |i: usize| i == self.selected || hovered == Some(i);
        let card = card_fill(theme);
        let pill = inset(rect, PILL_INSET * scale);
        if index == self.selected {
            let fill = theme.accent_subtle_on(card);
            paint
                .quads
                .push(ChromeQuad::rounded(pill, fill, PILL_RADIUS * scale));
        } else if hovered == Some(index) {
            let fill = hover_fill(theme, card);
            paint
                .quads
                .push(ChromeQuad::rounded(pill, fill, PILL_RADIUS * scale));
        }
        if rule && !lit(index) && !lit(index.saturating_sub(1)) {
            let line = PxRect {
                x: rect.x + ROW_PAD_X * scale,
                y: top,
                w: rect.w - 2.0 * ROW_PAD_X * scale,
                h: scale.max(1.0),
            };
            paint
                .quads
                .push(ChromeQuad::fill(line, theme.border_subtle));
        }
        let focused = index == self.selected;
        let editing = self.editing.as_deref().filter(|_| focused);
        let right = rect.x + rect.w - ROW_PAD_X * scale;
        let widget_x = push_control(paint, measure, ctx, index, editing, (right, top));
        let control = &ctx.controls[index];
        let x = rect.x + ROW_PAD_X * scale;
        let room = (widget_x - LABEL_GAP * scale - x).max(0.0);
        let label_h = measure.line_height(FontRole::Label);
        let key_h = measure.line_height(FontRole::Mono);
        let y = top + (rect.h - label_h - KEY_GAP * scale - key_h) * 0.5;
        paint.labels.push(ProseLabel {
            text: control.label.to_owned(),
            x,
            y,
            role: FontRole::Label,
            color: theme.fg_primary,
            weight: None,
            max_w: room,
        });
        paint.labels.push(ProseLabel {
            text: control.key.to_owned(),
            x,
            y: y + label_h + KEY_GAP * scale,
            role: FontRole::Mono,
            color: theme.fg_muted,
            weight: None,
            max_w: room,
        });
    }

    /// One line of the theme grid at `top`: a preview card per theme on it, each a click target
    /// that picks the theme.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the grid has a handful of columns"
    )]
    fn push_theme_line(
        &self,
        paint: &mut Paint,
        measure: &mut TextMeasure,
        ctx: &PageCtx<'_>,
        unit: Unit,
        top: f32,
    ) {
        let index = unit.control;
        let Kind::Choice { options, get, .. } = &ctx.controls[index].kind else {
            return;
        };
        let chosen = get(ctx.config);
        let (scale, cols) = (ctx.scale, ctx.cols);
        let gap = THEME_GAP * scale;
        let w = (ctx.right - ctx.left - gap * (cols - 1) as f32) / cols as f32;
        let start = unit.line * cols;
        for option in start..(start + cols).min(options.len()) {
            let rect = PxRect {
                x: ctx.left + (option - start) as f32 * (w + gap),
                y: top,
                w,
                h: THEME_CARD_H * scale,
            };
            let card = ThemeCard {
                id: THEMES[option].0,
                name: options[option],
                selected: option == chosen,
                focused: index == self.selected,
                hovered: self.hover == Some(Target::Choose { index, option }),
            };
            let strip = THEME_STRIP_H * scale;
            let on = page_fill(ctx.theme);
            let quads = &mut paint.quads;
            card.push(
                quads,
                &mut paint.labels,
                measure,
                (rect, strip),
                on,
                scale,
                ctx.theme,
            );
            paint.targets.push((rect, Target::Choose { index, option }));
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

/// Where the view lays out, physical px.
struct Layout {
    /// The whole settings panel.
    panel: PxRect,
    /// The nav/content divider's x.
    divider: f32,
    /// The content column's left and right edges.
    left: f32,
    right: f32,
    /// The content area's right edge, where the close button ends (past the column's right
    /// when a wide window centers the column).
    edge: f32,
    /// The top of the heading's line.
    top: f32,
    /// The page body's top and bottom.
    body_top: f32,
    body_bottom: f32,
    /// The top of the footer's key-hint line.
    footer_y: f32,
}

impl Layout {
    fn new(panel: PxRect, scale: f32) -> Self {
        let divider = panel.x + NAV_WIDTH * scale;
        let area_left = divider + CONTENT_PAD * scale;
        let edge = panel.x + panel.w - PAD * scale;
        let spare = (edge - area_left - COLUMN_MAX_W * scale).max(0.0) * 0.5;
        let top = panel.y + PAD * scale;
        let footer_y = panel.y + panel.h - (PAD + HINT_H) * scale;
        Self {
            panel,
            divider,
            left: area_left + spare,
            right: edge - spare,
            edge,
            top,
            body_top: top + (HEADER_H + HEADER_GAP) * scale,
            body_bottom: footer_y - FOOTER_GAP * scale,
            footer_y,
        }
    }
}

/// What a config page draws with: its controls, the config they read, the content column, and
/// the shared widths worked out once per paint.
struct PageCtx<'a> {
    controls: &'static [Control],
    config: &'a Config,
    theme: &'a Theme,
    scale: f32,
    /// The content column's left and right edges.
    left: f32,
    right: f32,
    /// The theme grid's column count.
    cols: usize,
    /// One readout column for every slider on the page, sized to the widest value any of them
    /// can show, so the tracks line up and never shift while a value changes.
    readout_w: f32,
}

impl<'a> PageCtx<'a> {
    fn new(
        controls: &'static [Control],
        config: &'a Config,
        at: &Layout,
        scale: f32,
        theme: &'a Theme,
        measure: &mut TextMeasure,
    ) -> Self {
        let readout_w = controls
            .iter()
            .filter_map(|c| c.kind.readout_width(measure))
            .fold(0.0, f32::max);
        Self {
            controls,
            config,
            theme,
            scale,
            left: at.left,
            right: at.right,
            cols: theme_columns(at.right - at.left, scale),
            readout_w,
        }
    }
}

/// One block of a config page, top to bottom: a control's row, or one line of the theme grid.
/// The page scrolls by whole units.
#[derive(Clone, Copy)]
struct Unit {
    /// The control it draws.
    control: usize,
    /// Its line of the theme grid (`0` for a row).
    line: usize,
    /// Whether it is a line of theme cards rather than a control row.
    cards: bool,
    /// The section label (and the gap above it) drawn over it, physical px; `0` mid-section.
    head: f32,
    /// Its full height, `head` included, physical px.
    h: f32,
}

/// A page's units for `controls`, with the theme grid `cols` cards wide.
fn units(controls: &[Control], cols: usize, scale: f32) -> Vec<Unit> {
    let mut out = Vec::new();
    for (i, control) in controls.iter().enumerate() {
        let opens = i == 0 || controls[i - 1].section != control.section;
        let head = match (opens, i) {
            (false, _) => 0.0,
            (true, 0) => SECTION_LABEL_H * scale,
            (true, _) => (SECTION_GAP + SECTION_LABEL_H) * scale,
        };
        if let Kind::Choice {
            options,
            look: Look::Themes,
            ..
        } = control.kind
        {
            let lines = options.len().div_ceil(cols);
            for line in 0..lines {
                let head = if line == 0 { head } else { 0.0 };
                let gap = if line + 1 < lines { THEME_GAP } else { 0.0 };
                out.push(Unit {
                    control: i,
                    line,
                    cards: true,
                    head,
                    h: head + (THEME_CARD_H + gap) * scale,
                });
            }
        } else {
            out.push(Unit {
                control: i,
                line: 0,
                cards: false,
                head,
                h: head + ROW_H * scale,
            });
        }
    }
    out
}

/// The section label over `unit` (whose top is `y`): the section's name in uppercase
/// `micro`, and, over the theme grid (which has no row to show it), its config key at the
/// right.
fn push_section_label(
    paint: &mut Paint,
    measure: &mut TextMeasure,
    ctx: &PageCtx<'_>,
    unit: Unit,
    y: f32,
) {
    let control = &ctx.controls[unit.control];
    let micro = measure.line_height(FontRole::Micro);
    // The label sits on the bottom of its line, a small gap above the card.
    let line_bottom = y + unit.head - 8.0 * ctx.scale;
    paint.labels.push(ProseLabel {
        text: control.section.to_uppercase(),
        x: ctx.left + 2.0 * ctx.scale,
        y: line_bottom - micro,
        role: FontRole::Micro,
        color: ctx.theme.fg_muted,
        weight: None,
        max_w: f32::MAX,
    });
    if unit.cards {
        let mono = measure.line_height(FontRole::Mono);
        let w = measure.width(control.key, FontRole::Mono, None);
        paint.labels.push(ProseLabel {
            text: control.key.to_owned(),
            x: ctx.right - w,
            y: line_bottom - micro * 0.5 - mono * 0.5,
            role: FontRole::Mono,
            color: ctx.theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
        });
    }
}

/// How many controls `units` draw (a control's units are adjacent).
fn controls_in(units: &[Unit]) -> usize {
    let mut controls: Vec<usize> = units.iter().map(|u| u.control).collect();
    controls.dedup();
    controls.len()
}

/// The theme grid's columns in a content column `width` wide: four, or two where four would
/// squeeze a card below its narrowest. An even count keeps each dark/light pair side by side.
fn theme_columns(width: f32, scale: f32) -> usize {
    let four = 4.0 * THEME_CARD_MIN_W + 3.0 * THEME_GAP;
    if width >= four * scale {
        4
    } else {
        2
    }
}

/// The card behind each run of placed control rows that share a section: a `bg.base` fill in
/// a `border.subtle` ring, spanning the rows drawn (a scrolled section's card starts at its
/// first visible row).
fn push_section_cards(quads: &mut Vec<ChromeQuad>, ctx: &PageCtx<'_>, placed: &[(Unit, f32)]) {
    let scale = ctx.scale;
    let mut run: Option<(f32, f32)> = None;
    let mut flush = |run: &mut Option<(f32, f32)>| {
        if let Some((top, bottom)) = run.take() {
            let card = PxRect {
                x: ctx.left,
                y: top,
                w: ctx.right - ctx.left,
                h: bottom - top,
            };
            let (ring, fill) = (ctx.theme.border_subtle, card_fill(ctx.theme));
            push_ringed(quads, card, ring, fill, CARD_RADIUS * scale, scale);
        }
    };
    for &(unit, y) in placed {
        if unit.cards || unit.head > 0.0 {
            flush(&mut run);
        }
        if !unit.cards {
            let top = y + unit.head;
            let bottom = top + ROW_H * scale;
            run = Some(run.map_or((top, bottom), |(t, _)| (t, bottom)));
        }
    }
    flush(&mut run);
}

/// Every nav entry as `(icon, label)`: the config categories, then the Workspaces page.
fn nav_entries() -> impl Iterator<Item = (&'static str, &'static str)> {
    CATEGORIES
        .iter()
        .map(|c| (c.icon, c.label))
        .chain(std::iter::once(WORKSPACES_NAV))
}

/// The settings page's surface, under the nav and the content alike: `bg.sidebar`, which the
/// renderer fills the panel with (`cells::settings_frame_quads`). Washes over the page
/// composite over it.
pub(crate) fn page_fill(theme: &Theme) -> Srgb {
    theme.bg_sidebar
}

/// A section card's surface: `bg.base`, the surface the §09 widgets are drawn for, a step off
/// the page.
pub(crate) fn card_fill(theme: &Theme) -> Srgb {
    theme.bg_base.to_srgb()
}

/// A lighter wash than the focused row's `accent.subtle` band, for the row under the pointer.
pub(crate) fn hover_fill(theme: &Theme, base: Srgb) -> Srgb {
    theme.accent.over(base, theme.accent_subtle_alpha * 0.45)
}

/// `rect` shrunk by `d` on every side.
fn inset(rect: PxRect, d: f32) -> PxRect {
    PxRect {
        x: rect.x + d,
        y: rect.y + d,
        w: (rect.w - 2.0 * d).max(0.0),
        h: (rect.h - 2.0 * d).max(0.0),
    }
}

/// Push control `index`'s §09 widget for its [`Kind`] - a toggle switch, a segmented control,
/// a slider, or a text box - right-anchored at `at = (right, top)` in its row, recording the
/// widget's click [`Target`]s. `editing` is the row's open text draft. Returns the widget's
/// left edge, where the row's label must stop.
fn push_control(
    paint: &mut Paint,
    measure: &mut TextMeasure,
    ctx: &PageCtx<'_>,
    index: usize,
    editing: Option<&str>,
    at: (f32, f32),
) -> f32 {
    let (right, top) = at;
    let (scale, theme, config) = (ctx.scale, ctx.theme, ctx.config);
    let control = &ctx.controls[index];
    match &control.kind {
        Kind::Text { get, .. } => {
            let placeholder = if control.key == "voice.speech_voice" {
                "(system default)"
            } else {
                "(not set)"
            };
            let value = editing.unwrap_or_else(|| get(config));
            let max_w = (right - ctx.left) * 0.5;
            let text = (value, placeholder);
            let rect = push_input(
                paint,
                measure,
                text,
                editing.is_some(),
                (right, top, max_w),
                scale,
                theme,
            );
            let target = PxRect {
                x: rect.x,
                y: top,
                w: right - rect.x,
                h: ROW_H * scale,
            };
            paint.targets.push((target, Target::Text(index)));
            rect.x
        }
        Kind::Toggle { get, .. } => {
            let rect = push_toggle(&mut paint.quads, get(config), right, top, scale, theme);
            paint.targets.push((rect, Target::Toggle(index)));
            rect.x
        }
        Kind::Choice {
            look: Look::Themes, ..
        } => right,
        Kind::Choice {
            options,
            get,
            look: Look::Segments,
            ..
        } => {
            let selected = get(config).min(options.len().saturating_sub(1));
            let segments = push_segmented(
                &mut paint.quads,
                &mut paint.labels,
                measure,
                options,
                selected,
                right,
                top,
                scale,
                theme,
            );
            let left = segments.first().map_or(right, |r| r.x - SEG_PAD * scale);
            paint.targets.extend(
                segments
                    .into_iter()
                    .enumerate()
                    .map(|(option, rect)| (rect, Target::Choose { index, option })),
            );
            left
        }
        Kind::Range { .. } => push_range(paint, measure, ctx, index, at),
    }
}

/// Control `index`'s slider, right-anchored at `at = (right, top)` in its row, with its grab
/// target. Returns where the grab area starts, where the row's label must stop.
#[allow(
    clippy::cast_precision_loss,
    reason = "range bounds are small config values (u8/u16)"
)]
fn push_range(
    paint: &mut Paint,
    measure: &mut TextMeasure,
    ctx: &PageCtx<'_>,
    index: usize,
    at: (f32, f32),
) -> f32 {
    let (right, top) = at;
    let (scale, config) = (ctx.scale, ctx.config);
    let kind = &ctx.controls[index].kind;
    let Kind::Range { min, max, get, .. } = kind else {
        return right;
    };
    let fraction = if max > min {
        (get(config) - min) as f32 / (max - min) as f32
    } else {
        0.0
    };
    let track = push_slider(
        &mut paint.quads,
        &mut paint.labels,
        measure,
        &kind.value(config),
        fraction.clamp(0.0, 1.0),
        right,
        ctx.readout_w,
        top,
        scale,
        ctx.theme,
    );
    // The grab area spans the row's height and half a knob past each end, so the extremes are
    // as easy to hit as the middle.
    let slop = SLIDER_KNOB * scale * 0.5;
    let grab = PxRect {
        x: track.x - slop,
        y: top,
        w: track.w + 2.0 * slop,
        h: ROW_H * scale,
    };
    let target = Target::Slider {
        index,
        x: track.x,
        w: track.w,
    };
    paint.targets.push((grab, target));
    grab.x
}

/// A text value's input box, right-anchored at `at = (right, top, max_w)` in its row: a
/// `bg.inset` well in a `border` ring (`accent` while editing, with a caret), the `text.0` value
/// in mono - its tail when it does not fit, so a long path keeps its file name - or the muted
/// `text.1` placeholder when empty. Returns the box.
fn push_input(
    paint: &mut Paint,
    measure: &mut TextMeasure,
    text: (&str, &str),
    editing: bool,
    at: (f32, f32, f32),
    scale: f32,
    theme: &Theme,
) -> PxRect {
    let (value, placeholder) = text;
    let (right, top, max_w) = at;
    let w = max_w.min(INPUT_W * scale);
    let h = INPUT_H * scale;
    let rect = PxRect {
        x: right - w,
        y: top + (ROW_H * scale - h) * 0.5,
        w,
        h,
    };
    let ring = if editing { theme.accent } else { theme.border };
    push_ringed(
        &mut paint.quads,
        rect,
        ring,
        theme.bg_inset,
        INPUT_RADIUS * scale,
        scale,
    );
    let (shown, color) = if editing {
        (format!("{value}\u{258f}"), theme.fg_primary)
    } else if value.is_empty() {
        (placeholder.to_owned(), theme.fg_muted)
    } else {
        (value.to_owned(), theme.fg_primary)
    };
    let pad = INPUT_PAD_X * scale;
    let room = (w - 2.0 * pad).max(0.0);
    let line = measure.line_height(FontRole::Mono);
    paint.labels.push(ProseLabel {
        text: tail_fit(&shown, room, measure),
        x: rect.x + pad,
        y: rect.y + (h - line) * 0.5,
        role: FontRole::Mono,
        color,
        weight: None,
        max_w: room,
    });
    rect
}

/// `text` if it fits `room` in mono, else its tail behind an ellipsis: the end of a path (its
/// file name) and an edit's caret matter most.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a mono character count that fits a box, small and non-negative"
)]
fn tail_fit(text: &str, room: f32, measure: &mut TextMeasure) -> String {
    if measure.width(text, FontRole::Mono, None) <= room {
        return text.to_owned();
    }
    let char_w = measure.width("0", FontRole::Mono, None).max(1.0);
    let keep = ((room / char_w).floor() as usize).saturating_sub(1);
    let count = text.chars().count();
    let tail: String = text.chars().skip(count.saturating_sub(keep)).collect();
    format!("\u{2026}{tail}")
}

/// A §09 toggle switch, right-anchored: a 38x22 pill (`accent` on / `border` off) with an
/// 18px knob sprung to the right (on) or left (off). Returns the pill's rect.
fn push_toggle(
    quads: &mut Vec<ChromeQuad>,
    on: bool,
    right: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
) -> PxRect {
    let w = TOGGLE_W * scale;
    let h = TOGGLE_H * scale;
    let x = right - w;
    let y = top + (ROW_H * scale - h) * 0.5;
    let track = if on { theme.accent } else { theme.border };
    quads.push(ChromeQuad::rounded(PxRect { x, y, w, h }, track, h * 0.5));
    let knob = TOGGLE_KNOB * scale;
    let inset = TOGGLE_INSET * scale;
    let knob_x = if on { x + w - inset - knob } else { x + inset };
    // Knob-on reads as `bg.base`; `bg.inset` is the same near-black in `Srgb` at knob size.
    let knob_color = if on { theme.bg_inset } else { theme.fg_muted };
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: knob_x,
            y: y + inset,
            w: knob,
            h: knob,
        },
        knob_color,
        knob * 0.5,
    ));
    PxRect { x, y, w, h }
}

/// A §09 segmented control, right-anchored: a `bg.inset` container in a `border` ring holding
/// one padded segment per option; the selected one gets a `bg.elevated` fill. Returns each
/// segment's rect, in option order.
#[allow(
    clippy::too_many_arguments,
    reason = "one focused segmented-control builder"
)]
fn push_segmented(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    options: &[&str],
    selected: usize,
    right: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
) -> Vec<PxRect> {
    let pad = SEG_PAD * scale;
    let item_pad = SEG_ITEM_PAD_X * scale;
    let seg_h = SEG_H * scale;
    let line_h = measure.line_height(FontRole::Caption);
    // Measure each segment (text + horizontal padding) to size the container.
    let widths: Vec<f32> = options
        .iter()
        .map(|o| measure.width(o, FontRole::Caption, None) + 2.0 * item_pad)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 2.0 * pad;
    let cx = right - total;
    let cy = top + (ROW_H * scale - (seg_h + 2.0 * pad)) * 0.5;
    let container = PxRect {
        x: cx,
        y: cy,
        w: total,
        h: seg_h + 2.0 * pad,
    };
    let radius = SEG_RADIUS * scale;
    push_ringed(
        quads,
        container,
        theme.border,
        theme.bg_inset,
        radius,
        scale,
    );
    let mut x = cx + pad;
    let mut rects = Vec::with_capacity(options.len());
    for (i, option) in options.iter().enumerate() {
        let w = widths[i];
        let active = i == selected;
        rects.push(PxRect {
            x,
            y: cy,
            w,
            h: seg_h + 2.0 * pad,
        });
        if active {
            quads.push(ChromeQuad::rounded(
                PxRect {
                    x,
                    y: cy + pad,
                    w,
                    h: seg_h,
                },
                theme.bg_elevated,
                SEG_ITEM_RADIUS * scale,
            ));
        }
        labels.push(ProseLabel {
            text: (*option).to_owned(),
            x: x + item_pad,
            y: cy + pad + (seg_h - line_h) * 0.5,
            role: FontRole::Caption,
            color: if active {
                theme.fg_primary
            } else {
                theme.fg_secondary
            },
            weight: None,
            max_w: f32::MAX,
        });
        x += w;
    }
    rects
}

/// A §09 slider, right-anchored: a mono `accent` value readout right-aligned in a
/// `readout_w` column, then a 6px `border.subtle` track with an `accent` fill up to `fraction`
/// and a 14px `fg.primary` knob at that point. Returns the track's rect.
#[allow(clippy::too_many_arguments, reason = "one focused slider builder")]
fn push_slider(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    value: &str,
    fraction: f32,
    right: f32,
    readout_w: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
) -> PxRect {
    let row_h = ROW_H * scale;
    // The value readout (mono accent), right-aligned.
    let value_w = measure.width(value, FontRole::Mono, None);
    let column_w = readout_w.max(value_w);
    let vline = measure.line_height(FontRole::Mono);
    labels.push(ProseLabel {
        text: value.to_owned(),
        x: right - value_w,
        y: top + (row_h - vline) * 0.5,
        role: FontRole::Mono,
        color: theme.accent,
        weight: None,
        max_w: f32::MAX,
    });
    // The track, to the left of the value.
    let track_w = SLIDER_W * scale;
    let track_h = SLIDER_TRACK_H * scale;
    let track_x = right - column_w - SLIDER_VALUE_GAP * scale - track_w;
    let track_y = top + (row_h - track_h) * 0.5;
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: track_x,
            y: track_y,
            w: track_w,
            h: track_h,
        },
        theme.border_subtle,
        track_h * 0.5,
    ));
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: track_x,
            y: track_y,
            w: (track_w * fraction).max(track_h),
            h: track_h,
        },
        theme.accent,
        track_h * 0.5,
    ));
    let knob = SLIDER_KNOB * scale;
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: track_x + track_w * fraction - knob * 0.5,
            y: top + (row_h - knob) * 0.5,
            w: knob,
            h: knob,
        },
        theme.fg_primary,
        knob * 0.5,
    ));
    PxRect {
        x: track_x,
        y: track_y,
        w: track_w,
        h: track_h,
    }
}

/// A theme's preview card (§10.9): a miniature of the theme in its own tokens - a sidebar strip
/// with its tabs, and lines of text in its ink, accent and diff colors - over a `bg.surface`
/// strip naming it, ringed in accent with a check when it is the theme in use.
struct ThemeCard<'a> {
    /// The theme's config name, which resolves its tokens.
    id: &'a str,
    /// Its display name.
    name: &'a str,
    /// Whether it is the theme in use.
    selected: bool,
    /// Whether its picker has keyboard focus (the selected card's ring turns solid and thick).
    focused: bool,
    /// Whether the pointer is over it.
    hovered: bool,
}

impl ThemeCard<'_> {
    /// Draw the card in `at = (rect, strip_h)` - its name strip `strip_h` tall - on the surface
    /// `on` (which the selected ring blends over), in the active `theme`'s chrome.
    #[allow(clippy::too_many_arguments, reason = "one focused card builder")]
    fn push(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        measure: &mut TextMeasure,
        at: (PxRect, f32),
        on: Srgb,
        scale: f32,
        theme: &Theme,
    ) {
        let (rect, strip_h) = at;
        let stroke = scale.max(1.0);
        let (ring, ring_w) = match (self.selected, self.focused) {
            (true, true) => (theme.accent, 2.0 * stroke),
            (true, false) => (theme.accent.over(on, 0.6), stroke),
            (false, _) if self.hovered => (theme.border_strong, stroke),
            (false, _) => (theme.border, stroke),
        };
        let radius = THEME_RADIUS * scale;
        quads.push(ChromeQuad::rounded(rect, ring, radius));
        let inner = inset(rect, ring_w);
        let radius = (radius - ring_w).max(0.0);
        let card = Theme::resolve(self.id);
        quads.push(ChromeQuad::rounded(inner, card.bg_base.to_srgb(), radius));
        let band = PxRect {
            h: (inner.h - strip_h).max(0.0),
            ..inner
        };
        push_preview(quads, band, radius, &card, scale);
        // The name strip, square-topped under the preview.
        let strip = PxRect {
            y: band.y + band.h,
            h: inner.h - band.h,
            ..inner
        };
        quads.push(ChromeQuad::rounded(strip, theme.bg_surface, radius));
        let top = PxRect {
            h: radius.min(strip.h),
            ..strip
        };
        quads.push(ChromeQuad::fill(top, theme.bg_surface));
        let pad = 9.0 * scale;
        let check_w = if self.selected {
            let w = measure.width(CHECK_ICON, FontRole::Icon, None);
            let line = measure.line_height(FontRole::Icon);
            labels.push(ProseLabel {
                text: CHECK_ICON.to_owned(),
                x: strip.x + strip.w - pad - w,
                y: strip.y + (strip.h - line) * 0.5,
                role: FontRole::Icon,
                color: theme.accent,
                weight: None,
                max_w: f32::MAX,
            });
            w + 4.0 * scale
        } else {
            0.0
        };
        let line = measure.line_height(FontRole::Caption);
        labels.push(ProseLabel {
            text: self.name.to_owned(),
            x: strip.x + pad,
            y: strip.y + (strip.h - line) * 0.5,
            role: FontRole::Caption,
            color: if self.selected {
                theme.fg_primary
            } else {
                theme.fg_secondary
            },
            weight: None,
            max_w: (strip.w - 2.0 * pad - check_w).max(0.0),
        });
    }
}

/// A theme card's miniature in its top `band` (whose top corners round by `radius`), all in the
/// `card` theme's tokens: a `bg.sidebar` strip down the left with an `accent` tab over two quiet
/// ones, and four lines of text on `bg.base` - a prompt with its command and cursor, a hunk
/// header, an added line and a removed one.
fn push_preview(quads: &mut Vec<ChromeQuad>, band: PxRect, radius: f32, card: &Theme, scale: f32) {
    let side_w = (band.w * 0.2).max(2.0 * radius);
    let side = PxRect {
        w: side_w,
        h: band.h + radius,
        ..band
    };
    quads.push(ChromeQuad::rounded(side, card.bg_sidebar, radius));
    let square = PxRect {
        x: band.x + radius,
        w: side_w - radius,
        ..band
    };
    quads.push(ChromeQuad::fill(square, card.bg_sidebar));
    let bar_h = (band.h * 0.08).clamp(3.0 * scale, 5.0 * scale);
    // Each bar is `(x, at, w, color)`: its left, its center as a fraction down the band, its
    // width, and its token.
    let mut bar = |x: f32, at: f32, w: f32, color: Srgb| {
        let rect = PxRect {
            x,
            y: band.y + band.h * at - bar_h * 0.5,
            w,
            h: bar_h,
        };
        quads.push(ChromeQuad::rounded(rect, color, bar_h * 0.5));
        x + w + bar_h
    };
    let tab_x = band.x + side_w * 0.22;
    let tab_w = side_w * 0.56;
    bar(tab_x, 0.3, tab_w, card.accent);
    bar(tab_x, 0.5, tab_w * 0.8, card.fg_faint);
    bar(tab_x, 0.7, tab_w * 0.9, card.fg_faint);
    let pad = 8.0 * scale;
    let x = band.x + side_w + pad;
    let room = (band.x + band.w - pad - x).max(0.0);
    let next = bar(x, 0.22, room * 0.07, card.accent);
    let next = bar(next, 0.22, room * 0.4, card.fg_primary);
    // The prompt's cursor.
    bar(next, 0.22, bar_h * 1.4, card.accent);
    let next = bar(x, 0.42, room * 0.22, card.diff_hunk);
    bar(next, 0.42, room * 0.3, card.fg_muted);
    bar(x, 0.62, room * 0.56, card.diff_add);
    bar(x, 0.82, room * 0.44, card.diff_del);
}

/// A row of key hints from `at = (x, y)`: each key in a `bg.surface` keycap (`micro`), then
/// what it does (`caption`, `fg.muted`).
fn push_key_hints(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    hints: &[(&str, &str)],
    at: (f32, f32),
    scale: f32,
    theme: &Theme,
) {
    let (left, y) = at;
    let key_pad = HINT_KEY_PAD * scale;
    let h = HINT_H * scale;
    let key_line = measure.line_height(FontRole::Micro);
    let action_line = measure.line_height(FontRole::Caption);
    let mut x = left;
    for (i, (key, action)) in hints.iter().enumerate() {
        if i > 0 {
            x += HINT_GAP * scale;
        }
        let key_w = measure.width(key, FontRole::Micro, None) + 2.0 * key_pad;
        let cap = PxRect { x, y, w: key_w, h };
        quads.push(ChromeQuad::rounded(cap, theme.bg_surface, 4.0 * scale));
        labels.push(ProseLabel {
            text: (*key).to_owned(),
            x: x + key_pad,
            y: y + (h - key_line) * 0.5,
            role: FontRole::Micro,
            color: theme.fg_primary,
            weight: None,
            max_w: f32::MAX,
        });
        x += key_w + HINT_KEY_GAP * scale;
        labels.push(ProseLabel {
            text: (*action).to_owned(),
            x,
            y: y + (h - action_line) * 0.5,
            role: FontRole::Caption,
            color: theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
        });
        x += measure.width(action, FontRole::Caption, None);
    }
}

/// Push one label vertically centered in a row of `row_h` logical px whose top is physical
/// `top` (`row_h = 0` centers the label on `top`).
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
pub(crate) fn push_row(
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

/// Push a right-anchored label ending at `right`, vertically centered in the row.
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
) {
    let x = right - measure.width(text, role, None);
    push_row(labels, measure, text, role, color, x, top, row_h, scale);
}

#[cfg(test)]
mod tests {
    use super::{
        card_fill, hover_fill, page_fill, target_at, Kind, Settings, Target, CATEGORIES,
        CHECK_ICON, CLOSE_ICON, HEADER_H, HINT_H, PAD,
    };
    use skelly_config::Config;
    use skelly_render::{PxRect, Srgb, TextMeasure, Theme, THEMES, THEME_LABELS};

    #[test]
    fn adjusting_theme_writes_the_config_and_returns_its_key() {
        let mut config = Config::default();
        let settings = Settings::new(); // Appearance / Theme is the first control
        assert_eq!(config.appearance.theme, "ossein-dark");
        let key = settings.adjust(&mut config, 1);
        assert_eq!(key, Some("appearance.theme"));
        assert_eq!(config.appearance.theme, "ossein-light");
        // Left from the first option clamps (stays on Ossein Dark once back at 0).
        settings.adjust(&mut config, -1);
        assert_eq!(config.appearance.theme, "ossein-dark");
        settings.adjust(&mut config, -1);
        assert_eq!(config.appearance.theme, "ossein-dark");
        // Right walks every theme in picker order, ending on the last.
        for (name, _) in &THEMES[1..] {
            settings.adjust(&mut config, 1);
            assert_eq!(config.appearance.theme, *name);
        }
        settings.adjust(&mut config, 1);
        assert_eq!(config.appearance.theme, "kanagawa-light");
    }

    #[test]
    fn range_controls_clamp_at_their_bounds() {
        let mut config = Config::default();
        let mut settings = Settings::new();
        settings.move_selection(1); // Appearance / Font size (8..=32)
        for _ in 0..100 {
            settings.adjust(&mut config, 1);
        }
        assert_eq!(config.appearance.font_size, 32);
        for _ in 0..100 {
            settings.adjust(&mut config, -1);
        }
        assert_eq!(config.appearance.font_size, 8);
    }

    #[test]
    fn activate_flips_a_toggle_and_cycles_a_choice() {
        let mut config = Config::default();
        let mut settings = Settings::new();
        // Jump to the Git category (Diff layout is a choice).
        while settings.category_label() != "Git" {
            settings.cycle_category(true);
        }
        assert_eq!(config.git.diff_view, skelly_config::DiffView::Unified);
        settings.activate(&mut config);
        assert_eq!(config.git.diff_view, skelly_config::DiffView::Split);
        settings.activate(&mut config); // wraps back
        assert_eq!(config.git.diff_view, skelly_config::DiffView::Unified);
    }

    #[test]
    fn selection_and_category_stay_in_range() {
        let mut settings = Settings::new();
        settings.move_selection(-5); // cannot go below the first control
        assert_eq!(settings.selected_index(), 0);
        settings.move_selection(1000); // clamps to the last control of Appearance
        assert_eq!(settings.selected_index(), CATEGORIES[0].controls.len() - 1);
        // Cycling category resets the control selection.
        settings.cycle_category(true); // Appearance -> Sidebar
        assert_eq!(settings.selected_index(), 0);
        // Sidebar -> Appearance -> (wrap) the Workspaces page, then the last config category.
        settings.cycle_category(false);
        settings.cycle_category(false);
        assert_eq!(settings.category_label(), "Workspaces");
        assert!(settings.on_workspaces_page());
        settings.cycle_category(false);
        assert_eq!(
            settings.category_label(),
            CATEGORIES[CATEGORIES.len() - 1].label
        );
    }

    #[test]
    fn build_marks_the_active_category_and_focused_control() {
        let config = Config::default();
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(1.0);
        let mut settings = Settings::new();
        settings.move_selection(2); // Appearance / Line height
        let paint = settings.build(panel(), 1.0, &config, &[], &theme, &mut m);
        // The heading names the page over its summary, and the nav pill and the focused row's
        // pill each wear `accent.subtle` over their surface.
        assert!(paint.labels.iter().any(|l| l.text == "Appearance"));
        assert!(paint.labels.iter().any(|l| l.text == CATEGORIES[0].about));
        assert!(has_fill(&paint, theme.accent_subtle_on(page_fill(&theme))));
        assert!(has_fill(&paint, theme.accent_subtle_on(card_fill(&theme))));
        // The active nav icon is accent; the others are muted.
        let icon = |text: &str| paint.labels.iter().find(|l| l.text == text).unwrap().color;
        assert_eq!(icon(CATEGORIES[0].icon), theme.accent);
        assert_eq!(icon(CATEGORIES[1].icon), theme.fg_muted);
        // Rows show their config key under their label.
        assert!(paint
            .labels
            .iter()
            .any(|l| l.text == "appearance.line_height"));
        assert!(paint.nav_divider_x > 0.0);
    }

    /// Whether `paint` draws a quad filled with `color`.
    fn has_fill(paint: &super::Paint, color: Srgb) -> bool {
        paint.quads.iter().any(|q| q.color == color)
    }

    fn panel() -> PxRect {
        PxRect {
            x: 0.0,
            y: 0.0,
            w: 1200.0,
            h: 800.0,
        }
    }

    /// The center of the first recorded rect whose target matches `want`.
    fn center_of(targets: &[(PxRect, Target)], want: impl Fn(&Target) -> bool) -> (f32, f32) {
        let (r, _) = targets
            .iter()
            .find(|(_, t)| want(t))
            .expect("target recorded");
        (r.x + r.w * 0.5, r.y + r.h * 0.5)
    }

    #[test]
    fn clicks_land_on_the_nav_row_and_widget_that_is_drawn() {
        let config = Config::default();
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(1.0);
        let settings = Settings::new();
        let paint = settings.build(panel(), 1.0, &config, &[], &theme, &mut m);
        let t = &paint.targets;
        for i in 0..=CATEGORIES.len() {
            let (x, y) = center_of(t, |t| *t == Target::Category(i));
            assert_eq!(target_at(t, x, y), Some(Target::Category(i)));
        }
        // A widget wins over its row; the row's label side still focuses the row.
        let (x, y) = center_of(t, |t| matches!(t, Target::Toggle(_)));
        assert!(matches!(target_at(t, x, y), Some(Target::Toggle(_))));
        let (row, _) = t.iter().find(|(_, t)| *t == Target::Row(1)).unwrap();
        let label_side = (row.x + 40.0, row.y + row.h * 0.5);
        assert_eq!(
            target_at(t, label_side.0, label_side.1),
            Some(Target::Row(1))
        );
        // Every theme is a card of its own.
        for option in 0..THEMES.len() {
            let want = Target::Choose { index: 0, option };
            let (x, y) = center_of(t, |t| *t == want);
            assert_eq!(target_at(t, x, y), Some(want));
        }
        assert_eq!(target_at(t, -10.0, -10.0), None);
        assert!(t.iter().any(|(_, t)| *t == Target::Close));
    }

    #[test]
    fn the_theme_grid_shows_every_theme_and_checks_the_one_in_use() {
        let mut config = Config::default();
        config.appearance.theme = "gruvbox-light".into();
        let theme = Theme::resolve("ossein-light");
        let mut m = TextMeasure::new(1.0);
        let settings = Settings::new();
        let paint = settings.build(panel(), 1.0, &config, &[], &theme, &mut m);
        for name in THEME_LABELS {
            assert!(paint.labels.iter().any(|l| l.text == *name), "{name}");
        }
        let chosen = THEMES
            .iter()
            .position(|(id, _)| *id == "gruvbox-light")
            .unwrap();
        let card = |option: usize| {
            paint
                .targets
                .iter()
                .find(|(_, t)| *t == Target::Choose { index: 0, option })
                .map(|(r, _)| *r)
                .unwrap()
        };
        // One check, on the chosen card, in accent.
        let checks: Vec<_> = paint
            .labels
            .iter()
            .filter(|l| l.text == CHECK_ICON)
            .collect();
        assert_eq!(checks.len(), 1);
        let (check, rect) = (checks[0], card(chosen));
        assert!(check.x > rect.x && check.x < rect.x + rect.w);
        assert!(check.y > rect.y && check.y < rect.y + rect.h);
        assert_eq!(check.color, theme.accent);
        // Each card previews its own theme: its `bg.base` fills the card behind the preview.
        for (option, (id, _)) in THEMES.iter().enumerate() {
            let own = Theme::resolve(id).bg_base.to_srgb();
            let rect = card(option);
            assert!(
                paint.quads.iter().any(|q| q.color == own
                    && q.rect.x >= rect.x
                    && q.rect.x + q.rect.w <= rect.x + rect.w + 0.01),
                "{id} previews its own bg.base"
            );
        }
        // Dark and light variants sit side by side: four to a line here.
        assert!((card(0).y - card(3).y).abs() < 0.01);
        assert!(card(4).y > card(3).y);
    }

    #[test]
    fn config_rows_scroll_above_the_footer_in_a_minimum_size_window() {
        let config = Config::default();
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(1.0);
        // The guide's 720 x 480 minimum window, below the 38px title strip.
        let short = PxRect {
            x: 0.0,
            y: 38.0,
            w: 720.0,
            h: 442.0,
        };
        let footer_top = short.y + short.h - PAD - HINT_H;
        let rows = |paint: &super::Paint| -> Vec<(PxRect, usize)> {
            paint
                .targets
                .iter()
                .filter_map(|(r, t)| match t {
                    Target::Row(i) => Some((*r, *i)),
                    _ => None,
                })
                .collect()
        };
        let mut settings = Settings::new(); // Appearance: more rows than fit
        let count = settings.controls().len();
        let paint = settings.build(short, 1.0, &config, &[], &theme, &mut m);
        let drawn = rows(&paint);
        assert!(drawn.len() < count, "not every Appearance row fits");
        assert!(drawn.iter().all(|(r, _)| r.y + r.h <= footer_top));
        assert!(paint.labels.iter().any(|l| l.text.starts_with('\u{2193}')));

        // Moving focus to the last row scrolls it into view, still above the footer.
        settings.move_selection(i32::try_from(count).unwrap());
        let paint = settings.build(short, 1.0, &config, &[], &theme, &mut m);
        settings.settle_rows(paint.rows_first);
        let drawn = rows(&paint);
        assert_eq!(drawn.last().map(|(_, i)| *i), Some(count - 1));
        assert!(drawn.iter().all(|(r, _)| r.y + r.h <= footer_top));
        assert!(paint.labels.iter().any(|l| l.text.starts_with('\u{2191}')));

        // A long wheel scroll up brings the theme grid, the first block, back.
        settings.scroll_rows(100);
        let paint = settings.build(short, 1.0, &config, &[], &theme, &mut m);
        assert_eq!(paint.rows_first, 0);
        assert!(paint.targets.iter().any(|(_, t)| *t
            == Target::Choose {
                index: 0,
                option: 0
            }));
    }

    #[test]
    fn the_close_button_sits_top_right_and_stays_live_while_editing() {
        let config = Config::default();
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let mut settings = Settings::new();
        let paint = settings.build(panel(), 2.0, &config, &[], &theme, &mut m);
        let button = paint
            .targets
            .iter()
            .find(|(_, t)| *t == Target::Close)
            .map(|(r, _)| *r)
            .expect("a close button");
        let p = panel();
        assert!((button.x + button.w - (p.x + p.w - PAD * 2.0)).abs() < 0.5);
        assert!(button.y < p.y + (PAD + HEADER_H) * 2.0);
        assert!(paint.labels.iter().any(|l| l.text == CLOSE_ICON));
        let (x, y) = (button.x + button.w * 0.5, button.y + button.h * 0.5);
        assert_eq!(target_at(&paint.targets, x, y), Some(Target::Close));

        // While a text edit is open the hint turns into "enter save", but the button stays.
        settings.editing = Some(String::new());
        let paint = settings.build(panel(), 2.0, &config, &[], &theme, &mut m);
        let closes: Vec<_> = paint
            .targets
            .iter()
            .filter(|(_, t)| *t == Target::Close)
            .collect();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].0, button);
    }

    #[test]
    fn choice_and_slider_clicks_write_their_one_key() {
        let mut config = Config::default();
        let mut settings = Settings::new();
        // Cursor style is a segmented choice (control 5 of Appearance).
        assert_eq!(
            settings.choose(&mut config, 5, 2),
            Some("appearance.cursor")
        );
        assert_eq!(
            config.appearance.cursor,
            skelly_config::CursorStyle::Underline
        );
        assert_eq!(settings.selected_index(), 5);
        // A theme card picks its theme; an out-of-range option clamps to the last.
        let last = THEMES.len() - 1;
        assert_eq!(settings.choose(&mut config, 0, 2), Some("appearance.theme"));
        assert_eq!(config.appearance.theme, THEMES[2].0);
        assert_eq!(
            settings.choose(&mut config, 0, 99),
            Some("appearance.theme")
        );
        assert_eq!(config.appearance.theme, THEMES[last].0);
        // Font size slides 8..=32 along the track, snapping to whole steps.
        assert_eq!(
            settings.slide(&mut config, 1, 1.0),
            Some("appearance.font_size")
        );
        assert_eq!(config.appearance.font_size, 32);
        assert_eq!(
            settings.slide(&mut config, 1, 0.5),
            Some("appearance.font_size")
        );
        assert_eq!(config.appearance.font_size, 20);
        // A drag that stays on the same step writes nothing.
        assert_eq!(settings.slide(&mut config, 1, 0.51), None);
        assert_eq!(
            settings.slide(&mut config, 1, -3.0),
            Some("appearance.font_size")
        );
        assert_eq!(config.appearance.font_size, 8);
        // A slide on a non-range control writes nothing.
        assert_eq!(settings.slide(&mut config, 0, 0.5), None);
    }

    #[test]
    fn slider_tracks_share_one_column_whatever_their_values() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(1.0);
        let settings = Settings::new();
        let track_xs = |config: &Config, m: &mut TextMeasure| -> Vec<f32> {
            let paint = settings.build(panel(), 1.0, config, &[], &theme, m);
            paint
                .targets
                .iter()
                .filter_map(|(_, t)| match t {
                    Target::Slider { x, .. } => Some(*x),
                    _ => None,
                })
                .collect()
        };
        let mut config = Config::default();
        let before = track_xs(&config, &mut m);
        assert!(before.len() >= 2);
        assert!(before.iter().all(|x| (x - before[0]).abs() < 0.01));
        // A wider readout (8px -> 32px) must not move its track mid-drag.
        config.appearance.font_size = 8;
        let narrow = track_xs(&config, &mut m);
        config.appearance.font_size = 32;
        assert_eq!(narrow, track_xs(&config, &mut m));
    }

    #[test]
    fn hovering_a_row_draws_a_wash_but_not_over_the_focused_row() {
        let config = Config::default();
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(1.0);
        let mut settings = Settings::new();
        settings.move_selection(1); // Font size
        let row_wash = hover_fill(&theme, card_fill(&theme));
        let page_wash = hover_fill(&theme, page_fill(&theme));
        let paint = settings.build(panel(), 1.0, &config, &[], &theme, &mut m);
        assert!(!has_fill(&paint, row_wash) && !has_fill(&paint, page_wash));
        settings.hover = Some(Target::Row(1)); // already focused: no wash
        let paint = settings.build(panel(), 1.0, &config, &[], &theme, &mut m);
        assert!(!has_fill(&paint, row_wash));
        settings.hover = Some(Target::Toggle(3));
        let paint = settings.build(panel(), 1.0, &config, &[], &theme, &mut m);
        let washes: Vec<_> = paint.quads.iter().filter(|q| q.color == row_wash).collect();
        let row = paint
            .targets
            .iter()
            .find(|(_, t)| *t == Target::Row(3))
            .unwrap()
            .0;
        assert_eq!(washes.len(), 1);
        assert!(washes[0].rect.y > row.y && washes[0].rect.y + washes[0].rect.h < row.y + row.h);
        for hover in [Target::Category(2), Target::Close] {
            settings.hover = Some(hover);
            let paint = settings.build(panel(), 1.0, &config, &[], &theme, &mut m);
            let washes = paint.quads.iter().filter(|q| q.color == page_wash).count();
            assert_eq!(washes, 1, "{hover:?}");
        }
    }

    #[test]
    fn the_workspaces_page_lists_rows_and_keeps_config_controls_out() {
        let config = Config::default();
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(1.0);
        let entries = vec![
            crate::workspace::Entry {
                name: "Personal".into(),
                icon: crate::workspace::ICONS[0],
                tabs: 2,
                active: true,
                groups: Vec::new(),
            },
            crate::workspace::Entry {
                name: "Work".into(),
                icon: crate::workspace::ICONS[9],
                tabs: 1,
                active: false,
                groups: Vec::new(),
            },
        ];
        let mut settings = Settings::new();
        settings.open_workspaces(1, Some("Work".into()));
        assert!(settings.on_workspaces_page());
        assert_eq!(settings.editing_index(), Some(1));
        assert!(settings.controls().is_empty(), "no config control here");
        let paint = settings.build(panel(), 1.0, &config, &entries, &theme, &mut m);
        assert!(paint.labels.iter().any(|l| l.text == "Workspaces"));
        assert!(paint.labels.iter().any(|l| l.text == "Work\u{258f}"));
        assert!(paint
            .targets
            .iter()
            .any(|(_, t)| *t == Target::Workspaces(crate::workspace::Target::Add)));
        // Rows clamp; a category click drops the open rename.
        settings.move_row(10, 3);
        assert_eq!(settings.selected_row(), 2);
        settings.select_category(0);
        assert!(settings.editing.is_none());
        assert!(!settings.on_workspaces_page());
    }

    #[test]
    fn pane_setup_opens_voice_without_changing_config() {
        let mut settings = Settings::new();
        settings.editing = Some("unfinished setting".into());
        settings.open_voice();
        assert!(settings.open);
        assert!(settings.editing.is_none());
        assert_eq!(settings.controls()[settings.selected].key, "voice.enabled");
    }

    #[test]
    fn voice_text_edits_are_validated_and_cancellable() {
        let mut settings = Settings::new();
        settings.category = CATEGORIES.iter().position(|c| c.label == "Voice").unwrap();
        settings.selected = 3; // language
        let mut config = Config::default();
        assert!(settings.start_text_edit(&config));
        settings.select_text();
        settings.append_text("invalid-language");
        assert!(settings.commit_text(&mut config).is_err());
        assert_eq!(config.voice.language, "en");
        assert_eq!(settings.editing.as_deref(), Some("invalid-language"));
        settings.select_text();
        settings.append_text("fr");
        assert_eq!(
            settings.commit_text(&mut config).unwrap(),
            Some("voice.language")
        );
        assert_eq!(config.voice.language, "fr");
        settings.selected = 2; // model path
        settings.start_text_edit(&config);
        settings.append_text("/tmp/private model.bin");
        settings.close();
        assert!(config.voice.model_path.is_empty());
        assert!(settings.editing.is_none());
    }

    #[test]
    fn voice_settings_use_the_active_theme() {
        for theme_name in ["ossein-dark", "ossein-light"] {
            let mut settings = Settings::new();
            settings.category = CATEGORIES.iter().position(|c| c.label == "Voice").unwrap();
            settings.selected = 2;
            let config = Config::default();
            let theme = Theme::resolve(theme_name);
            let mut measure = TextMeasure::new(1.0);
            let paint = settings.build(
                PxRect {
                    x: 0.0,
                    y: 0.0,
                    w: 1000.0,
                    h: 680.0,
                },
                1.0,
                &config,
                &[],
                &theme,
                &mut measure,
            );
            assert!(paint
                .labels
                .iter()
                .any(|label| label.text == "Model path" && label.color == theme.fg_primary));
            assert!(paint
                .labels
                .iter()
                .any(|label| label.text.contains("(not set)") && label.color == theme.fg_muted));
            assert!(paint
                .labels
                .iter()
                .any(|label| label.text == "voice.model_path" && label.color == theme.fg_muted));
        }
    }

    #[test]
    #[ignore = "manual GPU capture: writes /tmp/skelly-settings-<page>-<theme>.png"]
    fn capture_settings_pages() {
        let entries = vec![
            crate::workspace::Entry {
                name: "Personal".into(),
                icon: crate::workspace::ICONS[0],
                tabs: 3,
                active: true,
                groups: vec![crate::workspace::GroupEntry {
                    name: "Review".into(),
                    tabs: 2,
                }],
            },
            crate::workspace::Entry {
                name: "Work".into(),
                icon: crate::workspace::ICONS[9],
                tabs: 1,
                active: false,
                groups: Vec::new(),
            },
        ];
        for theme_name in ["ossein-dark", "ossein-light"] {
            for page in 0..=CATEGORIES.len() {
                let mut settings = Settings::new();
                settings.category = page;
                // Appearance opens on its theme grid; the other pages focus their second row.
                settings.selected = usize::from(page != 0);
                let mut config = Config::default();
                config.voice.enabled = true;
                config.voice.model_path = "~/Models/whisper/ggml-base.en.bin".into();
                config.appearance.theme = theme_name.into();
                let theme = Theme::resolve(theme_name);
                let (w, h, scale) = (1100_u16, 760_u16, 1.0_f32);
                let mut measure = TextMeasure::new(scale);
                let panel = PxRect {
                    x: 0.0,
                    y: 0.0,
                    w: f32::from(w),
                    h: f32::from(h),
                };
                let paint = settings.build(panel, scale, &config, &entries, &theme, &mut measure);
                let view = skelly_render::CaptureSettings {
                    panel,
                    nav_divider_x: paint.nav_divider_x,
                    quads: paint.quads,
                    labels: paint.labels,
                };
                let rgba = skelly_render::capture_settings_rgba(
                    &config.appearance,
                    u32::from(w),
                    u32::from(h),
                    f64::from(scale),
                    &view,
                );
                let name = super::nav_entries()
                    .nth(page)
                    .map_or("page", |(_, label)| label)
                    .split_whitespace()
                    .next()
                    .unwrap_or("page")
                    .to_ascii_lowercase();
                let file =
                    std::fs::File::create(format!("/tmp/skelly-settings-{name}-{theme_name}.png"))
                        .unwrap();
                let mut encoder =
                    png::Encoder::new(std::io::BufWriter::new(file), u32::from(w), u32::from(h));
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&rgba)
                    .unwrap();
            }
        }
    }

    #[test]
    fn every_control_round_trips_exactly_one_config_key() {
        // AGENTS Hard rule 1, enforced: changing any single control must alter exactly
        // one `config.toml` leaf, and it must be the key the control declares. We diff
        // the serialized config before/after the change and assert the changed leaf set
        // is precisely `{control.key}`.
        for category in CATEGORIES {
            for control in category.controls {
                let mut config = Config::default();
                let before: toml::Value =
                    toml::from_str(&config.to_toml_string().unwrap()).unwrap();

                if let Kind::Text { set, .. } = control.kind {
                    let value = match control.key {
                        "voice.language" => "fr",
                        "voice.dictation_shortcut" => "alt+d",
                        "voice.turn_shortcut" => "alt+v",
                        _ => "/tmp/local-voice-fixture",
                    };
                    set(&mut config, value.into());
                } else {
                    control.kind.adjust(&mut config, 1);
                }
                config
                    .validate()
                    .expect("every control writes a valid config");
                if config == Config::default() {
                    // A no-op nudge (toggle already on, range at its max): go the other
                    // way so the control actually changes.
                    control.kind.adjust(&mut config, -1);
                }
                let after: toml::Value = toml::from_str(&config.to_toml_string().unwrap()).unwrap();

                let mut changed = Vec::new();
                diff_paths(&before, &after, "", &mut changed);
                assert_eq!(
                    changed,
                    vec![control.key.to_owned()],
                    "control {:?} must change exactly its own key",
                    control.label
                );
            }
        }
    }

    /// Collect the dotted paths of leaves that differ between two TOML trees.
    fn diff_paths(a: &toml::Value, b: &toml::Value, prefix: &str, out: &mut Vec<String>) {
        if let (toml::Value::Table(ta), toml::Value::Table(tb)) = (a, b) {
            let mut keys: Vec<&String> = ta.keys().chain(tb.keys()).collect();
            keys.sort_unstable();
            keys.dedup();
            for key in keys {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match (ta.get(key), tb.get(key)) {
                    (Some(va), Some(vb)) => diff_paths(va, vb, &path, out),
                    _ => out.push(path),
                }
            }
        } else if a != b {
            out.push(prefix.to_owned());
        }
    }

    #[test]
    fn every_control_declares_a_unique_existing_key() {
        // Guards against a copy-paste leaving two controls pointing at one key, and
        // (with the round-trip test) that each key is real. Keys are dotted TOML paths.
        let mut seen = std::collections::HashSet::new();
        let default = Config::default().to_toml_string().expect("serialize");
        for category in CATEGORIES {
            for control in category.controls {
                assert!(seen.insert(control.key), "duplicate key {}", control.key);
                let section = control.key.split('.').next().unwrap();
                assert!(
                    default.contains(&format!("[{section}]")),
                    "key {} names a section not in the config",
                    control.key
                );
            }
        }
    }

    // Test-only introspection helpers.
    impl Settings {
        fn category_label(&self) -> &'static str {
            super::nav_entries().nth(self.category).unwrap().1
        }
        fn selected_index(&self) -> usize {
            self.selected
        }
    }
}
