# Plan: hang / RPC / named-pipe lifecycle — default practice

| Field | Value |
|-------|--------|
| **Nature** | Implementer-owned **living plan** (not operator SSOT; not a work-report close-out) |
| **Date** | 2026-08-16 |
| **Status** | **Decided** 2026-08-16. §3.A–§3.D and §5–§6 filled. **P0–P3 implemented and reviewed** 2026-08-16. **P4** optional TCP not started. |
| **Verdict** | **Accept** — `doc/dev/decision-ipc-lifecycle-default-practice-2026-08-16.md` |
| **Code** | Decision turn did not implement. **P0–P3** are in the tree (unreleased). **P4** not started. |
| **Frozen inputs (read-only)** | `doc/dev/ux-*.md` operator notes; `doc/ssot/pillars/*` |
| **Related evidence (read-only)** | Archaeology at `7dcffe0`; B2 Job/spawn `doc/dev/research-windows-daemon-spawn-2026-08-16.md`; B1 / B3 / B4 `doc/dev/research-named-pipe-tokio-2026-08-16.md`; B5 RPC lifecycle `doc/dev/research-rpc-daemon-lifecycle-2026-08-16.md`; B6 survey `doc/dev/research-comparable-ipc-daemons-2026-08-16.md`; critique `doc/dev/critique-hang-fixes-rc7-rc9-2026-08-16.md`; `work-report-rc6-p0-*.md`, `work-report-error-hiding-audit-2026-07-26.md`, `AUDIT-error-hiding-self-review-2026-07-26.md`, `doc/research/omp-vs-acp-hub-2026-07-24/02-acp-hub-architecture.md`, `doc/dev/cursor-adapter/e2e-investigation-2026-07-24.md` |

This file is the working skeleton for a deep audit and redesign of CLI↔daemon IPC, JSON-RPC request lifetime, and Windows named-pipe / Unix-socket teardown. **§5–§6 are now the accepted default practice.** The normative memo is `doc/dev/decision-ipc-lifecycle-default-practice-2026-08-16.md`. Do not treat rc.8/rc.9 patches as the lifecycle law.

### Decision block (plan §0 step 5)

| Field | Value |
|-------|--------|
| **Date** | 2026-08-16 |
| **Verdict** | **Accept** §5 / §6 as written below |
| **Normative memo** | `doc/dev/decision-ipc-lifecycle-default-practice-2026-08-16.md` |
| **Research siblings (all read)** | named-pipe / Tokio; windows-daemon-spawn; comparable IPC; RPC lifecycle; critique rc.7–rc.9 |
| **Implementation** | **P0–P3 done** (reviewed 2026-08-16). **P4** not started. |

---

## 0. Workflow — how this audit proceeds

Order is fixed. Later steps may not start by rewriting earlier ones into “done.”

| Step | Work | Output in this file (or a dated sibling under `doc/dev/`) | Code? |
|------|------|----------------------------------------------------------|-------|
| **1. Code archaeology** | Map ownership and waits: `RpcClient` connect/call/Drop, daemon spawn + `daemon.lock` / `daemon.json`, handshake, RPC dispatch, cancel / registry mutate, idle exit, Windows pipe + Job/process-tree | **Done** — §3.A at `7dcffe0`. §9 A.* checked | No |
| **2. Industry research** | Named-pipe / Unix-socket teardown, async Drop vs I/O, JSON-RPC over a byte stream, Windows Job objects, process breakaway, comparable local-daemon CLIs | §3.B notes + citations. Fill §9 items B.* | No |
| **3. Critique** | For each prior patch: what it made honest, what wait it left, what contract it blurred | **Done** — §3.C + `doc/dev/critique-hang-fixes-rc7-rc9-2026-08-16.md`. §9 C.* checked | No |
| **4. Synthesis** | One default practice that names owners, return-vs-exit, Drop rules, and failure visibility | §5 filled; §6/§7/§8 tightened. Fill §9 items D.* | No |
| **5. Written decision** | Explicit accept / revise / reject of §5. If the change is larger than a local fix, also produce or update spec / design / BDD / TDD per `doc/ssot/dev-principles/实现规划原则.md`, then adversarial review | Decision block at top of this file (date + verdict) | No |
| **6. Implementation** | Only after step 5 | Work-report + tests; do **not** write back into `ux-*.md`. **P0–P3 done** 2026-08-16; **P4** not started | Yes |

Rules for every step:

1. Prefer `code:path` / `doc:path` / `git:sha` over memory.
2. Do not treat a green unit suite as proof that a CLI process can exit, or that a pipe Drop is safe.
3. Do not close an operator hang by adding a wall-clock timeout that prints success.
4. Do not edit frozen operator feedback (`doc/dev/ux-*.md`) or `doc/ssot/pillars/*`.
5. Do not create `docs/dev/`. Implementer notes stay under `doc/dev/`.

---

## 1. Problem

Operators have seen Hub commands that do not return, or that print success while the parent process still waits. The same family of failures has appeared on **register**, **cancel**, **mid-turn daemon EOF**, and **post-kill pipe/lock reuse**.

What is already on record (not new archaeology):

| Symptom | Recorded mechanism | Source |
|---------|--------------------|--------|
| Cold `agent add` never returns; `agents.json` may already contain the id | `mutate_registry` waited on generation write / init while assembling or replacing handles | `doc/dev/work-report-rc6-p0-root-cause-2026-07-26.md` §1 |
| Long-run `cancel` stuck for many minutes | Cancel RPC `await`ed `send_notification` on the shared ACP connection while the agent was generating | same file §2 |
| After hub-side register returned, QA `WaitForExit` still hung | Windows named-pipe `Drop` / Job tree; CLI had already printed `registered` | `doc/dev/work-report-error-hiding-audit-2026-07-26.md` §3 |
| Mid-`send` `daemon closed the connection` | Client RPC reader hit EOF on the daemon pipe | `doc/dev/cursor-adapter/e2e-investigation-2026-07-24.md` |
| After force-kill: `Access denied` and cascading hangs | Named pipe / DB / lock left in a bad Windows state | same investigation (P0 process-tree kill) |

