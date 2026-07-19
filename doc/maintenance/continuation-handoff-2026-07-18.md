# ACP Hub Repository Review and Maintenance Continuation Handoff

Date: 2026-07-18
Status: **historical handoff; reopened on 2026-07-19; not current completion evidence**

> The status originally written here was “local maintenance complete and
> verified.” A later live-checkout review found additional actionable work in
> repository-wide module boundaries, registry/store atomicity, resource
> admission, SDK alignment, adapters, MCP continuation, and release gates.
> Preserve the body as historical evidence, but use the latest reconciliation
> appendices in the Review Book and Task Plan for current status.

## 1. Authority and reading rule

This file is the current operational handoff for the interrupted repository-wide
review and maintenance pass.

The user has explicitly corrected the prior completion claim: the work is only
partially complete. Therefore:

1. project pillars under `doc/ssot/` remain the highest project authority;
2. this file supersedes completion and progress labels in
   `complete-task-plan-2026-07-18.md`;
3. `complete-review-book-2026-07-18.md` remains a useful defect inventory, but
   its `resolved` labels mean only that a candidate fix exists;
4. local green tests are evidence for a build state, not final repository
   acceptance;
5. no next agent may report completion until the closeout gates in section 13
   have been satisfied against the final diff.

## 2. Current objective

The objective is to bring `RatmmmhSquishyRat/acp-hub` from a previously claimed
finished state to an evidence-backed, maintainable release candidate.

The work covers:

1. review the complete live repository rather than inheriting earlier agent
   assumptions;
2. locate incorrect product assumptions, fake-green tests, private/local
   machine data, unsafe defaults, stale documentation, and incomplete release
   surfaces;
3. repair protocol, daemon, persistence, registry, CLI, MCP, adapters, tests,
   skill instructions, installation instructions, CI, and release packaging;
4. keep the Hub core ACP-oriented while isolating unavoidable vendor-private
   storage behavior inside vendor adapters;
5. prove behavior with bounded, observable tests and package inspection;
6. keep implementation verification, independent review, hosted CI,
   installation verification, and publication as separate states.

The objective does not authorize:

- destructive tests against real Cursor or Grok sessions;
- stopping or killing a user's persistent ACP Hub daemon;
- staging, committing, rebasing, pushing, tagging, publishing crates, or
  creating a GitHub Release without an explicit continuation decision.

## 3. Repository identity and Git state

Snapshot taken after a live `git fetch origin main`:

| Item | Current value |
|---|---|
| Repository | `https://github.com/RatmmmhSquishyRat/acp-hub.git` |
| Branch | `codex/resolve-review-feedback` |
| Local HEAD | `ebd544e1a15c4470633e900bf92342b2f86c66bf` |
| Live `origin/main` | `af859b8dccbb9664f3917c3ae4219ea1e1d75125` |
| Merge base | `cebef648442152ad2f8220265ca41c7a6286236c` |
| Divergence | local branch 3 commits ahead and 7 commits behind `origin/main` |
| Tracked worktree changes | 52 files |
| Untracked files | 8 files, including this handoff |
| Staged changes | none |
| Publication state | no commit, push, tag, crate upload, or GitHub Release |
| Relevant live processes | no `cargo`, `rustc`, or `acp-hub` process found at handoff |

The local and remote histories contain patch/merge equivalents with different
commit ids. Do not infer that a normal rebase is conflict-free from matching
commit subjects. Before any history operation:

1. preserve the complete dirty worktree;
2. inspect `git log --left-right HEAD...origin/main`;
3. separate already-merged semantic changes from the current uncommitted
   maintenance diff;
4. decide whether the continuation should rebase, merge, or start from a fresh
   branch and reapply the reviewed patch.

No reset, checkout-discard, clean, or destructive history operation was used in
the interrupted pass.

## 4. Project map

| Area | Main paths | Responsibility |
|---|---|---|
| Project laws | `doc/ssot/pillars/`, `doc/ssot/dev-principles/` | Product and implementation authority |
| Hub core | `crates/hub/src/` | ACP endpoints, callbacks, daemon, registry, persistence, transports |
| Public CLI/MCP | `crates/cli/src/`, `crates/cli/tests/` | User commands, JSON/NDJSON, MCP tools, process contracts |
| End-to-end tests | `crates/integration-tests/` | Protocol, proxy, callback, cancellation, concurrency, full flow |
| Vendor adapters | `adapters/cursor/`, `adapters/grok/`, `adapters/codex/`, `adapters/omp/` | Vendor discovery/replay/resume bridges and samples |
| Skill | `.grok/skills/acp-hub/` | Agent-facing operational instructions |
| Development docs | `doc/dev/`, `doc/roles/` | Spec, design, BDD, TDD, implementation and role boundaries |
| Installation/release | `README.md`, `RELEASING.md`, `.github/workflows/` | Build, package, install, publish, cross-platform checks |
| Review control | `doc/review/`, `doc/maintenance/` | Findings, attempted plan, current handoff |

## 5. Worktree change inventory

The current patch is large: the tracked diff is approximately 7,098 insertions
and 2,107 deletions across 52 files. The eight untracked files are:

- `adapters/codex/README.md`
- `crates/cli/tests/cli_contract.rs`
- `crates/cli/tests/mcp_smoke.rs`
- `crates/hub/src/bounded_transport.rs`
- `crates/integration-tests/tests/callback_roundtrip.rs`
- `doc/maintenance/complete-task-plan-2026-07-18.md`
- `doc/maintenance/continuation-handoff-2026-07-18.md`
- `doc/review/complete-review-book-2026-07-18.md`

Changed-path distribution, including untracked files:

| Area | Files |
|---|---:|
| `.github` | 2 |
| `.grok` | 2 |
| `adapters` | 11 |
| `crates` | 27 |
| `doc` | 11 after adding this handoff |
| root Cargo/config/docs | 7 |

`adapters/codex/agents.json`, `adapters/codex/README.md`, and some line-ending
state were reported as pre-existing user work when maintenance began. They must
not be discarded or silently attributed to this review.

## 6. Review Book status

The Review Book contains 32 findings. It is the most complete issue inventory
produced so far. Candidate fixes exist for every row, but no row should be
treated as finally closed until another agent reviews the final implementation
and its test actually proves the closure condition.

