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
1. **Bind the session BEFORE** sending `LoadSessionRequest`
   (agent_session_id is known upfront from session/list)
2. Agent replays messages via `session/update` notifications DURING the request
3. Notification handler finds binding → captures messages into Store with `source='load_replay'`
4. Request completes → messages are already in projection

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
- Non-destructive `session/load` replay: supersede prior rows, retain as audit
- `conversations_fts`: title search
- `messages_fts`: body search (search_body extractor skips base64)

### 3.3 ACP Driver (`acp.rs`)
- Per-agent connection task: `Client.builder().connect_with(component, |cx| { loop })`
- Notification handler: exhaustive capture of ALL `session/update` variants
- Callback handlers: permission/fs(read+write)/terminal(create/output/wait/kill/release)
- Cancel: `cx.send_notification(CancelNotification)` directly (bypasses blocked loop)
- Capability gating: every ACP call checked against advertised capabilities
- **Binding order**: bind BEFORE load/resume, AFTER create

### 3.4 Conductor (`conductor.rs`)
- Empty proxy chain → direct agent
- Non-empty → `ConductorImpl::new_agent(name, ProxiesAndAgent::new(agent).proxy(p1)...)`

### 3.5 CoreHub (`hub.rs`)
- Owns Registry + Store + RuntimeCache
- `list_agent_sessions`: session/list → upsert metadata → load messages for each (if capability)
- `create_conversation`: session/new (or session/load if agent_session_id provided)
- `send_prompt`: per-conv mutex (single-flight) → create_run → set_current_run → prompt → finalize
- `messages(conv_id)`: return BOTH layers, labeled by source

### 3.6 Daemon (`daemon.rs`)
- On-demand singleton: file lock + socket/pipe + metadata JSON
- Idle exit: `active_clients=0 AND active_runs=0 AND elapsed > IDLE_TIMEOUT`
- `ensure_daemon(home)`: discover or spawn

### 3.7 Entry Points
- CLI: clap derive, 23+ subcommands, all via HubClient RPC
- MCP: rmcp ServerHandler with 19 #[tool] methods
- Embedded: HubClient::connect_or_spawn (same RPC path as CLI/MCP)

## 4. Transport Design

| Transport | Component | Construction |
|-----------|-----------|-------------|
| stdio | `AcpAgent::new(McpServer::Stdio(...))` | command + args + env |
| HTTP | `HttpClient::with_client(url, reqwest::Client)` | url + headers |
| WebSocket | `HttpClient::with_client(ws_url, reqwest::Client)` | ws/wss url + headers |
| Proxy (stdio) | `AcpAgent` as `ConnectTo<Conductor>` | command + args + env |

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
