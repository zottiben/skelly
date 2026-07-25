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

mod cheatsheet;
mod confirm;
mod contextmenu;
mod deadpane;
mod emptystate;
mod gitdock;
#[cfg(target_os = "macos")]
mod menu;
mod motion;
mod onboarding;
mod palette;
mod session_state;
mod settings;
mod sidebar;
mod statusline;
mod timeline;
mod toast;
mod tooltip;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use skelly_config::{Config, SidebarMode, TabTitle};
use skelly_pane::{Dir, PaneId, PaneTree, Rect};
use skelly_render::{
    AnsiPalette, CursorShape, GitDockView, GridCell, OverlayView, PaneView, ProseLabel, PxRect,
    Renderer, SettingsView, SidebarView, Srgb, TextMeasure, Theme, TimelineView,
};
use skelly_session::{Actor, Repo, SessionEvent, ShadowWorktree, Status, Timeline};
use skelly_term::{CellAttrs, CellColor, ExitStatus, KeyboardMode, TermCell, Terminal};

use confirm::{CloseTarget, Confirm};
use contextmenu::{ContextMenu, MenuAction, MenuContext};
use gitdock::GitDock;
use palette::Palette;
use settings::Settings;
use sidebar::Sidebar;
use timeline::TimelineDock;
use toast::{Toast, ToastKind};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{CursorIcon, Window, WindowId};

/// Logical padding (px) around the whole pane area - the window content margin.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset (px) inside each pane, between its border and its cells.
const PANE_INSET: f32 = 6.0;
/// Logical height (px) of the top control strip that clears the macOS traffic lights
/// (design §08 anatomy #1). The window uses a transparent, full-size-content-view title bar
/// (the standard native-terminal look), so app content reserves this band at the top; it is
/// zero where the platform keeps native decorations.
#[cfg(target_os = "macos")]
const TITLE_STRIP: f32 = 38.0;
/// Logical width (px) spanned by the macOS traffic lights at the top-left. When the sidebar is
/// at least this wide it covers them; when it is narrower (hidden, or the slim rail) the pane
/// viewport must reserve the title strip so the top-left pane never sits under the lights.
const TRAFFIC_LIGHT_WIDTH: f32 = 80.0;
/// The workspace-chip icons (design §08 #2), a curated set of distinct geometric marks assigned
/// to workspaces by position - a stable, recognizable icon per workspace instead of an initial
/// derived from its name (the guide's `P`/`W` were only illustrative).
const WORKSPACE_ICONS: [char; 8] = [
    '\u{25C6}', '\u{25CF}', '\u{25B2}', '\u{25A0}', '\u{2605}', '\u{25C7}', '\u{25CB}', '\u{25A1}',
];
/// One keyboard resize step, as a fraction of the enclosing split's extent.
const RESIZE_STEP: f32 = 0.04;
/// The dock's expand/collapse toggle: a square handle (logical px) that straddles the dock's
/// left edge - half over the dock, half over the terminal - vertically centered, so it reads
/// unmistakably as a "expand this panel" control and sits clear of the header content.
const DOCK_BUTTON_SIZE: f32 = 22.0;
/// Logical width (px) of the slim icon rail (`⇧⌘B`), per design §08 ("Icon rail 56px").
const RAIL_WIDTH: f32 = 56.0;
/// How long a transient toast stays up before it auto-dismisses (design §12).
const TOAST_DURATION: Duration = Duration::from_secs(4);
/// How often the background thread polls the repo's working tree to record edits into the session
/// timeline (design §10.5). Off the UI thread, so the interval is a battery/freshness trade-off.
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(4);
/// How often the background thread re-reads each pane's shell working directory (for the live
/// status-line cwd, the cwd-based tab title, and the active tab's git dock). `cd` doesn't change
/// the shell pid, so the cwd is polled on this interval rather than only when the foreground job
/// changes. The read is a cheap symlink (Linux) or a short `lsof` (macOS), and runs **off the UI
/// thread** ([`start_cwd_poll`](App::start_cwd_poll)) so a slow read never stalls a repaint.
const CWD_POLL_INTERVAL: Duration = Duration::from_millis(700);
/// How long the empty-state mark + hint chips take to fade out once a pristine tab goes live
/// (the design §10.2 "chips fade the first time the user runs a command", here the moment the
/// user starts typing). A gentle fade, so the hand-off from empty state to live terminal reads.
const EMPTY_FADE: Duration = Duration::from_millis(500);
/// The caret blink half-period (design §06 "caret block blinks", steps(1) ~1.1s = 530ms on/off).
/// Only applies when the running program requests a blinking cursor (`DECSCUSR`); a steady cursor
/// (vim normal mode, most shells) never blinks. The phase resets on each keypress.
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);
/// How long the pointer must rest on an icon-only element before its tooltip shows (design §09).
const HOVER_DELAY: Duration = Duration::from_millis(450);
/// Logical width (px) of the git diff dock - the guide's default (resizable 360-560 is a
/// later slice, so it is fixed for now).
const GIT_DOCK_WIDTH: f32 = 420.0;
/// The right dock's resizable width bounds (logical px), per the guide's dims (420 default).
const DOCK_WIDTH_MIN: f32 = 360.0;
const DOCK_WIDTH_MAX: f32 = 560.0;
/// The grab zone (logical px, each side) around the dock's left edge for a resize drag.
const DOCK_GRAB: f32 = 5.0;
/// Diff lines scrolled per `PageUp`/`PageDown` in the git dock.
const DIFF_SCROLL_LINES: i32 = 10;
/// The command palette's width as a fraction of the window (design: ~half-width, centered),
/// floored at the palette's comfortable minimum and capped clear of the window edges.
const PALETTE_WIDTH_FRAC: f32 = 0.5;
/// The command palette's maximum height as a fraction of the window. The fixed input/count/footer
/// chrome is always shown; the results region caps to what is left, so a long list scrolls
/// instead of running off the card (design §10.8).
const PALETTE_MAX_HEIGHT_FRAC: f32 = 0.72;
/// Terminal font-size bounds + reset default (the `⌘=/-/0` bindings, §11), matching the
/// `[appearance] font_size` valid range (8..=32) and its spec default (14).
const MIN_FONT_SIZE: u16 = 8;
const MAX_FONT_SIZE: u16 = 32;
const DEFAULT_FONT_SIZE: u16 = 14;

/// A user event that wakes the UI loop: a shell produced output (needs a repaint), or the git
/// poll thread posted a fresh working-tree status (drained, repaints only if it changed).
#[derive(Debug, Clone, Copy)]
enum Wakeup {
    Shell,
    GitPoll,
    CwdPoll,
}

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
    /// Swap the focused pane with its neighbor in this direction.
    Swap(Dir),
    /// Reset every split to an even 50/50.
    EvenOut,
    /// Cycle the panes through preset layouts (even columns / rows / main-vertical).
    CycleLayout,
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
    /// The focused pane's foreground-job command name (design §10.3: tabs titled by their
    /// running command, e.g. `nvim`), cached with the pid it was read for so `ps` only runs
    /// when the job changes. `None` when the shell is idle at its prompt.
    job_name: Option<String>,
    /// The pid `job_name` was resolved for, so the cache is invalidated on a job change.
    job_pid: Option<u32>,
    /// The empty-state fade-out (design §10.2): started when this pristine tab first goes live
    /// (the user starts typing), it tweens the mark + hint chips from full to gone. `None` when
    /// the tab is still pristine (mark shown solid) or long past its hand-off (nothing drawn).
    empty_fade: Option<motion::Anim>,
    /// Each pane's live working directory (home-collapsed for display), refreshed on a throttle
    /// from the pane's shell process. Backs the status-line cwd (§10.3) and the cwd-based tab
    /// title (§10.3), so both track `cd` instead of showing a stale process-cwd snapshot.
    pane_cwd: HashMap<PaneId, String>,
    /// The cwd-basename tab title captured once, for `[tabs] follow_cwd = false` (title does not
    /// re-title as the pane `cd`s). `None` until first captured; ignored when `follow_cwd` is on.
    frozen_cwd_title: Option<String>,
    /// Whether this tab is pinned to the sidebar's 3-up pinned grid (design §08 #4). Pinned
    /// tabs show as icon tiles above the tab list instead of in it.
    pinned: bool,
    /// A user-set custom name (design §11 `F2` rename); overrides the auto job-name title and
    /// stops it following the running command. `None` = the automatic title.
    custom_title: Option<String>,
    /// The collapsible group this tab belongs to (index into the workspace's `groups`, design
    /// §08 #5), or `None` for an ungrouped top-level tab.
    group: Option<usize>,
}

/// A collapsible tab group (design §08 #5): a named header with a member count that clusters
/// tabs ("Groups map to a working directory or project"). Collapsing hides its children in the
/// sidebar but keeps their shells running. Groups belong to a workspace.
struct TabGroup {
    /// The header name (e.g. `skelly · main`), shown uppercased-ish in a mono label.
    name: String,
    /// Whether the group is collapsed - its member tabs are hidden from the list (their shells
    /// stay alive), leaving just the header + count.
    collapsed: bool,
}

/// The sidebar's ordered tab layout (design §08 #4/#5): the pinned tabs, the unpinned tabs in
/// display order (ungrouped first, then each group's members clustered), and the group spans
/// over that ordered list. All values are real `App.tabs` indices; each span's `[start, len)`
/// ranges over positions in `ordered`.
struct TabLayout {
    pinned: Vec<usize>,
    ordered: Vec<usize>,
    spans: Vec<GroupSpanData>,
}

/// One group's span over the ordered unpinned list: the header name + collapsed flag and the
/// `[start, start + len)` range of member positions within `TabLayout::ordered`.
struct GroupSpanData {
    name: String,
    collapsed: bool,
    start: usize,
    len: usize,
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
            job_name: None,
            job_pid: None,
            empty_fade: None,
            pane_cwd: HashMap::new(),
            frozen_cwd_title: None,
            pinned: false,
            custom_title: None,
            group: None,
        }
    }

    /// Whether this tab should show the empty-state overlay: pristine (no command run yet)
    /// and still a single pane (a split means the user is working, so it clears).
    fn is_empty_state(&self) -> bool {
        !self.activated && self.tree.count() == 1
    }
}

/// A workspace: a named, isolated tab set (design §08 #2 - "Each isolates tabs, cwd & theme").
/// The *active* workspace's tabs + groups live in `App.tabs`/`active`/`groups`; the others are
/// stashed here (their shells keep running in the background), swapped in on switch. cwd
/// isolation falls out of the tab isolation - each workspace's tabs are separate shell processes
/// that own their own cwd. Per-workspace *theme* is an open decision (no config key for it yet;
/// see design/README.md "Per-workspace cwd/theme isolation"), so the UI theme stays global.
struct Workspace {
    /// The display name; its first letter is the sidebar chip glyph.
    name: String,
    /// The stashed `(tabs, active, groups)` for an *inactive* workspace; `None` while it is
    /// active (its tabs/groups are live in `App.tabs`/`active`/`groups`). Each workspace
    /// isolates its own collapsible groups (design §08 #2).
    stash: Option<(Vec<Tab>, usize, Vec<TabGroup>)>,
}

/// One repo's session-timeline state, keyed by repo root in [`App::timelines`]. Edits are
/// working-tree changes, which belong to a repo (not a tab), so tabs sharing a repo share this;
/// a tab that `cd`s across a repo boundary switches which one the dock renders (design §10.5).
#[derive(Default)]
struct RepoTimeline {
    /// The append-only event log for this repo.
    timeline: Timeline,
    /// Repo-relative paths known dirty, the baseline that distinguishes a *session* edit from
    /// pre-session changes.
    tracked_dirty: HashSet<String>,
    /// Whether the per-repo "session started" anchor has been recorded (lazy on first sight).
    started: bool,
    /// Last-known working-tree `(added, removed)` totals - the status-line dirty projection source.
    dirty: Option<(u32, u32)>,
    /// Last-known branch - the status-line branch projection source.
    branch: Option<String>,
}

/// One repo's freshly-polled status, posted from the git-poll thread. `head` (short SHA) is
/// computed on the thread so the UI never shells out for a repo's session-start anchor.
struct RepoStatus {
    status: Status,
    head: Option<String>,
}

/// The active repo's event log from the per-repo map, or the empty fallback outside a repo. A free
/// function (not a method) so a caller borrows only these three fields and can still `&mut` the
/// dock / measurer in the same statement (field-disjoint borrows the borrow checker accepts).
fn active_log<'a>(
    active_root: Option<&std::path::Path>,
    timelines: &'a HashMap<std::path::PathBuf, RepoTimeline>,
    empty: &'a Timeline,
) -> &'a Timeline {
    active_root
        .and_then(|r| timelines.get(r))
        .map_or(empty, |rt| &rt.timeline)
}

/// The set of dirty working-tree paths (repo-relative) in `status` - the timeline's edit baseline.
fn dirty_paths(status: &Status) -> HashSet<String> {
    status
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect()
}

/// The `(added, removed)` line totals across `status`, or `None` when the tree is clean.
fn dirty_totals(status: &Status) -> Option<(u32, u32)> {
    let (a, r) = status
        .files
        .iter()
        .fold((0u32, 0u32), |(a, r), f| (a + f.added, r + f.removed));
    (a > 0 || r > 0).then_some((a, r))
}

/// The timeline edit event's `(title, detail)` for the files `(path, added, removed)` newly dirty
/// since the last poll, or `None` when nothing is new. A single file names itself; several collapse
/// to a count + combined totals (design §10.5).
fn edit_text(newly: &[(String, u32, u32)]) -> Option<(String, String)> {
    match newly {
        [] => None,
        [(path, added, removed)] => {
            let name = path.rsplit('/').next().unwrap_or(path);
            Some((
                format!("Edited {name}"),
                format!("+{added} \u{2212}{removed} \u{b7} {path}"),
            ))
        }
        many => {
            let (a, r) = many
                .iter()
                .fold((0u32, 0u32), |(a, r), (_, add, rem)| (a + add, r + rem));
            Some((
                format!("Edited {} files", many.len()),
                format!("+{a} \u{2212}{r}"),
            ))
        }
    }
}

