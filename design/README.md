# design/ - source of truth for skelly

This directory is the **binding design spec**. Build the app against it. When a
decision isn't covered by the guide, decide it, then record the decision here so
the next contributor (human or agent) inherits it.

## The guide

- **`Skelly Design Guide.dc.html`** - the full v0.1 spec: foundations, brand,
  color/token system, ANSI palette, type, scale/motion, icons, layout/window
  anatomy, the component library with states, every core screen mockup,
  keybindings, behavior/edge states, and the `config.toml` schema. Open it in a
  browser to view. `support.js`, `.thumbnail`, `screenshots/`, and `uploads/` are
  assets for that export.

**Generated file - do not hand-edit.** The `.dc.html` is a Claude Design export.
To change the design, edit it in the design tool and re-export; don't patch the
HTML by hand. (See AGENTS.md Hard rule 6.)

## Open decisions

The guide flags these as "Confirm first" - genuine product/architecture calls to
settle before or as the relevant code lands. Resolve each, then replace its line
with the decision + date.

- [ ] **Timeline AI-actions contract** - how the session timeline receives AI
  actions. Needs an explicit integration hook/contract, not shell heuristics.
- [x] **Windowing** - single OS window for v0.1 via `winit`; multi-window is a
  later additive decision. _Resolved 2026-07-11 (ADR-0004)._
- [ ] **Rewind + edit** - when viewing a past state, fork a branch on edit, or
  block edits while detached?
- [ ] **Persist scope** - restore layout only, or attempt to re-run processes?
- [ ] **Scrollback search scope** - active pane only, or all panes?

Deferred stack/foundation choices (from init; keep TBD until crates are picked):

- [x] **GPU cell renderer** - `wgpu` + `cosmic-text`/`glyphon`. _Resolved
  2026-07-11 (ADR-0003)._
- [x] **PTY / shell I/O** - `portable-pty`. _Resolved 2026-07-11 (ADR-0002)._
- [x] **Font shaping + Nerd Font fallback** - `cosmic-text` (shaping + fallback +
  atlas). _Resolved 2026-07-11 (ADR-0003)._
- [x] **Terminal core** - `alacritty_terminal` behind a Skelly-owned trait.
  _Resolved 2026-07-11 (ADR-0001)._

## Decision log

Record settled decisions here, newest first: `YYYY-MM-DD - <decision> (was: <the
open question>)`.

- 2026-07-11 - Architecture decisions are recorded as ADRs in `docs/adr/`
  (ADR-0000). Foundation-stack choices are proposed in ADR-0001..0004, pending
  maintainer ratification before the M1 walking skeleton lands them.
