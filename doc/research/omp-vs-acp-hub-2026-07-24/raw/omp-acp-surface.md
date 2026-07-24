# Oh My Pi ACP endpoint surface (research extract)

**Date:** 2026-07-24  
**Scope:** OMP native ACP server (`omp acp`) — process lifecycle, protocol methods, permissions, modes/config, subagent interaction, reliability choices.  
**Role:** Raw lane-B notes for comparing OMP (single full agent product) vs acp-hub (multi-agent conductor).  
**Evidence tags:** `code:path` / `doc:path` / hub adapter samples.

---

## 1. Architecture overview

OMP exposes ACP as a **first-class mode**, not a side adapter.

| Layer | Path | Role |
|-------|------|------|
| CLI entry | `packages/coding-agent/src/commands/acp.ts` | Forces `mode: "acp"` (unless `--acp-terminal-auth`) |
| Launch | `packages/coding-agent/src/main.ts` | Settings overrides, session factory, hands off to runner |
| Transport | `packages/coding-agent/src/modes/acp/acp-mode.ts` | NDJSON over stdio via `@agentclientprotocol/sdk` |
| Protocol agent | `packages/coding-agent/src/modes/acp/acp-agent.ts` | Implements ACP `Agent` methods |
| Client bridge | `packages/coding-agent/src/modes/acp/acp-client-bridge.ts` | Maps client fs/terminal/permission into `ClientBridge` |
| Event mapping | `packages/coding-agent/src/modes/acp/acp-event-mapper.ts` | `AgentSessionEvent` → ACP `sessionUpdate` |
| Auth helper | `packages/coding-agent/src/modes/acp/terminal-auth.ts` | `--acp-terminal-auth` strips `--mode` for interactive setup |
| Session tools | `session/client-bridge.ts`, `session/agent-session.ts` | Permission gate + tool routing |
| Write routing | `tools/acp-bridge.ts` | Editor buffer writes when client advertises fs write |

**Mental model:** one long-lived process = one ACP connection = many concurrent managed sessions (Map by `sessionId`), each backed by a real `AgentSession` with local disk session JSONL.

Hub registration sample (`repos/acp-hub/adapters/omp/`):

```text
acp-hub agent add omp --type stdio --command omp --args acp
```

Hub sample defaults: `permission_policy: reject`, client fs/terminal **off**. Those govern **hub client callbacks**, not OMP’s internal tool set.

---

## 2. Process lifecycle of `omp acp`

### 2.1 Entry

1. `omp acp` → `commands/acp.ts` → `prepareAcpTerminalAuthArgs` → `parseArgs` → `runRootCommand`.
2. Unless terminal-auth flag is present, `parsed.mode = "acp"`.
3. `main.ts` path when `mode === "acp"`:
   - Applies `applyAcpDefaultSettingOverrides` (host-neutral defaults for task/memory/advisor knobs when not explicitly configured).
   - Sets `PI_NO_TITLE=1` (no title-generation side traffic on protocol modes).
   - Treats stdin as **protocol**, not piped prompt (`pipedInput` skipped).
   - Builds `createAcpSessionFactory(...)` then `runAcpMode(createAcpSession)`.
   - Stops startup watchdog before blocking on the transport.

`code:packages/coding-agent/src/commands/acp.ts`  
`code:packages/coding-agent/src/main.ts` (~1117–1369)

### 2.2 Transport start

`runAcpMode`:

- If **stdin is a TTY**, writes a stderr explanation that this is a JSON-RPC server for ACP clients (not meant for humans to type into).
- Wires **stdout = write, stdin = read** through `ndJsonStream`.
- Creates `AgentSideConnection` with `AcpAgent`.
- Awaits `connection.closed`, then `process.exit(0)`.

`code:packages/coding-agent/src/modes/acp/acp-mode.ts`

**Stdout hygiene:** stdout is exclusively NDJSON frames. Banners/progress must not leak. Enforced by smoke test `test/acp-stdout-hygiene.test.ts`. Logs go to `~/.omp/logs/`.

### 2.3 Session factory (per `session/new`)

`createAcpSessionFactory`:

