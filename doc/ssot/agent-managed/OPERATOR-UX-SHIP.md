# Operator UX — Ship / M* evaluation

**Date:** 2026-07-24  
**Status:** Phases 1–4 shipped against contracts; M1–M8 evaluated below.  
**Authority:** SYSTEM §L · PHASE1–4 contracts · IMPLEMENTATION-PLAN

## Why prior “Phase-1 only” was wrong

The product goal was **full operator UX design ship**, not a contract-exit note. Phase contracts are **implementation gates**, not permission to stop. Stopping at Phase 1 left inspect empty, transcript unreadable, no progress, and no journey surface — the exact QoL failures reported in QA.

## Contracts frozen

| Phase | Contract | Runtime |
|-------|----------|---------|
| 1 | OPERATOR-UX-PHASE1-CONTRACT | workbench, discover RO, gates, errors |
| 2 | OPERATOR-UX-PHASE2-CONTRACT | transcript merge, preview, search IX |
| 3 | OPERATOR-UX-PHASE3-CONTRACT | inspect probe honesty, progress/timings |
| 4 | OPERATOR-UX-PHASE4-CONTRACT | doctor + G.0 journey |

## M1–M8

| ID | Standard | Result | Evidence |
|----|----------|--------|----------|
| **M1** | Cold-start runbook without guessing (incl. probe) | **PASS** | `doctor` prints G.0; `inspect` `probeStatus=skipped` + message; create/send progress stages |
| **M2** | List row shows W/R | **PASS** | Phase1 list IX column; search IX; sessions IX |
| **M3** | SC-13 thought merge | **PASS** | `operator_ux_full::sc13_show_conversation_merges_thoughts_on_shipped_path` |
| **M4** | create/send stage+timings | **PASS** | CLI ProgressTracker on create/send; `progress::tests::*` |
| **M5** | workbench + search recoverability | **PASS** | Phase1 flood tests; search hits with interaction/origin |
| **M6** | RO/IDE mis-send stable code + next wording | **PASS** | Phase1 write-gate + IDE wire tests |
| **M7** | CLI/MCP isomorphic surfaces | **PASS** | MCP `inspect_agent` probe; `show_conversation`; list workbench params |
| **M8** | SC regression pack | **PASS** | `phase1_operator_ux` + `operator_ux_full` + CLI contract/smoke |

## Residual honesty

- Live multi-agent Cursor daemon-kill mid-prompt: fixture-level recovery exists; full kill E2E remains environment-limited (not synthetic theater).
- Layer1 auto-refresh on show: `layer1Refreshed` is honest `false` until a future optional load path; no silent session/new.
- Daemon-internal `agent_spawn`/`initialize` timings are omitted when CLI cannot observe them (contract allows omit keys).

## Commands (operator)

```
acp-hub doctor
acp-hub agent inspect <id> --probe
acp-hub conv create <id> --cwd <abs>
acp-hub send <conv> --text "…"
acp-hub conv show <conv>           # merged transcript
acp-hub conv show <conv> --raw
acp-hub conv list                  # workbench
acp-hub conv list --all
acp-hub search "…"
acp-hub agent sessions <id>        # museum RO metadata
```
