# Decision: IPC lifecycle default practice

| Field | Value |
|-------|--------|
| **Nature** | Normative implementer decision (not operator SSOT; not a work-report close-out) |
| **Date** | 2026-08-16 |
| **Verdict** | **Accept** the default practice in §4 and the phases in §5. |
| **HEAD at decision** | `7dcffe0` (`0.2.1-rc.9`) |
| **Living plan** | `doc/dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` §5–§6 |
| **Implementation** | **P0–P3 done** (reviewed 2026-08-16). **P4** not started. Decision turn did not implement (see §7). |
| **Code** | Decision turn did not implement from this file. Later phases follow the living plan §6. |
| **Frozen** | `doc/dev/ux-*.md`; `doc/ssot/pillars/*` — unread for rewrite. |

This memo is the written decision required by the living plan §0 step 5. rc.8/rc.9 remain incident repairs; they are no longer the undocumented default. **P0–P3 are implemented and reviewed** (2026-08-16). **P4** optional TCP is not started.

---

## 0. TL;DR

The hang family (cold `agent add` that prints `registered` while a parent `WaitForExit` stays up, cancel bound to a generation write, mid-turn daemon EOF, leftover pipe after kill) was patched one operator path at a time. The patches that remain worth keeping are the honesty deletions and the hub lock-order / mark-first changes. The stacked teardown (`mem::forget`, abort-as-close, `DETACHED` reported as detach, `start /B`, `process::exit` as GC) is not a lifecycle.

The five research notes converge on one ownership rule: **RPC return, client close, and process exit are three events.** Control RPCs do not join agent I/O. Windows parent waits are first a handle-inheritance problem (H1), not a Job-breakaway problem. Named-pipe limbo is skipped after the reply is read; it is not flushed on the CLI thread.

The decision is: delete `mem::forget`; add `RpcClient::shutdown()`; keep CLI `process::exit` only after that close; spawn with `CreateProcessW` `bInheritHandles=FALSE` and honest Job handling; keep mark-first cancel and replace the notify path; stay on the interprocess local socket for now; never restore timeout-as-success.

---

## 1. Evidence set

### 1.1 Files read

| Path | Role |
|------|------|
| `doc/dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` | Archaeology §3.A at `7dcffe0`; criteria §4 |
| `doc/dev/research-named-pipe-tokio-2026-08-16.md` | B1 / B3 / B4: Drop = abort only; `forget` redundant with `process::exit`; Wait-for-EOF vs pipe Drop |
| `doc/dev/research-windows-daemon-spawn-2026-08-16.md` | B2 / C5: H1–H5 taxonomy; default is handle hygiene + `CREATE_NO_WINDOW` xor `DETACHED`; breakaway is not default; `start /B` delete |
| `doc/dev/research-rpc-daemon-lifecycle-2026-08-16.md` | B5: four call classes; unread-only retry; `spawn_blocking` smell; shutdown join ≤250ms; records the breakaway tension for synthesis |
| `doc/dev/research-comparable-ipc-daemons-2026-08-16.md` | B6: patterns A–E; sccache no-inherit; interprocess limbo API; ACP/LSP/MCP cancel notifications |
| `doc/dev/critique-hang-fixes-rc7-rc9-2026-08-16.md` | C1–C6 keep / replace / delete |
| `doc/dev/design.md` §3.3 | Cancel rollback text still opposite HEAD |
| `doc/dev/work-report-error-hiding-audit-2026-07-26.md` | rc.9 stacked WaitForExit self-test |
| `doc/dev/AUDIT-error-hiding-self-review-2026-07-26.md` | Timeout-as-success class |
| In-tree at `7dcffe0` | `rpc.rs` Drop; `commands.rs` `mem::forget`; `daemon.rs` spawn cascade; `prompt.rs` cancel; `main.rs` `process::exit` |
| `interprocess` 2.4.2 (workspace pin) | `assume_flushed` / `evade_limbo` on Windows `PipeStream`; `local_socket` wraps that type; Tokio `poll_shutdown` on the wrapper is a no-op |