/// Application state driven by the winit event loop. The window and renderer are
/// `None` until the platform signals `resumed`; the tab list exists from the start.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent UI-mode flags (selecting, leader-pending, cheatsheet, dock-focused)"
)]
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
    /// The active pane's live working directory (home-collapsed), mirrored from its tab's
    /// `pane_cwd` on each status poll - the sidebar group label + the fallback for a pane with
    /// no tracked cwd read it. Per-pane cwds live on each [`Tab`].
    status_cwd: String,
    status_branch: Option<String>,
    /// The process repo's working-tree diff `(added, removed)` lines for the status-line dirty
    /// indicator (§10.3); refreshed at startup + when the git dock refreshes.
    status_dirty: Option<(u32, u32)>,
    status_shell: String,
    /// The pids the background cwd-poll thread should read next cycle (each pane's shell pid),
    /// written cheaply by the UI thread ([`refresh_poll_pids`](App::refresh_poll_pids)) whenever
    /// the pane set changes. The `lsof`/`/proc` reads happen on the thread, never in a repaint.
    poll_pids: Arc<Mutex<Vec<u32>>>,
    /// The latest `pid -> absolute cwd` map the poll thread produced; drained on the UI thread
    /// ([`drain_cwd_poll`](App::drain_cwd_poll)) into each pane's cwd. A pid absent from the map
    /// (a dead shell / failed read) keeps that pane's last known cwd rather than clearing it.
    pending_cwds: Arc<Mutex<HashMap<u32, std::path::PathBuf>>>,
    /// The active tab's focused-pane cwd the git dock last scoped to (absolute), so a tab switch
    /// or `cd` that changes the active repo re-points the open dock at it. `None` until first poll.
    active_cwd: Option<std::path::PathBuf>,
    /// The cwd a split's new pane should start in (design §11, `[panes] split_inherits_cwd`): set
    /// to the split source pane's absolute cwd just before `sync_layout` spawns the new pane, and
    /// consumed (taken) there so only that one spawn inherits it. `None` = spawn in the default
    /// start dir (a fresh tab / workspace / restored pane).
    inherit_cwd: Option<std::path::PathBuf>,
    /// The per-repo git diff dock (right dock) state.
    git_dock: GitDock,
    /// The session-timeline dock (right dock; mutually exclusive with the git dock) - view state
    /// only; it renders the active repo's log from [`timelines`](Self::timelines).
    timeline: TimelineDock,
    /// Per-repo session event logs, keyed by repo root. Every open tab's repo gets one (the git
    /// poll records edits into all of them); the dock + rewind scope to [`active_root`](Self::active_root).
    timelines: HashMap<std::path::PathBuf, RepoTimeline>,
    /// The active tab's focused-pane repo root (`Repo::discover(active_cwd).root()`), or `None`
    /// outside a repo. The dock renders `timelines[active_root]`; rewind + status project from it.
    active_root: Option<std::path::PathBuf>,
    /// An always-empty log the dock renders when the active tab is in no repo (so the view methods
    /// can take a `&Timeline` without a special case).
    empty_timeline: Timeline,
    /// A pending "close with a running job" confirm modal (design §12), if any. While set,
    /// it captures input; `Enter` / a second close-press confirms, `Esc` cancels.
    confirm: Option<Confirm>,
    /// The first-run onboarding modal (design §10.1), shown once on a fresh install (no config
    /// file yet). While set, it captures input; Skip/Start write the config and dismiss it.
    onboarding: Option<onboarding::Onboarding>,
    /// The in-progress rename buffer for the active tab (design §11 `F2`); `Some` = the tab's
    /// sidebar row is an editable field. Typing edits it, `Enter` commits, `Esc` cancels.
    renaming: Option<String>,
    /// A stack of recently-closed tabs' titles, for `⇧⌘T` reopen (design §11) - a fresh tab is
    /// opened carrying the title back (the shell itself can't be resurrected).
    closed_titles: Vec<String>,
    /// Whether the tmux-style pane leader (`[panes] leader`, default `ctrl+a`, §11) was just
    /// pressed, so the next key is a leader pane chord (`hjkl`/`⇧hjkl`/`z`/`x`/`|`/`-`).
    leader_pending: bool,
    /// Whether the keybinding cheatsheet overlay (`⌘/`, §11) is open. While up it captures input
    /// (`Esc` / `⌘/` closes).
    cheatsheet_open: bool,
    /// The scrollback find state (`⌘F`, §11), `Some` while the find bar is open. Captures typing
    /// into the query; `Enter`/`↑↓` navigate matches, `Esc` closes.
    find: Option<FindState>,
    /// The live shadow worktree while rewound to a past state (`None` = at HEAD/now). Its
    /// drop removes the worktree, so returning to now / closing just clears it.
    shadow: Option<ShadowWorktree>,
    /// When the session began, for the timeline's session-relative event times.
    session_start: Instant,
    /// The repository backing the dock (from the process cwd), cached while it is open so
    /// moving the file selection re-diffs without re-discovering.
    git_repo: Option<Repo>,
    clipboard: Option<arboard::Clipboard>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    /// The open tabs of the *active workspace*; `active` indexes the visible one. Always at
    /// least one.
    tabs: Vec<Tab>,
    /// Index of the visible tab in `tabs`.
    active: usize,
    /// The active workspace's collapsible tab groups (design §08 #5); each `Tab.group` indexes
    /// this. Inactive workspaces stash their own. Empty when no groups have been created.
    groups: Vec<TabGroup>,
    /// The workspaces (design §08 #2). The active one's tabs live in `tabs`/`active` above; the
    /// others are stashed in their `Workspace`. Always at least one.
    workspaces: Vec<Workspace>,
    /// Index of the active workspace in `workspaces`.
    active_workspace: usize,
    /// Current surface size in physical px.
    size: (u32, u32),
    scale: f64,
    modifiers: ModifiersState,
    pointer: (f64, f64),
    /// Whether a mouse-drag selection is in progress (in the active tab).
    selecting: bool,
    /// The right dock's width in **logical** px (resizable 360-560, the guide's dims); dragging
    /// its left edge changes it.
    dock_width: f32,
    /// Whether the right dock's left edge is being dragged to resize it.
    dock_resizing: bool,
    /// Whether the sidebar's right edge is being dragged to resize it (design §12: narrowing
    /// past 180 snaps to the rail, past 90 hides it).
    sidebar_resizing: bool,
    /// Whether the open right dock (git diff / timeline) holds keyboard focus. Docks are layers
    /// over a live terminal (Hard rule 4): opening one keeps focus on the panes (so the user can
    /// keep typing); clicking the dock focuses it for its own keyboard controls, clicking a pane
    /// returns focus to the terminal.
    dock_focused: bool,
    /// Whether the open right dock is expanded to full width, overlaying the panes (toggled by
    /// its header button or `⇧⌘F`). The panes keep their normal layout underneath.
    dock_full_width: bool,
    /// The tab index currently being drag-reordered in the sidebar (updated as it moves), or
    /// `None` when no tab drag is in progress.
    dragging_tab: Option<usize>,
    /// Whether the slim icon rail is transiently hover-expanded to the full panel (design §08,
    /// "56px · hover to expand"). Transient pointer state, not a config key: the rail overlays
    /// the panes while expanded, so the terminal never reflows on hover.
    rail_expanded: bool,
    /// The open right-click tab action menu (design §08), or `None` when closed. Reuses the
    /// shared overlay card, anchored at the click; captures keys + clicks while up.
    context_menu: Option<ContextMenu>,
    /// The current transient toast (design §12), or `None`. Non-modal: it auto-dismisses at
    /// `toast_expires` (the loop wakes at that deadline) and never captures input.
    toast: Option<Toast>,
    /// When the current toast disappears (only meaningful while `toast` is `Some`).
    toast_expires: Instant,
    /// The icon-only element the pointer is resting on and when the hover began (design §09
    /// tooltip): once the rest exceeds `HOVER_DELAY` a tooltip with this label shows. `None` when
    /// the pointer is not over a tooltip-able element.
    hover_tip: Option<(String, Instant)>,
    /// Whether the hover tooltip has crossed its delay and is currently drawn (a one-shot latch so
    /// the reveal repaints exactly once, not every idle wake).
    tooltip_visible: bool,
    /// When the caret-blink cycle last reset (each keypress). The blink phase is measured from
    /// here so the cursor is solid right after typing, then blinks while idle (design §06).
    blink_epoch: Instant,
    /// The last-applied blink "off" state, so the loop repaints only when the phase actually
    /// flips (edge-triggered), never in a redraw loop.
    blink_phase: bool,
    /// The latest per-repo working-tree status produced by the background git-poll thread (keyed
    /// by repo root), drained on the UI thread to record timeline edit events for every watched
    /// repo. Empty until the first poll lands.
    pending_status: Arc<Mutex<HashMap<std::path::PathBuf, RepoStatus>>>,
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
            status_cwd: String::new(),
            status_branch: None,
            status_dirty: None,
            status_shell: String::new(),
            poll_pids: Arc::new(Mutex::new(Vec::new())),
            pending_cwds: Arc::new(Mutex::new(HashMap::new())),
            active_cwd: None,
            inherit_cwd: None,
            git_dock: GitDock::new(),
            timeline: TimelineDock::new(),
            timelines: HashMap::new(),
            active_root: None,
            empty_timeline: Timeline::new(),
            confirm: None,
            // Fresh install (no config file yet) -> show the first-run onboarding (design §10.1).
            onboarding: Config::is_first_run().then(onboarding::Onboarding::new),
            renaming: None,
            closed_titles: Vec::new(),
            leader_pending: false,
            cheatsheet_open: false,
            find: None,
            shadow: None,
            session_start: Instant::now(),
            git_repo: None,
            clipboard: arboard::Clipboard::new().ok(),
            window: None,
            renderer: None,
            tabs: vec![Tab::new()],
            active: 0,
            groups: Vec::new(),
            workspaces: vec![Workspace {
                name: "Personal".to_owned(),
                stash: None,
            }],
            active_workspace: 0,
            size: (0, 0),
            scale: 1.0,
            modifiers: ModifiersState::empty(),
            pointer: (0.0, 0.0),
            selecting: false,
            dock_width: GIT_DOCK_WIDTH,
            dock_resizing: false,
            sidebar_resizing: false,
            dock_focused: false,
            dock_full_width: false,
            dragging_tab: None,
            rail_expanded: false,
            context_menu: None,
            toast: None,
            toast_expires: Instant::now(),
            hover_tip: None,
            tooltip_visible: false,
            blink_epoch: Instant::now(),
            blink_phase: false,
            pending_status: Arc::new(Mutex::new(HashMap::new())),
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

    /// The height (physical px) of the top control strip reserved for the macOS traffic
    /// lights (design §08 anatomy #1). Zero on platforms that keep native window decorations,
    /// so their layout is unchanged.
    #[cfg_attr(
        not(target_os = "macos"),
        allow(clippy::unused_self, reason = "the strip is macOS-only; 0 elsewhere")
    )]
    fn content_top(&self) -> f32 {
        #[cfg(target_os = "macos")]
        {
            TITLE_STRIP * scale32(self.scale)
        }
        #[cfg(not(target_os = "macos"))]
        {
            0.0
        }
    }

    /// The pane area within the window: the surface inset by the window margin, by the
    /// sidebar's width on the left when it is shown, and by the git dock's width on the right
    /// when it is open. When a wide sidebar covers the macOS traffic lights the panes fill to
    /// the window top; when the sidebar is hidden or the slim rail (narrower than the lights),
    /// the panes reserve the title strip so the top-left pane never sits under the lights.
    fn viewport_rect(&self) -> Rect {
        let scale = scale32(self.scale);
        let pad = WINDOW_PAD * scale;
        let sidebar = self.sidebar_footprint_px();
        let dock = self.right_dock_width_px();
        let w = dim_f32(self.size.0);
        let h = dim_f32(self.size.1);
        // Reserve the traffic-light strip at the top only when the sidebar doesn't cover it.
        let top = if sidebar < TRAFFIC_LIGHT_WIDTH * scale {
            pad.max(self.content_top())
        } else {
            pad
        };
        Rect::new(
            sidebar + pad,
            top,
            (w - sidebar - dock - 2.0 * pad).max(1.0),
            (h - top - pad).max(1.0),
        )
    }

    /// The sidebar's **painted / hit** width in physical px, or `0.0` when hidden. The panel
    /// occupies the strip `[0, width)`. The slim rail is a fixed 56px regardless of
    /// `sidebar.width` - unless it is hover-expanded, when it paints at the full panel width
    /// (overlaying the panes, design §08 "56px · hover to expand").
    fn sidebar_width_px(&self) -> f32 {
        if !self.sidebar.visible() {
            return 0.0;
        }
        let logical = if self.sidebar_rail_now() {
            RAIL_WIDTH
        } else {
            f32::from(self.config.sidebar.width)
        };
        logical * scale32(self.scale)
    }

    /// The sidebar's **layout footprint** in physical px - the strip reserved from the pane
    /// viewport. A hover-expanded rail overlays the panes, so its footprint stays the slim
    /// rail width and the terminal never reflows on hover; otherwise this equals the painted
    /// width.
    fn sidebar_footprint_px(&self) -> f32 {
        if self.sidebar.is_rail() && self.rail_expanded {
            RAIL_WIDTH * scale32(self.scale)
        } else {
            self.sidebar_width_px()
        }
    }

    /// Whether the sidebar is drawn as the slim icon rail right now: the rail mode, but not
    /// while it is hover-expanded (when it paints as the full panel).
    fn sidebar_rail_now(&self) -> bool {
        self.sidebar.is_rail() && !self.rail_expanded
    }

    /// The right dock's width in physical px, or `0.0` when neither right dock is open. The
    /// git diff dock and the session timeline are mutually exclusive (Hard rule 4) and both
    /// occupy the right strip `[surface_w - width, surface_w)`; the pane viewport ends
    /// before it. Both use the guide's 420px default.
    fn right_dock_width_px(&self) -> f32 {
        if self.git_dock.open || self.timeline.open {
            self.dock_width * scale32(self.scale)
        } else {
            0.0
        }
    }

    /// The open right dock's panel rect (physical px). Normally right-anchored at `dock_width`;
    /// when [`dock_full_width`](Self::dock_full_width) it spans from the sidebar to the right edge,
    /// overlaying the panes. `w = 0` when no dock is open.
    fn dock_panel_rect(&self) -> PxRect {
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        if !(self.git_dock.open || self.timeline.open) {
            return PxRect {
                x: surface_w,
                y: 0.0,
                w: 0.0,
                h: surface_h,
            };
        }
        if self.dock_full_width {
            let x = self.sidebar_footprint_px();
            PxRect {
                x,
                y: 0.0,
                w: (surface_w - x).max(1.0),
                h: surface_h,
            }
        } else {
            let dock_w = self.right_dock_width_px();
            PxRect {
                x: (surface_w - dock_w).max(0.0),
                y: 0.0,
                w: dock_w,
                h: surface_h,
            }
        }
    }

    /// The open dock's *content* rect (physical px): the full panel, but with its top aligned to
    /// the pane viewport (so the header sits level with the terminal content, not pushed down by
    /// the full title strip - the dock is on the right, clear of the traffic lights). The panel's
    /// opaque fill still spans the full height ([`dock_panel_rect`](Self::dock_panel_rect)), so a
    /// full-width dock covers the panes with no sliver; only the content is inset.
    fn dock_content_rect(&self) -> PxRect {
        let panel = self.dock_panel_rect();
        let top = self.viewport_rect().y;
        PxRect {
            x: panel.x,
            y: top,
            w: panel.w,
            h: (panel.y + panel.h - top).max(1.0),
        }
    }

    /// The expand/collapse toggle's rect (physical px), or `None` when no dock is open. It always
    /// **straddles the dock's left edge** (half over the dock, half over whatever is left of it) as
    /// a drawer-handle sitting on the border line, vertically centered on the dock body. When
    /// full-width that edge coincides with the sidebar's right-resize grab, so a drag started in
    /// the toggle's 22px band toggles the dock instead of resizing the sidebar (resize still works
    /// everywhere else on the edge) - the on-the-border look is worth that small overlap.
    fn dock_button_rect(&self) -> Option<PxRect> {
        if !(self.git_dock.open || self.timeline.open) {
            return None;
        }
        let panel = self.dock_panel_rect();
        let content = self.dock_content_rect();
        let size = DOCK_BUTTON_SIZE * scale32(self.scale);
        Some(PxRect {
            x: panel.x - size * 0.5,
            y: content.y + (content.h - size) * 0.5,
            w: size,
            h: size,
        })
    }

    /// The dock layer's label clip (physical px): the panel, widened leftward by the toggle's
    /// overhang so the handle's glyph - which straddles the left edge - isn't clipped away. All
    /// other dock labels start well inside the panel, so the wider clip never lets them bleed.
    fn dock_clip_rect(&self) -> PxRect {
        let panel = self.dock_panel_rect();
        let overhang = DOCK_BUTTON_SIZE * 0.5 * scale32(self.scale);
        PxRect {
            x: panel.x - overhang,
            y: panel.y,
            w: panel.w + overhang,
            h: panel.h,
        }
    }

    /// Whether the pointer is over the open right dock's body (used to give the dock keyboard
    /// focus on click). `false` when no dock is open.
    fn pointer_in_right_dock(&self) -> bool {
        if !(self.git_dock.open || self.timeline.open) {
            return false;
        }
        let panel = self.dock_panel_rect();
        let (px, _) = point_f32(self.pointer);
        px >= panel.x
    }

    /// Whether the pointer sits on the open right dock's left edge (its resize grab zone). Only
    /// the normal (non-full-width) dock is resizable.
    fn on_dock_edge(&self) -> bool {
        if !(self.git_dock.open || self.timeline.open) || self.dock_full_width {
            return false;
        }
        let (px, _) = point_f32(self.pointer);
        let edge = dim_f32(self.size.0) - self.right_dock_width_px();
        (px - edge).abs() <= DOCK_GRAB * scale32(self.scale)
    }

    /// Resize the right dock from the current pointer x (dragging its left edge), clamped to the
    /// guide's 360-560 range, and re-fit the panes to the new viewport.
    fn resize_dock_to_pointer(&mut self) {
        let scale = scale32(self.scale);
        let (px, _) = point_f32(self.pointer);
        let width = (dim_f32(self.size.0) - px) / scale;
        self.dock_width = width.clamp(DOCK_WIDTH_MIN, DOCK_WIDTH_MAX);
        self.sync_layout();
        self.request_redraw();
    }

    /// Whether the pointer sits on the full sidebar's right edge (its resize grab zone). Only the
    /// full panel (Fixed) is drag-resizable; the rail is widened via `⇧⌘B`.
    fn on_sidebar_edge(&self) -> bool {
        if self.sidebar.mode() != SidebarMode::Fixed {
            return false;
        }
        let (px, _) = point_f32(self.pointer);
        (px - self.sidebar_width_px()).abs() <= DOCK_GRAB * scale32(self.scale)
    }

    /// Resize the sidebar from the current pointer x (dragging its right edge), applying the
    /// guide's §12 snap thresholds: >=180 logical stays the full panel at that width (<=360);
    /// 90..180 snaps to the slim rail (Autohide); <90 hides it (`⌘B` restores). Updates the
    /// in-memory config; [`end_sidebar_resize`](Self::end_sidebar_resize) persists once on release.
    fn resize_sidebar_to_pointer(&mut self) {
        let (px, _) = point_f32(self.pointer);
        let target = px / scale32(self.scale);
        let mode = if target < 90.0 {
            SidebarMode::Hidden
        } else if target < 180.0 {
            SidebarMode::Autohide
        } else {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "target is a guarded, in-range positive width"
            )]
            let width = (target.round() as u16).min(360);
            self.config.sidebar.width = width;
            SidebarMode::Fixed
        };
        self.sidebar.set_mode(mode);
        self.config.sidebar.mode = mode;
        self.rail_expanded = false;
        self.sync_layout();
        self.request_redraw();
    }

    /// Finish a sidebar resize drag: persist the settled width + mode once (the file is the
    /// source of truth, Hard rule 1; per-frame writes during the drag are avoided).
    fn end_sidebar_resize(&mut self) {
        self.sidebar_resizing = false;
        self.persist_config();
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

    /// The focused pane's terminal, borrowed immutably.
    fn focused_term_ref(&self) -> Option<&Terminal> {
        let ws = self.active_tab();
        ws.panes.get(&ws.tree.focused())
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
        // Reserve the status-line strip only when it is shown (design §10.6 toggle).
        let status_h = if self.config.appearance.show_status_line {
            statusline::HEIGHT * scale32(self.scale)
        } else {
            0.0
        };
        let viewport = self.viewport_rect();
        let proxy = self.proxy.clone();
        let shell = self.config.shell.program.clone();
        let cursor = config_cursor_shape(self.config.appearance.cursor);
        // The cwd a newly-spawned pane inherits this cycle (a split from a pane in that dir);
        // taken so it applies once, then reverts to the default start dir.
        let inherit = std::mem::take(&mut self.inherit_cwd);
        let layout = self.active_tab().tree.layout(viewport);

        let ws = self.active_tab_mut();
        // Drop shells for panes no longer in the tree (closed panes). Hidden-by-zoom
        // panes stay, since `tree.panes()` still lists them.
        let live: HashSet<PaneId> = ws.tree.panes().into_iter().collect();
        ws.panes.retain(|id, _| live.contains(id));
        ws.dims.retain(|id, _| live.contains(id));

        for (id, rect) in layout {
            let target = pane_dims(rect, cell_w, cell_h, inset, status_h);
            if let Some(term) = ws.panes.get_mut(&id) {
                // Existing pane: resize only when its grid size actually changed.
                if ws.dims.get(&id) != Some(&target) {
                    term.resize(target.0, target.1);
                    ws.dims.insert(id, target);
                }
            } else {
                let proxy = proxy.clone();
                match Terminal::spawn_shell_in(
                    target.0,
                    target.1,
                    Some(shell.as_str()),
                    inherit.as_deref(),
                    move || {
                        let _ = proxy.send_event(Wakeup::Shell);
                    },
                ) {
                    Ok(term) => {
                        // Apply the configured default cursor shape (appearance.cursor) to the
                        // fresh shell; a program's DECSCUSR still overrides it live.
                        term.set_default_cursor_shape(cursor);
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
        // The pane set may have changed (spawned/closed shells); republish pids to the poll thread.
        self.refresh_poll_pids();
    }

    /// Build the owned per-pane frame data for the active tab: each visible pane's
    /// resolved cell grid, cursor, selection, rectangle, and focus flag.
    fn pane_frames(&self) -> Vec<PaneFrame> {
        let inset = self.pane_inset();
        let scale = scale32(self.scale);
        let viewport = self.viewport_rect();
        let blink_off = self.cursor_blink_off(Instant::now());
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
                let rows: Vec<Vec<GridCell>> = term
                    .cells()
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|c| {
                                resolve_cell(
                                    c,
                                    &self.ansi_palette,
                                    self.config.appearance.bold_is_bright,
                                )
                            })
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
                // A pristine tab paints the vertebra mark (here) + hint chips (built as a
                // proportional pane-overlay in `redraw`) over its blank grid.
                let logo = if empty_state && term.exit_status().is_none() {
                    emptystate::logo_bounds(rect, scale)
                } else {
                    None
                };
                let selection = match ws.selection {
                    Some((sid, sel)) if sid == id => selection_cells(sel, rows.len(), cols),
                    _ => Vec::new(),
                };
                // Blink the caret only in the focused pane, only when the program requested a
                // blinking cursor, and only in the "off" half of the cycle (design §06).
                let is_focused = id == focused;
                let cursor_shape = if is_focused && blink_off && term.cursor_blinking() {
                    CursorShape::Hidden
                } else {
                    render_cursor_shape(term.cursor_shape())
                };
                Some(PaneFrame {
                    rect,
                    origin,
                    rows,
                    cursor: term.cursor(),
                    cursor_shape,
                    selection,
                    focused: is_focused,
                    logo,
                })
            })
            .collect()
    }

    /// The opacity to paint the active tab's empty-state mark + hint chips at, or `None` when it
    /// shows no empty state: `1.0` while the tab is still pristine, then an eased fade to `0` over
    /// [`EMPTY_FADE`] once it goes live (the user starts typing). Shared by the paint and the
    /// animation loop so the two agree on when the hand-off is over.
    fn empty_alpha(&self, now: Instant) -> Option<f32> {
        let ws = self.active_tab();
        if ws.is_empty_state() {
            return Some(1.0);
        }
        let fade = ws.empty_fade?;
        (!fade.done(now)).then(|| 1.0 - motion::ACCELERATE.ease(fade.progress(now)))
    }

    /// Paint the empty-state mark + hint chips into the pane-overlay display list at `alpha`
    /// opacity, seated in pane `rect`. While `pristine` the mark is the watermark drawn *behind*
    /// the glyphs (via `PaneView::logo`), so only the chips are added here; during the fade the
    /// tab is no longer pristine, so the fading mark is drawn here too. `alpha < 1.0` dissolves
    /// the lockup: each pill's alpha scales and each label blends toward the pane background.
    fn push_empty_state(
        &mut self,
        quads: &mut Vec<skelly_render::ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        rect: PxRect,
        alpha: f32,
        pristine: bool,
        scale: f32,
    ) {
        let Some(logo) = emptystate::logo_bounds(rect, scale) else {
            return;
        };
        if !pristine {
            quads.extend(skelly_render::logo_chrome_quads(
                logo.x,
                logo.y,
                logo.w,
                &self.theme,
                skelly_render::LOGO_WATERMARK_OPACITY * alpha,
            ));
        }
        let (mut q, mut l) =
            emptystate::chips_paint(logo, rect, scale, &self.theme, &mut self.measure);
        if alpha < 1.0 {
            let bg = self.theme.bg_base.to_srgb();
            for quad in &mut q {
                quad.alpha *= alpha;
            }
            for label in &mut l {
                label.color = label.color.over(bg, alpha);
            }
        }
        quads.extend(q);
        labels.extend(l);
    }

    /// Build the pane-level overlay display list drawn above the terminal text: a `bg.base`
    /// scrim + centered "shell exited" message for each pane whose shell ended, and - on a
    /// pristine single-pane tab - the empty-state hint chips beneath the vertebra mark. Empty
    /// while every pane is a live shell (the common case), so it costs nothing then.
    #[allow(
        clippy::type_complexity,
        reason = "a local per-pane (rect, exit, cursor, editor-mode) collection"
    )]
    fn pane_overlay_paint(
        &mut self,
    ) -> (Vec<PxRect>, Vec<skelly_render::ChromeQuad>, Vec<ProseLabel>) {
        let scale = scale32(self.scale);
        let viewport = self.viewport_rect();
        let ws = &self.tabs[self.active];
        let empty_state = ws.is_empty_state();
        let focused_id = ws.tree.focused();
        // The editor mode for the focused pane (design §10.4), when its foreground process is a
        // modal editor. `job_name` is the cached focused-pane process (refreshed each redraw);
        // the shape is the real DECSCUSR signal the editor sets, so this is not a guess.
        let job = ws.job_name.as_deref();
        // The focused pane's rect, captured for the `⌘F` find-match highlight.
        let mut focused_rect = None;
        // Collect the pane rects + exit statuses + cursor position/shape + focus first, so the
        // tab borrow ends before the measurer's mutable borrow.
        let panes: Vec<(
            PxRect,
            Option<ExitStatus>,
            (usize, usize),
            Option<&'static str>,
            Option<&'static str>,
            Option<String>,
        )> = ws
            .tree
            .layout(viewport)
            .into_iter()
            .filter_map(|(id, rect)| {
                let term = ws.panes.get(&id)?;
                let px = PxRect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: rect.h,
                };
                if id == focused_id {
                    focused_rect = Some(px);
                }
                // Only the focused pane surfaces an editor mode + filetype (its job is tracked).
                let (mode, filetype) = if id == focused_id {
                    (
                        editor_mode(job, term.cursor_shape()),
                        editor_filetype(job, term.title().as_deref()),
                    )
                } else {
                    (None, None)
                };
                // This pane's live working directory (from the throttled status poll), owned so
                // the tab borrow can end before the measurer's mutable borrow below.
                let cwd = ws.pane_cwd.get(&id).cloned();
                Some((px, term.exit_status(), term.cursor(), mode, filetype, cwd))
            })
            .collect();

        let mut scrims = Vec::new();
        let mut quads = Vec::new();
        let mut labels = Vec::new();
        for (rect, status, cursor, mode, filetype, cwd) in &panes {
            if let Some(status) = status {
                scrims.push(*rect);
                labels.extend(deadpane::message_labels(
                    status,
                    *rect,
                    scale,
                    &self.theme,
                    &mut self.measure,
                ));
            } else if !empty_state && self.config.appearance.show_status_line {
                // A live, in-use pane: seat its status line along the bottom (guide §08 anatomy
                // #9), unless the user hid it (§10.6). The pristine empty state (§10.2) shows
                // only the mark + chips, no strip.
                let (q, l) = statusline::paint(
                    &statusline::Info {
                        // A pane with no polled cwd yet (just split) shows a neutral `~` rather
                        // than borrowing the active pane's path (which would be wrong for it).
                        cwd: cwd.as_deref().unwrap_or("~"),
                        branch: self.status_branch.as_deref(),
                        dirty: self.status_dirty,
                        mode: *mode,
                        filetype: *filetype,
                        shell: &self.status_shell,
                        cursor: *cursor,
                    },
                    *rect,
                    scale,
                    &self.theme,
                    &mut self.measure,
                );
                quads.extend(q);
                labels.extend(l);
            }
        }
        // The empty state (design §10.2): a pristine single-pane tab shows the mark + hint chips,
        // which fade out together once the tab goes live (`empty_alpha` drives the fade).
        if let Some(alpha) = self.empty_alpha(Instant::now()) {
            if scrims.is_empty() {
                if let Some((rect, None, _, _, _, _)) = panes.first() {
                    self.push_empty_state(
                        &mut quads,
                        &mut labels,
                        *rect,
                        alpha,
                        empty_state,
                        scale,
                    );
                }
            }
        }
        // The scrollback find bar + match highlight (design §11 `⌘F`).
        if let Some((query, hit, searched)) = self
            .find
            .as_ref()
            .map(|f| (f.query.clone(), f.hit, f.searched))
        {
            self.push_find_overlay(&mut quads, &mut labels, focused_rect, &query, hit, searched);
        }
        (scrims, quads, labels)
    }

    /// Draw the find-match highlight over the focused pane and the find bar along the window
    /// bottom (design §11 `⌘F`): `Find: <query>` with a match / no-match status.
    fn push_find_overlay(
        &mut self,
        quads: &mut Vec<skelly_render::ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        focused_rect: Option<PxRect>,
        query: &str,
        hit: Option<skelly_term::FindHit>,
        searched: bool,
    ) {
        use skelly_render::{ChromeQuad, FontRole};
        let scale = scale32(self.scale);
        // The match highlight: an accent tint over the matched cells in the focused pane.
        if let (Some(rect), Some(hit)) = (focused_rect, hit) {
            let (cell_w, cell_h) = self.cell_size();
            let inset = self.pane_inset();
            #[allow(
                clippy::cast_precision_loss,
                reason = "cell col/row are small grid coordinates"
            )]
            let (col, row, len) = (hit.col as f32, hit.row as f32, hit.len.max(1) as f32);
            quads.push(ChromeQuad::tint(
                PxRect {
                    x: rect.x + inset + col * cell_w,
                    y: rect.y + inset + row * cell_h,
                    w: len * cell_w,
                    h: cell_h,
                },
                self.theme.accent,
                0.4,
                0.0,
            ));
        }
        // The find bar: a bg.inset strip along the window bottom with a border.subtle top
        // hairline, `⌕ Find: <query>` on the left, and a match status on the right.
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        let bar_h = statusline::HEIGHT * scale;
        let top = surface_h - bar_h;
        quads.push(ChromeQuad::fill(
            PxRect {
                x: 0.0,
                y: top,
                w: surface_w,
                h: bar_h,
            },
            self.theme.bg_inset,
        ));
        quads.push(ChromeQuad::fill(
            PxRect {
                x: 0.0,
                y: top,
                w: surface_w,
                h: scale.max(1.0),
            },
            self.theme.accent,
        ));
        let pad = 14.0 * scale;
        let line = self.measure.line_height(FontRole::Mono);
        let cy = top + (bar_h - line) * 0.5;
        labels.push(ProseLabel {
            text: format!("\u{2315} find: {query}"),
            x: pad,
            y: cy,
            role: FontRole::Mono,
            color: self.theme.fg_primary,
            weight: None,
            max_w: f32::MAX,
        });
        let status = if hit.is_some() {
            "match".to_owned()
        } else if searched {
            "no match".to_owned()
        } else {
            "\u{2191}\u{2193} next / prev  \u{21a9} search  esc close".to_owned()
        };
        let sw = self.measure.width(&status, FontRole::Mono, None);
        labels.push(ProseLabel {
            text: status,
            x: surface_w - pad - sw,
            y: cy,
            role: FontRole::Mono,
            color: if hit.is_none() && searched {
                self.theme.diff_del
            } else {
                self.theme.fg_muted
            },
            weight: None,
            max_w: f32::MAX,
        });
    }

    /// Publish the current panes' shell pids for the background cwd-poll thread to read next
    /// cycle. Cheap (no subprocess) - just collects pids under a short lock, so it can run on the
    /// UI thread whenever the pane set changes (e.g. from [`sync_layout`](Self::sync_layout)).
    fn refresh_poll_pids(&self) {
        let pids: Vec<u32> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.values())
            .filter_map(Terminal::shell_pid)
            .collect();
        if let Ok(mut guard) = self.poll_pids.lock() {
            *guard = pids;
        }
    }

    /// Start the background thread that reads each pane's shell working directory (for the live
    /// status-line cwd, cwd-based tab titles, and the active git dock). It reads the pids the UI
    /// published in [`poll_pids`](Self::poll_pids), does the `lsof`/`/proc` reads **off the UI
    /// thread**, stores the results in [`pending_cwds`](Self::pending_cwds), and wakes the loop via
    /// `Wakeup::CwdPoll` - so a slow read (e.g. a hung mount) can never stall a repaint.
    fn start_cwd_poll(&mut self) {
        let pids_slot = Arc::clone(&self.poll_pids);
        let cwds_slot = Arc::clone(&self.pending_cwds);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(CWD_POLL_INTERVAL);
            let pids: Vec<u32> = pids_slot.lock().map(|g| g.clone()).unwrap_or_default();
            if pids.is_empty() {
                continue;
            }
            let results: HashMap<u32, std::path::PathBuf> = pids
                .into_iter()
                .filter_map(|pid| process_cwd(pid).map(|dir| (pid, dir)))
                .collect();
            if let Ok(mut guard) = cwds_slot.lock() {
                *guard = results;
            }
            if proxy.send_event(Wakeup::CwdPoll).is_err() {
                break; // the event loop is gone; stop polling.
            }
        });
    }

    /// Apply the latest `pid -> cwd` map from the poll thread to each pane's tracked cwd, and
    /// follow the active tab's repo. A pid missing from the map (dead shell / failed read) keeps
    /// that pane's last known cwd - so a transient miss never blanks the status line or re-scopes
    /// the git dock to the launch dir (Hard rule 3: rewind stays put, so we also skip while
    /// rewound). Repaints only when a cwd actually changed, so idle polls stay free.
    fn drain_cwd_poll(&mut self) {
        let cwds = match self.pending_cwds.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => return,
        };
        let active_focused = (self.active, self.active_tab().tree.focused());
        let mut active_abs: Option<std::path::PathBuf> = None;
        let mut changed = false;
        for (ti, tab) in self.tabs.iter_mut().enumerate() {
            let live: HashSet<PaneId> = tab.tree.panes().into_iter().collect();
            tab.pane_cwd.retain(|id, _| live.contains(id));
            for (&id, term) in &tab.panes {
                if let Some(dir) = term.shell_pid().and_then(|pid| cwds.get(&pid)) {
                    if (ti, id) == active_focused {
                        active_abs = Some(dir.clone());
                    }
                    let display = home_relative(dir);
                    if tab.pane_cwd.get(&id) != Some(&display) {
                        changed = true;
                        tab.pane_cwd.insert(id, display);
                    }
                }
            }
        }
        // Mirror the active pane's cwd to the app-level field (sidebar group label + fallback), and
        // follow the active repo when the focused pane's cwd changes (a `cd`). Only a successful
        // (Some) read updates these - a None keeps the last known repo.
        if let Some(dir) = active_abs {
            self.status_cwd = home_relative(&dir);
            if self.active_cwd.as_ref() != Some(&dir) {
                self.active_cwd = Some(dir);
                self.rescope_active();
            }
        }
        if changed {
            self.request_redraw();
        }
    }

    /// Follow the active tab's focused-pane repo: recompute `active_root` from `active_cwd`; if it
    /// changed, discard any rewind (decision 2: a tab switch / cross-repo `cd` returns to now -
    /// Hard rule 3 keeps HEAD/refs untouched), reset the timeline cursor to the new repo's now,
    /// re-project the status line, and re-scope the git dock. Idempotent - a no-op when the active
    /// repo is unchanged (a within-repo `cd` keeps any rewind), so it is safe to over-call from
    /// every tab/focus/workspace switch.
    fn rescope_active(&mut self) {
        // Adopt the active focused pane's cwd from the already-polled cache, so a tab / focus switch
        // follows immediately (the cwd poll's own drain also calls this after updating active_cwd).
        let pid = {
            let ws = self.active_tab();
            ws.panes
                .get(&ws.tree.focused())
                .and_then(Terminal::shell_pid)
        };
        if let Some(dir) = pid.and_then(|pid| {
            self.pending_cwds
                .lock()
                .ok()
                .and_then(|g| g.get(&pid).cloned())
        }) {
            self.active_cwd = Some(dir);
        }
        let root = self
            .active_cwd
            .as_deref()
            .and_then(|c| Repo::discover(c).ok().flatten())
            .map(|r| r.root().to_path_buf());
        if root == self.active_root {
            return;
        }
        self.active_root = root;
        self.discard_shadow();
        let branch = self
            .active_root
            .as_ref()
            .and_then(|r| self.timelines.get(r))
            .and_then(|rt| rt.branch.clone());
        self.timeline_reset_to_now(branch);
        self.sync_active_status();
        if self.git_dock.open {
            self.refresh_git();
        }
    }

    /// Repaint every visible pane from its terminal grid, resolving cell colors and
    /// overlaying the selection and the focused-pane ring.
    fn redraw(&mut self) {
        let redraw_start = Instant::now();
        let frames = self.pane_frames();

        let views: Vec<PaneView> = frames.iter().map(PaneFrame::view).collect();

        // Build the pane-level overlays (exited-pane scrims/messages + empty-state chips) and
        // the chrome frames, all before the mutable renderer borrow.
        let (scrims, pane_overlay_quads, pane_overlay_labels) = self.pane_overlay_paint();
        let sidebar = self.sidebar.visible().then(|| self.build_sidebar_frame());
        let git_dock = self.git_dock.open.then(|| self.build_git_dock_frame());
        let timeline = self.timeline.open.then(|| self.build_timeline_frame());
        // The first-run onboarding modal (design §10.1) reuses the overlay pass and takes
        // precedence over everything else while it is up.
        let onboarding = self
            .onboarding
            .is_some()
            .then(|| self.build_onboarding_frame())
            .flatten();
        // The keybinding cheatsheet (design §11) reuses the overlay pass, below onboarding.
        let cheatsheet = (onboarding.is_none() && self.cheatsheet_open)
            .then(|| self.build_cheatsheet_frame())
            .flatten();
        // The right-click tab menu (design §08) reuses the overlay pass as an anchored card.
        let context_menu = self
            .context_menu
            .is_some()
            .then(|| self.build_context_menu_frame())
            .flatten();
        // A transient toast (design §12) reuses the overlay card at the lowest priority.
        let toast = self
            .toast
            .is_some()
            .then(|| self.build_toast_frame())
            .flatten();
        // A hover tooltip (design §09) reuses the overlay card at the very lowest priority.
        let tooltip = self
            .tooltip_visible
            .then(|| self.build_tooltip_frame())
            .flatten();
        let show_overlay = onboarding.is_none() && cheatsheet.is_none();
        let overlay = (show_overlay && self.palette.open).then(|| self.build_palette_frame());
        // The confirm modal reuses the overlay pass; it never coexists with the palette.
        let confirm = (show_overlay && !self.palette.open)
            .then(|| self.build_confirm_frame())
            .flatten();
        let settings = self.settings.open.then(|| self.build_settings_frame());

        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_panes(&views);
            renderer.set_pane_overlays(&scrims, &pane_overlay_quads, &pane_overlay_labels);
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
                    clip: frame.clip,
                    quads: &frame.quads,
                    labels: &frame.labels,
                })),
                None => renderer.set_git_dock(None),
            }
            match &timeline {
                Some(frame) => renderer.set_timeline(Some(&TimelineView {
                    panel: frame.panel,
                    clip: frame.clip,
                    quads: &frame.quads,
                    labels: &frame.labels,
                })),
                None => renderer.set_timeline(None),
            }
            // Overlay precedence: onboarding (first run) > cheatsheet > context menu > palette >
            // confirm (mutually exclusive in practice; the order guards accidental overlaps).
            let overlay_frame = onboarding
                .as_ref()
                .map(|f| (f.panel, &f.quads, &f.labels))
                .or_else(|| cheatsheet.as_ref().map(|f| (f.panel, &f.quads, &f.labels)))
                .or_else(|| {
                    context_menu
                        .as_ref()
                        .map(|f| (f.panel, &f.quads, &f.labels))
                })
                .or_else(|| overlay.as_ref().map(|f| (f.panel, &f.quads, &f.labels)))
                .or_else(|| confirm.as_ref().map(|f| (f.panel, &f.quads, &f.labels)))
                .or_else(|| toast.as_ref().map(|f| (f.panel, &f.quads, &f.labels)))
                .or_else(|| tooltip.as_ref().map(|f| (f.panel, &f.quads, &f.labels)));
            match overlay_frame {
                Some((panel, quads, labels)) => renderer.set_overlay(Some(&OverlayView {
                    panel,
                    quads,
                    labels,
                })),
                None => renderer.set_overlay(None),
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
        tracing::debug!(micros = redraw_start.elapsed().as_micros(), "redraw");
    }

    /// Lay out the command palette as a centered floating card, sized to its proportional
    /// content, animated by the open/close rise offset.
    fn build_palette_frame(&mut self) -> PaletteFrame {
        let scale = scale32(self.scale);
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));

        // Width: half the window, floored at the palette's comfortable minimum and capped clear
        // of the window edges (so a narrow window still leaves a margin).
        let edge_cap = surface_w * 0.9;
        let panel_w = (surface_w * PALETTE_WIDTH_FRAC)
            .max((palette::MIN_WIDTH * scale).min(edge_cap))
            .min(edge_cap);

        // Height: the input + count chrome and the footer are fixed; the results region between
        // them caps to a fraction of the window so a long list scrolls. The panel shrinks to its
        // content when the list is short.
        let (top_chrome, bottom_chrome) = palette::Palette::chrome_heights(scale);
        let max_results_h =
            (surface_h * PALETTE_MAX_HEIGHT_FRAC - top_chrome - bottom_chrome).max(0.0);
        let results_h = self.palette.results_height(scale).min(max_results_h);
        let panel_h = top_chrome + results_h + bottom_chrome;
        // Keep the selected row within the (now capped) results region before painting it.
        self.palette.ensure_selected_visible(results_h, scale);

        let x = ((surface_w - panel_w) / 2.0).max(0.0);
        let max_y = (surface_h - panel_h).max(0.0);
        let rest_y = ((surface_h - panel_h) / 2.0).max(0.0); // vertically centered
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
            .build(panel, results_h, scale, &self.theme, &mut self.measure);
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

    /// Lay out the first-run onboarding modal (design §10.1) as a centered card and build its
    /// content display list (the vertebra mark, shell/theme pickers, hints, Skip/Start).
    fn build_onboarding_frame(&mut self) -> Option<OnboardingFrame> {
        self.onboarding.as_ref()?;
        let panel = self.onboarding_panel();
        let scale = scale32(self.scale);
        let onb = self.onboarding.as_ref()?;
        let (quads, labels) = onboarding::build(onb, panel, scale, &self.theme, &mut self.measure);
        Some(OnboardingFrame {
            panel,
            quads,
            labels,
        })
    }

    /// Lay out the keybinding cheatsheet (design §11 `⌘/`) as a centered card, clamped to the
    /// window, and build its two-column content.
    fn build_cheatsheet_frame(&mut self) -> Option<OnboardingFrame> {
        if !self.cheatsheet_open {
            return None;
        }
        let scale = scale32(self.scale);
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        let (mut w, mut h) = cheatsheet::card_size(scale, &mut self.measure);
        w = w.min(surface_w * 0.94);
        h = h.min(surface_h * 0.9);
        let panel = PxRect {
            x: ((surface_w - w) / 2.0).max(0.0),
            y: ((surface_h - h) / 2.0).max(0.0),
            w,
            h,
        };
        let (quads, labels) = cheatsheet::build(panel, scale, &self.theme, &mut self.measure);
        Some(OnboardingFrame {
            panel,
            quads,
            labels,
        })
    }

    /// The placed panel (physical px) of the open context menu: its natural size clamped inside
    /// the window. `None` when no menu is open.
    fn context_menu_panel(&mut self) -> Option<PxRect> {
        let scale = scale32(self.scale);
        let surface = (dim_f32(self.size.0), dim_f32(self.size.1));
        let menu = self.context_menu.as_ref()?;
        let size = menu.natural_size(scale, &mut self.measure);
        Some(menu.place(size, surface))
    }

    /// Lay out the right-click tab menu (design §08) as an anchored overlay card.
    fn build_context_menu_frame(&mut self) -> Option<OnboardingFrame> {
        let panel = self.context_menu_panel()?;
        let scale = scale32(self.scale);
        let menu = self.context_menu.as_ref()?;
        let (quads, labels) = menu.build(panel, scale, &self.theme, &mut self.measure);
        Some(OnboardingFrame {
            panel,
            quads,
            labels,
        })
    }

    /// Lay out the left sidebar (design §08): a full-height panel of `sidebar.width` (or
    /// the 56px rail), its tab list built as a proportional display list in the guide's
    /// fonts + UI tokens and clipped to the panel.
    /// The sidebar group header (design §08 #5): the active repo's `name · branch` context, or
    /// `None` outside a git repo. Derived from the cached status context.
    fn group_label(&self) -> Option<String> {
        let branch = self.status_branch.as_deref()?;
        let repo = self
            .status_cwd
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or(self.status_cwd.as_str());
        Some(format!("{repo} \u{b7} {branch}"))
    }

    /// Whether each tab has a live foreground job (its focused pane is running something other
    /// than an idle shell) - for the tab's `●` running dot (design §10.3).
    fn tab_running(&self) -> Vec<bool> {
        self.tabs
            .iter()
            .map(|tab| {
                let id = tab.tree.focused();
                tab.panes
                    .get(&id)
                    .is_some_and(|t| t.foreground_job_pid().is_some())
            })
            .collect()
    }

    /// The tab titles per config (design §10.3, `[tabs] title` + `follow_cwd`). Refreshes the
    /// per-tab job-name cache and captures the frozen-cwd title (for `follow_cwd = false`) first,
    /// then resolves each title via [`resolved_tab_title`](Self::resolved_tab_title) so the sidebar
    /// list and the rail tooltip agree.
    fn tab_titles(&mut self) -> Vec<String> {
        let follow_cwd = self.config.tabs.follow_cwd;
        for tab in &mut self.tabs {
            let id = tab.tree.focused();
            let pid = tab.panes.get(&id).and_then(Terminal::foreground_job_pid);
            if pid != tab.job_pid {
                tab.job_pid = pid;
                tab.job_name = pid.and_then(process_name);
            }
            // With `follow_cwd = false`, the cwd title is captured once and never re-titles on a
            // later `cd`; with it on, the live `pane_cwd` is used and this stays cleared.
            if follow_cwd {
                tab.frozen_cwd_title = None;
            } else if tab.frozen_cwd_title.is_none() {
                tab.frozen_cwd_title = tab.pane_cwd.get(&id).map(|cwd| dir_basename(cwd));
            }
        }
        (0..self.tabs.len())
            .map(|i| self.resolved_tab_title(i))
            .collect()
    }

    /// Resolve tab `i`'s title honoring `[tabs] title` (Hard rule 1). A manual `F2` rename always
    /// wins; otherwise the configured source is used - `command` the running-command name,
    /// `cwd` the focused pane's directory basename (frozen when `follow_cwd = false`), `custom`
    /// only the rename - each falling back to `Tab N` when its source is absent. Shared by the
    /// sidebar tab list and the rail/pinned tooltip so the two never diverge.
    fn resolved_tab_title(&self, i: usize) -> String {
        let Some(tab) = self.tabs.get(i) else {
            return format!("Tab {}", i + 1);
        };
        if let Some(custom) = &tab.custom_title {
            return custom.clone();
        }
        let cwd_title = || {
            if self.config.tabs.follow_cwd {
                tab.pane_cwd
                    .get(&tab.tree.focused())
                    .map(|c| dir_basename(c))
            } else {
                tab.frozen_cwd_title.clone()
            }
        };
        match self.config.tabs.title {
            TabTitle::Command => tab.job_name.clone().or_else(cwd_title),
            TabTitle::Cwd => cwd_title().or_else(|| tab.job_name.clone()),
            TabTitle::Custom => None,
        }
        .unwrap_or_else(|| format!("Tab {}", i + 1))
    }

    /// The workspace chip glyphs: a curated set of distinct geometric marks assigned by position
    /// (the guide's `P`/`W` initials were only illustrative). Each workspace gets a stable,
    /// recognizable icon rather than a letter derived from its name.
    fn workspace_chips(&self) -> Vec<char> {
        self.workspaces
            .iter()
            .enumerate()
            .map(|(i, _)| WORKSPACE_ICONS[i % WORKSPACE_ICONS.len()])
            .collect()
    }

    /// The tab indices split into `(unpinned, pinned)` - the unpinned show in the tab list, the
    /// pinned in the 3-up grid (design §08 #4). When `sidebar.show_pinned` is off, the grid is
    /// hidden and pinned tabs fall back into the list (their pinned state is preserved).
    fn split_pinned(&self) -> (Vec<usize>, Vec<usize>) {
        let show_pinned = self.config.sidebar.show_pinned;
        let mut unpinned = Vec::new();
        let mut pinned = Vec::new();
        for (i, tab) in self.tabs.iter().enumerate() {
            if tab.pinned && show_pinned {
                pinned.push(i);
            } else {
                unpinned.push(i);
            }
        }
        (unpinned, pinned)
    }

    fn build_sidebar_frame(&mut self) -> sidebar::Paint {
        let scale = scale32(self.scale);
        // The sidebar bg fills the whole column (traffic lights sit on it); its content clears
        // the control strip via `top_inset` (logical px, macOS only).
        let titles = self.tab_titles();
        let running = self.tab_running();
        let chips = self.workspace_chips();
        let layout = self.tab_layout();
        // The tab list shows the unpinned tabs in display order (ungrouped, then each group's
        // members); the grid shows the pinned ones (glyph = the tab title's first letter).
        // `active_tab` is the active tab's position in the ordered list, or `usize::MAX` when the
        // active tab is pinned (then `active_pinned` marks its tile).
        let mut list_titles: Vec<String> =
            layout.ordered.iter().map(|&i| titles[i].clone()).collect();
        // While renaming (F2), the active tab's row is an editable field: show the buffer + a
        // caret in place of its title.
        if let Some(buf) = &self.renaming {
            if let Some(pos) = layout.ordered.iter().position(|&i| i == self.active) {
                list_titles[pos] = format!("{buf}\u{2502}");
            }
        }
        let list_running: Vec<bool> = layout.ordered.iter().map(|&i| running[i]).collect();
        let pinned_glyphs: Vec<char> = layout
            .pinned
            .iter()
            .map(|&i| {
                titles[i]
                    .chars()
                    .next()
                    .unwrap_or('\u{2022}')
                    .to_ascii_uppercase()
            })
            .collect();
        let groups: Vec<sidebar::GroupSpan> = layout
            .spans
            .iter()
            .map(|s| sidebar::GroupSpan {
                name: &s.name,
                collapsed: s.collapsed,
                start: s.start,
                len: s.len,
            })
            .collect();
        let view = sidebar::View {
            tab_count: layout.ordered.len(),
            active_tab: layout
                .ordered
                .iter()
                .position(|&i| i == self.active)
                .unwrap_or(usize::MAX),
            chips: &chips,
            active_chip: self.active_workspace,
            pinned: &pinned_glyphs,
            active_pinned: layout.pinned.iter().position(|&i| i == self.active),
            groups: &groups,
            tab_running: &list_running,
            tab_titles: &list_titles,
            rail: self.sidebar_rail_now(),
            top_inset: self.content_top() / scale,
        };
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w: self.sidebar_width_px(),
            h: dim_f32(self.size.1),
        };
        sidebar::build(&view, panel, scale, &self.theme, &mut self.measure)
    }

    /// Lay out the settings view: a full-window panel, its nav + control grid rendered
    /// in UI tokens and clipped to the window.
    fn build_settings_frame(&mut self) -> SettingsFrame {
        let scale = scale32(self.scale);
        let top = self.content_top();
        let (w, h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        // The background fills the whole window (so it reads full-height), but the content is
        // laid out below the title strip so the heading + nav clear the macOS traffic lights.
        let bg_panel = PxRect {
            x: 0.0,
            y: 0.0,
            w,
            h,
        };
        let content_panel = PxRect {
            x: 0.0,
            y: top,
            w,
            h: (h - top).max(1.0),
        };
        let paint = self.settings.build(
            content_panel,
            scale,
            &self.config,
            &self.theme,
            &mut self.measure,
        );
        SettingsFrame {
            panel: bg_panel,
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
        // The opaque fill/divider span the full panel; the content is inset below the title strip.
        let panel = self.dock_panel_rect();
        let content = self.dock_content_rect();
        let mut paint = self
            .git_dock
            .build(content, scale, &self.theme, &mut self.measure);
        self.push_dock_button(&mut paint.quads, &mut paint.labels, scale);
        // Write the clamped diff scroll back so repeated paging past the end settles.
        self.git_dock.set_scroll(paint.diff_scroll);
        GitDockFrame {
            panel,
            clip: self.dock_clip_rect(),
            quads: paint.quads,
            labels: paint.labels,
        }
    }

    /// Lay out the session-timeline dock on the right edge (design §10.5): the status
    /// banner + event list + foot as a proportional display list clipped to the panel.
    fn build_timeline_frame(&mut self) -> TimelineFrame {
        let scale = scale32(self.scale);
        // The opaque fill/divider span the full panel; the content is inset below the title strip.
        let panel = self.dock_panel_rect();
        let content = self.dock_content_rect();
        let log = active_log(
            self.active_root.as_deref(),
            &self.timelines,
            &self.empty_timeline,
        );
        let mut paint = self
            .timeline
            .build(log, content, scale, &self.theme, &mut self.measure);
        self.push_dock_button(&mut paint.quads, &mut paint.labels, scale);
        TimelineFrame {
            panel,
            clip: self.dock_clip_rect(),
            quads: paint.quads,
            labels: paint.labels,
        }
    }

    /// Draw the dock's full-width toggle button (a `bg.surface` tile with an expand / collapse
    /// glyph) into the dock's display list, seated in the header via [`Self::dock_button_rect`].
    fn push_dock_button(
        &mut self,
        quads: &mut Vec<skelly_render::ChromeQuad>,
        labels: &mut Vec<ProseLabel>,
        scale: f32,
    ) {
        let Some(rect) = self.dock_button_rect() else {
            return;
        };
        quads.push(skelly_render::ChromeQuad::rounded(
            rect,
            self.theme.bg_surface,
            4.0 * scale,
        ));
        // `⤢` expands to full width; `⤡` collapses back to the side dock.
        let glyph = if self.dock_full_width {
            "\u{2921}"
        } else {
            "\u{2922}"
        };
        let gw = self
            .measure
            .width(glyph, skelly_render::FontRole::Caption, None);
        let line = self.measure.line_height(skelly_render::FontRole::Caption);
        labels.push(ProseLabel {
            text: glyph.to_owned(),
            x: rect.x + (rect.w - gw) * 0.5,
            y: rect.y + (rect.h - line) * 0.5,
            role: skelly_render::FontRole::Caption,
            color: self.theme.fg_secondary,
            weight: None,
            max_w: f32::MAX,
        });
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
            // A live font change: rebuild the renderer's cell metrics + re-fit the grids.
            "appearance.font_size" | "appearance.line_height" => {
                let (size, line) = (
                    self.config.appearance.font_size,
                    self.config.appearance.line_height,
                );
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_font_size(size, line);
                }
                self.sync_layout();
            }
            "sidebar.mode" => {
                self.sidebar.set_mode(self.config.sidebar.mode);
                self.rail_expanded = false;
                self.sync_layout();
            }
            // The default cursor shape (what the shell shows when no program overrides it) - apply
            // live to every open pane.
            "appearance.cursor" => {
                let cursor = config_cursor_shape(self.config.appearance.cursor);
                for tab in &self.tabs {
                    for term in tab.panes.values() {
                        term.set_default_cursor_shape(cursor);
                    }
                }
            }
            // Layout-affecting toggles: re-fit the grids. Hiding the status line reclaims its
            // rows; the sidebar width shifts the pane viewport.
            "appearance.show_status_line" | "sidebar.width" => self.sync_layout(),
            _ => {}
        }
        self.persist_config();
        self.request_redraw();
    }

    /// `⇧⌘F` while a dock is open toggles its full-width; returns whether it handled the key.
    fn on_dock_fullwidth_chord(&mut self, key_event: &KeyEvent) -> bool {
        let is_f = matches!(key_event.logical_key.as_ref(), Key::Character(c) if c.eq_ignore_ascii_case("f"));
        if (self.git_dock.open || self.timeline.open)
            && self.modifiers.super_key()
            && self.modifiers.shift_key()
            && is_f
        {
            self.toggle_dock_full_width();
            return true;
        }
        false
    }

    /// Toggle the open right dock between its normal right-side width and full-width (overlaying
    /// the panes). A no-op when no dock is open. The panes keep their layout underneath, so no
    /// re-fit is needed - just a repaint.
    fn toggle_dock_full_width(&mut self) {
        if self.git_dock.open || self.timeline.open {
            self.dock_full_width = !self.dock_full_width;
            self.request_redraw();
        }
    }

    /// Show or hide the sidebar (`⌘B`). The pane viewport changes width, so re-fit the
    /// shells; the chosen mode persists (design §08, Hard rule 1).
    fn toggle_sidebar(&mut self) {
        self.sidebar.toggle();
        self.rail_expanded = false;
        self.persist_sidebar_mode();
        self.sync_layout();
        self.request_redraw();
    }

    /// Cycle the sidebar between the full panel and the slim icon rail (`⇧⌘B`, design
    /// §08). The viewport changes width, so re-fit the shells; the mode persists.
    fn cycle_sidebar_mode(&mut self) {
        self.sidebar.cycle_rail();
        self.rail_expanded = false;
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

    /// Start the background thread that watches **every open tab's repo** working tree, so the
    /// session timeline records the user's edits (design §10.5) - which happen inside the panes
    /// (vim), invisible to Skelly otherwise. It reads the panes' cwds (`pending_cwds`, filled by the
    /// cwd thread), discovers + `status()`es each distinct repo off the UI thread, and posts a
    /// per-repo map via `Wakeup::GitPoll`. A non-repo cwd contributes nothing.
    fn start_git_poll(&mut self) {
        let cwds_slot = Arc::clone(&self.pending_cwds);
        let status_slot = Arc::clone(&self.pending_status);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(GIT_POLL_INTERVAL);
            // The working set: every pane's cwd (covers background tabs). Before the first cwd poll
            // fills the map, fall back to the process cwd so the launch repo is watched from cycle 1.
            let mut cwds: Vec<std::path::PathBuf> = cwds_slot
                .lock()
                .map(|g| g.values().cloned().collect())
                .unwrap_or_default();
            if cwds.is_empty() {
                cwds.extend(std::env::current_dir());
            }
            // Discover + status each distinct repo once (dedup cwds that share a repo root).
            let mut result: HashMap<std::path::PathBuf, RepoStatus> = HashMap::new();
            for cwd in cwds {
                let Ok(Some(repo)) = Repo::discover(&cwd) else {
                    continue;
                };
                let root = repo.root().to_path_buf();
                if result.contains_key(&root) {
                    continue; // another cwd already statused this repo this cycle
                }
                if let Ok(status) = repo.status() {
                    let head = repo.head_short().ok();
                    result.insert(root, RepoStatus { status, head });
                }
            }
            if let Ok(mut guard) = status_slot.lock() {
                *guard = result;
            }
            if proxy.send_event(Wakeup::GitPoll).is_err() {
                break; // the event loop is gone; stop polling.
            }
        });
    }

    /// Drain the latest per-repo statuses from the poll thread: for each repo, seed its "session
    /// started" anchor on first sight, else record any file that became dirty since its last poll as
    /// an edit event into *its* timeline, and refresh its cached dirty/branch. Then project the
    /// active repo's status to the status line. Repaints only when something changed.
    fn drain_git_poll(&mut self) {
        let statuses = match self.pending_status.lock() {
            Ok(mut g) => std::mem::take(&mut *g),
            Err(_) => return,
        };
        let mut changed = false;
        for (root, RepoStatus { status, head }) in statuses {
            let current = dirty_paths(&status);
            let totals = dirty_totals(&status);
            let branch = status.branch.clone();
            if self.timelines.get(&root).is_some_and(|rt| rt.started) {
                // Files newly dirty since this repo's last poll = edits made this session.
                let newly: Vec<(String, u32, u32)> = {
                    let tracked = self.timelines.get(&root).map(|rt| &rt.tracked_dirty);
                    status
                        .files
                        .iter()
                        .filter(|f| {
                            tracked.is_none_or(|t| !t.contains(f.path.to_string_lossy().as_ref()))
                        })
                        .map(|f| (f.path.to_string_lossy().into_owned(), f.added, f.removed))
                        .collect()
                };
                if let Some((title, detail)) = edit_text(&newly) {
                    tracing::debug!(files = newly.len(), %title, "timeline: recorded edit");
                    let event =
                        SessionEvent::new(Actor::Human, self.elapsed_label(), title, detail);
                    self.record_edit(&root, event);
                    changed = true;
                }
                if let Some(rt) = self.timelines.get_mut(&root) {
                    changed |= rt.tracked_dirty != current || rt.dirty != totals;
                    rt.tracked_dirty = current;
                    rt.dirty = totals;
                    rt.branch = branch;
                }
            } else {
                // First sight: anchor + baseline (pre-session dirt is not counted as an edit).
                self.seed_repo(&root, branch, head, current, totals);
                changed = true;
            }
        }
        // The active repo's dirty/branch may have moved; keep the status line coherent (F3).
        self.sync_active_status();
        if changed {
            self.request_redraw();
        }
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
        // Opening keeps keyboard focus on the terminal; closing clears the dock focus.
        self.dock_focused = false;
        self.dock_full_width = false;
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
            self.ensure_active_seeded();
            let branch = self
                .active_root
                .as_ref()
                .and_then(|r| self.timelines.get(r))
                .and_then(|rt| rt.branch.clone());
            self.timeline_open(branch);
            self.reconcile_shadow();
        }
        // Opening keeps keyboard focus on the terminal; closing clears the dock focus.
        self.dock_focused = false;
        self.dock_full_width = false;
        self.sync_layout();
        self.request_redraw();
    }

    /// Close the timeline dock and return to now (discarding any shadow worktree).
    fn close_timeline(&mut self) {
        if self.timeline.open {
            self.timeline.close();
        }
        self.dock_focused = false;
        self.discard_shadow();
    }

    /// Append `event` to `root`'s repo timeline (creating it if new). When `root` is the active
    /// repo and the dock cursor was already at now, re-pin it to the new newest, so recording keeps
    /// you at HEAD unless you have scrubbed to a past state. Field-disjoint borrows keep this legal.
    fn record_edit(&mut self, root: &std::path::Path, event: SessionEvent) {
        let active = self.active_root.as_deref() == Some(root);
        // Decide the pin from immutable borrows of disjoint fields, before mutating the map.
        let was_at_now = active && {
            let cursor = self.timeline.selected();
            self.timelines
                .get(root)
                .is_none_or(|rt| rt.timeline.newest().is_none_or(|n| cursor == n))
        };
        let rt = self.timelines.entry(root.to_path_buf()).or_default();
        rt.timeline.record(event);
        let len = rt.timeline.len();
        if active {
            self.timeline.set_duration(self.session_start.elapsed());
            if was_at_now {
                self.timeline.snap_to_newest(len);
            }
        }
    }

    /// Record a git-action event (commit / stage / …) into the *active* repo's timeline, stamped
    /// with the session-relative elapsed time. A no-op outside a repo.
    fn record_event(
        &mut self,
        actor: Actor,
        title: impl Into<String>,
        detail: impl Into<String>,
        restore: Option<String>,
    ) {
        let Some(root) = self.active_root.clone() else {
            return;
        };
        let mut event = SessionEvent::new(actor, self.elapsed_label(), title, detail);
        if let Some(sha) = restore {
            event = event.restoring(sha);
        }
        self.record_edit(&root, event);
    }

    /// Record `root`'s one-time "session started" anchor (restorable to its then-HEAD) and seed its
    /// dirty baseline, so pre-session changes aren't mistaken for edits (design §10.5).
    fn seed_repo(
        &mut self,
        root: &std::path::Path,
        branch: Option<String>,
        head: Option<String>,
        tracked: HashSet<String>,
        dirty: Option<(u32, u32)>,
    ) {
        let detail = branch.clone().unwrap_or_else(|| "no repository".to_owned());
        let mut event = SessionEvent::new(
            Actor::System,
            self.elapsed_label(),
            "Session started",
            detail,
        );
        if let Some(sha) = head {
            event = event.restoring(sha);
        }
        self.record_edit(root, event);
        let rt = self.timelines.entry(root.to_path_buf()).or_default();
        rt.started = true;
        rt.tracked_dirty = tracked;
        rt.dirty = dirty;
        rt.branch = branch;
    }

    /// Synchronously seed the active repo if it has not been (so opening the timeline shows its
    /// anchor before the first background poll for that repo lands). Reads the repo once.
    fn ensure_active_seeded(&mut self) {
        let Some(root) = self.active_root.clone() else {
            return;
        };
        if self.timelines.get(&root).is_some_and(|rt| rt.started) {
            return;
        }
        let (branch, head, tracked, dirty) = match Repo::discover(&root) {
            Ok(Some(repo)) => {
                let status = repo.status().ok();
                let branch = status.as_ref().and_then(|s| s.branch.clone());
                let (tracked, dirty) = status.map_or_else(
                    || (HashSet::new(), None),
                    |s| (dirty_paths(&s), dirty_totals(&s)),
                );
                (branch, repo.head_short().ok(), tracked, dirty)
            }
            _ => (None, None, HashSet::new(), None),
        };
        self.seed_repo(&root, branch, head, tracked, dirty);
    }

    /// A short session-relative time label (`M:SS` into the session) for a recorded event.
    fn elapsed_label(&self) -> String {
        let secs = self.session_start.elapsed().as_secs();
        format!("{}:{:02}", secs / 60, secs % 60)
    }

    /// Open the dock over the active repo's log (borrow contained to disjoint fields).
    fn timeline_open(&mut self, branch: Option<String>) {
        let log = active_log(
            self.active_root.as_deref(),
            &self.timelines,
            &self.empty_timeline,
        );
        self.timeline.open(log, branch);
    }

    /// Reset the dock cursor to the active repo's now, adopting its branch for the summary.
    fn timeline_reset_to_now(&mut self, branch: Option<String>) {
        let log = active_log(
            self.active_root.as_deref(),
            &self.timelines,
            &self.empty_timeline,
        );
        self.timeline.reset_to_now(log, branch);
    }

    /// Move the timeline selection by `delta` over the active repo's log. Returns whether it moved.
    fn timeline_move_selection(&mut self, delta: i32) -> bool {
        let log = active_log(
            self.active_root.as_deref(),
            &self.timelines,
            &self.empty_timeline,
        );
        self.timeline.move_selection(log, delta)
    }

    /// Snap the timeline selection to now over the active repo's log. Returns whether it moved.
    fn timeline_select_now(&mut self) -> bool {
        let log = active_log(
            self.active_root.as_deref(),
            &self.timelines,
            &self.empty_timeline,
        );
        self.timeline.select_now(log)
    }

    /// Whether the timeline selection is at now (HEAD) over the active repo's log.
    fn timeline_at_now(&self) -> bool {
        let log = active_log(
            self.active_root.as_deref(),
            &self.timelines,
            &self.empty_timeline,
        );
        self.timeline.selection_is_now(log)
    }

    /// The restorable commit for the current timeline selection over the active repo's log.
    fn timeline_restore(&self) -> Option<String> {
        let log = active_log(
            self.active_root.as_deref(),
            &self.timelines,
            &self.empty_timeline,
        );
        self.timeline.selected_restore(log)
    }

    /// Project the status-line branch + dirty from the active repo's last-known status - the single
    /// source (F3), so the status line always describes the same repo as the shown cwd.
    fn sync_active_status(&mut self) {
        let rt = self
            .active_root
            .as_ref()
            .and_then(|r| self.timelines.get(r));
        self.status_dirty = rt.and_then(|t| t.dirty);
        self.status_branch = rt.and_then(|t| t.branch.clone());
    }

    /// Reconcile the shadow worktree to the timeline's current selection **in the active repo**: at
    /// now, discard any worktree; on a past state, ensure a shadow worktree is checked out to its
    /// commit (Hard rule 3 - never touches HEAD/refs). A git failure is logged and treated as
    /// "stay at now" rather than left half-applied.
    fn reconcile_shadow(&mut self) {
        if self.timeline_at_now() {
            self.discard_shadow();
            return;
        }
        let Some(sha) = self.timeline_restore() else {
            self.discard_shadow();
            return;
        };
        // Already viewing this commit? Nothing to do.
        if self.shadow.as_ref().is_some_and(|w| w.committish() == sha) {
            return;
        }
        self.discard_shadow();
        let Some(root) = self.active_root.clone() else {
            tracing::warn!("cannot rewind: no active repository");
            return;
        };
        match Repo::discover(&root) {
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

    /// The active tab's focused-pane working directory (absolute) - the repo the git diff dock
    /// scopes to (design §10.4 "Scoped to the active tab's repo"). Read from the pane's shell
    /// process; falls back to the process cwd when it can't be read or there is no focused pane.
    /// Only used to seed the very first git refresh (before the poll thread has filled the cache);
    /// steady state reads the cached [`active_cwd`](Self::active_cwd) instead.
    fn active_pane_cwd(&self) -> std::path::PathBuf {
        let ws = self.active_tab();
        ws.panes
            .get(&ws.tree.focused())
            .and_then(Terminal::shell_pid)
            .and_then(process_cwd)
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            })
    }

    /// Refresh the git diff dock from the active tab's focused-pane repo (design §10.4), so it
    /// follows the selected tab / its cwd. Scopes to the cached [`active_cwd`](Self::active_cwd)
    /// (the last *successfully* polled cwd, so a transient read failure never re-points the dock at
    /// the launch dir), seeding it from a fresh read only before the first poll lands. Caches the
    /// repo, loads the working status, then the selected file's diff.
    fn refresh_git(&mut self) {
        let start = self
            .active_cwd
            .clone()
            .unwrap_or_else(|| self.active_pane_cwd());
        match Repo::discover(&start) {
            Ok(Some(repo)) => match repo.status() {
                Ok(status) => {
                    // The status-line dirty/branch are projected from the git-poll thread's per-repo
                    // cache (`sync_active_status`), not written here - the dock owns only the dock.
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

    /// Initialize a git repository in the active tab's focused-pane cwd (the git dock's "Init
    /// repo" empty-state action, design §12 "Not a git repo") and refresh the dock to show the
    /// new, empty repo. A failure surfaces in the dock's error line.
    fn init_repo(&mut self) {
        let start = self.active_pane_cwd();
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
        let scale = scale32(self.scale);
        let chips = self.workspace_chips();
        let layout = self.tab_layout();
        // Layout only depends on the counts + group spans, so glyph placeholders and empty
        // title/running slices are fine here (the real strings feed rendering, not hit-testing).
        let pinned_glyphs = vec!['\u{2022}'; layout.pinned.len()];
        let groups: Vec<sidebar::GroupSpan> = layout
            .spans
            .iter()
            .map(|s| sidebar::GroupSpan {
                name: &s.name,
                collapsed: s.collapsed,
                start: s.start,
                len: s.len,
            })
            .collect();
        let view = sidebar::View {
            tab_count: layout.ordered.len(),
            active_tab: layout
                .ordered
                .iter()
                .position(|&i| i == self.active)
                .unwrap_or(usize::MAX),
            chips: &chips,
            active_chip: self.active_workspace,
            pinned: &pinned_glyphs,
            active_pinned: layout.pinned.iter().position(|&i| i == self.active),
            groups: &groups,
            tab_running: &[],
            tab_titles: &[],
            rail: self.sidebar_rail_now(),
            top_inset: self.content_top() / scale,
        };
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w: self.sidebar_width_px(),
            h: dim_f32(self.size.1),
        };
        // The sidebar reports positions within the ordered list; map them back to real tab
        // indices (a pinned-tile click just activates that tab). Group-header hits pass through.
        match sidebar::hit(&view, panel, scale, px, py) {
            Some(sidebar::Hit::Tab(pos)) => layout.ordered.get(pos).copied().map(sidebar::Hit::Tab),
            Some(sidebar::Hit::Pinned(gi)) => layout.pinned.get(gi).copied().map(sidebar::Hit::Tab),
            other => other,
        }
    }

    /// Run a sidebar utility-bar toggle (design §08 #7). Each is a second entry point to an
    /// existing command; the theme icon flips between Ossein Dark and Light.
    fn on_util_action(&mut self, action: sidebar::UtilAction) {
        match action {
            sidebar::UtilAction::Settings => self.open_settings(),
            sidebar::UtilAction::Theme => {
                let next = if self.config.appearance.theme == "ossein-light" {
                    "ossein-dark"
                } else {
                    "ossein-light"
                };
                self.apply_theme(next);
            }
            sidebar::UtilAction::Timeline => self.toggle_timeline(),
            sidebar::UtilAction::Git => self.toggle_git_dock(),
        }
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

    /// Insert a file dropped onto the window at the focused pane's prompt (design intent:
    /// dragging a screenshot into a pane running Claude Code / Pi, which then attaches it as
    /// `[Image #N]`). The shell-escaped path is written as terminal input followed by a space -
    /// the same wire behavior as other terminals, so a TUI recognizes it as a dropped file and
    /// the space separates it from the next drop or typed text. winit delivers one event per
    /// file with no drop position, so it targets the focused pane.
    fn on_file_dropped(&mut self, path: &std::path::Path) {
        let mut input = shell_escape_path(path);
        input.push(' ');
        if let Some(term) = self.focused_term() {
            term.scroll_to_bottom();
            term.write(input.as_bytes());
        }
        self.request_redraw();
    }

    /// Type `text` into the focused pane's shell (the palette's `/` file-entry insert, §10.8):
    /// the user picks a file and its path lands at the prompt to complete a command.
    fn type_into_focused(&mut self, text: &str) {
        if let Some(term) = self.focused_term() {
            term.scroll_to_bottom();
            term.write(text.as_bytes());
        }
        self.request_redraw();
    }

    /// The focused pane's absolute cwd from the latest cwd poll, falling back to the app-level
    /// active cwd, or `None` before the first poll. Seeds a split's inherited working directory.
    fn focused_pane_abs_cwd(&self) -> Option<std::path::PathBuf> {
        let ws = self.active_tab();
        let pid = ws
            .panes
            .get(&ws.tree.focused())
            .and_then(Terminal::shell_pid);
        pid.and_then(|pid| {
            self.pending_cwds
                .lock()
                .ok()
                .and_then(|g| g.get(&pid).cloned())
        })
        .or_else(|| self.active_cwd.clone())
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
        // A split at the pane cap no-ops; surface why with a toast (design §12 flow 1).
        if matches!(action, PaneAction::Split(_)) && self.active_tab().tree.count() >= cap {
            self.show_toast(format!("Pane limit reached ({cap} max)"), ToastKind::Info);
            return;
        }
        // A split inherits the source (focused) pane's cwd, so the new pane opens in the same
        // directory (design §11, `[panes] split_inherits_cwd`). Captured before the split moves
        // focus to the new pane; armed only once the split applies, so it can never leak onto an
        // unrelated later spawn. `sync_layout` consumes it when it spawns that pane's shell.
        let inherit = (matches!(action, PaneAction::Split(_))
            && self.config.panes.split_inherits_cwd)
            .then(|| self.focused_pane_abs_cwd())
            .flatten();
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
            PaneAction::Swap(dir) => ws.tree.swap(dir),
            PaneAction::EvenOut => {
                ws.tree.even_out();
                true
            }
            PaneAction::CycleLayout => ws.tree.cycle_layout(),
        };
        if changed {
            self.inherit_cwd = inherit;
            let ws = self.active_tab_mut();
            ws.selection = None;
            ws.activated = true; // operating on panes means the tab is in use; clear its empty state
            self.sync_layout();
            // A focus/close/zoom may have changed which pane is focused; follow its repo at once
            // (a differently-`cd`'d pane) instead of lagging the next poll.
            self.rescope_active();
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
        // Remember the closed tab's title for `⇧⌘T` reopen, but only if it was meaningfully
        // named (a custom name or a running command) - a bare "Tab N" is not worth reopening.
        let title = self.tab_titles().get(self.active).cloned();
        if let Some(title) = title {
            let tab = &self.tabs[self.active];
            if tab.custom_title.is_some() || tab.job_name.is_some() {
                self.closed_titles.push(title);
            }
        }
        let count = self.tabs.len();
        if count <= 1 {
            // Never quit: replace the only tab with a pristine one (the old tab drops, so
            // its shells are killed) and show the empty state.
            self.tabs[self.active] = Tab::new();
        } else {
            self.tabs.remove(self.active);
            self.active = index_after_close(self.active, count);
        }
        // Closing a tab may empty its group; keep the group list dense (design §08 #5).
        self.prune_empty_groups();
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
            // Immediately follow the switched-to tab's repo from the cached cwds (no wait for the
            // next poll); the status-line cwd catches up on the next drain.
            self.rescope_active();
            // The now-visible tab may have been sized for an earlier window; re-fit it.
            self.sync_layout();
            self.request_redraw();
        }
    }

    /// Pin or unpin the active tab (design §08 #4, ⇧⌘P): pinned tabs move into the sidebar's
    /// 3-up icon grid and out of the tab list, keeping their shell alive. The active tab stays
    /// active in either list.
    fn toggle_pin(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.pinned = !tab.pinned;
            self.request_redraw();
        }
    }

    /// `⇧⌘N` - create a new collapsible group (design §08 #5) from the active tab. It takes the
    /// active tab's repo·branch context as its name (e.g. `skelly · main`), or a numbered
    /// fallback, and the active tab becomes its first member ("Groups map to a working directory
    /// or project"). A tab already in a group just moves to the new one.
    fn new_group(&mut self) {
        let name = self
            .group_label()
            .unwrap_or_else(|| format!("Group {}", self.groups.len() + 1));
        self.groups.push(TabGroup {
            name,
            collapsed: false,
        });
        let gi = self.groups.len() - 1;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.group = Some(gi);
        }
        // The active tab may have vacated a group, leaving it empty; keep the list dense.
        self.prune_empty_groups();
        self.request_redraw();
    }

    /// Collapse or expand the group at `gi` (design §08 #5): a collapsed group hides its member
    /// tabs from the sidebar list while their shells keep running. The active tab stays active
    /// even when its own group collapses (its pane still shows in the terminal area).
    fn toggle_group(&mut self, gi: usize) {
        if let Some(group) = self.groups.get_mut(gi) {
            group.collapsed = !group.collapsed;
            self.request_redraw();
        }
    }

    /// Drop any group left with no member tabs and re-index the survivors so `Tab.group`
    /// indices stay dense and valid (called after a tab closes or changes group).
    fn prune_empty_groups(&mut self) {
        let mut used = vec![false; self.groups.len()];
        for tab in &self.tabs {
            if let Some(g) = tab.group {
                if let Some(slot) = used.get_mut(g) {
                    *slot = true;
                }
            }
        }
        if used.iter().all(|&u| u) {
            return; // every group still has members - nothing to prune
        }
        // Build an old -> new index remap, keeping only groups that still have members.
        let mut remap = vec![None; self.groups.len()];
        let mut kept = Vec::new();
        for (old, group) in std::mem::take(&mut self.groups).into_iter().enumerate() {
            if used[old] {
                remap[old] = Some(kept.len());
                kept.push(group);
            }
        }
        self.groups = kept;
        for tab in &mut self.tabs {
            if let Some(g) = tab.group {
                tab.group = remap.get(g).copied().flatten();
            }
        }
    }

    /// `⌘1…9` - jump to the nth tab (0-based) within the active tab's group (design §11: "Number
    /// jumps to nth tab in the active group"); when the active tab is ungrouped, the nth
    /// ungrouped tab. A no-op when there is no nth member.
    fn goto_tab_in_active_group(&mut self, n: usize) {
        let active_group = self.tabs.get(self.active).and_then(|t| t.group);
        let target = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.group == active_group)
            .nth(n)
            .map(|(i, _)| i);
        if let Some(i) = target {
            self.goto_tab(i);
        }
    }

    /// The sidebar's ordered tab layout (design §08 #4/#5): pinned tabs, the unpinned tabs in
    /// display order (ungrouped first, then each group's members), and the group spans over
    /// that ordered list. See [`TabLayout`].
    fn tab_layout(&self) -> TabLayout {
        let (unpinned, pinned) = self.split_pinned();
        let mut ordered: Vec<usize> = unpinned
            .iter()
            .copied()
            .filter(|&i| self.tabs[i].group.is_none())
            .collect();
        let mut spans = Vec::new();
        for (gi, group) in self.groups.iter().enumerate() {
            let start = ordered.len();
            let before = ordered.len();
            ordered.extend(
                unpinned
                    .iter()
                    .copied()
                    .filter(|&i| self.tabs[i].group == Some(gi)),
            );
            spans.push(GroupSpanData {
                name: group.name.clone(),
                collapsed: group.collapsed,
                start,
                len: ordered.len() - before,
            });
        }
        TabLayout {
            pinned,
            ordered,
            spans,
        }
    }

    /// Right-click (design §08 "Right-click any tab for the full action menu"): open the tab
    /// action menu when the click lands on a sidebar tab. The tab is focused first (right-click
    /// selects), then the menu opens anchored at the pointer.
    fn on_right_click(&mut self) {
        if self.context_menu.is_some() {
            self.close_context_menu();
        }
        if let Some(sidebar::Hit::Tab(index)) = self.sidebar_hit() {
            self.goto_tab(index);
            self.open_context_menu();
        }
    }

    /// Open the right-click menu for the active (just-clicked) tab, anchored at the pointer.
    fn open_context_menu(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let group = tab.group;
        let ctx = MenuContext {
            pinned: tab.pinned,
            group,
            other_groups: self
                .groups
                .iter()
                .enumerate()
                .filter(|(gi, _)| Some(*gi) != group)
                .map(|(gi, g)| (gi, g.name.clone()))
                .collect(),
        };
        self.context_menu = Some(ContextMenu::new(point_f32(self.pointer), &ctx));
        self.request_redraw();
    }

    /// Dismiss the right-click menu.
    fn close_context_menu(&mut self) {
        if self.context_menu.take().is_some() {
            self.request_redraw();
        }
    }

    /// Show a transient toast (design §12) for `TOAST_DURATION`; the event loop wakes at the
    /// deadline to dismiss it. A new toast replaces any current one.
    fn show_toast(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast::new(message, kind));
        self.toast_expires = Instant::now() + TOAST_DURATION;
        self.request_redraw();
    }

    /// The tooltip label for the icon-only element under the pointer (design §09), or `None` when
    /// the pointer is not over one (or over a tab whose title is already visible in the panel).
    fn tooltip_label_at_pointer(&self) -> Option<String> {
        let label = match self.sidebar_hit()? {
            sidebar::Hit::Util(action) => match action {
                sidebar::UtilAction::Settings => "Settings  \u{2318},",
                sidebar::UtilAction::Theme => "Toggle theme",
                sidebar::UtilAction::Timeline => "Session timeline  \u{21e7}\u{2318}H",
                sidebar::UtilAction::Git => "Git diff  \u{21e7}\u{2318}G",
            }
            .to_owned(),
            sidebar::Hit::CommandInput => "Search or run  \u{2318}K".to_owned(),
            sidebar::Hit::AddWorkspace => "New workspace".to_owned(),
            sidebar::Hit::Workspace(i) => self.workspaces.get(i).map(|w| w.name.clone())?,
            // Tabs only need a tooltip when their title isn't already shown: the slim rail (just a
            // number) or a pinned icon tile.
            sidebar::Hit::Tab(i)
                if self.sidebar_rail_now() || self.tabs.get(i).is_some_and(|t| t.pinned) =>
            {
                // Same resolver the sidebar list uses, so the tooltip never diverges from the title.
                self.resolved_tab_title(i)
            }
            _ => return None,
        };
        Some(label)
    }

    /// Lay out the hover tooltip (design §09) near the pointer once it has crossed its delay, or
    /// `None` while still within the delay / not hovering an element.
    fn build_tooltip_frame(&mut self) -> Option<OnboardingFrame> {
        if !self.tooltip_visible {
            return None;
        }
        let label = self.hover_tip.as_ref()?.0.clone();
        let scale = scale32(self.scale);
        let surface = (dim_f32(self.size.0), dim_f32(self.size.1));
        let size = tooltip::natural_size(&label, scale, &mut self.measure);
        let panel = tooltip::place(point_f32(self.pointer), size, surface, scale);
        let (quads, labels) = tooltip::build(&label, panel, scale, &self.theme, &mut self.measure);
        Some(OnboardingFrame {
            panel,
            quads,
            labels,
        })
    }

    /// Lay out the current toast (design §12) as a bottom-anchored overlay card, or `None`.
    fn build_toast_frame(&mut self) -> Option<OnboardingFrame> {
        let scale = scale32(self.scale);
        let surface = (dim_f32(self.size.0), dim_f32(self.size.1));
        let toast = self.toast.as_ref()?;
        let size = toast.natural_size(scale, &mut self.measure);
        let panel = toast::place(size, surface, scale);
        let (quads, labels) = toast.build(panel, scale, &self.theme, &mut self.measure);
        Some(OnboardingFrame {
            panel,
            quads,
            labels,
        })
    }

    /// The entry index under the pointer in the open menu, if any (shared placed panel).
    fn context_menu_hit(&mut self) -> Option<usize> {
        let panel = self.context_menu_panel()?;
        let (px, py) = point_f32(self.pointer);
        let scale = scale32(self.scale);
        self.context_menu.as_ref()?.hit(panel, scale, px, py)
    }

    /// Run a chosen menu action against the active (right-clicked) tab, then close the menu.
    fn run_menu_action(&mut self, action: MenuAction) {
        self.context_menu = None;
        match action {
            MenuAction::TogglePin => self.toggle_pin(),
            MenuAction::Rename => self.start_rename(),
            MenuAction::Duplicate => self.duplicate_active_tab(),
            MenuAction::NewGroup => self.new_group(),
            MenuAction::MoveToGroup(gi) => self.move_active_tab_to_group(Some(gi)),
            MenuAction::RemoveFromGroup => self.move_active_tab_to_group(None),
            MenuAction::Close => self.request_close_tab(),
        }
        self.request_redraw();
    }

    /// Move the active tab into `target` (an existing group, or `None` to ungroup) and prune any
    /// group the move leaves empty.
    fn move_active_tab_to_group(&mut self, target: Option<usize>) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.group = target;
        }
        self.prune_empty_groups();
        self.request_redraw();
    }

    /// Duplicate the active tab (context menu): insert a fresh tab right after it, inheriting its
    /// group + custom name, and focus the copy (its shell is a new process, spawned by
    /// `sync_layout`).
    fn duplicate_active_tab(&mut self) {
        let (group, custom_title) = self
            .tabs
            .get(self.active)
            .map_or((None, None), |t| (t.group, t.custom_title.clone()));
        let mut tab = Tab::new();
        tab.group = group;
        tab.custom_title = custom_title;
        let at = (self.active + 1).min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = at;
        self.selecting = false;
        self.sync_layout();
        self.request_redraw();
    }

    /// Keys while the right-click menu is up (design §08): `↑/↓` move the highlight (skipping
    /// dividers), `Enter` runs the highlighted action, `Esc` dismisses. Captured while open.
    fn on_context_menu_key(&mut self, key_event: &KeyEvent) {
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => self.close_context_menu(),
            Key::Named(NamedKey::ArrowDown) => {
                if let Some(menu) = self.context_menu.as_mut() {
                    menu.move_selection(1);
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(menu) = self.context_menu.as_mut() {
                    menu.move_selection(-1);
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) => {
                let action = self
                    .context_menu
                    .as_ref()
                    .and_then(ContextMenu::selected_action);
                if let Some(action) = action {
                    self.run_menu_action(action);
                }
            }
            _ => {}
        }
    }

    /// Begin renaming the active tab (design §11 `F2`): its sidebar row becomes an editable
    /// field seeded with the current title. `Enter` commits, `Esc` cancels.
    fn start_rename(&mut self) {
        let title = self
            .tab_titles()
            .get(self.active)
            .cloned()
            .unwrap_or_default();
        self.renaming = Some(title);
        self.request_redraw();
    }

    /// Feed a key to the in-progress rename (a typed char, `Backspace`, `Enter` = commit,
    /// `Esc` = cancel). Returns whether the rename consumed the key.
    fn on_rename_key(&mut self, key_event: &KeyEvent) -> bool {
        let Some(buf) = self.renaming.as_mut() else {
            return false;
        };
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Enter) => {
                let name = self.renaming.take().unwrap_or_default();
                let trimmed = name.trim();
                // An empty name clears the custom title back to the automatic one.
                self.active_tab_mut().custom_title =
                    (!trimmed.is_empty()).then(|| trimmed.to_owned());
            }
            Key::Named(NamedKey::Escape) => self.renaming = None,
            Key::Named(NamedKey::Backspace) => {
                buf.pop();
            }
            Key::Character(ch) => buf.push_str(ch),
            Key::Named(NamedKey::Space) => buf.push(' '),
            _ => {}
        }
        self.request_redraw();
        true
    }

    /// Open (or refocus) the scrollback find bar (design §11 `⌘F`) for the focused pane.
    fn open_find(&mut self) {
        if self.find.is_none() {
            self.find = Some(FindState {
                query: String::new(),
                hit: None,
                searched: false,
            });
        }
        self.request_redraw();
    }

    /// Close the find bar, clear its highlight, and snap the focused pane back to the bottom.
    fn close_find(&mut self) {
        if self.find.take().is_some() {
            if let Some(term) = self.focused_term() {
                term.scroll_to_bottom();
            }
            self.request_redraw();
        }
    }

    /// Run the current find query over the focused pane's scrollback (design §11): `from` = the
    /// current match line to continue from (`None` = a fresh search), `forward` toward newer
    /// output. Stores the hit (which scrolls it into view) or marks "no match".
    fn run_find(&mut self, from: Option<i32>, forward: bool) {
        let Some(query) = self.find.as_ref().map(|f| f.query.clone()) else {
            return;
        };
        let hit = if query.is_empty() {
            None
        } else {
            self.focused_term()
                .and_then(|t| t.find(&query, from, forward))
        };
        if let Some(find) = self.find.as_mut() {
            find.hit = hit;
            find.searched = !query.is_empty();
        }
        self.request_redraw();
    }

    /// Feed a key to the open find bar: typing edits the query (re-searching from the top),
    /// `Enter`/`↓` next match, `⇧Enter`/`↑` previous, `Esc` closes. Returns whether it consumed
    /// the key.
    fn on_find_key(&mut self, key_event: &KeyEvent) -> bool {
        if self.find.is_none() {
            return false;
        }
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => self.close_find(),
            Key::Named(NamedKey::Enter | NamedKey::ArrowDown) => {
                let from = self.find.as_ref().and_then(|f| f.hit.map(|h| h.line));
                self.run_find(from, self.modifiers.shift_key());
            }
            Key::Named(NamedKey::ArrowUp) => {
                let from = self.find.as_ref().and_then(|f| f.hit.map(|h| h.line));
                self.run_find(from, true);
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(find) = self.find.as_mut() {
                    find.query.pop();
                }
                self.run_find(None, false);
            }
            Key::Character(ch) => {
                if let Some(find) = self.find.as_mut() {
                    find.query.push_str(ch);
                }
                self.run_find(None, false);
            }
            Key::Named(NamedKey::Space) => {
                if let Some(find) = self.find.as_mut() {
                    find.query.push(' ');
                }
                self.run_find(None, false);
            }
            _ => {}
        }
        true
    }

    /// Reopen the most recently closed tab (design §11 `⇧⌘T`): a fresh tab carrying its title
    /// back (the killed shell can't be resurrected). A no-op when nothing was closed.
    fn reopen_closed_tab(&mut self) {
        let Some(title) = self.closed_titles.pop() else {
            return;
        };
        self.new_tab();
        self.active_tab_mut().custom_title = Some(title);
        self.request_redraw();
    }

    /// Move the tab at `from` to index `to` (a sidebar drag-reorder), keeping the moved tab
    /// active. No-op when the indices are equal or out of range.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = to;
        self.request_redraw();
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

    /// Stash the active workspace's tab set + groups (its shells keep running) so another can
    /// swap in.
    fn stash_active_workspace(&mut self) {
        let tabs = std::mem::take(&mut self.tabs);
        let groups = std::mem::take(&mut self.groups);
        self.workspaces[self.active_workspace].stash = Some((tabs, self.active, groups));
    }

    /// Switch to workspace `index` (design §08 #2): stash the active workspace's tabs, swap in
    /// the target's, and re-fit its shells to the current viewport. A no-op for the active one.
    fn switch_workspace(&mut self, index: usize) {
        if index == self.active_workspace || index >= self.workspaces.len() {
            return;
        }
        self.stash_active_workspace();
        let (tabs, active, groups) = self.workspaces[index]
            .stash
            .take()
            .unwrap_or_else(|| (vec![Tab::new()], 0, Vec::new()));
        self.tabs = tabs;
        self.active = active;
        self.groups = groups;
        self.active_workspace = index;
        self.selecting = false;
        self.sync_layout(); // re-fit the swapped-in shells to the viewport
        self.request_redraw();
    }

    /// Add a new workspace (the `+` chip) with a fresh single tab, and switch to it.
    fn add_workspace(&mut self) {
        self.stash_active_workspace();
        // The first two get the guide's "Personal"/"Work" names (chips P / W); the rest are
        // numbered.
        let name = match self.workspaces.len() {
            1 => "Work".to_owned(),
            n => format!("Space {}", n + 1),
        };
        self.workspaces.push(Workspace { name, stash: None });
        self.tabs = vec![Tab::new()];
        self.active = 0;
        self.groups = Vec::new();
        self.active_workspace = self.workspaces.len() - 1;
        self.selecting = false;
        self.sync_layout();
        self.request_redraw();
    }

    /// Capture the whole window state (all workspaces, their tabs + groups, each pane's tiling
    /// and cwd) as a [`SessionState`](session_state::SessionState) for launch-time restore
    /// (design/README.md persist scope: **layout only**). The active workspace's tabs live in
    /// `self.tabs`/`self.groups`; the others read from their stash.
    fn session_snapshot(&self) -> session_state::SessionState {
        let workspaces = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| {
                if i == self.active_workspace {
                    Self::workspace_snapshot(&ws.name, &self.tabs, self.active, &self.groups)
                } else if let Some((tabs, active, groups)) = &ws.stash {
                    Self::workspace_snapshot(&ws.name, tabs, *active, groups)
                } else {
                    Self::workspace_snapshot(&ws.name, &[], 0, &[])
                }
            })
            .collect();
        session_state::SessionState {
            active_workspace: self.active_workspace,
            workspaces,
        }
    }

    fn workspace_snapshot(
        name: &str,
        tabs: &[Tab],
        active: usize,
        groups: &[TabGroup],
    ) -> session_state::WorkspaceState {
        session_state::WorkspaceState {
            name: name.to_owned(),
            active,
            groups: groups
                .iter()
                .map(|g| session_state::GroupState {
                    name: g.name.clone(),
                    collapsed: g.collapsed,
                })
                .collect(),
            tabs: tabs.iter().map(Self::tab_snapshot).collect(),
        }
    }

    fn tab_snapshot(tab: &Tab) -> session_state::TabState {
        // Leaves serialize in `panes()` order; record each pane's cwd in that same order so
        // `restore_tab` pairs them back by index.
        let cwds = tab
            .tree
            .panes()
            .iter()
            .map(|id| tab.pane_cwd.get(id).cloned())
            .collect();
        session_state::TabState {
            layout: tab.tree.to_layout(),
            cwds,
            pinned: tab.pinned,
            custom_title: tab.custom_title.clone(),
            group: tab.group,
            activated: tab.activated,
        }
    }

    /// Rebuild all workspaces/tabs/groups from a saved [`SessionState`](session_state::SessionState), spawning a shell per
    /// restored pane in its saved cwd (layout only - the prior process is never re-run). Called
    /// once from [`resumed`](Self::resumed) before the initial `sync_layout`, so the renderer
    /// (hence cell metrics) already exists. A session with no workspaces is ignored.
    fn restore_session(&mut self, state: session_state::SessionState) {
        let Some((cell_w, cell_h, _)) = self.renderer.as_ref().map(Renderer::cell_metrics) else {
            return;
        };
        if state.workspaces.is_empty() {
            return;
        }
        let inset = self.pane_inset();
        let status_h = if self.config.appearance.show_status_line {
            statusline::HEIGHT * scale32(self.scale)
        } else {
            0.0
        };
        let viewport = self.viewport_rect();
        let active_ws = state.active_workspace.min(state.workspaces.len() - 1);
        let mut workspaces = Vec::with_capacity(state.workspaces.len());
        let mut live: Option<(Vec<Tab>, usize, Vec<TabGroup>)> = None;
        for (i, wss) in state.workspaces.into_iter().enumerate() {
            let groups = wss
                .groups
                .into_iter()
                .map(|g| TabGroup {
                    name: g.name,
                    collapsed: g.collapsed,
                })
                .collect();
            let mut tabs: Vec<Tab> = wss
                .tabs
                .iter()
                .map(|ts| self.restore_tab(ts, viewport, cell_w, cell_h, inset, status_h))
                .collect();
            if tabs.is_empty() {
                tabs.push(Tab::new());
            }
            let active = wss.active.min(tabs.len() - 1);
            if i == active_ws {
                live = Some((tabs, active, groups));
                workspaces.push(Workspace {
                    name: wss.name,
                    stash: None,
                });
            } else {
                workspaces.push(Workspace {
                    name: wss.name,
                    stash: Some((tabs, active, groups)),
                });
            }
        }
        let (tabs, active, groups) = live.expect("the active workspace is always rebuilt");
        self.tabs = tabs;
        self.active = active;
        self.groups = groups;
        self.workspaces = workspaces;
        self.active_workspace = active_ws;
    }

    /// Rebuild one tab from its saved state: reconstruct the pane tiling and spawn a shell per
    /// leaf in its saved cwd (or the default when the cwd is missing / gone). A pane whose shell
    /// fails to spawn is left empty; `sync_layout` re-spawns it when the tab next becomes active.
    fn restore_tab(
        &self,
        ts: &session_state::TabState,
        viewport: Rect,
        cell_w: f32,
        cell_h: f32,
        inset: f32,
        status_h: f32,
    ) -> Tab {
        let shell = self.config.shell.program.clone();
        let cursor = config_cursor_shape(self.config.appearance.cursor);
        let mut tab = Tab::new();
        tab.tree = PaneTree::from_layout(&ts.layout);
        tab.pinned = ts.pinned;
        tab.custom_title.clone_from(&ts.custom_title);
        tab.group = ts.group;
        tab.activated = ts.activated;
        let rects: HashMap<PaneId, Rect> = tab.tree.layout(viewport).into_iter().collect();
        for (i, id) in tab.tree.panes().into_iter().enumerate() {
            let saved_cwd = ts.cwds.get(i).and_then(Clone::clone);
            let start = saved_cwd.as_deref().map(expand_home);
            let rect = rects.get(&id).copied().unwrap_or(viewport);
            let target = pane_dims(rect, cell_w, cell_h, inset, status_h);
            let proxy = self.proxy.clone();
            match Terminal::spawn_shell_in(
                target.0,
                target.1,
                Some(shell.as_str()),
                start.as_deref(),
                move || {
                    let _ = proxy.send_event(Wakeup::Shell);
                },
            ) {
                Ok(term) => {
                    term.set_default_cursor_shape(cursor);
                    tab.panes.insert(id, term);
                    tab.dims.insert(id, target);
                    if let Some(cwd) = saved_cwd {
                        tab.pane_cwd.insert(id, cwd);
                    }
                }
                Err(err) => {
                    tracing::error!(%err, "failed to spawn a restored pane's shell");
                }
            }
        }
        tab
    }

    /// Dispatch a decoded tab chord to its handler.
    fn run_tab_action(&mut self, action: TabAction) {
        match action {
            TabAction::New => self.new_tab(),
            TabAction::Close => self.request_close_tab(),
            TabAction::Goto(index) => self.goto_tab_in_active_group(index),
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

    /// Keys while the first-run onboarding modal is up (design §10.1): Tab / ↑↓ move focus,
    /// ←→ change the shell/theme (theme previews live), Enter runs Start (Skip when focused on
    /// it), Esc skips. Everything else is swallowed (the modal is captured).
    fn on_onboarding_key(&mut self, key_event: &KeyEvent) {
        let Some(onb) = self.onboarding.as_mut() else {
            return;
        };
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => {
                self.finish_onboarding(false);
                return;
            }
            Key::Named(NamedKey::Enter) => {
                let start = onb.enter_is_start();
                self.finish_onboarding(start);
                return;
            }
            Key::Named(NamedKey::Tab) => onb.cycle_focus(!self.modifiers.shift_key()),
            Key::Named(NamedKey::ArrowDown) => onb.cycle_focus(true),
            Key::Named(NamedKey::ArrowUp) => onb.cycle_focus(false),
            Key::Named(dir @ (NamedKey::ArrowRight | NamedKey::ArrowLeft)) => {
                // `horizontal` always changes the shell/theme selection; a theme change also
                // asks for a live preview (applied after the `onb` borrow ends).
                let theme_changed = onb.horizontal(dir == NamedKey::ArrowRight);
                if theme_changed {
                    let theme = onb.theme_name();
                    self.apply_theme(theme);
                }
            }
            _ => {}
        }
        self.request_redraw();
    }

    /// Dismiss the onboarding modal: on `start`, apply the picked shell + theme; on Skip, revert
    /// to the defaults (login shell + Ossein Dark). Either way the config is written (so next
    /// launch skips onboarding, Hard rule 1) and the active tab's shells respawn under the
    /// chosen shell.
    fn finish_onboarding(&mut self, start: bool) {
        let Some(onb) = self.onboarding.take() else {
            return;
        };
        if start {
            onb.shell_program()
                .clone_into(&mut self.config.shell.program);
            self.apply_theme(onb.theme_name()); // already previewed; idempotent
        } else {
            self.config.shell.program.clear();
            self.apply_theme("ossein-dark");
        }
        if let Err(err) = self.config.save_default() {
            tracing::warn!(%err, "failed to write config after onboarding");
        }
        // Respawn the active tab's shells so the chosen shell takes effect on the first pane
        // (it was spawned with the login shell before onboarding was dismissed).
        let ws = self.active_tab_mut();
        ws.panes.clear();
        ws.dims.clear();
        self.sync_layout();
        self.request_redraw();
    }

    /// The onboarding card's centered rectangle (physical px), shared by its frame builder and
    /// hit-test so a click lands on exactly what is drawn.
    fn onboarding_panel(&self) -> PxRect {
        let scale = scale32(self.scale);
        let (surface_w, surface_h) = (dim_f32(self.size.0), dim_f32(self.size.1));
        let (panel_w, panel_h) = onboarding::Onboarding::card_size(scale);
        let panel_w = panel_w.min(surface_w * 0.9);
        PxRect {
            x: ((surface_w - panel_w) / 2.0).max(0.0),
            y: ((surface_h - panel_h) / 2.0).max(0.0),
            w: panel_w,
            h: panel_h,
        }
    }

    /// Handle a click while the onboarding modal is up: select a shell/theme (theme previews
    /// live) or activate Skip / Start.
    fn on_onboarding_click(&mut self) {
        let panel = self.onboarding_panel();
        let (px, py) = point_f32(self.pointer);
        let Some(hit) = onboarding::hit(panel, scale32(self.scale), px, py) else {
            return;
        };
        match hit {
            onboarding::Hit::Skip => self.finish_onboarding(false),
            onboarding::Hit::Start => self.finish_onboarding(true),
            other => {
                if let Some(onb) = self.onboarding.as_mut() {
                    onb.click(other);
                    if matches!(other, onboarding::Hit::Theme(_)) {
                        let theme = onb.theme_name();
                        self.apply_theme(theme);
                    }
                }
                self.request_redraw();
            }
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
                // A file entry (files mode) types its path into the focused pane; otherwise run
                // the selected command / tab action. `⌘↵` "runs in a new pane" (design §11): it
                // first splits a fresh pane (respecting the 8-pane cap) so the path lands there.
                let file = self.palette.selected_file();
                let action = self.palette.selected_action();
                let new_pane = self.modifiers.super_key();
                self.palette.close();
                self.palette_anim = None;
                if let Some(path) = file {
                    if new_pane {
                        self.apply_pane_action(PaneAction::Split(Dir::Right));
                    }
                    self.type_into_focused(&format!("{path} "));
                } else if let Some(action) = action {
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
            Action::GotoTab(index) => self.goto_tab_in_active_group(index),
            Action::TogglePin => self.toggle_pin(),
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
                // A transient success toast confirms the commit (design §12 flow 3: "a toast
                // shows the short SHA"); the git dock's inline hint keeps the ⌘U Undo affordance.
                if !sha.is_empty() {
                    self.show_toast(format!("Committed {sha}"), ToastKind::Success);
                }
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

    /// Handle the tmux-style pane leader (`[panes] leader`, default `ctrl+a`, §11). Returns
    /// whether the key was consumed. Pressing the leader arms it; the next key is a leader pane
    /// chord (`hjkl`/`⇧hjkl`/`z`/`x`/`|`/`-`), or anything else cancels it.
    fn on_leader(&mut self, key_event: &KeyEvent) -> bool {
        let PhysicalKey::Code(code) = key_event.physical_key else {
            // A pending leader is cancelled by a non-code key (e.g. a dead key).
            if self.leader_pending {
                self.leader_pending = false;
                return true;
            }
            return false;
        };
        if self.leader_pending {
            self.leader_pending = false;
            if let Some(action) = leader_chord(code, self.modifiers) {
                if action == PaneAction::Close {
                    self.request_close_pane();
                } else {
                    self.apply_pane_action(action);
                }
            }
            // Consume the key either way (a non-chord just cancels the leader).
            return true;
        }
        // Arm the leader when its exact chord is pressed.
        let mask = ModifiersState::CONTROL
            | ModifiersState::ALT
            | ModifiersState::SHIFT
            | ModifiersState::SUPER;
        if let Some((mods, key)) = parse_leader(&self.config.panes.leader) {
            if code == key && (self.modifiers & mask) == mods {
                self.leader_pending = true;
                return true;
            }
        }
        false
    }

    /// The global Session shortcuts (design §11): `⌥⌘←` rewind one step, `⌥⌘→` fast-forward,
    /// `⌥⌘0` return to now. These work whether or not the timeline dock is open; a rewind /
    /// fast-forward opens it so the rewound state is visible. Returns whether a chord fired.
    fn on_session_chord(&mut self, key_event: &KeyEvent) -> bool {
        if !(self.modifiers.alt_key() && self.modifiers.super_key()) {
            return false;
        }
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::ArrowLeft) => {
                if !self.timeline.open {
                    self.toggle_timeline();
                }
                self.timeline_step(-1);
            }
            Key::Named(NamedKey::ArrowRight) => {
                if !self.timeline.open {
                    self.toggle_timeline();
                }
                self.timeline_step(1);
            }
            Key::Character("0") => self.timeline_return_to_now(),
            _ => return false,
        }
        true
    }

    /// Move the timeline selection by `delta` and reconcile the shadow worktree to it.
    fn timeline_step(&mut self, delta: i32) {
        if self.timeline_move_selection(delta) {
            self.reconcile_shadow();
        }
        self.request_redraw();
    }

    /// Snap the timeline selection back to now (HEAD), discarding any shadow worktree.
    fn timeline_return_to_now(&mut self) {
        if self.timeline_select_now() {
            self.reconcile_shadow();
        }
        self.request_redraw();
    }

    /// The Cmd/Super chords, matched on the logical character: `⌘K` palette, `⌘Q` quit, `⌘C`/`⌘V`
    /// copy/paste, `⌘B`/`⇧⌘B` sidebar toggle/mode, `⇧⌘G` git dock, `⇧⌘H` timeline, `⇧⌘P` pin,
    /// `⌘,` settings. Returns `true` when a chord fired (the caller then stops routing the key).
    fn on_super_chord(&mut self, event_loop: &ActiveEventLoop, key_event: &KeyEvent) -> bool {
        if !self.modifiers.super_key() {
            return false;
        }
        let Key::Character(ch) = key_event.logical_key.as_ref() else {
            return false;
        };
        let shift = self.modifiers.shift_key();
        if ch.eq_ignore_ascii_case("k") {
            self.open_palette();
        } else if ch.eq_ignore_ascii_case("q") {
            event_loop.exit();
        } else if ch.eq_ignore_ascii_case("c") {
            self.copy_selection();
        } else if ch.eq_ignore_ascii_case("v") {
            self.paste();
        } else if ch.eq_ignore_ascii_case("b") {
            if shift {
                self.cycle_sidebar_mode();
            } else {
                self.toggle_sidebar();
            }
        } else if shift && ch.eq_ignore_ascii_case("g") {
            self.toggle_git_dock();
        } else if shift && ch.eq_ignore_ascii_case("h") {
            self.toggle_timeline();
        } else if shift && ch.eq_ignore_ascii_case("p") {
            self.toggle_pin();
        } else if shift && ch.eq_ignore_ascii_case("n") {
            self.new_group();
        } else if shift && ch.eq_ignore_ascii_case("t") {
            self.reopen_closed_tab();
        } else if ch == "," {
            self.open_settings();
        } else if ch == "/" {
            self.cheatsheet_open = !self.cheatsheet_open;
            self.request_redraw();
        } else if ch.eq_ignore_ascii_case("f") {
            self.open_find();
        } else if ch.eq_ignore_ascii_case("l") {
            self.clear_focused_scrollback();
        } else if ch == "=" || ch == "+" {
            self.adjust_font_size(self.config.appearance.font_size.saturating_add(1));
        } else if ch == "-" || ch == "_" {
            self.adjust_font_size(self.config.appearance.font_size.saturating_sub(1));
        } else if ch == "0" {
            self.adjust_font_size(DEFAULT_FONT_SIZE);
        } else {
            return false;
        }
        true
    }

    /// Whether the caret is in the "off" half of its blink cycle right now, measured from
    /// `blink_epoch` (the last keypress). The renderer hides the focused caret when this is true
    /// and the program requested a blinking cursor (design §06).
    fn cursor_blink_off(&self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.blink_epoch);
        (elapsed.as_millis() / CURSOR_BLINK_INTERVAL.as_millis()) % 2 == 1
    }

    /// Whether the focused pane's cursor is currently requesting a blink (so the loop knows to
    /// wake at the next toggle to repaint it).
    fn focused_cursor_blinking(&self) -> bool {
        self.focused_term_ref()
            .is_some_and(skelly_term::Terminal::cursor_blinking)
    }

    /// Clear the focused pane's scrollback history (design §11 `⌘L`).
    fn clear_focused_scrollback(&mut self) {
        if let Some(term) = self.focused_term() {
            term.clear_scrollback();
            self.request_redraw();
        }
    }

    /// Set the terminal font size to `size` (clamped to the config's valid 8..=32 range), the
    /// live `⌘=/-/0` bindings (design §11): update the config (source of truth, Hard rule 1),
    /// rebuild the renderer's cell metrics, and re-fit the PTY grids to the new cell size.
    fn adjust_font_size(&mut self, size: u16) {
        let clamped = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if clamped == self.config.appearance.font_size {
            return;
        }
        self.config.appearance.font_size = clamped;
        let line_height = self.config.appearance.line_height;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_font_size(clamped, line_height);
        }
        self.sync_layout();
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
        // The first-run onboarding modal (design §10.1) captures all input while up.
        if self.onboarding.is_some() {
            self.on_onboarding_key(key_event);
            return;
        }
        // The right-click tab menu (design §08) captures input while open.
        if self.context_menu.is_some() {
            self.on_context_menu_key(key_event);
            return;
        }
        // The keybinding cheatsheet (§11) captures input: Esc or ⌘/ closes, everything else is
        // swallowed while it is up.
        if self.cheatsheet_open {
            let toggle = self.modifiers.super_key()
                && matches!(key_event.logical_key.as_ref(), Key::Character(c) if c == "/");
            if toggle || matches!(key_event.logical_key.as_ref(), Key::Named(NamedKey::Escape)) {
                self.cheatsheet_open = false;
                self.request_redraw();
            }
            return;
        }
        // The scrollback find bar (§11) captures input while open (typing / navigate / Esc). `⌘F`
        // and `⌘K` etc. still route (super chords), so let those fall through first.
        if self.find.is_some() && !self.modifiers.super_key() && self.on_find_key(key_event) {
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
        // `⇧⌘F` toggles the open dock's full-width, whether or not the dock is focused (so it
        // works alongside typing in the panes). Checked before the focus-gated dock capture.
        if self.on_dock_fullwidth_chord(key_event) {
            return;
        }
        // The right docks (git diff / timeline) capture keys only while *focused* (clicked into);
        // otherwise they are passive layers over a live terminal (Hard rule 4) and keys reach the
        // focused pane. The toggle chords (⇧⌘G / ⇧⌘H) and session chords stay global below, so a
        // dock can always be closed / scrubbed without focusing it.
        if self.git_dock.open && self.dock_focused {
            self.on_gitdock_key(event_loop, key_event);
            return;
        }
        if self.timeline.open && self.dock_focused {
            self.on_timeline_key(event_loop, key_event);
            return;
        }
        // Renaming a tab (F2) captures every typed key into its name buffer until Enter/Esc.
        if self.renaming.is_some() {
            self.on_rename_key(key_event);
            return;
        }
        if matches!(key_event.logical_key.as_ref(), Key::Named(NamedKey::F2)) {
            self.start_rename();
            return;
        }
        // The tmux-style pane leader (`[panes] leader`): arm it, or apply the pending chord.
        // Checked before terminal input so the leader key doesn't reach the shell.
        if self.on_leader(key_event) {
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
        // Session shortcuts (`⌥⌘←/→` step the rewind, `⌥⌘0` return to now, §11) work globally -
        // opening the timeline dock so the rewound state is visible. Checked before the plain
        // super chords so `⌥⌘0` isn't swallowed by `⌘0` (reset font size).
        if self.on_session_chord(key_event) {
            return;
        }
        // Platform combos (Cmd/Super + K/Q/C/V/B/L, font size, and the ⇧-modified dock/pin
        // chords). The terminal owns every other key - Ctrl+C etc. still reach the shell.
        if self.on_super_chord(event_loop, key_event) {
            return;
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
        if self.on_scrollback_key(key_event) {
            return;
        }
        self.forward_key_to_focused(key_event);
    }

    /// Shift + PageUp/PageDown scrolls the focused pane's scrollback. Returns whether it consumed
    /// the key.
    fn on_scrollback_key(&mut self, key_event: &KeyEvent) -> bool {
        if !self.modifiers.shift_key() {
            return false;
        }
        let up = match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::PageUp) => true,
            Key::Named(NamedKey::PageDown) => false,
            _ => return false,
        };
        if let Some(term) = self.focused_term() {
            term.scroll_page(up);
        }
        self.request_redraw();
        true
    }

    /// Route a plain key to the focused pane's shell (the fall-through after every chord).
    /// A dead pane instead restarts on Enter and swallows the rest; a live pane forwards
    /// the bytes, and submitting a command (Enter) retires the tab's empty state.
    fn forward_key_to_focused(&mut self, key_event: &KeyEvent) {
        // Typing resets the caret-blink cycle so the cursor is solid while the user works.
        self.blink_epoch = Instant::now();
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
        let application_cursor = self
            .focused_term_ref()
            .is_some_and(Terminal::application_cursor_mode);
        let keyboard_mode = self
            .focused_term_ref()
            .map(Terminal::keyboard_mode)
            .unwrap_or_default();
        if let Some(bytes) =
            key_to_bytes(key_event, self.modifiers, application_cursor, keyboard_mode)
        {
            if let Some(term) = self.focused_term() {
                // Typing jumps back to the live prompt.
                term.scroll_to_bottom();
                term.write(&bytes);
            }
            let now = Instant::now();
            let ws = self.active_tab_mut();
            ws.selection = None; // typing clears the selection
                                 // The empty state retires the moment the user starts typing:
                                 // kick off the mark + chips fade-out (§10.2) and mark the tab
                                 // in use. Any real keystroke counts, Enter included.
            if ws.is_empty_state() {
                ws.empty_fade = Some(motion::Anim::start(now, EMPTY_FADE));
                ws.activated = true;
            }
        }
    }

    /// Extend the active drag selection to the pointer (a no-op unless a drag is live).
    fn on_cursor_moved(&mut self) {
        // The right-click menu takes the pointer while open: hovering highlights its items.
        if self.context_menu.is_some() {
            if let Some(panel) = self.context_menu_panel() {
                let (px, py) = point_f32(self.pointer);
                let scale = scale32(self.scale);
                if let Some(menu) = self.context_menu.as_mut() {
                    menu.hover(panel, scale, px, py);
                }
                self.request_redraw();
            }
            return;
        }
        // Dragging the dock's left edge resizes it.
        if self.dock_resizing {
            self.resize_dock_to_pointer();
            return;
        }
        // Dragging the sidebar's right edge resizes it (snapping to rail / hidden, design §12).
        if self.sidebar_resizing {
            self.resize_sidebar_to_pointer();
            return;
        }
        // Dragging a sidebar tab over another reorders the tab list.
        if let Some(from) = self.dragging_tab {
            if let Some(sidebar::Hit::Tab(target)) = self.sidebar_hit() {
                if target != from {
                    self.move_tab(from, target);
                    self.dragging_tab = Some(target);
                    self.request_redraw();
                }
            }
            return;
        }
        // Show a horizontal-resize cursor when hovering either draggable edge (dock or sidebar).
        if let Some(window) = self.window.as_ref() {
            window.set_cursor(if self.on_dock_edge() || self.on_sidebar_edge() {
                CursorIcon::EwResize
            } else {
                CursorIcon::Default
            });
        }
        // Auto-hide rail: hovering the slim rail expands it to the full panel (design §08,
        // "56px · hover to expand"); the pointer leaving the panel collapses it. Overlay - the
        // pane viewport keeps the rail footprint, so the terminal doesn't reflow on hover.
        if self.sidebar.is_rail() {
            let (px, _) = point_f32(self.pointer);
            let within = px < self.sidebar_width_px();
            if within != self.rail_expanded {
                self.rail_expanded = within;
                self.request_redraw();
            }
        }
        // Hover tooltips (design §09): track the icon-only element under the pointer + when the
        // hover began; the tooltip reveals after `HOVER_DELAY` (in `about_to_wait`). Moving to a
        // different element (or off) restarts the timer and hides any shown tip.
        let label = self.tooltip_label_at_pointer();
        let changed = match (&self.hover_tip, &label) {
            (Some((cur, _)), Some(new)) => cur != new,
            (None, None) => false,
            _ => true,
        };
        if changed {
            if self.tooltip_visible {
                self.request_redraw(); // repaint to remove the old tooltip
            }
            self.tooltip_visible = false;
            self.hover_tip = label.map(|l| (l, Instant::now()));
        }
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
        // The first-run onboarding modal captures clicks: hit a control (select / focus) or a
        // button (Skip / Start), and swallow clicks elsewhere so nothing behind it reacts.
        if self.onboarding.is_some() {
            if state == ElementState::Pressed {
                self.on_onboarding_click();
            }
            return;
        }
        // The right-click tab menu captures clicks: an item runs its action, a click elsewhere
        // dismisses it (design §08). Either way nothing behind the menu reacts.
        if self.context_menu.is_some() {
            if state == ElementState::Pressed {
                match self.context_menu_hit() {
                    Some(i) => {
                        let action = self.context_menu.as_mut().and_then(|m| m.action_at(i));
                        if let Some(action) = action {
                            self.run_menu_action(action);
                        }
                    }
                    None => self.close_context_menu(),
                }
            }
            return;
        }
        // A click during an overlay's fade-out dismisses it (and is consumed) instead of
        // falling through to the panes behind the still-visible palette / confirm modal.
        if self.settle_confirm_close() || self.settle_palette_close() {
            return;
        }
        match state {
            ElementState::Pressed => {
                // The expand toggle straddles the dock's left edge, so check it *before* the
                // resize-edge grab - a click on the handle flips the dock width, not a resize.
                if let Some(btn) = self.dock_button_rect() {
                    let (px, py) = point_f32(self.pointer);
                    if px >= btn.x && px < btn.x + btn.w && py >= btn.y && py < btn.y + btn.h {
                        self.toggle_dock_full_width();
                        return;
                    }
                }
                // Grabbing the open dock's left edge starts a resize drag (consumes the press).
                if self.on_dock_edge() {
                    self.dock_resizing = true;
                    return;
                }
                // Grabbing the full sidebar's right edge starts a resize drag (design §12).
                if self.on_sidebar_edge() {
                    self.sidebar_resizing = true;
                    return;
                }
                // Clicking the open right dock focuses it (its keyboard controls take over);
                // clicking anywhere else returns keyboard focus to the terminal panes.
                if self.pointer_in_right_dock() {
                    if !self.dock_focused {
                        self.dock_focused = true;
                        self.request_redraw();
                    }
                    return;
                }
                self.dock_focused = false;
                if let Some(hit) = self.sidebar_hit() {
                    match hit {
                        sidebar::Hit::Workspace(index) => self.switch_workspace(index),
                        sidebar::Hit::AddWorkspace => self.add_workspace(),
                        sidebar::Hit::CommandInput => self.open_palette(),
                        // `sidebar_hit` maps a pinned-tile hit to `Tab`, so `Pinned` is
                        // unreachable here; both just activate the tab defensively. A press also
                        // arms a drag so moving the pointer reorders the tab.
                        sidebar::Hit::Tab(index) | sidebar::Hit::Pinned(index) => {
                            self.goto_tab(index);
                            self.dragging_tab = Some(index);
                        }
                        // Clicking a group header collapses / expands it (design §08 #5).
                        sidebar::Hit::GroupHeader(gi) => self.toggle_group(gi),
                        sidebar::Hit::NewTab => self.new_tab(),
                        sidebar::Hit::Util(action) => self.on_util_action(action),
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
                self.dock_resizing = false;
                self.dragging_tab = None;
                // Persist the settled sidebar width/mode once when a resize drag ends.
                if self.sidebar_resizing {
                    self.end_sidebar_resize();
                }
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
            // Scrolling the view produces no shell output, so nothing else wakes the loop to
            // repaint - request it here so the scrolled scrollback shows immediately.
            self.request_redraw();
        }
    }
}

/// Owned per-pane frame data the borrowed [`PaneView`]s point at during a repaint.
struct PaneFrame {
    rect: PxRect,
    origin: (f32, f32),
    rows: Vec<Vec<GridCell>>,
    cursor: (usize, usize),
    cursor_shape: CursorShape,
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
            cursor_shape: self.cursor_shape,
            selection: &self.selection,
            focused: self.focused,
            logo: self.logo,
        }
    }
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

/// The scrollback find bar state (design §11 `⌘F`): the query and the current match hit.
struct FindState {
    query: String,
    hit: Option<skelly_term::FindHit>,
    /// Whether a non-empty query has been searched (so the bar can show "no match").
    searched: bool,
}

/// Owned first-run onboarding frame data the borrowed [`OverlayView`] points at.
struct OnboardingFrame {
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
    /// The label clip (panel widened left for the edge-straddling toggle glyph).
    clip: PxRect,
    quads: Vec<skelly_render::ChromeQuad>,
    labels: Vec<skelly_render::ProseLabel>,
}

/// Owned timeline-dock frame data the borrowed [`TimelineView`] points at.
struct TimelineFrame {
    panel: PxRect,
    /// The label clip (panel widened left for the edge-straddling toggle glyph).
    clip: PxRect,
    quads: Vec<skelly_render::ChromeQuad>,
    labels: Vec<skelly_render::ProseLabel>,
}

/// `path` with the home directory collapsed to `~` (for the status line), else the path
/// as-is. Best-effort; falls back to the lossy string form.
/// Expand a home-collapsed cwd (`~` / `~/rel`, as [`home_relative`] produces) back to an
/// absolute path, for re-opening a restored pane in its saved directory. A path that is
/// already absolute (was outside `$HOME`) is returned unchanged.
fn expand_home(s: &str) -> std::path::PathBuf {
    if let Some(rest) = s.strip_prefix('~') {
        if let Some(home) = std::env::var_os("HOME") {
            let mut path = std::path::PathBuf::from(home);
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            if !rest.is_empty() {
                path.push(rest);
            }
            return path;
        }
    }
    std::path::PathBuf::from(s)
}

fn home_relative(path: &std::path::Path) -> String {
    let full = path.to_string_lossy().into_owned();
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::Path::new(&home);
        if let Ok(rel) = path.strip_prefix(home) {
            return if rel.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", rel.to_string_lossy())
            };
        }
    }
    full
}

