# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1-rc.1] - 2026-07-24

### Changed

- **Pre-release: UX-first operator defaults.** Supersedes 0.2.0 sample
  least-privilege defaults and connection-fatal lag policy for *new*
  registrations and live daemon behavior. The `0.2.0` section remains a
  historical release note.
- **Local trusted defaults:** new agent registrations (CLI, MCP, and omitted
  JSON fields) default to `permission_policy: auto-allow` with filesystem
  read/write and terminal callbacks enabled. Empty `allowed_roots` still means
  the conversation cwd only. Use `acp-hub agent add … --sandbox` (or explicit
  reject / `--allow-*=false`) for a tight registration.
- **Existing `agents.json` entries are not auto-migrated.** Endpoints already
  stored with `reject` / disabled callbacks keep that shape until you re-run
  `agent add` or edit the registry. Ship samples under `adapters/*/agents.json`
  now match the usable defaults.
- Daemon notification lag no longer closes the client connection or aborts
  in-flight RPCs. Lag affects live `hub/conv/update` fan-out only; durable
  conversation remains Hub Store-owned (capture writes Store before notify).
- Resume/load RPC error sources rehydrate into distinct classes (daemon vs
  agent ACP vs I/O vs timeout vs redacted internal) instead of a single
  misleading `daemon unavailable: resume/load operation failed` string.
- Agent-managed product overlay under `doc/ssot/agent-managed/` (frozen
  `doc/ssot/pillars/*` unchanged).
- Refresh the resolved `tokio`, `futures`, `thiserror`, and `anyhow` minor or
  patch dependency line after the `0.2.0` release. This includes the
  `futures` 0.3.33 soundness and memory-leak fixes.

## [0.2.0] - 2026-07-19

### Changed

- Prepare the `0.2.0` public API line: upgrade the official ACP Rust SDK family
  to 1.2.0 and `rmcp` to 2.2.0. ACP SDK types exposed by `acp-hub-core` therefore
  have new Rust type identities; ACP wire protocol negotiation remains v1.
- Compile a disposable external crate against the packaged core and crates.io
  ACP SDK line, so workspace patches cannot hide public type-identity drift.
- Include exactly the four source-verification scripts referenced by bundled
  maintainer documents in platform release archives; exclude the internal
  crates.io publishing helper.
- Ordinary endpoint inspection now redacts local stdio command paths in daemon,
  CLI, and MCP output.

### Fixed

- Make imported Cursor/Grok prompts reject unsupported mixed content instead of
  dropping blocks, use Cursor's canonical terminal result, and prevent vendor
  stderr from leaking private local diagnostics.
- Keep refresh rollback/recovery message cursors stale-safe, serialize external
  session creation by complete agent/session identity, and keep committed
  registry state consistent across disk, cache, epoch, and live handles.
- Partition daemon retained-byte admission so incomplete requests cannot starve
  responses, and acknowledge every physical proxy leg by canonical identity,
  reservation token, and exact retained bytes instead of logical FIFO.
- Preserve real null-id request completion while treating uncorrelated null-id
  SDK notification errors as protocol errors rather than connection-ending
  request responses.
- Recover daemon discovery when the previous singleton exits between lock
  contention and metadata connection, instead of polling stale state until the
  startup timeout.
- Keep owner-only Unix daemon sockets portable when the platform cannot apply
  the requested mode atomically (including macOS): bind inside the already
  owner-only directory, then immediately enforce mode `0600`.
- Require a side-effect-free daemon protocol handshake before any business
  RPC, so a newly installed client safely rejects an incompatible resident
  daemon instead of executing a turn before discovering response-schema drift.
- Charge incomplete stdio frames progressively against the aggregate flow
  budget, reset capture budgets at every load/resume operation, and roll back
  every local `session/new` publication if snapshots or pending-update capture
  fails. Pre-response updates are quarantined under the active connection
  generation: matching updates publish atomically, existing bound-session
  updates are preserved, and rejected or owner-conflicting session updates
  cannot contaminate a retry.
- Keep filesystem authorization roots out of ordinary endpoint reads, map
  invalid/stale pagination cursors to structured MCP client errors, and
  serialize Grok prompts with deletion while terminating every in-flight
  deletion process tree during adapter shutdown.
- Acquire exact persisted run ownership before emitting cancellation, serialize
  cancellation with prompt finalization, and keep notification-send rollback
  atomic across persisted, runtime, and operation state.
- Close daemon clients that fall behind the notification broadcast instead of
  silently continuing after an update gap. Retire unreachable terminal quota
  and activity ownership before bounded, lock-free teardown retries.
- Wait for Grok prompt/delete process trees during shutdown with bounded
  forced-kill fallback. Tombstone successfully deleted live sessions so later
  requests and late upstream updates cannot reach the in-memory copy.
