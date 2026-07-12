//! `skelly` - the binary: window, event loop, and the wiring that binds the library
//! crates together.
//!
//! M1c completed the walking skeleton (one shell, one pane). M3 wires the pane tree
//! (`skelly-pane`) into the window: a live terminal (`skelly-term`) per pane, the
//! renderer (`skelly-render`) drawing each pane at its computed rectangle with a
//! divider and a focused-pane ring, input routed to the focused pane, and the
//! split / focus / zoom / resize / close keybindings. The reader thread of each
//! shell wakes the event loop via an `EventLoopProxy` so we repaint only on new
//! output. Errors are contextualized here with `anyhow`; `wgpu`/`alacritty` types
//! never leak up.

mod deadpane;
mod emptystate;
mod gitdock;
mod palette;
mod settings;
mod sidebar;
mod timeline;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use skelly_config::Config;
use skelly_pane::{Dir, PaneId, PaneTree, Rect};
use skelly_render::{
    AnsiPalette, DeadPaneView, GitDockView, GridCell, OverlayView, PaneView, PxRect, Renderer,
    SettingsView, SidebarView, Srgb, Theme, TimelineView,
};
use skelly_session::{Actor, Repo, SessionEvent, ShadowWorktree};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

use gitdock::GitDock;
use palette::Palette;
use settings::Settings;
use sidebar::Sidebar;
use timeline::TimelineDock;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

/// Logical padding (px) around the whole pane area - the window content margin.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset (px) inside each pane, between its border and its cells.
const PANE_INSET: f32 = 6.0;
/// One keyboard resize step, as a fraction of the enclosing split's extent.
const RESIZE_STEP: f32 = 0.04;
/// Logical padding (px) inside the command palette panel.
const PALETTE_PAD: f32 = 12.0;
/// Logical inset (px) of the sidebar's text from the sidebar's top-left corner.
const SIDEBAR_PAD: f32 = 12.0;
/// Logical width (px) of the slim icon rail (`⇧⌘B`), per design §08 ("Icon rail 56px").
const RAIL_WIDTH: f32 = 56.0;
/// Logical horizontal inset (px) of the rail's centered content - smaller than
/// `SIDEBAR_PAD` so a couple of glyphs fit inside 56px.
const RAIL_PAD: f32 = 6.0;
/// Logical inset (px) of the full-window settings view's text from the window edge.
const SETTINGS_PAD: f32 = 20.0;
/// Logical width (px) of the git diff dock - the guide's default (resizable 360-560 is a
/// later slice, so it is fixed for now).
const GIT_DOCK_WIDTH: f32 = 420.0;
/// Logical inset (px) of the git dock's text from its top-left corner.
const GIT_DOCK_PAD: f32 = 14.0;
/// Diff lines scrolled per `PageUp`/`PageDown` in the git dock.
const DIFF_SCROLL_LINES: i32 = 10;

/// Event the reader thread sends to wake the UI when a shell produces output.
#[derive(Debug, Clone, Copy)]
struct Wakeup;

fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::load_default().context("loading configuration")?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        theme = %config.appearance.theme,
        "starting skelly"
    );

    let event_loop = EventLoop::<Wakeup>::with_user_event()
        .build()
        .context("creating the event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(config, proxy);
    event_loop
        .run_app(&mut app)
        .context("running the event loop")?;
    Ok(())
}

/// A text selection, in a pane's visible-grid cell coordinates `(column, row)`.
#[derive(Clone, Copy)]
struct Selection {
    anchor: (usize, usize),
    head: (usize, usize),
}

/// A pane operation bound to a keyboard chord (the `⌥`-leader pane bindings).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaneAction {
    /// Split the focused pane, placing the new pane in this direction.
    Split(Dir),
    /// Move focus to the neighbor in this direction.
    Focus(Dir),
    /// Focus the pane at this 0-based index in layout order.
    FocusIndex(usize),
    /// Close the focused pane.
    Close,
    /// Toggle zoom on the focused pane.
    Zoom,
    /// Nudge the focused pane's enclosing divider in this direction.
    Resize(Dir),
    /// Reset every split to an even 50/50.
    EvenOut,
}

/// A tab operation bound to a keyboard chord.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TabAction {
    /// Open a new tab and switch to it.
    New,
    /// Close the active tab.
    Close,
    /// Jump to the tab at this 0-based index.
    Goto(usize),
    /// Cycle to the next tab.
    Next,
    /// Cycle to the previous tab.
    Prev,
}

/// One tab: an independent tiling workspace - its own pane tree, a live shell per
/// pane, the per-pane grid-size cache, and its own text selection. Tabs are fully
/// isolated; switching tabs swaps the whole terminal workspace and its shells keep
/// running in the background. (The sidebar that lists tabs is a later slice; today
/// tabs are created, closed, and switched from the keyboard and command palette.)
struct Tab {
    /// The tiling model; every leaf maps to a live terminal in `panes`.
    tree: PaneTree,
    /// One live shell per pane.
    panes: HashMap<PaneId, Terminal>,
    /// Each pane's last-applied grid size, so we only resize on a real change.
    dims: HashMap<PaneId, (u16, u16)>,
    /// The active selection and the pane it belongs to.
    selection: Option<(PaneId, Selection)>,
    /// Whether the user has started using this tab yet. A fresh tab is pristine
    /// (`false`) and shows the empty-state overlay (a faint mark + hint chips); the first
    /// command run (or a split) activates it and the overlay clears (guide §10.2).
    activated: bool,
}

impl Tab {
    /// A fresh tab: a single-pane tree with no shells yet (`sync_layout` spawns them),
    /// pristine so it shows the empty state until the first command runs.
    fn new() -> Self {
        Self {
            tree: PaneTree::new(),
            panes: HashMap::new(),
            dims: HashMap::new(),
            selection: None,
            activated: false,
        }
    }

    /// Whether this tab should show the empty-state overlay: pristine (no command run yet)
    /// and still a single pane (a split means the user is working, so it clears).
    fn is_empty_state(&self) -> bool {
        !self.activated && self.tree.count() == 1
    }
}

/// Application state driven by the winit event loop. The window and renderer are
/// `None` until the platform signals `resumed`; the tab list exists from the start.
struct App {
    config: Config,
    proxy: EventLoopProxy<Wakeup>,
    ansi_palette: AnsiPalette,
    /// The resolved UI theme tokens (for the command palette and other chrome).
    theme: Theme,
    /// The command-palette overlay state.
    palette: Palette,
    /// The full-window settings view state.
    settings: Settings,
    /// The persistent left sidebar (the tab list) state.
    sidebar: Sidebar,
    /// The per-repo git diff dock (right dock) state.
    git_dock: GitDock,
    /// The session-timeline dock (right dock; mutually exclusive with the git dock).
    timeline: TimelineDock,
    /// The live shadow worktree while rewound to a past state (`None` = at HEAD/now). Its
    /// drop removes the worktree, so returning to now / closing just clears it.
    shadow: Option<ShadowWorktree>,
    /// When the session began, for the timeline's session-relative event times.
    session_start: Instant,
    /// Whether the "session started" timeline event has been recorded yet (once, on the
    /// first window activation, when a repo is known).
    session_started: bool,
    /// The repository backing the dock (from the process cwd), cached while it is open so
    /// moving the file selection re-diffs without re-discovering.
    git_repo: Option<Repo>,
    clipboard: Option<arboard::Clipboard>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    /// The open tabs; `active` indexes the visible one. Always at least one.
    tabs: Vec<Tab>,
    /// Index of the visible tab in `tabs`.
    active: usize,
    /// Current surface size in physical px.
    size: (u32, u32),
    scale: f64,
    modifiers: ModifiersState,
    pointer: (f64, f64),
    /// Whether a mouse-drag selection is in progress (in the active tab).
    selecting: bool,
}

impl App {
    fn new(config: Config, proxy: EventLoopProxy<Wakeup>) -> Self {
        let ansi_palette = AnsiPalette::resolve(&config.appearance.theme);
        let theme = Theme::resolve(&config.appearance.theme);
        let sidebar = Sidebar::new(config.sidebar.mode);
        Self {
            config,
            proxy,
            ansi_palette,
            theme,
            palette: Palette::new(),
            settings: Settings::new(),
            sidebar,
            git_dock: GitDock::new(),
            timeline: TimelineDock::new(),
            shadow: None,
            session_start: Instant::now(),
            session_started: false,
            git_repo: None,
            clipboard: arboard::Clipboard::new().ok(),
            window: None,
            renderer: None,
            tabs: vec![Tab::new()],
            active: 0,
            size: (0, 0),
            scale: 1.0,
            modifiers: ModifiersState::empty(),
            pointer: (0.0, 0.0),
            selecting: false,
        }
    }

    /// The currently visible tab.
    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    /// The currently visible tab, mutably.
    fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// The pane area within the window: the surface inset by the window margin, by the
    /// sidebar's width on the left when it is shown, and by the git dock's width on the
    /// right when it is open.
    fn viewport_rect(&self) -> Rect {
        let pad = WINDOW_PAD * scale32(self.scale);
        let sidebar = self.sidebar_width_px();
        let dock = self.right_dock_width_px();
        let w = dim_f32(self.size.0);
        let h = dim_f32(self.size.1);
        Rect::new(
            sidebar + pad,
            pad,
            (w - sidebar - dock - 2.0 * pad).max(1.0),
            (h - 2.0 * pad).max(1.0),
        )
    }

    /// The sidebar's width in physical px, or `0.0` when it is hidden. The panel occupies
    /// the strip `[0, width)` and the pane viewport starts after it; the slim rail is a
    /// fixed 56px regardless of `sidebar.width`.
    fn sidebar_width_px(&self) -> f32 {
        if !self.sidebar.visible() {
            return 0.0;
        }
        let logical = if self.sidebar.is_rail() {
            RAIL_WIDTH
        } else {
            f32::from(self.config.sidebar.width)
        };
        logical * scale32(self.scale)
    }

    /// The right dock's width in physical px, or `0.0` when neither right dock is open. The
    /// git diff dock and the session timeline are mutually exclusive (Hard rule 4) and both
    /// occupy the right strip `[surface_w - width, surface_w)`; the pane viewport ends
    /// before it. Both use the guide's 420px default.
    fn right_dock_width_px(&self) -> f32 {
        if self.git_dock.open || self.timeline.open {
            GIT_DOCK_WIDTH * scale32(self.scale)
        } else {
            0.0
        }
    }

    /// The physical-pixel inset inside each pane (border-to-cells gap).
    fn pane_inset(&self) -> f32 {
        PANE_INSET * scale32(self.scale)
    }

    /// The focused pane's live terminal in the active tab, if any.
    fn focused_term(&mut self) -> Option<&mut Terminal> {
        let ws = self.active_tab_mut();
        let id = ws.tree.focused();
        ws.panes.get_mut(&id)
    }

    /// Whether the focused pane's shell has exited (so it shows the "shell exited" overlay
    /// and swallows input, waiting for a restart).
    fn focused_pane_dead(&self) -> bool {
        let ws = self.active_tab();
        ws.panes
            .get(&ws.tree.focused())
            .is_some_and(|term| term.exit_status().is_some())
    }

