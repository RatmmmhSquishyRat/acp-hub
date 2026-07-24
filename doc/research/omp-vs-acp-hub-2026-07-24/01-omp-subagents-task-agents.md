# OMP Subagents / Task Agents — Design Reference

**Date:** 2026-07-24  
**Scope:** Oh My Pi (OMP) coding-agent task tool, subagent lifecycle, approval, session continuity, and ACP/RPC surfaces related to tasks.  
**Mode:** Evidence-first gold-standard reference. No acp-hub proposals.

Sources: OMP docs under `repos/ref_repos/oh-my-pi/docs/` and implementation under `repos/ref_repos/oh-my-pi/packages/coding-agent/src/`. Paths below are relative to `repos/ref_repos/oh-my-pi/` unless absolute.

---

## 1. Mental model (task vs subagent vs main session)

### Three nested identities

| Concept | What it is | Identity / storage |
|--------|------------|-------------------|
| **Main session** | User-facing agent loop (TUI / RPC / ACP). Registry id is always `"Main"`. | Session JSONL under `~/.omp/agent/sessions/<dir-encoded>/<timestamp>_<sessionId>.jsonl`. Tree of entries + leaf pointer. |
| **Task tool call** | Parent tool invocation named `task` (`approval: "exec"`). One call can spawn one or many subagents (batch). | Parent conversation tool block; progress/results live in tool `details`. Background spawns also register async jobs. |
| **Subagent (task agent)** | In-process child `AgentSession` with its own system prompt, tool set, transcript, and registry id. Starts with **blank conversation history**. | Registry id = CamelCase name (e.g. `SwiftRiver`); artifacts `<id>.md` / `<id>.jsonl` under parent session artifacts dir. |

The main session is the conductor. The `task` tool is the spawn API. Each subagent is a full agent session, not a function call with a prompt fragment.

### Agent *type* vs agent *instance*

- **Agent definition** (`AgentDefinition`): markdown frontmatter + body discovered at spawn time (`scout`, `task`, `reviewer`, custom `.omp/agents/*.md`, …). Fields: `name`, `description`, `systemPrompt`, optional `tools`, `spawns`, `model`, `thinkingLevel`, `output`, `blocking`, `autoloadSkills`, `readSummarize`, `source`, `filePath`.
- **Agent instance**: one registry entry created by a spawn. Id comes from wire `name` or generated AdjectiveNoun (`name-generator.ts`), uniquified by `AgentOutputManager` (`Anna`, `Anna-2`, nested `Parent.Child`).

### Execution modes (orthogonal axes)

1. **Sync vs async (parent-level)**  
   - `async.enabled` default **true**: non-blocking spawns register `AsyncJobManager` jobs and return immediately with agent/job ids; final result injects into the parent conversation later.  
   - `async.enabled` false, no job manager, or **every** item is `blocking: true`: parent waits; tool returns settled summaries.

2. **Per-item blocking**  
   - Agent frontmatter `blocking: true` → that item runs **inline** even when async is on (mixed batches: blocking results in `results[]`, others background).  
   - Bundled `scout` docs historically cite blocking; current `scout.md` frontmatter does **not** set `blocking: true` — only explicit frontmatter enables it.

3. **Batch vs flat wire shape**  
   - `task.batch` default **true**: `{ context, tasks: [{ name?, agent?, task, isolated? }] }`. Shared `context` is required and prepended into every child system prompt.  
   - Flat: one `{ name?, agent?, task, isolated? }` per call. Shared background via `local://` files (parent and children share `local://` root).

4. **Isolation**  
   - `task.isolation.mode !== "none"` and per-item `isolated: true`: workspace clone (PAL backends: APFS, btrfs, overlayfs, ProjFS, rcopy, …), patch or branch merge, then workspace teardown. Isolated agents are **not revivable**.

### What a subagent is *not*

