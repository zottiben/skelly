# 0003. GPU renderer + font stack

- Status: Accepted
- Date: 2026-07-11
- Deciders: maintainers
- Related: design/README.md (open foundation decision); ADR-0004

## Context

`skelly-render` must draw a terminal cell grid fast: shape text into glyphs (with
Nerd Font fallback and ligatures), rasterize once, cache in a GPU texture atlas, and
draw batched quads - the standard high-throughput approach Ghostty uses (Metal on
macOS, OpenGL on Linux, HarfBuzz shaping, atlas caching). Skelly targets macOS +
Linux and wants one rendering codebase, not two native backends. The design demands
pixel-accurate tokens, ligatures, and first-class Nerd Font coverage.

The modern Rust equivalent of Ghostty's pipeline is `wgpu` (one API over Metal +
Vulkan + DX/GL) plus `cosmic-text` (shaping via a HarfBuzz-family shaper +
rasterization via `swash` + `fontdb` discovery + `etagere` shelf-atlas), with
`glyphon` gluing cosmic-text to wgpu. All MIT/Apache-licensed.

## Decision

We will render with **`wgpu` + `cosmic-text` (+ `glyphon`/`etagere`)** on a
dedicated render thread that draws from a snapshot of terminal state, behind a
Skelly-owned renderer trait. Frame pacing is capped to the display refresh with
dirty-cell tracking (full render only when state changed; animation-only frames for
cursor blink). Semantic theme tokens resolve here, in a type distinct from the ANSI
palette (Hard rule 2).

## Consequences

- One rendering backend targets both platforms (Metal + Vulkan via wgpu), instead
  of maintaining two - the maintainability win over Ghostty's per-platform backends.
- `cosmic-text` gives shaping + fallback + atlas in one crate, architecturally the
  same shape as Ghostty's font system, so Nerd Fonts and ligatures are first-class.
- We own frame pacing and damage tracking; getting these wrong wastes power and
  hurts latency, so they are explicit from the first renderer slice.
- We accept a heavier dependency graph (wgpu pulls in GPU stack crates); justified
  by the correctness/perf it provides. `unsafe` is confined to this crate and the
  windowing layer, each block justified with `// SAFETY:`.

## Alternatives considered

- **Native Metal + OpenGL** (Ghostty's path) - maximal per-platform fidelity, but
  two backends to write and maintain for a small team; deferred.
- **CPU rasterization** (`fontdue`/`tiny-skia`) - simpler, but cannot sustain
  smooth scroll/throughput for a terminal; rejected for the cell grid.
- **A higher-level GUI toolkit** (egui/iced/GTK) - fights us on a custom cell grid
  and pixel control; rejected. Chrome may still use lightweight custom widgets.
