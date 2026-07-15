//! End-to-end terminal-core test: spawn a real shell, send a command, and assert
//! its *executed* output lands in the grid. This exercises the full round-trip -
//! input -> PTY -> shell -> PTY -> parser -> grid - the M1c walking-skeleton
//! contract. Polling with a timeout is intrinsic to an e2e test over an async
//! shell; it is not a unit test.

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_term::{CellAttrs, CellColor, Terminal};

/// Block until the shell has drawn something (its prompt) into the grid.
fn wait_for_prompt(term: &Terminal) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while term.snapshot().iter().all(String::is_empty) {
        assert!(Instant::now() < deadline, "shell never produced a prompt");
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn device_query_reply_is_sent_back_to_the_shell() {
    // The parser answers device queries (here Primary Device Attributes, `ESC [ c`) by asking
    // the terminal to write the reply back to the PTY. That reply used to be dropped, so
    // programs that probe the terminal - Neovim's Kitty keyboard-protocol handshake among them
    // - waited on it forever. Emit the query, then run `cat -v` (which renders control bytes
    // visibly); the injected reply is fed back as input and shows up as `^[[?...c`.
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");
    wait_for_prompt(&term);

    term.write(b"printf '\\033[c'; cat -v\n");

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if term.snapshot().iter().any(|line| line.contains("^[[?")) {
            return; // the device-attributes reply reached the shell
        }
        assert!(
            Instant::now() < deadline,
            "device-attributes reply never reached the shell; grid was:\n{}",
            term.snapshot().join("\n")
        );
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn kitty_keyboard_protocol_mode_is_tracked() {
    // A program turns on the Kitty keyboard protocol by pushing flags (`ESC [ > flags u`), which
    // is what lets it distinguish e.g. Shift+Enter from Enter. The binary reads `keyboard_mode()`
    // to choose the key encoding, so it must reflect what the program enabled. Flag 1 is
    // "disambiguate escape codes", the level Neovim requests.
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");
    wait_for_prompt(&term);
    assert!(
        !term.keyboard_mode().disambiguate,
        "a fresh shell is in legacy keyboard mode"
    );

    term.write(b"printf '\\033[>1u'\n");

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if term.keyboard_mode().disambiguate {
            return; // the enabled protocol is now visible to the key encoder
        }
        assert!(
            Instant::now() < deadline,
            "keyboard mode never reflected the enabled Kitty protocol"
        );
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn shell_is_spawned_as_a_login_shell() {
    // A login shell is invoked with argv0 prefixed by `-` (e.g. `-zsh`), which is
    // what makes it source the user's profile (`~/.zprofile`, and on macOS
    // `/etc/zprofile` -> `path_helper`) and inherit a full `PATH`. Without it a
    // GUI-launched Skelly can't find `nvim`, `brew`, etc. `$0` echoes that argv0,
    // so a leading `-` proves the login spawn; the old non-login spawn printed the
    // bare shell path (e.g. `/bin/zsh`), which this asserts against.
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");
    let deadline = Instant::now() + Duration::from_secs(15);
    while term.snapshot().iter().all(String::is_empty) {
        assert!(Instant::now() < deadline, "shell never produced a prompt");
        sleep(Duration::from_millis(50));
    }

    // The executed output reads `SKELLY_ARGV0=-<shell>`; the echoed input line keeps
    // `$0` literal, so matching on the `-` right after `=` finds only the result.
    term.write(b"echo \"SKELLY_ARGV0=$0\"\n");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if term
            .snapshot()
            .iter()
            .any(|line| line.contains("SKELLY_ARGV0=-"))
        {
            return; // argv0 begins with `-`: a login shell
        }
        assert!(
            Instant::now() < deadline,
            "shell was not a login shell (no `-`-prefixed argv0); grid was:\n{}",
            term.snapshot().join("\n")
        );
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn shell_pid_goes_none_after_exit() {
    // A live shell exposes its pid (for reading the pane's cwd); once it exits, the pid is stale
    // and the OS may reuse it, so `shell_pid` must report `None` rather than a reusable pid.
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");
    assert!(term.shell_pid().is_some(), "a live shell exposes its pid");

    term.write(b"exit 0\n");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if term.exit_status().is_some() {
            assert_eq!(
                term.shell_pid(),
                None,
                "an exited shell reports no pid (it is stale / reusable)"
            );
            return;
        }
        assert!(Instant::now() < deadline, "shell exit was never reported");
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn shell_exit_is_reported() {
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");
    // A live shell has not exited yet.
    assert!(term.exit_status().is_none(), "fresh shell reports no exit");

    // Ask the shell to exit; its status should become available shortly after.
    term.write(b"exit 0\n");

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = term.exit_status() {
            assert_eq!(status.code, 0, "clean exit reports code 0");
            assert!(status.success(), "exit 0 is a success");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "shell exit was never reported after `exit 0`"
        );
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn foreground_job_is_detected() {
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");

    // Wait for the shell to come up (it prints a prompt), then let it settle as the
    // controlling terminal's foreground process group.
    let deadline = Instant::now() + Duration::from_secs(15);
    while term.snapshot().iter().all(String::is_empty) {
        assert!(Instant::now() < deadline, "shell never produced a prompt");
        sleep(Duration::from_millis(50));
    }
    sleep(Duration::from_millis(300));
    // Idle at the prompt: the shell itself owns the foreground group, so there is no job.
    assert_eq!(
        term.foreground_job_pid(),
        None,
        "an idle shell reports no foreground job"
    );

    // Start a long-running foreground job; it takes over the terminal's foreground group.
    term.write(b"sleep 30\n");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(pid) = term.foreground_job_pid() {
            assert!(pid > 0, "a foreground job pid is positive");
            return; // the `sleep` job was detected as the foreground process group
        }
        assert!(
            Instant::now() < deadline,
            "the foreground `sleep` job was never detected"
        );
        sleep(Duration::from_millis(50));
    }
    // `term` drops here, killing the shell and its `sleep` child.
}

#[test]
fn find_locates_text_scrolled_into_history() {
    // A short grid so a burst of output pushes an early line off-screen into scrollback.
    let mut term = Terminal::spawn(80, 8, || {}).expect("spawn shell");
    let deadline = Instant::now() + Duration::from_secs(15);
    while term.snapshot().iter().all(String::is_empty) {
        assert!(Instant::now() < deadline, "shell never produced a prompt");
        sleep(Duration::from_millis(50));
    }
    // Print a unique marker, then enough filler to scroll it out of the 8-row screen.
    term.write(b"printf 'UNIQ_FIND_MARKER_42\\n'; for i in $(seq 1 40); do echo filler$i; done\n");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !term.snapshot().iter().any(|l| l.contains("filler40")) {
        assert!(Instant::now() < deadline, "filler output never completed");
        sleep(Duration::from_millis(50));
    }
    let has_marker = |t: &Terminal| {
        t.snapshot()
            .iter()
            .any(|l| l.contains("UNIQ_FIND_MARKER_42"))
    };
    assert!(
        !has_marker(&term),
        "the marker scrolled off-screen into history"
    );

    // Find pulls the marker back into view; a missing query finds nothing.
    let hit = term
        .find("UNIQ_FIND_MARKER_42", None, false)
        .expect("the marker is found in scrollback");
    assert!(hit.len >= "UNIQ_FIND_MARKER_42".len());
    assert!(has_marker(&term), "the match was scrolled into view");
    assert!(
        term.find("NO_SUCH_TEXT_ZZZ", None, false).is_none(),
        "a query with no match returns None"
    );
}

#[test]
fn shell_executes_a_command_and_output_reaches_the_grid() {
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");

    // The adjacent quotes concatenate only when the shell *executes* printf, so the
    // marker appears in the output but NOT in the echoed input line - proving
    // execution, not just input echo.
    term.write(b"printf 'SKELLY''_M1C_OK\\n'\n");

    let marker = "SKELLY_M1C_OK";
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if term.snapshot().iter().any(|line| line.contains(marker)) {
            return; // success
        }
        assert!(
            Instant::now() < deadline,
            "marker {marker:?} never appeared; grid was:\n{}",
            term.snapshot().join("\n")
        );
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn scrollback_reveals_history() {
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");

    // Print 60 numbered lines (more than the 24-row screen). The split quotes make
    // the marker appear only in the executed output, not the echoed command.
    term.write(b"for i in $(seq 1 60); do printf 'SCROLL''TEST%03d\\n' \"$i\"; done\n");

    let deadline = Instant::now() + Duration::from_secs(20);
    while !term
        .snapshot()
        .iter()
        .any(|line| line.contains("SCROLLTEST060"))
    {
        assert!(Instant::now() < deadline, "output never completed");
        sleep(Duration::from_millis(50));
    }

    // The first line has scrolled off the visible screen into history.
    let visible = term.snapshot().join("\n");
    assert!(
        !visible.contains("SCROLLTEST001"),
        "first line should be off-screen:\n{visible}"
    );

    // Scrolling up into history brings it back into view.
    term.scroll_lines(60);
    let scrolled = term.snapshot().join("\n");
    assert!(
        scrolled.contains("SCROLLTEST001"),
        "scrollback should reveal the first line:\n{scrolled}"
    );
}

#[test]
fn text_attributes_reach_the_grid() {
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");

    // SGR 1 bold, 4 underline, 7 reverse video on the marker. The split quotes make
    // the marker appear only in the *executed* output, not the echoed input.
    term.write(b"printf '\\033[1;4;7mATTR''MARK\\033[0m\\n'\n");

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let cells = term.cells();
        let wanted = CellAttrs::BOLD | CellAttrs::UNDERLINE | CellAttrs::INVERSE;
        let attributed: Option<String> = cells.iter().find_map(|row| {
            let text: String = row
                .iter()
                .filter(|cell| cell.attrs.contains(wanted))
                .map(|cell| cell.c)
                .collect();
            text.contains("ATTRMARK").then_some(text)
        });
        if attributed.is_some() {
            return; // bold + underline + reverse all parsed onto the marker cells
        }
        assert!(
            Instant::now() < deadline,
            "bold/underline/reverse never reached the marker cells"
        );
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn background_color_escape_reaches_the_grid() {
    let mut term = Terminal::spawn(80, 24, || {}).expect("spawn shell");

    // SGR 44 sets a blue background (ANSI index 4). The split quotes ensure the
    // marker only appears in the *executed* output row, not the echoed input.
    term.write(b"printf '\\033[44mBG''MARK\\033[0m\\n'\n");

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let cells = term.cells();
        let output_row = cells.iter().find(|row| {
            row.iter()
                .map(|cell| cell.c)
                .collect::<String>()
                .contains("BGMARK")
        });
        if let Some(row) = output_row {
            if row.iter().any(|cell| cell.bg == CellColor::Indexed(4)) {
                return; // the blue background parsed onto the output cells
            }
        }
        assert!(
            Instant::now() < deadline,
            "blue background never reached the grid"
        );
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn spawn_shell_in_starts_in_the_given_cwd() {
    // A split inherits the source pane's cwd (binary `[panes] split_inherits_cwd`), which the
    // binary implements by spawning the new pane's shell in that directory. Point a shell at a
    // temp dir and assert `pwd` reports it. `canonicalize` resolves macOS's `/var` -> `/private/var`
    // symlink so the comparison holds.
    let dir = std::env::temp_dir().join(format!("skelly-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("make temp dir");
    let want = std::fs::canonicalize(&dir).expect("canonicalize temp dir");

    let mut term =
        Terminal::spawn_shell_in(80, 24, None, Some(&want), || {}).expect("spawn shell in cwd");
    wait_for_prompt(&term);
    term.write(b"pwd\n");

    let want_str = want.to_string_lossy().into_owned();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if term.snapshot().iter().any(|line| line.contains(&want_str)) {
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        assert!(
            Instant::now() < deadline,
            "pwd never reported the spawn cwd {want_str}; grid was:\n{}",
            term.snapshot().join("\n")
        );
        sleep(Duration::from_millis(50));
    }
}
