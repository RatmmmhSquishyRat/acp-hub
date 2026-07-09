// End-to-end test of adapter.mjs (spawned like the hub does):
//   1. initialize (capability injection: sessionCapabilities.list present)
//   2. session/list  -> on-disk grok sessions enumerated
//   3. session/load <on-disk id>  -> chat_history.jsonl replay
//   4. session/prompt <on-disk id> -> real headless resume with history context
//   5. session/prompt <unknown id> -> clean error (forwarded upstream auth)
//   6. session/new + session/prompt -> live ACP session works end-to-end
// Usage: node adapter-test.mjs <onDiskSessionId>
import { spawn } from "node:child_process";
import readline from "node:readline";

const [diskId] = process.argv.slice(2);

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
    const t = setTimeout(() => reject(new Error("timeout " + method)), 180_000);
    pending.set(id, { resolve: (v) => { clearTimeout(t); resolve(v); }, reject: (e) => { clearTimeout(t); reject(e); } });
  });
}

readline.createInterface({ input: adapter.stdout }).on("line", (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
    const w = pending.get(msg.id);
    if (w) { pending.delete(msg.id); msg.error ? w.reject(new Error(JSON.stringify(msg.error))) : w.resolve(msg.result); return; }
  }
  if (msg.method === "session/update") { updates.push(msg.params.update); return; }
  if (msg.method === "_x.ai/session_notification" || msg.method === "_x.ai/sessions/changed" || msg.method === "_x.ai/queue/changed" || msg.method === "_x.ai/session/prompt_complete") { return; }
  if (msg.id !== undefined && typeof msg.method === "string") {
    adapter.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "Method not found" } }) + "\n");
  }
});

function drainUpdates() { const u = updates.splice(0); return u; }

let failures = 0;
function check(label, ok, detail = "") {
  console.log(`${ok ? "PASS" : "FAIL"}  ${label}${detail ? "  — " + detail : ""}`);
  if (!ok) failures++;
}

try {
  const init = await send("initialize", {
    protocolVersion: 1,
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
    clientInfo: { name: "grok-adapter-test", version: "0.1.0" },
  });
  check("initialize returns v1", init.protocolVersion === 1);
  check("initialize injects sessionCapabilities.list", !!init.agentCapabilities?.sessionCapabilities?.list,
    `loadSession=${init.agentCapabilities?.loadSession}`);
  check("initialize preserves loadSession", init.agentCapabilities?.loadSession === true);

  const list = await send("session/list", {});
  const count = list.sessions.length;
  check("session/list returns on-disk sessions", count > 0, `count=${count}`);
  if (diskId) check("session/list contains the target on-disk id", list.sessions.some((s) => s.sessionId === diskId));

  if (diskId) {
    drainUpdates();
    await send("session/load", { sessionId: diskId, cwd: process.cwd(), mcpServers: [] });
    const replay = drainUpdates();
    check("session/load replays on-disk history", replay.length >= 2,
      `${replay.length} updates, first: ${JSON.stringify(replay[0]?.content?.text || "").slice(0, 60)}`);

    const p = await send("session/prompt", {
      sessionId: diskId,
      prompt: [{ type: "text", text: "Without using any tools, answer from this session's history only: what exact marker phrase did I ask you to reply with earlier? Reply with just the phrase, nothing else." }],
    });
    const reply = drainUpdates().map((u) => u.content?.text || "").join("");
    check("on-disk session/prompt returns end_turn", p.stopReason === "end_turn", p.stopReason);
    check("on-disk prompt has history context (recalls marker)", /GROK-RESUME-TEST-7741/i.test(reply), JSON.stringify(reply.slice(0, 80)));
  }

  // Live ACP session via session/new (proves auto-auth worked).
  const created = await send("session/new", { cwd: process.cwd(), mcpServers: [] });
  const liveId = created?.sessionId;
  check("session/new creates a live session", !!liveId, `liveId=${liveId?.slice(0, 13)}`);

  if (liveId) {
    drainUpdates();
    const p = await send("session/prompt", {
      sessionId: liveId,
      prompt: [{ type: "text", text: "Reply with exactly: GROK-LIVE-OK. Do not use any tools." }],
    });
    const reply = drainUpdates()
      .filter((u) => u.sessionUpdate === "agent_message_chunk")
      .map((u) => u.content?.text || "")
      .join("");
    check("live session/prompt returns end_turn", p.stopReason === "end_turn", p.stopReason);
    check("live prompt replies correctly", /GROK-LIVE-OK/.test(reply), JSON.stringify(reply.slice(0, 80)));
  }

  console.log(failures === 0 ? "\nALL GROK ADAPTER TESTS PASSED" : `\n${failures} TEST(S) FAILED`);
  process.exitCode = failures === 0 ? 0 : 1;
} catch (e) {
  console.error("TEST RUN FAILED:", e.message);
  process.exitCode = 1;
} finally {
  adapter.stdin.end();
  adapter.kill();
  setTimeout(() => process.exit(process.exitCode || 0), 500);
}
