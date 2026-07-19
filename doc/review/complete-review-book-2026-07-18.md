# ACP Hub Complete Review Book

Date: 2026-07-18

> **Reconciliation notice (2026-07-19):** F-001 through F-032 record the first
> maintenance pass. A later live-checkout review reopened the repository and
> found additional actionable findings. Sections 10 and 11 are the current
> reconciliation; earlier completion counts remain historical evidence.

## 1. Purpose

This review checks whether the repository satisfies the project pillars as a
complete ACP client and conductor. It covers:

- Hub protocol and daemon behavior
- conversation, run, message, and search persistence
- endpoint and proxy registration
- CLI and MCP surfaces
- Cursor, Grok, and Codex integration material
- project specifications, plans, tests, skill instructions, installation, and
  release packaging
- privacy, credential handling, and local-environment assumptions

The review does not treat a successful compile or a single green test suite as
product acceptance. Each finding is tied to an observable implementation or
documentation surface and to a closure condition.

## 2. Source of truth

The review uses this precedence:

1. `doc/ssot/pillars/`
2. `doc/ssot/dev-principles/`
3. current implementation and public command surface
4. current specifications, design, BDD, TDD, and implementation plan
5. tests, examples, skills, and release artifacts
6. dated research and validation evidence

Research transcripts and dated validation records are evidence, not project
requirements. Local machine paths, installed versions, session counts, branch
names, markers, and one-time command results must not be promoted to durable
product claims.

## 3. Acceptance model

Repository completion requires all of the following states:

1. **Protocol correctness** — negotiated ACP capabilities and version are
   enforced, sessions are isolated by endpoint, and callbacks cannot escape
   their registered permissions.
2. **Persistence correctness** — original agent replay and Hub capture remain
   independently visible, ordered, recoverable, and free of silent loss.
3. **Concurrency correctness** — long turns do not block cancellation or
   unrelated clients; deletion and registry mutation are serialized safely.
4. **Interface correctness** — CLI, MCP, library, examples, and skill use real
   command names and expose intentional, documented differences.
5. **Security correctness** — credentials are redacted from ordinary output,
   sensitive files have restricted permissions, and terminal/filesystem access
   is bound to the requesting endpoint and session.
6. **Distribution correctness** — installation and release artifacts contain
   every file referenced by their bundled documentation.
7. **Evidence correctness** — tests assert the behavior named by the test and
   include real CLI/MCP protocol smoke coverage.

## 4. System boundary

The Hub core communicates with registered endpoints through ACP. A vendor
adapter may read vendor-owned storage only when the official endpoint does not
provide the required session discovery or replay surface. Such access must be:

- isolated to the vendor adapter
- version-scoped and fail-closed
- explicit about read and write behavior
- covered by isolated fixtures
- absent from the Hub core

Reading a session database and resuming an original vendor session are different
operations. A read-only workspace/tool policy does not prove that the vendor's
session store remains unchanged.

## 5. Findings

### F-001 — Session identity is not endpoint-scoped

Severity: critical

Callback bindings, loading state, and active runs were keyed only by the ACP
`session_id`. ACP session identifiers are endpoint-local; two registered
endpoints can return the same value.

Impact:

- messages can be written to another endpoint's conversation
- permission decisions can use another endpoint's policy
- cancellation and loading state can cross endpoint boundaries

Closure:

- key callback state by `(agent_id, session_id)`
- bind callback requests to their connection's endpoint identity
- add a collision test using two endpoints that return the same session id

### F-002 — Imported replay can arrive before its conversation exists

Severity: critical

The existing-session creation path invoked `session/load` before inserting the
Hub conversation row. Replay notifications could therefore fail the message
foreign key and be reduced to a warning, after which an empty conversation was
created.

Closure:

- create the parent projection before loading replay
- remove or fail the provisional projection if load fails
- surface capture failures to the caller
- test replay arrival during `session/load`

