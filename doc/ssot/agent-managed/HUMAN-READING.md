# Human Reading Law — ACP Hub

**Status:** LAW (binding product principle)  
**Date:** 2026-07-25  
**Version:** 1.0  
**Closed by:** [HUMAN-READING-DESIGN.md](./HUMAN-READING-DESIGN.md) · [HUMAN-READING-CONTRACT.md](./HUMAN-READING-CONTRACT.md) · [HUMAN-READING-REVIEW.md](./HUMAN-READING-REVIEW.md)

---

## 0. Why this document exists

Operator UX Phases 1–4 made Hub **usable**. QA (rc.2→rc.4) proved the **direction** is right but human output still leaks wire/Debug noise.

User mandate (2026-07-25):

> 人类可读和 agent 可读 **完全不冲突**。人类可读、超快速阅读标准是 **比纯 agent UX 更高一级** 的要求，**不是取舍**。

This law sits **above** feature polish patches. Implementation **must not** start until the design + contract are APPROVED (see REVIEW).

---

## 1. Hard law (non-negotiable)

| # | Law |
|---|-----|
| H1 | **Human channel first.** Default TTY (no `--json`) is optimized for a human to understand in seconds. |
| H2 | **Agent channel is full, not privileged.** `--json` / MCP expose complete structure; they never force the human channel to stay technical. |
| H3 | **One glance = one fact.** Each human line answers one question. |
| H4 | **No wire debris in human mode** unless `--raw` / `--json`: no toolCallId, no `content type`, no stray vendor `text` tokens, no Rust `Debug` (`Some(...)`). |
| H5 | **Noise is opt-in.** Museum dumps, full tool payloads, full paths require explicit flags. |
| H6 | **Success is unmistakable.** A completed send ends with a short human **done** line (reason + time). |
| H7 | **Design before code.** No runtime field invention outside an APPROVED contract for this surface. |

---

## 2. Relationship to existing SSOT

```
frozen pillars (do not edit)
    → Product-UX
    → OPERATOR-UX-CHARTER / SYSTEM / PHASE1–4  (objects, journeys, Store, gates)
    → HUMAN-READING law (this file)           (scan quality of human channel)
    → HUMAN-READING-DESIGN                    (full interaction design)
    → HUMAN-READING-CONTRACT                  (coding SSOT)
    → implementation + tests
```

- **SYSTEM / PHASE\*** still own objects, Store-first, workbench, discover, probe, progress stages.
- **This stack owns how humans read defaults** — labels, cleaning, list noise, done/timings formatting.
- Does **not** reopen Store-first, Option A, IDE forever RO, or lag policy.

---

## 3. Two channels (same product, different surfaces)

| Channel | When | Goal |
|---------|------|------|
| **Human** | default CLI, non-json tables/lines | Ultra-fast scan, zero wire debris |
| **Agent** | `--json`, MCP tools | Complete stable fields for machines |

Both must stay **semantically consistent** (same truth). Human is a **projection**, not a second product.

---

## 4. Success criteria (product)

A human watching only **stdout** of a successful write `send` can answer without log diving:

1. What did the model say / intend?  
2. What tool action ran (title, not id)?  
3. Did the turn finish (reason + time)?  

Default `agent sessions` must not dump an entire museum on first open.

---

## 5. Document control

| Doc | Role | Code allowed? |
|------|------|----------------|
| This law | Principles | No alone |
| DESIGN | Full as-is / to-be / per-command | No |
| CONTRACT | Frozen implementable rules | **Yes, only after REVIEW APPROVED** |
| REVIEW | Adversarial close | Gate |

**Frozen pillars:** never edited under this program.
