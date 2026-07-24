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
- [x] Multi-round adversarial refine → SYSTEM v0.3 + PHASE1-CONTRACT v1.1（coding-ready）  
- [ ] Phase 1 implement against PHASE1-CONTRACT only（F-RO F-DISC F-BIND F-FIND；Option A）  
- [ ] Phase 2: preview + transcript view + search 降噪（F-FIND F-READ F-SRCH）  
- [ ] Phase 3: inspect probe + progress + error next-step（F-COG F-PROG F-FAIL）  
- [ ] Phase 4: docs/skill by G.0 + scenario regression + ship notes  
- [ ] Phase 5 optional only after 1–4  
- [ ] 禁止用 TTL/gc 冒充会话 UX 完成；禁止未登记 F-* 的野生命令  

**User judgment recorded:** 功能不齐全、语义重叠 → 根本无法使用；根因是缺少使用者正向动线与完整 UX 系统规划。
