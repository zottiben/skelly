# Changelog

All notable changes are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/). This file is generated from
Conventional Commits - do not hand-edit it.

## [Unreleased]

### Added

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
