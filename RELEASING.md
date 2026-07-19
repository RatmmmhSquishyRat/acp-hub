# Releasing

Production release procedure for **binaries** (GitHub Releases) and **crates** (crates.io).

All commands and repository paths in this document require a full source
checkout at the release commit. The binary archive bundles this document and
its referenced verification scripts for provenance; it does not contain the
Cargo workspace needed to run them.

## Prerequisites

| Requirement | Notes |
|-------------|--------|
| CI green on `main` | Windows + Linux + macOS + package-verify + cargo-deny |
| crates.io ownership | Owner/collaborator of `acp-hub-core` and `acp-hub-cli` |
| Secret `CARGO_REGISTRY_TOKEN` | Repo → Settings → Secrets; publish-capable token |
| Tag matches versions | `vX.Y.Z` == version in both crate manifests |

## Version bump (single source of truth)

1. Set the **same** `version` in:
   - `crates/hub/Cargo.toml`
   - `crates/cli/Cargo.toml`
2. Keep the path dependency in sync:

   ```toml
   acp-hub-core = { path = "../hub", version = "X.Y.Z" }
   ```

3. Update `CHANGELOG.md`: move `[Unreleased]` notes under `[X.Y.Z] - YYYY-MM-DD`.
4. Commit on `main` after CI is green.

Check locally:

```bash
bash scripts/ci/check-crate-versions.sh
```

## Ship

```bash
git checkout main
git pull
git fetch --no-tags origin main
test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
git tag vX.Y.Z
git push origin vX.Y.Z
```

The `release` workflow then:

1. **Preflight** — exact equality among the tag commit, checked-out event SHA,
   and current `origin/main` HEAD; version lockstep; token present for stable
   tags.
2. **Build** — four targets with `--locked`, smoke `--version`, and archive only
   the binary, licenses, root operator documents, adapters, the ACP Hub skill,
   referenced CI verification scripts, and `BUILD_INFO.txt` identity metadata.
   Each archive gets a separate `.sha256` sidecar.
3. **crates.io** (stable tags only, no `-` in tag) — idempotently publish
   `acp-hub-core`, wait until it is visible, publish `acp-hub-cli`, and wait
   until the CLI crate is visible.
4. **GitHub Release** — publish all archives plus aggregate `SHA256SUMS`. Stable
   releases run only after step 3 succeeds. Prereleases run after the crates.io
   job is intentionally skipped.

Prerelease tags (`vX.Y.Z-rc1`) produce GitHub assets only. Their release body
documents archive installation and does not advertise an unpublished crate.

## Local verification (before tagging)

```bash
bash scripts/ci/check-crate-versions.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked -- --test-threads=1
cargo publish -p acp-hub-core --dry-run --locked
bash scripts/ci/check-packaged-consumer.sh
cargo package -p acp-hub-cli --list --locked
```

On Windows, use
`powershell -File scripts/ci/check-packaged-consumer.ps1` for the packaged
external-consumer check.

The CLI package has an exact dependency on the same release of
`acp-hub-core`. Before that new core version exists in the crates.io index,
`cargo publish -p acp-hub-cli --dry-run` is expected to fail dependency
resolution. The release workflow publishes and waits for the core first, then
packages and publishes the CLI.

## Idempotency & failure recovery

- Re-running the workflow for the same tag is **safe** for crates.io: already-published versions are skipped.
- GitHub Release uploads may need a manual cleanup if you re-create the same tag; prefer a new patch version for corrections.
- Yank a bad crates.io release only as last resort (`cargo yank`).

## Support matrix (release artifacts)

| Target | Runner | Notes |
|--------|--------|--------|
| `x86_64-unknown-linux-gnu` | ubuntu-22.04 | glibc linked |
| `x86_64-pc-windows-msvc` | windows-latest | `acp-hub.exe` |
| `x86_64-apple-darwin` | macos-14 (cross) | `CMAKE_OSX_ARCHITECTURES=x86_64` |
| `aarch64-apple-darwin` | macos-14 | native |

## Design notes

- Workspace `[patch.crates-io]` aligns local/CI types with the unpublished ACP test harness. **Published** crates only declare crates.io version requirements; CI `package-verify` builds the packaged core against pure crates.io.
- `crates/integration-tests` is `publish = false` and holds Testy-based end-to-end tests.
- On Unix, if `$home/daemon.sock` would exceed `sun_path`, the daemon binds a short socket inside an owner-only directory under the process temp dir and stores that path in `daemon.json` (clients always connect via metadata). Linux applies socket mode `0600` atomically; platforms such as macOS that reject that operation bind inside the already mode-`0700` directory and immediately enforce `0600` on the socket.
