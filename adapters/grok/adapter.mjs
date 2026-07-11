#!/usr/bin/env node
/**
 * Grok Build ACP Adapter — official `grok agent stdio` plus session-space
 * extensions for on-disk sessions.
 *
 * The official `grok agent stdio` ACP agent only knows about sessions created
 * within its own process lifetime (in-memory). Grok persists ALL sessions
 * (TUI, headless, ACP) to `~/.grok/sessions/<url-encoded-cwd>/<uuid>/`, but
 * the ACP surface cannot list or load them:
 *
 *   - `session/list`  -> upstream returns "Method not found" (not implemented)
 *   - `session/load`  <on-disk-id> -> upstream returns "Path not found"
 *
 * This adapter proxies the official agent verbatim for live ACP sessions and
 * extends it so the Hub can see and continue every on-disk Grok session:
 *
 *   space        | store                                        | list/load          | prompt
 *   -------------|----------------------------------------------|--------------------|--------------------------------
 *   acp-live     | upstream process memory (also on disk)       | upstream passthrough| upstream passthrough (full ACP)
 *   on-disk      | ~/.grok/sessions/<enc-cwd>/<uuid>/           | local ro replay    | `grok -r <id> -p`
 *               |  (chat_history.jsonl + summary.json)         | (chat_history.jsonl)| read-only plan continuation
 *
 * All on-disk access is strictly read-only. No Grok-internal storage is ever
 * written by this adapter.
 *
 * Empirical facts (verified 2026-07-09 against grok 0.2.93):
 *   1. `grok -r <id> -p "..."` resumes by SESSION ID, not by cwd bucket.
 *      Resuming from a different cwd still uses the same id and does NOT fork
 *      (unlike Cursor's CLI). The adapter still spawns resume from the
 *      session's original cwd to preserve workspace context (MCP, AGENTS.md).
 *   2. Headless resume truly continues history: a session asked to reply with
 *      a marker phrase recalls it on resume.
 *   3. ACP `session/new` sessions are also persisted to disk, so local
 *      enumeration covers every session regardless of origin.
 *   4. Grok requires `authenticate` before `session/new`. The adapter
 *      auto-authenticates with the default method right after `initialize`,
 *      absorbing this vendor step so the Hub never has to.
 *
 * Env overrides:
 *   GROK_CMD    path to grok launcher (default ~/.grok/bin/grok[.exe])
 *   GROK_HOME   path to ~/.grok
 */

import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const IS_WIN = process.platform === "win32";
const GROK_CMD =
  process.env.GROK_CMD ||
  (IS_WIN ? join(homedir(), ".grok", "bin", "grok.exe") : join(homedir(), ".grok", "bin", "grok"));
const GROK_HOME = process.env.GROK_HOME || join(homedir(), ".grok");
const SESSIONS_DIR = join(GROK_HOME, "sessions");

function log(msg) {
  process.stderr.write(`[grok-adapter] ${msg}\n`);
}

// ---- upstream: official grok agent stdio -------------------------------------

const upstream = spawn(GROK_CMD, ["agent", "stdio"], { stdio: ["pipe", "pipe", "inherit"] });

upstream.on("exit", (code) => {
  log(`upstream grok exited (${code}); shutting down`);
  process.exit(code ?? 0);
});

function toUpstream(msg) {
  upstream.stdin.write(JSON.stringify(msg) + "\n");
}
function toClient(msg) {
  process.stdout.write(JSON.stringify(msg) + "\n");
}
function respond(id, result) {
  toClient({ jsonrpc: "2.0", id, result });
}
function respondError(id, code, message) {
  toClient({ jsonrpc: "2.0", id, error: { code, message } });
}
function notifyUpdate(sessionId, update) {
  toClient({ jsonrpc: "2.0", method: "session/update", params: { sessionId, update } });
}
function chunkUpdate(kind, text) {
  return { sessionUpdate: kind, content: { type: "text", text } };
}

// ---- session space detection ------------------------------------------------

const liveSessions = new Set(); // sessionIds currently held in upstream memory
const newSessionReqIds = new Set(); // client request ids for session/new (to track live ids)

function sessionDir(id) {
  if (!/^[-0-9a-f]{16,}$/i.test(id)) return null;
  let buckets = [];
  try { buckets = readdirSync(SESSIONS_DIR); } catch { return null; }
  for (const b of buckets) {
    if (b.endsWith(".sqlite")) continue;
    const dir = join(SESSIONS_DIR, b, id);
    if (existsSync(join(dir, "summary.json"))) return dir;
  }
  return null;
}

