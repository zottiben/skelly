import assert from "node:assert/strict";
import { chmod, mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { join } from "node:path";
import { test } from "node:test";
import skellyBridge from "../index.ts";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function wait(fn) {
  const start = Date.now();
  while (Date.now() - start < 2500) {
    const result = fn();
    if (result) return result;
    await sleep(5);
  }
  throw new Error("condition timed out");
}

async function harness(t, mode = "tui", welcome = true) {
  const dir = await mkdtemp("/tmp/skelly-pi-test-");
  await chmod(dir, 0o700);
  const path = join(dir, "pi.sock");
  const token = "a".repeat(64);
  const oldPath = process.env.SKELLY_VOICE_SOCKET;
  const oldToken = process.env.SKELLY_VOICE_TOKEN;
  process.env.SKELLY_VOICE_SOCKET = path;
  process.env.SKELLY_VOICE_TOKEN = token;
  const frames = [];
  const clients = [];
  const server = createServer((client) => {
    clients.push(client);
    let input = "";
    client.setEncoding("utf8");
    client.on("data", (data) => {
      input += data;
      let i;
      while ((i = input.indexOf("\n")) !== -1) {
        const frame = JSON.parse(input.slice(0, i));
        frames.push(frame);
        if (welcome && frame.type === "hello") {
          client.write(JSON.stringify({ type: "welcome", version: 1, session: frame.session }) + "\n");
        }
        input = input.slice(i + 1);
      }
    });
    client.on("error", () => {});
  });
  await new Promise((resolve) => server.listen(path, resolve));
  await chmod(path, 0o600);
  const handlers = new Map();
  let draft = "existing draft: ";
  let busy = false;
  let session = "s1";
  let aborts = 0;
  const sent = [];
  const statuses = [];
  const ctx = {
    mode, isIdle: () => !busy,
    sessionManager: { getSessionId: () => session },
    ui: {
      pasteToEditor: (text) => { draft += text; },
      setStatus: (_key, status) => statuses.push(status),
    },
    abort: () => { aborts++; },
  };
  const pi = {
    on: (event, handler) => handlers.set(event, handler),
    sendUserMessage: (text, options) => sent.push({ text, options }),
  };
  const emit = async (event, payload = {}) => handlers.get(event)?.({ type: event, ...payload }, ctx);
  skellyBridge(pi);
  t.after(async () => {
    await emit("session_shutdown");
    clients.forEach((c) => c.destroy());
    await new Promise((resolve) => server.close(resolve));
    await rm(dir, { recursive: true, force: true });
    if (oldPath === undefined) delete process.env.SKELLY_VOICE_SOCKET;
    else process.env.SKELLY_VOICE_SOCKET = oldPath;
    if (oldToken === undefined) delete process.env.SKELLY_VOICE_TOKEN;
    else process.env.SKELLY_VOICE_TOKEN = oldToken;
  });
  return {
    frames, clients, sent, statuses, emit, token, path, dir,
    draft: () => draft, aborts: () => aborts,
    setBusy: (value) => { busy = value; },
    setSession: (value) => { session = value; },
    start: async () => { await emit("session_start"); if (mode === "tui") await wait(() => frames.find((f) => f.type === "hello")); },
    send: (record) => clients.at(-1).write(JSON.stringify(record) + "\n"),
    reply: (id) => wait(() => frames.find((f) => f.type === "reply" && f.id === id)),
  };
}

test("factory is inert; registration starts only in interactive session_start", async (t) => {
  const h = await harness(t);
  await sleep(30);
  assert.equal(h.clients.length, 0);
  await h.start();
  assert.deepEqual(h.frames[0], { type: "hello", version: 1, token: h.token, session: "s1", pid: process.pid });
});

test("does not claim connection until Skelly authenticates the handshake", async (t) => {
  const h = await harness(t, "tui", false);
  await h.start();
  assert.equal(h.statuses.includes("Skelly connected"), false);
  h.send({ type: "welcome", version: 1, session: "s1" });
  await wait(() => h.statuses.includes("Skelly connected"));
});

test("non-interactive modes never connect", async (t) => {
  const h = await harness(t, "rpc");
  await h.start();
  await sleep(30);
  assert.equal(h.clients.length, 0);
});

test("dictation preserves the draft, Unicode/newlines, and never submits", async (t) => {
  const h = await harness(t);
  await h.start();
  const text = "/new\n中文\u2028🙂";
  h.send({ type: "insert", session: "s1", id: 1, text });
  assert.equal((await h.reply(1)).accepted, true);
  assert.equal(h.draft(), "existing draft: " + text);
  assert.deepEqual(h.sent, []);
});

test("busy prompts require explicit delivery and remain literal", async (t) => {
  const h = await harness(t);
  await h.start();
  h.setBusy(true);
  h.send({ type: "prompt", session: "s1", id: 1, text: "/new", delivery: "idle" });
  assert.equal((await h.reply(1)).accepted, false);
  h.send({ type: "prompt", session: "s1", id: 2, text: "/new", delivery: "steer" });
  assert.equal((await h.reply(2)).accepted, true);
  h.send({ type: "prompt", session: "s1", id: 3, text: "later", delivery: "follow_up" });
  await h.reply(3);
  assert.deepEqual(h.sent, [
    { text: "/new", options: { expandPromptTemplates: false, deliverAs: "steer" } },
    { text: "later", options: { expandPromptTemplates: false, deliverAs: "followUp" } },
  ]);
});

test("modal input cannot approve tools; abort is a separate explicit command", async (t) => {
  const h = await harness(t);
  await h.start();
  await h.emit("ui_prompt_start");
  h.send({ type: "insert", session: "s1", id: 1, text: "yes" });
  assert.equal((await h.reply(1)).accepted, false);
  assert.equal(h.draft(), "existing draft: ");
  assert.equal(h.aborts(), 0);
  h.send({ type: "abort", session: "s1", id: 2 });
  assert.equal((await h.reply(2)).accepted, true);
  assert.equal(h.aborts(), 1);
});

test("stale sessions and duplicated operations are rejected", async (t) => {
  const h = await harness(t);
  await h.start();
  h.send({ type: "insert", session: "old", id: 1, text: "wrong" });
  assert.equal((await h.reply(1)).accepted, false);
  const command = { type: "insert", session: "s1", id: 2, text: "once" };
  h.send(command);
  await h.reply(2);
  h.send(command);
  await wait(() => h.frames.filter((f) => f.type === "reply" && f.id === 2).length === 2);
  assert.equal(h.frames.filter((f) => f.type === "reply").at(-1).accepted, false);
  assert.equal(h.draft(), "existing draft: once");
});

test("speech receives only final assistant text at settlement", async (t) => {
  const h = await harness(t);
  await h.start();
  const assistant = (stopReason, content) => ({ message: { role: "assistant", stopReason, content } });
  await h.emit("message_end", assistant("toolUse", [{ type: "text", text: "not done" }]));
  await h.emit("agent_end");
  assert.equal(h.frames.some((f) => f.type === "settled"), false);
  await h.emit("message_end", assistant("stop", [
    { type: "thinking", thinking: "PRIVATE" },
    { type: "toolCall", name: "bash", arguments: { secret: "PRIVATE" } },
    { type: "text", text: "Finished." },
  ]));
  await h.emit("message_end", { message: { role: "toolResult", content: [{ type: "text", text: "PRIVATE" }] } });
  await h.emit("agent_settled");
  assert.equal((await wait(() => h.frames.find((f) => f.type === "settled"))).text, "Finished.");
  assert.equal(JSON.stringify(h.frames).includes("PRIVATE"), false);
});

test("oversized or malformed frames close rather than accumulate or dispatch", async (t) => {
  const h = await harness(t);
  await h.start();
  const client = h.clients[0];
  client.write("x".repeat(65537));
  await wait(() => client.destroyed);
  assert.deepEqual(h.sent, []);
});

test("session replacement reconnects without replaying or stale context", async (t) => {
  const h = await harness(t);
  await h.start();
  h.setSession("s2");
  await h.emit("session_start");
  await wait(() => h.frames.find((f) => f.type === "hello" && f.session === "s2"));
  assert.equal(h.clients[0].destroyed, true);
  h.send({ type: "insert", session: "s1", id: 1, text: "old" });
  assert.equal((await h.reply(1)).accepted, false);
  await h.emit("session_shutdown");
  const connections = h.clients.length;
  await sleep(550);
  assert.equal(h.clients.length, connections, "shutdown stops reconnect timers");
});

test("group/world-accessible endpoint is refused", async (t) => {
  const h = await harness(t);
  await chmod(h.dir, 0o755);
  await h.emit("session_start");
  await sleep(30);
  assert.equal(h.clients.length, 0);
});
