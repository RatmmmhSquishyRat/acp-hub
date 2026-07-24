# ACP Hub architecture map (as implemented)

> **SUPERSEDED snapshots (2026-07-24):** sections that state default least
> privilege / lag connection-fatal describe **pre-UX-rebalance** code. Current
> defaults and lag policy: `doc/ssot/agent-managed/pillars/Product-UX.md` and
> live `endpoint.rs` / `daemon/rpc_io.rs`. Do not use this file as product law.

**Date:** 2026-07-24  
**Scope:** code + durable docs under `repos/acp-hub`  
**Lane:** C of `00-WORKFLOW.md`  
**Authority chain:** pillars → `doc/dev/spec.md` / `design.md` → implementation in `crates/hub`, `crates/cli`, `adapters/*`  
**Related operational evidence:** `doc/dev/cursor-adapter/e2e-investigation-2026-07-24.md`

This document describes ACP Hub **as implemented**, not as a product wishlist. Claims are grounded in source paths. Where docs and code diverge, both sides are called out.

---

## 1. Role boundary (what hub is / is not)

### What it is

ACP Hub is a **generic ACP client + conductor facility**:

| Role | Meaning in this codebase |
|------|--------------------------|
| **Registry** | Register ACP Agent Endpoints and stdio Proxies in `agents.json` (MCP-like object map: `acpAgents` / `acpProxies`). `code:crates/hub/src/endpoint.rs` |
| **Client** | Speaks official ACP v1 over stdio / HTTP / WebSocket via the rust-sdk and project bounded transports. `code:crates/hub/src/acp.rs`, `transport.rs`, `bounded_transport.rs` |
| **Conductor** | Optionally wraps an agent behind an ordered proxy chain (`ConductorImpl` / `ProxiesAndAgent`). `code:crates/hub/src/conductor.rs` |
| **Projection owner** | Captures messages/static snapshots into a SQLite Hub-owned store (two-layer model). `code:crates/hub/src/store.rs`, `callbacks/capture.rs` |
| **On-demand singleton daemon** | One daemon per Hub home; CLI / MCP / `HubClient` all talk JSON-RPC to it. `code:crates/hub/src/daemon.rs`, `hub/client.rs` |

Pillars (`doc/ssot/pillars/README.md`): multi-endpoint registration, conversation/message CRUD + search, prompt/wait/view, config/mode params, proxies, capability-gated operations, no opinion about particular client apps.

### What it is not

| Not this | Why (code/docs) |
|----------|-----------------|
| **Not a full agent product** | No built-in LLM, tools UI, or OMP-style task/subagent orchestration. It drives *registered* agent endpoints. `doc:ssot/pillars/README.md`, `doc:dev/spec.md` §6 |
| **Not a vendor storage parser** | Core never opens Cursor/Grok private DBs. Vendor adapters are separate stdio endpoints. `doc:dev/spec.md` §6, `doc:dev/design.md` §1.1 |
| **Not a permission UX shell** | Permission is a fixed policy enum (`reject` / `auto-cancel` / `auto-allow`), not interactive prompts. `code:endpoint.rs` `PermissionPolicy`, `callbacks/permission_filesystem.rs` |
| **Not multi-tenant remote service** | Local interprocess daemon per home; privacy/redaction and file hardening assume local operator. |
| **Not ACP multi-major** | Initialize must negotiate ACP v1; other majors fail closed. `code:acp.rs` initialize check, `HubError::UnsupportedProtocolVersion` |

### Two-layer data (role of Hub projection)

| Layer | Source | Store `messages.source` | Semantics |
|-------|--------|-------------------------|-----------|
| Layer 1 Agent Original | `session/list` + `session/load` (and resume path when used) | `load_replay` | Agent’s own history projection |
| Layer 2 Hub Capture | Hub-initiated turns’ `session/update` | `local_turn` | Client-side capture of live traffic |

Layers are **parallel, not mutually exclusive**. Fallback to capture-only only when list/load are unavailable. `doc:dev/spec.md` §3, `doc:dev/design.md` §2.

**Doc note:** `MessageSource::AgentList` exists in `store.rs` as conversation-row / import provenance vocabulary; design text says message rows are only `local_turn` | `load_replay`. Treat `agent_list` as metadata provenance, not a third message display layer. `code:store.rs` `MessageSource`, `doc:dev/design.md` §3.2.