function readSummary(dir) {
  try {
    const s = JSON.parse(readFileSync(join(dir, "summary.json"), "utf8"));
    return s || {};
  } catch { return {}; }
}

function cwdOfSummary(s, bucket) {
  if (s?.info?.cwd) return s.info.cwd;
  try { return decodeURIComponent(bucket); } catch { return null; }
}

// Extract a clean user prompt from a chat_history user entry, or null if the
// entry is injected environment context (not a real user turn).
function userTextFromEntry(rec) {
  let text = "";
  const c = rec?.content;
  if (typeof c === "string") text = c;
  else if (Array.isArray(c)) {
    for (const b of c) if (b && b.type === "text" && b.text) text += b.text;
  }
  if (!text.trim()) return null;
  if (text.startsWith("<user_info>")) return null;
  if (text.startsWith("<system-reminder>")) return null;
  const q = text.match(/<user_query>\n?([\s\S]*?)\n?<\/user_query>/);
  if (q) return q[1];
  return text;
}

function assistantTextFromEntry(rec) {
  const c = rec?.content;
  if (typeof c === "string") return c;
  let t = "";
  if (Array.isArray(c)) for (const b of c) if (b && b.type === "text" && b.text) t += b.text;
  return t;
}

function reasoningTextFromEntry(rec) {
  const sum = rec?.summary;
  if (!Array.isArray(sum)) return "";
  let t = "";
  for (const b of sum) {
    if (b && b.type === "summary_text" && b.text) t += b.text;
  }
  return t;
}

function firstPrompt(dir) {
  try {
    const lines = readFileSync(join(dir, "chat_history.jsonl"), "utf8").split(/\r?\n/);
    for (const l of lines) {
      if (!l.trim()) continue;
      let rec;
      try { rec = JSON.parse(l); } catch { continue; }
      if (rec?.type !== "user") continue;
      const t = userTextFromEntry(rec);
      if (t) return t;
    }
  } catch {}
  return null;
}

// ---- on-disk enumeration (session/list) --------------------------------------

function listOnDiskSessions() {
  const sessions = [];
  let buckets = [];
  try { buckets = readdirSync(SESSIONS_DIR); } catch { return sessions; }
  for (const b of buckets) {
    if (b.endsWith(".sqlite")) continue;
    let ids = [];
    try { ids = readdirSync(join(SESSIONS_DIR, b)); } catch { continue; }
    for (const id of ids) {
      const dir = join(SESSIONS_DIR, b, id);
      if (!existsSync(join(dir, "summary.json"))) continue;
      const s = readSummary(dir);
      const cwd = cwdOfSummary(s, b) || homedir();
      const title = (s?.session_summary || firstPrompt(dir) || "Grok Session").slice(0, 160);
      const updated = s?.updated_at || s?.created_at;
      sessions.push({
        sessionId: id,
        cwd,
        title,
        updatedAt: updated || null,
        _meta: { "grok-adapter": { space: "on-disk", model: s?.current_model_id || null } },
      });
    }
  }
  sessions.sort((a, b) => String(b.updatedAt || "").localeCompare(String(a.updatedAt || "")));
  return sessions;
}

// ---- on-disk replay (session/load) -------------------------------------------

function onDiskMessages(dir) {
  const msgs = [];
  const lines = readFileSync(join(dir, "chat_history.jsonl"), "utf8").split(/\r?\n/);
  for (const l of lines) {
    if (!l.trim()) continue;
    let rec;
    try { rec = JSON.parse(l); } catch { continue; }
    if (rec?.type === "user") {
      const t = userTextFromEntry(rec);
      if (t) msgs.push({ role: "user", text: t });
    } else if (rec?.type === "assistant") {
      const t = assistantTextFromEntry(rec);
      if (t && t.trim()) msgs.push({ role: "assistant", text: t });
    } else if (rec?.type === "reasoning") {
      const t = reasoningTextFromEntry(rec);
      if (t && t.trim()) msgs.push({ role: "thought", text: t });
    }
  }
  return msgs;
}

function handleOnDiskLoad(msg) {
  const sid = msg.params.sessionId;
  const dir = sessionDir(sid);
  if (!dir) return respondError(msg.id, -32002, `Session not found: ${sid}`);
  let msgs;
  try { msgs = onDiskMessages(dir); }
  catch (e) { return respondError(msg.id, -32603, `failed to read on-disk chat: ${e.message}`); }
  for (const m of msgs) {
    const kind = m.role === "user" ? "user_message_chunk" : m.role === "thought" ? "agent_thought_chunk" : "agent_message_chunk";
    notifyUpdate(sid, chunkUpdate(kind, m.text));
  }
  respond(msg.id, {});
}