### 1.2 Research completeness

All five requested siblings are on disk and were read before this memo was accepted:

- `research-named-pipe-tokio-2026-08-16.md`
- `research-windows-daemon-spawn-2026-08-16.md`
- `research-comparable-ipc-daemons-2026-08-16.md`
- `research-rpc-daemon-lifecycle-2026-08-16.md`
- `critique-hang-fixes-rc7-rc9-2026-08-16.md`

The RPC memo §7 records a tension (breakaway unique-and-necessary vs inherit-is-the-hang) and forbids promoting either claim to §5 from that file. This memo resolves it in §3.B using the spawn note’s H1–H5 taxonomy.

### 1.3 Decision criteria (plan §4) — all required

1. RPC return ≠ process exit — §4.3 / §4.4.
2. No control RPC joins agent generation I/O — already true for register/cancel after rc.8; cancel *delivery* is moved off `spawn_blocking` in P2.
3. Drop is specified — abort-only Drop + explicit `shutdown()`; `mem::forget` is not the rule.
4. Failure is visible — honest `Err` / typed partial-success / honest `CancelResult`; disk presence is not success.
5. Windows and Unix share one contract; Job / `unlink` are notes under it.
6. Singleton daemon remains one process per Hub home.
7. `ux-*.md` stays frozen.
8. Each §4 rule maps to a test in §5.

---

## 2. Keep / replace / delete

| Mechanism | Verdict | Notes |
|-----------|---------|--------|
| `mutate_registry` skip generation on new ids; commit-then-revoke; 10s/15s bounds; Conflict on in-flight ops | **KEEP** | rc.8 P0-1 root fix |
| CLI add = connect + `register_agent` only | **KEEP** | No local `agents.json` write |
| Timeout-as-success / `register_agent_local` / silent send retry / `connect_with_retry` / CLI cancel wall-clock | **KEEP DELETED** | rc.7 hiding; do not restore |
| `CancelResult.requested` + `acp_notify_enqueued`; no `agent_handle` cold-start | **KEEP** | Notification, not ACK |
| Ordinary RPC 30s → `Err`; `send wait=true` unbounded except product `--timeout` | **KEEP** | Reclassify committed-register timeout in P3 |
| `RpcClient` Drop abort, no join | **KEEP** as Drop | Not a close protocol by itself |
| `mem::forget(client)` on add | **DELETE** | Redundant with CLI exit; asymmetric; MCP/lib unprotected |
| `RpcClient::shutdown()` (async) | **ADD** | Mark closed, stop write, abort reader, optional short timeout-join, then Drop |
| CLI `process::exit` after `run()` | **KEEP** as last resort **after** `shutdown()` | Not connection GC |
| `CREATE_BREAKAWAY_FROM_JOB` as the default first path | **REPLACE** | Opt-in only (`BREAKAWAY_OK` jobs). Default is stay in-job with no inherit |
| In-job `DETACHED` reported as detach success | **DELETE** | Still in the job; H1 untouched |
| `cmd /c start /B` | **DELETE** | Shares job + inheritable handles; .NET redirected Wait still joins the grandchild |
| `DETACHED_PROCESS \| CREATE_NO_WINDOW` | **DELETE** as a combo | MSDN: `CREATE_NO_WINDOW` is ignored when `DETACHED_PROCESS` is set |
| `CreateProcessW` `bInheritHandles=FALSE` + `CREATE_NO_WINDOW \| CREATE_NEW_PROCESS_GROUP \| CREATE_UNICODE_ENVIRONMENT` | **ADD** (P1 primary spawn fix) | Spawn note (c)+(d); nginx / git / sccache. Stable `Command` cannot express this |
| Honest fail when **opt-in** breakaway is requested and the job denies it | **ADD** | Do not fall through to `DETACHED` / `start /B`. In-job no-inherit spawn without a breakaway request is **allowed** |
| Cancel `spawn_blocking` notify | **DELETE** | Replace with dedicated async notify; do not join on the cancel RPC |
| Eternal `Cancelling` / no supervisor | **REPLACE** | Force-finalize or surface after a budget |
| `design.md` §3.3 rollback-on-notify-fail | **REPLACE** (doc) | Align to mark-first + supervisor; do **not** restore rollback that blocks the RPC |
| Register 30s → `DaemonUnavailable` after disk commit | **REPLACE** | Typed partial-success / “committed but reply lost” |
| `refresh_registry_from_disk_if_stale` warn-and-continue | **REPLACE** (P3) | Epoch/fencing or `InvalidRegistry` |
| Northbound named pipe / UDS (`interprocess` local socket) | **KEEP** through P3 | TCP `127.0.0.1` only in optional P4 |
| `assume_flushed` / `evade_limbo` after reply read | **ADD** if reachable on the Windows send half | Do **not** `FlushFileBuffers` / `.flush()` on the CLI path |
| Official ACP stdio-as-Hub / Windows Service npipe | **REJECT** | Wrong product shape |
| Isolation tests: Drop vs inherit vs Job | **ADD** (P3) | rc.9 5× WaitForExit stacked four mechanisms |

