//! The agents' own CLIs, which Settings > Agents reads and writes each agent's MCP config
//! through: `pi mcp list --json`, `claude mcp get skelly` and `codex mcp get skelly --json` to
//! read, each one's `mcp add` to write. Skelly never edits an agent's config file itself, as
//! the agent owns its format. The CLIs run on the [`Prober`]'s thread, as some take seconds
//! (`pi mcp list` starts every server Pi knows), with the `PATH` of the user's login shell:
//! an app opened from the Finder or a desktop launcher gets a minimal `PATH` that holds none
//! of the agents.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde_json::Value;

use super::{Agent, State};

/// How long one CLI may run before it counts as hung.
const CLI_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the login shell may take to print its `PATH`.
const SHELL_TIMEOUT: Duration = Duration::from_secs(10);
/// The marks around the `PATH` the login shell prints, as a profile may print lines of its own.
const MARK: &str = "__skelly_path__";

/// What a CLI printed, and whether it exited 0.
#[derive(Debug, Default)]
pub(crate) struct Output {
    pub(crate) ok: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl Output {
    /// The CLI's own complaint: its first line of stderr, else of stdout.
    fn complaint(&self) -> String {
        [&self.stderr, &self.stdout]
            .iter()
            .find_map(|text| text.lines().map(str::trim).find(|l| !l.is_empty()))
            .map_or_else(|| "it printed nothing".to_owned(), str::to_owned)
    }

    /// Whether the CLI said it has no server of that name (Claude Code and Codex exit 1 so).
    fn no_server(&self) -> bool {
        [&self.stderr, &self.stdout]
            .iter()
            .any(|text| text.contains("No MCP server named"))
    }
}

/// Where the CLIs run.
#[derive(Clone, Debug)]
pub(crate) struct Host {
    /// The `PATH` they are found on, and run with.
    pub(crate) path: OsString,
    /// The directory they run in: home, so no project's own MCP config is in scope.
    pub(crate) home: PathBuf,
}

impl Host {
    /// What a pane's shell sees: the `PATH` of `shell` started as a pane starts it, else this
    /// process's own, in the home directory.
    pub(crate) fn login(shell: &Path) -> Self {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let path = login_path(shell, &home)
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        Self { path, home }
    }

