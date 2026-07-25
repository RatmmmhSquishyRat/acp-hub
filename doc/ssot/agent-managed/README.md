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
| **[UX-CORE.md](./UX-CORE.md)** | **产品表面 SSOT** — send / wait / show / cancel；CLI/MCP；验收（**read first**） |
| [WORKFLOW.md](./WORKFLOW.md) | Loop goals and protocol |
| [PLAN.md](./PLAN.md) | Implementation checklist for this overlay |
| [CONVERGENCE.md](./CONVERGENCE.md) | Acceptance map criterion → code → test |
| [pillars/Product-UX.md](./pillars/Product-UX.md) | UX-first architecture law and defaults（Store-first / auto-allow / RO） |
| [OPERATOR-UX-CHARTER.md](./OPERATOR-UX-CHARTER.md) | **Historical** — 大型 UX 问题登记；产品表面 → UX-CORE |
| [OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md) | **Historical** — F-* / journey 系统评估；产品表面 → UX-CORE |
| [OPERATOR-UX-PHASE1-CONTRACT.md](./OPERATOR-UX-PHASE1-CONTRACT.md) | **Historical wire** — Phase1 schema/meta/discover/bind/list/errors（冲突以 UX-CORE 为准） |
| [HUMAN-READING.md](./HUMAN-READING.md) | **人类超快扫读法**（高于 agent-only UX，非取舍） |
| [HUMAN-READING-DESIGN.md](./HUMAN-READING-DESIGN.md) | 全量交互设计（as-is / to-be / 命令契约） |
| [HUMAN-READING-CONTRACT.md](./HUMAN-READING-CONTRACT.md) | **可实现合同**（REVIEW APPROVED 后才可写代码） |
| [HUMAN-READING-REVIEW.md](./HUMAN-READING-REVIEW.md) | 对抗审核结论 |
| [COMPLIANCE.md](./COMPLIANCE.md) | Frozen + Product-UX compliance matrix with code evidence |
| [INDEX.md](./INDEX.md) | Index |

## Product surface precedence (2026-07-25)

1. [UX-CORE.md](./UX-CORE.md) — what operators remember and how CLI/MCP behave  
2. [HUMAN-READING.md](./HUMAN-READING.md) — default presentation  
3. [pillars/Product-UX.md](./pillars/Product-UX.md) — architecture defaults  
4. OPERATOR-UX-\* / PHASE\* — historical implementation notes only

## Change log

| Date | Note |
|------|------|
| 2026-07-25 | **UX-CORE** product surface SSOT; OPERATOR-UX-\* superseded for operator narrative. |
| 2026-07-24 | Created after incorrect edit to frozen pillars (restored). |
| 2026-07-24 | Removed agent-invented `RESIDUALS.md` completion packaging; control plane re-written for zero-trust rework. |
| 2026-07-24 | Product-UX §5 Store-first: Hub owns durable dual-layer conversation; lag ≠ incomplete Store / agent refresh. |
| 2026-07-24 | Product-UX §6: read-only explicit + session discoverability for operator agents. |
| 2026-07-24 | **OPERATOR-UX-CHARTER:** root cause = missing journey design; large UX register; design-before-implement mandate. |
| 2026-07-24 | **OPERATOR-UX-SYSTEM v0.2:** full UX eval + F-* catalog + closed R1–R8 policies + journeys + phases. |
