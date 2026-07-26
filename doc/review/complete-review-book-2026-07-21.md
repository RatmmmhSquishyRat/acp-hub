# ACP Hub Complete Review Book

**Date:** 2026-07-21  
**Reviewed revision:** `05c2d2c6dcd9fc6f644d80e60cf0641d697bad16` (`main`, post `v0.2.0` publication closure)  
**Crates:** `acp-hub-core` / `acp-hub-cli` **0.2.0**  
**Authority:** `doc/ssot/pillars/` → `doc/ssot/dev-principles/` → implementation → `doc/dev/*` → tests/adapters

> This book is a **fresh whole-repository review** of the published 0.2.0 line.
> It does **not** treat the 2026-07-18 review ledger’s “all Critical/High/Medium
> closed” publication claim as still binding for pillar intent. Prior F-\* /
> R-\* closures remain historical evidence that many defects were fixed; this
> review re-opens the product against pillars and looks for remaining gaps,
> incorrect assumptions, security/privacy issues, and runtime hazards.

---

## 1. Purpose

Review whether the current repository satisfies the project pillars as a
**complete, general, non-opinionated ACP client and conductor**, with attention
to:

1. **Functional completeness** — Spec S1–S5, Design D1–D5, FAQ two-layer model
2. **Pillar understanding correctness** — implementation semantics match pillar
   intent (not merely nearby tests passing)
3. **Assumption correctness** — no false assumptions that break generality
4. **Security and privacy (去隐私性)** — credentials, paths, argv, DB, adapters
5. **Runtime safety** — bounds, ownership, concurrency, recovery, IPC

This review is documentation-only. It does not claim a green hosted CI matrix
for this commit beyond what publication records already established for
`v0.2.0`.

---

## 2. Source of truth

Precedence used by this review:

1. `doc/ssot/pillars/README.md` (Intro / Spec / design / FAQ)
2. `doc/ssot/pillars/TechSel.md`
3. `doc/ssot/dev-principles/实现规划原则.md`
4. Current implementation under `crates/` and `adapters/`
5. `doc/dev/spec.md`, `design.md`, BDD/TDD/impl_plan, adapter specs
6. Tests, skill, README, CHANGELOG, SECURITY.md
7. Prior review book `doc/review/complete-review-book-2026-07-18.md` as
   historical evidence only

Research transcripts and machine-local validation artifacts are evidence, not
requirements.

---

## 3. Pillars (verbatim summary of binding intent)

From `doc/ssot/pillars/README.md`:

| ID | Binding intent |
|----|----------------|
| **S1** | Register ACP Agent Endpoints like MCP (stdio / HTTP / WebSocket); users may ship their own adapters |
| **S2** | Global keyword search; create/delete conversations per endpoint; view messages |
| **S3** | Send on a conversation, wait for reply, view reply |
| **S4** | Set full message parameters (model / thinking / mode / …) beyond slash-command coverage |
| **S5** | ACP proxies for outbound pre-processing and inbound post-processing |
| **D1** | Register stdio JSON-RPC / HTTP / WebSocket endpoints |
| **D2** | Unified `AgentEndpoint` abstraction; runtime capability negotiation; operable surface depends on negotiated capabilities |
| **D3** | Interact only through that abstraction |
| **D4** | Hub is ACP Client **and** Conductor: `Client(Hub) – Conductor(Hub)&Proxies – Agents` |
| **D5** | On-demand singleton daemon; CLI/MCP/lib discover+lock via files; interprocess JSON-RPC; idle exit |
| **FAQ** | Two parallel layers: Agent Original vs Hub Capture. Hub must CRUD endpoint-discoverable sessions, not only Hub-created ones. Fallback to Hub capture only when list/load are unavailable. Static snapshots must be recorded whenever an ACP call succeeds |
| **TechSel** | Small project → **no MVP**; full implementation; Rust + current best-practice crates |

---

## 4. Acceptance model

Repository completion against pillars requires:

1. **Protocol correctness** — ACP v1 enforced; sessions isolated by endpoint;
   callbacks cannot escape registered permissions/ownership
