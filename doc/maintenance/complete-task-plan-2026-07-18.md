# ACP Hub Complete Maintenance Task Plan

Date: 2026-07-18

> **Reconciliation notice (2026-07-19):** a fresh live-checkout review after
> the prior completion commit found additional actionable module,
> registry/store, resource-budget, SDK, adapter, MCP and release findings.
> Historical task states below are evidence for the first pass. The final
> reconciliation appendix is the current task state.

This plan implements the findings in
`doc/review/complete-review-book-2026-07-18.md`. Status values are:

- `planned`
- `in progress`
- `verified`
- `blocked`

## Phase 0 — Baseline and control documents

### T-000 — Preserve the live working tree

Status: verified

Actions:

- record branch, HEAD, live remote main, and dirty state
- preserve existing adapter and Codex documentation changes
- do not reset, discard, or silently replace user work

Acceptance:

- the final diff contains the original semantic changes or an intentional
  reviewed replacement
- unrelated line-ending status is not mistaken for semantic modification

### T-001 — Create the Review Book and Task Plan

Status: verified

Actions:

- record all repository-wide findings with impact and closure conditions
- define a verification matrix
- map implementation work to bounded phases

Acceptance:

- findings cover code, persistence, CLI/MCP, adapters, documentation, skill,
  installation, and release

## Phase 1 — Protocol, callbacks, and daemon

### T-100 — Scope every ACP session to its endpoint

Status: verified

Findings: F-001

Actions:

- introduce an endpoint-scoped session key
- carry endpoint identity through callback dispatch
- migrate binding, load, and run maps
- add a collision regression test

Acceptance:

- two endpoints can use the same session id without sharing any callback state

### T-101 — Enforce advertised callback capabilities

Status: verified

Findings: F-003, F-004

Actions:

- build initialization capabilities from registry configuration
- validate negotiated ACP protocol version
- reject disabled filesystem and terminal callbacks
- scope terminal resources to their endpoint/session owner

Acceptance:

- disabled callbacks return protocol errors and perform no local action

### T-102 — Remove terminal I/O deadlocks

Status: verified

Findings: F-014

Actions:

- move pipe reads and process waits outside terminal-map locks
- drain stdout and stderr concurrently
- release child resources during shutdown

Acceptance:

- a fixture that fills both pipes completes and remains cancellable

### T-103 — Make daemon request handling concurrent

Status: verified

Findings: F-005

Actions:

- separate connection reading from request execution
- serialize response writes
- retain request ids and orderly EOF behavior
- account for active work

Acceptance:

- one connection can cancel its own long send

### T-104 — Surface capture failures

Status: verified

Findings: F-013

Actions:

- associate callback persistence failures with an active load/run
- fail or report partial capture in the public result

Acceptance:

- a forced store error cannot produce an unqualified success

## Phase 2 — Store, conversations, registry, and search

### T-200 — Make imported replay transactional

Status: verified

Findings: F-002, F-007

Actions:

- insert the parent projection before invoking load
- stage original replay independently
- replace only the original replay layer
- clean up failed provisional imports

Acceptance:

- replay is present exactly once and every local turn remains current

### T-201 — Protect active conversation state

Status: verified

Findings: F-008, F-009, F-015

Actions:

- reject deletion during an active run
- validate run/conversation ownership during finalization
- recover interrupted states at startup

Acceptance:

- concurrent deletion cannot lose an active turn
- reopening a crashed database has no ghost running conversation

### T-202 — Remove daemon cwd inheritance

Status: verified

Findings: F-006

Actions:

- resolve caller cwd in CLI/MCP/library requests
- canonicalize and validate it
- reject unresolved defaults in daemon-side creation

Acceptance:

- conversations created from two clients use their respective directories

### T-203 — Serialize and validate registry mutation

Status: verified

Findings: F-010, F-011, F-017

Actions:

- reject referenced proxy removal
- hold one mutation lock through validate/save/swap
- use safe atomic replacement
- invalidate affected live handles
- enforce state-file permissions
- document registry reload behavior

Acceptance:

- concurrent distinct mutations are both retained
- every persisted registry reloads successfully