### F-003 — Filesystem and terminal capabilities are not enforced end to end

Severity: critical

The Hub had capability configuration but did not advertise it during
initialization, and terminal callbacks did not enforce the bound session's
terminal capability or terminal ownership.

Closure:

- advertise the configured client capability object
- reject callbacks that are disabled for the bound session
- scope terminal ids to endpoint and session
- reject output/wait/kill/release requests from a different owner

### F-004 — ACP protocol version is not validated

Severity: high

Initialization requested ACP v1 but did not reject an incompatible response.

Closure:

- require ACP v1 before creating a usable endpoint handle
- preserve the remote error and negotiated capability snapshot
- test incompatible initialization

### F-005 — A long RPC blocks cancellation on the same connection

Severity: high

The daemon awaited each request before reading the next request on that
connection. MCP and other shared-client users could not submit cancellation
while a send was in progress.

Closure:

- read requests continuously
- execute independent requests concurrently
- serialize response writes and preserve response ids
- retain orderly connection shutdown and activity accounting

### F-006 — Default conversation cwd can be the daemon startup directory

Severity: high

When a client omitted cwd, the daemon-side process current directory was used.
Because the daemon is a singleton, a later project could inherit the directory
of the project that first started it.

Closure:

- resolve and send the caller cwd at every CLI/library entry point, or require
  cwd explicitly
- canonicalize or clearly reject invalid directories
- never derive a new conversation cwd from daemon process state

### F-007 — Original replay and Hub capture are not independent

Severity: critical

Refresh was skipped once any current message existed, while replay staging could
supersede Hub-captured local turns. Repeated load behavior was not consistently
staged or deduplicated.

Closure:

- refresh the original layer independently of local capture
- replace only the prior original replay snapshot
- preserve every Hub-captured local turn
- make replay replacement transactional and deterministic

### F-008 — Active conversations can be deleted

Severity: high

Local-only deletion did not reject a conversation with an active run. Remaining
notifications could be dropped after the binding and projection disappeared.

Closure:

- reject deletion while a run is running or cancelling
- alternatively cancel and wait before deletion through an explicit operation
- test concurrent delete and send

### F-009 — Run finalization does not validate conversation ownership

Severity: high

Run finalization updated the run by run id, then updated a caller-supplied
conversation id without proving that the run belonged to that conversation.

Closure:

- fetch and verify the run's stored `conv_id`
- perform run and conversation state changes in one transaction
- test a mismatched run/conversation pair

### F-010 — Registry mutation can create invalid or lost state

Severity: high

Removing a referenced proxy produced a registry that failed the next load.
Concurrent clone/save/swap operations could lose one update, and replacing an
agent did not invalidate its cached live handle.

Closure:

- reject deletion of referenced proxies unless an explicit cascade is used
- serialize the complete registry mutation and save operation
- use a unique temporary file and atomic replacement
- invalidate affected endpoint/proxy handles after a committed change

### F-011 — External registry edits were described but not observed

Severity: medium

A file fingerprint type existed but the daemon loaded `agents.json` only at
startup.

Closure:

- implement validated reload and affected-handle invalidation, or
- document the file as startup input and require registry changes through RPC

The maintained design chooses one behavior and removes the unused promise.

### F-012 — Search pagination and snippets are inconsistent

Severity: high

Message results were limited before title results were appended. Offset applied
only to one source, title hits could repeat, zero or overflowing limits had
surprising behavior, and message snippets were empty.

Closure:

- normalize and bound limit/offset
- combine result sources before global pagination
- return useful bounded snippets
- expose offset or cursor through CLI and MCP

### F-013 — Callback persistence failures can appear as success

Severity: high

Several message and snapshot writes ignored errors or logged a warning while the
ACP or Hub request returned success.

Closure:

- associate capture failures with the active load or run
- return an explicit failure or partial-capture result
- reserve warnings for failures that the public result also reports

### F-014 — Terminal I/O can deadlock

Severity: high

