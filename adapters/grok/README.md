# Grok ACP Adapter

`adapter.mjs` proxies the official Grok Build ACP agent (`grok agent stdio`)
and extends it so the Hub can see and continue every on-disk Grok session.

The official `grok agent stdio` only knows about sessions created within its
own process lifetime. Grok persists **all** sessions (TUI, headless, ACP) to
`~/.grok/sessions/<url-encoded-cwd>/<uuid>/`, but the ACP surface cannot list
or load them:

| method | upstream behavior |
|--------|-------------------|
| `session/list` | `Method not found` (not implemented) |
| `session/load` `<on-disk-id>` | `Path not found` (only in-memory sessions) |
| `session/new` + `session/prompt` | works (live sessions) |

The adapter fills the gap:

| space | store | list/load | prompt |
|-------|-------|-----------|--------|
| acp-live | upstream process memory (also on disk) | upstream passthrough | upstream passthrough (full ACP) |
| on-disk | `~/.grok/sessions/<enc-cwd>/<uuid>/` (`chat_history.jsonl` + `summary.json`) | adapter read-only replay | `grok -r <id> -p --permission-mode plan`, read-only history continuation |

All on-disk access is strictly read-only. The adapter never writes any
Grok-internal storage.

## Key experimental facts (2026-07-09, grok 0.2.93)

- `grok -r <id> -p "..."` resumes by **session ID**, not by cwd bucket. Resuming
  from a different cwd still uses the same id and does **not** fork (unlike
  Cursor's CLI). The adapter still spawns resume from the session's original
  cwd to preserve workspace context (MCP servers, AGENTS.md).
- Headless resume truly continues history: a session asked to reply with a
  marker phrase recalls it on resume.
- ACP `session/new` sessions are also persisted to disk, so local enumeration
  covers every session regardless of origin.
- Grok requires `authenticate` before `session/new`. The adapter
  auto-authenticates with the advertised default method (`cached_token`) right
  after `initialize`, absorbing this vendor step so the Hub never has to.
- The adapter injects `agentCapabilities.sessionCapabilities.list = {}` into the
  `initialize` response (upstream omits it), so the Hub calls `session/list`,
  which the adapter serves from on-disk enumeration.

## Prerequisites

1. Grok Build CLI installed (typically `~/.grok/bin/grok[.exe]`).
2. Authenticated: `grok login` (cached token in `~/.grok/auth.json`), or set
   `XAI_API_KEY`.
3. Node.js >= 22.

## Register

```sh
acp-hub agent add grok --type stdio --command node --args "<this directory>/adapter.mjs"
```

Env overrides (optional): `GROK_CMD` (launcher path), `GROK_HOME` (`~/.grok`).

## Hub-side usage

```sh
acp-hub agent sessions grok                 # list all on-disk grok sessions
acp-hub agent sessions grok --import        # import history for full-text search
acp-hub conv create grok --agent-session-id <id> --cwd <dir>   # bind a session
acp-hub conv send --text "follow up" <conv-id>                 # continue history
acp-hub conv search grok "<query>"                             # search all sessions
```

## Test

```sh
node adapter-test.mjs <on-disk-session-id>
```

Verifies: capability injection, on-disk list/load replay, headless resume with
history context, live ACP `session/new` + `session/prompt` (auto-auth), and
clean error handling.
