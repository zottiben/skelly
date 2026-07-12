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

mod confirm;
mod deadpane;
mod emptystate;
mod gitdock;
mod motion;
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
    SettingsView, SidebarView, Srgb, TextMeasure, Theme, TimelineView,
};
use skelly_session::{Actor, Repo, SessionEvent, ShadowWorktree};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

use confirm::{CloseTarget, Confirm};
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
/// Logical width (px) of the slim icon rail (`⇧⌘B`), per design §08 ("Icon rail 56px").
const RAIL_WIDTH: f32 = 56.0;
/// Logical width (px) of the git diff dock - the guide's default (resizable 360-560 is a
/// later slice, so it is fixed for now).
const GIT_DOCK_WIDTH: f32 = 420.0;
/// Diff lines scrolled per `PageUp`/`PageDown` in the git dock.
const DIFF_SCROLL_LINES: i32 = 10;

/// Event the reader thread sends to wake the UI when a shell produces output.
#[derive(Debug, Clone, Copy)]
struct Wakeup;

fn main() -> anyhow::Result<()> {
    // Hold the log-file guard for the whole run so buffered logs flush on exit; install the
    // panic hook right after so any panic during startup is logged too.
    let _log_guard = init_tracing();
    install_panic_hook();

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
    /// The proportional-text measurer for laying out chrome (the sidebar, and the other
    /// surfaces as they migrate) in the guide's fonts - GPU-free, so hit-testing and
    /// rendering agree on glyph widths. Kept in step with the DPI scale.
    measure: TextMeasure,
    /// The per-repo git diff dock (right dock) state.
    git_dock: GitDock,
    /// The session-timeline dock (right dock; mutually exclusive with the git dock).
    timeline: TimelineDock,
    /// A pending "close with a running job" confirm modal (design §12), if any. While set,
    /// it captures input; `Enter` / a second close-press confirms, `Esc` cancels.
    confirm: Option<Confirm>,
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
    /// The command palette's open / close animation (design §03 motion), live only while it
    /// plays. While any animation is set the event loop polls + redraws each frame; it clears
    /// itself when done (finalizing the close), returning the loop to its idle `Wait`.
    palette_anim: Option<OverlayAnim>,
    /// The "running job" confirm modal's open / close animation - the same overlay tween as
    /// the palette (rise in, fall out); live only while it plays, cleared when it settles.
    confirm_anim: Option<OverlayAnim>,
}