### 6.1 Protocol, identity, and resource safety

| Finding | Original risk | Candidate implementation surface | Handoff state |
|---|---|---|---|
| F-001 critical | ACP session ids were not endpoint-scoped | `callbacks.rs`, `endpoint.rs`, `hub.rs`, collision tests | Candidate fix exists; independent audit required |
| F-003 critical | fs/terminal capabilities and ownership were not enforced | `acp.rs`, `callbacks.rs`, callback roundtrip test | Candidate fix exists; independent audit required |
| F-004 high | negotiated ACP version was not validated | `acp.rs`, protocol tests | Candidate fix exists; independent audit required |
| F-005 high | one long RPC blocked cancellation on the same connection | `daemon.rs`, concurrency tests | Candidate fix exists; independent audit required |
| F-013 high | callback persistence errors could still appear successful | `callbacks.rs`, `hub.rs` | Candidate fix exists; fault-path audit required |
| F-014 high | terminal I/O and lock ordering could deadlock | `callbacks.rs`, terminal tests | Candidate fix exists; concurrency audit required |

Additional fixes were added after adversarial review in
`bounded_transport.rs`: 32 MiB frame ceilings, bounded outstanding ledgers,
bounded inbound callback requests, bounded SSE streams and partial-event bytes,
and proxy-flow acknowledgement.

The current proxy accounting assumes an operator-selected, trusted,
one-input/one-output proxy that preserves order. The latest candidate uses
strict FIFO acknowledgement. Any future proxy that may drop, duplicate,
inject, or reorder messages invalidates this model and needs per-message flow
tokens.

### 6.2 Persistence, replay, registry, and search

| Finding | Original risk | Candidate implementation surface | Handoff state |
|---|---|---|---|
| F-002 critical | imported replay could arrive before its parent conversation | `hub.rs`, `store.rs`, replay tests | Candidate fix exists; rollback/failure paths need audit |
| F-007 critical | original replay and Hub-captured turns were not independent | `store.rs`, store tests | Candidate two-layer model exists; recovery audit required |
| F-008 high | active conversations could be deleted | `hub.rs`, concurrency tests | Candidate conflict guard exists |
| F-009 high | run finalization did not validate conversation ownership | `store.rs`, store tests | Candidate transactional validation exists |
| F-010 high | registry mutation could lose or persist invalid state | `hub.rs`, registry tests | Candidate serialized mutation exists |
| F-011 medium | external registry edits were promised but not observed | docs and startup behavior | Candidate decision is startup input plus RPC mutation |
| F-012 high | search pagination combined sources incorrectly | `store.rs`, CLI/MCP surfaces | Candidate global pagination exists |
| F-015 medium | crash recovery left ghost running state | `store.rs`, reopen tests | Candidate startup recovery exists |
| F-017 high | sensitive state-file permissions were ambient | `daemon.rs`, state setup | Candidate Unix/Windows hardening exists; platform audit required |

A late store re-audit found a proxy/transport FIFO accounting defect where an
acknowledgement could search past the oldest outstanding frame. The code and a
regression test were changed so an out-of-order logical acknowledgement cannot
free later bytes. This was tested locally, but the final diff still needs an
independent inspection.

### 6.3 CLI, MCP, privacy, and public contracts

| Finding | Original risk | Candidate implementation surface | Handoff state |
|---|---|---|---|
| F-006 high | conversation cwd could inherit the daemon startup directory | `main.rs`, `mcp.rs`, `hub.rs` | Candidate explicit caller cwd exists |
| F-016 critical | CLI/MCP registry output could disclose credentials | `main.rs`, `mcp.rs`, `endpoint.rs` | Candidate shared redaction exists; adversarial secret cases need review |
| F-018 high | MCP management surface was incomplete | `mcp.rs`, MCP smoke test | Candidate registration/session/cancel tools exist |
| F-019 medium | MCP returned unbounded conversation history | paged Hub APIs and `mcp.rs` | Candidate bounded paging exists |
| F-023 high | docs/adapters used invented CLI commands | CLI help, docs, skill | Candidate docs use top-level `send` and `search` |
| F-026 high | plans and test claims were stale | dev docs and process tests | Candidate alignment exists; current handoff corrects overstated closure |
| F-027 high | integration tests contained false-green assertions | integration and CLI tests | Candidate stronger assertions exist; test semantics need re-read |

The legacy public `hub/conv/messages` RPC was removed. The Rust compatibility
method is intended to traverse bounded `hub/conv/messages_page` calls. The next
review must confirm that no public request still materializes unbounded history
inside the daemon.

### 6.4 Adapters, documentation, and skill

| Finding | Original risk | Candidate implementation surface | Handoff state |
|---|---|---|---|
| F-020 high | read-only claims ignored vendor writes caused by resume/delete | Cursor/Grok specs and READMEs | Candidate wording distinguishes discovery from mutation |
| F-021 critical | adapter tests could mutate and print private sessions | adapter test scripts | Fixture-default candidate exists; live probes intentionally skipped |
| F-022 high | Grok initialization could wrap upstream error as success | `adapters/grok/adapter.mjs` | Candidate error propagation exists |
| F-024 high | adapter CRUD behavior was incomplete/inaccurate | Grok adapter and docs | Candidate deletion boundary exists; installed-agent behavior unverified |
| F-025 high | specs disagreed about private storage | `doc/dev/`, `doc/roles/` | Candidate boundary alignment exists |
| F-028 medium | examples included local evidence and unsafe defaults | adapter samples/docs | Candidate placeholders and least privilege exist |
| F-029 medium | paths and shell examples were not portable | adapter docs and samples | Candidate POSIX/PowerShell separation exists |
| F-031 medium | runtime logs exposed local paths | adapter diagnostics | Candidate path-free default logs exist |
| F-032 medium | skill/docs contained non-runnable syntax and broken references | `.grok/skills/acp-hub/`, docs | Candidate repair exists; packaged-copy audit pending |

No live Cursor/Grok prompt, resume, or destructive deletion probe was run. That
was intentional to avoid reading or mutating user-owned vendor sessions. It
also means installed-version compatibility is not accepted.

### 6.5 Installation, CI, and release

