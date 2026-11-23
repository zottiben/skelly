use std::ffi::OsString;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use skelly_render::{PxRect, TextMeasure, Theme};

use super::probe::{self, Host, Output, Prober, Report, Request};
use super::{shell_line, Agent, Agents, Out, Page, State, Target};
use crate::workspace::Area;

const SKELLY: &str = "/Applications/Skelly.app/Contents/MacOS/skelly";

fn out(ok: bool, stdout: &str, stderr: &str) -> Output {
    Output {
        ok,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    }
}

fn line(agent: Agent, state: &State) -> String {
    shell_line(&agent.connect(Path::new(SKELLY), state))
}

/// Write an executable script from a child shell, so this process never holds a write fd on
/// it: a process another test thread is spawning would inherit that fd, and running a script
/// some process can still write fails with `ETXTBSY`.
fn script(path: &Path, body: &str) {
    let status = Command::new("/bin/sh")
        .args([
            "-c",
            r#"printf '%s' "$1" > "$2" && chmod 700 "$2""#,
            "sh",
            body,
        ])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "write {}", path.display());
}

#[test]
fn each_agent_connects_through_its_own_mcp_add() {
    assert_eq!(
        line(Agent::Pi, &State::Off),
        format!("pi mcp add skelly --exposure direct -- {SKELLY} mcp")
    );
    assert_eq!(
        line(Agent::Claude, &State::Off),
        format!("claude mcp add --scope user skelly -- {SKELLY} mcp")
    );
    assert_eq!(
        line(Agent::Codex, &State::Off),
        format!("codex mcp add skelly -- {SKELLY} mcp")
    );
    // Pi's and Codex's `add` replace a broken entry; Claude Code's refuses one, so it goes first.
    let broken = State::Broken("ENOENT".into());
    assert_eq!(line(Agent::Pi, &broken), line(Agent::Pi, &State::Off));
    assert_eq!(
        line(Agent::Claude, &broken),
        format!(
            "claude mcp remove --scope user skelly && claude mcp add --scope user skelly -- \
             {SKELLY} mcp"
        )
    );
}

#[test]
fn a_command_line_quotes_what_a_shell_would_split() {
    let steps = vec![vec![
        OsString::from("codex"),
        "--".into(),
        "/Users/a b/it's/skelly".into(),
        "mcp".into(),
    ]];
    assert_eq!(
        shell_line(&steps),
        r"codex -- '/Users/a b/it'\''s/skelly' mcp"
    );
}

#[test]
fn pi_reads_its_server_list_whatever_its_exit_code() {
    let list = |server: &str| format!(r#"{{"servers":[{server}],"errors":[]}}"#);
    let connected = r#"{"name":"skelly","scope":"global","enabled":true,"exposure":"direct",
        "transport":"/Applications/Skelly.app/Contents/MacOS/skelly mcp","state":"connected",
        "tools":["list_review_comments"]}"#;
    assert_eq!(
        probe::read_pi(&out(true, &list(connected), "")),
        State::Connected
    );
    // Another server failing makes `pi mcp list` exit 1; its list still holds the answer.
    assert_eq!(
        probe::read_pi(&out(false, &list(connected), "figma needs auth")),
        State::Connected
    );
    let failed = r#"{"name":"skelly","enabled":true,"exposure":"direct",
        "transport":"/nope/skelly mcp","state":"failed","tools":[],
        "error":"spawn /nope/skelly ENOENT"}"#;
    assert_eq!(
        probe::read_pi(&out(false, &list(failed), "")),
        State::Broken("spawn /nope/skelly ENOENT".into())
    );
    let hidden = r#"{"name":"skelly","enabled":true,"exposure":"codemode","state":"connected"}"#;
    assert!(matches!(
        probe::read_pi(&out(true, &list(hidden), "")),
        State::Broken(why) if why.contains("codemode")
    ));
    let off = r#"{"name":"skelly","enabled":false,"exposure":"direct","state":"connected"}"#;
    assert_eq!(
        probe::read_pi(&out(true, &list(off), "")),
        State::Broken("Turned off in Pi".into())
    );
    let other = r#"{"name":"herd","enabled":true,"state":"connected"}"#;
    assert_eq!(probe::read_pi(&out(true, &list(other), "")), State::Off);
    assert_eq!(
        probe::read_pi(&out(false, "", "pi: unknown command mcp\n")),
        State::Unknown("pi: unknown command mcp".into())
    );
}

