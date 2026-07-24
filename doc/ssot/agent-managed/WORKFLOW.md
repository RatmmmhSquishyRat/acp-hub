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

### Open: Operator journey / large UX（**未闭合**）

权威问题登记与强制设计前置：[OPERATOR-UX-CHARTER.md](./OPERATOR-UX-CHARTER.md)

| ID | Goal | Done when |
|----|------|-----------|
| G7 | UX system design complete | [OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md) 评估+F-*+动线+分期；review-rework 共识 |
| G8 | Session workbench semantics | Phase1–2：interaction、discover≠workbench、list 可发现、transcript 可读 |
| G9 | Readable operator surface | Phase3：inspect probe、进度、错误→下一步 |
| G10 | Scenario regression | Phase4：SC-* 正向流程回归 + 文档按 G.0 |

**产品判定（用户）：** 功能不齐全 + 语义重叠时 **不能当作可完整使用**；根因是 **从未完整设计使用者动线与 UX 功能体系**。G1–G6 底座 **不** 关闭 G7–G10。

## Non-goals (out of this overlay’s scope — not “completion labels”)

- Editing frozen pillars without permission  
- Porting OMP task runtime into CoreHub  
- Auto-migrating existing on-disk reject registries  
- Rewriting historical review-book text (supersession notes only)  
- Treating idle session accumulation as the primary UX problem  
- Implementing large UX (G7–G10) without charter design + review

## Protocol

1. Prefer coherent defaults + lag + errors in one parity package.  
2. Never touch `doc/ssot/pillars/*`.  
3. Prove with real crate tests, not prior-session narrative.  
4. Do not invent completion documents that mark unfinished product work as
   officially deferred deliverables.  
5. **Large UX:** design journeys first ([OPERATOR-UX-CHARTER.md](./OPERATOR-UX-CHARTER.md)); then implement; independent review loop required.

## Status

- G1–G6: closed on main (defaults, Store-first, compliance).  
- G7–G10: **open** — charter recorded; journey design not yet written/reviewed.  