2. **Persistence correctness** — Agent Original and Hub Capture remain
   independently visible, ordered, recoverable; no silent Layer-1 wipe
3. **Discovery correctness** — endpoint-existing sessions are operable without
   requiring Hub `session/new` as the only path
4. **Interface correctness** — CLI, MCP, library expose intentional surfaces;
   multimodal/params promised by S4 are reachable or explicitly delimited
5. **Security / privacy correctness** — credentials redacted; sensitive files
   restricted; FS/terminal bound; no argv/log path leakage of secrets
6. **Assumption generality** — Hub core has no vendor-private parsers; adapters
   are fail-closed, OS-portable by default or explicit override
7. **Runtime safety** — bounds, cancel/delete races, recovery, IPC handshake,
   no unbounded materialization

A green unit suite alone is not acceptance.

---

## 5. Executive verdict

**Overall:** ACP Hub 0.2.0 is a **substantially complete** local ACP
client/conductor. Prior maintenance closed a large class of critical defects
(session collision, capability advertisement, daemon concurrency, two-layer
message commit mechanics, redaction, release packaging). Against the pillars as
written, the product is **not yet closed**.

| Dimension | Verdict |
|-----------|---------|
| Functional completeness (S1–S5 / D1–D5) | **Mostly complete**; S4 and FAQ discovery/static-snapshot remain incomplete |
| Pillar understanding | **Mostly correct**; two residual semantic mismatches (resume-as-refresh; conv list vs agent sessions) |
| Assumption generality | **Core clean**; adapters carry vendor/OS/schema assumptions; one Hub FS footgun |
| Security / privacy | **Strong baseline**; several high footguns remain under enabled FS/terminal |
| Runtime safety | **Strong**; handshake/server env/tmp-socket residuals are medium |

**Open findings this review:** Critical **1**, High **7**, Medium **9**, Low **5**.

---

## 6. System map (as implemented)

```
CLI / MCP / HubClient
        │  interprocess JSON-RPC + handshake (client-enforced)
        ▼
 on-demand singleton daemon  (idle exit, lock, daemon.json)
        │
   CoreHub (registry · conversations · prompt · conductor)
        │
   ACP SDK Conductor + Proxies (stdio) → Agents (stdio/HTTP/WS)
        │
   callbacks (permission / fs / terminal / capture)
        │
   SQLite hub.db  (local_turn ‖ load_replay ‖ snapshots ‖ FTS)
```

Vendor private-store bridges live only under `adapters/cursor` and
`adapters/grok`. Hub core does not parse Cursor/Grok storage (boundary holds).

---

## 7. Pillar alignment matrix

| Pillar | Status | Notes |
|--------|--------|-------|
| S1 Register endpoints | **Met** | stdio/HTTP/WS agents; `agents.json`; CLI + MCP |
| S2 Search + CRUD | **Mostly met** | Store CRUD + FTS strong; agent-existing sessions need explicit `agent sessions` import before `conv list` |
| S3 Send / wait / view | **Mostly met** | Core awaits prompt; CLI/MCP post-fetch; library can subscribe to live updates, facades do not stream |
| S4 Params / modes / content modes | **Partial** | param/mode set works; facades are **text-only**; core multimodal gates exist but are unreachable from CLI/MCP |
| S5 Proxies | **Mostly met** | Conductor chain works; proxies are **stdio-only** (documented SDK limit) |
| D1 Transports | **Met** for agents; proxies stdio-only | |
| D2 Abstraction + negotiation | **Partial** | Unified config + DynConnectTo; initialize + per-op gates; no min-capability connect policy; set_param/set_mode ungated |
| D3 Unified interaction | **Met** | |
| D4 Client + Conductor | **Met** | |
| D5 On-demand daemon | **Met** | lock, metadata, idle exit, tests |
| FAQ two-layer messages | **Mostly met** | `load_replay` vs `local_turn` independent for **load** refreshes; **resume-as-refresh** can empty Layer-1 |
| FAQ discover existing sessions | **Partial** | Implemented via `agent sessions` / MCP `list_agent_sessions`, not via `conv list` alone |
| FAQ static snapshots | **Partial** | Messages/config/modes/plan/commands/usage largely covered; `SessionInfo.updated_at` / `_meta` incomplete vs `doc/dev/spec.md` §9 |
| TechSel full implementation | **Mostly** | Core is deep; S4 facade + FAQ gaps contradict “no MVP” completeness claim |
| Core vs adapter boundary | **Met** | No vendor parsers in `crates/` |