- Pin both public ACP SDK requirements to exact `=1.2.0` and verify those exact
  requirements in the packaged core manifest.
- Revalidate repository-wide module, registry/store atomicity, aggregate
  resource, release-tag, package, and adapter boundaries before completion.

### Included from the unpublished 0.1.3 candidate

#### Added

- Open-source community defaults: Discussions, issue/PR templates, CONTRIBUTING / SUPPORT / Code of Conduct, CODEOWNERS, standard labels; private vulnerability reporting + Dependabot security updates + secret scanning enabled on the GitHub repo.
- Process-level CLI/MCP smoke tests, endpoint-collision coverage, callback
  capability tests, and server-side message pagination tests.

#### Fixed

- Endpoint-scoped session, run, callback, and terminal ownership; ACP capability
  negotiation; load replay ordering; active-run deletion/finalization; registry
  mutation; crash recovery; daemon concurrency; bounded callback resources; and
  combined search pagination.
- Resource-bounded stdio, HTTP/SSE, and WebSocket ACP framing before JSON
  deserialization, with outstanding-message, callback-amplification, SSE
  stream, and shared partial-event budgets; legacy unpaged message RPC removal
  with client-side traversal of bounded server pages.
- Registry output redaction, Hub-home/database/IPC permissions, stale daemon
  cleanup, caller cwd propagation, control-character-safe table output, and
  callback/store error propagation.
- Correct the declared MSRV to Rust 1.91, matching the locked dependency graph,
  and enforce it in CI; run adapter fixtures on the declared Node.js 22.13
  minimum.
- Cursor and Grok adapter privacy defaults, JSON-RPC error forwarding, durable
  command examples, cross-platform paths, fail-closed storage parsing,
  sanitized probes, prompt-file shutdown cleanup, and vendor-session mutation
  wording.
- MCP management coverage, cancellation, destructive annotations, and bounded
  current-run/message responses.

#### Changed

- Release archives now use an explicit operator allowlist: the binary, licenses,
  root operator documents, adapters, the ACP Hub skill, and `BUILD_INFO.txt`,
  with extracted archive verification in CI.
- Sample endpoint registries now default to least privilege.
- Workflow actions are pinned to full revisions; workflows default to
  read-only repository permissions and elevate only the release-upload job.

## [0.1.2] - 2026-07-09

### Added

- **Cursor adapter** (`adapters/cursor`): proxy the official `cursor-agent` ACP surface across session spaces, with local read-only `session/list` + `session/load`, smoke tests, and design notes under `doc/dev/cursor-adapter/`.
- **Grok agent skill** (`.grok/skills/acp-hub/`): operating skill + cheat sheet for the real `acp-hub` CLI (golden path, command map, session bind via `--agent-session-id`).
- Daemon unit coverage for Unix/Windows endpoint path selection.

### Fixed

- Daemon idle-exit waits made robust under slow Windows CI (poll readiness instead of brittle fixed sleeps).

### Changed

- **README** rewritten for scanners: purpose → install → getting started → cheatsheet → state (crate + GitHub homepage share this file).

### Notes

- Adapter and skill artifacts ship in the repository; published crates remain `acp-hub-core` + `acp-hub-cli` (binary/library). Install channels unchanged.

## [0.1.1] - 2026-07-09

### Added

- Production CI: Windows + Linux + macOS, `--locked` builds/tests, crate version lockstep check, `cargo package` verify against pure crates.io deps, `cargo-deny` (advisories + licenses).
- Release hardening: preflight version/token checks, idempotent crates.io publish, LICENSE/README/BUILD_INFO inside binary archives, aggregate `SHA256SUMS`.
- Project hygiene: `SECURITY.md`, `CHANGELOG.md`, `deny.toml`, `rust-toolchain.toml`, maintainer `RELEASING.md`.
- Grok adapter sample under `adapters/grok` (proxy official grok agent stdio with on-disk session coverage).

### Fixed

- Unix daemon sockets: when `$home/daemon.sock` would exceed platform `sun_path` limits (common on macOS with deep temp paths), bind a short socket under the process temp dir and record it in `daemon.json`.

## [0.1.0] - 2026-07-05

### Added

- Initial public release of **acp-hub** (ACP client/conductor hub).
- Crates on crates.io: `acp-hub-core` (library) and `acp-hub-cli` (binary `acp-hub`).
- GitHub Release multi-platform binaries: Linux x86_64, Windows x86_64, macOS x86_64 + aarch64.
- On-demand singleton daemon, agent/proxy registry, conversation projection + FTS search, CLI and MCP stdio facade.

[Unreleased]: https://github.com/RatmmmhSquishyRat/acp-hub/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/RatmmmhSquishyRat/acp-hub/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/RatmmmhSquishyRat/acp-hub/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/RatmmmhSquishyRat/acp-hub/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/RatmmmhSquishyRat/acp-hub/releases/tag/v0.1.0
