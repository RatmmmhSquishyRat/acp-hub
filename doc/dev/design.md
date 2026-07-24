# ACP Hub — Technical Design

> Grounded in: `doc/ssot/pillars/README.md` (design 1-5), `doc/ssot/pillars/TechSel.md`
> Research basis: `doc/research/from-chatgpt/acp_discussion_docs/`

## 1. System Architecture

```
┌─────────────────────────────────────────────────┐
│              ACP Hub Daemon (singleton)           │
│                                                   │
│  ┌─────────┐  ┌──────────┐  ┌─────────────────┐ │
│  │ Registry │  │  Store   │  │  RuntimeCache   │ │
│  │ (JSON)   │  │ (SQLite) │  │  + RunLease     │ │
│  └────┬─────┘  └────┬─────┘  └────────┬────────┘ │
│       │              │                  │          │
│  ┌────┴──────────────┴──────────────────┴───────┐ │
│  │              CoreHub                          │ │
│  │  (session mgmt, prompt dispatch, capture)     │ │
│  └───────────────────┬───────────────────────────┘ │
│                      │                             │
│  ┌───────────────────┴───────────────────────────┐ │
│  │           ACP Driver (per agent)               │ │
│  │  Client.builder() → connect_with(component)    │ │
│  │  Notification handler → Store (capture)        │ │
│  │  Callback handlers → permission/fs/terminal    │ │
│  └───────────────────┬───────────────────────────┘ │
│                      │                             │
│         ┌────────────┴────────────┐               │
│         │  Conductor + Proxies    │               │
│         │  (if proxy chain set)   │               │
│         └────────────┬────────────┘               │
│                      │                             │
└──────────────────────┼─────────────────────────────┘
                       │ ACP stdio/HTTP/WS
          ┌────────────┴────────────┐
          │   ACP Agent Endpoint    │
          │ (omp acp / codex-acp /  │
          │  cursor agent acp / ...)│
          └─────────────────────────┘

Entry points (all connect to daemon via JSON-RPC):
  CLI (acp-hub ...)     MCP facade (acp-hub mcp)    Embedded library (HubClient)
```

### 1.1 Vendor adapter and mutation boundaries

Vendor compatibility is provided by explicitly registered adapter endpoints,
not by CoreHub importing vendor storage formats:

```text
Discovery and replay (read-only):
  CoreHub --ACP--> registered vendor adapter
                     └--> private-format parser --read-only--> vendor session store

Resume and delete (mutation, when supported):
  CoreHub --ACP--> registered vendor adapter
                     └--> official vendor ACP/CLI command --> vendor-managed state
```

The private-format parser may enumerate and replay vendor sessions, but it must
not synthesize, update, or delete vendor records. A resume or delete request is
a separate mutation edge: the adapter invokes the vendor's supported ACP or CLI
surface and reports its actual result. For example, Cursor CLI continuation
uses the supported resume route while IDE continuation remains rejected; Grok
continuation uses `grok -r` and deletion uses `grok sessions delete`. Capability
gating still applies when an official mutation route is unavailable.

## 2. Two-Layer Data Model Design

### 2.1 Data Flow

```
Agent-side session discovery:
  session/list → SessionInfo[] → upsert conversation rows (Layer 1 metadata)
     ↓ (if load_session capability)
  session/load → session/update notifications → Store (source='load_replay', Layer 1 messages)

Hub-initiated turn:
  session/prompt → session/update notifications → Store (source='local_turn', Layer 2 messages)

Display:
  conv show → messages WHERE conv_id=? ORDER BY seq
     → each row has source column: 'load_replay' | 'local_turn'
     → UI labels: [agent-original] or [hub-capture]
```

### 2.2 Session Binding Timing (Critical)

When loading an existing agent session via `session/load`:
1. Create/upsert the Hub conversation row before the agent can replay messages.
2. **Bind the session BEFORE** sending `LoadSessionRequest`
   (agent_session_id is known upfront from session/list)
3. Key the binding by `(agent_id, agent_session_id)`; an agent-local id is not
   globally unique.
4. Agent replays messages via `session/update` notifications DURING the request.
5. Stage replay rows in a transaction.
6. On success, replace only the prior current Layer 1 snapshot; Layer 2
   `local_turn` rows remain current.