/// A floating overlay's in-flight open or close animation - shared by the command palette and
/// the "running job" confirm modal (both centered cards drawn through the overlay pass). The
/// panel tweens its vertical offset (logical px below its resting spot) from `from` to `to`
/// along `curve` - the open *decelerates* up into place (rise -> 0), the close *accelerates*
/// back down (0 -> fall). `from` is captured from the panel's *current* offset when the
/// animation starts, so an open interrupted by a dismiss (or vice-versa) continues smoothly
/// instead of jumping. `closing` marks the dismiss so [`animating`](App::animating) finalizes
/// the close when it settles; the panel keeps rendering (but a key/click settles it shut at
/// once) meanwhile.
#[derive(Clone, Copy)]
struct OverlayAnim {
    anim: motion::Anim,
    from: f32,
    to: f32,
    curve: motion::Bezier,
    closing: bool,
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
            measure: TextMeasure::new(1.0),
            git_dock: GitDock::new(),
            timeline: TimelineDock::new(),
            confirm: None,
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
            palette_anim: None,
            confirm_anim: None,
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
        let (cell_w, cell_h) = self.cell_size();
        let scale = scale32(self.scale);
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
                let origin = (rect.x + inset, rect.y + inset);
                let rect = PxRect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: rect.h,
                };
                let cols = rows.first().map_or(0, Vec::len);
                let logo = if empty_state && term.exit_status().is_none() {
                    emptystate::overlay_onto(&mut rows, &self.theme);
                    empty_state_logo(origin, cols, rows.len(), cell_w, cell_h, scale)
                } else {
                    None
                };
                let selection = match ws.selection {
                    Some((sid, sel)) if sid == id => selection_cells(sel, rows.len(), cols),
                    _ => Vec::new(),
                };
                Some(PaneFrame {
                    rect,
                    origin,
                    rows,
                    cursor: term.cursor(),
                    selection,
                    focused: id == focused,
                    logo,
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

        let views: Vec<PaneView> = frames.iter().map(PaneFrame::view).collect();

        // Build the dim overlays for any exited panes and the chrome frames, all before
        // the mutable renderer borrow.
        let dead = self.dead_pane_frames();
        let sidebar = self.sidebar.visible().then(|| self.build_sidebar_frame());
        let git_dock = self.git_dock.open.then(|| self.build_git_dock_frame());
        let timeline = self.timeline.open.then(|| self.build_timeline_frame());
        let overlay = self.palette.open.then(|| self.build_palette_frame());
        // The confirm modal reuses the overlay pass; it never coexists with the palette.
        let confirm = (!self.palette.open)
            .then(|| self.build_confirm_frame())
            .flatten();
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
                Some(paint) => renderer.set_sidebar(Some(&SidebarView {
                    panel: paint.panel,
                    quads: &paint.quads,
                    labels: &paint.labels,
                })),
                None => renderer.set_sidebar(None),
            }
            match &git_dock {
                Some(frame) => renderer.set_git_dock(Some(&GitDockView {
                    panel: frame.panel,
                    quads: &frame.quads,
                    labels: &frame.labels,
                })),
                None => renderer.set_git_dock(None),
            }
            match &timeline {
                Some(frame) => renderer.set_timeline(Some(&TimelineView {
                    panel: frame.panel,
                    quads: &frame.quads,
                    labels: &frame.labels,
                })),
                None => renderer.set_timeline(None),
            }
            match (&overlay, &confirm) {
                (Some(frame), _) => renderer.set_overlay(Some(&OverlayView {
                    panel: frame.panel,
                    quads: &frame.quads,
                    labels: &frame.labels,
                })),
                (None, Some(frame)) => renderer.set_overlay(Some(&OverlayView {
                    panel: frame.panel,
                    quads: &frame.quads,
                    labels: &frame.labels,
                })),
                (None, None) => renderer.set_overlay(None),
            }
            match &settings {
                Some(frame) => renderer.set_settings(Some(&SettingsView {
                    panel: frame.panel,
                    nav_divider_x: frame.nav_divider_x,
                    quads: &frame.quads,
                    labels: &frame.labels,
                })),
                None => renderer.set_settings(None),
            }
            if let Err(err) = renderer.render() {
                tracing::error!(%err, "frame render failed");
            }
        }
    }

    /// Lay out the command palette as a centered floating card, sized to its proportional
    /// content, animated by the open/close rise offset.
    fn build_palette_frame(&mut self) -> PaletteFrame {
        let scale = scale32(self.scale);
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        let (mut panel_w, panel_h) = self.palette.natural_size(scale, &mut self.measure);
        panel_w = panel_w.min(surface_w * 0.9);
        let x = ((surface_w - panel_w) / 2.0).max(0.0);
        let max_y = (surface_h - panel_h).max(0.0);
        let rest_y = (surface_h * 0.16).min(max_y);
        let offset = overlay_rise_offset(self.palette_anim, Instant::now(), scale);
        let y = overlay_panel_top(rest_y, offset, max_y);
        let panel = PxRect {
            x,
            y,
            w: panel_w,
            h: panel_h,
        };
        let paint = self
            .palette
            .build(panel, scale, &self.theme, &mut self.measure);
        PaletteFrame {
            panel,
            quads: paint.quads,
            labels: paint.labels,
        }
    }

    /// Lay out the "running job" confirm modal as a centered floating card (like the
    /// palette, but with no input, just a centered message), animated by the rise offset.
    /// Returns `None` when no confirm is pending.
    fn build_confirm_frame(&mut self) -> Option<ConfirmFrame> {
        let confirm = self.confirm.as_ref()?;
        let scale = scale32(self.scale);
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        let (mut panel_w, panel_h) = confirm.natural_size(scale, &mut self.measure);
        panel_w = panel_w.min(surface_w * 0.9);
        let x = ((surface_w - panel_w) / 2.0).max(0.0);
        let max_y = (surface_h - panel_h).max(0.0);
        let rest_y = (surface_h * 0.16).min(max_y);
        let offset = overlay_rise_offset(self.confirm_anim, Instant::now(), scale);
        let y = overlay_panel_top(rest_y, offset, max_y);
        let panel = PxRect {
            x,
            y,
            w: panel_w,
            h: panel_h,
        };
        let labels = confirm.build(panel, scale, &self.theme, &mut self.measure);
        Some(ConfirmFrame {
            panel,
            quads: Vec::new(),
            labels,
        })
    }

    /// Lay out the left sidebar (design §08): a full-height panel of `sidebar.width` (or
    /// the 56px rail), its tab list built as a proportional display list in the guide's
    /// fonts + UI tokens and clipped to the panel.
    fn build_sidebar_frame(&mut self) -> sidebar::Paint {
        let rail = self.sidebar.is_rail();
        let scale = scale32(self.scale);
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w: self.sidebar_width_px(),
            h: dim_f32(self.size.1),
        };
        sidebar::build(
            self.tabs.len(),
            self.active,
            panel,
            rail,
            scale,
            &self.theme,
            &mut self.measure,
        )
    }

    /// Lay out the settings view: a full-window panel, its nav + control grid rendered
    /// in UI tokens and clipped to the window.
    fn build_settings_frame(&mut self) -> SettingsFrame {
        let scale = scale32(self.scale);
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w: dim_f32(self.size.0),
            h: dim_f32(self.size.1),
        };
        let paint = self
            .settings
            .build(panel, scale, &self.config, &self.theme, &mut self.measure);
        SettingsFrame {
            panel,
            nav_divider_x: paint.nav_divider_x,
            quads: paint.quads,
            labels: paint.labels,
        }
    }

    /// Lay out the git dock (design §10.6): a fixed-width panel on the right edge, its
    /// status bar + file list + selected-file diff + commit box as a proportional display
    /// list clipped to the panel.
    fn build_git_dock_frame(&mut self) -> GitDockFrame {
        let scale = scale32(self.scale);
        let dock_w = self.right_dock_width_px();
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        let panel = PxRect {
            x: (surface_w - dock_w).max(0.0),
            y: 0.0,
            w: dock_w,
            h: surface_h,
        };
        let paint = self
            .git_dock
            .build(panel, scale, &self.theme, &mut self.measure);
        GitDockFrame {
            panel,
            quads: paint.quads,
            labels: paint.labels,
            diff_scroll: paint.diff_scroll,
        }
    }

    /// Lay out the session-timeline dock on the right edge (design §10.5): the status
    /// banner + event list + foot as a proportional display list clipped to the panel.
    fn build_timeline_frame(&mut self) -> TimelineFrame {
        let scale = scale32(self.scale);
        let dock_w = self.right_dock_width_px();
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        let panel = PxRect {
            x: (surface_w - dock_w).max(0.0),
            y: 0.0,
            w: dock_w,
            h: surface_h,
        };
        let paint = self
            .timeline
            .build(panel, scale, &self.theme, &mut self.measure);
        TimelineFrame {
            panel,
            quads: paint.quads,
            labels: paint.labels,
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
        let panel_h = dim_f32(self.size.1);
        sidebar::hit(
            self.tabs.len(),
            self.active,
            panel_h,
            scale32(self.scale),
            py,
        )
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

    /// Request closing the focused pane (`⌥w`). If a foreground job is running in it, arm
    /// the confirm modal instead of closing (design §12 "Process running on close"); the
    /// user confirms with `Enter` or a second `⌥w`.
    fn request_close_pane(&mut self) {
        self.request_close(CloseTarget::Pane);
    }

    /// Request closing the active tab (`⌘W`). If any of its panes has a foreground job,
    /// arm the confirm modal instead of closing.
    fn request_close_tab(&mut self) {
        self.request_close(CloseTarget::Tab);
    }

    /// Shared close gate: close immediately when nothing is running, else arm the confirm
    /// modal (with the same rise-in entrance as the palette).
    fn request_close(&mut self, target: CloseTarget) {
        match self.foreground_job_name(target) {
            Some(process) => {
                self.confirm = Some(Confirm::new(target, process));
                let from = overlay_offset_logical(self.confirm_anim).unwrap_or(OVERLAY_RISE);
                self.confirm_anim = Some(OverlayAnim {
                    anim: motion::Anim::start(Instant::now(), motion::BASE),
                    from,
                    to: 0.0,
                    curve: motion::DECELERATE,
                    closing: false,
                });
                self.request_redraw();
            }
            None => self.perform_close(target),
        }
    }

    /// Perform the actual close for `target` (after a confirm, or when nothing is running),
    /// clearing any pending confirm instantly - the close's result takes over at once (like
    /// running a palette command), so there is nothing to animate out.
    fn perform_close(&mut self, target: CloseTarget) {
        self.confirm = None;
        self.confirm_anim = None;
        match target {
            CloseTarget::Pane => self.apply_pane_action(PaneAction::Close),
            CloseTarget::Tab => self.close_tab(),
        }
    }

    /// Dismiss the confirm modal without closing: ease it out (accelerate fall), keeping it
    /// rendered until [`animating`](Self::animating) finalizes the dismissal (or a key/click
    /// settles it shut first). A no-op if it is closed or already animating out.
    fn cancel_confirm(&mut self) {
        if self.confirm.is_some() && !self.confirm_anim.is_some_and(|a| a.closing) {
            let from = overlay_offset_logical(self.confirm_anim).unwrap_or(0.0);
            self.confirm_anim = Some(OverlayAnim {
                anim: motion::Anim::start(Instant::now(), motion::BASE),
                from,
                to: OVERLAY_RISE,
                curve: motion::ACCELERATE,
                closing: true,
            });
            self.request_redraw();
        }
    }

    /// If the confirm modal is mid-dismissal, settle it shut immediately and report `true` (so
    /// the caller can consume the interrupting event) - mirrors
    /// [`settle_palette_close`](Self::settle_palette_close). A key/click during the fade is then
    /// handled as if the modal were already gone; nothing leaks into the still-visible modal.
    fn settle_confirm_close(&mut self) -> bool {
        if self.confirm_anim.is_some_and(|a| a.closing) {
            self.confirm = None;
            self.confirm_anim = None;
            self.request_redraw();
            true
        } else {
            false
        }
    }

    /// A key interrupting either overlay's fade-out (palette or confirm modal): settle whichever
    /// is fading and report whether the key should be *swallowed* rather than routed on to the
    /// pane - `true` for a repeat dismiss gesture (Esc / ⌘K) that only spent the fade. The two
    /// never fade at once, so `||` settles whichever is live.
    fn key_settles_fading_overlay(&mut self, key_event: &KeyEvent) -> bool {
        (self.settle_confirm_close() || self.settle_palette_close())
            && is_palette_dismiss(key_event, self.modifiers)
    }

    /// The name of a foreground job that closing `target` would kill, if any: the focused
    /// pane's shell for a pane close, any pane in the active tab for a tab close. Falls back
    /// to a generic label when the process name can't be read.
    fn foreground_job_name(&self, target: CloseTarget) -> Option<String> {
        let ws = self.active_tab();
        let pid = match target {
            CloseTarget::Pane => ws
                .panes
                .get(&ws.tree.focused())
                .and_then(Terminal::foreground_job_pid),
            CloseTarget::Tab => ws.panes.values().find_map(Terminal::foreground_job_pid),
        }?;
        Some(process_name(pid).unwrap_or_else(|| "a process".to_owned()))
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
            TabAction::Close => self.request_close_tab(),
            TabAction::Goto(index) => self.goto_tab(index),
            TabAction::Next => self.cycle_tab(true),
            TabAction::Prev => self.cycle_tab(false),
        }
    }

    /// Keys while the "running job" confirm modal is up (design §12): `Enter` or a second
    /// press of a close chord (`⌥w` / `⌘W`, the design's "twice" fast path) confirms the
    /// close; `Esc` cancels; anything else is swallowed (the modal is captured).
    fn on_confirm_key(&mut self, key_event: &KeyEvent) {
        let Some(target) = self.confirm.as_ref().map(|c| c.target) else {
            return;
        };
        let close_chord = matches!(key_event.physical_key, PhysicalKey::Code(code)
            if pane_action(code, self.modifiers) == Some(PaneAction::Close)
                || tab_action(code, self.modifiers) == Some(TabAction::Close));
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => self.cancel_confirm(),
            Key::Named(NamedKey::Enter) => self.perform_close(target),
            _ if close_chord => self.perform_close(target),
            _ => {}
        }
    }

    /// Handle a key while the command palette is open: it captures all input (typing
    /// filters, arrows navigate, Enter runs, Esc closes). `⌘K` toggles it shut and
    /// `⌘Q` still quits.
    fn on_palette_key(&mut self, event_loop: &ActiveEventLoop, key_event: &KeyEvent) {
        if self.modifiers.super_key() {
            if let Key::Character(ch) = key_event.logical_key.as_ref() {
                if ch.eq_ignore_ascii_case("k") {
                    self.close_palette();
                    return;
                }
                if ch.eq_ignore_ascii_case("q") {
                    event_loop.exit();
                    return;
                }
            }
        }
        match key_event.logical_key.as_ref() {
            // Dismiss (Esc / ⌘K) eases the palette out; running a command closes it instantly
            // so the command's result (which may itself be an overlay) takes over at once.
            Key::Named(NamedKey::Escape) => self.close_palette(),
            Key::Named(NamedKey::Enter) => {
                let action = self.palette.selected_action();
                self.palette.close();
                self.palette_anim = None;
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
            Action::ClosePane => self.request_close_pane(),
            Action::FocusLeft => self.apply_pane_action(PaneAction::Focus(Dir::Left)),
            Action::FocusDown => self.apply_pane_action(PaneAction::Focus(Dir::Down)),
            Action::FocusUp => self.apply_pane_action(PaneAction::Focus(Dir::Up)),
            Action::FocusRight => self.apply_pane_action(PaneAction::Focus(Dir::Right)),
            Action::NewTab => self.new_tab(),
            Action::CloseTab => self.request_close_tab(),
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
        // A key during either overlay's fade-out settles it shut, then routes as if it were
        // already closed (a repeat dismiss is swallowed, anything else reaches the pane).
        if self.key_settles_fading_overlay(key_event) {
            return;
        }
        // The "running job" confirm modal captures input while fully up (design §12).
        if self.confirm.is_some() {
            self.on_confirm_key(key_event);
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
                    self.open_palette();
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
                // A pane close routes through the confirm gate (design §12); other pane
                // actions apply directly.
                if action == PaneAction::Close {
                    self.request_close_pane();
                } else {
                    self.apply_pane_action(action);
                }
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
        // A click during an overlay's fade-out dismisses it (and is consumed) instead of
        // falling through to the panes behind the still-visible palette / confirm modal.
        if self.settle_confirm_close() || self.settle_palette_close() {
            return;
        }
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
        // A scroll during an overlay's fade-out dismisses it rather than scrolling the pane
        // behind the still-visible palette / confirm modal.
        if self.settle_confirm_close() || self.settle_palette_close() {
            return;
        }
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
    /// The empty-state brand watermark's bounding box, for a pristine tab (design §10.2).
    logo: Option<PxRect>,
}

impl PaneFrame {
    /// Borrow this owned frame as a [`PaneView`] for the renderer / headless capture.
    fn view(&self) -> PaneView<'_> {
        PaneView {
            rect: self.rect,
            origin: self.origin,
            rows: &self.rows,
            cursor: self.cursor,
            selection: &self.selection,
            focused: self.focused,
            logo: self.logo,
        }
    }
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
    quads: Vec<skelly_render::ChromeQuad>,
    labels: Vec<skelly_render::ProseLabel>,
}

/// Owned "running job" confirm-modal frame data the borrowed [`OverlayView`] points at
/// (like [`PaletteFrame`], but with no content quads - just the centered message).
struct ConfirmFrame {
    panel: PxRect,
    quads: Vec<skelly_render::ChromeQuad>,
    labels: Vec<skelly_render::ProseLabel>,
}

/// Owned settings-view frame data the borrowed [`SettingsView`] points at.
struct SettingsFrame {
    panel: PxRect,
    nav_divider_x: f32,
    quads: Vec<skelly_render::ChromeQuad>,
    labels: Vec<skelly_render::ProseLabel>,
}

/// Owned git-dock frame data the borrowed [`GitDockView`] points at.
struct GitDockFrame {
    panel: PxRect,
    quads: Vec<skelly_render::ChromeQuad>,
    labels: Vec<skelly_render::ProseLabel>,
    /// The clamped diff scroll the view actually used, written back to the dock.
    diff_scroll: usize,
}

/// Owned timeline-dock frame data the borrowed [`TimelineView`] points at.
struct TimelineFrame {
    panel: PxRect,
    quads: Vec<skelly_render::ChromeQuad>,
    labels: Vec<skelly_render::ProseLabel>,
}

/// The current branch of the process-cwd repo (for the timeline summary), best-effort.
fn current_branch() -> Option<String> {
    let start = std::env::current_dir().ok()?;
    Repo::discover(&start).ok().flatten()?.status().ok()?.branch
}

/// The command name of process `pid`, for the "process running on close" confirm. Shells
/// `ps -o comm= -p <pid>` (portable across macOS and Linux) and returns just the basename;
/// `None` if the lookup fails. Best-effort - the caller falls back to a generic label.
fn process_name(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout);
    let name = name.trim();
    // `comm` can be a full path on macOS; show just the executable name.
    let base = name.rsplit('/').next().unwrap_or(name).trim();
    (!base.is_empty()).then(|| base.to_owned())
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
        self.measure.set_scale(scale32(self.scale));
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // While any animation is live, poll and repaint each frame so it advances; otherwise
        // idle in `Wait` until the next real event (shell output, input, resize) - a terminal
        // stays silent when nothing is moving.
        if self.animating(Instant::now()) {
            event_loop.set_control_flow(ControlFlow::Poll);
            self.request_redraw();
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
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

    /// Open the command palette with the "enter" rise (design §03 motion) - decelerating up
    /// from `OVERLAY_RISE` below rest (or wherever the panel currently sits) to `0`. The loop
    /// polls until it settles.
    fn open_palette(&mut self) {
        let from = overlay_offset_logical(self.palette_anim).unwrap_or(OVERLAY_RISE);
        self.palette.open();
        self.palette_anim = Some(OverlayAnim {
            anim: motion::Anim::start(Instant::now(), motion::BASE),
            from,
            to: 0.0,
            curve: motion::DECELERATE,
            closing: false,
        });
        self.request_redraw();
    }

    /// Begin dismissing the palette: the "exit" fall accelerates the panel from its current
    /// offset down to `OVERLAY_RISE`, keeping it rendered until [`animating`](Self::animating)
    /// finalizes the actual close when it settles (or a keypress settles it shut first). A
    /// no-op if the palette is closed or already animating out.
    fn close_palette(&mut self) {
        if self.palette.open && !self.palette_anim.is_some_and(|pa| pa.closing) {
            let from = overlay_offset_logical(self.palette_anim).unwrap_or(0.0);
            self.palette_anim = Some(OverlayAnim {
                anim: motion::Anim::start(Instant::now(), motion::BASE),
                from,
                to: OVERLAY_RISE,
                curve: motion::ACCELERATE,
                closing: true,
            });
            self.request_redraw();
        }
    }

    /// If the palette is mid-dismissal, settle it shut immediately and report `true` (so the
    /// caller can consume the interrupting event). A key/click during the fade-out is then
    /// handled as if the palette were already closed - nothing leaks into the still-visible
    /// palette and no second surface layers over it. Requests a repaint so the dismissed
    /// palette leaves the screen even when the interrupting event itself paints nothing.
    fn settle_palette_close(&mut self) -> bool {
        if self.palette_anim.is_some_and(|pa| pa.closing) {
            self.palette.close();
            self.palette_anim = None;
            self.request_redraw();
            true
        } else {
            false
        }
    }

    /// Advance the animation clocks: when an animation finishes, drop it (finalizing a close
    /// dismissal), then report whether any is still live - the signal the event loop uses to
    /// keep polling + repainting versus falling back to idle `Wait`.
    fn animating(&mut self, now: Instant) -> bool {
        if let Some(pa) = self.palette_anim {
            if pa.anim.done(now) {
                self.palette_anim = None;
                if pa.closing {
                    // The exit finished: actually dismiss the palette now.
                    self.palette.close();
                }
                // Paint one final frame at the settled position, in case this last tick
                // landed a hair before the resting frame was drawn. The queued redraw is
                // delivered even under `Wait`, so no extra poll is needed.
                self.request_redraw();
            }
        }
        if let Some(ca) = self.confirm_anim {
            if ca.anim.done(now) {
                self.confirm_anim = None;
                if ca.closing {
                    // The cancel fall finished: drop the modal now.
                    self.confirm = None;
                }
                self.request_redraw();
            }
        }
        self.palette_anim.is_some() || self.confirm_anim.is_some()
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

/// Logical px a floating overlay (palette / confirm modal) travels through as it opens or
/// closes (design §03 motion).
const OVERLAY_RISE: f32 = 10.0;

/// A floating overlay's current vertical offset (physical px) for its open / close animation:
/// the panel's `from`->`to` tween eased along its curve, in logical px, scaled. Returns `0.0`
/// when no animation is playing (the resting panel). Pure, so the eased curves are
/// unit-testable without an `App`.
fn overlay_rise_offset(anim: Option<OverlayAnim>, now: Instant, scale: f32) -> f32 {
    anim.map_or(0.0, |pa| {
        let eased = pa.curve.ease(pa.anim.progress(now));
        (pa.from + (pa.to - pa.from) * eased) * scale
    })
}

/// An overlay panel's current vertical offset in *logical* px (below its resting spot), or
/// `None` when it is not animating - used to start a new tween from where the panel currently
/// sits so open<->close interruptions are continuous.
fn overlay_offset_logical(anim: Option<OverlayAnim>) -> Option<f32> {
    anim.map(|_| overlay_rise_offset(anim, Instant::now(), 1.0))
}

/// The animated top (physical px) of a centered overlay panel: its resting top plus the
/// current rise / fall `offset`, clamped to the on-screen max so the travel never pushes the
/// panel's bottom off a short window (it just shrinks the travel there).
fn overlay_panel_top(rest_y: f32, offset: f32, max_y: f32) -> f32 {
    (rest_y + offset).min(max_y)
}

/// Whether `key_event` (with `mods`) is a palette-dismiss gesture - `Esc` or `⌘K`. A repeat
/// of one that only settled the fading palette is swallowed rather than routed to the shell.
fn is_palette_dismiss(key_event: &KeyEvent, mods: ModifiersState) -> bool {
    match key_event.logical_key.as_ref() {
        Key::Named(NamedKey::Escape) => true,
        Key::Character(c) => mods.super_key() && c.eq_ignore_ascii_case("k"),
        _ => false,
    }
}

/// The empty-state brand watermark's square bounding box for a pristine pane: a
/// `MARK_SIZE` square (physical px) seated `MARK_GAP` above the hint-chip row (design
/// §10.2), so the vector mark and the chip text read as one lockup. It is centered on the
/// *cell grid* (`origin.x + cols·cell_w/2`), not the pane rect - the chips are centered on
/// the grid too, and a pane's content width is rarely an exact multiple of `cell_w`, so
/// centering on the rect would leave the mark up to half a cell off from the chips. `origin`
/// is the pane's cell `(0,0)` top-left; `cols`/`rows_len` the grid dimensions. `None` when
/// the grid is too small to seat the lockup (mirrors [`emptystate::chip_row`]).
#[allow(
    clippy::cast_precision_loss,
    reason = "grid row/column counts are small non-negative values, exact as f32"
)]
fn empty_state_logo(
    origin: (f32, f32),
    cols: usize,
    rows_len: usize,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
) -> Option<PxRect> {
    let chip_row = emptystate::chip_row(rows_len)?;
    let mark = emptystate::MARK_SIZE * scale;
    let gap = emptystate::MARK_GAP * scale;
    let chip_top = origin.1 + chip_row as f32 * cell_h;
    let grid_center_x = origin.0 + cols as f32 * cell_w / 2.0;
    Some(PxRect {
        x: grid_center_x - mark / 2.0,
        y: (chip_top - gap - mark).max(origin.1),
        w: mark,
        h: mark,
    })
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

/// Initialize `tracing` with an env filter (`SKELLY_LOG`, default `info`). Logs go to
/// stderr and, when a state directory is resolvable, are also appended (non-blocking) to a
/// daily-rotating log file for bug reports. Returns the `tracing-appender` worker guard,
/// which the caller must keep alive so buffered logs - including a panic backtrace - are
/// flushed even on an abrupt exit; `None` when only stderr logging is active.
#[must_use]
fn init_tracing() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_env("SKELLY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr_layer = fmt::layer().with_target(false).with_writer(std::io::stderr);

    // A file layer, only when we can resolve + create the state directory. The file gets no
    // ANSI colors (it is not a terminal); any failure degrades to stderr-only logging.
    let (file_layer, guard) = match log_writer() {
        Some((writer, guard)) => {
            let layer = fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(writer);
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    if guard.is_none() {
        tracing::warn!("no writable state directory; logging to stderr only");
    }
    guard
}

/// Open a non-blocking, daily-rotating log appender in Skelly's state directory. `None` if
/// no state directory is resolvable or it cannot be created (so logging falls back to
/// stderr only).
fn log_writer() -> Option<(
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
)> {
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let appender = tracing_appender::rolling::daily(&dir, "skelly.log");
    Some(tracing_appender::non_blocking(appender))
}

/// Skelly's log directory: `$XDG_STATE_HOME/skelly`, falling back to
/// `$HOME/.local/state/skelly` (the XDG base-dir spec's place for logs / state). `None` if
/// neither var is set - mirroring how `skelly-config` resolves its directory.
fn log_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("skelly"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("skelly"),
    )
}

/// Install a panic hook that logs the panic (message, location, thread, and a captured
/// backtrace) at `ERROR` before chaining to the default hook, so a crash is persisted to
/// the log file for a bug report instead of vanishing (playbook §7). Recovering a single
/// panicking pane in-window without tearing down the window is a tracked follow-up.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let location = info
            .location()
            .map_or_else(|| "unknown".to_owned(), ToString::to_string);
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("unnamed");
        tracing::error!(
            location = %location,
            thread = %thread,
            "panic: {}\n{backtrace}",
            panic_message(info.payload()),
        );
        // Chain to the default hook so the familiar stderr message + process handling stay.
        default_hook(info);
    }));
}