| Finding | Original risk | Candidate implementation surface | Handoff state |
|---|---|---|---|
| F-030 high | release archives omitted files referenced by bundled docs | `release.yml`, local archive simulation | Windows candidate archive passed locally; hosted matrix not run |
| F-017/F-028/F-029 | installation defaults, paths, and state permissions | README, registry samples, adapter docs | Candidate docs/samples exist; clean-machine install not run |
| Supply-chain follow-up | mutable action tags and excessive permissions | both workflows | Candidate full-SHA pins and default `contents: read` exist |
| MSRV follow-up | locked dependency graph no longer supports prior floor | root Cargo config and CI | Candidate floor is Rust 1.91; Windows local check blocked |

The local archive still exists under ignored build output:

`target/release-package-check/acp-hub-v0.1.3-x86_64-pc-windows-msvc.zip`

It is evidence for one Windows staging simulation only. It is not a signed,
published, or cross-platform release artifact.

## 7. Work completed or materially advanced

The following implementation work is present in the dirty worktree:

1. endpoint-scoped callback, session, run, and terminal ownership;
2. generation-scoped endpoint connection state;
3. advertised and enforced filesystem/terminal capabilities;
4. bounded stdio, HTTP/SSE, WebSocket, callback, message, and search surfaces;
5. concurrent daemon request processing and bounded RPC frames;
6. transactional/two-layer replay and crash-state recovery;
7. active-run guards for conversation deletion and agent mutation;
8. serialized registry validation/save/swap behavior;
9. CLI/MCP paging, cwd propagation, management tools, and secret redaction;
10. isolated adapter fixtures, stricter malformed-data handling, privacy
    sanitation, and Grok child/temp cleanup;
11. corrected ACP Hub command names and safer skill examples;
12. Rust 1.91 and Node 22.13 candidate toolchain declarations;
13. pinned workflow actions and release archive staging/inspection;
14. new CLI, MCP, callback, transport, store, registry, cancellation, and proxy
    regressions;
15. a Review Book, attempted Task Plan, and this corrective handoff.

These are implementation facts, not final acceptance claims.

## 8. Verification evidence already obtained

The interrupted pass recorded the following local results after the latest code
changes:

| Surface | Recorded result | Trust boundary |
|---|---|---|
| `cargo fmt --all -- --check` | pass | Local Windows formatting only |
| Clippy, workspace/all targets/features/locked, warnings denied | pass | Local stable toolchain |
| `cargo test --workspace --all-targets --locked -- --test-threads=1` | 81 passed, 0 failed | Local Windows; must rerun after any code change |
| Hub/store subset after late FIFO fix | 56 passed | Local focused regression |
| Bounded transport subset | 9 passed | Local unit scope |
| Cursor adapter fixture | 10 passed, 1 live-write skip | Node 24.14 locally, not declared CI Node |
| Grok adapter fixture | 16 passed, 1 live-write skip | Node 24.14 locally, not declared CI Node |
| Four adapter JSON files | parse pass | Syntax only |
| Workflow YAML | parse pass | Local PyYAML parsing, not GitHub execution |
| Workflow action pins | 28 full-SHA references | Static scan |
| Markdown | 26 files, balanced fences and local links | Mechanical validation only |
| Sensitive string scan | only synthetic values/example paths found | Pattern scan, not proof of complete secrecy |
| `cargo deny check` | policy checks pass with duplicate/allowance warnings | Local dependency snapshot |
| Release CLI build/version | pass; `acp-hub 0.1.3` | Windows binary only |
| Core publish dry run | pass; 23 files | `--allow-dirty`; no upload |
| CLI package list | pass; 11 paths | Full CLI dry run requires published core 0.1.3 |
| Windows release archive simulation | stage/extract/smoke pass | Local unsiged archive only |
| `git diff --check` | pass with CRLF conversion notices | Text integrity only |

The Windows archive was recomputed during this handoff:

- size: `7,736,171` bytes
- SHA-256:
  `332c07d159d54ab829359996b442a7a8c34183e955ccd3d6300984a2d1f2cac0`

The next agent should recompute it after any packaging change.

### MSRV boundary

The locked dependency graph failed on Rust 1.88 and 1.90 because
`libsqlite3-sys 0.38.1` uses `cfg_select!`. The candidate minimum was raised to
Rust 1.91.

Three Windows attempts to run the complete Rust 1.91 check did not produce a
valid result. They failed while Cargo linked or copied the `rustls`
build-script executable with `Access is denied (os error 5)`. Temporary target
directories and related compiler processes were removed. The new Ubuntu MSRV
job has not run on this dirty worktree.

Therefore:

- Rust 1.91 is a reasoned candidate floor;
- local Windows Rust 1.91 compatibility is unverified;
- hosted Ubuntu Rust 1.91 remains a required gate.

## 9. Independent review status

Three review lanes were attempted:

1. protocol/runtime review: completed one re-audit and reported no remaining
   Critical/High issue under the documented trusted one-to-one proxy boundary;
2. store/registry review: found a logical FIFO accounting issue, which was
   changed and locally regressed; its final report found no remaining
   Critical/High issue in that lane;
3. documentation/release review: the final re-audit did not return before the
   task was interrupted.

The first two reports are useful evidence, but they do not replace a final
whole-diff review. The documentation/release lane is explicitly incomplete.

## 10. Current work status by lifecycle

| Lifecycle state | Status | Meaning |
|---|---|---|
| Live repository intake | complete for this snapshot | Branch, heads, divergence, dirty state, and main areas recorded |
| Review inventory | advanced, not final | 32 findings exist; final whole-diff validation remains |
| Candidate implementation | advanced, not accepted | Large code/doc/release patch exists |
| Local development verification | previously green | Must be rerun after continuation changes |
| Independent code review | partial | Protocol and store lanes returned; final combined review missing |
| Documentation/release review | incomplete | Interrupted before final report |
| Cross-platform CI | not run | Dirty local worktree has no hosted revision |
| Clean-machine installation | not run | No isolated installed binary/skill/adapter end-to-end acceptance |
| Live vendor compatibility | intentionally not run | Would access or mutate user-owned sessions |
| Git integration | not started | Diverged branch plus dirty worktree; no stage/commit |
| Publication | not started | No tag, crates.io upload, or GitHub Release |
| Final user acceptance | not reached | User explicitly rejected the prior completion implication |

