# Phase 1 Wire + Store Contract

**Status:** Implementable engineering contract for **Phase 1 only** — **v1.2 APPROVED (coding may start)**  
**Authority:** [OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md) v0.3  
**Date:** 2026-07-24  
**Bar:** Staff engineer implements **without inventing** product decisions.  
**Review:** multi-round adversarial → v1.1 fixes → v1.2 closed close-busy last_outcome + `--all` vs closed.  
**Gate:** **Phase 1 coding may start against this contract.**  
**Out of Phase 1:** transcript merge polish, full inspect probe fields, search IX, F-MIG auto-migrate, progress stream (Phase 3), cancel-ignored 30s (Phase 3).

---

## 1. Storage schema (R3 = hybrid, freeze)

### 1.1 Columns on `conversations` (additive migration)

| Column | Type | Default | Notes |
|--------|------|---------|-------|
| `origin` | TEXT NOT NULL | `'imported_list'` on backfill; app sets on write | CHECK IN (`hub_created`,`bound`,`imported_list`) |
| `interaction` | TEXT NOT NULL | computed then stored | CHECK IN (`writable`,`read_only`) — **stored = gate truth** |
| `phase` | TEXT NOT NULL | `'open'` | CHECK IN (`open`,`closed`,`deleted`) |
| `busy` | TEXT NOT NULL | `'none'` | CHECK IN (`none`,`running`,`cancelling`) |
| `last_outcome` | TEXT NOT NULL | `'none'` | CHECK IN (`none`,`completed`,`failed`,`cancelled`) |

Keep legacy `status` TEXT column **one release** as **generated mirror** of synthetic STATUS (below) for old readers; new code **writes phase/busy/last_outcome first**, then sets `status` = synthetic.

### 1.2 Synthetic STATUS (list filter + CLI column)

Priority (first match):

1. `phase=deleted` → not listed (default list excludes)
2. `busy=running` → `running`
3. `busy=cancelling` → `cancelling`
4. `phase=closed` → `closed`
5. `last_outcome=failed` → `failed`
6. `last_outcome=cancelled` → `cancelled`
7. `last_outcome=completed` → `completed`
8. else → `idle`

### 1.3 Run lifecycle transitions

| Event | phase | busy | last_outcome |
|-------|-------|------|--------------|
| create hub_created | open | none | none |
| discover new import | open | none | none |
| send accepted | open | running | *(unchanged until end)* |
| run completed | open | none | completed |
| run failed | open | none | failed |
| run cancelled | open | none | cancelled |
| cancel requested | open | cancelling | *(unchanged)* |
| close success/degraded | **closed** | none | *(unchanged)* |
| delete | deleted | none | *(unchanged)* |

**Idle:** only `open + busy=none + last_outcome=none` (new / never finished a run).

### 1.4 Legacy `status` backfill

| Old status | phase | busy | last_outcome |
|------------|-------|------|--------------|
| running | open | running | none |
| cancelling | open | cancelling | none |
| completed | open | none | completed |
| failed | open | none | failed |
| cancelled | open | none | cancelled |
| idle | open | none | none |
| deleted | deleted | none | none |

### 1.5 Origin backfill (R1.7) — deterministic

```sql
-- Layer2 exists
UPDATE conversations SET origin = 'hub_created'
WHERE id IN (
  SELECT DISTINCT conv_id FROM messages
  WHERE source = 'local_turn' AND current_projection = 1
);

-- Remaining with no messages and no agent_session_id unlikely; if agent_session_id set
-- and no Layer2: imported_list (may include empty creates — create path MUST set hub_created going forward)
UPDATE conversations SET origin = 'imported_list'
WHERE origin IS NULL OR origin = '';

-- Safety: any row created only via session/new after this migration sets origin in app code.
```

**Forward create:** `session/new` / hub create path **always** sets `origin=hub_created`, `interaction=writable`, `phase=open`, `busy=none`, `last_outcome=none` **before** any message exists (fixes empty-create poison).