/// Extract a human-readable message from a panic payload, which is a `&str` (a literal
/// `panic!`), a `String` (a formatted one), or - rarely - some other boxed value.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cycle_index, dim, empty_state_logo, index_after_close, order, overlay_panel_top,
        overlay_rise_offset, pane_action, pane_dims, panic_message, pointer_cell_in, process_name,
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
    fn empty_state_logo_centers_the_mark_on_the_grid_above_the_chips() {
        // 80x24 grid, cell (0,0) at (12,12), 8x16 cells, scale 1: a roomy grid seats a mark.
        let bounds = empty_state_logo((12.0, 12.0), 80, 24, 8.0, 16.0, 1.0).expect("seats a mark");
        // A MARK_SIZE (56) square at scale 1.
        assert!((bounds.w - 56.0).abs() < 1e-3 && (bounds.h - 56.0).abs() < 1e-3);
        // Centered on the grid content (origin.x + cols*cell_w/2) so it lines up with the
        // grid-centered chips, NOT on the pane rect.
        let grid_center_x = 12.0 + 80.0 * 8.0 / 2.0;
        assert!((bounds.x + bounds.w / 2.0 - grid_center_x).abs() < 1e-3);
        // Seated in the upper half, above the (centered) chip row.
        assert!(bounds.y + bounds.h < 12.0 + 24.0 * 16.0 / 2.0);
    }

    #[test]
    fn empty_state_logo_is_none_when_the_grid_is_too_small() {
        assert!(empty_state_logo((0.0, 0.0), 20, 3, 8.0, 16.0, 1.0).is_none());
    }

    #[test]
    fn overlay_rise_offset_tweens_from_to_along_the_curve() {
        // The shared overlay tween that both the command palette and the confirm modal use to
        // rise in / fall out (design §03 motion).
        use crate::{motion, OverlayAnim};
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let tween = |from: f32, to: f32, curve, dt| {
            overlay_rise_offset(
                Some(OverlayAnim {
                    anim: motion::Anim::start(t0, motion::BASE),
                    from,
                    to,
                    curve,
                    closing: false,
                }),
                t0 + dt,
                2.0,
            )
        };
        let full = super::OVERLAY_RISE * 2.0; // the rise at scale 2 (20px).
                                              // No animation playing: the panel rests, no offset.
        assert!(overlay_rise_offset(None, Instant::now(), 2.0).abs() < 1e-3);
        // Open: decelerate from a full rise down to rest.
        let open = |dt| tween(super::OVERLAY_RISE, 0.0, motion::DECELERATE, dt);
        assert!(
            (open(Duration::ZERO) - full).abs() < 1e-3,
            "open starts lifted"
        );
        assert!(
            open(motion::BASE / 2) < open(Duration::ZERO),
            "open eases down"
        );
        assert!(open(motion::BASE).abs() < 1e-3, "open reaches rest");
        // Close (dismiss / cancel): accelerate from rest down to the full fall offset.
        let close = |dt| tween(0.0, super::OVERLAY_RISE, motion::ACCELERATE, dt);
        assert!(close(Duration::ZERO).abs() < 1e-3, "close starts at rest");
        assert!(
            (close(motion::BASE) - full).abs() < 1e-3,
            "close falls away"
        );
        // An interrupt tween starts from the panel's current offset (no jump): from 5 logical
        // px (10px at scale 2) it begins exactly there.
        assert!((tween(5.0, 0.0, motion::DECELERATE, Duration::ZERO) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn overlay_panel_top_shifts_down_and_clamps_to_a_short_window() {
        // At rest (no offset) the panel sits at its resting top.
        assert!((overlay_panel_top(100.0, 0.0, 500.0) - 100.0).abs() < 1e-3);
        // A rise / fall offset shifts the panel down by exactly that many px.
        assert!((overlay_panel_top(100.0, 20.0, 500.0) - 120.0).abs() < 1e-3);
        // On a short window the offset is clamped so the panel's bottom never runs off-screen.
        assert!((overlay_panel_top(100.0, 20.0, 110.0) - 110.0).abs() < 1e-3);
    }

    #[test]
    fn process_name_reads_a_running_pids_command() {
        // Our own pid resolves to a non-empty command name (the test binary) with no path
        // separators - proving the `ps` invocation + basename trimming work.
        let name = process_name(std::process::id()).expect("own process has a name");
        assert!(!name.is_empty());
        assert!(!name.contains('/'), "name is the basename, not a full path");
    }

    #[test]
    fn panic_message_reads_str_and_string_payloads() {
        // `panic!("literal")` yields a `&str` payload; `panic!("{x}")` yields a `String`.
        let literal: &str = "boom";
        assert_eq!(panic_message(&literal), "boom");
        let formatted: String = "formatted boom".to_owned();
        assert_eq!(panic_message(&formatted), "formatted boom");
        // Anything else is summarized, not lost.
        assert_eq!(panic_message(&42_i32), "non-string panic payload");
    }

    #[test]
    fn panic_hook_logs_the_panic_through_tracing() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        // A `MakeWriter` that appends into a shared buffer, so we can read back what the
        // hook emitted through `tracing` (mirroring what reaches the log file at runtime).
        #[derive(Clone)]
        struct BufWriter(Arc<Mutex<Vec<u8>>>);
        impl Write for BufWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl tracing_subscriber::fmt::MakeWriter<'_> for BufWriter {
            type Writer = BufWriter;
            fn make_writer(&self) -> Self::Writer {
                self.clone()
            }
        }

        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(BufWriter(buf.clone()))
            .with_ansi(false)
            .finish();

        super::install_panic_hook();
        // The global hook fires during the unwind, on this thread, where the scoped
        // subscriber is active - so its `tracing::error!` lands in our buffer.
        let outcome = tracing::subscriber::with_default(subscriber, || {
            std::panic::catch_unwind(|| panic!("boom from the hook test"))
        });
        assert!(outcome.is_err(), "the closure panicked and was caught");

        let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            logged.contains("ERROR"),
            "logged at error level: {logged:?}"
        );
        assert!(
            logged.contains("boom from the hook test"),
            "panic message reached the log: {logged:?}"
        );
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
