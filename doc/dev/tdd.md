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
3. Request image/audio without capability and assert a typed error.
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

### Hub module boundaries

1. Compile the full workspace after moving code; unchanged `crate::hub::*`
   callsites prove the facade re-exports remain compatible.
2. Run all `hub::tests` after the split and compare test names with the
   pre-split inventory; no test may disappear or be duplicated.
3. Run the stale-cancel and external-refresh-publication regressions by exact
   name after moving them into split test modules.
4. Count `crates/hub/src/hub.rs`, `crates/hub/src/hub/*.rs`, and
   `crates/hub/src/hub/tests/*.rs`; every file must stay below 1,000 lines, with
   approximately 900 lines treated as the proactive split threshold.
5. Run Clippy with all targets and all features so sibling-module visibility,
   unused re-exports, and test-only imports cannot hide behind the default
   build graph.

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
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --all-targets --locked
cargo test --workspace --locked
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
cargo package -p acp-hub-cli --list --locked
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
