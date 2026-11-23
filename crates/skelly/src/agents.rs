//! Settings > Agents: whether each coding agent Skelly knows - Pi, Claude Code and Codex - can
//! use Skelly's review tools over MCP (`skelly mcp`), with one key to connect it or repair it.
//! Skelly checks at launch, when the page opens and after each change, and the sidebar's
//! settings gear carries a dot while an installed agent needs setting up. The page is a view of
//! each agent's own MCP config, read and written only through that agent's CLI ([`probe`]),
//! never `config.toml`: like the Workspaces page, it sits outside Hard rule 1's one-key
//! contract. Pure state + layout like [`crate::workspace`]; the binary owns the
//! [`probe::Prober`], routes keys and clicks, and copies a command to the clipboard.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};

use crate::gitdock::wrap;
use crate::settings::{
    card_fill, hover_fill, push_row, CARD_RADIUS, PILL_INSET, PILL_RADIUS, ROW_PAD_X,
};
use crate::workspace::{push_button, push_ringed, Area};

pub(crate) mod probe;
#[cfg(test)]
mod tests;
pub(crate) mod wiring;

/// Page layout constants in **logical** px (multiplied by the DPI scale). The list's label
/// line, above its card.
const LIST_LABEL_H: f32 = 24.0;
/// An agent's row, as tall as a settings control row.
const ROW_H: f32 = 56.0;
/// The status dot at a row's left, and the gap from it to the agent's name.
const DOT: f32 = 8.0;
const NAME_GAP: f32 = 12.0;
/// The gap between a name and its status line.
const LINE_GAP: f32 = 1.0;
/// The least gap between a status line and the row's button.
const BUTTON_GAP: f32 = 16.0;
/// The focused agent's command under the list: the gap above it, its caption line, and the
/// well that holds it.
const DETAIL_GAP: f32 = 18.0;
const DETAIL_LABEL_H: f32 = 28.0;
const WELL_GAP: f32 = 4.0;
const WELL_PAD_X: f32 = 12.0;
const WELL_PAD_Y: f32 = 9.0;
const WELL_RADIUS: f32 = 6.0;
/// The gap above the notes under the well, and between two notes.
const NOTE_GAP: f32 = 10.0;
const NOTE_SPACING: f32 = 4.0;

/// A coding agent that can start Skelly's MCP server.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Agent {
    Pi,
    Claude,
    Codex,
}

impl Agent {
    /// Every agent, in the page's order.
    pub(crate) const ALL: [Agent; 3] = [Agent::Pi, Agent::Claude, Agent::Codex];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Agent::Pi => "Pi",
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    /// Its CLI, which [`probe`] finds on the login `PATH`.
    pub(crate) fn program(self) -> &'static str {
        match self {
            Agent::Pi => "pi",
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// Its place in [`Agent::ALL`].
    fn index(self) -> usize {
        self as usize
    }

    /// What connecting it sets up, under its command.
    fn note(self) -> &'static str {
        match self {
            Agent::Pi => {
                "Adds Skelly to Pi's global MCP servers with its tools exposed directly, so a \
                 model sees them without searching first."
            }
            Agent::Claude => "Adds Skelly to your Claude Code user config, for every project.",
            Agent::Codex => {
                "Adds Skelly to Codex's config, which the ChatGPT desktop app reads too."
            }
        }
    }

    /// The CLI steps that register `skelly` (the Skelly to start) with it while it is in
    /// `state`, ending in its `mcp add`. Pi's and Codex's replace a server of the same name;
    /// Claude Code's refuses one, so a broken one is removed first.
    pub(crate) fn connect(self, skelly: &Path, state: &State) -> Vec<Vec<OsString>> {
        let words = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        let add: &[&str] = match self {
            Agent::Pi => &["pi", "mcp", "add", "skelly", "--exposure", "direct"],
            Agent::Claude => &["claude", "mcp", "add", "--scope", "user", "skelly"],
            Agent::Codex => &["codex", "mcp", "add", "skelly"],
        };
        let mut add = words(add);
        add.extend([OsString::from("--"), skelly.into(), OsString::from("mcp")]);
        let mut steps = Vec::new();
        if self == Agent::Claude && matches!(state, State::Broken(_)) {
            steps.push(words(&[
                "claude", "mcp", "remove", "--scope", "user", "skelly",
            ]));
        }
        steps.push(add);
        steps
    }
}

