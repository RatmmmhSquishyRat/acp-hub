# Adversarial review: UX rebalance design (2026-07-24)

**Reviewer stance:** adversarial, UX-first pillars (not fail-closed-over-UX)  
**Design under review:** [`ux-rebalance-2026-07-24.md`](./ux-rebalance-2026-07-24.md)  
**Authority:** [`doc/ssot/pillars/README.md`](../ssot/pillars/README.md), [`Product-UX.md`](../ssot/agent-managed/pillars/Product-UX.md)  
**Research cross-check:** [`06-omp-ux-strengths.md`](../research/omp-vs-acp-hub-2026-07-24/06-omp-ux-strengths.md), Cursor E2E investigation  
**Code skim (feasibility):** `endpoint::PermissionPolicy` / `FsConfig`, `daemon/rpc_io` Lagged, `rpc/error_data::SafeResumeSourceData`, `cli` `agent add` args + MCP register defaults

---

## Verdict (top)

**Sound enough to implement P0** (D1 usable defaults + D2 lag non-fatal + D3 minimum error-class honesty + sample/docs flip). The three technical levers map cleanly onto known call sites; partial landings already appear in-tree (e.g. `PermissionPolicy` default `AutoAllow`, CLI/MCP omit → usable, Lagged continues, `SafeResumeSourceData` splits Internal vs DaemonUnavailable).

**Residual risks (do not treat “Open Questions: None” as true):**

1. **Existing `agents.json` with explicit `"permission_policy": "reject"` / fs/terminal false will not heal** when serde/CLI defaults change — only omitted fields change. Operators who copied samples stay broken until re-register or migration.
2. **Windows E2E reliability is outside this design’s P0 box.** Cursor investigation: capability proven once; disconnect / hang / opaque resume remain primary failure surfaces. Defaults alone do not make Product-UX golden path continuous on Windows.
3. **D3 is a partial honesty fix.** Folding ACP/IO/timeout into `Internal` with a redacted generic still fails Product-UX §6’s demand for agent vs permission vs load vs daemon **actionable** distinction under resume/load.
4. **Lag-without-stale is silently incomplete projection.** Pillar allows this over kill-connection, but operators may think history is complete when it is not (OMP U4/U5 gap).
5. **Downstream SSOT (`design.md` / `spec.md` / `bdd.md` / `tdd.md` / `impl_plan.md`) not in PR plan** despite Product-UX §8 requiring them — risk of reintroducing least-privilege samples via doc drift.

---

## Pillar alignment (summary)

| Pillar demand | Design coverage | Alignment |
|---------------|-----------------|-----------|
| P0 main path: `agent add` → `conv create` → `param`/`mode` → `send` → result → second `send` | D1 unblocks tools; D2 unblocks mid-send; D3 helps second-send diagnostics. **param/mode hang, ensure_live fragility, agent-add hang not in scope** | **Partial** — correct for permission/lag slice; incomplete for full Product-UX §2/§4 checklist |
| Default = local trust usable | D1 table matches Product-UX §3.1 | **Strong** (if all surfaces listed below flip together) |
| Security opt-in, not default | D1 tighten column; Security section | **Strong** intent; tighten UX underspecified |
| Lag: buffer / degrade, not kill in-flight | D2 explicit override of R-DAEMON-004 | **Strong** for non-fatal; **weak** on resync/stale |
| Errors distinguish daemon / agent / permission / load | D3 only Internal vs DaemonUnavailable messages + “prefer RPC message” | **Weak / incomplete** |
| Docs/samples match defaults | D4 + PR5 | **Incomplete** vs Product-UX §8 file list |
| Fail-closed only when unrecoverable | Goals / Non-goals | **Strong** |

Direction of the design **matches** the 2026-07-24 pillar correction. Completeness for **shipping Product-UX P0 acceptance end-to-end** is **not** fully specified by this doc alone.

---

## Feasibility notes (code skim)

