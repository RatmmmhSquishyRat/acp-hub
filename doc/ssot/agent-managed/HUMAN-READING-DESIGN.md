# Human Reading — Full Design

**Status:** DESIGN v1.1 — closed for coding after REVIEW APPROVED  
**Date:** 2026-07-25  
**Inputs:** QA rc.2 / rc.3 / rc.4 walkthroughs; SYSTEM; PHASE1–4; user mandate (human > agent UX bar)  
**Outputs:** [HUMAN-READING-CONTRACT.md](./HUMAN-READING-CONTRACT.md)

---

## A. Problem statement

### A.1 What is already good (do not rebuild)

From rc.4 QA (A− workbench):

- doctor journey, probe honesty, workbench list, Store-first, error codes  
- progress on stderr, reveal-paths flag exists  
- sessions no longer crash daemon  
- main path write/ask/close trusted this host  

### A.2 What is still wrong (design target)

| ID | Symptom (evidence) | Root cause class |
|----|--------------------|------------------|
| HR-1 | send `[tool]` still shows call ids / raw dumps | Human formatter is residual, not designed grammar |
| HR-2 | thinking has `text Creating… text with…` | clean rules incomplete vs vendor chunks |
| HR-3 | sessions dumps museum history | No product default for "what to open next" |
| HR-4 | `--reveal-paths` list TARGET still `node <1 arg>` | Reveal only half-implemented on list path |
| HR-5 | timings `prompt_ms=Some(14886)` | Human path uses Debug format |
| HR-6 | success ends as technical `final:` | No human "done" grammar |
| HR-7 | show ROLE still semi-wire | Labels not unified with send |
| HR-8 | (release) stable 0.2.0 lag | Distribution — design notes only, separate release decision |

**Wrong prior approach:** patch one formatter per QA bullet without a closed interaction grammar → inconsistent half-products.

**Correct approach:** design the **human scan language** + per-command contracts → review → implement once.

---

## B. Design principles (expanded)

1. **Human ultra-scan > agent completeness on the human channel.** Completeness lives in JSON/MCP.  
2. **Grammar over adjectives.** Labels and line budgets are specified; "more readable" is not a requirement.  
3. **Default = current work.** Museum/history is `--all` / explicit.  
4. **Same cleaning pure functions** for send, show, search snippet, previews.  
5. **ASCII-safe defaults** for Windows consoles (no fancy unicode in product strings).  
6. **No second Store.** All human lines project Store/RPC truth; no invented durable state.

---

## C. Human scan grammar (to-be)

### C.1 Labels (fixed, ASCII, 6-char pad in streaming lines)

| Label | Use |
|-------|-----|
| `you` | User content when shown |
| `think` | Merged reasoning |
| `say` | Merged model reply |
| `tool` | One tool **title** (optional short path/status suffix only if free of ids) |
| `done` | Turn complete: reason + duration |
| `err` / `warn` | Operator failures (existing error: code: msg stays) |

### C.2 Example — successful write send (human stdout only)

**As-is (rc.4, bad):**

```text
[thinking] text Creating ux-rc4.txt text with the single line text UX-RC4-OK. ...
[assistant] text Creating `ux-rc4.txt` ...
[tool] fc_… title Edit File kind edit rawInput | … status in_progress
[assistant] text Done.
final: conv=… stop_reason=end_turn
```

**To-be (required):**

```text
think  Creating ux-rc4.txt with the single line UX-RC4-OK. Then stopping.
say    Creating `ux-rc4.txt` with the requested line.
tool   Edit File
say    Done.
done   end_turn  (14.9s)
```

**stderr (unchanged channel split):**

```text
[acp-hub] stage=daemon_connect
[acp-hub] stage=prompt
[acp-hub] stage=end
[acp-hub] timings total_ms=14889 prompt_ms=14886
```

### C.3 show

- Header: keep FIELD/VALUE table (origin, interaction, status, phase, busy, last_outcome) — already scannable.  
- Body ROLE column: `think` / `say` / `tool` / `you` (same grammar).  
- BODY: cleaned; tools = title only.  
- SEQ may skip (merged nodes) — document as view seq, not wire seq; optional note `(merged)` when merged_count>1 in JSON only.

### C.4 agent sessions