Blocking pipe reads were performed while holding shared terminal state, and
stdout/stderr were drained sequentially. A child filling one pipe could block
the other and prevent wait or release.

Closure:

- use asynchronous pipe readers
- never hold the terminal registry lock during I/O or child wait
- drain stdout and stderr concurrently
- terminate or release children during endpoint/daemon shutdown

### F-015 — Crash recovery leaves ghost running state

Severity: medium

Persisted `running` or `cancelling` rows were not reconciled when the daemon
started with an empty in-memory active-run map.

Closure:

- perform startup recovery in one transaction
- mark interrupted runs and restore their conversations to a terminal state
- test reopening a database with an unfinished run

### F-016 — Registry and MCP output can disclose credentials

Severity: critical

MCP returned full endpoint/proxy configuration. CLI redaction depended on a
small set of key-name fragments and did not cover authorization headers,
cookies, URL credentials, query secrets, or command arguments.

Closure:

- use one safe public DTO for CLI and MCP
- redact every environment and header value by default
- remove URL userinfo and sensitive query values
- avoid emitting command arguments that can contain credentials
- require an explicit dangerous opt-in for raw configuration

### F-017 — Sensitive state file permissions are not guaranteed

Severity: high

Registry environment variables and headers are stored in plaintext, while
directory and file creation relied on ambient defaults.

Closure:

- restrict Hub home and sensitive files to the current user
- enforce Unix modes and Windows ACL behavior where supported
- check existing state and fail or warn clearly when permissions are unsafe

### F-018 — MCP does not cover the intentional Hub management surface

Severity: high

Agent registration omitted headers, proxy chain, permission policy, and client
capabilities. Conversation creation omitted MCP servers. MCP lacked session
listing and cancellation.

Closure:

- expose complete tagged registration input
- add session discovery and cancellation
- document any intentionally CLI-only operation
- apply accurate side-effect and destructive annotations

### F-019 — MCP responses grow with the entire conversation

Severity: medium

`send_message` returned the complete stored history after each turn, and
`get_messages` had no public limit or cursor.

Closure:

- return the final result and messages added by the current run
- paginate message reads with a server-side maximum

### F-020 — Adapter read-only claims exceed the implementation

Severity: high

Cursor and Grok documentation described all native storage access as strictly
read-only, while prompt handling resumed the original vendor session.

Closure:

- distinguish discovery/replay reads from prompt continuation
- state that resuming a vendor session may update vendor-owned history
- describe workspace/tool restrictions separately

### F-021 — Adapter tests can modify and print private sessions

Severity: critical

The adapter test scripts accepted real session ids, prompted those sessions, and
printed message excerpts.

Closure:

- default to isolated fixture homes
- do not print message bodies
- require an explicit destructive opt-in for a real installed agent
- state the mutation and privacy risk before execution

### F-022 — Grok initialization can report an error as success

Severity: high

An upstream JSON-RPC `error` was wrapped inside a downstream `result`.

Closure:

- preserve JSON-RPC error code, message, and data
- test initialization failure

### F-023 — Adapter and specification commands are not the real CLI

Severity: high

Documents used `--import`, `conv send`, and `conv search`, although the real
surface uses automatic import through `agent sessions` and top-level `send` and
`search`.

Closure:

- replace invented commands
- validate documented commands against `acp-hub --help`
- keep one canonical command reference in the skill

### F-024 — Adapter CRUD is incomplete or inaccurately described

Severity: high

Imported Grok sessions could be discovered and resumed, but deletion was not
bridged even when the installed vendor CLI exposed a delete operation.

Closure:

- implement safe vendor deletion when it can be identified and confirmed, or
- explicitly mark native deletion unsupported and keep Hub projection deletion
  separate

### F-025 — Specifications disagree about private storage

Severity: high

The project specification prohibited direct reads such as Cursor
`state.vscdb`, while the implementer role required a private-storage bridge when
the endpoint lacks history.

