# Agent-managed SSOT extensions

**Not frozen.** Owned by the implementing agent for product-direction overlays
that must **not** rewrite frozen pillars under `doc/ssot/pillars/`.

| Path | Role |
|------|------|
| `doc/ssot/pillars/` | **Frozen** user SSOT — do not edit without explicit user permission |
| `doc/ssot/agent-managed/` | Agent-authored UX / defaults / lifecycle product law overlay |

## Precedence

1. Frozen pillars define baseline “what hub is.”
2. This tree states operator **defaults and main-path UX** when the user has
   directed UX-first correction (2026-07-24).
3. Never silently overwrite frozen pillar files.
4. Implementation and active `doc/dev/*` operator law must match this overlay
   for registration defaults, Store-first conversation ownership, lag handling
   (live fan-out only), and error honesty.

## Contents

| File | Summary |
|------|---------|
| [WORKFLOW.md](./WORKFLOW.md) | Loop goals and protocol |
| [PLAN.md](./PLAN.md) | Implementation checklist for this overlay |
| [CONVERGENCE.md](./CONVERGENCE.md) | Acceptance map criterion → code → test |
| [pillars/Product-UX.md](./pillars/Product-UX.md) | UX-first priority and defaults |
| [OPERATOR-UX-CHARTER.md](./OPERATOR-UX-CHARTER.md) | **大型 UX 问题登记 + 正向动线设计强制前置**（非实现清单） |
| [OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md) | **从零 UX/QoL/Journey 系统评估 + 功能规范 F-* + 动线 + 分期**（结束功能混乱） |
| [OPERATOR-UX-PHASE1-CONTRACT.md](./OPERATOR-UX-PHASE1-CONTRACT.md) | **Phase1 可实现 wire/store 合同**（schema/meta/discover/bind/list/errors/SC） |
| [COMPLIANCE.md](./COMPLIANCE.md) | Frozen + Product-UX compliance matrix with code evidence |
| [INDEX.md](./INDEX.md) | Index |

## Change log

| Date | Note |
|------|------|
| 2026-07-24 | Created after incorrect edit to frozen pillars (restored). |
| 2026-07-24 | Removed agent-invented `RESIDUALS.md` completion packaging; control plane re-written for zero-trust rework. |
| 2026-07-24 | Product-UX §5 Store-first: Hub owns durable dual-layer conversation; lag ≠ incomplete Store / agent refresh. |
| 2026-07-24 | Product-UX §6: read-only explicit + session discoverability for operator agents. |
| 2026-07-24 | **OPERATOR-UX-CHARTER:** root cause = missing journey design; large UX register; design-before-implement mandate. |
| 2026-07-24 | **OPERATOR-UX-SYSTEM v0.2:** full UX eval + F-* catalog + closed R1–R8 policies + journeys + phases. |