7. On failure, discard the staged replay and keep the previous projection.

When creating a new session via `session/new`:
1. Acquire the current endpoint connection-generation lease and a bounded,
   per-agent creation quarantine before sending `NewSessionRequest`. A second
   `session/new` for that agent is rejected while this lease is active.
2. Quarantine `session/update` notifications during the request because the new
   `agent_session_id` is not known before the response. The quarantine and the
   ordinary pre-bind queues share the same session, update-count, and byte
   limits.
3. After the response, acquire `(agent_id, agent_session_id)` ownership and
   create the conversation row and static snapshots. An existing row or binding
   is a conflict and remains unchanged.
4. Publish only notifications matching the returned session id, then bind and
   drain them in protocol order. Notifications for another existing bound
   session are replayed to that owner; non-matching notifications without a
   known binding are discarded.
5. A capture, snapshot, bind, or runtime-publication failure discards matching
   quarantined updates and removes only the row, binding, and runtime state
   claimed by this operation. If no session id is returned, updates for known
   bound sessions are replayed and unknown unbound updates are discarded.
   Connection-generation and session-identity leases remain held through this
   publication or rollback.
6. Subsequent prompts capture via the notification handler with
   `source='local_turn'`.

### 2.3 Fallback Rule

```
conv list / conv show data source priority:
  IF agent supports session/list:
    → show agent-discovered sessions (Layer 1) + Hub-created sessions (Layer 2)
  ELIF agent supports session/load (but not list):
    → show Hub-created sessions with loaded messages
  ELSE:
    → fallback to Hub capture only (Layer 2)
```

## 3. Module Design

### 3.1 Registry (`endpoint.rs`)
- `agents.json` = SSOT for endpoint configs (acpAgents / acpProxies object maps)
- SQLite `agent_cache` = negotiated capabilities only; JSON wins on conflict
- BTreeMap for deterministic output

### 3.2 Store (`store.rs`)
- SQLite projection with FTS5 full-text search
- `messages.source` column: `local_turn` (Layer 2) | `load_replay` (Layer 1). (`agent_list` is conversation-row metadata provenance only, not a message layer.)
- `messages.current_projection`: 1 = current view, 0 = superseded audit
- Non-destructive `session/load` replay: supersede prior Layer 1 rows and retain
  them as audit
- Layer refresh supersedes prior `load_replay` rows only; `local_turn` remains
  independently current
- `conversations_fts`: title search
- `messages_fts`: body search (search_body extractor skips base64)

### 3.3 ACP Driver (`acp.rs`)
- Per-agent connection task: `Client.builder().connect_with(component, |cx| { loop })`
- Notification handler: exhaustive capture of ALL `session/update` variants;
  every lookup is endpoint/session scoped
- Callback handlers: permission/fs(read+write)/terminal(create/output/wait/kill/release);
  every callback enforces the negotiated client capability and owning session
- Cancel: under the conversation operation lock, CAS the exact persisted run
  from running to cancelling, transition runtime Live to Cancelling, then call
  `cx.send_notification(CancelNotification)` directly (bypasses blocked loop).
  A send failure rolls every state back before the caller may retry.
- Capability gating: every ACP call checked against advertised capabilities
- **Binding order**: bind BEFORE load/resume, AFTER create

### 3.4 Conductor (`conductor.rs`)
- Empty proxy chain → direct agent
- Non-empty → `ConductorImpl::new_agent(name, ProxiesAndAgent::new(agent).proxy(p1)...)`

### 3.5 CoreHub (`hub.rs`, `hub/`)

`hub.rs` 是稳定 facade，只声明私有子模块并 re-export `CoreHub`、
`HubClient` 和公共 DTO。具体职责位于：

| 模块 | 职责 |
|---|---|
| `hub/types.rs` | JSON-RPC 请求/响应 DTO |
| `hub/state.rs` | `CoreHub` 状态、operation admission、replay lock accounting |
| `hub/registry.rs` | agent/proxy 注册、连接初始化、session discovery |
| `hub/conversation.rs` | 对话创建/读取/搜索、session replay 和 live publication |
| `hub/prompt.rs` | prompt、cancel、param、mode |
| `hub/lifecycle.rs` | close/delete |
| `hub/dispatch.rs` | CoreHub JSON-RPC dispatch |
| `hub/client.rs` | daemon-backed `HubClient` |
| `hub/tests/` | 共享 fixture 与 registry/client/operation/replay 测试 |