---

## 8. Functional completeness detail

### 8.1 Implemented and verified (strong)

- Endpoint registry: stdio / HTTP / WebSocket agents; proxy chain (stdio)
- Capability initialize to ACP **v1 only**; disconnect on mismatch
- Conversation create (new + bind `--agent-session-id`), list, show, close, delete
- Prompt send with param/mode overlays; cancel with CAS ownership
- Two-layer message store with begin/commit/rollback load-replay
- Crash recovery for orphaned runs and interrupted load-replays
- Search with combined pagination and snippets
- Daemon: singleton lock, concurrent RPC, notification lag fail-closed, idle exit
- Bounded transports (stdio/HTTP/SSE/WS) with retained-byte budgets
- MCP management parity largely restored (register, sessions, cancel, paging)
- Public DTO redaction of commands/args/env/headers/URL secrets/`allowed_roots`
- File modes for hub home / agents.json / hub.db / sockets
- Release packaging allowlist + archive verification (established at `v0.2.0`)

### 8.2 Incomplete vs pillars / current spec

| Gap | Spec / pillar link | Evidence |
|-----|--------------------|----------|
| `ensure_live_session` prefers `session/resume` then commits via `commit_load_replay` | FAQ two-layer; design replay semantics | `conversation.rs:721–776`, `store/replay.rs:357–367` |
| `conv list` reads SQLite only | FAQ CRUD existing endpoint sessions | `conversation.rs:329–335` |
| `session/list` always `cwd: None` | Generality of agent session spaces | `registry.rs:187–189` |
| List import ignores agent `updated_at`; `apply_session_info` writes Hub clock | Spec §9 static snapshot | `registry.rs:201–235`, `lifecycle.rs:367–371` |
| No privacy projection / size-type validation for `_meta` on ordinary reads | Spec §9 | `session_meta` returned raw on `ConversationRow` |
| No typed partial-import error with completed count | Spec §9 | Mid-batch failure returns other errors; prior commits retained without typed count |
| CLI/MCP prompts are `ContentBlock::Text` only | S4 | `commands.rs` send path; `mcp.rs` send_message |
| Proxies cannot be HTTP/WS | S5/D1 symmetry | `endpoint.rs` `ProxyTransport::Stdio` only |
| `MessageSource::AgentList` unused | Schema vs design | Enum/schema only |

---

## 9. Findings

Severity legend: **Critical** (pillar-breaking data loss / security boundary failure),
**High** (incorrect product behavior or serious footgun), **Medium** (real gap,
workaround exists or scope limited), **Low** (polish / docs / residual).

### Critical

#### C-001 — `session/resume` used as Layer-1 refresh can wipe Agent Original messages

Severity: **critical**  
Pillar: FAQ two-layer independence; Spec static snapshot fidelity

**Behavior:** Before each prompt/param/mode, `ensure_live_session` prefers
`session/resume` when the agent advertises resume. Both Resume and Load paths
call `begin_load_replay` → ACP method → `commit_load_replay_with_static`.
Commit **always** demotes prior `load_replay` rows with `seq <= starting_seq`
to non-current (`current_projection = 0`), even when resume emitted **zero**
history updates.

**Impact:**

- A successful empty resume (common for “make session live” without replaying
  history) **supersedes** the entire Agent Original projection
- Users who imported via `agent sessions` / `session/load` can lose Layer-1
  visibility while Layer-2 Hub capture remains — the opposite of FAQ intent
- Confuses ACP **resume** (continue a live session) with **load** (replay
  history into Hub)

**Evidence:**

