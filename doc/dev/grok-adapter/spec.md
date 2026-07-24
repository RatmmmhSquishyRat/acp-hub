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
- a successfully deleted live id is tombstoned until the adapter and its
  upstream process exit together, because the vendor delete command cannot
  evict one session from an already-running upstream ACP process;
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
- reject malformed summaries and ambiguous ids found in multiple workspace
  buckets;
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
- `--no-subagents`
- `--no-memory`
- `--disable-web-search`
- deny rules for edit, shell, filesystem read/search, web fetch, and MCP tools
- `--cwd`

The prompt is stored temporarily in a random private directory and passed by
filename. It uses mode `0600` on POSIX; Windows uses the temporary directory's
inherited user ACL. Prompt text never appears in the OS argument vector. The
temporary directory is removed after the managed process tree exits or on
spawn failure.

Because a detached process cannot relay tool approvals to the ACP client, the
adapter disables plan writes, subagents, cross-session memory, web access, and
workspace read/search/edit/shell/MCP tools. Work requiring ACP permission
callbacks must use a live session.

Streaming JSON maps to ACP:

| Grok event | ACP update/result |
|---|---|
| thought | `agent_thought_chunk` |
| text | `agent_message_chunk` |
| end | normalized ACP stop reason |

The adapter buffers mapped updates under a fixed byte limit until stdout and
stderr have closed. Success requires exactly one recognized `end` record and a
zero child exit. Malformed JSON, unknown events, an unknown stop reason, a
missing or duplicate `end`, or any record after `end` fails the turn without
publishing buffered updates.

Cancel terminates the local process and resolves the prompt as cancelled.
Adapter shutdown retains ownership of every prompt and delete child tree:
POSIX process groups receive `SIGTERM`, get a bounded grace period, and receive
`SIGKILL` if still alive; Windows uses bounded `taskkill /T /F` cleanup. The
adapter does not exit until these cleanup attempts and their bounded waits
finish.

## 6. Delete

`session/delete` validates the id and rejects deletion while a local prompt is
active. It then runs:

```text
grok sessions delete <session-id>
```

Exit zero produces an empty ACP success response and removes the id from the
live-session set. When the id belonged to the current upstream ACP process, the
adapter also tombstones it for the rest of that process lifetime. All later
session requests for that id fail as session-not-found instead of falling
through to the upstream process, and late upstream updates for it are
discarded. Nonzero exit or process failure produces a JSON-RPC error. The Hub
caller remains responsible for choosing remote delete versus
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
  "permission_policy": "auto-allow",
  "client_capabilities": {
    "fs": {
      "read_text_file": true,
      "write_text_file": true,
      "allowed_roots": []
    },
    "terminal": true
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
| Persisted summary is malformed | internal error; no partial replay |
| Session id exists in multiple workspace buckets | invalid-params error; no local replay, prompt, or delete |
| Original workspace is absent | internal error; resume is not attempted |
| Unsupported content block | invalid-params error |
| Headless process or canonical stream fails | internal error with exit status; buffered updates are discarded |
| Delete is requested during local prompt | conversation-busy error |
| `grok sessions delete` fails | internal error; Hub projection is not told that remote delete succeeded |
| Deleted live id is used again before adapter restart | session-not-found; request is not forwarded upstream |

## 9. Verification matrix

The durable spec does not embed one machine's version, session ids, counts,
dates, branch names, commits, or marker phrases.

| Surface | Probe | Acceptance |
|---|---|---|
| Syntax | `node --check adapter.mjs` and `node --check adapter-test.mjs` | both exit zero |
| CLI contract | `grok --help`, `grok sessions delete --help` | required flags/subcommand are present |
| Initialize error | mock upstream error | client receives JSON-RPC error, not `{result:{error:...}}` |
| Capabilities | initialize success | list/delete injected; upstream load preserved |
| List | explicit test store | returns structured sessions |
| Replay | `session/load` on explicit id | emits valid updates without writes |
| Prompt privacy | process inspection | prompt text absent from argv; temp file removed |
| Synthetic continuation | fixture headless child | waits for close, accepts one valid terminal record, and rejects malformed/unknown/missing/duplicate terminal records without partial output |
| Shutdown ownership | signal-resistant fixture child tree | bounded forced termination completes before adapter exit and prompt cleanup |
| Live continuation | destructive opt-in | terminal stop reason and assistant update |
| Delete | delete newly created probe session | Grok and adapter both report success; later load/prompt and late updates cannot reach the deleted upstream copy |
| Logging | default startup | ready log contains no absolute paths |

`adapter-test.mjs` defaults to a synthetic Grok home and mock upstream.
Installed-agent compatibility requires `ACP_ADAPTER_LIVE_TESTS=1`.
Resume/new/prompt/delete also require `ACP_ADAPTER_DESTRUCTIVE_TESTS=1`. The
probe does not print message bodies, session ids, local paths, prompts, or model
replies.
