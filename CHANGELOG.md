# Changelog

All notable changes are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/). This file is generated from
Conventional Commits - do not hand-edit it.

## [Unreleased]

### Added

- M2a (core terminal, colors): render per-cell foreground colors. `skelly-term`
  exposes a colored cell snapshot (`cells()` with a palette-independent `CellColor`);
  `skelly-render` gains an `AnsiPalette` (Ossein Dark/Light 16-color + 256-color
  resolution, kept separate from UI tokens per Hard rule 2) and a colored
  `set_cells`/`capture_cells_rgba`. Terminal text now also uses a monospace face so
  columns align.
- M1c (walking skeleton complete): spawn the login shell in a PTY (`portable-pty`)
  and parse its output with `alacritty_terminal` into a grid (`skelly-term`), paint
  the live grid on the GPU, and forward keystrokes to the shell. The reader thread
  wakes the event loop via an `EventLoopProxy` so repaints happen only on new
  output. Verified by an e2e test (a real shell executes a command, output reaches
  the grid) and a `session_capture` example that renders a real shell session to a
  PNG. Promoted the headless offscreen render into a reusable `capture_rgba`.
- M1b (walking skeleton, text): render shaped text via `glyphon`/`cosmic-text` in
  the configured cell font and the `fg.primary` token. Extracted a reusable
  `TextLayer` (clear + text into any target) shared by the windowed renderer and a
  headless `capture` example that writes a PNG (visual/golden verification with no
  window or screen-recording permission).
- M1a (walking skeleton, first slice): a native window (`winit`) with a `wgpu` GPU
  surface that clears to the resolved Ossein theme background each frame, quitting
  cleanly on window-close, Escape, or `q`. `skelly-render` gains the `Renderer` and
  a minimal semantic `Theme` token resolver (no raw hex in the binary).
- M0 foundation: Cargo workspace with the five design crates (`skelly`,
  `skelly-render`, `skelly-term`, `skelly-session`, `skelly-config`).
- Quality gates: pinned toolchain, rustfmt, clippy (`-D warnings`, pedantic),
  `cargo-deny`, and a CI matrix (macOS + Linux) with an MSRV leg and a daily
  security audit.
- `skelly-config`: the `config.toml` schema, spec-accurate defaults, loading,
  validation, and round-trip serialization (the single source of truth).
- Project docs: `README`, `CONTRIBUTING`, `ARCHITECTURE`, `SECURITY`, and the ADR
  log.

[Unreleased]: https://github.com/zottiben/skelly/commits/main
