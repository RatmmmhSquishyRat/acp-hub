# Agent-managed loop — UX-first hub CLI

**Authority:** frozen `doc/ssot/pillars/*` (read-only) + this tree  
**User direction (2026-07-24):** 完整可用、流畅手感、可平替内嵌 ACP / subagent
操作模型；安全不得挡主路径；不改冻结 pillar 正文。

## Goals

### Closed overlay (defaults / Store-first / compliance) — 2026-07-24

| ID | Goal | Done when |
|----|------|-----------|
| G1 | New registration usable by default | CLI / MCP / JSON omit → auto-allow + fs r/w + terminal |
| G2 | Explicit tight registration works | reject / sandbox preserve disabled caps |
| G3 | Lag does not fail turns | Lagged continues; tests assert non-fatal |
| G3b | Store-first conversation ownership | Capture Store-before-broadcast; lag ≠ incomplete Store; no agent-refresh resync narrative |
| G4 | Resume/load errors honest | Distinct classes; no bare “daemon unavailable: resume/load operation failed” for endpoint failures |
| G5 | Docs match code | Operator-facing + active design docs not teaching reject-default / lag-fatal / resync-as-projection-repair as current law |
| G6 | Zero-trust proof | In-repo tests pass; evidence captured under goal scratch |

### Closed: Operator journey / large UX (G7–G10) — 2026-07-25（历史）

**产品表面 SSOT（现行）：** [UX-CORE.md](./UX-CORE.md) — **send / wait / show / cancel**。  
OPERATOR-UX-CHARTER / SYSTEM / PHASE1–4 / SHIP = **历史实现笔记**，不再扩展为操作者心智模型。

| ID | Goal | Done when |
|----|------|-----------|
| G7–G10 | Operator UX Phases 1–4 shipped | historical — see OPERATOR-UX-SHIP.md |
| **G12** | UX-CORE product surface | send `--no-wait` + `wait` + show filters + doctor four-primitive |

**产品判定：** 日常心智模型 = UX-CORE 四原语，不是 journey 百科。

## Non-goals (out of this overlay’s scope — not “completion labels”)

- Editing frozen pillars without permission  
- Porting OMP task runtime into CoreHub  
- Auto-migrating existing on-disk reject registries  
- Rewriting historical review-book text (supersession notes only)  
- Treating idle session accumulation as the primary UX problem  
- Phase-5 optional (pin/archive)  

## Protocol

1. Prefer coherent defaults + lag + errors in one parity package.  
2. Never touch `doc/ssot/pillars/*`.  
3. Prove with real crate tests, not prior-session narrative.  
4. Do not invent completion documents that mark unfinished product work as
   officially deferred deliverables.  
5. **Product surface:** implement against [UX-CORE.md](./UX-CORE.md) + [HUMAN-READING.md](./HUMAN-READING.md); do not re-expand OPERATOR journey encyclopedias.

## Status

- **G1–G6:** closed on main (defaults, Store-first, compliance).  
- **G7–G10:** closed historically (Operator UX Phases 1–4).  
- **G11:** HUMAN-READING v2 natural CLI closed on main.  
- **G12:** UX-CORE four-primitive surface (this ship).
