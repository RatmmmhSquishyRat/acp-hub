# Grok ACP adapter

`adapter.mjs` proxies `grok agent stdio` for live sessions and adds operations
for Grok sessions persisted below `~/.grok/sessions`.

| Operation | Live ACP session | Existing on-disk session |
|---|---|---|
| `session/list` | Returned from local Grok metadata | Returned from local Grok metadata |
| `session/load` | Proxied when the session is live | Replayed from `chat_history.jsonl` |
| `session/prompt` | Proxied through ACP | Continued with the supported headless resume command |
| `session/delete` | `grok sessions delete <id>` | `grok sessions delete <id>` |

Listing and replay only read Grok files. Resume intentionally appends a turn to
Grok-managed state, and delete intentionally removes a Grok session. The adapter
therefore does not claim that every operation is read-only.

For on-disk resume, workspace tools are denied because a detached headless
process cannot relay approval requests through the Hub ACP connection. The
prompt is written to a randomly named temporary directory and passed with
`--prompt-file`; prompt text is not placed in the OS argument vector. The file
uses mode `0600` on POSIX, while Windows relies on the temporary directory's
inherited user ACL. The temporary directory is removed on process exit or spawn
failure.

## Prerequisites

1. Install and authenticate Grok Build.
2. Install Node.js and ACP Hub.
3. Verify that the installed Grok CLI exposes:

   ```sh
   grok agent stdio --help
   grok sessions delete --help
   grok --help
   ```

The adapter depends on `-r`, `--prompt-file`, `--output-format streaming-json`,
the deny flags, and `sessions delete`. These are compatibility
requirements, not permanent version assumptions.

Optional environment overrides:

- `GROK_CMD`: absolute Grok executable path
- `GROK_HOME`: Grok data root
- `GROK_AGENT_SCRIPT`: test-only Node fixture entry point; production
  registrations should leave it unset

The normal ready log does not expose either absolute path.

## Register

POSIX shell:

```sh
adapter=/absolute/path/to/acp-hub/adapters/grok/adapter.mjs
acp-hub agent add grok --type stdio --command node --args "$adapter"
```

PowerShell:

```powershell
$adapter = (Resolve-Path '.\adapters\grok\adapter.mjs').Path
acp-hub agent add grok --type stdio --command (Get-Command node).Source --args $adapter
```

The complete [agents.json](agents.json) sample starts with rejected permissions
and disabled Hub filesystem/terminal callbacks.

## Use

POSIX:

```sh
session_id='replace-with-disposable-grok-session-id'
workspace=$(pwd)
acp-hub agent sessions grok
acp-hub conv list --agent grok
conv_id=$(acp-hub conv create grok --agent-session-id "$session_id" --cwd "$workspace")
acp-hub conv show "$conv_id"
acp-hub send "$conv_id" --text "Follow up"
acp-hub search "Follow up" --agent grok
acp-hub conv delete "$conv_id"
```

PowerShell:

```powershell
$sessionId = 'replace-with-disposable-grok-session-id'
$workspace = (Get-Location).Path
acp-hub agent sessions grok
acp-hub conv list --agent grok
$convId = (acp-hub conv create grok --agent-session-id "$sessionId" --cwd "$workspace").Trim()
acp-hub conv show "$convId"
acp-hub send "$convId" --text 'Follow up'
acp-hub search 'Follow up' --agent grok
acp-hub conv delete "$convId"
```

There is no `agent sessions --import`, `conv send`, or `conv search`. Deleting
without `--local-only` is destructive: after the Hub confirms the endpoint's
delete capability, the adapter invokes Grok's supported session delete command.
Use `--local-only` when only the Hub projection should be removed.

## Verification

The default probe creates and removes an isolated synthetic Grok home:

```powershell
node .\adapters\grok\adapter-test.mjs
```

Installed-agent read/list/load is explicitly opt-in.

POSIX:

```sh
unset ACP_ADAPTER_DESTRUCTIVE_TESTS
export ACP_ADAPTER_LIVE_TESTS=1
grok_session_id='replace-with-disposable-grok-session-id'
node ./adapters/grok/adapter-test.mjs "$grok_session_id"
```

PowerShell:

```powershell
Remove-Item Env:ACP_ADAPTER_DESTRUCTIVE_TESTS -ErrorAction SilentlyContinue
$env:ACP_ADAPTER_LIVE_TESTS = '1'
$grokSessionId = 'replace-with-disposable-grok-session-id'
node .\adapters\grok\adapter-test.mjs "$grokSessionId"
```

Installed-agent resume/new/prompt/delete requires a separate destructive
opt-in.

POSIX:

```sh
export ACP_ADAPTER_LIVE_TESTS=1
export ACP_ADAPTER_DESTRUCTIVE_TESTS=1
grok_session_id='replace-with-disposable-grok-session-id'
node ./adapters/grok/adapter-test.mjs "$grok_session_id"
```

PowerShell:

```powershell
$env:ACP_ADAPTER_LIVE_TESTS = '1'
$env:ACP_ADAPTER_DESTRUCTIVE_TESTS = '1'
$grokSessionId = 'replace-with-disposable-grok-session-id'
node .\adapters\grok\adapter-test.mjs "$grokSessionId"
```

The destructive probe appends to the supplied session, creates a separate live
probe session, and deletes the newly created probe session. It never prints
session ids, local paths, prompts, or reply bodies.

Record OS, Node version, Grok version, advertised capabilities, replay count,
stop reason, delete result, and exit status in a separate validation report.
Keep machine-specific ids, counts, dates, commits, and marker phrases out of
this durable README.
