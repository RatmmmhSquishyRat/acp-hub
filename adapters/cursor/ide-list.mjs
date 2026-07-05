// List IDE composer sessions from Cursor desktop state.vscdb (read-only).
import { DatabaseSync } from "node:sqlite";
import { join } from "node:path";

const DB = process.env.CURSOR_DB_PATH ||
  join(process.env.APPDATA || "", "Cursor", "User", "globalStorage", "state.vscdb");

const db = new DatabaseSync(DB, { readOnly: true });
const rows = db.prepare("SELECT key, value FROM cursorDiskKV WHERE key LIKE 'composerData:%'").all();
const parsed = rows.map((r) => {
  try {
    const d = JSON.parse(r.value);
    return {
      id: String(r.key).slice("composerData:".length),
      title: (d.name || d.text || "").slice(0, 50),
      updated: d.lastUpdatedAt || d.createdAt || 0,
      headers: Array.isArray(d.fullConversationHeadersOnly) ? d.fullConversationHeadersOnly.length : 0,
    };
  } catch { return null; }
}).filter(Boolean).sort((a, b) => b.updated - a.updated);

console.log("total composers:", rows.length);
for (const p of parsed.slice(0, 8)) {
  console.log(`${p.id} | ${new Date(p.updated).toISOString()} | msgs:${p.headers} | ${p.title}`);
}
db.close();