Closure:

- keep Hub core ACP-only
- allow optional vendor adapters under the boundary in section 4
- update spec, design, role, BDD, TDD, and adapter specs together

### F-026 — Plans and test claims are stale

Severity: high

The implementation plan reported an old test count and listed implemented files
as missing. TDD described a missing protocol test that now existed and claimed
an MCP smoke test that did not exist.

Closure:

- replace speculative status with a current verification matrix
- keep historical plans clearly marked as historical
- add the missing CLI and MCP process-level tests

### F-027 — Existing integration tests contain false-green assertions

Severity: high

The proxy test did not create a proxy, cancellation accepted normal completion,
close discarded an inner result, load did not assert replay capture, and
session listing did not exercise pagination.

Closure:

- assert the named behavior directly
- use observable fixtures
- fail on normal completion when testing cancellation
- test every promised protocol surface

### F-028 — Examples contain local validation evidence and unsafe defaults

Severity: medium

Adapter material included dated local versions, session counts, temporary
branches and commits, roundtrip markers, and a high-permission Codex sample.

Closure:

- move dated evidence to a dedicated validation report only when it is useful
- keep durable documentation version-neutral
- make sample registry permissions deny-by-default
- place full-access examples behind an explicit trusted-environment warning

### F-029 — Adapter paths and shell examples are not portable

Severity: medium

Sample Node paths used platform-specific separators or malformed placeholders,
and Windows instructions mixed Bash syntax with PowerShell.

Closure:

- provide separate PowerShell and POSIX examples
- use valid absolute script path placeholders
- test JSON examples and command parsing

### F-030 — Release archives omit referenced integration files

Severity: high

The platform release archive bundled the binary and root README, while that
README directed users to adapter and skill files absent from the archive.

Closure:

- bundle the adapters, skill, and required documentation, or
- replace archive-local paths with versioned repository URLs
- inspect every release archive in CI

### F-031 — Runtime logs expose local paths

Severity: medium

Adapter ready messages printed full storage and home paths by default.

Closure:

- print path details only in explicit debug mode
- abbreviate the user home when a path is needed

### F-032 — Documentation and skill syntax contain copy failures

Severity: medium

The skill placed optional-argument brackets in copyable commands, BDD fencing
was malformed, and adapter documentation referenced absent scripts.

Closure:

- keep syntax notation separate from runnable examples
- validate Markdown fences and local links
- remove or provide every referenced script

## 6. Resolution ledger

The findings above record the defects found in the original completed-state
claim. This table records the maintained 0.1.3 candidate disposition; a finding
is not considered closed merely because a nearby test passed.

