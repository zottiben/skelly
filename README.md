# skelly

A barebones, keyboard-driven terminal emulator built natively in Rust. Minimal by
design, for vim / neovim / LazyVim development: multi-pane splits, per-repo git
diff, and a rewindable session timeline.

## Install

**macOS & Linux:**

```sh
curl -fsSL https://zottiben.github.io/skelly/install.sh | sh
```

On macOS this installs a universal (Apple Silicon + Intel) `Skelly.app` into
Applications plus a `skelly` command on your PATH; on Linux it installs the `skelly`
binary and a desktop entry. You can also grab a build from the
[releases page](https://github.com/zottiben/skelly/releases/latest).

### Update

Once Skelly is installed, upgrade in place - no need to remember the install
command:

```sh
skelly update           # install the latest release
skelly update --check   # only report whether a newer release exists
```

It re-runs the same install script, so an update and a fresh install do exactly the
same thing. `--force` reinstalls the current version, and `--version v0.1.8` installs
a specific release. Restart any open window afterwards to pick up the new build.

<details>
<summary>Build from source</summary>

Requires the pinned toolchain (installed automatically by `rustup` from
[`rust-toolchain.toml`](rust-toolchain.toml)).

```sh
cargo build --release -p skelly    # binary at target/release/skelly
```
</details>

## Quickstart

Requires the pinned toolchain (installed automatically by `rustup` from
[`rust-toolchain.toml`](rust-toolchain.toml)).

```sh
cargo run -p skelly      # build + run the current harness
cargo test --workspace   # run the test suite
```

Right now the binary loads `~/.config/skelly/config.toml` (or spec defaults when
there is no file) and reports the resolved settings.

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) for the workflow and quality gates, and
[`AGENTS.md`](AGENTS.md) for the project's hard rules. Architecture decisions live
in [`docs/adr/`](docs/adr/).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