关键并发边界：

- conversation operation 先取得 per-conversation admission，再读取 endpoint
  config 或取得 handle；registry replacement 因而只能发生在 operation 之前或
  完成之后。
- enqueue 成功后，owned worker 持有 admission，直到 replay、存储、
  session binding 和 `RuntimeCache::Live` publication 完成。
- cancel 在异步 handle lookup 前保存 prompt token，并在发送同步
  `session/cancel` 前重新核对 token/run/session。prompt completion 与 cancel
  在同一 operation mutex 下竞争 persisted run CAS：terminal winner 不发送
  stale cancel，cancel winner 先发布 persisted/runtime cancelling。通知发送
  失败恢复 operation flag、runtime、run 与 conversation，不能取消替换后的
  新 run，也不能留下不可重试的半状态。
- replay lock map 在同一互斥区内按 guard user 计数；`lock_owned()` 的临时
  `Arc` 不参与清理判定。
- registry state carries a monotonic epoch. Endpoint initialization records
  the epoch/config it read and revalidates both immediately before cache
  publication. Mutation waits for or invalidates in-flight initialization and
  clears capability cache for affected agents.
- Registry persistence uses a commit outcome that distinguishes pre-replace
  failure from post-replace state. External fingerprints are rechecked after
  internal admission locks are held and before replace; post-replace recovery
  reloads the actual disk image before publishing in-memory state.
- Runtime direct edits to `agents.json` by an uncoordinated process are outside
  the supported writer protocol. The final fingerprint check is best-effort
  drift detection, not a cross-platform filesystem CAS. Supported concurrent
  writers use Hub RPC serialization; manual edits occur while the daemon is
  stopped.
- `session/list` import captures a metadata/FTS before-image before provisional
  upsert. Replay uses durable generation staging: per-update append, atomic
  generation commit, compensating rollback, and crash recovery. It does not
  hold a SQLite transaction across an asynchronous ACP request. Atomicity is
  per session: earlier successful sessions in a batch remain committed, the
  failing session rolls back, and later sessions are not processed. Duplicate
  identities count against budgets; first occurrence wins and replays once.
- Public run lifecycle is admission-owned. A prompt completion publishes
  success only when its finalization CAS commits the owned active run.

`hub/` 领域模块继续维持在 1,000 行以下，并以约 900 行作为主动拆分边界。
该边界不改变 `crate::hub::*` 公共 API。

### 3.6 Daemon (`daemon.rs`)
- On-demand singleton: file lock + socket/pipe + metadata JSON
- Idle exit: `active_clients=0 AND active_runs=0 AND elapsed > IDLE_TIMEOUT`
- `ensure_daemon(home)`: discover or spawn
- `HubClient::connect_or_spawn` completes a side-effect-free daemon handshake
  with an exact, independently versioned RPC contract before exposing the
  client. An old, malformed, or mismatched resident daemon is rejected before
  any business request.
- Global RPC admission is byte-weighted and fixed at 128 MiB: 87 MiB for
  retained request frames, 40 MiB for ordinary encoded responses, and 1 MiB
  reserved for bounded terminal/fallback errors. Request bytes are admitted
  progressively as they are read instead of reserving a maximum-size frame on
  the first byte. The request reservation covers parsing and dispatch; response
  reservations remain held through flush, including a slow writer. The
  independent fallback partition prevents response saturation from suppressing
  the typed terminal error.

### 3.7 Entry Points
- CLI: clap-derived command surface, all daemon operations via HubClient RPC
- MCP: rmcp ServerHandler tools backed by the same HubClient path
- Embedded: HubClient::connect_or_spawn (same RPC path as CLI/MCP)

### 3.8 Repository-wide Rust module decomposition

The single-file boundary applies to all production and test Rust modules. Stable
facades retain current public paths while domain modules own implementation:

| Stable facade | Domain modules |
|---|---|
| `callbacks.rs` | connection/session state, capture, permission/filesystem, terminal/process-tree, tests |
| `bounded_transport.rs` | shared flow budget, stdio, HTTP/SSE, WebSocket, tests |
| `daemon.rs` | activity, server/client I/O, response codec, endpoint/state files, tests |
| `rpc.rs` | wire types/safe errors, client actors, bounded I/O, tests |
| `store.rs` | public rows/types, schema/migrations, conversations/runs, messages/replay, search |
| `acp.rs` | command DTOs, connection lifecycle, session operations, capability helpers, tests |
| CLI `main.rs` | Clap arguments, command execution, output/paging/redaction |
| CLI `mcp.rs` | server/tool handlers, request schemas, conversion/error helpers, tests |

The facade owns only declarations, shared public types that cannot move without
an API change, and intentional re-exports. Child modules may implement methods
on facade-owned types, but dependencies must remain one-directional: shared
types/state first, domain operations second, public dispatch last.

Mechanical movement is accepted only when:

1. every pre-split test remains present exactly once;
2. exact public paths, command surfaces, MCP schemas and serialized forms remain
   unchanged;
3. every production and test Rust file is below 1,000 lines after formatting;
4. focused protocol, callback, transport, daemon, store, CLI and MCP regressions
   pass before the final workspace matrix.

Public compatibility is checked with an external consumer fixture, DTO/MCP
schema goldens, raw ACP v1 JSON-frame goldens, and an old-database
reopen/schema dump fixture. Test inventory comparison includes ignored state,
while independent review checks that moved assertions and fault injections
retain their original semantics.

### 3.9 Official SDK integration boundary

The workspace keeps ACP protocol, conductor and test utilities on one official
rust-sdk release line. HTTP/WebSocket remain project bounded transports over
that line's core types; the unused upstream HTTP crate is not declared. The
manifest may use a git patch only
when the test harness is unpublished and the revision is the exact source of
the published release line; production package metadata must still resolve
published ACP crates for `cargo publish --dry-run`.

`rmcp` is upgraded independently to its current stable major. The MCP facade
continues to expose the same intentional tool set and closed request schemas.
Migration adapters belong at the crate integration edge; CoreHub, daemon RPC
and store semantics do not fork into old-SDK and new-SDK implementations.

SDK upgrades are accepted only after:

- ACP initialize, session list/load/prompt/cancel/callback and proxy tests use
  the upgraded official types;
- bounded stdio/HTTP/SSE/WebSocket transports continue to apply project budgets
  before deserialization or unbounded queueing; incomplete stdio lines reserve
  aggregate partial bytes progressively and atomically transfer those exact
  wire bytes to the completed physical-frame reservation;
- MCP process smoke initializes, lists tools and executes representative
  read/write/error paths using the upgraded `rmcp`;
- package verification proves published-crate resolution without relying on a
  local patch.

The ACP crate major changes the identity of ACP types already exposed by the
library. For the current pre-1.0 project this is an intentional public API
migration to at least `0.2.0`, documented as such and verified by an external
consumer fixture.
Mechanical module movement and SDK API adaptation are separate, independently
green and independently revertible changes.

The mechanical phase preserves each logical case through an explicit
old-target/test to new-target/test manifest plus ignored/body review. The SDK
phase permits only the approved ACP Rust type/source update in the external
consumer; endpoint/Hub DTO, MCP schema and database schema remain exact-equal.
ACP v1 goldens compare canonical semantic JSON with required/forbidden fields,
not object key order or harmless omission of optional defaults.

### 3.10 Stable paging, bounded discovery, and capability admission

- Message traversal uses a cursor containing projection generation and the last
  stable ordering key. The Store persists a per-conversation monotonic
  generation and increments it in the same transaction that switches replay
  membership. The opaque, versioned cursor is not a security authentication
  token; its decoded fields and checksum validate conversation id,
  generation, last key, include-audit and run/filter identity. Tail append
  preserves traversal; projection replacement, restart-time mismatch or query
  identity/checksum corruption returns an explicit invalid/stale-cursor error.
- Session discovery processes at most 256 pages, 20,000 received sessions,
  8 KiB per cursor and 64 MiB canonical serialized input. It charges received
  items before dedupe and uses first-occurrence metadata for each
  `(agent_id, session_id)`.
- `SessionInfo` cwd and additional roots are validated as absolute before
  provisional storage or `session/load`.
