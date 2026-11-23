//! The App's side of Settings > Agents: the worker it starts at launch, what it does with the
//! worker's reports, and the page's keys and clicks.

use std::sync::mpsc::{self, Receiver};

use winit::event::KeyEvent;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{Key, NamedKey};

use super::probe::{self, Host, Prober, Report, Request};
use super::{shell_line, skelly_program, Agent, Agents, State, Target};
use crate::{clipboard_action, App, ClipboardAction, ToastKind, Wakeup};

/// Start the worker that runs the agents' CLIs from `shell`'s login `PATH`, its reports waking
/// the event loop through `proxy`, and ask it for every agent's state at once, so the settings
/// gear can call out an agent that needs setting up from launch.
pub(crate) fn start(
    shell: &str,
    proxy: &EventLoopProxy<Wakeup>,
) -> (Agents, Prober, Receiver<Report>) {
    let (found, reports) = mpsc::channel();
    let (wake, shell) = (proxy.clone(), probe::login_shell(shell));
    let prober = Prober::spawn(
        move || Host::login(&shell),
        move |report| {
            if found.send(report).is_ok() {
                let _ = wake.send_event(Wakeup::Agents);
            }
        },
    );
    let mut agents = Agents::new(skelly_program());
    if agents.begin_check() {
        prober.send(Request::Check);
    }
    (agents, prober, reports)
}

impl App {
    /// Take in what the agent worker found, saying how each connect went in a toast, and repaint
    /// what shows it: the open settings view, or the settings gear when its dot comes or goes.
    pub(crate) fn drain_agents(&mut self) {
        let attention = self.agents.attention();
        while let Ok(report) = self.agent_reports.try_recv() {
            let done = match &report {
                Report::Connected(agent, result) => Some((*agent, result.clone())),
                _ => None,
            };
            self.agents.apply(report);
            if let Some((agent, result)) = done {
                self.report_connect(agent, result);
            }
        }
        if self.settings.open || self.agents.attention() != attention {
            self.request_redraw();
        }
    }

    /// Say how connecting `agent` went: the `mcp add`'s `result`, then what the agent said
    /// about its `skelly` server afterwards.
    fn report_connect(&mut self, agent: Agent, result: Result<(), String>) {
        let name = agent.name();
        let (message, kind) = match (result, self.agents.state(agent)) {
            (Err(why), _) => (format!("Could not connect {name}: {why}"), ToastKind::Error),
            (Ok(()), State::Connected) => (
                format!("{name} is connected - a new session can use Skelly's review tools"),
                ToastKind::Success,
            ),
            (Ok(()), State::Broken(why)) => (
                format!("{name} has Skelly, but it does not start: {why}"),
                ToastKind::Error,
            ),
            (Ok(()), _) => (
                format!("{name} took the command, but did not say it is connected"),
                ToastKind::Error,
            ),
        };
        self.show_toast(message, kind);
    }

    /// Ask every agent for its state afresh, unless a check is running already.
    pub(crate) fn check_agents(&mut self) {
        if self.agents.begin_check() {
            self.agent_prober.send(Request::Check);
            self.request_redraw();
        }
    }

    /// Connect `agent` to Skelly, or repair its connection, when it needs it.
    fn connect_agent(&mut self, agent: Agent) {
        if let Some(steps) = self.agents.begin_connect(agent) {
            self.agent_prober.send(Request::Connect(agent, steps));
            self.request_redraw();
        }
    }

    /// Copy the command that connects `agent`, for running it by hand.
    fn copy_agent_command(&mut self, agent: Agent) {
        let line = shell_line(&self.agents.steps(agent));
        let copied = self
            .clipboard
            .as_mut()
            .is_some_and(|clipboard| clipboard.set_text(line).is_ok());
        if copied {
            let message = format!("Copied the command that connects {}", agent.name());
            self.show_toast(message, ToastKind::Success);
        } else {
            self.show_toast("Clipboard unavailable", ToastKind::Error);
        }
    }

    /// Keys on the Settings > Agents page: `↑/↓` move between agents, Enter connects the focused
    /// one or repairs it, the copy chord copies its command, and `r` checks every agent again.
    pub(crate) fn on_agents_key(&mut self, key_event: &KeyEvent) {
        let count = Agent::ALL.len();
        let agent = Agent::ALL[self.settings.selected_row().min(count - 1)];
        if clipboard_action(&key_event.logical_key, self.modifiers) == Some(ClipboardAction::Copy) {
            self.copy_agent_command(agent);
            return;
        }
        let mods = self.modifiers;
        let plain = !(mods.super_key() || mods.control_key() || mods.alt_key());
        match key_event.logical_key.as_ref() {
            Key::Named(NamedKey::ArrowUp) => self.settings.move_row(-1, count),
            Key::Named(NamedKey::ArrowDown) => self.settings.move_row(1, count),
            Key::Named(NamedKey::Enter) => self.connect_agent(agent),
            Key::Character(ch) if plain && ch.eq_ignore_ascii_case("r") => self.check_agents(),
            _ => {}
        }
        self.request_redraw();
    }

    /// A click on the Agents page: focus an agent, connect or repair it, copy the focused agent's
    /// command, or check every agent again.
    pub(crate) fn on_agents_click(&mut self, target: Target) {
        match target {
            Target::Row(i) => self.settings.select_row(i),
            Target::Connect(i) => {
                self.settings.select_row(i);
                self.connect_agent(Agent::ALL[i]);
            }
            Target::Copy => {
                let i = self.settings.selected_row().min(Agent::ALL.len() - 1);
                self.copy_agent_command(Agent::ALL[i]);
            }
            Target::Check => self.check_agents(),
        }
    }

    /// Check the agents afresh when the settings view has just turned to the Agents page
    /// (`was_on_agents`: whether it showed before), so the page shows their state now and not
    /// as it was at launch.
    pub(crate) fn check_agents_on_arrival(&mut self, was_on_agents: bool) {
        if self.settings.open && self.settings.on_agents_page() && !was_on_agents {
            self.check_agents();
        }
    }
}
