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

## Exit

Acceptance under goal scratch + [COMPLIANCE.md](./COMPLIANCE.md):
full workspace tests green; Store-first and dual-layer proven in code;
frozen pillars unmodified; no “force agent refresh” durable law;
no residual-completion packaging.
