# Agent-managed loop — UX-first hub CLI

**Authority:** frozen `doc/ssot/pillars/*` (read-only) + this tree  
**User direction (2026-07-24):** 完整可用、流畅手感、可平替内嵌 ACP / subagent
操作模型；安全不得挡主路径；不改冻结 pillar 正文。

## Goals

| ID | Goal | Done when |
|----|------|-----------|
| G1 | New registration usable by default | CLI / MCP / JSON omit → auto-allow + fs r/w + terminal |
| G2 | Explicit tight registration works | reject / sandbox preserve disabled caps |
| G3 | Lag does not fail turns | Lagged continues; tests assert non-fatal |
| G4 | Resume/load errors honest | Distinct classes; no bare “daemon unavailable: resume/load operation failed” for endpoint failures |
| G5 | Docs match code | Operator-facing + active design docs not teaching reject-default / lag-fatal as current law |
| G6 | Zero-trust proof | In-repo tests pass; evidence captured under goal scratch |

## Non-goals (out of this overlay’s scope — not “completion labels”)

- Editing frozen pillars without permission  
- Porting OMP task runtime into CoreHub  
- Auto-migrating existing on-disk reject registries  
- Rewriting historical review-book text (supersession notes only)

## Protocol

1. Prefer coherent defaults + lag + errors in one parity package.  
2. Never touch `doc/ssot/pillars/*`.  
3. Prove with real crate tests, not prior-session narrative.  
4. Do not invent completion documents that mark unfinished product work as
   officially deferred deliverables.

## Status

- Overlay + code rework under zero-trust re-proof (2026-07-24).  
