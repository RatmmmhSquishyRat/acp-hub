// Manual compatibility probe for adapter.mjs.
//
// By default this script creates an isolated synthetic Cursor home and mock ACP
// upstream. It cannot read or modify the user's Cursor data.
//
// Installed-agent compatibility is opt-in: set ACP_ADAPTER_LIVE_TESTS=1 and
// provide explicit ids. CLI resume appends to Cursor-managed session state, so
// live session/prompt is additionally gated by
// ACP_ADAPTER_DESTRUCTIVE_TESTS=1.
//
// PowerShell:
//   $env:ACP_ADAPTER_LIVE_TESTS='1'
//   node adapter-test.mjs <cliChatId> <ideComposerId>
//
// Destructive continuation probe:
//   $env:ACP_ADAPTER_DESTRUCTIVE_TESTS='1'
//   node adapter-test.mjs <cliChatId> <ideComposerId>

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import readline from "node:readline";
import { DatabaseSync } from "node:sqlite";

const live = process.env.ACP_ADAPTER_LIVE_TESTS === "1";
const destructive = process.env.ACP_ADAPTER_DESTRUCTIVE_TESTS === "1";
let [cliId, ideId] = process.argv.slice(2);
let corruptCliId = null;
let corruptIdeId = null;
let whitespaceCliId = null;
let emptyQueryCliId = null;
let malformedComposerId = null;
let invalidComposerRows = [];
let resultFailureText = null;
let fixtureRoot = null;
let adapterEnv = { ...process.env };

