# OMP UX strengths — extraction for ACP Hub refinement

**Date:** 2026-07-24  
**Sources:** `01-omp-subagents-task-agents.md`, `raw/omp-acp-surface.md`, OMP docs (`approval-mode`, `task-agent-discovery`, session ops)  
**Agent-managed UX pillar:** `doc/ssot/agent-managed/pillars/Product-UX.md` (UX > defensive defaults; frozen `pillars/` unchanged)

---

## 1. What “feels good” about OMP (operator, not engineer)

| # | Strength | Operator experience | Mechanism (brief) |
|---|----------|---------------------|-------------------|
| U1 | **Open and work** | Install/auth once; start coding without a permission ceremony | Schema usable; explicit yolo for unattended ACP; subagents yolo after parent task |
| U2 | **Clear power switch** | “I want unattended” is one flag/config, not archaeology | `--yolo` / `tools.approvalMode` / `--config` overlay |
| U3 | **Tasks complete honestly** | Tool returns when work is done; partial results salvageable | yield contract, soft budgets, forced final yield |
| U4 | **Progress is visible** | Know something is running; subagent table / cards | Event bus, registry status, progress channels |
| U5 | **Failure is legible** | Error names tools/agents/reasons; not “daemon died” | Product-level errors; fail closed only when truly denied |
| U6 | **Continue later** | `/resume`, `--continue`, park/revive | Session JSONL + registry keep-alive |
| U7 | **Typed workers** | scout vs reviewer vs default — right tool for job | Agent definitions + spawn policy |
| U8 | **Batch + context once** | Shared Goal/Constraints; many tasks | `context` + `tasks[]` |
| U9 | **Doesn’t stall headless** | Subagents never block on UI | Forced yolo in child; parent task is auth boundary |
| U10 | **Cancel is real** | Stop works; no infinite zombie turn | Cancel lifecycle, bounded abort |
| U11 | **Stdout hygiene (ACP)** | Protocol path doesn’t spam UI into JSON-RPC | stderr/logs only |
| U12 | **Load ≠ resume** | Choose cost: full replay vs reattach | Distinct ACP methods |
| U13 | **Depth & spawn policy** | No runaway agent trees | maxRecursionDepth, spawns allowlist |
| U14 | **Artifacts addressable** | Results live at stable ids | agent://, history://, named outputs |

---

## 2. Mapping to Hub CLI (must improve)

| OMP strength | Hub today (problem) | Target Hub behavior |
|--------------|---------------------|---------------------|
| U1 Open and work | Default reject + no fs/terminal | Default auto-allow + fs/terminal; roots = cwd |
| U2 Power switch | Must memorize long register flags | Default usable; `--permission-policy reject` to tighten |
| U3 Tasks complete | send fails after successful tools (lag disconnect) | In-flight turn survives notification lag |
| U4 Progress | partial stream then death | Stable stream; optional stale notice not hard kill |
| U5 Legible failure | `daemon unavailable: resume/load…` | Distinct daemon / agent / permission / load |
| U6 Continue | ensure_live fragile + opaque | Continuous send; real errors on load fail |
| U7 Typed workers | multi-endpoint = types | Keep multi-endpoint; docs as “agent roster” |
| U10 Cancel | exists | Keep; ensure not blocked by hang |
| U11 Hygiene | good enough | Keep |
| U12 Load/resume | internal | Keep capability matrix; improve messaging |

**Not porting into CoreHub:** U7/U8/U13/U14 runtime (task tool, yield, isolation) — those stay inside OMP when registered as endpoint. Hub wins by **not blocking** that path.

---

## 3. Anti-patterns OMP avoids (hub fell into)

1. **Safety default that disables the product** — OMP keeps client gate but documents explicit unattended; hub default reject made “hello world write” a research project.  
2. **Silent or misleading failure classes** — OMP fails tools clearly; hub folded sources into daemon wording.  
3. **Integrity over completion** — OMP prefers salvage/yield; hub preferred kill connection on lag.  
4. **Ceremony before value** — OMP invests in discovery/defaults; hub invested in least-privilege samples post-review.

---

## 4. Acceptance checklist (product)

- [ ] `acp-hub agent add <id> --command …` without extra flags → agent can request write/terminal and succeed under auto-allow  
- [ ] `conv create` + `send` “create marker file” → file on disk **and** CLI exit 0 when agent ends cleanly  
- [ ] High-churn thought stream does not alone fail the send  
- [ ] Second `send` on same conv works without mysterious load errors (when agent supports it)  
- [ ] Failures name the layer (daemon / endpoint / permission / load)  
- [ ] Tightening remains one explicit policy change, not the default  

---

## 5. Relation to prior research

Earlier packs (`03-history-attribution`, `05-findings-summary`) correctly identified many behaviors as **intentional under old review pressure**.  
**Pillar 2026-07-24 overrides that product priority.** Attribution remains historically true; **direction is no longer “preserve fail-closed defaults.”**
