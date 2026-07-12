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
//! This first slice is keyboard-driven: a left category nav, a right control list,
//! `↑/↓` to move between controls, `←/→` (and Enter) to change a value. The mockup's
//! richer widgets (theme cards, sliders) are represented textually. Deferred: the
//! keybindings / shell / advanced categories (they need config keys or the `[keys]`
//! registry we do not have yet) and mouse hit-testing.

use skelly_config::{Config, CursorStyle, DiffView, SidebarMode, TabTitle};
use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};

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
/// Control row height.
const CTRL_ROW_H: f32 = 34.0;
/// The footer hint line.
const FOOTER: &str = "up/down move   left/right change   tab category   esc close";

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
            } => {
                let v = get(c);
                if *divisor <= 1 {
                    format!("{v}{suffix}")
                } else {
                    let decimals = decimals_for(*divisor);
                    format!(
                        "{:.*}{}",
                        decimals,
                        f64::from(v) / f64::from(*divisor),
                        suffix
                    )
                }
            }
        }
    }

    /// Change the value by a signed `←/→` nudge, staying within bounds.
    fn adjust(&self, c: &mut Config, delta: i32) {
        match self {
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
                    options: &["Ossein Dark", "Ossein Light"],
                    get: |c| usize::from(c.appearance.theme == "ossein-light"),
                    set: |c, i| {
                        let name = if i == 1 {
                            "ossein-light"
                        } else {
                            "ossein-dark"
                        };
                        name.clone_into(&mut c.appearance.theme);
                    },
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
                },
            },
            Control {
                label: "Width",
                key: "sidebar.width",
                kind: Kind::Range {
                    min: 56,
                    max: 360,
                    step: 8,
                    divisor: 1,
                    suffix: "px",
                    get: |c| i32::from(c.sidebar.width),
                    set: |c, v| c.sidebar.width = v.clamp(56, 360) as u16,
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
        controls: &[
            Control {
                label: "Max panes",
                key: "panes.max",
                kind: Kind::Range {
                    min: 1,
                    max: 8,
                    step: 1,
                    divisor: 1,
                    suffix: "",
                    get: |c| i32::from(c.panes.max),
                    set: |c, v| c.panes.max = v.clamp(1, 8) as u8,
                },
            },
            Control {
                label: "Splits inherit cwd",
                key: "panes.split_inherits_cwd",
                kind: Kind::Toggle {
                    get: |c| c.panes.split_inherits_cwd,
                    set: |c, v| c.panes.split_inherits_cwd = v,
                },
            },
        ],
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
            },
        }],
    },
];

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
}

/// Settings-view state: whether it is open, the active category, and the focused
/// control within that category.
pub(crate) struct Settings {
    /// Whether the settings view is showing (captures all input while open).
    pub(crate) open: bool,
    category: usize,
    selected: usize,
}

