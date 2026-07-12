//! Hot-path benchmarks for the VT/ANSI parser (playbook §4 perf budgets).
//!
//! Untrusted shell bytes flow straight into `Parser::advance` - it is the throughput-
//! critical path (and, per §7, the threat surface the fuzzer already hardened). These
//! benches feed representative streams (plain text, SGR-heavy color, a full-screen TUI
//! repaint) through a fresh 80x24 parser and report bytes/sec, plus the per-frame grid
//! read (`cells`). Run with `cargo bench -p skelly-term`.
//!
//! Tracked budgets (a regression past ~-20% on this machine warrants investigation; these
//! are soft targets, not a CI gate yet - see ROADMAP): plain text and SGR should each
//! clear tens of MiB/s, and a `cells()` read of a full screen should stay well under
//! ~50us. `alacritty_terminal` does the actual VT work; these guard our wrapping + reads.
#![allow(
    missing_docs,
    reason = "benches are not public API; criterion_group! generates undocumented items"
)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use skelly_term::Parser;

/// Target grid the streams are sized against - a conventional terminal.
const COLS: u16 = 80;
const ROWS: u16 = 24;
/// Roughly how many bytes each stream should contain, so a single `advance` is a few
/// hundred microseconds and the throughput number is stable.
const TARGET_BYTES: usize = 64 * 1024;

/// Plain ASCII lines that scroll the grid - the `cat a-file` / build-log case.
fn plain_text() -> Vec<u8> {
    let line = "The quick brown fox jumps over the lazy dog 0123456789\r\n";
    repeat_to(line.as_bytes(), TARGET_BYTES)
}

/// Color-heavy output with SGR sequences on nearly every word - the `ls --color`,
/// syntax-highlighted, or colored-log-output case.
fn sgr_heavy() -> Vec<u8> {
    let unit = concat!(
        "\x1b[31mERROR\x1b[0m \x1b[32mok\x1b[0m \x1b[1;34minfo\x1b[0m ",
        "\x1b[38;5;208m256\x1b[0m \x1b[38;2;120;200;80mtruecolor\x1b[0m ",
        "\x1b[3mitalic\x1b[0m \x1b[4munder\x1b[0m\r\n",
    );
    repeat_to(unit.as_bytes(), TARGET_BYTES)
}

/// A full-screen TUI repaint frame (home, per-row clear-and-write, a cursor move) repeated
/// - the vim / htop / lazygit redraw case, which exercises CUP + EL + SGR churn.
fn tui_repaint() -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(b"\x1b[H"); // cursor home
    for row in 1..=ROWS {
        // Move to the row, clear it, draw a decorated line.
        frame.extend_from_slice(format!("\x1b[{row};1H\x1b[2K").as_bytes());
        frame.extend_from_slice(b"\x1b[7m \x1b[0m \x1b[36mstatus\x1b[0m: ");
        frame.extend_from_slice(b"redraw line with some content to fill the width");
    }
    repeat_to(&frame, TARGET_BYTES)
}

/// Repeat `unit` until the buffer is at least `target` bytes.
fn repeat_to(unit: &[u8], target: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target + unit.len());
    while out.len() < target {
        out.extend_from_slice(unit);
    }
    out
}

fn bench_advance(c: &mut Criterion) {
    let streams = [
        ("plain", plain_text()),
        ("sgr", sgr_heavy()),
        ("tui", tui_repaint()),
    ];
    let mut group = c.benchmark_group("parser_advance");
    for (name, data) in &streams {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), data, |b, data| {
            // A fresh parser per iteration (setup untimed) so scrollback growth from a prior
            // iteration never skews the measurement.
            b.iter_batched_ref(
                || Parser::new(COLS, ROWS),
                |parser| parser.advance(black_box(data)),
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_cells_read(c: &mut Criterion) {
    // A populated screen, then measure the per-frame grid snapshot the renderer reads.
    let mut parser = Parser::new(COLS, ROWS);
    parser.advance(&sgr_heavy());
    c.bench_function("parser_cells_read", |b| {
        b.iter(|| black_box(parser.cells()));
    });
}

criterion_group!(benches, bench_advance, bench_cells_read);
criterion_main!(benches);