| Mode | Behavior |
|------|----------|
| **Default** | Max **20** rows; sort prefer `in_hub_before=true`, then space `acp` > `cli` > `ide` > unknown, then titled; print one header line if truncated: `showing N of M (use --all)`. |
| **`--all`** | Full museum (current behavior). |
| **Empty** | `No remote sessions. Create with: conv create <id>`. |

### C.5 agent list + reveal

| Mode | TARGET column |
|------|----------------|
| Default | Short: executable name + `<N argument(s)>` (redacted paths OK) |
| `--reveal-paths` | **Full** `command arg1 arg2 …` from local `agents.json` (trusted debug) |

### C.6 doctor

Already strong. Keep cache-aware next steps. Ensure journey ASCII. No new wild commands.

### C.7 search

Snippet: strip content-type and standalone `text` tokens; leave FTS `[highlight]` as-is (agent useful; human acceptable) unless trivial strip of brackets breaks ranking display — **do not** reimplement FTS.

### C.8 Optional files summary (P2, in-scope if cheap)

If any human `tool` line title matches edit/write/create patterns, after tools and before `done`, one line:

```text
files  Edit File (+N tools)
```

If cannot detect reliably from titles, **omit** (no fake summary). CONTRACT marks this SHOULD not MUST.

---

## D. Cleaning rules (shared pure module)

Owned by pure functions (no I/O), used by send/show/search display:

1. Strip leading `content type` (case-insensitive).  
2. Remove whitespace-separated token equal to `text` (vendor marker).  
3. Tool human title: prefer content JSON `title`; else parse body for `title <Name>` before `kind`/`raw`/`status`; drop tokens matching toolCallId, `fc_*`, long hex/uuid-like.  
4. Short assistant message chunks (≤512 chars) glue; never glue large bodies.  
5. Thought: single-line collapse for streaming; full cleaned multi-line allowed in show if needed (default single-line for send).

---

## E. Module ownership (implementation map — not code)

| Concern | Owner layer |
|---------|-------------|
| clean / tool title / merge short chunks | `store/transcript_view` (pure) |
| human labels + done + timings format | CLI `output` / send path |
| sessions default slice | CLI `agent sessions` only (RPC still returns full list; CLI filters display) |
| reveal full command | CLI list path reading Registry when reveal set |
| JSON/MCP | unchanged completeness; optional camelCase fields already there |

**Important:** sessions filter is **presentation-layer** by default so agent/MCP can still get full list unless a later contract adds server-side filter (out of v1).

---

## F. Phased delivery (after contract APPROVED)

| Step | Deliverable | Exit |
|------|-------------|------|
| D0 | Law + DESIGN + CONTRACT + REVIEW APPROVED | this package |
| I1 | pure clean/title/label + unit tests | cargo test pure |
| I2 | send/show human grammar + done + timings | cli unit + contract tests |
| I3 | sessions default slice + list reveal full cmdline | cli tests |
| I4 | docs INDEX/WORKFLOW/CHANGELOG; optional rc tag | PR green |

No rc version bump until I1–I3 green.

---

## G. Explicit non-goals (v1)

- Stable crates.io 0.2.1 (release decision separate; design may recommend).  
- Mid-turn kill / concurrency test suite (stability program).  
- Changing Store schema.  
- Humanizing every MCP tool description.  
- Replacing table UI with TUI.

---

## H. Traceability (QA → design)

| QA ID | Design section |
|-------|----------------|
| UX-RC4-1 | C.2 tool title, D.3 |
| UX-RC4-2 | D.2, C.2 think |
| UX-RC4-3 | C.4 sessions |
| UX-RC4-4 | C.5 reveal |
| UX-RC4-5 | G non-goal / recommend only |
| UX-RC4-6 | note only; timings human format HR-5/C.2 stderr |
| UX-RC4-7 | C.2 timings |
| UX-RC4-8 | C.3 SEQ note |
| UX-RC4-9 | C.7 |
| UX-RC4-10 | C.8 SHOULD |

---

## I. Open questions — **closed in design**

| Q | Decision |
|---|----------|
| Chinese labels? | **No** for v1 — ASCII English labels for Windows + agents |
| Filter sessions in daemon? | **No** v1 — CLI presentation filter |
| files line MUST? | **SHOULD** — only if title heuristic confident |
| Change JSON field names? | **No** — human only; JSON keeps existing ViewMessage |

---

## J. Sign-off

Design is complete for coding **only after** HUMAN-READING-REVIEW status = **APPROVED**.  
Implementers implement **CONTRACT**, not free interpretation of this DESIGN.
