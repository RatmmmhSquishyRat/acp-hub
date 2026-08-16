# Research: CLI↔daemon RPC lifecycle

| Field | Value |
|-------|--------|
| **Nature** | Implementer research note (not operator SSOT; not a written decision) |
| **Date** | 2026-08-16 |
| **Status** | Fills plan §3.B **B5** (JSON-RPC stream / call classes / retry / cancel / EOF). Also records the **RPC-lifecycle** contract that B4 teardown and B6 peers assume. Does **not** close B1–B3 (pipe / Job / UDS API walks) or reopen B4 / B6. |
| **Code** | **Do not implement** from this file. |
| **Frozen inputs** | `doc/dev/ux-*.md`; `doc/ssot/pillars/*` — unread for rewrite. |
| **Sibling plan** | `doc/dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` |
| **Sibling notes** | B1–B4 `doc/dev/research-named-pipe-tokio-2026-08-16.md`; B6 `doc/dev/research-comparable-ipc-daemons-2026-08-16.md`; windows-daemon-spawn (sibling, Job / inherit — **tension in §7**) |
| **In-tree baseline** | Archaeology §3.A at `HEAD 7dcffe0` (`0.2.1-rc.9`) |

This note imports **call-class, ack, retry, shutdown, and supervision** rules for a one-shot CLI talking JSON-RPC to a singleton daemon. Transport teardown (named-pipe limbo, Tokio Drop) is already in the B1–B4 note. Peer topology (stdio child vs auto-spawn daemon) is already in the B6 note. This file answers: once the byte stream exists, **which RPCs may wait, which may retry, and what “done” means**.

Citations are public specs / reference implementations. In-tree facts stay at `code:` / `doc:` / `git:7dcffe0`.

---

## 0. What this note is for

acp-hub already multiplexes newline JSON-RPC over one pipe/UDS (`code:crates/hub/src/rpc.rs`). Ordinary calls use a 30s honest bound; `send wait=true` is unbounded; cancel is mark-first (`doc:dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` §3.A). The hang family that remains after rc.8/rc.9 is not “we forgot JSON-RPC.” It is **one connection carrying four different operation classes**, plus a client that used `mem::forget` to dodge Drop.

The official ACP SDK specifies the **southbound** agent link (`session/cancel` is a notification). It does not specify northbound CLI↔daemon retry, process exit, or supervision. Copying ACP stdio as Hub architecture is the B6 category error. This note stays on the **northbound** contract.

---

## 1. Four operation classes

Every Hub method belongs to exactly one class. Mixing classes on one timeout, one retry policy, or one “success” sentence is how rc.7 printed `registered` and how cancel joined a generation write.

| Class | Northbound shape | Bound | Retry / reconnect | Success is | In-tree examples |
|-------|------------------|-------|-------------------|------------|------------------|
| **Unary mutate** | JSON-RPC **request** (has `id`); daemon commits then replies | **At-most-once.** Ordinary **30s** (`DEFAULT_RPC_REQUEST_TIMEOUT`, `rpc.rs:52` / `rpc.rs:291`) | **Never** silent retry. Timeout / EOF is `Err`, even if disk already changed | **Daemon result only.** `agents.json` / `daemon.json` presence is not success | `register_agent`, `remove_agent`, `hub/conv/create`, handshake |
| **Unary cancel** | Northbound **request** whose **ack is the hub mark**; southbound ACP `session/cancel` is a **notification** | Ordinary 30s on the **mark**. Must not join agent I/O | Idempotent re-mark is ok (`requested=false` if already marked). Do not re-wait notify | `CancelResult { requested, acp_notify_enqueued }` — not “agent stopped” | `hub/conv/cancel` (`prompt.rs:190-348`, `types.rs:92-108`) |
| **Wait** | Store-poll attach (request per poll, or one long request) | **Product** timeout (`wait --timeout` → typed `Err`, `wait.rs:59-62`) | **Reconnect OK.** Polls are reads of durable Store; a new connection may continue | Terminal run **or** typed timeout. Empty wait is not success | `hub/conv/wait` / `wait_run_*` |
| **Streaming send** | Accept is a short request; join is product / unbounded | **Accept** uses the ordinary bound (`send wait=false`). **Join** (`wait=true`) is agent wall clock — `timeout=None` (`client.rs:142-152`) | **NEVER replay after drop.** Mid-stream EOF is `Err`. rc.9 deleted silent send retry — **keep that absence** | Accept = daemon admitted the turn. Join = terminal (or product timeout). Drop ≠ license to send again | `hub/conv/send` |

