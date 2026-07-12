//! End-to-end terminal-core test: spawn a real shell, send a command, and assert
//! its *executed* output lands in the grid. This exercises the full round-trip -
//! input -> PTY -> shell -> PTY -> parser -> grid - the M1c walking-skeleton
//! contract. Polling with a timeout is intrinsic to an e2e test over an async
//! shell; it is not a unit test.

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_term::{CellAttrs, CellColor, Terminal};

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