/// Backslash-escape a dropped file path so it pastes as a single shell argument (spaces and
/// shell metacharacters escaped). Alphanumerics, path separators, and safe punctuation pass
/// through; non-ASCII (UTF-8) bytes are left intact so the path stays valid.
fn shell_escape_path(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if c.is_ascii_alphanumeric()
            || matches!(
                c,
                '/' | '.' | '_' | '-' | '~' | '+' | ',' | '=' | ':' | '@' | '%'
            )
            || !c.is_ascii()
        {
            out.push(c);
        } else {
            out.push('\\');
            out.push(c);
        }
    }
    out
}

/// The login shell's command name (the `SHELL` env's basename), for the status line;
/// defaults to `sh`.
fn shell_name() -> String {
    std::env::var_os("SHELL")
        .map(std::path::PathBuf::from)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "sh".to_owned())
}

/// Modal editors whose terminal cursor shape signals their edit mode (block = normal, bar =
/// insert, underline = replace). Gating the status-line mode on a known modal editor (design
/// §10.4) avoids showing a bogus "mode" at an ordinary shell prompt.
const MODAL_EDITORS: &[&str] = &[
    "nvim", "vim", "vi", "nvi", "hx", "helix", "kak", "vis", "amp",
];

/// The editor mode to show in the focused pane's status line (design §10.4): `None` unless the
/// foreground process `job` is a known modal editor, else its mode derived from the real cursor
/// `shape` the editor set. This reports the editor's actual mode (from `DECSCUSR`), not a guess -
/// the filetype segment the guide also shows stays out (it needs editor RPC Skelly lacks).
fn editor_mode(job: Option<&str>, shape: skelly_term::CursorShape) -> Option<&'static str> {
    let name = job?;
    if !MODAL_EDITORS.iter().any(|e| name.eq_ignore_ascii_case(e)) {
        return None;
    }
    Some(match shape {
        skelly_term::CursorShape::Bar => "INSERT",
        skelly_term::CursorShape::Underline => "REPLACE",
        skelly_term::CursorShape::Block | skelly_term::CursorShape::Hidden => "NORMAL",
    })
}