| Finding | Status | Maintained resolution and evidence surface |
|---|---|---|
| F-001 | resolved | Endpoint-scoped `SessionKey`, connection generations, callback/terminal ownership, and same-id collision tests |
| F-002 | resolved | Provisional parent projection precedes load; queued early updates flush only after binding; failed imports clean up |
| F-003 | resolved | Initialize advertises configured client capabilities; every fs/terminal handler enforces endpoint, session, and capability ownership |
| F-004 | resolved | Agent connection rejects a negotiated protocol version other than ACP v1 |
| F-005 | resolved | Daemon reads continuously, executes bounded concurrent RPCs, and serializes response writes; same-connection cancellation is tested |
| F-006 | resolved | New conversations require an explicit absolute caller cwd; daemon process cwd is never used as a fallback |
| F-007 | resolved | Layer-1 replay refresh uses begin/commit/rollback markers, preserves Layer 2, and recovers interrupted refreshes on reopen |
| F-008 | resolved | Active or cancelling conversations return conflict on deletion |
| F-009 | resolved | Transactional finalization verifies the run's stored conversation owner |
| F-010 | resolved | Registry mutation is serialized through validate/save/swap; referenced proxies and active agents are protected; affected handles are invalidated |
| F-011 | resolved | `agents.json` is documented as startup input; supported live mutation goes through validated Hub RPC |
| F-012 | resolved | Search combines sources before one bounded limit/offset and returns bounded snippets; offset is exposed through CLI/MCP |
| F-013 | resolved | Callback persistence errors are attached to the active load/run and prevent unqualified success |
| F-014 | resolved | Terminal pipes drain concurrently outside the registry lock; terminal ownership/count/output limits and drop cleanup are enforced |
| F-015 | resolved | Store open terminalizes orphaned running/cancelling runs and repairs conversation state |
| F-016 | resolved | CLI and MCP share redacted public endpoint data; environment/header/URL/argument secret regressions are covered |
| F-017 | resolved | Hub home, registry, database, daemon metadata, Unix socket, and Windows named-pipe access are hardened |
| F-018 | resolved | MCP registration accepts complete tagged endpoint config and exposes session listing, cancellation, paging, and accurate tool annotations |
| F-019 | resolved | Send returns current-run output; public reads use server-side row and byte-bounded pages |
| F-020 | resolved | Adapter docs distinguish read-only discovery/replay from vendor-owned writes caused by resume/delete |
| F-021 | resolved | Default probes use synthetic homes, capture raw stderr, sanitize failures, and require two explicit opt-ins for live mutation |
| F-022 | resolved | Grok preserves upstream initialization errors as JSON-RPC errors; fixture regression covers the failure channel |
| F-023 | resolved | Docs/skill use top-level `send`/`search` and automatic import; CLI contract tests reject the invented surface |
| F-024 | resolved | Grok deletion uses the vendor CLI with sanitized success/failure handling; unsupported adapter mutations are explicitly delimited |
| F-025 | resolved | Spec/design/role documents keep Hub core ACP-only and private-store access inside fail-closed vendor adapters |
| F-026 | resolved | Test inventory names real files and derives counts dynamically; real CLI and MCP process smoke tests exist |
| F-027 | resolved | Proxy, cancel, close, replay, pagination, and callback tests now assert their named behavior |
| F-028 | resolved | Durable samples contain placeholders and least-privilege defaults; dated machine evidence was removed |
| F-029 | resolved | PowerShell/POSIX instructions and adapter path placeholders are separated and valid |
| F-030 | resolved | Release staging includes adapters, one archive-normalized `skills/acp-hub` layout, referenced root operator docs, and BUILD_INFO; internal Review Book/Task Plan records are excluded and extracted archives are verified |
| F-031 | resolved | Normal adapter diagnostics are path-free; probes capture vendor stderr and assert private ids/paths are absent |
| F-032 | resolved | Copyable syntax, Markdown fences, and local links are validated; absent script references were removed |

Two adversarial re-audits additionally found and closed issues that cut across
the original ledger:

- stdio, HTTP response/SSE, and WebSocket framing now enforce a 32 MiB ceiling
  before JSON deserialization. Per-leg ledgers limit unconsumed input to 4096
  frames / 32 MiB, inbound callback requests to eight, SSE streams to 64, and
  all partial SSE events to one shared 32 MiB reservation. Every physical
  proxy leg acknowledges a leg-local token, canonical semantic identity, and
  retained-byte reservation; duplicate identities conservatively release the
  smallest matching reservation;
- the legacy public `hub/conv/messages` RPC was removed. The Rust compatibility
  method traverses `hub/conv/messages_page`, so no server request materializes
  an unbounded conversation;
- endpoint replacement/deletion rejects agents with active runs, and callback
  rollback no longer reverses the session/pending lock order;
- workflow actions are pinned to full revisions, workflows default to
  `contents: read`, and only the GitHub Release upload job receives
  `contents: write`.

## 7. Verification matrix

The maintenance is not accepted until these rows pass:

| Surface | Required evidence |
|---|---|
| Rust formatting | `cargo fmt --all -- --check` |
| Rust lint | workspace/all-target clippy with warnings denied |
| Rust tests | workspace tests with locked dependencies |
| Session isolation | two endpoints reuse one session id without crossing data |
| Imported replay | replay captured after parent creation; load failure cleans up |
| Two-layer history | replay refresh preserves every local turn |
| Permissions | disabled fs/terminal callbacks are rejected |
| Cancellation | a second request cancels a long request on one client connection |
| Registry | referenced proxy deletion rejected; concurrent mutation retained |
| Search | combined global pagination and non-empty bounded snippets |
| CLI | process-level help, JSON/NDJSON, redaction, offset, cwd |
| MCP | initialize, tools/list, safe registry output, tool call, cancel |
| Adapters | Node syntax and isolated fixture tests |
| Documentation | no invented commands, local secrets, broken links, or stale test claims |
| Release | archive contains every bundled-document dependency |

## 8. Residual risks and completion rule

The repository can return to a completed state only after:

1. every critical and high finding is resolved or explicitly rejected by a
   pillar-level decision;
2. the verification matrix is green without weak assertions;
3. adapter behavior is verified without modifying a real user session;
4. current documentation describes the shipped implementation;
5. the final Git state and release contents are reviewed independently.

Residual operational boundaries after maintenance:

- registered endpoints and proxies are executable code chosen by the operator;
  framing limits reduce resource abuse but do not make an untrusted executable
  safe. Proxy flow accounting assumes the supported one-input/one-output
  contract. Every current physical leg uses identity-bound acknowledgements;
  a future feature that deliberately drops, duplicates, or injects messages
  must define a new accounting contract rather than bypass this ledger;
- Cursor/Grok private-store parsing is version-sensitive and therefore
  fail-closed; live installed-agent compatibility and destructive vendor
  probes remain explicit operator actions and were intentionally not run
  against user data during this maintenance;
- local verification proves the stable Windows checkout, package contents,
  workflow syntax, and archive simulation. The locked graph rejects Rust 1.90,
  establishing the need for the declared Rust 1.91 floor; an attempted Windows
  1.91 graph check was blocked by an OS access denial while Cargo linked the
  `rustls` build script. The authoritative 1.91 job therefore remains the
  pinned Ubuntu CI job. Linux/macOS and hosted release jobs remain CI evidence
  at the eventual commit/tag, not claims inferred from this worktree;
- publication is a separate state: no crate, tag, release, commit, or push is
  performed by this review.

At the maintained worktree, no critical or high finding is intentionally
accepted as residual risk. Final verification evidence and repository identity
are recorded in the Task Plan.

## 9. Final reconciliation — 2026-07-19

The final repository review rechecked all 32 findings and the cross-cutting
transport, operation-admission, RPC, adapter, and workflow corrections added
after the original ledger:

- Critical: 6 resolved, 0 open;
- High: 19 resolved, 0 open;
- Medium: 7 resolved, 0 open;
- additional actionable Critical/High/Medium findings from the final review:
  none.

The final local matrix passed formatting, warnings-denied Clippy, 185 Rust
tests, Cursor and Grok fixture suites, dependency policy, package dry-run,
isolated installation, daemon-backed registry operations, documentation
integrity, and diff-integrity checks. Three late candidate findings were
rejected after direct verification because the root child is reaped, the
reported CLI option does not exist, and the pinned release action updates an
existing release with overwrite enabled by default.

The Review Book acceptance conditions in section 8 are satisfied for the local
candidate. Hosted Linux/macOS/Rust-1.91/release jobs, live destructive vendor
probes, push, tag, crates.io publication, and GitHub Release remain explicit
operational or publication boundaries rather than unresolved findings.

## 10. Independent release/operator reconciliation — 2026-07-19

Section 9 records an earlier checkpoint. A later live-checkout audit found the
following additional release/operator defects. They were corrected in the
worktree, but that correction is local evidence only: hosted CI, a real tag,
publication, and installed-vendor probes remain outside this review.

