# ACP Hub — Behavioral Acceptance

> Grounded in `doc/dev/spec.md` and `doc/ssot/pillars/README.md`.

## Feature 1 — Endpoint registry and capabilities

### Register each transport

```gherkin
Given an isolated Hub home
When I register a valid stdio, HTTP, or WebSocket endpoint
Then the endpoint appears in agent list
And the persisted registry remains valid JSON
And agent inspect redacts environment, header, URL-userinfo, and command-argument secrets
```

### Replace an endpoint

```gherkin
Given agent "a" already has a live cached handle
When I replace agent "a" with a new transport configuration
Then the next operation uses the new configuration
And no daemon restart is required
```

### Negotiate capabilities

```gherkin
Given the endpoint is configured with filesystem disabled and terminal disabled
When ACP initialize completes
Then those disabled client capabilities are advertised
And later filesystem or terminal callbacks are rejected
And a non-v1 protocol response terminates initialization with a typed error
```

## Feature 2 — Independent history layers

### Import an existing session without losing replay

```gherkin
Given an endpoint exposes an existing session with replayed messages
When I create a Hub conversation with --agent-session-id
Then the Hub conversation exists before replay notifications arrive
And every replayed message is stored as load_replay
And load failure leaves the prior projection unchanged
```

### Preserve both layers

```gherkin
Given a conversation contains load_replay and local_turn messages
When Layer 1 is refreshed
Then the new Layer 1 snapshot replaces only the prior current Layer 1 snapshot
And local_turn messages remain current
And conv show and search expose both layers with source labels
```

### Isolate equal session ids

```gherkin
Given agent "a" and agent "b" both return session id "same"
When both sessions emit updates and callbacks
Then each update reaches only its own Hub conversation
And permissions, cwd, run state, and terminal handles cannot cross endpoints
```

### Paginate session discovery

```gherkin
Given session/list returns several pages
When I run "acp-hub agent sessions <agent>"
Then every page is consumed exactly once
And all unique sessions are projected
And a repeated cursor fails instead of looping forever
```

## Feature 3 — Prompt, cancellation, and deletion

### Send and capture

```gherkin
Given a conversation is ready
When I run top-level "acp-hub send <conv> --text Hello"
Then the agent receives the prompt
And updates are captured under the correct run and conversation
And the final result contains the actual ACP stop reason
```

### Cancel on the same logical client

```gherkin
Given a send is still in flight on a shared daemon client
When that client sends cancel
Then the daemon processes cancel before send finishes naturally
And the agent receives the ACP cancel notification
And the run reaches a cancelled terminal state
```

### Protect active deletion

```gherkin
Given a conversation has an active run
When local-only or remote deletion is requested
Then deletion returns a conflict
Or the run is cancelled and fully drained before any rows are removed
And no successful send can continue writing into a deleted conversation
```

## Feature 4 — Search

### Stable combined pagination

```gherkin
Given matching conversation titles and matching messages in both history layers
When I request search with limit 5 and an offset
Then at most 5 combined hits are returned
And every message hit has a useful snippet
And the next offset neither repeats nor skips hits
And agent and conversation filters apply to both hit types
```

## Feature 5 — Proxy chains

### Forward through a real proxy

```gherkin
Given agent "a" references proxy "p"
And proxy "p" transforms a unique prompt token
When I send the prompt
Then the agent observes the transformed token
And the returned response passes through the proxy
```

### Protect proxy references

```gherkin
Given agent "a" still references proxy "p"
When I remove proxy "p"
Then the operation fails with the referencing agent ids
And the saved registry can still be loaded after restart
```

## Feature 6 — Daemon lifecycle and cwd

### Caller cwd

```gherkin
Given one daemon was first started from project A
When a client in project B creates a conversation without --cwd
Then the resolved conversation cwd is project B
And no request inherits project A merely from daemon startup
```

### Singleton, idle exit, and recovery

```gherkin
Given several clients target the same Hub home
When they connect concurrently
Then only one daemon owns that home
And it remains alive while clients, runs, or terminal children are active
And after a crash the next daemon normalizes stale nonterminal runs
```

## Feature 7 — MCP facade

### Semantic parity and secret safety

```gherkin
Given an MCP client connects to "acp-hub mcp"
When it initializes, lists tools, inspects agents, sends, and searches
Then tool results match equivalent CLI operations
And registry credentials are redacted
And a long send does not prevent cancellation
```

## Feature 8 — Error integrity and concurrency

### Callback error channel

```gherkin
Given an ACP agent requests a denied or failing filesystem/terminal operation
When the Hub handles the callback
Then the callback returns an ACP error
And error text is not returned as file contents, a terminal id, or empty success
```

### Concurrent registry updates

```gherkin
Given two clients add or remove different endpoints concurrently
When both operations complete
Then neither successful update is lost
And agents.json remains parseable
And an external edit is either reloaded or rejected with a conflict
```

## Feature 9 — Vendor adapters

### Cursor boundary

```gherkin
Given explicit disposable Cursor CLI and IDE sessions
When the opt-in adapter probe lists and loads them
Then direct database access is read-only
And message bodies, ids, and paths are not printed
And IDE prompt is rejected before spawning Cursor
And CLI resume runs only under the destructive opt-in
```