    /// The program `name` runs: itself when it is a path, else the first executable of that
    /// name on the `PATH`.
    pub(crate) fn which(&self, name: &OsStr) -> Option<PathBuf> {
        let name = Path::new(name);
        if name.components().count() > 1 {
            return executable(name).then(|| name.to_path_buf());
        }
        std::env::split_paths(&self.path)
            .map(|dir| dir.join(name))
            .find(|candidate| executable(candidate))
    }
}

/// Whether `path` is a file anyone may run.
fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The shell a pane starts: `[shell] program`, else `$SHELL`, else `/bin/bash`, as
/// `skelly_config::Shell` documents.
pub(crate) fn login_shell(program: &str) -> PathBuf {
    let program = program.trim();
    if !program.is_empty() {
        return PathBuf::from(program);
    }
    std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .map_or_else(|| PathBuf::from("/bin/bash"), PathBuf::from)
}

/// The `PATH` of `shell` started as an interactive login shell, as a pane starts it, so its
/// profile and rc files run; `None` when it prints none.
fn login_path(shell: &Path, home: &Path) -> Option<OsString> {
    let script = format!(r#"printf '\n{MARK}%s{MARK}\n' "$PATH""#);
    let child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let out = finish(child, SHELL_TIMEOUT).ok()?;
    let start = out.stdout.find(MARK)? + MARK.len();
    let end = start + out.stdout[start..].find(MARK)?;
    let path = &out.stdout[start..end];
    (!path.is_empty()).then(|| path.into())
}

/// Run `step` (a program and its arguments) on `host`, the program found on the host's `PATH`.
fn run(host: &Host, step: &[OsString]) -> Result<Output, String> {
    let (program, args) = step.split_first().ok_or("there is nothing to run")?;
    let Some(exe) = host.which(program) else {
        return Err(format!("{} is not on your PATH", program.to_string_lossy()));
    };
    let child = Command::new(&exe)
        .args(args)
        .current_dir(&host.home)
        .env("PATH", &host.path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|err| format!("could not start {}: {err}", exe.display()))?;
    finish(child, CLI_TIMEOUT)
}

/// Wait up to `timeout` for `child`, collecting what it prints. Past it, stop its whole process
/// group: a CLI may be a script that started the real program.
pub(super) fn finish(child: Child, timeout: Duration) -> Result<Output, String> {
    let pid = child.id();
    let (done, outcome) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(child.wait_with_output());
    });
    match outcome.recv_timeout(timeout) {
        Ok(Ok(out)) => Ok(Output {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => {
            // The waiting thread still owns the child, so it is not reaped yet and its group id
            // cannot have been reused.
            if let Ok(pid) = i32::try_from(pid) {
                let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
            }
            Err(format!("no answer within {}s", timeout.as_secs()))
        }
    }
}

/// `words` as an argument vector.
fn argv(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

/// What `agent` says about its `skelly` server on `host`.
pub(crate) fn probe(agent: Agent, host: &Host) -> State {
    if host.which(OsStr::new(agent.program())).is_none() {
        return State::Missing;
    }
    let words: &[&str] = match agent {
        Agent::Pi => &["pi", "mcp", "list", "--json"],
        Agent::Claude => &["claude", "mcp", "get", "skelly"],
        Agent::Codex => &["codex", "mcp", "get", "skelly", "--json"],
    };
    match run(host, &argv(words)) {
        Err(why) => State::Unknown(why),
        Ok(out) => match agent {
            Agent::Pi => read_pi(&out),
            Agent::Claude => read_claude(&out),
            Agent::Codex => read_codex(&out, host),
        },
    }
}

/// Read `pi mcp list --json`, which starts each server to report its state. It exits 1 when
/// any server fails, so its list is read whatever the exit code.
pub(crate) fn read_pi(out: &Output) -> State {
    let list: Option<Value> = serde_json::from_str(&out.stdout).ok();
    let Some(servers) = list.as_ref().and_then(|l| l.get("servers")?.as_array()) else {
        return State::Unknown(out.complaint());
    };
    let Some(server) = servers
        .iter()
        .find(|s| s.get("name").and_then(Value::as_str) == Some("skelly"))
    else {
        return State::Off;
    };
    let field = |key: &str| server.get(key).and_then(Value::as_str);
    if server.get("enabled").and_then(Value::as_bool) == Some(false) {
        return State::Broken("Turned off in Pi".to_owned());
    }
    match field("state") {
        Some("connected") => {}
        state => {
            let why = field("error").map_or_else(
                || format!("Pi reports it {}", state.unwrap_or("not connected")),
                str::to_owned,
            );
            return State::Broken(why);
        }
    }
    match field("exposure") {
        Some("direct") => State::Connected,
        exposure => State::Broken(format!(
            "Its tools are not exposed directly (exposure: {})",
            exposure.unwrap_or("codemode")
        )),
    }
}

/// Read `claude mcp get skelly`, which health-checks the server. It prints text only: a
/// `Status:` line, and an `Issue:` line when the server fails.
pub(crate) fn read_claude(out: &Output) -> State {
    if !out.ok {
        return if out.no_server() {
            State::Off
        } else {
            State::Unknown(out.complaint())
        };
    }
    let field = |name: &str| {
        out.stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix(name))
            .map(|value| {
                value
                    .trim_start_matches(|c: char| !c.is_alphanumeric())
                    .trim()
            })
    };
    let Some(status) = field("Status:") else {
        return State::Unknown("Claude Code printed no status".to_owned());
    };
    let lower = status.to_lowercase();
    if lower.contains("connected") && !lower.contains("fail") && !lower.contains("not ") {
        State::Connected
    } else {
        State::Broken(field("Issue:").unwrap_or(status).to_owned())
    }
}

/// Read `codex mcp get skelly --json`. Codex does not start the server to list it, so the
/// command it would start is checked here.
pub(crate) fn read_codex(out: &Output, host: &Host) -> State {
    if !out.ok {
        return if out.no_server() {
            State::Off
        } else {
            State::Unknown(out.complaint())
        };
    }
    let Ok(server) = serde_json::from_str::<Value>(&out.stdout) else {
        return State::Unknown(out.complaint());
    };
    if server.get("enabled").and_then(Value::as_bool) == Some(false) {
        return State::Broken("Turned off in Codex".to_owned());
    }
    let transport = server.get("transport");
    let field = |key: &str| transport.and_then(|t| t.get(key));
    if field("type").and_then(Value::as_str) != Some("stdio") {
        return State::Broken("Codex reaches it over a URL, not through skelly mcp".to_owned());
    }
    let command = field("command").and_then(Value::as_str).unwrap_or_default();
    let args: Vec<&str> = field("args")
        .and_then(Value::as_array)
        .map(|args| args.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if args != ["mcp"] {
        let line: Vec<&str> = std::iter::once(command).chain(args).collect();
        return State::Broken(format!(
            "Codex starts {} instead of skelly mcp",
            line.join(" ")
        ));
    }
    if host.which(OsStr::new(command)).is_some() {
        State::Connected
    } else if Path::new(command).is_absolute() {
        State::Broken(format!("No program at {command}"))
    } else {
        State::Broken(format!("{command} is not on your PATH"))
    }
}

/// Run `steps` on `host`: every step but the last tidies up and may fail; the last is the
/// `mcp add`, whose complaint is the error.
fn connect(host: &Host, steps: &[Vec<OsString>]) -> Result<(), String> {
    let Some((add, tidy)) = steps.split_last() else {
        return Ok(());
    };
    for step in tidy {
        let _ = run(host, step);
    }
    let out = run(host, add)?;
    if out.ok {
        Ok(())
    } else {
        Err(out.complaint())
    }
}

/// What the page asks of the [`Prober`].
#[derive(Debug)]
pub(crate) enum Request {
    /// Ask every agent for its state, reading the login `PATH` afresh.
    Check,
    /// Run these CLI steps to connect `agent` (see [`Agent::connect`]), then ask it again.
    Connect(Agent, Vec<Vec<OsString>>),
}

/// What the [`Prober`] found.
#[derive(Debug, PartialEq)]
pub(crate) enum Report {
    /// `agent`'s current state.
    State(Agent, State),
    /// A [`Request::Check`] has reported every agent.
    Checked,
    /// A [`Request::Connect`] ran, after its agent's fresh [`Report::State`]: `Err` holds the
    /// CLI's complaint.
    Connected(Agent, Result<(), String>),
}

/// The worker that runs the agents' CLIs, one request at a time, so the UI never waits on one.
pub(crate) struct Prober {
    requests: Sender<Request>,
}

impl Prober {
    /// Start the worker, finding the CLIs where `locate` says and handing each finding to
    /// `report`. It ends when the `Prober` is dropped and its current request is done.
    pub(crate) fn spawn(
        locate: impl Fn() -> Host + Send + 'static,
        report: impl Fn(Report) + Send + 'static,
    ) -> Self {
        let (requests, inbox) = mpsc::channel();
        std::thread::spawn(move || serve(&inbox, &locate, &report));
        Self { requests }
    }

    /// Queue `request`.
    pub(crate) fn send(&self, request: Request) {
        let _ = self.requests.send(request);
    }
}

/// The worker's loop. A check asks the three agents at once and reports each as it answers.
fn serve(inbox: &Receiver<Request>, locate: &dyn Fn() -> Host, report: &dyn Fn(Report)) {
    let mut host: Option<Host> = None;
    while let Ok(request) = inbox.recv() {
        match request {
            Request::Check => {
                let fresh = locate();
                std::thread::scope(|scope| {
                    let (found, answers) = mpsc::channel();
                    for agent in Agent::ALL {
                        let (found, fresh) = (found.clone(), &fresh);
                        scope.spawn(move || {
                            let _ = found.send((agent, probe(agent, fresh)));
                        });
                    }
                    drop(found);
                    for (agent, state) in answers {
                        report(Report::State(agent, state));
                    }
                });
                host = Some(fresh);
                report(Report::Checked);
            }
            Request::Connect(agent, steps) => {
                let host = host.get_or_insert_with(locate);
                let result = connect(host, &steps);
                report(Report::State(agent, probe(agent, host)));
                report(Report::Connected(agent, result));
            }
        }
    }
}
