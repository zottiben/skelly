# Skelly's Pi bridge

Connects the **existing interactive Pi session** to Skelly's local voice controls.
It does not create an agent, read provider credentials, call a speech API or capture audio.
Tested against Pi 0.99.2 (`@earendil-works/pi-coding-agent`).

## Try the bridge

This diagnostic checks registration and safe text delivery without a microphone.
For local dictation and turn-based spoken replies, complete the speech setup below.

1. Launch Skelly (installed release or this checkout). In Settings → Voice enable **Pi voice bridge
   (new panes)**, or set:

   ```toml
   [voice]
   enabled = true
   ```

2. Open a **new pane**. Its shell receives a private endpoint; existing shells do
   not acquire new environment variables. Run Pi directly in that pane:

   ```sh
   pi -e /absolute/path/to/skelly/integrations/pi/index.ts
   ```

   No persistent Pi installation is needed for this test. Pi's status area shows
   `Skelly connected` while registered.

3. Type a partial draft in Pi. Copy some additional text, open Skelly's command
   palette, and select **Voice: insert clipboard into Pi draft**. The text should
   be inserted into Pi's editor, **not submitted**, and the draft preserved.
   Ordinary terminal paste is unchanged.

4. Try the same command in another pane without Pi: it reports no connection
   rather than sending bytes to the shell. While Pi is asking for a modal
   approval, insertion is rejected, never treated as “yes”.

5. Disable Voice: endpoints are revoked in every workspace. Re-enabling applies
   to new panes. Closing/restarting a pane also revokes its old endpoint.

### Installed releases (no checkout required)

The matching extension ships with Skelly. Replace the checkout path above with:

```sh
# macOS (adjust if installed in ~/Applications):
pi -e /Applications/Skelly.app/Contents/Resources/pi/index.ts

# Linux, installed by install.sh / skelly update:
pi -e "$HOME/.local/share/skelly/pi/index.ts"
```

A manually extracted Linux archive contains `share/skelly/pi`; keep that folder
alongside your install and use its absolute path. No engine or model is bundled.
If Pi reports an incompatible API, use the tested Pi version above; this is not
a promise that every historical/future Pi extension API works.