---

## 2. Process topology (CLI ↔ daemon ↔ agent)

```
┌──────────────────────┐     newline JSON-RPC 2.0      ┌─────────────────────────────┐
│ Entry points         │  (Unix socket / Win named pipe)│  acp-hub serve (daemon)       │
│  acp-hub CLI         │ ─────────────────────────────►│  CoreHub + Store + Registry │
│  acp-hub mcp         │  discover: daemon.json        │  ActivityTracker / idle exit│
│  HubClient (lib)     │  lock: daemon.lock            │                             │
└──────────────────────┘  handshake: hub/daemon/handshake│  per-agent connection task  │
                                                         │    cmd channel + ACP cx     │
                                                         └──────────────┬──────────────┘
                                                                        │ ACP stdio/HTTP/WS
                                                                        │ (+ optional proxies)
                                                         ┌──────────────▼──────────────┐
                                                         │ ACP Agent Endpoint          │
                                                         │ e.g. node adapter.mjs →     │
                                                         │   cursor-agent / grok / omp │
                                                         └─────────────────────────────┘
```

### Entry path (implemented)

1. CLI/MCP/`HubClient::connect_or_spawn(home)` → `daemon::ensure_daemon(home)`.
2. Try `daemon.json` endpoint connect; else take `daemon.lock`, spawn `acp-hub serve --home <home>` if needed, poll until metadata appears.
3. **Before any business RPC:** `hub/daemon/handshake` with `DAEMON_RPC_PROTOCOL_VERSION` (currently **2**). Mismatch → `DaemonUnavailable`, no business calls. `code:hub/client.rs` `verify_daemon_compatibility`, `daemon.rs` constants.
4. All work is `CoreHub::handle_rpc` method dispatch. `code:hub/dispatch.rs`.

### Daemon lifecycle

| Mechanism | Implementation |
|-----------|----------------|
| Singleton | Advisory `daemon.lock` (fd-lock write). Second `serve` fails. |
| Discovery | `daemon.json` `{ pid, endpoint, daemon_id, started_at }` |
| Idle exit | Quiescent when `active_clients==0 && active_rpcs==0 && active_runs==0` **and** idle duration > timeout (default **1800s**, env `ACP_HUB_IDLE_TIMEOUT`). `code:daemon.rs` `ActivityTracker::is_quiescent`, `idle_wait` |
| Spawn flags | Windows: `DETACHED_PROCESS \| CREATE_NO_WINDOW`. Unix: `setsid` in `pre_exec`. |
| Home | `$ACP_HUB_HOME` else `$HOME/.acp-hub` else `$USERPROFILE/.acp-hub`. Canonicalized via `dunce` on Windows. |

### Agent connection topology (inside daemon)

- **One long-lived connection task per agent id** (cached in `CoreHub.handles`).
- Initialization singleflight via `handle_inits` mutex map + 30s timeout.
- Commands flow: CoreHub → `mpsc::Sender<AgentCommand>` → command loop on ACP `ConnectionTo<Agent>`.
- **Cancel is special:** `session/cancel` is sent via cloned `AgentHandle.cx.send_notification(...)`, **not** through the command channel (loop is blocked on prompt). `code:acp.rs` comment, `hub/prompt.rs` `cancel`.

### Observed Windows tree (Cursor E2E)

When healthy (`doc:dev/cursor-adapter/e2e-investigation-2026-07-24.md`):

```text
acp-hub.exe serve --home <hub-home>
  └─ node adapters/cursor/adapter.mjs
       └─ <cursor-agent>/node.exe index.js acp
            └─ index.js worker-server
```

---

## 3. Registry & `permission_policy` / `client_capabilities` defaults and why

### Registry SSOT

| Artifact | Role |
|----------|------|
| `${home}/agents.json` | **SSOT** for endpoint configs (`acpAgents`, `acpProxies`). BTreeMap → deterministic key order. |
| SQLite `agent_cache` | Negotiated capabilities only; **JSON wins on conflict**. |
| Public DTO | Ordinary list/inspect redacts command/args/env/header values and omits `allowed_roots`. Writes still accept full config. `code:endpoint.rs` `public_endpoint_config` |

Transports:

- **Agent:** `stdio` | `http` | `websocket` (`AgentTransport`).
- **Proxy:** **stdio only** in this build (`ProxyTransport`); others → `UnsupportedProxyTransport`. Max chain length **16**, no duplicates.

### Defaults (explicit least privilege)

```rust
// PermissionPolicy — #[default] Reject
// ClientCapabilityConfig / FsConfig — #[default] all false, empty roots
// CLI AgentAddArgs: --permission-policy default reject; allow_* flags default off
```

| Field | Default | Why (intentional) |
|-------|---------|-------------------|
| `permission_policy` | **`reject`** | Deny agent tool/permission requests unless operator opts in. First reject option, else `Cancelled`. |
| `fs.read_text_file` / `write_text_file` | **false** | Do not advertise filesystem callbacks unless `--allow-read` / `--allow-write`. |
| `fs.allowed_roots` | **[]** | Empty means resolve against session cwd only when FS is enabled; roots are local auth boundaries and **redacted on public read**. |
| `terminal` | **false** | Terminal callbacks off unless `--allow-terminal`. |
| Adapter samples (cursor/grok/omp/codex) | Same reject + disabled FS/terminal | Matches impl_plan: “Default registration examples to rejected permission…” `doc:dev/impl_plan.md` §P1 docs |

**Operator implication (E2E):** real Cursor edits require re-registering with `--permission-policy auto-allow --allow-read --allow-write --allow-terminal --allow-root <work>`. Defaults will **reject** permission callbacks even if the agent wants to write. This is **I** (intentional) least-privilege, not a Cursor bug. E2E doc ranks it P1 for operator surprise. `doc:dev/cursor-adapter/e2e-investigation-2026-07-24.md` §2.2.

### Permission runtime behavior

`handle_permission` uses **session binding’s** policy (copied from agent config at bind time):

| Policy | Outcome |
|--------|---------|
| `auto-allow` | First AllowOnce/AllowAlways option, else Cancelled |
| `auto-cancel` | Cancelled |
| `reject` | First RejectOnce/RejectAlways, else Cancelled |

FS/terminal handlers also require **advertised client capabilities** on the connection config **and** binding flags. `code:callbacks/permission_filesystem.rs`.

### Registry mutation safety

`mutate_registry` (`hub/registry.rs`):

1. Global `registry_mutation` mutex.
2. Compute affected agents (including proxy-chain dependents).
3. Take init locks + generation writers for affected agents.
4. **`lock_agents_idle`**: refuse if any in-flight operation references affected agents → `Conflict(conv_id)`.
5. Recheck disk fingerprint vs expected (best-effort external edit detection).
6. Atomic save; publish memory + epoch; **evict handles** and revoke agent in `HubCtx`.

**Implication for hangs:** registry mutate waits on agent init locks and requires no active ops. Concurrent live turns or stuck agent init can make `agent add` appear to hang after writing is “expected” (E2E Trial E). Conflict path is explicit when ops are held; init lock wait is less visible.

---

## 4. Conversation lifecycle (create, live, ensure_live_session, prompt, close)

### 4.1 Create (`hub/conv/create` → `CoreHub::create_conversation`)

**Inputs:** `agent_id`, absolute **caller-supplied** `cwd` (required; never daemon cwd), optional `agent_session_id`, `additional_directories`, `mcp_servers`.

| Branch | Behavior |
|--------|----------|
| **Existing `agent_session_id` already in store** | Refresh via load; return existing `conv_id`. |
| **`agent_session_id` supplied, no row** | Create row, then load+publish with `remove_conversation_on_error`. Failure → unbind, delete row, `ResumeLoadFailed`. |
| **No session id (true new)** | Acquire connection-generation lease + **per-agent session/new quarantine** (`begin_session_creation_capture`). Send `CreateSession` / ACP `session/new`. On response: identity lease, create row, static snapshots, **publish matching** quarantined updates, bind, mark Live. |

**session/new invariants (code + design §2.2):**

- One concurrent `session/new` per agent (quarantine map keyed by agent).
- Quarantine shares pre-bind budgets with pending unbound updates (session/count/byte caps).
- After id known: only matching session notifications are published; bound-other sessions re-owned; unknown discarded on failure.
- Publication unit: row + snapshots + binding + runtime; rollback only claims owned by this op.

