# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