    /// Restart the focused pane's shell in place: drop the exited terminal and let
    /// `sync_layout` respawn a fresh shell for the same pane (which is still in the tree).
    /// The pane and its scrollback grid make way for a new prompt.
    fn restart_focused_pane(&mut self) {
        let ws = self.active_tab_mut();
        let id = ws.tree.focused();
        ws.panes.remove(&id);
        ws.dims.remove(&id);
        self.sync_layout();
        self.request_redraw();
    }

    /// The rectangle of pane `id` in the active tab's current layout, if it is visible.
    fn pane_rect(&self, id: PaneId) -> Option<Rect> {
        let viewport = self.viewport_rect();
        self.active_tab()
            .tree
            .layout(viewport)
            .into_iter()
            .find(|(pid, _)| *pid == id)
            .map(|(_, rect)| rect)
    }

    /// The active tab's pane whose rectangle contains the pointer, with that rectangle.
    fn pane_at_pointer(&self) -> Option<(PaneId, Rect)> {
        let (px, py) = point_f32(self.pointer);
        let viewport = self.viewport_rect();
        self.active_tab()
            .tree
            .layout(viewport)
            .into_iter()
            .find(|(_, r)| px >= r.x && px < r.x + r.w && py >= r.y && py < r.y + r.h)
    }

    /// Reconcile the live terminals with the pane tree: spawn a shell for any new
    /// pane, resize panes whose grid size changed, and drop shells for closed panes.
    /// Idempotent - safe to call after any layout change.
    fn sync_layout(&mut self) {
        let Some((cell_w, cell_h, _)) = self.renderer.as_ref().map(Renderer::cell_metrics) else {
            return;
        };
        let inset = self.pane_inset();
        let viewport = self.viewport_rect();
        let proxy = self.proxy.clone();
        let layout = self.active_tab().tree.layout(viewport);

        let ws = self.active_tab_mut();
        // Drop shells for panes no longer in the tree (closed panes). Hidden-by-zoom
        // panes stay, since `tree.panes()` still lists them.
        let live: HashSet<PaneId> = ws.tree.panes().into_iter().collect();
        ws.panes.retain(|id, _| live.contains(id));
        ws.dims.retain(|id, _| live.contains(id));

        for (id, rect) in layout {
            let target = pane_dims(rect, cell_w, cell_h, inset);
            if let Some(term) = ws.panes.get_mut(&id) {
                // Existing pane: resize only when its grid size actually changed.
                if ws.dims.get(&id) != Some(&target) {
                    term.resize(target.0, target.1);
                    ws.dims.insert(id, target);
                }
            } else {
                let proxy = proxy.clone();
                match Terminal::spawn(target.0, target.1, move || {
                    let _ = proxy.send_event(Wakeup);
                }) {
                    Ok(term) => {
                        ws.panes.insert(id, term);
                        ws.dims.insert(id, target);
                    }
                    Err(err) => {
                        tracing::error!(%err, "failed to spawn shell for a new pane");
                        // Roll the split back so every live pane still has a shell.
                        ws.tree.set_focus(id);
                        ws.tree.close();
                    }
                }
            }
        }
    }