### 4.2 Live binding & runtime

| Concept | Implementation |
|---------|----------------|
| Session key | `(agent_id, agent_session_id)` — not session id alone |
| Binding | `HubCtx` sessions map: `conv_id`, permission_policy, fs config, cwd |
| Runtime | `RuntimeCache`: Connecting / **Live** / Cancelling / Closed + generation |
| Operation admission | Per-`conv_id` `OperationLease` (`Prompt`, `Refresh`, `Delete`, `SetParam`, `SetMode`, `Close`, …). Prompt also single-flights **per agent** (one Prompt at a time per agent_id). |

### 4.3 `ensure_live_session` (critical for param/send after create)

Called by `send_prompt`, `set_param`, `set_mode` before ACP work. `code:hub/conversation.rs` ~697–780.

```text
if runtime is Live AND session still bound → OK
else clear Live if binding missing
require stored absolute cwd (refuse daemon cwd inheritance)
if agent advertises resume:
  try session/resume refresh
  on success: bind + Live; return
  on failure: if !load_session → ResumeLoadFailed(session/resume)
              else fall through
session/load refresh
on success: bind + Live
on failure: ResumeLoadFailed(session/load)
```

**Operator-facing effect:** after process death / handle eviction / failed bind, the next prompt or param **rehydrates** via resume-then-load. Cursor E2E “resume/load operation failed” is this path folding failures (see §7).

**Binding order (ACP driver):**

- **load/resume:** bind **before** request so replay notifications are captured as Layer 1. `code:acp/command_loop.rs`
- **new:** bind **after** response + Hub row creation (quarantine until then).

### 4.4 Prompt (`hub/conv/send`)

Order of effects (`hub/prompt.rs`):

1. Reserve Prompt operation (+ agent-level prompt exclusivity).
2. Get/create agent handle; **validate prompt content capabilities** (image/audio/embedded) before live-session or store side effects.
3. `ensure_live_session`.
4. `create_run`, acquire `RunLease`, set current run on session, append user message (`local_turn`).
5. Enqueue `SendPrompt` (optional config params + mode applied in driver before prompt).
6. Worker awaits stop reason; **CAS finalize** run under operations lock; clear run; complete lease. CAS loss → `Conflict`, never silent success.

Capture during turn: notification handler with budgets (`MAX_CAPTURE_UPDATES_PER_TURN=4096`, 16 MiB). Capture failure can fail the turn via `merge_capture_failure`.

### 4.5 Cancel

- Snapshot prompt op token/run/session; re-check under operations lock.
- CAS run `running → cancelling`; runtime `Live → Cancelling`; then `CancelNotification` on connection.
- Send failure rolls back op flag / runtime / run to retryable state.
- Terminal finalization races with cancel under same mutex.

### 4.6 Close / delete

| Op | Remote | Local |
|----|--------|-------|
| `close` | ACP `session/close` if capability | unbind, runtime remove, status Idle; **projection retained** |
| `delete` | ACP `session/delete` unless `local_only` | unbind, runtime remove, delete projection |

Capability missing → `UnsupportedCapability`.

### 4.7 Discovery import (`hub/agent/sessions`)

`session/list` with page budgets (256 pages / 20k sessions / 8 KiB cursor / 64 MiB serialized). Absolute path validation before upsert/load. Per-session atomic import with partial-batch semantics (documented in design/spec § registry invariants).

---

## 5. Config / param / mode model

Hub does **not** invent a parallel config schema. It stores and drives **ACP session config options and modes** as static snapshots + live set RPCs.

| Surface | RPC | ACP |
|---------|-----|-----|
| List snapshot | `hub/conv/config` | Reads Store `config_snapshot` + `modes_snapshot` from last successful load/new/refresh/update |
| Set param | `hub/conv/set_param` | `SetSessionConfigOptionRequest` |
| Set mode | `hub/conv/set_mode` | `SetSessionModeRequest` |
| Inline on send | `hub/conv/send` `params` + `mode_id` | Applied in command loop before `session/prompt` |

CLI:

```text
acp-hub param list|set <conv> ...
acp-hub mode list|set <conv> ...
acp-hub send <conv> --text ... [--param id=value] [--mode id]
```

Snapshots:

- Successful load/new refresh **replaces complete static snapshot set**; absent plan/commands/usage/config/modes remove prior current values (design §3.10 / spec §9).
- Live updates: `ConfigOptionUpdate`, `CurrentModeUpdate`, plan/commands/usage notifications update Store.

**Compared to “rich agent defaults” products:** Hub is intentionally **thin and session-scoped**. There is no Hub-side global model registry, effort presets, or product modes beyond what the endpoint advertises. Param/mode ops always go through **live session** (`ensure_live_session`), so they inherit connection/resume reliability (E2E: `param set` hang / load fail).

---

## 6. Capture & notification fanout (including lag-fatal policy)

### 6.1 Capture path (agent → Store)

```
ACP session/update
  → HubCtx::handle_notification (connection-scoped)
  → creation quarantine OR pre-bind queue OR capture_bound_notification
  → Store append / static snapshot updates
  → broadcast hub/conv/update (best-effort send)
```

Key rules:

- Namespace by **`(agent_id, session_id)`**.
- Loading flag selects `load_replay` vs `local_turn`.
- Exhaustive match on `SessionUpdate` variants (chunks, tool calls, plan, commands, mode, config, session info, usage, fallback).
- Pre-bind / creation quotas: e.g. 64 sessions, 1024 notifications, 4 MiB aggregate, 256 KiB single, 256 per session. `code:callbacks/capture.rs`

### 6.2 Fanout path (Store event → daemon clients)

1. `HubCtx` holds `broadcast::Sender` capacity **1024** (`callbacks/connection.rs`).
2. Each daemon client connection subscribes and `select!`s notifications into the client pipe/socket (`daemon/rpc_io.rs`).
3. Client `RpcClient` has its own broadcast channel capacity **256** for local subscribers (`rpc.rs`).
4. `HubClient::subscribe_notifications` exposes that stream (CLI send currently pages Store after completion rather than requiring live subscribe).

### 6.3 Lag-fatal policy (intentional)

```262:268:crates/hub/src/daemon/rpc_io.rs
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "daemon client lagged behind streamed notifications");
                        connection_error = Some(HubError::DaemonUnavailable(format!(
                            "daemon notification stream lagged by {skipped} messages; reconnect and resynchronize"
                        )));
                        abort_requests = true;
                        break;
```

**Policy:** lag is a **projection gap**. The server **closes that client connection** rather than skipping updates. Spec/design forbid “log and continue.” Client sees EOF → `daemon closed the connection` (`rpc.rs` `reader_loop` `Ok(None)`).

Tests: `lagged_notification_stream_closes_the_client_instead_of_hiding_a_gap` in `daemon/rpc_io/lifecycle_tests.rs`.

**E2E stressor:** Cursor streams many micro thought/tool updates; high fanout + slow CLI consumer can trip lag-fatal mid-send even after tools already wrote files. Ranked P0/P1 in E2E investigation. This is **intentional correctness** colliding with **high-churn agents** (attribute as **I** + operational **C** for lack of backpressure UX).

### 6.4 Capture failure vs lag

| Failure class | Effect |
|---------------|--------|
| Capture budget / store error during op | Recorded; can fail ACP op via `merge_capture_failure` |
| Notification lag on daemon→client | **Client connection killed**; in-flight RPC may abort as `DaemonUnavailable` |
| Oversized encoded notification | Dropped with warn (not lag); connection continues |

---

## 7. Error model & RPC typing/folding

### 7.1 HubError (library)

Defined in `error.rs`: `UnsupportedCapability`, `ResourceLimit`, `UnsupportedProxyTransport`, `UnsupportedProtocolVersion`, `AuthRequired`, `ResumeLoadFailed`, `Conflict`, `NotFound`, `InvalidRegistry`, `InvalidCursor`, `StaleCursor`, `DaemonUnavailable`, `Acp`, `Io`, `Sqlite`, `Json`, `Other`.

`ResumeLoadFailed` wraps:

- `attempted_method`: `"session/load"` | `"session/resume"`
- endpoint, conv_id, agent_session_id
- nested `source: Box<HubError>`
- Display: `could not {method} conversation on endpoint {endpoint}`

### 7.2 Wire codes