```721:776:crates/hub/src/hub/conversation.rs
        if handle.capabilities.session_capabilities.resume.is_some() {
            match self
                .refresh_session_projection_owned(
                    ...
                        method: ReplayMethod::Resume,
```

```357:367:crates/hub/src/store/replay.rs
        tx.execute(
            "UPDATE messages
             SET current_projection = 0, superseded_by_load_id = ?
             WHERE conv_id = ?
               AND source = 'load_replay'
               AND current_projection = 1
               AND seq <= ?",
```

**Closure:**

1. Treat resume-for-liveness separately from load-replay refresh
2. Only commit a Layer-1 replacement after a method that is defined to stream
   history (typically `session/load`), or after resume that actually delivered
   replay updates under an explicit refresh contract
3. Empty resume must not demote existing `load_replay` rows
4. Add a regression: import + load messages → empty resume → Layer-1 still
   current; local_turn untouched

---

### High

#### H-001 — Empty `allowed_roots` falls back to session cwd (agent-influenced on import)

Severity: **high** (security footgun)  
Area: filesystem callback sandbox

When FS callbacks are enabled and `allowed_roots` is empty, `resolve` uses the
session cwd as the sole root. Imported sessions persist **agent-supplied**
absolute cwd. Registry validation does not reject FS-enabled + empty roots.

**Evidence:** `permission_filesystem.rs:155–161`; `endpoint.rs:354–383`
(validate does not check roots); `registry.rs:202–273` (agent cwd persisted).

**Closure:** Reject FS-enabled configs with empty roots; never use
agent-listed cwd as the only jail without operator-chosen roots; regression
test import+read with agent cwd outside intended roots.

---

#### H-002 — FS read path lacks final-component symlink hardening

Severity: **high**  
Area: filesystem TOCTOU

Writes use `O_NOFOLLOW` / reparse-point open after resolve. Reads use
`fs::File::open` after canonicalize with no equivalent.

**Evidence:** `permission_filesystem.rs:45–47` vs `174–197`; write race tests
in `resolve_tests.rs`.

**Closure:** Open reads with `O_NOFOLLOW` (or open+fstat re-check under root);
add read-side race test mirroring writes.

---

#### H-003 — `terminal: true` is unrestricted process spawn

Severity: **high** (documented trust-boundary blast radius)

Terminal create spawns `req.command` with `req.args` and merges env into the
Hub process environment. Cwd is confined; command/args/env are not. Default
samples correctly disable terminal — one registry flip equals full
user-equivalent RCE via Hub.

**Evidence:** `callbacks/terminal.rs:283–307`; `SECURITY.md` same-user model.

**Closure:** Keep deny-by-default samples; document loudly; consider
allowlists / `env_clear` + explicit env; refuse `terminal` unless
`allowed_roots` non-empty.

---

#### H-004 — Agent Original static snapshot incomplete for `updated_at` / `_meta`

Severity: **high**  
Pillar: FAQ “全量记录静态资源 snapshot”; Spec §9

List import stores title/cwd/dirs only. `apply_session_info` ignores the agent
`updated_at` value and writes `now_iso()`. Ordinary reads lack size/type
validation and privacy projection for `_meta`.

**Evidence:** `registry.rs:201–235`; `lifecycle.rs:367–371`; Spec §9 lines
190–193.

**Closure:** Persist agent `updated_at` and bounded `_meta`; project/redact on
ordinary CLI/MCP DTOs; keep raw in store; round-trip + privacy tests.

---

#### H-005 — Endpoint-existing sessions are not visible via `conv list` alone

Severity: **high** (pillar FAQ)  
Pillar: “必然是要能够 CRUD 当前已经存在的对话”

`list_conversations` reads only the local store. Agent sessions enter after
explicit `agent sessions` / MCP `list_agent_sessions`. The capability exists,
but the default conversation UX contradicts FAQ wording that Hub operates on
endpoint-findable sessions, not only Hub-created ones.

**Evidence:** `conversation.rs:329–335`; `registry.rs:179–303`.