Topology (HEAD `7dcffe0`; full map and layers in §3.A):

```text
CLI commands.rs → HubClient (client.rs) → ensure_daemon (daemon.rs)
  → RpcClient (rpc.rs)
      -- \\.\pipe\acp-hub-{id} / Unix domain socket -->
  daemon rpc_io.rs → dispatch → registry.rs / prompt.rs → agent stdio
```

Cold `agent add` is register-only (no `agent_handle` spawn). Warm inspect/send spawn the agent.

The product problem is not one missing timeout. It is that **RPC return**, **client process exit**, **pipe/handle Drop**, **daemon idle/shutdown**, and **agent I/O** do not have one written ownership rule. Each hang was patched on the path that was in front of the operator that day.

---

## 2. Why previous fixes are insufficient

Prior tickets did isolate real bugs. They are insufficient as a **default practice** because they do not say who may wait on whom, or when a process is allowed to keep a pipe open.

| Change | What it fixed | Why it is not a lifecycle law |
|--------|---------------|-------------------------------|
| rc.5/rc.7 CLI 15s register timeout + local `agents.json` write | Operator sometimes got *a* return | Classified as error hiding: timeout + disk presence printed `registered` / exit 0 while daemon memory could diverge (`AUDIT-error-hiding-self-review-2026-07-26.md` §2.1) |
| rc.8: bounded `mutate_registry`; Conflict when in-flight; no unbounded generation wait | Register RPC no longer joins a live generation lock without a bound | Necessary for hub mutate. Does not define client Drop, Job tree, or “RPC Ok ⇒ process can exit” |
| rc.8: cancel mark-first; `send_notification` not joined on the RPC | Cancel RPC no longer bound to LLM wall clock | Delivery of `session/cancel` left fire-and-forget; operator contract is `requested` + later `acp_notify_enqueued`, not a pipe/lifecycle model |
| rc.9: remove CLI success-on-timeout, send retry, cancel CLI timeout, silent reconnect | Failures became visible | Honesty of *errors* ≠ ownership of *handles* |
| rc.9: `RpcClient` Drop non-blocking; `agent add` `mem::forget(client)`; Windows breakaway / `DETACHED` / `start /B`; 30s honest RPC timeout | Cold-add `WaitForExit` self-test passed in that report | `mem::forget` avoids a Drop wait; it does not specify who closes the pipe or when leak/orphan is acceptable |
| Historical F-005 (same-connection cancel vs long RPC) | Daemon concurrent RPC + serialized writes (review book) | Scoped to daemon request concurrency, not CLI Drop / Job / spawn |

Recurring pattern in those reports: **a wall-clock on the CLI cannot replace “this RPC path must not join agent I/O”**, and **a hub-side bound cannot replace “the client may drop the pipe without blocking the parent Wait.”** Those two sentences were discovered separately. A default practice has to state both, plus daemon spawn/idle/handshake, as one rule set.

§3.A now lists current joins, forgets, and timeouts with `code:path`. §3.C (keep / replace / delete) is filled — `doc/dev/critique-hang-fixes-rc7-rc9-2026-08-16.md`. That memo is a verdict on the rc.7–rc.9 patches, not a §5 default practice.

---

## 3. Research workstreams

§3.A is filled from HEAD `7dcffe0` (`0.2.1-rc.9`). §3.B **B2** (Windows Job/spawn) is filled — `doc/dev/research-windows-daemon-spawn-2026-08-16.md`. §3.B **B1 / B3 / B4** (named-pipe / Tokio), **B5** (RPC lifecycle), and **B6** (comparable projects) are filled (sibling research notes). **§3.C is filled** (sibling critique memo); **C5** is the spawn-stack row and points at the B2 note. §3.D stays **TODO**. Do not treat §3.A, B1–B6, or §3.C as a written decision.

### 3.A Code archaeology

**Status:** done (codebase evidence only). **Goal met:** wait graph and current mechanisms at `7dcffe0`. Industry teardown rules are **not** claimed here.

#### 3.A.1 Architecture map

```text
CLI  crates/cli/src/commands.rs
  →  HubClient           crates/hub/src/hub/client.rs
    →  ensure_daemon     crates/hub/src/daemon.rs
      →  RpcClient       crates/hub/src/rpc.rs
           transport: Windows named pipe  \\.\pipe\acp-hub-{id}
                      (daemon.rs:530) or Unix domain socket
           →  daemon rpc_io     crates/hub/src/daemon/rpc_io.rs
             →  dispatch
               →  registry.rs / prompt.rs
                 →  agent stdio (warm paths only)
```

| Layer | Role at HEAD |
|-------|----------------|
| CLI command | One-shot process; `process::exit` after `run()` (`main.rs:24-43`) |
| `HubClient` | Typed RPC; ordinary calls use 30s bound; `send` `wait=true` is unbounded (`client.rs:142-152`) |
| `ensure_daemon` | Singleton spawn + discover; Windows Job breakaway cascade (`daemon.rs:754-832`) |
| `RpcClient` | Interprocess JSON-RPC; Drop **aborts** reader/writer and does **not** join (`rpc.rs:374-392`) |
| `rpc_io` / dispatch | Daemon-side request I/O and method routing |
| `registry` / `prompt` | Control RPCs; cold add does **not** spawn `agent_handle`; inspect/send do |

#### 3.A.2 Timeline rc.5 → rc.9 (hang-related code)