| Area | Current / relevant surface | Design action feasibility |
|------|----------------------------|---------------------------|
| `PermissionPolicy` | `endpoint.rs`: `#[default] AutoAllow` (already flipped in tree) | Trivial; must keep serde omit → AutoAllow; explicit `reject` remains |
| `FsConfig` / `ClientCapabilityConfig` | Custom `Default`: read/write/terminal true | Trivial; `#[serde(default)]` means omit → usable; explicit false preserved |
| CLI `agent add` | `args.rs`: `default_value = "auto-allow"`; `allow_*` `default_value_t = true` + `ArgAction::Set` | Feasible; tighten is `--allow-read false` not `--no-terminal` (design wording mismatch) |
| MCP register | `mcp.rs`: omit policy → `"auto-allow"`; fs/terminal `unwrap_or(true)` | Feasible; **must stay locked to CLI** in one PR |
| Lag | `daemon/rpc_io.rs`: `RecvError::Lagged` → warn + `continue` (already) | Feasible; lifecycle test inverted to “continues” |
| Channel capacity | Hub fan-out `broadcast::channel(8192)` in `callbacks/connection.rs`; design text “1024 → 8192” slightly stale / imprecise about which channel | Soft margin only; lag policy is the real fix |
| `SafeResumeSourceData` | `Internal` → `HubError::other(...)`; `DaemonUnavailable` → distinct daemon string | Minimum D3 landed; **schema still redacts agent ACP detail** |

---

## Issues

### 1. No migration story for existing agents.json with explicit reject

| Field | Value |
|-------|--------|
| **Severity** | **critical** (for operator upgrade path) |
| **Section** | D1 / Goals / Open Questions |
| **Description** | Changing Rust `Default` and CLI/MCP omit defaults **does not rewrite** disk registry entries that already serialize `"permission_policy": "reject"` and `read_text_file`/`write_text_file`/`terminal`: false. That includes every shipped sample under `adapters/*/agents.json` **after** users copy them into `~/.acp-hub`, and any endpoint added under the old least-privilege CLI default. Product-UX U1 (“装好就能干活”) and OMP checklist item 1 fail for the upgrade population unless they re-run `agent add` with new flags or edit JSON by hand. Design states “Open Questions: None blocking P0” — this is blocking for **existing** installs. |
| **Suggestion** | Choose and document one of: (a) **explicit non-migration**: CHANGELOG/README “breaking product default for *new* registration only; re-register endpoints”; (b) **one-shot CLI** `agent upgrade-defaults` / first-run warn listing reject endpoints; (c) **versioned registry migration** (risky — may surprise operators who *chose* reject). At minimum: acceptance test that loading an old reject JSON **preserves** reject (compat), plus operator doc that samples must be re-applied. Update all `adapters/*/agents.json` in the same defaults PR so *new* copies are usable. |
| **Status** | open |

---

### 2. Windows E2E / continuous golden path not in acceptance

| Field | Value |
|-------|--------|
| **Severity** | **major** (pillar P0 completeness) |
| **Section** | Goals / PR Plan / missing Acceptance |
| **Description** | Product-UX §2/§4 and OMP §4 require: create → send (marker file + exit 0) → second send; high-churn stream does not alone fail send; failures name layer. Design PR plan is unit/doc oriented. Cursor E2E investigation (same day) shows **permission ceremony was only one failure mode**; primary residual is Windows daemon/session lifecycle (disconnect, hang, resume/load). Implementing D1–D3 without a **Windows host acceptance** definition risks “defaults green in CI, product still unusable on the path that motivated the rebalance.” |
| **Suggestion** | Add an Acceptance section that copies OMP §4 checklist and marks: automated (unit/integration) vs host-local Windows manual. Explicitly list residual **out of P0** items (agent-add hang under generation lock, ensure_live vendor load fragility) so implementers do not claim full Product-UX done. Prefer landing PR2+PR3 together before advertising “Cursor daily path fixed.” |
| **Status** | open |

---

### 3. D3 under-delivers Product-UX §6 (error honesty)

