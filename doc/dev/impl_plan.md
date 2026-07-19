# ACP Hub — Implementation and Maintenance Plan

> Pillars: `doc/ssot/pillars/README.md` and `doc/ssot/pillars/TechSel.md`
> Required behavior: `doc/dev/spec.md`, `doc/dev/design.md`, `doc/dev/bdd.md`
> Verification contract: `doc/dev/tdd.md`
>
> Current repository-internal progress handoff:
> `doc/maintenance/continuation-handoff-2026-07-18.md`
> Repository-internal review evidence:
> `doc/review/complete-review-book-2026-07-18.md`
> Repository-internal task plan evidence (historical and superseded):
> `doc/maintenance/complete-task-plan-2026-07-18.md`

The pillars, required-behavior documents, verification contract, and this
durable plan are the behavioral authority. The handoff reports non-product
progress evidence for the current maintenance work. The dated review and
superseded task plan are repository-internal evidence only. None of these
internal records overrides the durable authority chain or belongs in root
product marketing or operator release archives.

## 1. Status model

Repository state is reported in separate dimensions:

1. **Implemented**: a code path exists.
2. **Statically verified**: formatting, compilation, lint, syntax, or unit tests
   pass.
3. **Behaviorally accepted**: the relevant BDD scenario has a meaningful test
   that cannot pass without proving the behavior.
4. **Deployment accepted**: install/package/release artifacts contain the
   documented runtime components and have been exercised from a clean install.

One dimension never implies another. In particular, a green workspace test run
does not establish ACP interoperability, adapter compatibility, daemon
cancellation, two-layer history correctness, or release completeness.

## 2. Current repository surfaces

The repository contains:

- endpoint registry and transport configuration;
- SQLite projection and search;
- ACP driver/callback handling;
- conductor/proxy assembly;
- runtime/run state;
- singleton daemon and local JSON-RPC;
- CLI, MCP facade, and Rust client;
- Cursor and Grok vendor compatibility adapters;
- Codex and OMP registration examples;
- multi-platform CI and release workflows.

These surfaces remain subject to the acceptance gates below. Exact test counts,
dependency versions, local session counts, model ids, and vendor CLI versions
are obtained from the current checkout and validation environment; they are not
hard-coded as completion evidence in this plan.

## 3. Maintenance work packages

### P0 — Data isolation and history correctness

- Namespace callback state by endpoint and session, not session id alone.
- Create/bind the Hub conversation before `session/load` can replay messages.
- Preserve Layer 1 (`load_replay`) and Layer 2 (`local_turn`) independently.
- Make Layer 1 refresh transactional and idempotent; failed refresh leaves the
  previous projection current.
- Reject or safely cancel/drain deletion while a run is active.
- Finalize a run against the conversation stored on that run, not a
  caller-supplied unrelated id.

Acceptance: BDD Features 2, 3, and 8; TDD isolation/load/delete tests.

### P0 — Capability and execution security

- Send configured client filesystem/terminal capabilities during initialize.
- Reject unsupported ACP protocol versions.
- Enforce filesystem and terminal permission checks inside every callback.
- Bind terminal handles to the owning endpoint/session and avoid blocking reads
  while holding a global lock.
- Validate image/audio/embedded-resource prompt capability after initialize but
  before live-session/config/mode/prompt dispatch and Store effects.
- Redact registry secrets in CLI and MCP output; enforce or document private
  file permissions at creation.

Acceptance: BDD Features 1, 3, and 9; protocol/security tests.

### P1 — Daemon and runtime reliability

- Derive default conversation cwd from the caller request, never from the
  daemon's original startup directory.
- Allow a concurrent cancel request to be processed while a send is in flight
  on the same logical client.
- Recover stale running/cancelling rows after daemon failure.
- Avoid holding the global agent-handle map across connection initialization;
  apply timeouts and evict failed/stale handles.
- Invalidate cached handles after endpoint/proxy replacement.

Acceptance: BDD Features 3, 6, and 8; daemon/RPC concurrency tests.

### P1 — Registry, proxy, search, and pagination

- Serialize registry read-modify-write and detect external file changes.
- Refuse removal of a proxy still referenced by an agent, or update references
  atomically.
- Follow every `session/list` cursor until exhaustion with loop protection.
- Return stable search pagination, nonempty snippets, and at most `limit` hits
  across title and message results.
- Make callback failures return ACP errors instead of successful payloads that
  contain error text.
- Acquire persisted cancellation ownership before notifying an agent, serialize
  it with prompt finalization, and roll back all state when notification send
  fails.
- Treat daemon notification lag as a reconnect/resynchronize boundary.
- Retire terminal quota/activity ownership before best-effort teardown cleanup,
  while keeping explicit operations on still-owned terminals retryable.

Acceptance: BDD Features 1, 4, 5, and 8.

### P1 — Adapter compatibility

- Cursor direct DB access remains read-only; CLI resume is documented as a
  potential vendor-session write; IDE prompt stays rejected.
- Grok initialize errors remain JSON-RPC errors.
- Grok prompt text stays out of process arguments; the temporary prompt file is
  private and removed.
- Grok `session/delete` uses the supported vendor command and is advertised only
  while implemented.
- Adapter probes use isolated fixtures by default. Installed-agent and
  destructive modes require separate explicit opt-ins and never print session
  bodies, ids, or local paths.
- Durable docs use a reproducible matrix instead of machine-specific dates,
  counts, commits, branches, marker phrases, or model/version assumptions.

Acceptance: the adapter matrices in their specs. Real vendor sessions are never
used by default CI.

### P1 — Repository module ownership and maintainability

