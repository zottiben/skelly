//! End-to-end terminal-core test: spawn a real shell, send a command, and assert
//! its *executed* output lands in the grid. This exercises the full round-trip -
//! input -> PTY -> shell -> PTY -> parser -> grid - the M1c walking-skeleton
//! contract. Polling with a timeout is intrinsic to an e2e test over an async
//! shell; it is not a unit test.

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_term::{CellColor, Terminal};

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
