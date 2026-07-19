# ACP Hub — Test and Verification Plan

> Grounded in `doc/dev/bdd.md`. A test name does not count as coverage unless
> its assertions prove the named behavior.

## 1. Test layers

| Layer | Default safety | Purpose |
|---|---|---|
| Rust unit | hermetic | store, registry, redaction, state transitions |
| Rust integration | hermetic fixtures/temp homes | ACP driver, callbacks, proxy, daemon, RPC, MCP |
| JS static | hermetic | adapter syntax and fixture-based parser/router tests |
| CLI package | temp home and extracted archive | install/help/daemon/command surface |
| Vendor compatibility | explicit opt-in only | current Cursor/Grok behavior against disposable sessions |

Default CI must never inspect or mutate a user's real Cursor/Grok session store.

## 2. Current test inventory

The checkout contains:

- `crates/hub/tests/store.rs`
- `crates/hub/tests/registry.rs`
- `crates/hub/tests/daemon_idle.rs`
- `crates/integration-tests/tests/testy_full_flow.rs`
- `crates/integration-tests/tests/callback_roundtrip.rs`
- `crates/integration-tests/tests/concurrency.rs`
- `crates/integration-tests/tests/protocol_surface.rs`
- `crates/integration-tests/tests/proxies.rs`
- `crates/integration-tests/tests/sdk_probe.rs`
- `crates/cli/tests/cli_contract.rs`
- `crates/cli/tests/mcp_smoke.rs`

Exact test counts are obtained with:

```sh
cargo test --workspace --locked -- --list
```

The inventory is not acceptance by itself. The 2026-07-18 maintenance replaced
the following weak assertions:

- proxy coverage now instantiates an in-process transforming proxy;
- cancel coverage requires the cancelled stop reason;
- close coverage propagates the inner result and verifies the session list;
- load coverage verifies replay replacement and retained Hub capture;
- search coverage verifies combined pagination, snippets, and next offset;
- CLI process tests verify the public command map;
- MCP stdio coverage initializes, lists tools, checks annotations, calls
  `list_agents`, registers/removes an isolated fixture endpoint, and verifies
  daemon idle cleanup.
- callback round-trip coverage sends permission, filesystem, and terminal
  requests through the real ACP request wiring on both Windows and Unix.

## 3. Required regression tests

### Store and two-layer history

1. Create parent conversation before replay notifications; foreign-key errors
   fail the operation.
2. Refresh Layer 1 twice; only prior Layer 1 rows become audit, while every
   `local_turn` remains current.
3. A failed refresh leaves the previous current Layer 1 snapshot intact.
4. Finalize a run using mismatched run/conversation ids; reject and leave both
   conversations correct.
5. Delete during active run; verify conflict or cancel-and-drain.

### Endpoint/session isolation

1. Two agents return the same session id.
2. Emit updates, permission requests, filesystem callbacks, and terminal
   requests from both.
3. Assert each callback uses the correct agent, conversation, cwd, policy, run,
   and terminal owner.

### Initialize and capability enforcement

1. Inspect the actual initialize request and assert configured fs/terminal
   client capabilities.
2. Return a non-v1 protocol version and assert connection failure.
3. Request image/audio/embedded resource without the matching capability and
   assert a typed error before live-session/config/mode/prompt and Store effects.
4. Exercise every filesystem and terminal callback with allowed and denied
   configurations.
5. Make I/O fail and assert JSON-RPC/ACP error responses rather than successful
   placeholder payloads.

### Daemon/RPC/runtime

1. One shared client starts a blocking send and issues cancel; assert cancel is
   handled before natural completion.
2. Start daemon from directory A; create from client directory B without cwd;
   assert B.
3. Concurrent `ensure_daemon` calls create one daemon.
4. Idle exit accounts for clients, runs, and terminal children.
5. Seed stale running/cancelling rows; restart and assert recovery state.
6. Hang one endpoint initialize; other agents remain operable and the request
   times out.

### Registry/proxy

