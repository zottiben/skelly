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
- [ ] **Windowing** - single-window, or multiple OS windows?
- [ ] **Rewind + edit** - when viewing a past state, fork a branch on edit, or
  block edits while detached?
- [ ] **Persist scope** - restore layout only, or attempt to re-run processes?
- [ ] **Scrollback search scope** - active pane only, or all panes?

Deferred stack/foundation choices (from init; keep TBD until crates are picked):

- [ ] **GPU cell renderer** crate/approach (Alacritty/Ghostty-style vs other)
- [ ] **PTY / shell I/O** crate
- [ ] **Font shaping + Nerd Font fallback** stack

## Decision log

Record settled decisions here, newest first: `YYYY-MM-DD - <decision> (was: <the
open question>)`.

_(none yet)_
