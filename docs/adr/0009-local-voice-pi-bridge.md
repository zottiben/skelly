# 0009. Local voice augments the existing interactive Pi session

- Status: Accepted
- Date: 2026-10-01
- Deciders: maintainer (local speech and existing-session scope); implementation agent (bridge contract)
- Related: design/README.md voice decisions; ai-planner local-voice-mvp

## Context

The user wants dictation and spoken interaction with Pi already running in a
Skelly pane, without new API credentials or speech-service charges. Skelly owns
native input/rendering; Pi owns the conversation, model, extensions and tools.
Terminal bytes do not reliably identify agent messages or approval dialogs.
Pi's extension API exposes editor insertion, literal user-message submission,
explicit steering/follow-up, modal state and final settlement.

## Decision

**Keep Pi's existing interactive session and connect it to native Skelly voice
controls through a small companion extension and a private per-pane bridge.**

The bridge is opt-in (`voice.enabled = false`). Each shell incarnation inherits
its own Unix socket and random capability. Socket I/O runs off the UI thread.
Bounded JSONL records name the Pi session; captured targets also identify the
bridge and connection. The app checks foreground PID rather than guessing from
terminal output. Scope is direct local Pi, not SSH/tmux/wrapper routing.

Dictation inserts into the editor without submitting. Voice requests use Pi's
normal user-message pipeline with explicit busy-agent delivery. Modal approvals
are never accepted through this channel. Audio interruption and agent cancellation
are separate. Acknowledgments confirm dispatch, not task success; uncertain
delivery is never automatically replayed. Reconnection creates a new target.

Speech remains on-device, outside the terminal core. The MVP is a turn-based
STT → Pi → TTS pipeline; the Pi model itself may still be hosted and billed as
usual. The bridge does not persist extra transcripts; Pi retains its normal
session history.

An explicitly armed voice conversation observes new settled replies from its captured
session (including typed turns), not an inferred one-to-one request/reply mapping.
Connection-local revisions suppress cached replays and distinguish repeated identical
answers. The latest-only speech queue excludes capture-time, muted, busy and modal
results. Local output uses direct say/espeak-ng subprocesses with bounded, sanitized
prose in private temporary files; end/mute/stop never dispatch agent abort.

## Consequences

Existing Pi context, model selection, tools and approvals remain authoritative.
Adapters can later change local speech engines without changing the terminal
parser or replacing the agent. The Rust/extension wire contract needs integration
tests; workspace tests therefore require Node as well as Rust.

The matching companion is shipped with release artifacts but never enabled automatically.
Backend panics report terminal errors; cancelled engine process groups stop helpers
as well as the leader. Normal Quit joins cleanup; forced termination is not guaranteed
clean and can leave private files/processes.

There is an explicit extension setup step. Enabling endpoints affects new shells;
disabling revokes all existing endpoints. Private permissions and capabilities
are not a sandbox against hostile processes with the same UID. Native audio
capture, model setup, playback and real-device verification remain separate work.

## Alternatives considered

- **PTY injection / screen scraping:** cannot reliably preserve drafts, distinguish
  assistant answers from logs, or avoid interpreting speech as shell commands.
- **Pi RPC or SDK as a replacement agent:** appropriate for a new UI, not an
  attachment to the interactive session the user already has open.
- **Hosted speech / full-duplex voice delegation:** adds credentials and audio
  billing the user explicitly declined for the MVP.
