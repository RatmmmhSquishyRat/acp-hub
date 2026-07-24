# Design: UX-first rebalance for ACP Hub CLI

**Status:** Active implementation design (aligned to pillars 2026-07-24)  
**Frozen pillars:** `doc/ssot/pillars/` (read-only)  
**Agent-managed UX pillar:** `doc/ssot/agent-managed/pillars/Product-UX.md`  
**Research:** `doc/research/omp-vs-acp-hub-2026-07-24/`

---

## Overview

Rebalance defaults and connection/error behavior so the hub CLI is **complete and usable** as a daily ACP client — able to replace embedded ACP wiring and feel like a subagent-capable hub — without abandoning multi-endpoint architecture, dual-layer projection, or optional tight security modes.

## Background & Motivation

Post-0.2.0 review (least privilege samples, R-DAEMON-004 lag-fatal, RPC error folding) optimized for **defensive correctness** and left the **default operator path unusable** (permission reject, fs/terminal off, mid-send disconnect, opaque resume errors). Pillars never authorized security-over-UX; product intent is full client UX.

## Goals

1. Default registration path can perform real agent work (fs + terminal + auto-allow).  
2. Notification lag does not abort in-flight CLI turns by default.  
3. Operator-facing errors distinguish daemon vs agent vs permission vs load.  
4. Docs/samples/skill match the new defaults.  
5. Explicit opt-in remains for reject / deny-fs / deny-terminal.

## Non-Goals

- Porting OMP task/subagent runtime into CoreHub.  
- Removing dual-layer history or daemon architecture.  
- Global unbounded resources or disabling all privacy redaction on inspect.

## Proposed Design

### D1 — Usable defaults

| Surface | New default | Tighten |
|---------|-------------|---------|
| `PermissionPolicy` serde/CLI/MCP omit | `auto-allow` | `reject` / `auto-cancel` |
| `fs.read_text_file` / `write_text_file` | `true` | CLI `--allow-read=false` etc. / JSON |
| `terminal` | `true` | `false` in config |
| `allowed_roots` | empty → session cwd (existing) | explicit roots |

### D2 — Lag policy (pillar override of R-DAEMON-004 default)

On `broadcast::RecvError::Lagged`:

- **Log warn** with skipped count.  
- **Do not** set `abort_requests` / do not close client solely for lag.  
- Optional later: mark projection stale / emit one diagnostic notification.  
- Increase hub notification channel capacity (e.g. 1024 → 8192) as soft margin.

Integrity: best-effort capture remains; incomplete projection is preferable to failed successful turns.

### D3 — Error honesty

- `SafeResumeSourceData::Internal` / `DaemonUnavailable` deserialize to distinct, non-misleading `HubError` messages.  
- Prefer preserving RPC `message` when reconstructing where safe.  
- CLI continues to print error chain.

### D4 — Docs

- README / SECURITY / adapters samples: default usable; document lock-down flags.  
- Skill cheatsheet: golden path without auto-allow ceremony.  
- Note R-DAEMON-004 product override in design changelog.

## Key Decisions

1. **Default = local trust usable** — hub is a personal CLI, not multi-tenant SaaS.  
2. **Lag continues connection** — UX pillar beats silent-gap fear for default mode.  
3. **Architecture unchanged** — daemon, layers, adapters stay; only priority/defaults/error paths change.

## Alternatives Considered

| Alternative | Why not |
|-------------|---------|
| Keep reject default + better docs only | Still blocks first-run; violates Product-UX |
| Interactive permission prompts in CLI | Large UX surface; agents need unattended for automation |
| Remove notification fanout | Loses live projection value |
| Per-home “strict mode” only | Good later; defaults must already work |

## Security & Privacy

- Roots still constrain fs when set; empty roots = cwd only (existing resolve).  
- Ordinary inspect redaction unchanged.  
- Operators who need sandbox use reject + no-fs explicitly.

## PR Plan

| PR | Scope |
|----|--------|
| PR1 | Pillars + this design + OMP UX research (docs) |
| PR2 | Defaults: endpoint Default, CLI, MCP, samples, tests |
| PR3 | Lag non-fatal + buffer + lifecycle test update |
| PR4 | Error reconstruction honesty + tests |
| PR5 | README/SECURITY/skill/adapters prose |

Implementation may land PR2–4 in one change-set if tightly coupled.

## Open Questions

None blocking P0 — pillar owner directed UX-first.