### T-204 — Correct search and message pagination

Status: verified

Findings: F-012

Actions:

- bound limit and offset
- combine all result kinds before pagination
- produce bounded snippets
- expose offset through every public interface

Acceptance:

- pages contain no duplicate title hits and never exceed their limit

## Phase 3 — CLI and MCP

### T-300 — Use one safe public registry representation

Status: verified

Findings: F-016

Actions:

- redact all environment and header values
- sanitize URLs and command arguments
- use the same representation in CLI and MCP
- add regression tests for authorization, cookies, tokens, and URL credentials

Acceptance:

- ordinary CLI/MCP output contains no configured secret value

### T-301 — Complete the MCP management surface

Status: verified

Findings: F-018, F-019

Actions:

- accept tagged transport config including headers
- accept proxy chain, permission policy, and client capabilities
- preserve MCP servers during conversation creation
- add session listing and cancellation
- return current-run messages rather than unbounded history
- paginate message reads

Acceptance:

- documented CLI-equivalent operations are possible through MCP or explicitly
  identified as intentionally different

### T-302 — Correct CLI cwd, pagination, and output contracts

Status: verified

Findings: F-006, F-012, F-023

Actions:

- send a caller-resolved cwd for conversation creation
- expose search offset
- label streaming JSON output as NDJSON in help and documentation
- strip control characters from table cells

Acceptance:

- help text and process tests match actual output

### T-303 — Add real CLI and MCP smoke tests

Status: verified

Findings: F-026, F-027

Actions:

- add process-level CLI parsing/redaction tests
- run MCP initialize, tools/list, and representative tool calls over stdio
- make cancellation and proxy tests assert the named behavior

Acceptance:

- removing a public command, redaction, or MCP handler fails CI

## Phase 4 — Adapters, documentation, and skill

### T-400 — Correct adapter storage and permission claims

Status: verified

Findings: F-020, F-024, F-025

Actions:

- document discovery reads separately from resumed-session writes
- keep private storage access in vendor adapters
- define version and failure boundaries
- implement or accurately delimit vendor-session deletion

Acceptance:

- no document promises that resumed vendor sessions leave native storage
  unchanged

### T-401 — Isolate adapter tests and error propagation

Status: verified

Findings: F-021, F-022

Actions:

- require isolated fixture homes by default
- remove private message logging
- require explicit opt-in for destructive installed-agent tests
- preserve upstream JSON-RPC initialization errors

Acceptance:

- default adapter tests cannot read or modify a user's real sessions

### T-402 — Remove invented commands and local evidence

Status: verified

Findings: F-023, F-026, F-028, F-029, F-031, F-032

Actions:

- use top-level `send` and `search`
- remove `--import`
- separate PowerShell and POSIX examples
- replace local versions, branches, counts, paths, and markers with reproducible
  conditions
- repair Markdown and local references
- hide absolute paths outside debug mode

Acceptance:

- documented commands parse against current `--help`
- a sensitive/local-information scan contains only explicit placeholders

### T-403 — Align spec, design, BDD, TDD, plan, and role

Status: verified

Findings: F-025, F-026

Actions:

- apply the system boundary from the Review Book
- replace stale implementation status and test claims
- map BDD/TDD rows to real tests

Acceptance:

- the five development documents and implementer role describe the same
  boundaries and current verification state

### T-404 — Make the skill safe and executable

Status: verified

Findings: F-016, F-023, F-032

Actions:

- use only real commands
- keep syntax notation separate from copyable commands
- default to isolated Hub homes and least privilege
- describe secret-file and daemon handling accurately

Acceptance:

- every golden-path command is directly executable after replacing placeholders

## Phase 5 — Installation and release

### T-500 — Repair sample registry and installation instructions

Status: verified

Findings: F-017, F-028, F-029

Actions:

- make sample permissions deny-by-default
- use valid cross-platform path examples
- document state directory permissions
- validate sample JSON

Acceptance:

- sample configuration parses and does not grant terminal or write access by
  default

### T-501 — Include referenced integration material in releases

Status: verified

Findings: F-030

Actions:

- include adapters, skill, and required docs in platform archives
- add archive-content verification

