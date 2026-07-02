const fs = require("fs");
const readline = require("readline");

const DB_PATH = process.env.CURSOR_DB_PATH ||
  (process.env.APPDATA || "") + "\\Cursor\\User\\globalStorage\\state.vscdb";

function send(msg) { process.stdout.write(JSON.stringify(msg) + "\n"); }

function openDB() {
  try {
    const { DatabaseSync } = require("node:sqlite");
    return new DatabaseSync(DB_PATH, { readOnly: true });
  } catch(e) {
    process.stderr.write("DB open failed: " + e.message + "\n");
    return null;
  }
}

function getAllSessions(db) {
  const rows = db.prepare("SELECT key, value FROM cursorDiskKV WHERE key LIKE ?").all("composerData:%");
  return rows.map(row => {
    try {
      const d = JSON.parse(row.value);
      const id = row.key.replace("composerData:", "");
      const s = {
        sessionId: id,
        cwd: d.cwd || process.cwd(),
        additionalDirectories: [],
      };
      if (d.text) s.title = d.text;
      const ts = d.lastUpdatedAt || d.createdAt;
      if (ts) s.updatedAt = new Date(ts).toISOString();
      return s;
    } catch { return null; }
  }).filter(Boolean);
}

function getSessionMessages(db, sessionId) {
  // Read all bubbles for this session from bubbleId:<sessionId>:* keys.
  const rows = db.prepare("SELECT value FROM cursorDiskKV WHERE key LIKE ?")
    .all("bubbleId:" + sessionId + ":%");
  const bubbles = rows
    .map(r => {
      try { return JSON.parse(r.value); } catch { return null; }
    })
    .filter(Boolean)
    .sort((a, b) => (a.createdAt || "").localeCompare(b.createdAt || ""));

  const msgs = [];
  for (const b of bubbles) {
    // type 1 = user, type 2 = assistant
    const role = b.type === 1 ? "user" : "assistant";
    let text = b.text || "";
    // Fallback: extract from richText if text is empty
    if (!text && b.richText && b.richText.content) {
      for (const block of b.richText.content)
        if (block.content)
          for (const seg of block.content)
            if (seg.text) text += seg.text;
    }
    if (text && text.trim()) msgs.push({ role, text: text.trim() });
  }
  return msgs;
}

const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.method === "initialize") {
    send({ jsonrpc: "2.0", id: msg.id, result: {
      protocolVersion: 1,
      agentCapabilities: { loadSession: true, sessionCapabilities: { list: {}, close: {}, delete: {}, resume: { mode: "none" } } },
      authMethods: [],
    }});
  } else if (msg.method === "session/list") {
    const db = openDB();
    if (!db) return send({ jsonrpc: "2.0", id: msg.id, error: { code: -32603, message: "Cannot open Cursor DB" } });
    const sessions = getAllSessions(db);
    db.close();
    send({ jsonrpc: "2.0", id: msg.id, result: { sessions, nextCursor: null } });
  } else if (msg.method === "session/load") {
    const sid = msg.params.sessionId;
    const db = openDB();
    if (!db) return send({ jsonrpc: "2.0", id: msg.id, error: { code: -32603, message: "Cannot open Cursor DB" } });
    const msgs = getSessionMessages(db, sid);
    db.close();
    for (const m of msgs) {
      send({ jsonrpc: "2.0", method: "session/update", params: { sessionId: sid, update: {
        sessionUpdate: m.role === "user" ? "user_message_chunk" : "agent_message_chunk",
        content: { type: "text", text: m.text },
      }}});
    }
    send({ jsonrpc: "2.0", id: msg.id, result: {
      sessionId: sid, modes: [{ id: "default", name: "Default", active: true }], configOptions: [],
    }});
  } else if (msg.id) {
    send({ jsonrpc: "2.0", id: msg.id, result: {} });
  }
});

process.stderr.write("Cursor history adapter ready. DB: " + DB_PATH + "\n");