**Closure:** Either capability-gated auto-discover on `conv list`, or make
CLI/MCP UX force discover and document FAQ as “discover then CRUD” with BDD
that empty store + list-capable agent yields Layer-1 rows without Hub
`session/new`. Prefer auto-discover if “no MVP / full implementation” is kept.

---

#### H-006 — CLI/MCP cannot send non-text content blocks

Severity: **high** against S4 + TechSel “完整实现”

Core enforces image/audio/embedded capabilities before prompt. Facades only
construct `ContentBlock::Text`.

**Evidence:** `capabilities.rs`; CLI/MCP send paths.

**Closure:** Accept ContentBlock JSON (or file/image flags) on CLI/MCP; keep
capability rejection before ensure-live / run / user-message write; or
explicitly amend pillars to delimit facades as text-first (requires pillar
edit by author — **do not silently rewrite**).

---

#### H-007 — Cursor IDE default DB path is Windows-shaped

Severity: **high** (adapter generality)

Default `IDE_DB_PATH` joins `%APPDATA%/Cursor/...`. On Linux/macOS without
`CURSOR_DB_PATH`, IDE discovery uses a bad/empty path.

**Evidence:** `adapters/cursor/adapter.mjs:46–48`.

**Closure:** Platform defaults for macOS
(`~/Library/Application Support/Cursor/...`) and Linux
(`~/.config/Cursor/...`); keep `CURSOR_DB_PATH` override; fixture coverage.

---

### Medium

#### M-001 — `session/list` always passes `cwd: None`

Agents that scope sessions by cwd may under-report. Plumb optional cwd from
CLI/MCP/RPC; document semantics; fixture with cwd-filtered list.

**Evidence:** `registry.rs:187–189`.

---

#### M-002 — No typed partial-import error with completed count

Spec §9 requires typed partial-import with completed count when the Nth
session fails after earlier commits. Current errors wrap load failures without
that contract shape.

**Closure:** Typed error `{ completed, failed_session_id, source }`; contract
tests.

---

#### M-003 — Daemon handshake is client-enforced only

Server answers `hub/daemon/handshake` but still dispatches other methods
without a per-connection handshook gate. `HubClient` checks compatibility;
raw `RpcClient::connect` does not.

**Evidence:** `daemon/rpc_io.rs:536–547`; `hub/client.rs` vs `rpc.rs`.

**Closure:** Per-connection flag; reject non-handshake methods until
compatible; keep client check.

---

#### M-004 — Deep-home Unix socket fallback under temp dir has create/chmod race

Preferred `$home/daemon.sock` is fine. Fallback under `std::env::temp_dir()`
uses `create_dir_all` then chmod — multi-user `/tmp` window.

**Evidence:** `daemon.rs:529–575`.

**Closure:** Exclusive `mkdtemp`-style dir; refuse pre-existing non-owned
parent; or abstract sockets where available.

---

#### M-005 — Stdio agents and terminals inherit full Hub environment

Spawn paths add registry env but do not `env_clear`. Secrets in the Hub
process environment leak into children.

**Evidence:** `transport.rs`; `terminal.rs`.

**Closure:** Clear then apply allowlisted/registry env; tests that parent
secrets do not appear in child environ.

---

#### M-006 — Projection DB stores and FTS-indexes plaintext prompts

By design under local trust; still a privacy surface. No at-rest encryption;
search increases accidental disclosure.

**Evidence:** `store.rs` schema + FTS; `SECURITY.md:40–41`.

**Closure:** Optional encrypt-at-rest / ephemeral mode; redact high-entropy
tokens from FTS; warn on search of sensitive homes.

---

#### M-007 — Capability negotiation incomplete vs D2 wording

Initialize + per-op gates exist. No connect-time minimum capability matrix.
`set_param` / `set_mode` sent without presence checks.

**Closure:** Define min/optional matrix; gate set_config/mode if ACP exposes
presence; inspect shows coverage.

---

#### M-008 — CLI/MCP send does not stream live mid-turn updates

Daemon broadcasts `hub/conv/update`; library can subscribe; facades wait then
page stored messages.

