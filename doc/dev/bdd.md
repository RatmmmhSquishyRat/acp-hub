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

## Feature 11 — Hub module maintainability

### Preserve behavior while splitting oversized modules

```gherkin
Given crate::hub exposes CoreHub, HubClient, and the existing public DTOs
When the Hub implementation is organized by domain
Then hub.rs is only a thin module facade
And production responsibilities are separated into types, state, registry, conversation, prompt, lifecycle, dispatch, and client modules
And shared fixtures and operation/replay tests are not duplicated
And every Hub production or test Rust file remains below 1,000 lines
And workspace callers compile without changing crate::hub public paths
And the operation, cancellation, refresh publication, and replay-lock scenarios still pass
```
