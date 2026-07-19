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
 *   on-disk      | ~/.grok/sessions/<enc-cwd>/<uuid>/           | local ro replay    | `grok -r <id> --prompt-file`
 *               |  (chat_history.jsonl + summary.json)         | (chat_history.jsonl)| denied-tools continuation
 *
 * Listing and replay open the on-disk store read-only. Resuming an existing
 * session and deleting a session intentionally call the supported Grok CLI,
 * which may update or remove Grok-managed state.
 *
 * Compatibility assumptions are covered by the verification matrix in
 * `doc/dev/grok-adapter/spec.md`; do not treat one local CLI version as a
 * permanent contract.
 *
 * Observed behavior:
 *   1. `grok -r <id> --prompt-file <path>` resumes by SESSION ID, not by cwd bucket.
 *      Resuming from a different cwd still uses the same id and does NOT fork
 *      (unlike Cursor's CLI). The adapter still spawns resume from the
 *      session's original cwd to preserve workspace context (MCP, AGENTS.md).
 *   2. Headless resume is expected to continue the selected session history.
 *   3. ACP `session/new` sessions are also persisted to disk, so local
 *      enumeration covers every session regardless of origin.
 *   4. Grok requires `authenticate` before `session/new`. The adapter
 *      auto-authenticates with the default method right after `initialize`,
 *      absorbing this vendor step so the Hub never has to.
 *
 * Env overrides:
 *   GROK_CMD    path to grok launcher (default ~/.grok/bin/grok[.exe])
 *   GROK_HOME   path to ~/.grok
 *   GROK_AGENT_SCRIPT  test-only Node entry point used after GROK_CMD
 */

import { spawn, spawnSync } from "node:child_process";
import { createInterface } from "node:readline";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

const IS_WIN = process.platform === "win32";
const GROK_CMD =
  process.env.GROK_CMD ||
  (IS_WIN ? join(homedir(), ".grok", "bin", "grok.exe") : join(homedir(), ".grok", "bin", "grok"));
const GROK_AGENT_SCRIPT = process.env.GROK_AGENT_SCRIPT || null;
const GROK_HOME = process.env.GROK_HOME || join(homedir(), ".grok");
const SESSIONS_DIR = join(GROK_HOME, "sessions");
const VENDOR_STDERR_DIAGNOSTIC_LIMIT = 64 * 1024;
const MAX_HEADLESS_STREAM_BYTES = 16 * 1024 * 1024;
const CANONICAL_SESSION_ID = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i;

function grokArgs(args) {
  return GROK_AGENT_SCRIPT ? [GROK_AGENT_SCRIPT, ...args] : args;
}

function log(msg) {
  process.stderr.write(`[grok-adapter] ${msg}\n`);
}

function drainVendorStderr(stream) {
  const state = { bytes: 0, truncated: false, failed: false };
  if (!stream) {
    state.failed = true;
    return state;
  }
  stream.on("data", (chunk) => {
    const size = Buffer.isBuffer(chunk) ? chunk.length : Buffer.byteLength(String(chunk));
    const remaining = Math.max(0, VENDOR_STDERR_DIAGNOSTIC_LIMIT - state.bytes);
    state.bytes += Math.min(size, remaining);
    if (size > remaining) state.truncated = true;
  });
  stream.on("error", () => {
    state.failed = true;
  });
  return state;
}

function logDiscardedVendorStderr(label, state) {
  if (state.failed) {
    log(`${label} stderr could not be drained`);
  } else if (state.bytes > 0) {
    log(
      `${label} stderr discarded` +
        (state.truncated ? ` after ${VENDOR_STDERR_DIAGNOSTIC_LIMIT} byte diagnostic limit` : "")
    );
  }
}

// ---- upstream: official grok agent stdio -------------------------------------

const upstream = spawn(GROK_CMD, grokArgs(["agent", "stdio"]), {
  stdio: ["pipe", "pipe", "pipe"],
  windowsHide: true,
});
const upstreamStderr = drainVendorStderr(upstream.stderr);
let upstreamSettled = false;

function settleUpstream(kind, code = 1) {
  if (upstreamSettled) return;
  upstreamSettled = true;
  logDiscardedVendorStderr("upstream grok", upstreamStderr);
  if (kind === "spawn-error") {
    log("failed to start upstream grok; shutting down");
  } else {
    log(`upstream grok exited (${code}); shutting down`);
  }
  shutdown(code ?? 1);
}

upstream.once("error", () => settleUpstream("spawn-error", 1));
upstream.once("close", (code) => settleUpstream("close", code ?? 1));
upstream.stdin.on("error", () => {});

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

function isValidSessionId(id) {
  return typeof id === "string" && CANONICAL_SESSION_ID.test(id);
}

function sessionDirs(id) {
  if (!isValidSessionId(id)) return [];
  let buckets = [];
  try { buckets = readdirSync(SESSIONS_DIR); } catch { return []; }
  const matches = [];
  for (const b of buckets) {
    if (b.endsWith(".sqlite")) continue;
    const dir = join(SESSIONS_DIR, b, id);
    if (existsSync(join(dir, "summary.json"))) matches.push(dir);
  }
  return matches;
}

function sessionDir(id) {
  const matches = sessionDirs(id);
  return matches.length === 1 ? matches[0] : null;
}

function readSummary(dir) {
  const summary = JSON.parse(readFileSync(join(dir, "summary.json"), "utf8"));
  if (!summary || typeof summary !== "object" || Array.isArray(summary)) {
    throw new Error("unsupported Grok summary schema");
  }
  if (
    summary.info !== undefined &&
    (!summary.info || typeof summary.info !== "object" || Array.isArray(summary.info))
  ) {
    throw new Error("unsupported Grok summary schema");
  }
  for (const value of [
    summary.info?.cwd,
    summary.session_summary,
    summary.updated_at,
    summary.created_at,
    summary.current_model_id,
  ]) {
    if (value !== undefined && value !== null && typeof value !== "string") {
      throw new Error("unsupported Grok summary schema");
    }
  }
  return summary;
}

function cwdOfSummary(s, bucket) {
  if (typeof s?.info?.cwd === "string" && s.info.cwd) return s.info.cwd;
  try {
    const cwd = decodeURIComponent(bucket);
    return cwd ? cwd : null;
  } catch {
    return null;
  }
}

// Extract a clean user prompt from a chat_history user entry, or null if the
// entry is injected environment context (not a real user turn).
function userTextFromEntry(rec) {
  let text = "";
  const c = rec?.content;
  if (typeof c === "string") text = c;
  else if (Array.isArray(c)) {
    for (const b of c) {
      if (!b || b.type !== "text" || typeof b.text !== "string") return undefined;
      text += b.text;
    }
  } else {
    return undefined;
  }
  if (!text.trim()) return undefined;
  if (text.startsWith("<user_info>")) return null;
  if (text.startsWith("<system-reminder>")) return null;
  const q = text.match(/<user_query>\n?([\s\S]*?)\n?<\/user_query>/);
  if (q) return q[1].trim() ? q[1] : undefined;
  return text;
}

function assistantTextFromEntry(rec) {
  const c = rec?.content;
  if (typeof c === "string") return c.trim() ? c : undefined;
  let t = "";
  if (!Array.isArray(c)) return undefined;
  for (const b of c) {
    if (!b || b.type !== "text" || typeof b.text !== "string") return undefined;
    t += b.text;
  }
  return t.trim() ? t : undefined;
}

function reasoningTextFromEntry(rec) {
  const sum = rec?.summary;
  if (!Array.isArray(sum)) return undefined;
  let t = "";
  for (const b of sum) {
    if (!b || b.type !== "summary_text" || typeof b.text !== "string") return undefined;
    t += b.text;
  }
  return t.trim() ? t : undefined;
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
  const candidates = [];
  let buckets = [];
  try { buckets = readdirSync(SESSIONS_DIR); } catch { return candidates; }
  for (const b of buckets) {
    if (b.endsWith(".sqlite")) continue;
    let ids = [];
    try { ids = readdirSync(join(SESSIONS_DIR, b)); } catch { continue; }
    for (const id of ids) {
      if (!isValidSessionId(id)) continue;
      const dir = join(SESSIONS_DIR, b, id);
      if (!existsSync(join(dir, "summary.json"))) continue;
      let s;
      try {
        s = readSummary(dir);
      } catch {
        continue;
      }
      const cwd = cwdOfSummary(s, b) || homedir();
      const title = (s?.session_summary || firstPrompt(dir) || "Grok Session").slice(0, 160);
      const updated = s?.updated_at || s?.created_at;
      candidates.push({
        sessionId: id,
        cwd,
        title,
        updatedAt: updated || null,
        _meta: { "grok-adapter": { space: "on-disk", model: s?.current_model_id || null } },
      });
    }
  }
  const counts = new Map();
  for (const session of candidates) {
    counts.set(session.sessionId, (counts.get(session.sessionId) || 0) + 1);
  }
  const sessions = candidates.filter((session) => counts.get(session.sessionId) === 1);
  sessions.sort((a, b) => String(b.updatedAt || "").localeCompare(String(a.updatedAt || "")));
  return sessions;
}

// ---- on-disk replay (session/load) -------------------------------------------

function onDiskMessages(dir) {
  const msgs = [];
  let records = 0;
  let recognized = 0;
  const lines = readFileSync(join(dir, "chat_history.jsonl"), "utf8").split(/\r?\n/);
  for (const l of lines) {
    if (!l.trim()) continue;
    let rec;
    try { rec = JSON.parse(l); }
    catch { throw new Error("unsupported Grok chat storage schema"); }
    records++;
    if (rec?.type === "user") {
      recognized++;
      const t = userTextFromEntry(rec);
      if (t === undefined) throw new Error("unsupported Grok chat storage schema");
      if (t !== null) msgs.push({ role: "user", text: t });
    } else if (rec?.type === "assistant") {
      recognized++;
      const t = assistantTextFromEntry(rec);
      if (t === undefined) throw new Error("unsupported Grok chat storage schema");
      msgs.push({ role: "assistant", text: t });
    } else if (rec?.type === "reasoning") {
      recognized++;
      const t = reasoningTextFromEntry(rec);
      if (t === undefined) throw new Error("unsupported Grok chat storage schema");
      msgs.push({ role: "thought", text: t });
    }
  }
  if (records > 0 && recognized === 0) {
    throw new Error("unsupported Grok chat storage schema");
  }
  return msgs;
}

function handleOnDiskLoad(msg) {
  const sid = msg.params.sessionId;
  const dir = sessionDir(sid);
  if (!dir) return respondError(msg.id, -32002, "Session not found");
  let msgs;
  try {
    readSummary(dir);
    msgs = onDiskMessages(dir);
  }
  catch { return respondError(msg.id, -32603, "failed to read on-disk chat"); }
  for (const m of msgs) {
    const kind = m.role === "user" ? "user_message_chunk" : m.role === "thought" ? "agent_thought_chunk" : "agent_message_chunk";
    notifyUpdate(sid, chunkUpdate(kind, m.text));
  }
  respond(msg.id, {});
}

// ---- on-disk prompt (headless resume) ----------------------------------------

const runningPrompts = new Map(); // sessionId -> child process
let shuttingDown = false;

function terminatePrompt(child) {
  try { child?.cleanupPrompt?.(); } catch {}
  if (!child?.pid) return;
  if (IS_WIN) {
    spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
      windowsHide: true,
      stdio: "ignore",
    });
  } else {
    try { process.kill(-child.pid, "SIGTERM"); } catch { try { child.kill(); } catch {} }
  }
}

