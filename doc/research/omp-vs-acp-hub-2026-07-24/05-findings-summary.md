# Findings summary — OMP reference vs ACP Hub design reality

> **SUPERSEDED for current product direction (2026-07-24).** This summary
> explained pain as intentional fail-closed. Operator correction + agent-managed
> Product-UX now prioritize usable defaults and non-fatal lag. Keep for research
> history only; implement against `doc/ssot/agent-managed/`.

**Date:** 2026-07-24  
**Status:** research complete (no code changes)  
**Audience:** operator / maintainer deciding whether hub is “broken by refactor” or “painful by intent”

---

## 1. Direct answer to the operator concern

> “机制、调用参数、配置看起来都不合理——是不是 review/重构改坏了？”

**Most disputed behaviors are intentional (I), often reviewed into place during 0.2.0 / PR #29, not random regressions.**

| Feeling | Reality |
|---------|---------|
| “Default 不能写文件” | **I** least privilege (F-028, CHANGELOG 0.2.0), not a regression to safer defaults |
| “send 中途断了” | **I** lag-fatal (R-DAEMON-004) *fixed* a prior silent-skip defect; Cursor high churn stresses it |
| “resume/load 失败文案像 daemon 挂了” | **I** privacy/safe RPC fold + **C** diagnostic opacity |
| “agent add / list 卡住” | **I** generation locks + **C**/Windows rotten state after kill |
| “Cursor 不能干活” | **False** as sole cause — E2E once wrote disk; vendor load fragility is **V** |

What *did* get “fixed hard” in maintenance is **fail-closed security and projection integrity**. That can feel like product failure when the operator wants OMP-like unattended smoothness. Those are different products.

---

## 2. Category error: OMP satisfaction ≠ hub defect

OMP’s most loved surfaces:

- **Task / subagent** orchestration (typed agents, batch, park/revive, yield contract)
- **Approval** with clear tiers + explicit yolo for unattended
- **Session** continuity (JSONL, load vs resume, artifacts)
- **ACP** as first-class mode with careful cancel/bootstrap/stdout hygiene

ACP Hub’s pillars:

- Register **many** external ACP endpoints
- Conductor + **projection capture**
- Capability-gated ops
- Least-privilege **client** callbacks
- On-demand **daemon**

Porting OMP task trees into CoreHub would **violate** hub SSOT (hub is not a full agent product).  
**Correct use of OMP satisfaction:** run `omp acp` *as a registered endpoint* when you want task agents; use hub when you need multi-vendor + search projection.

---

## 3. Attribution dashboard (from history lane)

| Topic | Tag | Confidence | One line |
|-------|-----|------------|----------|
| `permission_policy` reject + fs/terminal off | **I+C** | high | Reviewed least privilege; friction is surface |
| Lag → close connection | **I** (was **R** pre-fix) | high | R-DAEMON-004 in `015733dc` / PR #29 |
| ensure_live resume→load | **I** | high | Capability recovery path |
| Error fold to “daemon unavailable: resume/load…” | **I+C** | high | SafeResumeSourceData intentional |
| Broadcast 1024/256 + per-update fanout | **I+C** | med | Bounds intentional; small vs Cursor |
| mutate_registry wait/reject | **I** | high | Generation safety / R-REG-001 |
| Daemon singleton + idle + Win pipes | **I** | high | Pillar D5 from day one |
| Dual Layer1/Layer2 history | **I** | high | Founding data model |
| Cursor local-only load → reject prompt | **I+V** | high | Adapter safety on vendor fail |
| Fail-closed philosophy | **I** (pain **C**) | high | Consistent review ledger |

**`wip/concurrent-refactor` is not authoritative** for current intent (pre–0.2.0).

---

## 4. OMP patterns worth learning (without redesign)

| Pattern | OMP does | Hub-compatible learning |
|---------|----------|-------------------------|
| Explicit automation switch | `--yolo`, `--config` overlay | One documented **trusted-write registration profile** (flags already exist) |
| Load ≠ resume | Different product ops | Keep matrix; document when ensure_live fires |
| Fail closed | No silent allow / no empty session | Keep; do not “soft succeed” |
| Process = cache | SessionId durable | Same intent; harden rebind + diagnostics |
| Subagents not separate ACP | Internal multi-agent | **Do not** invent hub subagents; use OMP endpoint |
| Parent authorizes child power | task=exec boundary | Analog: operator explicitly opts agent into auto-allow |

OpenAB (already studied in hub research): transient load fail → **error current turn, keep id, never silent new** — hub already aims here via ResumeLoadFailed.

---

## 5. What not to do next (stability constraint)

From architecture map + operator instruction:

1. **Do not** replace daemon/RPC with in-process only “like OMP.”  
2. **Do not** fold dual history into single layer.  
3. **Do not** default auto-allow globally (undoes F-028).  
4. **Do not** reintroduce silent lag skip (undoes R-DAEMON-004).  
5. **Do not** implement OMP task/subagent inside CoreHub.  
6. Prefer **surface** fixes: profiles, docs, error Display for typed ResumeLoadFailed, Windows clean-home checklist, optional larger buffers / coalescing **as knobs** if proven needed.

Any code change should cite: which invariant stays, which **C** is narrowed, and which test/review ID it must not violate.

---

## 6. Recommended research follow-ups (still non-redesign)

1. **OMP-via-hub smoke** on same Windows host (contrast Cursor E2E) — isolates vendor vs hub.  
2. Measure notification rate: OMP ACP vs Cursor under one send.  
3. Document operator “golden path” for trusted write (auto-allow + roots + single-flight).  
4. If diagnostics remain top pain: propose **minimal** typed error Display preservation (not full stack traces).  

---

## 7. Document pack index

| File | Content |
|------|---------|
| [00-WORKFLOW.md](./00-WORKFLOW.md) | Research method, tags, non-goals |
| [01-omp-subagents-task-agents.md](./01-omp-subagents-task-agents.md) | OMP task/subagent gold reference |
| [02-acp-hub-architecture.md](./02-acp-hub-architecture.md) | Hub as-implemented map + invariants |
| [03-history-attribution.md](./03-history-attribution.md) | Git/review I/R/O/C/V table |
| [04-comparison-matrix.md](./04-comparison-matrix.md) | Side-by-side axes + transfer table |
| [05-findings-summary.md](./05-findings-summary.md) | This summary |
| [raw/omp-acp-surface.md](./raw/omp-acp-surface.md) | OMP ACP endpoint deep dive |
| [raw/openab-session-pool-notes.md](./raw/openab-session-pool-notes.md) | OpenAB process/session notes |

---

## 8. Bottom line

1. **Hub is not “ruined by review.”** Review **hardened** fail-closed, least privilege, and projection integrity.  
2. **OMP is the right reference for lifecycle/permission *discipline***, not for replacing hub’s conductor role.  
3. **Operator pain is real** but mostly classified **C** (clumsy defaults/docs/errors) on top of **I** (security/correctness), plus **V** for Cursor.  
4. Next engineering, if any, should be **surgical surface + diagnostics + Windows lifecycle**, with SSOT invariants frozen.
