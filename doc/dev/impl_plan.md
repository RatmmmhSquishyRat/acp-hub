# ACP Hub — Implementation Plan

> Grounded in: `doc/dev/spec.md`, `doc/dev/design.md`, `doc/dev/bdd.md`, `doc/dev/tdd.md`
> Pillars: `doc/ssot/pillars/README.md`, `doc/ssot/pillars/TechSel.md`
> Dev principles: `doc/ssot/dev-principles/实现规划原则.md`

## Current State

已实现的模块（编译通过，13 tests pass，clippy clean）：
- endpoint.rs — JSON registry ✅
- store.rs — SQLite + FTS5 ✅
- transport.rs + conductor.rs — SDK transports + proxy chain ✅
- callbacks.rs — exhaustive capture + fs/terminal/permission ✅
- acp.rs — driver + connection loop + capability gating ✅
- runtime.rs — RuntimeCache + RunLease ✅
- hub.rs — CoreHub + HubClient ✅
- rpc.rs — JSON-RPC ✅
- daemon.rs — singleton daemon ✅
- main.rs — CLI 23+ subcommands ✅
- mcp.rs — MCP facade 19 tools ✅

## Required Changes (v1 → v2: Two-Layer Data Model)

### Change 1: Fix LoadSession binding order (CRITICAL BUG)

**文件**: `crates/hub/src/acp.rs`
**问题**: LoadSession/ResumeSession arm 在 load 完成后才 bind session。但 session/load 在请求期间通过 session/update notification 回放消息。binding 未就绪 → 所有回放消息被丢弃。
**修复**:
```
// LoadSession arm — BEFORE calling load_session():
ctx.bind_session(&agent_session_id, SessionBinding {
    conv_id, agent_id, permission_policy, fs, cwd
});
// THEN call load_session — notifications captured during await
```
**验证**: T9 (load session binds before load)

### Change 2: session/list auto-import with message loading

**文件**: `crates/hub/src/hub.rs` — `list_agent_sessions()`
**当前**: upsert metadata only
**修改为**:
```
for each SessionInfo:
    1. upsert_agent_session(agent_id, session_id, title, cwd, dirs)
    2. IF agent_capabilities.load_session:
       a. bind session (conv_id ↔ session_id)
       b. send LoadSession command → agent replays messages
       c. notification handler captures with source='load_replay'
       d. unbind session (or keep for future prompts)
```
**验证**: T14, T20

### Change 3: conv show displays both layers with labels

**文件**: `crates/hub/src/hub.rs` — messages RPC; `crates/cli/src/main.rs` — conv show
**当前**: messages query returns rows but doesn't label source
**修改为**:
- RPC response includes `source` field per message
- CLI prints `[agent-original]` for source='load_replay', `[hub-capture]` for source='local_turn'
**验证**: T6

### Change 4: conversations_fts title search (already implemented)

**文件**: `crates/hub/src/store.rs`
**状态**: ✅ 已实现 (create_conversation + apply_session_info 插入 FTS, search_conversations 查询)
**验证**: T5

## Implementation Order

1. **Change 1** (binding order fix) — acp.rs, 2 arms (LoadSession + ResumeSession)
2. **Change 2** (session/list message loading) — hub.rs list_agent_sessions
3. **Change 3** (two-layer display) — hub.rs messages RPC + main.rs conv show
4. **Verify**: cargo test + clippy + E2E with cursor (agent sessions → conv list → conv show)

## Non-Changes (Already Complete)

- Registry, Store schema, FTS, search_body extractor ✅
- Transport/conductor/proxy chain assembly ✅
- Callbacks (permission/fs/terminal) ✅
- Driver (connection, capability gating, cancel via cx) ✅
- RuntimeCache + RunLease ✅
- Daemon singleton + idle exit ✅
- CLI 23+ subcommands ✅
- MCP facade 19 tools ✅
- Error propagation (Arc<Mutex<Option<oneshot::Sender>>>) ✅

## Adapters

```
adapters/
├── omp/agents.json      — { command: "omp", args: ["acp"] } ✅ verified
├── codex/agents.json    — { command: "cmd", args: ["/c","codex-acp"], env: {CODEX_PATH:...} } ✅ verified
└── cursor/agents.json   — { command: "cmd", args: ["/c","...cursor-agent.cmd","acp"] } ✅ verified

### Change 5: Create missing test files

Per TDD doc, the following test files do not yet exist and must be created:
- `tests/protocol_surface.rs` — T13-T17 (auth, close, list pagination, fs, terminal)
- `tests/mcp_smoke.rs` — T19 (MCP initialize + tools/list smoke test)
- `tests/registry.rs` — T7 (registry add/remove/invalid-id unit tests)
- Daemon auto-spawn + singleton tests (T18b, T18c) added to `tests/daemon_idle.rs`
```
