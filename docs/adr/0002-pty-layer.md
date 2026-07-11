# 0002. PTY / shell I/O layer

- Status: Accepted
- Date: 2026-07-11
- Deciders: maintainers
- Related: design/README.md (open foundation decision); ADR-0001

## Context

`skelly-term` must spawn the user's shell in a pseudo-terminal, stream bytes both
ways, propagate resize (`SIGWINCH` / `TIOCSWINSZ`), and manage the child process
lifecycle across macOS and Linux. This is fiddly, platform-specific, `unsafe`-laden
code that is easy to get subtly wrong (leaked FDs, resize races, zombie processes).

Options: `portable-pty` (WezTerm's cross-platform PTY crate, MIT, runtime-selectable
backends, widely used); or hand-rolling on `nix`/`rustix` directly.

## Decision

We will use **`portable-pty`** for PTY creation, resize, and child lifecycle, behind
the same `skelly-term` trait boundary as the terminal core (ADR-0001), so the PTY
backend is swappable and `skelly-term` stays testable with a fake PTY.

## Consequences

- We get correct, cross-platform PTY handling without maintaining `unsafe` FD code
  ourselves - directly serving the charter's robustness and maintainability values.
- A dependency on a WezTerm crate; MIT-licensed, `cargo deny`-clean (confirm at
  adoption). The trait boundary is the reversal path.
- Tests inject a fake PTY, so `skelly-term` unit/conformance tests run headless with
  no real shell.

## Alternatives considered

- **Hand-roll on `nix`/`rustix`** - full control and one fewer dependency, but
  re-implements well-trodden, `unsafe`, platform-divergent code for no product
  benefit at v0.1. Reconsider only if `portable-pty` blocks a needed capability.
