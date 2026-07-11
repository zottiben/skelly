//! `skelly-render` - the GPU cell-grid renderer.
//!
//! Rasterizes the terminal grid on the GPU: glyph shaping + fallback, a glyph
//! texture atlas, batched quad draws, cursor, and selection. Also resolves the
//! active theme's semantic tokens (`category.role.state`) to concrete colors - the
//! UI never references raw hex (Hard rule 2), and the UI token set is kept separate
//! from the terminal ANSI palette so any scheme pairs with any theme.
//!
//! Renders off the main thread from a snapshot of terminal state so I/O and input
//! never block frames (the Ghostty model). Depends on `skelly-config` for tokens
//! and metrics; never on the binary.
//!
//! Status: M0 stub. The renderer stack (wgpu + cosmic-text) lands with M1; see
//! `docs/adr/0003-*` and `docs/adr/0004-*`.

#![doc(test(attr(deny(warnings))))]

#[cfg(test)]
mod tests {
    // Scaffold smoke test - proves the crate compiles and the test harness runs.
    // Replaced by golden-image / token-resolution tests when the renderer lands.
    #[test]
    fn crate_builds() {
        assert_eq!(2 + 2, 4);
    }
}
