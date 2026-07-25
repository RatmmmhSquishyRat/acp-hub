# Human-readable CLI output — product law

**Status:** LAW v2.0 (replaces v1.0 “grammar” mistake)  
**Date:** 2026-07-25  
**Detail design:** [HUMAN-READING-DESIGN.md](./HUMAN-READING-DESIGN.md)  
**Implement contract:** [HUMAN-READING-CONTRACT.md](./HUMAN-READING-CONTRACT.md)

---

## What this is NOT

**Not** inventing a custom “human language” (`think` / `say` / `tool` as protocol tags).  
That was a design error: it looks clever and reads like a second API, not like a CLI.

**Not** trading completeness for pretty labels. Human channel and agent/JSON channel both carry the **same truth**; human channel **presents** it.

---

## What this IS

Design **default CLI presentation** the way good CLIs do (cargo, git, docker):

| Goal | Meaning |
|------|---------|
| **Intuitive** | A person sees it and knows what happened without learning tags |
| **Natural** | Looks like normal terminal English / structure, not a invented dialect |
| **Complete** | Important facts are present (what was said, what tool ran, did it finish, how long) |
| **Useful** | Noise (wire ids, Debug dumps, museum flood) stays out of the default path |

Agent/`--json` remains full structured data. That is **not** a lower bar for humans — humans get a **better presentation** of the same facts.

---

## Hard rules

1. Default stdout is for **humans reading a terminal**.  
2. No wire debris unless `--raw` / `--json` (toolCallId, `content type`, stray `text` tokens, `Some(...)`).  
3. Noise is opt-in (`--all`, `--raw`, `--reveal-paths`).  
4. Success ends with a **plain English** completion line, not a pseudo-keyword language.  
5. Design before code; amend contract before inventing formats.

---

## Relation to Operator UX

SYSTEM / PHASE1–4 still own objects, Store, workbench, gates.  
This stack only owns **how default CLI text looks when a human glances at it**.
