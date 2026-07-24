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

Or download the archive for your platform from
[Releases](https://github.com/RatmmmhSquishyRat/acp-hub/releases), verify it
against `SHA256SUMS`, and place the extracted binary on `PATH`. Release archives
also contain `adapters/`, `skills/acp-hub/`, and the four `scripts/ci/`
source-verification helpers referenced by the maintainer documents. Those
scripts require a full source checkout and are not post-install checks. Vendor
adapters still require their documented Node/vendor CLI prerequisites.

Library: `cargo add acp-hub-core`.

## Getting started

First command starts a local daemon. Data lives in `~/.acp-hub` (override with `--home` or `ACP_HUB_HOME`).

After upgrading ACP Hub, a new client verifies the resident daemon protocol
before sending any command with side effects. If it reports an incompatible
resident daemon, close other Hub clients, let the previous on-demand daemon
exit, and retry. Do not delete `daemon.json` or `daemon.lock` while their owner
is still running.

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

PowerShell:

```powershell
$hubHome = Join-Path $env:TEMP 'acp-hub-example'
acp-hub --home $hubHome agent add omp --command omp --args acp
$conv = (acp-hub --home $hubHome conv create omp --cwd (Get-Location).Path).Trim()
'Hello' | acp-hub --home $hubHome send $conv --stdin
acp-hub --home $hubHome conv show $conv
acp-hub --home $hubHome search 'Hello'
```

Bind an existing agent session:  
`acp-hub conv create <agent> --agent-session-id <sid>`

Sample adapters: `adapters/`. Registry examples default to **local trusted use**
(`permission_policy: auto-allow`, filesystem + terminal callbacks on; empty
`allowed_roots` means the session cwd). Tighten with
`acp-hub agent add … --sandbox`, or `--permission-policy reject` and
`--allow-read false` / `--allow-write false` / `--allow-terminal false`.

**Upgrade note:** registries already on disk with explicit `reject` / disabled
callbacks are **not** rewritten automatically — re-register the agent or edit
`agents.json`. Optional MCP: `acp-hub mcp`.

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

Agents run as local processes with your privileges—only register commands you
trust. Registry environment variables and HTTP headers may contain credentials;
restrict the Hub home with OS ACLs/modes and do not print or commit a populated
`agents.json`.

## More

[Discussions](https://github.com/RatmmmhSquishyRat/acp-hub/discussions) · [Contributing](CONTRIBUTING.md) · [Support](SUPPORT.md) · [Code of Conduct](CODE_OF_CONDUCT.md)  
[CHANGELOG](CHANGELOG.md) · [RELEASING](RELEASING.md) · [SECURITY](SECURITY.md) · skill: repository `.grok/skills/acp-hub/`, release archive `skills/acp-hub/`
License: **MIT OR Apache-2.0**