#[test]
fn claude_code_reads_its_status_and_issue_lines() {
    let connected = "skelly:\n  Scope: User config (available in all your projects)\n  \
                     Status: \u{2714} Connected\n  Type: stdio\n  Command: /x/skelly\n";
    assert_eq!(
        probe::read_claude(&out(true, connected, "")),
        State::Connected
    );
    let failed = "skelly:\n  Scope: User config\n  Status: \u{2718} Failed to connect\n  \
                  Issue: ENOENT: no such file or directory, posix_spawn '/nope/skelly'\n";
    assert_eq!(
        probe::read_claude(&out(true, failed, "")),
        State::Broken("ENOENT: no such file or directory, posix_spawn '/nope/skelly'".into())
    );
    let missing = "No MCP server named \"skelly\". Run `claude mcp add` to add one.\n";
    assert_eq!(probe::read_claude(&out(false, missing, "")), State::Off);
    assert_eq!(probe::read_claude(&out(false, "", missing)), State::Off);
    assert_eq!(
        probe::read_claude(&out(false, "", "Error: config is corrupt\n")),
        State::Unknown("Error: config is corrupt".into())
    );
}

#[test]
fn codex_checks_the_command_it_would_start() {
    let dir = tempfile::tempdir().unwrap();
    let skelly = dir.path().join("skelly");
    script(&skelly, "#!/bin/sh\n");
    let host = Host {
        path: dir.path().into(),
        home: dir.path().into(),
    };
    let server = |enabled: bool, command: &str, args: &str| {
        format!(
            r#"{{"name":"skelly","enabled":{enabled},"disabled_reason":null,"transport":
            {{"type":"stdio","command":"{command}","args":{args},"env":null}}}}"#
        )
    };
    let read = |json: String| probe::read_codex(&out(true, &json, ""), &host);
    let path = skelly.to_string_lossy();
    assert_eq!(read(server(true, &path, r#"["mcp"]"#)), State::Connected);
    assert_eq!(read(server(true, "skelly", r#"["mcp"]"#)), State::Connected);
    assert_eq!(
        read(server(true, "/nope/skelly", r#"["mcp"]"#)),
        State::Broken("No program at /nope/skelly".into())
    );
    assert_eq!(
        read(server(true, "gone", r#"["mcp"]"#)),
        State::Broken("gone is not on your PATH".into())
    );
    assert_eq!(
        read(server(true, &path, "[]")),
        State::Broken(format!("Codex starts {path} instead of skelly mcp"))
    );
    assert_eq!(
        read(server(false, &path, r#"["mcp"]"#)),
        State::Broken("Turned off in Codex".into())
    );
    let missing = "Error: No MCP server named 'skelly' found.\n";
    assert_eq!(
        probe::read_codex(&out(false, "", missing), &host),
        State::Off
    );
}

#[test]
fn which_finds_executables_and_takes_paths_as_they_are() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, other) = (dir.path().join("bin"), dir.path().join("other"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    script(&other.join("pi"), "#!/bin/sh\n");
    std::fs::write(bin.join("pi"), "not executable").unwrap();
    let host = Host {
        path: std::env::join_paths([&bin, &other]).unwrap(),
        home: dir.path().into(),
    };
    assert_eq!(host.which("pi".as_ref()), Some(other.join("pi")));
    assert_eq!(host.which("codex".as_ref()), None);
    let absolute = other.join("pi");
    assert_eq!(host.which(absolute.as_os_str()), Some(absolute));
    assert_eq!(host.which(bin.join("pi").as_os_str()), None);
}

#[test]
fn the_login_path_is_read_between_its_marks() {
    let dir = tempfile::tempdir().unwrap();
    let shell = dir.path().join("fake-shell");
    // A profile that prints a banner, as a login shell's may; `$4` is the `-c` script.
    script(
        &shell,
        "#!/bin/sh\necho 'Welcome back'\nPATH=/opt/agents/bin:/usr/bin\nexport PATH\neval \"$4\"\n",
    );
    let host = Host::login(&shell);
    assert_eq!(host.path, OsString::from("/opt/agents/bin:/usr/bin"));
}

#[test]
fn the_page_needs_the_user_only_for_an_installed_agent_without_a_working_skelly() {
    let mut agents = Agents::new(SKELLY.into());
    assert!(
        !agents.attention(),
        "nothing is known before the first check"
    );
    assert!(agents.begin_check());
    assert!(!agents.begin_check(), "a running check is not queued again");
    agents.apply(Report::State(Agent::Pi, State::Connected));
    agents.apply(Report::State(Agent::Claude, State::Missing));
    agents.apply(Report::State(Agent::Codex, State::Unknown("hung".into())));
    agents.apply(Report::Checked);
    assert!(!agents.attention());
    assert!(agents.begin_check());
    assert!(
        agents.begin_connect(Agent::Pi).is_none(),
        "connected already"
    );
    assert!(
        agents.begin_connect(Agent::Claude).is_none(),
        "not installed"
    );
    agents.apply(Report::State(Agent::Codex, State::Off));
    assert!(agents.attention());
    let steps = agents.begin_connect(Agent::Codex).unwrap();
    assert_eq!(steps, Agent::Codex.connect(Path::new(SKELLY), &State::Off));
    assert!(
        !agents.attention(),
        "an agent being connected is not called out"
    );
    assert!(
        agents.begin_connect(Agent::Codex).is_none(),
        "one connect at a time"
    );
    agents.apply(Report::State(Agent::Codex, State::Connected));
    agents.apply(Report::Connected(Agent::Codex, Ok(())));
    assert!(!agents.connecting(Agent::Codex) && !agents.attention());
}

/// Fake `pi`, `claude` and `codex` CLIs in `bin` that keep their `skelly` server's command in
/// `state/<cli>` and log each call to `state/log`, answering as the real ones do (see the
/// output read by the tests above).
fn fake_agents(bin: &Path, state: &Path) {
    let head = |cli: &str| {
        format!(
            "#!/bin/sh\nstate='{}/{cli}'\nprintf '%s\\n' \"{cli} $*\" >> '{}/log'\n",
            state.display(),
            state.display()
        )
    };
    let take_command = "shift 2; while [ \"$1\" != -- ]; do shift; done; printf '%s' \"$2\" \
                        > \"$state\"\n";
    let pi = format!(
        "{}case \"$1 $2\" in\n\
         'mcp list')\n\
           if [ ! -f \"$state\" ]; then echo '{{\"servers\":[],\"errors\":[]}}'; exit 0; fi\n\
           cmd=$(cat \"$state\")\n\
           if [ -x \"$cmd\" ]; then\n\
             echo \"{{\\\"servers\\\":[{{\\\"name\\\":\\\"skelly\\\",\\\"enabled\\\":true,\
         \\\"exposure\\\":\\\"direct\\\",\\\"state\\\":\\\"connected\\\"}}]}}\"\n\
           else\n\
             echo \"{{\\\"servers\\\":[{{\\\"name\\\":\\\"skelly\\\",\\\"enabled\\\":true,\
         \\\"exposure\\\":\\\"direct\\\",\\\"state\\\":\\\"failed\\\",\
         \\\"error\\\":\\\"spawn $cmd ENOENT\\\"}}]}}\"; exit 1\n\
           fi ;;\n\
         'mcp add') {take_command} echo 'Added global MCP server \"skelly\"' ;;\n\
         esac\n",
        head("pi")
    );
    let claude = format!(
        "{}case \"$1 $2\" in\n\
         'mcp get')\n\
           if [ ! -f \"$state\" ]; then\n\
             echo 'No MCP server named \"skelly\". Run `claude mcp add` to add one.'; exit 1\n\
           fi\n\
           cmd=$(cat \"$state\")\n\
           if [ -x \"$cmd\" ]; then printf 'skelly:\\n  Status: \u{2714} Connected\\n'\n\
           else printf 'skelly:\\n  Status: \u{2718} Failed to connect\\n  Issue: ENOENT %s\\n' \"$cmd\"\n\
           fi ;;\n\
         'mcp add')\n\
           if [ -f \"$state\" ]; then\n\
             echo 'MCP server skelly already exists in user config' >&2; exit 1\n\
           fi\n\
           {take_command} ;;\n\
         'mcp remove') rm -f \"$state\" ;;\n\
         esac\n",
        head("claude")
    );
    let codex = format!(
        "{}case \"$1 $2\" in\n\
         'mcp get')\n\
           if [ ! -f \"$state\" ]; then\n\
             echo \"Error: No MCP server named 'skelly' found.\" >&2; exit 1\n\
           fi\n\
           echo \"{{\\\"name\\\":\\\"skelly\\\",\\\"enabled\\\":true,\\\"transport\\\":\
         {{\\\"type\\\":\\\"stdio\\\",\\\"command\\\":\\\"$(cat \"$state\")\\\",\
         \\\"args\\\":[\\\"mcp\\\"]}}}}\" ;;\n\
         'mcp add') {take_command} echo \"Added global MCP server 'skelly'.\" ;;\n\
         esac\n",
        head("codex")
    );
    script(&bin.join("pi"), &pi);
    script(&bin.join("claude"), &claude);
    script(&bin.join("codex"), &codex);
}

/// Collect `rx`'s reports through the next one that `last` matches.
fn reports_until(rx: &mpsc::Receiver<Report>, last: impl Fn(&Report) -> bool) -> Vec<Report> {
    let mut seen = Vec::new();
    loop {
        let report = rx.recv_timeout(Duration::from_secs(20)).expect("a report");
        let done = last(&report);
        seen.push(report);
        if done {
            return seen;
        }
    }
}

fn states(reports: &[Report]) -> Vec<(Agent, State)> {
    let mut states: Vec<(Agent, State)> = reports
        .iter()
        .filter_map(|r| match r {
            Report::State(agent, state) => Some((*agent, state.clone())),
            _ => None,
        })
        .collect();
    states.sort_by_key(|(agent, _)| *agent as usize);
    states
}

#[test]
fn the_prober_checks_connects_and_repairs_through_the_clis() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, state) = (dir.path().join("bin"), dir.path().join("state"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    fake_agents(&bin, &state);
    let skelly = dir.path().join("skelly");
    script(&skelly, "#!/bin/sh\n");
    // The fakes need `cat`; no real agent CLI lives in the system dirs.
    let path = std::env::join_paths([bin.as_path(), "/usr/bin".as_ref(), "/bin".as_ref()]);
    let host = Host {
        path: path.unwrap(),
        home: dir.path().into(),
    };
    let (tx, rx) = mpsc::channel();
    let probe_host = host.clone();
    let prober = Prober::spawn(
        move || probe_host.clone(),
        move |report| {
            let _ = tx.send(report);
        },
    );

    prober.send(Request::Check);
    let seen = reports_until(&rx, |r| *r == Report::Checked);
    let off: Vec<_> = Agent::ALL.into_iter().map(|a| (a, State::Off)).collect();
    assert_eq!(states(&seen), off);

    for agent in Agent::ALL {
        let steps = agent.connect(&skelly, &State::Off);
        prober.send(Request::Connect(agent, steps));
        let seen = reports_until(&rx, |r| matches!(r, Report::Connected(..)));
        assert_eq!(
            seen,
            vec![
                Report::State(agent, State::Connected),
                Report::Connected(agent, Ok(())),
            ]
        );
    }
    let log = std::fs::read_to_string(state.join("log")).unwrap();
    let skelly_mcp = format!("-- {} mcp", skelly.display());
    for add in [
        "pi mcp add skelly --exposure direct",
        "claude mcp add --scope user skelly",
        "codex mcp add skelly",
    ] {
        assert!(log.contains(&format!("{add} {skelly_mcp}")), "{add}: {log}");
    }

    // Skelly moved: every agent now starts a program that is gone.
    for cli in ["pi", "claude", "codex"] {
        std::fs::write(state.join(cli), "/nope/skelly").unwrap();
    }
    prober.send(Request::Check);
    let seen = reports_until(&rx, |r| *r == Report::Checked);
    let broken = states(&seen);
    assert_eq!(
        broken,
        vec![
            (Agent::Pi, State::Broken("spawn /nope/skelly ENOENT".into())),
            (Agent::Claude, State::Broken("ENOENT /nope/skelly".into())),
            (
                Agent::Codex,
                State::Broken("No program at /nope/skelly".into())
            ),
        ]
    );
    for (agent, was) in broken {
        prober.send(Request::Connect(agent, agent.connect(&skelly, &was)));
        let seen = reports_until(&rx, |r| matches!(r, Report::Connected(..)));
        assert_eq!(seen[0], Report::State(agent, State::Connected), "{agent:?}");
    }
    assert_eq!(
        std::fs::read_to_string(state.join("claude")).unwrap(),
        skelly.to_string_lossy()
    );

    // An agent whose CLI is not on the PATH is not installed, and a connect says why.
    std::fs::remove_file(bin.join("codex")).unwrap();
    assert_eq!(probe::probe(Agent::Codex, &host), State::Missing);
}

#[test]
fn a_hung_cli_is_stopped_with_everything_it_started() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let hang = dir.path().join("hang");
    script(
        &hang,
        &format!(
            "#!/bin/sh\nsleep 30 &\necho $! > '{}'\nwait\n",
            pid_file.display()
        ),
    );
    let child = Command::new(&hang)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let result = probe::finish(child, Duration::from_secs(1));
    assert_eq!(result.unwrap_err(), "no answer within 1s");
    assert!(started.elapsed() < Duration::from_secs(10));
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    let alive = || {
        Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    for _ in 0..50 {
        if !alive() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("the CLI's child {} outlived it", pid.trim());
}

fn page_paint(
    agents: &Agents,
    selected: usize,
) -> (Vec<skelly_render::ProseLabel>, Vec<(PxRect, Target)>) {
    let theme = Theme::resolve("ossein-dark");
    let mut measure = TextMeasure::new(1.0);
    let (mut quads, mut labels) = (Vec::new(), Vec::new());
    let mut out = Out {
        quads: &mut quads,
        labels: &mut labels,
        targets: Vec::new(),
    };
    let page = Page {
        agents,
        selected,
        hover: None,
    };
    let area = Area {
        x: 220.0,
        right: 940.0,
        top: 100.0,
        bottom: 700.0,
    };
    page.build(&area, 1.0, &theme, &mut measure, &mut out);
    let targets = out.targets;
    (labels, targets)
}

#[test]
fn the_page_offers_connect_and_fix_and_shows_the_focused_command() {
    let mut agents = Agents::new(SKELLY.into());
    agents.apply(Report::State(Agent::Pi, State::Connected));
    agents.apply(Report::State(
        Agent::Claude,
        State::Broken("ENOENT /old/skelly".into()),
    ));
    agents.apply(Report::State(Agent::Codex, State::Off));
    let (labels, targets) = page_paint(&agents, 1);
    let texts: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
    for text in [
        "Pi",
        "Claude Code",
        "Codex",
        "Connected",
        "Not connected",
        "Fix",
        "Connect",
    ] {
        assert!(texts.contains(&text), "{text} in {texts:?}");
    }
    assert!(
        !targets.iter().any(|(_, t)| *t == Target::Connect(0)),
        "a connected agent has no button"
    );
    for target in [
        Target::Connect(1),
        Target::Connect(2),
        Target::Copy,
        Target::Check,
    ] {
        assert!(targets.iter().any(|(_, t)| *t == target), "{target:?}");
    }
    // The focused (broken) agent's command is in the well, wrapped, with why it is broken.
    let command = texts
        .iter()
        .skip_while(|t| !t.starts_with("Command"))
        .skip(2)
        .take_while(|t| !t.starts_with("ENOENT"))
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(command, line(Agent::Claude, &State::Broken(String::new())));
    assert!(texts.contains(&"ENOENT /old/skelly"));
}

#[test]
fn a_missing_agent_says_how_to_get_it_and_has_nothing_to_click_but_copy() {
    let mut agents = Agents::new(SKELLY.into());
    agents.apply(Report::State(Agent::Codex, State::Missing));
    let (labels, targets) = page_paint(&agents, 2);
    assert!(labels
        .iter()
        .any(|l| l.text == "Not installed \u{b7} no codex on your PATH"));
    assert!(labels
        .iter()
        .any(|l| l.text == "Install Codex first, then check again."));
    assert!(!targets.iter().any(|(_, t)| matches!(t, Target::Connect(_))));
    assert!(targets.iter().any(|(_, t)| *t == Target::Copy));
}

#[test]
fn check_again_is_not_offered_while_a_check_runs() {
    let mut agents = Agents::new(SKELLY.into());
    assert!(agents.begin_check());
    let (labels, targets) = page_paint(&agents, 0);
    assert!(labels.iter().any(|l| l.text == "Checking\u{2026}"));
    assert!(!targets.iter().any(|(_, t)| *t == Target::Check));
}
