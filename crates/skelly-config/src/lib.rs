//! `skelly-config` - Skelly's single source of truth.
//!
//! `~/.config/skelly/config.toml` owns every setting; the UI is a *view* over it
//! (AGENTS.md Hard rule 1). This crate loads that file into a typed [`Config`],
//! validates it, and can serialize it back so the settings view round-trips
//! exactly one key per control. Every field carries a spec-accurate default, so
//! Skelly is usable with zero configuration and a partial file only overrides what
//! it names.
//!
//! This crate is a leaf: it has no dependency on rendering, the terminal, or the
//! window, so it stays pure and fast to test.

#![doc(test(attr(deny(warnings))))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Anything that can go wrong loading or validating a [`Config`].
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The file could not be read from disk.
    #[error("reading config at {path}")]
    Read {
        /// The path we tried to read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file was not valid TOML, or did not match the schema.
    #[error("parsing config")]
    Parse(#[from] toml::de::Error),
    /// A value parsed but fell outside its allowed range. Names the offending key.
    #[error("invalid config: {0}")]
    Invalid(String),
    /// The config could not be serialized back to TOML.
    #[error("serializing config")]
    Serialize(#[from] toml::ser::Error),
    /// The file could not be written to disk.
    #[error("writing config to {path}")]
    Write {
        /// The path we tried to write.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// The complete Skelly configuration - one field per `config.toml` key.
///
/// Sections mirror the design guide's schema exactly. Deserializing is lenient:
/// missing sections and missing keys fall back to their spec defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// `[appearance]` - theme, fonts, cursor, window compositing.
    pub appearance: Appearance,
    /// `[sidebar]` - the vertical tab sidebar.
    pub sidebar: Sidebar,
    /// `[tabs]` - tab titling behavior.
    pub tabs: Tabs,
    /// `[panes]` - the tiling pane tree.
    pub panes: Panes,
    /// `[session]` - timeline, persistence, non-destructive rewind.
    pub session: Session,
    /// `[git]` - git diff presentation.
    pub git: Git,
    /// `[shell]` - the shell program launched in each pane (the guide's "Shell & env").
    pub shell: Shell,
    /// `[keys]` - user keybinding overrides, `chord -> action`. Merged over the
    /// built-in bindings; empty by default (built-ins live in the binding registry).
    pub keys: BTreeMap<String, String>,
}

/// `[appearance]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Active UI + ANSI theme name (e.g. `ossein-dark`, `kanagawa`).
    pub theme: String,
    /// Terminal cell font. Any installed monospace; Nerd Fonts are first-class.
    pub font_family: String,
    /// Ordered fallback chain for glyphs the primary font lacks.
    pub font_fallback: Vec<String>,
    /// Cell font size in px. Valid range 8..=32.
    pub font_size: u16,
    /// Cell line height multiplier. Valid range 0.8..=3.0.
    pub line_height: f32,
    /// Enable programming ligatures in the cell grid.
    pub ligatures: bool,
    /// Cursor shape.
    pub cursor: CursorStyle,
    /// Render bold text using the bright ANSI colors.
    pub bold_is_bright: bool,
    /// Background blur radius (0 disables). Valid range 0..=100.
    pub bg_blur: u8,
    /// Window opacity. Valid range 0.0..=1.0.
    pub opacity: f32,
    /// Show the per-pane status line (design §08 #9 / §10.6 "Show pane status line").
    pub show_status_line: bool,
}

/// Cursor shape for the focused pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    /// A full cell block (default).
    #[default]
    Block,
    /// A thin vertical bar.
    Bar,
    /// A thin underline.
    Underline,
}

/// `[sidebar]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sidebar {
    /// Full-panel sidebar width in px (valid range 56..=360). The slim icon rail is a
    /// fixed 56px and is selected by `mode = "autohide"`, not by a narrow `width`.
    pub width: u16,
    /// Show the pinned-tab grid.
    pub show_pinned: bool,
    /// Sidebar display mode.
    pub mode: SidebarMode,
}

/// How the sidebar presents itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SidebarMode {
    /// Always visible at full width (default).
    #[default]
    Fixed,
    /// Collapses to the icon rail until hovered.
    Autohide,
    /// Fully hidden until recalled.
    Hidden,
}