Rules that follow from the table:

1. A wall-clock on the CLI cannot move a method into another class. A 12s cancel timeout (rc.7, removed) did not make cancel a wait.
2. `send wait=true` is class **streaming send / join**, not class **unary mutate**. Do not apply the 30s ordinary bound to it, and do not treat its unbounded wait as a license to replay.
3. Register commit-then-timeout is a **typed partial-success** problem for synthesis (archaeology A4), not a retry. The client must not invent `registered` from disk.

---

## 2. Ack versus notification

JSON-RPC 2.0 already splits the wire:

| Kind | `id` | Peer must reply? | Confirmable? |
|------|------|------------------|--------------|
| **Request** | present | Yes (`result` or `error`) | Yes — that reply **is** the ack |
| **Notification** | absent | **MUST NOT** reply | **No.** Delivery and application success are invisible on this message |

Source: [JSON-RPC 2.0 §4 / §4.1](https://www.jsonrpc.org/specification). “Notifications are not confirmable by definition.”

Two stacked protocols in this tree use that split differently:

| Link | Cancel primitive | What the caller may wait for |
|------|------------------|------------------------------|
| **Northbound** CLI → daemon | `hub/conv/cancel` **request** | Hub durable mark (`requested`) + whether a notify task was scheduled (`acp_notify_enqueued`) |
| **Southbound** daemon → agent | ACP `session/cancel` **notification** | Nothing. Agent later reports idle / `StopReason::Cancelled` on the prompt lifecycle, not as a reply to the notify |

ACP schema: `session/cancel` is a notification; the agent SHOULD stop work and later emit a cancelled stop reason. Clients SHOULD still accept late `session/update` after sending cancel. Same shape as LSP `$/cancelRequest` and MCP `notifications/cancelled` (B6 §4).

### 2.1 `CancelResult` is the mature northbound ack

```text
requested              = hub store CAS + runtime Cancelling applied on this call
acp_notify_enqueued    = live handle existed AND session/cancel was scheduled
                         (not delivery ACK; delivery may still fail)
```

Keep this shape (`types.rs:92-108`). Do not collapse it to a single `cancelled: bool`. Do not restore `design.md:172-175` “send failure rolls every state back before the caller may retry” as the RPC join rule — that sentence describes an older in-process path and contradicts the rc.8 don’t-join fix.

Idempotent second cancel: `requested=false`, `acp_notify_enqueued=false`, `run_id` still present (`hub::tests::cancel`). That is an honest ack, not a hang.

### 2.2 `spawn_blocking` is an implementation smell; don’t-join is the policy

Current notify (`prompt.rs:318-331`):

```text
mark first → tokio::task::spawn_blocking(|| handle.cx.send_notification(...)) → return
```

| Piece | Verdict |
|-------|---------|
| Mark first; return without joining agent write | **Keep.** This is the rc.8 root fix and the industry cancel contract. |
| Fire-and-forget (no `.await` on the RPC stack) | **Keep.** |
| Vehicle = `spawn_blocking` | **Smell.** `spawn_blocking` is the blocking-thread pool. Those tasks **cannot be aborted** (Tokio `JoinHandle::abort` is a no-op once the blocking work has started). `send_notification` is protocol I/O, not `std::fs` / CPU. Prefer `tokio::spawn` or a dedicated notify queue after a written decision. |
| Notify fail → warn; run stays `cancelling` | Honest, and already surfaced as `acp_notify_enqueued` when the spawn was skipped. Delivery failure after enqueue remains a residual (archaeology A4). |

Do not “fix” the smell by joining the notify on the cancel RPC. That rebinds cancel to generation wall clock (observed >10 min, `doc:dev/work-report-rc6-p0-root-cause-2026-07-26.md` §2).

---

## 3. Honest reconnect / retry

gRPC A6 + client-go are the reference for “when is a retry not a lie.”

Transparent retry is allowed only when the library can prove the **server application never saw the request** (never left the client; or `RST_STREAM/REFUSED_STREAM` / `GOAWAY` with last-stream-id below this stream). Configurable retry is for status codes that mean “not processed,” and only when the **service owner** says the method is safe to run twice. gRPC does **not** mark methods idempotent for you. Hedging a mutate is how you get two turns.

Import for Hub (northbound JSON-RPC over one byte stream):

| Situation | Allowed? | Why |
|-----------|----------|-----|
| Connect failed; no request bytes written | Retry **connect** (visible, bounded). Not a call retry | Unread |
| Reader EOF / `GOAWAY` before this request’s `id` could have been dispatched | Retry **that unread request** only if the client can prove it | gRPC case 2–3. Hub today has **no** such proof on a raw pipe — treat as **unread-unknown** |
| Ordinary mutate (`register`, `create`, `send` accept) — timeout, EOF after write, or `DaemonUnavailable` | **Never** silent retry | May already have committed. rc.9 deleted send retry and `connect_with_retry` — **keep** |
| Cancel already marked | Re-call is idempotent; do not claim a new mark | Ack fields say so |
| Wait / Store poll after disconnect | **Reconnect OK** | Read of durable state; new connection continues the poll |
| Streaming send after drop / mid-turn daemon EOF | **NEVER replay** | Second `session/prompt` is a new turn. Operator sees `daemon closed` (`doc:dev/cursor-adapter/e2e-investigation-2026-07-24.md`) |

“Unread-unknown” on a raw named pipe / UDS: the client wrote a line and then lost the socket. There is no `REFUSED_STREAM`. Synthesis must not invent a transparent retry here. The honest sentence is `Err` (daemon did not answer / connection reader stopped). If disk already has the id, that is archaeology’s **typed partial-success**, not a second `register_agent`.

Never: timeout + `agents.json` contains id ⇒ print `registered` (rc.5/rc.7; removed). Never: silent reconnect that hides the first failure (rc.9 deletion).

---

## 4. Structured shutdown (no `mem::forget`)

B4 already ranked alternatives. This section states the **RPC-lifecycle** close so call classes have a defined end.

Industry default (jsonrpsee Drop = oneshot, never join; rust-analyzer = protocol `shutdown`/`exit` **then** `IoThreads::join`; Tokio `TcpStream::shutdown` is async; `Runtime::drop` waits **forever** for a reader that does not yield):

```text
1. Stop sending (close outbound / mark closed).
2. Abort reader + writer (or cancel token). Do not join on Drop.
3. Optional: await joins inside async shutdown() with a short timeout (≤250ms).
4. Drop the client (signal only).
5. CLI: process::exit so leftover tasks cannot own WaitForExit.
```

| Rule | Status |
|------|--------|
| `RpcClient` Drop: mark closed, **abort** IO, **do not join** (`rpc.rs:374-392`) | **Already mature.** Keep. |
| Optional ≤250ms join **inside** `async fn shutdown(self)` only | Recommended. Sibling B4 sketch used 200ms; this memo caps at **250ms**. Expiry is leak-and-continue, not a hang. |
| CLI `process::exit` after `run()` (`main.rs:24-43`) | **Keep** for the one-shot binary. Lib / MCP do not get this belt. |
| `mem::forget(client)` after successful `agent add` (`commands.rs:71`) | **FORBIDDEN as a success path.** Skips abort; redundant with `process::exit`; leaves a live reader if exit is later removed. Asymmetric vs remove/list. |
| Unbounded `FlushFileBuffers` on the CLI thread | **FORBIDDEN.** MSDN: server-end flush waits until the client reads. Client-end flush on a daemon that is not draining is the limbo hang (B1 / `interprocess`). Drop and `shutdown` must not call it. |

`forget` is not a close. It is a leak that happened to pair with `process::exit` in one QA loop (`doc:dev/work-report-error-hiding-audit-2026-07-26.md` §3). B4 and B6 already call it a scar. This memo agrees: delete it after a written decision; do not keep it as the lifecycle law.

---

## 5. Process supervision

The Hub is a **user-spawned singleton that must outlive one CLI**. That is a supervision problem, not an RPC timeout problem.

| Layer | Who owns it | What “healthy” means |
|-------|-------------|----------------------|
| CLI / MCP / `HubClient` process | Operator / editor / test parent | One-shot: RPC returned **and** process exited. Long-lived lib: client `shutdown` without killing the daemon |
| Daemon process | `ensure_daemon` + `daemon.lock` / `daemon.json` | One process per Hub home; handshake then business RPC; idle-exit is daemon policy, not CLI Drop |
| Disk registry | Daemon (`agents.json`, store) | Durable config. Presence ≠ live handle, ≠ RPC success |
| Live handles | Daemon `handles` map | Warm ACP connections. Cancel notify uses **only** these; cold cancel must not `agent_handle` start |
| Agent child | Daemon (southbound Pattern A) | Dies with the session / revoke, not with the CLI |

Industry analogs (do not copy blindly):

- **systemd** `Type=notify` + `RemainAfterExit=no`: the service is the long-lived unit; the invoking CLI is not.
- **Kubernetes** create-vs-start: the API object can exist while the container is not running. Disk registry is create; live handle is start.
- **sccache** (B6): auto-spawn, idle-exit, timeout = failure. Closest CLI→daemon supervisor.
- **Docker Engine as a Windows Service** (B6 Pattern B): correct supervision, wrong product change for `ensure_daemon`.

Import:

1. CLI Drop / `process::exit` must not be the daemon’s supervisor. The daemon is supposed to survive.
2. `ensure_daemon` is allow-once spawn + discover, not a reconnect loop around a failed mutate.
3. Idle-exit vs mid-turn “daemon closed” stays an open plan question (§8.2 / C6). This memo only requires: mid-turn EOF is class-table `Err`, not a silent respawn+replay.

---

## 6. Copy versus unique

### 6.1 Copy (do not reinvent)

| Practice | Source | Hub mapping |
|----------|--------|-------------|
| Request vs notification | JSON-RPC 2.0 §4.1 | Northbound control = request. Southbound cancel = notification. Do not wait for a notify reply that the spec forbids. |
| Cancel is fire-and-forget | ACP `session/cancel`; LSP `$/cancelRequest`; MCP `notifications/cancelled` | Mark + enqueue; don’t join |
| Create vs start | K8s / Docker: object exists ≠ process running. ACP: `session/new` (create) vs `session/prompt` (start work). LSP: `initialize` request vs `initialized` notification | Cold `agent add` = registry create, **no** handle start. Inspect/send = start. Cancel with no live handle = mark without start |
| Ordinary control timeout in the **25–60s** band | gRPC deadlines commonly ~30s; HTTP clients ~30s; LSP `initialize` clients often 30–60s before they give up | Hub **30s** ordinary bound sits in-band. Keep as honest `Err`. Do not restore 15s success-on-timeout. Product waits (`send` join, `wait`) stay outside this band |
| Retry unread only; never silent mutate retry | gRPC A6 transparent retries; client-go / service-config retryable codes | §3 |
| Structured shutdown; no `forget`; no flush-on-Drop | jsonrpsee, rust-analyzer, Tokio, B4 | §4 |

### 6.2 Unique (do not flatten into a language-server story)

| Fact | Why it is not copyable from LSP / ACP SDK / sccache alone |
|------|----------------------------------------------------------|
| **Two stacked protocols** | Northbound Hub JSON-RPC (pipe/UDS, multiplexed ids) **and** southbound ACP stdio. Cancel ack lives on the northbound request; cancel delivery is a southbound notification. One timeout cannot cover both. |
| **Disk registry + live handles** | `agents.json` can contain an id with no `handles` entry (cold add). Live handle can exist after disk commit. Success, cancel notify, and “already registered” each key off a different layer. |
| **User-spawned singleton that must outlive the CLI** | Not an editor-child LS (B6 Pattern A). Not an installed Windows Service (Pattern B). `ensure_daemon` from a one-shot process that then **must exit** while the child **must remain**. |
| **Windows Job breakaway** | This memo still treats `CREATE_BREAKAWAY_FROM_JOB` (+ `DETACHED` / `CREATE_NO_WINDOW`) as **already correct and unique-and-necessary** for that last fact: a Job-aware parent (`Start-Process -Wait`, CI job objects, VS Code / PowerShell hosts) will wait the tree unless the daemon leaves the job. POSIX process groups are not a substitute. **See §7 before copying this sentence into §5.** |

---

## 7. Tension for the synthesizer (do not resolve here)

This memo’s unique-column still says: **Windows Job breakaway is already correct / unique-and-necessary.**

Sibling **windows-daemon-spawn** research argues the opposite default: **handle inheritance** (`bInheritHandles=TRUE`, Rust `Command` default) is the real parent-Wait hang, and **breakaway must not be the default**. That line matches B6’s sccache / agent-browser import (`CreateProcessW`, `bInheritHandles=FALSE`, no `start /B` happy path) and B4 §6.1 (inherited stdout write handle → parent waits for EOF after the CLI PID is gone).

Recorded, not decided:

| Claim | Who says it | Must not do in this file |
|-------|-------------|--------------------------|
| Breakaway is required so a user-spawned singleton outlives a Job-aware CLI parent | this memo §6.2; named-pipe/Tokio B2 (“right family of flags”) | Promote to §5 |
| Inherit-by-default is the hang; breakaway is compensating and must not be the default | windows-daemon-spawn sibling; B6 sccache/agent-browser | Promote to §5 |

Synthesis (§3.D / plan §5) owns the verdict. This file only forbids pretending the two claims agree.

---

## 8. Normative contract (input to synthesis — not a verdict)

The block below is the RPC-lifecycle law this research supports. Plan §0 step 5 still has to accept, revise, or reject it. It does not replace B1–B4 transport rules or B6 topology.

```text
CONTRACT: CLI ↔ daemon RPC lifecycle
====================================

Classes (every method is exactly one):
  MUTATE   unary request. at-most-once. 30s ordinary bound.
           success = daemon Result only. disk presence ≠ success.
           never silent retry. timeout after commit = typed partial-success, not Ok.
  CANCEL   unary request. ack = hub mark (requested) + enqueue bit
           (acp_notify_enqueued). ACP session/cancel is a notification:
           fire-and-forget; do not join agent I/O. keep CancelResult shape.
           spawn_blocking is a smell; don't-join is the rule.
  WAIT     product timeout. Store-poll. reconnect OK. timeout = Err.
  SEND     accept bounded (ordinary). join = product / unbounded.
           NEVER replay after drop / mid-stream EOF.

Reconnect:
  retry only what is proven unread (gRPC A6 cases 2–3).
  Hub pipe/UDS today cannot prove that after a write → treat as unread-unknown = Err.
  never silent mutate retry. never silent send replay.
  wait polls may reconnect.

Shutdown:
  Drop = abort IO, no join, no FlushFileBuffers, no mem::forget.
  optional async shutdown(): abort + join ≤250ms + drop.
  CLI may process::exit after run() so leftover tasks cannot own WaitForExit.
  FORBIDDEN: mem::forget as a success path.
  FORBIDDEN: unbounded FlushFileBuffers on the CLI thread.

Supervision:
  one daemon per Hub home; must outlive the CLI.
  disk registry ≠ live handles ≠ RPC ack.
  CLI is not the daemon supervisor.

Copy:     JSON-RPC request vs notification; create vs start; 25–60s ordinary timeout.
Unique:   two stacked protocols; disk + handles; user-spawned singleton.
Tension:  Job breakaway “necessary” (this memo) vs inherit-is-the-hang /
          breakaway-not-default (windows-daemon-spawn). synthesizer decides.
```

---

## 9. Mapping back to the plan

| Plan slot | This note |
|-----------|-----------|
| §3.B **B5** | **Filled.** Four classes, ack vs notification, retry, EOF/replay, cancel contract. |
| RPC-lifecycle workstream | This file. There is no separate §3 heading with that name; B5 is the industry slot. |
| §3.B B1–B4 | **Do not reopen.** Shutdown numbers here (≤250ms, forbid `forget` / flush) agree with B4; transport API stays in that note. |
| §3.B B6 | **Do not reopen.** Cancel keep and “do not copy ACP SDK as Hub” stand. |
| §3.C / §5 | Not filled. §8 is input, not a verdict. |
| §9 B5 | Done — this file. |

---

## 10. Sources

| Source | What was used |
|--------|----------------|
| [JSON-RPC 2.0](https://www.jsonrpc.org/specification) §4, §4.1, §5 | Request vs notification; notifications are not confirmable; correlation by `id` |
| [gRPC A6 client retries](https://github.com/grpc/proposal/blob/master/A6-client-retries.md) | Transparent retry only if application never saw the RPC; configurable retry is not a mutate license; no built-in idempotent bit |
| gRPC-Go / client-go service config | Same policy on the wire: retryable codes + unread; do not hedge writes |
| [ACP v2 schema / prompt lifecycle](https://agentclientprotocol.com/protocol/v2/schema) | `session/cancel` notification; late updates allowed; stop reason is later, not an ack |
| LSP `$/cancelRequest`; MCP `notifications/cancelled` | Same notification cancel (via B6) |
| ACP `session/new` vs `session/prompt`; LSP `initialize` vs `initialized`; Docker/K8s create vs start | Create ≠ start; ordinary handshake/control in the 25–60s band |
| Tokio `JoinHandle` / `spawn_blocking` docs | Drop detaches; `abort` required; blocking tasks are not abortable |
| Tokio `Runtime` Drop | Waits forever for a reader that does not yield (B4) |
| MSDN `FlushFileBuffers` | Server-end flush waits for client read — forbidden on CLI teardown |
| jsonrpsee / rust-analyzer `lsp-server` / tarpc | Drop = signal; join only after protocol close (B4) |

In-tree: `git:7dcffe0`; `code:crates/hub/src/rpc.rs`; `code:crates/hub/src/hub/client.rs`; `code:crates/hub/src/hub/prompt.rs`; `code:crates/hub/src/hub/types.rs`; `code:crates/hub/src/hub/wait.rs`; `code:crates/cli/src/commands.rs`; `code:crates/cli/src/main.rs`; `doc:dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` §3.A; `doc:dev/work-report-rc6-p0-root-cause-2026-07-26.md` §2; `doc:dev/work-report-error-hiding-audit-2026-07-26.md`; `doc:dev/AUDIT-error-hiding-self-review-2026-07-26.md`; `doc:dev/design.md:172-175` (rollback sentence — do not restore as join rule).
