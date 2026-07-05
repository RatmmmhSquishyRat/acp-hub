# acp-hub

**A generic ACP (Agent Client Protocol) client/conductor.** Register any ACP agent endpoint (stdio / HTTP / WebSocket), manage conversations, send prompts, and capture a searchable history projection — all through one on-demand local daemon, exposed via CLI, an MCP facade, and an embedded library.

> Status: early/alpha. Cross-platform CI runs on Windows and Linux. Real-agent round-trips have been verified against `omp acp`.

ACP Hub acts as the ACP **Client + Conductor**:

```
Client(Hub) ── Conductor(Hub) & Proxies(if any) ── ACP Agents (stdio/HTTP/WS)
```

It fills the gap left by opinionated client implementations: a single place to register agents, drive conversations, and keep a Hub-owned projection (two parallel layers — agent-original via `session/list`+`session/load`, and hub-capture via `session/update`).

## Install

**Prebuilt binary** (preferred — no Rust toolchain needed): download from [GitHub Releases](https://github.com/RatmmmhSquishyRat/acp-hub/releases) for your platform and put `acp-hub`/`acp-hub.exe` on your `PATH`.

**From source** (needs Rust ≥ 1.85):

```bash
# from the public repo
cargo install --git https://github.com/RatmmmhSquishyRat/acp-hub acp-hub-cli

# or from a local clone
cargo install --path crates/cli
```

Verify: `acp-hub --version`.

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

Note: the home is currently a single dotdir (`~/.acp-hub`) lumping config + data + runtime. It is **not** yet split per the XDG / platform-standard layout — a known follow-up.

## Project layout

- `crates/hub` — the core engine (registry, store, ACP driver, daemon, RPC, MCP facade logic)
- `crates/cli` — the `acp-hub` binary (CLI + MCP stdio facade)
- `adapters/` — example agent configs (`omp`, `codex`) and a read-only Cursor history adapter
- `doc/` — spec, design, BDD/TDD, and the SSOT pillars

## License

Dual-licensed under MIT OR Apache-2.0, at your option.
