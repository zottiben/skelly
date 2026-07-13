//! Hot-path benchmarks for the per-frame CPU render builders (playbook §4 perf budgets).
//!
//! Every frame the renderer turns a grid snapshot into GPU instance data: `grid_quads`
//! (background / underline / selection / cursor rectangles) and `text_runs` (coalescing
//! cells into shaping runs). Both are pure, GPU-free, and O(cells), so they bench cleanly
//! via the crate's `bench_support` seam without a window. Run with `cargo bench -p
//! skelly-render`.
//!
//! Tracked budgets (soft targets, not a CI gate yet - see ROADMAP): an 80x24 grid should
//! build in a handful of microseconds; a regression past ~-20% on this machine warrants a
//! look. The worst realistic case is a full grid of distinct-colored cells (no run
//! coalescing, a bg fill on every cell), so we bench that alongside a plain grid.
#![allow(
    missing_docs,
    reason = "benches are not public API; criterion_group! generates undocumented items"
)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use skelly_render::bench_support::{grid_quads_len, text_runs_len};
use skelly_render::{GridCell, Srgb, Theme};

const COLS: usize = 80;
const ROWS: usize = 24;
const CELL_W: f32 = 9.0;
const CELL_H: f32 = 18.0;

/// A plain grid: default foreground, no background fill, no attributes - the common case
/// where whole rows coalesce into a single run and no bg quads are emitted.
fn plain_grid(fg: Srgb) -> Vec<Vec<GridCell>> {
    let cell = GridCell {
        c: 'x',
        fg,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
    };
    vec![vec![cell; COLS]; ROWS]
}

/// The adversarial grid: every cell a distinct foreground color with a background fill and
/// an underline - so `text_runs` coalesces nothing and `grid_quads` emits a quad per cell.
fn busy_grid() -> Vec<Vec<GridCell>> {
    let mut rows = Vec::with_capacity(ROWS);
    // A `u8` counter that wraps naturally, so cells vary in color with no lossy casts.
    let mut n: u8 = 0;
    for _ in 0..ROWS {
        let mut row = Vec::with_capacity(COLS);
        for _ in 0..COLS {
            n = n.wrapping_add(37);
            let fg = Srgb {
                r: n,
                g: n.wrapping_mul(3),
                b: n.wrapping_mul(7),
            };
            row.push(GridCell {
                c: '@',
                fg,
                bg: Some(fg),
                bold: n.is_multiple_of(2),
                italic: false,
                underline: true,
            });
        }
        rows.push(row);
    }
    rows
}

fn bench_grid_quads(c: &mut Criterion) {
    let theme = Theme::resolve("ossein-dark");
    let plain = plain_grid(theme.fg_primary);
    let busy = busy_grid();
    let mut group = c.benchmark_group("grid_quads");
    for (name, grid) in [("plain", &plain), ("busy", &busy)] {
        group.bench_with_input(BenchmarkId::from_parameter(name), grid, |b, grid| {
            b.iter(|| grid_quads_len(CELL_W, CELL_H, black_box(grid), theme.accent));
        });
    }
    group.finish();
}

fn bench_text_runs(c: &mut Criterion) {
    let theme = Theme::resolve("ossein-dark");
    let plain = plain_grid(theme.fg_primary);
    let busy = busy_grid();
    let mut group = c.benchmark_group("text_runs");
    for (name, grid) in [("plain", &plain), ("busy", &busy)] {
        group.bench_with_input(BenchmarkId::from_parameter(name), grid, |b, grid| {
            b.iter(|| text_runs_len(black_box(grid)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_grid_quads, bench_text_runs);
criterion_main!(benches);
