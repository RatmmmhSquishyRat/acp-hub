#!/usr/bin/env node
/**
 * Cursor ACP Adapter — bridges Cursor IDE's internal conversation storage
 * to the ACP protocol so ACP Hub can CRUD ALL Cursor conversations.
 *
 * Spec 1: "为某个agent client开发一个ACP adapter程序并注册"
 *
 * Reads from: %APPDATA%/Cursor/User/globalStorage/state.vscdb
 * Table: cursorDiskKV, keys: composerData:<uuid>
 *
 * Implements: initialize, session/new (no-op), session/list, session/load,
 *             session/prompt (error: read-only adapter)
 */

import { DatabaseSync } from "node:sqlite";
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { join } from "node:path";

const DB_PATH = process.env.CURSOR_DB_PATH ||
  join(process.env.APPDATA || "", "Cursor", "User", "globalStorage", "state.vscdb");

// ---- ACP JSON-RPC over stdio ----

function send(msg) {
  process.stdout.write(JSON.stringify(msg) + "\n");
}

function sendNotification(method, params) {
  send({ jsonrpc: "2.0", method, params });
}

function sendResponse(id, result) {
  send({ jsonrpc: "2.0", id, result });
}

function sendError(id, code, message, data) {
  send({ jsonrpc: "2.0", id, error: { code, message, data } });
}

// ---- Cursor data access ----

function openDB() {
  try {
    return new DatabaseSync(DB_PATH, { readOnly: true });
  } catch (e) {
    process.stderr.write(`Failed to open Cursor DB at ${DB_PATH}: ${e.message}\n`);
    return null;
  }
}

function getAllConversations(db) {
  const rows = db.prepare(
    "SELECT key, value FROM cursorDiskKV WHERE key LIKE ?"
  ).all("composerData:%");
  return rows.map(row => {
    try {
      const data = JSON.parse(row.value);
      const id = row.key.replace("composerData:", "");
      return {
        sessionId: id,
        title: data.text || "",
        createdAt: data.createdAt ? new Date(data.createdAt).toISOString() : null,
        updatedAt: data.lastUpdatedAt ? new Date(data.lastUpdatedAt).toISOString() : null,
        rawData: data,
      };
    } catch {
      return null;
    }
  }).filter(Boolean);
}

function getConversationMessages(db, sessionId) {
  const row = db.prepare(
    "SELECT value FROM cursorDiskKV WHERE key = ?"
  ).get(`composerData:${sessionId}`);
  if (!row) return [];

  const data = JSON.parse(row.value);
  const messages = [];

  // Cursor stores messages in conversationMap (object keyed by bubble ID).
  const convMap = data.conversationMap || data.conversationState?.conversationMap || {};

  // Sort by timestamp if available.
  const bubbleIds = Object.keys(convMap);
  const sortedBubbles = bubbleIds
    .map(id => ({ id, ...convMap[id] }))
    .sort((a, b) => (a.createdAt || 0) - (b.createdAt || 0));

  for (const bubble of sortedBubbles) {
    const role = bubble.type === 1 ? "assistant" : "user";
    // Extract text content from richText or text fields.
    let text = "";
    if (bubble.text) {
      text = bubble.text;
    } else if (bubble.richText?.content) {
      // Extract plain text from rich text content blocks.
      for (const block of bubble.richText.content) {
        if (block.content) {
          for (const inline of block.content) {
            if (inline.text) text += inline.text;
          }
        }
      }
    }

    if (text.trim()) {
      messages.push({
        role,
        text: text.trim(),
        createdAt: bubble.createdAt ? new Date(bubble.createdAt).toISOString() : null,
      });
    }
  }

  return messages;
}

// ---- ACP Protocol Handlers ----

function handleInitialize(id, params) {
  sendResponse(id, {
    protocolVersion: 1,
    agentCapabilities: {
      loadSession: true,
      sessionCapabilities: {
        list: {},
        resume: { mode: "none" },
        close: {},
        delete: {},
      },
      promptCapabilities: { image: false, audio: false },
    },
    authMethods: [],
  });
}

function handleSessionList(id, params) {
  const db = openDB();
  if (!db) {
    sendError(id, -32603, "Cannot open Cursor database");
    return;
  }

  const conversations = getAllConversations(db);
  db.close();

  // Map to ACP SessionInfo format.
  const sessions = conversations.map(c => ({
    sessionId: c.sessionId,
    title: c.title || null,
    cwd: null,
    additionalDirectories: [],
    updatedAt: c.updatedAt || c.createdAt,
    meta: {},
  }));

  sendResponse(id, {
    sessions,
    nextCursor: null, // All sessions returned in one page.
  });
}

function handleSessionLoad(id, params) {
  const sessionId = params.sessionId;
  const db = openDB();
  if (!db) {
    sendError(id, -32603, "Cannot open Cursor database");
    return;
  }

  const messages = getConversationMessages(db, sessionId);
  db.close();

  // Replay messages as session/update notifications.
  for (const msg of messages) {
    if (msg.role === "user") {
      sendNotification("session/update", {
        sessionId,
        update: {
          type: "user_message_chunk",
          content: { type: "text", text: msg.text },
        },
      });
    } else {
      sendNotification("session/update", {
        sessionId,
        update: {
          type: "agent_message_chunk",
          content: { type: "text", text: msg.text },
        },
      });
    }
  }

  sendResponse(id, {
    sessionId,
    modes: [{ id: "default", name: "Default", active: true }],
    configOptions: [],
  });
}

function handleSessionNew(id, params) {
  sendError(id, -32601, "This is a read-only Cursor history adapter. Use the official cursor-agent for new conversations.");
}

function handleSessionPrompt(id, params) {
  sendError(id, -32601, "This is a read-only Cursor history adapter.");
}

// ---- Main loop ----

const rl = createInterface({ input: process.stdin });

rl.on("line", (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }

  if (msg.method === "initialize") {
    handleInitialize(msg.id, msg.params);
  } else if (msg.method === "notifications/initialized") {
    // No-op.
  } else if (msg.method === "session/new") {
    handleSessionNew(msg.id, msg.params);
  } else if (msg.method === "session/load") {
    handleSessionLoad(msg.id, msg.params);
  } else if (msg.method === "session/list") {
    handleSessionList(msg.id, msg.params);
  } else if (msg.method === "session/prompt") {
    handleSessionPrompt(msg.id, msg.params);
  } else if (msg.method === "session/cancel") {
    sendResponse(msg.id, {});
  } else if (msg.id) {
    sendError(msg.id, -32601, `Method not found: ${msg.method}`);
  }
});

process.stderr.write(`Cursor ACP adapter ready. DB: ${DB_PATH}\n`);