- Prompt capability admission maps image, audio and embedded resource blocks to
  ACP prompt capabilities after initialize but before live-session/config/mode/
  prompt dispatch or run/message persistence.
- Real proxy verification constructs conductor/registry stdio legs through the
  bounded transport. A test-only per-leg reservation/ACK ledger and controlled
  saturation gate expose each physical token, canonical semantic identity and
  retained-byte reservation/release; final success alone is not evidence.
- Config options and modes carry independent presence. A successful load/new
  refresh atomically replaces the complete static snapshot set; absent
  plan/commands/usage/config/modes delete prior current values. Endpoint
  standard `updated_at` and bounded opaque vendor `_meta` are stored in the
  Agent Original static projection and privacy-filtered on ordinary reads.
- Public `create_run` returns an operation-owned token. Registry mutation sees
  that real operation, and `finalize_run` must present the token. Prompt and
  external finalizers share the same CAS; losing ownership is an explicit
  conflict, never prompt success.

## 4. Transport Design

| Transport | Component | Construction |
|-----------|-----------|-------------|
| stdio | bounded line transport around `AcpAgent::spawn_process` | command + args + env |
| HTTP | bounded body/SSE transport | url + headers |
| WebSocket | bounded Tungstenite transport | ws/wss url + headers |
| Proxy (stdio) | bounded stdio transport as `ConnectTo<Conductor>` | command + args + env |

All endpoint transports enforce a 32 MiB serialized JSON-RPC ceiling before
JSON deserialization. They additionally cap unconsumed input at 4096 frames /
32 MiB, allow at most eight outstanding inbound callback requests, and keep
HTTP SSE framing to 64 streams with one shared 32 MiB partial-event budget.
Direct transports acknowledge exact message identities. A configured proxy
chain acknowledges each physical leg by a canonical semantic identity and a
leg-local monotonic reservation token. Notifications bind method plus canonical
params; responses bind their canonical success/error payload because conductor
legs may remap request ids. A missing identity match is a protocol error.
Ambiguous identical payloads release the smallest matching byte reservation,
so proxy reserialization or reordering can only conservatively overcount and
never undercount retained bytes. This assumes the supported proxy contract is
one logical input to one logical output; a proxy is operator-chosen executable
code, not a sandbox boundary. Callback updates, filesystem payloads, terminal
output, daemon RPC, and message pages have tighter operation-specific limits.

Daemon notification broadcast lag is logged and **does not** abort the client
or in-flight RPCs by default (Product-UX 2026-07-24). Incomplete
`hub/conv/update` delivery is preferable to failing a successful agent turn.
Buffer capacity is bounded; operators may still resync via `conv show` / reload.
(Historical note: R-DAEMON-004 briefly made lag connection-fatal; that default
was reversed for UX priority while keeping best-effort projection.)

Terminal teardown is ownership-first: unbind/revoke removes matching handles
from the active table under its mutex, then performs best-effort process-tree
cleanup outside the mutex. Cleanup failure is logged but cannot restore an
unreachable quota entry or retain its daemon activity lease. Explicit
kill/release retains retry behavior while the terminal still has a valid owner.

## 5. Capability Matrix

| Operation | Required Capability | Error if unsupported |
|-----------|--------------------|-----------------------|
| session/load | `load_session: true` | `UnsupportedCapability` |
| session/resume | `session_capabilities.resume.is_some()` | `UnsupportedCapability` |
| session/close | `session_capabilities.close.is_some()` | `UnsupportedCapability` |
| session/delete | `session_capabilities.delete.is_some()` | `UnsupportedCapability` |
| image prompt | `prompt_capabilities.image` | `UnsupportedCapability` |
| audio prompt | `prompt_capabilities.audio` | `UnsupportedCapability` |
| embedded resource prompt | `prompt_capabilities.embedded_context` | `UnsupportedCapability` |
| fs callbacks | advertised client caps | typed error |
| terminal callbacks | advertised client caps | typed error |

## 6. Error Handling

- `HubError`: UnsupportedCapability, UnsupportedProxyTransport, AuthRequired, ResumeLoadFailed, Conflict(conversation_busy), DaemonUnavailable, Acp(#[from])
- Init/connection errors propagated via `Arc<Mutex<Option<oneshot::Sender>>>` (not swallowed)
- session/load failure: leave projection unchanged, never silently create empty session