/// `[tabs]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tabs {
    /// What a tab is titled by.
    pub title: TabTitle,
    /// Re-title as the working directory changes, unless manually renamed.
    pub follow_cwd: bool,
}

/// Source of a tab's title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TabTitle {
    /// The current working directory name (default).
    #[default]
    Cwd,
    /// The running command.
    Command,
    /// A user-set custom name.
    Custom,
}

/// `[panes]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Panes {
    /// Maximum panes per tab. Hard cap 8 (Hard rule 4). Valid range 1..=8.
    pub max: u8,
    /// tmux-style leader chord for pane control.
    pub leader: String,
    /// New splits inherit the focused pane's working directory.
    pub split_inherits_cwd: bool,
}

/// `[session]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// Record the session timeline.
    pub timeline: bool,
    /// Restore tabs + pinned layout on launch (layout only).
    pub persist: bool,
    /// Use a shadow worktree so rewind never mutates HEAD (Hard rule 3).
    pub shadow_worktree: bool,
}

/// `[git]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Git {
    /// Diff layout.
    pub diff_view: DiffView,
}

/// `[shell]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Shell {
    /// The shell program to launch (e.g. `zsh`, `bash`, `fish`). Empty = the login shell
    /// (`$SHELL`, else `/bin/bash`), which is also what the first-run "Skip" accepts.
    pub program: String,
}

/// Git diff layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DiffView {
    /// A single unified column (default).
    #[default]
    Unified,
    /// Side-by-side old/new.
    Split,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "ossein-dark".to_owned(),
            font_family: "JetBrainsMono Nerd Font".to_owned(),
            font_fallback: vec![
                "Symbols Nerd Font".to_owned(),
                "Noto Color Emoji".to_owned(),
            ],
            font_size: 14,
            line_height: 1.2,
            ligatures: true,
            cursor: CursorStyle::Block,
            bold_is_bright: true,
            bg_blur: 18,
            opacity: 0.98,
            show_status_line: true,
        }
    }
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            width: 240,
            show_pinned: true,
            mode: SidebarMode::Fixed,
        }
    }
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            title: TabTitle::Cwd,
            follow_cwd: true,
        }
    }
}

impl Default for Panes {
    fn default() -> Self {
        Self {
            max: 8,
            leader: "ctrl+a".to_owned(),
            split_inherits_cwd: true,
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self {
            timeline: true,
            persist: true,
            shadow_worktree: true,
        }
    }
}

impl Default for Git {
    fn default() -> Self {
        Self {
            diff_view: DiffView::Unified,
        }
    }
}

/// The hard cap on panes per tab (Hard rule 4).
pub const MAX_PANES: u8 = 8;

impl Config {
    /// Parse a config from a TOML string and validate it.
    ///
    /// # Errors
    /// Returns [`ConfigError::Parse`] if the string is not valid TOML matching the
    /// schema, or [`ConfigError::Invalid`] if a value is out of range.
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(s)?;
        config.validate()?;
        Ok(config)
    }

