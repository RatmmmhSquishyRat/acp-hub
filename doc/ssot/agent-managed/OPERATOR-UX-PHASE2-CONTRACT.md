# Operator UX — Phase 2 Contract (transcript / preview / search)

**Status:** APPROVED for implementation  
**Date:** 2026-07-24  
**Authority:** [OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md) §F.3 / §F.6 / §F.7 / §G.9  
**Scope:** Readable operator surface only. Does not invent Phase-3 progress or Phase-4 doctor.

## 1. Transcript view (F-READ)

- Pure merge over Store `MessageRow`s; **never mutates** Store.
- Order: store page order (seq ascending for default materialization).
- Consecutive `kind=thought` → one view node; body joined with `\n`.
- Consecutive tool_call / tool_call_update with same `toolCallId` → one node; kind/title status from last.
- `clean_body`: strip `(?i)^content type\s+`; collapse `text text`.
- Caps: 200 view nodes **or** 256 KiB body bytes → `truncated=true`.
- `--raw` / `raw=true`: unmerged Store rows (existing message shape).
- Default show/send human path uses **merged** view.
- Envelope: `{ items: ViewMessage[], truncated, rawCount, viewCount }`.
- ViewMessage: `seq, role, kind?, bodyText, source, mergedCount`.

## 2. summary_preview (F.6)

- Prefer latest user body (cleaned), else latest non-thought assistant, else title.
- Unicode char truncate ≤80 + ellipsis.
- Present on list JSON as `summaryPreview` (nullable) and human TITLE column may show title; preview in JSON/show.

## 3. Search hits (F.7 / F-SRCH)

Each hit includes at least:

`kind, agentId, convId, interaction, origin, snippet (≤120 display), updatedAt`  
(plus existing rank/role/messageId when message hit).

Default limit 20 when caller passes 0 (CLI may keep higher explicit limits).

## 4. Show Layer1 refresh honesty

- Optional `layer1Refreshed: boolean` on show JSON when hub attempted session/load to fill empty Layer1.
- Phase-2 minimum: field present and honest (`false` when not attempted). Full auto-load ON may ship when agent supports load; must never silent session/new.

## 5. SC oracles

| ID | Assert |
|----|--------|
| SC-13 | ≥10 thought Store rows → default view_count == 1 (or ≪10); raw keeps multi-row |
| SC-PREVIEW | summary_preview prefers latest user |
| SC-SEARCH | hit carries interaction + origin |

## 6. Non-goals

Progress stages, inspect probe, doctor, pin/archive.
