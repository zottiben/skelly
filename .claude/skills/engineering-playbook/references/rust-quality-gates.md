# Rust quality gates - concrete configs

The enforced tooling for the Skelly workspace. These are the actual files to
commit in M0 (day-1 setup). Keep them in sync with the CI workflow so `local ==
CI`. Values (MSRV, versions) are the current intent - update deliberately, via a
commit, never silently.

## Workspace root `Cargo.toml`

Shared dependency versions and lint policy live once, at the root, so crates
inherit them. Resolver 2 is required for a modern workspace.

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
edition = "2021"
rust-version = "1.XX"          # MSRV - the oldest supported stable; bump deliberately
license = "…"                   # decide in an ADR; must be deny-compatible
repository = "…"

[workspace.dependencies]
# Pin shared deps here; crates reference `foo.workspace = true`.
thiserror = "…"
anyhow = "…"
tracing = "…"

[workspace.lints.rust]
unsafe_code = "forbid"          # opt back in per-crate only where FFI/GPU needs it
missing_docs = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
# ... allow back specific pedantic lints that fight idiomatic code, with a reason.
```

Each crate opts into the shared policy with `[lints] workspace = true`.

## `rust-toolchain.toml`

Pin the toolchain so every machine and CILane compile identically.

```toml
[toolchain]
channel = "1.XX.Y"              # explicit stable version, not "stable"
components = ["rustfmt", "clippy"]
```

## `rustfmt.toml`

Formatting is enforced (`cargo fmt --all --check` in CI, auto-on-save via the repo
hook). Keep config minimal; default rustfmt is fine for most of it.

```toml
edition = "2021"
# Add only deliberate, agreed deviations from default rustfmt.
```

## `deny.toml` (cargo-deny)

Supply-chain gate: licenses, security advisories, banned crates, source trust.

```toml
[advisories]
version = 2
yanked = "deny"
# ignore = ["RUSTSEC-XXXX-YYYY"]  # only with a comment + tracking issue

[licenses]
version = 2
allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-3-Clause", "ISC", "Unicode-3.0"]
# Confirm every foundation crate's license fits before adopting it (ADR).

[bans]
multiple-versions = "warn"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

## Clippy policy

- CI runs `cargo clippy --all-targets --all-features -- -D warnings`. Warnings are
  errors.
- Fix the root cause. `#[allow(...)]` is permitted only narrowly-scoped and with a
  `// reason:` comment; never a crate-wide blanket allow to duck a lint.

## GitHub Actions CI (`.github/workflows/ci.yml`) - shape

Gates on every PR and on `main`; set them as required status checks. Cross-OS
matrix because Skelly targets macOS + Linux.

```yaml
name: CI
on:
  pull_request:
  push: { branches: [main] }
concurrency: { group: "ci-${{ github.ref }}", cancel-in-progress: true }
jobs:
  gate:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable      # honors rust-toolchain.toml
      - uses: Swatinem/rust-cache@v2             # cache target/ + registry
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --all --all-features
      - run: cargo doc --no-deps --all-features
  supply-chain:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v2
```

Notes:
- Linux GPU/windowing tests need system libs (e.g. Wayland/X11, `libxkbcommon`)
  installed in the job; headless GPU may need a software adapter (llvmpipe/lavapipe)
  or must be gated out and documented, never faked green.
- `cargo test` on `skelly-term` must be runnable headless so parser/grid
  conformance (vttest/esctest harness) and fuzzing run in CI without a display.

## The gate summary (what "green" means)

`cargo fmt --check` · `cargo clippy -D warnings` · `cargo test --all` ·
`cargo doc --no-deps` · `cargo deny check` - all pass, on macOS + Linux, before
merge. `/pre-pr` runs these locally first.
