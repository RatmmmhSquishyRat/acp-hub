# ACP Hub — Implementation Plan v3 (Audit-Driven Rework)

> Grounded in: pillars `doc/ssot/pillars/README.md` + `TechSel.md`
> Audit basis: PillarAudit + CodeQualityReview + FeatureGapAnalysis + FixCorrectness (subagent reports)
> Dev principles: `doc/ssot/dev-principles/实现规划原则.md`

## Current State (verified 2026-07-02)

Build: ✅ | Tests: 24 passed | Clippy: ✅ clean (all-targets, -D warnings)

### E2E Verified Working
- Daemon lifecycle (singleton, idle exit, auto-spawn, shutdown grace)
- Agent registration (stdio, agents.json SSOT, UUID temp file writes)
- session/list → discovery + auto-import (cursor-history: 235 sessions)
- session/load → message capture (8261 messages, ALL `load_replay` source)
- send/prompt/stream/cancel (OMP live agent E2E)
- FTS5 search with snippets (`<<matched>>` highlighting)
- conv show with `[agent-original]` labels for Layer-1 messages

### Completed Changes (this session)

#### P0 — Critical Bugs (pillar-blocking)
| ID | File | Fix |
|----|------|-----|
| P0-1 | hub.rs | create_conversation creates row BEFORE LoadSession (was after) |
| P0-2 | store.rs | snapshot_json reads `Option<String>` (was `String`, crashed on NULL) |
| P0-3 | callbacks.rs | Source = `run_id.is_none()` (was `is_loading()`, race-prone) |
| #20 | hub.rs | LoadSession failure deletes orphan conversation row |
| #21 | callbacks.rs | Notification store errors logged via `tracing::warn!` (was `let _ =`) |
| #16 | hub.rs | list_agent_sessions checks `session_capabilities.list` before sending |

#### P2 — Feature Completion
| ID | File | Fix |
|----|------|-----|
| P2-3 | store.rs | FTS5 `snippet()` function in search results (was `String::new()`) |
| P2-5 | main.rs | `conv send/search/config/mode/cancel` subcommands added |

#### Already in Codebase (verified during audit)
| ID | File | Status |
|----|------|--------|
| P1-1 | endpoint.rs | UUID-suffixed temp file for atomic write ✅ |
| P1-2 | hub.rs | Registry write lock held across clone→modify→save ✅ |
| P1-3b | hub.rs | Handle evicted on register/remove ✅ |
| P1-5 | hub.rs | delete_conversation checks active_runs ✅ |
| P1-6 | daemon.rs | Shutdown grace period + abort_all ✅ |
| P1-8 | store.rs | Migration + logical writes in transactions ✅ |
| #3 | endpoint.rs | remove_proxy validates agent references ✅ |
| #15 | acp.rs | Protocol version checked after initialize ✅ |
| P2-1 | acp.rs | build_client_caps wired into InitializeRequest ✅ |
| P2-2 | acp.rs | session/list pagination loop + MAX_PAGES guard ✅ |

### Deferred (future iteration)
| ID | Description | Complexity |
|----|-------------|------------|
| P1-3a | agent_handle per-agent singleflight (lock-across-await) | High |
| P1-4 | Terminal async read + child kill-on-drop guard | High |
| P1-7 | Callback errors use responder.respond_with_error | Medium |
| P2-4 | Proxy chain initialization timeout + fast-fail | Medium |

### Adapters
```
adapters/
├── cursor/               — Custom JS adapter (reads Cursor's state.vscdb)
│   ├── adapter.js        — session/list from composerData, session/load from bubbleId
│   └── cursor-adapter.cmd — @echo off + node --experimental-sqlite wrapper
├── codex/                — Native codex-acp (Zed official, npm package)
│   └── (needs @openai/codex-win32-x64 + OPENAI_API_KEY)
└── omp/                  — Native omp acp (already working E2E)
```

### Adversarial Review History
1. PillarAudit — S1-S5, D1-D5, FAQ conformance table
2. CodeQualityReview — 27 issues (1 CRITICAL, 13 HIGH, 12 MEDIUM, 1 LOW)
3. FeatureGapAnalysis — S3 working (OMP), S4 broken (mode NULL), S5 broken (proxy hang)
4. FixCorrectness — Verified P0-1 correct, P0-2 wrong target (read not write), P0-3 premise false
5. PillarConformance — 3 missing items: prompt_capabilities, conv list auto-discovery, proxy CLI
6. CompletenessGaps — Data-loss cluster interdependency, 14 missing CQR items, 12 new risks