/// Map the configured default cursor style (`appearance.cursor`) to the terminal's cursor shape,
/// applied as each pane's resting cursor (a program's `DECSCUSR` still overrides it).
fn config_cursor_shape(cursor: skelly_config::CursorStyle) -> skelly_term::CursorShape {
    match cursor {
        skelly_config::CursorStyle::Block => skelly_term::CursorShape::Block,
        skelly_config::CursorStyle::Bar => skelly_term::CursorShape::Bar,
        skelly_config::CursorStyle::Underline => skelly_term::CursorShape::Underline,
    }
}

/// Map the terminal's requested cursor shape to the renderer's, so the drawn cursor honors what
/// the running program set via `DECSCUSR` (e.g. vim's block/bar/underline per mode).
fn render_cursor_shape(shape: skelly_term::CursorShape) -> CursorShape {
    match shape {
        skelly_term::CursorShape::Block => CursorShape::Block,
        skelly_term::CursorShape::Bar => CursorShape::Bar,
        skelly_term::CursorShape::Underline => CursorShape::Underline,
        skelly_term::CursorShape::Hidden => CursorShape::Hidden,
    }
}

/// The editor filetype to show in the focused pane's status line (design §10.4): `None` unless
/// the foreground process `job` is a modal editor that named an open file in the window `title`
/// (e.g. `main.rs (…) - NVIM`). The filetype is derived from that real filename's extension - not
/// a guess - and only for a known code extension, so an unrelated title never shows a bogus one.
fn editor_filetype(job: Option<&str>, title: Option<&str>) -> Option<&'static str> {
    let name = job?;
    if !MODAL_EDITORS.iter().any(|e| name.eq_ignore_ascii_case(e)) {
        return None;
    }
    // Scan the title's tokens for the first `name.ext` with a recognized code extension.
    title?
        .split(|c: char| c.is_whitespace() || matches!(c, '/' | '\\' | '(' | ')' | '[' | ']'))
        .filter_map(|tok| tok.rsplit_once('.'))
        .find_map(|(_, ext)| filetype_for_ext(ext))
}

