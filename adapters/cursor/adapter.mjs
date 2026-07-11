#!/usr/bin/env node
/**
 * Cursor ACP Adapter — official `cursor-agent acp` plus session-space extensions.
 *
 * The official agent only exposes its own ACP session space
 * (~/.cursor/acp-sessions). Cursor actually has three isolated conversation
 * stores; this adapter proxies the official agent verbatim and extends it:
 *
 *   space  | store                                   | list/load | prompt
 *   -------|------------------------------------------|-----------|-------------------------
 *   acp    | ~/.cursor/acp-sessions/<id>/             | upstream  | upstream (full ACP)
 *   cli    | ~/.cursor/chats/<ws-hash>/<chatId>/      | local ro  | `cursor-agent --resume
 *          |   (meta.json + store.db)                 |           |  <id> --mode ask -p`
 *          |                                          |           |  read-only continuation
 *   ide    | %APPDATA%/Cursor/User/globalStorage/     | local ro  | REJECTED — `--resume`
 *          |   state.vscdb (composerData/bubbleId)    |           |  with an IDE id silently
 *          |                                          |           |  creates a NEW empty CLI
 *          |                                          |           |  chat (verified 2026-07-05)
 *
 * All reads of cli/ide stores are strictly read-only. No Cursor-internal
 * storage is ever written by this adapter.
 *
 * Env overrides:
 *   CURSOR_AGENT_CMD  path to cursor-agent launcher
 *   CURSOR_DB_PATH    path to IDE state.vscdb
 *   CURSOR_HOME       path to ~/.cursor
 */

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";
import { existsSync, readdirSync, readFileSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";

const IS_WIN = process.platform === "win32";
const AGENT_CMD =
  process.env.CURSOR_AGENT_CMD ||
  (IS_WIN ? join(process.env.LOCALAPPDATA || "", "cursor-agent", "cursor-agent.cmd") : "cursor-agent");
const AGENT_SCRIPT = process.env.CURSOR_AGENT_SCRIPT || null;
const CURSOR_HOME = process.env.CURSOR_HOME || join(homedir(), ".cursor");
const ACP_SESSIONS_DIR = join(CURSOR_HOME, "acp-sessions");
const CHATS_DIR = join(CURSOR_HOME, "chats");
const IDE_DB_PATH =
  process.env.CURSOR_DB_PATH ||
  join(process.env.APPDATA || "", "Cursor", "User", "globalStorage", "state.vscdb");

function resolveAgentLaunch() {
  if (AGENT_SCRIPT) return { command: AGENT_CMD, prefix: [resolve(AGENT_SCRIPT)], nodeHosted: true };
  if (!IS_WIN) {
    let found = isAbsolute(AGENT_CMD)
      ? AGENT_CMD
      : String(spawnSync("which", [AGENT_CMD], { encoding: "utf8" }).stdout || "").trim();
    if (found) {
      try { found = realpathSync(found); } catch {}
      try {
        if (/^#!.*\bnode\b/.test(readFileSync(found, "utf8").slice(0, 256))) {
          return { command: process.execPath, prefix: [found], nodeHosted: true };
        }
      } catch {}
      const root = dirname(found);
      for (const [node, script] of [
        [join(root, "node"), join(root, "index.js")],
        [process.execPath, join(root, "index.js")],
      ]) {
        if (existsSync(node) && existsSync(script)) {
          return { command: node, prefix: [script], nodeHosted: true };
        }
      }
    }
    return { command: AGENT_CMD, prefix: [], nodeHosted: false };
  }
  if (/\.exe$/i.test(AGENT_CMD)) return { command: AGENT_CMD, prefix: [], nodeHosted: false };

  const roots = [];
  if (isAbsolute(AGENT_CMD)) roots.push(dirname(AGENT_CMD));
  roots.push(join(process.env.LOCALAPPDATA || "", "cursor-agent"));
  for (const root of [...new Set(roots)]) {
    const directNode = join(root, "node.exe");
    const directScript = join(root, "index.js");
    if (existsSync(directNode) && existsSync(directScript)) {
      return { command: directNode, prefix: [directScript], nodeHosted: true };
    }
    let versions = [];
    try {
      versions = readdirSync(join(root, "versions"), { withFileTypes: true })
        .filter((e) => e.isDirectory())
        .map((e) => e.name)
        .sort((a, b) => b.localeCompare(a));
    } catch {}
    for (const version of versions) {
      const node = join(root, "versions", version, "node.exe");
      const script = join(root, "versions", version, "index.js");
      if (existsSync(node) && existsSync(script)) return { command: node, prefix: [script], nodeHosted: true };
    }
  }
  throw new Error(
    "cannot safely launch cursor-agent on Windows: point CURSOR_AGENT_CMD at cursor-agent.exe, " +
      "or install the standard bundle containing node.exe and index.js"
  );
}

let agentLaunch;
try { agentLaunch = resolveAgentLaunch(); } catch (e) {
  process.stderr.write(`[cursor-adapter] ${e.message}\n`);
  process.exit(1);
}

function log(msg) {
  process.stderr.write(`[cursor-adapter] ${msg}\n`);
}

// ---- upstream: official cursor-agent acp -----------------------------------

const upstream = spawn(agentLaunch.command, [...agentLaunch.prefix, "acp"], {
  stdio: ["pipe", "pipe", "inherit"],
});

upstream.on("exit", (code) => {
  log(`upstream cursor-agent exited (${code}); shutting down`);
  shutdown(code ?? 0);
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

function isAcpSession(id) {
  if (!/^[0-9a-f-]{36}$/i.test(id)) return false;
  const dir = resolve(ACP_SESSIONS_DIR, id);
  return dirname(dir) === resolve(ACP_SESSIONS_DIR) && existsSync(join(dir, "store.db"));
}

/** Find all CLI chat directories for a chat id across workspace hashes. */
function cliChatDirs(id) {
  if (!/^[0-9a-f-]{36}$/i.test(id)) return [];
  let hashes = [];
  try { hashes = readdirSync(CHATS_DIR); } catch { return []; }
  const found = [];
  for (const h of hashes) {
    const dir = join(CHATS_DIR, h, id);
    if (!existsSync(join(dir, "store.db"))) continue;
    found.push(dir);
  }
  return found;
}

function cliChatDir(id) {
  const matches = cliChatDirs(id);
  return matches.length === 1 ? matches[0] : null;
}

function openIdeDb() {
  const db = new DatabaseSync(IDE_DB_PATH, { readOnly: true });
  db.exec("PRAGMA busy_timeout=3000");
  return db;
}

function ideComposerRaw(db, id) {
  const row = db.prepare("SELECT value FROM cursorDiskKV WHERE key = ?").get("composerData:" + id);
  if (!row) return null;
  try { return JSON.parse(String(row.value)); } catch { return null; }
}

function isIdeSession(id) {
  if (!/^[0-9a-f-]{36}$/i.test(id)) return false;
  try {
    const db = openIdeDb();
    const row = db.prepare("SELECT 1 FROM cursorDiskKV WHERE key = ?").get("composerData:" + id);
    db.close();
    return !!row;
  } catch {
    return false;
  }
}

// Routing precedence: acp (upstream) > cli > ide. An id polluted into two
// spaces (e.g. by a past `--resume <ide-id>` mistake) resolves to the space
// that can actually serve it.
function classify(id) {
  if (typeof id !== "string" || !id) return "acp";
  if (isAcpSession(id)) return "acp";
  const cliMatches = cliChatDirs(id);
  if (cliMatches.length === 1) return "cli";
  if (cliMatches.length > 1) return "cli-ambiguous";
  if (isIdeSession(id)) return "ide";
  return "acp"; // unknown ids go upstream, which owns the authoritative error
}

// ---- cli chat store (read-only) ----------------------------------------------

function readCliChatMeta(dir) {
  const out = { title: null, updatedAt: null, cwd: null, mode: null };
  try {
    const meta = JSON.parse(readFileSync(join(dir, "meta.json"), "utf8"));
    if (meta.updatedAtMs) out.updatedAt = new Date(meta.updatedAtMs).toISOString();
    else if (meta.createdAtMs) out.updatedAt = new Date(meta.createdAtMs).toISOString();
  } catch {}
  try {
    const db = new DatabaseSync(join(dir, "store.db"), { readOnly: true });
    db.exec("PRAGMA busy_timeout=3000");
    const row = db.prepare("SELECT value FROM meta LIMIT 1").get();
    if (row) {
      try {
        const decoded = JSON.parse(Buffer.from(String(row.value), "hex").toString("utf8"));
        if (decoded.name) out.title = decoded.name;
        if (decoded.mode) out.mode = decoded.mode;
      } catch {}
    }
    // Workspace path lives in the injected <user_info> context of the first blobs.
    const blobs = db.prepare("SELECT data FROM blobs ORDER BY rowid LIMIT 8").all();
    for (const b of blobs) {
      let rec;
      try { rec = JSON.parse(Buffer.from(b.data).toString("utf8")); } catch { continue; }
      const text = extractTextBlocks(rec?.content);
      const m = text.match(/Workspace Path: (.+)/);
      if (m) { out.cwd = m[1].trim(); break; }
    }
    db.close();
  } catch {}
  return out;
}

function listCliChats() {
  const sessions = [];
  let hashes = [];
  try { hashes = readdirSync(CHATS_DIR); } catch { return sessions; }
  for (const h of hashes) {
    let ids = [];
    try { ids = readdirSync(join(CHATS_DIR, h)); } catch { continue; }
    for (const id of ids) {
      const dir = join(CHATS_DIR, h, id);
      if (!existsSync(join(dir, "store.db"))) continue;
      const meta = readCliChatMeta(dir);
      sessions.push({
        sessionId: id,
        cwd: meta.cwd || homedir(),
        title: `[cli] ${meta.title || "CLI Chat"}`,
        updatedAt: meta.updatedAt,
        _meta: { "cursor-adapter": { space: "cli" } },
      });
    }
  }
  const counts = new Map();
  for (const session of sessions) counts.set(session.sessionId, (counts.get(session.sessionId) || 0) + 1);
  // Duplicate ids point at different cwd buckets and cannot be resumed safely.
  return sessions.filter((session) => counts.get(session.sessionId) === 1);
}

// ACP sessions share the CLI chat on-disk format (meta.json + store.db), so we
// can enumerate and replay them locally too. Upstream `cursor-agent acp
// session/list` is unreliable for on-disk ACP sessions (Cursor forum #158388 /
// Zed #56246: it often returns an empty list even though
// `~/.cursor/acp-sessions/<id>/` holds real sessions — verified 2026-07-09:
// 5 on disk, upstream returned 0), and upstream `session/load` requires auth
// even though agentCapabilities advertises no `authentication` field. Local
// enumeration + replay makes ACP sessions always discoverable/viewable for
// search/list/load without auth, matching how we already cover the cli and ide
// spaces. session/prompt for ACP ids still forwards to upstream (live
// continuation, needs auth).
function readAcpSessionMeta(dir) {
  const out = { title: null, updatedAt: null, cwd: null };
  let metaJson = null;
  try { metaJson = JSON.parse(readFileSync(join(dir, "meta.json"), "utf8")); } catch {}
  if (metaJson && typeof metaJson === "object") {
    if (typeof metaJson.cwd === "string" && metaJson.cwd) out.cwd = metaJson.cwd;
    if (typeof metaJson.title === "string" && metaJson.title) out.title = metaJson.title;
  }
  try {
    const db = new DatabaseSync(join(dir, "store.db"), { readOnly: true });
    db.exec("PRAGMA busy_timeout=3000");
    const row = db.prepare("SELECT value FROM meta LIMIT 1").get();
    if (row) {
      try {
        const decoded = JSON.parse(Buffer.from(String(row.value), "hex").toString("utf8"));
        if (!out.title && decoded.name) out.title = decoded.name;
        if (decoded.createdAt) out.updatedAt = new Date(decoded.createdAt).toISOString();
      } catch {}
    }
    if (!out.cwd) {
      const blobs = db.prepare("SELECT data FROM blobs ORDER BY rowid LIMIT 8").all();
      for (const b of blobs) {
        let rec;
        try { rec = JSON.parse(Buffer.from(b.data).toString("utf8")); } catch { continue; }
        const text = extractTextBlocks(rec?.content);
        const m = text.match(/Workspace Path: (.+)/);
        if (m) { out.cwd = m[1].trim(); break; }
      }
    }
    db.close();
  } catch {}
  return out;
}

function listAcpSessionsLocal() {
  const sessions = [];
  let ids = [];
  try { ids = readdirSync(ACP_SESSIONS_DIR); } catch { return sessions; }
  for (const id of ids) {
    if (!/^[0-9a-f-]{36}$/i.test(id)) continue;
    const dir = join(ACP_SESSIONS_DIR, id);
    if (!existsSync(join(dir, "store.db"))) continue;
    const meta = readAcpSessionMeta(dir);
    sessions.push({
      sessionId: id,
      cwd: meta.cwd || homedir(),
      title: meta.title || "ACP Session",
      updatedAt: meta.updatedAt,
      _meta: { "cursor-adapter": { space: "acp" } },
    });
  }
  return sessions;
}

function extractTextBlocks(content) {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    let t = "";
    for (const c of content) if (c && c.type === "text" && c.text) t += c.text;
    return t;
  }
  return "";
}

function cliChatMessages(dir) {
  const msgs = [];
  const db = new DatabaseSync(join(dir, "store.db"), { readOnly: true });
  db.exec("PRAGMA busy_timeout=3000");
  const rows = db.prepare("SELECT data FROM blobs ORDER BY rowid").all();
  db.close();
  for (const r of rows) {
    let rec;
    try { rec = JSON.parse(Buffer.from(r.data).toString("utf8")); } catch { continue; }
    if (!rec || (rec.role !== "user" && rec.role !== "assistant")) continue;
    let text = extractTextBlocks(rec.content);
    if (!text.trim()) continue;
    if (rec.role === "user") {
      if (text.startsWith("<user_info>")) continue; // injected env context, not a user turn
      const q = text.match(/<user_query>\n?([\s\S]*?)\n?<\/user_query>/);
      if (q) text = q[1];
    }
    msgs.push({ role: rec.role, text });
  }
  return msgs;
}

// ---- ide store (read-only) ----------------------------------------------------

function listIdeSessions() {
  const sessions = [];
  let db;
  try { db = openIdeDb(); } catch (e) { log(`ide db unavailable: ${e.message}`); return sessions; }
  try {
    const rows = db.prepare("SELECT key, value FROM cursorDiskKV WHERE key LIKE 'composerData:%'").all();
    for (const row of rows) {
      let d;
      try { d = JSON.parse(String(row.value)); } catch { continue; }
      if (!d || typeof d !== "object") continue;
      const id = String(row.key).slice("composerData:".length);
      const ts = d.lastUpdatedAt || d.createdAt;
      sessions.push({
        sessionId: id,
        cwd: typeof d.cwd === "string" && d.cwd ? d.cwd : homedir(),
        title: `[ide] ${(d.name || d.text || "").slice(0, 120) || "IDE Chat"}`,
        updatedAt: ts ? new Date(ts).toISOString() : null,
        _meta: { "cursor-adapter": { space: "ide" } },
      });
    }
  } finally {
    db.close();
  }
  return sessions;
}

function bubbleText(b) {
  if (b.text) return b.text;
  let t = "";
  const rich = b.richText && b.richText.content;
  if (Array.isArray(rich)) {
    for (const block of rich) {
      if (Array.isArray(block.content)) {
        for (const seg of block.content) if (seg.text) t += seg.text;
      }
    }
  }
  return t;
}

function ideChatMessages(id) {
  const db = openIdeDb();
  try {
    const composer = ideComposerRaw(db, id);
    if (!composer) return null;
    const getBubble = db.prepare("SELECT value FROM cursorDiskKV WHERE key = ?");
    let bubbles = [];
    const headers = Array.isArray(composer.fullConversationHeadersOnly)
      ? composer.fullConversationHeadersOnly
      : [];
    if (headers.length > 0) {
      for (const h of headers) {
        if (!h || !h.bubbleId) continue;
        const row = getBubble.get(`bubbleId:${id}:${h.bubbleId}`);
        if (!row) continue;
        try { bubbles.push(JSON.parse(String(row.value))); } catch {}
      }
    } else {
      const rows = db
        .prepare("SELECT value FROM cursorDiskKV WHERE key LIKE ? ESCAPE '\\'")
        .all(`bubbleId:${id}:%`);
      for (const r of rows) {
        try { bubbles.push(JSON.parse(String(r.value))); } catch {}
      }
      bubbles.sort((a, b) => String(a.createdAt || "").localeCompare(String(b.createdAt || "")));
    }
    const msgs = [];
    for (const b of bubbles) {
      const text = bubbleText(b);
      if (!text || !text.trim()) continue;
      msgs.push({ role: b.type === 1 ? "user" : "assistant", text });
    }
    return msgs;
  } finally {
    db.close();
  }
}

// ---- extended handlers ---------------------------------------------------------

const loadCwd = new Map(); // sessionId -> cwd supplied by the client at load time
const localOnlyAcpLoads = new Set(); // upstream load failed; history replayed read-only

function handleLocalLoad(msg, space) {
  const sid = msg.params.sessionId;
  if (msg.params.cwd) loadCwd.set(sid, msg.params.cwd);
  let msgs;
  try {
    if (space === "ide") {
      msgs = ideChatMessages(sid);
    } else {
      // acp and cli share the same on-disk chat format (meta.json + store.db
      // with role/content blobs), so both replay via cliChatMessages.
      const dir = space === "acp" && isAcpSession(sid) ? resolve(ACP_SESSIONS_DIR, sid) : cliChatDir(sid);
      if (!dir || !existsSync(dir)) return respondError(msg.id, -32002, `Session not found: ${sid}`);
      msgs = cliChatMessages(dir);
    }
  } catch (e) {
    return respondError(msg.id, -32603, `failed to read ${space} chat: ${e.message}`);
  }
  if (!msgs) return respondError(msg.id, -32002, `Session not found: ${sid}`);
  for (const m of msgs) {
    notifyUpdate(sid, chunkUpdate(m.role === "user" ? "user_message_chunk" : "agent_message_chunk", m.text));
  }
  respond(msg.id, {});
}

const runningPrompts = new Map(); // sessionId -> child process
let shuttingDown = false;

function terminatePrompt(child) {
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
  try { upstream.kill(); } catch {}
  process.exit(code);
}

function handleCliPrompt(msg) {
  const sid = msg.params.sessionId;
  const text = extractTextBlocks(
    Array.isArray(msg.params.prompt) ? msg.params.prompt : []
  );
  if (!text.trim()) return respondError(msg.id, -32602, "Empty prompt (only text blocks are supported for cli chats)");

  const dir = cliChatDir(sid);
  if (!dir) return respondError(msg.id, -32002, `Session not found: ${sid}`);
  // CRITICAL: `--resume` only finds the chat when run from the chat's own
  // workspace — chats live under ~/.cursor/chats/<md5(workspacePath)>/.
  // Resuming from any other cwd silently creates a NEW empty chat with the
  // same id in another hash bucket (verified 2026-07-05). So the cwd MUST
  // hash to this chat's bucket, or we refuse rather than fork the chat.
  const bucket = basename(dirname(dir));
  const candidates = [readCliChatMeta(dir).cwd, loadCwd.get(sid), process.cwd()].filter(Boolean);
  const cwd = candidates.find(
    (c) => createHash("md5").update(c, "utf8").digest("hex") === bucket
  );
  if (!cwd) {
    return respondError(
      msg.id,
      -32603,
      `cannot resolve the original workspace for cli chat ${sid} (bucket ${bucket}); ` +
        `refusing to resume from a different cwd because cursor-agent would silently create a new unrelated chat`
    );
  }
  if (!existsSync(cwd)) {
    return respondError(msg.id, -32603, `original workspace for cli chat ${sid} no longer exists: ${cwd}`);
  }

  // Imported CLI chats cannot relay Cursor's headless permission prompts back
  // through ACP. Resume them in read-only ask mode instead of bypassing the
  // Hub's permission policy with --trust/--force-style flags.
  if (!agentLaunch.nodeHosted) {
    return respondError(
      msg.id,
      -32603,
      "Safe CLI resume requires a Node-hosted cursor-agent bundle; set CURSOR_AGENT_CMD to node and CURSOR_AGENT_SCRIPT to index.js, or use a live ACP session"
    );
  }
  const args = ["--resume", sid, "--mode", "ask", "-p", "--output-format", "stream-json", "--stream-partial-output"];
  // cursor-agent requires the prompt as a positional argument. Read it over
  // stdin in a tiny Node bootstrap, then set the child process's in-memory
  // argv before loading index.js. The prompt never enters the OS command line.
  const bootstrap = `let p="";process.stdin.setEncoding("utf8");process.stdin.on("data",c=>p+=c);process.stdin.on("end",()=>{const [s,...a]=process.argv.slice(1);process.argv=[process.execPath,s,...a,p];require("module").runMain()})`;
  const child = spawn(agentLaunch.command, ["-e", bootstrap, ...agentLaunch.prefix, ...args], {
    stdio: ["pipe", "pipe", "inherit"],
    cwd,
    detached: !IS_WIN,
    windowsHide: true,
  });
  runningPrompts.set(sid, child);

  let streamedChunks = 0;
  let resultText = null;
  let isError = false;
  let stdinError = null;
  let cancelled = false;
  child.cancelPrompt = () => { cancelled = true; terminatePrompt(child); };
  child.stdin.on("error", (e) => { stdinError = e; });

  const rl = createInterface({ input: child.stdout });
  rl.on("line", (line) => {
    let ev;
    try { ev = JSON.parse(line); } catch { return; }
    if (ev.type === "assistant" && ev.timestamp_ms !== undefined) {
      const t = extractTextBlocks(ev.message?.content);
      if (t) { streamedChunks++; notifyUpdate(sid, chunkUpdate("agent_message_chunk", t)); }
    } else if (ev.type === "result") {
      resultText = typeof ev.result === "string" ? ev.result : null;
      isError = !!ev.is_error;
    }
  });

  child.on("exit", (code) => {
    runningPrompts.delete(sid);
    // stream-json without partial deltas (or format drift): fall back to the
    // final result payload so the reply is never lost.
    if (!cancelled && streamedChunks === 0 && resultText) {
      notifyUpdate(sid, chunkUpdate("agent_message_chunk", resultText));
    }
    if (cancelled) return respond(msg.id, { stopReason: "cancelled" });
    if (code !== 0 || isError || stdinError) {
      return respondError(
        msg.id,
        -32603,
        `cursor-agent headless run failed (exit ${code})${stdinError ? ": stdin " + stdinError.message : resultText ? ": " + resultText : ""}`
      );
    }
    respond(msg.id, { stopReason: "end_turn" });
  });
  child.on("error", (e) => {
    runningPrompts.delete(sid);
    respondError(msg.id, -32603, `failed to spawn cursor-agent: ${e.message}`);
  });
  child.stdin.write(text);
  child.stdin.end();
}

// ---- session/list merging --------------------------------------------------------

const pendingListMerges = new Map(); // request id -> { firstPage }
const pendingAcpLoads = new Map(); // request id -> original session/load request

function mergedLocalSessions(existingIds) {
  const extra = [];
  // ACP first (no title prefix → treated as acp space), then cli/ide (prefixed).
  // Dedup by sessionId so sessions upstream already listed are not duplicated.
  for (const s of [...listAcpSessionsLocal(), ...listCliChats(), ...listIdeSessions()]) {
    if (!existingIds.has(s.sessionId)) {
      existingIds.add(s.sessionId);
      extra.push(s);
    }
  }
  return extra;
}

// ---- main routing ------------------------------------------------------------------

const clientIn = createInterface({ input: process.stdin });
clientIn.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }

  // Client responses to upstream-initiated requests (permission, cursor/*).
  if (msg.method === undefined) {
    toUpstream(msg);
    return;
  }

  const sid = msg.params?.sessionId;

  switch (msg.method) {
    case "session/list":
      pendingListMerges.set(msg.id, { firstPage: !msg.params?.cursor });
      toUpstream(msg);
      return;
    case "session/load": {
      const space = classify(sid);
      if (space === "cli-ambiguous") {
        return respondError(msg.id, -32602, `Ambiguous CLI session id ${sid}: found in multiple workspace buckets`);
      }
      if (space === "cli" || space === "ide") return handleLocalLoad(msg, space);
      // Give upstream the first chance to attach an on-disk ACP session so a
      // later prompt continues the same live context. If upstream load fails,
      // the response handler below falls back to a local read-only replay.
      if (space === "acp" && isAcpSession(sid)) pendingAcpLoads.set(msg.id, msg);
      toUpstream(msg);
      return;
    }
    case "session/prompt": {
      const space = classify(sid);
      if (space === "cli-ambiguous") {
        return respondError(msg.id, -32602, `Ambiguous CLI session id ${sid}: found in multiple workspace buckets`);
      }
      if (space === "cli") return handleCliPrompt(msg);
      if (space === "acp" && localOnlyAcpLoads.has(sid)) {
        return respondError(
          msg.id,
          -32602,
          "This ACP session was replayed locally because cursor-agent could not load it upstream; it is read-only in this connection. Authenticate cursor-agent and load it again before prompting."
        );
      }
      if (space === "ide") {
        return respondError(
          msg.id,
          -32602,
          "IDE desktop chats are read-only through this adapter: `cursor-agent --resume <ide-id>` does not " +
            "continue the IDE conversation — it silently creates a new, unrelated CLI chat (verified 2026-07-05). " +
            "Use session/load to view history."
        );
      }
      toUpstream(msg);
      return;
    }
    case "session/set_mode":
    case "session/set_config_option": {
      const space = classify(sid);
      if (space === "cli" || space === "cli-ambiguous" || space === "ide") {
        return respondError(msg.id, -32602, `${msg.method} is not supported for ${space} chats`);
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
  shutdown(0);
});

process.on("SIGINT", () => shutdown(130));
process.on("SIGTERM", () => shutdown(143));

const upstreamOut = createInterface({ input: upstream.stdout });
upstreamOut.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }

  if (msg.id !== undefined && pendingAcpLoads.has(msg.id) && msg.method === undefined) {
    const original = pendingAcpLoads.get(msg.id);
    pendingAcpLoads.delete(msg.id);
    if (msg.error) {
      localOnlyAcpLoads.add(original.params.sessionId);
      handleLocalLoad(original, "acp");
      return;
    }
    localOnlyAcpLoads.delete(original.params.sessionId);
    toClient(msg);
    return;
  }

  if (msg.id !== undefined && pendingListMerges.has(msg.id) && msg.method === undefined) {
    const { firstPage } = pendingListMerges.get(msg.id);
    pendingListMerges.delete(msg.id);
    if (msg.error) {
      // Upstream list failed — still serve the local spaces.
      respond(msg.id, { sessions: mergedLocalSessions(new Set()), nextCursor: null });
      return;
    }
    const sessions = Array.isArray(msg.result?.sessions) ? msg.result.sessions : [];
    // The Hub may consume only the first page. Put local discoveries on that
    // page even when upstream advertises a nextCursor.
    if (firstPage) {
      const seen = new Set(sessions.map((s) => s.sessionId));
      msg.result.sessions = [...sessions, ...mergedLocalSessions(seen)];
    }
    toClient(msg);
    return;
  }

  toClient(msg);
});

log(`ready (upstream: ${agentLaunch.command} ${agentLaunch.prefix.join(" ")} acp; chats: ${CHATS_DIR}; ide db: ${IDE_DB_PATH})`);
