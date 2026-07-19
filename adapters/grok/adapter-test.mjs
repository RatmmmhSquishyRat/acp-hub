// Manual compatibility probe for adapter.mjs.
//
// By default this script creates an isolated synthetic Grok home and mock ACP
// upstream. It cannot read or modify the user's Grok data.
//
// Installed-agent compatibility requires explicit opt-in:
//   ACP_ADAPTER_LIVE_TESTS=1 node adapter-test.mjs <onDiskSessionId>
//
// Resuming, creating, prompting, and deleting sessions mutate Grok-managed
// state and require a second opt-in:
//   ACP_ADAPTER_DESTRUCTIVE_TESTS=1
//
// The probe reports only structure and counts; it never prints message bodies,
// prompts returned by the model, session ids, or local filesystem paths.

import { spawn } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import readline from "node:readline";

const live = process.env.ACP_ADAPTER_LIVE_TESTS === "1";
const destructive = process.env.ACP_ADAPTER_DESTRUCTIVE_TESTS === "1";
let [diskId] = process.argv.slice(2);
let corruptId = null;
let malformedKnownRows = [];
let missingWorkspaceId = null;
let deleteOkId = null;
let deleteFailId = null;
let deleteShutdownId = null;
let liveRaceId = null;
let completePromptId = null;
let malformedStreamId = null;
let missingEndPromptId = null;
let duplicateEndPromptId = null;
let unknownEventPromptId = null;
let malformedSummaryId = null;
let duplicateSessionId = null;
let fixtureRoot = null;
let adapterEnv = { ...process.env };
let promptMarker = null;
let deleteAttemptLedger = null;
let deleteSuccessMarker = null;
let deleteShutdownMarker = null;
let livePromptMarker = null;
let livePromptRelease = null;
let liveDeleteMarker = null;
let liveDeleteRelease = null;
let deletePrivateStderrSentinel = null;
let privateVendorStderr = null;
let fixtureWorkspace = null;

