# Human Reading — Implementation Contract

**Status:** APPROVED for implementation (see REVIEW)  
**Date:** 2026-07-25  
**Version:** 1.0  
**Authority:** [HUMAN-READING.md](./HUMAN-READING.md) · [HUMAN-READING-DESIGN.md](./HUMAN-READING-DESIGN.md)  
**Scope:** Human default channel + pure cleaners. Does not invent MCP tools or Store columns.

---

## 0. Gate

- Implement **only** rules in this document.  
- If ambiguous: stop and amend CONTRACT + REVIEW; do not invent.  
- Frozen `doc/ssot/pillars/*` untouched.

---

## 1. Pure cleaners (`store/transcript_view` or equivalent pure module)

### 1.1 `clean_body(s) -> String`

MUST:

1. Trim.  
2. If case-insensitive prefix `content type`, strip that prefix and following whitespace.  
3. Split on whitespace; **drop** tokens equal to `text` (case-insensitive).  
4. Re-join with single spaces; trim.

### 1.2 `human_role_label(role, kind) -> &'static str`

| kind / role | label |
|-------------|-------|
| kind=thought | `think` |
| kind=tool_call or tool_call_update | `tool` |
| role=user | `you` |
| else | `say` |

### 1.3 `compact_human_body(kind, body, content?) -> String`

- kind thought: `clean_body` then single-line, truncate 200 chars.  
- kind tool_*:  
  - If `content.title` or `/toolCall/title` or `name` string non-empty → use it (truncate 80).  
  - Else parse body for substring after `title ` until ` kind` / ` raw` / ` status` / ` toolcallid` / ` rawinput` (case-insensitive markers).  
  - Else filter tokens: drop toolcallid, `fc_*`, id-like (≥16 alnum with digit, or long hyphenated), and status keywords; take up to 6 tokens; if empty → `tool`.  
- else: `clean_body` only (preserve newlines for show multi-line answers if caller wants; send may single-line).

### 1.4 Merge (existing + clarify)

- Thought merge / tool_call id merge: keep PHASE2.  
- Short assistant message glue: only when each piece `clean_body` char count ≤ 512; do not glue larger bodies.  
- Show defaults: MergeLimits show (200 nodes / 256KiB).  
- Send run display: MergeLimits send_run (no node/byte cap).

### 1.5 Tests (MUST)

| Test | Assert |
|------|--------|
| clean drops content-type and text tokens | `content type text text hello` → `hello` |
| tool title from body | body with `title Edit File kind` → `Edit File` |
| no toolCallId in tool compact | output excludes `toolcallid` and `fc_` |
| short chunk glue | `UX-RC3-` + `ASK-OK` → one node |
| large bodies not glued | three >512 char messages stay separate |

---

## 2. Human send path (CLI)

### 2.1 Streaming / end-state body (stdout, non-json)

After merge with send_run limits, for each view item with role≠user and non-empty body:

```
{label:<6}{compact_body}
```

Example: `think  Creating file...` / `tool   Edit File` / `say    Done.`

### 2.2 Done line (stdout, non-json)

Exactly one after body:

```
done  {stop_reason}  ({seconds:.1}s)
```

where seconds = total_ms/1000.  
MUST NOT print Rust Debug of Option.  
MUST NOT require `final: conv=...` for human mode (json keeps `type=final`).

### 2.3 Timings (stderr, non-json)

```
[acp-hub] timings total_ms={n}[ prompt_ms={n}][ session_ms={n}]
```

Only include keys that are present (Some). Never `Some(...)`.

### 2.4 JSON mode

Unchanged completeness: progress NDJSON stderr; updates + final JSON stdout with timings object numeric options.

### 2.5 Tests

| Test | Assert |
|------|--------|
| format line tool | starts with `tool`, contains Edit File, no fc_ |
| done line | starts with `done`, has reason, no `Some(` |
| timings line | `prompt_ms=90` not `Some(90)` |

---

## 3. Human show path (CLI)

- Meta FIELD/VALUE table unchanged (already human).  
- Transcript ROLE column uses `human_role_label`.  
- BODY uses compact (tools = title).  
- `--raw` / `--json`: existing envelopes; raw unmerged.

---

## 4. agent sessions (CLI presentation)

### 4.1 Args

- `--all`: full list.  
- `--limit N` default **20** (human table only).  
- `--json`: full array from hub (no client-side drop) unless later version documents otherwise — **v1: JSON = full RPC result**.

### 4.2 Human default sort key

Ascending priority tuple:

1. `in_hub_before == true` first  
2. space: acp=0, cli=1, ide=2, other=3  
3. non-empty title first  

Then take first `limit` rows.

### 4.3 Truncation banner

If not `--all` and total > shown:

```
showing {shown} of {total} sessions (prefer in-hub/acp). Use --all for museum.
```

### 4.4 Empty

```
No remote sessions (museum empty). Create with: conv create {id}
```

---

## 5. agent list reveal (CLI)

When `--reveal-paths` and listing from local Registry:

- TARGET = `command` + space-joined `args` (full strings).  
- HTTP/WS: full url string.

When not reveal: keep short executable + `<N argument(s)>` / redacted url behavior.

Inspect reveal: overlay full agent config from Registry (already allowed).

---

## 6. doctor

No new commands. MUST keep:

- cache empty → probe next  
- cache ready → create next (not mandatory probe)  
- ASCII journey  
- progress channel + lifecycle + reveal tip  

---

## 7. search human table

Snippet display: after fetch, strip `content type text text` / `content type text` substrings before table. Do not change FTS engine.

---

## 8. Out of contract (forbidden in this PR without amendment)

- New top-level CLI commands  
- Store migrations  
- Changing PHASE1 origin/interaction rules  
- crates.io stable publish  
- Server-side sessions filter RPC  

---

## 9. Implementation checklist (exit)

- [x] §1 pure tests green  
- [x] §2 send human grammar + done + timings  
- [x] §3 show labels  
- [x] §4 sessions default slice  
- [x] §5 reveal full cmdline  
- [x] hub+cli tests + clippy (PR CI must stay green)  
- [x] INDEX/WORKFLOW/PLAN/README linked  

**Coding may begin only when REVIEW = APPROVED.** (This package: APPROVED before implement.)
