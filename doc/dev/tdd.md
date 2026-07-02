# ACP Hub — TDD (Test-Driven Development)

> Grounded in: `doc/dev/spec.md`, `doc/dev/bdd.md`, `doc/ssot/pillars/TechSel.md`

## Test Strategy

| Level | Count | Framework | Purpose |
|-------|-------|-----------|---------|
| Unit (Store) | 6 | `#[test]` + in-memory SQLite | Schema, FTS, CAS, non-destructive load |
| Integration (Driver) | 5 | `#[tokio::test]` + in-process Testy | Capture path, cancel, load failure |
| Integration (Protocol) | 5 | `#[tokio::test]` + Testy binary | auth/logout, close, list pagination, fs/terminal |
| Integration (Proxy) | 1 | `#[tokio::test]` + conductor assembly | Proxy chain forwarding |
| Integration (Daemon) | 3 | `#[tokio::test]` + temp home | Idle exit, auto-spawn, singleton |
| Integration (MCP) | 1 | pipe + JSON-RPC | MCP facade smoke test |
| E2E (Real Agents) | 3 | manual / scripted | omp, codex, cursor create→send→search |
| Registry (Unit) | 3 | `#[test]` | add/remove/invalid-id |

## Unit Tests — Store (`tests/store.rs`)

### T1: FTS search finds appended message
- Create conversation, append message with "hub-search-token"
- `search("hub-search-token")` → 1 hit, conv_id matches
- **Covers**: BDD Feature 4 "Search message content"

### T2: Non-destructive load replay
- Append 2 messages (local_turn)
- `stage_load_replay` with 1 replayed message
- `messages(include_audit=false)` → 1 row (load_replay, current_projection=1)
- `messages(include_audit=true)` → 3 rows (2 superseded + 1 current)
- `search("original")` → still finds superseded audit message
- **Covers**: Two-layer data model, non-destructive semantics

### T3: CAS finalize only from running/cancelling
- `create_run` → `finalize_run_cas(Completed)` → true
- `finalize_run_cas(Cancelled)` again → false (already terminal)
- **Covers**: Run lifecycle state machine

### T4: Seq allocation contiguous
- Append 2 messages → seq 1, 2
- **Covers**: BEGIN IMMEDIATE + UNIQUE(conv_id, seq)

### T5: Conversation title search (conversations_fts)
- Create conversation with title "Planning Session"
- `search("Planning")` → conversation-type hit
- **Covers**: conversations_fts populated on create

### T6: Two-layer message display
- Append local_turn message + stage_load_replay message
- `messages(include_audit=true)` → both visible, different source values
- **Covers**: BDD Feature 2 "Both layers displayed independently"

## Unit Tests — Registry (`tests/registry.rs`)

### T7: Registry add/remove/invalid-id
- `agent add omp --command omp` → appears in list → JSON valid
- `agent remove omp` → disappears
- `agent add 'bad id!'` → rejected with "invalid agent id"
- **Covers**: BDD Feature 1 registration

## Integration Tests — Driver (`tests/testy_full_flow.rs`, `tests/concurrency.rs`)

### T8: Full capture path through Testy
- spawn_agent_connection(Testy) → CreateSession → SendPrompt(Echo)
- Store has messages, search finds echoed token
- **Covers**: BDD Feature 3 "Send and receive"

### T9: Send + cancel returns cancelled
- CreateSession → SendPrompt(RunScenario::Full) → cancel via cx.send_notification
- Prompt resolves with terminal stop_reason
- **Covers**: BDD Feature 3 "Cancel in-flight turn"

### T10: Load session binds BEFORE load (critical fix)
- CreateSession → LoadSession(nonexistent)
- If Testy accepts → messages captured (binding was set up first)
- If Testy rejects → projection unchanged
- **Covers**: Binding order fix, FAQ two-layer model

### T11: Concurrent send → serialized
- Two concurrent SendPrompt to same conv → second waits (single-flight)
- **Covers**: RunLease, per-conv mutex

### T12: Proxy chain assembly
- DynConnectTo::new(Testy) → spawn → CreateSession → SendPrompt
- Messages captured through conductor path
- **Covers**: BDD Feature 5 "Proxy chain"

## Integration Tests — Protocol Surface (`tests/protocol_surface.rs`)

> **Note**: This test file does not yet exist and must be created during implementation.

### T13: authenticate / auth_required / logout
- Capability-gated; TestyScenario::Callbacks exercises permission flow
- **Covers**: Spec 4 auth lifecycle

### T14: session/close (capability-gated)
- CreateSession → CloseSession → session unbound
- **Covers**: Spec 2 conversation CRUD

### T15: session/list cursor pagination + additionalDirectories
- ListSessions → multiple pages via cursor → complete dirs (no merge)
- **Covers**: FAQ session/list → Layer 1 discovery

### T15b: session/list auto-import loads messages (Layer 1)
- agent sessions cursor → session/list discovers sessions → for each, if load_session, session/load captures messages as load_replay
- conv list shows all imported sessions (not just Hub-created)
- conv show on imported session shows agent-original messages
- **Covers**: FAQ two-layer model, BDD "Discover pre-existing agent-side session"

### T15c: Fallback when agent doesn't support session/list
- Register agent without session/list capability
- conv list → shows only Hub-created conversations
- conv show → only local_turn messages (no load_replay)
- **Covers**: FAQ line 38 "only fallback to Hub capture when endpoint doesn't support"

### T16: fs/read_text_file full + ranged + limit
- Bind session with fs config → agent requests read → response correct
- **Covers**: design 2 capability negotiation

### T17: terminal/* all 5 methods
- create → output → wait_for_exit → kill → release
- **Covers**: design 2 terminal callbacks

## Integration Tests — Daemon (`tests/daemon_idle.rs`)

### T18: Idle exit after timeout
- serve(home) with IDLE_TIMEOUT=2 → exits after 2s idle
- **Covers**: BDD Feature 6 "Idle exit"

### T18b: Daemon auto-spawn
- No daemon running → first command spawns it → connects via JSON-RPC
- **Covers**: BDD Feature 6 "Auto-spawn daemon"

### T18c: Singleton enforcement
- Two concurrent ensure_daemon calls → same daemon (not two)
- Stale metadata → cleaned up on spawn
- **Covers**: BDD Feature 6 "Singleton enforcement"

## Integration Tests — MCP Facade (`tests/mcp_smoke.rs`)

### T19: MCP facade smoke test
- Pipe MCP initialize + tools/list to `acp-hub mcp` → 19 tools listed → call list_agents → result
- **Covers**: BDD Feature 7 "MCP tools available"

## E2E Tests — Real Agents (manual/scripted)

### T20: omp end-to-end
- agent add omp → conv create omp → send "Reply: TOKEN" → search "TOKEN" → hit
- **Covers**: Full pillar Spec 1-3 with real agent

### T21: codex end-to-end
- agent add codex (with CODEX_PATH env) → conv create → send → search
- **Covers**: Real ACP agent with streaming

### T22: cursor end-to-end (two-layer coexistence)
- agent add cursor → conv create → send → search → agent sessions cursor → conv list
- conv show displays both agent-original and hub-capture messages
- **Covers**: Layer 1 discovery + Layer 2 capture coexistence

## Verification Commands

```bash
cargo build --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# E2E (manual):
ACP_HUB_HOME=/tmp/test acp-hub agent add omp --command omp --args acp
ACP_HUB_HOME=/tmp/test acp-hub conv create omp
ACP_HUB_HOME=/tmp/test acp-hub send <conv> --text "Reply: TOKEN"
ACP_HUB_HOME=/tmp/test acp-hub search "TOKEN"
```
