# Codex through ACP

Codex does not expose an `acp` subcommand directly. ACP Hub can connect through
the published stdio bridge
[`@agentclientprotocol/codex-acp`](https://www.npmjs.com/package/@agentclientprotocol/codex-acp).
This directory contains registration guidance only; there is no repository-local
proxy program.

## Prerequisites

1. Install the ACP bridge. Its published package includes a compatible Codex
   dependency:

   ```sh
   npm install --global @agentclientprotocol/codex-acp
   ```

2. Install ACP Hub and verify the bridge and Hub:

   ```sh
   codex-acp --version
   acp-hub --version
   ```

Authentication is advertised by the bridge during ACP initialization. Use the
client-visible ChatGPT login, API-key, or configured gateway flow. A separately
installed global Codex CLI is not required by the default bridge. Set
`CODEX_PATH` only when intentionally overriding the bridge's bundled compatible
Codex binary, and record that override in compatibility verification.

The model and configuration options are supplied by the connected bridge.
Discover them from the conversation instead of copying a version-specific model
name:

```sh
acp-hub param list <conv-id>
acp-hub mode list <conv-id>
```

## Register

The shortest registration uses the executable shim:

```sh
acp-hub agent add codex --type stdio --command codex-acp
```

On Windows, a direct Node entry point avoids npm shim and daemon `PATH`
differences. Discover the global package root with `npm root --global`, then use
an absolute path:

```powershell
$packageRoot = (npm root --global).Trim()
$bridge = Join-Path $packageRoot '@agentclientprotocol/codex-acp/dist/index.js'
acp-hub agent add codex --type stdio --command (Get-Command node).Source --args $bridge
```

The repository sample [agents.json](agents.json) uses this direct-Node shape.
It is a complete registry example, not an `agent add --json` endpoint file.
Replace its placeholder before installing it as a Hub home's `agents.json`.

## Minimum permissions

The sample starts with:

- `permission_policy: reject`
- filesystem callbacks disabled
- terminal callbacks disabled

This is a safe discovery/default posture. If a workflow needs tools, enable only
the required callback capabilities and allowed roots in a dedicated Hub home.
Do not copy credentials into chat, shell history, or a committed sample.

## Use

POSIX shell:

```sh
hub_home=/absolute/path/to/isolated-hub-home
conv="$(acp-hub --home "$hub_home" conv create codex --cwd "$PWD")"
acp-hub --home "$hub_home" send "$conv" --text "Hello"
acp-hub --home "$hub_home" conv show "$conv"
acp-hub --home "$hub_home" search "Hello"
```

PowerShell:

```powershell
$hubHome = Join-Path $env:TEMP 'acp-hub-codex'
$conv = (acp-hub --home $hubHome conv create codex --cwd (Get-Location).Path).Trim()
'Hello' | acp-hub --home $hubHome send $conv --stdin
acp-hub --home $hubHome conv show $conv
acp-hub --home $hubHome search 'Hello'
```

`send` and `search` are top-level commands. `conv send` and `conv search` do
not exist.

## Reproducible verification matrix

Run this matrix against an isolated Hub home; record the ACP Hub version, bridge
package version, bundled Codex dependency or explicit `CODEX_PATH` override,
OS, advertised auth methods, and command result in a separate dated validation
report.

| Check | Command | Acceptance |
|---|---|---|
| Registration | `acp-hub --home <home> agent inspect codex` | stdio command and args match the intended bridge |
| Create | `acp-hub --home <home> conv create codex --cwd <repo>` | prints a Hub conversation id |
| Prompt | `acp-hub --home <home> send <conv> --text "Hello"` | terminates with an ACP stop reason |
| Projection | `acp-hub --home <home> conv show <conv>` | shows the Hub-captured turn |
| Search | `acp-hub --home <home> search "Hello"` | returns that conversation |

Do not restart or terminate the Hub daemon during an in-flight turn. If a
conversation cannot be restored after an intentional restart, inspect the
endpoint/session capability and bind or create a session explicitly; do not
assume every bridge can restore every historical session.
