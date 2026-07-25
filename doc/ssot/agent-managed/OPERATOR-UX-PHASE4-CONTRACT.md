# Operator UX — Phase 4 Contract (journey / doctor / MIG / ship bar)

**Status:** APPROVED for implementation  
**Date:** 2026-07-24  
**Authority:** SYSTEM §G.0 / §G.8 / §L M1–M8 / F-DOC / F-MIG

## 1. Help journey (F-DOC)

- Root CLI about / `acp-hub doctor` (or `acp-hub help-journey`) prints **G.0 order** in plain text:
  1. install / PATH
  2. `agent add`
  3. `agent inspect --probe`
  4. `conv create`
  5. `send`
  6. `conv show` / `conv list` / `search`
  7. discover: `agent sessions` (read-only museum) vs bind / new writable
- No new wild F-* commands beyond `doctor`.

## 2. Doctor (F-DOC / F-MIG)

`acp-hub doctor [--json]`:

Scans hub home registry + store (no force rewrite):

| Check | Severity | Next step text |
|-------|----------|----------------|
| agents.json missing/empty | warn | agent add |
| any agent permission_policy=reject | warn | fixed substring `permission_policy=reject; re-add agent with defaults or edit agents.json` |
| agent cache empty | info | inspect --probe |
| ok | info | journey pointer |

**Forbidden:** silently rewriting agents.json reject → auto-allow.

## 3. Ship / M* evaluation

Agent-managed `OPERATOR-UX-SHIP.md` must mark M1–M8 with PASS/FAIL evidence pointers (test names or log paths). Full ship requires PASS on M1–M8 for contracted surfaces (live Cursor E2E optional if fixtures cover).

## 4. SC pack

Regression tests remain in-repo: Phase1 gates + Phase2 merge + Phase3 probe/progress + doctor reject substring.

## 5. Non-goals

Phase-5 pin/archive; crates.io; auto-migrate.
