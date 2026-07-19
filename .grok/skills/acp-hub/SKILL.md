---
name: acp-hub
description: >
  Operate the acp-hub CLI (ACP Hub client/conductor): install/verify the binary,
  register agents, create conversations, send prompts, search history, manage
  params/modes/proxies, and run MCP. Use when the user mentions acp-hub, ACP Hub,
  ACP agents, session/list, hub daemon, or asks to talk to coding agents via ACP.
  Slash: /acp-hub.
metadata:
  short-description: "Use the acp-hub CLI to drive ACP agents"
---

# acp-hub CLI — agent operating skill

Use the **`acp-hub` binary** (crate `acp-hub-cli`) as a local ACP **Client + Conductor**.
Prefer the CLI over reimplementing protocol logic. When unsure of flags, run
`acp-hub <cmd> --help` (source of truth).

## Mental model (read once)

```
You / CLI / MCP  ──JSON-RPC──►  on-demand Hub daemon  ──ACP──►  registered Agents
                                      │
                                      ▼
                              Hub home (agents.json, hub.db, daemon.*)
```

- **Daemon**: first CLI command auto-spawns a singleton daemon per home; idle exit after inactivity. Rarely need `acp-hub serve` (foreground).
- **Home**: `--home <path>` or env `ACP_HUB_HOME`, else `~/.acp-hub` (Windows: user profile `\.acp-hub`). Isolates state; use a temp home for experiments.
- **Agent id**: registry key you choose (e.g. `omp`, `codex`).
- **conv id**: Hub conversation id (usually `conv-<uuid>` from `conv create`).
- **Two history layers**: agent-native sessions (`agent sessions`) vs Hub projection (`conv show` / `search`).
- **Trust**: agents run as local processes with the same privileges as the Hub. Only register commands the user trusts.

## Preconditions

```bash
acp-hub --version          # must exist
# if missing:
#   cargo install acp-hub-cli --locked
#   # or download GitHub Release binary for the platform
```

Prefer machine-readable output with `--json` when parsing.

## Golden path (default workflow)

```bash
hub_home=$(mktemp -d "${TMPDIR:-/tmp}/acp-hub-skill.XXXXXX")

# 1) Register an ACP agent (stdio). Prefer absolute paths for adapter scripts.
acp-hub --home "$hub_home" agent add omp --type stdio --command omp --args acp
# Grok adapter (this repo):
# grok_adapter="$(cd ./adapters/grok && pwd)/adapter.mjs"
# acp-hub --home "$hub_home" agent add grok --type stdio --command node \
#   --args "$grok_adapter"
acp-hub --home "$hub_home" agent list

# 2) Create a conversation (starts agent session; stdout is conv id)
conv=$(acp-hub --home "$hub_home" conv create omp)

# 3) Send a prompt (required: --text or --stdin)  — top-level `send`, NOT `conv send`
acp-hub --home "$hub_home" send "$conv" --text "Hello"

# 4) Inspect Hub projection + search  — top-level `search`, NOT `conv search`
acp-hub --home "$hub_home" conv show "$conv"
acp-hub --home "$hub_home" search "Hello" --agent omp

# 5) Optional: cancel in-flight run
acp-hub --home "$hub_home" cancel "$conv"
```

**Do not invent** commands like `conv send` / `conv search` / `agent sessions --import`
unless `acp-hub <cmd> --help` shows them. Live help is authoritative.

**Windows PowerShell**:

```powershell
$hubHome = Join-Path ([IO.Path]::GetTempPath()) ("acp-hub-skill-{0}" -f [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $hubHome | Out-Null
acp-hub --home $hubHome agent add omp --type stdio --command omp --args acp
$conv = (acp-hub --home $hubHome conv create omp --cwd (Get-Location).Path).Trim()
'Hello' | acp-hub --home $hubHome send $conv --stdin
acp-hub --home $hubHome conv show $conv
acp-hub --home $hubHome search 'Hello' --agent omp
```

These examples intentionally do not kill the per-home daemon or delete the
temporary home immediately. Let the daemon exit after its idle timeout, confirm
that no command or run is active, and only then remove the temporary directory.

## Command map

Grammar only: `acp-hub [--home <dir>] <command>…`. Square brackets in this
single grammar line mean optional syntax; command blocks below are copyable
after replacing angle-bracket placeholders.

| Command | Purpose |
|---------|---------|
| `serve` | Run daemon in foreground (usually unnecessary) |
| `agent …` | Register / inspect / auth agents |
| `proxy …` | Register prompt/response proxies |
| `conv …` | Create / list / show / close / delete conversations |
| `send` | Prompt a conversation |
| `param …` | List/set per-conversation config values |
| `mode …` | List/set per-conversation modes |
| `cancel <conv_id>` | Cancel active run |
| `search <query>` | Full-text search Hub projection |
| `mcp` | MCP server on stdio (tools for MCP clients) |

### `agent`