1. Concurrent unrelated updates both persist.
2. External fingerprint change is reloaded or returns a conflict.
3. Replace an endpoint with a cached handle; next call uses the replacement.
4. Remove a referenced proxy; assert a typed reference error and reloadable
   registry.
5. Instantiate a real in-process proxy that transforms a unique token and
   assert the downstream agent sees the transformation.

### Search and session pagination

1. Traverse several session/list cursors and deduplicate ids.
2. Repeat a cursor and assert loop detection.
3. Combine title and message hits; assert total length never exceeds limit.
4. Page twice; assert no repeats/skips and a correct next offset.
5. Assert nonempty snippets and both filters on both hit types.

### CLI/MCP/security

1. Spawn the built `acp-hub mcp`; send MCP initialize and tools/list.
2. Call representative read/write tools and compare equivalent CLI results.
3. Register secrets in env, headers, URL userinfo, and command args; assert CLI
   and MCP redaction.
4. Check new Hub-home file permissions on supported platforms.
5. Parse `--help` and statically verify every command copied into README/skill.

### Repository module boundaries

1. Capture the complete pre-split Rust test list with
   `cargo test --workspace --all-targets --all-features --locked -- --list`.
2. Compile the full workspace after moving code; unchanged external callsites
   prove that facade re-exports and public paths remain compatible.
3. Maintain an explicit one-to-one manifest from every pre-split target/test to
   its post-split target/test using stable logical case ids. Compare normal and
   ignored inventories through that manifest; no case may disappear, duplicate
   or change ignored state. An independent reviewer compares moved assertion,
   fixture and fault-injection bodies; names alone are not semantic proof.
4. Run exact focused regressions for each moved domain:
   - endpoint/session ownership and incompatible ACP version;
   - terminal ownership, process-tree cleanup and capture failure;
   - stdio/HTTP/SSE/WebSocket budgets and identity-bound physical proxy ACK;
   - same-client daemon cancellation and typed RPC errors;
   - replay rollback/recovery, message paging and combined search;
   - CLI paging/redaction and MCP stdio initialize/list/call.
5. Count every `*.rs` file under `crates/`; production and test files must stay
   below 1,000 lines, with approximately 900 lines treated as the proactive
   split threshold.
6. Run formatting and Clippy with all targets and all features so
   sibling-module visibility, unused re-exports, and test-only imports cannot
   hide behind the default build graph.
7. Compile a workspace-external consumer fixture, compare endpoint/Hub DTO and
   MCP tool-schema goldens, compare canonical-semantic ACP v1
   initialize/list/load/prompt/cancel/callback JSON-frame goldens with
   required/forbidden fields, and reopen an
   old database fixture while comparing migration version and schema dump. For
   mechanical movement every consumer source and golden is exact-equal to the
   pre-split baseline.

### Official SDK upgrades

1. Record current crates.io stable versions for direct ACP SDK and `rmcp`
   dependencies from primary package metadata.
2. Move ACP protocol, conductor and test harness to one official release
   tag/revision; reject a graph containing incompatible duplicate ACP core
   types or an unused `agent-client-protocol-http` dependency.
3. Compile every target and update only the integration edge required by
   official API changes.
4. Re-run ACP initialize, session pagination, load/replay, prompt/cancel,
   callback, proxy and bounded-transport tests.
5. Run an official-SDK-driven ACP stdio child-process smoke covering
   initialize/new/list/load/prompt/cancel/callback, not only an in-process Testy
   fixture.
6. Run the real `acp-hub mcp` stdio smoke against the upgraded `rmcp`, including
   tool schema, annotations, structured error and mutation paths.
7. Compare canonical-semantic ACP v1 frame goldens with an independent JSON peer in addition
   to same-SDK tests. Update the external consumer source only for the approved
   ACP Rust type identity change; endpoint/Hub DTO, MCP schema, database schema
   and the canonical-semantic wire contract remains equivalent; raw bytes,
   object field order and legal omission of optional defaults need not match.