| Era | Hang-related change | At `7dcffe0` |
|-----|---------------------|--------------|
| rc.5 | CLI 15s register timeout-as-success | **Removed** |
| rc.7 | Timeout-as-success; local `agents.json` write; cancel CLI 12s timeout; `connect_with_retry`; send retry | **Removed** (keep that absence) |
| rc.8 | `mutate_registry` bounded waits; cancel mark-first + fire-and-forget notify | **Still present** (root fixes) |
| rc.9 | `RpcClient` Drop abort; `mem::forget` on add; `CREATE_BREAKAWAY_FROM_JOB` cascade; 30s honest RPC timeout; honest `CancelResult` | **Still present** |

#### 3.A.3 Current mechanisms (keep / still-workaround)

| Mechanism | Location | Class |
|-----------|----------|-------|
| `mem::forget(client)` after successful add only | `crates/cli/src/commands.rs:61-72` | **still-workaround** — leaks until `process::exit`; remove/list Drop normally |
| `RpcClient` Drop: mark closed, abort reader/writer, no join | `crates/hub/src/rpc.rs:374-392` | **still-workaround** — avoids Windows pipe-teardown hang; relies on abort + later OS reclaim |
| Windows spawn: breakaway → DETACHED → `cmd start /B` | `crates/hub/src/daemon.rs:754-832` | **still-workaround** — `start /B` may still share the parent Job |
| Cold add skips generation wait; init wait 10s; live gen wait 15s | `crates/hub/src/hub/registry.rs:565-598` | **keep** (rc.8 root fix) |
| Cancel: mark then `spawn_blocking` notify; notify fail is warn-only | `crates/hub/src/hub/prompt.rs:295-331` | **keep** mark-first; **still-workaround** on delivery. Contradicts `doc/dev/design.md:172-175` (send failure rolls state back) |
| Honest `CancelResult` (`requested`, `acp_notify_enqueued`) | `crates/hub/src/hub/types.rs:92-108` | **keep** |
| `DEFAULT_RPC_REQUEST_TIMEOUT` 30s on ordinary RPC | `crates/hub/src/rpc.rs:52` (applied via `rpc.rs:291`) | **keep** as honest bound; see register commit-then-timeout risk |
| `send` `wait=true` unbounded (`timeout=None`) | `crates/hub/src/hub/client.rs:142-152` | **keep** (agent wall clock; not the ordinary 30s bound) |
| `process::exit` so aborted RPC tasks cannot hold the runtime | `crates/cli/src/main.rs:24-43` | **still-workaround** (pairs with Drop abort / `mem::forget`) |
| No `connect_with_retry`; no send retry | CLI / client (rc.9 deletion) | **keep** |

#### 3.A.4 Remaining risks (`file:line`)

| Risk | Where |
|------|--------|
| Cancel notify fails → run stays **cancelling**; warn only | `prompt.rs:323-328` |
| Cancel with no live handle: mark applied, agent may never see `session/cancel` | `prompt.rs:332-337` |
| Register can commit disk, then client hits 30s RPC timeout → operator sees Err while daemon memory/disk may already have the id | `rpc.rs:52` + `rpc.rs:291` + register path; no CLI disk fallback (good) but no typed partial-success yet |
| Windows Job fallback `start /B` may still share job → parent Wait can outlive CLI print | `daemon.rs:807-831` |
| `mem::forget` is add-only (asymmetric vs remove/other commands) | `commands.rs:71` vs `commands.rs:74-78` |
| Abrupt disconnect / daemon drop: no silent reconnect (honest) and **no** reconnect | client/CLI after rc.9; leftover pipe/lock still an operator hazard |
| Docs drift: UX-CORE still says **agent add ≤15s** hard timeout; code ordinary RPC bound is **30s** | `doc/ssot/agent-managed/UX-CORE.md` (implementation-status line) vs `rpc.rs:52` — do **not** rewrite frozen `ux-*.md`; synthesis must say which doc is updated |

#### 3.A.5 Per-question findings

| ID | Question | Finding |
|----|----------|---------|
| A1 | `RpcClient` / `HubClient` Drop | Drop aborts reader/writer and does not join (`rpc.rs:374-392`). CLI then `process::exit` (`main.rs:24-43`). Add path additionally `mem::forget`s the client (`commands.rs:71`) so Drop may not run before exit. |
| A2 | Windows named pipe | Client/daemon use `\\.\pipe\acp-hub-{id}` (`daemon.rs:530`). This pass did not re-walk `ConnectNamedPipe` / last-handle-close API (that is §3.B). In-tree risk: Drop-join hang motivated abort (`rpc.rs:378-383`); leftover pipe after kill remains a recorded operator hazard. |
| A3 | Unix socket | Same `RpcClient` over UDS. Unlink/accept/EOF not re-walked beyond “UDS is the Unix transport.” Do not invent POSIX close rules here. |
| A4 | `ensure_daemon` / spawn / lock / handshake | Spawn + Job cascade in `daemon.rs:754-832`. Handshake-before-business-RPC still the connect contract (prior map + `HubClient`); this pass did not re-list handshake version constants. |
| A5 | Idle exit vs activity counters | **Not re-measured** in this archaeology pass. Related HEAD risk: daemon drop, no reconnect. Last idle-exit note remains the 2026-07-24 map (1800s / `ActivityTracker`); re-verify if synthesis needs idle-vs-disconnect. |
| A6 | `mutate_registry` | Cold add skips generation wait; live handles: init 10s, gen 15s (`registry.rs:565-598`). |
| A7 | Cancel | Mark first; live-handle-only notify via `spawn_blocking`; no `agent_handle` cold start (`prompt.rs:295-331`). Honest fields in `types.rs:92-108`. Design.md still documents rollback-on-send-failure (`design.md:172-175`). |
| A8 | Windows Job / spawn | Breakaway cascade `daemon.rs:754-832`; comment states Job-aware parents wait the tree; `start /B` “may still share job.” |
| A9 | Mid-RPC daemon EOF | No `connect_with_retry`, no send retry (**keep**). Abrupt disconnect surfaces as error; can still leave the operator mid-command with no reconnect. `send wait=true` has no 30s cap (`client.rs:142-152`). |
| A10 | MCP / lib vs CLI | Same `HubClient`/`RpcClient` Drop abort. `mem::forget` is **CLI add only**. `process::exit` is **CLI `main`**. Lib/MCP do not share that exit/forget pair; synthesis must say whether they need the same teardown. |