```bash
acp-hub agent list
acp-hub agent list --json
acp-hub agent add <ID> --type stdio --command <BIN>
acp-hub agent add <ID> --type stdio --command <BIN> --args <ARG>
acp-hub agent add <ID> --type http --url <URL>
acp-hub agent add <ID> --type ws --url <URL>
acp-hub agent add <ID> --json <file>          # full AgentEndpointConfig JSON
acp-hub agent inspect <ID>
acp-hub agent inspect <ID> --json
acp-hub agent remove <ID>
acp-hub agent auth <ID> <method_id>
acp-hub agent logout <ID>
acp-hub agent sessions <ID>                   # agent-native session/list
```

Examples:

```bash
acp-hub agent add omp --command omp --args acp
acp-hub agent add codex --command codex-acp
# `agent add --json` accepts one AgentEndpointConfig (or `{ "config": ... }`).
# Repo samples under adapters/*/agents.json are complete registries for the
# Hub home's agents.json; do not pass those full registry files to --json.
```

### `conv`

```bash
acp-hub conv create <AGENT_ID>
acp-hub conv create <AGENT_ID> --cwd <path> --agent-session-id <sid> --json
acp-hub conv list
acp-hub conv list --agent <AGENT_ID> --json
acp-hub conv show <CONV_ID>
acp-hub conv show <CONV_ID> --json
acp-hub conv close <CONV_ID>                  # close remote ACP session; keep Hub rows
acp-hub conv delete <CONV_ID>
acp-hub conv delete <CONV_ID> --local-only    # skip remote delete
```

### `send`

```bash
acp-hub send <CONV_ID> --text "..."
acp-hub send <CONV_ID> --text "..." --param CONFIG_ID=VALUE --mode <MODE_ID> --json
acp-hub send <CONV_ID> --stdin < prompt.txt
```

Exactly one of `--text` / `--stdin` is required.

### `param` / `mode`

```bash
acp-hub param list <CONV_ID>
acp-hub param set <CONV_ID> <config_id> <value>
acp-hub mode list <CONV_ID>
acp-hub mode set <CONV_ID> <mode_id>
```

Discover ids via `param list` / `mode list` for that conversation (agent-dependent).

### `proxy`

```bash
acp-hub proxy add <ID> --command <BIN>
acp-hub proxy add <ID> --command <BIN> --args <ARG>
acp-hub proxy add <ID> --json <file>
acp-hub proxy list
acp-hub proxy list --json
acp-hub proxy remove <ID>
```

Proxies are stdio ACP components in the conductor chain (pre/post process). Wire into agent config when required by product docs / registry JSON.

### `search`

```bash
acp-hub search "<query>"
acp-hub search "<query>" --agent <ID> --conv <CONV_ID> --limit 50 --json
```

### `mcp`

```bash
acp-hub --home <dir> mcp
```

Runs until stdin closes. Use only when attaching an MCP client; not for one-shot shell scripts.

## Agent decision rules

1. **Always verify binary** with `acp-hub --version` before multi-step flows.
2. **Register before create**: `agent list` must show the agent id used in `conv create`.
3. **Capture conv id** from `conv create` stdout (or `--json`) before `send`.
4. **Use `--json`** when you will parse output; human tables are for display only.
5. **Isolate experiments** with `--home` under a temp dir; delete when done.
6. **Do not invent flags** — run `--help` for the subcommand.
7. **Do not put secrets in chat or command arguments**: `--env KEY=VAL` and
   `--header KEY=VAL` can leak through shell history and process listings. For
   sensitive values, use an external secret launcher or create `agents.json`
   outside chat and explicitly restrict it with the OS ACL/mode. Do not assume
   the current Hub version hardened a pre-existing file, and never dump a raw
   registry that may contain tokens.
8. **Long agent replies**: `send` streams; wait for process exit. On hang, `cancel <conv_id>` then re-check `conv show`.
9. **Failure recovery**:
   - Unknown agent → `agent add` then retry create.
   - Daemon stuck → inspect the selected home's daemon metadata and try a new
     isolated `--home`. Do not kill Hub/agent processes or delete a user's home
     unless the user explicitly authorizes that operation.
   - Auth required → `agent auth <id> <method_id>` after inspecting agent capabilities.
10. **MCP vs CLI**: one-shot automation → CLI; IDE/MCP host integration → `mcp`.

## State layout (home)

| Path | Role |
|------|------|
| `agents.json` | Registered agents/proxies |
| `hub.db` | Conversations, messages, runs, FTS |
| `daemon.json` / `daemon.lock` / `daemon.id` | Singleton daemon metadata |
| `daemon.sock` or short temp sock / Windows named pipe | Local RPC (clients use metadata) |

Do not hand-edit `hub.db` while the daemon is running.

## Quick troubleshooting

| Symptom | Action |
|---------|--------|
| `acp-hub` not found | Install CLI; ensure PATH |
| create fails: agent missing | `agent add` / `agent list` |
| send hangs | `cancel`; inspect agent process; check agent logs |
| empty search | confirm messages via `conv show`; search is Hub projection only |
| cross-talk between projects | separate `--home` per project |
| permission/auth errors | `agent inspect`, `agent auth` |

## Out of scope for this skill

- Implementing ACP agents or editing Hub Rust code (use repo README / RELEASING).
- Assuming a specific third-party agent is installed (`omp`, `codex`, etc. must be on PATH).
- Guaranteeing agent-native history equals Hub capture — treat them as parallel layers.
