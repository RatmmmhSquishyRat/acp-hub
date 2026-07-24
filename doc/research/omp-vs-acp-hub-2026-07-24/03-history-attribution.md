# History attribution — disputed acp-hub behaviors

> **HISTORICAL attribution only.** Verdicts that label reject-default or
> lag-fatal as intentional **I** describe 0.2.0 review-era product law.
> **Current product overlay (agent-managed):** usable defaults + lag non-fatal —
> see `doc/ssot/agent-managed/`. Attribution of *why* old code existed remains valid.

**Date:** 2026-07-24  
**Repo:** `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub`  
**HEAD (main):** `05c2d2c6dcd9fc6f644d80e60cf0641d697bad16`  
**Mode:** attribute only (I / R / O / C / V). No redesign recommendations.

## Method

| Source class | What was used |
|---|---|
| Git refs / reflog | `.git/logs/HEAD`, `.git/logs/refs/heads/main`, `.git/refs/heads/main`, `.git/refs/remotes/origin/wip/concurrent-refactor` |
| Dated product history | `CHANGELOG.md`, release notes in review/maintenance docs |
| Review ledger | `doc/review/complete-review-book-2026-07-18.md` (F-*, R-*) |
| Task / closure | `doc/maintenance/complete-task-plan-2026-07-18.md`, `final-completion-summary-2026-07-19.md`, `continuation-handoff-2026-07-18.md` |
| SSOT / design | `doc/ssot/pillars/README.md`, `doc/dev/spec.md`, `doc/dev/design.md`, `doc/dev/bdd.md`, `doc/dev/impl_plan.md`, `doc/roles/implementer.md` |
| Current code | paths listed per topic |

**Confidence key**

- **high** — current code + explicit review finding ID / SSOT principle + named commit or changelog line  
- **med** — strong doc/code agreement; exact first-introducing SHA not fully blameseeked via `git log -p` in this pass  
- **low** — inference from structure / default Rust derives without a dated “introduce” commit

**Legend**

| Tag | Meaning |
|---|---|
| **I** | Intentional design (documented principle / reviewed decision) |
| **R** | Regression during review/refactor (history shows earlier better behavior, then a fix restored intent) |
| **O** | Original incomplete / bug-like incomplete path |
| **C** | Correct for hub role, but clumsy operator surface |
| **V** | Vendor-dependent; hub/adapter only surfaces it |

---

## Anchor commits (shared)