function shutdown(code = 0) {
  if (shuttingDown) return;
  shuttingDown = true;
  for (const [, child] of runningPrompts) terminatePrompt(child);
  runningPrompts.clear();
  try { upstream.kill(); } catch {}
  process.exit(code);
}

function extractTextBlocks(prompt) {
  if (!Array.isArray(prompt)) return null;
  let text = "";
  for (const block of prompt) {
    if (
      !block ||
      typeof block !== "object" ||
      Array.isArray(block) ||
      block.type !== "text" ||
      typeof block.text !== "string"
    ) {
      return null;
    }
    text += block.text;
  }
  return text;
}

function mapStopReason(raw) {
  const r = String(raw || "").toLowerCase();
  if (r === "endturn" || r === "end_turn") return "end_turn";
  if (r === "maxturns" || r === "max_turns") return "max_turns";
  if (r === "cancelled" || r === "canceled") return "cancelled";
  if (r === "toolapprovaldenied" || r === "tool_approval_denied") {
    return "tool_approval_denied";
  }
  return null;
}

function handleOnDiskPrompt(msg) {
  const sid = msg.params.sessionId;
  const text = extractTextBlocks(msg.params.prompt);
  if (text === null) {
    return respondError(
      msg.id,
      -32602,
      "On-disk Grok sessions accept text prompt blocks only"
    );
  }
  if (!text.trim()) return respondError(msg.id, -32602, "Empty prompt (only text blocks are supported for on-disk sessions)");
  if (runningPrompts.has(sid)) {
    return respondError(msg.id, -32009, "the session has an in-flight prompt");
  }

  const dir = sessionDir(sid);
  if (!dir) return respondError(msg.id, -32002, "Session not found");
  let summary;
  try {
    summary = readSummary(dir);
  } catch {
    return respondError(msg.id, -32603, "failed to read on-disk session metadata");
  }
  const bucket = dir.split(/[\\/]/).slice(-2, -1)[0];
  const cwd = cwdOfSummary(summary, bucket);
  if (!cwd || !existsSync(cwd)) {
    return respondError(msg.id, -32603, "the original workspace for this Grok session no longer exists");
  }

  // The installed CLI exposes --prompt-file but no stdin prompt flag. Keep
  // prompt text out of the OS argument vector by using a random private
  // temporary directory (mode 0600 where the platform honors POSIX modes) and
  // remove it as soon as the child exits.
  let promptDir;
  try {
    promptDir = mkdtempSync(join(tmpdir(), "acp-hub-grok-prompt-"));
    writeFileSync(join(promptDir, "prompt.txt"), text, { encoding: "utf8", mode: 0o600 });
  } catch {
    if (promptDir) {
      try { rmSync(promptDir, { recursive: true, force: true }); } catch {}
    }
    return respondError(msg.id, -32603, "failed to prepare Grok prompt input");
  }
  const promptPath = join(promptDir, "prompt.txt");
  let promptCleaned = false;
  const cleanupPrompt = () => {
    if (promptCleaned) return;
    promptCleaned = true;
    try { rmSync(promptDir, { recursive: true, force: true }); } catch {}
  };

  const args = [
    "--no-auto-update",
    "-r", sid,
    "--prompt-file", promptPath,
    "--output-format", "streaming-json",
    // A detached headless resume cannot relay tool approval requests through
    // ACP, so deny workspace tools. The vendor session history can still be
    // appended by resume. Full permission-aware work must use live upstream ACP.
    "--permission-mode", "dontAsk",
    "--no-plan",
    "--no-subagents",
    "--no-memory",
    "--disable-web-search",
    "--deny", "Edit(*)",
    "--deny", "Bash(*)",
    "--deny", "Read",
    "--deny", "Grep",
    "--deny", "WebFetch",
    "--deny", "MCPTool(*)",
    "--cwd", cwd,
  ];
  let child;
  try {
    child = spawn(GROK_CMD, grokArgs(args), {
      stdio: ["ignore", "pipe", "pipe"],
      cwd,
      detached: !IS_WIN,
      windowsHide: true,
    });
  } catch {
    cleanupPrompt();
    return respondError(msg.id, -32603, "failed to spawn Grok");
  }
  child.cleanupPrompt = cleanupPrompt;
  const childStderr = drainVendorStderr(child.stderr);
  runningPrompts.set(sid, child);

  let cancelled = false;
  let stopReason = null;
  let sawEnd = false;
  let streamError = false;
  let streamBytes = 0;
  const pendingUpdates = [];
  let settled = false;
  child.cancelPrompt = () => { cancelled = true; terminatePrompt(child); };
  child.stdout.on("data", (chunk) => {
    streamBytes += Buffer.isBuffer(chunk)
      ? chunk.length
      : Buffer.byteLength(String(chunk));
    if (streamBytes > MAX_HEADLESS_STREAM_BYTES && !streamError) {
      streamError = true;
      terminatePrompt(child);
    }
  });

  const rl = createInterface({ input: child.stdout });
  rl.on("line", (line) => {
    if (!line.trim()) return;
    if (streamError) return;
    let ev;
    try { ev = JSON.parse(line); } catch {
      streamError = true;
      return;
    }
    if (sawEnd) {
      streamError = true;
      return;
    }
    if (ev.type === "thought") {
      if (typeof ev.data !== "string") {
        streamError = true;
        return;
      }
      pendingUpdates.push(chunkUpdate("agent_thought_chunk", ev.data));
    } else if (ev.type === "text") {
      if (typeof ev.data !== "string") {
        streamError = true;
        return;
      }
      pendingUpdates.push(chunkUpdate("agent_message_chunk", ev.data));
    } else if (ev.type === "end") {
      const mapped = mapStopReason(ev.stopReason);
      if (mapped === null) {
        streamError = true;
        return;
      }
      sawEnd = true;
      stopReason = mapped;
    } else {
      streamError = true;
    }
  });
  rl.on("error", () => {
    streamError = true;
  });

  const settlePrompt = (kind, code = null) => {
    if (settled) return;
    settled = true;
    runningPrompts.delete(sid);
    cleanupPrompt();
    logDiscardedVendorStderr("grok headless run", childStderr);
    if (kind === "spawn-error") {
      return respondError(msg.id, -32603, "failed to spawn Grok");
    }
    if (cancelled) return respond(msg.id, { stopReason: "cancelled" });
    if (code !== 0 || !sawEnd || streamError || stopReason === null) {
      return respondError(msg.id, -32603, `grok headless run failed (exit ${code})`);
    }
    for (const update of pendingUpdates) {
      notifyUpdate(sid, update);
    }
    respond(msg.id, { stopReason });
  };
  child.once("close", (code) => settlePrompt("close", code));
  child.once("error", () => settlePrompt("spawn-error"));
}

