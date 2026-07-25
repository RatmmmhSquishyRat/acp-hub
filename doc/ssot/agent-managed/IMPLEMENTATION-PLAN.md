# Operator UX — Implementation Plan (decoupled)

**Status:** Phases 1–4 **shipped**  
**Date:** 2026-07-24  
**Authority:** [OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md) → Phase contracts → this plan  
**Ship evaluation:** [OPERATOR-UX-SHIP.md](./OPERATOR-UX-SHIP.md)

---

## 1. Workflow (who owns what)

| Layer | Owns | Does not own |
|-------|------|--------------|
| **Policy pure** (`store/conversation_policy`) | origin/interaction/phase/busy/last_outcome; synthetic STATUS; meta→space | I/O |
| **Transcript pure** (`store/transcript_view`) | merge, clean_body, summary_preview | DB writes |
| **Progress pure** (`progress`) | stage events + timings keys | transport |
| **Store** | schema, list filters, search hits, soft-delete | CLI tables |
| **Hub Core** | discover, create/bind, gates, show_conversation, inspect±probe | display |
| **CLI / MCP** | args, tables, JSON, progress stderr, doctor | business rules |

---

## 2. Phase map → F-* → surfaces

| Phase | Contract | F-* | Modules |
|-------|----------|-----|---------|
| **1** | PHASE1-CONTRACT | F-DISC F-BIND F-NEW F-FIND F-SEND gates F-RO F-CLOSE/DEL F-FAIL subset | conversation_policy, lifecycle, registry discover, prompt gate |
| **2** | PHASE2-CONTRACT | F-READ F-SRCH F.6 preview | transcript_view, show_conversation, search SQL fields |
| **3** | PHASE3-CONTRACT | F-COG F-PROG | inspect probe, progress tracker, CLI create/send |
| **4** | PHASE4-CONTRACT | F-DOC F-MIG messaging | `doctor`, SHIP notes |

---

## 3. Shipped checklist

### Phase 1
- [x] Migration + hybrid fields + workbench + Option A + discover + errors

### Phase 2
- [x] merge_transcript + show `--raw` + summaryPreview on list/show  
- [x] Search hit interaction/origin/updated_at  
- [x] SC-13 tests on shipped `show_conversation`

### Phase 3
- [x] inspect probeStatus skipped/cached/ok/failed + reject hint  
- [x] ProgressTracker on create/send (daemon_connect, session_op|prompt, end + timings)

### Phase 4
- [x] `acp-hub doctor` G.0 + reject scan (no silent rewrite)  
- [x] M1–M8 evaluation in OPERATOR-UX-SHIP.md  

---

## 4. Verification

- `cargo test -p acp-hub-core --test phase1_operator_ux --test operator_ux_full`  
- `cargo test -p acp-hub-cli`  
- clippy `-D warnings` on hub+cli  
- Frozen pillars untouched  

---

## 5. Non-goals remaining

Phase 5 optional (pin/archive). Live Cursor full daemon-kill E2E environment-limited. Layer1 auto-load on show deferred with honest `layer1Refreshed=false`.
