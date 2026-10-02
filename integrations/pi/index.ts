import { lstatSync } from "node:fs";
import { createConnection, type Socket } from "node:net";
import { dirname, isAbsolute } from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

const MAX_FRAME = 65_536;
const MAX_TEXT = 8_192;
const STATUS = "skelly-voice";

type Request =
  | { id: number; session: string; type: "insert"; text: string }
  | { id: number; session: string; type: "prompt"; text: string; delivery: "idle" | "steer" | "follow_up" }
  | { id: number; session: string; type: "abort" };

function parseRequest(value: unknown): Request {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid request");
  const v = value as Record<string, unknown>;
  if (!Number.isSafeInteger(v.id) || Number(v.id) < 1 || typeof v.session !== "string") {
    throw new Error("Invalid request identity");
  }
  const keys = ["id", "session", "type"];
  if (v.type === "insert" || v.type === "prompt") {
    keys.push("text");
    if (typeof v.text !== "string" || !v.text.trim() || Buffer.byteLength(v.text) > MAX_TEXT
      || /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/u.test(v.text)) {
      throw new Error("Invalid voice text");
    }
  }
  if (v.type === "prompt") {
    keys.push("delivery");
    if (!["idle", "steer", "follow_up"].includes(String(v.delivery))) throw new Error("Invalid delivery");
  } else if (v.type !== "insert" && v.type !== "abort") {
    throw new Error("Unknown voice operation");
  }
  if (Object.keys(v).some((key) => !keys.includes(key))) throw new Error("Unexpected request field");
  return v as Request;
}

function boundedText(text: string): string {
  const bytes = Buffer.from(text);
  if (bytes.length <= MAX_TEXT) return text;
  let end = MAX_TEXT;
  while ((bytes[end] & 0xc0) === 0x80) end--;
  return bytes.subarray(0, end).toString("utf8");
}

function privateEndpoint(path: string): boolean {
  if (!isAbsolute(path) || !process.getuid) return false;
  const dir = lstatSync(dirname(path));
  const socket = lstatSync(path);
  return dir.isDirectory() && socket.isSocket()
    && dir.uid === process.getuid() && socket.uid === process.getuid()
    && (dir.mode & 0o077) === 0 && (socket.mode & 0o077) === 0;
}

