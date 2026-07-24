# OpenAB SessionPool notes (for OMP vs ACP Hub)

**Date:** 2026-07-24  
**Scope:** process vs ACP session-id, resume/load failure semantics, idle eviction, hub design takeaways, OMP task-agent contrast.  
**Not covered:** chat adapters, Discord/Slack, agy-acp protobuf reverse-engineering, full ACP protocol matrix.

## Sources

| ID | Path / ref |
|----|------------|
| T1 | `doc/research/from-chatgpt/acp_discussion_docs/openab_acp_discussion_transcript.md` |
| T2 | `doc/research/from-chatgpt/acp_discussion_docs/acp_plugin_hub_discussion_transcript_2.md` |
| C1 | `repos/ref_repos/openab/src/acp/pool.rs` (local mirror of OpenAB SessionPool) |
| W1 | `doc/research/omp-vs-acp-hub-2026-07-24/00-WORKFLOW.md` (OMP vs hub framing) |

Line refs for C1 are from the local checkout as of this extract.

---

## 1. Process vs session-id decoupling

### Claim (research)

OpenAB’s highest-value ACP lifecycle rule: **the child process is a cache; the ACP `sessionId` is the durable state.**  
A live process is neither necessary nor sufficient proof of conversation continuity. Mapping must outlive the process. [T1 §1, §3–4; C1]

### Pool state model [C1 `PoolState`, ~L19–39]

| Map | Key | Value | Meaning |
|-----|-----|-------|---------|
| `active` | thread_key | `Arc<Mutex<AcpConnection>>` | Live CLI process + JSON-RPC transport |
| `cancel_handles` | thread_key | `(stdin, session_id)` | Cancel without holding connection mutex |
| `suspended` | thread_key | ACP `sessionId` | No live process; resume candidate via `session/load` |
| `persisted` | thread_key | ACP `sessionId` | Disk-backed map (active **and** suspended) for broker restart |
| `creating` | thread_key | gate mutex | Serialize create/resume; no double `session/load` races |
| `session_workdirs` | thread_key | cwd | Per-session workspace immutability after first bind |

Persistence files (under `~/.openab/`):

- `thread_map.json` ← `persisted` (thread → sessionId)
- `session_meta.json` ← workdirs

### Operational meaning

```text
thread_key (chat thread / external key)
  → ACP sessionId          # durable identity
  → optional live AcpConnection  # transport cache only
```

Implications already called out in research [T1]:

1. Killing or idle-evicting a process does **not** mean “forget conversation.”
2. Broker restart can still attempt `session/load` if the agent advertises `loadSession`.
3. `is_fresh` for directives/first-message rules treats “had process or had saved sessionId” as **not** a brand-new conversation—even after spawning a new process [C1 ~L368–372].

### Capability probe

On `initialize`, OpenAB records `agentCapabilities.loadSession` → `supports_load_session` on the connection. Resume path only uses `session/load` when that flag is true [T1 connection model; C1 ~L252–276].

---

## 2. Resume / load on transient failure — do **not** silently `session/new`

### Decision tree (`get_or_create`) [C1 ~L158–293; T1 §3.3]

```text
if active connection alive:
    reuse
else:
    spawn process + initialize
    if saved_session_id and supports_load_session:
        session/load(saved_session_id)
        on success → resume (resumed=true)
        on transient failure → ERROR, keep old sessionId, NO session/new
        on permanent failure → warn, fall through to session/new (+ session_reset flag)
    else:
        session/new
```

### Transient vs permanent [C1 L12–15, L259–285]

```rust
const TRANSIENT_LOAD_ERRORS: &[&str] = &["timeout waiting for", "channel closed"];
```

| Failure class | Behavior | Why |
|---------------|----------|-----|
| **Transient** (timeout / channel closed) | Preserve `persisted` sessionId; **return `Err`**; do not process current message against empty context | Next message can retry `session/load` automatically |
| **Permanent** (agent rejection / missing session file, etc.) | Log; call `session/new`; set `session_reset = true` if prior state existed | Honest “context lost” signal to upper layers |