**Interaction backfill (after origin):**

```sql
UPDATE conversations SET interaction = CASE
  WHEN origin = 'hub_created' THEN 'writable'
  ELSE 'read_only'
END;
```

Legacy `status` CHECK / mirror values must include synthetic **`closed`**.

### 1.6 Interaction storage

Always **persist** `interaction` after recompute. Display **must equal** stored value.  
**Option A:** if `origin=imported_list` → force `interaction=read_only` regardless of meta (write gate).

---

## 2. SessionInfo meta contract (adapter wire)

### 2.1 Hub read order (first hit wins)

```
1. sessionInfo._meta.acp_hub.interaction ∈ {"writable","read_only"}
2. sessionInfo._meta.acp_hub.space ∈ {"acp","cli","ide","unknown"}
3. sessionInfo._meta["cursor-adapter"].space ∈ {"acp","cli","ide"}  // Cursor today
4. else space = "unknown"
```

Persist full `_meta` object into `session_meta_json` on discover upsert (merge replace remote keys).

### 2.2 Cursor mapping (normative)

| space | after bind origin=bound | notes |
|-------|-------------------------|-------|
| ide | **read_only** | never sendable; bind does not help |
| acp | **writable** | unless explicit interaction read_only |
| cli | **writable** | Phase1: if space=cli → W (Cursor adapter omits ambiguous ids from list). Bind/send failures use `resume_load_failed` / `agent_acp` — hub does **not** re-derive workspace. |
| unknown | **read_only** | prefer R |

### 2.3 Interaction recompute function (pseudo)

```
fn recompute(origin, meta) -> interaction:
  if origin == imported_list: return read_only          // Option A
  if origin == hub_created: return writable
  // origin == bound:
  if meta.interaction == read_only: return read_only
  if space == ide: return read_only
  if space == acp: return writable
  if space == cli: return writable   // Phase1 definition (no resume_prompt_ok symbol)
  return read_only  // unknown / missing
```

---

## 3. Discover algorithm (list_agent_sessions)

### 3.1 Steps

1. Call agent `session/list` (paged as today).  
2. **Do NOT** call `session/load` in this path (remove current loadSession branch for discover).  
3. For each SessionInfo with unique sid:
   - Parse meta (§2.1); compute provisional space.
   - `IN_HUB_before = EXISTS (agent_id, sid) AND phase != deleted`
   - Upsert:
     - **If missing:** INSERT origin=`imported_list`, interaction=`read_only`, phase=`open`, busy=`none`, last_outcome=`none`, title/cwd/dirs/meta from info.
     - **If exists with phase=deleted:** UPDATE same conv_id → origin=`imported_list`, interaction=`read_only`, phase=`open`, busy=`none`, last_outcome=`none`; refresh title/cwd/meta; `in_hub_before=false` for response.
     - **If exists and origin in (hub_created, bound):** **do not** change origin; refresh title/cwd per §3.2; always refresh `session_meta_json` merge; recompute interaction via §2.3 (hub_created stays W; bound may be R/W).
     - **If exists and origin=imported_list:** keep origin; refresh title/cwd/meta; interaction stays read_only.
4. Return **Hub DTO array** (not raw-only SessionInfo):

```json
{
  "agent_session_id": "...",
  "title": "...",
  "interaction": "read_only",
  "space": "ide",
  "in_hub_before": true,
  "conv_id": "..." 
}
```

CLI table: `SESSION | IX | SPACE | IN_HUB | CONV | TITLE`

### 3.2 Title/cwd merge matrix

| Local title | Remote title | Result |
|-------------|--------------|--------|
| empty/null | any | remote |
| non-empty | empty | local keep |
| non-empty | non-empty | **keep local** for hub_created/bound; for imported_list **prefer remote** (discover is source of truth for museum rows) |

cwd: same matrix.

---

## 4. Bind / create state machine

### 4.1 `conv create <agent> [--cwd] [--additional-directory…]`

