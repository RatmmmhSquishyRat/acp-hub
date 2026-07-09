# Releasing

## Prerequisites

1. crates.io account owns (or is collaborator on) `acp-hub-core` and `acp-hub-cli`.
2. GitHub repo secret `CARGO_REGISTRY_TOKEN` = crates.io API token (`cargo login` / account → API Tokens).
3. `main` is green on CI.

## Version bump

1. Bump `version` in **both** `crates/hub/Cargo.toml` and `crates/cli/Cargo.toml` to the same value (e.g. `0.1.1`).
2. Keep `acp-hub-core = { path = "../hub", version = "…" }` in `crates/cli/Cargo.toml` in sync.
3. Commit on `main`.

## Tag and ship

```bash
git tag v0.1.1
git push origin v0.1.1
```

The `release` workflow will:

1. Build + package binaries for Linux / Windows / macOS → GitHub Release.
2. On **stable** tags only (`vX.Y.Z`, no hyphen): `cargo publish` core then CLI to crates.io.

Prerelease tags (`v0.2.0-rc1`) publish GitHub assets only (no crates.io).

## Local verification (before tagging)

```bash
cargo test --workspace
cargo publish -p acp-hub-core --dry-run --locked
cargo publish -p acp-hub-cli --dry-run --locked
```

## Notes

- Workspace `[patch.crates-io]` is for local/CI type-identity with the unpublished ACP test harness; it is **not** included in published crates.
- `crates/integration-tests` is `publish = false` and holds Testy-based end-to-end tests.