    /// Build the owned per-pane frame data for the active tab: each visible pane's
    /// resolved cell grid, cursor, selection, rectangle, and focus flag.
    fn pane_frames(&self) -> Vec<PaneFrame> {
        let inset = self.pane_inset();
        let viewport = self.viewport_rect();
        let ws = self.active_tab();
        let focused = ws.tree.focused();
        // A pristine single-pane tab paints the empty-state mark + hint chips over its
        // (blank) grid, until the first command runs.
        let empty_state = ws.is_empty_state();
        ws.tree
            .layout(viewport)
            .into_iter()
            .filter_map(|(id, rect)| {
                let term = ws.panes.get(&id)?;
                let mut rows: Vec<Vec<GridCell>> = term
                    .cells()
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|c| resolve_cell(c, &self.ansi_palette))
                            .collect()
                    })
                    .collect();
                if empty_state && term.exit_status().is_none() {
                    emptystate::overlay_onto(&mut rows, &self.theme);
                }
                let cols = rows.first().map_or(0, Vec::len);
                let selection = match ws.selection {
                    Some((sid, sel)) if sid == id => selection_cells(sel, rows.len(), cols),
                    _ => Vec::new(),
                };
                Some(PaneFrame {
                    rect: PxRect {
                        x: rect.x,
                        y: rect.y,
                        w: rect.w,
                        h: rect.h,
                    },
                    origin: (rect.x + inset, rect.y + inset),
                    rows,
                    cursor: term.cursor(),
                    selection,
                    focused: id == focused,
                })
            })
            .collect()
    }

    /// Build the "shell exited" overlay for each visible pane whose shell has ended: the
    /// centered exit-message grid, positioned within the pane's rectangle. Empty while
    /// every pane's shell is alive (the common case), so it costs nothing then.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the message grid is a handful of small cells; the usize->f32 cast is exact"
    )]
    fn dead_pane_frames(&self) -> Vec<DeadPaneFrame> {
        let (cell_w, cell_h) = self.cell_size();
        let viewport = self.viewport_rect();
        let ws = self.active_tab();
        ws.tree
            .layout(viewport)
            .into_iter()
            .filter_map(|(id, rect)| {
                let status = ws.panes.get(&id)?.exit_status()?;
                let rows = deadpane::overlay_grid(&status, &self.theme);
                // Center the message grid within the pane rect.
                let grid_w = rows.first().map_or(0, Vec::len) as f32 * cell_w;
                let grid_h = rows.len() as f32 * cell_h;
                Some(DeadPaneFrame {
                    rect: PxRect {
                        x: rect.x,
                        y: rect.y,
                        w: rect.w,
                        h: rect.h,
                    },
                    text_origin: (
                        rect.x + ((rect.w - grid_w) / 2.0).max(0.0),
                        rect.y + ((rect.h - grid_h) / 2.0).max(0.0),
                    ),
                    rows,
                })
            })
            .collect()
    }

    /// Repaint every visible pane from its terminal grid, resolving cell colors and
    /// overlaying the selection and the focused-pane ring.
    fn redraw(&mut self) {
        let frames = self.pane_frames();

        let views: Vec<PaneView> = frames
            .iter()
            .map(|f| PaneView {
                rect: f.rect,
                origin: f.origin,
                rows: &f.rows,
                cursor: f.cursor,
                selection: &f.selection,
                focused: f.focused,
            })
            .collect();

        // Build the dim overlays for any exited panes and the chrome frames, all before
        // the mutable renderer borrow.
        let dead = self.dead_pane_frames();
        let sidebar = self.sidebar.visible().then(|| self.build_sidebar_frame());
        let git_dock = self.git_dock.open.then(|| self.build_git_dock_frame());
        let timeline = self.timeline.open.then(|| self.build_timeline_frame());
        let overlay = self.palette.open.then(|| self.build_palette_frame());
        let settings = self.settings.open.then(|| self.build_settings_frame());
        // Write the clamped diff scroll back so repeated paging past the end settles.
        if let Some(frame) = &git_dock {
            self.git_dock.set_scroll(frame.diff_scroll);
        }

        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_panes(&views);
            let dead_views: Vec<DeadPaneView> = dead
                .iter()
                .map(|f| DeadPaneView {
                    rect: f.rect,
                    text_origin: f.text_origin,
                    rows: &f.rows,
                })
                .collect();
            renderer.set_pane_overlays(&dead_views);
            match &sidebar {
                Some(frame) => renderer.set_sidebar(Some(&SidebarView {
                    panel: frame.panel,
                    text_origin: frame.origin,
                    rows: &frame.rows,
                    active_row: frame.active_row,
                })),
                None => renderer.set_sidebar(None),
            }
            match &git_dock {
                Some(frame) => renderer.set_git_dock(Some(&GitDockView {
                    panel: frame.panel,
                    text_origin: frame.origin,
                    rows: &frame.rows,
                    selected_file_row: frame.selected_file_row,
                    add_rows: &frame.add_rows,
                    del_rows: &frame.del_rows,
                    hunk_rows: &frame.hunk_rows,
                    focused_hunk_row: frame.focused_hunk_row,
                    caret: frame.caret,
                })),
                None => renderer.set_git_dock(None),
            }
            match &timeline {
                Some(frame) => renderer.set_timeline(Some(&TimelineView {
                    panel: frame.panel,
                    text_origin: frame.origin,
                    rows: &frame.rows,
                    selected_row: frame.selected_row,
                    viewing_row: frame.viewing_row,
                })),
                None => renderer.set_timeline(None),
            }
            match &overlay {
                Some(frame) => renderer.set_overlay(Some(&OverlayView {
                    panel: frame.panel,
                    text_origin: frame.origin,
                    rows: &frame.rows,
                    selected_row: frame.selected_row,
                    caret: Some(frame.caret),
                })),
                None => renderer.set_overlay(None),
            }
            match &settings {
                Some(frame) => renderer.set_settings(Some(&SettingsView {
                    panel: frame.panel,
                    text_origin: frame.origin,
                    rows: &frame.rows,
                    nav_cols: frame.nav_cols,
                    nav_active_row: frame.nav_active_row,
                    selected_row: frame.selected_row,
                })),
                None => renderer.set_settings(None),
            }
            if let Err(err) = renderer.render() {
                tracing::error!(%err, "frame render failed");
            }
        }
    }

    /// Lay out the command palette as a centered panel: pick a width in cells, render
    /// the palette grid in UI tokens, and size the panel to fit it.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "palette grid dimensions are small, non-negative values"
    )]
    fn build_palette_frame(&self) -> PaletteFrame {
        let (cell_w, cell_h) = self.cell_size();
        let pad = PALETTE_PAD * scale32(self.scale);
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));

        // The palette sizes itself to its content, but never wider than fits.
        let max_cols = ((surface_w * 0.9) / cell_w).floor().max(28.0) as usize;
        let view = self.palette.view(max_cols, &self.theme);
        let grid_cols = view.rows.first().map_or(max_cols, Vec::len);
        let rows = view.rows.len();

        let panel_w = grid_cols as f32 * cell_w + 2.0 * pad;
        let panel_h = rows as f32 * cell_h + 2.0 * pad;
        let x = ((surface_w - panel_w) / 2.0).max(0.0);
        let y = (surface_h * 0.16).min((surface_h - panel_h).max(0.0));

        PaletteFrame {
            panel: PxRect {
                x,
                y,
                w: panel_w,
                h: panel_h,
            },
            origin: (x + pad, y + pad),
            rows: view.rows,
            selected_row: view.selected_row,
            caret: view.caret,
        }
    }

    /// Lay out the left sidebar: a full-height panel of `sidebar.width`, its tab list
    /// rendered in UI tokens and clipped to that width.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the sidebar cell width is a small, non-negative value"
    )]
    fn build_sidebar_frame(&self) -> SidebarFrame {
        let (cell_w, _) = self.cell_size();
        let rail = self.sidebar.is_rail();
        let scale = scale32(self.scale);
        // Narrower horizontal inset for the rail so its glyphs center inside 56px; the
        // vertical inset stays `SIDEBAR_PAD` (matched by `sidebar_hit`'s `origin_y`).
        let pad_x = if rail { RAIL_PAD } else { SIDEBAR_PAD } * scale;
        let pad_y = SIDEBAR_PAD * scale;
        let sidebar_w = self.sidebar_width_px();
        let surface_h = dim_f32(self.size.1);

        let cols = ((sidebar_w - 2.0 * pad_x) / cell_w).floor().max(1.0) as usize;
        let view = sidebar::view(self.tabs.len(), self.active, cols, rail, &self.theme);

        SidebarFrame {
            panel: PxRect {
                x: 0.0,
                y: 0.0,
                w: sidebar_w,
                h: surface_h,
            },
            origin: (pad_x, pad_y),
            rows: view.rows,
            active_row: view.active_row,
        }
    }

    /// Lay out the settings view: a full-window panel, its nav + control grid rendered
    /// in UI tokens and clipped to the window.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the settings cell width is a small, non-negative value"
    )]
    fn build_settings_frame(&self) -> SettingsFrame {
        let (cell_w, _) = self.cell_size();
        let pad = SETTINGS_PAD * scale32(self.scale);
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));

        let cols = ((surface_w - 2.0 * pad) / cell_w).floor().max(1.0) as usize;
        let view = self.settings.view(cols, &self.config, &self.theme);

        SettingsFrame {
            panel: PxRect {
                x: 0.0,
                y: 0.0,
                w: surface_w,
                h: surface_h,
            },
            origin: (pad, pad),
            rows: view.rows,
            nav_cols: view.nav_cols,
            nav_active_row: view.nav_active_row,
            selected_row: view.selected_row,
        }
    }

    /// Lay out the git dock: a fixed-width panel on the right edge, its status bar +
    /// file list + selected-file diff rendered in UI tokens and clipped to the panel.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the dock cell dimensions are small, non-negative values"
    )]
    fn build_git_dock_frame(&self) -> GitDockFrame {
        let (cell_w, cell_h) = self.cell_size();
        let pad = GIT_DOCK_PAD * scale32(self.scale);
        let dock_w = self.right_dock_width_px();
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        let panel_x = (surface_w - dock_w).max(0.0);

        let cols = ((dock_w - 2.0 * pad) / cell_w).floor().max(1.0) as usize;
        let rows = ((surface_h - 2.0 * pad) / cell_h).floor().max(1.0) as usize;
        let view = self.git_dock.view(cols, rows, &self.theme);

        GitDockFrame {
            panel: PxRect {
                x: panel_x,
                y: 0.0,
                w: dock_w,
                h: surface_h,
            },
            origin: (panel_x + pad, pad),
            rows: view.rows,
            selected_file_row: view.selected_file_row,
            add_rows: view.add_rows,
            del_rows: view.del_rows,
            hunk_rows: view.hunk_rows,
            focused_hunk_row: view.focused_hunk_row,
            caret: view.caret,
            diff_scroll: view.diff_scroll,
        }
    }

    /// Lay out the session-timeline dock on the right edge, mirroring the git dock: the
    /// event list + status banner + foot rendered in UI tokens and clipped to the panel.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the dock cell dimensions are small, non-negative values"
    )]
    fn build_timeline_frame(&self) -> TimelineFrame {
        let (cell_w, cell_h) = self.cell_size();
        let pad = GIT_DOCK_PAD * scale32(self.scale);
        let dock_w = self.right_dock_width_px();
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        let panel_x = (surface_w - dock_w).max(0.0);

        let cols = ((dock_w - 2.0 * pad) / cell_w).floor().max(1.0) as usize;
        let rows = ((surface_h - 2.0 * pad) / cell_h).floor().max(1.0) as usize;
        let view = self.timeline.view(cols, rows, &self.theme);

        TimelineFrame {
            panel: PxRect {
                x: panel_x,
                y: 0.0,
                w: dock_w,
                h: surface_h,
            },
            origin: (panel_x + pad, pad),
            rows: view.rows,
            selected_row: view.selected_row,
            viewing_row: view.viewing_row,
        }
    }

    /// Open the settings view (`⌘,` or the palette command).
    fn open_settings(&mut self) {
        self.settings.open();
        self.request_redraw();
    }

    /// Persist the config to disk after a settings edit - the file is the source of
    /// truth (Hard rule 1). A write failure is logged, not fatal (the in-memory config
    /// still reflects the change for this session).
    fn persist_config(&self) {
        if let Err(err) = self.config.save_default() {
            tracing::warn!(%err, "failed to persist config");
        }
    }

    /// Apply a settings change: the live effects Skelly can do cheaply now (theme and
    /// sidebar re-layout), then persist the file and repaint. Font / cursor / opacity
    /// changes are persisted and take effect on the next launch (live font re-shaping is
    /// a later slice).
    fn apply_setting_change(&mut self, key: &str) {
        match key {
            "appearance.theme" => self.set_theme_live(),
            "sidebar.mode" => {
                self.sidebar.set_mode(self.config.sidebar.mode);
                self.sync_layout();
            }
            "sidebar.width" => self.sync_layout(),
            _ => {}
        }
        self.persist_config();
        self.request_redraw();
    }

    /// Show or hide the sidebar (`⌘B`). The pane viewport changes width, so re-fit the
    /// shells; the chosen mode persists (design §08, Hard rule 1).
    fn toggle_sidebar(&mut self) {
        self.sidebar.toggle();
        self.persist_sidebar_mode();
        self.sync_layout();
        self.request_redraw();
    }

    /// Cycle the sidebar between the full panel and the slim icon rail (`⇧⌘B`, design
    /// §08). The viewport changes width, so re-fit the shells; the mode persists.
    fn cycle_sidebar_mode(&mut self) {
        self.sidebar.cycle_rail();
        self.persist_sidebar_mode();
        self.sync_layout();
        self.request_redraw();
    }

    /// Write the sidebar's current display mode back to the config (the file is the
    /// source of truth, Hard rule 1; the chosen mode persists per workspace) and save it.
    fn persist_sidebar_mode(&mut self) {
        self.config.sidebar.mode = self.sidebar.mode();
        self.persist_config();
    }

    /// Toggle the git diff dock (`⇧⌘G`). Opening refreshes the repo status and the
    /// selected file's diff; either way the pane viewport changes width, so re-fit the
    /// shells and repaint.
    fn toggle_git_dock(&mut self) {
        if self.git_dock.open {
            self.git_dock.close();
            self.git_repo = None;
        } else {
            // Only one right dock at a time (Hard rule 4): opening the diff closes the
            // timeline and returns to now.
            self.close_timeline();
            self.git_dock.open();
            self.refresh_git();
        }
        self.sync_layout();
        self.request_redraw();
    }

    /// Toggle the session-timeline dock (`⇧⌘H`). Opening it closes the git dock (Hard rule
    /// 4), records the session-start event if it has not been yet, and snaps to now; either
    /// way the pane viewport changes width, so re-fit the shells and repaint.
    fn toggle_timeline(&mut self) {
        if self.timeline.open {
            self.close_timeline();
        } else {
            self.git_dock.close();
            self.git_repo = None;
            self.record_session_start();
            let branch = current_branch();
            self.timeline.open(branch);
            self.reconcile_shadow();
        }
        self.sync_layout();
        self.request_redraw();
    }

    /// Close the timeline dock and return to now (discarding any shadow worktree).
    fn close_timeline(&mut self) {
        if self.timeline.open {
            self.timeline.close();
        }
        self.discard_shadow();
    }

    /// Record the one-time "session started" event, anchored to the launch HEAD (so the
    /// pre-session state is restorable). Discovers the process-cwd repo once; a non-repo or
    /// a repo with no commits simply records a non-restorable anchor.
    fn record_session_start(&mut self) {
        if self.session_started {
            return;
        }
        self.session_started = true;
        let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let (detail, restore) = match Repo::discover(&start) {
            Ok(Some(repo)) => {
                let branch = repo.status().ok().and_then(|s| s.branch);
                let head = repo.head_short().ok();
                (branch.unwrap_or_else(|| "(detached)".to_owned()), head)
            }
            _ => ("no repository".to_owned(), None),
        };
        let mut event = SessionEvent::new(
            Actor::System,
            self.elapsed_label(),
            "Session started",
            detail,
        );
        if let Some(sha) = restore {
            event = event.restoring(sha);
        }
        self.timeline.record(self.session_start.elapsed(), event);
    }

    /// Record a timeline event now, stamping it with the session-relative elapsed time.
    fn record_event(
        &mut self,
        actor: Actor,
        title: impl Into<String>,
        detail: impl Into<String>,
        restore: Option<String>,
    ) {
        let mut event = SessionEvent::new(actor, self.elapsed_label(), title, detail);
        if let Some(sha) = restore {
            event = event.restoring(sha);
        }
        self.timeline.record(self.session_start.elapsed(), event);
    }

    /// A short session-relative time label (`M:SS` into the session) for a recorded event.
    fn elapsed_label(&self) -> String {
        let secs = self.session_start.elapsed().as_secs();
        format!("{}:{:02}", secs / 60, secs % 60)
    }

    /// Reconcile the shadow worktree to the timeline's current selection: at now, discard
    /// any worktree; on a past state, ensure a shadow worktree is checked out to its commit
    /// (Hard rule 3 - never touches HEAD/refs). A git failure is logged and treated as
    /// "stay at now" rather than left half-applied.
    fn reconcile_shadow(&mut self) {
        if self.timeline.selection_is_now() {
            self.discard_shadow();
            return;
        }
        let Some(sha) = self.timeline.selected_restore() else {
            self.discard_shadow();
            return;
        };
        // Already viewing this commit? Nothing to do.
        if self.shadow.as_ref().is_some_and(|w| w.committish() == sha) {
            return;
        }
        self.discard_shadow();
        let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        match Repo::discover(&start) {
            Ok(Some(repo)) => match repo.shadow_checkout(&sha) {
                Ok(worktree) => self.shadow = Some(worktree),
                Err(err) => tracing::warn!(%err, %sha, "shadow checkout failed"),
            },
            Ok(None) => tracing::warn!("cannot rewind: not in a git repository"),
            Err(err) => tracing::warn!(%err, "cannot rewind: git discovery failed"),
        }
    }

    /// Discard the live shadow worktree, if any (`git worktree remove --force` via its
    /// drop), returning to the real HEAD.
    fn discard_shadow(&mut self) {
        if let Some(worktree) = self.shadow.take() {
            if let Err(err) = worktree.discard() {
                tracing::warn!(%err, "failed to remove shadow worktree");
            }
        }
    }

    /// Refresh the dock from the repository of the process working directory (a v1
    /// limitation: real per-pane cwd tracking is a follow-up, the same blocker as
    /// cwd-based tab titles). Caches the repo, loads the working status, then the selected
    /// file's diff.
    fn refresh_git(&mut self) {
        let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        match Repo::discover(&start) {
            Ok(Some(repo)) => match repo.status() {
                Ok(status) => {
                    self.git_dock.load(status);
                    self.git_repo = Some(repo);
                    self.load_selected_diff();
                }
                Err(err) => {
                    self.git_dock.set_error(err.to_string());
                    self.git_repo = None;
                }
            },
            Ok(None) => {
                self.git_dock.set_no_repo();
                self.git_repo = None;
            }
            Err(err) => {
                self.git_dock.set_error(err.to_string());
                self.git_repo = None;
            }
        }
    }

    /// Initialize a git repository in the process cwd (the git dock's "Init repo"
    /// empty-state action, design §12 "Not a git repo") and refresh the dock to show the
    /// new, empty repo. A failure surfaces in the dock's error line.
    fn init_repo(&mut self) {
        let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        match Repo::init(&start) {
            Ok(_) => self.refresh_git(),
            Err(err) => self.git_dock.set_error(err.to_string()),
        }
        self.request_redraw();
    }

    /// Load the selected file's unified diff into the dock from the cached repo. Shows the
    /// unstaged change when there is one, else the staged change; untracked files have no
    /// diff (the dock shows a placeholder for them).
    fn load_selected_diff(&mut self) {
        let Some(repo) = self.git_repo.clone() else {
            return;
        };
        let Some(file) = self.git_dock.selected_file() else {
            return;
        };
        if matches!(file.status, skelly_session::FileStatus::Untracked) {
            self.git_dock
                .set_diff(skelly_session::FileDiff::default(), false);
            return;
        }
        let path = file.path.clone();
        let staged = file.staged && !file.unstaged;
        match repo.diff(&path, staged) {
            Ok(diff) => self.git_dock.set_diff(diff, staged),
            Err(err) => self.git_dock.set_error(err.to_string()),
        }
    }

    /// Map the pointer to a sidebar row action, or `None` if it isn't over the sidebar.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the row index is computed from a guarded non-negative offset"
    )]
    fn sidebar_hit(&self) -> Option<sidebar::Hit> {
        if !self.sidebar.visible() {
            return None;
        }
        let (px, py) = point_f32(self.pointer);
        if px >= self.sidebar_width_px() {
            return None;
        }
        let (_, cell_h) = self.cell_size();
        let origin_y = SIDEBAR_PAD * scale32(self.scale);
        if py < origin_y {
            return None;
        }
        let row = ((py - origin_y) / cell_h).floor() as usize;
        sidebar::hit(self.tabs.len(), row)
    }

    /// Copy the current selection to the clipboard.
    fn copy_selection(&mut self) {
        let Some((id, sel)) = self.active_tab().selection else {
            return;
        };
        let Some(term) = self.active_tab().panes.get(&id) else {
            return;
        };
        let text = selection_text(sel, &term.cells());
        if text.is_empty() {
            return;
        }
        if let Some(clipboard) = self.clipboard.as_mut() {
            if let Err(err) = clipboard.set_text(text) {
                tracing::warn!(%err, "clipboard copy failed");
            }
        }
    }

    /// Paste the clipboard contents into the focused pane's shell.
    fn paste(&mut self) {
        let Some(text) = self.clipboard.as_mut().and_then(|c| c.get_text().ok()) else {
            return;
        };
        if let Some(term) = self.focused_term() {
            term.scroll_to_bottom();
            term.write(text.as_bytes());
        }
    }

    /// Apply a pane-tree operation, then reconcile terminals and request a repaint.
    fn apply_pane_action(&mut self, action: PaneAction) {
        // Closing the only pane closes the whole tab (design edge state "Close last pane":
        // the only pane closing closes the tab; the only tab closing shows the empty state).
        if matches!(action, PaneAction::Close) && self.active_tab().tree.count() == 1 {
            self.close_tab();
            return;
        }
        let cap = usize::from(self.config.panes.max).min(skelly_pane::MAX_PANES);
        let ws = self.active_tab_mut();
        let changed = match action {
            PaneAction::Split(dir) => ws.tree.count() < cap && ws.tree.split(dir).is_some(),
            PaneAction::Focus(dir) => ws.tree.focus(dir),
            PaneAction::FocusIndex(index) => ws
                .tree
                .panes()
                .get(index)
                .is_some_and(|&id| ws.tree.set_focus(id)),
            PaneAction::Close => ws.tree.close(),
            PaneAction::Zoom => {
                ws.tree.zoom_toggle();
                true
            }
            PaneAction::Resize(dir) => ws.tree.resize(dir, RESIZE_STEP),
            PaneAction::EvenOut => {
                ws.tree.even_out();
                true
            }
        };
        if changed {
            let ws = self.active_tab_mut();
            ws.selection = None;
            ws.activated = true; // operating on panes means the tab is in use; clear its empty state
            self.sync_layout();
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
    }

    /// Ask the window to repaint, if it exists.
    fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Re-resolve every theme-derived surface from the current `config.appearance.theme`
    /// and repaint (Hard rule 2: switching theme repaints everything live). The ANSI
    /// palette stays a separate concept from the UI tokens; both currently key off the
    /// one theme name. Assumes the config already holds the desired theme.
    fn set_theme_live(&mut self) {
        let name = self.config.appearance.theme.clone();
        self.theme = Theme::resolve(&name);
        self.ansi_palette = AnsiPalette::resolve(&name);
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_theme(&name);
        }
        self.request_redraw();
    }

    /// Switch the active UI theme by name (the palette theme commands). Writes the
    /// config (the source of truth, Hard rule 1) then repaints live.
    fn apply_theme(&mut self, name: &str) {
        if self.config.appearance.theme == name {
            return;
        }
        name.clone_into(&mut self.config.appearance.theme);
        self.set_theme_live();
    }

    /// Open a new tab (its own pane tree + fresh shell) and switch to it.
    fn new_tab(&mut self) {
        self.tabs.push(Tab::new());
        self.active = self.tabs.len() - 1;
        self.selecting = false;
        self.sync_layout(); // spawns the new tab's initial shell, sized to the viewport
        self.request_redraw();
    }

    /// Close the active tab, dropping its shells (each dropped `Terminal` kills its shell).
    /// Closing the **last** tab does not quit (design edge state): it resets to a fresh
    /// empty tab that shows the empty state, so the window always holds at least one tab.
    fn close_tab(&mut self) {
        let count = self.tabs.len();
        if count <= 1 {
            // Never quit: replace the only tab with a pristine one (the old tab drops, so
            // its shells are killed) and show the empty state.
            self.tabs[self.active] = Tab::new();
        } else {
            self.tabs.remove(self.active);
            self.active = index_after_close(self.active, count);
        }
        self.selecting = false;
        // Re-fit the now-visible tab (it may have been sized for an earlier window) and
        // spawn the fresh tab's shell.
        self.sync_layout();
        self.request_redraw();
    }

    /// Switch to the tab at `index` (0-based), if it exists and isn't already active.
    fn goto_tab(&mut self, index: usize) {
        if index < self.tabs.len() && index != self.active {
            self.active = index;
            self.selecting = false;
            // The now-visible tab may have been sized for an earlier window; re-fit it.
            self.sync_layout();
            self.request_redraw();
        }
    }

    /// Cycle to the next (`forward`) or previous tab, wrapping around.
    fn cycle_tab(&mut self, forward: bool) {
        let next = cycle_index(self.active, self.tabs.len(), forward);
        if next != self.active {
            self.active = next;
            self.selecting = false;
            self.sync_layout();
            self.request_redraw();
        }
    }

    /// Dispatch a decoded tab chord to its handler.
    fn run_tab_action(&mut self, action: TabAction) {
        match action {
            TabAction::New => self.new_tab(),
            TabAction::Close => self.close_tab(),
            TabAction::Goto(index) => self.goto_tab(index),
            TabAction::Next => self.cycle_tab(true),
            TabAction::Prev => self.cycle_tab(false),
        }
    }

    /// Handle a key while the command palette is open: it captures all input (typing
    /// filters, arrows navigate, Enter runs, Esc closes). `⌘K` toggles it shut and
    /// `⌘Q` still quits.
    fn on_palette_key(&mut self, event_loop: &ActiveEventLoop, key_event: &KeyEvent) {
        if self.modifiers.super_key() {
            if let Key::Character(ch) = key_event.logical_key.as_ref() {
                if ch.eq_ignore_ascii_case("k") {
                    self.palette.close();
                    self.request_redraw();
                    return;
                }
                if ch.eq_ignore_ascii_case("q") {
                    event_loop.exit();
                    return;
                }
            }
        }
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => self.palette.close(),
            Key::Named(NamedKey::Enter) => {
                let action = self.palette.selected_action();
                self.palette.close();
                if let Some(action) = action {
                    self.run_palette_action(event_loop, action);
                }
            }
            Key::Named(NamedKey::ArrowDown) => self.palette.move_selection(1),
            Key::Named(NamedKey::ArrowUp) => self.palette.move_selection(-1),
            Key::Named(NamedKey::Backspace) => self.palette.backspace(),
            _ => {
                if let Some(text) = key_event.text.as_ref() {
                    for c in text.chars() {
                        if !c.is_control() {
                            self.palette.push_char(c);
                        }
                    }
                }
            }
        }
        self.request_redraw();
    }

    /// Dispatch a chosen palette command to the matching handler.
    fn run_palette_action(&mut self, event_loop: &ActiveEventLoop, action: palette::Action) {
        use palette::Action;
        match action {
            Action::SplitRight => self.apply_pane_action(PaneAction::Split(Dir::Right)),
            Action::SplitDown => self.apply_pane_action(PaneAction::Split(Dir::Down)),
            Action::Zoom => self.apply_pane_action(PaneAction::Zoom),
            Action::EvenOut => self.apply_pane_action(PaneAction::EvenOut),
            Action::ClosePane => self.apply_pane_action(PaneAction::Close),
            Action::FocusLeft => self.apply_pane_action(PaneAction::Focus(Dir::Left)),
            Action::FocusDown => self.apply_pane_action(PaneAction::Focus(Dir::Down)),
            Action::FocusUp => self.apply_pane_action(PaneAction::Focus(Dir::Up)),
            Action::FocusRight => self.apply_pane_action(PaneAction::Focus(Dir::Right)),
            Action::NewTab => self.new_tab(),
            Action::CloseTab => self.close_tab(),
            Action::NextTab => self.cycle_tab(true),
            Action::PrevTab => self.cycle_tab(false),
            Action::ToggleSidebar => self.toggle_sidebar(),
            Action::CycleSidebarMode => self.cycle_sidebar_mode(),
            Action::ShowGitDiff => self.toggle_git_dock(),
            Action::ShowTimeline => self.toggle_timeline(),
            Action::OpenSettings => self.open_settings(),
            Action::ThemeDark => self.apply_theme("ossein-dark"),
            Action::ThemeLight => self.apply_theme("ossein-light"),
            Action::Quit => event_loop.exit(),
        }
    }

    /// Handle a key while the settings view is open: it captures all input. `↑/↓` move
    /// between controls, `←/→` change the focused value, `Tab` / `Shift+Tab` switch
    /// category, `Enter` activates (flip / cycle), `Esc` (or `⌘,`) closes, `⌘Q` quits.
    fn on_settings_key(&mut self, event_loop: &ActiveEventLoop, key_event: &KeyEvent) {
        if self.modifiers.super_key() {
            if let Key::Character(ch) = key_event.logical_key.as_ref() {
                if ch == "," {
                    self.settings.close();
                    self.request_redraw();
                    return;
                }
                if ch.eq_ignore_ascii_case("q") {
                    event_loop.exit();
                    return;
                }
            }
        }
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => {
                self.settings.close();
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.settings.move_selection(-1);
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.settings.move_selection(1);
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if let Some(key) = self.settings.adjust(&mut self.config, -1) {
                    self.apply_setting_change(key);
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                if let Some(key) = self.settings.adjust(&mut self.config, 1) {
                    self.apply_setting_change(key);
                }
            }
            Key::Named(NamedKey::Tab) => {
                self.settings.cycle_category(!self.modifiers.shift_key());
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(key) = self.settings.activate(&mut self.config) {
                    self.apply_setting_change(key);
                }
            }
            _ => {}
        }
    }

    /// Handle a key while the git dock is open. `⇧⌘G` closes and `⌘Q` quits in either
    /// focus; otherwise input routes to the file list or the commit box depending on which
    /// has focus. The terminal stays live underneath but receives no keys until close.
    fn on_gitdock_key(&mut self, event_loop: &ActiveEventLoop, key_event: &KeyEvent) {
        if self.modifiers.super_key() {
            if let Key::Character(ch) = key_event.logical_key.as_ref() {
                if ch.eq_ignore_ascii_case("g") && self.modifiers.shift_key() {
                    self.toggle_git_dock();
                    return;
                }
                if ch.eq_ignore_ascii_case("h") && self.modifiers.shift_key() {
                    // Switch to the timeline dock (the two right docks are exclusive).
                    self.toggle_timeline();
                    return;
                }
                if ch.eq_ignore_ascii_case("q") {
                    event_loop.exit();
                    return;
                }
            }
        }
        if self.git_dock.commit_focused() {
            self.on_commit_key(key_event);
        } else {
            self.on_gitlist_key(key_event);
        }
    }

    /// Keys while the file list has focus: arrows move between files (re-diffing),
    /// `PageUp/PageDown` scroll the diff, `Space` stages/unstages the selected file, `a`
    /// stages everything, `u` undoes the last commit, `Tab` moves to the commit box, and
    /// `Esc` closes the dock.
    fn on_gitlist_key(&mut self, key_event: &KeyEvent) {
        // `⌘↵` stages (or unstages) the focused hunk.
        if self.modifiers.super_key()
            && matches!(key_event.logical_key.as_ref(), Key::Named(NamedKey::Enter))
        {
            self.stage_hunk();
            return;
        }
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => self.toggle_git_dock(),
            // In the no-repo empty state, `Enter` runs the "Init repo" action.
            Key::Named(NamedKey::Enter) if self.git_dock.no_repo() => self.init_repo(),
            Key::Named(NamedKey::Tab) => {
                self.git_dock.focus_commit();
                self.request_redraw();
            }
            // `[` / `]` move the focused hunk (jumping the diff scroll to it).
            Key::Character("[") => {
                self.git_dock.focus_hunk(-1);
                self.request_redraw();
            }
            Key::Character("]") => {
                self.git_dock.focus_hunk(1);
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.git_dock.move_selection(-1) {
                    self.load_selected_diff();
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowDown) => {
                if self.git_dock.move_selection(1) {
                    self.load_selected_diff();
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::PageUp) => {
                self.git_dock.scroll_diff(-DIFF_SCROLL_LINES);
                self.request_redraw();
            }
            Key::Named(NamedKey::PageDown) => {
                self.git_dock.scroll_diff(DIFF_SCROLL_LINES);
                self.request_redraw();
            }
            Key::Named(NamedKey::Space) => self.toggle_stage_selected(),
            Key::Character(ch) if ch.eq_ignore_ascii_case("a") => self.stage_all(),
            Key::Character(ch) if ch.eq_ignore_ascii_case("u") => self.undo_last_commit(),
            _ => {}
        }
    }

    /// Keys while the commit box has focus: printable characters edit the message,
    /// `Backspace` deletes, `Enter` commits (when allowed), `Esc` / `Tab` return to the
    /// file list.
    fn on_commit_key(&mut self, key_event: &KeyEvent) {
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape | NamedKey::Tab) => self.git_dock.focus_list(),
            Key::Named(NamedKey::Enter) => self.commit(),
            Key::Named(NamedKey::Backspace) => self.git_dock.backspace(),
            _ => {
                if let Some(text) = key_event.text.as_ref() {
                    for c in text.chars() {
                        if !c.is_control() {
                            self.git_dock.push_char(c);
                        }
                    }
                }
            }
        }
        self.request_redraw();
    }

    /// Commit the staged changes with the box's message (when allowed), then keep the
    /// short SHA for the Undo hint and reload the status + diff.
    fn commit(&mut self) {
        if !self.git_dock.can_commit() {
            return;
        }
        let Some(repo) = self.git_repo.clone() else {
            return;
        };
        let message = self.git_dock.message().to_owned();
        match repo.commit(&message) {
            Ok(()) => {
                let sha = repo.head_short().unwrap_or_default();
                // Record a restorable timeline event for the commit (first line as title).
                let subject = message.lines().next().unwrap_or("").to_owned();
                let branch = repo
                    .status()
                    .ok()
                    .and_then(|s| s.branch)
                    .unwrap_or_default();
                self.record_event(
                    Actor::Human,
                    format!("git commit - {subject}"),
                    format!("{sha} - {branch}"),
                    (!sha.is_empty()).then(|| sha.clone()),
                );
                self.git_dock.set_committed(sha);
                self.reload_git_status();
            }
            Err(err) => self.git_dock.set_error(err.to_string()),
        }
        self.request_redraw();
    }

    /// Undo the just-made commit (soft reset), if the Undo hint is still showing.
    fn undo_last_commit(&mut self) {
        if self.git_dock.last_commit().is_none() {
            return;
        }
        let Some(repo) = self.git_repo.clone() else {
            return;
        };
        match repo.undo_commit() {
            Ok(()) => {
                self.git_dock.clear_last_commit();
                self.reload_git_status();
            }
            Err(err) => self.git_dock.set_error(err.to_string()),
        }
        self.request_redraw();
    }

    /// Stage (or unstage) the selected file, then reload the status + diff and repaint.
    /// A file that is fully staged toggles back to unstaged; anything else stages.
    fn toggle_stage_selected(&mut self) {
        let Some(repo) = self.git_repo.clone() else {
            return;
        };
        let Some(file) = self.git_dock.selected_file() else {
            return;
        };
        let path = file.path.clone();
        let fully_staged = file.staged && !file.unstaged;
        let result = if fully_staged {
            repo.unstage(&path)
        } else {
            repo.stage(&path)
        };
        match result {
            Ok(()) => {
                let name = path.file_name().map_or_else(
                    || path.to_string_lossy().into_owned(),
                    |n| n.to_string_lossy().into_owned(),
                );
                let verb = if fully_staged { "Unstaged" } else { "Staged" };
                self.record_event(Actor::Human, format!("{verb} {name}"), String::new(), None);
                self.reload_git_status();
            }
            Err(err) => self.git_dock.set_error(err.to_string()),
        }
        self.request_redraw();
    }

    /// Stage (or unstage) the focused hunk of the selected file, then reload and repaint.
    /// The direction follows which diff is shown: staging when viewing the working-tree
    /// diff, unstaging (`--reverse`) when viewing the staged diff.
    fn stage_hunk(&mut self) {
        let Some(repo) = self.git_repo.clone() else {
            return;
        };
        let Some(file) = self.git_dock.selected_file() else {
            return;
        };
        let path = file.path.clone();
        let reverse = self.git_dock.diff_is_staged();
        let Some(hunk) = self.git_dock.focused_hunk().cloned() else {
            return;
        };
        match repo.apply_hunk(&path, &hunk, reverse) {
            Ok(()) => self.reload_git_status(),
            Err(err) => self.git_dock.set_error(err.to_string()),
        }
        self.request_redraw();
    }

    /// Stage every change in the working tree (`git add -A`), then reload and repaint.
    fn stage_all(&mut self) {
        let Some(repo) = self.git_repo.clone() else {
            return;
        };
        match repo.stage_all() {
            Ok(()) => {
                self.record_event(Actor::Human, "Staged all changes", String::new(), None);
                self.reload_git_status();
            }
            Err(err) => self.git_dock.set_error(err.to_string()),
        }
        self.request_redraw();
    }

    /// Reload the working status from the cached repo (after a stage/unstage), keeping the
    /// file selection, then re-diff the selected file. Cheaper than [`Self::refresh_git`]
    /// (no re-discovery).
    fn reload_git_status(&mut self) {
        let Some(repo) = self.git_repo.clone() else {
            return;
        };
        match repo.status() {
            Ok(status) => {
                self.git_dock.load(status);
                self.load_selected_diff();
            }
            Err(err) => self.git_dock.set_error(err.to_string()),
        }
    }

    /// Handle a key while the session-timeline dock is open. `⇧⌘G` switches to the git dock,
    /// `⇧⌘H`/`Esc` close, `⌘Q` quits; `↑/↓` (or `⌥⌘←/→`) scrub events, `⌥⌘0` returns to now.
    /// The terminal stays live underneath but receives no keys until close.
    fn on_timeline_key(&mut self, event_loop: &ActiveEventLoop, key_event: &KeyEvent) {
        if self.modifiers.super_key() {
            if let Key::Character(ch) = key_event.logical_key.as_ref() {
                if ch.eq_ignore_ascii_case("g") && self.modifiers.shift_key() {
                    self.toggle_git_dock();
                    return;
                }
                if ch.eq_ignore_ascii_case("h") && self.modifiers.shift_key() {
                    self.toggle_timeline();
                    return;
                }
                if ch.eq_ignore_ascii_case("q") {
                    event_loop.exit();
                    return;
                }
            }
        }
        // Session bindings: `⌥⌘←/→` step, `⌥⌘0` return to now (the guide's Session & Git
        // shortcuts).
        if self.modifiers.alt_key() && self.modifiers.super_key() {
            match key_event.logical_key.as_ref() {
                Key::Named(NamedKey::ArrowLeft) => {
                    self.timeline_step(-1);
                    return;
                }
                Key::Named(NamedKey::ArrowRight) => {
                    self.timeline_step(1);
                    return;
                }
                Key::Character("0") => {
                    self.timeline_return_to_now();
                    return;
                }
                _ => {}
            }
        }
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => self.toggle_timeline(),
            Key::Named(NamedKey::ArrowUp) => self.timeline_step(-1),
            Key::Named(NamedKey::ArrowDown) => self.timeline_step(1),
            Key::Named(NamedKey::Home) => self.timeline_return_to_now(),
            _ => {}
        }
    }

    /// Move the timeline selection by `delta` and reconcile the shadow worktree to it.
    fn timeline_step(&mut self, delta: i32) {
        if self.timeline.move_selection(delta) {
            self.reconcile_shadow();
        }
        self.request_redraw();
    }

    /// Snap the timeline selection back to now (HEAD), discarding any shadow worktree.
    fn timeline_return_to_now(&mut self) {
        if self.timeline.select_now() {
            self.reconcile_shadow();
        }
        self.request_redraw();
    }

    /// Handle a key press: the palette (when open), platform combos (quit/copy/paste/
    /// palette), pane chords, scrollback keys, then terminal input to the focused pane.
    fn on_key(&mut self, event_loop: &ActiveEventLoop, key_event: &KeyEvent) {
        if key_event.state != ElementState::Pressed {
            return;
        }
        // The settings view, command palette, and git dock each capture input while open.
        if self.settings.open {
            self.on_settings_key(event_loop, key_event);
            return;
        }
        if self.palette.open {
            self.on_palette_key(event_loop, key_event);
            return;
        }
        if self.git_dock.open {
            self.on_gitdock_key(event_loop, key_event);
            return;
        }
        if self.timeline.open {
            self.on_timeline_key(event_loop, key_event);
            return;
        }
        // Tab management (⌘T new, ⌘W close, ⌘1..9 go-to, ⌥⇧[ ] cycle). Matched on the
        // physical key so macOS Option-glyph remapping doesn't interfere.
        if let PhysicalKey::Code(code) = key_event.physical_key {
            if let Some(action) = tab_action(code, self.modifiers) {
                self.run_tab_action(action);
                return;
            }
        }
        // Platform combos (Cmd/Super + K/Q/C/V). The terminal owns every other key -
        // Ctrl+C etc. still reach the shell.
        if self.modifiers.super_key() {
            if let Key::Character(ch) = key_event.logical_key.as_ref() {
                if ch.eq_ignore_ascii_case("k") {
                    self.palette.open();
                    self.request_redraw();
                    return;
                }
                if ch.eq_ignore_ascii_case("q") {
                    event_loop.exit();
                    return;
                }
                if ch.eq_ignore_ascii_case("c") {
                    self.copy_selection();
                    return;
                }
                if ch.eq_ignore_ascii_case("v") {
                    self.paste();
                    return;
                }
                if ch.eq_ignore_ascii_case("b") {
                    if self.modifiers.shift_key() {
                        self.cycle_sidebar_mode();
                    } else {
                        self.toggle_sidebar();
                    }
                    return;
                }
                if ch.eq_ignore_ascii_case("g") && self.modifiers.shift_key() {
                    self.toggle_git_dock();
                    return;
                }
                if ch.eq_ignore_ascii_case("h") && self.modifiers.shift_key() {
                    self.toggle_timeline();
                    return;
                }
                if ch == "," {
                    self.open_settings();
                    return;
                }
            }
        }
        // Pane control (the `⌥` leader chords). Matched on the physical key so macOS
        // Option-key glyph remapping does not get in the way.
        if let PhysicalKey::Code(code) = key_event.physical_key {
            if let Some(action) = pane_action(code, self.modifiers) {
                self.apply_pane_action(action);
                return;
            }
        }
        // Shift + PageUp/PageDown scrolls the focused pane's scrollback.
        if self.modifiers.shift_key() {
            match key_event.logical_key.as_ref() {
                Key::Named(NamedKey::PageUp) => {
                    if let Some(term) = self.focused_term() {
                        term.scroll_page(true);
                    }
                    return;
                }
                Key::Named(NamedKey::PageDown) => {
                    if let Some(term) = self.focused_term() {
                        term.scroll_page(false);
                    }
                    return;
                }
                _ => {}
            }
        }
        self.forward_key_to_focused(key_event);
    }

    /// Route a plain key to the focused pane's shell (the fall-through after every chord).
    /// A dead pane instead restarts on Enter and swallows the rest; a live pane forwards
    /// the bytes, and submitting a command (Enter) retires the tab's empty state.
    fn forward_key_to_focused(&mut self, key_event: &KeyEvent) {
        let is_enter = matches!(key_event.logical_key.as_ref(), Key::Named(NamedKey::Enter));
        // A focused pane whose shell has exited shows the "shell exited" overlay: Enter
        // restarts the shell, and every other key is swallowed (there is no shell to send
        // it to). Pane/tab/app chords were already handled above, so they still work.
        if self.focused_pane_dead() {
            if is_enter {
                self.restart_focused_pane();
            }
            return;
        }
        if let Some(bytes) = key_to_bytes(key_event, self.modifiers) {
            if let Some(term) = self.focused_term() {
                // Typing jumps back to the live prompt.
                term.scroll_to_bottom();
                term.write(&bytes);
            }
            let ws = self.active_tab_mut();
            ws.selection = None; // typing clears the selection
                                 // Running a command (submitting with Enter) retires the empty state (§10.2:
                                 // "chips fade the first time the user runs a command").
            if is_enter {
                ws.activated = true;
            }
        }
    }

    /// Extend the active drag selection to the pointer (a no-op unless a drag is live).
    fn on_cursor_moved(&mut self) {
        if !self.selecting {
            return;
        }
        if let Some((id, _)) = self.active_tab().selection {
            if let Some(rect) = self.pane_rect(id) {
                let (cell_w, cell_h) = self.cell_size();
                let cell = pointer_cell_in(rect, cell_w, cell_h, self.pane_inset(), self.pointer);
                if let Some((_, sel)) = self.active_tab_mut().selection.as_mut() {
                    sel.head = cell;
                }
            }
        }
        self.request_redraw();
    }

    /// Handle a left mouse button press/release: a press either switches tabs (in the
    /// sidebar) or focuses a pane and starts a selection; a release ends the drag and
    /// clears a zero-width (click-only) selection.
    fn on_left_click(&mut self, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if let Some(hit) = self.sidebar_hit() {
                    match hit {
                        sidebar::Hit::Tab(index) => self.goto_tab(index),
                        sidebar::Hit::NewTab => self.new_tab(),
                    }
                } else if let Some((id, rect)) = self.pane_at_pointer() {
                    self.active_tab_mut().tree.set_focus(id);
                    let (cell_w, cell_h) = self.cell_size();
                    let cell =
                        pointer_cell_in(rect, cell_w, cell_h, self.pane_inset(), self.pointer);
                    self.active_tab_mut().selection = Some((
                        id,
                        Selection {
                            anchor: cell,
                            head: cell,
                        },
                    ));
                    self.selecting = true;
                }
            }
            ElementState::Released => {
                self.selecting = false;
                // A click with no drag clears the (single-cell) selection.
                if self
                    .active_tab()
                    .selection
                    .is_some_and(|(_, sel)| sel.anchor == sel.head)
                {
                    self.active_tab_mut().selection = None;
                }
            }
        }
        self.request_redraw();
    }

    /// Scroll the scrollback of the pane under the pointer by a wheel `delta`.
    fn on_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => wheel_lines(f64::from(y)),
            MouseScrollDelta::PixelDelta(pos) => wheel_lines(pos.y / 20.0),
        };
        if lines == 0 {
            return;
        }
        if let Some((id, _)) = self.pane_at_pointer() {
            if let Some(term) = self.active_tab_mut().panes.get_mut(&id) {
                term.scroll_lines(lines);
            }
        }
    }
}