#### 3.A.6 Codebase-derived “correct lifecycle” (not industry default)

The following is **section 7 of the archaeology report**: what *this tree* already implies after rc.8/rc.9. It is **not** an industry conclusion, **not** §5, and **not** a written decision.

- Ordinary RPC: 30s honest `Err` (not success-on-timeout).
- No `mem::forget` as the steady-state rule; prefer non-blocking teardown + optional close handshake.
- Daemon must escape the parent Job when the OS permits; fallback (`start /B`) must be observable.
- Cold add: no agent I/O; never CLI disk fallback; if disk/daemon commit succeeds and the client then fails to see the response, that is a **typed partial-success**, not silent `registered` and not a local `agents.json` write.
- Cancel: mark then return honest fields; close the `design.md:172-175` rollback vs fire-and-forget/watchdog gap in a later decision.
- Mid-command: no silent reconnect.

#### 3.A.7 Must-touch files (when implementation is later allowed)

| Path | Why |
|------|-----|
| `crates/hub/src/rpc.rs` | Drop abort, 30s bound, pipe/UDS client |
| `crates/cli/src/commands.rs` | `mem::forget` add-only; register/cancel CLI |
| `crates/cli/src/main.rs` | `process::exit` vs runtime keep-alive |
| `crates/hub/src/daemon.rs` | spawn/Job cascade, pipe name |
| `crates/hub/src/daemon/rpc_io.rs` | daemon-side RPC I/O |
| `crates/hub/src/hub/registry.rs` | mutate waits, cold-add skip |
| `crates/hub/src/hub/prompt.rs` | cancel mark + notify |
| `crates/hub/src/hub/types.rs` | `CancelResult` contract |
| `crates/hub/src/callbacks/connection.rs` | agent connection / handle lifetime |
| `crates/hub/src/hub/client.rs` | handshake, 30s vs unbounded send |
| `crates/hub/src/acp.rs` | ACP connection / cancel notify surface |
| `crates/hub/src/bounded_transport/flow.rs` | agent-side flow / backpressure |

Do not start edits from this list until §0 step 5.

### 3.B Industry research

**Goal:** Import constraints that the OS and common local-daemon designs already impose. Cite specs or reference implementations.