**Closure:** Subscribe during send, or document intentional batch semantics in
pillars-facing docs.

---

#### M-009 — List-without-load imports metadata-only rows

When `load_session` is false, import upserts conversation metadata without
Layer-1 messages. Acceptable fallback path, but weaker than “尽可能完整体现”
static snapshot language.

**Evidence:** `registry.rs:247–258`.

**Closure:** Document clearly; surface capability gap in inspect/UX.

---

### Low

#### L-001 — Sensitive files chmod’d after read; prior world-readable mode not rejected

`Registry::load` hardens after read. Historical exposure before Hub start is
undetected.

---

#### L-002 — `std::sync::Mutex::expect("mutex poisoned")` on HTTP connection id

`bounded_transport.rs` — prefer `parking_lot` or map_err.

---

#### L-003 — Dead `agent_list` message source + CLI label path

Remove from CHECK/CLI or use as conversation provenance as design states.

---

#### L-004 — Search table UI omits `source` column

JSON includes source; human table does not. Add SOURCE or `--source` filter.

---

#### L-005 — Cursor header comment still says “read-only continuation” for CLI prompt

Body correctly states resume may append. Fix header table wording.

---

## 10. Assumption inventory (generality)

### 10.1 Hub core — correct / good

| Assumption | Assessment |
|------------|------------|
| ACP v1 only | Correct and enforced |
| Sessions keyed by `(agent_id, session_id)` | Correct (prior F-001 closed) |
| Caller must supply absolute cwd | Correct (prior F-006 closed) |
| Vendor private stores stay in adapters | Correct |
| Proxies stdio-only this SDK line | Documented limit, not a silent assumption |
| Same OS user trust for registered agents | Explicit in SECURITY.md |

### 10.2 Incorrect or over-strong assumptions

| ID | Assumption | Why it hurts generality | Finding |
|----|------------|-------------------------|---------|
| A1 | Resume streaming equals Layer-1 refresh | Empty resume wipes original projection | C-001 |
| A2 | Empty FS roots ⇒ cwd jail is safe | Agent cwd can widen jail | H-001 |
| A3 | `session/list` without cwd is complete | Misses cwd-scoped agents | M-001 |
| A4 | Facades are text-complete for S4 | Multimodal unreachable | H-006 |
| A5 | `conv list` alone is the conversation surface | Hides agent-existing sessions | H-005 |
| A6 | Cursor IDE default path is APPDATA-shaped | Breaks Linux/macOS IDE discovery | H-007 |
| A7 | Vendor schemas/CLI flags are stable | Fail-closed (good) but operationally brittle | residual |
| A8 | UUID-only local session ids | Drops non-UUID vendor sessions | residual adapter |
| A9 | Handshake on client is enough | Raw IPC peers skip gate | M-003 |
| A10 | Child processes need parent env | Leaks secrets | M-005 |

### 10.3 Adapter assumptions (accepted if documented + fail-closed)

Cursor: `~/.cursor` chats/acp-sessions layouts; IDE `state.vscdb` schema;
Node ≥ 22.13; CLI `--resume --mode ask`; UUID session ids.

Grok: `~/.grok` session buckets; `summary.json` + `chat_history.jsonl`;
fixed CLI flags; auto-authenticate after initialize; live tombstones until
restart.

Codex / omp: registration samples only; no private-store bridge — if ACP lacks
history, Hub capture-only (matches FAQ fallback).

---

## 11. Security and privacy assessment

### 11.1 Done well

1. Ordinary public DTOs redact command/args/env/headers/URL secrets and omit
   `allowed_roots`
2. Hub home `0700`; agents.json / hub.db / daemon metadata / sockets hardened
3. Callbacks bound by `(agent_id, session_id)` + connection generation
4. FS writes refuse symlink leaf follow; capability AND of advertised + bound
5. Cursor prompt via stdin bootstrap; Grok `--prompt-file` mode `0600` + cleanup
6. Transport error sanitization strips handshake/body secrets
7. Permission default `Reject`; adapter samples disable FS/terminal
8. Delete blocked while run active/cancelling
9. Honest local-trust threat model in SECURITY.md