/// What an agent's CLI says about its `skelly` server.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum State {
    /// Not asked yet.
    Checking,
    /// Its CLI is not on the login `PATH`.
    Missing,
    /// Installed, with no `skelly` server.
    Off,
    /// Its `skelly` server starts and its tools are offered to the model.
    Connected,
    /// It has a `skelly` server that does not work, and why.
    Broken(String),
    /// Its CLI hung or answered in a way Skelly cannot read, and how.
    Unknown(String),
}

impl State {
    /// Whether the user has something to set up: an installed agent without Skelly, or with
    /// one that does not work.
    pub(crate) fn needs_setup(&self) -> bool {
        matches!(self, State::Off | State::Broken(_))
    }
}

/// The Skelly an agent should start: this executable, resolved past symlinks so a client
/// without the shell's `PATH` (a desktop app opened from the Dock) finds it too. A cargo build registers
/// the installed `skelly` on the `PATH` instead, as a build directory is not an install.
pub(crate) fn skelly_program() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("skelly"));
    let chosen = if crate::cli::is_build_dir(&exe) {
        crate::cli::which("skelly").unwrap_or(exe)
    } else {
        exe
    };
    std::fs::canonicalize(&chosen).unwrap_or(chosen)
}

/// `steps` as one line to paste into a shell.
pub(crate) fn shell_line(steps: &[Vec<OsString>]) -> String {
    let step = |args: &Vec<OsString>| {
        let args: Vec<String> = args.iter().map(|a| quote(&a.to_string_lossy())).collect();
        args.join(" ")
    };
    steps.iter().map(step).collect::<Vec<_>>().join(" && ")
}

/// `arg` single-quoted for a POSIX shell when it holds anything but plain path characters.
fn quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Every agent's last known state, and what is in flight.
pub(crate) struct Agents {
    /// The Skelly a connect registers ([`skelly_program`]).
    skelly: PathBuf,
    states: [State; 3],
    connecting: [bool; 3],
    checking: bool,
}

impl Agents {
    /// Agents not asked yet, to be connected to `skelly`.
    pub(crate) fn new(skelly: PathBuf) -> Self {
        Self {
            skelly,
            states: [State::Checking, State::Checking, State::Checking],
            connecting: [false; 3],
            checking: false,
        }
    }

    pub(crate) fn state(&self, agent: Agent) -> &State {
        &self.states[agent.index()]
    }

    /// Whether a connect of `agent` is running.
    pub(crate) fn connecting(&self, agent: Agent) -> bool {
        self.connecting[agent.index()]
    }

    /// Whether any agent needs setting up that is not being connected now: the sidebar's
    /// settings gear and the page's nav entry show a dot while it does.
    pub(crate) fn attention(&self) -> bool {
        Agent::ALL
            .into_iter()
            .any(|agent| !self.connecting(agent) && self.state(agent).needs_setup())
    }

    /// Note a check going out; `false` when one is running already, so checks never queue up.
    pub(crate) fn begin_check(&mut self) -> bool {
        !std::mem::replace(&mut self.checking, true)
    }

    /// Note a connect of `agent` going out, returning the steps it runs; `None` when the agent
    /// has nothing to set up or is being connected already.
    pub(crate) fn begin_connect(&mut self, agent: Agent) -> Option<Vec<Vec<OsString>>> {
        if self.connecting(agent) || !self.state(agent).needs_setup() {
            return None;
        }
        self.connecting[agent.index()] = true;
        Some(self.steps(agent))
    }

    /// The steps connecting `agent` runs from its current state.
    pub(crate) fn steps(&self, agent: Agent) -> Vec<Vec<OsString>> {
        agent.connect(&self.skelly, self.state(agent))
    }

    /// Take in what the prober found.
    pub(crate) fn apply(&mut self, report: probe::Report) {
        match report {
            probe::Report::State(agent, state) => self.states[agent.index()] = state,
            probe::Report::Checked => self.checking = false,
            probe::Report::Connected(agent, _) => self.connecting[agent.index()] = false,
        }
    }
}