/// Owned per-pane frame data the borrowed [`PaneView`]s point at during a repaint.
struct PaneFrame {
    rect: PxRect,
    origin: (f32, f32),
    rows: Vec<Vec<GridCell>>,
    cursor: (usize, usize),
    selection: Vec<(usize, usize)>,
    focused: bool,
}

/// Owned "shell exited" overlay data the borrowed [`DeadPaneView`]s point at during a
/// repaint (the scrimmed pane rect + its centered exit message).
struct DeadPaneFrame {
    rect: PxRect,
    text_origin: (f32, f32),
    rows: Vec<Vec<GridCell>>,
}

/// Owned command-palette frame data the borrowed [`OverlayView`] points at.
struct PaletteFrame {
    panel: PxRect,
    origin: (f32, f32),
    rows: Vec<Vec<GridCell>>,
    selected_row: Option<usize>,
    caret: (usize, usize),
}

/// Owned sidebar frame data the borrowed [`SidebarView`] points at.
struct SidebarFrame {
    panel: PxRect,
    origin: (f32, f32),
    rows: Vec<Vec<GridCell>>,
    active_row: Option<usize>,
}

/// Owned settings-view frame data the borrowed [`SettingsView`] points at.
struct SettingsFrame {
    panel: PxRect,
    origin: (f32, f32),
    rows: Vec<Vec<GridCell>>,
    nav_cols: usize,
    nav_active_row: Option<usize>,
    selected_row: Option<usize>,
}

