---
name: design-guide
description: Open the Skelly design guide (design/Skelly Design Guide.dc.html) live in a claude-in-chrome browser tab for visual reference and 1:1 parity checks against the running app. Use whenever you need to see the guide rendered, compare a Skelly UI surface to its spec pixel-for-pixel, or the user says "open the design guide" / "load the guide" / "check parity with the guide".
---

# Open the Skelly design guide in a browser tab

The guide (`design/Skelly Design Guide.dc.html`) is the binding product spec (AGENTS Hard
rule 5). It's a generated static mockup that must be *rendered in a browser* to read its
tokens, component states, and layout - and to compare side-by-side with the live app when
chasing 1:1 parity. This skill loads it into a `claude-in-chrome` tab reliably.

## Why it's not one step

- **The browser extension refuses `file://` URLs** ("Can't interact with browser-internal or
  unparseable URLs"). So the guide must be served over local HTTP.
- **Serve the `design/` folder, not the repo root** - the guide loads `support.js` (and
  `uploads/`, `screenshots/`) by *relative* path, so those must sit at the server root.
- The filename has spaces -> URL-encode them as `%20`.
- Do **not** hand-edit the guide to "fix" anything (Hard rule 6) - it's a generated export.

## Procedure

1. **Serve `design/` over HTTP** (idempotent - reuse an already-running server on 8765; from
   the repo root):
   ```bash
   if ! curl -sfI "http://localhost:8765/Skelly%20Design%20Guide.dc.html" >/dev/null 2>&1; then
     (cd design && python3 -m http.server 8765 >/tmp/skelly_designserver.log 2>&1 &)
     sleep 1.5
   fi
   ```
   If port 8765 is held by an unrelated process (the curl check fails *and* python errors with
   "address already in use"), pick another port (8766, ...) and use it below.

2. **Load the browser tools** (they're deferred) in one ToolSearch call:
   ```
   select:mcp__claude-in-chrome__tabs_context_mcp,mcp__claude-in-chrome__navigate,mcp__claude-in-chrome__computer,mcp__claude-in-chrome__read_page,mcp__claude-in-chrome__tabs_create_mcp
   ```

3. **Get tab context first** - `tabs_context_mcp` with `createIfEmpty: true`. Reuse the tab this
   session already opened if there is one; otherwise the blank new tab it returns is fine (don't
   spawn a fresh tab every time).

4. **Navigate** that tab to `http://localhost:8765/Skelly%20Design%20Guide.dc.html`, then
   **screenshot** to confirm it rendered (you should see the vertebra mark + `skelly.` wordmark +
   the left nav `01 Principles` … `13 Handoff notes`).

## Using it for 1:1 parity

- The left nav sections map to the spec areas: `03 Color & themes` (tokens), `05 Typography`,
  `06 Space · radius · motion`, `07 Iconography`, `08 Window anatomy`, `09 Components`,
  `10 Screens`, `11 Keybindings`, `12 Flows & states`. Click a nav item (or scroll) to jump.
- To compare a surface: screenshot the guide section, then render the matching Skelly capture
  (see the build-state memory's capture cheatsheet - `pane_capture`, `settings_capture`,
  `git_dock_capture`, `timeline_capture`, `empty_state_capture`, etc.) and diff them by eye for
  tokens, radii, elevation, spacing, and typography.
- The guide's raw design data (the authoritative token/scale/spacing/radii/elevation/motion/icon
  arrays) lives in the HTML's JS at roughly lines 1540-1720 - `read_page` or grep the file when
  you need exact values rather than the rendered view.

## Teardown

The `python3 -m http.server` runs in the background across the session. Leave it for repeated
parity checks; kill it when done (`pkill -f "http.server 8765"`) or when the user says they're
finished.