| Field | Value |
|-------|--------|
| **Severity** | **major** |
| **Section** | D3 |
| **Description** | Product-UX §6: ResumeLoadFailed **source** at CLI must remain distinguishable (agent ACP, unsupported, timeout); ban folding to `daemon unavailable: resume/load operation failed`. Design only requires Internal vs DaemonUnavailable **distinct non-misleading messages** and “prefer preserving RPC message where safe.” Wire `SafeResumeSourceData` still maps `Acp` / `Io` / timeouts / nested failures to `Internal {}` with **no structured class**. Operator still cannot act differently on agent vs load vs network. “Prefer RPC message” is not a schema decision — current tests intentionally strip message content for privacy. |
| **Suggestion** | For P0 minimum: keep distinct Internal vs DaemonUnavailable strings (good). For P0+ or same PR if cheap: extend safe source with **closed tags** e.g. `agent_acp`, `timeout`, `io` (no free-form agent payload), and assert CLI error chain shows ResumeLoadFailed + tag. Document privacy boundary: what never crosses RPC. Update `rpc/tests.rs` expectations explicitly so privacy fold is intentional, not accidental opacity. |
| **Status** | open |

---

### 4. MCP ↔ CLI default parity is claimed but not specified as a single contract

| Field | Value |
|-------|--------|
| **Severity** | **major** (if any surface lags) |
| **Section** | D1 table (“serde/CLI/MCP omit”) |
| **Description** | Pillar and design say omit → usable on all entry points. Concrete surfaces that historically **diverged**: CLI clap defaults, MCP `unwrap_or("reject")` / `unwrap_or(false)`, `PermissionPolicy`/`FsConfig` Default, `--json` path (full file), samples. Design table is one row; no explicit “parity matrix” or test that MCP omit and CLI omit produce equal `AgentEndpointConfig` capability bits. A half-landed PR can “fix” CLI and leave MCP reject (or vice versa). |
| **Suggestion** | Spec table: | Surface | policy | read | write | terminal | roots |. Require one test or shared helper asserting MCP default config == CLI default config. Call out `--json` / explicit JSON **does not** apply CLI defaults for missing nested fields beyond serde Default (document). |
| **Status** | open |

---

### 5. Tighten-path UX underspecified / inconsistent with clap reality

| Field | Value |
|-------|--------|
| **Severity** | **minor** |
| **Section** | D1 “Tighten” column; Product-UX §3.1 |
| **Description** | Design: `--allow-read=false` / JSON; Product-UX mentions `--no-terminal` or equivalent. Implementation pattern `ArgAction::Set` + `default_value_t = true` means operators pass `--allow-terminal false`, not a `--no-terminal` flag. OMP U2 “one clear power switch” is only half met: policy has `--permission-policy reject`, but fs/terminal are three separate bools. Easy to leave terminal true while rejecting permissions (or reverse) — confusing. |
| **Suggestion** | Document exact clap syntax in skill/README. Optionally add convenience `--strict` / `--sandbox` that sets reject + fs false + terminal false in one shot (aligns U2). Keep granular flags. |
| **Status** | open |

---

### 6. Lag policy: no operator-visible incomplete projection

| Field | Value |
|-------|--------|
| **Severity** | **minor** (pillar allows; OMP legibility suffers) |
| **Section** | D2 |
| **Description** | D2 correctly stops `abort_requests` / connection kill on Lagged. “Optional later: mark projection stale / emit one diagnostic” is deferred. After lag, `conv show` / search may miss updates with **no** CLI signal — fails OMP U4/U5 “progress visible / failure legible,” even if send exit 0. Integrity note in design is right; observability is not. |
| **Suggestion** | P0: at least one `tracing` warn (already) + design note that CLI may show stale until reconnect. P1: single `hub/conv/stale` notification or flag on next list/show. Do not reintroduce connection-fatal as default. |
| **Status** | open |

---

### 7. Missing explicit test matrix (implementation incompleteness risk)

