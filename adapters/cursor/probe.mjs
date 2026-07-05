#!/usr/bin/env node
/**
 * Deep probe of the official Cursor ACP agent (`cursor-agent acp`).
 *
 * Captures full wire shapes the Hub will see:
 *   - initialize response (capabilities, authMethods)
 *   - session/new response (modes, configOptions)
 *   - session/set_mode support
 *   - plan-mode prompt: how the agent behaves when the client answers
 *     cursor/* extension requests with JSON-RPC method-not-found
 *     (exactly what acp-hub's generic SDK client does)
 *   - session/list SessionInfo shape
 *
 * Usage: node probe.mjs
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
    }, 120_000);
    pending.set(id, {
      resolve: (v) => { clearTimeout(timer); resolve(v); },
      reject: (e) => { clearTimeout(timer); reject(e); },
    });
  });
}

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
    console.log(`  [update] ${u?.sessionUpdate}${u?.content?.text ? ": " + JSON.stringify(u.content.text.slice(0, 80)) : ""}`);
    return;
  }

  if (msg.method === "session/request_permission" && msg.id !== undefined) {
    console.log(`  [permission request] ${JSON.stringify(msg.params?.toolCall?.title)} options=${JSON.stringify(msg.params?.options)}`);
    // Simulate hub permission_policy=reject: pick first reject option.
    const opts = msg.params?.options || [];
    const rej = opts.find((o) => o.kind === "reject_once") || opts[0];
    agent.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result: { outcome: { outcome: "selected", optionId: rej?.optionId } } }) + "\n");
    return;
  }

  if (typeof msg.method === "string" && msg.method.startsWith("cursor/")) {
    if (msg.id !== undefined) {
      // Simulate acp-hub's generic SDK client: unknown method -> -32601.
      console.log(`  [cursor ext REQUEST] ${msg.method} -> replying method-not-found`);
      agent.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "Method not found" } }) + "\n");
    } else {
      console.log(`  [cursor ext notification] ${msg.method}`);
    }
    return;
  }

  console.log(`  [other incoming] ${JSON.stringify(msg).slice(0, 200)}`);
});

function dump(label, obj) {
  console.log(`\n=== ${label} ===`);
  console.log(JSON.stringify(obj, null, 2));
}

try {
  const init = await send("initialize", {
    protocolVersion: 1,
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
    clientInfo: { name: "acp-hub-probe", version: "0.1.0" },
  });
  dump("initialize", init);

  const created = await send("session/new", { cwd: process.cwd(), mcpServers: [] });
  dump("session/new", created);
  const sessionId = created.sessionId;

  try {
    const sm = await send("session/set_mode", { sessionId, modeId: "plan" });
    dump("session/set_mode -> plan", sm);
  } catch (e) {
    console.log("\n=== session/set_mode FAILED ===\n" + e.message);
  }

  console.log("\n=== plan-mode prompt (watch for cursor/* extension methods) ===");
  try {
    const r = await send("session/prompt", {
      sessionId,
      prompt: [{ type: "text", text: "Reply with a one-sentence plan. Do not read any files." }],
    });
    dump("session/prompt result", r);
  } catch (e) {
    console.log("session/prompt FAILED/TIMEOUT:", e.message);
  }

  const list = await send("session/list", {});
  dump("session/list (first 3 SessionInfo)", { count: list.sessions?.length, sample: list.sessions?.slice(0, 3), nextCursor: list.nextCursor ?? null });

  console.log("\nPROBE DONE");
} catch (e) {
  console.error("PROBE FAILED:", e.message);
  process.exitCode = 1;
} finally {
  agent.stdin.end();
  agent.kill();
  setTimeout(() => process.exit(process.exitCode || 0), 500);
}