- Not a separate OS process by default: `runSubprocess` is **in-process** (`executor.ts` comment: "Run a single agent in-process"). Env `PI_SUBPROCESS_CMD` remains documented for spawn-command override but the core path is same-process session creation.
- Not a continuation of parent chat: no parent transcript inheritance; only explicit carry-over (workspace tree, skills, context files, batch `context`, plan reference, shared artifacts/`local://`, MCP proxy tools).
- Not a peer of the main agent for advisor kind: registry `kind: "advisor"` is observability-only, not messageable.

### Process-global registry status machine

`AgentRegistry` (`registry/agent-registry.ts`):

| Status | Meaning |
|--------|---------|
| `running` | Turn in flight |
| `idle` | Live `AgentSession` in memory, finished or waiting; still addressable |
| `parked` | Session disposed; `AgentRef` + `sessionFile` kept for revive |
| `aborted` | Hard-killed; terminal |

Lifecycle owner: `AgentLifecycleManager` (idle TTL → park → `ensureLive` revive). Main is never parked.

### Completion contract: `yield`

Child sessions set `requireYieldTool: true`. They must finish via the hidden `yield` tool. Up to **3** reminder prompts; last can force `toolChoice = yield`. Output assembly reconciles yield payload, structured schema, `report_finding`, abort/salvage text (`finalizeSubprocessOutput`).

### Depth / tree

- Parent `taskDepth` (main = 0); child = parent + 1.
- `task.maxRecursionDepth` default **2**: at max depth, child loses `task` tool and `spawns` becomes `""`.
- Parent spawn policy from session `spawns` (`*` / CSV / empty) further restricts which agent *types* children may request.

---

## 2. Parameters / config inheritance

### Wire parameters (task tool)

| Field | Role |
|-------|------|
| `context` | Batch-only, required: shared Goal/Constraints/Contract background → child system prompt `CONTEXT` section |
| `tasks[]` / flat `task` | Self-contained assignment; prompt template requires Target / Change / Acceptance sections |
| `name` | Registry/IRC/artifact id (optional; generated if omitted) |
| `agent` | Agent *type*; omit for spawn-policy default (usually `task`); never pass the default name explicitly (prompt guidance) |
| `isolated` | Only when isolation mode ≠ `none` |

**No** per-call `schema` parameter. Structured output: agent frontmatter `output` → else parent session `outputSchema` → else eval bridge `agent(prompt, schema)`.

### Settings that govern tasks (`settings-schema.ts`)

| Key | Default | Role |
|-----|---------|------|
| `async.enabled` | `true` | Background jobs for non-blocking spawns |
| `task.batch` | `true` | Batch wire schema |
| `task.maxConcurrency` | `32` | Session-scoped semaphore (0 = unlimited); sized at first use |
| `task.maxRecursionDepth` | `2` | Nesting cap (`-1` unlimited, `0` no child spawns) |
| `task.maxRuntimeMs` | `0` | Wall-clock abort per spawn (0 = off) |
| `task.agentIdleTtlMs` | `420000` (7 min) | Idle → park TTL (`<=0` never park) |
| `task.softRequestBudget` | `200` | Soft request budget; 1.5× force-stop; scout/sonic built-in 100 |
| `task.softRequestBudgetNotice` | `true` | Steering notice at soft budget |
| `task.enableLsp` | `false` | Child LSP (also needs parent LSP) |
| `task.disabledAgents` | `[]` | Hard-deny agent type names |
| `task.agentModelOverrides` | `{}` | Per-type model override map |
| `task.eager` | `default` | Prompt pressure to delegate (`preferred` / `always`) |
| `task.isolation.mode` | `none` | Isolation backend selection |
| `task.isolation.merge` | `patch` | `patch` \| `branch` |
| `task.isolation.commits` | `generic` | Nested-repo commit style |
| `worktree.base` | unset → `~/.omp/wt` | Isolation/worktree base (`OMP_WORKTREE_DIR` overrides) |
| `tier.subagent` | `inherit` | Service-tier policy for children |
| `advisor.subagents` | `false` | Advisor on spawned sessions |
| `modelRoles.task` | (role) | Default model role for `pi/task` agent |

