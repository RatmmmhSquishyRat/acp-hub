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
- [x] Resume error classes  
- [x] Operator docs + samples + active design/spec/impl_plan aligned  
- [x] Research historical packs labeled superseded where they stated old law  
- [x] Zero-trust re-proof: full cargo tests + pollution scan → goal scratch  

## Exit

Acceptance criteria verified with evidence under the active goal scratch
(`ux-converge-tests.log`, `defaults-assert.log`, `must-pass-hits.txt`,
`doc-pollution-scan.txt`, `agent-pollution.txt`, `pillars-status.txt`).
No residual-completion packaging file.