/// The editor filetype name for a file extension (nvim's names for the common code types), or
/// `None` for an unrecognized extension (so a stray dotted token never reads as a filetype).
fn filetype_for_ext(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" | "cjs" | "mjs" => "javascript",
        "ts" => "typescript",
        "jsx" => "javascriptreact",
        "tsx" => "typescriptreact",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" => "cpp",
        "rb" => "ruby",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "lua" => "lua",
        "vim" => "vim",
        "sh" | "bash" | "zsh" => "sh",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "scss",
        "sql" => "sql",
        "zig" => "zig",
        _ => return None,
    })
}

/// The cap on files the palette's `/` mode gathers (keeps the walk fast on a huge tree).
const MAX_WALK_FILES: usize = 4000;
/// The deepest directory level the file walk descends.
const MAX_WALK_DEPTH: usize = 8;

/// The working-directory files for the palette's `/` files mode (design §10.8): a bounded walk
/// of the process cwd returning paths relative to it, skipping hidden entries and heavy vendor /
/// vcs directories. Best-effort - unreadable directories are silently skipped.
fn gather_files() -> Vec<String> {
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut out = Vec::new();
    walk_files(&root, &root, &mut out, 0);
    out
}

fn walk_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>, depth: usize) {
    if out.len() >= MAX_WALK_FILES || depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_WALK_FILES {
            return;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Skip hidden entries + notoriously large vendor / build / vcs trees.
        if name.starts_with('.') || matches!(name.as_ref(), "node_modules" | "target" | "vendor") {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            walk_files(root, &path, out, depth + 1);
        } else if file_type.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
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

/// The working directory of process `pid`, best-effort (for the live status-line cwd + the
/// cwd-based tab title). On Linux this reads the `/proc/<pid>/cwd` symlink; macOS has no
/// `/proc`, so it shells `lsof` for the process's `cwd` descriptor. `None` on any failure - the
/// caller keeps the pane's last known cwd.
#[cfg(target_os = "linux")]
fn process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// See [`process_cwd`]. macOS build: `lsof -a -p <pid> -d cwd -Fn` prints field-prefixed lines;
/// the working directory is the one prefixed with `n`.
#[cfg(target_os = "macos")]
fn process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix('n'))
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from)
}