Settings precedence (all features): built-in defaults ← global `~/.omp/agent/config.yml` ← project `<cwd>/.omp/config.yml` ← `--config` overlays ← runtime flags. Project settings are **cwd-local only** (no ancestor walk).

### What parent passes into `runSubprocess` / child session

From `TaskTool.#runSpawn` → `sharedRunOptions` and `buildSubagentSessionOptions`:

**Passed / shared**

- `cwd` (or isolation worktree path)
- Full parent `settings` snapshot, then **mutated** into isolated subagent settings
- Auth storage + model registry (same instances; model refresh skipped when reusing parent registry)
- Model resolution: `task.agentModelOverrides[type]` > agent frontmatter `model` > parent active model pattern / roles; auth fallback to parent model if child model has no credentials
- Thinking: model pattern `:level` suffix > agent `thinkingLevel` > pattern-derived
- Context files (parent list **minus** `AGENTS.md` basename), skills, workspace tree, rules, prompt templates
- Preloaded extension/custom-tool paths
- MCP: parent `MCPManager` → **proxy tools** with 60s timeout; child disables standalone MCP discovery when manager present
- Artifact manager **adoption** (shared ID space + parent artifacts dir)
- `local://` protocol options (shared root)
- Plan reference when parent is executing an approved plan (not during plan-mode exploration)
- Batch `context` string into system prompt
- Parent agent id, task depth, telemetry handoff, optional hindsight/mnemopi session state, eval session id
- Parent service tiers for `tier.subagent: inherit`
- Autoload skills from agent definition matched against available skills

**Child-owned / forced**

- New session file `<artifactsDir>/<id>.jsonl` (or in-memory if no artifacts)
- New system prompt (agent body + context/plan/worktree/schema/IRC roster), `hasUI: false`
- Tool list: agent `tools` if set; auto-add `task` when `spawns` and depth allow; always ensure `irc` if whitelist present; expand `exec` → `eval`+`bash`; **strip parent-owned `todo`**
- `requireYieldTool: true`
- Spawns env string derived from agent + depth
- Output schema: `effectiveAgent.output ?? parent.outputSchema`
- Child-internal async **disabled**: `async.enabled = false`, `bash.autoBackground.enabled = false` (no fire-and-forget grandchildren)
- Approval: `tools.approvalMode: yolo` (see §3)

### Agent discovery (type registry)

Order, first-wins by exact name (`docs/task-agent-discovery.md`):

1. Project `.omp/agents`
2. User `~/.omp/agent/agents`
3. Claude plugin `agents/` dirs (if provider enabled)
4. Bundled: `scout`, `designer`, `reviewer`, `librarian`, `task`, `sonic`

Cross-harness dirs (`.claude/agents`, `.codex/agents`, `.gemini/agents`) are **skipped** for task agents (schema mismatch).

Create-time discovery is memoized for the tool description; **execution-time** `discoverAgents` is always fresh.

### Env overrides (task-related)

| Env | Effect |
|-----|--------|
| `PI_BLOCKED_AGENT` | Deny spawning that agent type from within itself (recursion guard at tool construction) |
| `PI_TASK_MAX_OUTPUT_BYTES` | Default 500_000 — truncated return to parent; full still in `<id>.md` |
| `PI_TASK_MAX_OUTPUT_LINES` | Default 5000 |
| `PI_SUBPROCESS_CMD` | Documented spawn command override |
| `OMP_WORKTREE_DIR` | Isolation worktree base |

### Plan mode mutation

When parent plan mode is enabled, `effectiveAgent` gets plan-mode system prompt prefix, tools restricted to read-like allowlist (`read`, `grep`, `glob`, `lsp`, `web_search` + agent-declared `ast_grep`/`report_finding`), and **spawns cleared**.

---

## 3. Permissions / approval

### Tool tiers and modes

Every tool may declare `approval`: `read` | `write` | `exec` (or dynamic function). Omitted → **exec**. MCP tools default **write**.

User mode `tools.approvalMode`:

| Mode | Auto-approves | Prompts |
|------|---------------|---------|
| `always-ask` | read | write, exec |
| `write` | read, write | exec |
| `yolo` (schema **default**) | read, write, exec | none (unless user policy) |

Per-tool overrides: `tools.approval.<name>: allow | deny | prompt`. Safety `override: true` on tools (e.g. critical bash patterns) forces prompt in non-yolo modes; in yolo, overrides do not force prompt unless user policy is `prompt`/`deny`.

Flags: `--yolo` / `--auto-approve` / `--approval-mode` force runtime mode.

### Task tool itself

- `TaskTool.approval = "exec"` — spawning is an exec-tier action.
- `formatApprovalDetails`: agent type, name, task preview (assignment text).
- Approving/allowing `task` is the **authorization boundary** for all child work.

### Subagent approval policy

`createSubagentSettings` forces:

```text
tools.approvalMode: yolo
```

Rationale (code comment): headless children must not stall on UI. User `tools.approval.*` policies still apply for allow/deny/prompt resolution inside the child. Parent already authorized the spawn.

### ACP client gate (separate from OMP approval)

ACP (`omp acp`) uses the same settings stack. Important split:

1. **OMP approval wrapper** — mode + per-tool policy.
2. **ACP client permission gate** — `session/request_permission` for tools in a fixed set (`bash`, `edit`, `delete`, `move` per docs; implemented via `#wrapToolForAcpPermission` and `PERMISSION_REQUIRED_TOOLS`).

Yolo **schema default** does **not** skip the ACP client gate. Gate is skipped only when **explicit** auto-approve is set:

- CLI/SDK `autoApprove`, or  
- `settings.isConfigured("tools.approvalMode") && value === "yolo"`

(documented in `docs/approval-mode.md` and `agent-session.ts` `#isExplicitAutoApproveMode`).

Launch helpers: `omp acp --yolo`, `--auto-approve`, `--approval-mode yolo`, or `--config` with `tools.approvalMode: yolo`.

When permission is required:

- Client-gated tools → ACP `session/request_permission` (`acp-client-bridge.ts` → `connection.requestPermission`).
- Generic OMP approval prompts → form elicitation if client advertises `elicitation.form`.
- Rejected / cancelled / unsupported → tool fails; **no silent allow**.
- Persistable allow_always / reject_always decisions cached per session.

ACP does **not** define per-`session/new|load|resume` approval fields; per-session yolo needs process flags or overlay config.

### Subagents under ACP

Because children force `tools.approvalMode: yolo` and run headless, they do not drive the TUI approval UI. Whether ACP client gates still wrap child tools depends on child session construction and client bridge attachment (children reuse parent MCP; permission wrapping is on `AgentSession` tool activation). Design intent remains: **parent `task` approval is the human authorization boundary**; children are unattended.

### Eager delegation prompt (policy soft-layer)

`prompts/system/eager-task.md` (when `task.eager` is preferred/always): instruct main agent to fan out to `task` after scoping design; exceptions for tiny single-file edits / direct answers / single slice. This is prompt policy, not a hard permission.

---

## 4. Connection / process lifecycle

### Spawn pipeline (summary)

1. Validate/repair params; resolve spawn items and default agent from parent `getSessionSpawns()`.
2. Split blocking vs async; async → allocate ids, register jobs, return; jobs acquire semaphore then `#executeSync`.
3. `#runSpawn`: rediscover agents; deny unknown / disabled / blocked / spawn-policy; plan-mode rewrite; prepare isolation if requested.
4. Allocate id; mkdir artifacts; call `runSubprocess` or `runIsolatedSubprocess`.
5. `runSubprocess`: build subagent settings; open child session JSONL; `createAgentSession`; register status sync; append `session_init` (systemPrompt, task, tools, spawns, outputSchema, readSummarize); drive until yield; write `<id>.md`; finalize lifecycle.

### Isolation

- Requires git repo for isolated mode.
- PAL (`pi-natives` / `crates/pi-iso`): resolve backend with fallback chain.
- Merge: patch apply or branch `omp/task/<id>` cherry-pick; nested repos patched separately.
- After merge: workspace cleaned; agent parked **without** reviver.

