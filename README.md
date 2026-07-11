# skelly

A barebones, keyboard-driven terminal emulator built natively in Rust. Minimal by
design, for vim / neovim / LazyVim development: multi-pane splits, per-repo git
diff, and a rewindable session timeline. Ghostty-grade minimalism with a Zen-style
tab sidebar. Targets macOS and Linux.

> **Status: greenfield, design-driven.** The M0 foundation is in place (workspace,
> quality gates, CI, docs) and the configuration layer is implemented. The GPU
> window, PTY, and cell renderer land with the M1 walking skeleton. See
> [`ROADMAP.md`](ROADMAP.md).

## Quickstart

Requires the pinned toolchain (installed automatically by `rustup` from
[`rust-toolchain.toml`](rust-toolchain.toml)).

```sh
cargo run -p skelly      # build + run the current harness
cargo test --workspace   # run the test suite
```

Right now the binary loads `~/.config/skelly/config.toml` (or spec defaults when
there is no file) and reports the resolved settings - proving the config slice
end-to-end. Configuration is the single source of truth: every setting maps 1:1 to
a `config.toml` key.

## Layout

Cargo workspace, one crate per concern, dependencies flowing one way (nothing
depends on the binary):

- [`crates/skelly`](crates/skelly) - the binary: window, sidebar, pane tree,
  command palette, wiring.
- [`crates/skelly-render`](crates/skelly-render) - GPU cell-grid renderer, fonts,
  semantic theme-token resolution.
- [`crates/skelly-term`](crates/skelly-term) - PTY, shell I/O, ANSI/VT parsing,
  grid, scrollback.
- [`crates/skelly-session`](crates/skelly-session) - session timeline,
  non-destructive rewind (shadow worktree), git diff.
- [`crates/skelly-config`](crates/skelly-config) - `config.toml` load / validate /
  schema (the source of truth).

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the crate map and data flow, and
[`design/`](design/) for the binding design spec.

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) for the workflow and quality gates, and
[`AGENTS.md`](AGENTS.md) for the project's hard rules. Architecture decisions live
in [`docs/adr/`](docs/adr/).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