Acceptance:

- every bundled README path resolves inside the archive or to a versioned URL

## Phase 6 — Final verification

### T-600 — Run the complete verification matrix

Status: verified

Actions:

- formatting, lint, workspace tests
- CLI and MCP process smoke
- Node syntax and isolated adapter tests
- sample JSON, Markdown, link, and sensitive-information checks
- package and release-archive content inspection

Acceptance:

- every Review Book verification row has current evidence

### T-601 — Independent adversarial review

Status: verified

Actions:

- review the final diff from protocol, security, persistence, documentation, and
  operator perspectives
- rework every confirmed finding

Acceptance:

- no unresolved critical or high finding remains

### T-602 — Final repository-state report

Status: verified

Actions:

- list changed files and retained pre-existing work
- report tests and intentionally unrun destructive checks
- report local HEAD, live remote main, and dirty state

Acceptance:

- completion, verification, publication, and worktree state are reported
  separately

## Execution record

This section is the evidence behind the `verified` statuses above. It separates
local verification from hosted or intentionally unrun work; a configured
future CI job is not reported as if it already ran on this worktree.

### Repository identity and preservation

Recorded after `git fetch origin main`:

| Item | Value |
|---|---|
| Repository | `RatmmmhSquishyRat/acp-hub` |
| Branch | `codex/resolve-review-feedback` |
| Local HEAD | `ebd544e1a15c4470633e900bf92342b2f86c66bf` |
| Live `origin/main` | `af859b8dccbb9664f3917c3ae4219ea1e1d75125` |
| Worktree | dirty by design; 52 tracked paths changed and 7 untracked paths/directories |
| Publication | no commit, stage, push, tag, crate upload, or GitHub Release |

The pre-existing `adapters/codex/agents.json`,
`adapters/codex/README.md`, and unrelated line-ending state were retained and
reviewed in place. No reset or discard operation was used.

### Local verification evidence

| Surface | Command/evidence | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Lint | workspace, all targets/features, locked Clippy with `-D warnings` | pass |
| Rust tests | `cargo test --workspace --all-targets --locked -- --test-threads=1` | 81 passed, 0 failed |
| CLI process contract | unit plus `cli_contract.rs` | 12 passed |
| MCP process smoke | real stdio initialize/list/call flow | 1 passed |
| Callback ACP round trip | permission, filesystem, terminal over one ACP connection | 1 passed |
| Transport budgets | framing, outstanding ledger, callback amplification, SSE partial bytes, proxy FIFO | 9 passed |
| Cursor adapter | Node syntax plus isolated fixture | 10 passed, 1 explicit vendor-write skip |
| Grok adapter | Node syntax, privacy/errors/delete, shutdown cleanup | 16 passed, 1 explicit vendor-write skip |
| Registry samples | four `agents.json` files parsed | pass |
| Workflow syntax | both workflow YAML files parsed | pass |
| Workflow supply chain | every external `uses:` reference checked | 28 full-SHA pins; least-privilege permissions confirmed |
| Documentation | 26 Markdown files, 570 fence markers, 16 local links | pass |
| Sensitive strings | durable tree scan | only synthetic test values and example paths found |
| Dependency policy | `cargo deny check` | advisories/bans/licenses/sources pass; duplicate/unmatched-allowance warnings retained |
| Release build | locked release workspace build and `acp-hub --version` | pass, version `0.1.3` |
| Core package | `cargo publish -p acp-hub-core --dry-run --allow-dirty --locked` | pass; 23 packaged files |
| CLI package surface | `cargo package -p acp-hub-cli --list --allow-dirty --locked` | pass |
| Windows archive | stage, ZIP, extract, required-file checks, binary/adapter smoke | pass |
| Diff integrity | `git diff --check` | pass; only configured CRLF conversion notices |

The lockfile does not compile on Rust 1.90 because
`libsqlite3-sys 0.38.1` uses the stabilized `cfg_select!` surface; therefore the
declared floor is Rust 1.91. A separate-target Windows
`cargo +1.91.0 check --workspace --all-targets --locked` was attempted three
times. Cargo was stopped only after the compiler repeatedly returned
`Access is denied (os error 5)` while copying the `rustls` build-script
executable. Temporary MSRV target directories and their compiler processes were
then removed. The pinned Ubuntu `Rust 1.91 MSRV` workflow is the remaining
authoritative cross-platform check, not a locally claimed pass.