8. Run package verification for both `acp-hub-core` and `acp-hub-cli`, then
   compile the packaged crates from an isolated workspace without the source
   patch. CLI publish proof must retain its declared registry dependency:
   either publish the core candidate to a disposable local registry and resolve
   CLI from that registry, or run CLI `cargo publish --dry-run` after the exact
   core version is visible in the release registry. A temporary path dependency
   may be an additional build smoke, never the publish proof.
9. Run dependency policy and inspect the final graph for stale direct SDK
   majors. An indirect old version requires an owning upstream dependency and
   a documented compatibility boundary.

### Registry, store, admission and public privacy regressions

1. Inject first-import replay/capture failures and existing-import failures;
   compare full conversation/FTS/snapshot before-images after rollback.
2. Block endpoint initialization immediately before cache publication, replace
   and remove the endpoint, then prove the old epoch cannot publish.
3. Inject failure before and after registry replace and edit the file while
   mutation waits; compare RPC outcome with disk, memory, fingerprint, cache and
   handle generation.
4. Open a database interrupted between initial schema creation and migration
   marker. Inject each session-upsert statement failure and compare metadata/FTS.
   Open fixtures with malformed JSON and unknown persisted enum values and
   require an explicit corruption error.
5. Page messages, commit replay between pages, and assert every current row is
   returned exactly once or the generation cursor fails explicitly. Repeat
   after daemon restart and reject cursor reuse with another conversation,
   include-audit value, run id or filter.
6. Block 32 MiB-class RPC dispatches and a slow response writer; prove the
   fixed request/response/fallback partitions remain within 87/40/1 MiB,
   partial readers charge exact retained bytes, and response permits remain
   held through flush. Exercise discovery exactly at and beyond 256 pages,
   20,000 received sessions, 8 KiB cursor and 64 MiB input, charging duplicates
   before dedupe; require typed `ResourceLimit`.
7. Reject relative session cwd/root after discovery but before Store/load.
   Reject image/audio/resource after initialize but before live-session,
   config/mode/prompt or Store side effects.
8. Register an absolute private stdio command and inspect it through daemon,
   CLI JSON/human output and a real MCP client; assert only the public redacted
   projection appears.
9. Build a real registry/conductor bounded stdio proxy chain with a test-only
   per-leg reservation/ACK ledger and controlled saturation gate. Assert each
   ACK matches an earlier same-leg canonical identity and one unique token.
   Reorder same-method/different-payload frames and same-canonical/different-wire-
   size frames; the latter must release the smallest reservation so accounting
   never underestimates retained bytes.
10. Through real daemon RPC, create an active run, attempt agent replace/remove,
    attempt tokenless/wrong-owner finalization, and force the prompt worker's
    final CAS to return false. Assert conflict in every case and never prompt
    success.
11. Refresh a prior full static snapshot with a modes-only response; require
    modes to persist independently and absent plan/commands/usage/config to
    cease being current.
12. Import a duplicate session across pages with conflicting metadata; prove
    both records consume discovery budget, first metadata wins, and replay runs
    once. Fail a later session and assert prior sessions remain committed, the
    failing session rolls back, later sessions are skipped, and the typed error
    reports completed count.
13. Persist standard `SessionInfo.updated_at` and bounded opaque `_meta`; verify
    round-trip in the Agent Original projection and privacy filtering on public
    inspection.
14. Mutate cursor bytes, checksum, conversation, generation, include-audit,
    run/filter and last key; require invalid/stale cursor without treating the
    unkeyed cursor as an authorization token.

## 4. Adapter tests

Hermetic parser/router fixtures contain synthetic session stores. They cover
list/load, fail-closed malformed storage, Grok initialization error
propagation, safe prompt-routing decisions, path-free diagnostics, sanitized
errors, Grok delete success/failure handling, and Grok prompt-file cleanup on
adapter shutdown without launching a vendor agent.

Manual compatibility probes remain separate. Read-only installed-agent probes
require only the live opt-in.

POSIX:

```sh
unset ACP_ADAPTER_DESTRUCTIVE_TESTS
export ACP_ADAPTER_LIVE_TESTS=1
cursor_cli_id='replace-with-disposable-cursor-cli-session-id'
cursor_ide_id='replace-with-disposable-cursor-ide-session-id'
grok_session_id='replace-with-disposable-grok-session-id'
node ./adapters/cursor/adapter-test.mjs "$cursor_cli_id" "$cursor_ide_id"
node ./adapters/grok/adapter-test.mjs "$grok_session_id"
```

PowerShell:

```powershell
Remove-Item Env:ACP_ADAPTER_DESTRUCTIVE_TESTS -ErrorAction SilentlyContinue
$env:ACP_ADAPTER_LIVE_TESTS = '1'
$cursorCliId = 'replace-with-disposable-cursor-cli-session-id'
$cursorIdeId = 'replace-with-disposable-cursor-ide-session-id'
$grokSessionId = 'replace-with-disposable-grok-session-id'
node .\adapters\cursor\adapter-test.mjs "$cursorCliId" "$cursorIdeId"
node .\adapters\grok\adapter-test.mjs "$grokSessionId"
```

Mutation is a separate opt-in. These commands may append to or delete
vendor-managed state, so every supplied id must identify a disposable session.

POSIX:

```sh
export ACP_ADAPTER_LIVE_TESTS=1
export ACP_ADAPTER_DESTRUCTIVE_TESTS=1
cursor_cli_id='replace-with-disposable-cursor-cli-session-id'
cursor_ide_id='replace-with-disposable-cursor-ide-session-id'
grok_session_id='replace-with-disposable-grok-session-id'
node ./adapters/cursor/adapter-test.mjs "$cursor_cli_id" "$cursor_ide_id"
node ./adapters/grok/adapter-test.mjs "$grok_session_id"
```

PowerShell:

```powershell
$env:ACP_ADAPTER_LIVE_TESTS = '1'
$env:ACP_ADAPTER_DESTRUCTIVE_TESTS = '1'
$cursorCliId = 'replace-with-disposable-cursor-cli-session-id'
$cursorIdeId = 'replace-with-disposable-cursor-ide-session-id'
$grokSessionId = 'replace-with-disposable-grok-session-id'
node .\adapters\cursor\adapter-test.mjs "$cursorCliId" "$cursorIdeId"
node .\adapters\grok\adapter-test.mjs "$grokSessionId"
```

The scripts must not print message bodies, prompts, session ids, paths, branch
names, commits, or local marker phrases.

## 5. Verification commands

Static and Rust gates:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo build --workspace --all-targets --all-features --locked
cargo test --workspace --all-targets --all-features --locked -- --test-threads=1
cargo test --workspace --all-targets --all-features --locked -- --ignored --list
cargo deny check
cargo tree -d
node --check adapters/cursor/adapter.mjs
node --check adapters/cursor/adapter-test.mjs
node --check adapters/grok/adapter.mjs
node --check adapters/grok/adapter-test.mjs
node adapters/cursor/adapter-test.mjs
node adapters/grok/adapter-test.mjs
```

Package/release gates:

```sh
cargo publish -p acp-hub-core --dry-run --locked
cargo package -p acp-hub-cli --locked
# After the exact core version is available in the selected registry:
cargo publish -p acp-hub-cli --dry-run --locked
```

The release workflow must additionally extract each produced archive, compare
its top-level contents with the binary/licenses/root-docs/adapters/skill/
`BUILD_INFO.txt` allowlist, verify the required nested files, and reject
review/maintenance records or local-worktree control evidence.

## 6. Evidence format

Validation reports record:

- checkout commit and dirty-state summary;
- OS/toolchain/current dependency lock;
- exact command and exit status;
- test list and failures;
- archive manifest and checksum verification;
- vendor versions only for the dated compatibility run;
- whether any destructive opt-in was enabled.

Durable specs and README files contain the matrix, not local validation values.