impl Settings {
    /// A closed settings view, at the first category and control.
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            category: 0,
            selected: 0,
        }
    }

    /// Open the settings view fresh (first category, first control).
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.category = 0;
        self.selected = 0;
    }

    /// Close the settings view.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// The controls of the active category.
    fn controls(&self) -> &'static [Control] {
        CATEGORIES[self.category.min(CATEGORIES.len() - 1)].controls
    }

    /// Switch category by `delta` (wrapping), resetting the control selection.
    pub(crate) fn cycle_category(&mut self, forward: bool) {
        let n = CATEGORIES.len();
        self.category = if forward {
            (self.category + 1) % n
        } else {
            (self.category + n - 1) % n
        };
        self.selected = 0;
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
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> Paint {
        let nav_divider_x = panel.x + NAV_WIDTH * scale;
        let content_x = nav_divider_x + CONTENT_PAD * scale;
        let content_right = panel.x + panel.w - PAD * scale;
        let mut quads = Vec::new();
        let mut labels = Vec::new();

        // Header: the active category name (heading) + an esc hint.
        let hy = panel.y + PAD * scale;
        push_row(
            &mut labels,
            measure,
            CATEGORIES[self.category].label,
            FontRole::H2,
            theme.fg_primary,
            content_x,
            hy,
            HEADER_H,
            scale,
        );
        push_right(
            &mut labels,
            measure,
            "esc to close",
            FontRole::Caption,
            theme.fg_muted,
            content_right,
            hy,
            HEADER_H,
            scale,
        );

        let body_top = hy + HEADER_H * scale;
        self.push_nav(
            &mut quads,
            &mut labels,
            measure,
            panel,
            nav_divider_x,
            body_top,
            scale,
            theme,
        );
        self.push_controls(
            &mut quads,
            &mut labels,
            measure,
            config,
            (content_x, content_right, nav_divider_x),
            body_top,
            scale,
            theme,
        );

        // Footer.
        let fy = panel.y + panel.h - PAD * scale - FontRole::Caption.line_height_px(scale);
        push_row(
            &mut labels,
            measure,
            FOOTER,
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
        }
    }

    /// Draw the left category nav: each category's icon + label, the active one behind an
    /// `accent.subtle` fill + `accent` bar.
    #[allow(clippy::too_many_arguments, reason = "one focused nav builder")]
    fn push_nav(
        &self,
        quads: &mut Vec<ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        measure: &mut TextMeasure,
        panel: PxRect,
        nav_divider_x: f32,
        body_top: f32,
        scale: f32,
        theme: &Theme,
    ) {
        let mut y = body_top;
        for (index, category) in CATEGORIES.iter().enumerate() {
            let active = index == self.category;
            if active {
                let h = NAV_ROW_H * scale;
                quads.push(ChromeQuad::tint(
                    PxRect {
                        x: panel.x,
                        y,
                        w: nav_divider_x - panel.x,
                        h,
                    },
                    theme.accent,
                    0.14,
                    0.0,
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
            let label = format!("{} {}", category.icon, category.label);
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
        measure: &mut TextMeasure,
        config: &Config,
        x: (f32, f32, f32),
        body_top: f32,
        scale: f32,
        theme: &Theme,
    ) {
        let (content_x, content_right, nav_divider_x) = x;
        let mut y = body_top;
        for (index, control) in self.controls().iter().enumerate() {
            let focused = index == self.selected;
            if focused {
                quads.push(ChromeQuad::tint(
                    PxRect {
                        x: nav_divider_x,
                        y,
                        w: content_right + PAD * scale - nav_divider_x,
                        h: CTRL_ROW_H * scale,
                    },
                    theme.accent,
                    0.14,
                    0.0,
                ));
            }
            push_control(
                labels,
                measure,
                control,
                config,
                focused,
                content_x,
                content_right,
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

/// Push one control row: its `label` at the content inset and its value right-anchored. The
/// focused control brackets the value in guillemets and paints it in `accent`; otherwise the
/// value is a quiet secondary.
#[allow(clippy::too_many_arguments, reason = "one focused control-row builder")]
fn push_control(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    control: &Control,
    config: &Config,
    focused: bool,
    content_x: f32,
    content_right: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
) {
    let label_fg = if focused {
        theme.fg_primary
    } else {
        theme.fg_secondary
    };
    push_row(
        labels,
        measure,
        control.label,
        FontRole::Label,
        label_fg,
        content_x,
        top,
        CTRL_ROW_H,
        scale,
    );

    let value = control.kind.value(config);
    let value_fg = if focused {
        theme.accent
    } else {
        theme.fg_secondary
    };
    if focused {
        // `value ›` bracketed by guillemets: signals the value is adjustable with `←/→`.
        let close = " \u{203a}";
        let close_w = measure.width(close, FontRole::Label, None);
        let value_w = measure.width(&value, FontRole::Label, None);
        let open = "\u{2039} ";
        let open_w = measure.width(open, FontRole::Label, None);
        let mut x = content_right - close_w;
        push_row(
            labels,
            measure,
            close,
            FontRole::Label,
            theme.fg_muted,
            x,
            top,
            CTRL_ROW_H,
            scale,
        );
        x -= value_w;
        push_row(
            labels,
            measure,
            &value,
            FontRole::Label,
            value_fg,
            x,
            top,
            CTRL_ROW_H,
            scale,
        );
        x -= open_w;
        push_row(
            labels,
            measure,
            open,
            FontRole::Label,
            theme.fg_muted,
            x,
            top,
            CTRL_ROW_H,
            scale,
        );
    } else {
        let x = content_right - measure.width(&value, FontRole::Label, None);
        push_row(
            labels,
            measure,
            &value,
            FontRole::Label,
            value_fg,
            x,
            top,
            CTRL_ROW_H,
            scale,
        );
    }
}

/// Push one label vertically centered in a row of `row_h` logical px whose top is physical
/// `top` (`row_h = 0` places the label's top at `top`).
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
    use super::{Settings, CATEGORIES};
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
        // Sidebar -> Appearance -> (wrap) the last category.
        settings.cycle_category(false);
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
        let paint = settings.build(panel, 2.0, &config, &theme, &mut m);
        // The active category (Appearance) heading + the focused control value draw in accent;
        // the active-category fill/bar + focused-control fill are among the quads.
        assert!(paint.labels.iter().any(|l| l.text == "Appearance"));
        assert!(paint.labels.iter().any(|l| l.color == theme.accent));
        // >= 3 quads: the active-category subtle fill + accent bar + the focused-control fill.
        assert!(paint.quads.len() >= 3);
        assert!(paint.nav_divider_x > panel.x);
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

                control.kind.adjust(&mut config, 1);
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
            CATEGORIES[self.category].label
        }
        fn selected_index(&self) -> usize {
            self.selected
        }
    }
}
