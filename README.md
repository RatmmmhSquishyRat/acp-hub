# acp-hub

**One local CLI for many ACP coding agents.** Register endpoints, open conversations, send prompts, and search the history you captured.

```
you  ──►  acp-hub (CLI · MCP · lib)  ──►  on-demand daemon  ──►  ACP agents
```

Crates: **`acp-hub-cli`** (binary `acp-hub`) · **`acp-hub-core`** (library).

## Why

[ACP](https://agentclientprotocol.com/) agents each ship their own client. **acp-hub** is a shared **client + conductor**: same commands for omp, Codex, Cursor, Grok, or any stdio/HTTP/WebSocket agent you register. The Hub stores a searchable projection of turns—independent of each agent’s own UI.

## Install

```bash
cargo install acp-hub-cli --locked
acp-hub --version
```

Or grab a binary from [Releases](https://github.com/RatmmmhSquishyRat/acp-hub/releases). Library: `cargo add acp-hub-core`.

## Getting started

First command starts a local daemon. Data lives in `~/.acp-hub` (override with `--home` or `ACP_HUB_HOME`).

```bash
# register an agent (stdio example)
acp-hub agent add omp --command omp --args acp

# open a conversation → prints conv-…
CONV=$(acp-hub conv create omp)

# talk, then inspect / search Hub history
acp-hub send "$CONV" --text "Hello"
acp-hub conv show "$CONV"
acp-hub search "Hello"
```

PowerShell: `$conv = (acp-hub conv create omp).Trim()`

Bind an existing agent session:  
`acp-hub conv create <agent> --agent-session-id <sid>`

Sample adapters: `adapters/` · optional MCP: `acp-hub mcp`.

## Cheatsheet

| | |
|--|--|
| **agent** | `add` `list` `inspect` `remove` `auth` `logout` `sessions` |
| **conv** | `create` `list` `show` `close` `delete` |
| **send** | `send <conv> --text "…"` or `--stdin` *(not `conv send`)* |
| **search** | `search <query> [--agent] [--conv]` *(not `conv search`)* |
| **config** | `param list\|set` · `mode list\|set` · `proxy add\|list\|remove` |
| **other** | `cancel <conv>` · `mcp` · `serve` (foreground daemon; usually unnecessary) |

```
acp-hub [--home DIR] <cmd> …
```

Prefer `--json` when scripting. `acp-hub <cmd> --help` is authoritative.

## State

| | |
|--|--|
| `agents.json` | registered agents / proxies |
| `hub.db` | conversations, messages, full-text search |
| `daemon.*` | singleton daemon lock & metadata |

Agents run as local processes with your privileges—only register commands you trust.

## More

[Discussions](https://github.com/RatmmmhSquishyRat/acp-hub/discussions) · [Contributing](CONTRIBUTING.md) · [Support](SUPPORT.md) · [Code of Conduct](CODE_OF_CONDUCT.md)  
[CHANGELOG](CHANGELOG.md) · [RELEASING](RELEASING.md) · [SECURITY](SECURITY.md) · skill: `.grok/skills/acp-hub/`  
License: **MIT OR Apache-2.0**