### Concurrency & budgets

- One `Semaphore` per `TaskTool` instance from `task.maxConcurrency`.
- Soft request budget → wrap-up notice → force yield path → hard abort after grace (`BUDGET_STOP_GRACE_REQUESTS = 5`).
- Wall clock `task.maxRuntimeMs` → abort reason `timeout`.
- MCP proxy calls: 60_000 ms timeout.

### Output capture limits

| Layer | Limit | Notes |
|-------|-------|-------|
| Parent-facing tool output | `MAX_OUTPUT_BYTES` / `MAX_OUTPUT_LINES` (env-overridable) | Truncated tail in `SingleResult.output` |
| Summary preview | 5000 chars | Points to `agent://<id>` for full |
| Full raw | written to `<id>.md` | Always when artifacts dir available |
| Progress | coalesce 150 ms; recent output tail 8 KiB | Event bus channels |
| Session JSONL persistence | `MAX_PERSIST_CHARS` 500k; images → blob store | Separate from task output |

### Kill / abort

| Trigger | Registry outcome | Revivable? |
|---------|------------------|------------|
| Parent tool-call abort / job cancel / hard signal | `aborted`, session disposed | No |
| Wall-clock timeout / terminate | `aborted` | No |
| Soft-budget stop | Can remain keep-alive (`resumableAbort` when `abortKind === "budget"`) | Yes if keep-alive + non-isolated |
| Isolated completion | `parked` without reviver | No (transcript only via `history://`) |
| Normal finish / soft fail | `idle` + lifecycle adopt | Yes (irc / ensureLive) |

`keepAlive` defaults true for task spawns; one-shot helpers can dispose + unregister.

### Process boundary (ACP / RPC hosts)

- Main session may be the ACP agent process (`modes/acp/*`) or RPC stdio process (`modes/rpc/*`).
- Subagents are **not** separate ACP sessions; they are internal to the OMP process.
- RPC can subscribe to subagent traffic: `set_subagent_subscription` level `off` | `progress` | `events`; query `get_subagents`, `get_subagent_messages`.
- RPC mode resets some workflow settings (`task.*`, `async.*`, …) to built-in defaults rather than user overrides (host predictability).

### Idle park / revive

- After keep-alive finish: status `idle`, `AgentLifecycleManager.adopt(id, { idleTtlMs, revive })`.
- TTL expires → dispose session, `parked`, retain sessionFile.
- Message (irc) or hub focus → `ensureLive` → reviver reopens JSONL (writer closed on park so lock is free) with same tools/prompt contract.
- Cold revive after process restart: `createPersistedSubagentReviverFactory` peeks `session_init`, checks cwd still exists, rebuilds session from persisted contract + parent ambient deps (auth, models, settings, artifact manager, MCP proxies). Missing `session_init` or gone cwd → transcript-only (`history://`), no revive.

---

## 5. Resume / continuity

### What survives a successful task

1. **Parent tool result** — summary text + `details.results[]` / async injection message (agent id, job id, follow-up hint).
2. **`agent://<id>`** — full yield/report markdown under artifacts dir; JSON path/query extraction supported.
3. **`history://<id>`** — concise transcript from `<id>.jsonl` (live or parked).
4. **Child session JSONL** — full graph (messages, model/thinking changes, session_init, …).
5. **Registry peer** — idle/parked id for irc follow-up instead of respawn.
6. **Isolation patches** — `<id>.patch` / branch when isolated.

### Follow-up without respawn

- Preferred UX (tool prompt): message idle/parked agent via `irc` rather than new `task` spawn.
- `runSubagentFollowUpTurn`: `ensureLive(id)`, one more monitored turn to yield; **does not tear down** session; aborts only in-flight turn.
- Parked revive restores full message history via JSONL replay.

### Parent session resume (main)

Orthogonal but related continuity:

