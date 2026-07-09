# acp-hub

**A generic ACP (Agent Client Protocol) client/conductor.** Register any ACP agent endpoint (stdio / HTTP / WebSocket), manage conversations, send prompts, and capture a searchable history projection — all through one on-demand local daemon, exposed via CLI, an MCP facade, and an embedded library.

> Status: **0.1.x production-ready for local use** (stable install channels, multi-platform CI/release, crates.io). Early product surface; treat agent commands and Hub storage as trusted-local.

ACP Hub acts as the ACP **Client + Conductor**:

```
Client(Hub) ── Conductor(Hub) & Proxies(if any) ── ACP Agents (stdio/HTTP/WS)
```

It fills the gap left by opinionated client implementations: a single place to register agents, drive conversations, and keep a Hub-owned projection (two parallel layers — agent-original via `session/list`+`session/load`, and hub-capture via `session/update`).

## Install

**Prebuilt binary** (no Rust toolchain): download from [GitHub Releases](https://github.com/RatmmmhSquishyRat/acp-hub/releases), verify against `SHA256SUMS`, extract, and put `acp-hub` / `acp-hub.exe` on your `PATH`.

**From crates.io** (Rust ≥ 1.85, see `rust-version` in `Cargo.toml`):

```bash
cargo install acp-hub-cli --locked
```

**As a library** (crate name `acp-hub-core`, rustc crate name `acp_hub`):

```bash
cargo add acp-hub-core
```

```rust
use acp_hub::hub::HubClient;
```

**From source**:

```bash
cargo install --git https://github.com/RatmmmhSquishyRat/acp-hub acp-hub-cli --locked
# or from a local clone
cargo install --path crates/cli --locked
```

Verify: `acp-hub --version`.

### Platform support

| Platform | CI | Release binary |
|----------|----|----------------|
| Windows x86_64 | yes | yes |
| Linux x86_64 | yes | yes |
| macOS aarch64 | yes | yes |
| macOS x86_64 | (via release matrix) | yes |

## Quick start

The first command auto-spawns the on-demand daemon. Everything is stored under the Hub home (`~/.acp-hub` by default; override with `ACP_HUB_HOME`).

```bash
# Register an ACP agent (stdio). Example: omp
acp-hub agent add omp --command omp --args acp
acp-hub agent list

# Create a conversation (spawns the agent, ACP initialize + session/new)
acp-hub conv create omp        # prints a conv-<uuid>

# Send a prompt and stream the reply; both your prompt and the agent's
# response are captured into the projection.
acp-hub send <conv-id> --text "Hello"

# View the captured two-layer history
acp-hub conv show <conv-id>

# Full-text search across all conversations and messages
acp-hub search "hello"
```

Other entry points:

- `acp-hub mcp` — run the Hub as an **MCP server** (19 tools) over stdio, for any MCP-compatible client.
- `acp-hub proxy add ...` — register ACP proxies to pre/post-process prompts.
- `acp-hub param ...` / `acp-hub mode ...` — set per-conversation model/mode/config.

## Configuration & state

All state lives in the Hub home (`$ACP_HUB_HOME`, else `~/.acp-hub`):

| File | Purpose |
|---|---|
| `agents.json` | Registered ACP agent/proxy endpoints (edited via `agent`/`proxy` commands) |
| `hub.db` | SQLite projection: conversations, messages, runs + FTS5 full-text search |
| `daemon.json` / `daemon.lock` / `daemon.id` | On-demand singleton daemon metadata/lock/identity |
| `daemon.sock` (Unix) / named pipe (Windows) | Local RPC channel between CLI/MCP/library and the daemon |

**Home layout (stable for 0.1.x):** a single directory holds config, data, and runtime files. This is intentional for portable single-user installs; XDG/AppData splits are not part of the 0.1 contract.

## Project layout

- `crates/hub` — core engine (`acp-hub-core` / `acp_hub`)
- `crates/cli` — `acp-hub` binary (CLI + MCP stdio facade)
- `crates/integration-tests` — end-to-end Testy suites (not published)
- `adapters/` — example agent configs and adapters
- `doc/` — spec, design, BDD/TDD, SSOT pillars
- `scripts/ci/` — version check + idempotent publish helpers
- `RELEASING.md` — how to cut a release
- `SECURITY.md` — vulnerability reporting
- `CHANGELOG.md` — user-facing changes

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
bash scripts/ci/check-crate-versions.sh
cargo publish -p acp-hub-core --dry-run --locked
```

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See `LICENSE-MIT` and `LICENSE-APACHE`.