Custom JSON-RPC codes include `-32001` auth, `-32004` not found, `-32009` conflict, `-32010` capability, `-32011` registry, `-32012` protocol, `-32013` resume/load, `-32014` proxy transport, `-32015` resource, `-32016`/`-32017` cursors. `code:rpc.rs`.

### 7.3 Typed error data + privacy folding

`rpc/error_data.rs` serializes a **closed** `TypedHubErrorData` set. Nested resume sources use `SafeResumeSourceData`:

| Nested source | On rehydrate (`into_hub_error`) |
|---------------|----------------------------------|
| NotFound / Conflict / UnsupportedCapability / AuthRequired / known transport/protocol/registry | Typed HubError |
| `DaemonUnavailable {}` or **`Internal {}`** (Io, Sqlite, Acp, Other, ResourceLimit, cursors, nested ResumeLoadFailed, …) | Collapsed to **`DaemonUnavailable("resume/load operation failed")`** |

So a CLI error string like:

```text
could not session/load conversation on endpoint cursor
  caused by: daemon unavailable: resume/load operation failed
```

often **loses the root ACP/agent error detail** across the RPC boundary by design (safe typed surface). This matches E2E taxonomy and is a deliberate **privacy/typing fold**, not random string munging—but it is a major **operator diagnosis** gap (**C** / intentional privacy).

Client path without typed data: many codes fold to generic `DaemonUnavailable("daemon request failed")` or similar. `rpc_error_to_hub_error`.

### 7.4 Disconnect messages

| Symptom | Source |
|---------|--------|
| `daemon unavailable: daemon closed the connection` | Client reader EOF |
| `daemon notification stream lagged by N messages...` | Server lag-fatal (if delivered before close) |
| `conversation X is busy with an in-flight turn` | `HubError::Conflict` |

---

## 8. Windows-specific paths (named pipes, process trees)

### 8.1 Daemon transport

| OS | Endpoint |
|----|----------|
| **Windows** | Named pipe `\\.\pipe\acp-hub-{daemon_id}` (home not in path). SDDL: owner + SYSTEM + BA full access. |
| **Unix** | Prefer `$home/daemon.sock`; if path too long (macOS sun_path), fallback `$TMP/ah-{id12}/daemon.sock` mode 0600. |

Metadata always in `$home/daemon.json`. Stale cleanup on Windows does not delete pipes by path the same way as Unix sockets (pipes are kernel objects keyed by name).

### 8.2 Spawn / isolation

