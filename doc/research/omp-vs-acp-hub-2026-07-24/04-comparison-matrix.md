# OMP (task/subagent + ACP) vs ACP Hub — comparison matrix

> **Note (2026-07-24):** Hub rows that cite least-privilege defaults / lag-fatal
> as *current* hub behavior are outdated after the UX rebalance. See
> `doc/ssot/agent-managed/pillars/Product-UX.md`.

**Date:** 2026-07-24  
**Inputs:** `01-omp-subagents-task-agents.md`, `raw/omp-acp-surface.md`, `02-acp-hub-architecture.md`, `03-history-attribution.md`, `raw/openab-session-pool-notes.md`  
**Rule:** Compare *roles*, not products. OMP is a full agent product; hub is a multi-endpoint conductor.

Attribution tags (from `03`): **I** intentional · **R** regression-then-fixed · **O** original incomplete · **C** clumsy surface · **V** vendor.

---

## 0. Category mismatch (must read first)

| Dimension | OMP | ACP Hub |
|-----------|-----|---------|
| Product type | Single agent runtime (TUI / RPC / ACP / tasks) | Local multi-endpoint ACP client + conductor + projection |
| “Agent” | Main session + typed task subagents (in-process) | Registered external endpoint (`omp`, `cursor`, …) |
| Trust boundary | One product; subagent yolo authorized by parent `task` | Untrusted external processes; least-privilege client defaults |
| Continuity unit | Session JSONL + registry + artifacts under product home | Hub conversation row + agent `sessionId` + SQLite projection |
| What user optimizes | Task delegation UX, resume, approvals | Endpoint registration, capture fidelity, capability gating |

**Implication:** Lifting OMP subagent semantics *into* hub core would change hub’s pillar role. Useful transfer is **lifecycle/permission/resume *patterns***, not copying task graphs into CoreHub.

---

## 1. Parameters / configuration

| Concern | OMP | ACP Hub | Gap / note | Hub tag |
|---------|-----|---------|------------|---------|
| Config surface | Rich `settings-schema` + global/project/`--config`/flags; task.* knobs | Thin ACP session snapshots: `param list/set`, `mode list/set` map to agent-advertised options | Hub correctly defers model/thinking to agent; operator may expect OMP-like profiles | **I** (thin hub) + **C** (no “trusted write profile” preset) |
| Inheritance | Parent → child: model overrides, tiers, skills, context, cwd; forces child `approvalMode=yolo`, `async.enabled=false` | No subagent concept; each endpoint owns settings | N/A to core hub | — |
| Per-session policy | ACP: no session-level approval field; need separate process or overlay for per-session yolo | `permission_policy` is **per registered agent**, not per conversation | Both lack ACP-native per-session approval; OMP uses process launch knobs | **I** registry-scoped |
| Discoverability | `param` equivalent is rich slash + config docs | `param list` / `mode list` after create | Works if session live; hangs/load fail hide options | **C** under broken live session |

**OMP pattern worth learning (without redesign):** explicit, documented **launch profiles** (`omp acp --yolo`, `--config acp-yolo.yml`) so “automation mode” is one intentional switch—not silent default.

---

## 2. Permissions / approval

| Concern | OMP | ACP Hub | Gap / note | Hub tag |
|---------|-----|---------|------------|---------|
| Default interactive product | Schema default `yolo`; tools tiered read/write/exec | N/A (no tools of its own) | Different trust model | — |
| ACP unattended | Client permission **gate kept** unless **explicit** yolo config/flags; fail-closed on reject | Sample + default enum: `permission_policy: reject`; fs/terminal **off** | Stacked least-privilege: hub rejects callbacks *and* may not advertise fs | **I+C** (F-028 / least privilege) |
| Subagent / nested | Subagents force yolo; parent `task` is auth boundary | No nested agents in hub | Hub “nested” = separate endpoint process | — |
| Failure mode | Reject/cancel → tool fails; no silent allow | Policy reject → first reject option / cancelled | Aligned fail-closed | **I** |
| Operator pain | Must set yolo explicitly for headless ACP | Must re-register with `auto-allow` + roots for write smoke | Same class of pain; OMP documents it better | **C** |

