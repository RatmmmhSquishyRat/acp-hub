# Human-readable CLI — design review (v2)

**Date:** 2026-07-25  
**Subject:** HUMAN-READING v2 (natural CLI, not invented language)

---

## Finding on v1

| Issue | Severity |
|-------|----------|
| Invented `think`/`say`/`tool`/`done` as a user-facing dialect | **P0 product failure** |
| Framed “grammar” as the deliverable | Missed “intuitive natural CLI output” |

User correction accepted: **design CLI output, not a language.**

---

## v2 checks

| Check | Result |
|-------|--------|
| Natural terminal presentation | **PASS** — plain reply, indented tools/thinking, English completion |
| Complete useful info | **PASS** — speech + tool title + duration/reason |
| Agent/JSON still full | **PASS** |
| No protocol tags required to read | **PASS** |
| Implementable pure cleaners | **PASS** |
| QA noise items still addressed | **PASS** — title-only tools, text strip, sessions slice, reveal, timings |

---

## Verdict

# **APPROVED (v2)**

Implement HUMAN-READING-CONTRACT **v2.0** only.  
v1 label pads are **rejected** and must be removed from code if present.
