# 0012. Skelly serves its agent features over MCP with the official Rust SDK

- Status: Accepted
- Date: 2026-11-03
- Deciders: maintainer (an MCP server built into Skelly, so an agent finds Skelly's
  features without reading its source); implementation agent (SDK, tools, protocol eras)
- Related: ADR-0009 (Pi bridge); design/README.md decisions 2026-10-30 (review comments)
  and 2026-11-03 (`skelly mcp`); ai-planner `skelly-mcp-server`

## Context

The diff review is a conversation between the user and an agent: the user comments on
lines in the git dock and asks the agent to address them. An agent reaches the comments
through the Pi companion's tools or `skelly review`. An agent that is not Pi has to be told
that `skelly review` exists, or find the review file by reading Skelly's source. The user
wants to say "address my review comments" to any agent and have it find them.

MCP is how agent harnesses (Claude Code, Codex, Pi's MCP adapter and others) find tools. A
stdio server is started by the harness and lists its tools, with a description and an input
schema for each. The protocol changed shape in revision 2026-07-28: there is no
`initialize` handshake, every request carries its protocol version and the client's
capabilities in `_meta`, and `server/discover` replaces the handshake. Most clients in use
still speak 2025-11-25 or earlier and open with `initialize`.

## Decision

**`skelly mcp` is an MCP server on stdin/stdout, built into the `skelly` binary on the
official Rust SDK, `rmcp`.**

- **Tools.** `list_review_comments` (read-only), `reply_to_review_comment` and
  `resolve_review_comment`, each on the repository that holds an optional `directory`
  (else where the server started). They call the same helpers in `crates/skelly/src/review.rs`
  as `skelly review`, so both sides write the one review file the window watches, as the
  agent. The server's `instructions` say what Skelly is and which tool to use when. The tool
  names avoid `review_comments`, because adapters prefix the server name and Pi's own tool
  is `skelly_review_comments`.
- **Both protocol eras.** rmcp serves 2026-07-28 clients statelessly (`server/discover`,
  `_meta` checked on every request, `-32602` when it is missing, `-32022` for an unknown
  version) and earlier clients through `initialize`, from one `serve(stdio())`.
- **A failure is a tool error.** A missing comment, an empty note, a directory outside a
  repository or a bad argument comes back as a result with `isError: true` and a reason the
  agent can act on, never as a protocol error.
- **Schemas from types.** Each tool's input schema is derived from the struct that parses
  its arguments, so they cannot drift. `directory` is a plain optional string, not a
  nullable one, which single-type schema dialects (Gemini's function declarations) reject.
- **Isolation from the window.** The subcommand runs before any logging is set up, so only
  protocol messages reach stdout. A current-thread tokio runtime starts only for
  `skelly mcp`; git and the review file run on its blocking pool. A client that closes
  stdin, before or after its first message, ends the server with success.

## Consequences

Any MCP client finds and uses the review with no Skelly-specific setup beyond adding
`skelly mcp` once. Protocol compliance, including future revisions, is the SDK's job, not
Skelly's: a new MCP revision is an rmcp upgrade.

The binary carries tokio, futures, schemars and the rmcp proc macro. A local macOS (arm64)
release build grew from 15.86 MB to 17.90 MB (+2.03 MB). The GUI never starts the runtime. rmcp
moves fast (3.0.0 to 3.8.0 in three months), so upgrades are deliberate commits.

The server can reach only what is on disk: the review files. State that lives in the window
process (the session timeline, the panes) would need a channel into the running app, which
is not built.

## Alternatives considered

- **Hand-written JSON-RPC.** The tools need a few hundred lines with `serde_json`, but
  Skelly would own protocol compliance for both eras and every future revision.
- **A separate `skelly-mcp` binary.** Keeps tokio out of the GUI binary, but it is a second
  thing to install, package and keep in step with the review file format.
- **Only `skelly review` plus agent documentation.** Works when the agent is told, which is
  the problem this solves.