---

## 3. Reconciled tensions

### A. `mem::forget`

Every stream that reached disk, and the critique, say **delete**.

Named-pipe research: current Drop already aborts and does not join (jsonrpsee-shaped). `forget` skips that abort. CLI `process::exit` is what actually releases the Tokio runtime; `forget` is redundant with it and harmful if exit is later removed. Critique: if Drop is unsafe, every other command and MCP is already broken; if Drop is safe, add-only `forget` is unnecessary. Comparable: `forget` is blunt `evade_limbo`, not a client lifecycle.

**Decision:** delete `mem::forget`. Add `async fn shutdown(self)`. Keep `process::exit` on the CLI only as a last resort **after** close, not as GC. Lib / MCP use the same Drop + `shutdown()` and do not call `process::exit`.

Critique asked to “join the closer.” That join lives **inside** `shutdown()` behind a short timeout (RPC memo: **≤250ms**; expiry is leak-and-continue), never inside `Drop`. Named-pipe research and Tokio runtime docs agree that joining I/O in `Drop` / `block_on` is the hang. Those two notes are compatible once the join is moved to the async close.

### B. Windows spawn

The RPC memo §6.2 / §7 still says Job breakaway is unique-and-necessary so a user-spawned singleton outlives a Job-aware parent. It explicitly forbids promoting that sentence to §5. The spawn note answers with a taxonomy:

| Class | What is still alive | Does breakaway fix it? |
|-------|---------------------|------------------------|
| **H1** inherited stdout/stderr write-end (Wait-for-EOF) | Descendant holds the parent’s redirected pipe | **No** |
| **H2** console attachment | Daemon still on the parent console | `CREATE_NO_WINDOW` **xor** `DETACHED_PROCESS` |
| **H3** CLI still in `RpcClient` Drop | The CLI process has not exited | **No** (this is P0) |
| **H4** supervisor waits the Job object / `KILL_ON_JOB_CLOSE` | Daemon is a job member | Only if breakaway succeeds — and then the daemon **escapes a supervisor that wanted the tree dead** |
| **H5** `start /B` wrong PID | Waiter holds `cmd.exe` or inherited grandchild handles | **No** |

`WaitForSingleObject(hProcess)` returns when that process terminates. A hang after “the CLI printed `registered`” is almost always H1, H2, or H3 — not H4. Rust `std::process::Command` always sets `bInheritHandles=TRUE`; `Stdio::null()` still inherits every other inheritable handle, including CI stdout and (in this tree) `daemon.lock` while `ensure_daemon` holds it. nginx `CreateProcess(..., 0, CREATE_NO_WINDOW, ...)`, git HANDLE_LIST of stdio only, and sccache `bInheritHandles=FALSE` all close H1 without breakaway.