### Hosted or intentionally unrun evidence

| State | Item | Reason |
|---|---|---|
| hosted CI required | Ubuntu Rust 1.91, Linux tests, macOS tests, four release targets | Requires a committed GitHub revision and hosted runners |
| intentionally not run | live Cursor/Grok prompt, resume, or destructive delete probes | Would read or mutate real vendor-owned user sessions |
| intentionally not run | `cargo publish -p acp-hub-cli --dry-run` before core publication | The exact `acp-hub-core 0.1.3` dependency is not yet in the crates.io index; release publishes core first |
| not authorized/performed | commit, push, tag, crates.io publish, GitHub Release | Publication is a separate user-controlled state |

### Independent review outcome

Three independent review lanes covered protocol/runtime, store/registry, and
documentation/release. Re-audits found and drove fixes for unbounded SDK-facing
transport queues, callback response amplification, cross-SSE partial buffers,
proxy-leg accounting, adapter false-greens/privacy, and workflow supply-chain
permissions. The final protocol re-audit found no remaining Critical or High
issue under the documented operator-selected, one-to-one proxy trust contract.

## Final reconciliation — 2026-07-19

All 26 task IDs, T-000 through T-602, are `verified`. There are no `planned`,
`in progress`, `blocked`, or partially accepted tasks.

The execution-record counts above describe an earlier checkpoint. The final
re-run produced:

- 185 Rust tests passed across 17 suites; 5 deliberately ignored;
- Cursor adapter: 20 passed and one deliberate live-write skip;
- Grok adapter: 23 passed and one deliberate live-write skip;
- `acp-hub-core` publish dry-run: 37 files, 763.6 KiB
  (145.9 KiB compressed);
- CLI package list: 11 paths;
- isolated install plus daemon-backed agent add/list/inspect/remove: pass.

The complete task-by-task, finding-by-finding, and initial-todo comparison is in
`final-completion-summary-2026-07-19.md`. The user authorized a local commit of
this maintained candidate. Push, tag, hosted CI/release jobs, crates.io
publication, and GitHub Release remain separate external actions.

## Release/operator audit addendum — 2026-07-19

The preceding final-reconciliation paragraph is an earlier checkpoint, not the
current all-repository completion state. A later audit reopened the
release/operator lane and completed these additional tasks:

| ID | Task | State | Current evidence |
|---|---|---|---|
| T-503 | Bind a release tag to one exact source revision and immutable action commits | verified locally | The peeled tag, checkout, event SHA, and current `origin/main` HEAD must match. Every external action reference is a live-verified commit ID; checkout credentials are not persisted. |
| T-504 | Make the packaged-consumer and archive contract portable and allowlisted | verified locally | Both PowerShell 5.1 and PowerShell 7 packaged-consumer runs passed. Bash and PowerShell syntax passed. Archive simulation enforced the exact four verification helpers and resolved bundled-document links. |
| T-505 | Reconcile root operator documentation and the bundled skill with the live CLI/release surface | verified locally | Version is consistently `0.2.0`; the skill limits `--local-only` to conversation deletion; documentation distinguishes full-source, archive, and installed-binary checks; durable private-path scanning found no local machine data in this lane. |

The earlier execution-record SHAs, dirty-path counts, version `0.1.3`, package
counts, test counts, and statements that all tasks were complete are historical
observations. They must not be used as evidence for the current refactored
worktree. Current release/operator verification is recorded in Review Book
section 10. Hosted CI, real tag dispatch, crates.io publication, GitHub Release,
and live vendor-owned data probes remain unexecuted external states.

## Whole-repository final reconciliation — 2026-07-19

This appendix supersedes earlier completion counts and closes the tasks opened
by the live refactor review.