### Grok boundary

```gherkin
Given an explicit disposable Grok session
When the opt-in adapter probe loads and resumes it
Then load performs no Grok write
And resume may append to Grok history
And prompt text is absent from process arguments
And the temporary prompt file is removed
And deleting the separately created probe session invokes the supported Grok delete command
```

## Feature 10 — Installation and release

### Clean CLI install

```gherkin
Given a supported platform with no source checkout
When I install acp-hub-cli from crates.io or extract a release archive
Then acp-hub --version runs
And a temporary Hub home can start, list agents, and exit cleanly
```

### Complete release archive

```gherkin
Given a release tag build
When I extract its archive
Then it contains the binary, licenses, README, adapters, registry samples, and ACP Hub skill
And every documented relative path exists inside the archive
And SHA256SUMS verifies the archive
```

## Feature 11 — Repository module maintainability

### Preserve behavior while splitting oversized modules

```gherkin
Given the repository exposes its current Rust library, CLI, MCP, ACP and daemon surfaces
When an oversized production or test file is organized into domain modules
Then the original module path remains a thin stable facade
And callbacks, transports, daemon, RPC, store, ACP, CLI and MCP responsibilities are separated by domain
And every pre-split test remains present exactly once
And every production or test Rust file remains below 1,000 lines
And workspace callers compile without public API, command, schema or serialized-form changes
And the protocol, operation, cancellation, replay, transport, CLI and MCP scenarios still pass
```

## Feature 12 — Current official SDK compatibility

### Upgrade ACP and MCP SDKs without weakening Hub behavior

```gherkin
Given crates.io publishes a newer stable ACP rust-sdk line and rmcp stable major
When ACP Hub updates its direct SDK dependencies
Then ACP protocol, conductor and test types come from one official release line
And bounded HTTP/WebSocket transports use those core types without an unused HTTP dependency
And initialize still negotiates ACP protocol v1
And session list, load, prompt, cancel, callbacks and proxies still pass through real SDK paths
And project frame, queue, privacy and capability limits remain enforced
And a real MCP stdio client can initialize, list tools and call representative read and write tools
And publish-package verification resolves only the declared published production dependencies
```

## Feature 13 — Atomic registry and persistence state

### Failed import and concurrent replacement cannot publish partial state

```gherkin
Given an existing registry, conversation projection and endpoint initializer
When session import fails or the endpoint is replaced while initialization is blocked
Then a first import leaves no ghost conversation
And an existing import restores its metadata and snapshots
And an initializer from the old registry epoch cannot publish a handle
And the RPC result matches the registry image actually committed on disk
And memory, fingerprint, capability cache and live handle use that same image
```

### Migration and run finalization are single-owner commits

```gherkin
Given a partial initial schema migration or an active prompt run
When the process reopens the database or another caller attempts finalization
Then schema objects and the version marker recover atomically
And conversation metadata and FTS never split across commits
And only the owner can finalize the active run
And a failed finalization CAS cannot return prompt success
```

```gherkin
Given hub/conv/create_run created an active run through the real daemon RPC
When another client replaces or removes that run's agent
Then registry mutation returns an active-operation conflict
When another client attempts finalize_run without the owner token
Then finalization returns an ownership conflict
And a prompt worker that loses its finalization CAS cannot report success
```

```gherkin
Given a previous refresh stored plan, commands, usage, config and modes
When a successful refresh returns modes only
Then the modes snapshot is current
And the absent plan, commands, usage and config snapshots are no longer current
```

## Feature 14 — Aggregate resource and capability admission

```gherkin
Given concurrent maximum-size daemon requests, a slow response writer and a paginated ACP endpoint
When dispatch is blocked and session cursors continue changing
Then aggregate retained RPC bytes stay within the 128 MiB global budget until flush
And request, response and fallback partitions remain within 87 MiB, 40 MiB and 1 MiB
And partial request readers charge only bytes actually retained
And session discovery stops at 256 pages, 20,000 received sessions, 8 KiB cursor or 64 MiB input
And duplicate endpoint/session identities are imported once
And relative cwd or additional roots fail before storage or load
```

```gherkin
Given an agent that does not advertise image, audio or embedded-context support
When a prompt contains the corresponding content block
Then the Hub returns UnsupportedCapability
And no live-session/config/mode/prompt request, run or message is created after initialize
```

## Feature 15 — Safe public inspection and physical proxy flow

```gherkin
Given a registry containing a private absolute command path and credentials
When CLI, daemon or MCP lists or inspects the endpoint
Then registry inspection contains only the transport-specific public allowlist
And it contains no command path, argument/env/header value or private URL component
And conversation/session cwd remains available through its canonical surface
```

```gherkin
Given a real bounded stdio agent and bounded stdio proxy chain
When differently sized notifications, responses and callbacks traverse each leg
Then every physical leg reserves and releases the matching canonical identity
And duplicate identities release the smallest matching reservation so retained bytes are never underestimated
And a per-leg test ledger proves exact reservation token and acknowledgement identity
And no in-process shortcut can satisfy the scenario
```
