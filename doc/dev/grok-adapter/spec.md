# Grok ACP Adapter — Specification

## 1. Purpose

Provide an `acp-hub` adapter for xAI's Grok Build coding agent that is
functionally complete: the Hub can list, read, search, and continue every Grok
session, and can create new live ACP sessions with full upstream capability.

## 2. Background — Grok Build session model

Grok Build (`grok` CLI, xAI) is a terminal coding agent with three execution
modes: interactive TUI, headless (`-p`), and ACP (`grok agent stdio`). All
sessions — regardless of origin — are persisted to a single on-disk store:

```
~/.grok/sessions/<url-encoded-cwd>/<session-uuid>/
    chat_history.jsonl   # messages: system / user / assistant / reasoning
    summary.json         # {info:{id,cwd}, created_at, updated_at, session_summary, current_model_id, ...}
    events.jsonl         # lifecycle events
    prompt_context.json  # resolved config for the first prompt
    system_prompt.txt    # full system prompt
```

There is **one** session space (not three like Cursor). The session id is a
UUID-v7. The bucket directory name is `encodeURIComponent(cwd)`.

## 3. Empirical findings (2026-07-09, grok 0.2.93)

Official docs consulted: `docs.x.ai/build/overview`, `.../cli/reference`,
`.../cli/headless-scripting`, `.../modes-and-commands`; plus direct probing of
`grok agent stdio`.

1. **`session/list` is not implemented.** Upstream returns
   `{"code":-32601,"message":"Method not found"}`. The `initialize` response
   advertises no `sessionCapabilities.list`, so the Hub would never call
   `session/list` and would see zero Grok sessions.
2. **`session/load` cannot load on-disk sessions.** For an on-disk id the
   upstream returns `{"code":-32603,"message":"Path not found."}`. It only
   serves sessions still held in its own process memory.
3. **`session/new` + `session/prompt` work** for live sessions, streaming
   `agent_thought_chunk` + `agent_message_chunk` via `session/update`, plus
   extension notifications `_x.ai/session_notification`,
   `_x.ai/session/prompt_complete`, `_x.ai/sessions/changed`,
   `_x.ai/queue/changed`.
4. **`authenticate` is required before `session/new`.** `initialize` returns
   `authMethods: [{id:"cached_token",...},{id:"grok.com",...}]` and
   `_meta.defaultAuthMethodId: "cached_token"`.
5. **Headless resume continues real history.** `grok -r <id> -p "..."` with
   `--output-format streaming-json` recalls earlier turns (verified: a marker
   phrase asked for in a prior turn is correctly returned on resume).