| ID | Task | State | Current evidence |
|---|---|---|---|
| T-603 | Verify the official ACP SDK upgrade and packaged public type identity | verified locally | Both crates use exact ACP SDK 1.2.0. A disposable external consumer compiled against the packaged core and crates.io SDK without inheriting the repository patch. |
| T-604 | Close store, registry, cursor, and session-identity races | verified locally | Transactional cursor-generation advancement, keyed session ownership, handle/cache invalidation, fault-injection, rollback, recovery, and concurrent-identity tests pass. |
| T-605 | Close daemon retained-byte admission and physical proxy accounting | verified locally | Fixed 87/40/1 MiB partitions, progressive exact request admission, canonical per-leg identity/token/byte ACK, reordering, duplicate-identity, mismatch, and saturation coverage pass. |
| T-606 | Reconcile adapters, CLI/MCP, skill, workflows, archive, and installation surfaces | verified locally | Cursor 28 and Grok 44 fixture checks pass with one deliberate live skip each; schemas fail closed; output is redacted; version/package/archive checks, the external packaged consumer, and isolated daemon-backed install smoke pass. |
| T-607 | Execute the final integrated worktree verification and record honest boundaries | verified locally | Format, warnings-denied Clippy, 239 Rust tests with 5 ignored, dependency policy, syntax/link/JSON checks, package checks, and diff integrity pass. Avira quarantines the generated `rustls` build-script executable as generic `TR/W64.MalwareX`; the clean Ubuntu Rust 1.91 hosted job is the authoritative MSRV path. Publication and real vendor-owned probes remain separate states. |
| T-608 | Close daemon singleton-exit discovery race | verified locally | The previous lock owner may exit between metadata lookup and connection. Discovery now competes for the released lock, removes stale state, spawns one replacement, and keeps the original startup deadline. Focused CLI contract, daemon lifecycle, format, and warnings-denied Clippy pass locally. |
| T-609 | Restore portable owner-only daemon socket startup on macOS | verified locally; hosted rerun required | Hosted diagnostics proved that Interprocess returns `Unsupported` for atomic listener mode on macOS. The fallback now binds only inside a pre-hardened `0700` directory and immediately sets the socket to `0600`; startup stderr can be inherited explicitly for future diagnosis. Local format, warnings-denied Clippy, and CLI contract checks pass. |
| T-610 | Address all actionable PR #29 review findings | verified locally; integrated hosted rerun required | Eight current threads were traced to code and fixed: exact packaged ACP requirements, pre-business daemon handshake, public root redaction, caller-recoverable MCP cursors, per-operation capture budgets, newline-complete progressive stdio admission, generation-held and owner-aware local `session/new` quarantine/publication/rollback, and Grok deletion-tree shutdown ownership with prompt/delete mutual exclusion. Focused core/CLI/adapter/package checks pass; the combined head still requires the full hosted matrix. |

All task IDs through T-610 are verified for the maintained local worktree. No
task is `planned`, `in progress`, or `blocked`. “Verified locally” is not
synonymous with hosted CI, published crates, a GitHub Release, or a clean Git
worktree.

## Pull-request re-review and publication addendum — 2026-07-19

This addendum supersedes the completion boundary immediately above.

| ID | Task | State | Current evidence |
|---|---|---|---|
| T-611 | Close the second PR #29 review pass | verified locally; hosted rerun required | Five additional findings are closed: bounded Grok shutdown/reaping, live-delete tombstones, connection-fatal daemon notification gaps, persisted-before-notify cancellation with fail-closed rollback, and ownership-first terminal retirement. Three independent re-reviews approved the result. Final local gates pass: format, strict Clippy, 244 Rust tests/5 ignored fixtures, Cursor 28/1 skip, Grok 47/1 skip, dependency policy, exact packaged consumer, CLI package list, privacy/secret/file-size scans, and diff integrity. |
| T-612 | Publish the exact reviewed `0.2.0` source | in progress | Required order: commit and push the candidate, require the complete hosted CI matrix, merge PR #29, tag that exact current `main` commit as `v0.2.0`, then verify both crates.io packages and every GitHub Release artifact/checksum. Superseded dependency PRs close only after merge. |

T-612 is an external state transition, not evidence that the code review is
incomplete. Its final state must be appended only from live GitHub and
crates.io evidence.