/// Owned git-dock frame data the borrowed [`GitDockView`] points at.
struct GitDockFrame {
    panel: PxRect,
    origin: (f32, f32),
    rows: Vec<Vec<GridCell>>,
    selected_file_row: Option<usize>,
    add_rows: Vec<usize>,
    del_rows: Vec<usize>,
    hunk_rows: Vec<usize>,
    focused_hunk_row: Option<usize>,
    caret: Option<(usize, usize)>,
    /// The clamped diff scroll the view actually used, written back to the dock.
    diff_scroll: usize,
}

/// Owned timeline-dock frame data the borrowed [`TimelineView`] points at.
struct TimelineFrame {
    panel: PxRect,
    origin: (f32, f32),
    rows: Vec<Vec<GridCell>>,
    selected_row: Option<usize>,
    viewing_row: Option<usize>,
}

/// The current branch of the process-cwd repo (for the timeline summary), best-effort.
fn current_branch() -> Option<String> {
    let start = std::env::current_dir().ok()?;
    Repo::discover(&start).ok().flatten()?.status().ok()?.branch
}

/// Resolve a terminal cell into a render cell against the active ANSI palette,
/// folding in the palette-dependent SGR effects: *dim* reduces the foreground
/// intensity and *reverse video* swaps foreground and background (using the
/// palette's default background when the cell has none). Bold/italic/underline pass
/// through for the renderer to apply.
fn resolve_cell(cell: &TermCell, palette: &AnsiPalette) -> GridCell {
    let mut fg = resolve_fg(cell.fg, palette);
    let mut bg = resolve_bg(cell.bg, palette);
    if cell.attrs.contains(CellAttrs::DIM) {
        fg = dim(fg);
    }
    if cell.attrs.contains(CellAttrs::INVERSE) {
        let fill = fg;
        fg = bg.unwrap_or_else(|| palette.default_bg());
        bg = Some(fill);
    }
    GridCell {
        c: cell.c,
        fg,
        bg,
        bold: cell.attrs.contains(CellAttrs::BOLD),
        italic: cell.attrs.contains(CellAttrs::ITALIC),
        underline: cell.attrs.contains(CellAttrs::UNDERLINE),
    }
}