/// What a click on the page lands on. Row indices address [`Agent::ALL`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Target {
    /// An agent's row outside its button: focus it.
    Row(usize),
    /// An agent's Connect or Fix button.
    Connect(usize),
    /// The focused agent's Copy button: copy its command.
    Copy,
    /// "Check again": ask every agent afresh.
    Check,
}

/// The page's view state for one paint.
pub(crate) struct Page<'a> {
    pub(crate) agents: &'a Agents,
    /// The focused row (clamped to the agents).
    pub(crate) selected: usize,
    pub(crate) hover: Option<Target>,
}

/// The display list the page draws into, plus every clickable rect in draw order.
pub(crate) struct Out<'a> {
    pub(crate) quads: &'a mut Vec<ChromeQuad>,
    pub(crate) labels: &'a mut Vec<ProseLabel>,
    pub(crate) targets: Vec<(PxRect, Target)>,
}

impl Page<'_> {
    /// Lay the page out: a card with a row per agent - its status dot, name and state, and a
    /// Connect or Fix button when it needs one - under a label line with "Check again", then
    /// the focused agent's command in a well, with Copy, and notes on what it does.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the list is a handful of agents"
    )]
    pub(crate) fn build(
        &self,
        area: &Area,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
        out: &mut Out<'_>,
    ) {
        let selected = self.selected.min(Agent::ALL.len() - 1);
        self.push_list_label(area, scale, theme, measure, out);
        let top = area.top + LIST_LABEL_H * scale;
        let card = PxRect {
            x: area.x,
            y: top,
            w: area.right - area.x,
            h: Agent::ALL.len() as f32 * ROW_H * scale,
        };
        let (ring, fill) = (theme.border_subtle, card_fill(theme));
        push_ringed(out.quads, card, ring, fill, CARD_RADIUS * scale, scale);
        for (i, agent) in Agent::ALL.into_iter().enumerate() {
            let y = top + i as f32 * ROW_H * scale;
            self.push_agent(i, agent, (y, selected), area, scale, theme, measure, out);
        }
        let detail_top = card.y + card.h + DETAIL_GAP * scale;
        let agent = Agent::ALL[selected];
        self.push_detail(agent, detail_top, area, scale, theme, measure, out);
    }

    /// The list's label line: "Coding agents" at the left in the settings section-label style,
    /// and "Check again" at the right, or "Checking…" while a check runs.
    fn push_list_label(
        &self,
        area: &Area,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
        out: &mut Out<'_>,
    ) {
        let micro = measure.line_height(FontRole::Micro);
        let line_bottom = area.top + (LIST_LABEL_H - 8.0) * scale;
        out.labels.push(ProseLabel {
            text: "CODING AGENTS".to_owned(),
            x: area.x + 2.0 * scale,
            y: line_bottom - micro,
            role: FontRole::Micro,
            color: theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
            shrink: 0.0,
        });
        let checking = self.agents.checking;
        let text = if checking {
            "Checking\u{2026}"
        } else {
            "Check again"
        };
        let caption = measure.line_height(FontRole::Caption);
        let w = measure.width(text, FontRole::Caption, None);
        let rect = PxRect {
            x: area.right - 2.0 * scale - w,
            y: line_bottom - micro * 0.5 - caption * 0.5,
            w,
            h: caption,
        };
        let color = if checking {
            theme.fg_muted
        } else if self.hover == Some(Target::Check) {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        out.labels.push(ProseLabel {
            text: text.to_owned(),
            x: rect.x,
            y: rect.y,
            role: FontRole::Caption,
            color,
            weight: None,
            max_w: f32::MAX,
            shrink: 0.0,
        });
        if !checking {
            out.targets.push((rect, Target::Check));
        }
    }

    /// Agent row `i` at `at = (top, selected)`: its click target, its focus or hover pill, a
    /// hairline above it (unless it or the row above is lit), its button when it needs one, its
    /// status dot, and its name over its state.
    #[allow(clippy::too_many_arguments, reason = "one focused row builder")]
    fn push_agent(
        &self,
        i: usize,
        agent: Agent,
        at: (f32, usize),
        area: &Area,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
        out: &mut Out<'_>,
    ) {
        let (y, selected) = at;
        let band = PxRect {
            x: area.x,
            y,
            w: area.right - area.x,
            h: ROW_H * scale,
        };
        out.targets.push((band, Target::Row(i)));
        let hovered = match self.hover {
            Some(Target::Row(row) | Target::Connect(row)) => Some(row),
            _ => None,
        };
        let lit = |row: usize| row == selected || hovered == Some(row);
        let card = card_fill(theme);
        let pill = PxRect {
            x: band.x + PILL_INSET * scale,
            y: band.y + PILL_INSET * scale,
            w: band.w - 2.0 * PILL_INSET * scale,
            h: band.h - 2.0 * PILL_INSET * scale,
        };
        let radius = PILL_RADIUS * scale;
        if i == selected {
            let fill = theme.accent_subtle_on(card);
            out.quads.push(ChromeQuad::rounded(pill, fill, radius));
        } else if hovered == Some(i) {
            out.quads
                .push(ChromeQuad::rounded(pill, hover_fill(theme, card), radius));
        }
        if i > 0 && !lit(i) && !lit(i - 1) {
            let rule = PxRect {
                x: band.x + ROW_PAD_X * scale,
                y,
                w: band.w - 2.0 * ROW_PAD_X * scale,
                h: scale.max(1.0),
            };
            out.quads.push(ChromeQuad::fill(rule, theme.border_subtle));
        }
        let state = self.agents.state(agent);
        let connecting = self.agents.connecting(agent);
        let left = band.x + ROW_PAD_X * scale;
        let mut right = band.x + band.w - ROW_PAD_X * scale;
        if let Some(text) = action(state).filter(|_| !connecting) {
            let at = (right, y, ROW_H);
            let (quads, labels) = (&mut *out.quads, &mut *out.labels);
            let rect = push_button(quads, labels, text, theme.accent, at, scale, theme, measure);
            out.targets.push((rect, Target::Connect(i)));
            right = rect.x - BUTTON_GAP * scale;
        }
        let d = DOT * scale;
        let dot = PxRect {
            x: left,
            y: y + (band.h - d) * 0.5,
            w: d,
            h: d,
        };
        let color = dot_color(state, connecting, theme);
        out.quads.push(ChromeQuad::rounded(dot, color, d * 0.5));
        let x = left + d + NAME_GAP * scale;
        let room = (right - x).max(1.0);
        let name_h = measure.line_height(FontRole::Label);
        let status_h = measure.line_height(FontRole::Caption);
        let text_top = y + (band.h - name_h - LINE_GAP * scale - status_h) * 0.5;
        out.labels.push(ProseLabel {
            text: agent.name().to_owned(),
            x,
            y: text_top,
            role: FontRole::Label,
            color: theme.fg_primary,
            weight: None,
            max_w: room,
            shrink: 0.0,
        });
        let (status, color) = status_line(agent, state, connecting, theme);
        let overflows = measure.width(&status, FontRole::Caption, None) > room;
        out.labels.push(ProseLabel {
            text: status,
            x,
            y: text_top + name_h + LINE_GAP * scale,
            role: FontRole::Caption,
            color,
            weight: None,
            max_w: room,
            shrink: if overflows { 1.0 } else { 0.0 },
        });
    }

    /// The focused agent's command, from `top`: a caption naming it with a Copy button, the
    /// command in a `bg.inset` well wrapped to fit, then its notes - why it is broken, how to
    /// install it, what connecting it sets up - stopping at the page's bottom.
    #[allow(
        clippy::too_many_arguments,
        clippy::cast_precision_loss,
        reason = "one focused detail builder; a command wraps to a few lines"
    )]
    fn push_detail(
        &self,
        agent: Agent,
        top: f32,
        area: &Area,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
        out: &mut Out<'_>,
    ) {
        let caption = format!("Command \u{b7} {}", agent.name());
        let (role, muted) = (FontRole::Caption, theme.fg_muted);
        push_row(
            out.labels,
            measure,
            &caption,
            role,
            muted,
            area.x,
            top,
            DETAIL_LABEL_H,
            scale,
        );
        let at = (area.right, top, DETAIL_LABEL_H);
        let (quads, labels) = (&mut *out.quads, &mut *out.labels);
        let copy = theme.fg_secondary;
        let rect = push_button(quads, labels, "Copy", copy, at, scale, theme, measure);
        out.targets.push((rect, Target::Copy));
        let width = area.right - area.x;
        let pad = (WELL_PAD_X * scale, WELL_PAD_Y * scale);
        let command = shell_line(&self.agents.steps(agent));
        let lines = wrap(&command, FontRole::Mono, width - 2.0 * pad.0, measure);
        let line_h = measure.line_height(FontRole::Mono);
        let well = PxRect {
            x: area.x,
            y: top + (DETAIL_LABEL_H + WELL_GAP) * scale,
            w: width,
            h: lines.len() as f32 * line_h + 2.0 * pad.1,
        };
        let (ring, inset) = (theme.border_subtle, theme.bg_inset);
        push_ringed(out.quads, well, ring, inset, WELL_RADIUS * scale, scale);
        for (n, line) in lines.into_iter().enumerate() {
            out.labels.push(ProseLabel {
                text: line,
                x: well.x + pad.0,
                y: well.y + pad.1 + n as f32 * line_h,
                role: FontRole::Mono,
                color: theme.fg_primary,
                weight: None,
                max_w: width - 2.0 * pad.0,
                shrink: 0.0,
            });
        }
        let mut notes: Vec<(String, Srgb)> = Vec::new();
        match self.agents.state(agent) {
            State::Broken(why) => notes.push((why.clone(), theme.diff_del)),
            State::Missing => notes.push((
                format!("Install {} first, then check again.", agent.name()),
                muted,
            )),
            State::Unknown(why) => notes.push((format!("Could not check: {why}"), muted)),
            _ => {}
        }
        notes.push((agent.note().to_owned(), muted));
        let note_h = measure.line_height(role);
        let mut y = well.y + well.h + NOTE_GAP * scale;
        for (text, color) in notes {
            for line in wrap(&text, role, width, measure) {
                if y + note_h > area.bottom {
                    return;
                }
                out.labels.push(ProseLabel {
                    text: line,
                    x: area.x,
                    y,
                    role,
                    color,
                    weight: None,
                    max_w: width,
                    shrink: 0.0,
                });
                y += note_h;
            }
            y += NOTE_SPACING * scale;
        }
    }
}

