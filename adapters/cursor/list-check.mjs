// Quick check: does ACP session/list include chats created outside ACP
// (e.g. via `cursor-agent create-chat`)? Pass the expected chat id as argv[2].
import { spawn } from "node:child_process";
import readline from "node:readline";
import { join } from "node:path";

const expect = process.argv[2];
const AGENT_CMD =
  process.env.CURSOR_AGENT_CMD ||
  join(process.env.LOCALAPPDATA || "", "cursor-agent", "cursor-agent.cmd");

const agent = spawn("cmd", ["/c", AGENT_CMD, "acp"], { stdio: ["pipe", "pipe", "inherit"] });
let nextId = 1;
const pending = new Map();

function send(method, params) {
  const id = nextId++;
  agent.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("timeout " + method)), 60_000);
    pending.set(id, { resolve: (v) => { clearTimeout(t); resolve(v); }, reject });
  });
}

readline.createInterface({ input: agent.stdout }).on("line", (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
    const w = pending.get(msg.id);
    if (w) { pending.delete(msg.id); msg.error ? w.reject(new Error(JSON.stringify(msg.error))) : w.resolve(msg.result); }
  }
});

try {
  await send("initialize", {
    protocolVersion: 1,
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
    clientInfo: { name: "list-check", version: "0.1.0" },
  });
  const { sessions } = await send("session/list", {});
  console.log(`total sessions: ${sessions.length}`);
  for (const s of sessions) console.log(`  ${s.sessionId}  ${s.title ?? ""}`);
  if (expect) console.log(`\nCLI-created chat ${expect}: ${sessions.some((s) => s.sessionId === expect) ? "VISIBLE via ACP" : "NOT VISIBLE via ACP"}`);
} catch (e) {
  console.error("FAILED:", e.message);
  process.exitCode = 1;
} finally {
  agent.stdin.end();
  agent.kill();
  setTimeout(() => process.exit(process.exitCode || 0), 300);
}