/// Resolve a cell's foreground color against the active ANSI palette.
fn resolve_fg(color: CellColor, palette: &AnsiPalette) -> Srgb {
    match color {
        CellColor::Default => palette.default_fg(),
        CellColor::Indexed(index) => palette.indexed(index),
        CellColor::Rgb(r, g, b) => Srgb { r, g, b },
    }
}

/// Resolve a cell's background; the default background gets no fill (`None`).
fn resolve_bg(color: CellColor, palette: &AnsiPalette) -> Option<Srgb> {
    match color {
        CellColor::Default => None,
        CellColor::Indexed(index) => Some(palette.indexed(index)),
        CellColor::Rgb(r, g, b) => Some(Srgb { r, g, b }),
    }
}

/// Reduce a foreground color's intensity to ~60% for the SGR *dim* attribute.
fn dim(c: Srgb) -> Srgb {
    let faint = |v: u8| u8::try_from(u16::from(v) * 3 / 5).unwrap_or(v);
    Srgb {
        r: faint(c.r),
        g: faint(c.g),
        b: faint(c.b),
    }
}

impl ApplicationHandler<Wakeup> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("skelly")
            .with_inner_size(LogicalSize::new(960.0, 600.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                tracing::error!(%err, "failed to create window");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        self.scale = window.scale_factor();
        self.size = (size.width, size.height);
        let renderer = Renderer::new(
            window.clone(),
            size.width,
            size.height,
            self.scale,
            &self.config.appearance,
        );

        self.window = Some(window);
        self.renderer = Some(renderer);
        // Spawn the shell for the initial pane (and size it to the viewport).
        self.sync_layout();
        if self.active_tab().panes.is_empty() {
            tracing::error!("failed to spawn the initial shell");
            event_loop.exit();
            return;
        }
        // Seed the session timeline with its "session started" anchor, so it is the first
        // event even if the user commits before ever opening the timeline (guarded to once).
        self.record_session_start();
        tracing::info!(
            panes = self.active_tab().panes.len(),
            "window, GPU, and shell ready"
        );
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: Wakeup) {
        // New shell output arrived; ask the window to repaint.
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                self.on_key(event_loop, &key_event);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x, position.y);
                self.on_cursor_moved();
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => self.on_left_click(state),
            WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),
            WindowEvent::Resized(size) => {
                self.size = (size.width, size.height);
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                self.sync_layout();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

impl App {
    /// The current cell size `(width, height)` in physical px, or a `1 x 1` fallback
    /// before the renderer exists.
    fn cell_size(&self) -> (f32, f32) {
        self.renderer.as_ref().map_or((1.0, 1.0), |r| {
            let (w, h, _) = r.cell_metrics();
            (w, h)
        })
    }
}

/// Decode a physical key + modifiers into a pane action. Pane control uses `Alt`
/// (`⌥`) as its leader-less modifier, matching the design guide's shown chords
/// (`⌥|` split right, `⌥-` split down, `⌥Z` zoom, `⌥1..⌥8` focus by number), plus
/// `⌥h/j/k/l` directional focus, `⌥⇧h/j/k/l` resize, `⌥w` close, and `⌥=` even out.
/// Returns `None` for anything else (which then reaches the shell).
fn pane_action(code: KeyCode, mods: ModifiersState) -> Option<PaneAction> {
    if !mods.alt_key() {
        return None;
    }
    let shift = mods.shift_key();
    Some(match code {
        KeyCode::Backslash => PaneAction::Split(Dir::Right),
        KeyCode::Minus => PaneAction::Split(Dir::Down),
        KeyCode::Equal => PaneAction::EvenOut,
        KeyCode::KeyZ => PaneAction::Zoom,
        KeyCode::KeyW => PaneAction::Close,
        KeyCode::KeyH if shift => PaneAction::Resize(Dir::Left),
        KeyCode::KeyJ if shift => PaneAction::Resize(Dir::Down),
        KeyCode::KeyK if shift => PaneAction::Resize(Dir::Up),
        KeyCode::KeyL if shift => PaneAction::Resize(Dir::Right),
        KeyCode::KeyH => PaneAction::Focus(Dir::Left),
        KeyCode::KeyJ => PaneAction::Focus(Dir::Down),
        KeyCode::KeyK => PaneAction::Focus(Dir::Up),
        KeyCode::KeyL => PaneAction::Focus(Dir::Right),
        KeyCode::Digit1 => PaneAction::FocusIndex(0),
        KeyCode::Digit2 => PaneAction::FocusIndex(1),
        KeyCode::Digit3 => PaneAction::FocusIndex(2),
        KeyCode::Digit4 => PaneAction::FocusIndex(3),
        KeyCode::Digit5 => PaneAction::FocusIndex(4),
        KeyCode::Digit6 => PaneAction::FocusIndex(5),
        KeyCode::Digit7 => PaneAction::FocusIndex(6),
        KeyCode::Digit8 => PaneAction::FocusIndex(7),
        _ => return None,
    })
}

/// Decode a physical key + modifiers into a tab action. Tab management uses the
/// platform command modifier (`⌘` on macOS, mapped to `Super` here to match the
/// other `⌘` bindings): `⌘T` new tab, `⌘W` close tab, `⌘1..⌘9` jump to the nth tab;
/// plus `⌥⇧[` / `⌥⇧]` to cycle prev / next (the guide's bracket chords). Matched on
/// the physical key. Returns `None` for anything else (which then reaches the shell).
fn tab_action(code: KeyCode, mods: ModifiersState) -> Option<TabAction> {
    if mods.super_key() && !mods.alt_key() {
        return Some(match code {
            KeyCode::KeyT => TabAction::New,
            KeyCode::KeyW => TabAction::Close,
            KeyCode::Digit1 => TabAction::Goto(0),
            KeyCode::Digit2 => TabAction::Goto(1),
            KeyCode::Digit3 => TabAction::Goto(2),
            KeyCode::Digit4 => TabAction::Goto(3),
            KeyCode::Digit5 => TabAction::Goto(4),
            KeyCode::Digit6 => TabAction::Goto(5),
            KeyCode::Digit7 => TabAction::Goto(6),
            KeyCode::Digit8 => TabAction::Goto(7),
            KeyCode::Digit9 => TabAction::Goto(8),
            _ => return None,
        });
    }
    if mods.alt_key() && mods.shift_key() && !mods.super_key() {
        return Some(match code {
            KeyCode::BracketRight => TabAction::Next,
            KeyCode::BracketLeft => TabAction::Prev,
            _ => return None,
        });
    }
    None
}

/// The active-tab index after cycling from `active` among `count` tabs (`forward`
/// advances, otherwise steps back), wrapping around. A lone tab stays put.
fn cycle_index(active: usize, count: usize, forward: bool) -> usize {
    if count <= 1 {
        return active;
    }
    if forward {
        (active + 1) % count
    } else {
        (active + count - 1) % count
    }
}

/// The new active-tab index after closing the tab at `active`, where `count` is the
/// tab count *before* removal (always >= 2 when a tab is actually closed). Clamps into
/// the surviving range so focus lands on a real tab.
fn index_after_close(active: usize, count: usize) -> usize {
    active.min(count.saturating_sub(2))
}

/// Convert a wheel delta (in lines, or approximated from pixels) to a line count.
/// Positive scrolls up into history, matching winit's convention.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a wheel step is a small number of lines"
)]
fn wheel_lines(delta: f64) -> i32 {
    delta.round() as i32
}

