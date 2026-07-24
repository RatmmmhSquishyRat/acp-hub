# Research workflow — OMP subagents/task agents vs ACP Hub

> **SUPERSEDED for product law (2026-07-24):** least-privilege-by-default and
> lag-as-connection-fatal are **not** current product acceptance. Current
> direction lives in `doc/ssot/agent-managed/` (Product-UX, WORKFLOW, RESIDUALS).
> This research pack remains a historical investigation only.

**Date:** 2026-07-24  
**Mode:** evidence-first research only (no product redesign, no large implementation swings)  
**Trigger:** Cursor E2E reliability report + operator dissatisfaction with hub params/permissions/connection/resume

## Goals

1. Fully understand **Oh My Pi (OMP)** designs the operator trusts most:
   - subagents / task agents
   - parameters & config
   - permissions / approval
   - connection / process lifecycle
   - resume / session continuity
2. Fully understand **acp-hub** as implemented and as designed (SSOT + code).
3. Attribute each “unreasonable” hub behavior to one of:
   - **I** — intentional design (documented principle / reviewed decision)
   - **R** — regression during review/refactor (history shows earlier better behavior)
   - **O** — original bug / incomplete implementation
   - **C** — correct for hub role, but **clumsy operator surface** (docs/defaults/UX)
   - **V** — vendor-dependent (Cursor/OMP/etc.), hub only surfaces it
4. Produce a comparison research pack that can guide *small, stable* follow-ups — not a redesign mandate.

## Non-goals

- Rewriting hub architecture or replacing daemon/RPC model.
- Treating OMP as something hub should become (hub is multi-agent conductor; OMP is a full agent product).
- Implementing fixes in this research pass.

## Method

### Parallel investigation lanes

| Lane | Owner style | Sources |
|------|-------------|---------|
| A — OMP task/subagent design | explore/general | `repos/ref_repos/oh-my-pi` docs + packages |
| B — OMP ACP endpoint surface | explore/general | OMP ACP docs/code; hub `adapters/omp` |
| C — acp-hub architecture map | explore | `doc/ssot`, `doc/dev`, `crates/hub` |
| D — history attribution | general/shell | git log/blame/PR notes for disputed behaviors |
| E — synthesis | orchestrator | matrix + verdicts |

### Evidence rules

- Prefer **code + dated docs + git history** over memory.
- Every claim tagged: `code:path:line` or `doc:path` or `git:sha`.
- Distinguish **hub principle** (multi-endpoint conductor, least privilege) from **OMP principle** (single product, rich internal task model).
- If intentional, cite SSOT/design/review book; if regression, cite commit that changed behavior.

### Deliverables (this folder)

| File | Content |
|------|---------|
| `00-WORKFLOW.md` | This workflow |
| `01-omp-subagents-task-agents.md` | OMP design reference |
| `02-acp-hub-architecture.md` | Hub map (as-is) |
| `03-history-attribution.md` | I/R/O/C/V for disputed points |
| `04-comparison-matrix.md` | Side-by-side params/perm/conn/resume |
| `05-findings-summary.md` | Operator-facing verdicts + safe next steps |
| `raw/` | Optional extracts from subagents |

## Disputed surfaces (from E2E + operator notes)

1. Permission defaults (`reject` vs auto-allow / OMP yolo + client gate)
2. Config/param model (per-conv set vs rich agent defaults)
3. Connection lifecycle (daemon singleton, idle exit, Windows pipes)
4. Resume/load after process death
5. Notification lag = connection-fatal
6. RPC error folding (`resume/load operation failed`)
7. Registry mutate hang under live agents
8. Agent process tree kill / recovery
9. Cursor streaming volume vs hub capture fanout

## Status

- [x] Scaffold
- [x] Lane A — OMP task/subagent (`01-omp-subagents-task-agents.md`)
- [x] Lane B — OMP ACP (`raw/omp-acp-surface.md`)
- [x] Lane C — Hub architecture (`02-acp-hub-architecture.md`)
- [x] Lane D — History attribution (`03-history-attribution.md`)
- [x] Lane E — OpenAB extract (`raw/openab-session-pool-notes.md`)
- [x] Synthesis — matrix + summary (`04`, `05`)
- [x] Signed off for research-only handoff (2026-07-24)