/// See [`process_cwd`]. Fallback for platforms without a known cwd source (Skelly targets macOS
/// and Linux; this keeps the build honest elsewhere).
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_cwd(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

/// The final path component of a (possibly home-collapsed) directory string, for the cwd-based
/// tab title (design §10.3): `~/Developer/skelly` -> `skelly`, `~` -> `~`, `/` -> `/`.
fn dir_basename(cwd: &str) -> String {
    cwd.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(cwd)
        .to_owned()
}

/// Resolve a terminal cell into a render cell against the active ANSI palette,
/// folding in the palette-dependent SGR effects: *dim* reduces the foreground
/// intensity and *reverse video* swaps foreground and background (using the
/// palette's default background when the cell has none). Bold/italic/underline pass
/// through for the renderer to apply.
fn resolve_cell(cell: &TermCell, palette: &AnsiPalette, bold_bright: bool) -> GridCell {
    let bold = cell.attrs.contains(CellAttrs::BOLD);
    // With `bold_is_bright`, bold text in a normal ANSI color (0-7) renders in that color's
    // bright variant (8-15) - the common terminal convention.
    let fg_color = match cell.fg {
        CellColor::Indexed(i) if bold && bold_bright && i < 8 => CellColor::Indexed(i + 8),
        other => other,
    };
    let mut fg = resolve_fg(fg_color, palette);
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
        bold,
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
        // macOS: a transparent, full-size-content-view title bar so app chrome extends to the
        // top edge with the traffic lights floating inset top-left (design §08 anatomy #1, the
        // standard native-terminal look). The title text is hidden; the buttons stay visible +
        // functional, and the title-bar height stays draggable. `content_top` reserves the
        // strip below them so nothing sits under the lights.
        #[cfg(target_os = "macos")]
        let attributes = {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attributes
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true)
        };
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                tracing::error!(%err, "failed to create window");
                event_loop.exit();
                return;
            }
        };
        // Install the native macOS application menu so the standard system shortcuts (⌘H hide,
        // ⌘M minimize, ⌘Q quit, …) work - winit provides none. `resumed` runs on the main thread.
        #[cfg(target_os = "macos")]
        if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            menu::install(mtm);
        }

        let size = window.inner_size();
        self.scale = window.scale_factor();
        self.measure.set_scale(scale32(self.scale));
        self.size = (size.width, size.height);
        // Seed the status-line context (cwd · shell) for the first paint; branch/dirty come from
        // the active repo's timeline projection once seeded (below).
        self.status_cwd = home_relative(&std::env::current_dir().unwrap_or_default());
        self.status_shell = shell_name();
        let renderer = Renderer::new(
            window.clone(),
            size.width,
            size.height,
            self.scale,
            &self.config.appearance,
        );

        self.window = Some(window);
        self.renderer = Some(renderer);
        // Restore the saved workspace / tab / pane layout before the first `sync_layout`, so
        // its shells are spawned in their saved cwds (design/README.md persist scope: layout
        // only). `sync_layout` then just re-fits them and heals any that failed to spawn.
        if self.config.session.persist {
            if let Some(state) = session_state::SessionState::load_default() {
                self.restore_session(state);
            }
        }
        // Spawn the shell for the initial pane (and size it to the viewport).
        self.sync_layout();
        if self.active_tab().panes.is_empty() {
            tracing::error!("failed to spawn the initial shell");
            event_loop.exit();
            return;
        }
        // Seed the launch repo's timeline with its "session started" anchor (so it exists even if
        // the user commits before opening the timeline) and project its branch/dirty to the status
        // line. `rescope_active` later re-points active_root to the active tab's repo.
        self.active_root = Repo::discover(&std::env::current_dir().unwrap_or_default())
            .ok()
            .flatten()
            .map(|r| r.root().to_path_buf());
        self.ensure_active_seeded();
        self.sync_active_status();
        // Start recording working-tree edits into every tab's repo timeline (design §10.5).
        self.start_git_poll();
        // Publish the initial panes' pids and start the off-thread cwd poll (status line + titles).
        self.refresh_poll_pids();
        self.start_cwd_poll();
        tracing::info!(
            panes = self.active_tab().panes.len(),
            "window, GPU, and shell ready"
        );
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: Wakeup) {
        match event {
            // New shell output arrived; ask the window to repaint.
            Wakeup::Shell => self.request_redraw(),
            // A fresh working-tree status; record any new edits (repaints only if it changed).
            Wakeup::GitPoll => self.drain_git_poll(),
            // Fresh per-pane cwds from the poll thread; apply them (status line, titles, git dock).
            Wakeup::CwdPoll => self.drain_cwd_poll(),
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Persist the current layout so the next launch restores it (design/README.md persist
        // scope: layout only). Best-effort - a write failure just means a fresh next launch.
        if self.config.session.persist {
            if let Err(err) = self.session_snapshot().save_default() {
                tracing::warn!(%err, "failed to save session state");
            }
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
            WindowEvent::CursorLeft { .. } => {
                // The pointer left the window - collapse a hover-expanded rail (design §08).
                if self.rail_expanded {
                    self.rail_expanded = false;
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => self.on_left_click(state),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => self.on_right_click(),
            WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),
            WindowEvent::DroppedFile(path) => self.on_file_dropped(&path),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // The window moved to a display with a different backing scale factor (e.g. a
                // retina laptop to an external 1x monitor). Re-scale the whole UI so it keeps its
                // intended size at the new pixel density - without this, the surface is physical px
                // while layout/fonts stay at the old scale, so the UI renders tiny on retina. A
                // `Resized` with the new physical size follows and reconfigures the surface.
                self.scale = scale_factor;
                self.measure.set_scale(scale32(self.scale));
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_scale(
                        self.scale,
                        self.config.appearance.font_size,
                        self.config.appearance.line_height,
                    );
                }
                self.sync_layout();
            }
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
        let now = Instant::now();
        // Dismiss an expired toast (design §12) and repaint without it.
        if self.toast.is_some() && now >= self.toast_expires {
            self.toast = None;
            self.request_redraw();
        }
        // Reveal a hover tooltip once its delay elapses (design §09) - a one-shot repaint.
        if let Some((_, since)) = &self.hover_tip {
            if !self.tooltip_visible && now >= *since + HOVER_DELAY {
                self.tooltip_visible = true;
                self.request_redraw();
            }
        }
        // Toggle a blinking caret (design §06) - edge-triggered so it repaints once per flip, only
        // while the focused program requested a blink.
        let blinking = self.focused_cursor_blinking();
        if blinking {
            let off = self.cursor_blink_off(now);
            if off != self.blink_phase {
                self.blink_phase = off;
                self.request_redraw();
            }
        }
        // While any animation is live, poll and repaint each frame so it advances; otherwise idle
        // in `Wait` until the next real event (shell output, input, resize) - or sleep only until
        // the earliest pending deadline (a toast's expiry, or a tooltip's reveal) so the loop
        // wakes exactly then.
        if self.animating(now) {
            event_loop.set_control_flow(ControlFlow::Poll);
            self.request_redraw();
            return;
        }
        let mut deadline: Option<Instant> = self.toast.is_some().then_some(self.toast_expires);
        if let Some((_, since)) = &self.hover_tip {
            if !self.tooltip_visible {
                let reveal = *since + HOVER_DELAY;
                deadline = Some(deadline.map_or(reveal, |d| d.min(reveal)));
            }
        }
        // Wake at the next caret-blink toggle so the blink keeps ticking while idle.
        if blinking {
            let elapsed = now.saturating_duration_since(self.blink_epoch);
            let ticks = elapsed.as_millis() / CURSOR_BLINK_INTERVAL.as_millis();
            let next = self.blink_epoch
                + CURSOR_BLINK_INTERVAL * u32::try_from(ticks + 1).unwrap_or(u32::MAX);
            deadline = Some(deadline.map_or(next, |d| d.min(next)));
        }
        match deadline {
            Some(d) => event_loop.set_control_flow(ControlFlow::WaitUntil(d)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
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
        // Surface the open tabs (by their real titles) + the working-directory files (§10.8).
        let tabs = self.tab_titles();
        let files = gather_files();
        self.palette.open(tabs, files);
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
        // The active tab's empty-state fade-out (going live): keep ticking until it settles,
        // then drop it so the mark + chips stop being drawn.
        let fade_done = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.empty_fade)
            .is_some_and(|fade| fade.done(now));
        if fade_done {
            self.tabs[self.active].empty_fade = None;
            self.request_redraw();
        }
        let fading = self
            .tabs
            .get(self.active)
            .is_some_and(|tab| tab.empty_fade.is_some());
        self.palette_anim.is_some() || self.confirm_anim.is_some() || fading
    }
}

/// Decode a physical key + modifiers into a pane action. Pane control uses `Alt`
/// (`⌥`) as its leader-less modifier, matching the design guide's shown chords
/// (`⌥|` split right, `⌥-` split down, `⌥Z` zoom, `⌥1..⌥8` focus by number), plus
/// `⌥h/j/k/l` directional focus, `⌥⇧h/j/k/l` resize, `⌥w` close, and `⌥=` even out.
/// Returns `None` for anything else (which then reaches the shell).
/// Parse a `[panes] leader` spec like `ctrl+a` into `(modifiers, key)`, or `None` when it names
/// no key. Modifiers: `ctrl`/`control`, `alt`/`opt`/`option`, `shift`, `cmd`/`super`/`meta`.
fn parse_leader(spec: &str) -> Option<(ModifiersState, KeyCode)> {
    let mut mods = ModifiersState::empty();
    let mut key = None;
    for part in spec.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "" => {}
            "ctrl" | "control" => mods |= ModifiersState::CONTROL,
            "alt" | "opt" | "option" => mods |= ModifiersState::ALT,
            "shift" => mods |= ModifiersState::SHIFT,
            "cmd" | "super" | "meta" | "win" => mods |= ModifiersState::SUPER,
            other => key = letter_key_code(other),
        }
    }
    key.map(|k| (mods, k))
}