/// The button a row in `state` offers: Connect for an agent without Skelly, Fix for a broken
/// one.
fn action(state: &State) -> Option<&'static str> {
    match state {
        State::Off => Some("Connect"),
        State::Broken(_) => Some("Fix"),
        _ => None,
    }
}

/// A row's status dot: `status.success` when connected, `status.warning` (the accent) while it
/// needs connecting or is being connected, `status.danger` when broken, faint when there is
/// nothing to say yet.
fn dot_color(state: &State, connecting: bool, theme: &Theme) -> Srgb {
    match state {
        _ if connecting => theme.accent,
        State::Connected => theme.diff_add,
        State::Off => theme.accent,
        State::Broken(_) => theme.diff_del,
        State::Unknown(_) => theme.fg_muted,
        State::Checking | State::Missing => theme.fg_faint,
    }
}

/// A row's state under its name, and its color: `status.danger` for why it is broken.
fn status_line(agent: Agent, state: &State, connecting: bool, theme: &Theme) -> (String, Srgb) {
    let muted = theme.fg_muted;
    match state {
        _ if connecting => ("Connecting\u{2026}".to_owned(), muted),
        State::Checking => ("Checking\u{2026}".to_owned(), muted),
        State::Missing => (
            format!("Not installed \u{b7} no {} on your PATH", agent.program()),
            muted,
        ),
        State::Off => ("Not connected".to_owned(), muted),
        State::Connected => ("Connected".to_owned(), muted),
        State::Broken(why) => (format!("Not working \u{b7} {why}"), theme.diff_del),
        State::Unknown(why) => (format!("Could not check \u{b7} {why}"), muted),
    }
}