function handleSessionDelete(msg) {
  const sid = msg.params?.sessionId;
  if (!isValidSessionId(sid)) {
    return respondError(msg.id, -32602, "session/delete requires a valid Grok session id");
  }
  if (sessionDirs(sid).length > 1) {
    return respondError(msg.id, -32602, "Grok session id is ambiguous across workspace buckets");
  }
  if (runningPrompts.has(sid)) {
    return respondError(msg.id, -32009, "the session has an in-flight prompt");
  }

  let child;
  try {
    child = spawn(GROK_CMD, grokArgs(["sessions", "delete", sid]), {
      stdio: ["ignore", "ignore", "pipe"],
      windowsHide: true,
    });
  } catch {
    return respondError(msg.id, -32603, "failed to run Grok session deletion");
  }
  const childStderr = drainVendorStderr(child.stderr);
  let settled = false;
  const settleDelete = (kind, code = null) => {
    if (settled) return;
    settled = true;
    logDiscardedVendorStderr("grok session deletion", childStderr);
    if (kind === "spawn-error") {
      return respondError(msg.id, -32603, "failed to run Grok session deletion");
    }
    if (code !== 0) {
      return respondError(
        msg.id,
        -32603,
        `grok sessions delete failed (exit ${code})`
      );
    }
    liveSessions.delete(sid);
    respond(msg.id, {});
  };
  child.once("close", (code) => settleDelete("close", code));
  child.once("error", () => settleDelete("spawn-error"));
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
    const t = setTimeout(() => {
      pendingUpstream.delete(id);
      reject(new Error("upstream request timed out"));
    }, 30_000);
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
  if (!caps.sessionCapabilities.delete) caps.sessionCapabilities.delete = {};
  initBuffered.agentCapabilities = caps;

  // Best-effort auto-authenticate with the advertised default method, so the
  // Hub can call session/new without a manual authenticate step. Grok requires
  // auth before session/new.
  if (!authDone) {
    const methods = Array.isArray(initBuffered.authMethods) ? initBuffered.authMethods : [];
    const defaultId =
      (initBuffered._meta && initBuffered._meta.defaultAuthMethodId) ||
      (methods.find((m) => m.id === "cached_token")?.id) ||
      (methods[0] && methods[0].id) ||
      "cached_token";
    try {
      await sendUpstreamRequest("authenticate", { methodId: defaultId, _meta: { headless: true } });
      log("auto-authentication completed");
    } catch {
      log("auto-authentication failed; the Hub may need to authenticate manually");
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
      const dirs = sessionDirs(sid);
      if (dirs.length > 1) {
        return respondError(msg.id, -32602, "Grok session id is ambiguous across workspace buckets");
      }
      if (dirs.length === 1) return handleOnDiskLoad(msg);
      // Unknown id: forward upstream so it owns the authoritative error.
      toUpstream(msg);
      return;
    }
    case "session/prompt": {
      if (sid && liveSessions.has(sid)) { toUpstream(msg); return; }
      const dirs = sessionDirs(sid);
      if (dirs.length > 1) {
        return respondError(msg.id, -32602, "Grok session id is ambiguous across workspace buckets");
      }
      if (dirs.length === 1) return handleOnDiskPrompt(msg);
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
      const dirs = liveSessions.has(sid) ? [] : sessionDirs(sid);
      if (dirs.length > 1) {
        return respondError(msg.id, -32602, "Grok session id is ambiguous across workspace buckets");
      }
      if (dirs.length === 1) {
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
    case "session/delete":
      handleSessionDelete(msg);
      return;
    default:
      toUpstream(msg);
  }
});

clientIn.on("close", () => {
  shutdown(0);
});

process.on("SIGINT", () => shutdown(130));
process.on("SIGTERM", () => shutdown(143));

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
      // Preserve the upstream JSON-RPC error channel. Returning `{error: ...}`
      // as a successful result makes clients treat a failed handshake as ready.
      toClient(msg);
      initClientId = null;
      return;
    }
    initBuffered = msg.result;
    finalizeInit();
    return;
  }

  // session/new response: record the live session id.
  if (msg.id !== undefined && msg.method === undefined && newSessionReqIds.has(msg.id)) {
    newSessionReqIds.delete(msg.id);
    if (msg.result && msg.result.sessionId) liveSessions.add(msg.result.sessionId);
  }

  // Forward everything else (session/update, _x.ai/* notifications, request
  // responses for live sessions, upstream-initiated requests) to the client.
  toClient(msg);
});

log("ready");