**Key OMP insight:** two independent layers—(1) product approval mode, (2) ACP client gate. Hub only implements layer (2) as a static policy. That is **not a bug**; missing **documented one-command trusted profile** is the **C**.

---

## 3. Connection / process lifecycle

| Concern | OMP | ACP Hub | Gap / note | Hub tag |
|---------|-----|---------|------------|---------|
| Process model | One `omp acp` process; multi-session map in memory | Daemon singleton per home; per-agent long-lived ACP connection task | Hub adds IPC hop | **I** (pillars D5) |
| Session vs process | Session JSONL durable; process = runtime | Conversation + agent sessionId in SQLite; agent process is cache-like but **Live** binding is memory-runtime | OpenAB-style “process is cache” is partially present via ensure_live, fragile under daemon death | **I** design, **C/O** operational hardness on Windows |
| Idle behavior | Subagent idle TTL → park → revive; main not parked | Daemon idle exit (default 1800s) when quiescent | Daemon exit drops Live bindings → next op must resume/load | **I** |
| Concurrency | task.maxConcurrency; generation of child sessions | Single-flight ops per conversation; generation gates per agent | Hub serializes endpoint replacement vs commands | **I** |
| Kill recovery | Parked/revivable vs hard abort terminal | Force-kill leaves pipes/DB/locks rotten (E2E) | Hub Windows recovery weaker than OMP lifecycle manager | **C** / platform **O** edges |
| Subagent isolation | Optional worktree; default in-process | N/A | — | — |

**OpenAB cross-check:** best practice = keep sessionId durable; transient load fail → **error, no silent new session**. Hub’s `ResumeLoadFailed` + “never empty session on load fail” matches that **I**. Pain is when Live is lost and load fails or errors are opaque.

---

## 4. Resume / continuity

| Concern | OMP | ACP Hub | Gap / note | Hub tag |
|---------|-----|---------|------------|---------|
| Load vs resume | `session/load` opens **and replays**; `session/resume` opens **without** replay | Prefer resume if capability; else load; wrap failures | OMP splits semantics cleanly; hub collapses recovery into ensure_live | **I** |
| Subagent resume | Registry park + revive from sessionFile; `agent://` / artifacts | N/A | OMP internal | — |
| After disconnect | Same process multi-session; dispose on connection close | New CLI process reconnects daemon; may re-spawn agent | After daemon restart, ensure_live must reload agent session | **I** path, **C** if agent load weak |
| Failure honesty | No silent allow; revive failures surface | No silent new empty session on load fail | Aligned with OpenAB | **I** |
| Error opacity | Product-level errors | Nested source folded: often `daemon unavailable: resume/load operation failed` | Diagnostic loss | **I+C** (privacy fold) |
| Vendor ACP load | OMP owns full load/resume | Cursor adapter: upstream load fail → local-only → **reject prompt** | Safety **I** + vendor **V** | **I+V** |

---

## 5. Streaming / notifications / progress

| Concern | OMP | ACP Hub | Gap / note | Hub tag |
|---------|-----|---------|------------|---------|
| Progress | Event bus + tool cards; subagent progress channels | Every session update → store + `hub/conv/update` | High fanout under chatty agents | **I+C** |
| Lag policy | Product buffers UI | **Lag = connection-fatal** (R-DAEMON-004) | Correctness over silent gap; stresses Cursor | **I** (post-0.2.0) |
| Subagent vs ACP | Subagents **not** separate ACP sessions | Each registered endpoint is separate ACP | Hub multi-agent = multi-endpoint, not OMP task tree | — |

---

## 6. Registry / discovery of “agents”