### 11.2 Remaining privacy / security issues

| Finding | Class |
|---------|-------|
| H-001 empty roots / agent cwd | Authorization boundary |
| H-002 read symlink TOCTOU | Authorization boundary |
| H-003 terminal unrestricted spawn | Blast radius |
| M-004 tmp socket race | IPC on multi-user hosts |
| M-005 env inheritance | Secret leakage to children |
| M-006 plaintext DB + FTS | At-rest privacy |
| L-001 prior world-readable agents.json | Detection gap |

**去隐私性 (de-identification / non-disclosure of private local data):**

- Ordinary inspect paths no longer emit private roots or command paths (good;
  prior R-PRIV-001 / 0.2.0 redaction)
- Adapter normal diagnostics are path-free (prior F-031)
- Residual: Hub projections still materialize full vendor conversation text
  into `hub.db` + FTS after import/prompt — expected for a history Hub, but
  operators need clearer “import = copy into Hub home” privacy docs
- Residual: Grok prompt path remains on argv (`--prompt-file`); content itself
  is not in argv

---

## 12. Runtime safety assessment

### 12.1 Strong areas

- Concurrent RPC on one connection with cancel (prior F-005)
- Resource budgets: frames, SSE, callbacks, retained RPC bytes, terminal quotas
- Replay begin/commit/rollback + interrupted refresh recovery
- Run/cancel CAS serialization; notification-send rollback
- Notification lag closes connection (no silent gap)
- Active-run deletion conflict
- Endpoint replace/delete rejects agents with active runs
- Idle exit when clients/rpcs/runs are quiescent
- Module sizes under ~900-line proactive split boundary

### 12.2 Residual runtime risks

| Finding | Risk |
|---------|------|
| C-001 | Logical data loss under normal prompt path |
| M-003 | Version-skew logic bugs via raw IPC |
| M-004 | Socket hijack window on shared `/tmp` |
| L-002 | Poisoned mutex abort on HTTP id path |
| Warm agent handles outside idle counters | By design; idle exit tears them down |

---

## 13. Prior review ledger disposition

The 2026-07-18 book’s F-001…F-032 and later R-\* items were largely closed in
the 0.2.0 maintenance and publication path. This review **does not reopen**
those IDs as unresolved defects of the old wording, except where current code
still violates pillar intent:

| Prior theme | 0.2.0 status | This review |
|-------------|--------------|-------------|
| SessionKey isolation (F-001) | Resolved | Still holds |
| Replay before parent (F-002) | Resolved | Still holds |
| FS/terminal capability (F-003) | Resolved for ownership gates | H-001/H-002/H-003 remain as policy/TOCTOU/blast-radius |
| Two-layer independence (F-007) | Resolved for **load** refresh | **C-001** shows resume path still unsafe |
| Redaction / permissions (F-016/F-017) | Resolved baseline | M-004/M-005/L-001 residuals |
| MCP coverage (F-018) | Largely resolved | H-006 multimodal still missing |
| Adapter read vs resume docs (F-020) | Docs improved | L-005 header drift; H-007 path portability |
| Publication / packaging | Closed at v0.2.0 | Not re-litigated here |

---

## 14. Spec / docs / process notes

- `doc/dev/spec.md` already requires typed partial-import, `_meta` privacy
  projection, and agent `updated_at` persistence — implementation lags the
  maintained spec on those rows (H-004, M-002)
- Dev-principles require five docs + adversarial review before non-small
  work; this review book itself is an audit artifact, not a design change
- `docs/dev/` session-log path from some agent rules is **absent**; project
  uses `doc/` (no `docs/dev/`). No session-log write was required by that rule
- Pillar conflicts must not be silently rewritten. If S4 text-only facades or
  “discover-then-list” UX are intentional, the **author** must amend pillars

---

## 15. Recommended closure order