| ID | Topic | Finding (fill) |
|----|-------|----------------|
| B1 | Windows named-pipe: `ConnectNamedPipe` / `DisconnectNamedPipe` / `FlushFileBuffers` / last handle; `ERROR_PIPE_BUSY` / access-denied after kill | **Done** — `doc/dev/research-named-pipe-tokio-2026-08-16.md` §1–§2. Connect is `CreateFile(OPEN_EXISTING)` + `ERROR_PIPE_BUSY` / `WaitNamedPipe` retry. Server: keep one idle instance, `connect().await`; after the client goes away, `DisconnectNamedPipe` before reuse. `FlushFileBuffers` on the **server** end **blocks until the client reads** (SO_LINGER analog) — do not use on CLI teardown. Last-handle close is `CloseHandle`. Tokio/mio Drop requests `CancelIoEx` and **does not wait** (waiting in Drop is the hang). Leftover pipe / access-denied after kill remains an operator hazard (archaeology A2); this row is the OS/API constraint, not leftover-state recovery. |
| B2 | Windows Job objects vs `CREATE_BREAKAWAY_FROM_JOB` / `DETACHED_PROCESS` — when a child death or Wait hangs the parent | **Done** — `doc/dev/research-windows-daemon-spawn-2026-08-16.md` (this row’s home; named-pipe note §6 is **not** the Job/spawn default). Default is **(c)+(d)**: handle hygiene + `CREATE_NO_WINDOW` **xor** `DETACHED_PROCESS` + `CREATE_NEW_PROCESS_GROUP` + existing `daemon.json` ready handshake, then the CLI actually exits. **Not** breakaway-from-job. **Not** a Windows Service for a per-user hub. `WaitForSingleObject(hProcess)` does not hang after true process exit; leftover Wait is inherited pipe write-end, console attachment, or CLI thread still in `RpcClient` Drop. Rust `Command` always `bInheritHandles=TRUE`; `Stdio::null()` still inherits every inheritable handle (textbook CI hang). `CREATE_NO_WINDOW` + `DETACHED_PROCESS` ⇒ `CREATE_NO_WINDOW` ignored (MSDN); in-tree ORs them on every path. `CREATE_BREAKAWAY_FROM_JOB` fails without `BREAKAWAY_OK`; `KILL_ON_JOB_CLOSE` jobs typically omit it; success means escaping a supervisor that wanted the tree dead. Delete `start /B`. `ACP_HUB_DAEMON_STDERR=inherit` reopens the hole. Recommended flags in the note §4. Archaeology A8 / §3.A.6 left as-is (incident law, not this default). |
| B3 | Unix domain socket: implicit EOF on last close; unlink vs accept | **Done (teardown hang; not a full unlink walk)** — same note §2.A. Mechanism A is identical on UDS: leftover reader parked on a live daemon socket → `Runtime::drop` waits forever. Implicit EOF arrives only when the **peer** closes the last handle; a still-running daemon will not EOF just because the last RPC finished. In-tree unlink/accept not re-walked beyond archaeology A3. |
| B4 | Async Rust: dropping a future that owns a pipe/socket; `AsyncDrop` absence; `forget` vs explicit shutdown | **Done** — same note §0–§4. Industry default is structured `shutdown()` + abort-only Drop, **not** `mem::forget`. jsonrpsee Drop sends oneshot, never joins; rust-analyzer does protocol shutdown then `IoThreads::join`; tarpc `Cancel`; Tokio `TcpStream::shutdown` is async; `Runtime::drop` waits **forever** for non-yielding tasks. Current `RpcClient` Drop abort-without-join is already mature. `mem::forget` is **redundant**: CLI already `process::exit` (skips runtime Drop); forget skips abort; if someone removes `process::exit`, the reader lives forever. Recommend: `async shutdown(self)` + abort-only Drop; keep CLI `process::exit`; **delete** `forget`. Ranked alternatives in the note §3. |
| B5 | JSON-RPC 2.0 over a single byte stream: request/response correlation, cancel of an in-flight call, unclean EOF | **Done** — `doc/dev/research-rpc-daemon-lifecycle-2026-08-16.md` (RPC-lifecycle workstream). Four classes: **unary mutate** (at-most-once, 30s, daemon result only); **unary cancel** (ack = hub mark; ACP `session/cancel` notify fire-and-forget; keep `CancelResult {requested, acp_notify_enqueued}`; `spawn_blocking` is a smell, don’t-join is the rule); **wait** (product timeout, reconnect OK); **streaming send** (accept bounded, join product/unbounded, **never replay after drop**). Reconnect: gRPC A6 / client-go — retry **unread** only; never silent mutate retry. Shutdown: abort IO + optional ≤250ms join + CLI `process::exit`; **FORBIDDEN** `mem::forget` as success path; **FORBIDDEN** unbounded `FlushFileBuffers` on the CLI thread. Copy: request vs notification, create-vs-start, 25–60s ordinary timeout. Unique: two stacked protocols; disk registry + live handles; user-spawned singleton that must outlive CLI. **Tension (resolved in §5 / decision §3.B):** RPC memo’s “breakaway unique-and-necessary” is rejected. Primary spawn fix is no-inherit `CreateProcessW`; breakaway is opt-in. |
| B6 | Peer CLIs / daemons (e.g. language servers, other ACP clients) — connect, idle, shutdown handshake | **Done** — `doc/dev/research-comparable-ipc-daemons-2026-08-16.md`. Five patterns: **A** stdio child (ACP SDK / MCP / Copilot LS / rust-analyzer) does **not** fit a Hub that outlives one CLI — do not copy official ACP SDK as Hub architecture. **B** HTTP-over-npipe + Windows Service (Docker/Podman) is a product change. **C** auto-spawn + TCP loopback (sccache, ra-multiplex) is the dominant Rust CLI→daemon pattern that does not hang; sccache is the closest analog. **D** named pipe / AF_UNIX with explicit half-close / `evade_limbo` (interprocess, wezterm, nushell). **E** `mem::forget` + `process::exit` is a workaround scar. Cancel store-first + fire-and-forget **matches** ACP / LSP `$/cancelRequest` / MCP `notifications/cancelled` — **keep**. Recommended order and negatives are in the note §5; they are **not** a §5 verdict. B1–B4 are closed by the named-pipe / Tokio note; **B5 is not** closed by this row. |

Windows Job/spawn write-up (B2 / C5): `doc/dev/research-windows-daemon-spawn-2026-08-16.md`. Named-pipe / Tokio write-up (B1 / B3 / B4; do not use that note §6 as the Job/spawn default): `doc/dev/research-named-pipe-tokio-2026-08-16.md`. RPC-lifecycle workstream (B5; no separate §3 heading): `doc/dev/research-rpc-daemon-lifecycle-2026-08-16.md`. Comparable-projects write-up (B6 only): `doc/dev/research-comparable-ipc-daemons-2026-08-16.md`. Critique write-up (§3.C): `doc/dev/critique-hang-fixes-rc7-rc9-2026-08-16.md`. None of these notes is a §5 verdict.

### 3.C Critique of current patches

**Status:** done (verdict on rc.7–rc.9 patches at `7dcffe0`). **Not** a §5 default practice. Full autopsy + keep/replace/delete table: `doc/dev/critique-hang-fixes-rc7-rc9-2026-08-16.md`.

| ID | Patch or path | Still joins / hides / leaks? |
|----|---------------|------------------------------|
| C1 | `mem::forget(client)` on `agent add` | **DELETE.** Add-only leak until `process::exit` (`commands.rs:71`). Asymmetric vs remove/list. MCP/lib unprotected. |
| C2 | Non-blocking `RpcClient` Drop (all commands?) | **REPLACE.** Abort-no-join (`rpc.rs:374-392`) is a workaround. Explicit close: mark closed, shutdown write, drain reader with timeout, then drop. `process::exit` only after that, last resort. |
| C3 | Cancel notify fire-and-forget | **KEEP** mark-first, `CancelResult` honesty, no `agent_handle` cold-start. **DELETE** unjoined `spawn_blocking` as the notify mechanism. **REPLACE** fail-open with dedicated writer or kill-child + bound wait for `Cancelled` + force-finalize. `design.md` §3.3 still says rollback-on-notify-fail — undocumented inversion. |
| C4 | Honest 30s RPC timeout vs unbounded wait / attach (`wait`) | **KEEP** 30s ordinary RPC as `Err`; **KEEP** send/wait unbounded except product `--timeout`. **REPLACE** committed-register + 30s `DaemonUnavailable` with typed partial-success. Do not turn the bound back into success. |
| C5 | Daemon spawn DETACHED + CLI Job membership | **Done** — `doc/dev/research-windows-daemon-spawn-2026-08-16.md` §6 (spawn-stack critique; this row’s home). **REPLACE** the rc.9 cascade (`CREATE_BREAKAWAY_FROM_JOB` → `DETACHED` → `start /B`) with **(c)+(d)** in the B2 note. **DELETE** `start /B`. Treat breakaway as **explicit opt-in**, not default. `CREATE_NO_WINDOW \| DETACHED_PROCESS` is a no-op for `CREATE_NO_WINDOW` (MSDN). rust std inherit + `Stdio::null()` is the textbook hang; `ACP_HUB_DAEMON_STDERR=inherit` reopens it. Critique memo still has a job-query / “run `serve` out of band” sketch — synthesizer should prefer the B2 note over that sketch and over named-pipe note §6 (“BREAKAWAY is the right family”). |
| C6 | Handshake + idle exit vs “daemon closed” mid-turn | **KEEP** no silent reconnect / no send retry. **ADD** P0-3 daemon self-heal / projection rebuild. Exposing `daemon_unavailable` is not that fix. |