- Keep `hub.rs` as the stable `crate::hub::*` facade.
- Separate DTOs, engine state, registry, conversation/replay, prompt/cancel,
  lifecycle, dispatch, and daemon-backed client responsibilities.
- Split inline Hub tests into shared fixtures and registry/client/operation/
  replay groups without duplicating fixture programs.
- Apply the same facade/domain rule to callbacks, bounded transports, daemon,
  RPC, store, ACP, CLI and MCP modules.
- Split oversized test files by observable behavior while retaining each test
  exactly once.
- Reserve a conversation operation before reading endpoint config or acquiring
  the handle used by that operation.
- Keep every production and test Rust file below 1,000 lines; use
  approximately 900 lines as the point for proactive decomposition.
- Require independent spec and code-quality review for non-small boundary
  changes.

Acceptance: BDD Feature 11, the Hub boundary checks in TDD section 3, workspace
compilation, and the existing operation/replay regression suite.

### P0 — Official SDK current-major migration

- Move `agent-client-protocol`, conductor and the test harness to the same
  current official stable release line; keep bounded HTTP/WebSocket on those
  core types and remove the unused official HTTP manifest entry.
- Pin any unpublished official test harness to the exact matching release
  revision; keep publishable crate manifests on crates.io version requirements.
- Move `rmcp` to its current stable major and adapt the MCP integration edge
  without removing tools or weakening closed schemas and annotations.
- Preserve ACP v1 wire negotiation, project resource budgets, privacy
  sanitation, capability gates, cancellation and two-layer history semantics.
- Remove stale direct-major dependencies from the final graph.

Acceptance: BDD Feature 12, the SDK upgrade checks in TDD section 3, real ACP
integration tests, real MCP stdio smoke, dependency policy and package dry-run.

### P0 — Registry, persistence, resource and privacy closure

- Make session import, registry commit/recovery, endpoint initialization
  publication, Store migration/upsert, run ownership and message paging atomic
  or explicitly conflict-detecting.
- Add daemon dispatch-lifetime byte admission and aggregate session discovery
  budgets using the fixed limits and accounting model from the spec.
- Validate absolute session paths before Store/load, and prompt content
  capabilities before live-session/config/mode/prompt or Store side effects.
- Clear stale capability cache on registry mutation and fail closed on corrupt
  persisted JSON/enum values.
- Expose only the redacted public endpoint projection through daemon, CLI and
  MCP reads.
- Exercise identity-bound physical proxy ACK through real bounded legs,
  including reordered and canonically identical/different-size frames.

Acceptance: BDD Features 13–15 and the registry/store/admission/privacy
regressions in TDD section 3.

### P1 — Documentation, skill, installation, and release

- Keep all command examples aligned with live Clap help: top-level `send` and
  `search`; no `agent sessions --import`.
- Keep Core Hub's ACP-only boundary distinct from explicitly registered vendor
  adapters.
- Default registration examples to rejected permission, disabled filesystem,
  and disabled terminal callbacks.
- Provide valid POSIX and PowerShell examples with portable placeholders.
- Keep default adapter ready logs free of absolute local paths.
- Include the binary, licenses, root operator documents, `adapters/`, the ACP
  Hub skill, and `BUILD_INFO.txt` in release archives.
- Validate the explicit allowlist from extracted archive contents and reject
  internal review/maintenance records or local-worktree control evidence.

Acceptance: clean-install/package scenarios in BDD Feature 10.

## 4. Implementation order

1. Freeze the live checkout baseline and preserve unrelated dirty files.
2. Repair P0 isolation/history invariants and add focused regression tests.
3. Repair capability/permission enforcement and error propagation.
4. Repair daemon concurrency, cwd provenance, handle lifecycle, and recovery.
5. Repair registry/proxy/search/pagination semantics.
6. Maintain adapters and their opt-in probes.
7. Close registry/store/resource/privacy findings and obtain independent
   adversarial re-review.
8. Count every Rust file. For each file at or above the documented boundary,
   update spec/design/BDD/TDD/impl_plan, complete third-party review, and split
   it into domain modules while preserving public API and behavior. The current
   decomposition order is shared state/types, callbacks/transport, daemon/RPC,
   store/ACP, CLI/MCP, then oversized tests.
9. Finish the mechanical split as an independently green, revertible change.
10. Upgrade direct ACP and MCP SDKs at their integration edges as a separate
   independently green, revertible change and rerun their
   real process/protocol tests.
11. Align docs, skill, samples, install instructions, and release payload.
12. Run static/unit/integration gates.
13. Run isolated daemon/CLI/MCP acceptance.
14. Run explicit vendor E2E only in disposable homes/sessions.
15. Inspect the built release archives and record results in a dated validation
    report outside this durable plan.

## 5. Completion gates

The project can be called complete only when:

- every P0/P1 item is implemented or explicitly rejected by a pillar change;
- all BDD scenarios have meaningful automated coverage, except clearly labeled
  external-vendor compatibility probes;
- proxy, cancel, close, pagination, and callback permission tests cannot pass
  without exercising the named behavior;
- CLI and MCP use the same daemon semantics and neither exposes credentials;
- extracted release archives contain only the binary, licenses, root operator
  documents, adapters, the ACP Hub skill, and `BUILD_INFO.txt` at the top level;
- extracted release content excludes internal review/maintenance records and
  local-worktree control evidence;
- default tests do not read or modify real Cursor/Grok sessions;
- the final report separates source state, verification state, deployment
  state, and remaining external compatibility constraints.

No “already complete” list is maintained here. Completion is derived from the
current evidence produced by these gates.
