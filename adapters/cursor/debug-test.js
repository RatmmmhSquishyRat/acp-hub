const { DatabaseSync } = require("node:sqlite");
const fs = require("fs");
const LOG = "/tmp/cursor-adapter-debug.log";
function log(msg) { fs.appendFileSync(LOG, new Date().toISOString() + " " + msg + "\n"); }
log("ADAPTER STARTED");
log("DB_PATH env: " + (process.env.CURSOR_DB_PATH || "not set"));
log("APPDATA env: " + (process.env.APPDATA || "not set"));
const dbPath = process.env.CURSOR_DB_PATH || (process.env.APPDATA + "\\Cursor\\User\\globalStorage\\state.vscdb");
log("Resolved DB path: " + dbPath);
try {
  const db = new DatabaseSync(dbPath, { readOnly: true });
  const count = db.prepare("SELECT COUNT(*) as c FROM cursorDiskKV WHERE key LIKE ?").get("composerData:%");
  log("DB opened. composerData count: " + count.c);
  db.close();
} catch(e) {
  log("DB ERROR: " + e.message);
}
const readline = require("readline");
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  log("RECV: " + line.slice(0, 200));
  let msg;
  try { msg = JSON.parse(line); } catch { log("PARSE FAIL"); return; }
  if (msg.method === "initialize") {
    const resp = {jsonrpc:"2.0",id:msg.id,result:{protocolVersion:1,agentCapabilities:{loadSession:true,sessionCapabilities:{list:{}}}}};
    const out = JSON.stringify(resp);
    log("SEND init: " + out.slice(0, 200));
    process.stdout.write(out + "\n");
  } else if (msg.method === "session/list") {
    const db = new DatabaseSync(dbPath, { readOnly: true });
    const rows = db.prepare("SELECT key FROM cursorDiskKV WHERE key LIKE ?").all("composerData:%");
    const sessions = rows.slice(0, 10).map(r => ({sessionId: r.key.replace("composerData:",""), title: null}));
    const resp = {jsonrpc:"2.0",id:msg.id,result:{sessions, nextCursor: null}};
    const out = JSON.stringify(resp);
    log("SEND list: " + out.slice(0, 200) + " ... (" + sessions.length + " sessions)");
    process.stdout.write(out + "\n");
    db.close();
  } else {
    log("UNKNOWN METHOD: " + msg.method);
  }
});
