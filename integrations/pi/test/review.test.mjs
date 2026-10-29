import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import skellyBridge from "../index.ts";

const BUILT = fileURLToPath(new URL("../../../target/debug/skelly", import.meta.url));

// Pi's `exec`: no shell, a non-zero or failed spawn is a code rather than a throw.
function piExec(calls) {
  return (command, args, options) => new Promise((resolve) => {
    calls.push({ command, args, cwd: options?.cwd });
    execFile(command, args, { cwd: options?.cwd, env: process.env }, (error, stdout, stderr) => {
      resolve({ stdout, stderr, code: error ? (typeof error.code === "number" ? error.code : 1) : 0, killed: false });
    });
  });
}

function host(exec) {
  const tools = new Map();
  skellyBridge({ registerTool: (tool) => tools.set(tool.name, tool), on: () => {}, exec });
  return tools;
}

async function withBin(t, bin) {
  const old = process.env.SKELLY_BIN;
  process.env.SKELLY_BIN = bin;
  t.after(() => {
    if (old === undefined) delete process.env.SKELLY_BIN;
    else process.env.SKELLY_BIN = old;
  });
}

test("registers the two review tools with plain JSON Schema parameters", () => {
  const tools = host(async () => ({ stdout: "", stderr: "", code: 0, killed: false }));
  assert.deepEqual([...tools.keys()], ["skelly_review_comments", "skelly_review_resolve"]);
  const resolve = tools.get("skelly_review_resolve");
  assert.deepEqual(resolve.parameters.required, ["id", "note"]);
  assert.equal(tools.get("skelly_review_comments").annotations.readOnlyHint, true);
});

test("runs the pane's Skelly in the session directory, one argument per value", async (t) => {
  await withBin(t, "/opt/skelly/bin/skelly");
  const calls = [];
  const tools = host(async (command, args, options) => {
    calls.push({ command, args, cwd: options.cwd });
    return { stdout: "No open review comments.\n\n", stderr: "", code: 0, killed: false };
  });
  const ctx = { cwd: "/work/app" };
  const read = await tools.get("skelly_review_comments").execute("1", { all: true }, undefined, undefined, ctx);
  assert.equal(read.content[0].text, "No open review comments.\n\n");
  await tools.get("skelly_review_resolve").execute("2", { id: 3, note: "renamed it; \"quoted\" $HOME" }, undefined, undefined, ctx);
  assert.deepEqual(calls, [
    { command: "/opt/skelly/bin/skelly", args: ["review", "--all"], cwd: "/work/app" },
    { command: "/opt/skelly/bin/skelly", args: ["review", "resolve", "3", "--note", "renamed it; \"quoted\" $HOME"], cwd: "/work/app" },
  ]);
});

test("a failed run is a failed tool call that says why", async (t) => {
  await withBin(t, "/nowhere/skelly");
  const failing = (result) => host(async () => ({ stdout: "", stderr: "", killed: false, ...result }));
  const ctx = { cwd: "/" };
  await assert.rejects(
    failing({ code: 1, stderr: "Error: there is no comment #7\n" }).get("skelly_review_resolve")
      .execute("1", { id: 7, note: "x" }, undefined, undefined, ctx),
    /there is no comment #7/,
  );
  await assert.rejects(
    failing({ code: 1 }).get("skelly_review_comments").execute("1", {}, undefined, undefined, ctx),
    /could not run \/nowhere\/skelly review; is Skelly installed\?/,
  );
  await assert.rejects(
    failing({ code: 0, killed: true }).get("skelly_review_comments").execute("1", {}, undefined, undefined, ctx),
    /did not finish/,
  );
});

test("reads and resolves real comments through the built skelly", { skip: !existsSync(BUILT) && "build skelly first" }, async (t) => {
  const home = await mkdtemp(join(tmpdir(), "skelly-review-"));
  t.after(() => rm(home, { recursive: true, force: true }));
  const oldState = process.env.XDG_STATE_HOME;
  process.env.XDG_STATE_HOME = join(home, "state");
  t.after(() => {
    if (oldState === undefined) delete process.env.XDG_STATE_HOME;
    else process.env.XDG_STATE_HOME = oldState;
  });
  await withBin(t, BUILT);
  const repo = join(home, "app");
  await mkdir(repo);
  const git = (...args) => execFileSync("git", ["-C", repo, ...args], {
    env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_SYSTEM: "/dev/null" },
  });
  git("init", "-q", "-b", "main");
  await writeFile(join(repo, "lib.rs"), "fn total() {}\n");
  execFileSync(BUILT, ["review", "add", "lib.rs:1", "call it sum"], { cwd: repo, env: process.env });

  const calls = [];
  const tools = host(piExec(calls));
  const ctx = { cwd: repo };
  const read = await tools.get("skelly_review_comments").execute("1", {}, undefined, undefined, ctx);
  assert.match(read.content[0].text, /#1 lib\.rs:1\n  > fn total\(\) \{\}\n  call it sum\n/);
  const resolved = await tools.get("skelly_review_resolve").execute("2", { id: 1, note: "renamed to sum" }, undefined, undefined, ctx);
  assert.equal(resolved.content[0].text, "Resolved #1.\n");
  const after = await tools.get("skelly_review_comments").execute("3", { all: true }, undefined, undefined, ctx);
  assert.match(after.content[0].text, /^No open review comments\./);
  assert.match(after.content[0].text, /#1 lib\.rs:1 - resolved\n[\s\S]*- agent: renamed to sum\n/);
});