Named-pipe research treated `DETACHED | BREAKAWAY | NO_WINDOW` as the right *family*. MSDN: `CREATE_NO_WINDOW` is **ignored** when ORed with `DETACHED_PROCESS`. That combo is on every in-tree path (`daemon.rs:786-822`). Path 1 fails unless the job has `BREAKAWAY_OK` (`KILL_ON_JOB_CLOSE` jobs usually do not). Path 2 then reports in-job `DETACHED` as success. Path 3 (`start /B`) still shares the job and inheritable handles; .NET redirected `WaitForExit` still waits the grandchild (dotnet/runtime#103384).

Critique C5 said “fail honest if cannot detach.” That is right as **do not claim detach when still in-job**. It is wrong as **must detach or refuse to spawn**. The spawn default is stay in-job with inheritance fixed. Supported parents wait the CLI **process handle**. Breakaway remains an explicit opt-in for a caller that set `BREAKAWAY_OK` on purpose. When that opt-in is requested and the job denies it, fail with a typed error (operator may run `acp-hub serve` out of band). Do not fall through to `DETACHED` / `start /B`.

**Decision — primary fix is no-inherit `CreateProcessW` (spawn (c)+(d)).**

- Always: `bInheritHandles=FALSE`, or `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` of exactly the intended NUL stdio handles. Do not inherit `daemon.lock`, the named-pipe server handle, or the parent’s redirected stdout/stderr. Stable `Command` cannot express this (`inherit_handles` is unstable, rust#146407); P1 needs a thin `CreateProcessW` wrapper.
- Flags: `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT`. Do **not** OR `CREATE_NO_WINDOW` with `DETACHED_PROCESS`. Ready remains `daemon.json` + connect (already in-tree), not `spawn` Ok.
- Breakaway: **off by default.** Opt-in only when the caller is in a job **and** `JOB_OBJECT_LIMIT_BREAKAWAY_OK` is set **and** the caller asked to leave. Successful breakaway from a `KILL_ON_JOB_CLOSE` harness is a misfire, not a win.
- In-job no-inherit spawn without a breakaway request is **allowed** and is the default. Document that supported parents wait the CLI process handle, not the Job object.
- **Delete** `cmd /c start /B`. **Delete** in-job `DETACHED` as a “detach succeeded” path.
- `ACP_HUB_DAEMON_STDERR=inherit` stays a debug hatch that reopens H1; do not combine it with a HANDLE_LIST that includes that pipe if the parent Wait is pipe-EOF.
- Unix `setsid` in `pre_exec` stays; it is session detach, not Job breakaway.

### C. Cancel

ACP `session/cancel`, LSP `$/cancelRequest`, and MCP `notifications/cancelled` are all **notifications**. Comparable B6 and critique C3 agree on mark-first + honest fields. They disagree only on the *delivery mechanism* and on what happens if the agent never stops.

**Decision:**

- **KEEP** mark-first (store CAS + runtime Cancelling), live-handle-only notify, `CancelResult` honesty, no `agent_handle` cold-start.
- **DELETE** `tokio::task::spawn_blocking` as the notify path (RPC memo: blocking-pool tasks cannot be aborted once started; `send_notification` is protocol I/O). Replace with `tokio::spawn` or a dedicated notify queue that is **not** joined on the cancel RPC. Do not “fix” the smell by joining notify on the RPC.
- **REPLACE** eternal `Cancelling`: after a documented budget, force-finalize the run or surface a typed “still running after cancel budget” result. Do **not** restore `design.md` rollback-that-blocks-RPC.
- **Align** `doc/dev/design.md` §3.3 (and the concurrency note at lines 207–212) to this contract in P2. Leaving the doc and the code opposed is how the next hang ticket re-litigates the same path.

### D. Transport

acp-hub already uses `interprocess` local sockets (Windows named pipe `\\.\pipe\acp-hub-{id}`, Unix domain socket). That is comparable pattern D. Pattern C (TCP `127.0.0.1`, sccache / ra-multiplex) removes limbo but is a protocol/endpoint change.

`interprocess` 2.4.2 documents Windows send-half **limbo**: Drop of a dirty send half runs `FlushFileBuffers` on a linger pool. After the RPC reply is fully read, the peer has consumed the request; the correct API is `assume_flushed()` / `evade_limbo()`, which **skip** that flush. Named-pipe research forbids `FlushFileBuffers` on the CLI teardown path — the same fact, from the other side. The public `local_socket` wrapper’s `poll_shutdown` is a no-op; P0 must reach the inner Windows `PipeStream` (or keep the unsplit stream) to call `assume_flushed`. If the inner type is not reachable without a crate change, P0 still ships `shutdown()` + abort-only Drop + CLI `process::exit`; limbo reachability is then a P0 follow-up, not a transport switch.

**Decision:** stay on the interprocess named pipe / UDS through P3. After each one-shot reply: shutdown write + `assume_flushed` / `evade_limbo` when the crate exposes it on the held type. Consider TCP `127.0.0.1` only in optional P4 if limbo remains after evade **and** no-inherit spawn. Do not switch transport in P0/P1. Do not adopt Docker-style Windows Service + npipe (pattern B) or ACP stdio-as-Hub (pattern A).

### E. Retry / honesty

**Never restore:** timeout-as-success, local `agents.json` as register success, silent send retry, `connect_with_retry`. Mid-command daemon EOF stays a visible error. No silent **mutate** reconnect. Wait / Store polls may reconnect (RPC class **WAIT** — reads of durable state). After a write on this pipe/UDS, Hub cannot prove unread (no `REFUSED_STREAM`); treat as unread-unknown = `Err`, or typed partial-success if disk already committed.

### F. Register timeout after commit

`mutate_registry` can commit `agents.json` and publish memory, then the client’s 30s bound fires. Mapping that to `DaemonUnavailable` is an honest *failure* and the wrong *class*: the daemon is up and the id may already exist.

**Decision:** typed partial-success / “committed but reply lost” (new `HubError` variant or structured RPC error data). Operator-visible: not `registered` exit 0, not “daemon unavailable.” CLI/MCP must not write `agents.json` locally to “heal” it. Idempotent retry of `register_agent` with the same id is a later product choice; this decision only forbids lying about the first attempt.

---

## 4. Default practice (normative)

Windows and Unix share this contract. Only the transport primitive and the spawn flags differ.

### 4.1 Owners

| Object | Owner | Reclaim |
|--------|--------|---------|
| Named pipe / UDS instance | Daemon listener + one accepted client stream | Client `shutdown()` / Drop abort; daemon `disconnect` after handler; no server `FlushFileBuffers` unless a proven “client still reading” invariant |
| RPC reader/writer tasks | `RpcClient` | Abort on Drop; timeout-join only in `shutdown()` |
| `daemon.lock` / `daemon.json` | Singleton `serve` per Hub home | Discover then handshake; leftover after kill is `doctor` / visible access-denied, not silent reuse |
| Agent handles / ACP stdio | Daemon, warm paths only | Cold add does not spawn a handle. Cancel never cold-starts `agent_handle`. |

### 4.2 Connect

Discover `daemon.json` → spawn if missing (P1 spawn law) → connect local socket → handshake → business RPC. Connect failure is `DaemonUnavailable` (or the existing typed connect error). No `connect_with_retry`. `ERROR_PIPE_BUSY` / `NotFound` stay connect-time retries only while the just-spawned daemon is coming up, bounded, and still `Err` if the bound expires.

### 4.3 Call (four classes — RPC memo §1)

Every Hub method is exactly one class. Mixing classes on one timeout or one “success” sentence is how rc.7 printed `registered`.

| Class | Bound | Retry | Success is | May join agent I/O? |
|-------|-------|-------|------------|---------------------|
| **MUTATE** (register, remove, create, handshake) | 30s honest `Err` | Never silent. Timeout after commit = typed partial-success | Daemon `Result` only. Disk presence ≠ success | **No** |
| **CANCEL** | 30s on the **mark** | Idempotent re-mark ok | `CancelResult { requested, acp_notify_enqueued }` | **No** |
| **WAIT** | Product `--timeout` → typed `Err` | **Reconnect OK** (Store poll) | Terminal run or typed timeout | Store only |
| **SEND** | Accept: ordinary 30s. Join (`wait=true`): product / unbounded | **NEVER replay** after drop / mid-stream EOF | Accept = admitted. Join = terminal | Join only |

“In flight” for cancel means a persisted run in `running` / `cancelling`, not “the cancel RPC is still writing to the agent.”

### 4.4 Return (three events)

1. **Bytes on the wire:** daemon wrote the JSON-RPC response (or the connection EOF’d).
2. **Rust `Result`:** `HubClient` / CLI future completed. This is **not** process exit.
3. **OS process exit:** CLI `main` calls `shutdown()` (best effort), then `process::exit`. Parent `WaitForExit` / `Start-Process -Wait` is allowed to return only after (3), and only if the daemon did not inherit the parent’s redirected handles.

MCP and `HubClient` in-process: (1) and (2) plus `shutdown()`. They do not use (3).

### 4.5 Drop / shutdown

```text
last RPC result read
  → shutdown write (if the type’s shutdown is not a no-op)
  → assume_flushed / evade_limbo on Windows send half if reachable
  → mark closed, abort reader/writer
  → optional timeout join inside shutdown() only (≤250ms; expiry = leak-and-continue)
  → Drop (abort again; never join; never FlushFileBuffers; never forget)
  → CLI: process::exit
```

Daemon idle-exit (ActivityTracker) stays. Last-client close does **not** by itself kill the singleton. No handshake version bump for P0: close is client-local. A later shutdown notification would be a protocol change and would need a version bump then.

### 4.6 Cancel

Mark durable hub state first. Return `requested` + `acp_notify_enqueued`. Notify is a dedicated async task, not joined on the RPC. After a budget: force-finalize or a typed still-running result. `design.md` is updated in P2.

### 4.7 Registry mutate

Disk commit is product truth. New ids skip generation wait. Live handles: bounded init / generation wait → `Err`. In-flight ops → `Conflict`. No CLI disk fallback. Commit-then-timeout → typed partial-success (P3). `refresh_registry_from_disk_if_stale` warn-and-continue is replaced in P3 with fencing or `InvalidRegistry`.

### 4.8 Windows-only notes

No-inherit `CreateProcessW`; do not inherit `daemon.lock` or the pipe server handle; `CREATE_NO_WINDOW` xor `DETACHED_PROCESS`; breakaway off by default (opt-in only); no `start /B`; leftover pipe / access-denied after force-kill is visible (`doctor` / manual home cleanup), not an unbounded hang. Isolation tests in P3 separate Drop (H3), inherit (H1), and Job (H4).

### 4.9 Unix-only notes

Same Drop / `shutdown()` / 30s / cancel contract. UDS unlink is the daemon’s listen-path job. Implicit EOF on last close is enough; no `forget`. `setsid` stays on spawn.

### 4.10 Forbidden

- Success-on-timeout
- Local `agents.json` as register success
- Silent reconnect or silent send retry
- `mem::forget` of a live `RpcClient`
- Joining agent generation I/O on a control RPC
- Reporting Job detach when the child is still in the job
- `FlushFileBuffers` / blocking flush on the CLI teardown path
- Rewriting `doc/dev/ux-*.md` or `doc/ssot/pillars/*`

UX-CORE’s “agent add ≤15s” line stays frozen. Implementer docs and CHANGELOG state the ordinary RPC bound is 30s honest `Err`. That drift is recorded, not “fixed” by editing operator notes.

---

## 5. Implementation phases

Do not start code until the gates in §7 for that phase are met. Each phase: files, tests, CHANGELOG if operator-visible, **no** `ux-*.md` rewrite.

| Phase | Intent | Depends | Acceptance |
|-------|--------|---------|------------|
| **P0** | Client close contract | This memo | `mem::forget` gone from all commands. `RpcClient::shutdown()` exists and is called on one-shot CLI/MCP paths. Drop remains abort-only (no join, no flush, no forget). Windows send half calls `assume_flushed` / `evade_limbo` when the held type exposes it. CLI still `process::exit` **after** `shutdown()`. Unit: Drop does not block; add path no longer leaks the client. |
| **P1** | Windows spawn | P0 | Thin `CreateProcessW` wrapper: `bInheritHandles=FALSE` (or HANDLE_LIST of NULs only). Flags `CREATE_NO_WINDOW \| CREATE_NEW_PROCESS_GROUP \| CREATE_UNICODE_ENVIRONMENT`. `start /B` deleted. `DETACHED\|NO_WINDOW` combo gone. Breakaway off by default; opt-in fail is typed, no `DETACHED` fallthrough. In-job no-inherit spawn is allowed. Child does not inherit `daemon.lock` or parent redirected stdout. Unix `setsid` unchanged. Test: Wait-for-EOF fixture (inherit=TRUE would hang; no-inherit does not). |
| **P2** | Cancel notify + design align | P0 | `spawn_blocking` notify gone. Dedicated async notify, not joined on the cancel RPC. Force-finalize or typed surface after budget. `design.md` §3.3 matches code. Tests: `acp_notify_enqueued=false` still honest; new escalation/finalize case; no rollback-that-blocks-RPC. |
| **P3** | Partial-success + fencing + isolation | P0–P2 | Register commit + lost reply is a typed partial-success, not `DaemonUnavailable`. `refresh_registry` fenced or `InvalidRegistry`. Isolation tests: (1) Drop/shutdown without `forget` unblocks process exit (H3); (2) inherit=TRUE would hang the fixture, no-inherit does not (H1); (3) default in-job spawn does not claim detach; opt-in breakaway denial is typed (H4). |
| **P4** (optional) | TCP loopback | P0–P1, and only if limbo remains after evade + no-inherit | RPC on `127.0.0.1`; named pipe may remain a oneshot ready handshake. Not started because of taste. |

Must-touch files (from archaeology §3.A.7), by phase:

- P0: `crates/hub/src/rpc.rs`, `crates/cli/src/commands.rs`, `crates/cli/src/main.rs` (keep exit; call shutdown first), MCP connect paths that own a client
- P1: `crates/hub/src/daemon.rs`
- P2: `crates/hub/src/hub/prompt.rs`, `crates/hub/src/hub/types.rs`, `crates/hub/src/acp.rs` / connection notify surface, `doc/dev/design.md`, cancel tests
- P3: `crates/hub/src/error.rs`, `crates/hub/src/rpc/error_data.rs`, `crates/hub/src/hub/client.rs`, `crates/hub/src/hub/registry.rs`, new isolation tests
- P4: `rpc.rs` / `daemon.rs` endpoint + handshake docs

### 5.1 Verification layers (plan §7)

A phase is not done on unit green alone.

| Layer | Required for |
|-------|----------------|
| Unit / crate | P0 shutdown, P2 cancel fields, P3 error class |
| CLI process exit (parent Wait after Ok and Err) | P0, P1 |
| Daemon leftover (second connect after kill/idle) | P1, P3 |
| Honesty (no success-on-uncertainty) | every phase |

---

## 6. Answers to plan §8

1. **May `RpcClient` outlive the CLI command?** No for one-shot CLI/MCP tool calls. Every command owns a close (`shutdown()` then Drop). In-process `HubClient` reuse across calls in one process is allowed; that process still owns the eventual `shutdown()`.
2. **Daemon shutdown?** Combination: idle timer (existing) + last-handle-close of *that client’s* pipe, not process death. No new shutdown RPC in P0–P3.
3. **Keep the pipe for the next in-process command?** Yes inside one long-lived MCP/`HubClient`. One-shot CLI still closes after the command.
4. **Cancel notify guarantee?** Enqueue (dedicated async task) + honest `acp_notify_enqueued`. Observed agent stop is a later supervisor/finalize signal, not the cancel RPC return.
5. **Are breakaway and `DETACHED` still required if Drop is specified?** Breakaway is optional/opt-in. `DETACHED|NO_WINDOW` as a combo is deleted. No-inherit is required regardless of Drop.
6. **Handshake version bump?** Not for P0–P3 client-local close. Bump only if a shutdown notification is added later.
7. **Leftover pipe / access-denied?** Visible error; recover via `doctor` / home cleanup. Not an unbounded hang and not silent reuse.
8. **`design.md`?** Update §3.3 in P2 as part of implementation, not by rewriting this memo. A dated `doc/dev/` addendum is this file; it is not a substitute for aligning `design.md` on cancel.

This change set is larger than a local fix. Per `doc/ssot/dev-principles/实现规划原则.md`, P1+ should refresh spec / design / BDD / TDD (at least cancel + spawn/teardown addenda) and close the review loop before those phases land. P0 is a local teardown fix under this memo and does not wait for a full five-doc rewrite, but it still must not start in the same turn as this decision.

---

## 7. Gates

- [x] D2 §5 default practice filled (this memo + plan §5)
- [x] D3 §6 phases filled (this memo §5 + plan §6)
- [x] D4 written decision recorded
- [ ] Spec / design / BDD / TDD addenda for P1+ (P2 must edit `design.md` §3.3)
- [ ] Adversarial review of those addenda if the implementer treats P1+ as non-small
- [ ] Implementation **not** started in the turn that wrote this memo
- [ ] No edits to `doc/dev/ux-*.md`
- [ ] No `docs/dev/` tree created

---

## Leftover (P2 review 2026-08-16)

Force-finalize CAS-closes a still-`cancelling` run; it does not kill the agent child or revoke the ACP handle. Prompt `OperationLease` / `RunLease` stay with the worker until the agent returns — releasing them earlier would admit a second turn on a live session; revoking the shared per-agent handle would drop every conversation on that connection. Kill-child is a later product choice (critique C3 offered it; this memo’s P2 only required force-finalize or a typed surface). Late `session/update` may still attach via `current_run` until the worker exits.

---

## Leftover (P3 review 2026-08-16)

P3 landed: isolation tests (H1 no-inherit fixture, H3 Drop/shutdown, H4 process-handle wait) + `HubError::CommittedReplyLost` (`-32018`) + fail-closed `refresh_registry` (`InvalidRegistry`, no disk-peek success). Confirmed: list timeout stays `DaemonUnavailable`; MUTATE timeout/EOF after write is `CommittedReplyLost`; CLI/MCP never print `registered` from a timeout or `agents.json` peek.

Still open (not P3 acceptance):

- **inspect / send unfenced memory reads.** `list_agents` / `list_proxies` / `mutate_registry` call `refresh_registry_from_disk_if_stale`. `inspect_agent` → `agent_config` and `send_prompt` → `agent_config` / `agent_handle` still read daemon memory without that fence.
- **No already-exists product error.** Register remains upsert. `CommittedReplyLost` copy says a later already-exists or not-found is a new call’s truth; that product error is not implemented.
- **No kill-child** (P2 leftover, still open).
- **P4 not started.** Optional TCP `127.0.0.1` if named-pipe limbo remains after evade + no-inherit.

---

**Document end.**