| SHA | Message (from reflog / docs) | Role |
|---|---|---|
| `6085b649eaa5278ea6774600cf2980136a2c5122` | Initial import: ACP Hub — ACP client/conductor core | Public line origin (0.1.0 era) |
| `ddb0d8d6e3920971a5c29cb485609b703400270c` | feat(cursor-adapter): local read-only ACP session list + load | Cursor local list/load surface |
| `80f6d0f7a8b6ccd59229ff22fedf2a6a37489ff8` / v0.1.2 | release: v0.1.2 cursor adapter, daemon tests, and acp-hub skill | Ships cursor adapter + skill |
| `1dbd986ca6ecbf0be6e44896ce4657ab319f23a4` … `d89b254ff5908e05bd577d3ac3d66500406726a8` | fix: address outstanding Codex review feedback → fix: complete ACP Hub 0.2.0 maintenance | Large 0.1.3/0.2.0 maintenance series on `codex/resolve-review-feedback` |
| `9633c96af24242534b49d027ac633e25f1b7c94b` | fix: recover daemon startup after singleton exit | R-DAEMON-001 |
| `12a7fc2d95cf785acb238e35cb9ccaf312eb2cab` | chore(deps): update clap and uuid *(review baseline cited in book §12)* | Pre–second-pass PR review checkpoint |
| `2311943256994cbfa49f631a27ddd80cf7062d12` | fix: address complete PR review feedback | Closes many §11 findings |
| `015733dc1ecd2e77d3d659ce978613aa9384a60e` | fix: close final release review findings | Closes R-DAEMON-004 + four peers (T-611) |
| `148e42d12e450f926785814612ccb456d64e5077` | PR [#29](https://github.com/RatmmmhSquishyRat/acp-hub/pull/29) squash merge; tag `v0.2.0` | Published 0.2.0 (2026-07-19) |
| `05c2d2c6dcd9fc6f644d80e60cf0641d697bad16` | docs: record 0.2.0 publication closure (+ follow-ups) | Current `main` |

---

## Summary table

| # | Topic | Verdict | confidence | Primary evidence |
|---|---|---|---|---|
| 1 | `permission_policy` default `reject` + disabled fs/terminal | **I** + **C** | high | `#[default] Reject`; F-028; CHANGELOG least privilege; README warning |
| 2 | Daemon notification lag connection-fatal (R-DAEMON-004) | **I** *(after deliberate R→I fix)* | high | Review book R-DAEMON-004; `015733dc`; design/spec/bdd; CHANGELOG 0.2.0 |
| 3 | `ensure_live_session` resume then load after Live binding lost | **I** | high | `conversation.rs`; design load/resume matrix; ResumeLoadFailed wrap |
| 4 | ResumeLoadFailed source folded to `daemon unavailable: resume/load operation failed` over RPC | **I** *(privacy)* + **C** *(operator opacity)* | high | `SafeResumeSourceData::into_hub_error`; F-016/R-PRIV family |
| 5 | Broadcast 1024 hub / 256 client; `hub/conv/update` per session update | **I** *(bounds)* + **C** *(small buffers vs high churn)* | med | `connection.rs:59`, `rpc.rs:218`, `capture.rs` notify |
| 6 | `mutate_registry` waits on generation write locks; idle/active agent gates | **I** | high | `registry.rs` + test `endpoint_removal_waits_for_old_load`; F-010 / R-REG-001 |
| 7 | Daemon singleton + idle exit + Windows named pipes | **I** | high | Pillars D5; `daemon.rs`; F-017; R-DAEMON-001/002 |
| 8 | Layer1 (agent original) vs Layer2 (hub projection) dual history | **I** | high | Pillars FAQ; spec §3; store `load_replay` / `local_turn` |
| 9 | Cursor adapter local-only ACP load then reject prompt | **I** + **V** | high | `adapter.mjs` `localOnlyAcpLoads`; cursor-adapter spec; ddb0d8d6 / 0.1.2 |
| 10 | Intentional “fail closed” security philosophy vs accidental UX pain | **I** *(philosophy)*; pain is often **C** of that **I** | high | Review book + SECURITY.md + adapter fail-closed; README least privilege |

---

## Per-topic detail

### 1. `permission_policy` default reject + disabled fs/terminal

| Field | Value |
|---|---|
| **Verdict** | **I** (security default) + **C** (operator friction when write workflows need `auto-allow` + fs/terminal) |
| **confidence** | high |
| **Code** | `crates/hub/src/endpoint.rs` — `PermissionPolicy` `#[default] Reject`; `ClientCapabilityConfig` / `FsConfig` bools default `false` via `#[serde(default)]` + `Default`. Samples: `adapters/cursor/agents.json`, `adapters/codex/agents.json` (`permission_policy: "reject"`, fs false, terminal false). |
| **History** | Review **F-028** (medium): samples had unsafe / high-permission defaults → closure “make sample registry permissions **deny-by-default**”. Ledger: F-028 resolved with “least-privilege defaults”. Task plan T-500: “make sample permissions deny-by-default”. CHANGELOG 0.2.0 *Included from 0.1.3 candidate* → **Changed**: “Sample endpoint registries now default to least privilege.” README: samples default to rejected permissions with fs/terminal disabled. Spec samples in `doc/dev/cursor-adapter/spec.md` / `grok-adapter/spec.md` still show `reject` + disabled caps. |
| **Not a regression** | Defaults were tightened *toward* reject/least privilege during maintenance, not loosened then broken. Operator pain is surface documentation / registration ergonomics, not accidental default flip. |
| **wip/concurrent-refactor** | Branch tip predates 0.2.0 least-privilege sample pass; do not treat that branch as current sample policy. |

---

### 2. Daemon notification lag is connection-fatal (R-DAEMON-004)

| Field | Value |
|---|---|
| **Verdict** | **I** (maintained design). Transient **R** only in the sense that *pre-fix* code silently continued after lag (review-confirmed defect); fix restored fail-closed projection integrity. |
| **confidence** | high |
| **Code** | `crates/hub/src/daemon/rpc_io.rs` — `RecvError::Lagged(skipped)` → warn + `HubError::DaemonUnavailable("daemon notification stream lagged by {skipped} messages; reconnect and resynchronize")` → abort requests / close connection. |
| **History** | Review book **§12 / R-DAEMON-004** (Medium): lagged receiver “logged skipped events and then continued, exposing an undetectably incomplete projection” → resolution: connection-fatal + reconnect/resynchronize. Identified against baseline `12a7fc2d95cf785acb238e35cb9ccaf312eb2cab`. Closed in final review-findings series; reflog commit **`015733dc1ecd2e77d3d659ce978613aa9384a60e`** *fix: close final release review findings*. Task plan **T-611** (PR #29 second review pass): “connection-fatal daemon notification gaps”. CHANGELOG 0.2.0 Fixed: “Close daemon clients that fall behind the notification broadcast instead of silently continuing after an update gap.” Design/spec/bdd/`impl_plan` all codify the fatal boundary after the fix. |
| **PR** | [#29](https://github.com/RatmmmhSquishyRat/acp-hub/pull/29) merge `148e42d12e450f926785814612ccb456d64e5077` (v0.2.0, 2026-07-19). |
| **Operator note (attribution only)** | High-churn Cursor streaming can *stress* the intentional bound; drop is by design after R-DAEMON-004, not accidental UX regression relative to post-0.2.0 docs. |
| **wip/concurrent-refactor** | Tip `86de504e…` is pre–R-DAEMON-004; likely still on silent-skip semantics if that code path existed. |

---

### 3. `ensure_live_session` resume/load path after Live binding lost

| Field | Value |
|---|---|
| **Verdict** | **I** |
| **confidence** | high |
| **Code** | `crates/hub/src/hub/conversation.rs` `ensure_live_session`: if runtime `Live` **and** session still bound → Ok; if Live but unbound → drop runtime; prefer `session/resume` when capability present; on resume error fall through to `session/load` if `load_session`; wrap failures as `ResumeLoadFailed`. Call sites: `hub/prompt.rs` before prompt/config/mode. |
| **History** | Capability matrix in `doc/dev/design.md` §5 (resume/load required caps). Design: load failure leaves projection unchanged. F-002 / F-007: provisional projection + Layer-1 refresh integrity (same recovery family). No review finding treats resume-then-load as accidental. Pillar: capability-gated operations (`doc/roles/implementer.md` #5). |
| **Related errors** | Wrapping via `wrap_load_failure` / `ResumeLoadFailed` is deliberate so projection is not replaced with empty session on failure (`error.rs` doc comment). |

---

### 4. ResumeLoadFailed source folded to `daemon unavailable: resume/load operation failed` over RPC

| Field | Value |
|---|---|
| **Verdict** | **I** for safe typed RPC surface (fold opaque / private sources) + **C** for operator diagnosis (root cause becomes generic) |
| **confidence** | high |
| **Code** | `crates/hub/src/rpc/error_data.rs` — `SafeResumeSourceData`: maps many `HubError` variants to coarse tags; `DaemonUnavailable` and `Internal` both deserialize via `into_hub_error` → `HubError::DaemonUnavailable("resume/load operation failed".to_string())`. Nested Acp/Io/Other sources become `Internal` at the safe boundary. Wire path still has typed `ResumeLoadFailed` envelope when `TypedHubErrorData` is accepted (`RESUME_LOAD_FAILED_ERROR` in daemon). |
| **History** | Aligns with privacy redaction theme: F-016 (public DTO redaction), R-PRIV-001 (no absolute roots on ordinary reads), adapter privacy fixes in 0.2.0. Not documented as a “bug that folded too much by accident”; the safe enum is explicitly allowlisted. |
| **Clumsy surface** | Callers that only print `Display` of reconstructed client-side error see `daemon unavailable: resume/load operation failed` even when the server-side source was e.g. agent ACP error or missing capability — diagnostic loss is a **C** consequence of **I** privacy/schema hardening. |

---

### 5. Broadcast buffer sizes (1024 hub, 256 client) and hub/conv/update per session update

| Field | Value |
|---|---|
| **Verdict** | **I** (bounded fan-out + projection stream) + **C** under high notification volume (interacts with topic 2) |
| **confidence** | med (constants intentional/bounded; no separate review ID for the specific numbers 1024/256) |
| **Code** | Hub fan-out: `callbacks/connection.rs` `broadcast::channel(1024)`. Client-side notification bus: `rpc.rs` `broadcast::channel(256)`. Capture path: every bound update ends with `RpcRequest::notification("hub/conv/update", { agentId, sessionId, conversationId, runId, source, update })` in `callbacks/capture.rs`. Pending caps: `MAX_PENDING_NOTIFICATIONS = 1_024`, `MAX_PENDING_PER_SESSION = 256`, etc. |
| **History** | Resource bounding is a sustained 0.2.0 theme (R-RES-001, R-CAPTURE-001, frame budgets). R-DAEMON-004 treats lag past the hub broadcast capacity as fatal rather than growing unbounded queues. No evidence the 1024/256 pair was a regression from larger buffers; they look like original/maintenance-era hard caps. |
| **E2E note** | `doc/dev/cursor-adapter/e2e-investigation-2026-07-24.md` links Cursor micro-update volume to lag → connection drop under R-DAEMON-004. |

---

### 6. Agent register `mutate_registry` waiting on generation write locks / idle agents

| Field | Value |
|---|---|
| **Verdict** | **I** |
| **confidence** | high |
| **Code** | `hub/registry.rs` `mutate_registry`: global `registry_mutation` mutex → per-agent init locks (await) → `agent_generation_writer` (await **write** on command+callback gates) → `handles` lock → `lock_agents_idle` (`reject_active_agents` → **Conflict** if any operation for affected agent ids) → fingerprint check → atomic save/verify/swap (R-REG-001). Test `endpoint_removal_waits_for_old_load` asserts removal **waits** for in-flight load command before completing. |
| **History** | F-010: registry mutation validate/save/swap; protect referenced proxies and **active agents**; invalidate handles. R-REG-001: fail-closed commit consistency. Operation admission / generation gates are part of the 0.2.0 concurrency story (final-completion-summary cross-cutting bullets). |
| **Semantics nuance** | “Wait” applies to generation/init locks (serialize endpoint replacement with command generation). Active **operations** are **rejected** with `Conflict`, not spun until idle forever — still intentional fail-closed against mid-turn mutation. |

---

### 7. Daemon singleton + idle exit + Windows named pipes

| Field | Value |
|---|---|
| **Verdict** | **I** (architecture pillar) |
| **confidence** | high |
| **Code** | `daemon.rs`: file lock singleton; `DEFAULT_IDLE_TIMEOUT` 1800s (`ACP_HUB_IDLE_TIMEOUT` override); `ActivityTracker`; Windows endpoint `\\.\pipe\acp-hub-{daemon_id}` with owner-only SDDL; Unix socket + short-path fallback (macOS). R-DAEMON-001 recovery when prior owner exits mid-discovery (`9633c96a` *fix: recover daemon startup after singleton exit*). R-DAEMON-002 macOS socket mode. Idle tests: `crates/hub/tests/daemon_idle.rs`; flaky-wait fix `c15984a7` *fix(test): make daemon idle-exit waits robust on slow Windows CI* (v0.1.2 era). |
| **History** | Pillars (`doc/ssot/pillars/README.md` design #5): on-demand singleton, file discovery/lock, interprocess JSON-RPC, auto-exit when unused. Spec D5; design §3.6; BDD “Singleton, idle exit, and recovery”. F-017: home/registry/DB/daemon metadata/Unix socket/**Windows named-pipe** hardening. CHANGELOG 0.1.0: “On-demand singleton daemon…”. |
| **Not accidental** | Idle exit and pipe transport are founding product constraints, not post-hoc hacks. |

---

### 8. Capture Layer1 (agent original) vs Layer2 (hub projection) dual history

| Field | Value |
|---|---|
| **Verdict** | **I** (founding data model) |
| **confidence** | high |
| **Code** | Store schema `messages.source IN ('local_turn','load_replay','agent_list')`, `current_projection`; replay APIs in `store/replay.rs` (`begin_load_replay` / commit / rollback). Capture writes Layer2/`local_turn` (and load_replay on refresh) via `callbacks/capture.rs`. |
| **History** | Pillars FAQ: agent original vs hub capture are **parallel layers**, both displayed. Spec §3 Two-Layer Data Model; design §2; implementer principle #2; TDD Layer1 refresh cases; F-007 Layer-1 begin/commit/rollback preserving Layer 2. Present from initial product framing through 0.2.0 (F-007 closed as maintained resolution). |
| **Not V** | Layering is hub-owned projection semantics; vendors only supply Layer1 payloads when capabilities exist. |

---

### 9. Cursor adapter local-only ACP load then reject prompt

| Field | Value |
|---|---|
| **Verdict** | **I** (adapter safety) + **V** (upstream load/auth/session-space limitations drive the local fallback) |
| **confidence** | high |
| **Code** | `adapters/cursor/adapter.mjs`: `localOnlyAcpLoads` set when upstream `session/load` errors and local ACP replay is used; subsequent `session/prompt` for that sid returns `-32602` read-only error requiring re-auth/reload. IDE space always rejects prompt; CLI uses restricted resume. Header comment documents three spaces. |
| **History** | Commit **`ddb0d8d6e3920971a5c29cb485609b703400270c`** *feat(cursor-adapter): local read-only ACP session list + load*; released in **v0.1.2** (CHANGELOG 2026-07-09). Spec `doc/dev/cursor-adapter/spec.md`: fail-closed private storage; IDE reject; ACP prompt proxies upstream when live. F-020/F-021/F-025/R-ADAPTER-001: fail-closed adapters, privacy, no false success. Later maintenance: `ebd544e1` *fix: harden imported session read-only resumes*; `f990d05a` *fix: keep Cursor prompts out of process arguments*. |
| **V aspect** | Whether upstream load fails (auth, missing acp-sessions, vendor schema) is Cursor/vendor; hub only consumes adapter ACP errors. |

---

### 10. Evidence of intentional “fail closed” security philosophy vs accidental UX pain

| Field | Value |
|---|---|
| **Verdict** | **I** — fail-closed / least privilege is a repeated, reviewed design philosophy. Much operator “pain” is **C** (clumsy defaults/docs/error opacity) sitting on top of that **I**, not random regressions. |
| **confidence** | high |

**Documented intentional fail-closed / least-privilege evidence**

| Artifact | Statement |
|---|---|
| Review book F-028 | Samples deny-by-default; full-access only with trusted-environment warning |
| Review book R-DAEMON-004 | Prefer connection death over silent projection gap |
| Review book R-REG-001 / R-RUN-001 / R-ADAPTER-001 | Ambiguous registry, cancel rollback, adapter parse → fail closed |
| Review book F-025 | Hub core ACP-only; private store only in fail-closed vendor adapters |
| CHANGELOG 0.2.0 | Least-privilege samples; lag closes client; adapter privacy defaults |
| `SECURITY.md` | Local trust model; agents run with hub privileges; treat registry/DB sensitive |
| README | Samples reject permissions; only enable capabilities a trusted workflow needs |
| Spec §6 / adapter specs | Schema mismatch → explicit error, not empty success |
| final-completion-summary | Multiple “fail closed” closures (adapters, cancel rollback, registry) |

**Where pain is attribution-classified as C (not accidental product bug)**

- Default `reject` + disabled fs/terminal without a one-shot “trusted local yolo” profile  
- Lag-fatal streams under chatty agents without operator-facing backpressure knobs  
- Resume errors folded to generic daemon-unavailable strings on some client paths  
- Registry mutation blocked/serialized while agents are live  

**Where pain is V**

- Cursor IDE resume unsafe → adapter rejects  
- Cursor/Grok private schema / auth → local-only or fail-closed load  

**Where history shows R (defect then intentional fix)**

- Pre-R-DAEMON-004 silent lag continue  
- Pre-R-DAEMON-001 stale singleton poll  
- Various F-* original incomplete security/isolation gaps fixed toward fail-closed, not away from it  

---

## `wip/concurrent-refactor` note

| Field | Value |
|---|---|
| **Tip** | `origin/wip/concurrent-refactor` → `86de504ed88c5b24b5a3f6616a6c9d20f34c5eba` |
| **Reflog path** | Branched from early main after `cbdf3af…` (LICENSE/CI); commits: `51b557b5` WIP concurrent local refactor (**do not merge**), `c5338c0c` cursor adapter, `86de504e` grok adapter |
| **Relative to main** | Diverged before crates.io publish series, v0.1.1–0.1.2 mainline polish, and especially before **0.2.0 / PR #29** maintenance (R-DAEMON-004, least-privilege samples F-028, SafeResumeSourceData hardening, registry generation-lock discipline as currently reviewed, etc.) |
| **Attribution impact** | Do **not** use this branch as evidence of *current* intentional behavior. For disputed topics 2, 4, 6, 10 the authoritative intent is **main @ v0.2.0+** review book + SSOT. Concurrent-refactor may retain pre-fix lag/skip or older registry/adapter edges. |

---

## Matrix: tag combinations used

| Topic | Tags | One-line reason |
|---|---|---|
| 1 Permissions defaults | **I+C** | Least privilege is intentional; friction is operator surface |
| 2 Lag fatal | **I** (post-fix); historical **R** closed | Review-driven fail-closed projection integrity |
| 3 ensure_live_session | **I** | Capability-gated resume/load recovery |
| 4 Error folding | **I+C** | Safe RPC/privacy intentional; diagnostics clumsy |
| 5 Broadcast sizes / update fanout | **I+C** | Bounded by design; small vs Cursor churn is surface stress |
| 6 mutate_registry locks | **I** | Atomic endpoint replacement + generation safety |
| 7 Singleton/idle/pipes | **I** | Pillar architecture |
| 8 Dual history layers | **I** | Pillar data model |
| 9 Cursor local-only then reject | **I+V** | Adapter safety on vendor failure modes |
| 10 Fail-closed philosophy | **I** (pain often **C**) | Consistent across review ledger and samples |

---

## Methodology limits

This pass used **git reflog + fixed SHAs in review/maintenance docs + CHANGELOG + current code**, not a full interactive `git blame` / `git log -S` walk of every constant. Where intro SHAs are approximate, confidence is marked **med** and the **maintained intent** is still anchored by dated review IDs (F-*/R-*) and v0.2.0 publication commit `148e42d1…`.

---

## File index (absolute)

| Path |
|---|
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\crates\hub\src\endpoint.rs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\crates\hub\src\daemon\rpc_io.rs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\crates\hub\src\rpc\error_data.rs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\crates\hub\src\hub\conversation.rs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\crates\hub\src\hub\registry.rs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\crates\hub\src\callbacks\capture.rs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\crates\hub\src\callbacks\connection.rs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\crates\hub\src\rpc.rs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\crates\hub\src\daemon.rs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\adapters\cursor\adapter.mjs` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\doc\review\complete-review-book-2026-07-18.md` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\doc\maintenance\complete-task-plan-2026-07-18.md` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\doc\maintenance\final-completion-summary-2026-07-19.md` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\CHANGELOG.md` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\doc\ssot\pillars\README.md` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\doc\dev\spec.md` |
| `C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\doc\dev\design.md` |
