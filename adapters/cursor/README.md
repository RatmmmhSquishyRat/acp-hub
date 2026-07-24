# Cursor ACP adapter

`adapter.mjs` proxies Cursor's ACP endpoint and adds discovery/replay for
Cursor-managed session spaces that the upstream ACP surface may not expose.

| Space | Adapter list/load behavior | Prompt behavior |
|---|---|---|
| ACP | Reads `~/.cursor/acp-sessions` when needed and otherwise proxies upstream | Proxies the official ACP session; authentication may be required |
| CLI | Opens `~/.cursor/chats/.../store.db` read-only for discovery/replay | Runs Cursor's supported resume command in `ask` mode; only all-text prompts are accepted, and this may append to Cursor-managed session history |
| IDE | Opens `state.vscdb` read-only for discovery/replay | Rejected because CLI resume does not safely continue an IDE conversation |

“Read-only” in this document applies only to the adapter's direct SQLite reads.
It does not mean that a resumed Cursor process will leave Cursor's own session
store unchanged. `ask` mode restricts workspace tool use; it is not a guarantee
about Cursor's internal history bookkeeping.

## Prerequisites

1. Install Cursor CLI and authenticate it.
2. Install Node.js 22.13 or newer (`node:sqlite` must work without an
   experimental flag).
3. Install ACP Hub.

Optional environment overrides:

- `CURSOR_AGENT_CMD`: Cursor launcher or Node executable
- `CURSOR_AGENT_SCRIPT`: absolute path to Cursor's Node entry point
- `CURSOR_DB_PATH`: IDE `state.vscdb`
- `CURSOR_HOME`: Cursor CLI data root

The adapter never prints these paths in its normal ready message.

## Register

POSIX shell:

```sh
adapter=/absolute/path/to/acp-hub/adapters/cursor/adapter.mjs
acp-hub agent add cursor --type stdio --command node --args "$adapter"
```

PowerShell:

```powershell
$adapter = (Resolve-Path '.\adapters\cursor\adapter.mjs').Path
acp-hub agent add cursor --type stdio --command (Get-Command node).Source --args $adapter
```

The complete [agents.json](agents.json) sample defaults to **local trusted use**
(`auto-allow`, filesystem + terminal on; empty `allowed_roots` ⇒ session cwd).
Replace its portable placeholder before installing it as a Hub registry. For a
locked-down registration use `acp-hub agent add … --sandbox` or explicit
`--permission-policy reject` with `--allow-read false` etc.

## Use

POSIX:

```sh
session_id='replace-with-disposable-cursor-session-id'
workspace=$(pwd)
acp-hub agent sessions cursor
acp-hub conv list --agent cursor
conv_id=$(acp-hub conv create cursor --agent-session-id "$session_id" --cwd "$workspace")
acp-hub conv show "$conv_id"
acp-hub send "$conv_id" --text "Follow up"
acp-hub search "Follow up" --agent cursor
```

PowerShell:

```powershell
$sessionId = 'replace-with-disposable-cursor-session-id'
$workspace = (Get-Location).Path
acp-hub agent sessions cursor
acp-hub conv list --agent cursor
$convId = (acp-hub conv create cursor --agent-session-id "$sessionId" --cwd "$workspace").Trim()
acp-hub conv show "$convId"
acp-hub send "$convId" --text 'Follow up'
acp-hub search 'Follow up' --agent cursor
```

`agent sessions` performs ACP `session/list`; there is no `--import` flag.
Current Hub behavior updates its projection while processing discovered
sessions. Use top-level `send` and `search`.

IDE conversations are view/search only. For tool-capable work, create or use a
live ACP session so permission callbacks remain on the ACP connection.

## Verification

The default probe creates a synthetic Cursor home and mock upstream, then
removes them:

```powershell
node .\adapters\cursor\adapter-test.mjs
```

Installed-agent compatibility is intentionally opt-in and never prints message
bodies. The live read probe lists and loads explicit sessions and verifies that
IDE prompting is rejected locally.

POSIX:

```sh
unset ACP_ADAPTER_DESTRUCTIVE_TESTS
export ACP_ADAPTER_LIVE_TESTS=1
cursor_cli_id='replace-with-disposable-cursor-cli-session-id'
cursor_ide_id='replace-with-disposable-cursor-ide-session-id'
node ./adapters/cursor/adapter-test.mjs "$cursor_cli_id" "$cursor_ide_id"
```

PowerShell:

```powershell
Remove-Item Env:ACP_ADAPTER_DESTRUCTIVE_TESTS -ErrorAction SilentlyContinue
$env:ACP_ADAPTER_LIVE_TESTS = '1'
$cursorCliId = 'replace-with-disposable-cursor-cli-session-id'
$cursorIdeId = 'replace-with-disposable-cursor-ide-session-id'
node .\adapters\cursor\adapter-test.mjs "$cursorCliId" "$cursorIdeId"
```

CLI resume mutates Cursor-managed session state and therefore requires the
separate destructive opt-in. Run these probes only against disposable sessions.

POSIX:

```sh
export ACP_ADAPTER_LIVE_TESTS=1
export ACP_ADAPTER_DESTRUCTIVE_TESTS=1
cursor_cli_id='replace-with-disposable-cursor-cli-session-id'
cursor_ide_id='replace-with-disposable-cursor-ide-session-id'
node ./adapters/cursor/adapter-test.mjs "$cursor_cli_id" "$cursor_ide_id"
```

PowerShell:

```powershell
$env:ACP_ADAPTER_LIVE_TESTS = '1'
$env:ACP_ADAPTER_DESTRUCTIVE_TESTS = '1'
$cursorCliId = 'replace-with-disposable-cursor-cli-session-id'
$cursorIdeId = 'replace-with-disposable-cursor-ide-session-id'
node .\adapters\cursor\adapter-test.mjs "$cursorCliId" "$cursorIdeId"
```

Record OS, Node version, Cursor version, list/load counts, stop reason, and exit
status in a separate validation report. Do not put session ids, paths, message
bodies, branch names, commits, or one-machine counts in this durable README.

## Compatibility boundary

Cursor's on-disk formats are vendor-internal and can change. The adapter:

- opens discovered databases with read-only SQLite connections;
- never writes reverse-engineered records into Cursor databases;
- rejects ambiguous CLI ids found in multiple workspace buckets;
- validates the original workspace before CLI resume;
- passes prompts to the Cursor bootstrap through stdin so prompt text does not
  appear in the OS process argument list;
- rejects mixed image/resource prompts instead of silently dropping blocks;
- publishes the terminal Cursor `result` as the single canonical response,
  ignoring duplicate/buffered assistant copies in partial streams;
- waits for the child streams to close and requires exactly one well-formed
  terminal `result`; malformed, missing, or duplicate terminal records fail
  without publishing buffered assistant output;
- drains and discards bounded vendor stderr without forwarding private vendor
  text; adapter diagnostics contain only static, path-free failure categories.

When a vendor format changes, fail with a clear adapter error and update the
compatibility matrix before changing parsing behavior.