/// The `KeyCode` for a single lowercase ASCII letter (`a`..=`z`), else `None`.
fn letter_key_code(s: &str) -> Option<KeyCode> {
    if s.len() != 1 {
        return None;
    }
    Some(match s.as_bytes()[0] {
        b'a' => KeyCode::KeyA,
        b'b' => KeyCode::KeyB,
        b'c' => KeyCode::KeyC,
        b'd' => KeyCode::KeyD,
        b'e' => KeyCode::KeyE,
        b'f' => KeyCode::KeyF,
        b'g' => KeyCode::KeyG,
        b'h' => KeyCode::KeyH,
        b'i' => KeyCode::KeyI,
        b'j' => KeyCode::KeyJ,
        b'k' => KeyCode::KeyK,
        b'l' => KeyCode::KeyL,
        b'm' => KeyCode::KeyM,
        b'n' => KeyCode::KeyN,
        b'o' => KeyCode::KeyO,
        b'p' => KeyCode::KeyP,
        b'q' => KeyCode::KeyQ,
        b'r' => KeyCode::KeyR,
        b's' => KeyCode::KeyS,
        b't' => KeyCode::KeyT,
        b'u' => KeyCode::KeyU,
        b'v' => KeyCode::KeyV,
        b'w' => KeyCode::KeyW,
        b'x' => KeyCode::KeyX,
        b'y' => KeyCode::KeyY,
        b'z' => KeyCode::KeyZ,
        _ => return None,
    })
}

/// The pane action for a leader chord (after the leader prefix, §11): `hjkl` focus, `⇧hjkl`
/// resize, `z` zoom, `x` close, `|` (Backslash) / `-` split, else `None`.
fn leader_chord(code: KeyCode, mods: ModifiersState) -> Option<PaneAction> {
    let shift = mods.shift_key();
    Some(match code {
        KeyCode::KeyH if shift => PaneAction::Resize(Dir::Left),
        KeyCode::KeyJ if shift => PaneAction::Resize(Dir::Down),
        KeyCode::KeyK if shift => PaneAction::Resize(Dir::Up),
        KeyCode::KeyL if shift => PaneAction::Resize(Dir::Right),
        KeyCode::KeyH => PaneAction::Focus(Dir::Left),
        KeyCode::KeyJ => PaneAction::Focus(Dir::Down),
        KeyCode::KeyK => PaneAction::Focus(Dir::Up),
        KeyCode::KeyL => PaneAction::Focus(Dir::Right),
        KeyCode::KeyZ => PaneAction::Zoom,
        KeyCode::KeyX => PaneAction::Close,
        KeyCode::Backslash => PaneAction::Split(Dir::Right),
        KeyCode::Minus => PaneAction::Split(Dir::Down),
        _ => return None,
    })
}