| Concern | OMP | ACP Hub | Gap / note | Hub tag |
|---------|-----|---------|------------|---------|
| Discovery | `.omp/agents`, user, plugins, bundled; first-wins | `agents.json` explicit register | Hub is operator-declared | **I** |
| Mutation safety | Settings/files; runtime rediscover | `mutate_registry` waits generation writers; rejects busy ops | Intentional mid-turn safety | **I** |
| Spawn policy | `spawns` allowlist, depth, PI_BLOCKED_AGENT | N/A | — | — |

---

## 7. What OMP does *not* solve for hub

| OMP strength | Why it does not auto-fix hub |
|--------------|------------------------------|
| Task tool + in-process subagents | Hub must drive **external** ACP binaries safely |
| Parent-authorizes-child yolo | Hub cannot “trust” an external Cursor process like an in-process child |
| Agent Hub kill/revive/IRC | Product UI; hub is library/CLI conductor |
| Artifact/agent:// continuity | Vendor-owned session stores differ |

---

## 8. What hub does *not* need to become OMP

From hub pillars (do not casually redesign):

1. Multi-endpoint registration without opinionated agent product  
2. Two-layer projection (agent original ∥ hub capture)  
3. Capability-gated operations  
4. On-demand singleton daemon  
5. Fail-closed least privilege for client callbacks  
6. Adapters own private vendor storage (core ACP-only)

Transferring OMP patterns should **preserve** these, not replace them.

---

## 9. Transferable patterns (stable, non-redesign)

| # | OMP / OpenAB pattern | Hub-compatible form (research only) | Touches |
|---|----------------------|-------------------------------------|---------|
| T1 | Explicit automation launch (`--yolo` / config overlay) | Documented “trusted local write” **registration profile** (flags/docs already exist; packaging) | **C** surface |
| T2 | Load ≠ resume semantics | Keep capability matrix; improve operator docs on when ensure_live runs | **I** keep |
| T3 | Fail closed, never silent empty session | Keep ResumeLoadFailed semantics | **I** keep |
| T4 | Transient load fail → retryable error, preserve id | Align diagnostics; avoid mislabel as “daemon unavailable” when source is ACP | **C** fold |
| T5 | Process is cache; sessionId durable | Strengthen Live rebind after daemon/agent death without silent new | **I** path, **C** ops |
| T6 | Subagent yolo only after parent task approval | Not applicable to multi-endpoint; keep per-agent policy | — |
| T7 | Soft budgets / max concurrency | Hub already has capture budgets; lag-fatal is budget extreme | **I** |
| T8 | Clear park/revive vs hard kill | Windows clean shutdown checklist + home isolation | **C**/platform |

---

## 10. E2E failure mapping (Cursor report → this matrix)

| E2E symptom | Matrix row | Attribution |
|-------------|------------|-------------|
| Write works, send fails | §5 lag-fatal + §3 daemon hop | **I** hub + **V** Cursor churn |
| create OK, param/send load fail | §4 ensure_live + vendor load | **I** + **V** + **C** errors |
| Default cannot write tools | §2 least privilege | **I+C** |
| agent add / list hang | §3/§6 generation locks / rotten daemon | **I** + Windows **C** |
| Access denied after kill | §3 kill recovery | Platform **C/O** |

---

## 11. One-line verdict per axis

| Axis | Verdict |
|------|---------|
| Params | Hub intentionally thin; OMP rich — gap is **profiles/docs**, not wrong abstraction |
| Permissions | Both fail-closed; hub defaults stricter for multi-endpoint trust — **I**, pain **C** |
| Connection | Hub daemon+IPC is pillar; OMP single process simpler — reliability gap is **ops**, not wrong pillar |
| Resume | Intent aligned with OpenAB honesty; opacity + vendor load fragility drive failures |
| Subagents | **Do not port** into CoreHub; use OMP *as* an endpoint when task trees are needed |