if (live) {
  if (!diskId) {
    console.error("Usage: node adapter-test.mjs <onDiskSessionId>");
    process.exit(2);
  }
} else {
  if (destructive) {
    console.error(
      "ACP_ADAPTER_DESTRUCTIVE_TESTS applies only with ACP_ADAPTER_LIVE_TESTS=1."
    );
    process.exit(2);
  }
  fixtureRoot = mkdtempSync(join(tmpdir(), "acp-hub-grok-adapter-test-"));
  const grokHome = join(fixtureRoot, "grok-home");
  const workspace = join(fixtureRoot, "workspace");
  fixtureWorkspace = workspace;
  const mockAgentPath = join(fixtureRoot, "mock-grok-agent.mjs");
  promptMarker = join(fixtureRoot, "prompt-marker.json");
  deleteAttemptLedger = join(fixtureRoot, "delete-attempts.jsonl");
  deleteSuccessMarker = join(fixtureRoot, "delete-success.jsonl");
  deleteShutdownMarker = join(fixtureRoot, "delete-shutdown-marker.json");
  livePromptMarker = join(fixtureRoot, "live-prompt-marker.json");
  livePromptRelease = join(fixtureRoot, "live-prompt-release");
  liveDeleteMarker = join(fixtureRoot, "live-delete-marker.json");
  liveDeleteRelease = join(fixtureRoot, "live-delete-release");
  deletePrivateStderrSentinel = "GROK_PRIVATE_DELETE_STDERR_SENTINEL";
  privateVendorStderr =
    `private Grok vendor stderr ${fixtureRoot} GROK_PRIVATE_STDERR_SENTINEL `;
  diskId = "33333333-3333-4333-8333-333333333333";
  corruptId = "55555555-5555-4555-8555-555555555555";
  malformedKnownRows = [
    {
      label: "user",
      id: "99999999-9999-4999-8999-999999999999",
      record: { type: "user", content: { unexpected: "value" } },
    },
    {
      label: "assistant",
      id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
      record: {
        type: "assistant",
        content: [{ type: "tool_use", text: "must not be silently dropped" }],
      },
    },
    {
      label: "reasoning",
      id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
      record: {
        type: "reasoning",
        summary: [{ type: "unexpected", text: "must not be silently dropped" }],
      },
    },
  ];
  missingWorkspaceId = "66666666-6666-4666-8666-666666666666";
  deleteOkId = "77777777-7777-4777-8777-777777777777";
  deleteFailId = "88888888-8888-4888-8888-888888888888";
  deleteShutdownId = "15151515-1515-4151-8151-151515151515";
  liveRaceId = "16161616-1616-4161-8161-161616161616";
  completePromptId = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
  malformedStreamId = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
  missingEndPromptId = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
  duplicateEndPromptId = "ffffffff-ffff-4fff-8fff-ffffffffffff";
  unknownEventPromptId = "12121212-1212-4121-8121-121212121212";
  malformedSummaryId = "13131313-1313-4131-8131-131313131313";
  duplicateSessionId = "14141414-1414-4141-8141-141414141414";
  const sessionDir = join(grokHome, "sessions", encodeURIComponent(workspace), diskId);
  mkdirSync(workspace, { recursive: true });
  mkdirSync(sessionDir, { recursive: true });
  writeFileSync(
    join(sessionDir, "summary.json"),
    JSON.stringify({
      info: { id: diskId, cwd: workspace },
      created_at: "2000-01-01T00:00:00Z",
      updated_at: "2000-01-01T00:00:01Z",
      session_summary: "Fixture Grok session",
    }),
    "utf8"
  );
  writeFileSync(
    join(sessionDir, "chat_history.jsonl"),
    [
      JSON.stringify({ type: "user", content: "fixture question" }),
      JSON.stringify({ type: "assistant", content: "fixture answer" }),
    ].join("\n") + "\n",
    "utf8"
  );
  for (const id of [
    completePromptId,
    malformedStreamId,
    missingEndPromptId,
    duplicateEndPromptId,
    unknownEventPromptId,
  ]) {
    const promptSessionDir = join(
      grokHome,
      "sessions",
      encodeURIComponent(workspace),
      id
    );
    mkdirSync(promptSessionDir, { recursive: true });
    writeFileSync(
      join(promptSessionDir, "summary.json"),
      JSON.stringify({ info: { id, cwd: workspace } }),
      "utf8"
    );
    writeFileSync(
      join(promptSessionDir, "chat_history.jsonl"),
      JSON.stringify({ type: "user", content: "fixture prompt session" }) + "\n",
      "utf8"
    );
  }
  const malformedSummaryDir = join(
    grokHome,
    "sessions",
    encodeURIComponent(workspace),
    malformedSummaryId
  );
  mkdirSync(malformedSummaryDir, { recursive: true });
  writeFileSync(
    join(malformedSummaryDir, "summary.json"),
    JSON.stringify({
      info: { id: malformedSummaryId, cwd: workspace },
      session_summary: { private: "unsupported" },
    }),
    "utf8"
  );
  writeFileSync(
    join(malformedSummaryDir, "chat_history.jsonl"),
    JSON.stringify({ type: "user", content: "must not be replayed" }) + "\n",
    "utf8"
  );
  const duplicateWorkspace = join(fixtureRoot, "duplicate-workspace");
  mkdirSync(duplicateWorkspace, { recursive: true });
  for (const duplicateCwd of [workspace, duplicateWorkspace]) {
    const duplicateDir = join(
      grokHome,
      "sessions",
      encodeURIComponent(duplicateCwd),
      duplicateSessionId
    );
    mkdirSync(duplicateDir, { recursive: true });
    writeFileSync(
      join(duplicateDir, "summary.json"),
      JSON.stringify({ info: { id: duplicateSessionId, cwd: duplicateCwd } }),
      "utf8"
    );
    writeFileSync(
      join(duplicateDir, "chat_history.jsonl"),
      JSON.stringify({ type: "user", content: "ambiguous fixture" }) + "\n",
      "utf8"
    );
  }
  const corruptDir = join(grokHome, "sessions", encodeURIComponent(workspace), corruptId);
  mkdirSync(corruptDir, { recursive: true });
  writeFileSync(
    join(corruptDir, "summary.json"),
    JSON.stringify({ info: { id: corruptId, cwd: workspace } }),
    "utf8"
  );
  writeFileSync(
    join(corruptDir, "chat_history.jsonl"),
    [
      JSON.stringify({ type: "user", content: "valid before corruption" }),
      "{not-json",
    ].join("\n") + "\n",
    "utf8"
  );
  for (const malformed of malformedKnownRows) {
    const malformedDir = join(
      grokHome,
      "sessions",
      encodeURIComponent(workspace),
      malformed.id
    );
    mkdirSync(malformedDir, { recursive: true });
    writeFileSync(
      join(malformedDir, "summary.json"),
      JSON.stringify({ info: { id: malformed.id, cwd: workspace } }),
      "utf8"
    );
    writeFileSync(
      join(malformedDir, "chat_history.jsonl"),
      [
        JSON.stringify({ type: "user", content: "valid before known malformed record" }),
        JSON.stringify(malformed.record),
      ].join("\n") + "\n",
      "utf8"
    );
  }
  const missingWorkspace = join(fixtureRoot, "private-missing-workspace");
  const missingDir = join(
    grokHome,
    "sessions",
    encodeURIComponent(missingWorkspace),
    missingWorkspaceId
  );
  mkdirSync(missingDir, { recursive: true });
  writeFileSync(
    join(missingDir, "summary.json"),
    JSON.stringify({ info: { id: missingWorkspaceId, cwd: missingWorkspace } }),
    "utf8"
  );
  writeFileSync(
    join(missingDir, "chat_history.jsonl"),
    JSON.stringify({ type: "user", content: "private fixture" }) + "\n",
    "utf8"
  );
  writeFileSync(
    mockAgentPath,
    `import { spawn } from "node:child_process";
import readline from "node:readline";
import { appendFileSync, existsSync, writeFileSync } from "node:fs";
const argv = process.argv.slice(2);
if (process.env.GROK_PRIVATE_STDERR) {
  process.stderr.write(process.env.GROK_PRIVATE_STDERR + "X".repeat(70_000) + "\\n");
}
const headlessWorkerFlag = "--fixture-headless-worker";
const hardDeadlineMs = 15_000;
if (argv[0] === headlessWorkerFlag) {
  setInterval(() => {}, 1000);
  setTimeout(() => process.exit(10), hardDeadlineMs);
} else {
  if (argv[0] === "sessions" && argv[1] === "delete") {
    appendFileSync(process.env.GROK_DELETE_ATTEMPT_LEDGER, JSON.stringify(argv) + "\\n");
    if (argv[2] === ${JSON.stringify(deleteShutdownId)}) {
      const worker = spawn(process.execPath, [process.argv[1], headlessWorkerFlag], {
        detached: process.platform === "win32",
        stdio: "ignore",
        windowsHide: true
      });
      worker.unref();
      writeFileSync(
        process.env.GROK_DELETE_SHUTDOWN_MARKER,
        JSON.stringify({ pid: process.pid, descendantPid: worker.pid })
      );
      setInterval(() => {}, 1000);
      setTimeout(() => process.exit(10), hardDeadlineMs);
    } else if (argv[2] === ${JSON.stringify(liveRaceId)}) {
      writeFileSync(process.env.GROK_LIVE_DELETE_MARKER, JSON.stringify({ pid: process.pid }));
      const release = setInterval(() => {
        if (!existsSync(process.env.GROK_LIVE_DELETE_RELEASE)) return;
        clearInterval(release);
        appendFileSync(process.env.GROK_DELETE_SUCCESS_MARKER, JSON.stringify(argv) + "\\n");
        process.exit(0);
      }, 5);
      setTimeout(() => process.exit(10), hardDeadlineMs);
    } else if (argv[2] === ${JSON.stringify(deleteFailId)}) {
      process.stderr.write(${JSON.stringify(`vendor detail ${fixtureRoot} ${deletePrivateStderrSentinel}\n`)});
      process.exit(7);
    } else {
      if (argv.length !== 3 || argv[2] !== ${JSON.stringify(deleteOkId)}) process.exit(9);
      appendFileSync(process.env.GROK_DELETE_SUCCESS_MARKER, JSON.stringify(argv) + "\\n");
      process.exit(0);
    }
  }
  const promptIndex = argv.indexOf("--prompt-file");
  if (promptIndex >= 0) {
    const resumeIndex = argv.indexOf("-r");
    const resumedId = resumeIndex >= 0 ? argv[resumeIndex + 1] : null;
    if (resumedId === ${JSON.stringify(completePromptId)}) {
      process.stdout.end(
        JSON.stringify({ type: "text", data: "fixture response" }) + "\\n" +
        JSON.stringify({ type: "end", stopReason: "end_turn" }) + "\\n"
      );
    } else if (resumedId === ${JSON.stringify(malformedStreamId)}) {
      process.stdout.end(
        JSON.stringify({ type: "text", data: "must not be replayed" }) + "\\n" +
        "{not-json\\n" +
        JSON.stringify({ type: "end", stopReason: "end_turn" }) + "\\n"
      );
    } else if (resumedId === ${JSON.stringify(missingEndPromptId)}) {
      process.stdout.end(JSON.stringify({ type: "text", data: "must not be replayed" }) + "\\n");
    } else if (resumedId === ${JSON.stringify(duplicateEndPromptId)}) {
      process.stdout.end(
        JSON.stringify({ type: "text", data: "must not be replayed" }) + "\\n" +
        JSON.stringify({ type: "end", stopReason: "end_turn" }) + "\\n" +
        JSON.stringify({ type: "end", stopReason: "end_turn" }) + "\\n"
      );
    } else if (resumedId === ${JSON.stringify(unknownEventPromptId)}) {
      process.stdout.end(
        JSON.stringify({ type: "text", data: "must not be replayed" }) + "\\n" +
        JSON.stringify({ type: "unknown", data: "vendor drift" }) + "\\n" +
        JSON.stringify({ type: "end", stopReason: "end_turn" }) + "\\n"
      );
    } else {
      const worker = spawn(process.execPath, [process.argv[1], headlessWorkerFlag], {
        detached: process.platform === "win32",
        stdio: "ignore",
        windowsHide: true
      });
      worker.unref();
      writeFileSync(
        process.env.GROK_PROMPT_MARKER,
        JSON.stringify({
          argv,
          cwd: process.cwd(),
          pid: process.pid,
          descendantPid: worker.pid
        })
      );
      setInterval(() => {}, 1000);
      setTimeout(() => process.exit(10), hardDeadlineMs);
    }
  } else {
    const input = readline.createInterface({ input: process.stdin });
    const send = (msg) => process.stdout.write(JSON.stringify(msg) + "\\n");
    input.on("line", (line) => {
      let msg;
      try { msg = JSON.parse(line); } catch { return; }
      if (msg.method === "initialize") {
        if (msg.params?.clientInfo?.name === "force-init-error") {
          send({ jsonrpc: "2.0", id: msg.id, error: { code: -32001, message: "fixture init rejected" } });
          return;
        }
        send({ jsonrpc: "2.0", id: msg.id, result: {
          protocolVersion: 1,
          agentCapabilities: { loadSession: true, sessionCapabilities: {} },
          authMethods: [{ id: "fixture" }],
          _meta: { defaultAuthMethodId: "fixture" }
        } });
      } else if (msg.method === "authenticate") {
        send({ jsonrpc: "2.0", id: msg.id, result: {} });
      } else if (msg.method === "session/new") {
        send({ jsonrpc: "2.0", id: msg.id, result: { sessionId: ${JSON.stringify(liveRaceId)} } });
      } else if (
        msg.method === "session/prompt" &&
        msg.params?.sessionId === ${JSON.stringify(liveRaceId)}
      ) {
        writeFileSync(process.env.GROK_LIVE_PROMPT_MARKER, JSON.stringify({ id: msg.id }));
        const release = setInterval(() => {
          if (!existsSync(process.env.GROK_LIVE_PROMPT_RELEASE)) return;
          clearInterval(release);
          send({ jsonrpc: "2.0", id: msg.id, result: { stopReason: "end_turn" } });
        }, 5);
      } else if (msg.id !== undefined) {
        send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "fixture method unavailable" } });
      }
    });
  }
}
`,
    "utf8"
  );
  adapterEnv = {
    ...adapterEnv,
    GROK_HOME: grokHome,
    GROK_CMD: process.execPath,
    GROK_AGENT_SCRIPT: mockAgentPath,
    GROK_PROMPT_MARKER: promptMarker,
    GROK_DELETE_ATTEMPT_LEDGER: deleteAttemptLedger,
    GROK_DELETE_SUCCESS_MARKER: deleteSuccessMarker,
    GROK_DELETE_SHUTDOWN_MARKER: deleteShutdownMarker,
    GROK_LIVE_PROMPT_MARKER: livePromptMarker,
    GROK_LIVE_PROMPT_RELEASE: livePromptRelease,
    GROK_LIVE_DELETE_MARKER: liveDeleteMarker,
    GROK_LIVE_DELETE_RELEASE: liveDeleteRelease,
    GROK_PRIVATE_STDERR: privateVendorStderr,
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
const adapterStderrClosed = new Promise((resolve) => {
  adapter.stderr.once("close", resolve);
});

let nextId = 1;
const pending = new Map();
const updates = [];
let adapterClosed = false;

function send(method, params) {
  const id = nextId++;
  adapter.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timeout ${method}`)), 180_000);
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
  if (
    msg.method === "_x.ai/session_notification" ||
    msg.method === "_x.ai/sessions/changed" ||
    msg.method === "_x.ai/queue/changed" ||
    msg.method === "_x.ai/session/prompt_complete"
  ) {
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
adapter.on("exit", () => {
  for (const waiter of pending.values()) {
    waiter.reject(new Error("adapter exited"));
  }
  pending.clear();
});

function drainUpdates() {
  return updates.splice(0);
}

function readMarkerEntries(path) {
  if (!path || !existsSync(path)) return [];
  return readFileSync(path, "utf8")
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

function processIsAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

async function waitForProcessExit(pid, timeoutMs = 5_000) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!processIsAlive(pid)) return true;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  return !processIsAlive(pid);
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
    clientInfo: { name: "grok-adapter-probe", version: "test" },
  });
  check("initialize returns ACP v1", init.protocolVersion === 1);
  check("initialize advertises list", !!init.agentCapabilities?.sessionCapabilities?.list);
  check("initialize advertises delete", !!init.agentCapabilities?.sessionCapabilities?.delete);
  check("initialize preserves loadSession", init.agentCapabilities?.loadSession === true);
  check(
    "default diagnostics hide fixture paths",
    !fixtureRoot || !adapterDiagnostics.includes(fixtureRoot)
  );

  if (!live) {
    let initErrorForwarded = false;
    try {
      await send("initialize", {
        protocolVersion: 1,
        clientCapabilities: {},
        clientInfo: { name: "force-init-error", version: "test" },
      });
    } catch (error) {
      initErrorForwarded = /-32001|fixture init rejected/.test(error.message);
    }
    check("upstream initialize errors stay errors", initErrorForwarded);
  }

  const list = await send("session/list", {});
  check("session/list returns an array", Array.isArray(list.sessions));
  check("explicit on-disk id is discoverable", list.sessions.some((s) => s.sessionId === diskId));
  if (!live) {
    check(
      "malformed and duplicate private-store sessions are excluded from discovery",
      !list.sessions.some(
        (session) =>
          session.sessionId === malformedSummaryId ||
          session.sessionId === duplicateSessionId
      )
    );
  }

  drainUpdates();
  await send("session/load", { sessionId: diskId, cwd: process.cwd(), mcpServers: [] });
  const replayUpdates = drainUpdates();
  const expectedReplayUpdates = [
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
    live ? "session/load replays history" : "session/load replays exact fixture history",
    live
      ? replayUpdates.length >= 1
      : JSON.stringify(replayUpdates) === JSON.stringify(expectedReplayUpdates)
  );

  if (!live) {
    drainUpdates();
    let corruptError = "";
    try {
      await send("session/load", {
        sessionId: corruptId,
        cwd: process.cwd(),
        mcpServers: [],
      });
    } catch (error) {
      corruptError = error.message;
    }
    const corruptUpdates = drainUpdates();
    check(
      "valid-prefix Grok corruption fails before replay",
      /"code":-32603/.test(corruptError) && corruptUpdates.length === 0
    );

    for (const privateStoreCase of [
      [malformedSummaryId, -32603, "malformed Grok summary fails before replay"],
      [duplicateSessionId, -32602, "duplicate Grok session id fails before replay"],
    ]) {
      drainUpdates();
      let privateStoreError = "";
      try {
        await send("session/load", {
          sessionId: privateStoreCase[0],
          cwd: process.cwd(),
          mcpServers: [],
        });
      } catch (error) {
        privateStoreError = error.message;
      }
      check(
        privateStoreCase[2],
        privateStoreError.includes(`"code":${privateStoreCase[1]}`) &&
          drainUpdates().length === 0
      );
    }

    for (const malformed of malformedKnownRows) {
      drainUpdates();
      let malformedError = "";
      try {
        await send("session/load", {
          sessionId: malformed.id,
          cwd: process.cwd(),
          mcpServers: [],
        });
      } catch (error) {
        malformedError = error.message;
      }
      const malformedUpdates = drainUpdates();
      check(
        `mixed valid and malformed known ${malformed.label} record fails before replay`,
        /"code":-32603/.test(malformedError) && malformedUpdates.length === 0
      );
    }

    let privateError = "";
    try {
      await send("session/prompt", {
        sessionId: missingWorkspaceId,
        prompt: [{ type: "text", text: "fixture" }],
      });
    } catch (error) {
      privateError = error.message;
    }
    check(
      "adapter errors omit private ids and paths",
      privateError.length > 0 &&
        !privateError.includes(missingWorkspaceId) &&
        !privateError.includes(fixtureRoot)
    );

    let mixedPromptRejected = false;
    try {
      await send("session/prompt", {
        sessionId: diskId,
        prompt: [
          { type: "text", text: "must not run" },
          { type: "image", data: "private-fixture-data", mimeType: "image/png" },
        ],
      });
    } catch (error) {
      mixedPromptRejected = /"code":-32602/.test(error.message);
    }
    check(
      "mixed prompt content is rejected before vendor invocation",
      mixedPromptRejected && !existsSync(promptMarker)
    );

    drainUpdates();
    const completedPrompt = await send("session/prompt", {
      sessionId: completePromptId,
      prompt: [{ type: "text", text: "fixture" }],
    });
    const completedUpdates = drainUpdates();
    check(
      "headless prompt requires and accepts one valid terminal event",
      completedPrompt.stopReason === "end_turn" &&
        completedUpdates.length === 1 &&
        completedUpdates[0]?.sessionUpdate === "agent_message_chunk"
    );

    drainUpdates();
    let malformedStreamError = "";
    try {
      await send("session/prompt", {
        sessionId: malformedStreamId,
        prompt: [{ type: "text", text: "fixture" }],
      });
    } catch (error) {
      malformedStreamError = error.message;
    }
    check(
      "malformed headless output cannot become a successful turn",
      /"code":-32603/.test(malformedStreamError) && drainUpdates().length === 0
    );

    for (const streamCase of [
      [missingEndPromptId, "missing headless terminal event cannot become success"],
      [duplicateEndPromptId, "duplicate headless terminal event cannot become success"],
      [unknownEventPromptId, "unknown headless event cannot become success"],
    ]) {
      drainUpdates();
      let streamError = "";
      try {
        await send("session/prompt", {
          sessionId: streamCase[0],
          prompt: [{ type: "text", text: "fixture" }],
        });
      } catch (error) {
        streamError = error.message;
      }
      check(
        streamCase[1],
        /"code":-32603/.test(streamError) && drainUpdates().length === 0
      );
    }

    let invalidDeleteRejected = false;
    try {
      await send("session/delete", { sessionId: "invalid" });
    } catch {
      invalidDeleteRejected = true;
    }
    const validationAttempts = readMarkerEntries(deleteAttemptLedger);
    check(
      "delete validation rejects without vendor invocation or success",
      invalidDeleteRejected &&
        validationAttempts.length === 0 &&
        !existsSync(deleteSuccessMarker)
    );

    let deleteFailure = "";
    try {
      await send("session/delete", { sessionId: deleteFailId });
    } catch (error) {
      deleteFailure = error.message;
    }
    check(
      "fixture delete failure is sanitized",
      /exit 7/.test(deleteFailure) &&
        !deleteFailure.includes(fixtureRoot) &&
        !deleteFailure.includes(deletePrivateStderrSentinel)
    );
    const failureAttempts = readMarkerEntries(deleteAttemptLedger);
    check(
      "failed vendor delete records exactly its expected invocation without success",
      failureAttempts.length === 1 &&
        JSON.stringify(failureAttempts[0]) ===
          JSON.stringify(["sessions", "delete", deleteFailId]) &&
        !existsSync(deleteSuccessMarker)
    );

    rmSync(deleteAttemptLedger, { force: true });
    await send("session/delete", { sessionId: deleteOkId });
    const successAttempts = readMarkerEntries(deleteAttemptLedger);
    const successfulDeletes = readMarkerEntries(deleteSuccessMarker);
    const expectedSuccessfulDelete = ["sessions", "delete", deleteOkId];
    check(
      "successful delete records exactly one expected vendor invocation and success",
      successAttempts.length === 1 &&
        JSON.stringify(successAttempts[0]) === JSON.stringify(expectedSuccessfulDelete) &&
        successfulDeletes.length === 1 &&
        JSON.stringify(successfulDeletes[0]) === JSON.stringify(expectedSuccessfulDelete)
    );

    const liveRaceSession = await send("session/new", {
      cwd: fixtureWorkspace,
      mcpServers: [],
    });
    const livePrompt = send("session/prompt", {
      sessionId: liveRaceSession.sessionId,
      prompt: [{ type: "text", text: "hold fixture live prompt" }],
    });
    for (let attempt = 0; attempt < 100 && !existsSync(livePromptMarker); attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    check(
      "fixture creates and starts one tracked live prompt",
      liveRaceSession.sessionId === liveRaceId && existsSync(livePromptMarker)
    );
    let deleteDuringPromptError = "";
    try {
      await send("session/delete", { sessionId: liveRaceId });
    } catch (error) {
      deleteDuringPromptError = error.message;
    }
    check(
      "live prompt blocks deletion for the same session",
      /"code":-32009/.test(deleteDuringPromptError)
    );
    writeFileSync(livePromptRelease, "");
    const livePromptResult = await livePrompt;

    const liveDeletion = send("session/delete", { sessionId: liveRaceId });
    for (let attempt = 0; attempt < 100 && !existsSync(liveDeleteMarker); attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    check(
      "completed live prompt permits a tracked deletion",
      livePromptResult.stopReason === "end_turn" && existsSync(liveDeleteMarker)
    );
    let promptDuringDeleteError = "";
    try {
      await send("session/prompt", {
        sessionId: liveRaceId,
        prompt: [{ type: "text", text: "must not race deletion" }],
      });
    } catch (error) {
      promptDuringDeleteError = error.message;
    }
    check(
      "tracked deletion blocks live prompt for the same session",
      /"code":-32009/.test(promptDuringDeleteError)
    );
    writeFileSync(liveDeleteRelease, "");
    const liveDeleteResult = await liveDeletion;
    check(
      "tracked live-session deletion completes after release",
      !!liveDeleteResult && typeof liveDeleteResult === "object"
    );
  }

  if (destructive) {
    drainUpdates();
    const resumed = await send("session/prompt", {
      sessionId: diskId,
      prompt: [{ type: "text", text: "Reply with OK. Do not use tools." }],
    });
    const resumeChunks = drainUpdates().filter(
      (update) => update.sessionUpdate === "agent_message_chunk"
    ).length;
    check("opt-in on-disk resume completes", resumed.stopReason === "end_turn");
    check("opt-in on-disk resume emits an assistant response", resumeChunks > 0);

    const created = await send("session/new", { cwd: process.cwd(), mcpServers: [] });
    check("opt-in session/new creates a live session", typeof created?.sessionId === "string");
    if (created?.sessionId) {
      try {
        drainUpdates();
        const liveResult = await send("session/prompt", {
          sessionId: created.sessionId,
          prompt: [{ type: "text", text: "Reply with OK. Do not use tools." }],
        });
        check("opt-in live prompt completes", liveResult.stopReason === "end_turn");
        check(
          "opt-in live prompt emits an assistant response",
          drainUpdates().some((update) => update.sessionUpdate === "agent_message_chunk")
        );
      } finally {
        let deleted = null;
        let deleteError = "";
        try {
          deleted = await send("session/delete", { sessionId: created.sessionId });
        } catch (error) {
          deleteError = error.message;
        }
        check(
          "opt-in cleanup deletes the probe session",
          !deleteError && !!deleted && typeof deleted === "object"
        );
      }
    }
  } else {
    console.log(
      live
        ? "SKIP  resume/new/prompt/delete " +
            "(set ACP_ADAPTER_DESTRUCTIVE_TESTS=1 to permit Grok-managed state changes)"
        : "SKIP  resume/new/prompt/delete (isolated parser/router test performs no vendor writes)"
    );
  }

  if (!live) {
    const promptText = "shutdown cleanup fixture";
    const prompt = send("session/prompt", {
      sessionId: diskId,
      prompt: [{ type: "text", text: promptText }],
    }).catch(() => null);
    for (let attempt = 0; attempt < 100 && !existsSync(promptMarker); attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    const markerReady = existsSync(promptMarker);
    check("shutdown fixture starts a pending headless prompt", markerReady);
    let promptPath = null;
    let headlessPid = null;
    let headlessDescendantPid = null;
    if (markerReady) {
      const marker = JSON.parse(readFileSync(promptMarker, "utf8"));
      headlessPid = Number.isInteger(marker.pid) && marker.pid > 0 ? marker.pid : null;
      headlessDescendantPid =
        Number.isInteger(marker.descendantPid) && marker.descendantPid > 0
          ? marker.descendantPid
          : null;
      const promptIndex = Array.isArray(marker.argv) ? marker.argv.indexOf("--prompt-file") : -1;
      promptPath = promptIndex >= 0 ? marker.argv[promptIndex + 1] : null;
      const expectedArgv = [
        "--no-auto-update",
        "-r",
        diskId,
        "--prompt-file",
        promptPath,
        "--output-format",
        "streaming-json",
        "--permission-mode",
        "dontAsk",
        "--no-plan",
        "--no-subagents",
        "--no-memory",
        "--disable-web-search",
        "--deny",
        "Edit(*)",
        "--deny",
        "Bash(*)",
        "--deny",
        "Read",
        "--deny",
        "Grep",
        "--deny",
        "WebFetch",
        "--deny",
        "MCPTool(*)",
        "--cwd",
        fixtureWorkspace,
      ];
      check(
        "headless resume uses the exact safe vendor argv",
        JSON.stringify(marker.argv) === JSON.stringify(expectedArgv)
      );
      const recordedPhysicalCwd = realpathSync(marker.cwd);
      const expectedPhysicalCwd = realpathSync(fixtureWorkspace);
      check(
        "headless resume child uses the physical original workspace",
        recordedPhysicalCwd === expectedPhysicalCwd
      );
      check(
        "prompt text is absent from vendor argv",
        Array.isArray(marker.argv) &&
          !marker.argv.some((arg) => String(arg).includes(promptText))
      );
      check("prompt temp file exists before shutdown", !!promptPath && existsSync(promptPath));
    }
    const exited = new Promise((resolve) => adapter.once("exit", resolve));
    let concurrentPromptError = "";
    try {
      await send("session/prompt", {
        sessionId: diskId,
        prompt: [{ type: "text", text: "must not start a second process" }],
      });
    } catch (error) {
      concurrentPromptError = error.message;
    }
    check(
      "a second headless prompt for the same session is rejected",
      /"code":-32009/.test(concurrentPromptError)
    );
    const deletion = send("session/delete", {
      sessionId: deleteShutdownId,
    }).catch(() => null);
    for (let attempt = 0; attempt < 100 && !existsSync(deleteShutdownMarker); attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    const deleteMarkerReady = existsSync(deleteShutdownMarker);
    check("shutdown fixture starts a pending Grok deletion", deleteMarkerReady);
    let concurrentDeletionError = "";
    try {
      await send("session/delete", { sessionId: deleteShutdownId });
    } catch (error) {
      concurrentDeletionError = error.message;
    }
    check(
      "a second deletion for the same session is rejected",
      /"code":-32009/.test(concurrentDeletionError)
    );
    let deletionPid = null;
    let deletionDescendantPid = null;
    if (deleteMarkerReady) {
      const marker = JSON.parse(readFileSync(deleteShutdownMarker, "utf8"));
      deletionPid = Number.isInteger(marker.pid) && marker.pid > 0 ? marker.pid : null;
      deletionDescendantPid =
        Number.isInteger(marker.descendantPid) && marker.descendantPid > 0
          ? marker.descendantPid
          : null;
    }
    adapter.stdin.end();
    await exited;
    adapterClosed = true;
    const headlessExited = headlessPid
      ? await waitForProcessExit(headlessPid)
      : false;
    const headlessDescendantExited = headlessDescendantPid
      ? await waitForProcessExit(headlessDescendantPid)
      : false;
    const headlessTreeExited = headlessExited && headlessDescendantExited;
    check("shutdown terminates the recorded headless child tree", headlessTreeExited);
    const deletionExited = deletionPid
      ? await waitForProcessExit(deletionPid)
      : false;
    const deletionDescendantExited = deletionDescendantPid
      ? await waitForProcessExit(deletionDescendantPid)
      : false;
    check(
      "shutdown terminates the recorded Grok deletion child tree",
      deletionExited && deletionDescendantExited
    );
    await prompt;
    await deletion;
    check(
      "shutdown removes the private prompt directory after child exit",
      headlessTreeExited && !!promptPath && !existsSync(dirname(promptPath))
    );
    await adapterStderrClosed;
    check(
      "settled adapter diagnostics omit all private vendor stderr",
      !adapterDiagnostics.includes(fixtureRoot) &&
        !adapterDiagnostics.includes(deletePrivateStderrSentinel) &&
        !adapterDiagnostics.includes(privateVendorStderr) &&
        !adapterDiagnostics.includes("GROK_PRIVATE_STDERR_SENTINEL")
    );
  }

  process.exitCode = failures === 0 ? 0 : 1;
} catch (error) {
  console.error("TEST RUN FAILED during adapter protocol verification");
  process.exitCode = 1;
} finally {
  if (!adapterClosed) {
    adapter.stdin.end();
    adapter.kill();
  }
  setTimeout(() => {
    if (fixtureRoot) {
      try { rmSync(fixtureRoot, { recursive: true, force: true }); } catch {}
    }
    process.exit(process.exitCode || 0);
  }, 500);
}