| ID | Severity | Confirmed finding | Resolution and evidence |
|---|---|---|---|
| R-REL-001 | High | The release preflight accepted any tagged commit that was an ancestor of `origin/main`, despite claiming to verify the exact tag SHA. A stale main commit could therefore enter the release workflow. | The gate now requires the peeled tag commit, checked-out commit, event SHA, and current `origin/main` HEAD to be identical. Every release checkout explicitly selects the event SHA and disables credential persistence. |
| R-REL-002 | High | Two external action references were 40-character annotated-tag object IDs, not immutable commit IDs. Full hexadecimal length alone had produced a false positive. | The references now use the tags' peeled commit IDs. All unique action references were then checked against the GitHub commit endpoint; each resolved to the exact pinned commit. Workflow-wide permissions remain `contents: read`, with `contents: write` scoped only to the release-upload job. |
| R-REL-003 | Medium | The archive copied the complete `scripts` directory even though the operator contract described verification helpers. This unintentionally included the crate-publish helper, and the Windows archive text scan omitted PowerShell and shell scripts. | Staging and extracted-archive checks now enforce an exact four-file verification-script allowlist. The Windows text scan covers both `.ps1` and `.sh`. Archive simulation confirmed the documented top-level and nested surfaces, local Markdown links, binary version, and adapter syntax. |
| R-REL-004 | High | The new packaged-consumer PowerShell check used `Set-Content -Encoding utf8NoBOM`, which is unavailable in the documented Windows PowerShell 5.1 environment. | File creation now uses .NET UTF-8 without BOM. The complete packaged-consumer check passed under both Windows PowerShell 5.1 and PowerShell 7, including crates.io dependency resolution and compilation against exact ACP SDK 1.2.0. |
| R-DOC-001 | Medium | The skill cheatsheet grouped `--local-only` with `conv list`, `show`, and `close`, and the source-verification scripts were not clearly distinguished from post-install checks. | The flag is now shown only for `conv delete`; README, release, support, contribution, and changelog text distinguish full-source verification, archive provenance, and installed-binary checks. |

The targeted release/operator validation also passed YAML and JSON parsing,
Markdown fence and local-link checks, PowerShell and Bash syntax checks, crate
version/tag checks for `0.2.0`, package-surface listing, durable private-path
scanning, and `git diff --check`. The simulated archive contained only the
documented operator surface and the four source-verification helpers.

This appendix supersedes section 9's statement that the final review found no
additional actionable findings. It does not by itself reinstate an
all-repository completion claim: code/runtime findings and their full test
matrix are owned by the corresponding review lanes, while hosted release and
publication evidence can exist only for a committed and tagged revision.

## 11. Whole-repository final reconciliation — 2026-07-19

After the release/operator appendix, the complete refactored worktree was
reviewed again across public SDK identity, persistence, registry mutation,
session ownership, daemon admission, physical proxy accounting, adapters,
CLI/MCP, documentation, skill, installation, and packaging.

### 11.1 Newly confirmed findings and closure

