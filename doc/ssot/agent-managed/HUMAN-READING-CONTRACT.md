# Human-readable CLI — implementation contract (v2)

**Status:** APPROVED for implementation  
**Date:** 2026-07-25  
**Version:** 2.0 — supersedes v1.0 label-language contract  
**Authority:** [HUMAN-READING.md](./HUMAN-READING.md) v2 · [HUMAN-READING-DESIGN.md](./HUMAN-READING-DESIGN.md) v2  

**Do not implement v1 `think`/`say`/`tool`/`done` protocol pads.**

---

## 0. Gate

Implement only this contract. Amend docs before inventing formats.

---

## 1. Pure cleaners (hub `transcript_view`)

### 1.1 `clean_body`

1. Trim  
2. Strip case-insensitive prefix `content type` + following space  
3. Drop whitespace-separated tokens equal to `text` (case-insensitive)  
4. Rejoin with single spaces  

### 1.2 Tool title for human

`compact_human_body` / `with_content` for tool kinds:

1. Prefer content JSON `title` / `/toolCall/title` / `name`  
2. Else parse body for `title <Name>` until ` kind` / ` raw` / ` status` / ` toolcallid` / ` rawinput`  
3. Else filter out ids (`fc_*`, toolcallid, long alnum ids) and status junk; up to 6 tokens; else `tool`  

Thought: clean + single-line, truncate ~200.  
Other: clean_body result.

### 1.3 Merge

Keep PHASE2 thought/tool merges. Short assistant glue ≤512 chars each; do not glue larger bodies.

### 1.4 Tests

| Assert |
|--------|
| `content type text text hello` → `hello` |
| `text Creating x text with y` → no lone `text` tokens |
| tool body with `title Edit File kind` → `Edit File` |
| short `UX-RC3-`+`ASK-OK` glue; two 600-char messages stay 2 nodes |

---

## 2. Human `send` stdout (non-json)

After merge (`send_run` limits), emit each non-user non-empty item:

| kind | Format |
|------|--------|
| thought | `  {cleaned}` (two leading spaces) |
| tool_call / tool_call_update | `  {title}` (two spaces + title only) |
| else (assistant reply) | `{cleaned}` plain, **no** role tag |

Between a plain reply and a following tool line, if previous line was non-empty and non-indented, print one blank line before the tool block when useful (implement: blank line before first tool after a non-indented line).

**Completion (exactly one):**

```text
Completed in {secs:.1}s ({stop_reason})
```

No `done  ` keyword language. No `final: conv=` on human path (JSON keeps final object).

---

## 3. Human timings stderr (non-json)

```text
acp-hub: {secs:.1}s total
```

Optionally append ` (prompt {p:.1}s)` / ` (session {s:.1}s)` when those ms present.  
**Never** `Some(...)`.  
Stage lines may remain `[acp-hub] stage=...` or English `acp-hub: prompting…` — pick one family and stay consistent: **prefer** existing `stage=` if already shipped widely, but timings line MUST be human numbers.  

**Decision:** keep stage= lines (operational, already documented); fix **only** the timings Debug dump to:

```text
[acp-hub] timings total_ms=14889 prompt_ms=14886
```

(numeric only — already contracted earlier; do not invent a third style).

---

## 4. Human `conv show` body

Same stream rules as §2 for transcript items (plain reply, indented thinking/tools).  
Meta FIELD/VALUE table unchanged.

---

## 5. sessions / reveal / search

Unchanged from useful v1 product rules:

- sessions human: default limit 20, sort in-hub then acp…; `--all` full; banner if truncated; JSON full list  
- list `--reveal-paths`: full `command args…`  
- search snippet: strip content-type noise substrings  

---

## 6. Forbidden

- Fixed protocol labels `think`/`say`/`tool`/`done` as user-facing opcodes  
- New top-level commands  
- Store migrations  

---

## 7. Checklist

- [x] pure clean/title tests  
- [x] human send stream natural layout + Completed line  
- [x] show uses same stream formatter  
- [x] timings without Some(  
- [x] sessions/reveal/search as above  
- [x] tests + clippy  
