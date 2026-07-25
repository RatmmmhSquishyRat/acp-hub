# Operator UX — Phase 3 Contract (inspect probe / progress / timings)

**Status:** APPROVED for implementation  
**Date:** 2026-07-24  
**Authority:** SYSTEM §F.4 / §G.1–G.2 / §G.8

## 1. Inspect + probe (F-COG)

### CLI / MCP

- `agent inspect <id> [--probe] [--json]`
- MCP `inspect_agent` accepts `probe: boolean` (default false).

### Envelope (always)

```json
{
  "agentId": "...",
  "config": { "...public..." },
  "agentInfo": null | {},
  "capabilities": null | {},
  "cachePopulated": false,
  "probeStatus": "skipped" | "ok" | "failed" | "cached",
  "authMethods": null | [ { "id", "kind", "display?" } ],
  "permissionPolicy": "auto-allow" | "reject" | "auto-cancel",
  "message": null | "human next-step string"
}
```

### Rules

| Condition | probeStatus | message |
|-----------|-------------|---------|
| no probe, empty cache | `skipped` | must suggest `--probe` / probe=true; **must not look like full success alone** |
| no probe, cache present | `cached` | optional |
| probe succeeds | `ok` | null or empty |
| probe fails | `failed` | error summary (no silent empty ok) |
| permission_policy=reject | any | message **must contain** fixed substring: `permission_policy=reject; re-add agent with defaults or edit agents.json` |

Probe connects agent (initialize + cache upsert). Does not session/new for a work conv.

## 2. Progress + timings (F-PROG)

Blocking CLI `conv create` and `send`:

**Progress (JSON mode or always as NDJSON on stderr when `--json` for create/send; human: stderr lines):**

- Human: `[acp-hub] stage=<stage>`
- JSON line: `{"type":"progress","stage":"...","atMs":n}`

**Stages (F.4):** `daemon_connect | agent_spawn | initialize | session_op | prompt | end`

CLI-visible minimum (must emit when applicable):

| Command | Stages |
|---------|--------|
| create | daemon_connect → session_op → end |
| send | daemon_connect → prompt → end |

Skipped stages **omit** timings keys.  
**Timings object** (JSON final or human one-liner): keys among `daemonMs, agentSpawnMs, initializeMs, sessionMs, promptMs, totalMs` (camelCase in JSON).

Daemon-internal spawn/initialize may be absent from CLI if not observed — omit keys (honest).

## 3. SC oracles

| ID | Assert |
|----|--------|
| SC-02 skipped | inspect no probe empty cache → probeStatus=skipped + message mentions probe |
| SC-02 reject | policy reject → fixed substring in message |
| SC-PROG | create/send emit stage=end and totalMs ≥ 0 via shipped Progress helper |

## 4. Non-goals

Doctor, auto-migrate reject, transcript (Phase 2).