1. **C-001** — separate resume-liveness from load-replay commit (data integrity)
2. **H-001 + H-002** — FS roots required + read `O_NOFOLLOW`
3. **H-004 + M-002** — static snapshot + typed partial-import (spec already requires)
4. **H-005** — discovery UX vs FAQ (product decision or auto-import)
5. **H-006** — multimodal facade or explicit pillar delimitation
6. **H-007** — Cursor IDE portable defaults
7. **H-003 + M-005 + M-003 + M-004** — terminal/env/handshake/tmp socket hardening
8. Medium/Low polish (streaming docs, agent_list, search SOURCE, comments)

---

## 16. Verification recommendations (for a future fix PR)

Not executed as acceptance of this documentation-only review. When closing
findings, require:

| Surface | Evidence |
|---------|----------|
| Format / Clippy / tests | `cargo fmt`, warnings-denied clippy, `--locked` workspace tests |
| C-001 | Empty resume preserves Layer-1; load refresh still replaces Layer-1 only |
| H-001/H-002 | Registry reject empty roots; read symlink race fails closed |
| H-004/M-002 | `updated_at`/`_meta` round-trip; typed partial-import contract |
| H-005 | BDD: list-capable agent → operable conversations without Hub `session/new` |
| H-006 | CLI/MCP image prompt rejected or accepted per capability |
| H-007 | Linux/macOS IDE default path fixtures |
| Adapters | Isolated fixture suites; no live vendor mutation by default |
| Privacy | Contract tests that ordinary inspect lacks secrets/paths |

---

## 17. Residual operational boundaries (not findings)

- Registered agents/proxies are operator-chosen executables with Hub privileges
- Live destructive Cursor/Grok probes against real user sessions remain
  opt-in and were not run for this review
- Hosted CI / crates.io / GitHub Release evidence for **0.2.0** stands at tag
  `v0.2.0` / PR #29; this review targets post-publication `main` including
  Dependabot lock refresh (#30) and publication closure docs (#31)
- Network exposure of the local daemon socket is out of scope per SECURITY.md

---

## 18. Summary counts

| Severity | Open in this review |
|----------|---------------------|
| Critical | 1 (C-001) |
| High | 7 (H-001…H-007) |
| Medium | 9 (M-001…M-009) |
| Low | 5 (L-001…L-005) |

**Bottom line:** ACP Hub 0.2.0 is a serious, largely hardened local ACP Hub.
Against its own pillars, it is **not yet a completed “no MVP” product**: the
resume-as-Layer-1-refresh path can erase Agent Original history; conversation
discovery and S4 multimodal surfaces under-deliver FAQ/S4; and FS/terminal
defaults still contain high-severity operator footguns when capabilities are
enabled. Close Critical/High before claiming pillar completion again.

---

## Appendix A — Key file index

| Area | Paths |
|------|-------|
| Pillars | `doc/ssot/pillars/README.md`, `TechSel.md` |
| Spec | `doc/dev/spec.md` |
| Core hub | `crates/hub/src/hub/{conversation,registry,prompt,client}.rs` |
| Store / replay | `crates/hub/src/store.rs`, `store/{replay,lifecycle,snapshots}.rs` |
| Callbacks | `crates/hub/src/callbacks/{permission_filesystem,terminal,capture,connection}.rs` |
| Daemon / IPC | `crates/hub/src/daemon.rs`, `daemon/rpc_io.rs`, `rpc.rs` |
| Transport | `crates/hub/src/bounded_transport.rs`, `transport.rs`, `endpoint.rs` |
| CLI / MCP | `crates/cli/src/{args,commands,mcp,output}.rs` |
| Adapters | `adapters/{cursor,grok,codex,omp}/` |
| Prior review | `doc/review/complete-review-book-2026-07-18.md` |
| Security model | `SECURITY.md` |

## Appendix B — Review method

1. Read pillars, TechSel, spec, SECURITY, CHANGELOG, prior review book
2. Parallel deep dives: Hub functional completeness; security/privacy/runtime;
   adapters and assumption inventory
3. Direct verification of Critical/High evidence at cited file regions on
   `05c2d2c`
4. Findings written only where observable in code or durable docs; no silent
   pillar rewrites

*End of Review Book — 2026-07-21*
