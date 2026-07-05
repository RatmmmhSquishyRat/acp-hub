#!/usr/bin/env node
/**
 * Smoke test for the official Cursor ACP agent (`cursor-agent acp`).
 *
 * Verifies the full ACP flow the Hub relies on:
 *   initialize -> session/new -> session/prompt (streamed reply)
 *   -> session/list -> session/load (history replay)
 *
 * Usage: node smoke-test.mjs
 * Requires: cursor-agent installed and logged in (`cursor-agent login`).
 */

import { spawn } from "node:child_process";
import readline from "node:readline";
import { join } from "node:path";

const AGENT_CMD =
  process.env.CURSOR_AGENT_CMD ||
  join(process.env.LOCALAPPDATA || "", "cursor-agent", "cursor-agent.cmd");

const agent = spawn("cmd", ["/c", AGENT_CMD, "acp"], {
  stdio: ["pipe", "pipe", "inherit"],
  cwd: process.cwd(),
});

let nextId = 1;
const pending = new Map();

function send(method, params) {
  const id = nextId++;
  agent.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`timeout waiting for ${method}`));
    }, 180_000);
    pending.set(id, {
      resolve: (v) => { clearTimeout(timer); resolve(v); },
      reject: (e) => { clearTimeout(timer); reject(e); },
    });
  });
}

function respond(id, result) {
  agent.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n");
}

const chunks = [];
const replayed = [];
let loading = false;

const rl = readline.createInterface({ input: agent.stdout });
rl.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }

  if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
    const waiter = pending.get(msg.id);
    if (waiter) {
      pending.delete(msg.id);
      msg.error ? waiter.reject(new Error(JSON.stringify(msg.error))) : waiter.resolve(msg.result);
      return;
    }
  }

  if (msg.method === "session/update") {
    const u = msg.params?.update;
    if (u?.sessionUpdate === "agent_message_chunk" && u.content?.text) {
      (loading ? replayed : chunks).push(u.content.text);
    }
    if (u?.sessionUpdate === "user_message_chunk" && u.content?.text && loading) {
      replayed.push(`[user] ${u.content.text}`);
    }
    return;
  }

  if (msg.method === "session/request_permission" && msg.id !== undefined) {
    const opts = msg.params?.options || [];
    const allow = opts.find((o) => o.kind === "allow_once") || opts[0];
    console.log(`[permission] ${msg.params?.toolCall?.title || "?"} -> ${allow?.optionId}`);
    respond(msg.id, { outcome: { outcome: "selected", optionId: allow?.optionId } });
    return;
  }

  // Cursor extension methods: answer blocking ones so the agent never hangs.
  if (msg.id !== undefined && typeof msg.method === "string" && msg.method.startsWith("cursor/")) {
    console.log(`[cursor ext] ${msg.method}`);
    respond(msg.id, { outcome: { outcome: "cancelled" } });
  }
});

try {
  const init = await send("initialize", {
    protocolVersion: 1,
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
    clientInfo: { name: "acp-hub-smoke-test", version: "0.1.0" },
  });
  console.log("initialize OK:", JSON.stringify(init.agentCapabilities));

  const { sessionId } = await send("session/new", { cwd: process.cwd(), mcpServers: [] });
  console.log("session/new OK:", sessionId);

  const result = await send("session/prompt", {
    sessionId,
    prompt: [{ type: "text", text: "Reply with exactly: ACP-SMOKE-OK" }],
  });
  console.log("session/prompt OK: stopReason =", result.stopReason);
  console.log("reply:", chunks.join(""));

  const list = await send("session/list", {});
  const found = list.sessions?.find((s) => s.sessionId === sessionId);
  console.log(`session/list OK: ${list.sessions?.length} sessions, new session ${found ? "FOUND" : "MISSING"}`);

  loading = true;
  await send("session/load", { sessionId, cwd: process.cwd(), mcpServers: [] });
  loading = false;
  console.log(`session/load OK: replayed ${replayed.length} chunks`);
  if (replayed.length) console.log("replay sample:", replayed.slice(0, 3).join(" | ").slice(0, 300));

  console.log("\nALL SMOKE TESTS PASSED");
} catch (e) {
  console.error("SMOKE TEST FAILED:", e.message);
  process.exitCode = 1;
} finally {
  agent.stdin.end();
  agent.kill();
}
