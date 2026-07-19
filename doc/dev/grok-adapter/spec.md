# Grok ACP Adapter — Specification

> Grounded in `doc/ssot/pillars/README.md`, with the private-storage boundary
> defined by `doc/dev/spec.md`.

## 1. Purpose

`adapters/grok/adapter.mjs` proxies `grok agent stdio` for live ACP sessions and
adds list, replay, continuation, and delete operations for sessions persisted in
Grok's local session store.

The adapter exists because the upstream ACP process may only know sessions held
in its current process. It must not turn that limitation into an empty or
fabricated Hub history.

## 2. Session model

Grok keeps session metadata and history below:

```text
~/.grok/sessions/<encoded-cwd>/<session-id>/
  summary.json
  chat_history.jsonl
  events.jsonl
  prompt_context.json
  system_prompt.txt
```

The adapter uses one session-id namespace:

- ids returned by proxied `session/new` are considered live in the current
  upstream process;
- other valid ids found on disk are considered persisted sessions;
- unknown ids return an adapter or upstream error.

## 3. Protocol routing

```text
Hub -> adapter -> grok agent stdio
          |
          +-- initialize: preserve upstream result/error; add list/delete caps
          +-- session/list: local structured enumeration
          +-- session/load:
          |      live -> upstream
          |      persisted -> local replay
          +-- session/prompt:
          |      live -> upstream
          |      persisted -> grok -r <id> --prompt-file <path>
          +-- session/delete: grok sessions delete <id>
          +-- session/cancel: terminate local child or proxy upstream
```

After a successful initialize result, the adapter attempts the upstream
advertised default authentication method. Failure is logged and the client can
still call the normal Hub authentication operation. An upstream initialize
error remains a JSON-RPC error; it is never wrapped inside a successful result.

The adapter adds:

- `sessionCapabilities.list`, because it serves local enumeration;
- `sessionCapabilities.delete`, because it invokes Grok's supported delete
  command.

Upstream `loadSession` and all unrelated capabilities remain unchanged.

## 4. Read and mutation boundary

List/load:

- enumerate structured Grok files;
- parse `summary.json` and `chat_history.jsonl`;
- skip injected environment/system context;
- emit user, assistant, and reasoning chunks;
- do not modify Grok state.

Prompt/delete:

- persisted prompt invokes Grok resume and may append to that Grok session;
- delete invokes `grok sessions delete <id>` and removes Grok-managed state;
- live prompt continues through upstream ACP.

The implementation therefore makes operation-level claims instead of calling
the complete adapter read-only.

## 5. Headless persisted-session prompt

The installed CLI must expose:

- `-r`
- `--prompt-file`
- `--output-format streaming-json`
- `--permission-mode dontAsk`
- `--no-plan`
- deny rules
- `--cwd`

The prompt is stored temporarily in a random private directory and passed by
filename. It uses mode `0600` on POSIX; Windows uses the temporary directory's
inherited user ACL. Prompt text never appears in the OS argument vector. The
temporary directory is removed on process exit or spawn failure.

Because a detached process cannot relay tool approvals to the ACP client, the
adapter disables plan writes and denies edit, shell, and MCP tool calls.
Work requiring ACP permission callbacks must use a live session.

Streaming JSON maps to ACP:

| Grok event | ACP update/result |
|---|---|
| thought | `agent_thought_chunk` |
| text | `agent_message_chunk` |
| end | normalized ACP stop reason |

Cancel terminates the local process and resolves the prompt as cancelled.

## 6. Delete

`session/delete` validates the id and rejects deletion while a local prompt is
active. It then runs:

```text
grok sessions delete <session-id>
```

Exit zero produces an empty ACP success response and removes the id from the
live-session set. Nonzero exit or process failure produces a JSON-RPC error.
The Hub caller remains responsible for choosing remote delete versus
`conv delete --local-only`.

## 7. Registration

```json
{
  "transport": {
    "type": "stdio",
    "command": "node",
    "args": ["/absolute/path/to/acp-hub/adapters/grok/adapter.mjs"],
    "env": {}
  },
  "proxy_chain": [],
  "permission_policy": "reject",
  "client_capabilities": {
    "fs": {
      "read_text_file": false,
      "write_text_file": false,
      "allowed_roots": []
    },
    "terminal": false
  }
}
```

Environment overrides: `GROK_CMD`, `GROK_HOME`, and the fixture-only
`GROK_AGENT_SCRIPT`. Production registrations leave `GROK_AGENT_SCRIPT` unset.
The ready log is path-free.

## 8. Errors

| Condition | Result |
|---|---|
| Upstream initialize fails | same JSON-RPC error channel |
| Persisted session is absent | session-not-found error |
| Original workspace is absent | internal error; resume is not attempted |
| Unsupported content block | invalid-params error |
| Headless process fails | internal error with exit status |
| Delete is requested during local prompt | conversation-busy error |
| `grok sessions delete` fails | internal error; Hub projection is not told that remote delete succeeded |

## 9. Verification matrix

The durable spec does not embed one machine's version, session ids, counts,
dates, branch names, commits, or marker phrases.

| Surface | Probe | Acceptance |
|---|---|---|
| Syntax | `node --check adapter.mjs` | exits zero |
| CLI contract | `grok --help`, `grok sessions delete --help` | required flags/subcommand are present |
| Initialize error | mock upstream error | client receives JSON-RPC error, not `{result:{error:...}}` |
| Capabilities | initialize success | list/delete injected; upstream load preserved |
| List | explicit test store | returns structured sessions |
| Replay | `session/load` on explicit id | emits valid updates without writes |
| Prompt privacy | process inspection | prompt text absent from argv; temp file removed |
| Continuation | destructive opt-in | terminal stop reason and assistant update |
| Delete | delete newly created probe session | Grok and adapter both report success |
| Logging | default startup | ready log contains no absolute paths |

`adapter-test.mjs` defaults to a synthetic Grok home and mock upstream.
Installed-agent compatibility requires `ACP_ADAPTER_LIVE_TESTS=1`.
Resume/new/prompt/delete also require `ACP_ADAPTER_DESTRUCTIVE_TESTS=1`. The
probe does not print message bodies, session ids, local paths, prompts, or model
replies.
