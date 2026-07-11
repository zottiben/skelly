//! Property-based robustness: untrusted bytes must never panic the parser or our
//! grid readers, and the grid never collapses to the reflow-pathological width.
//!
//! A terminal feeds attacker-controllable PTY output straight into the VT parser
//! (playbook §7 threat model), and our [`Parser`] readers do index arithmetic
//! (`i32::try_from`, `Line`/`Column` indexing, `display_offset` math) over whatever
//! grid state results. This fuzzes that path in CI on the stable toolchain: feed
//! arbitrary byte chunks, then exercise every read and mutation, asserting only that
//! nothing panics and the cursor stays inside the grid. A coverage-guided
//! `cargo-fuzz` target reaches deeper; this is the always-on regression guard.
//!
//! `minimum_grid_width_is_enforced` locks a fix this fuzzer found: resizing to a
//! single column drove `alacritty_terminal`'s reflow into a multi-second loop, so
//! the core now clamps the grid to a usable floor (see `skelly-term`'s `MIN_COLS`).

use proptest::prelude::*;
use skelly_term::Parser;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Arbitrary bytes, fed in arbitrary chunks, never panic `advance` or the grid
    /// readers, and never leave the cursor outside the grid.
    #[test]
    fn arbitrary_bytes_never_panic(
        chunks in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 0..64),
            0..16,
        ),
    ) {
        let (cols, rows) = (24_u16, 8_u16);
        let mut p = Parser::new(cols, rows);

        for chunk in &chunks {
            p.advance(chunk);
            // Every reader must hold up against adversarial grid state.
            let _ = p.snapshot();
            let _ = p.cells();
            let (col, row) = p.cursor();
            prop_assert!(
                col <= usize::from(cols) && row < usize::from(rows),
                "cursor ({col}, {row}) escaped the {cols}x{rows} grid",
            );
        }

        // Scrolling far past the history bounds must clamp, not panic.
        p.scroll_lines(10_000);
        let _ = p.cells();
        p.scroll_lines(-10_000);
        let _ = p.cells();

        // Resizing over the resulting grid, through the realistic range a window can
        // produce, must stay sound: the readers still return without panicking.
        for (w, h) in [(2_u16, 1_u16), (8, 4), (200, 60), (80, 24)] {
            p.resize(w, h);
            let _ = p.cells();
        }
    }
}

/// The core clamps the grid to at least [`MIN_COLS`](../src/lib.rs) columns, whether
/// requested at construction or on resize. A single-column grid drives
/// `alacritty_terminal`'s reflow into a pathological multi-second loop, so this
/// invariant is what keeps a hostile stream from wedging a dragged-narrow window.
#[test]
fn minimum_grid_width_is_enforced() {
    // Requested at construction.
    let narrow = Parser::new(1, 4);
    assert!(
        narrow.cells()[0].len() >= 2,
        "grid width should be clamped to at least 2 columns at construction",
    );

    // Requested on resize, even from a healthy grid.
    let mut p = Parser::new(24, 8);
    p.advance(b"some content that will be reflowed on resize");
    p.resize(1, 4);
    assert!(
        p.cells()[0].len() >= 2,
        "grid width should be clamped to at least 2 columns on resize",
    );
}