Research wording (worth treating as a hub invariant candidate) [T1]:

> 失败时宁可中止，也不要悄悄新建空上下文 session。

### Race / lock notes that matter for hub reading [C1]

- Per-thread `creating` gate prevents concurrent double-load.
- Spawn/initialize/load run **outside** the pool state write lock so one stuck init does not block all sessions.
- After init, re-check active map (another task may have won the race).
- Eviction uses `remove_if_same_handle` so a replaced connection is not wrongly suspended.

### Explicit reset vs accidental new session

`reset_session` is the **only** intentional full forget: cancel, drop active/suspended/persisted/creating/workdir, save maps [C1 ~L457–497].  
Research notes OpenAB does **not** call ACP `session/delete`—reset is broker-side forget, not agent-side conversation deletion [T1 conversation matrix].

---

## 3. Idle eviction

Two related mechanisms:

### A. TTL idle cleanup — `cleanup_idle(ttl_secs)` [C1 ~L499–544]

1. Snapshot active connections.
2. `try_lock` only (busy/streaming sessions skipped — busy ≠ idle).
3. If `last_active < cutoff` **or** `!alive()`:
   - remove from `active` / `cancel_handles`
   - if sessionId present → insert into `persisted` + `suspended`
   - if no sessionId → drop from `persisted` (and workdir)
4. Save mappings.

Net: **evict process, keep sessionId** when available.

### B. Pool-full eviction on create [C1 ~L199–340]

When `active.len() >= max_sessions`:

- Scan unlocked candidates for oldest `last_active`.
- Suspend oldest idle handle into `persisted`/`suspended` (same process-vs-id split).
- If all others busy (skipped locks) → warn; may then fail with `pool exhausted`.

### C. Shutdown [C1 ~L546–577]

Flush all active sessionIds into `persisted`/`suspended`, clear active handles — process cache dropped, ids kept for later load.

### Design slogan (research + code)

> process is cache, session id is state [T1 cleanup_idle commentary; C1]

---

## 4. How this should influence hub design reading

Cross-walk with hub research goals [W1 disputed surfaces: connection lifecycle, resume/load after process death, RPC error folding].

| OpenAB rule | Hub-oriented reading |
|-------------|----------------------|
| Process ≠ session | Hub daemon/agent process death must not be equated with conversation loss if endpoint supports `session/load` and hub stored the backend session id. |
| Transient load → hard error, keep id | Folding failures into a generic “resume/load operation failed” is OK only if **mapping is retained** and the operator can retry; silent `session/new` on timeout is a **context-corruption bug**, not resilience. |
| Permanent load fail → new + `session_reset` | If hub creates a new backend session after genuine loss, surface **reset/context-lost** explicitly (do not pretend continuity). |
| Idle eviction keeps ids | Hub idle-exit / agent idle kill should mirror “suspend mapping, kill transport,” not “delete conversation.” Distinguish: daemon idle timeout (hub process) vs agent process pool TTL (endpoint child). |
| `creating` gate | Concurrent resume/send on same conv must not race into two loads or load+new. |
| `supports_load_session` | Capability-gated resume; if false, mark sessions stale after process death [T2 hot-reload resume rules]. |
| No silent empty context | Aligns with hub maintenance language elsewhere: do not silently replace user work / context. |

### What OpenAB is **not** (limits of the pattern) [T1]

OpenAB is an **ACP session runtime broker** (thread → sessionId → prompt stream), **not** a Conversation CRUD / message-store product:

- No first-class list/search conversations in the broker.
- No unified message history in the middle layer (upstream chat UI or agent-local store).
- Reset ≠ agent `session/delete`.

Hub already aims higher on conversation projection/search (product goal in README/W1). **Copy OpenAB’s process/session failure semantics**, not OpenAB’s thin conversation model.

### Plugin-hub transcript reinforcement [T2]

Recommended hub shape: Rust owns session/run/process; adapters implement hooks. On adapter reload:

```text
if loadSession supported → reuse backend session id
else → mark sessions stale, require new session
```

Same invariant as OpenAB pool, applied to multi-adapter hub.

---

## 5. Differences from OMP task-agent model

Framing from research workflow [W1]: **do not treat OMP as something hub should become** — hub is multi-endpoint conductor; OMP is a full agent product with a rich internal task/subagent model.

| Dimension | OpenAB SessionPool | OMP (product-internal task agents) | ACP Hub (target role) |
|-----------|-------------------|------------------------------------|------------------------|
| Identity key | External `thread_key` → one ACP session | Internal task/subagent graph inside one product agent | Hub `conversation` + registered **endpoint** + optional backend session id |
| Process | One pooled child per active thread (max_sessions, idle suspend) | Product owns worker lifecycle for tasks; not multi-vendor process pool | Many endpoints (omp, Codex, Cursor, Grok, …); per-endpoint process/daemon policy |
| Continuity | `session/load` + persisted map | Continuity is product-internal (task state, product session store) | Must **surface** vendor resume; cannot invent OMP-only task semantics for all agents |
| Failure semantics | Transient load refuses silent new session | Product can define task retry/replan inside one trust domain | Same OpenAB rule applies at conductor boundary; task-level retry stays **inside** OMP if at all |
| Idle policy | Evict process, keep ACP sessionId | Product may park tasks / workers under product policy | Split: hub daemon idle-exit vs agent child eviction; neither should silently rebind empty context |
| Permissions / params | OpenAB auto-allows permission for unattended bots [T1] | OMP product defaults (e.g. yolo-style) trusted by operator [W1] | Hub least-privilege conductor; must not copy OpenAB auto-allow or OMP yolo as universal hub defaults [W1] |
| Abstraction level | Session runtime only | Full agent product (tasks, subagents, params, UI) | Conductor + capture/search; multi-agent, not single-product task OS |

### Practical takeaway for OMP-vs-hub work

1. **Borrow from OpenAB (pool semantics):** process/session decoupling; transient-load non-silent-fail; idle = drop process not drop id; reset only when intentional.
2. **Do not borrow from OpenAB as product model:** auto-permission, no message store, thread-as-only-key.
3. **Do not force OMP task-agent model onto hub:** task agents are product-internal parallelism/decomposition; hub’s job is stable multi-endpoint session continuity and operator-clear failure surfaces.
4. When comparing “resume feels wrong on hub,” classify with W1 tags (I/R/O/C/V) against this OpenAB baseline first: is hub violating the OpenAB invariant, or is the endpoint (OMP/Cursor/…) V-dependent?

---

## 6. Minimal invariants checklist (extract)

Copy/paste candidates for comparison matrix / findings later:

- [ ] **I1** Backend session id is durable; process handle is cache.
- [ ] **I2** `session/load` timeout / connection-lost → error current turn; **retain** mapping; retry later.
- [ ] **I3** Never silently `session/new` while a recoverable id still exists.
- [ ] **I4** Genuine permanent load failure may `session/new` only with explicit reset/context-lost signaling.
- [ ] **I5** Idle/pool eviction moves id to suspended/persisted; does not delete conversation projection.
- [ ] **I6** Concurrent create/resume on same key is gated.
- [ ] **I7** Resume path gated on advertised `loadSession`.
- [ ] **I8** Intentional forget is explicit reset (and still may not delete agent-side history unless endpoint supports delete).

---

## 7. Pointers for deeper code reading

| Topic | File |
|-------|------|
| Pool lifecycle | `repos/ref_repos/openab/src/acp/pool.rs` |
| Transport / initialize / loadSession flag | `repos/ref_repos/openab/src/acp/connection.rs` |
| Event normalization | `repos/ref_repos/openab/src/acp/protocol.rs` |
| Research narrative | T1 (especially pool + “session id is real state”) |
| Hub plugin framing | T2 §7 session recovery rules |
| OMP vs hub goals | W1 |

---

*End of extract. ~focus only: pool continuity semantics for omp-vs-acp-hub research.*
