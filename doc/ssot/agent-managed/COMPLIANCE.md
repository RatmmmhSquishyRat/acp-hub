# Compliance — frozen pillars + Product-UX Store-first

**Date:** 2026-07-24  
**Scope:** current `repos/acp-hub` implementation vs  
- frozen `doc/ssot/pillars/README.md` + `TechSel.md` (read-only)  
- agent-managed `pillars/Product-UX.md` (UX defaults + Store-first ownership)

This is an **evidence-backed compliance map**, not a residual backlog pack.

---

## 1. Frozen pillars (`doc/ssot/pillars/README.md`)

| Law | Implementation evidence | Verdict |
|-----|-------------------------|---------|
| Generic ACP client/conductor: register, conv manage, send/recv, search | CLI/MCP + `CoreHub` RPC (`hub/agent/*`, `hub/conv/*`, `hub/proxy/*`) | **PASS** |
| Dual-layer history: agent-original ∥ hub-capture, both shown when available | `MessageSource::{LoadReplay,LocalTurn}`; capture tags; CLI `[agent-original]` / `[hub-capture]` (`cli/src/output.rs`) | **PASS** |
| Capability negotiation; ops depend on agent caps | `validate_prompt_capabilities`, `UnsupportedCapability`, list/load/resume gates | **PASS** |
| On-demand singleton daemon | `daemon` lock + `connect_or_spawn` | **PASS** |
| Successful agent APIs → full static snapshots | `replace_static_snapshots`, plan/config/mode/usage capture | **PASS** |
| Layer1 load failure leaves projection untouched | `begin/commit/rollback_load_replay`; Layer2 preserved on Layer1 supersede | **PASS** |
| TechSel: Rust full implementation | workspace crates | **PASS** (process) |

Frozen pillar files remain **unmodified** by this goal.

---

## 2. Product-UX overlay

| Law | Evidence | Verdict |
|-----|----------|---------|
| Defaults: auto-allow + fs R/W + terminal | `endpoint.rs` Default; CLI args; MCP register omit; sample `agents.json` | **PASS** |
| Explicit reject / `--sandbox` | CLI + MCP tests | **PASS** |
| Lag non-fatal (no kill connection / in-flight RPC) | `daemon/rpc_io.rs` `Lagged` → warn + continue; lifecycle test | **PASS** |
| **Store-first durable ownership** | capture: Store write then `hub/conv/update`; CLI send reads Store pages after prompt RPC | **PASS** |
| Live lag ≠ incomplete Store; no force-agent-refresh | log text live-only; no resync runtime path in `crates/**` | **PASS** |
| Normal / error / incomplete terminalized in Store | `finalize_run_cas` Completed/Cancelled/Failed; capture failure merged into prompt Err; `recover_interrupted_runs` | **PASS** |
| Resume errors distinguishable | RPC `SafeResumeSourceData` + tests; MCP structured `source` tags | **PASS** |
| Docs not teaching reject-default / lag-fatal / resync-as-Store-repair as current law | Product-UX §5, design/spec/impl_plan/bdd/CHANGELOG | **PASS** (historical review/research remain labeled or superseded) |

---

## 3. Capture / turn ownership (Store-first detail)

```
session/update
  → budget check + charge
  → Store mutation (? / restore budget on failure)
  → hub/conv/update (best-effort live)
```

| Outcome | Store | Run | Live fan-out |
|---------|-------|-----|--------------|
| Normal EndTurn | messages + snapshots | Completed + stop_reason | may lag |
| Capture fail mid-turn | partial messages kept; failed update not written | Failed (merge_capture_failure) | no event for failed write |
| Store write fail | nothing for that update; budget restored | prompt fails if during turn | no event |
| Agent/command error | user prompt + any captured chunks | Failed | n/a |
| Cancel | captured so far | Cancelled | n/a |
| Daemon crash | recover runs → failed / daemon_restarted | Failed | n/a |
| Client lag only | **unchanged** | **unchanged** | dropped frames |

**Not** a product law: force external agent to re-emit history because live lagged.

Hub-initiated `session/load|resume` on `ensure_live_session` is **Layer1 continuity / binding**, allowed by Product-UX §5.4 — not lag repair.

---

## 4. Correctness fixes landed in this review pass

| Item | Change |
|------|--------|
| Capture budget leak after failed Store write | charge then restore on Err (`callbacks/capture.rs`); test `store_write_failure_restores_capture_budget_charge` |
| MCP ResumeLoadFailed opacity | structured `reason` + `source` tags (`cli/src/mcp.rs`); test `resume_load_failed_maps_to_structured_mcp_source` |
| Narrative pollution | Product-UX §5 + design/spec/impl_plan/bdd/rpc_io Store-first language |

### Known non-blocking nuances

| Item | Notes |
|------|-------|
| Historical review/research docs | May still describe R-DAEMON-004 lag-fatal; not current law |

### Fixed after compliance note

| Item | Notes |
|------|-------|
| Conv status after terminal run | `finalize_run_cas` now mirrors run outcome (`failed`/`cancelled`/`completed`); next send is gated by active runs, not Idle |

---

## 5. Verification evidence (goal scratch)

Path: `C:\Users\15480\AppData\Local\Temp\grok-goal-77d8ef75d9a7\implementer\`

| Artifact | Content |
|----------|---------|
| `full-cargo-test.log` | workspace tests (re-run after fixes) |
| `fmt-check.log` / `clippy.log` | style gates |
| `store-first-test.log` / `lag-test.log` | Store-first + lag |
| `pillars-status.txt` | frozen pillars clean |
| `doc-pollution-scan*.txt` | active-law scan |

Must-pass filters include:  
`store_persists_capture_*`, `store_write_failure_restores_*`, `lagged_notification_stream_continues_*`, `endpoint_defaults_*`, `resume_load_*`, `resume_load_failed_maps_*`, `agent_registration_defaults_*`.