if (live) {
  if (!cliId || !ideId) {
    console.error("Usage: node adapter-test.mjs <cliChatId> <ideComposerId>");
    process.exit(2);
  }
} else {
  if (destructive) {
    console.error(
      "ACP_ADAPTER_DESTRUCTIVE_TESTS applies only with ACP_ADAPTER_LIVE_TESTS=1."
    );
    process.exit(2);
  }

  fixtureRoot = mkdtempSync(join(tmpdir(), "acp-hub-cursor-adapter-test-"));
  const cursorHome = join(fixtureRoot, "cursor-home");
  const workspace = join(fixtureRoot, "workspace");
  const ideDbPath = join(fixtureRoot, "state.vscdb");
  const mockAgentPath = join(fixtureRoot, "mock-cursor-agent.mjs");
  cliId = "11111111-1111-4111-8111-111111111111";
  ideId = "22222222-2222-4222-8222-222222222222";
  corruptCliId = "44444444-4444-4444-8444-444444444444";
  corruptIdeId = "55555555-5555-4555-8555-555555555555";
  whitespaceCliId = "66666666-6666-4666-8666-666666666666";
  emptyQueryCliId = "77777777-7777-4777-8777-777777777777";
  malformedComposerId = "88888888-8888-4888-8888-888888888888";
  invalidComposerRows = [
    {
      label: "JSON null",
      id: "99999999-9999-4999-8999-999999999999",
      value: "null",
    },
    {
      label: "JSON false",
      id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
      value: "false",
    },
    {
      label: "JSON zero",
      id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
      value: "0",
    },
    {
      label: "empty value",
      id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
      value: "",
    },
  ];
  resultFailureText = `private Cursor failure ${fixtureRoot} CURSOR_RESULT_PRIVATE_SENTINEL`;

  mkdirSync(workspace, { recursive: true });
  const bucket = createHash("md5").update(workspace, "utf8").digest("hex");
  const chatDir = join(cursorHome, "chats", bucket, cliId);
  mkdirSync(chatDir, { recursive: true });
  writeFileSync(
    join(chatDir, "meta.json"),
    JSON.stringify({ createdAtMs: 1, updatedAtMs: 2 }),
    "utf8"
  );
  const chatDb = new DatabaseSync(join(chatDir, "store.db"));
  chatDb.exec("CREATE TABLE meta(value TEXT); CREATE TABLE blobs(data BLOB)");
  chatDb
    .prepare("INSERT INTO meta(value) VALUES (?)")
    .run(Buffer.from(JSON.stringify({ name: "Fixture CLI", mode: "ask" })).toString("hex"));
  const insertBlob = chatDb.prepare("INSERT INTO blobs(data) VALUES (?)");
  insertBlob.run(
    Buffer.from(
      JSON.stringify({
        role: "user",
        content: `<user_info>\nWorkspace Path: ${workspace}\n</user_info>`,
      })
    )
  );
  insertBlob.run(Buffer.from(JSON.stringify({ role: "user", content: "fixture question" })));
  insertBlob.run(Buffer.from(JSON.stringify({ role: "assistant", content: "fixture answer" })));
  chatDb.close();

  const corruptDir = join(cursorHome, "chats", bucket, corruptCliId);
  mkdirSync(corruptDir, { recursive: true });
  writeFileSync(
    join(corruptDir, "meta.json"),
    JSON.stringify({ createdAtMs: 1, updatedAtMs: 2 }),
    "utf8"
  );
  const corruptDb = new DatabaseSync(join(corruptDir, "store.db"));
  corruptDb.exec("CREATE TABLE meta(value TEXT); CREATE TABLE blobs(data BLOB)");
  corruptDb
    .prepare("INSERT INTO blobs(data) VALUES (?)")
    .run(Buffer.from(JSON.stringify({ role: "user", content: "valid before corruption" })));
  corruptDb.prepare("INSERT INTO blobs(data) VALUES (?)").run(Buffer.from("{not-json"));
  corruptDb.close();

  const whitespaceDir = join(cursorHome, "chats", bucket, whitespaceCliId);
  mkdirSync(whitespaceDir, { recursive: true });
  writeFileSync(
    join(whitespaceDir, "meta.json"),
    JSON.stringify({ createdAtMs: 1, updatedAtMs: 2 }),
    "utf8"
  );
  const whitespaceDb = new DatabaseSync(join(whitespaceDir, "store.db"));
  whitespaceDb.exec("CREATE TABLE meta(value TEXT); CREATE TABLE blobs(data BLOB)");
  const insertWhitespace = whitespaceDb.prepare("INSERT INTO blobs(data) VALUES (?)");
  insertWhitespace.run(
    Buffer.from(JSON.stringify({ role: "user", content: "valid before whitespace" }))
  );
  insertWhitespace.run(Buffer.from(JSON.stringify({ role: "assistant", content: " \t" })));
  whitespaceDb.close();

  const emptyQueryDir = join(cursorHome, "chats", bucket, emptyQueryCliId);
  mkdirSync(emptyQueryDir, { recursive: true });
  writeFileSync(
    join(emptyQueryDir, "meta.json"),
    JSON.stringify({ createdAtMs: 1, updatedAtMs: 2 }),
    "utf8"
  );
  const emptyQueryDb = new DatabaseSync(join(emptyQueryDir, "store.db"));
  emptyQueryDb.exec("CREATE TABLE meta(value TEXT); CREATE TABLE blobs(data BLOB)");
  const insertEmptyQuery = emptyQueryDb.prepare("INSERT INTO blobs(data) VALUES (?)");
  insertEmptyQuery.run(
    Buffer.from(JSON.stringify({ role: "user", content: "valid before empty query" }))
  );
  insertEmptyQuery.run(
    Buffer.from(
      JSON.stringify({ role: "user", content: "<user_query>\n   \n</user_query>" })
    )
  );
  emptyQueryDb.close();

  const ideDb = new DatabaseSync(ideDbPath);
  ideDb.exec("CREATE TABLE cursorDiskKV(key TEXT PRIMARY KEY, value TEXT)");
  const insertIde = ideDb.prepare("INSERT INTO cursorDiskKV(key, value) VALUES (?, ?)");
  insertIde.run(
    `composerData:${ideId}`,
    JSON.stringify({
      name: "Fixture IDE",
      cwd: workspace,
      createdAt: 1,
      fullConversationHeadersOnly: [{ bubbleId: "u" }, { bubbleId: "a" }],
    })
  );
  insertIde.run(
    `bubbleId:${ideId}:u`,
    JSON.stringify({ type: 1, text: "fixture IDE question", createdAt: 1 })
  );
  insertIde.run(
    `bubbleId:${ideId}:a`,
    JSON.stringify({ type: 2, text: "fixture IDE answer", createdAt: 2 })
  );
  insertIde.run(
    `composerData:${corruptIdeId}`,
    JSON.stringify({
      name: "Corrupt Fixture IDE",
      cwd: workspace,
      createdAt: 1,
      fullConversationHeadersOnly: [{ bubbleId: "valid" }, { bubbleId: "missing" }],
    })
  );
  insertIde.run(
    `bubbleId:${corruptIdeId}:valid`,
    JSON.stringify({ type: 1, text: "valid before missing bubble", createdAt: 1 })
  );
  insertIde.run(`composerData:${malformedComposerId}`, "{not-json");
  for (const invalidComposer of invalidComposerRows) {
    insertIde.run(`composerData:${invalidComposer.id}`, invalidComposer.value);
  }
  ideDb.close();

  writeFileSync(
    mockAgentPath,
    `import readline from "node:readline";
const argv = process.argv.slice(2);
if (argv.includes("--resume")) {
  process.stdout.write(JSON.stringify({
    type: "result",
    result: process.env.CURSOR_RESULT_FAILURE_TEXT,
    is_error: true
  }) + "\\n");
  process.exitCode = 7;
} else {
  const input = readline.createInterface({ input: process.stdin });
  const send = (msg) => process.stdout.write(JSON.stringify(msg) + "\\n");
  input.on("line", (line) => {
    let msg;
    try { msg = JSON.parse(line); } catch { return; }
    if (msg.method === "initialize") {
      send({ jsonrpc: "2.0", id: msg.id, result: {
        protocolVersion: 1,
        agentCapabilities: { loadSession: true, sessionCapabilities: { list: {} } }
      } });
    } else if (msg.method === "session/list") {
      send({ jsonrpc: "2.0", id: msg.id, result: { sessions: [], nextCursor: null } });
    } else if (msg.id !== undefined) {
      send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "fixture method unavailable" } });
    }
  });
}
`,
    "utf8"
  );

  adapterEnv = {
    ...adapterEnv,
    CURSOR_HOME: cursorHome,
    CURSOR_DB_PATH: ideDbPath,
    CURSOR_AGENT_CMD: process.execPath,
    CURSOR_AGENT_SCRIPT: mockAgentPath,
    CURSOR_RESULT_FAILURE_TEXT: resultFailureText,
  };
}

