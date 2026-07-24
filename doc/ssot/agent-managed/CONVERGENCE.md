# Convergence — UX-first acp-hub overlay

## Where product law lives

| Kind | Path |
|------|------|
| Frozen user SSOT | `doc/ssot/pillars/*` (read-only) |
| Agent-managed overlay | this tree |
| Active design | `doc/dev/design.md`, `spec.md`, `impl_plan.md` must match overlay |

## Criterion → shipped surface → test

| # | Criterion | Surface | Test (crate) |
|---|-----------|---------|--------------|
| 1 | Agent-managed overlay; frozen pillars clean | `doc/ssot/agent-managed/*` | `git status doc/ssot/pillars` empty of modifications |
| 2a | Omit → usable | `endpoint` Default, CLI, MCP | `endpoint_defaults_*`, CLI `agent_registration_defaults_*`, MCP `register_agent_defaults_*` |
| 2b | Explicit reject / sandbox | CLI `--sandbox`, JSON reject | `endpoint_explicit_reject_*`, `agent_registration_sandbox_*`, MCP `register_agent_explicit_reject_*` |
| 3 | Lag continues | `daemon/rpc_io` Lagged → continue | `lagged_notification_stream_continues_*` |
| 3b | Store-first durable ownership | capture writes Store before `hub/conv/update`; lag log is live-only | `store_persists_*dropped_live*`, capture tests with Store asserts |
| 4 | Resume classes | `SafeResumeSourceData` + MCP structured source | `resume_load_source_classes_*`, `resume_load_encode_maps_acp_*`, `resume_load_failed_maps_*` |
| 5 | Docs not current-law reject/lag-fatal/resync-as-Store-repair | README, SECURITY, adapters, design, impl_plan, spec, Product-UX | pollution scan |
| 6 | Full law matrix | frozen + Product-UX | [COMPLIANCE.md](./COMPLIANCE.md) + full cargo test |

## Migration honesty (operator fact, not a residual backlog)

On-disk `agents.json` entries that already store explicit `reject` / disabled
caps are **not** rewritten by defaulting code. Operators re-register or edit
JSON. Documented in README + CHANGELOG Unreleased.
