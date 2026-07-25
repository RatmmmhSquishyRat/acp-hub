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

### Closed: Operator journey / large UX (G7–G10) — 2026-07-25

权威：[OPERATOR-UX-CHARTER.md](./OPERATOR-UX-CHARTER.md) · [OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md) · contracts PHASE1–4 · [OPERATOR-UX-SHIP.md](./OPERATOR-UX-SHIP.md)

| ID | Goal | Done when |
|----|------|-----------|
| G7 | UX system design complete | SYSTEM + PHASE1–4 contracts + refine/ship notes |
| G8 | Session workbench semantics | interaction、discover≠workbench、list 可发现、transcript 可读（Phase1–2 code） |
| G9 | Readable operator surface | inspect probe、progress/timings、错误→下一步（Phase3） |
| G10 | Scenario regression | SC pack + doctor G.0 + M1–M8 PASS（Phase4） |

**产品判定（用户）：** 功能不齐全 + 语义重叠时 **不能当作可完整使用**。G1–G6 底座 **与** G7–G10 均已在 main 闭合；M1–M8 评估见 OPERATOR-UX-SHIP.md。

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
5. **Large UX:** design journeys first; implement against phase contracts; independent review loop required.

## Status

- **G1–G6:** closed on main (defaults, Store-first, compliance).  
- **G7–G10:** **closed on main** (full Operator UX ship Phases 1–4).  
- **M1–M8:** **met** — see [OPERATOR-UX-SHIP.md](./OPERATOR-UX-SHIP.md).  
- Residual honesty only: live Cursor daemon-kill E2E environment-limited; Layer1 auto-load on show remains `layer1Refreshed=false` until optional load path.