- JSONL append-only tree + leaf; `buildSessionContext` walks leaf→root, applies compaction/branch summaries.
- `--continue` / terminal breadcrumb; `--resume` id/path/picker; in-session `/resume` → `AgentSession.switchSession`.
- Fork copies artifacts directory; blobs are global content-addressed (`blob:sha256:…`).
- Export HTML embeds nested subagent transcripts (`collectSubSessions`).

Subagent resume does **not** use main session leaf navigation; it uses lifecycle revive + own JSONL.

### Artifacts vs blobs

| System | Scope | Used for |
|--------|-------|----------|
| Blob store `~/.omp/agent/blobs/<sha>` | Global | Large images in session entries |
| Artifacts `<sessionStem>/` | Session | Tool spill logs, `<id>.md`, `<id>.jsonl`, patches |

Subagents adopt parent `ArtifactManager` so IDs stay unique across the tree and files stay flat in the parent dir.

### Gaps / intentional limits

- Hard abort / isolated teardown: **not** follow-upable as a live agent; transcript may remain.
- In-memory parent (`--no-session`): temp artifacts dir; weaker durability.
- Process death without disk session: registry lost; cold revive only if child JSONL + session_init on disk and cwd valid.
- Children force `async.enabled=false`: nested background job trees are not a thing.

---

## 6. Operator UX that makes this design "satisfying"

These are concrete mechanisms, not slogans:

1. **Typed specialists with first-wins discovery** — `scout` (smol, read-only, structured output), `reviewer`, `librarian`, `sonic`, general `task`; project agents override bundled same names.

2. **Blank-slate + required self-contained brief** — forces the parent model to write complete assignments; batch `context` avoids N× paste of shared contracts.

3. **Background by default, results deliver themselves** — parent keeps working; progress streams into the same tool block; completion injects with irc/history pointers.

4. **Stable human-readable ids** — AdjectiveNoun / CamelCase names; `agent://` / `history://` / job ids align.

5. **Keep-alive + irc** — finished workers become addressable peers; parking reclaims memory without deleting identity; messaging revives.

6. **Authorization once at the boundary** — parent approves `task` (exec); children yolo headless so nested loops do not spam the user.

7. **Depth, budget, wall-clock, concurrency caps** — runaway fan-out is bounded without killing the product model.

8. **Yield contract** — structured completion instead of hoping the last assistant text is the answer; salvage path when cancelled mid-flight.

9. **Isolation optional, not default** — shared cwd is the common case; isolation for patch-safe parallel edits when enabled.

10. **RPC observability** — hosts can subscribe to subagent lifecycle/progress/events without reimplementing the registry.

11. **Session_init persistence** — revive rebuilds the real tool/prompt contract, not a generic top-level agent.

12. **Plan-mode safety** — auto-demotes children to read-only toolsets when planning.

13. **Eager-task soft policy** — optional prompt pressure to parallelize after design settles, without mandating always-spawn.

14. **Export/share fidelity** — nested sub-sessions visible in HTML export; operator can audit what workers did.

---

## 7. Explicit non-goals / boundaries of the design

1. **Not multi-tenant process isolation by default** — in-process sessions share the OMP process (CPU, memory, native addons). Isolation is filesystem (worktree/clone), not sandbox VM.

2. **Not parent-history inheritance** — no automatic conversation memory for children; prevents context bloat and force explicit handoff.

3. **Not independent ACP sessions per subagent** — one ACP session maps to main; children are internal. Hosts that need multi-agent ACP endpoints are outside this model.

4. **Not re-prompting the user for every child tool** — by design; parent spawn is the trust boundary. Unattended child yolo is intentional.

5. **Not unbounded recursion** — default depth 2; children of children have async forced off.

6. **Not cross-harness agent markdown** — Claude/Codex/Gemini agent dirs intentionally ignored for task discovery.

7. **Not reviving isolated workers** — workspace is gone after merge; only transcript.

8. **Not treating advisor as a peer** — kind `advisor` excluded from irc/history rosters.

9. **Not a distributed job queue** — AsyncJobManager is process-local; jobs die with the process unless artifacts remain.

