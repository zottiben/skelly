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
//! Keyboard- and mouse-driven: a left category nav, a right control list, `↑/↓` to move between
//! controls, `←/→` (and Enter) to change a value, or a click on any nav row or widget. Each
//! control renders its §09 widget - a toggle switch, a segmented control, a compact `‹ value ›`
//! cycler, or a slider (see [`push_control`]) - so the view matches the guide's §10.9 controls.
//! [`Settings::build`] records a [`Target`] for every clickable rect it draws, so a click lands
//! on exactly what is on screen. The theme picker cycles (it has too many / too-long options to
//! fit a segmented control; the guide's swatch *cards* remain a visual follow-up). Deferred: the
//! keybindings / advanced categories (they need the `[keys]` registry).
//!
//! The nav ends with the Workspaces page ([`crate::workspace`]): a manager over the session
//! layout rather than the config, so it sits outside [`CATEGORIES`] and the one-key contract.

use skelly_config::{Config, CursorStyle, DiffView, SidebarMode, TabTitle};
use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};

use crate::workspace;

/// Settings-view layout constants in **logical** px (multiplied by the DPI scale). Tuned to
/// the guide's settings screen: a left category nav strip and a right control panel.
const NAV_WIDTH: f32 = 210.0;
/// Content pad from the window edges (top / bottom / right), the guide's content pad.
const PAD: f32 = 20.0;
/// Left inset of a nav label (leaving room for the active accent bar).
const NAV_PAD: f32 = 16.0;
/// Left inset of the content column (label + control), from the nav/content divider.
const CONTENT_PAD: f32 = 24.0;
/// Header-row height (the category heading).
const HEADER_H: f32 = 44.0;
/// Category nav row height.
const NAV_ROW_H: f32 = 32.0;
/// Control row height (tall enough to seat the §09 widgets).
const CTRL_ROW_H: f32 = 40.0;
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
/// The footer hint line.
const FOOTER: &str = "up/down move   left/right change   enter edit   tab category   esc close";
/// The footer hint line on the Workspaces page.
const WORKSPACES_FOOTER: &str =
    "up/down move   left/right icon   enter rename   delete remove   tab category   esc close";
/// The Workspaces page's nav entry, after every config category.
const WORKSPACES_NAV: (char, &str) = ('*', "Workspaces");

