// Check: can ACP session/load load an arbitrary chat id (CLI chat / IDE composer)?
// Usage: node load-check.mjs <sessionId> [--prompt "text"]
import { spawn } from "node:child_process";
import readline from "node:readline";
import { join } from "node:path";

const sessionId = process.argv[2];
const promptIdx = process.argv.indexOf("--prompt");
const promptText = promptIdx > 0 ? process.argv[promptIdx + 1] : null;

const AGENT_CMD =
  process.env.CURSOR_AGENT_CMD ||
  join(process.env.LOCALAPPDATA || "", "cursor-agent", "cursor-agent.cmd");

const agent = spawn("cmd", ["/c", AGENT_CMD, "acp"], { stdio: ["pipe", "pipe", "inherit"], cwd: process.cwd() });
let nextId = 1;
const pending = new Map();

function send(method, params) {
  const id = nextId++;
  agent.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("timeout " + method)), 90_000);
    pending.set(id, { resolve: (v) => { clearTimeout(t); resolve(v); }, reject: (e) => { clearTimeout(t); reject(e); } });
  });
}

readline.createInterface({ input: agent.stdout }).on("line", (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
    const w = pending.get(msg.id);
    if (w) { pending.delete(msg.id); msg.error ? w.reject(new Error(JSON.stringify(msg.error))) : w.resolve(msg.result); return; }
  }
  if (msg.method === "session/update") {
    const u = msg.params?.update;
    console.log(`  [update] ${u?.sessionUpdate}${u?.content?.text ? ": " + JSON.stringify(String(u.content.text).slice(0, 100)) : ""}`);
    return;
  }
  if (msg.method === "session/request_permission" && msg.id !== undefined) {
    const opts = msg.params?.options || [];
    const rej = opts.find((o) => o.kind === "reject_once") || opts[0];
    agent.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result: { outcome: { outcome: "selected", optionId: rej?.optionId } } }) + "\n");
    return;
  }
  if (msg.id !== undefined && typeof msg.method === "string") {
    agent.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "Method not found" } }) + "\n");
  }
});

try {
  await send("initialize", {
    protocolVersion: 1,
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
    clientInfo: { name: "load-check", version: "0.1.0" },
  });
  console.log(`session/load ${sessionId} ...`);
  const r = await send("session/load", { sessionId, cwd: process.cwd(), mcpServers: [] });
  console.log("session/load OK:", JSON.stringify({ modes: r.modes?.currentModeId, configOptions: (r.configOptions || []).length }));
  if (promptText) {
    console.log(`session/prompt ...`);
    const p = await send("session/prompt", { sessionId, prompt: [{ type: "text", text: promptText }] });
    console.log("session/prompt OK:", JSON.stringify(p));
  }
} catch (e) {
  console.error("FAILED:", e.message);
  process.exitCode = 1;
} finally {
  agent.stdin.end();
  agent.kill();
  setTimeout(() => process.exit(process.exitCode || 0), 300);
}