## 11. Remaining task plan

### P0 — Preserve and re-establish the baseline

#### H-001 — Protect the dirty worktree

Actions:

- capture `git status --short`, `git diff --binary`, and untracked-file
  inventory before any history operation;
- confirm which Codex adapter changes predated maintenance;
- do not reset, clean, or rebase with an unprotected worktree.

Acceptance:

- every current file is recoverable;
- user-owned/pre-existing changes are distinguished from review changes.

#### H-002 — Reconcile with current `origin/main`

Actions:

- inspect the 3-ahead/7-behind divergence;
- compare equivalent local/merged commits by patch, not subject line;
- choose a safe integration method only after the patch is preserved.

Acceptance:

- the maintained diff is based on the intended current upstream state;
- no upstream fix is silently reverted.

### P0 — Finish the actual review

#### H-100 — Whole-diff protocol and security review

Review:

- endpoint/session/generation ownership;
- callback persistence failure propagation;
- terminal lifecycle and lock ordering;
- all inbound/outbound resource ceilings;
- direct and proxy flow acknowledgement;
- public secret redaction and state-file permissions.

Required tests:

- same session id on two endpoints;
- incompatible protocol version;
- disabled callback capabilities;
- cross-owner terminal access;
- callback response amplification;
- strict FIFO proxy acknowledgement;
- daemon cancellation on the same client connection.

Acceptance:

- every Critical/High claim is tied to code and an observable regression;
- no untrusted or reorder-capable proxy behavior is implied by the current FIFO
  design.

#### H-101 — Whole-diff persistence and concurrency review

Review:

- provisional import creation and cleanup;
- begin/commit/rollback of original replay;
- preservation of Hub-captured turns;
- store reopen and crash recovery;
- run/conversation ownership;
- registry mutation and live-handle invalidation;
- active-run deletion/mutation conflicts;
- bounded global search and message paging.

Acceptance:

- forced failure paths cannot leave ghost rows, lose local messages, or report
  unqualified success;
- every transaction has a reviewed rollback state.

#### H-102 — Re-read every changed test for false-green behavior

Review:

- `crates/cli/tests/cli_contract.rs`
- `crates/cli/tests/mcp_smoke.rs`
- `crates/integration-tests/tests/callback_roundtrip.rs`
- changed integration/store/registry tests
- both adapter test scripts

Look specifically for:

- accepting normal completion in a cancellation test;
- ignoring an inner result;
- tests that never instantiate the named proxy/path;
- assertions against fixtures that bypass the production code;
- skips that are reported as passes;
- privacy tests that log the private value they are meant to protect.

Acceptance:

- each test fails when its named production behavior is removed or reversed.

### P1 — Finish documentation, skill, installation, and release review

#### H-200 — Final documentation consistency sweep

Read together:

- `README.md`
- `CHANGELOG.md`
- `CONTRIBUTING.md`
- `RELEASING.md`
- `doc/dev/spec.md`
- `doc/dev/design.md`
- `doc/dev/bdd.md`
- `doc/dev/tdd.md`
- `doc/dev/impl_plan.md`
- both adapter specs and READMEs
- `doc/roles/implementer.md`

Check:

- real CLI command names and flags;
- no local user path, version snapshot, branch, marker, session id, or machine
  count presented as durable product truth;
- one consistent boundary for private vendor storage;
- no read-only claim around vendor resume/delete;
- BDD/TDD rows map to real tests;
- historical evidence is labeled historical;
- no completion claim exceeds the current handoff status.

Acceptance:

- docs describe the final code rather than the interrupted agent's intent.

#### H-201 — Skill installation and packaged-copy review

Review:

- `.grok/skills/acp-hub/SKILL.md`
- `.grok/skills/acp-hub/references/cheatsheet.md`
- the release-copied `skills/acp-hub` layout

Check:

- executable top-level `send` and `search` examples;
- explicit `--home` behavior;
- least-privilege defaults;
- no instruction to kill the daemon;
- separate PowerShell/POSIX commands;
- all referenced files are inside the release archive;
- no secret is placed in an ordinary command line or diagnostic output.

Acceptance:

- a new operator can follow the packaged skill without repository-only paths or
  hidden local assumptions.

#### H-202 — Clean-machine install simulation

Run in isolated temporary homes:

1. build/package the CLI from the final source;
2. install the package or extracted binary into an isolated path;
3. invoke `--version` and `--help`;
4. load each bundled sample registry;
5. invoke the packaged skill's non-destructive golden path;
6. verify state-directory permissions;
7. verify adapter prerequisites fail clearly when vendor CLIs are absent.

Do not use a real user's Hub home or vendor session database.

Acceptance:

- the extracted archive is self-contained for everything its README references;
- failure messages contain no absolute private paths or credentials.

#### H-203 — Hosted workflow and release review

Before publication:

- parse both workflow YAML files;
- verify every `uses:` pin against the intended upstream action/release;
- verify permissions, tag/version rules, prerelease behavior, crate publish
  ordering, index wait/retry behavior, archive names, and checksums;
- ensure the release archive includes this handoff or intentionally excludes
  internal maintenance material;
- run the workflow on a reviewable GitHub revision.

Acceptance:

- Rust 1.91 Ubuntu, Windows, Linux, macOS, package, supply-chain, and four
  release-target jobs are green on the exact candidate commit.

### P1 — Rerun final local verification

#### H-300 — Rust and Node gates