- Clones settings for the **session cwd** (`settings.cloneForCwd(cwd)`).
- Creates `SessionManager.create(cwd, sessionDir)`.
- `agentId = acp:${sessionId}`.
- Re-resolves `TITLE_SYSTEM.md` against **that** cwd (not launch cwd).
- Calls `createAgentSession` with:
  - `hasUI: false`
  - **`enableMCP: false`** always (MCP only from client’s `mcpServers`; issue #1234)
- Applies extension CLI flags from original process argv.

`code:packages/coding-agent/src/main.ts` (`createAcpSessionFactory`)  
`code:packages/coding-agent/test/acp-mcp-isolation.test.ts`

### 2.4 Connection teardown

On first `initialize`, registers `connection.signal` abort → `#disposeAllSessions`:

- Marks open records closed (`ACP_SESSION_CLOSED`).
- Cancels in-flight prompts.
- Disconnects MCP managers, disposes each `AgentSession`, disposes optional `initialSession`.

`code:acp-agent.ts` `#registerConnectionCleanup`, `#disposeAllSessions`

### 2.5 Terminal auth side-path

If client advertises `clientCapabilities.auth.terminal` and user picks auth method `terminal`:

- Agent advertises method with `args: ["--acp-terminal-auth"]`.
- Client re-spawns OMP with that flag → flag stripped, **mode not forced to acp** → interactive TUI for key/setup.

`code:terminal-auth.ts`, `acp-agent.ts` `initialize` / `authenticate`  
`test:acp-initialize-conformance.test.ts`

---

## 3. `initialize` capabilities advertised

### 3.1 Request handling

- Stores `params.clientCapabilities` for later bridge + elicitation gating.
- Returns `protocolVersion: PROTOCOL_VERSION` (SDK constant).
- `agentInfo`: `{ name: "oh-my-pi", title: "Oh My Pi", version: VERSION }`.

### 3.2 Auth methods

| Method | When | Meaning |
|--------|------|---------|
| `agent` (no `type`) | Always | Use existing `~/.omp` credentials / OAuth |
| `terminal` (`type: "terminal"`) | Only if client has `auth.terminal` | Launch TUI setup via `--acp-terminal-auth` |

`authenticate` rejects unknown `methodId`s (fail-fast).

### 3.3 `agentCapabilities`

```ts
{
  loadSession: true,
  mcpCapabilities: { http: true, sse: true },  // not ACP-channel
  promptCapabilities: {
    embeddedContext: true,
    image: true,
  },
  sessionCapabilities: {
    list: {},
    fork: {},
    resume: {},
    close: {},
  },
}
```

**Not advertised as agent capabilities (but used when client offers them):**

- Client `fs.readTextFile` / `fs.writeTextFile` / `terminal` → tool routing via ClientBridge.
- Client `elicitation.form` → plan approval, extension UI, generic approval prompts.
- Client `auth.terminal` → terminal auth method.

**Note:** experimental MCP `type: "acp"` transport is **rejected** if a client sends it; not in `mcpCapabilities`.

`code:acp-agent.ts` `initialize` (~472–516), `#toMcpConfig`  
`test:acp-initialize-conformance.test.ts`

---

## 4. Session methods

All session mutations go through managed records:

```ts
ManagedSessionRecord {
  session, mcpManager?, promptTurn?, promptQueue,
  liveMessageId/progress, toolArgsById,
  extensionsConfigured, lifetimeUnsubscribe?,
  closedError?, promptEventHandlers, extensionUserMessageTasks
}
```

Cwd must be **absolute** (`#assertAbsoluteCwd`).

### 4.1 `session/new`

1. Create session via factory for `params.cwd`.
2. `ensureOnDisk()` so session has a durable id/path.
3. `#registerPreparedSession`:
   - `setClientBridge(createAcpClientBridge(connection, sessionId, clientCapabilities))`
   - configure extensions (`session_start`)
   - configure MCP from `params.mcpServers`
4. Return `{ sessionId, configOptions, modes }`.
5. `#scheduleBootstrapUpdates` (50ms race guard) then:
   - install lifetime subscription (thinking → config updates)
   - emit `available_commands_update`
   - emit `session_info_update` (title, updatedAt)

### 4.2 `session/load`

1. If already in memory: assert matching cwd, reconfigure MCP, return (no re-open).
2. Else open stored session by id+cwd, `switchSession` onto path.
3. **`#replaySessionHistory`** — emit historical user/assistant/tool updates as session notifications (with stable `messageId`s for assistant chunks).
4. Return config/modes; schedule bootstrap updates.

**Load is the “show history in the client” path.**

### 4.3 `session/resume`

Implementation mirrors load open path (`#resumeManagedSession` ≈ `#loadManagedSession`) but **does not call `#replaySessionHistory`**.

Both load and resume:

- require session exists under cwd
- re-apply MCP servers
- reinstall ClientBridge + extensions on open
- return config/modes + bootstrap commands

**Resume is “reattach without replaying transcript into the UI.”**

`code:acp-agent.ts` `loadSession` / `resumeSession` / `#loadManagedSession` / `#resumeManagedSession`

### 4.4 Related session ops

| Method | Behavior |
|--------|----------|
| `session/list` | Flush live sessions, list stored by cwd (or all), cursor pagination (page size 50) |
| `unstable_session/fork` | Open source path, `switchSession` + `fork()`, refuse if source prompt in-flight |
| `session/close` | Mark closed, cancel prompt, dispose MCP + session |
| `session/set_mode` | default ↔ plan; emits `current_mode_update` + config options |
| `session/set_config_option` | mode / model / thinking; returns updated options |

### 4.5 `session/prompt`

Lifecycle carefully layered for reliability:

1. **Implicit cancel** if previous turn still streaming when a new prompt arrives (client message after stop without cancel).
2. **Per-session prompt queue** (`#queuePrompt`) serializes turns.
3. Wait for previous turn’s promise + cancel cleanup; throw if record closed.
4. Convert prompt blocks (text, image, embedded resource, resource_link; audio → placeholder).
5. Subscribe to session events → map to ACP updates (tool starts/ends, message/thought chunks, etc.).
6. Run skill slash / ACP builtin slash / extension command / normal `session.prompt`.
7. On `agent_end`:
   - flush missed final assistant text if only thoughts were delivered (#4902)
   - flush unreported provider errors as message chunks
   - emit usage + session_info
   - drain async job deliveries (with temporary `allowAcpAgentInitiatedTurns`)
   - resolve prompt with `stopReason` + turn `usage` delta

**Stop reasons:** `end_turn` | `cancelled` | `max_tokens` | `refusal` (content-filter heuristics).

Slash / skill / extension-only commands can complete with `end_turn` without a model turn.

### 4.6 `session/cancel` (and implicit cancel)

`#beginCancelCleanup` (idempotent):

1. Mark `cancelRequested`, unsubscribe live events.
2. **Immediately** resolve prompt with `stopReason: "cancelled"` (client sees acceptance without waiting for abort).
3. Race `session.abort({ reason: USER_INTERRUPT_LABEL })` vs **5s** timeout (`ACP_CANCEL_CLEANUP_TIMEOUT_MS`).
4. On timeout: **close the managed session** (do not leave a half-aborted session registered).

Queued prompts after a timed-out cancel fail with the cleanup error / closed error rather than running on a dying session.

`code:acp-agent.ts` `cancel`, `#beginCancelCleanup`, `prompt` implicit cancel branch  
`test:acp-agent.test.ts` (auto-cancel mid-flight, cleanup timeout closes session)

### 4.7 Bootstrap race guard

`ACP_BOOTSTRAP_RACE_GUARD_MS = 50`

Zed (and similar clients) can race response handling vs first notifications, logging “session notification for unknown session” and **dropping** slash-command palette updates. OMP deliberately delays first notifications and lifetime subscription install by 50ms so the client registers the session id first.

`code:acp-agent.ts` `#scheduleBootstrapUpdates`  
`test:acp-agent.test.ts` bootstrap suppression tests

---

## 5. Permissions: client present vs default / yolo

### 5.1 Two layers

1. **OMP tool approval** (`tools.approvalMode`, `tools.approval.<tool>`) — same resolver as TUI/RPC.  
2. **ACP client permission gate** — additional proxy on tools when ClientBridge is attached.

`doc:docs/approval-mode.md` (ACP sessions + Subagents sections)

### 5.2 ClientBridge always installs permission capability

```ts
// acp-client-bridge.ts
requestPermission: true  // always usable; policy is agent's choice
```

Bridge is set on every registered ACP session (`#registerPreparedSession`), regardless of whether the hub/client will auto-reject.

### 5.3 Which tools are gated

`PERMISSION_REQUIRED_TOOLS = bash | edit | delete | move`

- `bash`: always asks (title = command prefix); kind `execute`.
- `delete` / `move`: path titles.
- `edit`: **only destructive intents** (delete/move ops inside edit/patch); plain file edits skip the ACP permission gate (still subject to OMP approval tiers).

Options presented:

| optionId | kind |
|----------|------|
| allow_once | allow_once |
| allow_always | allow_always |
| reject_once | reject_once |
| reject_always | reject_always |

`allow_always` / `reject_always` cached per `cacheKey` (e.g. tool name or `edit:delete`) on the session.

Reject / cancel → `ToolError` or `ToolAbortError` — **never silent allow**.

`code:agent-session.ts` `#wrapToolForAcpPermission`, `getPermissionIntent`  
`test:agent-session-acp-permission.test.ts`

### 5.4 When the gate is skipped

Gate is skipped only on **explicit** auto-approve:

- CLI `--yolo` / `--auto-approve` / `--approval-mode yolo`, or
- settings where `tools.approvalMode` is **configured** as `yolo` (`settings.isConfigured(...)`)

**Important subtlety:** schema default of `yolo` does **not** count as explicit configuration for ACP. Default-config ACP sessions **keep the client permission gate**. Operators must set `tools.approvalMode: yolo` explicitly (config file or runtime flag) for unattended execution.

Per-tool `tools.approval.<tool>` of `prompt` or `deny` still applies even under yolo.

There is **no** ACP `session/new` field for approval policy; per-session yolo requires a separate process or process-level `--config` / flags.

`doc:docs/approval-mode.md`  
`code:agent-session.ts` `#isExplicitAutoApproveMode`

### 5.5 Generic approvals vs permission gate

- Client-gated tools → ACP `session/request_permission`.
- Other OMP approval prompts → form elicitation when `elicitation.form` is advertised; otherwise reject/cancel semantics (no silent allow).

### 5.6 Client fs/terminal routing (orthogonal to permission)

When client advertises capabilities:

| Capability | Routing |
|------------|---------|
| `fs.readTextFile` | `read` tool prefers client; falls back to disk on failure |
| `fs.writeTextFile` | `write`/`edit` route via bridge for workspace files; **not** for `local://` / plan sandbox / OMP artifacts |
| `terminal` | non-PTY `bash` uses `createTerminal` (wins over auto-background) |

Hub’s sample with fs/terminal false means tools run **locally inside the OMP process**, still subject to permission gate if not yolo.

`code:tools/acp-bridge.ts`, `tools/read.ts`, `tools/bash.ts`

### 5.7 Hub sample interaction

`adapters/omp/agents.json`:

- `permission_policy: "reject"` → hub answers permission requests with reject.
- Client fs/terminal disabled → no buffer/terminal hosting.

Combined with OMP default (gate on unless explicit yolo): destructive tools will hit client permission and be **rejected** unless hub policy or OMP yolo changes. That is intentional least-privilege layering, not an OMP bug.

---

## 6. Config options and modes

### 6.1 Session modes (`modes` / `setSessionMode`)

| id | Name | When available | Effect |
|----|------|----------------|--------|
| `default` | Default | Always | Standard tools |
| `plan` | Plan | `plan.enabled` in settings | Read-only plan mode; standing `resolve` handler for plan approval |

Plan approval:

- Uses form elicitation when available (`Approve and execute` vs `Refine plan`).
- **Without** form elicitation: **auto-approve** so plan mode cannot strand the agent.
- On approve: set plan reference, exit plan mode, emit mode + config updates.
- On refine/dismiss: stay in plan mode.

`code:acp-agent.ts` `#applyModeChange`, `#runAcpPlanApprovalResolve`, `#requestAcpPlanApprovalChoice`

### 6.2 Config options (`configOptions`)

| id | category | Values |
|----|----------|--------|
| `mode` | mode | default / plan |
| `model` | model | `provider/id` from available models |
| `thinking` | thought_level | off, auto, model-specific levels |

Changes via `setSessionConfigOption` or lifetime events (thinking_level_changed after bootstrap).

### 6.3 Process-level ACP host defaults

`applyAcpDefaultSettingOverrides` resets (only if **not** already configured):

- `task.isolation.*`, `task.eager`, `task.batch`, `task.maxConcurrency`, `task.maxRecursionDepth`, `task.disabledAgents`, `task.agentModelOverrides`
- `memory.backend`, `memories.enabled`
- `advisor.*`, `tier.advisor`

**Does not** force-reset async/bash auto-background the way RPC mode does (RPC has extra `RPC_BACKGROUND_DEFAULTED_SETTING_PATHS`).

Rationale: protocol hosts should get neutral product defaults, not the operator’s interactive TUI preferences, while honoring explicit project/`--config` choices.

`code:main.ts` `HOST_DEFAULTED_SETTING_PATHS`, `applyAcpDefaultSettingOverrides`

### 6.4 Extension / custom methods

`extMethod` proprietary surface (`_omp/...`): list all sessions, projects-by-cwd, chats by cwd, usage reports, extensions list/toggle; plus `speech.models.list`.

Extension UI in ACP: form elicitation only for select/confirm/input when client supports it; TUI-only surfaces stubbed.

### 6.5 Slash commands

ACP builtins filtered to commands with text-mode `handle` (no `/quit` dashboards). Skills and extensions participate via available_commands_update. Plugin reload re-emits commands.

---

## 7. Subagents and ACP

### 7.1 Do subagents share the ACP connection?

**No separate ACP connection.** Subagents are **in-process** `AgentSession`s spawned by the `task` executor under the parent session. They do **not**:

- implement ACP methods
- call `setClientBridge`
- appear as separate ACP session ids

There are **zero** references to ClientBridge under `src/task/` and **zero** “subagent” references under `src/modes/acp/`.

Contrast: RPC mode has explicit `rpc-subagents.ts` frames (`subagent_lifecycle` / `progress` / `event`). ACP has no analogous protocol surface — subagent activity is folded into the **parent** session’s tool/events stream (parent `task` tool execution).

### 7.2 Permission model for subagents

From task executor settings snapshot:

```ts
"tools.approvalMode": "yolo"  // explicit override in isolated settings
```

Docs: parent `task` approval is the authorization boundary; subagents must not block on UI. User `tools.approval.<tool>` still allow/deny.

Because subagents have **no ClientBridge**, they also skip the ACP permission proxy entirely and use local fs/bash. Parent turn may still have been client-gated when `task` started (if `task` is not in PERMISSION_REQUIRED_TOOLS — note only bash/edit/delete/move are gated).

`code:task/executor.ts` (~798–801, createAgentSession with `hasUI: false`)  
`doc:docs/approval-mode.md` Subagents section

### 7.3 Implications for a multi-agent hub

- Hub sees **one** vendor ACP endpoint process with **N** OMP sessions.
- Parallel work inside OMP is product-internal (task/subagent tree), not multi-process ACP agents.
- Hub permission policy applies to the **parent** session’s gated tools only; subagent tools run under OMP yolo + local IO.

---

## 8. Reliability / design choices that make ACP usage stable

| Choice | Why it matters |
|--------|----------------|
| **Stdout purity** | NDJSON only; human notices on stderr when TTY |
| **Protocol owns stdin** | Never treat stdin as prompt in acp/rpc |
| **MCP client ownership** | `enableMCP: false`; only `mcpServers` from client — no host `.mcp.json` shadowing |
| **Bootstrap 50ms guard** | Avoids client dropping first session notifications (Zed race) |
| **Immediate cancel acceptance** | Prompt resolves `cancelled` before abort completes |
| **Cancel timeout → session close** | Prevents zombie streaming sessions after hung abort |
| **Implicit cancel on new prompt** | Handles stop-then-type without explicit cancel |
| **Prompt queue + in-flight predicate** | Settled-but-aborting still blocks fork/queue/event forwarding |
| **Missed text / error flush on agent_end** | Fixes silent turns when message_end races or errors never stream |
| **Async delivery drain at turn end** | Temporarily allows agent-initiated delivery so jobs finish into the same turn |
| **`deferAgentInitiatedTurns`** | Blocks server-started turns after prompt response (ACP v1 busy-state limitation); only drained under controlled end-of-turn path |
| **Idempotent cancel cleanup** | Concurrent cancel + new prompt safe |
| **Connection abort disposes all** | No leaked sessions/MCP children when client dies |
| **Explicit yolo for gate skip** | Safer default with IDE hosts that implement request_permission |
| **Plan mode auto-approve without elicitation** | Avoids stranded read-only mode on limited clients |
| **Absolute cwd enforcement** | Deterministic session listing/resume |
| **History replay on load only** | Resume cheap reattach; load rebuilds UI transcript |
| **Fork blocked during prompt** | Consistent fork source snapshot |
| **Per-cwd settings + title policy** | Multi-workspace clients get correct project config |
| **Image/blob handling + BlobStore** | Stable media in updates/replay |
| **Conformance tests** | initialize contract, MCP isolation, stdout hygiene, permission gate, cancel races, bootstrap races |

Constants of note:

- `ACP_BOOTSTRAP_RACE_GUARD_MS = 50`
- `ACP_CANCEL_CLEANUP_TIMEOUT_MS = 5_000`
- `ACP_ASYNC_DELIVERY_DRAIN_TIMEOUT_MS = 250` (up to 3 passes)

---

## 9. End-to-end call sketch

```text
Client spawns: omp acp
  → main applies ACP host defaults, builds createAcpSessionFactory
  → runAcpMode: NDJSON stdio, AcpAgent

Client: initialize
  → store clientCapabilities; advertise agent caps + auth methods

Client: authenticate (optional)
  → agent | terminal only

Client: session/new { cwd, mcpServers }
  → AgentSession (no host MCP)
  → ClientBridge from client caps
  → extensions session_start
  → connect client MCP servers
  → response { sessionId, configOptions, modes }
  → +50ms: available_commands_update, session_info_update

Client: session/prompt
  → queue → convert blocks → subscribe events
  → model/tools; permissions via request_permission when needed
  → agent_end → flush text/error → usage → resolve stopReason

Client: session/cancel
  → immediate cancelled response; abort with 5s timeout

Client dies / stdin closes
  → connection.closed → dispose all sessions → exit 0
```

---

## 10. Hub adapter notes (OMP via acp-hub)

From `repos/acp-hub/adapters/omp/README.md` + `agents.json`:

- Transport: stdio `omp` + args `["acp"]`.
- Registry does **not** reimplement OMP models/auth/session semantics — “recheck after OMP upgrade.”
- Sample least privilege: reject permissions, disable hub fs/terminal.
- Operators inspect live ads: `acp-hub param list` / `mode list`.

This is a **thin registration**, not a proxy that understands OMP session files.

---

## Design principles (extractable for multi-agent hub comparison)

- **One process, one protocol connection, many durable sessions** — session map is the concurrency unit, not multi-process forking per chat.
- **Stdout is sacred** — transport channel never shares human UI; diagnostics go stderr/logs.
- **Client owns the integration surface** — MCP list, fs/terminal capabilities, permission UI; agent refuses to invent silent fallbacks that grant power.
- **Fail closed on safety UX** — reject/cancel permission; never “allow because UI missing” for destructive tools (plan-mode exception is the inverse: auto-approve exit so the agent is not stranded).
- **Explicit opt-in for unattended power** — schema defaults ≠ ACP gate skip; yolo must be configured or flagged at process level.
- **Host defaults neutralize interactive preferences** — protocol mode re-applies product defaults for workflow-altering settings unless the host/project configured them.
- **Race-aware bootstrap** — delay first notifications until the client can know the session id.
- **Cancel is a first-class lifecycle** — immediate protocol completion + bounded abort + session eviction on hang; new prompts auto-cancel previous turns.
- **Turn completion is loss-averse** — flush missed text and errors before resolving prompt; drain async deliveries within the same turn.
- **Load vs resume are different product ops** — load rebuilds client transcript; resume reattaches without replay cost.
- **Internal multi-agent ≠ multi-ACP** — subagents stay in-process, yolo, no ClientBridge, no separate ACP session; parent task is the hub-visible authorization/tool boundary.
- **Vendor endpoint authenticity** — advertise real version/info; reject unknown auth methods; don’t claim transports you don’t implement.
- **Per-workspace fidelity** — absolute cwd, per-cwd settings clone, per-cwd title policy for multi-root clients.
- **Dispose on connection death** — abort signal tears down all sessions/MCP; no orphan work after client exit.
- **Thin hub registration** — conductor registers `omp acp` and applies **client** policy; it does not re-encode OMP’s internal task/subagent model.

---

## Source index

| Area | Primary paths |
|------|----------------|
| Agent protocol | `repos/ref_repos/oh-my-pi/packages/coding-agent/src/modes/acp/acp-agent.ts` |
| Transport | `.../modes/acp/acp-mode.ts` |
| Client bridge | `.../modes/acp/acp-client-bridge.ts` |
| Event map | `.../modes/acp/acp-event-mapper.ts` |
| CLI | `.../commands/acp.ts`, `.../main.ts` |
| Permission gate | `.../session/agent-session.ts`, `.../session/client-bridge.ts` |
| Approval docs | `repos/ref_repos/oh-my-pi/docs/approval-mode.md` |
| RPC contrast | `repos/ref_repos/oh-my-pi/docs/rpc.md` (subagent frames; different product surface) |
| Subagent yolo | `.../task/executor.ts` |
| Hub adapter | `repos/acp-hub/adapters/omp/*` |
| Tests | `packages/coding-agent/test/acp-*.ts`, `agent-session-acp-permission.test.ts` |