| ID | Severity | Confirmed finding | Maintained resolution |
|---|---|---|---|
| R-SDK-001 | High | Workspace patches could make local builds pass while the packaged core exposed ACP types incompatible with the declared public SDK line. | Both crates declare the exact crates.io ACP SDK 1.2.0 line. A disposable external consumer compiles against the packaged core and crates.io SDK, proving public Rust type identity without inheriting workspace patches. |
| R-MOD-001 | High | Several production, CLI, MCP, and persistence files had grown beyond the project's proactive split boundary, obscuring ownership and review. | The files were decomposed into focused modules without compatibility facades that hide duplicate implementations. Every production and test Rust file is now below 900 lines. |
| R-STORE-001 | High | Refresh rollback and reopen recovery could delete partially replayed rows without advancing the message-cursor generation. | Rollback and recovery perform deletion and generation advancement in the same transaction; old cursors return the typed stale-cursor error. |
| R-STORE-002 | High | Session import/create ownership was not keyed by the complete external identity, allowing same-session races or unnecessarily blocking independent agents. | Tokenized RAII ownership is keyed by `(agent_id, agent_session_id)` and covers discovery, caller-supplied creation, persistence, failure cleanup, and lock-map pruning. |
| R-REG-001 | High | A registry save followed by reload-verification failure could leave disk, epoch, handles, and in-memory state describing different versions. | The confirmed disk commit invalidates affected handles and cache state atomically from the caller's perspective; ambiguous save/load failure is fail-closed and covered by fault-injection tests. |
| R-RES-001 | High | A request could reserve the entire global daemon byte budget before reading its frame, starving the response needed to release capacity. | The 128 MiB retained budget is partitioned into 87 MiB request, 40 MiB ordinary response, and 1 MiB terminal/fallback pools. Request admission is progressive and charged once for exact retained bytes. |
| R-DAEMON-001 | High | When daemon discovery observed the singleton lock as busy and that owner exited before metadata connection, the client polled stale state for the complete startup timeout instead of taking over the released lock. Slow macOS CI exposed this as repeated 15-second CLI failures. | Contending clients now poll both metadata and singleton-lock ownership. After the prior owner exits, one client removes stale state, spawns the replacement daemon, and connects within the remaining original timeout. |
| R-PROXY-001 | High | Logical FIFO completion could acknowledge the wrong physical proxy message after id remapping or reordering, and duplicate identities could undercount retained bytes. | Each leg records a monotonic token, canonical semantic identity, and bytes. Notifications bind method/params; responses bind result/error. A missing match fails explicitly, and ambiguous duplicates release the smallest reservation. |
| R-RPC-001 | High | An SDK notification-handler error is represented as an error response with null id; the outgoing stdio path mistook it for completion of a request and tore down the connection. | Uncorrelated null-id protocol errors no longer complete a request. A real reserved null-id request still completes exactly, and other unmatched responses remain protocol failures. |
| R-ADAPTER-001 | High | Cursor and Grok adapters could produce false success, lose prompt blocks, expose private stderr/path data, or leave child processes alive on malformed output. | Adapters now fail closed on unsupported/malformed streams, emit one terminal result, use prompt files outside argv, bound stdout, sanitize errors, and terminate/reap the process tree. |

### 11.2 Current verification result

The final local code matrix passed:

- formatting and warnings-denied workspace Clippy;
- 218 Rust tests, with 5 fixture-dependent tests deliberately ignored;
- Cursor: 28 fixture checks passed, with 1 live-vendor mutation skipped;
- Grok: 36 fixture checks passed, with 1 live-vendor mutation skipped;
- `cargo deny --all-features check`;
- packaged external-consumer compilation against ACP SDK 1.2.0;
- isolated `cargo install` plus daemon-backed add/list/inspect/remove with
  redacted public output;
- workflow, adapter JSON, script syntax, Markdown structure/link, package, and
  diff-integrity checks.

No confirmed Critical, High, or Medium code, documentation, skill,
installation, or local release-preparation finding remains in this maintained
worktree. This statement does not claim a clean or published Git state.

### 11.3 Explicit external boundaries

- Live Cursor/Grok destructive or prompt probes were not run against
  vendor-owned user sessions.
- Registered endpoints and proxies remain operator-selected executables, not
  sandboxed untrusted code.
- Hosted Linux/macOS/Rust-1.91/release jobs have not run for this dirty
  candidate.
- Avira quarantines the generated `rustls` build-script executable under the
  generic `TR/W64.MalwareX` heuristic, which causes Cargo's Windows Rust 1.91
  link/copy step to return `Access is denied (os error 5)`. No antivirus
  disablement or permanent exclusion is accepted as release evidence; the
  isolated Ubuntu Rust 1.91 hosted job is the clean authoritative MSRV path.
- No stage, new commit, push, tag, crates.io publication, or GitHub Release was
  performed by this final reconciliation.
