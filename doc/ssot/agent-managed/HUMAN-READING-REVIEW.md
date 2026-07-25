# Human Reading — Design Review

**Date:** 2026-07-25  
**Subject:** HUMAN-READING law v1.0 + DESIGN v1.1 + CONTRACT v1.0  
**Method:** Adversarial product / implementability / QA-trace review (single agent, three passes)

---

## Pass 1 — Product (human ultra-scan bar)

| Check | Result | Note |
|-------|--------|------|
| Human vs agent not a tradeoff | **PASS** | H1/H2 explicit; JSON full; human projection |
| Grammar concrete (not "nicer") | **PASS** | Labels + budgets + before/after example |
| QA P1 mapped | **PASS** | DESIGN §H table |
| Noise opt-in | **PASS** | sessions --all; raw; reveal |
| Success unmistakable | **PASS** | done line |

**Findings fixed into DESIGN/CONTRACT:**

- F1: files summary was vague → CONTRACT § files as SHOULD omit if weak (DESIGN C.8).  
- F2: JSON sessions filter ambiguity → CONTRACT §4.1 JSON = full RPC.  

---

## Pass 2 — Implementability

| Check | Result | Note |
|-------|--------|------|
| Pure vs I/O split | **PASS** | cleaners pure; CLI presentation |
| No Store migration | **PASS** | non-goal |
| No new wire fields required | **PASS** | human-only formatting |
| Test oracles listed | **PASS** | CONTRACT §1.5 §2.5 |
| sessions filter location clear | **PASS** | CLI only v1 |

**Findings fixed:**

- F3: tool title parse markers specified in CONTRACT 1.3.  
- F4: large body non-glue threshold 512 aligned with existing short-chunk rule.  

---

## Pass 3 — Anti-mess / process

| Check | Result | Note |
|-------|--------|------|
| Forbids code before APPROVED | **PASS** | Law H7 + CONTRACT §0 + this status |
| Does not pretend already shipped | **PASS** | Implementation checklist open |
| Links control plane | **PASS** | INDEX/WORKFLOW to update with this package |
| Frozen pillars safe | **PASS** | stated |

**Prior process failure (recorded):**

Partial code edits without closed design (rc.5 attempt) → **reverted** from working tree before this package. Correct process: design close → implement → release.

---

## Residual risks (accepted)

| Risk | Handling |
|------|----------|
| Vendor tool body without `title` | Fall back to filtered tokens or `tool` |
| files line weak heuristic | SHOULD omit |
| Stable 0.2.1 not in contract | Explicit non-goal; recommend in ship notes later |

---

## Verdict

# **APPROVED**

**HUMAN-READING-CONTRACT v1.0 may be implemented.**

No further design blockers. Implementation PRs must cite this REVIEW and check CONTRACT §9 checklist.

| Role | Outcome |
|------|---------|
| Product | APPROVE |
| Implementability | APPROVE |
| Process / anti-mess | APPROVE (after code revert + package close) |
