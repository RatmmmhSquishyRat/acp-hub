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
   for registration defaults, lag handling, and error honesty.

## Contents

| File | Summary |
|------|---------|
| [WORKFLOW.md](./WORKFLOW.md) | Loop goals and protocol |
| [PLAN.md](./PLAN.md) | Implementation checklist for this overlay |
| [CONVERGENCE.md](./CONVERGENCE.md) | Acceptance map criterion → code → test |
| [pillars/Product-UX.md](./pillars/Product-UX.md) | UX-first priority and defaults |
| [INDEX.md](./INDEX.md) | Index |

## Change log

| Date | Note |
|------|------|
| 2026-07-24 | Created after incorrect edit to frozen pillars (restored). |
| 2026-07-24 | Removed agent-invented `RESIDUALS.md` completion packaging; control plane re-written for zero-trust rework. |