10. **Not per-ACP-session approval policy fields** — yolo for ACP clients is process/config scoped.

11. **Not automatic AGENTS.md injection into children** — basename filtered out of context files (parent-owned instructions stay parent-side unless put in assignment/context).

12. **Not child-owned todo lists** — `todo` stripped as parent-owned tool.

13. **Not silent success on cancel** — abort surfaces salvage / aborted status; missing yield after retries surfaces system warnings.

14. **Not marketing “unlimited agents”** — concurrency semaphore, request budget, wall clock, and idle TTL are first-class.

---

## 8. File index

### Docs (`repos/ref_repos/oh-my-pi/docs/`)

| Path | Relevance |
|------|-----------|
| `docs/task-agent-discovery.md` | Discovery order, merge rules, spawn/depth gates, plan mode, output schema precedence |
| `docs/tools/task.md` | End-to-end task tool: inputs, outputs, flow, lifecycle, caps, isolation, errors |
| `docs/approval-mode.md` | Tiers, modes, yolo, ACP client gate, subagent yolo boundary |
| `docs/session.md` | JSONL format, entries (incl. `session_init`), leaf tree, persistence, blobs |
| `docs/session-operations-export-share-fork-resume.md` | Export/share/fork/resume/continue operator ops; subSessions in HTML export |
| `docs/session-switching-and-recent-listing.md` | Resume picker, breadcrumb, `switchSession` runtime rebuild |
| `docs/settings.md` | Layers, merge rules, catalog pointer for `task.*` |
| `docs/rpc.md` | RPC stdio protocol; subagent subscription commands/frames |
| `docs/blob-artifact-architecture.md` | Blobs vs artifacts; agent://; fork/resume numbering |
| `docs/environment-variables.md` | `PI_BLOCKED_AGENT`, `PI_TASK_MAX_OUTPUT_*`, `PI_SUBPROCESS_CMD` |

### Prompts (`packages/coding-agent/src/prompts/`)

| Path | Relevance |
|------|-----------|
| `prompts/tools/task.md` | Model-facing task tool description template |
| `prompts/tools/task-summary.md` | Settled result summary render |
| `prompts/system/eager-task.md` | Delegation pressure system reminder |
| `prompts/system/plan-mode-subagent.md` | Plan-mode child prefix (referenced) |
| `prompts/system/subagent-user-prompt.md` | Assignment wrapper |
| `prompts/system/subagent-yield-reminder.md` | Yield reminder |
| `prompts/agents/task.md` | Default worker body |
| `prompts/agents/scout.md` | Read-only scout + structured output |
| `prompts/agents/reviewer.md`, `designer.md`, `librarian.md` | Specialist agents |
| `prompts/agents/frontmatter.md` | Embedded frontmatter template |

### Task subsystem (`packages/coding-agent/src/task/`)

| Path | Relevance |
|------|-----------|
| `task/index.ts` | `TaskTool`, execute/batch/async split, `#runSpawn` |
| `task/executor.ts` | `runSubprocess`, settings, monitor, yield finalize, lifecycle finalize, follow-up turn, MCP proxies |
| `task/types.ts` | Schemas, `AgentDefinition`, caps, progress/lifecycle payloads |
| `task/discovery.ts` | Agent filesystem discovery |
| `task/agents.ts` | Bundled agents embedding |
| `task/spawn-policy.ts` | Parent `spawns` → allowed set / default |
| `task/output-manager.ts` | `agent://` id allocation |
| `task/name-generator.ts` | Default AdjectiveNoun ids |
| `task/parallel.ts` | Semaphore / concurrency helpers |
| `task/worktree.ts` | Isolation mode parse / lifecycle |
| `task/isolation-runner.ts` | Isolated spawn + merge orchestration |
| `task/persisted-revive.ts` | Cold revive factory from `session_init` |
| `task/commands.ts` | Workflow commands (parallel pattern, not agents) |
| `task/yield-assembly.ts` | Yield payload assembly |
| `task/render.ts` / `renderer.ts` | TUI/result rendering |
| `task/repair-args.ts` | Streaming/param repair |