- session/new → INSERT origin=`hub_created`, interaction=`writable`, phase=`open`, busy=`none`, last_outcome=`none`.
- Return `{ "conv_id", "origin", "interaction", "agent_session_id" }` (JSON mode).

### 4.2 `conv create <agent> --agent-session-id <sid> …` (bind)

| Existing row | Action |
|--------------|--------|
| none | INSERT origin=`bound`, interaction=recompute(bound, meta or unknown→R), then try ensure_live/load; if load fails → keep row, return error `resume_load_failed` with data.source, **origin stays bound** (operator can show empty/local); **never session/new** |
| imported_list | UPDATE origin=`bound`, recompute interaction, ensure_live; **same load-fail rule** as none; return same conv_id |
| bound | idempotent: recompute interaction, ensure_live; **same load-fail rule**; return same conv_id |
| hub_created | **keep origin=hub_created** (higher); recompute interaction (still W); ensure_live; **same load-fail rule**; same conv_id |
| deleted row same sid | treat as **revive**: same as missing→bound path but UPDATE phase=open, origin=bound (not insert duplicate) |

**If ensure_live/load fails after bind state write:** return `resume_load_failed` + `data.source`; origin/interaction **already committed**; do **not** session/new.

**Bind does not guarantee W** (IDE → still R).  
CLI exit 0 only when bind path fully succeeds (including optional load if required by ensure_live policy). If load fails → exit 1 with resume_load_failed (state still committed).

### 4.3 Verb gates

| Verb | imported_list | bound R | bound W | hub_created | closed |
|------|---------------|---------|---------|-------------|--------|
| list/show/search | allow | allow | allow | allow | show allow; list default hide closed |
| send | **deny** `read_only_conversation` | deny RO | allow | allow | deny `conversation_closed` |
| param/mode set | deny `read_only_conversation` | deny RO same code | allow | allow | deny `conversation_closed` |
| param/mode list | allow local snapshot | allow | allow | allow | allow local only |
| cancel | if busy≠none allow else `not_busy` | same | same | same | not_busy |
| close | allow → §4.4 | allow | allow | allow | idempotent |
| delete | §4.5 | §4.5 | §4.5 | §4.5 | §4.5 |

### 4.4 Close while busy

`close` while `busy ∈ {running, cancelling}`: set `phase=closed`, `busy=none`, **`last_outcome=failed` always** (overwrite prior last_outcome); finalize in-flight run as failed with stop reason `closed`.  
`close` while `busy=none`: set `phase=closed`; leave `last_outcome` unchanged.  
Remote close best-effort; unsupported → local closed + `warnings:["remote_close_unsupported"]`, exit **0**.

### 4.5 Delete

1. If `busy≠none` → deny `conversation_busy`.  
2. If not `--local-only` and remote delete capability: try remote session/delete; failure → continue local (warning).  
3. Soft-delete: set `phase=deleted`, `busy=none`; **keep row** (UNIQUE preserved).  
4. Success JSON: `{ "conv_id", "phase": "deleted" }`.  
5. show of deleted: allow if id known; default list excludes; Phase1 **no** `--status deleted` listing required.

---

## 5. Error wire (CLI + MCP)

### 5.1 Envelope

```json
{
  "ok": false,
  "error": {
    "code": "read_only_conversation",
    "message": "conversation is read-only; bind cannot make IDE sessions writable — create a new conversation to send",
    "data": {
      "conv_id": "…",
      "origin": "imported_list",
      "interaction": "read_only",
      "source": null
    }
  }
}
```

MCP tools: same object in error payload; also set `data.reason` = same as `code` for back-compat.

CLI non-JSON: stderr `error: <code>: <message>` exit **1**.

### 5.2 Codes Phase 1 must emit