| Field | Value |
|-------|--------|
| **Severity** | **major** (for “complete enough to implement without thrash”) |
| **Section** | PR2–PR4 “+ tests” |
| **Description** | Design mentions tests but does not list cases. Known inverted tests that **must** move with the design: `cli_tests` registration defaults, `mcp_tests` register defaults, `lifecycle_tests` lag behavior (close → continue), `rpc/tests` resume source rehydrate strings, adapter sample JSON expectations in docs/specs, any SECURITY/README “least privilege samples” assertions, `impl_plan.md` P1 bullet “Default registration examples to rejected permission…”. Without a matrix, PR review will miss a surface and re-break CI or product. |
| **Suggestion** | Add Test Plan subsection: **must-pass** cases — (1) CLI omit → AutoAllow+fs+terminal; (2) MCP omit same; (3) serde omit on `AgentEndpointConfig` same; (4) explicit reject JSON still Reject; (5) Lagged does not close duplex / subsequent notification delivered; (6) in-flight RPC completes after lag (if testable); (7) Internal vs DaemonUnavailable rehydrate messages differ and neither claims bare “daemon unavailable: resume/load operation failed” for Internal; (8) samples `agents.json` usable defaults; (9) optional host Windows smoke for marker file + exit 0. |
| **Status** | open |

---

### 8. Product-UX §8 downstream docs out of scope of PR5

| Field | Value |
|-------|--------|
| **Severity** | **major** (doc SSOT drift) |
| **Section** | D4 / PR5 |
| **Description** | Product-UX §8 **requires** review/update of `design.md`, `spec.md`, `bdd.md`, `tdd.md`, `impl_plan.md` for least-privilege defaults and lag-fatal, plus SECURITY/README/adapters, and historical research “fail-closed conductor” → corrected. Design PR5 only README/SECURITY/skill/adapters prose. `impl_plan.md` still says sample defaults are rejected permission / disabled fs/terminal — implementers following impl_plan will fight the pillar. |
| **Suggestion** | Expand PR5 (or PR1 follow-up) to touch every §8 path: flip least-privilege sample language; note R-DAEMON-004 **product override**; leave review-book historical text with “superseded by Product-UX 2026-07-24” rather than rewriting history. Adapter specs (`cursor-adapter/spec.md`, `grok-adapter/spec.md`) still embed reject samples. |
| **Status** | open |

---

### 9. R-DAEMON-004 override not formalized for future reviewers

| Field | Value |
|-------|--------|
| **Severity** | **minor** |
| **Section** | D2 / D4 / Key Decisions |
| **Description** | Design notes product override of R-DAEMON-004. Review book still records lag-as-connection-fatal as the **closure**. Future adversarial pass can re-“fix” lag-fatal unless SSOT points at Product-UX conflict rule (P0 > review finding). |
| **Suggestion** | One paragraph in `doc/review/` or pillars changelog: R-DAEMON-004 historical; default policy now non-fatal lag; reintroduce kill only as opt-in. Link from design Key Decisions. |
| **Status** | open |

---

### 10. Scope hole: agent-add hang / ensure_live / param path

| Field | Value |
|-------|--------|
| **Severity** | **minor** if explicitly deferred; **major** if claimed as full UX rebalance |
| **Section** | Goals / Non-Goals / Background |
| **Description** | Research and Product-UX call out `agent add` hang after registry write, `param`/`mode`/`send` via `ensure_live_session`, and misleading daemon wording on load. Design Background cites these motives but Goals only cover defaults, lag, error reconstruction, docs. Risk: stakeholders read “UX rebalance” as full OMP checklist. |
| **Suggestion** | Rename Goals to “P0 slice: defaults + lag + error class + docs”; add “Deferred (known residual)” with hang/ensure_live/Windows lifecycle. Status remains Active for this slice. |
| **Status** | open |

---

### 11. Design capacity note imprecise

| Field | Value |
|-------|--------|
| **Severity** | **nit** |
| **Section** | D2 “1024 → 8192” |
| **Description** | Multiple broadcast channels exist (hub notification fan-out vs RPC client). Lag that kills CLI is the **daemon client** subscription from `hub.ctx().subscribe_notifications()`. Soft margin on wrong channel does not fix the product bug. |
| **Suggestion** | Cite `HubCtx` fan-out constant / file. State buffer increase is secondary to non-fatal Lagged. |
| **Status** | open |

---

### 12. “Open Questions: None blocking P0” is false under adversarial bar

