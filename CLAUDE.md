<!--
CLAUDE.md — Claude Code entry point for this project.

Keep this file THIN. The single source of truth is AGENTS.md (Codex and
OpenCode read it natively; this import makes Claude Code read it too). Put
durable, cross-harness project knowledge in AGENTS.md — not here.
-->

@AGENTS.md

<!-- Claude-Code-only notes. Cross-harness knowledge lives in AGENTS.md. -->

- `design/` is the source of truth; the guide is in `design/Skelly Design Guide.dc.html`. Read `design/README.md` for open decisions before making product calls.
- Use `/pre-pr` before pushing or opening a PR (runs this repo's real checks + `/code-review` on the diff), and `/capture` to record any project gotcha into AGENTS.md.
- Use the context7 MCP for up-to-date Rust crate docs when picking or wiring the renderer / PTY / font-shaping crates.
