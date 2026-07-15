//! Launch-time session persistence: the workspace / tab / group / pane layout is saved
//! on quit and restored on the next launch (design/README.md "Persist scope" - resolved
//! **layout only**: tabs + pinned + splits are restored, the prior shell processes are
//! never re-run). This is *state*, not config, so it lives in the state dir, separate from
//! `config.toml` (Hard rule 1 keeps the config file to 1:1 settings).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use skelly_pane::PaneLayout;

/// The whole saved session: every workspace (each with its tabs + groups) and which one was
/// active. Written on quit, read once on launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionState {
    /// The active workspace's index into `workspaces`.
    pub(crate) active_workspace: usize,
    pub(crate) workspaces: Vec<WorkspaceState>,
}

/// One workspace's persisted layout (design §08 #2): its name, its tabs, its collapsible
/// groups, and which tab was active.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkspaceState {
    pub(crate) name: String,
    /// The active tab's index into `tabs`.
    pub(crate) active: usize,
    pub(crate) groups: Vec<GroupState>,
    pub(crate) tabs: Vec<TabState>,
}

/// One collapsible tab group (design §08 #5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupState {
    pub(crate) name: String,
    pub(crate) collapsed: bool,
}

/// One tab's persisted layout: its pane tiling, each pane's last-known cwd (in `panes()`
/// leaf order), and the sidebar metadata (pinned, custom name, group membership).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TabState {
    pub(crate) layout: PaneLayout,
    /// Each leaf pane's saved cwd, in `panes()` order. An entry is `None` for a pane whose
    /// cwd was never polled; a saved path that no longer exists falls back to the default.
    pub(crate) cwds: Vec<Option<String>>,
    pub(crate) pinned: bool,
    pub(crate) custom_title: Option<String>,
    pub(crate) group: Option<usize>,
    /// Whether the tab had been used (a fresh, untouched tab restores to its empty state).
    pub(crate) activated: bool,
}

impl SessionState {
    /// The state-file path: `$XDG_STATE_HOME/skelly/session.json`, falling back to
    /// `$HOME/.local/state/skelly/session.json`. `None` if neither var is set (no place to
    /// persist, so restore is simply skipped, mirroring [`skelly_config::Config::default_path`]).
    #[must_use]
    pub(crate) fn default_path() -> Option<PathBuf> {
        if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
            if !xdg.is_empty() {
                return Some(PathBuf::from(xdg).join("skelly").join("session.json"));
            }
        }
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("skelly")
                .join("session.json"),
        )
    }

    /// Load the saved session from the default path, or `None` if there is no path, no file,
    /// or it cannot be parsed (a corrupt file is ignored rather than blocking launch).
    #[must_use]
    pub(crate) fn load_default() -> Option<Self> {
        let path = Self::default_path()?;
        let text = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str(&text) {
            Ok(state) => Some(state),
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "ignoring unreadable session state");
                None
            }
        }
    }

    /// Write the session to the default path atomically (temp file + rename, like the config
    /// save), creating parent directories as needed. A missing state path is a silent no-op.
    ///
    /// # Errors
    /// Returns any I/O or serialization error encountered while writing.
    pub(crate) fn save_default(&self) -> io::Result<()> {
        let Some(path) = Self::default_path() else {
            return Ok(());
        };
        self.save(&path)
    }

    fn save(&self, path: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skelly_pane::{Dir, PaneTree};

    #[test]
    fn session_state_round_trips_through_json() {
        let mut tree = PaneTree::new();
        tree.split(Dir::Right);
        tree.split(Dir::Down);
        let state = SessionState {
            active_workspace: 1,
            workspaces: vec![
                WorkspaceState {
                    name: "Personal".into(),
                    active: 0,
                    groups: vec![GroupState {
                        name: "skelly".into(),
                        collapsed: true,
                    }],
                    tabs: vec![TabState {
                        layout: tree.to_layout(),
                        cwds: vec![Some("~/src/skelly".into()), None, Some("~".into())],
                        pinned: true,
                        custom_title: Some("build".into()),
                        group: Some(0),
                        activated: true,
                    }],
                },
                WorkspaceState {
                    name: "Work".into(),
                    active: 0,
                    groups: vec![],
                    tabs: vec![TabState {
                        layout: PaneTree::new().to_layout(),
                        cwds: vec![None],
                        pinned: false,
                        custom_title: None,
                        group: None,
                        activated: false,
                    }],
                },
            ],
        };

        let json = serde_json::to_string(&state).expect("serialize");
        let back: SessionState = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.active_workspace, 1);
        assert_eq!(back.workspaces.len(), 2);
        let ws = &back.workspaces[0];
        assert_eq!(ws.name, "Personal");
        assert_eq!(ws.groups[0].name, "skelly");
        assert!(ws.groups[0].collapsed);
        let tab = &ws.tabs[0];
        assert_eq!(tab.layout.pane_count(), 3);
        assert_eq!(
            tab.cwds,
            vec![Some("~/src/skelly".into()), None, Some("~".into())]
        );
        assert!(tab.pinned);
        assert_eq!(tab.custom_title.as_deref(), Some("build"));
        assert_eq!(tab.group, Some(0));
        assert!(tab.activated);

        // The restored tree rebuilds to the same pane count.
        assert_eq!(PaneTree::from_layout(&tab.layout).count(), 3);
    }
}