`read_only_conversation` · `conversation_closed` · `conversation_busy` · `not_busy` · `conversation_not_found` · `agent_not_found` · `invalid_argument` · `resume_load_failed` (+ `data.source` ∈ `agent_acp|timeout|io|unsupported|internal`) · `daemon_unavailable` · `agent_spawn_failed` · `agent_acp` · `unsupported_capability` · `run_failed` · `permission_policy_reject`

### 5.2b Daemon death mid-send (single primary)

| Situation | Client return (primary) | Store before any successful new RPC | Next |
|-----------|-------------------------|-------------------------------------|------|
| Daemon dies during in-flight send | **`daemon_unavailable`** (exit 1) | recovery job or next connect: `busy=none`, `last_outcome=failed` | `conv show`; **one** re-send |
| Next CLI after daemon already recovered | normal path; list shows `failed` | same | re-send once if desired |
| Auto-spawn daemon fails | `daemon_unavailable` | same terminalization if run was open | fix home/lock; retry |

Do **not** leave `busy=running` across successful daemon restart. Do **not** dual-code OR without table (primary = `daemon_unavailable` for client mid-flight).

### 5.2c Reject policy runtime

| Event | code / exit | message MUST contain | next |
|-------|-------------|----------------------|------|
| inspect sees permission_policy=reject | exit 0 + warning | `permission_policy=reject; re-add agent with defaults or edit agents.json` | re-add or edit |
| send or create blocked because endpoint policy reject / agent denies as policy | **`permission_policy_reject`** | same substring + `permission_policy=reject` | re-add or edit; **not** `agent_spawn_failed` alone |
| hang forever | **forbidden** | — | — |

### 5.3 Examples

**read_only_conversation (IDE / imported):**

```json
{"ok":false,"error":{"code":"read_only_conversation","message":"conversation is read-only (origin=imported_list). Use conv create for a writable session, or bind only if space allows write.","data":{"conv_id":"c1","origin":"imported_list","interaction":"read_only"}}}
```

**conversation_busy:**

```json
{"ok":false,"error":{"code":"conversation_busy","message":"conversation c1 has an in-flight run","data":{"conv_id":"c1","busy":"running"}}}
```

---

## 6. List / sessions surface Phase 1

### 6.1 Default `conv list` = **workbench**

Workbench predicate:

```sql
phase = 'open' AND (
  origin IN ('hub_created','bound')
  OR EXISTS (
    SELECT 1 FROM messages m
    WHERE m.conv_id = conversations.id
      AND m.source = 'local_turn'
      AND m.current_projection = 1
  )
)
```

**ORDER BY `updated_at` DESC, `id` ASC.**

**Filter composition:**

| Flags | Result set |
|-------|------------|
| default | workbench ∧ phase=open |
| `--all` / `include_imported=true` | phase=open all origins (not workbench filter) |
| `--status S` | synthetic STATUS = S; **workbench default off**; if S=`closed` include phase=closed; deleted never listed in Phase1 |
| `--status` + `--workbench` | both AND |
| `--interaction` / `--agent` | AND with above |
| limit/offset | default limit **100** |

### 6.2 JSON list **envelope** (not bare array)

```json
{
  "items": [
    {
      "id": "…",
      "conv_id": "…",
      "agent_id": "…",
      "agent_session_id": "…",
      "origin": "hub_created",
      "interaction": "writable",
      "status": "idle",
      "phase": "open",
      "busy": "none",
      "last_outcome": "none",
      "title": "…",
      "summary_preview": null,
      "updated_at": "…"
    }
  ],
  "limit": 100,
  "offset": 0,
  "truncated": false
}
```

`truncated=true` iff more rows may exist (`total > offset + len(items)`).  
CLI `--json` prints envelope; human table prints `items` only.  
Phase 1: `summary_preview` may be null.

Human table: `CONV | AGENT | IX | ORIGIN | STATUS | TITLE | UPDATED` (IX = `W`/`R`).

### 6.3 MCP `list_conversations` params

```json
{
  "agent_id": null,
  "workbench": true,
  "include_imported": false,
  "status": null,
  "interaction": null,
  "limit": 100,
  "offset": 0
}
```

