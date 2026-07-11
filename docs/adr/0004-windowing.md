# 0004. Windowing / input layer

- Status: Accepted
- Date: 2026-07-11
- Deciders: maintainers
- Related: design/README.md (open foundation decision + "single vs multi window"); ADR-0003

## Context

The `skelly` binary needs a window, an event loop, and keyboard/mouse/IME input on
macOS and Linux (Wayland + X11). The design is keyboard-first with heavy chord/leader
use, so **input fidelity - especially IME and dead keys - matters**. Ghostty went
fully native per platform (AppKit / GTK) precisely because native input and feel are
hard to fake; the cost is a separate UI shell per platform.

Options: `winit` (the de-facto Rust cross-platform window/input layer, used by
Alacritty, pairs directly with `wgpu`, MIT) versus direct native integration
(AppKit via `objc2` on macOS, GTK on Linux) as Ghostty does.

## Decision

We will use **`winit`** for the window and event loop, paired with `wgpu`
(ADR-0003), behind a thin Skelly-owned platform trait. This keeps one Rust codebase
for v0.1 and matches the charter's "native Rust" intent. Where `winit` is weak -
notably IME - we integrate more deeply with the platform as specific slices demand,
rather than adopting two full native UI shells up front. Single OS window for v0.1
(resolving the design's windowing open question); multi-window is a later, additive
decision.

## Consequences

- One codebase for both platforms; fast path to the M1 walking skeleton, and the
  natural `winit` + `wgpu` pairing keeps the render surface simple.
- IME and some native niceties are winit's known weak spots; the charter's
  pixel-perfection standard means we will invest in these deliberately, and the
  platform trait leaves room to drop to AppKit/GTK for specific gaps without a
  rewrite.
- Choosing single-window for v0.1 simplifies the session/tab and focus model;
  revisit for multi-window in a later ADR if demand appears.

## Alternatives considered

- **Full native AppKit + GTK** (Ghostty's path) - best-in-class native feel and
  IME, but two UI shells to build and maintain; disproportionate for v0.1 and a
  small team. The platform trait keeps a partial move open where it pays off.
- **A GUI toolkit** (GTK-rs / Tauri / egui) - either pulls in a heavy stack or
  fights our custom GPU cell grid; rejected (see ADR-0003).
