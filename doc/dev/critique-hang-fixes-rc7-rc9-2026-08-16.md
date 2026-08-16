# Critique: rc.7–rc.9 hang fixes

| Field | Value |
|-------|--------|
| **Nature** | Implementer **verdict memo** (not operator SSOT; not a written decision; not a work-report close-out) |
| **Date** | 2026-08-16 |
| **Status** | Fills plan §3.C only. Does **not** close archaeology, industry B1–B6, or synthesis §5. |
| **Code** | **Do not implement** from this file. |
| **HEAD** | `7dcffe0` (`0.2.1-rc.9`) |
| **Commits** | `8346c56` (rc.7 / #65), `397c497` (rc.8 / #67), `7dcffe0` (rc.9 / #69) |
| **Frozen** | `doc/dev/ux-*.md`; `doc/ssot/pillars/*` — unread for rewrite. |
| **Sibling plan** | `doc/dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` |

This memo is an adversarial pass over the hang / error-hiding patches from 0.2.1-rc.7 through rc.9. It records what those commits made honest, what wait they left, and which in-tree mechanisms to keep, replace, or delete. Citations are `git:sha`, `code:path`, and `doc:path` at HEAD `7dcffe0`.

Industry notes (B1–B4, B6) are a separate workstream. Where they disagree with this table — in particular B4’s “abort-only Drop + keep `process::exit`” — synthesis must reconcile. This file does not rewrite those notes.

---

## 0. Verdict

The operator P0s were real: cold `agent add` that never returned after `agents.json` was written, `cancel` bound to a generation write, mid-turn `daemon closed`, and a parent `WaitForExit` that outlived a printed `registered`. The three releases did not produce a lifecycle. They produced a sequence of incident repairs.

- **rc.7** treated uncertainty as success (timeout + local `agents.json`, silent send retry, `connect_with_retry`).
- **rc.8** fixed the register lock-order hang and stopped joining cancel on agent I/O. It kept a CLI timeout-as-success path and inverted the documented cancel contract (`doc/dev/design.md` §3.3 still says notify failure rolls state back; the code is fail-open).
- **rc.9** deleted the error-hiding paths and added honest `CancelResult` fields. It then stacked `RpcClient` abort-no-join, `mem::forget(client)` on add, CLI `process::exit`, and a Job-flag cascade (`CREATE_BREAKAWAY_FROM_JOB` → in-job `DETACHED` → `cmd /c start /B`) so a five-run `WaitForExit` self-test would pass. Those stacked exits are not a close protocol. The self-test did not isolate Drop vs Job vs `process::exit`.

`doc/dev/design.md` §3.3 still says cancel notify failure rolls every state back before the caller may retry. The code marks first and returns even when notify is skipped or later fails. That inversion is undocumented.

What remains worth keeping is listed in §3. The rest is either already deleted (keep it deleted) or still in tree as a workaround that synthesis must replace. Until §5 records a written decision, treat rc.8/rc.9 as **incident repairs**, not the default practice.

---

## 1. Per-commit autopsy

### 1.1 `8346c56` — rc.7 (`#65`)

**What they thought they were solving.** rc.6 operator P0s (`doc/dev/ux-unified-feedback-2026-07-26-rc6.md`): cold `agent add` never returns after `agents.json` is written; long-task `cancel` can sit >10 min; mid-chat `daemon closed`. The work report (`doc/dev/work-report-rc6-p0-add-cancel-daemon-2026-07-26.md`) states the intended design: 15s register timeout + local write, 8s/12s cancel budgets, reconnect-once + send retry.

**What they actually changed.**

| Path | Change |
|------|--------|
| CLI `agent add` | Wrap connect+register in a 15s timeout. On timeout **or** daemon error, `register_agent_local` writes `agents.json` and prints `registered` / exit 0. Same fake success if the id is already on disk. |
| Cancel | Hub marks first, then **awaits** `session/cancel` with an 8s budget. CLI adds a 12s wall-clock wrapper. |
| Send | String-match `"daemon"` / `"connection is closed"` and send again. |
| Connect | `connect_with_retry` swallows the first `DaemonUnavailable`. |
| Registry | `refresh_registry_from_disk_if_stale` so the daemon can ingest the CLI’s local write. `let _ = refresh`. |
| Tests | `failed_cancel_notification_rolls_back_*` rewritten so notify failure is treated as success. |

**What they got right.** Mark-first on cancel was the first correct move: the operator must get a durable hub mark without waiting on the generation pipe. Disk-fingerprint refresh was a real problem (external edits / tooling), even though it was introduced to absorb an illicit CLI writer.

**What is still wrong (and why it is not a lifecycle).** Disk presence was treated as RPC success. A second send was treated as healing a drop. The first connect error was swallowed. Those are the error-hiding cases later named in `doc/dev/AUDIT-error-hiding-self-review-2026-07-26.md` §2. The register hang was not instrumented; a second registry writer was invented instead. Cold add still hung (timeout printed success while the process could still be stuck). Cancel still joined agent I/O after the 8s budget path. P0-3 (daemon death / projection) was not a self-heal; it was a silent retry.

**Classification.** Contract lie on add/send/connect. Thin symptom bandage on cancel. Do not bring any of the deleted success-on-uncertainty paths back.

---

### 1.2 `397c497` — rc.8 (`#67`)

**What they thought they were solving.** They correctly named rc.7 as unfit (`doc/dev/work-report-rc6-p0-root-cause-2026-07-26.md`). Claimed P0-1 was unbounded `agent_generation_writer` before `agents.json` commit; P0-2 was joining `send_notification` on the shared ACP connection.

**What they actually changed.**

| Path | Change |
|------|--------|
| `mutate_registry` (`crates/hub/src/hub/registry.rs`) | Conflict if ops are in flight; 10s init wait; **new agent ids skip generation locks**; live handles `try_write` with 15s/20ms poll; **commit disk + epoch, then revoke handles**. |
| CLI add | Deleted `register_agent_local` as the primary path. **Kept** timeout-as-success if `agent_on_disk` after a stalled RPC (removed one release later). |
| Cancel (`prompt.rs`) | Mark CAS + runtime Cancelling, then `tokio::task::spawn_blocking` `send_notification` **with no join**, no `agent_handle` cold start. |
| CLI cancel | Timeout wrapper kept (later deleted in rc.9). |

**What they got right.** The `mutate_registry` lock/order change is a root-cause fix for *one* hang: register RPC blocked on generation write. New ids skip that wait; live handles are bounded; in-flight ops are `Conflict`; commit happens before revoke. Mark-first cancel plus “never cold-start `agent_handle` on cancel” is also correct. A wall-clock on the CLI cannot replace “this RPC path must not join agent I/O.”

**What is still wrong.** QA on rc.8 still hung with `registered` printed (`doc/dev/work-report-error-hiding-audit-2026-07-26.md` §3). The lock fix was necessary and not sufficient. Fire-and-forget cancel unblocked the RPC by refusing to observe the write; it did not make notify out-of-band. `spawn_blocking` with no join can park a blocking-pool thread behind a saturated generation write. `design.md` §3.3 specified rollback-on-notify-fail; the code inverted that instead of splitting the write path or escalating (dedicated notify writer, kill child, force-finalize). Remaining CLI disk-success was still error hiding. P0-3 untouched. `refresh_registry_from_disk_if_stale` remained, now without its original illicit client.

**Classification.** Register lock-order: **keep**. Cancel mark-first / no cold-start: **keep**. Unjoined `spawn_blocking` as the notify mechanism: **delete**. Fail-open without a supervisor: **replace**. Remaining timeout-as-success: already deleted in rc.9 — **stay deleted**.

---

### 1.3 `7dcffe0` — rc.9 (`#69`)

**What they thought they were solving.** Operator: timeout-as-success / local write / swallow = error hiding. QA: rc.8 still hung after `registered`. They diagnosed (a) `RpcClient` Drop blocking on named-pipe teardown, (b) job-aware parents waiting on `serve`.

**What they actually changed.**

Honest-contract work (keep):

- Deleted CLI timeout-as-success, send retry, cancel wall-clock, `connect_with_retry`.
- `CancelResult.acp_notify_enqueued` + mark-only / already-requested copy (`types.rs`, `commands.rs` `handle_cancel`).
- `tracing::warn` on registry refresh / close `finalize_run_cas`; wait serialize to stderr; daemon probe `debug`.
- Ordinary RPC 30s timeout as **failure** (`rpc.rs` `DEFAULT_RPC_REQUEST_TIMEOUT`). `send` `wait=true` stays unbounded except product `--timeout`.

Lifecycle workarounds (replace or delete):

- `std::mem::forget(client)` after successful add only (`commands.rs:71`).
- `RpcClient` fields in `ManuallyDrop`; Drop aborts reader/writer and **does not join** (`rpc.rs:374-392`). Comment: leak the connection.
- Windows spawn: `CREATE_BREAKAWAY_FROM_JOB` → `DETACHED` → `cmd /c start /B` (`daemon.rs:754-831`).
- CLI `main.rs` `process::exit` so leftover RPC tasks cannot hold the runtime.

**What they got right.** Deleting the rc.7 hiding paths is the correct contract. `CancelResult` honesty is the correct operator surface: `requested` = hub mark; `acp_notify_enqueued` = scheduled, not delivered. 30s ordinary RPC as `Err` is an honest bound (the error *class* still needs work when register may have committed). Warn-on-skip is better than silent skip. CLI add is now connect + `register_agent` only.

**What is still wrong.** They did not implement a close protocol. They arranged not to run destructors, and they did not verify job membership after a successful spawn.

- `mem::forget` is add-only. list/remove/send still Drop. MCP / lib have no `forget` and no `process::exit` (`plan` §3.A.5 A10).
- Abort-without-join moves `CloseHandle` onto a runtime worker and refuses to wait; CLI then `process::exit`s. That is process death used as connection GC, not a specified Drop.
- Spawn path 1 fails when the job denies breakaway; path 2 (`DETACHED` without breakaway) **succeeds in-job** — the common case when breakaway is denied. Path 3 (`start /B`) may still share the job (`daemon.rs:807` comment). Changelog text that treats `start /B` as the fix overstates what the cascade does.
- Five green `WaitForExit`s after four stacked mechanisms is not an isolation of Drop vs Job vs `process::exit`.
- 30s timeout as `DaemonUnavailable` is honest as a bound and wrong as a class when the daemon may already have committed `agents.json`.
- Cancel fail-open remains: mark, then `handles.get`; revoke/replace in between → `acp_notify_enqueued=false`, run stuck `Cancelling`; second cancel is a no-op (`requested=false`) with no notify retry. Audit §6 admits delivery failure and defers force-finalize; there is no force-finalize.
- P0-3: silent retry removed (keep that absence). Exposing `daemon_unavailable` is not a projection rebuild.

**Classification.** Error-hiding deletions and honesty fields: **keep**. Hang “fix”: **symptom bandage**. The self-test proved a stacked workaround can exit. It did not prove which layer blocked the parent Wait, or that MCP/lib can Drop.

---

## 2. What they got right vs still wrong

**Correctly identified as rc.7 band-aids (do not bring back)**

- Timeout + `agents.json` already has id → `registered` exit 0
- `register_agent_local` as the success path
- Send-again after drop (duplicate turn)
- `connect_with_retry` swallowing the first error
- CLI cancel wall-clock as a substitute for not joining agent I/O

**Still wrong after rc.9**

- `mem::forget(client)` — destructor skip, add-only
- `CREATE_BREAKAWAY_FROM_JOB` then silent in-job `DETACHED` then `start /B`
- `RpcClient` Drop abort-without-join sold as a close protocol
- `process::exit` as the real destructor
- Fire-and-forget `spawn_blocking` cancel with no delivery supervisor
- `refresh_registry_from_disk_if_stale` leftover from the illicit writer, now “external edits” (design said those are unsupported)
- No isolation of Drop vs job vs runtime; 5× self-test after stacking
- `design.md` cancel rollback vs code fail-open

**Honest-error-contract work that should be kept**

- Daemon `register_agent` Ok is the only add success
- No silent send retry; no `connect_with_retry`
- `CancelResult.requested` = hub mark; `acp_notify_enqueued` = notify scheduled, not delivered
- CLI mark-only / already-requested copy (never “no active run” when `run_id` is known)
- 30s ordinary RPC timeout as **Err** (reclassify the error code later)
- `send` / `wait` unbounded except product `--timeout`
- warn on refresh / close finalize / wait row skip
- `mutate_registry`: skip generation for new ids; bounded try_write for live handles; commit then teardown; Conflict on in-flight ops

---

## 3. Keep / replace / delete / add

Hang-related mechanisms at HEAD `7dcffe0`. “Keep deleted” means the absence is the contract.

| Mechanism | Location | Verdict | Why |
|-----------|----------|---------|-----|
| `mutate_registry` skip generation on new ids; commit-then-revoke | `crates/hub/src/hub/registry.rs` | **KEEP** | Actual P0-1 lock-order fix. |
| Bounded init (10s) / live `try_agent_generation_writer` (15s) → **Err** | same | **KEEP** | Honest bound. Expiry is a typed error, not success. |
| Conflict on in-flight ops | same | **KEEP** | Fail-fast, not spin. |
| `refresh_registry_from_disk_if_stale` + warn-and-continue | `registry.rs` `list_agents` / `mutate_registry` | **REPLACE** | Built for the rc.7 local write. Either a real external-edit protocol with epoch/fencing, or treat foreign writes as `InvalidRegistry`. Warn-and-continue is stale-as-truth. |
| CLI add = connect + `register_agent` only | `crates/cli/src/commands.rs:61-72` | **KEEP** | The one rc.9 add contract that is right. No local write; no timeout-as-success. |
| `std::mem::forget(client)` | `commands.rs:71` | **DELETE** | Skips Drop on one command. If Drop is safe, this is unnecessary. If Drop is unsafe, the transport is broken for every other command and for MCP. |
| `RpcClient` `ManuallyDrop` + abort, no join | `crates/hub/src/rpc.rs:374-392` | **REPLACE** | Abort-without-join is “don’t wait for CloseHandle.” Replace with an explicit close: mark closed, shutdown write, drain reader with timeout, then drop. Join the closer. |
| `std::process::exit` after CLI `run()` | `crates/cli/src/main.rs:24-43` | **REPLACE** | Acceptable as a last-resort hard exit **after** a real close. Not a substitute for Drop. Today it is the GC. |
| `CREATE_BREAKAWAY_FROM_JOB` first | `crates/hub/src/daemon.rs:785-796` | **REPLACE** | Valid *attempt*. On failure, do not pretend detach worked. Query job limits; fail honest if the process cannot leave the job. |
| `DETACHED` fallback as success | `daemon.rs:798-805` | **DELETE** (as a success path) | Stays in the job. This is the common case when breakaway is denied. |
| `cmd /c start /B` last resort | `daemon.rs:807-831` | **DELETE** | Does not leave the job. Changelog overstates this as the fix. |
| Honest “cannot detach from this job” | (missing) | **ADD** | Query job limits; tell the operator to run `acp-hub serve` out of band. |
| `DEFAULT_RPC_REQUEST_TIMEOUT` 30s → Err | `rpc.rs:52`, `rpc.rs:291` | **KEEP** (reclassify later) | Honest silence bound. Do not map committed-but-slow register to `DaemonUnavailable` forever. |
| 30s `DaemonUnavailable` when register may have committed | `rpc.rs` + register path | **REPLACE** | Typed partial-success (disk/daemon committed, client did not see the ACK). Better than fake `registered`; still a confused class. |
| `send` wait=true / product `wait --timeout` unbounded-or-explicit | `client.rs:142-152` / wait | **KEEP** | Agent wall clock. Not hiding. |
| Cancel mark-first (CAS + runtime Cancelling) | `prompt.rs:221-293` | **KEEP** | Operator must get a durable hub mark without joining agent I/O. |
| `acp_notify_enqueued` + CLI copy | `types.rs:92-108`, `commands.rs:788-820` | **KEEP** | `requested` = hub mark; `acp_notify_enqueued` = scheduled, not delivered. |
| `spawn_blocking` + no join (as the notify mechanism) | `prompt.rs:318-330` | **DELETE** | Does not make notify out-of-band. Leaks blocking threads. `enqueued=true` is a scheduled hope. |
| No `agent_handle` on cancel (live map only) | `prompt.rs:299-311` | **KEEP** | Cold-start on cancel was a real footgun. |
| Cancel fail-open (no rollback, no supervisor) | `prompt.rs` + deleted rollback tests | **REPLACE** | Mark-first is fine. Then: dedicated notify writer **or** kill agent child; bound wait for `StopReason::Cancelled`; else **force-finalize** and surface it. Today `Cancelling` can be eternal. |
| Tests asserting notify-fail is success | `crates/hub/src/hub/tests/cancel.rs` | **REPLACE** | Keep `acp_notify_enqueued=false`. Add delivery/escalation tests. Restore a fail-closed path for “mark succeeded, agent still running past budget.” |
| No silent send retry / no `connect_with_retry` | CLI (rc.9 deletion) | **KEEP** (stay deleted) | First failure must surface. |
| Timeout-as-success / `register_agent_local` | — | **KEEP DELETED** | Already gone. Do not restore. |
| warn on refresh / close finalize / wait row skip | `registry.rs`, `lifecycle.rs:165-176`, wait | **KEEP** | Observable skip is better than silent skip. Refresh *policy* is still REPLACE (row above). |
| P0-3 daemon self-heal / projection rebuild | (missing) | **ADD** | Exposing `daemon_unavailable` is honest. It is not a fix. |
| Isolation tests: Drop vs job vs `process::exit` A/B | (missing) | **ADD** | The rc.9 self-test stacked four mechanisms. A later close protocol needs a test that fails if the wrong layer is what unblocked `WaitForExit`. |

---

## 4. Undocumented inversion — `design.md` §3.3

`doc/dev/design.md` §3.3 (lines 172–175) still says:

> Cancel: under the conversation operation lock, CAS the exact persisted run from running to cancelling, transition runtime Live to Cancelling, then call `cx.send_notification(CancelNotification)` directly (bypasses blocked loop). **A send failure rolls every state back before the caller may retry.**

The same file’s concurrency notes (lines 207–212) repeat: notify send failure restores operation flag, runtime, run, and conversation.

HEAD code (`prompt.rs:221-348`) marks first, schedules `spawn_blocking` without join, and returns `requested=true` even when notify is forced-skipped or later fails (`tracing::warn` only). `CancelResult` documents that `acp_notify_enqueued` is not a delivery ACK (`types.rs:92-108`). The test `cancel_marks_requested_even_when_agent_notify_fails` asserts that contract.

This is not a mature, reviewed contract change. It is an inversion that was never written back into `design.md`. Synthesis must pick one:

1. Keep fail-open mark + honest fields, then add a supervisor (dedicated writer / kill child / force-finalize) and update `design.md` in the written-decision step; or
2. Restore fail-closed rollback and make notify actually out-of-band so rollback is rare.

Leaving the doc and the code opposed is how the next hang ticket will re-litigate the same path.

---

## 5. Remaining waits and success-on-uncertainty (for plan §9 C.*)

### 5.1 Unbounded `.await` on control RPC stacks

After rc.8, register / cancel / list no longer join generation write without a bound. What remains:

| Stack | Wait | Class |
|-------|------|-------|
| Ordinary hub RPC (register/list/cancel/…) | 30s client bound → `Err` | **KEEP** as bound; **REPLACE** `DaemonUnavailable` when register may have committed |
| `send` `wait=true` | unbounded (`client.rs:142-152`) | **KEEP** (product attach) |
| Product `wait --timeout` | explicit product bound; expiry is error | **KEEP** |
| Cancel `spawn_blocking` notify | unbounded, **not joined** on the RPC | **DELETE** as the notify mechanism |
| `RpcClient` Drop | abort, no join; CLI then `process::exit` | **REPLACE** with explicit close |
| Windows spawn fallback | `DETACHED` / `start /B` reported as spawn Ok while still in job | parent Wait can outlive CLI print |

Control RPCs should stay off the agent generation write. That rule is already correct. The leftover unbounded work is teardown, Job membership, and cancel *delivery*, not another CLI wall-clock.

### 5.2 Remaining success-on-uncertainty

| Path | What the operator / caller sees | Why it is still uncertainty |
|------|--------------------------------|-----------------------------|
| `refresh_registry_from_disk_if_stale` warn-and-continue | list/mutate proceeds from memory | Disk and memory may diverge; no `InvalidRegistry` |
| `DETACHED` fallback Ok | daemon “spawned” | Child may still be in the parent Job |
| `cmd /c start /B` Ok | same | Comment admits job share |
| Cancel notify fail / no handle | `requested=true` or mark-only copy | Honest fields; run can stay `Cancelling` with no supervisor |
| `cancel_marks_requested_even_when_agent_notify_fails` | test green | Encodes fail-open as the success path |
| Register commit then 30s timeout | `DaemonUnavailable` | Honest *failure*, wrong *class* — needs typed partial-success |
| `mem::forget` + `process::exit` | CLI process gone | Pipe/tasks not closed; MCP/lib do not share this pair |

Deleted and must stay deleted: timeout-as-success, `register_agent_local`, silent send retry, `connect_with_retry`.

---

## 6. C1–C6 mapped to the wait graph

| ID | Patch | Verdict (short) |
|----|-------|-----------------|
| C1 | `mem::forget(client)` on `agent add` | **DELETE.** Add-only leak until `process::exit`. Asymmetric vs remove/list. MCP/lib unprotected. |
| C2 | Non-blocking `RpcClient` Drop | **REPLACE.** Abort-no-join is a workaround. Explicit close: mark closed, shutdown write, drain reader with timeout, then drop. `process::exit` only after that, last resort. |
| C3 | Cancel notify fire-and-forget | **KEEP** mark-first, honesty fields, no `agent_handle` cold-start. **DELETE** unjoined `spawn_blocking` as the notify mechanism. **REPLACE** fail-open with dedicated writer or kill-child + bound wait for `Cancelled` + force-finalize. |
| C4 | 30s RPC vs unbounded wait/attach | **KEEP** 30s ordinary RPC as `Err`; **KEEP** send/wait unbounded except product `--timeout`. **REPLACE** committed-register + 30s `DaemonUnavailable` with typed partial-success. Reclassify later; do not turn the bound back into success. |
| C5 | Daemon spawn DETACHED + Job | **REPLACE** breakaway-first with query-limits + honest fail. **DELETE** in-job `DETACHED` as success and `cmd /c start /B`. **ADD** “cannot detach from this job; run `serve` out of band.” |
| C6 | Handshake + idle vs “daemon closed” | **KEEP** no silent reconnect / no send retry. **ADD** P0-3 daemon self-heal / projection rebuild. Exposing `daemon_unavailable` is not that fix. Idle-exit vs mid-turn EOF was not re-measured in archaeology (A5); synthesis still owes a reconnect-vs-rebuild rule. |

---

## 7. What synthesis must not treat as settled

1. A green unit suite, or five cold-add `WaitForExit`s after stacked workarounds, is not proof that Drop is safe or that a Job-aware parent can return.
2. B4 (named-pipe / Tokio note) recommends abort-only Drop and keeping CLI `process::exit`. This critique requires an explicit close first, with `process::exit` only as last resort. Reconcile in §5; do not silently pick one.
3. B6 notes that store-first + fire-and-forget *matches* ACP / LSP / MCP cancel *shape*. That is an argument for mark-first and notification-not-request. It is not an argument for unjoined `spawn_blocking` or eternal `Cancelling`.
4. Do not edit `doc/dev/ux-*.md`. If UX-CORE still says `agent add` ≤15s, that drift is recorded in archaeology A3.A.4; the written decision says which implementer doc moves.

---

**Document end.**