Returns **same envelope** as §6.2; compose filters as §6.1.

Tool description (exact):

> Workbench list of Hub conversations (default excludes pure remote imports). Set include_imported=true for discover museum rows. Use list_agent_sessions to discover remote sessions.

`list_agent_sessions` description (exact):

> Discover remote sessions for one agent (metadata only; does not open send). Imported rows are read-only until conv create --agent-session-id bind.

### 6.4 Send success (minimal)

Success = process exit 0. Optional JSON:

```json
{ "ok": true, "conv_id": "…", "busy": "none", "last_outcome": "completed" }
```

Operator may also re-read via list/show.

---

## 7. Close

1. Try remote session/close if capability present.  
2. Always set `phase=closed`, `busy=none` on success of **local** step (busy rules §4.4).  
3. If remote unsupported/fails: still local closed; JSON `warnings:["remote_close_unsupported"]`; CLI stderr warning; **exit 0**.  
4. Default list and `--all` / `include_imported` **never** include `phase=closed`. Closed rows **only** via `--status closed` (optionally AND other filters).

---

## 8. Concurrency

- Single-flight **per conv_id** only.  
- Parallel send on different conv_ids (incl. different agents) **allowed**.  
- Second send same conv → `conversation_busy`.

---

## 9. SC oracles Phase 1 (fixtures)

### SC-06/07 IDE RO

1. Fixture agent returns session sid=`ide-1` with `_meta.cursor-adapter.space=ide`.  
2. `agent sessions` → row IX=R SPACE=ide.  
3. `conv list --all` → origin=imported_list interaction=read_only.  
4. `send` → exit 1 code `read_only_conversation`.  
5. `conv create agent --agent-session-id ide-1` → origin=bound interaction=read_only.  
6. `send` again → still `read_only_conversation` message mentions cannot make IDE writable.  
7. `conv create agent` (new) → W → send may proceed (fixture end_turn).

### SC-FLOOD

1. sessions returns 120 sids.  
2. `conv list` (workbench default) count = 0 if no hub work.  
3. `conv list --all` count ≥ 100 (limit) with truncated flag if >100.  
4. Create one hub_created → workbench count = 1.

### SC-BIND-ACP

1. space=acp session imported R.  
2. bind → interaction=writable.  
3. send allowed (fixture).

### SC-NODEGRADE

1. hub_created row.  
2. sessions includes same sid.  
3. origin remains hub_created.

### SC-MK-BUSY

1. send in flight.  
2. second send → conversation_busy.

### SC-DAEMON

1. Start send; kill daemon mid-RPC.  
2. Client gets **`daemon_unavailable`**.  
3. Next successful hub connect: conv `busy=none`, `last_outcome=failed`.  
4. `show` works; one re-send allowed.

### SC-REJECT

1. Register agent with permission_policy=reject.  
2. `inspect` exit 0; stderr/JSON warning contains exact substring `permission_policy=reject; re-add agent with defaults or edit agents.json`.  
3. If send blocked by policy → code `permission_policy_reject` with same substring (not bare spawn_failed).

---

## 10. Phase 1 exit checklist

- [ ] Migration applied; origin+interaction backfill tests  
- [ ] discover no session/load; deleted-row revive  
- [ ] Option A send/param-set/mode-set  
- [ ] bind state machine + load-fail on all rows  
- [ ] list default workbench + envelope truncated + ORDER BY  
- [ ] sessions DTO columns  
- [ ] error envelope codes incl. permission_policy_reject  
- [ ] close-while-busy + delete soft  
- [ ] SC-06/07/FLOOD/BIND-ACP/NODEGRADE/MK-BUSY/DAEMON/REJECT green  
- [ ] Cursor adapter still emits cursor-adapter.space (hub accepts)  

**Coding gate:** this contract v1.1 is the SSOT.  
**Do not claim** full product UX (inspect/progress/transcript M1–M6) after Phase 1 alone.