const adapterPath = new URL("./adapter.mjs", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
const adapter = spawn(process.execPath, [adapterPath], {
  stdio: ["pipe", "pipe", "pipe"],
  windowsHide: true,
  env: adapterEnv,
});
let adapterDiagnostics = "";
adapter.stderr.on("data", (chunk) => {
  if (adapterDiagnostics.length < 16_384) adapterDiagnostics += String(chunk);
});

let nextId = 1;
const pending = new Map();
const updates = [];

function send(method, params) {
  const id = nextId++;
  adapter.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timeout ${method}`)), 120_000);
    pending.set(id, {
      resolve: (value) => { clearTimeout(timer); resolve(value); },
      reject: (error) => { clearTimeout(timer); reject(error); },
    });
  });
}

readline.createInterface({ input: adapter.stdout }).on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
    const waiter = pending.get(msg.id);
    if (waiter) {
      pending.delete(msg.id);
      msg.error ? waiter.reject(new Error(JSON.stringify(msg.error))) : waiter.resolve(msg.result);
      return;
    }
  }
  if (msg.method === "session/update") {
    updates.push(msg.params.update);
    return;
  }
  if (msg.id !== undefined && typeof msg.method === "string") {
    adapter.stdin.write(
      JSON.stringify({
        jsonrpc: "2.0",
        id: msg.id,
        error: { code: -32601, message: "Method not found" },
      }) + "\n"
    );
  }
});

function drainUpdates() {
  return updates.splice(0);
}

let failures = 0;
function check(label, ok, detail = "") {
  console.log(`${ok ? "PASS" : "FAIL"}  ${label}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures++;
}

try {
  const init = await send("initialize", {
    protocolVersion: 1,
    clientCapabilities: {
      fs: { readTextFile: false, writeTextFile: false },
      terminal: false,
    },
    clientInfo: { name: "cursor-adapter-probe", version: "test" },
  });
  check(
    "initialize passthrough",
    init.protocolVersion === 1 && init.agentCapabilities?.loadSession === true
  );
  check(
    "default diagnostics hide fixture paths",
    !fixtureRoot || !adapterDiagnostics.includes(fixtureRoot)
  );

  const list = await send("session/list", {});
  check("session/list returns an array", Array.isArray(list.sessions));
  check("explicit CLI id is discoverable", list.sessions.some((s) => s.sessionId === cliId));
  check("explicit IDE id is discoverable", list.sessions.some((s) => s.sessionId === ideId));

  drainUpdates();
  await send("session/load", { sessionId: cliId, cwd: process.cwd(), mcpServers: [] });
  const cliUpdates = drainUpdates();
  const expectedCliUpdates = [
    {
      sessionUpdate: "user_message_chunk",
      content: { type: "text", text: "fixture question" },
    },
    {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "fixture answer" },
    },
  ];
  check(
    live ? "CLI session/load replays history" : "CLI session/load replays exact fixture history",
    live ? cliUpdates.length >= 1 : JSON.stringify(cliUpdates) === JSON.stringify(expectedCliUpdates)
  );

  drainUpdates();
  await send("session/load", { sessionId: ideId, cwd: process.cwd(), mcpServers: [] });
  const ideUpdates = drainUpdates();
  const expectedIdeUpdates = [
    {
      sessionUpdate: "user_message_chunk",
      content: { type: "text", text: "fixture IDE question" },
    },
    {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "fixture IDE answer" },
    },
  ];
  check(
    live ? "IDE session/load replays history" : "IDE session/load replays exact fixture history",
    live ? ideUpdates.length >= 1 : JSON.stringify(ideUpdates) === JSON.stringify(expectedIdeUpdates)
  );

  let ideRejected = false;
  let ideError = "";
  try {
    await send("session/prompt", {
      sessionId: ideId,
      prompt: [{ type: "text", text: "compatibility probe" }],
    });
  } catch (error) {
    ideError = error.message;
    ideRejected = /read-only|does not continue/i.test(error.message);
  }
  check("IDE session/prompt is rejected before invoking Cursor", ideRejected);
  check(
    "adapter errors omit private ids and paths",
    !ideError.includes(ideId) && (!fixtureRoot || !ideError.includes(fixtureRoot))
  );

  if (!live) {
    drainUpdates();
    let corruptCliError = "";
    try {
      await send("session/load", {
        sessionId: corruptCliId,
        cwd: process.cwd(),
        mcpServers: [],
      });
    } catch (error) {
      corruptCliError = error.message;
    }
    const corruptCliUpdates = drainUpdates();
    const corruptCliClosed =
      /"code":-32603/.test(corruptCliError) && corruptCliUpdates.length === 0;
    check(
      "mixed malformed CLI storage fails before replay",
      corruptCliClosed,
      corruptCliClosed
        ? ""
        : `accepted=${corruptCliError.length === 0} updates=${corruptCliUpdates.length}`
    );

    drainUpdates();
    let whitespaceCliError = "";
    try {
      await send("session/load", {
        sessionId: whitespaceCliId,
        cwd: process.cwd(),
        mcpServers: [],
      });
    } catch (error) {
      whitespaceCliError = error.message;
    }
    const whitespaceCliUpdates = drainUpdates();
    const whitespaceCliClosed =
      /"code":-32603/.test(whitespaceCliError) && whitespaceCliUpdates.length === 0;
    check(
      "mixed whitespace CLI storage fails before replay",
      whitespaceCliClosed,
      whitespaceCliClosed
        ? ""
        : `accepted=${whitespaceCliError.length === 0} updates=${whitespaceCliUpdates.length}`
    );

    drainUpdates();
    let emptyQueryCliError = "";
    try {
      await send("session/load", {
        sessionId: emptyQueryCliId,
        cwd: process.cwd(),
        mcpServers: [],
      });
    } catch (error) {
      emptyQueryCliError = error.message;
    }
    const emptyQueryCliUpdates = drainUpdates();
    const emptyQueryCliClosed =
      /"code":-32603/.test(emptyQueryCliError) && emptyQueryCliUpdates.length === 0;
    check(
      "CLI user_query that extracts to whitespace fails before replay",
      emptyQueryCliClosed,
      emptyQueryCliClosed
        ? ""
        : `accepted=${emptyQueryCliError.length === 0} updates=${emptyQueryCliUpdates.length}`
    );

    drainUpdates();
    let corruptIdeError = "";
    try {
      await send("session/load", {
        sessionId: corruptIdeId,
        cwd: process.cwd(),
        mcpServers: [],
      });
    } catch (error) {
      corruptIdeError = error.message;
    }
    const corruptIdeUpdates = drainUpdates();
    const corruptIdeClosed =
      /"code":-32603/.test(corruptIdeError) && corruptIdeUpdates.length === 0;
    check(
      "IDE history with a missing expected bubble fails before replay",
      corruptIdeClosed,
      corruptIdeClosed
        ? ""
        : `accepted=${corruptIdeError.length === 0} updates=${corruptIdeUpdates.length}`
    );
    drainUpdates();
    let malformedComposerError = "";
    try {
      await send("session/load", {
        sessionId: malformedComposerId,
        cwd: process.cwd(),
        mcpServers: [],
      });
    } catch (error) {
      malformedComposerError = error.message;
    }
    const malformedComposerUpdates = drainUpdates();
    check(
      "malformed IDE composer fails as internal corruption before replay",
      /"code":-32603/.test(malformedComposerError) && malformedComposerUpdates.length === 0
    );
    for (const invalidComposer of invalidComposerRows) {
      drainUpdates();
      let invalidComposerError = "";
      try {
        await send("session/load", {
          sessionId: invalidComposer.id,
          cwd: process.cwd(),
          mcpServers: [],
        });
      } catch (error) {
        invalidComposerError = error.message;
      }
      const invalidComposerUpdates = drainUpdates();
      check(
        `IDE composer containing ${invalidComposer.label} is corruption, not absence`,
        /"code":-32603/.test(invalidComposerError) &&
          !/"code":-32002/.test(invalidComposerError) &&
          invalidComposerUpdates.length === 0
      );
    }

    drainUpdates();
    let resultFailureError = "";
    try {
      await send("session/prompt", {
        sessionId: cliId,
        prompt: [{ type: "text", text: "synthetic result-only failure" }],
      });
    } catch (error) {
      resultFailureError = error.message;
    }
    const resultFailureUpdates = drainUpdates();
    check(
      "result-only Cursor failure emits no assistant fallback",
      /"code":-32603/.test(resultFailureError) &&
        !resultFailureUpdates.some(
          (update) => update.sessionUpdate === "agent_message_chunk"
        )
    );
    check(
      "result-only Cursor failure text remains private",
      !resultFailureError.includes(resultFailureText) &&
        !resultFailureError.includes("CURSOR_RESULT_PRIVATE_SENTINEL") &&
        !resultFailureError.includes(fixtureRoot)
    );

  }

  if (destructive) {
    drainUpdates();
    const result = await send("session/prompt", {
      sessionId: cliId,
      prompt: [{ type: "text", text: "Reply with OK. Do not use tools." }],
    });
    const assistantChunks = drainUpdates().filter(
      (update) => update.sessionUpdate === "agent_message_chunk"
    ).length;
    check("opt-in CLI resume completes", result.stopReason === "end_turn");
    check("opt-in CLI resume emits an assistant response", assistantChunks > 0);
  } else {
    console.log(
      live
        ? "SKIP  CLI session/prompt (set ACP_ADAPTER_DESTRUCTIVE_TESTS=1 to append a test turn)"
        : "SKIP  CLI session/prompt (isolated parser/router test does not invoke a vendor CLI)"
    );
  }

  process.exitCode = failures === 0 ? 0 : 1;
} catch (error) {
  console.error("TEST RUN FAILED during adapter protocol verification");
  process.exitCode = 1;
} finally {
  adapter.stdin.end();
  adapter.kill();
  setTimeout(() => {
    if (fixtureRoot) {
      try { rmSync(fixtureRoot, { recursive: true, force: true }); } catch {}
    }
    process.exit(process.exitCode || 0);
  }, 300);
}