    /// Load and validate the config from a specific path.
    ///
    /// # Errors
    /// Returns [`ConfigError::Read`] if the file cannot be read, or the same errors
    /// as [`Config::from_toml_str`] for a malformed or invalid file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&text)
    }

    /// Load from the default path if it exists, otherwise return validated defaults.
    ///
    /// This is the launch path: a fresh install with no file gets spec defaults
    /// ("usable with zero configuration"), a partial file overrides only what it
    /// names.
    ///
    /// # Errors
    /// Returns an error only when a file *exists* at the default path but is
    /// unreadable, malformed, or invalid. A missing file is not an error.
    pub fn load_default() -> Result<Self, ConfigError> {
        match Self::default_path() {
            Some(path) if path.exists() => Self::load(&path),
            _ => Ok(Self::default()),
        }
    }

    /// Whether this is a first run: no config file exists yet at the default path. The
    /// binary shows the first-run onboarding (design §10.1) when true; writing the config
    /// (its Skip/Start both `save_default`) clears it for next launch. A path that cannot
    /// be resolved (no `HOME`/`XDG_CONFIG_HOME`) is treated as not-first-run (no place to
    /// persist a choice, so onboarding would recur every launch).
    #[must_use]
    pub fn is_first_run() -> bool {
        Self::default_path().is_some_and(|p| !p.exists())
    }

    /// The default config path: `$XDG_CONFIG_HOME/skelly/config.toml`, falling back
    /// to `$HOME/.config/skelly/config.toml`. `None` if neither var is set.
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Some(PathBuf::from(xdg).join("skelly").join("config.toml"));
            }
        }
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".config")
                .join("skelly")
                .join("config.toml"),
        )
    }

    /// Serialize back to TOML - the write side of the settings-view round-trip.
    ///
    /// # Errors
    /// Returns [`toml::ser::Error`] only if a value cannot be represented as TOML,
    /// which the typed schema makes unreachable in practice.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Validate, serialize, and write the config to `path`, creating parent
    /// directories as needed. The write is atomic: it lands in a sibling temp file
    /// that is renamed over `path`, so a crash mid-write never truncates the real
    /// config. This is the settings view's persistence path - every control edit
    /// writes the whole file back (the file stays the single source of truth, Hard
    /// rule 1).
    ///
    /// # Errors
    /// Returns [`ConfigError::Invalid`] if the config is out of range,
    /// [`ConfigError::Serialize`] if it cannot be encoded, or [`ConfigError::Write`]
    /// on any I/O failure.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        let text = self.to_toml_string()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text.as_bytes()).map_err(|source| ConfigError::Write {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, path).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Save to the default config path ([`Config::default_path`]), the launch path's
    /// mirror of [`Config::load_default`].
    ///
    /// # Errors
    /// Returns [`ConfigError::Invalid`] if no default path resolves (neither
    /// `XDG_CONFIG_HOME` nor `HOME` is set) or the same errors as [`Config::save`].
    pub fn save_default(&self) -> Result<(), ConfigError> {
        let path = Self::default_path().ok_or_else(|| {
            ConfigError::Invalid("no config path (neither XDG_CONFIG_HOME nor HOME is set)".into())
        })?;
        self.save(&path)
    }

    /// Check every value is within its allowed range.
    ///
    /// # Errors
    /// Returns [`ConfigError::Invalid`] naming the first offending key.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |msg: String| Err(ConfigError::Invalid(msg));

        if !(8..=32).contains(&self.appearance.font_size) {
            return invalid(format!(
                "appearance.font_size = {} (must be 8..=32)",
                self.appearance.font_size
            ));
        }
        if !(0.8..=3.0).contains(&self.appearance.line_height) {
            return invalid(format!(
                "appearance.line_height = {} (must be 0.8..=3.0)",
                self.appearance.line_height
            ));
        }
        if self.appearance.bg_blur > 100 {
            return invalid(format!(
                "appearance.bg_blur = {} (must be 0..=100)",
                self.appearance.bg_blur
            ));
        }
        if !(0.0..=1.0).contains(&self.appearance.opacity) {
            return invalid(format!(
                "appearance.opacity = {} (must be 0.0..=1.0)",
                self.appearance.opacity
            ));
        }
        if !(56..=360).contains(&self.sidebar.width) {
            return invalid(format!(
                "sidebar.width = {} (must be 56..=360)",
                self.sidebar.width
            ));
        }
        if self.panes.max == 0 || self.panes.max > MAX_PANES {
            return invalid(format!(
                "panes.max = {} (must be 1..={MAX_PANES})",
                self.panes.max
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let c = Config::default();
        assert_eq!(c.appearance.theme, "ossein-dark");
        assert_eq!(c.appearance.font_family, "JetBrainsMono Nerd Font");
        assert_eq!(c.appearance.font_size, 14);
        assert_eq!(c.appearance.cursor, CursorStyle::Block);
        assert!(c.appearance.bold_is_bright);
        assert_eq!(c.appearance.bg_blur, 18);
        assert_eq!(c.sidebar.width, 240);
        assert_eq!(c.sidebar.mode, SidebarMode::Fixed);
        assert_eq!(c.tabs.title, TabTitle::Cwd);
        assert_eq!(c.panes.max, 8);
        assert_eq!(c.panes.leader, "ctrl+a");
        assert!(c.session.shadow_worktree);
        assert_eq!(c.git.diff_view, DiffView::Unified);
    }

    #[test]
    fn defaults_are_valid() {
        Config::default()
            .validate()
            .expect("spec defaults must validate");
    }

    #[test]
    fn round_trips_through_toml() {
        let original = Config::default();
        let text = original.to_toml_string().expect("serialize");
        let parsed = Config::from_toml_str(&text).expect("reparse");
        assert_eq!(original, parsed);
    }

    #[test]
    fn shell_program_maps_to_a_config_key() {
        // The first-run onboarding / "Shell & env" settings map 1:1 to `[shell] program`.
        assert_eq!(
            Config::default().shell.program,
            "",
            "empty = the login shell"
        );
        let c = Config::from_toml_str("[shell]\nprogram = \"zsh\"\n").expect("parse");
        assert_eq!(c.shell.program, "zsh");
        // It round-trips (the settings write path, Hard rule 1).
        let text = c.to_toml_string().expect("serialize");
        assert_eq!(
            Config::from_toml_str(&text).expect("reparse").shell.program,
            "zsh"
        );
    }

    #[test]
    fn parses_the_design_guide_sample() {
        // The exact snippet from the design guide's handoff section.
        let text = r#"
            [appearance]
            theme = "ossein-dark"
            font_family = "JetBrainsMono Nerd Font"
            font_fallback = ["Symbols Nerd Font", "Noto Color Emoji"]
            font_size = 14
            line_height = 1.2
            ligatures = true
            cursor = "block"
            bold_is_bright = true
            bg_blur = 18
            opacity = 0.98

            [sidebar]
            width = 240
            show_pinned = true
            mode = "fixed"

            [panes]
            max = 8
            leader = "ctrl+a"
            split_inherits_cwd = true

            [keys]
            "cmd+k" = "palette.open"
            "alt+bar" = "pane.split_right"
            "alt+minus" = "pane.split_down"
        "#;
        let c = Config::from_toml_str(text).expect("design sample must parse");
        assert_eq!(c.appearance.cursor, CursorStyle::Block);
        assert_eq!(c.panes.max, 8);
        assert_eq!(
            c.keys.get("cmd+k").map(String::as_str),
            Some("palette.open")
        );
    }

    #[test]
    fn partial_config_only_overrides_named_keys() {
        // A file that sets a single key must leave every other field at its default.
        let c = Config::from_toml_str("[appearance]\nfont_size = 18\n").expect("parse");
        assert_eq!(c.appearance.font_size, 18);
        assert_eq!(c.appearance.theme, "ossein-dark"); // untouched default
        assert_eq!(c.sidebar.width, 240); // untouched default
    }

    #[test]
    fn rejects_font_size_out_of_range() {
        let err = Config::from_toml_str("[appearance]\nfont_size = 200\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(msg) if msg.contains("font_size")));
    }

    #[test]
    fn rejects_too_many_panes() {
        let err = Config::from_toml_str("[panes]\nmax = 12\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(msg) if msg.contains("panes.max")));
    }

    #[test]
    fn rejects_malformed_toml() {
        let err = Config::from_toml_str("this is not = = toml").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn save_then_load_round_trips_through_a_file() {
        // Write a non-default config to a temp path and read it straight back.
        let mut original = Config::default();
        original.appearance.font_size = 18;
        original.appearance.theme = "ossein-light".to_owned();
        original.sidebar.mode = SidebarMode::Hidden;

        let dir = std::env::temp_dir().join(format!("skelly-cfg-{}", std::process::id()));
        let path = dir.join("nested").join("config.toml");
        original.save(&path).expect("save creates dirs and writes");

        let reloaded = Config::load(&path).expect("reload the saved file");
        assert_eq!(original, reloaded);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let dir = std::env::temp_dir().join(format!("skelly-cfg-tmp-{}", std::process::id()));
        let path = dir.join("config.toml");
        Config::default().save(&path).expect("save");
        assert!(path.exists(), "the real config exists");
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "the atomic temp file is renamed away, never left behind"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