/// Cast a DPI scale factor to `f32`. Sub-pixel precision loss is irrelevant here.
#[allow(
    clippy::cast_possible_truncation,
    reason = "DPI scale precision loss is irrelevant for layout"
)]
fn scale32(scale: f64) -> f32 {
    scale as f32
}

/// Cast a pixel dimension to `f32` for layout math.
#[allow(
    clippy::cast_precision_loss,
    reason = "window pixel dimensions are far within f32's exact-integer range"
)]
fn dim_f32(value: u32) -> f32 {
    value as f32
}

/// Cast a pointer position to `f32`.
#[allow(
    clippy::cast_possible_truncation,
    reason = "pointer coordinates are small pixel values"
)]
fn point_f32(pointer: (f64, f64)) -> (f32, f32) {
    (pointer.0 as f32, pointer.1 as f32)
}

/// The `(cols, rows)` that fit a pane rect, given cell metrics and the inner content
/// inset (all physical px). Clamped to a small positive range; `skelly-term` applies
/// its own 2-column floor.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "cols/rows are clamped to a small positive range before the cast"
)]
fn pane_dims(rect: Rect, cell_w: f32, cell_h: f32, inset: f32) -> (u16, u16) {
    let cols = ((rect.w - 2.0 * inset) / cell_w).floor().clamp(1.0, 1000.0) as u16;
    let rows = ((rect.h - 2.0 * inset) / cell_h).floor().clamp(1.0, 1000.0) as u16;
    (cols, rows)
}

/// Map a pointer position (physical px) to a `(column, row)` cell within `rect`'s
/// content area.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pointer and cell metrics are small, non-negative pixel values"
)]
fn pointer_cell_in(
    rect: Rect,
    cell_w: f32,
    cell_h: f32,
    inset: f32,
    pointer: (f64, f64),
) -> (usize, usize) {
    let (px, py) = point_f32(pointer);
    let col = ((px - rect.x - inset) / cell_w).floor().max(0.0) as usize;
    let row = ((py - rect.y - inset) / cell_h).floor().max(0.0) as usize;
    (col, row)
}

/// Order a selection's endpoints in reading order (row-major).
fn order(sel: Selection) -> ((usize, usize), (usize, usize)) {
    let (a, h) = (sel.anchor, sel.head);
    if (a.1, a.0) <= (h.1, h.0) {
        (a, h)
    } else {
        (h, a)
    }
}

/// The cells covered by a linear selection, clamped to a `rows` x `cols` grid.
fn selection_cells(sel: Selection, rows: usize, cols: usize) -> Vec<(usize, usize)> {
    if rows == 0 || cols == 0 {
        return Vec::new();
    }
    let ((start_col, start_row), (end_col, end_row)) = order(sel);
    if start_row >= rows {
        return Vec::new();
    }
    let end_row = end_row.min(rows - 1);
    let mut cells = Vec::new();
    for row in start_row..=end_row {
        let first = if row == start_row { start_col } else { 0 };
        let last = if row == end_row { end_col } else { cols - 1 };
        for col in first..=last.min(cols - 1) {
            cells.push((col, row));
        }
    }
    cells
}

/// The text of a linear selection, row by row (trailing spaces trimmed).
fn selection_text(sel: Selection, cells: &[Vec<TermCell>]) -> String {
    if cells.is_empty() {
        return String::new();
    }
    let ((start_col, start_row), (end_col, end_row)) = order(sel);
    if start_row >= cells.len() {
        return String::new();
    }
    let end_row = end_row.min(cells.len() - 1);
    let mut lines = Vec::new();
    for (row, cols) in cells.iter().enumerate().take(end_row + 1).skip(start_row) {
        let last_col = cols.len().saturating_sub(1);
        let first = if row == start_row { start_col } else { 0 };
        let last = if row == end_row {
            end_col.min(last_col)
        } else {
            last_col
        };
        let text: String = (first..=last)
            .filter_map(|c| cols.get(c).map(|cell| cell.c))
            .collect();
        lines.push(text.trim_end().to_owned());
    }
    lines.join("\n")
}

