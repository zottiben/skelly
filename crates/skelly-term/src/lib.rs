//! `skelly-term` - the terminal core.
//!
//! Owns the PTY, shell I/O, the ANSI/VT state machine, the cell grid, scrollback,
//! selection, and resize/reflow. This is Skelly's analog of `libghostty-vt`: it is
//! deliberately free of any window, GPU, or OS-UI dependency so it can be fuzzed
//! and conformance-tested (vttest / esctest) headless, and so the render or PTY
//! backend can be swapped without touching it.
//!
//! Dependency direction: nothing in the workspace depends *up* into the binary;
//! this crate depends only on small, general-purpose libraries.
//!
//! Status: M0 stub. The walking skeleton (M1) lands the PTY + parser behind a
//! Skelly-owned trait; see `docs/adr/0001-*` and `docs/adr/0002-*`.

#![doc(test(attr(deny(warnings))))]

#[cfg(test)]
mod tests {
    // Scaffold smoke test - proves the crate compiles and the test harness runs.
    // Replaced by real parser/grid/conformance tests when M1/M2 land the engine.
    #[test]
    fn crate_builds() {
        assert_eq!(2 + 2, 4);
    }
}
