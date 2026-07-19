# Contributing to acp-hub

Thanks for helping. This repo ships a small Rust workspace (`acp-hub-core`, `acp-hub-cli`) plus optional adapters under `adapters/`.

## Where to talk

| Kind | Where |
|------|--------|
| Questions, ideas, show-and-tell | [GitHub Discussions](https://github.com/RatmmmhSquishyRat/acp-hub/discussions) |
| Bugs and concrete feature work | [GitHub Issues](https://github.com/RatmmmhSquishyRat/acp-hub/issues) (use a template) |
| Security | **Do not** open a public issue — see [SECURITY.md](SECURITY.md) |

## Development setup

Requirements: Rust **≥ 1.91** (see `Cargo.toml` and the MSRV CI job), and
Node.js **≥ 22.13** when you touch `adapters/*` (required by Cursor's
`node:sqlite`).

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
# Windows:
powershell -File scripts/ci/check-crate-versions.ps1
# Unix:
bash scripts/ci/check-crate-versions.sh
```

Prefer isolated Hub homes when experimenting:

```bash
acp-hub --home /tmp/acp-hub-dev agent list
```

Do not commit secrets, tokens, or real `~/.acp-hub` data.

## Pull requests

1. Branch from `main`.
2. Keep the change focused (one concern per PR when practical).
3. Match existing style; run the checks above.
4. Update `CHANGELOG.md` under `[Unreleased]` for user-visible changes.
5. Crate version bumps follow [RELEASING.md](RELEASING.md) (maintainers cut releases).

CLI surface is defined by `acp-hub <cmd> --help` — do not invent subcommands in docs.

## Code of conduct

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).