| Field | Value |
|-------|--------|
| **Severity** | **major** (process / honesty of design) |
| **Section** | Open Questions |
| **Description** | At least migration policy (#1), Windows acceptance boundary (#2), D3 schema depth (#3), and §8 doc set (#8) are open decisions that change implementation and release notes. Claiming none blocks P0 invites incomplete landings. |
| **Suggestion** | Replace with a short open-questions list; mark which are “decide in PR description” vs “defer with residual risk.” |
| **Status** | open |

---

### 13. Interactive prompts correctly rejected; automation path needs one documented “yolo”

| Field | Value |
|-------|--------|
| **Severity** | **nit** |
| **Section** | Alternatives / D1 |
| **Description** | Alternatives correctly reject interactive permission prompts for unattended agents. OMP U2 still wants a **named** unattended mode. Default auto-allow is that mode for hub; tighten is the opposite switch. Skill cheatsheet must lead with golden path **without** flag archaeology — design D4 says this; ensure it is the **first** example, not a footnote after reject samples. |
| **Suggestion** | Skill: first block = `agent add …` (no capability flags) → create → send. Second block = sandbox flags. |
| **Status** | open |

---

### 14. Security section OK but roots/cwd Windows edge untested in design

| Field | Value |
|-------|--------|
| **Severity** | **minor** |
| **Section** | Security & Privacy / D1 roots |
| **Description** | “empty roots = session cwd” is existing behavior. Cursor E2E required explicit `--allow-root <work-dir>`. If session cwd ≠ workspace the agent edits, default “usable” still fails writes with root errors — looks like permission/UX regression. |
| **Suggestion** | Document when cwd is enough vs when `--allow-root` is required. Add Windows note in skill. Optional test: empty roots resolve to session cwd and allow write under that path. |
| **Status** | open |

---

## Completeness checklist (implementation readiness)

| Item | In design? | Ready? |
|------|------------|--------|
| Flip PermissionPolicy default | Yes (D1) | Yes |
| Flip Fs/terminal defaults | Yes (D1) | Yes |
| CLI flag defaults | Implied | Yes if ArgAction documented |
| MCP omit parity | Implied | **Specify + test** |
| Sample agents.json | D4 | **Must include all adapters** |
| Existing user migration | **No** | **Blocker for upgrade UX** |
| Lag non-fatal | Yes (D2) | Yes |
| Lag tests inverted | PR3 “lifecycle test update” | Yes if named |
| Stale/resync | Optional later | Defer OK with residual |
| Error class honesty (min) | Partial D3 | Min OK; full pillar §6 not OK |
| Product-UX §8 docs | Partial | Incomplete |
| Windows E2E acceptance | **No** | Residual risk |
| OMP §4 checklist as DoD | Research only | **Import into design** |

---

## Recommendation

1. **Proceed with P0 implementation** of D1+D2+D3+sample flip **in one change-set or tightly ordered PR2–4**, matching design note that they may land together.  
2. **Before calling Product-UX satisfied:** resolve issue #1 (migration/re-register policy), issue #4 (MCP/CLI parity test), issue #7 (test matrix), issue #8 (SSOT docs).  
3. **Track as residual (explicit):** Windows lifecycle reliability, ensure_live opacity beyond D3 min, projection stale marker, agent-add hang.  
4. **Do not** reintroduce fail-closed defaults to satisfy historical R-\*/F-\* without pillar owner override (Product-UX conflict rule).

---

## Issue index

| # | Severity | One-liner | Status |
|---|----------|-----------|--------|
| 1 | critical | No migration for existing reject agents.json | open |
| 2 | major | No Windows E2E / golden-path acceptance | open |
| 3 | major | D3 incomplete vs Product-UX §6 | open |
| 4 | major | MCP/CLI parity not a hard contract | open |
| 5 | minor | Tighten flag UX / `--no-terminal` mismatch | open |
| 6 | minor | Lag leaves silent incomplete projection | open |
| 7 | major | Missing explicit test matrix | open |
| 8 | major | Product-UX §8 SSOT docs not in PR plan | open |
| 9 | minor | R-DAEMON-004 override not formalized for reviewers | open |
| 10 | minor/major | Hang/ensure_live out of scope without labeling | open |
| 11 | nit | Channel capacity cite imprecise | open |
| 12 | major | “No open questions” is false | open |
| 13 | nit | Skill golden path ordering | open |
| 14 | minor | empty roots vs Windows workspace cwd | open |

---

*End of review.*