Minimum commands:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked -- --test-threads=1
cargo deny check
node --check adapters/cursor/adapter.mjs
node --check adapters/cursor/adapter-test.mjs
node adapters/cursor/adapter-test.mjs
node --check adapters/grok/adapter.mjs
node --check adapters/grok/adapter-test.mjs
node adapters/grok/adapter-test.mjs
```

Run the adapter commands against fixture defaults only.

#### H-301 — Static content gates

Re-run:

- JSON parsing for every `adapters/**/agents.json`;
- YAML parsing for both workflows;
- full-SHA action reference scan;
- Markdown fence and local-link validation;
- sensitive/local-information scan;
- `git diff --check`;
- package file lists and extracted archive validation.

Record the command, tool version, timestamp, and exact result in the final
execution record. Do not copy counts from this handoff after files change.

### P2 — Git integration and acceptance

#### H-400 — Final independent review

Use at least these distinct review lenses:

1. protocol/security/resource-bounds;
2. store/transaction/concurrency;
3. CLI/MCP/adapter behavior;
4. docs/skill/install/release;
5. operator experience and privacy.

Resolve or explicitly reject every Critical/High finding with a documented
pillar-level reason.

#### H-401 — Prepare an intentional commit series

Only after the review is green:

- decide whether to split code, tests, docs, and release maintenance;
- ensure each commit is buildable where practical;
- include every currently untracked source/test/doc file;
- keep generated `target/` artifacts out of Git;
- review the staged diff before commit.

#### H-402 — Hosted CI and publication decision

After an exact commit exists:

- push only with user authorization;
- wait for required CI on that commit;
- do not tag or publish from a merely green local worktree;
- treat crates.io publication, tag creation, and GitHub Release as separate,
  explicit actions.

## 12. Known blockers, uncertainties, and residual risks

1. **Branch divergence:** the dirty branch is not based on the current remote
   tip. Integration could duplicate or undo already-merged fixes.
2. **Large uncommitted diff:** 60 files are changed or untracked after this
   handoff. A single broad commit would be hard to review.
3. **Completion labels are stale:** the older plan and Review Book contain
   `verified`/`resolved` labels from an interrupted pass. This handoff overrides
   them.
4. **Final docs/release audit is missing:** its subagent did not return before
   interruption.
5. **MSRV is not proven locally:** Rust 1.91 Windows verification was blocked by
   an OS access denial.
6. **Hosted platforms are untested:** Linux, macOS, hosted Windows, package, and
   release workflows have not run on this worktree.
7. **Live adapter behavior is untested:** installed Cursor/Grok schema and
   destructive behavior remain version-sensitive.
8. **Proxy trust contract is narrow:** FIFO accounting is valid only for the
   documented order-preserving one-to-one proxy.
9. **Release artifact is local and ignored:** it is reproducible evidence, not
   a distributable release.
10. **Line-ending warnings remain:** Git reports LF-to-CRLF conversion notices
    on many changed files. Inspect attributes and avoid a mechanical
    line-ending-only rewrite.
11. **No acceptance commit exists:** all test evidence points to a mutable
    worktree, not an immutable Git object.

## 13. Definition of done

The repository is complete only when all of these are true on the same final
candidate:

1. every Review Book Critical/High finding has a reviewed implementation and a
   meaningful regression or an explicit pillar-level rejection;
2. the final whole diff has independent protocol, persistence, interface,
   documentation, installation, and release review;
3. local formatting, lint, complete tests, adapter fixtures, dependency policy,
   JSON/YAML/Markdown/link/privacy scans, package checks, and archive checks
   pass;
4. Rust 1.91, Windows, Linux, macOS, package, supply-chain, and release matrix
   evidence is attached to the exact candidate commit;
5. a clean isolated installation can use the binary, registry samples, bundled
   adapters, and skill without repository-only files;
6. documentation matches the shipped behavior and contains no local/private
   assumptions;
7. the branch is reconciled with the intended current upstream;
8. the final worktree, commit, push, CI, tag, crates.io, and GitHub Release
   states are reported separately;
9. the user accepts the result as complete.

Until then, report the project as:

> active repository-wide maintenance with a substantial candidate patch and
> partial local verification; final review, integration, hosted CI,
> installation acceptance, and publication remain open.

## 14. Recommended continuation reading order

1. `doc/ssot/pillars/README.md`
2. `doc/ssot/pillars/TechSel.md`
3. `doc/ssot/dev-principles/实现规划原则.md`
4. this handoff
5. `../review/complete-review-book-2026-07-18.md`
6. `complete-task-plan-2026-07-18.md`
7. `doc/dev/spec.md`, `design.md`, `bdd.md`, `tdd.md`, `impl_plan.md`
8. the changed code and tests by the review lanes in section 11
9. adapter docs, skill, CI, and release workflow

The next agent should begin by refreshing Git state and reading the final diff.
It should not begin by trusting the earlier `verified` table.

## 15. Continuation execution record

### 15.1 Takeover baseline

The continuation refreshed `origin/main` before changing the candidate:

| Item | Observed value |
|---|---|
| Local HEAD | `ebd544e1a15c4470633e900bf92342b2f86c66bf` |
| Current `origin/main` | `af859b8dccbb9664f3917c3ae4219ea1e1d75125` |
| Merge base | `cebef648442152ad2f8220265ca41c7a6286236c` |
| Patch-equivalent local commits | all three local commits already have merged equivalents on `origin/main` |
| Remaining upstream-only changes | four Dependabot workflow-action upgrades |
| Dirty state | 52 tracked paths and 8 untracked files; none staged |

Outside `.github/workflows/ci.yml` and `.github/workflows/release.yml`, `HEAD`
and `origin/main` have no content difference. The continuation therefore keeps
the existing worktree in place, reviews the effective candidate against
`origin/main`, and defers every history operation until the patch is reviewed.
This preserves the existing dirty files without treating the stale branch base
as the integration target.

### 15.2 Fresh local baseline

The following commands were rerun before continuation edits:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| workspace/all-target/all-feature Clippy with locked dependencies and warnings denied | pass |
| `cargo test --workspace --all-targets --locked -- --test-threads=1` | 81 passed, 0 failed |
| `cargo deny check` | advisories, bans, licenses, and sources pass; duplicate and unmatched-license warnings remain |
| Cursor adapter syntax and fixture suite | 10 passed, 1 explicit vendor-write skip |
| Grok adapter syntax and fixture suite | 16 passed, 1 explicit vendor-write skip |

The observed tools were Rust/Cargo 1.95.0 and Node.js 24.14.0. These results
establish only the pre-edit Windows baseline.

### 15.3 Active continuation strategy

Five read-only review lanes are running against the effective candidate:

1. protocol, security, callback ownership, resource bounds, and daemon;
2. store transactions, replay, registry, paging, and concurrency;
3. changed-test semantics and false-green detection;
4. CLI, MCP, adapters, public behavior, and privacy;
5. documentation, skill, clean installation, CI, and release packaging.

Confirmed findings will be converted into focused repair tasks with observable
regressions. Configuration-only corrections use static validation plus package
or workflow simulation. No live vendor session, publication action, or Git
history mutation is part of this stage.

### 15.4 Newly confirmed integration defect

The dirty workflows pin older major versions than the four Dependabot upgrades
already merged on `origin/main`:

- `actions/checkout`: candidate v4, upstream v7;
- `actions/upload-artifact`: candidate v4, upstream v7;
- `actions/download-artifact`: candidate v4, upstream v8;
- `softprops/action-gh-release`: candidate v2, upstream v3.

This violates H-002 because the maintenance patch would silently revert
upstream fixes. The repair must retain immutable full-commit pins while moving
to the merged major versions, then re-run workflow syntax, pin, package, and
archive checks.

### 15.5 Independent audit result

The first continuation review did not accept the candidate. Source inspection
and mutation-oriented test review reopened these areas:

1. **H-100 / protocol and security**
   - terminal ownership checks omit the session id for same-endpoint sessions;
   - a replacement connection can inherit a stale `Live` runtime state without
     loading or resuming the session;
   - the connection task retains a command sender, so evicted endpoint/proxy
     processes do not shut down;
   - ordinary HubClient/daemon registry reads expose raw endpoint secrets;
   - configured URLs can appear in HTTP/WebSocket failure text;
   - HTTP pending POST storage and direct cancellation admission are not fully
     bounded;
   - incompatible-version and same-client public cancellation tests do not
     exercise the named production paths.
2. **H-101 / persistence and concurrency**
   - ACP notification-handler errors are out-of-band in the pinned SDK, so
     callback capture failures do not fail a successful load or prompt;
   - failed replay rollback restores messages but not replay-mutated metadata,
     plan, command, mode, config, or usage snapshots;
   - a failed load of a newly discovered session leaves a provisional
     conversation;
   - refresh, send, cancel, and delete are not serialized through one
     conversation operation state;
   - an in-flight endpoint initializer can cache stale configuration after
     registry replacement;
   - one early `send_prompt` error leaks its active-run reservation;
   - discovered relative paths and re-imported cwd values can make ACP and
     callback roots disagree.
3. **H-102 / evidence quality**
   - permission outcome, driver pagination, MCP registry mutations, CLI
     stderr, public cancellation, Grok deletion, search exhaustion, and exact
     adapter replay are not all observed by their current tests.
4. **Public interface and adapters**
   - CLI send output can stop at a byte-bounded page before the run tail;
   - typed Hub errors collapse to generic errors across daemon RPC;
   - MCP endpoint registration is a lossy flat object rather than a tagged
     transport union;
   - mixed valid/corrupt Cursor history can replay partially instead of
     failing closed.

The pinned ACP SDK source confirms the capture-failure mechanism: an error from
`on_receive_notification` is reported to the peer as a null-id JSON-RPC error,
does not fail the concurrent `session/load` or `session/prompt` future, and
normally does not close the connection. The Hub must therefore record and
correlate capture failure state itself.

### 15.6 Active repair packages

The continuation is applying the findings in non-overlapping packages:

| Package | Scope |
|---|---|
| protocol lifecycle | terminal session ownership, handle sender lifetime, incompatible-version regression |
| CLI/MCP/RPC | byte-page continuation, typed daemon errors, tagged registration input, observable process tests |
| adapter integrity | Cursor fail-closed replay, exact Cursor/Grok replay, observable Grok delete |
| safe registry boundary | core redacted public DTOs for list/inspect/proxy reads |
| transport privacy and queue | URL-free failures and bounded HTTP pending POST admission |
| persistence lifecycle | capture-error correlation, replay snapshot rollback, provisional cleanup, operation serialization, handle epochs, cwd consistency, reconnect state |
| evidence completion | public same-client cancellation, driver pagination, search exhaustion, file-permission and package/install assertions |
| documentation and release | align claims and packaged material only after behavior is fixed |

Each package requires a focused failing regression before its source change,
targeted green evidence, specification review, and quality review. Broad
formatting, lint, workspace tests, package simulation, and clean installation
remain final integrated gates.

## 16. User-requested pause checkpoint

The user requested that the already-running repair slices finish where possible,
then that the current state be persisted and work stop until the next session.
This section supersedes the earlier “active” wording in section 15. It is a
continuation checkpoint, not a completion certificate.

### 16.1 Worktree snapshot

The final closeout snapshot observed:

| Item | Value |
|---|---|
| Branch | `codex/resolve-review-feedback` |
| Staged paths | 0 |
| Modified tracked paths | 53 |
| Untracked paths | 9 |
| Diff size | 53 files, +16,256 / -2,710 |
| Commit/push/tag/publication | none |

The worktree intentionally remains dirty. Do not reset, clean, rebase, stage,
commit, or discard it. The counts may move by one or two paths if an in-flight
test-only operation worker completed after this checkpoint; refresh `git status`
and `git diff --check` before relying on the table.

### 16.2 Completed focused repair evidence

The following candidate slices reached focused GREEN evidence and independent
approval during this continuation:

1. workflow action major-version/full-SHA reconciliation;
2. adapter fail-closed parsing, exact replay/deletion fixtures, and private-data
   isolation;
3. redacted public registry DTOs, canonical URL handling, strict proxy-chain
   validation, and side-effect-free validation ordering;
4. bounded transport queues, URL-free errors, response/request cleanup, and
   replay before-image/nonce recovery;
5. CLI/MCP bounded run-tail paging, `promptSeq` plus exact `runId`, typed
   process evidence, closed request schemas, and destructive typo rejection;
6. connection-generation draining, capture-failure correlation, full
   replacement purge, stale-command rejection, and terminal ownership;
7. cross-platform terminal teardown: retained child handles, Unix process
   groups, Windows kill-on-close Jobs with suspended pre-assignment, owned pipe
   readers, and cleanup-before-quota release.

Fresh focused terminal/protocol evidence at the pause:

| Command/filter | Result |
|---|---|
| `cargo check -p acp-hub-core --tests` | pass at terminal checkpoint |
| Windows suspended pre-assignment regression | 1 passed |
| retained-child retry | 1 passed |
| descendant process-tree/reader cleanup | 1 passed |
| repeated kill cached status | 1 passed |
| terminal spawn/unbind reap and quota | 1 passed |
| `protocol_lifecycle` | 4 passed |
| `capture_failure` | 5 passed |
| final closeout `cargo check -p acp-hub-core --tests` | pass |
| final closeout `git diff --check` | pass; only local AutoCRLF notices |

The RPC/daemon owner reported a frozen focused snapshot of 11 RPC unit tests,
16 daemon connection tests, and a core test-target check passing. That snapshot
includes bounded reader/writer actors, pending-request cancellation cleanup,
strict frame/ID handling, global admission bounds, write deadlines,
allocation-bounded response encoding, and sanitized typed Hub errors.

The final independent RPC review did **not** approve the slice. The owner GREEN
is therefore focused evidence only, not final acceptance.

### 16.3 Explicitly unfinished or unverified

One already-running operation-lifecycle package had not produced its final
checkpoint response before the requested stop:

1. the detached prompt worker and its strengthened caller-abort regression were
   reported GREEN;
2. detached refresh, set-mode, set-config, delete, close, new-session, and
   supplied-session-load finalizers compiled at the last clean checkpoint;
3. the strengthened refresh/set/mode/delete/close loop, new-session abort
   barrier, and supplied-session load-error cleanup regression were started but
   no final result was received;
4. therefore operation cancellation safety is **unverified**, even if the
   source and tests are present in the worktree.

Before doing anything else, the next session must confirm that no background
agent is still editing `crates/hub/src/hub.rs`, then run the exact operation
tests and inspect their failure/output rather than assuming success.

The final RPC review left three exact, current tasks:

1. map valid-method parameter decoding failures to fixed, data-free JSON-RPC
   `INVALID_PARAMS` (`-32602`) rather than `INTERNAL_ERROR`;
2. strengthen the normal-ID oversized-response regression to assert the same
   request ID, error code, absent data, and a successful follow-up request on
   the still-open connection;
3. preserve the first dispatch/delivery or non-cancelled join failure observed
   while draining requests after clean EOF instead of logging it and returning
   `Ok(())`.

These are established review findings, not invitations for a new broad audit.

The following integrated gates were deliberately not started after the pause
request:

- final whole-workspace format, check, Clippy, and test runs;
- final adapter fixture matrix and dependency-policy checks;
- final package, archive, clean-install, and installed end-to-end smoke tests;
- hosted CI/release-matrix execution;
- upstream history reconciliation, commit, push, tag, crate publication, or
  GitHub Release.

### 16.4 Next-session restart order

1. Read the project pillars, this handoff, and the final dirty diff.
2. Confirm all prior agents/processes are idle and refresh Git state.
3. Run `git diff --check`; correct only real current defects.
4. Run the pending operation cancellation/finalizer regressions first.
5. Obtain the outstanding final RPC verdict and resolve only an exact remaining
   defect from its established review list.
6. Re-review the integrated Hub operation/RPC changes for specification and
   code quality.
7. Only then run the complete local, adapter, packaging, installation, archive,
   and hosted gates from sections 12 and 13.

Continue to report the repository as a substantial uncommitted maintenance
candidate with focused local evidence. It is not yet a release candidate
accepted by the user.

## 17. Resumed implementation checkpoint

This section supersedes the unfinished-operation instructions in sections 16.3
and 16.4. They describe the earlier user-requested pause and must not be used as
the current restart order.

### 17.1 Hub decomposition implemented

The user added this standing development principle:

> 我看到你的hub文件已经1000行以上了, 及时自行规划设计, 完整进行文件和代码拆分, 不要等我来告诉你

The sentence is preserved verbatim in
`doc/ssot/dev-principles/实现规划原则.md`. The five durable development
documents and `doc/roles/implementer.md` now carry the corresponding module,
acceptance, test, implementation-order, and implementer-responsibility changes.

The former monolithic `crates/hub/src/hub.rs` is now a 21-line facade. It
re-exports the unchanged `crate::hub::{CoreHub, HubClient, ...}` public surface
and delegates to:

- `hub/types.rs`, `state.rs`, `registry.rs`, `conversation.rs`, `prompt.rs`,
  `lifecycle.rs`, `dispatch.rs`, and `client.rs`;
- `hub/tests/{support,registry,client,operation,replay}.rs`.

The largest resulting file is `hub/tests/operation.rs` at 726 lines. Every Hub
production and test file is below the 900-line proactive boundary.

### 17.2 Final operation and RPC corrections

- New-session, prompt, set-param, and set-mode paths reserve per-conversation
  admission before reading endpoint config.
- External refresh reserves admission and then resolves both endpoint config
  and the live handle; a registry replacement cannot leave it using a revoked
  pre-admission handle.
- Cancel snapshots the exact prompt token before async handle lookup and
  revalidates token/run/session before sending the session-scoped notification.
- External refresh keeps its owned admission through replay commit, session
  binding, and `RuntimeCache::Live` publication.
- Replay locks count active prune guards under the replay-map mutex, independent
  of temporary `lock_owned()` `Arc` clones.
- RPC parameter errors are fixed, data-free `INVALID_PARAMS`; clean-EOF drain
  preserves the first dispatch failure; oversized normal-ID responses preserve
  correlation and leave the connection reusable.

### 17.3 Fresh local evidence

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | pass |
| `cargo test --workspace --all-targets --locked -- --test-threads=1` | 185 passed, 5 ignored |
| Cursor fixture adapter | 20 passed, 1 live-write skip |
| Grok fixture adapter | 23 passed, 1 live-write skip |
| `cargo deny check` | policy pass; documented duplicate/unmatched-allowance warnings only |
| Workflow YAML / adapter JSON / full-SHA action pins | pass |
| Markdown fences, local links, public local-path scan | 29 files; no findings |
| `cargo publish --dry-run --allow-dirty -p acp-hub-core` | pass; 23 packaged files |
| CLI package list | pass; 12 packaged files |
| release build | `acp-hub 0.1.3` Windows binary built |
| isolated `cargo install --path` smoke | version plus daemon-backed agent add/list/inspect/remove pass |
| staged Windows archive simulation | allowlist, extraction, nested adapters/skill, checksum, and extracted `--version` pass |

The CLI crate dry-run still cannot resolve `acp-hub-core = 0.1.3` from
crates.io because that core version is not published. `RELEASING.md` and the
release workflow intentionally publish and wait for the core before publishing
the CLI.

### 17.4 External state intentionally unchanged

No commit, push, tag, crate publication, GitHub Release, or hosted CI/release
matrix has been created from this dirty worktree. Those actions require an
immutable candidate and repository-owner publication decision. Future work
must start from the decomposed paths above, not from the obsolete monolithic
`hub.rs` instructions in section 16.

## 18. Final repository review checkpoint

This section supersedes the candidate-review and local-evidence counts in section
17 where they differ.

### 18.1 Independent review closure

Six independent review lanes covered Hub runtime/state, daemon/RPC/interfaces,
adapters, persistence/security, CLI/MCP, and release/documentation surfaces.
Three returned `APPROVED`. The other three reported candidate findings which
were rejected after direct source or upstream-contract verification:

1. `TerminalHandle::cleanup` does reap the root child: after process-tree
   termination, the `try_wait` `None` branch reaches `child.wait()` whether or
   not the root needed a separate `child.kill()`.
2. `SendArgs` has one required Clap input group containing only `--text` and
   `--stdin`; the reported `--prompt-file` option does not exist.
3. `softprops/action-gh-release@v3` documents that an existing tag release is
   updated, existing files are overwritten by default, and
   `overwrite_files` defaults to `true`; the workflow's rerun statement is
   therefore consistent with the pinned action contract.

No actionable Critical, High, or Medium defect remained after this
revalidation. No production-code change was made merely to satisfy a
false-positive review comment.

### 18.2 Fresh final local evidence

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | pass |
| `cargo test --workspace --all-targets --all-features --locked -- --test-threads=1` | 185 passed across 17 suites, 5 ignored |
| Cursor fixture adapter | 20 passed, 1 deliberate live-write skip |
| Grok fixture adapter | 23 passed, 1 deliberate live-write skip |
| adapter `node --check` matrix | pass |
| `cargo deny check` | advisories, bans, licenses, and sources pass; configured duplicate and unmatched-allowance warnings remain non-fatal |
| public/authority Markdown fences and local links | 21 files checked; no finding |
| `cargo publish --dry-run --allow-dirty --locked --package acp-hub-core` | pass; 37 files, 763.6 KiB (145.9 KiB compressed) |
| CLI package list | pass; 11 packaged paths |
| isolated `cargo install --path crates/cli --locked` | installed `acp-hub 0.1.3` |
| installed daemon-backed agent add/list/inspect/remove | pass |
| final `git diff --check` | pass; only local AutoCRLF notices |

The final review also added section G to `doc/roles/implementer.md`: use the
normal subagent interface for independent review, and stop a model-provider
branch after a clear connection failure instead of building a parallel
agent-management system around it.

### 18.3 Current status and external boundary

The repository-wide maintenance, cleanup, local review, and local installation
verification are complete for the current worktree. The worktree remains a
large uncommitted candidate. No commit, push, hosted CI/release matrix, tag,
crate publication, or GitHub Release was performed. Those external publication
steps require an immutable candidate and the repository owner's release
decision; local evidence must not be relabeled as hosted or published evidence.

## 19. Final summary and commit transition

Section 18 recorded the state before the repository owner requested a complete
comparison and local commit. The final comparison is now available at
`final-completion-summary-2026-07-19.md`; it reconciles:

- all 14 initial session todos plus the later Hub-decomposition tasks;
- all 26 Task Plan IDs, with no non-verified item;
- all 32 Review Book findings, with no open Critical, High, or Medium item;
- every unfinished operation/RPC/local-verification item from section 16.

The user authorized committing the complete maintained worktree. The commit
changes only the local repository state. Push, tag, hosted CI/release jobs,
crates.io publication, GitHub Release, and live destructive vendor probes
remain outside this action.

## 20. Completed continuation state — 2026-07-19

Section 19 describes an earlier transition. Another agent then refactored a
large part of the repository, so the candidate was reopened and reviewed as a
new live worktree rather than trusted from that checkpoint.

The continuation work is now complete locally:

- all production and test Rust files are below the proactive 900-line split
  boundary;
- official ACP SDK/package identity, store/registry/session ownership,
  daemon resource admission, proxy physical ACK, adapter fail-closed behavior,
  CLI/MCP redaction, skill, workflows, and package/archive contracts were
  independently rechecked and corrected;
- the integrated matrix passed 218 Rust tests with 5 ignored, Cursor 28 with
  one live skip, Grok 36 with one live skip, formatting, warnings-denied
  Clippy, dependency policy, packaged external-consumer compilation, and
  isolated daemon-backed installation, and documentation/script/package
  integrity checks. Avira quarantines the generated `rustls` build-script
  executable under the generic `TR/W64.MalwareX` heuristic; no permanent
  exclusion is used, and the Ubuntu Rust 1.91 hosted job is the authoritative
  MSRV path.

There is no remaining implementation restart order. A future agent should not
repeat the historical repair plan unless the live checkout changes. The only
next actions are external-state decisions:

1. review and intentionally commit the current dirty candidate;
2. reconcile/push the branch without silently dropping its work;
3. let hosted CI, including Ubuntu Rust 1.91, validate the immutable commit;
4. only after that, decide whether to tag and publish crates/GitHub Release;
5. run live vendor probes only with explicit operator approval and disposable
   or backed-up vendor data.

No stage, new commit, push, tag, publication, or destructive live-vendor probe
was performed by this final continuation.

Final live Git snapshot after fetching `origin/main`:

- branch: `codex/resolve-review-feedback`;
- existing HEAD: `4b3d4e019ff03495b604df03376dfafd02408c38`;
- live `origin/main`: `af859b8dccbb9664f3917c3ae4219ea1e1d75125`;
- raw divergence: 7 remote commits / 4 local commits;
- patch-equivalence: 1 local-unique maintenance commit and 4 remote-unique
  dependency-action commits;
- worktree: 67 tracked paths modified, 39 untracked files, 0 staged.

The untracked files are required split modules, tests, fixtures, and package
verification helpers, not disposable output. Preserve them when reconciling
history and forming the next intentional commit.