/// Translate a key press into the bytes a terminal expects, or `None` if it has no
/// terminal representation.
fn key_to_bytes(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    match &event.logical_key {
        Key::Named(NamedKey::Enter) => Some(vec![b'\r']),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
        Key::Named(NamedKey::Tab) => Some(vec![b'\t']),
        Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
        Key::Named(NamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
        Key::Named(NamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
        Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
        Key::Named(NamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
        // Ctrl + letter -> the corresponding control byte (Ctrl+A = 0x01, etc.).
        Key::Character(ch) if modifiers.control_key() => {
            let upper = ch.chars().next()?.to_ascii_uppercase();
            if upper.is_ascii_alphabetic() {
                u8::try_from(upper).ok().map(|byte| vec![byte - b'@'])
            } else {
                event.text.as_ref().map(|text| text.as_bytes().to_vec())
            }
        }
        _ => event.text.as_ref().map(|text| text.as_bytes().to_vec()),
    }
}

/// Initialize `tracing` with an env filter (`SKELLY_LOG`, default `info`), writing
/// structured logs to stderr.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("SKELLY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

#[cfg(test)]
mod tests {
    use super::{
        cycle_index, dim, index_after_close, order, pane_action, pane_dims, pointer_cell_in,
        resolve_cell, selection_cells, selection_text, tab_action, PaneAction, Selection,
        TabAction,
    };
    use skelly_pane::{Dir, Rect};
    use skelly_render::{AnsiPalette, Srgb};
    use skelly_term::{CellAttrs, CellColor, TermCell};
    use winit::keyboard::{KeyCode, ModifiersState};

    fn plain(c: char) -> TermCell {
        TermCell {
            c,
            fg: CellColor::Default,
            bg: CellColor::Default,
            attrs: CellAttrs::empty(),
        }
    }

    fn grid(lines: &[&str]) -> Vec<Vec<TermCell>> {
        lines
            .iter()
            .map(|line| line.chars().map(plain).collect())
            .collect()
    }

    #[test]
    fn single_row_selection_text() {
        let g = grid(&["hello world"]);
        let sel = Selection {
            anchor: (0, 0),
            head: (4, 0),
        };
        assert_eq!(selection_text(sel, &g), "hello");
    }

    #[test]
    fn multi_row_selection_reads_in_order() {
        let g = grid(&["abcde", "fghij", "klmno"]);
        let sel = Selection {
            anchor: (2, 0),
            head: (2, 2),
        };
        assert_eq!(selection_text(sel, &g), "cde\nfghij\nklm");
    }

    #[test]
    fn reversed_endpoints_are_ordered() {
        let g = grid(&["abcde"]);
        let sel = Selection {
            anchor: (4, 0),
            head: (1, 0),
        };
        let (start, end) = order(sel);
        assert_eq!((start, end), ((1, 0), (4, 0)));
        assert_eq!(selection_text(sel, &g), "bcde");
    }

    #[test]
    fn selection_cells_clamp_to_the_grid() {
        let cells = selection_cells(
            Selection {
                anchor: (0, 0),
                head: (100, 100),
            },
            3,
            5,
        );
        assert_eq!(cells.len(), 15); // 3 rows x 5 cols
        assert!(cells.contains(&(4, 2)));
        assert!(!cells.iter().any(|&(c, r)| c >= 5 || r >= 3));
    }

    #[test]
    fn reverse_video_swaps_fg_and_bg() {
        let palette = AnsiPalette::resolve("ossein-dark");
        let mut cell = plain('x');
        cell.fg = CellColor::Indexed(1); // red
        cell.attrs.insert(CellAttrs::INVERSE);
        let resolved = resolve_cell(&cell, &palette);
        // The red foreground becomes the fill; the (defaulted) background becomes the
        // glyph color, drawn as the palette's default background.
        assert_eq!(resolved.bg, Some(palette.indexed(1)));
        assert_eq!(resolved.fg, palette.default_bg());
    }

    #[test]
    fn reverse_video_with_an_explicit_background() {
        let palette = AnsiPalette::resolve("ossein-dark");
        let mut cell = plain('x');
        cell.fg = CellColor::Indexed(2); // green
        cell.bg = CellColor::Indexed(4); // blue
        cell.attrs.insert(CellAttrs::INVERSE);
        let resolved = resolve_cell(&cell, &palette);
        assert_eq!(resolved.fg, palette.indexed(4));
        assert_eq!(resolved.bg, Some(palette.indexed(2)));
    }

    #[test]
    fn dim_darkens_the_foreground() {
        let bright = Srgb {
            r: 200,
            g: 100,
            b: 50,
        };
        let faint = dim(bright);
        assert_eq!(
            faint,
            Srgb {
                r: 120,
                g: 60,
                b: 30
            }
        );
        assert_eq!(dim(Srgb { r: 0, g: 0, b: 0 }), Srgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn attributes_pass_through_to_the_render_cell() {
        let palette = AnsiPalette::resolve("ossein-dark");
        let mut cell = plain('b');
        cell.attrs
            .insert(CellAttrs::BOLD | CellAttrs::ITALIC | CellAttrs::UNDERLINE);
        let resolved = resolve_cell(&cell, &palette);
        assert!(resolved.bold && resolved.italic && resolved.underline);
    }

    // ----- pane wiring geometry + keybindings ---------------------------------

    #[test]
    fn pane_dims_fit_cells_inside_the_inset() {
        // 800 wide, 12px inset each side, 10px cells -> floor(776 / 10) = 77 cols.
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let (cols, rows) = pane_dims(rect, 10.0, 20.0, 12.0);
        assert_eq!(cols, 77);
        assert_eq!(rows, 28); // floor((600 - 24) / 20)
    }

    #[test]
    fn pointer_maps_to_a_cell_relative_to_the_pane_origin() {
        // Pane at (100, 200), 6px inset, 10x20 cells. A pointer 3.5 cells in.
        let rect = Rect::new(100.0, 200.0, 400.0, 300.0);
        let cell = pointer_cell_in(
            rect,
            10.0,
            20.0,
            6.0,
            (100.0 + 6.0 + 35.0, 200.0 + 6.0 + 50.0),
        );
        assert_eq!(cell, (3, 2));
    }

    #[test]
    fn pointer_above_the_content_clamps_to_the_first_cell() {
        let rect = Rect::new(100.0, 200.0, 400.0, 300.0);
        // Pointer in the inset gutter maps to cell (0, 0), never negative.
        let cell = pointer_cell_in(rect, 10.0, 20.0, 6.0, (100.0, 200.0));
        assert_eq!(cell, (0, 0));
    }

    #[test]
    fn alt_chords_decode_to_pane_actions() {
        let alt = ModifiersState::ALT;
        let alt_shift = ModifiersState::ALT | ModifiersState::SHIFT;
        assert_eq!(
            pane_action(KeyCode::Backslash, alt),
            Some(PaneAction::Split(Dir::Right))
        );
        assert_eq!(
            pane_action(KeyCode::Minus, alt),
            Some(PaneAction::Split(Dir::Down))
        );
        assert_eq!(pane_action(KeyCode::KeyZ, alt), Some(PaneAction::Zoom));
        assert_eq!(pane_action(KeyCode::KeyW, alt), Some(PaneAction::Close));
        assert_eq!(
            pane_action(KeyCode::KeyL, alt),
            Some(PaneAction::Focus(Dir::Right))
        );
        assert_eq!(
            pane_action(KeyCode::KeyL, alt_shift),
            Some(PaneAction::Resize(Dir::Right))
        );
        assert_eq!(
            pane_action(KeyCode::Digit3, alt),
            Some(PaneAction::FocusIndex(2))
        );
    }

    #[test]
    fn keys_without_alt_are_not_pane_actions() {
        // Without the Alt leader the key belongs to the shell.
        assert_eq!(pane_action(KeyCode::KeyL, ModifiersState::empty()), None);
        assert_eq!(pane_action(KeyCode::KeyA, ModifiersState::ALT), None);
    }

    // ----- tab management -----------------------------------------------------

    #[test]
    fn super_chords_decode_to_tab_actions() {
        let sup = ModifiersState::SUPER;
        assert_eq!(tab_action(KeyCode::KeyT, sup), Some(TabAction::New));
        assert_eq!(tab_action(KeyCode::KeyW, sup), Some(TabAction::Close));
        assert_eq!(tab_action(KeyCode::Digit1, sup), Some(TabAction::Goto(0)));
        assert_eq!(tab_action(KeyCode::Digit9, sup), Some(TabAction::Goto(8)));
    }

    #[test]
    fn alt_shift_brackets_cycle_tabs() {
        let alt_shift = ModifiersState::ALT | ModifiersState::SHIFT;
        assert_eq!(
            tab_action(KeyCode::BracketRight, alt_shift),
            Some(TabAction::Next)
        );
        assert_eq!(
            tab_action(KeyCode::BracketLeft, alt_shift),
            Some(TabAction::Prev)
        );
        // Brackets without Shift are not tab actions (they reach the shell).
        assert_eq!(tab_action(KeyCode::BracketRight, ModifiersState::ALT), None);
    }

    #[test]
    fn tab_chords_need_the_right_modifiers() {
        // ⌘T needs Super alone; Super+Alt is a different (unbound) chord.
        assert_eq!(tab_action(KeyCode::KeyT, ModifiersState::empty()), None);
        assert_eq!(
            tab_action(KeyCode::KeyT, ModifiersState::SUPER | ModifiersState::ALT),
            None
        );
        // Digit0 has no tab; Super+Digit0 is unbound.
        assert_eq!(tab_action(KeyCode::Digit0, ModifiersState::SUPER), None);
    }

    #[test]
    fn cycle_index_wraps_both_ways() {
        // Three tabs, forward from the last wraps to the first; back from the first
        // wraps to the last.
        assert_eq!(cycle_index(2, 3, true), 0);
        assert_eq!(cycle_index(0, 3, false), 2);
        assert_eq!(cycle_index(0, 3, true), 1);
        assert_eq!(cycle_index(1, 3, false), 0);
        // A lone tab stays put in either direction.
        assert_eq!(cycle_index(0, 1, true), 0);
        assert_eq!(cycle_index(0, 1, false), 0);
    }

    #[test]
    fn index_after_close_lands_on_a_surviving_tab() {
        // Closing the last of three tabs focuses the new last (index 1).
        assert_eq!(index_after_close(2, 3), 1);
        // Closing a middle or first tab keeps the same index (the next tab slides in).
        assert_eq!(index_after_close(1, 3), 1);
        assert_eq!(index_after_close(0, 3), 0);
        // Closing one of two tabs always lands on the sole survivor (index 0).
        assert_eq!(index_after_close(1, 2), 0);
        assert_eq!(index_after_close(0, 2), 0);
    }
}