- Detached daemon process, no window.
- Home canonicalized with `dunce` (strip `\\?\` when safe) for child argv and comparisons.
- Sensitive files: Windows owner ACL hardening (`set_windows_owner_acl`).

### 8.3 Terminal process trees

ACP terminal callbacks:

- **Windows:** Job Object with `KILL_ON_JOB_CLOSE`; terminate via `TerminateJobObject`.
- **Unix:** process group kill (`kill(-pgid, SIGKILL)`).

Unbind/delete/revoke: **ownership-first** — remove handles from active table under mutex, then best-effort process-tree cleanup **outside** lock; cleanup failure must not restore quota. `code:callbacks/terminal.rs`, `connection.rs` `retire_terminals_matching`.

### 8.4 Agent process trees (adapters)

Hub’s stdio transport owns the **direct** child (e.g. `node adapter.mjs`). Grandchildren (`cursor-agent`, workers) are **vendor process trees**. Forced `taskkill` of Hub mid-turn can leave:

- orphaned agents holding resources
- rotten locks / DB WAL
- subsequent **Access denied (os error 5)** on pipes/DB

E2E ranks this P0 for Windows reliability. Hub does not currently job-object-wrap entire agent adapter trees the way it wraps **terminal** children.

### 8.5 Path / cwd rules

- Conversation cwd must be absolute and caller-provided.
- Session list cwd / additional dirs validated absolute before persist/load.
- FS callbacks resolve paths under `allowed_roots` or session cwd with no-follow open/write semantics (Windows reparse-point care).

---

## 9. Adapter layer vs core

### Core boundary (hard)

Core Hub:

- Speaks **only ACP** to endpoints.
- Owns registry, projection DB, daemon RPC, capabilities, capture, permission/fs/terminal **as client callbacks**.
- Does **not** import vendor storage formats or call vendor private CLIs.

### Adapter layer (explicit registration)

| Adapter | Path | Role |
|---------|------|------|
| Cursor | `adapters/cursor/` | stdio ACP adapter (Node) bridging Cursor agent; optional read-only discovery per adapter spec |
| Grok | `adapters/grok/` | stdio ACP adapter; resume/delete via supported Grok CLI surfaces |
| OMP | `adapters/omp/agents.json` | Sample: `omp acp` stdio (no custom adapter code in-tree) |
| Codex | `adapters/codex/agents.json` | Sample registration for codex-acp |

Sample `agents.json` shape (all four samples):

```json
{
  "acpAgents": {
    "<id>": {
      "transport": { "type": "stdio", "command": "...", "args": [...], "env": {} },
      "permission_policy": "reject",
      "client_capabilities": {
        "fs": { "read_text_file": false, "write_text_file": false, "allowed_roots": [] },
        "terminal": false
      }
    }
  }
}
```

Vendor adapter **mutation** rules (design §1.1): private-format parsers are read-only; resume/delete must use official vendor ACP/CLI; capability gate when unsupported.

### Conductor / proxies

- Empty `proxy_chain` → direct agent component.
- Non-empty → ordered stdio proxies as `ConnectTo<Conductor>` chain.
- Proxies are operator-chosen code, not a sandbox; flow ACK budgets enforce retained bytes per physical leg.

---

## 10. Intentional invariants that must NOT be casually redesigned

Each item is a reviewed product/security invariant. Changing it requires pillar/spec/design updates and regression coverage—not a drive-by “make Cursor happier” patch.

| # | Invariant | Citations |
|---|-----------|-----------|
| 1 | **Hub is multi-endpoint ACP client/conductor**, not a single-vendor agent app. | `doc:ssot/pillars/README.md`; `doc:dev/spec.md` §1, §6 |
| 2 | **Two-layer history is parallel** (`load_replay` vs `local_turn`); failed Layer 1 refresh must not wipe Layer 2; never silently create empty session on load failure. | `doc:dev/spec.md` §3; `doc:dev/design.md` §2; `HubError::ResumeLoadFailed` |
| 3 | **Session identity is `(agent_id, agent_session_id)`**; callback/capture state namespaced by endpoint+session. | `doc:dev/design.md` §2.2, §3.3; `impl_plan` P0 isolation |
| 4 | **Bind before load/resume; bind after create** with session/new quarantine publication rules. | `doc:dev/design.md` §2.2; `acp/command_loop.rs`; `conversation.rs` create path |
| 5 | **ACP v1 only**; reject other protocol majors. | `doc:dev/spec.md` §6; `acp.rs` initialize |
| 6 | **Capability gating** for load/resume/close/delete/prompt media/list; no “try and hope.” | `doc:dev/design.md` §5; `error.rs` `UnsupportedCapability` |
| 7 | **Default least privilege**: `permission_policy=reject`, FS/terminal off in samples/CLI defaults. | `endpoint.rs` defaults; all `adapters/*/agents.json`; `impl_plan` docs package |
| 8 | **cwd never inherited from daemon process**; must be absolute caller cwd. | `conversation.rs` `require_absolute_cwd`; design/impl_plan P1 |
| 9 | **Daemon notification lag is connection-fatal** (reconnect/resynchronize); no silent skip. | `doc:dev/spec.md` §10; `doc:dev/design.md` §4; `daemon/rpc_io.rs` |
| 10 | **Cancel ownership CAS** before notify; serialize with prompt finalize; full rollback on send failure. | `doc:dev/spec.md` §9; `hub/prompt.rs` |
| 11 | **Run finalization is ownership/CAS**; losing CAS is Conflict, never prompt success. | `doc:dev/design.md` §3.5, §3.10; `prompt.rs` worker |
| 12 | **Registry mutation atomicity / fingerprint / epoch**; no post-replace publish of stale handles; external edit while daemon runs is unsupported. | `doc:dev/spec.md` §9; `registry.rs` `mutate_registry` |
| 13 | **Public registry DTO redaction** (commands, secrets, roots); ordinary reads must not leak. | `doc:dev/spec.md` §10; `endpoint.rs` public projection |
| 14 | **Core does not parse vendor private storage**; adapters are explicit registered endpoints. | `doc:dev/spec.md` §6; `doc:dev/design.md` §1.1 |
| 15 | **Daemon RPC handshake exact version** before business RPC. | `DAEMON_RPC_PROTOCOL_VERSION=2`; `hub/client.rs` |
| 16 | **Byte budgets**: daemon 128 MiB partitioned RPC admission; 32 MiB frame; session list / capture / message page caps. | `daemon.rs` constants; `doc:dev/spec.md` §10 |
| 17 | **Terminal teardown ownership-first**; process-tree cleanup outside lock; fail cannot restore quota. | `doc:dev/design.md` §4; `callbacks/connection.rs` |
| 18 | **Proxy transport stdio-only** this SDK revision; identity-bound flow ACK; conservative ambiguous release. | `endpoint.rs` ProxyTransport; `bounded_transport/flow.rs` |
| 19 | **Generation-aware message cursors**; no bare offsets across projection replacement. | `doc:dev/spec.md` §9; `store.rs` cursor model |
| 20 | **Typed RPC error surface is closed/safe**; nested resume sources fold internals—do not “fix” by leaking arbitrary messages without a privacy design. | `rpc/error_data.rs`; design error section |

---

## Documented vs implemented divergences / caveats

| Topic | Docs say | Code does | Notes |
|-------|----------|-----------|-------|
| Idle exit conditions | design often: clients=0 **and** runs=0 | Also requires **active_rpcs==0** | Stricter than some short design blurbs |
| `RuntimeCache::with_singleflight` | design: singleflight helper | Method is effectively a **no-op wrapper**; real singleflight is `handle_inits` in `agent_handle` | API leftover; don’t rely on RuntimeCache for connect serialisation |
| Message `agent_list` source | design: messages only local_turn/load_replay | Enum includes `AgentList` | Metadata/import vocabulary |
| HTTP/WS agents | supported in design | Implemented in `AgentTransport` + bounded transports | Proxies still stdio-only |
| Notification channel sizes | design: lag-fatal principle | HubCtx 1024, RpcClient 256, tests use tiny buffers | Capacities are implementation knobs; policy is lag-fatal |
| Operator error detail on resume/load | typed ResumeLoadFailed with source | Nested **Internal** rehydrates as generic `resume/load operation failed` | By design privacy fold; hurts Cursor debugging (E2E) |
| Reliability under Cursor streaming | design assumes reconnect/resync | E2E: disconnect/hang after real work | Policy correct; product still operationally fragile on Windows |

---

## Code map (quick index)

| Concern | Primary modules |
|---------|-----------------|
| Daemon / idle / pipes | `daemon.rs`, `daemon/rpc_io.rs` |
| Client RPC | `rpc.rs`, `rpc/error_data.rs` |
| Engine facade | `hub.rs` → `state`, `registry`, `conversation`, `prompt`, `lifecycle`, `dispatch`, `client`, `types` |
| ACP driver | `acp.rs`, `acp/command_loop.rs`, `acp/capabilities.rs`, `acp/session_list.rs` |
| Capture / callbacks | `callbacks.rs`, `capture.rs`, `connection.rs`, `permission_filesystem.rs`, `terminal.rs` |
| Conductor | `conductor.rs`, `transport.rs`, `bounded_transport/*` |
| Registry file | `endpoint.rs` |
| Projection | `store.rs`, `store/*` |
| Runtime | `runtime.rs` |
| Errors | `error.rs` |
| CLI | `crates/cli/src/args.rs`, `commands.rs`, `mcp.rs` |
| Samples | `adapters/{cursor,grok,omp,codex}/agents.json` |

---

## Relevance to disputed operator surfaces (preview for later lanes)

| Disputed surface | Architectural placement |
|------------------|-------------------------|
| Permission defaults reject | §3 intentional least privilege |
| Config/param thin session model | §5 |
| Daemon singleton / idle / Windows pipes | §2, §8 |
| Resume/load after death | §4.3 `ensure_live_session` |
| Lag = connection-fatal | §6.3 |
| `resume/load operation failed` fold | §7.3 |
| Registry mutate under live agents | §3 `lock_agents_idle` + init locks |
| Process tree kill recovery | §8.3–8.4 |
| Cursor streaming vs fanout | §6 + E2E doc |

Attribution (I/R/O/C/V) is out of scope for this file; see planned `03-history-attribution.md`.