/// One editable control. It maps to exactly one `config.toml` key (Hard rule 1):
/// `key` is the dotted TOML path (also what the round-trip test asserts against), and
/// [`Kind`] carries the getter/setter that read and write only that key.
struct Control {
    /// The human label, shown left in the content column.
    label: &'static str,
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
        /// Render as a compact `‹ value ›` cycler rather than an all-options segmented control -
        /// for choices with many / long options (the theme picker) that would otherwise overflow
        /// the row. `←/→` still cycles either way.
        cycle: bool,
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
    /// A quiet leading glyph (kept ASCII-safe so any monospace font renders it).
    icon: char,
    /// The category name, shown in the nav and as the content heading.
    label: &'static str,
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
        icon: '#',
        label: "Appearance",
        controls: &[
            Control {
                label: "Theme",
                key: "appearance.theme",
                kind: Kind::Choice {
                    options: &["Ossein Dark", "Ossein Light", "Tokyo Night", "Kanagawa"],
                    get: |c| match c.appearance.theme.as_str() {
                        "ossein-light" => 1,
                        "tokyonight" => 2,
                        "kanagawa" => 3,
                        _ => 0,
                    },
                    set: |c, i| {
                        let name = match i {
                            1 => "ossein-light",
                            2 => "tokyonight",
                            3 => "kanagawa",
                            _ => "ossein-dark",
                        };
                        name.clone_into(&mut c.appearance.theme);
                    },
                    // Many, long options - render compactly so it never overflows the row.
                    cycle: true,
                },
            },
            Control {
                label: "Font size",
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
                label: "Cursor style",
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
                    cycle: false,
                },
            },
            Control {
                label: "Font ligatures",
                key: "appearance.ligatures",
                kind: Kind::Toggle {
                    get: |c| c.appearance.ligatures,
                    set: |c, v| c.appearance.ligatures = v,
                },
            },
            Control {
                label: "Bold uses bright colors",
                key: "appearance.bold_is_bright",
                kind: Kind::Toggle {
                    get: |c| c.appearance.bold_is_bright,
                    set: |c, v| c.appearance.bold_is_bright = v,
                },
            },
            Control {
                label: "Background blur",
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
            Control {
                label: "Window opacity",
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
                label: "Show pane status line",
                key: "appearance.show_status_line",
                kind: Kind::Toggle {
                    get: |c| c.appearance.show_status_line,
                    set: |c, v| c.appearance.show_status_line = v,
                },
            },
        ],
    },
    Category {
        icon: '=',
        label: "Sidebar",
        controls: &[
            Control {
                label: "Mode",
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
                    cycle: false,
                },
            },
            Control {
                label: "Width",
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
                key: "sidebar.show_pinned",
                kind: Kind::Toggle {
                    get: |c| c.sidebar.show_pinned,
                    set: |c, v| c.sidebar.show_pinned = v,
                },
            },
        ],
    },
    Category {
        icon: '+',
        label: "Tabs",
        controls: &[
            Control {
                label: "Title source",
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
                    cycle: false,
                },
            },
            Control {
                label: "Follow directory",
                key: "tabs.follow_cwd",
                kind: Kind::Toggle {
                    get: |c| c.tabs.follow_cwd,
                    set: |c, v| c.tabs.follow_cwd = v,
                },
            },
        ],
    },
    Category {
        icon: '|',
        label: "Panes",
        controls: &[Control {
            label: "New panes inherit cwd",
            key: "panes.split_inherits_cwd",
            kind: Kind::Toggle {
                get: |c| c.panes.split_inherits_cwd,
                set: |c, v| c.panes.split_inherits_cwd = v,
            },
        }],
    },
    Category {
        icon: '\u{276f}',
        label: "Shell & env",
        controls: &[Control {
            label: "Shell",
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
                cycle: false,
            },
        }],
    },
    Category {
        icon: '@',
        label: "Session",
        controls: &[
            Control {
                label: "Record timeline",
                key: "session.timeline",
                kind: Kind::Toggle {
                    get: |c| c.session.timeline,
                    set: |c, v| c.session.timeline = v,
                },
            },
            Control {
                label: "Persist layout",
                key: "session.persist",
                kind: Kind::Toggle {
                    get: |c| c.session.persist,
                    set: |c, v| c.session.persist = v,
                },
            },
            Control {
                label: "Shadow worktree",
                key: "session.shadow_worktree",
                kind: Kind::Toggle {
                    get: |c| c.session.shadow_worktree,
                    set: |c, v| c.session.shadow_worktree = v,
                },
            },
        ],
    },
    Category {
        icon: '%',
        label: "Git",
        controls: &[Control {
            label: "Diff layout",
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
                cycle: false,
            },
        }],
    },
    Category {
        icon: 'v',
        label: "Voice",
        controls: &[
            Control {
                label: "Pi voice bridge (new panes)",
                key: "voice.enabled",
                kind: Kind::Toggle {
                    get: |c| c.voice.enabled,
                    set: |c, v| c.voice.enabled = v,
                },
            },
            Control {
                label: "Whisper executable",
                key: "voice.whisper_program",
                kind: Kind::Text {
                    get: |c| &c.voice.whisper_program,
                    set: |c, v| c.voice.whisper_program = v,
                },
            },
            Control {
                label: "Model path",
                key: "voice.model_path",
                kind: Kind::Text {
                    get: |c| &c.voice.model_path,
                    set: |c, v| c.voice.model_path = v,
                },
            },
            Control {
                label: "Language (code or auto)",
                key: "voice.language",
                kind: Kind::Text {
                    get: |c| &c.voice.language,
                    set: |c, v| c.voice.language = v,
                },
            },
            Control {
                label: "Dictation shortcut",
                key: "voice.dictation_shortcut",
                kind: Kind::Text {
                    get: |c| &c.voice.dictation_shortcut,
                    set: |c, v| c.voice.dictation_shortcut = v,
                },
            },
            Control {
                label: "Recording limit",
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
        icon: 's',
        label: "Speech",
        controls: &[
            Control {
                label: "Spoken replies (voice mode)",
                key: "voice.spoken_replies",
                kind: Kind::Toggle {
                    get: |c| c.voice.spoken_replies,
                    set: |c, v| c.voice.spoken_replies = v,
                },
            },
            Control {
                label: "Record/send shortcut",
                key: "voice.turn_shortcut",
                kind: Kind::Text {
                    get: |c| &c.voice.turn_shortcut,
                    set: |c, v| c.voice.turn_shortcut = v,
                },
            },
            Control {
                label: "Busy Pi delivery",
                key: "voice.busy_delivery",
                kind: Kind::Choice {
                    options: &["Refuse", "Steer", "Follow up"],
                    cycle: false,
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
            Control {
                label: "Speech voice (optional)",
                key: "voice.speech_voice",
                kind: Kind::Text {
                    get: |c| &c.voice.speech_voice,
                    set: |c, v| c.voice.speech_voice = v,
                },
            },
            Control {
                label: "Speech rate",
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

/// The rendered settings grid plus the highlight rows, for a
/// [`skelly_render::SettingsView`].
pub(crate) struct Paint {
    /// The x of the nav/content divider (physical px); the nav strip fills to its left.
    pub(crate) nav_divider_x: f32,
    /// The content quads over the frame (active-category fill + bar, focused-control fill).
    pub(crate) quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub(crate) labels: Vec<ProseLabel>,
    /// Every clickable rect drawn this frame (physical px), in draw order: a widget's own
    /// target follows its row's, so [`target_at`] (which searches last-first) prefers it.
    pub(crate) targets: Vec<(PxRect, Target)>,
    /// The Workspaces page's first drawn row, to keep as its scroll ([`Settings::settle_rows`]).
    pub(crate) rows_first: usize,
}

/// What a click on the settings view lands on. Indices address the active category's
/// controls (or the nav's categories).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Target {
    /// The `esc to close` hint: close the view.
    Close,
    /// A nav row: switch to this category.
    Category(usize),
    /// A control row outside its widget: focus the control.
    Row(usize),
    /// A toggle switch: flip it.
    Toggle(usize),
    /// One segment of a segmented control: choose that option.
    Segment {
        /// The control's index.
        index: usize,
        /// The option's index.
        option: usize,
    },
    /// One half of a `‹ value ›` cycler: step back (`-1`) or forward (`1`), wrapping.
    Cycle {
        /// The control's index.
        index: usize,
        /// The step direction.
        delta: i32,
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
            Target::Segment { index, .. }
            | Target::Cycle { index, .. }
            | Target::Slider { index, .. } => Some(index),
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
    /// The Workspaces page's scroll: its first drawn row.
    rows_first: usize,
    /// Whether the next paint scrolls the focused Workspaces-page row into view.
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

    /// Scroll the Workspaces-page rows by wheel `lines` (positive = up), leaving focus put.
    pub(crate) fn scroll_rows(&mut self, lines: i32) {
        let by = usize::try_from(lines.unsigned_abs()).unwrap_or(usize::MAX);
        self.rows_first = if lines > 0 {
            self.rows_first.saturating_sub(by)
        } else {
            self.rows_first.saturating_add(by)
        };
        self.reveal_row = false;
    }

    /// Keep the Workspaces-page scroll the last paint settled on (see [`Paint::rows_first`]).
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

    /// Choose option `option` of the choice control at `index` (a segment click), returning
    /// the config key it wrote.
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

    /// Step the choice control at `index` by `delta`, wrapping (a cycler click), returning the
    /// config key it wrote.
    pub(crate) fn step(
        &mut self,
        config: &mut Config,
        index: usize,
        delta: i32,
    ) -> Option<&'static str> {
        self.select(index);
        let control = self.controls().get(index)?;
        let Kind::Choice {
            options, get, set, ..
        } = &control.kind
        else {
            return None;
        };
        let n = i32::try_from(options.len().max(1)).unwrap_or(1);
        let cur = i32::try_from(get(config)).unwrap_or(0);
        set(
            config,
            usize::try_from((cur + delta).rem_euclid(n)).unwrap_or(0),
        );
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

    /// Build the settings view's proportional display list within `panel` (physical px) at
    /// DPI `scale`, reading current values from `config` in `theme`'s UI tokens: the category
    /// heading, the left category nav (active one filled + barred), the control rows for the
    /// active category (label + value, the focused one filled with a guillemet-bracketed
    /// accent value), and the footer. The renderer draws the panel + nav strip + divider.
    pub(crate) fn build(
        &self,
        panel: PxRect,
        scale: f32,
        config: &Config,
        workspaces: &[workspace::Entry],
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> Paint {
        let nav_divider_x = panel.x + NAV_WIDTH * scale;
        let content_x = nav_divider_x + CONTENT_PAD * scale;
        let content_right = panel.x + panel.w - PAD * scale;
        let mut quads = Vec::new();
        let mut labels = Vec::new();
        let mut targets = Vec::new();

        let hy = panel.y + PAD * scale;
        self.push_header(
            &mut labels,
            &mut targets,
            measure,
            (content_x, content_right),
            hy,
            scale,
            theme,
        );
        let body_top = hy + HEADER_H * scale;
        self.push_nav(
            &mut quads,
            &mut labels,
            &mut targets,
            measure,
            panel,
            nav_divider_x,
            body_top,
            scale,
            theme,
        );
        // The footer sits at the panel's foot; the Workspaces rows scroll above it.
        let fy = panel.y + panel.h - PAD * scale - FontRole::Caption.line_height_px(scale);
        let mut rows_first = 0;
        if self.on_workspaces_page() {
            rows_first = self.push_workspaces(
                &mut quads,
                &mut labels,
                &mut targets,
                measure,
                workspaces,
                (content_x, content_right, nav_divider_x),
                (body_top, fy - PAD * scale),
                scale,
                theme,
            );
        } else {
            self.push_controls(
                &mut quads,
                &mut labels,
                &mut targets,
                measure,
                config,
                (content_x, content_right, nav_divider_x),
                body_top,
                scale,
                theme,
            );
        }
        push_row(
            &mut labels,
            measure,
            if self.on_workspaces_page() {
                WORKSPACES_FOOTER
            } else {
                FOOTER
            },
            FontRole::Caption,
            theme.fg_muted,
            content_x,
            fy,
            0.0,
            scale,
        );

        Paint {
            nav_divider_x,
            quads,
            labels,
            targets,
            rows_first,
        }
    }

    /// Draw the header at `hy`: the active category name (heading) + an esc hint, which is
    /// also the close button unless a text edit is open. `x = (content_x, content_right)`.
    #[allow(clippy::too_many_arguments, reason = "one focused header builder")]
    fn push_header(
        &self,
        labels: &mut Vec<ProseLabel>,
        targets: &mut Vec<(PxRect, Target)>,
        measure: &mut TextMeasure,
        x: (f32, f32),
        hy: f32,
        scale: f32,
        theme: &Theme,
    ) {
        let (content_x, content_right) = x;
        push_row(
            labels,
            measure,
            nav_entries()
                .nth(self.category)
                .map_or(WORKSPACES_NAV.1, |(_, label)| label),
            FontRole::H2,
            theme.fg_primary,
            content_x,
            hy,
            HEADER_H,
            scale,
        );
        let editing = self.editing.is_some();
        let hint = if editing {
            "enter save · esc cancel"
        } else {
            "esc to close"
        };
        push_right(
            labels,
            measure,
            hint,
            FontRole::Caption,
            theme.fg_muted,
            content_right,
            hy,
            HEADER_H,
            scale,
        );
        if !editing {
            let w = measure.width(hint, FontRole::Caption, None);
            targets.push((
                PxRect {
                    x: content_right - w,
                    y: hy,
                    w,
                    h: HEADER_H * scale,
                },
                Target::Close,
            ));
        }
    }

    /// Draw the Workspaces page: the session's workspaces and groups, focused + hovered from
    /// this view's state. `x = (content_x, content_right, nav_divider_x)`, `y = (top, bottom)`;
    /// returns the first row drawn (its scroll).
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors push_controls, the other content-page builder"
    )]
    fn push_workspaces(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        targets: &mut Vec<(PxRect, Target)>,
        measure: &mut TextMeasure,
        entries: &[workspace::Entry],
        x: (f32, f32, f32),
        y: (f32, f32),
        scale: f32,
        theme: &Theme,
    ) -> usize {
        let (content_x, content_right, nav_divider_x) = x;
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
            x: content_x,
            right: content_right,
            band_x: nav_divider_x,
            band_right: content_right + PAD * scale,
            top: y.0,
            bottom: y.1,
        };
        let mut out = workspace::Out {
            quads,
            labels,
            targets: Vec::new(),
        };
        let first = page.build(&area, scale, theme, measure, &mut out);
        targets.extend(
            out.targets
                .into_iter()
                .map(|(rect, t)| (rect, Target::Workspaces(t))),
        );
        first
    }

    /// Draw the left category nav: each category's icon + label, the active one behind an
    /// `accent.subtle` fill + `accent` bar, a hovered one behind a lighter wash.
    #[allow(clippy::too_many_arguments, reason = "one focused nav builder")]
    fn push_nav(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        targets: &mut Vec<(PxRect, Target)>,
        measure: &mut TextMeasure,
        panel: PxRect,
        nav_divider_x: f32,
        body_top: f32,
        scale: f32,
        theme: &Theme,
    ) {
        let mut y = body_top;
        for (index, (icon, name)) in nav_entries().enumerate() {
            let active = index == self.category;
            let row = PxRect {
                x: panel.x,
                y,
                w: nav_divider_x - panel.x,
                h: NAV_ROW_H * scale,
            };
            targets.push((row, Target::Category(index)));
            if !active && self.hover == Some(Target::Category(index)) {
                quads.push(ChromeQuad::fill(
                    row,
                    hover_fill(theme, theme.bg_base.to_srgb()),
                ));
            }
            if active {
                let h = NAV_ROW_H * scale;
                // accent.subtle active-row fill, composited in sRGB over the nav column
                // (bg.base) so it reads at the guide's weight.
                quads.push(ChromeQuad::fill(
                    PxRect {
                        x: panel.x,
                        y,
                        w: nav_divider_x - panel.x,
                        h,
                    },
                    theme.accent_subtle_on(theme.bg_base.to_srgb()),
                ));
                quads.push(ChromeQuad::fill(
                    PxRect {
                        x: panel.x,
                        y,
                        w: (2.0 * scale).max(1.0),
                        h,
                    },
                    theme.accent,
                ));
            }
            let color = if active {
                theme.fg_primary
            } else {
                theme.fg_secondary
            };
            let label = format!("{icon} {name}");
            push_row(
                labels,
                measure,
                &label,
                FontRole::Label,
                color,
                panel.x + NAV_PAD * scale,
                y,
                NAV_ROW_H,
                scale,
            );
            y += NAV_ROW_H * scale;
        }
    }

    /// Draw the active category's control rows: each label + value, the focused one behind an
    /// `accent.subtle` fill. `x = (content_x, content_right, nav_divider_x)`.
    #[allow(
        clippy::too_many_arguments,
        reason = "one focused control-list builder"
    )]
    fn push_controls(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        targets: &mut Vec<(PxRect, Target)>,
        measure: &mut TextMeasure,
        config: &Config,
        x: (f32, f32, f32),
        body_top: f32,
        scale: f32,
        theme: &Theme,
    ) {
        let (content_x, content_right, nav_divider_x) = x;
        let mut y = body_top;
        let hovered = self.hover.and_then(Target::control);
        // One readout column for every slider in the category, sized to the widest value any
        // of them can show, so the tracks line up and never shift while a value changes.
        let readout_w = self
            .controls()
            .iter()
            .filter_map(|c| c.kind.readout_width(measure))
            .fold(0.0, f32::max);
        for (index, control) in self.controls().iter().enumerate() {
            let focused = index == self.selected;
            let row = PxRect {
                x: nav_divider_x,
                y,
                w: content_right + PAD * scale - nav_divider_x,
                h: CTRL_ROW_H * scale,
            };
            targets.push((row, Target::Row(index)));
            if focused {
                // accent.subtle selected-row band, composited in sRGB over the content surface
                // (bg.elevated) to match the guide's weight.
                quads.push(ChromeQuad::fill(
                    row,
                    theme.accent_subtle_on(theme.bg_elevated),
                ));
            } else if hovered == Some(index) {
                quads.push(ChromeQuad::fill(row, hover_fill(theme, theme.bg_elevated)));
            }
            push_control(
                quads,
                labels,
                targets,
                index,
                measure,
                control,
                config,
                focused,
                self.editing.as_deref().filter(|_| focused),
                (content_x, content_right, readout_w),
                y,
                scale,
                theme,
            );
            y += CTRL_ROW_H * scale;
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

/// Every nav entry as `(icon, label)`: the config categories, then the Workspaces page.
fn nav_entries() -> impl Iterator<Item = (char, &'static str)> {
    CATEGORIES
        .iter()
        .map(|c| (c.icon, c.label))
        .chain(std::iter::once(WORKSPACES_NAV))
}

/// A lighter wash than the focused row's `accent.subtle` band, for the row under the pointer.
pub(crate) fn hover_fill(theme: &Theme, base: Srgb) -> Srgb {
    theme.accent.over(base, theme.accent_subtle_alpha * 0.45)
}

/// Push one control row: its `label` at the content inset and its value rendered as the §09
/// widget for its [`Kind`] - a toggle switch, a segmented control, or a slider - right-aligned
/// to `content_right`, recording the widget's click [`Target`]s. The focused row's fill (drawn
/// by the caller) is the focus cue.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    reason = "one focused control-row builder; range bounds are small config values (u8/u16)"
)]
fn push_control(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    targets: &mut Vec<(PxRect, Target)>,
    index: usize,
    measure: &mut TextMeasure,
    control: &Control,
    config: &Config,
    focused: bool,
    editing: Option<&str>,
    x: (f32, f32, f32),
    top: f32,
    scale: f32,
    theme: &Theme,
) {
    let (content_x, content_right, readout_w) = x;
    push_row(
        labels,
        measure,
        control.label,
        FontRole::Label,
        if focused {
            theme.fg_primary
        } else {
            theme.fg_secondary
        },
        content_x,
        top,
        CTRL_ROW_H,
        scale,
    );
    match &control.kind {
        Kind::Text { get, .. } => {
            let value = editing.unwrap_or_else(|| get(config));
            let value =
                if editing.is_none() && value.is_empty() && control.key == "voice.speech_voice" {
                    "(system default)"
                } else {
                    value
                };
            let display = text_value(value, editing.is_some());
            let left = content_x + (content_right - content_x) * 0.52;
            targets.push((
                PxRect {
                    x: left,
                    y: top,
                    w: content_right - left,
                    h: CTRL_ROW_H * scale,
                },
                Target::Text(index),
            ));
            push_row(
                labels,
                measure,
                &display,
                FontRole::Label,
                if focused {
                    theme.accent
                } else {
                    theme.fg_secondary
                },
                left,
                top,
                CTRL_ROW_H,
                scale,
            );
            if let Some(label) = labels.last_mut() {
                label.max_w = (content_right - left).max(0.0);
            }
        }
        Kind::Toggle { get, .. } => {
            let rect = push_toggle(quads, get(config), content_right, top, scale, theme);
            targets.push((rect, Target::Toggle(index)));
        }
        Kind::Choice {
            options,
            get,
            cycle,
            ..
        } => {
            let selected = get(config).min(options.len().saturating_sub(1));
            if *cycle {
                let rect = push_cycle(
                    labels,
                    measure,
                    options.get(selected).copied().unwrap_or(""),
                    content_right,
                    top,
                    scale,
                    theme,
                    focused,
                );
                let half = rect.w * 0.5;
                for (x, delta) in [(rect.x, -1), (rect.x + half, 1)] {
                    targets.push((
                        PxRect {
                            x,
                            y: top,
                            w: half,
                            h: CTRL_ROW_H * scale,
                        },
                        Target::Cycle { index, delta },
                    ));
                }
            } else {
                let segments = push_segmented(
                    quads,
                    labels,
                    measure,
                    options,
                    selected,
                    content_right,
                    top,
                    scale,
                    theme,
                );
                targets.extend(
                    segments
                        .into_iter()
                        .enumerate()
                        .map(|(option, rect)| (rect, Target::Segment { index, option })),
                );
            }
        }
        Kind::Range { min, max, get, .. } => {
            let fraction = if max > min {
                (get(config) - min) as f32 / (max - min) as f32
            } else {
                0.0
            };
            let track = push_slider(
                quads,
                labels,
                measure,
                &control.kind.value(config),
                fraction.clamp(0.0, 1.0),
                content_right,
                readout_w,
                top,
                scale,
                theme,
            );
            // The grab area spans the row's height and half a knob past each end, so the
            // extremes are as easy to hit as the middle.
            let slop = SLIDER_KNOB * scale * 0.5;
            targets.push((
                PxRect {
                    x: track.x - slop,
                    y: top,
                    w: track.w + 2.0 * slop,
                    h: CTRL_ROW_H * scale,
                },
                Target::Slider {
                    index,
                    x: track.x,
                    w: track.w,
                },
            ));
        }
    }
}

/// Keep the end of a long path visible; the row clips to its value column.
fn text_value(value: &str, editing: bool) -> String {
    let value = if value.is_empty() { "(not set)" } else { value };
    let tail: String = value
        .chars()
        .rev()
        .take(36)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if editing {
        format!("{tail}▏")
    } else {
        format!("‹ {tail} ›")
    }
}

/// A §09 toggle switch, right-anchored: a 38x22 pill (`accent` on / `border` off) with an
/// 18px knob sprung to the right (on) or left (off). Returns the pill's rect.
fn push_toggle(
    quads: &mut Vec<ChromeQuad>,
    on: bool,
    content_right: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
) -> PxRect {
    let w = TOGGLE_W * scale;
    let h = TOGGLE_H * scale;
    let x = content_right - w;
    let y = top + (CTRL_ROW_H * scale - h) * 0.5;
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

/// A compact cycler, right-anchored: the current option in `‹ value ›` guillemets - for choices
/// with many / long options that would overflow a segmented control. `←/→` cycles the value.
/// Returns the label's rect (its left half steps back, its right half forward).
#[allow(clippy::too_many_arguments, reason = "one focused cycler builder")]
fn push_cycle(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    value: &str,
    content_right: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
    focused: bool,
) -> PxRect {
    let color = if focused {
        theme.accent
    } else {
        theme.fg_primary
    };
    let text = format!("\u{2039} {value} \u{203A}");
    let w = measure.width(&text, FontRole::Label, None);
    let line = measure.line_height(FontRole::Label);
    labels.push(ProseLabel {
        text,
        x: content_right - w,
        y: top + (CTRL_ROW_H * scale - line) * 0.5,
        role: FontRole::Label,
        color,
        weight: None,
        max_w: f32::MAX,
    });
    PxRect {
        x: content_right - w,
        y: top,
        w,
        h: CTRL_ROW_H * scale,
    }
}

/// A §09 segmented control, right-anchored: a `bg.inset` container (with a `border.subtle`
/// ring) holding one padded segment per option; the selected one gets a `bg.elevated` fill.
/// Returns each segment's rect, in option order.
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
    content_right: f32,
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
    let cx = content_right - total;
    let cy = top + (CTRL_ROW_H * scale - (seg_h + 2.0 * pad)) * 0.5;
    // Container.
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: cx,
            y: cy,
            w: total,
            h: seg_h + 2.0 * pad,
        },
        theme.bg_inset,
        SEG_RADIUS * scale,
    ));
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
    content_right: f32,
    readout_w: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
) -> PxRect {
    // The value readout (mono accent), right-aligned.
    let value_w = measure.width(value, FontRole::Mono, None);
    let column_w = readout_w.max(value_w);
    let vline = measure.line_height(FontRole::Mono);
    labels.push(ProseLabel {
        text: value.to_owned(),
        x: content_right - value_w,
        y: top + (CTRL_ROW_H * scale - vline) * 0.5,
        role: FontRole::Mono,
        color: theme.accent,
        weight: None,
        max_w: f32::MAX,
    });
    // The track, to the left of the value.
    let track_w = SLIDER_W * scale;
    let track_h = SLIDER_TRACK_H * scale;
    let track_x = content_right - column_w - SLIDER_VALUE_GAP * scale - track_w;
    let track_y = top + (CTRL_ROW_H * scale - track_h) * 0.5;
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
            y: top + (CTRL_ROW_H * scale - knob) * 0.5,
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

/// Push one label vertically centered in a row of `row_h` logical px whose top is physical
/// `top` (`row_h = 0` places the label's top at `top`).
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
    use super::{target_at, Kind, Settings, Target, CATEGORIES};
    use skelly_config::Config;
    use skelly_render::{PxRect, TextMeasure, Theme};

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
        let mut m = TextMeasure::new(2.0);
        let mut settings = Settings::new();
        settings.move_selection(2); // third control of Appearance
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w: 1200.0,
            h: 800.0,
        };
        let paint = settings.build(panel, 2.0, &config, &[], &theme, &mut m);
        // The active category (Appearance) heading + the focused control value draw in accent;
        // the active-category fill/bar + focused-control fill are among the quads.
        assert!(paint.labels.iter().any(|l| l.text == "Appearance"));
        assert!(paint.labels.iter().any(|l| l.color == theme.accent));
        // >= 3 quads: the active-category subtle fill + accent bar + the focused-control fill.
        assert!(paint.quads.len() >= 3);
        assert!(paint.nav_divider_x > panel.x);
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
        let mut m = TextMeasure::new(2.0);
        let settings = Settings::new();
        let paint = settings.build(panel(), 2.0, &config, &[], &theme, &mut m);
        let t = &paint.targets;
        for i in 0..CATEGORIES.len() {
            let (x, y) = center_of(t, |t| *t == Target::Category(i));
            assert_eq!(target_at(t, x, y), Some(Target::Category(i)));
        }
        // A widget wins over its row; the row's label side still focuses the row.
        let (x, y) = center_of(t, |t| matches!(t, Target::Toggle(_)));
        assert!(matches!(target_at(t, x, y), Some(Target::Toggle(_))));
        let (_, y) = center_of(t, |t| *t == Target::Row(0));
        assert_eq!(target_at(t, 300.0 * 2.0, y), Some(Target::Row(0)));
        // The theme cycler splits into a back half and a forward half.
        let (x, y) = center_of(t, |t| matches!(t, Target::Cycle { delta: 1, .. }));
        assert_eq!(
            target_at(t, x, y),
            Some(Target::Cycle { index: 0, delta: 1 })
        );
        assert_eq!(target_at(t, -10.0, -10.0), None);
        assert!(t.iter().any(|(_, t)| *t == Target::Close));
    }

    #[test]
    fn segment_cycle_and_slider_clicks_write_their_one_key() {
        let mut config = Config::default();
        let mut settings = Settings::new();
        // Cursor style is a segmented choice (control 3 of Appearance).
        assert_eq!(
            settings.choose(&mut config, 3, 2),
            Some("appearance.cursor")
        );
        assert_eq!(
            config.appearance.cursor,
            skelly_config::CursorStyle::Underline
        );
        assert_eq!(settings.selected_index(), 3);
        // The theme cycler wraps backwards from the first option.
        assert_eq!(settings.step(&mut config, 0, -1), Some("appearance.theme"));
        assert_eq!(config.appearance.theme, "kanagawa");
        assert_eq!(settings.step(&mut config, 0, 1), Some("appearance.theme"));
        assert_eq!(config.appearance.theme, "ossein-dark");
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
        let plain = settings
            .build(panel(), 1.0, &config, &[], &theme, &mut m)
            .quads
            .len();
        settings.hover = Some(Target::Row(0)); // already focused: no extra wash
        assert_eq!(
            settings
                .build(panel(), 1.0, &config, &[], &theme, &mut m)
                .quads
                .len(),
            plain
        );
        settings.hover = Some(Target::Toggle(4));
        assert_eq!(
            settings
                .build(panel(), 1.0, &config, &[], &theme, &mut m)
                .quads
                .len(),
            plain + 1
        );
        settings.hover = Some(Target::Category(2));
        assert_eq!(
            settings
                .build(panel(), 1.0, &config, &[], &theme, &mut m)
                .quads
                .len(),
            plain + 1
        );
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
                .any(|label| label.text.contains("(not set)") && label.color == theme.accent));
        }
    }

    #[test]
    #[ignore = "manual GPU capture: writes /tmp/skelly-{voice,speech}-settings-<theme>.png"]
    fn capture_voice_settings() {
        for category in ["Voice", "Speech"] {
            for theme_name in ["ossein-dark", "ossein-light"] {
                let mut settings = Settings::new();
                settings.category = CATEGORIES.iter().position(|c| c.label == category).unwrap();
                settings.selected = 2;
                let mut config = Config::default();
                config.voice.enabled = true;
                config.voice.model_path = "~/Models/whisper/ggml-base.en.bin".into();
                config.appearance.theme = theme_name.into();
                let theme = Theme::resolve(theme_name);
                let mut measure = TextMeasure::new(1.0);
                let panel = PxRect {
                    x: 0.0,
                    y: 0.0,
                    w: 1000.0,
                    h: 680.0,
                };
                let paint = settings.build(panel, 1.0, &config, &[], &theme, &mut measure);
                let view = skelly_render::CaptureSettings {
                    panel,
                    nav_divider_x: paint.nav_divider_x,
                    quads: paint.quads,
                    labels: paint.labels,
                };
                let rgba =
                    skelly_render::capture_settings_rgba(&config.appearance, 1000, 680, 1.0, &view);
                let file = std::fs::File::create(format!(
                    "/tmp/skelly-{}-settings-{theme_name}.png",
                    category.to_ascii_lowercase()
                ))
                .unwrap();
                let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 1000, 680);
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