/** No resources start at factory time: Pi can load extensions without a session. */
export default function skellyBridge(pi: ExtensionAPI) {
  let context: ExtensionContext | undefined;
  let socket: Socket | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let stopped = true;
  let generation = 0;
  let modalDepth = 0;
  let lastAssistant = "";
  let lastId = 0;

  function send(record: object) {
    const client = socket;
    if (!client || client.destroyed || client.connecting) return;
    const line = JSON.stringify(record) + "\n";
    // Do not let a stalled UI turn assistant output into an unbounded queue.
    if (Buffer.byteLength(line) > MAX_FRAME || client.writableLength + Buffer.byteLength(line) > MAX_FRAME * 2) {
      client.destroy(new Error("Skelly bridge backpressure"));
      return;
    }
    client.write(line);
  }

  function session() { return context?.sessionManager.getSessionId(); }

  function state() {
    if (!context) return;
    send({ type: "state", session: session(), busy: !context.isIdle(), blocked: modalDepth > 0 });
  }

  function stop() {
    stopped = true;
    generation++;
    if (retry) clearTimeout(retry);
    retry = undefined;
    const previous = socket;
    socket = undefined;
    previous?.destroy();
    context?.ui.setStatus(STATUS, undefined);
  }

  function connect(path: string, token: string) {
    if (stopped || !context) return;
    const currentGeneration = generation;
    // A disabled/restarted Skelly endpoint is not recreated by the extension.
    try {
      if (!privateEndpoint(path)) {
        context.ui.setStatus(STATUS, "Skelly bridge: unsafe endpoint");
        return;
      }
    } catch (error) {
      context.ui.setStatus(STATUS, "Skelly bridge: unavailable");
      if (!(error instanceof Error)) throw error;
      return;
    }
    const client = createConnection(path);
    socket = client;
    lastId = 0;
    let input = Buffer.alloc(0);
    let ready = false;
    const handshakeTimeout = setTimeout(() => client.destroy(new Error("Skelly handshake timed out")), 2000);
    handshakeTimeout.unref();
    client.on("connect", () => {
      if (stopped || generation !== currentGeneration) { client.destroy(); return; }
      send({ type: "hello", version: 1, token, session: session(), pid: process.pid });
      state();
      context?.ui.setStatus(STATUS, "Skelly connecting");
    });
    client.on("data", (chunk: Buffer) => {
      if (generation !== currentGeneration || !context) return;
      // Bound each accumulated frame even if a peer never sends LF.
      input = Buffer.concat([input, chunk]);
      let end: number;
      while ((end = input.indexOf(10)) !== -1) {
        if (stopped || generation !== currentGeneration) return;
        if (end > MAX_FRAME) { client.destroy(new Error("Skelly frame too large")); return; }
        const line = input.subarray(0, end);
        input = input.subarray(end + 1);
        let request: Request;
        try {
          // fatal UTF-8 validation: Buffer.toString alone would silently replace bad bytes.
          const record = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(line));
          if (!ready) {
            if (!record || record.type !== "welcome" || record.version !== 1 || record.session !== session()) {
              throw new Error("Skelly handshake required");
            }
            ready = true;
            clearTimeout(handshakeTimeout);
            context.ui.setStatus(STATUS, "Skelly connected");
            continue;
          }
          request = parseRequest(record);
        } catch {
          client.destroy(new Error("Invalid Skelly request"));
          return;
        }
        const reply = { type: "reply", session: session(), id: request.id, accepted: false, error: null as string | null };
        try {
          if (request.id <= lastId) throw new Error("Duplicate or out-of-order request; not replayed");
          lastId = request.id;
          if (request.session !== session()) throw new Error("Pi session changed");
          if (modalDepth > 0 && request.type !== "abort") throw new Error("Finish the Pi dialog first");
          if (request.type === "insert") {
            context.ui.pasteToEditor(request.text);
          } else if (request.type === "prompt") {
            if (!context.isIdle() && request.delivery === "idle") {
              throw new Error("Pi is busy; choose steering or follow-up");
            }
            pi.sendUserMessage(request.text, {
              expandPromptTemplates: false,
              ...(request.delivery === "idle" ? {} : {
                deliverAs: request.delivery === "steer" ? "steer" as const : "followUp" as const,
              }),
            });
          } else {
            context.abort();
          }
          reply.accepted = true;
        } catch (error) {
          // Never echo untrusted text or credential-bearing exception details to the UI.
          reply.error = error instanceof Error && [
            "Duplicate or out-of-order request; not replayed", "Pi session changed",
            "Finish the Pi dialog first", "Pi is busy; choose steering or follow-up",
          ].includes(error.message) ? error.message : "Pi could not dispatch the voice request";
        }
        send(reply);
        state();
      }
      if (input.length > MAX_FRAME) client.destroy(new Error("Skelly frame too large"));
    });
    client.on("error", () => {
      if (generation === currentGeneration) context?.ui.setStatus(STATUS, "Skelly bridge: disconnected");
    });
    client.on("close", () => {
      clearTimeout(handshakeTimeout);
      if (generation !== currentGeneration || stopped) return;
      socket = undefined;
      context?.ui.setStatus(STATUS, "Skelly bridge: disconnected");
      // Reconnect registration only. Never replay input whose outcome is unknown.
      retry = setTimeout(() => connect(path, token), 500);
      retry.unref();
    });
  }

  function start(ctx: ExtensionContext) {
    stop();
    context = ctx;
    modalDepth = 0;
    lastAssistant = "";
    const path = process.env.SKELLY_VOICE_SOCKET;
    const token = process.env.SKELLY_VOICE_TOKEN;
    if (ctx.mode !== "tui" || !path || !token || !/^[a-f0-9]{64}$/u.test(token)) return;
    stopped = false;
    connect(path, token);
  }

  pi.on("session_start", (_event, ctx) => start(ctx));
  pi.on("session_tree", (_event, ctx) => start(ctx));
  pi.on("session_shutdown", () => { stop(); context = undefined; });
  pi.on("agent_start", (_event, ctx) => { context = ctx; lastAssistant = ""; state(); });
  pi.on("ui_prompt_start", (_event, ctx) => { context = ctx; modalDepth++; state(); });
  pi.on("ui_prompt_end", (_event, ctx) => { context = ctx; modalDepth = Math.max(0, modalDepth - 1); state(); });
  pi.on("message_end", (event) => {
    if (event.message.role !== "assistant") return;
    // Only successful, user-visible assistant text. Never thinking, tool input,
    // tool results, aborted/error messages, or speculative stream fragments.
    lastAssistant = event.message.stopReason === "stop"
      ? boundedText(event.message.content.filter((part) => part.type === "text").map((part) => part.text).join("\n"))
      : "";
  });
  pi.on("agent_settled", (_event, ctx) => {
    context = ctx;
    send({ type: "settled", session: session(), text: lastAssistant });
    state();
    lastAssistant = "";
  });
}
