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
1. Send `NewSessionRequest` → get `agent_session_id` (only known after response)
2. **Bind the session AFTER** response
3. Subsequent prompts capture via notification handler with `source='local_turn'`

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
- Cancel: `cx.send_notification(CancelNotification)` directly (bypasses blocked loop)
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
  `session/cancel` 前重新核对 token/run/session，不能取消替换后的新 run。
- replay lock map 在同一互斥区内按 guard user 计数；`lock_owned()` 的临时
  `Arc` 不参与清理判定。

所有 Hub 生产与测试文件维持在 1,000 行以下，并以约 900 行作为主动拆分
边界。该边界不改变 `crate::hub::*` 公共 API。
### 3.6 Daemon (`daemon.rs`)
- On-demand singleton: file lock + socket/pipe + metadata JSON
- Idle exit: `active_clients=0 AND active_runs=0 AND elapsed > IDLE_TIMEOUT`
- `ensure_daemon(home)`: discover or spawn

### 3.7 Entry Points
- CLI: clap-derived command surface, all daemon operations via HubClient RPC
- MCP: rmcp ServerHandler tools backed by the same HubClient path
- Embedded: HubClient::connect_or_spawn (same RPC path as CLI/MCP)

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
chain acknowledges one message per leg in strict FIFO order because proxy
request ids and methods are connection-local. This assumes the supported proxy
contract is one logical input to one logical output; a proxy is operator-chosen
executable code, not a sandbox boundary. Callback updates, filesystem payloads,
terminal output, daemon RPC, and message pages have tighter
operation-specific limits.

## 5. Capability Matrix

| Operation | Required Capability | Error if unsupported |
|-----------|--------------------|-----------------------|
| session/load | `load_session: true` | `UnsupportedCapability` |
| session/resume | `session_capabilities.resume.is_some()` | `UnsupportedCapability` |
| session/close | `session_capabilities.close.is_some()` | `UnsupportedCapability` |
| session/delete | `session_capabilities.delete.is_some()` | `UnsupportedCapability` |
| image/audio prompt | `prompt_capabilities.image/audio` | `UnsupportedCapability` |
| fs callbacks | advertised client caps | typed error |
| terminal callbacks | advertised client caps | typed error |

## 6. Error Handling

- `HubError`: UnsupportedCapability, UnsupportedProxyTransport, AuthRequired, ResumeLoadFailed, Conflict(conversation_busy), DaemonUnavailable, Acp(#[from])
- Init/connection errors propagated via `Arc<Mutex<Option<oneshot::Sender>>>` (not swallowed)
- session/load failure: leave projection unchanged, never silently create empty session
