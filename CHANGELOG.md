# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-07-09

### Added

- Production CI: Windows + Linux + macOS, `--locked` builds/tests, crate version lockstep check, `cargo package` verify against pure crates.io deps, `cargo-deny` (advisories + licenses).
- Release hardening: preflight version/token checks, idempotent crates.io publish, LICENSE/README/BUILD_INFO inside binary archives, aggregate `SHA256SUMS`.
- Project hygiene: `SECURITY.md`, `CHANGELOG.md`, `deny.toml`, `rust-toolchain.toml`, maintainer `RELEASING.md`.

### Fixed

- Unix daemon sockets: when `$home/daemon.sock` would exceed platform `sun_path` limits (common on macOS with deep temp paths), bind a short socket under the process temp dir and record it in `daemon.json`.

## [0.1.0] - 2026-07-05

### Added

- Initial public release of **acp-hub** (ACP client/conductor hub).
- Crates on crates.io: `acp-hub-core` (library) and `acp-hub-cli` (binary `acp-hub`).
- GitHub Release multi-platform binaries: Linux x86_64, Windows x86_64, macOS x86_64 + aarch64.
- On-demand singleton daemon, agent/proxy registry, conversation projection + FTS search, CLI and MCP stdio facade.

[Unreleased]: https://github.com/RatmmmhSquishyRat/acp-hub/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/RatmmmhSquishyRat/acp-hub/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/RatmmmhSquishyRat/acp-hub/releases/tag/v0.1.0
