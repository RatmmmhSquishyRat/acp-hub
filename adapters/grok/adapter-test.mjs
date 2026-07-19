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
let missingWorkspaceId = null;
let deleteOkId = null;
let deleteFailId = null;
let fixtureRoot = null;
let adapterEnv = { ...process.env };
let promptMarker = null;
let deleteAttemptLedger = null;
let deleteSuccessMarker = null;
let deletePrivateStderrSentinel = null;
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
  deletePrivateStderrSentinel = "GROK_PRIVATE_DELETE_STDERR_SENTINEL";
  diskId = "33333333-3333-4333-8333-333333333333";
  corruptId = "55555555-5555-4555-8555-555555555555";
  missingWorkspaceId = "66666666-6666-4666-8666-666666666666";
  deleteOkId = "77777777-7777-4777-8777-777777777777";
  deleteFailId = "88888888-8888-4888-8888-888888888888";
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
import { appendFileSync, writeFileSync } from "node:fs";
const argv = process.argv.slice(2);
const headlessWorkerFlag = "--fixture-headless-worker";
const hardDeadlineMs = 15_000;
if (argv[0] === headlessWorkerFlag) {
  setInterval(() => {}, 1000);
  setTimeout(() => process.exit(10), hardDeadlineMs);
} else {
  if (argv[0] === "sessions" && argv[1] === "delete") {
    appendFileSync(process.env.GROK_DELETE_ATTEMPT_LEDGER, JSON.stringify(argv) + "\\n");
    if (argv[2] === ${JSON.stringify(deleteFailId)}) {
      process.stderr.write(${JSON.stringify(`vendor detail ${fixtureRoot} ${deletePrivateStderrSentinel}\n`)});
      process.exit(7);
    }
    if (argv.length !== 3 || argv[2] !== ${JSON.stringify(deleteOkId)}) process.exit(9);
    appendFileSync(process.env.GROK_DELETE_SUCCESS_MARKER, JSON.stringify(argv) + "\\n");
    process.exit(0);
  }
  const promptIndex = argv.indexOf("--prompt-file");
  if (promptIndex >= 0) {
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
    GROK_HOME: grokHome,
    GROK_CMD: process.execPath,
    GROK_AGENT_SCRIPT: mockAgentPath,
    GROK_PROMPT_MARKER: promptMarker,
    GROK_DELETE_ATTEMPT_LEDGER: deleteAttemptLedger,
    GROK_DELETE_SUCCESS_MARKER: deleteSuccessMarker,
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
        "--deny",
        "Edit(*)",
        "--deny",
        "Bash(*)",
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
    await prompt;
    check(
      "shutdown removes the private prompt directory after child exit",
      headlessTreeExited && !!promptPath && !existsSync(dirname(promptPath))
    );
    await adapterStderrClosed;
    check(
      "settled adapter diagnostics omit private delete stderr",
      !adapterDiagnostics.includes(fixtureRoot) &&
        !adapterDiagnostics.includes(deletePrivateStderrSentinel)
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