fn pane_action(code: KeyCode, mods: ModifiersState) -> Option<PaneAction> {
    // `⌥` chords only; `⌥⌘arrows` are the global session (timeline) shortcuts, handled first.
    // Plain ⌥←/→ are intentionally left to the terminal for word-wise cursor movement.
    if !mods.alt_key() || mods.super_key() {
        return None;
    }
    let shift = mods.shift_key();
    let ctrl = mods.control_key();
    Some(match code {
        KeyCode::Backslash => PaneAction::Split(Dir::Right),
        KeyCode::Minus => PaneAction::Split(Dir::Down),
        KeyCode::Equal => PaneAction::EvenOut,
        KeyCode::KeyZ => PaneAction::Zoom,
        KeyCode::KeyW => PaneAction::Close,
        KeyCode::Space => PaneAction::CycleLayout,
        // Modified arrows retain pane resize/swap. Plain horizontal Option-arrows belong to the
        // terminal's standard word-wise cursor shortcuts; HJKL (or the leader) moves pane focus.
        KeyCode::ArrowLeft if ctrl => PaneAction::Resize(Dir::Left),
        KeyCode::ArrowDown if ctrl => PaneAction::Resize(Dir::Down),
        KeyCode::ArrowUp if ctrl => PaneAction::Resize(Dir::Up),
        KeyCode::ArrowRight if ctrl => PaneAction::Resize(Dir::Right),
        KeyCode::ArrowLeft if shift => PaneAction::Swap(Dir::Left),
        KeyCode::ArrowDown if shift => PaneAction::Swap(Dir::Down),
        KeyCode::ArrowUp if shift => PaneAction::Swap(Dir::Up),
        KeyCode::ArrowRight if shift => PaneAction::Swap(Dir::Right),
        // `⇧hjkl` resize (the vim aliases, also the leader chords); plain `hjkl` moves focus.
        // Vertical Option-arrows remain focus aliases since word-wise movement is horizontal.
        KeyCode::KeyH if shift => PaneAction::Resize(Dir::Left),
        KeyCode::KeyJ if shift => PaneAction::Resize(Dir::Down),
        KeyCode::KeyK if shift => PaneAction::Resize(Dir::Up),
        KeyCode::KeyL if shift => PaneAction::Resize(Dir::Right),
        KeyCode::KeyH => PaneAction::Focus(Dir::Left),
        KeyCode::ArrowDown | KeyCode::KeyJ => PaneAction::Focus(Dir::Down),
        KeyCode::ArrowUp | KeyCode::KeyK => PaneAction::Focus(Dir::Up),
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
    // Plain `⌘` chords (no shift) - so the `⇧⌘` chords (`⇧⌘N` new group, `⇧⌘T` reopen, `⇧⌘P`
    // pin) fall through to `on_super_chord`.
    if mods.super_key() && !mods.alt_key() && !mods.shift_key() {
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
fn pane_dims(rect: Rect, cell_w: f32, cell_h: f32, inset: f32, reserved_bottom: f32) -> (u16, u16) {
    let cols = ((rect.w - 2.0 * inset) / cell_w).floor().clamp(1.0, 1000.0) as u16;
    let rows = ((rect.h - 2.0 * inset - reserved_bottom) / cell_h)
        .floor()
        .clamp(1.0, 1000.0) as u16;
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
/// terminal representation. Cursor/edit keys carry an xterm modifier parameter so a program
/// (vim, readline) can distinguish e.g. Shift+Arrow from a bare arrow. Unmodified cursor keys
/// honor the program's application-cursor mode, which shell line editors use for history.
fn key_to_bytes(
    event: &KeyEvent,
    modifiers: ModifiersState,
    application_cursor: bool,
    keyboard_mode: KeyboardMode,
) -> Option<Vec<u8>> {
    // When a program has negotiated the Kitty keyboard protocol (Neovim and friends probe for
    // it at startup; see the PTY-reply wiring in `skelly-term`), it expects the Kitty CSI-u
    // encoding so it can tell modified keys apart - `Shift+Enter` from `Enter`, `Ctrl+I` from
    // `Tab`, `Alt+F` from an `ESC f` word-jump. Legacy xterm encoding cannot express those, so
    // the shift key on `Shift+Enter` would otherwise be silently dropped.
    if keyboard_mode.is_active() {
        if let Some(bytes) = kitty_key_to_bytes(event, modifiers, keyboard_mode) {
            return Some(bytes);
        }
    }
    if let Key::Named(key) = event.logical_key {
        if let Some(bytes) = cursor_navigation_shortcut(key, modifiers) {
            return Some(bytes);
        }
    }
    let m = xterm_modifier_code(modifiers);
    let cursor = |final_byte: u8| Some(cursor_key(final_byte, m, application_cursor));
    let csi_tilde = |num: u8| Some(csi_edit(num, m));
    match &event.logical_key {
        Key::Named(NamedKey::Enter) => Some(enter_bytes(modifiers)),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
        Key::Named(NamedKey::Tab) => Some(vec![b'\t']),
        Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
        Key::Named(NamedKey::ArrowUp) => cursor(b'A'),
        Key::Named(NamedKey::ArrowDown) => cursor(b'B'),
        Key::Named(NamedKey::ArrowRight) => cursor(b'C'),
        Key::Named(NamedKey::ArrowLeft) => cursor(b'D'),
        Key::Named(NamedKey::Home) => cursor(b'H'),
        Key::Named(NamedKey::End) => cursor(b'F'),
        Key::Named(NamedKey::PageUp) => csi_tilde(5),
        Key::Named(NamedKey::PageDown) => csi_tilde(6),
        Key::Named(NamedKey::Insert) => csi_tilde(2),
        Key::Named(NamedKey::Delete) => csi_tilde(3),
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

/// Encode a key press per the Kitty keyboard protocol, for when the focused pane's program has
/// enabled it. Returns `None` for anything this level of the protocol leaves as legacy - an
/// unmodified special key, or a plain/shift-only text key - which then falls back to the legacy
/// encoding below (so ordinary typing, `Enter`, and application-cursor history are untouched).
///
/// A key keeps its legacy final byte where it has one (arrows end in `A`..=`D`, `Home`/`End` in
/// `H`/`F`, edit keys in `~`, `F1`..=`F4` in `P`..=`S`) and uses `u` otherwise. The modifier
/// parameter is `1 + shift + 2*alt + 4*ctrl + 8*super`; it is emitted whenever a key carries
/// modifiers, which is exactly what makes `Shift+Enter` (`CSI 13;2u`) distinguishable from
/// `Enter`. `Esc` is always disambiguated so it is never mistaken for an escape-sequence prefix.
fn kitty_key_to_bytes(
    event: &KeyEvent,
    modifiers: ModifiersState,
    mode: KeyboardMode,
) -> Option<Vec<u8>> {
    let m = kitty_modifier_code(modifiers);
    let modified = m > 1;
    let all = mode.report_all_as_esc;
    // A special key is Kitty-encoded once it carries a modifier (or when every key is being
    // reported as an escape sequence); unmodified it falls through to the legacy path.
    let named = |num: u32, final_byte: u8| (modified || all).then(|| kitty_csi(num, m, final_byte));
    match &event.logical_key {
        Key::Named(NamedKey::Escape) => Some(kitty_csi(27, m, b'u')),
        Key::Named(NamedKey::Enter) => named(13, b'u'),
        Key::Named(NamedKey::Tab) => named(9, b'u'),
        Key::Named(NamedKey::Backspace) => named(127, b'u'),
        Key::Named(NamedKey::ArrowUp) => named(1, b'A'),
        Key::Named(NamedKey::ArrowDown) => named(1, b'B'),
        Key::Named(NamedKey::ArrowRight) => named(1, b'C'),
        Key::Named(NamedKey::ArrowLeft) => named(1, b'D'),
        Key::Named(NamedKey::Home) => named(1, b'H'),
        Key::Named(NamedKey::End) => named(1, b'F'),
        Key::Named(NamedKey::PageUp) => named(5, b'~'),
        Key::Named(NamedKey::PageDown) => named(6, b'~'),
        Key::Named(NamedKey::Insert) => named(2, b'~'),
        Key::Named(NamedKey::Delete) => named(3, b'~'),
        Key::Named(NamedKey::F1) => named(1, b'P'),
        Key::Named(NamedKey::F2) => named(1, b'Q'),
        Key::Named(NamedKey::F3) => named(1, b'R'),
        Key::Named(NamedKey::F4) => named(1, b'S'),
        Key::Named(NamedKey::F5) => named(15, b'~'),
        Key::Named(NamedKey::F6) => named(17, b'~'),
        Key::Named(NamedKey::F7) => named(18, b'~'),
        Key::Named(NamedKey::F8) => named(19, b'~'),
        Key::Named(NamedKey::F9) => named(20, b'~'),
        Key::Named(NamedKey::F10) => named(21, b'~'),
        Key::Named(NamedKey::F11) => named(23, b'~'),
        Key::Named(NamedKey::F12) => named(24, b'~'),
        // Text keys: Ctrl/Alt/Super (or report-all) force the escape-sequence form, keyed by the
        // base-layout codepoint - so `Ctrl+A` is `CSI 97;5u`, not the `0x01` control byte. Plain
        // and shift-only presses return `None` to send their produced text via the legacy path.
        Key::Character(text) => {
            let force =
                modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() || all;
            if !force {
                return None;
            }
            let base = text.chars().next()?.to_ascii_lowercase();
            Some(kitty_csi(u32::from(base), m, b'u'))
        }
        _ => None,
    }
}

/// The Kitty modifier parameter: `1 + shift + 2*alt + 4*ctrl + 8*super`. Unlike the legacy
/// xterm code, `super`/`⌘` is encoded - a Kitty-aware program handles it itself.
fn kitty_modifier_code(modifiers: ModifiersState) -> u32 {
    1 + u32::from(modifiers.shift_key())
        + (u32::from(modifiers.alt_key()) << 1)
        + (u32::from(modifiers.control_key()) << 2)
        + (u32::from(modifiers.super_key()) << 3)
}

/// Build a Kitty key sequence `CSI <number> ; <m> <final>`. Arrow / `Home` / `End` / `F1`-`F4`
/// keys carry the default number `1`, which is dropped when there is no modifier (`CSI A`); edit
/// (`~`) and text (`u`) keys always keep their number since it identifies the key. The modifier
/// parameter is dropped when it is the default (`1`, no modifiers).
fn kitty_csi(number: u32, m: u32, final_byte: u8) -> Vec<u8> {
    let letter_key = final_byte != b'u' && final_byte != b'~';
    if m == 1 {
        if letter_key && number == 1 {
            format!("\x1b[{}", char::from(final_byte)).into_bytes()
        } else {
            format!("\x1b[{number}{}", char::from(final_byte)).into_bytes()
        }
    } else {
        format!("\x1b[{number};{m}{}", char::from(final_byte)).into_bytes()
    }
}

/// Platform-standard command-line editing shortcuts. Shell line editors universally bind
/// Meta-B/F to previous/next word, Ctrl-A/E to start/end of line, Meta-Backspace to delete the
/// previous word, and Ctrl-U to delete to the line start - so translating the macOS
/// Option/Command gestures here works without shell-specific integration.
fn cursor_navigation_shortcut(key: NamedKey, modifiers: ModifiersState) -> Option<Vec<u8>> {
    match (key, modifiers) {
        (NamedKey::ArrowLeft, ModifiersState::ALT) => Some(b"\x1bb".to_vec()),
        (NamedKey::ArrowRight, ModifiersState::ALT) => Some(b"\x1bf".to_vec()),
        (NamedKey::ArrowLeft, ModifiersState::SUPER) => Some(vec![0x01]),
        (NamedKey::ArrowRight, ModifiersState::SUPER) => Some(vec![0x05]),
        // Option+Backspace deletes the previous word (readline `backward-kill-word`, bound to
        // Meta-Backspace = ESC + DEL); Command+Backspace deletes to the line start (Ctrl-U).
        (NamedKey::Backspace, ModifiersState::ALT) => Some(b"\x1b\x7f".to_vec()),
        (NamedKey::Backspace, ModifiersState::SUPER) => Some(vec![0x15]),
        _ => None,
    }
}

/// The bytes `Enter` sends under the legacy (non-Kitty) encoding.
///
/// Legacy xterm has no parameterized form for `Enter`, so a bare `CR` is all an unmodified press
/// can be - but sending that for `Shift+Enter` too drops the modifier and submits the line in the
/// very TUIs whose "insert a newline" gesture this is. Most of those never negotiate the Kitty
/// protocol (so [`kitty_key_to_bytes`] never sees the key) and instead read the meta-prefixed
/// carriage return `ESC CR` - the sequence they ask users to bind `Shift+Enter` to in terminals
/// without Kitty support. It is inert in shell line editors (zsh / readline leave the buffer
/// untouched and execute nothing), so an unmodified `Enter` still submits everywhere.
fn enter_bytes(modifiers: ModifiersState) -> Vec<u8> {
    if modifiers.shift_key() || modifiers.alt_key() {
        vec![0x1b, b'\r']
    } else {
        vec![b'\r']
    }
}

/// The xterm modifier parameter: `1 + Shift + (Alt<<1) + (Ctrl<<2)`. `Super`/`⌘` is an
/// application modifier (the menu + Skelly's own chords), never encoded into a terminal sequence.
fn xterm_modifier_code(modifiers: ModifiersState) -> u8 {
    1 + u8::from(modifiers.shift_key())
        + (u8::from(modifiers.alt_key()) << 1)
        + (u8::from(modifiers.control_key()) << 2)
}

/// Encode an arrow/Home/End key. In application-cursor mode an unmodified key uses SS3
/// (`ESC O <final>`), matching xterm/terminfo; modified keys always use xterm's parameterized CSI
/// form so applications can distinguish their modifiers.
fn cursor_key(final_byte: u8, m: u8, application_cursor: bool) -> Vec<u8> {
    if m > 1 {
        format!("\x1b[1;{m}{}", char::from(final_byte)).into_bytes()
    } else if application_cursor {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

/// An edit key CSI sequence (`ESC[<num>~`, e.g. `PageUp` = 5). Unmodified uses the short form; a
/// modifier `m > 1` inserts the `<num>;<m>` parameters.
fn csi_edit(num: u8, m: u8) -> Vec<u8> {
    if m == 1 {
        format!("\x1b[{num}~").into_bytes()
    } else {
        format!("\x1b[{num};{m}~").into_bytes()
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
        csi_edit, cursor_key, cursor_navigation_shortcut, cycle_index, dim, editor_filetype,
        editor_mode, enter_bytes, index_after_close, kitty_csi, kitty_modifier_code, leader_chord,
        order, overlay_panel_top, overlay_rise_offset, pane_action, pane_dims, panic_message,
        parse_leader, pointer_cell_in, process_name, resolve_cell, selection_cells, selection_text,
        shell_escape_path, tab_action, xterm_modifier_code, PaneAction, Selection, TabAction,
    };
    use skelly_pane::{Dir, Rect};
    use skelly_render::{AnsiPalette, Srgb};
    use skelly_term::{CellAttrs, CellColor, CursorShape, TermCell};
    use winit::keyboard::{KeyCode, ModifiersState, NamedKey};

    #[test]
    fn dropped_path_escapes_shell_metacharacters_but_keeps_utf8() {
        assert_eq!(
            shell_escape_path(std::path::Path::new(
                "/Users/me/Desktop/Screenshot 2026.png"
            )),
            "/Users/me/Desktop/Screenshot\\ 2026.png"
        );
        // Shell-special punctuation is escaped; safe path punctuation and UTF-8 pass through.
        assert_eq!(
            shell_escape_path(std::path::Path::new("/tmp/a(b)&c'd.png")),
            "/tmp/a\\(b\\)\\&c\\'d.png"
        );
        assert_eq!(
            shell_escape_path(std::path::Path::new("/tmp/café/naïve.png")),
            "/tmp/café/naïve.png"
        );
    }

    #[test]
    fn default_cursor_navigation_shortcuts_use_shell_line_editor_bindings() {
        assert_eq!(
            cursor_navigation_shortcut(NamedKey::ArrowLeft, ModifiersState::ALT),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            cursor_navigation_shortcut(NamedKey::ArrowRight, ModifiersState::ALT),
            Some(b"\x1bf".to_vec())
        );
        assert_eq!(
            cursor_navigation_shortcut(NamedKey::ArrowLeft, ModifiersState::SUPER),
            Some(vec![0x01])
        );
        assert_eq!(
            cursor_navigation_shortcut(NamedKey::ArrowRight, ModifiersState::SUPER),
            Some(vec![0x05])
        );
        // Option+Backspace kills the previous word (ESC + DEL); Command+Backspace kills to the
        // line start (Ctrl-U).
        assert_eq!(
            cursor_navigation_shortcut(NamedKey::Backspace, ModifiersState::ALT),
            Some(b"\x1b\x7f".to_vec())
        );
        assert_eq!(
            cursor_navigation_shortcut(NamedKey::Backspace, ModifiersState::SUPER),
            Some(vec![0x15])
        );
        assert_eq!(
            cursor_navigation_shortcut(
                NamedKey::ArrowLeft,
                ModifiersState::ALT | ModifiersState::SHIFT
            ),
            None,
            "modified variants remain available to applications and pane bindings"
        );
    }

    #[test]
    fn modified_keys_encode_xterm_modifier_sequences() {
        // Bare arrow: the short form; Super is ignored (an app modifier).
        assert_eq!(xterm_modifier_code(ModifiersState::empty()), 1);
        assert_eq!(xterm_modifier_code(ModifiersState::SUPER), 1);
        assert_eq!(cursor_key(b'A', 1, false), b"\x1b[A");
        // Application cursor mode uses the terminfo-compatible SS3 sequence. This is what shell
        // line editors expect when cycling through command history with bare Up/Down.
        assert_eq!(cursor_key(b'A', 1, true), b"\x1bOA");
        assert_eq!(cursor_key(b'B', 1, true), b"\x1bOB");
        // Shift+Up -> ESC[1;2A even in application mode, preserving the modifier.
        assert_eq!(xterm_modifier_code(ModifiersState::SHIFT), 2);
        assert_eq!(cursor_key(b'A', 2, true), b"\x1b[1;2A");
        // Ctrl (code 5) and Ctrl+Shift (6) on a cursor key.
        assert_eq!(xterm_modifier_code(ModifiersState::CONTROL), 5);
        assert_eq!(cursor_key(b'D', 5, false), b"\x1b[1;5D");
        assert_eq!(
            xterm_modifier_code(ModifiersState::CONTROL | ModifiersState::SHIFT),
            6
        );
        // Edit keys (PageDown = 6) use the `~` form, modified inserts `;<mod>`.
        assert_eq!(csi_edit(6, 1), b"\x1b[6~");
        assert_eq!(csi_edit(6, 2), b"\x1b[6;2~");
    }

    #[test]
    fn shift_enter_stays_distinguishable_without_the_kitty_protocol() {
        // The reported bug: `Shift+Enter` was encoded as a bare `CR`, identical to `Enter`, so a
        // TUI that uses it to insert a newline (Claude Code and friends) submitted the prompt
        // instead. Most TUIs never negotiate the Kitty protocol, so the legacy path has to carry
        // the modifier itself - as the meta `ESC` prefix those programs are configured to expect.
        assert_eq!(enter_bytes(ModifiersState::empty()), b"\r");
        assert_eq!(enter_bytes(ModifiersState::SHIFT), b"\x1b\r");
        // `Alt`/`⌥` is the modifier the `ESC` prefix classically encodes, so it sends the same.
        assert_eq!(enter_bytes(ModifiersState::ALT), b"\x1b\r");
        assert_eq!(
            enter_bytes(ModifiersState::ALT | ModifiersState::SHIFT),
            b"\x1b\r"
        );
        // Ctrl+Enter has no legacy encoding, and `Super`/`⌘` is an application modifier that is
        // never encoded into a terminal sequence: both still submit.
        assert_eq!(enter_bytes(ModifiersState::CONTROL), b"\r");
        assert_eq!(enter_bytes(ModifiersState::SUPER), b"\r");
    }

    #[test]
    fn kitty_modifier_code_encodes_all_four_modifiers() {
        // `1 + shift + 2*alt + 4*ctrl + 8*super` - and, unlike xterm, Super is encoded.
        assert_eq!(kitty_modifier_code(ModifiersState::empty()), 1);
        assert_eq!(kitty_modifier_code(ModifiersState::SHIFT), 2);
        assert_eq!(kitty_modifier_code(ModifiersState::ALT), 3);
        assert_eq!(kitty_modifier_code(ModifiersState::CONTROL), 5);
        assert_eq!(kitty_modifier_code(ModifiersState::SUPER), 9);
        assert_eq!(
            kitty_modifier_code(ModifiersState::CONTROL | ModifiersState::SHIFT),
            6
        );
    }

    #[test]
    fn kitty_csi_keeps_final_bytes_and_drops_default_params() {
        // The user's bug: Shift+Enter must be distinguishable. Enter is key code 13, Shift is
        // modifier 2, so it encodes as `CSI 13 ; 2 u` - which programs map to `<S-CR>`.
        assert_eq!(kitty_csi(13, 2, b'u'), b"\x1b[13;2u");
        // Ctrl+A (code 97, modifier 5) is a CSI-u sequence, not the 0x01 control byte.
        assert_eq!(kitty_csi(97, 5, b'u'), b"\x1b[97;5u");
        // Esc is always disambiguated, even unmodified: `CSI 27 u`.
        assert_eq!(kitty_csi(27, 1, b'u'), b"\x1b[27u");
        // Arrow / Home / F1-F4 keys carry the default number 1, dropped when unmodified.
        assert_eq!(kitty_csi(1, 1, b'A'), b"\x1b[A");
        assert_eq!(kitty_csi(1, 2, b'A'), b"\x1b[1;2A");
        assert_eq!(kitty_csi(1, 5, b'P'), b"\x1b[1;5P");
        // Edit keys keep their number even unmodified, since it identifies the key.
        assert_eq!(kitty_csi(5, 1, b'~'), b"\x1b[5~");
        assert_eq!(kitty_csi(3, 3, b'~'), b"\x1b[3;3~");
    }

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
    fn editor_filetype_reads_the_open_file_from_an_editor_title() {
        // An editor's window title names the open file; its extension gives the filetype.
        assert_eq!(
            editor_filetype(Some("nvim"), Some("main.rs (~/proj/src) - NVIM")),
            Some("rust")
        );
        assert_eq!(
            editor_filetype(Some("vim"), Some("~/app/server.py + (app) - VIM")),
            Some("python")
        );
        // Not an editor -> no filetype even if the title looks like a file.
        assert_eq!(editor_filetype(Some("zsh"), Some("build.rs")), None);
        // Editor but no recognizable file in the title -> nothing (never a bogus filetype).
        assert_eq!(
            editor_filetype(Some("nvim"), Some("v1.2.3 - release notes")),
            None
        );
        assert_eq!(editor_filetype(Some("nvim"), None), None);
    }

    #[test]
    fn editor_mode_maps_cursor_shape_only_for_modal_editors() {
        // A modal editor's cursor shape becomes its mode (a real DECSCUSR signal, not a guess).
        assert_eq!(
            editor_mode(Some("nvim"), CursorShape::Block),
            Some("NORMAL")
        );
        assert_eq!(editor_mode(Some("nvim"), CursorShape::Bar), Some("INSERT"));
        assert_eq!(
            editor_mode(Some("vim"), CursorShape::Underline),
            Some("REPLACE")
        );
        // A non-editor foreground process (or none) shows no mode - a shell prompt's block
        // cursor must not read as "NORMAL".
        assert_eq!(editor_mode(Some("zsh"), CursorShape::Block), None);
        assert_eq!(editor_mode(Some("cargo"), CursorShape::Bar), None);
        assert_eq!(editor_mode(None, CursorShape::Bar), None);
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
        let resolved = resolve_cell(&cell, &palette, false);
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
        let resolved = resolve_cell(&cell, &palette, false);
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
        let resolved = resolve_cell(&cell, &palette, false);
        assert!(resolved.bold && resolved.italic && resolved.underline);
    }

    // ----- pane wiring geometry + keybindings ---------------------------------

    #[test]
    fn pane_dims_fit_cells_inside_the_inset() {
        // 800 wide, 12px inset each side, 10px cells -> floor(776 / 10) = 77 cols.
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let (cols, rows) = pane_dims(rect, 10.0, 20.0, 12.0, 0.0);
        assert_eq!(cols, 77);
        assert_eq!(rows, 28); // floor((600 - 24) / 20)
    }

    #[test]
    fn pane_dims_reserve_room_for_the_status_line() {
        // Same rect, but reserving 40px at the bottom drops two rows: floor((600-24-40)/20) = 26.
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let (cols, rows) = pane_dims(rect, 10.0, 20.0, 12.0, 40.0);
        assert_eq!(cols, 77); // columns are unaffected
        assert_eq!(rows, 26);
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
    fn leader_parses_and_decodes_pane_chords() {
        // The default `ctrl+a` leader parses to Ctrl + KeyA.
        assert_eq!(
            parse_leader("ctrl+a"),
            Some((ModifiersState::CONTROL, KeyCode::KeyA))
        );
        assert_eq!(
            parse_leader("Ctrl + B"),
            Some((ModifiersState::CONTROL, KeyCode::KeyB))
        );
        assert_eq!(parse_leader("ctrl+"), None, "no key named");
        // Leader chords: hjkl focus, ⇧hjkl resize, z/x/|/- (Backslash/Minus).
        let none = ModifiersState::empty();
        let shift = ModifiersState::SHIFT;
        assert_eq!(
            leader_chord(KeyCode::KeyL, none),
            Some(PaneAction::Focus(Dir::Right))
        );
        assert_eq!(
            leader_chord(KeyCode::KeyK, shift),
            Some(PaneAction::Resize(Dir::Up))
        );
        assert_eq!(leader_chord(KeyCode::KeyX, none), Some(PaneAction::Close));
        assert_eq!(
            leader_chord(KeyCode::Backslash, none),
            Some(PaneAction::Split(Dir::Right))
        );
        assert_eq!(leader_chord(KeyCode::KeyQ, none), None);
    }

    #[test]
    fn option_cursor_shortcuts_do_not_conflict_with_pane_nav() {
        // Plain horizontal Option-arrows belong to terminal word navigation. Modified arrows still
        // resize/swap panes, and vertical arrows remain convenient focus aliases.
        let alt = ModifiersState::ALT;
        let ctrl_alt = ModifiersState::ALT | ModifiersState::CONTROL;
        let alt_shift = ModifiersState::ALT | ModifiersState::SHIFT;
        assert_eq!(pane_action(KeyCode::ArrowLeft, alt), None);
        assert_eq!(pane_action(KeyCode::ArrowRight, alt), None);
        assert_eq!(
            pane_action(KeyCode::ArrowDown, alt),
            Some(PaneAction::Focus(Dir::Down))
        );
        assert_eq!(
            pane_action(KeyCode::ArrowRight, ctrl_alt),
            Some(PaneAction::Resize(Dir::Right))
        );
        assert_eq!(
            pane_action(KeyCode::ArrowUp, alt_shift),
            Some(PaneAction::Swap(Dir::Up))
        );
        // Arrows without Alt are the shell's own (escape sequences), not a pane action.
        assert_eq!(
            pane_action(KeyCode::ArrowLeft, ModifiersState::empty()),
            None
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
