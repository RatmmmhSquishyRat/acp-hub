# Cursor ACP Adapter — Specification

> Grounded in `doc/ssot/pillars/README.md`, with the private-storage boundary
> defined by `doc/dev/spec.md`.

## 1. Purpose

Cursor's ACP endpoint and its CLI/IDE products can expose different session
sets. `adapters/cursor/adapter.mjs` remains an ACP proxy for live behavior and
adds discovery/replay for sessions that are visible only in Cursor-managed
local stores.

This adapter is a compatibility component. Cursor's internal schemas are not an
ACP contract, so parsing failures must be explicit and must not be disguised as
an empty successful history.

## 2. Session-space model

| Space | Discovery and replay | Prompt |
|---|---|---|
| ACP | Enumerate/replay the local ACP session directory when necessary; otherwise proxy upstream | Proxy upstream ACP |
| CLI | Read `~/.cursor/chats/<workspace-hash>/<id>/store.db` | Run the supported CLI resume path in `ask` mode |
| IDE | Read the configured `state.vscdb` | Reject |

The adapter labels CLI and IDE titles and attaches
`_meta["cursor-adapter"].space`. Classification precedence is ACP, unambiguous
CLI, IDE, then upstream-owned unknown id.

Duplicate CLI ids in different workspace buckets are rejected. Resume is
allowed only when the original workspace can be resolved and matches the
session's workspace hash.

## 3. Storage and mutation boundary

Direct adapter reads:

- use SQLite read-only connections;
- read only the records required for session metadata and message replay;
- never create reverse-engineered Cursor records;
- never update or delete Cursor databases.

These constraints apply to the adapter's parser, not to a vendor process that
the adapter deliberately invokes. A CLI `session/prompt` resumes the original
Cursor session and may append to Cursor-managed history. `--mode ask` limits
workspace tool behavior because headless permission requests cannot be relayed
through ACP; it does not make Cursor's session store immutable.

IDE prompt is rejected because Cursor's CLI resume route does not reliably
attach to IDE history and can create an unrelated CLI session or overwrite
derived transcript data.

## 4. Protocol routing

```text
Hub -> adapter -> cursor-agent acp
          |
          +-- session/list: upstream page plus local ACP/CLI/IDE discoveries
          +-- session/load: local read-only replay when local classification wins
          +-- session/prompt:
          |      ACP -> upstream
          |      CLI -> restricted headless resume
          |      IDE -> typed rejection
          +-- session/cancel: terminate local CLI child or proxy upstream
          +-- set_mode/config:
                 ACP -> upstream
                 CLI/IDE -> typed rejection
```

Client responses to upstream permission or vendor-extension requests pass
through unchanged. Unknown methods and notifications also pass through.

The CLI prompt enters a small Node bootstrap over stdin and is inserted into
the child process's in-memory argument array before the Cursor entry point is
loaded. The prompt does not appear in the OS process argument list.

The headless child is settled from its `close` event so stdout has been fully
consumed. Assistant events in the partial stream are not forwarded directly.
Exactly one well-formed terminal `result` is required; its text is the single
canonical assistant update. Malformed JSON, a missing result, a duplicate
result, a result followed by another record, a nonzero exit, or a vendor error
fails the turn without publishing buffered assistant copies.

## 5. Capability boundary

- Session list/load are available through the adapter.
- ACP prompts retain upstream modes, config, tools, and authentication.
- CLI prompts support text and a terminal stop reason but cannot relay
  interactive permission callbacks.
- IDE prompts are unsupported.
- Cursor session close/delete are not synthesized by writing internal storage.
  Hub projection deletion remains available with `conv delete --local-only`;
  remote delete is capability-gated.

This is the pillar's endpoint-capability boundary: unsupported remote mutation
must return a typed error instead of a fabricated success.

## 6. Registration

Node.js 22.13 or newer is required so `node:sqlite` works without an
experimental flag. CI runs the fixture suite on that declared minimum.

```json
{
  "transport": {
    "type": "stdio",
    "command": "node",
    "args": ["/absolute/path/to/acp-hub/adapters/cursor/adapter.mjs"],
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

Environment overrides: `CURSOR_AGENT_CMD`, `CURSOR_AGENT_SCRIPT`,
`CURSOR_DB_PATH`, `CURSOR_HOME`.

The ready log is path-free. Detailed filesystem paths belong in explicit debug
output or a private validation report, not the default ACP stderr stream.

## 7. Errors

| Condition | Result |
|---|---|
| Invalid or missing local session | session-not-found error |
| Duplicate CLI id in multiple buckets | invalid-params error |
| Original CLI workspace cannot be verified | internal error; resume is not attempted |
| IDE prompt | invalid-params error explaining the capability boundary |
| Local ACP replay was used because upstream load failed | prompt is rejected until upstream can load it |
| Cursor child fails | internal error with exit status; no fabricated stop reason |
| Cursor terminal stream is malformed, missing, or duplicated | internal error; no assistant fallback is published |

## 8. Verification matrix

The durable spec records what must be checked, not one machine's counts,
session ids, dates, branches, commits, model names, or marker phrases.

| Surface | Probe | Acceptance |
|---|---|---|
| Syntax | `node --check adapter.mjs` and `node --check adapter-test.mjs` | both exit zero |
| Handshake | ACP `initialize` | upstream result or upstream JSON-RPC error is preserved |
| Discovery | `session/list` | returns an array and deduplicates ids |
| CLI replay | `session/load` on an explicit test id | emits at least one valid message update |
| IDE replay | `session/load` on an explicit test id | emits valid message updates without DB writes |
| IDE safety | `session/prompt` on IDE id | rejects before spawning Cursor |
| Synthetic CLI continuation | fixture headless child | waits for close, publishes one canonical terminal result, and rejects malformed/missing/duplicate terminal records |
| Live CLI continuation | destructive opt-in probe | emits a reply chunk and terminal stop reason |
| Privacy | inspect logs/process args | no ready-path disclosure and no prompt text in OS argv |

`adapter-test.mjs` defaults to a synthetic Cursor home and mock upstream.
Installed-agent compatibility requires `ACP_ADAPTER_LIVE_TESTS=1`. The mutating
CLI continuation probe additionally requires
`ACP_ADAPTER_DESTRUCTIVE_TESTS=1` and must use an explicitly selected disposable
session. The script does not print message bodies, session ids, or local paths.

## 9. Compatibility references

- [Cursor ACP documentation](https://cursor.com/docs/cli/acp)
- [ACP session/load specification](https://agentclientprotocol.com/protocol/session-setup)
- [Cursor forum discussion about session/load history](https://forum.cursor.com/t/acp-no-conversation-history-is-restored-when-loading-an-existing-session/158388)
- [Zed issue tracking Cursor ACP history](https://github.com/zed-industries/zed/issues/56246)

References provide compatibility context. Live CLI help and the verification
matrix decide whether the current vendor version is supported.
