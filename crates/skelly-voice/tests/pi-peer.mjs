// A deterministic Pi API test host around the real companion extension.
// No agent/model is constructed, so this test cannot incur provider charges.
import skellyBridge from "../../../integrations/pi/index.ts";

const handlers = new Map();
let draft = "draft: ";
let busy = false;
const context = {
  mode: "tui",
  sessionManager: { getSessionId: () => "fixture-session" },
  isIdle: () => !busy,
  ui: {
    setStatus() {},
    pasteToEditor(text) {
      draft += text;
      process.stdout.write(JSON.stringify({ type: "draft", text: draft }) + "\n");
    },
  },
  abort() { busy = false; },
};
skellyBridge({
  on(event, handler) { handlers.set(event, handler); },
  sendUserMessage(text, options) {
    process.stdout.write(JSON.stringify({ type: "prompt", text, options }) + "\n");
    busy = true;
    handlers.get("agent_start")?.({ type: "agent_start" }, context);
    setImmediate(() => {
      handlers.get("message_end")?.({
        type: "message_end",
        message: {
          role: "assistant", stopReason: "stop",
          content: [
            { type: "thinking", thinking: "never relay this" },
            { type: "text", text: "Fixture reply." },
          ],
        },
      }, context);
      busy = false;
      handlers.get("agent_settled")?.({ type: "agent_settled" }, context);
    });
  },
});
handlers.get("session_start")({ type: "session_start", reason: "startup" }, context);
process.on("SIGTERM", () => {
  handlers.get("session_shutdown")?.({ type: "session_shutdown" }, context);
  process.exit(0);
});
