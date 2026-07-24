# Phase 1 ship note (Operator UX)

**Date:** 2026-07-24  
**Contract:** [OPERATOR-UX-PHASE1-CONTRACT.md](./OPERATOR-UX-PHASE1-CONTRACT.md) v1.2  
**Plan:** [IMPLEMENTATION-PLAN.md](./IMPLEMENTATION-PLAN.md)

## Shipped

| Area | Implementation |
|------|----------------|
| Pure policy | `store/conversation_policy.rs` — origin/interaction/phase/busy/last_outcome, synthetic STATUS, meta parse, recompute |
| Schema | migration **7** — hybrid columns + status CHECK includes `closed`; deterministic origin/interaction backfill |
| Discover | `list_agent_sessions` metadata upsert only; Hub DTO with IX/SPACE/IN_HUB/CONV; **no session/load** |
| Option A | `imported_list` always RO; send/param/mode `assert_write_gate` |
| Bind | promote + keep row on load fail; IDE stays RO |
| List | default workbench; `--all` / filters; JSON envelope |
| Lifecycle | soft-delete; close-while-busy → last_outcome=failed; recover clears busy |
| Tests | `phase1_operator_ux.rs` SC oracles + full workspace green |

## Explicit non-claims

- **Not** M1–M6 product complete  
- **Not** Phase 2 transcript merge / search IX  
- **Not** Phase 3 progress / inspect probe  
- IDE writable resume still impossible (honest RO only)

## Review-rework

1. Discover-load tests updated to metadata-only law  
2. Soft-delete vs hard-delete for fixture id reuse  
3. Reject policy is **not** a send hard-gate (only permission callbacks)  
4. Clippy type-complexity factoring for discover rows  