For ongoing use, Pi supports installing the local package explicitly (substitute
the release's directory, not `index.ts`, when using a binary install):

```sh
pi install /absolute/path/to/skelly/integrations/pi
```

The bridge is inert outside Skelly or in non-interactive Pi modes. Nothing is
installed into your Pi configuration automatically.

## Contract

- Skelly owns one Unix socket per shell incarnation, in a random `0700`
  directory, with socket mode `0600`. `SKELLY_VOICE_SOCKET` identifies it;
  `SKELLY_VOICE_TOKEN` is a random 256-bit capability. **Never log the token.**
- Pi connects on `session_start`, authenticates, and reports its session ID and
  PID. The app verifies that the connected Pi is the pane's foreground process
  before delivering input. MVP scope is **direct local Pi**: no inferred SSH,
  tmux, process-wrapper or nested-agent routing.
- The wire protocol is UTF-8 JSON, one record terminated by LF. Unicode line
  separators inside strings do not split records. Max frame: 65,536 bytes;
  max input/answer: 8,192 bytes. Control sequences in input are rejected.
- Skelly captures a target before asynchronous work. Connection/session changes
  invalidate it, even when the same Pi session reconnects. Only one request may
  be unacknowledged; acknowledgments time out after five seconds.
- Delivery is never replayed. On a lost acknowledgment the outcome is unknown:
  inspect Pi rather than automatically submitting again.
- `insert` uses Pi's editor paste API. `prompt` uses literal
  `sendUserMessage` with explicit `idle`, `steer` or `follow_up` delivery.
  Neither interprets dictated slash commands. Acknowledgment means dispatched,
  not that the model accepted or completed the task.
- `abort` requests agent cancellation separately from audio playback. It cannot
  undo completed actions. Modal UI input is never approved over this bridge.
- Pi publishes busy/modal state and successful assistant-visible text at
  `agent_settled`, not `agent_end`. Thinking, tool calls/results, failed and
  aborted assistant messages are excluded. Long answers are bounded.
- Session replacement, tree navigation, reload and shutdown revoke the old
  connection. Sockets/timers start only with a session and are cleaned up
  idempotently. The bridge stores no audio or extra transcript on disk. Submitted
  text and assistant replies follow Pi\'s ordinary session-history persistence.

Permissions/capabilities prevent accidental cross-pane access and access by other
OS users. They are **not a sandbox against hostile processes with your UID**.
Explicitly loading the extension grants it Pi's normal process permissions.

## Verification

With Rust and Node.js 22.19+ (Node 24 in CI):

```sh
cargo test -p skelly-voice
npm --prefix integrations/pi test
```

The Rust tests include a real socket conversation with this exact extension in a
deterministic Pi API test host. No provider/model is started and no microphone
permission is requested. This proves protocol interoperability, not a manual GUI
or audio-device test.

## Local speech path

1. Explicitly install [whisper.cpp](https://github.com/ggml-org/whisper.cpp)
   **1.8.2 or newer**, and separately obtain a compatible ggml model using its
   [model setup instructions](https://github.com/ggml-org/whisper.cpp/tree/master/models).
   Skelly does not install either. Start with **base.en** for English; **tiny.en**
   is smaller/faster but less accurate. Other languages need a multilingual model.
2. In Settings → Voice, enter your executable and model paths. Enter edits/saves
   a text row; Esc discards it. Ctrl/Cmd+A selects all; Ctrl/Cmd+V pastes.
   An absolute executable path is recommended: Finder's PATH often differs from
   your terminal shell's PATH. Equivalent configuration:

   ```toml
   [voice]
   enabled = true
   whisper_program = "/absolute/path/to/whisper-cli"
   model_path = "~/Models/whisper/ggml-base.en.bin"
   language = "en"                     # language code or "auto"
   dictation_shortcut = "ctrl+shift+d"  # empty disables; palette still works
   max_recording_seconds = 60          # 5–120
   ```

   Changes made in Settings apply to the next utterance and are saved to
   config.toml. Restart Skelly after editing the file externally.
3. In a **new pane**, run Pi with the companion extension as above. Focus that Pi
   pane and press **Ctrl+Shift+D**, or choose **Voice: start/stop dictation**.
   Missing setup fails before microphone access. On macOS, grant permission to
   Skelly (or the launching terminal for a development binary); permission prompts
   can cancel the first attempt when focus changes, so start again after granting it.
4. Speak, then press the shortcut again. The persistent card changes from
   **Recording** to **Transcribing locally**. The OS default input is used, and
   stops before inference. The duration limit also stops capture automatically.
   Finished text is inserted into the **same Pi draft**, without sending it.
5. **Esc** or **Voice: cancel dictation** discards in-flight work. Switching
   pane/session/foreground process, opening Settings, window blur, disabling Voice
   or exiting also cancels. These controls do not abort Pi or undo text already
   inserted. While cancelling, wait for cleanup before starting another recording.

Audio never goes to a speech service. The capture worker converts native input to
mono PCM; whisper.cpp resamples it locally. Audio and result files use a private
temporary directory and are removed on success, failure or cancellation. A crash
or forced termination can leave private temporary files or an engine process;
deletion is not secure erase. Use normal Quit for cleanup. Silence/very low
signal and oversized output are rejected; inference times out after two minutes.
Audio-device errors suggest checking the OS input/privacy settings.

If insertion is rejected or its acknowledgment is lost, **inspect the Pi draft**.
Do not blindly retry. **Voice: copy last transcript** explicitly copies the last
completed result for manual recovery. It is kept only in memory until another
utterance starts, Voice is disabled, or Skelly exits. Clipboard contents are never
changed automatically. Cancellation discards unfinished transcription.

## Spoken turns and replies

After completing the dictation setup:

1. On macOS, playback uses the built-in `/usr/bin/say` and an installed voice.
   On Linux, explicitly install `espeak-ng` using your distribution's package
   manager. Skelly does not install voices or use a cloud speech fallback.
2. Focus the connected Pi pane and press **Ctrl+Alt+Shift+V** (or **Voice:
   record/send spoken turn**). This is **not dictation**: pressing it again stops
   capture, transcribes and **sends** the turn to Pi. Normal model/tool usage and
   charges apply; existing approval dialogs still require interaction in Pi.
3. The original Pi session remains armed for speech. New settled assistant replies,
   including replies to **typed turns**, are read locally. For replies-only mode,
   use **Voice: start/end conversation**; this opens no microphone.
4. Continue with the same record/send shortcut. During playback, the first press
   stops audio; **press again after cleanup** to record. This is turn-based:
   there is no always-on listening, simultaneous capture/playback, or echo cancellation.
5. **Esc** cancels capture, otherwise stops playback, otherwise ends the armed
   conversation. The palette also has **stop playback**, **mute/unmute spoken
   replies**, and **start/end conversation**. These never abort Pi.
   **Voice: abort Pi in this pane** is the separate, explicit agent-cancellation
   command; it cannot undo actions already completed.

Settings → **Speech** owns these additional keys under the same `[voice]` section:

```toml
turn_shortcut = "ctrl+alt+shift+v" # empty disables; cannot duplicate dictation/copy/paste
spoken_replies = true         # only within explicitly armed conversation
busy_delivery = "idle"        # idle (refuse), steer, follow_up
speech_voice = ""             # installed voice; empty = system default
speech_rate = 180             # words/minute, 80–400
```

Ctrl+Shift+C/V and Cmd/Super+C/V remain clipboard shortcuts on both platforms;
voice settings reject those bindings. The spoken-turn shortcut includes **Alt**
to preserve Linux paste.

Use `say -v '?'` on macOS or `espeak-ng --voices` on Linux to list installed
voices; the names differ between engines. Busy delivery defaults to **Refuse**.
Opt into **Steer** (next supported boundary, not tool cancellation) or **Follow up**
(queue after current work) explicitly in Speech settings. The chosen policy is
captured when the recording starts; it is checked again by Pi on dispatch.

Speech follows the **session**, not a particular voice request. Only the newest
pending settled answer is kept; busy/modal/capture-time and muted answers are
discarded. Old answers never replay on start, unmute or reconnection. Thinking,
tool logs, code, HTML, images, tables, footnotes and URL destinations are not
narrated. Speech is capped at 2000 characters plus an ellipsis and five minutes;
the full reply remains visible in Pi. Temporary speech text is private and
removed on normal completion/failure/cancel (crashes can leave it); it is never
passed as shell syntax or command-line text.

Changing pane/session/foreground process, opening Settings, losing window focus,
disabling Voice, or quitting ends local voice. **Already dispatched Pi work
continues.** Starting editable dictation also ends the conversation. After an
uncertain dispatch, inspect Pi instead of retrying; the last transcript remains
available through the explicit copy command.

Implementation tracking: ai-planner `local-voice-mvp`.

### Verification boundary

The automated tests use deterministic audio/CLI fixtures, validate cancellation,
cleanup and settings, and exercise the real companion extension against a Pi API
test host. Speech tests exercise both CLI argument dialects with silent fixtures,
including interruption/reaping, no agent abort on playback stop, and no cached-reply
replay. They do **not** prove recognition quality, microphone permissions or audible
playback on actual devices.
Before treating the slice as complete, exercise actual dictation on macOS and
Linux: existing drafts, denied permission/missing device, stop/cancel, target
changes, disable/quit during inference and playback, repeated identical replies,
muting, busy-agent delivery, and no network access to a speech service.
A real installed model and interactive Skelly/Pi are required for that check.

### Packaged-device release gate

Keep live results in ai-planner, not an assertion that automated fixtures passed.
On **each** OS, using the packaged app rather than only `cargo run`:

- With Voice off, verify no capture starts. Enable it, open a new pane and load
  the bundled extension; preserve an existing Unicode draft with the clipboard
  diagnostic, then with Ctrl+Shift+D. Confirm no submission.
- Grant then deny microphone permission; retry after granting it. Remove/switch
  the default input during capture. Errors must release capture and never send
  partial text. Measure actual recording-stop → draft/submission latency.
- Use Ctrl+Alt+Shift+V for a spoken turn. Verify intelligible recognition,
  audible output on the selected device, identical repeated replies, typed
  replies, Refuse/Steer/Follow up, and Pi's modal approvals.
- Stop/mute/end voice while Pi is working: Pi continues. Explicit Abort is
  separate. Focus away, change/close panes, change Pi sessions, reload/reconnect,
  disable Voice across workspaces, and quit during capture/inference/playback.
  Verify no delayed dispatch, replay, shell input or surviving engine on normal Quit.
- Check overlay/Esc behavior, both themes and that copy/paste still works.

macOS bundles contain the microphone usage string and audio-input entitlement.
Default releases are **ad-hoc signed**, not Developer ID notarized; the release
workflow upgrades to Developer ID/notarization only when its documented signing
secrets are configured. Neither plist/signature checks nor synthesis to an audio
file prove TCC permission or real-device playback. Ad-hoc updates may prompt for
permission again. No signing credentials are installed by Skelly.

Linux requires the ALSA runtime (`libasound.so.2`) even with Voice disabled.
Builds additionally need ALSA headers/pkg-config. Confirm the default ALSA route
reaches the intended input/output under the machine's PipeWire/PulseAudio setup;
the container missing-device test cannot establish desktop audio routing.