### Registry & async

| Path | Relevance |
|------|-----------|
| `registry/agent-registry.ts` | Process-global agent directory |
| `registry/agent-lifecycle.ts` | Idle TTL, park, ensureLive |
| `async/job-manager.ts` | Background task jobs |

### Session / artifacts / URLs

| Path | Relevance |
|------|-----------|
| `session/session-manager.ts` | Session files, artifacts dir, fork/move |
| `session/session-entries.ts` | Entry types including `session_init` |
| `session/agent-session.ts` | ACP permission wrap, auto-approve detection |
| `session/artifacts.ts` | ArtifactManager |
| `session/blob-store.ts` | Content-addressed blobs |
| `internal-urls/agent-protocol.ts` | `agent://` |
| `internal-urls/history-protocol.ts` | `history://` |
| `internal-urls/artifact-protocol.ts` | `artifact://` |

### Tools / approval / IRC

| Path | Relevance |
|------|-----------|
| `tools/approval.ts` | Mode resolution |
| `tools/irc.ts` | Peer messaging; enable when spawns or taskDepth > 0 |
| `config/settings-schema.ts` | All `task.*` / `async.*` / approval keys |
| `config/settings.ts` | Legacy migrations (`task.isolation.enabled`, etc.) |

### Host modes

| Path | Relevance |
|------|-----------|
| `modes/acp/acp-mode.ts` | ACP server entry |
| `modes/acp/acp-agent.ts` | ACP session orchestration (plan approval, turns) |
| `modes/acp/acp-client-bridge.ts` | `requestPermission`, fs, terminal bridges |
| `modes/acp/acp-event-mapper.ts` | Event mapping (incl. todo phases) |
| `modes/rpc/rpc-mode.ts` / `rpc-types.ts` | RPC commands including subagent subscription |
| `sdk.ts` | `createAgentSession`, child wiring notes |
| `main.ts` | CLI approval overrides, session creation |

### Discovery helpers

| Path | Relevance |
|------|-----------|
| `discovery/helpers.ts` | `parseAgentFields` (tools, spawns, blocking, output, readSummarize) |

---

## Appendix A — Lifecycle state diagram (textual)

```text
task call
   │
   ├─[async + non-blocking]──► job queued ──► semaphore ──► runSubprocess
   │
   └─[sync or blocking]─────────────────────► runSubprocess
                                                    │
                              createAgentSession (hasUI=false, yield required)
                                                    │
                                              running (registry)
                                                    │
                                         driveSessionToYield
                                                    │
                    ┌───────────────┬───────────────┴───────────────┐
                    │               │                               │
              hard abort      isolated done                   normal/fail
                    │               │                               │
                aborted           parked                     idle + adopt
              (dispose)     (no reviver;                  (TTL timer)
                             history:// only)
                                                                    │
                                                          idle TTL expires
                                                                    │
                                                                 parked
                                                                    │
                                              irc / ensureLive / hub focus
                                                                    │
                                                            revive (JSONL)
                                                                    │
                                                                   idle
```

## Appendix B — Inheritance cheat sheet

| Concern | Parent | Child |
|---------|--------|-------|
| Conversation history | Full | Empty (assignment + optional context only) |
| Settings | Source snapshot | Isolated copy; async/bg bash off; approvalMode yolo |
| CWD | session cwd | same, or isolation worktree |
| Tools | active set | agent tools / filtered; no todo; depth-gated task |
| MCP | manager | proxies, no rediscovery |
| Artifacts | manager | adopted (shared) |
| Model | active | override chain + auth fallback |
| Service tier | live map | `tier.subagent` inherit or override |
| Approval UI | TUI / ACP client | none (yolo); parent task was the gate |
| Async jobs | may use | forced off |
| Persistence | main JSONL | sibling `<id>.jsonl` under artifacts |

---

*End of Lane A reference. Comparison to acp-hub belongs in later files in this research pack, not here.*
