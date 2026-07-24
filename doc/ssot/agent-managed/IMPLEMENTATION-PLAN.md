# Operator UX — Implementation Plan (decoupled)

**Status:** Agent-managed engineering plan  
**Date:** 2026-07-24  
**Authority:** [OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md) → Phase contracts → this plan  
**Gate:** No Phase N runtime code without that phase’s frozen contract.  
**Honest bar:** Phase 1 ships workbench/discover/bind/gates only. **Do not claim M1–M6.**

---

## 1. Workflow (who owns what)

| Layer | Owns | Does not own |
|-------|------|--------------|
| **Policy pure** (`store/conversation_policy`) | origin/interaction/phase/busy/last_outcome enums; synthetic STATUS; meta→space; recompute(interaction); legacy backfill maps | I/O, SQL, ACP |
| **Store** | schema migration, persistence, list filters/envelope query, run/busy CAS, soft-delete | CLI formatting, ACP transport |
| **Hub Core** | discover upsert, create/bind state machine, send/param/mode **gates**, close busy rules, ensure_live errors | display tables |
| **RPC / error_data** | envelope codes + data fields for Phase-1 codes | product copy inventing new F-* |
| **CLI** | args, human tables, JSON envelope print, stderr `error: code: message` | business rules |
| **MCP** | same JSON surfaces as CLI | divergent semantics |
| **Adapter (Cursor)** | emit `_meta.cursor-adapter.space` (already) | hub store columns |

```
adapter SessionInfo._meta
        │
        ▼
  conversation_policy::recompute   ◄── unit-tested, no I/O
        │
        ▼
  Store (origin/interaction/phase/busy/last_outcome + status mirror)
        │
   ┌────┴────┐
   ▼         ▼
 Hub gates   list/discover RPC
   │         │
   └────┬────┘
        ▼
   CLI / MCP formatters
```

---

## 2. Phase map → F-* → surfaces

| Phase | Contract | F-* | Primary modules (indicative, not product law) |
|-------|----------|-----|-----------------------------------------------|
| **1** | [PHASE1-CONTRACT v1.2](./OPERATOR-UX-PHASE1-CONTRACT.md) **APPROVED** | F-DISC, F-BIND, F-NEW, F-FIND, F-SEND gates, F-RO, F-CLOSE/DEL/CXL, F-FAIL subset, F-MULTI rule | `conversation_policy`, store migration/list, `registry::list_agent_sessions`, `conversation` create/bind, `prompt` gate, `lifecycle` close/delete, CLI list/sessions, MCP list_* |
| **2** | *write contract before code* | F-READ transcript merge, F-SRCH IX | show/send view, search |
| **3** | *write contract before code* | F-COG probe, F-PROG, F-FAIL full, F-CONT timings | inspect, progress stream |
| **4** | *write contract before code* | F-DOC, F-MIG/SHIP | doctor, release notes |

**Rule:** Unregistered F-* commands forbidden. Phase 2+ field invention without contract forbidden.

---

## 3. Phase 1 deliverables (exit checklist ownership)

| Checklist item | Owner |
|----------------|-------|
| Migration + origin/interaction backfill | Store |
| discover no session/load; no-downgrade; deleted revive | Hub registry + Store upsert |
| Option A send/param/mode gates | Hub prompt + param/mode paths |
| bind state machine + load-fail keep row | Hub conversation |
| workbench list + envelope + ORDER BY | Store + CLI/MCP |
| sessions DTO columns | Hub list_agent_sessions + CLI |
| error envelope codes | HubError + rpc error_data + CLI |
| close-while-busy + soft delete | Store + lifecycle |
| SC oracles | hub/cli tests |
| Cursor meta still readable | hub parse + adapter (already emits space) |

---

## 4. Implementation order (Phase 1)

1. Pure policy module + unit tests  
2. Additive migration 7 + ConversationRow fields + create/finalize/list rewrites  
3. Discover metadata-only path (remove load branch)  
4. Create/bind + write gates  
5. List filters/envelope + CLI/MCP args  
6. Close/delete/busy + recover_interrupted_runs hybrid fields  
7. SC tests + review-rework  

---

## 5. Later phases (design-before-code only)

- **Phase 2:** OPERATOR-UX-PHASE2-CONTRACT (transcript merge algorithm freeze, search IX field)  
- **Phase 3:** progress stages + inspect probe fields  
- **Phase 4:** doctor journey + migration UX for reject→auto-allow  

Do not implement Phase 2–4 features in this ship without those contracts.

---

## 6. Verification discipline

- Tests must call **shipped** store/hub/CLI paths.  
- Evidence under goal scratch; PLAN/COMPLIANCE updated.  
- Frozen `doc/ssot/pillars/*` never edited.  
- Green Phase 1 ≠ product-complete UX.  
