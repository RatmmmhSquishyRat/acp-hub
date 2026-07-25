# Human-readable CLI — design (v2)

**Status:** DESIGN v2.0 — supersedes v1.1 “scan grammar / think-say-tool language”  
**Date:** 2026-07-25  
**Contract:** [HUMAN-READING-CONTRACT.md](./HUMAN-READING-CONTRACT.md)  
**Review:** [HUMAN-READING-REVIEW.md](./HUMAN-READING-REVIEW.md)

---

## 1. Failure of v1

v1 defined a **fixed label language** (`think` / `say` / `tool` / `done` with 6-char pads). That is:

- Not how real CLIs present work  
- Not “natural”  
- Forces users to learn a dialect we invented  

User correction: **design CLI output**, not a language.

---

## 2. Reference style (what “good CLI” looks like)

Patterns we emulate (spirit, not copy):

- **cargo:** English verbs + facts — `Compiling foo`, `Finished … in 1.23s`  
- **git status:** structured but still English sections  
- **docker compose:** service action lines, not `svc run ok` pseudo-opcodes  

For an agent turn, a human should feel they are watching a **session**, not decoding a protocol.

---

## 3. Default `send` stdout (to-be)

**As-is v1 (wrong):**

```text
think  Creating file...
say    Creating `ux-rc4.txt`...
tool   Edit File
done   end_turn  (14.9s)
```

**To-be (natural CLI):**

```text
  Creating the file with UX-RC4-OK. Then stopping.

Creating `ux-rc4.txt` with the requested line.

  Edit File

Completed in 14.9s (end_turn)
```

### Rules

| Kind of content | Presentation |
|-----------------|--------------|
| Model **thinking** | Indented 2 spaces; cleaned prose; no tag word required. Optional quiet lead-in only if needed for empty-body cases. |
| Model **reply** | **Plain paragraph**, no role prefix. This is the main content. |
| **Tool** | Indented 2 spaces + **title only** (e.g. `  Edit File`). Never toolCallId / fc_ / rawInput. |
| **User** (if shown) | Plain, or `You: …` only when mixed history needs distinction (show). |
| **Finish** | `Completed in {secs:.1}s ({stop_reason})` |

Blank line between reply and tools / between tool block and completion when it aids scan.

### stderr (progress — operational, still natural)

```text
acp-hub: connecting…
acp-hub: prompting…
acp-hub: finished
acp-hub: 14.9s total (prompt 14.9s)
```

Avoid Debug formatting. Stage lines may stay short English; not a second invented opcode set.

**JSON mode:** unchanged structured updates + final object (machine complete).

---

## 4. `conv show` human

- Keep meta as FIELD / VALUE table (already natural).  
- Message stream uses **same presentation rules as send** (plain reply, indented tools/thinking).  
- Table form if retained: ROLE = English words `thinking` / `assistant` / `tool` / `user` — **not** protocol tags `think`/`say`. BODY cleaned.  
- Prefer stream layout over pseudo-language columns when printing default human show.

**Decision for implement:** default human `show` body = same line formatter as send (stream), not `think  ` pads.

---

## 5. Other commands (keep useful; don’t invent dialects)

| Command | Human default |
|---------|----------------|
| `agent sessions` | Slice to useful set (≤20, prefer in-hub/acp); `--all` museum; one English banner if truncated |
| `agent list` + reveal | Full command line when `--reveal-paths` |
| `doctor` | English journey + state-aware next steps (already) |
| `search` | Clean snippets; no content-type noise |
| errors | Keep `error: code: message` — already CLI-natural |

---

## 6. Cleaning (still pure, still required)

Same as before for **content hygiene**, not for labeling:

- strip `content type…`  
- drop standalone vendor token `text`  
- tool title from JSON title or body `title …`  
- short chunk glue ≤512; no large-body glue  

---

## 7. Module ownership

Unchanged: pure cleaners in hub; CLI formats human stream; JSON full path separate.

---

## 8. Non-goals

- Stable 0.2.1 publish (release decision)  
- TUI / colors required (optional later)  
- Hiding thinking entirely by default (completeness: keep, but quiet indent)  
- Replacing Store-first / workbench semantics  

---

## 9. Acceptance (human glance test)

On a successful write send, from **stdout alone**:

1. Can read what the model said in plain text  
2. Can see which tool action ran by name  
3. Can see it finished and how long  

Without learning any house “grammar”.