Remaining unbounded control-RPC `.await`s and success-on-uncertainty paths: critique memo §5. Isolation tests (Drop vs job vs `process::exit` A/B) are **ADD**. Do not overwrite §3.A / §3.B from this row.

### 3.D Synthesis (after A–C)

| ID | Deliverable | Status |
|----|-------------|--------|
| D1 | Single wait-graph diagram (CLI thread, RPC task, pipe, daemon, agent connection) | **Done** — see below |
| D2 | Default practice text in §5 | **Done** — §5; normative memo |
| D3 | Phase list in §6 with acceptance per phase | **Done** — §6 |
| D4 | Written decision (date, verdict, link to spec/design if required) | **Done** — 2026-08-16 **accept**; `doc/dev/decision-ipc-lifecycle-default-practice-2026-08-16.md` |

Wait graph (who may wait on whom). Solid arrows are allowed joins; dashed arrows are the hangs this decision forbids.

```text
parent WaitForExit ──► CLI PID (process handle only; not Job, not stdout EOF)
                         │
                         ├── run() ──► HubClient RPC future ──► RpcClient reader/writer
                         │                 │                         │
                         │                 │  30s MUTATE/CANCEL      │ abort on Drop
                         │                 │  product WAIT/SEND      │ shutdown() ≤250ms join
                         │                 ▼                         ▼
                         │            daemon rpc_io ──► dispatch ──► registry / prompt
                         │                 ▲                         │
                         │                 │ pipe/UDS                ├── MUTATE: disk then reply
                         │                 │                         ├── CANCEL: mark, then async
                         │                 │                         │   notify (not on this stack)
                         │                 │                         └── SEND join: agent stdio
                         │                 │
                         └── process::exit after shutdown()
                                      │
                                      X  mem::forget          (deleted)
                                      X  inherit parent stdout (P1)
                                      X  FlushFileBuffers     (forbidden)
                                      X  join agent I/O on cancel/register

daemon serve ── idle timer ──► exit (not CLI Drop)
agent child  ── daemon handles only (Pattern A southbound)
```

---

## 4. Decision criteria

A proposed default practice is acceptable only if it satisfies all of the following. “Looks simpler” is not a criterion.

1. **RPC return ≠ process exit.** The document states, for each entry point (CLI, MCP, lib), when the RPC future completes and when the OS process is allowed to exit.
2. **No RPC path joins agent generation I/O.** Register, cancel, list, handshake, and other control RPCs do not `.await` ACP prompt/generation write on the caller’s stack. If a bound exists, expiry is a typed error, not success.
3. **Drop is specified.** Closing the client is either (a) a bounded, non-blocking teardown or (b) an explicit shutdown RPC then close. `mem::forget` is not the steady-state rule unless synthesis argues a leak is required and names the reclaim path.
4. **Failure is visible.** Uncertain daemon/pipe state is `Err` (or a typed intermediate such as cancel `requested` + `acp_notify_enqueued`). Disk presence of `agents.json` / `daemon.json` is not success.
5. **Windows and Unix share one contract**; only the transport primitive differs. Job/breakaway and `unlink` are implementation notes under that contract.
6. **Singleton daemon remains one process per Hub home.** Redesign may change spawn/idle/handshake, not silently become multi-daemon.
7. **Operator `ux-*.md` stays frozen.** Behavior changes are recorded in implementer docs + CHANGELOG, not by rewriting feedback books.
8. **Testable.** Each rule in §5 maps to a test that fails if the wait or the false-success returns.

---

## 5. Proposed default practice (**accepted** 2026-08-16)

**Status:** decided. Normative text: `doc/dev/decision-ipc-lifecycle-default-practice-2026-08-16.md` §4. This section is the in-plan copy. rc.8/rc.9 are incident repairs; they are no longer the undocumented law.

