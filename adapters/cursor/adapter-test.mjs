// End-to-end test of adapter.mjs (spawned like the hub does):
//   1. initialize (upstream passthrough)
//   2. session/list  -> expect acp + [cli] + [ide] sessions merged
//   3. session/load <cli chat>  -> history replay
//   4. session/prompt <cli chat> -> real reply with history context
//   5. session/load <ide chat>  -> history replay
//   6. session/prompt <ide chat> -> clean rejection
// Usage: node adapter-test.mjs <cliChatId> <ideComposerId>
import { spawn } from "node:child_process";
import readline from "node:readline";

const [cliId, ideId] = process.argv.slice(2);

const adapter = spawn(process.execPath, [new URL("./adapter.mjs", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1")], {
  stdio: ["pipe", "pipe", "inherit"],
});

let nextId = 1;
const pending = new Map();
const updates = [];

function send(method, params) {
  const id = nextId++;
  adapter.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("timeout " + method)), 120_000);
    pending.set(id, { resolve: (v) => { clearTimeout(t); resolve(v); }, reject: (e) => { clearTimeout(t); reject(e); } });
  });
}

readline.createInterface({ input: adapter.stdout }).on("line", (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
    const w = pending.get(msg.id);
    if (w) { pending.delete(msg.id); msg.error ? w.reject(new Error(JSON.stringify(msg.error))) : w.resolve(msg.result); return; }
  }
  if (msg.method === "session/update") {
    updates.push(msg.params.update);
    return;
  }
  if (msg.id !== undefined && typeof msg.method === "string") {
    adapter.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "Method not found" } }) + "\n");
  }
});

function drainUpdates() {
  const u = updates.splice(0);
  return u;
}

let failures = 0;
function check(label, ok, detail = "") {
  console.log(`${ok ? "PASS" : "FAIL"}  ${label}${detail ? "  — " + detail : ""}`);
  if (!ok) failures++;
}

try {
  const init = await send("initialize", {
    protocolVersion: 1,
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
    clientInfo: { name: "adapter-test", version: "0.1.0" },
  });
  check("initialize passthrough", init.protocolVersion === 1 && init.agentCapabilities?.loadSession === true);

  const list = await send("session/list", {});
  const acpCount = list.sessions.filter((s) => !String(s.title || "").startsWith("[")).length;
  const cliCount = list.sessions.filter((s) => String(s.title || "").startsWith("[cli]")).length;
  const ideCount = list.sessions.filter((s) => String(s.title || "").startsWith("[ide]")).length;
  check("session/list merges three spaces", acpCount > 0 && cliCount > 0 && ideCount > 0,
    `acp=${acpCount} cli=${cliCount} ide=${ideCount} total=${list.sessions.length}`);
  check("cli chat present in list", list.sessions.some((s) => s.sessionId === cliId));
  check("ide chat present in list", list.sessions.some((s) => s.sessionId === ideId));

  drainUpdates();
  await send("session/load", { sessionId: cliId, cwd: process.cwd(), mcpServers: [] });
  const cliReplay = drainUpdates();
  check("cli session/load replays history", cliReplay.length >= 2,
    `${cliReplay.length} updates, first: ${JSON.stringify(cliReplay[0]?.content?.text || "").slice(0, 60)}`);

  const p = await send("session/prompt", {
    sessionId: cliId,
    prompt: [{ type: "text", text: "What exact text did I first ask you to reply with in this conversation? Answer with just that text, no tools." }],
  });
  const reply = drainUpdates().map((u) => u.content?.text || "").join("");
  check("cli session/prompt returns end_turn", p.stopReason === "end_turn");
  check("cli reply has history context", reply.includes("CLI-CHAT-OK"), JSON.stringify(reply.slice(0, 80)));

  await send("session/load", { sessionId: ideId, cwd: process.cwd(), mcpServers: [] });
  const ideReplay = drainUpdates();
  check("ide session/load replays history", ideReplay.length >= 2, `${ideReplay.length} updates`);

  let ideRejected = false, ideErr = "";
  try {
    await send("session/prompt", { sessionId: ideId, prompt: [{ type: "text", text: "hello" }] });
  } catch (e) { ideRejected = true; ideErr = e.message; }
  check("ide session/prompt cleanly rejected", ideRejected && ideErr.includes("read-only"), ideErr.slice(0, 100));

  console.log(failures === 0 ? "\nALL ADAPTER TESTS PASSED" : `\n${failures} TEST(S) FAILED`);
  process.exitCode = failures === 0 ? 0 : 1;
} catch (e) {
  console.error("TEST RUN FAILED:", e.message);
  process.exitCode = 1;
} finally {
  adapter.stdin.end();
  adapter.kill();
  setTimeout(() => process.exit(process.exitCode || 0), 300);
}