// ---- on-disk prompt (headless resume) ----------------------------------------

const runningPrompts = new Map(); // sessionId -> child process

function extractTextBlocks(prompt) {
  if (Array.isArray(prompt)) {
    let t = "";
    for (const b of prompt) if (b && b.type === "text" && b.text) t += b.text;
    return t;
  }
  if (typeof prompt === "string") return prompt;
  return "";
}

function mapStopReason(raw) {
  const r = String(raw || "").toLowerCase();
  if (r === "endturn" || r === "end_turn") return "end_turn";
  if (r === "maxturns" || r === "max_turns") return "max_turns";
  if (r === "cancelled" || r === "canceled") return "cancelled";
  if (r === "toolapprovaldenied") return "tool_approval_denied";
  return "end_turn";
}

function handleOnDiskPrompt(msg) {
  const sid = msg.params.sessionId;
  const text = extractTextBlocks(msg.params.prompt);
  if (!text.trim()) return respondError(msg.id, -32602, "Empty prompt (only text blocks are supported for on-disk sessions)");

  const dir = sessionDir(sid);
  if (!dir) return respondError(msg.id, -32002, `Session not found: ${sid}`);
  const summary = readSummary(dir);
  const bucket = dir.split(/[\\/]/).slice(-2, -1)[0];
  const cwd = cwdOfSummary(summary, bucket);
  if (!cwd || !existsSync(cwd)) {
    return respondError(msg.id, -32603, `original workspace for grok session ${sid} no longer exists: ${cwd || "(unknown)"}`);
  }

  const args = [
    "--no-auto-update",
    "-r", sid,
    "-p", text,
    "--output-format", "streaming-json",
    // A detached headless resume cannot relay tool approval requests through
    // ACP, so keep imported on-disk sessions read-only. Full permission-aware
    // work must use a live upstream ACP session.
    "--permission-mode", "plan",
    "--cwd", cwd,
  ];
  const child = spawn(GROK_CMD, args, { stdio: ["ignore", "pipe", "inherit"], cwd });
  runningPrompts.set(sid, child);

  let cancelled = false;
  let stopReason = "end_turn";
  let sawEnd = false;
  child.cancelPrompt = () => { cancelled = true; child.kill(); };

  const rl = createInterface({ input: child.stdout });
  rl.on("line", (line) => {
    if (!line.trim()) return;
    let ev;
    try { ev = JSON.parse(line); } catch { return; }
    if (ev.type === "thought" && typeof ev.data === "string") {
      notifyUpdate(sid, chunkUpdate("agent_thought_chunk", ev.data));
    } else if (ev.type === "text" && typeof ev.data === "string") {
      notifyUpdate(sid, chunkUpdate("agent_message_chunk", ev.data));
    } else if (ev.type === "end") {
      sawEnd = true;
      stopReason = mapStopReason(ev.stopReason);
    }
  });

  child.on("exit", (code) => {
    runningPrompts.delete(sid);
    if (cancelled) return respond(msg.id, { stopReason: "cancelled" });
    if (code !== 0 && !sawEnd) {
      return respondError(msg.id, -32603, `grok headless run failed (exit ${code})`);
    }
    respond(msg.id, { stopReason });
  });
  child.on("error", (e) => {
    runningPrompts.delete(sid);
    respondError(msg.id, -32603, `failed to spawn grok: ${e.message}`);
  });
}

// ---- initialize: capability injection + auto-authenticate --------------------

const pendingUpstream = new Map(); // id -> {resolve, reject}
let initBuffered = null; // the upstream init result, held until auth completes
let initClientId = null; // the client's initialize request id
let authDone = false;

function sendUpstreamRequest(method, params) {
  const id = `grok-adapter-${Math.random().toString(36).slice(2)}`;
  toUpstream({ jsonrpc: "2.0", id, method, params });
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("upstream timeout " + method)), 30_000);
    pendingUpstream.set(id, { resolve: (v) => { clearTimeout(t); resolve(v); }, reject: (e) => { clearTimeout(t); reject(e); } });
  });
}