6. **Resume is id-keyed, not cwd-bucket-keyed.** Resuming from a different cwd
   still uses the same session id and does **not** fork into a new bucket
   (contrast Cursor's CLI, which silently forks). The adapter still uses the
   session's original cwd to preserve workspace context.
7. **ACP-created sessions are persisted to disk.** Local enumeration of
   `~/.grok/sessions/` therefore covers every session regardless of origin.
8. **`grok sessions list/search/delete` and `grok export`** are official CLI
   subcommands for human-facing session management. The adapter reads the
   structured on-disk files directly rather than parsing the human text tables,
   but these subcommands confirm the on-disk store is the authoritative source.

## 4. Architecture

```
Hub ──stdio JSON-RPC──> adapter.mjs ──stdio JSON-RPC──> grok agent stdio (upstream)
                          │  ├─ initialize: forward, INJECT sessionCapabilities.list, auto-authenticate
                          │  ├─ session/list: local on-disk enumeration (upstream lacks it)
                          │  ├─ session/load: live→upstream; on-disk→local chat_history.jsonl replay
                          │  ├─ session/prompt: live→upstream; on-disk→headless `grok -r <id> -p`
                          │  ├─ session/new: forward upstream; track returned id as live
                          │  ├─ session/cancel: kill headless child OR forward upstream
                          │  ├─ session/set_mode|set_config_option: live→upstream; on-disk→reject
                          │  └─ everything else: passthrough (incl. _x.ai/* notifications)
```

### Capability injection

The adapter intercepts the upstream `initialize` result and sets
`agentCapabilities.sessionCapabilities.list = {}` (absent upstream) so the Hub
calls `session/list`. `loadSession: true` (already present upstream) is
preserved so `session/load` is permitted; the adapter intercepts load for
on-disk ids and replays locally.

### Auto-authentication

Right after `initialize`, before replying to the Hub, the adapter sends
`authenticate {methodId: defaultAuthMethodId || "cached_token"}` upstream and
awaits its result. This is best-effort: on failure it logs and continues, so
the Hub can still authenticate manually via `hub/agent/authenticate`.

### Live vs on-disk routing

A `liveSessions` set tracks ids returned by proxied `session/new` calls. For
`session/load` and `session/prompt`:

- id in `liveSessions` → forward upstream (full ACP: modes, config, permission
  gating, streaming).
- id absent from `liveSessions` but present on disk → local replay (load) /
  headless resume (prompt).
- unknown id → forward upstream so it owns the authoritative error.

When the adapter (or upstream) restarts, `liveSessions` is empty, so all
sessions become on-disk and prompts route to headless resume — which
re-establishes context from disk. The only loss is in-process mode/config
state, which is acceptable.

## 5. On-disk access — strictly read-only

- Enumeration: `readdirSync` over `~/.grok/sessions/<bucket>/<uuid>/`, reading
  `summary.json` for metadata and `chat_history.jsonl` for the first real user
  prompt (used as a fallback title).
- Replay: parse `chat_history.jsonl` line by line:
  - `type:"system"` → skip
  - `type:"user"` → extract text from `content[]`; skip `<user_info>` and
    `<system-reminder>` injected entries; strip `<user_query>` wrapper; emit
    `user_message_chunk`.
  - `type:"assistant"` → `content` is a string; emit `agent_message_chunk`.
  - `type:"reasoning"` → extract `summary[].summary_text`; emit
    `agent_thought_chunk`.
- No file is ever written, created, or deleted. No `~/.grok` state is mutated.

## 6. Headless resume — `session/prompt` for on-disk sessions

Spawns `grok --no-auto-update -r <id> -p <text> --output-format streaming-json
--permission-mode dontAsk --no-plan --deny 'Edit(*)' --deny 'Bash(*)'
--deny 'MCPTool(*)' --cwd <original-cwd>` (direct `.exe` spawn, no shell
wrapper). Imported on-disk sessions are read-only: unapproved operations are
silently denied, plan-file writes are disabled, and all edit/shell/MCP tools
are explicitly denied because approvals cannot flow through Hub ACP callbacks.
`--no-auto-update` suppresses background update checks in automation. The
prompt is passed as a single argv element (no shell quoting issues).

Streaming-JSON event translation:

| event | ACP notification |
|-------|------------------|
| `{"type":"thought","data":"..."}` | `session/update` `agent_thought_chunk` |
| `{"type":"text","data":"..."}` | `session/update` `agent_message_chunk` |
| `{"type":"end","stopReason":"EndTurn",...}` | resolve `{stopReason:"end_turn"}` |

`stopReason` mapping: `EndTurn`→`end_turn`, `MaxTurns`→`max_turns`,
`Cancelled`→`cancelled`, `ToolApprovalDenied`→`tool_approval_denied`.

`session/cancel` kills the headless child and resolves `{stopReason:"cancelled"}`.

## 7. Registration

`agents.json` registers `node adapter.mjs` as the `grok` agent with
`permission_policy: "reject"` and fs/terminal client capabilities disabled,
matching the cursor adapter's posture. Env overrides: `GROK_CMD`, `GROK_HOME`.

## 8. Verification

`adapter-test.mjs` exercises, end-to-end through the adapter:

1. `initialize` returns v1 with injected `sessionCapabilities.list` and
   preserved `loadSession`.
2. `session/list` returns on-disk sessions and includes a target id.
3. `session/load` replays on-disk history (>= 2 updates).
4. `session/prompt` on an on-disk session returns `end_turn` and **recalls the
   marker phrase** from history (proves real continuation, not a fresh chat).
5. `session/new` creates a live session (proves auto-auth worked).
6. `session/prompt` on the live session returns `end_turn` and replies
   `GROK-LIVE-OK` (proves live ACP passthrough works).

Run: `node adapter-test.mjs <on-disk-session-id>`.

Result (2026-07-09): all 11 checks PASS.
