//! End-to-end terminal-core test: spawn a real shell, send a command, and assert
//! its *executed* output lands in the grid. This exercises the full round-trip -
//! input -> PTY -> shell -> PTY -> parser -> grid - the M1c walking-skeleton
//! contract. Polling with a timeout is intrinsic to an e2e test over an async
//! shell; it is not a unit test.

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_term::Terminal;

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