- **Owners:** Daemon owns the listener, `daemon.lock` (for `serve` lifetime), `daemon.json`, and warm agent handles. `RpcClient` owns the client stream halves and reader/writer tasks. One-shot CLI/MCP owns a close (`shutdown()` then Drop). CLI `process::exit` is last resort after that close, not GC. Lib/MCP do not `process::exit`.
- **Connect:** Discover `daemon.json` → spawn if missing (P1 law) → connect pipe/UDS → handshake → business RPC. No `connect_with_retry`. `ERROR_PIPE_BUSY` / `NotFound` may retry only while a just-spawned daemon is coming up; expiry is `Err`. Ready is metadata + connect, not `spawn` Ok.
- **Call:** Four classes (RPC memo). **MUTATE** — 30s, at-most-once, daemon `Result` only, never silent retry; commit-then-timeout is typed partial-success. **CANCEL** — 30s on the mark; ack = `requested` + `acp_notify_enqueued`; southbound notify not joined. **WAIT** — product timeout; Store-poll reconnect OK. **SEND** — accept bounded; join product/unbounded; never replay after drop.
- **Return:** Three events: bytes on the wire; Rust `Result`; CLI `process::exit`. Parent `WaitForExit` waits the CLI **process handle** only (not Job, not stdout EOF). MCP/lib: first two events + `shutdown()`.
- **Drop / shutdown:** After last reply: shutdown write if real; `assume_flushed` / `evade_limbo` on Windows send half if reachable; mark closed; abort IO; optional ≤250ms join **inside** `shutdown()` only; Drop abort-only (no join, no flush, no `forget`). Daemon idle-exit stays. No shutdown RPC and no handshake bump in P0–P3.
- **Cancel:** Mark-first. Dedicated async notify (`tokio::spawn` / queue), not `spawn_blocking`, not joined on the RPC. After a budget: force-finalize or typed still-running. Align `design.md` §3.3 in P2. Do not restore rollback-that-blocks-RPC.
- **Registry mutate:** Keep skip-generation on new ids, bounded live waits, Conflict, commit-then-revoke. No CLI disk fallback. P3: typed partial-success; replace `refresh_registry` warn-and-continue with fencing or `InvalidRegistry`.
- **Windows-only notes:** Primary fix is `CreateProcessW` `bInheritHandles=FALSE` (or HANDLE_LIST of NULs). Flags: `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT`. Do not OR `DETACHED_PROCESS` with `CREATE_NO_WINDOW`. Do not inherit `daemon.lock` or the pipe server. Breakaway **off by default** (opt-in; denial is typed, no `DETACHED`/`start /B` fallthrough). In-job no-inherit spawn is allowed. Delete `start /B`. Leftover pipe after kill is visible (`doctor`), not an unbounded hang.
- **Unix-only notes:** Same Drop / `shutdown()` / call-class contract. UDS unlink is the daemon listen-path job. `setsid` stays.
- **Forbidden:** timeout-as-success; local `agents.json` as register success; silent send retry; `connect_with_retry`; `mem::forget`; joining agent I/O on a control RPC; claiming Job detach when still in-job; `FlushFileBuffers` on CLI teardown; rewriting `ux-*.md`.

---

## 6. Implementation phases (**accepted**)

Do not start code in the turn that wrote the decision. Each phase: files, tests, CHANGELOG if operator-visible, **no** `ux-*.md` rewrite. Full acceptance text: decision memo §5.

| Phase | Status | Intent | Depends on | Acceptance |
|-------|--------|--------|------------|------------|
| **P0** | **Done** 2026-08-16 (reviewed; no findings) | Client close: delete `mem::forget`; `RpcClient::shutdown()`; evade limbo if reachable; keep `process::exit` after close | This decision | `forget` gone. `shutdown()` on one-shot CLI/MCP. Drop abort-only. Windows `assume_flushed`/`evade_limbo` when the held type exposes it. CLI exits after `shutdown()`. |
| **P1** | **Done** 2026-08-16 (reviewed; no findings) | Windows spawn = no-inherit `CreateProcessW` + correct flags; delete `start /B`; honest opt-in breakaway | P0 | `bInheritHandles=FALSE` (stable `Command` cannot). `CREATE_NO_WINDOW` xor `DETACHED`. No `start /B`. Breakaway off by default. Child does not inherit `daemon.lock` or parent stdout. Wait-for-EOF fixture. |
| **P2** | **Done** 2026-08-16 (reviewed; no findings) | Cancel notify path + force-finalize + `design.md` align | P0 | `spawn_blocking` gone. Async notify not joined on RPC. Force-finalize or typed surface after budget. `design.md` §3.3 matches. |
| **P3** | **Done** 2026-08-16 (reviewed; no findings) | Typed register partial-success; `refresh_registry` fencing; isolation tests (Drop vs inherit vs job) | P0–P2 | Commit + lost reply ≠ `DaemonUnavailable`. Refresh fenced or `InvalidRegistry`. Isolation: H3 Drop, H1 inherit, H4 job claim. |
| **P4** (optional) | **Not started** | TCP `127.0.0.1` if limbo remains after evade + no-inherit | P0–P1 + residual limbo | Not started for taste. Named pipe may remain oneshot ready. |

Reviews (2026-08-16; no agent ids):

- **P0 + P1** — reviewed together; no findings. Residual isolation (H1 inherit / H3 Drop / H4 job claim) stays **P3**. On-disk note: `CHANGELOG.md` `[Unreleased]` Fixed/Changed bullets.
- **P2** — reviewed; no findings. Leftover (no kill-child / no early admission release) is recorded in the decision memo **Leftover (P2 review 2026-08-16)**. `design.md` §3.3 + concurrency note aligned.
- **P3** — reviewed; no findings. Isolation + typed `CommittedReplyLost` (`-32018`) + fail-closed `refresh_registry` landed. Leftovers (inspect/send unfenced memory reads; no already-exists product error; no kill-child; P4 not started) are recorded in the decision memo **Leftover (P3 review 2026-08-16)**.

Files by phase: P0 `rpc.rs`, `commands.rs`, `main.rs`, MCP client owners; P1 `daemon.rs`; P2 `prompt.rs`, `types.rs`, `design.md`, cancel tests; P3 `error.rs`, `error_data.rs`, `client.rs`, `registry.rs`, isolation tests.

---

## 7. Verification

§5 exists. Verification must include all four layers that prior hang tickets mixed together:

| Layer | What it proves | Not sufficient alone |
|-------|----------------|----------------------|
| Unit / crate tests | Lock bounds, cancel mark, handshake mismatch, typed errors | Process exit, pipe Drop, Job tree |
| CLI process exit | Parent `WaitForExit` / equivalent after Ok and after Err | Daemon leftover state |
| Daemon leftover | Second connect after kill/idle; `daemon.lock` / pipe / socket reuse | Operator honesty |
| Honesty | Failure and uncertainty are not printed as success | Hang-freedom |

