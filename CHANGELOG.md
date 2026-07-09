# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2] - 2026-07-09

### Added

- **Cursor adapter** (`adapters/cursor`): proxy the official `cursor-agent` ACP surface across session spaces, with local read-only `session/list` + `session/load`, smoke tests, and design notes under `doc/dev/cursor-adapter/`.
- **Grok agent skill** (`.grok/skills/acp-hub/`): operating skill + cheat sheet for the real `acp-hub` CLI (golden path, command map, session bind via `--agent-session-id`).
- Daemon unit coverage for Unix/Windows endpoint path selection.

### Fixed

- Daemon idle-exit waits made robust under slow Windows CI (poll readiness instead of brittle fixed sleeps).

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

[Unreleased]: https://github.com/RatmmmhSquishyRat/acp-hub/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/RatmmmhSquishyRat/acp-hub/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/RatmmmhSquishyRat/acp-hub/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/RatmmmhSquishyRat/acp-hub/releases/tag/v0.1.0
