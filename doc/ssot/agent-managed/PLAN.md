# PLAN — UX-first overlay (implementation)

**Control plane:** this directory  
**Product law:** [pillars/Product-UX.md](./pillars/Product-UX.md)  
**Acceptance map:** [CONVERGENCE.md](./CONVERGENCE.md)

## Checklist

- [x] Frozen pillars byte-stable (no edits)  
- [x] Agent-managed overlay (no residual-completion packaging)  
- [x] Defaults: AutoAllow + fs + terminal (CLI / MCP / serde)  
- [x] Sandbox / explicit reject preserved  
- [x] Lag non-fatal  
- [x] **Store-first conversation ownership** (Product-UX §5; kill incomplete-projection / operator-resync / force-agent-refresh narrative)  
- [x] Align design.md / spec.md / impl_plan.md / bdd / CHANGELOG / rpc_io log with Store-first  
- [x] Unit proof: Store survives dropped live fan-out; lag continues  
- [x] Resume error classes  
- [x] Operator docs + samples + active design/spec/impl_plan aligned (Store-first pass)  
- [x] Research historical packs labeled superseded where they stated old law  
- [x] Capture budget restore on failed Store write + unit test  
- [x] MCP ResumeLoadFailed structured source (Product-UX §6)  
- [x] Full code review vs frozen pillars + Product-UX → [COMPLIANCE.md](./COMPLIANCE.md)  
- [x] Full workspace cargo test + fmt/clippy → goal scratch  

## Exit (G1–G6 overlay only)

Acceptance under goal scratch + [COMPLIANCE.md](./COMPLIANCE.md):
full workspace tests green; Store-first and dual-layer proven in code;
frozen pillars unmodified; no “force agent refresh” durable law;
no residual-completion packaging.

---

## Next program — Operator UX System（G7–G10）

**Problem register:** [OPERATOR-UX-CHARTER.md](./OPERATOR-UX-CHARTER.md)  
**System design (eval + F-* features + journeys + phases):** [OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md)  

**User mandate:** 不只 journey 提纲 — 从零完整评估/理解/规划设计，规范化 feature，结束仓库功能混乱。Design before code.

- [x] As-Is chaos map + To-Be concept model + F-* catalog + phases in OPERATOR-UX-SYSTEM.md  
- [x] Multi-round adversarial refine → SYSTEM v0.3 + PHASE1-CONTRACT v1.2（coding-ready）  
- [x] Phase 1 implement against PHASE1-CONTRACT（schema/hybrid fields, discover metadata-only, Option A gates, workbench list, bind, soft-delete, SC oracles）  
- [x] Decoupled implement plan: [IMPLEMENTATION-PLAN.md](./IMPLEMENTATION-PLAN.md)  
- [ ] Phase 2: **requires PHASE2-CONTRACT first** — preview + transcript view + search IX（F-READ F-SRCH）  
- [ ] Phase 3: **requires PHASE3-CONTRACT first** — inspect probe + progress（F-COG F-PROG）  
- [ ] Phase 4: **requires PHASE4-CONTRACT first** — doctor + ship notes  
- [x] 禁止用 TTL/gc 冒充会话 UX 完成；禁止未登记 F-* 的野生命令  
- [x] **Honest:** Phase 1 ≠ M1–M6 product complete

**User judgment recorded:** 功能不齐全、语义重叠 → 根本无法使用；根因是缺少使用者正向动线与完整 UX 系统规划。