Concrete cases to attach to §5 rules (fill pass/fail later):

- [ ] Cold `agent add`: RPC returns; process exits; `agents.json` matches daemon memory; **no** `mem::forget` (§5 forbids it)
- [ ] `cancel` during generation: RPC returns without joining ACP write; fields remain honest
- [ ] Mid-turn daemon EOF: client error, no silent retry, no hang
- [ ] Force-kill then next command: no unbounded hang; access-denied is visible if reuse fails
- [ ] `wait --timeout`: timeout is an error, not a successful empty wait
- [ ] MCP and `HubClient` follow the same Drop/return rules as CLI, or §5 documents a deliberate difference

---

## 8. Open questions (**answered** 2026-08-16)

Full answers: decision memo §6.

1. One-shot CLI/MCP: every command owns a close. In-process `HubClient` may reuse the pipe; that process still owns the eventual `shutdown()`.
2. Combination: existing idle timer + last-handle-close of **that client’s** pipe. No new shutdown RPC in P0–P3. CLI Drop is not the daemon supervisor.
3. Long-lived MCP/`HubClient`: yes, keep the pipe across calls. One-shot CLI: close after the command.
4. Enqueue (dedicated async task) + honest `acp_notify_enqueued`. Observed agent stop is a later supervisor/finalize signal, not the cancel RPC return.
5. Compensating for H1+H2+H3, not required as default. Primary fix is no-inherit. Breakaway opt-in. `DETACHED|NO_WINDOW` combo deleted.
6. No handshake bump for P0–P3 client-local close. Bump only if a shutdown notification is added later.
7. Visible error; recover via `doctor` / home cleanup. Not an unbounded hang and not silent reuse.
8. This memo is the dated addendum. **P2 done:** `design.md` §3.3 + concurrency note aligned. Spec / BDD / TDD addenda for P1+ remain open.

---

## 9. Checklist for later synthesis

Copy/paste status: `open` → `evidence` → `decided`. Every `decided` row needs a pointer (section, path, or sha).

### Archaeology

- [x] A1 RpcClient / HubClient Drop and reader loop — §3.A.5; `rpc.rs:374-392`
- [x] A2 Windows named-pipe lifecycle in-tree — §3.A.1 / A2; `daemon.rs:530` (OS API → §3.B)
- [x] A3 Unix socket lifecycle in-tree — §3.A.5 A3 (UDS layer only; unlink/EOF → §3.B)
- [x] A4 ensure_daemon / lock / metadata / handshake — §3.A.5 A4; spawn `daemon.rs:754-832`
- [x] A5 idle exit vs activity counters — §3.A.5 A5 (**not re-measured**; daemon-drop risk noted)
- [x] A6 registry mutate waits — §3.A.3 / A6; `registry.rs:565-598`
- [x] A7 cancel waits — §3.A.3 / A7; `prompt.rs:295-331`, `types.rs:92-108`
- [x] A8 Windows Job / spawn flags — §3.A.3 / A8; `daemon.rs:754-832`
- [x] A9 daemon EOF mid-RPC — §3.A.4 / A9 (no silent reconnect)
- [x] A10 MCP / lib vs CLI — §3.A.5 A10 (`mem::forget` + `process::exit` are CLI-only)

### Industry

- [x] B1 Windows pipe API constraints — `doc/dev/research-named-pipe-tokio-2026-08-16.md` §1–§2; plan §3.B B1
- [x] B2 Job / breakaway / detached — `doc/dev/research-windows-daemon-spawn-2026-08-16.md` (default (c)+(d); not breakaway; not Service; delete `start /B`)
- [x] B3 Unix socket close / unlink — same note §2.A (UDS shares mechanism A; unlink/accept not re-walked)
- [x] B4 async Drop / forget — same note §0–§4 (`shutdown` + abort-only Drop; delete `forget`; keep CLI `process::exit`)
- [x] B5 JSON-RPC stream cancel / EOF — `doc/dev/research-rpc-daemon-lifecycle-2026-08-16.md` (four classes; ack vs notify; unread-only retry; structured shutdown)
- [x] B6 peer local-daemon practices — `doc/dev/research-comparable-ipc-daemons-2026-08-16.md` (patterns A–E; sccache closest analog; cancel keep)

### Critique

- [x] C1–C6 current patches mapped to the wait graph — §3.C; `doc/dev/critique-hang-fixes-rc7-rc9-2026-08-16.md` §6
- [x] C5 spawn cascade (breakaway → DETACHED → `start /B`) — `doc/dev/research-windows-daemon-spawn-2026-08-16.md` §6; plan §3.C C5
- [x] List of remaining unbounded `.await` on control RPC stacks — same memo §5.1
- [x] List of remaining success-on-uncertainty paths — same memo §5.2

### Synthesis and gate

- [x] D1 wait-graph diagram — §3.D
- [x] D2 §5 default practice filled — §5; decision memo §4
- [x] D3 §6 phases filled with acceptance — §6; decision memo §5
- [x] D4 written decision recorded — 2026-08-16 accept; `doc/dev/decision-ipc-lifecycle-default-practice-2026-08-16.md`
- [ ] If non-small: spec / BDD / TDD addenda for P1+ still open. **P2 done:** `design.md` §3.3 aligned (2026-08-16)
- [x] Implementation **not** started before D4 — held in the decision turn. **P0–P3** landed and reviewed 2026-08-16; **P4** not started

### Discipline (unchanged)

- [ ] No edits to `doc/dev/ux-*.md`
- [ ] No `docs/dev/` tree created
- [ ] No source changes attributed to this scaffold