async function finalizeInit() {
  if (!initBuffered || initClientId === null) return;
  // Inject sessionCapabilities.list so the Hub calls session/list (which the
  // adapter serves locally, since upstream does not implement it). Keep the
  // upstream's loadSession:true so session/load is permitted; the adapter
  // intercepts load for on-disk ids and replays locally.
  const caps = initBuffered.agentCapabilities || {};
  caps.sessionCapabilities = caps.sessionCapabilities || {};
  if (!caps.sessionCapabilities.list) caps.sessionCapabilities.list = {};
  initBuffered.agentCapabilities = caps;

  // Best-effort auto-authenticate with the advertised default method, so the
  // Hub can call session/new without a manual authenticate step. Grok requires
  // auth before session/new (verified 2026-07-09).
  if (!authDone) {
    const methods = Array.isArray(initBuffered.authMethods) ? initBuffered.authMethods : [];
    const defaultId =
      (initBuffered._meta && initBuffered._meta.defaultAuthMethodId) ||
      (methods.find((m) => m.id === "cached_token")?.id) ||
      (methods[0] && methods[0].id) ||
      "cached_token";
    try {
      await sendUpstreamRequest("authenticate", { methodId: defaultId, _meta: { headless: true } });
      log(`auto-authenticated with ${defaultId}`);
    } catch (e) {
      log(`auto-auth (${defaultId}) failed: ${e.message}; hub may need to authenticate manually`);
    }
    authDone = true;
  }

  respond(initClientId, initBuffered);
  initBuffered = null;
  initClientId = null;
}

// ---- main routing ------------------------------------------------------------

const clientIn = createInterface({ input: process.stdin });
clientIn.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }

  // Client responses to upstream-initiated requests (permission, _x.ai/*).
  if (msg.method === undefined) {
    toUpstream(msg);
    return;
  }

  const sid = msg.params?.sessionId;

  switch (msg.method) {
    case "initialize":
      // Hold the client's initialize; forward upstream, then inject caps +
      // auto-auth before replying.
      initClientId = msg.id;
      toUpstream(msg);
      return;
    case "session/list":
      // Upstream does not implement session/list. Serve on-disk sessions
      // locally in a single page.
      respond(msg.id, { sessions: listOnDiskSessions(), nextCursor: null });
      return;
    case "session/load": {
      if (sid && liveSessions.has(sid)) { toUpstream(msg); return; }
      if (sid && sessionDir(sid)) return handleOnDiskLoad(msg);
      // Unknown id: forward upstream so it owns the authoritative error.
      toUpstream(msg);
      return;
    }
    case "session/prompt": {
      if (sid && liveSessions.has(sid)) { toUpstream(msg); return; }
      if (sid && sessionDir(sid)) return handleOnDiskPrompt(msg);
      toUpstream(msg);
      return;
    }
    case "session/new":
      // Forward upstream; record the returned id as live when the response arrives.
      newSessionReqIds.add(msg.id);
      toUpstream(msg);
      return;
    case "session/set_mode":
    case "session/set_config_option": {
      if (sid && !liveSessions.has(sid) && sessionDir(sid)) {
        return respondError(msg.id, -32602, `${msg.method} is not supported for on-disk (headless-resumed) sessions`);
      }
      toUpstream(msg);
      return;
    }
    case "session/cancel": {
      const child = sid && runningPrompts.get(sid);
      if (child) { child.cancelPrompt(); return; } // notification: no response
      toUpstream(msg);
      return;
    }
    default:
      toUpstream(msg);
  }
});

clientIn.on("close", () => {
  for (const [, child] of runningPrompts) try { child.kill(); } catch {}
  upstream.kill();
  process.exit(0);
});

const upstreamOut = createInterface({ input: upstream.stdout });
upstreamOut.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }

  // Responses to adapter-initiated upstream requests (e.g. authenticate).
  if (msg.id !== undefined && msg.method === undefined && typeof msg.id === "string" && String(msg.id).startsWith("grok-adapter-")) {
    const w = pendingUpstream.get(msg.id);
    if (w) {
      pendingUpstream.delete(msg.id);
      msg.error ? w.reject(new Error(JSON.stringify(msg.error))) : w.resolve(msg.result);
      return;
    }
  }

  // initialize response: capture, inject caps, auto-auth, then reply to client.
  if (msg.id !== undefined && msg.id === initClientId && msg.method === undefined) {
    if (msg.error) {
      respond(initClientId, { error: msg.error });
      initClientId = null;
      return;
    }
    initBuffered = msg.result;
    finalizeInit();
    return;
  }

  // session/new response: record the live session id.
  if (msg.id !== undefined && msg.method === undefined && newSessionReqIds.has(msg.id) && msg.result && msg.result.sessionId) {
    newSessionReqIds.delete(msg.id);
    liveSessions.add(msg.result.sessionId);
  }

  // Forward everything else (session/update, _x.ai/* notifications, request
  // responses for live sessions, upstream-initiated requests) to the client.
  toClient(msg);
});

log(`ready (upstream: ${GROK_CMD} agent stdio; sessions: ${SESSIONS_DIR})`);
