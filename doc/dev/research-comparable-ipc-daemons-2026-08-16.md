# Research: comparable local IPC daemons (CLI → singleton)

| Field | Value |
|-------|--------|
| **Nature** | Implementer research note (not operator SSOT; not a written decision) |
| **Date** | 2026-08-16 |
| **Status** | Fills plan §3.B **B6** only. Does **not** close B1–B5 (named-pipe API, Job/spawn, UDS, async Drop, JSON-RPC stream). |
| **Code** | **Do not implement** from this file. |
| **Frozen inputs** | `doc/dev/ux-*.md`; `doc/ssot/pillars/*` — unread for rewrite. |
| **Sibling plan** | `doc/dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` |
| **In-tree baseline** | Archaeology §3.A at `HEAD 7dcffe0` (`0.2.1-rc.9`) |

This note imports **peer CLI→daemon** practice. It answers: what topology do local tools actually use, which of those topologies fit a Hub that outlives one CLI process, and which of acp-hub’s current scars (`mem::forget`, inherit-by-default spawn, timeout-as-success) have an industry counterpart.

Citations are public repo / spec names, not a claim that those trees were re-cloned in this pass. In-tree facts stay at `code:` / `doc:` / `git:7dcffe0`.

---

## 0. What this note is for

acp-hub is already a **singleton local daemon**: CLI / MCP / `HubClient` discover `daemon.json`, spawn `acp-hub serve` if needed, then speak JSON-RPC over `\\.\pipe\acp-hub-{id}` or a Unix domain socket (`doc:dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` §3.A.1). The hang family is **not** “we forgot to speak ACP.” It is CLI process exit vs pipe Drop vs Job tree vs a daemon that must survive the caller.

The official ACP SDK does **not** solve that. Copying its stdio-child client would be the wrong architecture for the Hub itself.

---

## 1. acp-hub baseline (HEAD `7dcffe0`)

| Concern | Current mechanism | Pattern (see §2) | Class from §3.A |
|---------|-------------------|------------------|-----------------|
| Product shape | One daemon per Hub home; auto-spawn; idle exit; CLI is one-shot | **C-shaped product**, **D transport** | keep singleton |
| Northbound transport | Windows named pipe / Unix domain socket; newline JSON-RPC | **D** | A2 / A3 |
| Southbound agent | ACP over stdio (warm paths) | **A** (correct *for agents*) | A7 / A10 |
| Windows spawn | `Command::spawn` cascade: `CREATE_BREAKAWAY_FROM_JOB` → `DETACHED_PROCESS` → `cmd start /B` (`daemon.rs:754-832`) | neither **C** nor sccache | still-workaround |
| Handle inherit | Rust `std::process::Command` defaults to inherit (`bInheritHandles=TRUE` on Windows) | same scar as agent-browser | still-workaround |
| Client Drop | `RpcClient` Drop aborts reader/writer, no join (`rpc.rs:374-392`) | blunt **D** / **E** | still-workaround |
| Add-path leak | `mem::forget(client)` after successful `agent add` (`commands.rs:61-72`) | **E** | still-workaround |
| Process exit | CLI `process::exit` after `run()` (`main.rs:24-43`) | **E** belt | still-workaround |
| Cancel | mark-first; `session/cancel` via `spawn_blocking`; do not join agent write (`prompt.rs:295-331`) | matches ACP / LSP / MCP | **keep** |
| Ordinary RPC bound | 30s → typed `Err`, not success (`rpc.rs:52`, `rpc.rs:291`) | honest timeout | **keep** |
| Timeout-as-success | removed in rc.9 | forbidden | **keep absence** |

Topology that must not be unlearned:

```text
CLI / MCP / HubClient  --pipe/UDS JSON-RPC-->  acp-hub daemon  --ACP stdio-->  agent
         one-shot                                      lives on                    Pattern A
```

The Hub **outlives one CLI**. Pattern A is the *agent* side. It is not the Hub side.

---

## 2. Five patterns (A–E)

These are the only topologies that showed up across the survey. A local ACP hub has to pick from C/D (and may keep E only as a temporary belt). A and B are category errors if copied as the Hub.

| ID | Pattern | Who uses it | Fits a Hub that outlives one CLI? |
|----|---------|-------------|-----------------------------------|
| **A** | **stdio child** — parent spawns the peer; JSON-RPC on stdin/stdout; peer dies with the parent | Official ACP SDK; MCP local servers; Copilot LS; rust-analyzer (editor child); Claude Code *as an ACP agent* | **No.** The Hub is the long-lived side. Do not copy ACP SDK as Hub architecture. |
| **B** | **HTTP-over-npipe + Windows Service** — engine is an installed service; clients use `\\.\pipe\…` as an HTTP socket | Docker Engine / Docker Desktop; Podman machine in service mode | **Product change.** Correct for a shipped system service, not for `ensure_daemon` from a CLI. |
| **C** | **auto-spawn daemon + TCP loopback** — CLI starts a detached daemon if missing; RPC on `127.0.0.1`; idle-exit; timeouts are failures | **sccache** (closest analog); **ra-multiplex** | **Yes.** Dominant Rust CLI→daemon pattern that does **not** hang on named-pipe Drop. |
| **D** | **named pipe / AF_UNIX with explicit half-close** — duplex byte stream; sender must flush-or-evade before Drop; Windows dirty send halves go to limbo | **interprocess** docs; **wezterm**; **nushell** | **Yes, if** teardown is specified (`FlushFileBuffers` / `evade_limbo` / `assume_flushed`). acp-hub is here today **without** that API. |
| **E** | **`mem::forget` + `process::exit`** — skip Drop so Windows limbo cannot block the parent Wait | acp-hub rc.9 add path; any code that leaks a dirty pipe to dodge `FlushFileBuffers` | **Workaround scar.** Blunt `evade_limbo`. Not a steady-state rule. |

Rules of thumb:

- **A** is what Hub already does *to* agents. Using A *as* the Hub would make every CLI invocation own the registry, store, and live sessions — the opposite of a singleton daemon.
- **B** solves Job/pipe lifetime by installing a service. That is a product decision, not a local fix.
- **C** is the pattern that repeatedly does **not** hang: RPC is TCP; any named pipe is a one-shot startup notify, not the session.
- **D** is viable on the current pipe/UDS transport **only** with an explicit half-close contract. Without it, Drop is a potential `WaitForExit` hang.
- **E** is what rc.9 did after Drop hung. Industry name for the same move: skip the limbo flush.

---

## 3. Per-project findings

### 3.1 Official ACP SDK — Pattern A (do not copy as Hub)

| Item | Finding |
|------|---------|
| Role | Client library + agent stdio transport. A front-end **spawns** an ACP agent and speaks JSON-RPC on the child’s stdin/stdout. |
| Lifetime | Agent process is a child of the client session. There is no “ACP daemon that outlives the IDE/CLI” in the SDK itself. |
| Cancel | `session/cancel` is a **notification** (no response). The client is not required to wait for the agent to acknowledge. |
| Import | Keep ACP cancel semantics on the *southbound* link. **Do not** make Hub northbound IPC “stdio to a child that is the Hub.” |

acp-hub already *is* an ACP client (southbound). The missing design is the **northbound** singleton, which the SDK does not specify.

### 3.2 Claude Code — Pattern A when used as an ACP agent

| Item | Finding |
|------|---------|
| Role | Operator CLI / ACP agent endpoint. When driven over ACP, it is a stdio child of whatever client spawned it. |
| Lifetime | Bound to that client session. Not a multi-caller local hub. |
| Import | Useful as a *registered agent*, not as a template for `ensure_daemon`. |

### 3.3 GitHub Copilot language server — Pattern A

| Item | Finding |
|------|---------|
| Role | LSP server spawned by the editor. JSON-RPC over stdio (sometimes TCP/named pipe when the editor asks, still a *session child*). |
| Cancel | LSP `$/cancelRequest` is a notification. The server should stop work; the client does **not** block the cancel RPC on request completion. |
| Import | Cancel protocol matches Hub’s store-first + fire-and-forget. Process topology does **not**: Copilot LS is not a singleton that outlives the editor. |

### 3.4 rust-analyzer and ra-multiplex — A (RA) vs C (multiplex)

| Item | Finding |
|------|---------|
| rust-analyzer | Default editor integration is Pattern **A**: one stdio LSP child per workspace window. |
| ra-multiplex | Multiplexes many editor clients onto one rust-analyzer. Clients talk to a **long-lived** multiplexer; the multiplexer auto-spawns RA. Transport is TCP/socket, not “each editor owns RA stdio.” |
| Idle | Shared server can persist across editor restarts; this is the point of the multiplex. |
| Import | ra-multiplex is the closest *language-server* analog to a Hub: **C**, not A. rust-analyzer-the-binary is A and is the wrong import. |

### 3.5 Docker / Podman — Pattern B (product change)

| Item | Finding |
|------|---------|
| Docker Engine (Windows) | Clients use HTTP over `\\.\pipe\docker_engine` (npipe). The engine is a **Windows Service** (or Linux VM + service), not `Command::spawn` from `docker.exe`. |
| Podman | Same class: machine/service, then API socket. |
| Teardown | Service lifetime is SCM / `podman machine`, not CLI Drop. npipe hang is an engine bug, not a client `mem::forget`. |
| Import | **Do not** adopt “become a Windows Service” as the default-practice fix. That is a product change (install, privilege, upgrade, `doctor`). Keep it off the incident path. |

### 3.6 rmcp / mcp-proxy — Pattern A (+ optional HTTP bridge)

| Item | Finding |
|------|---------|
| MCP local servers | Spec default is stdio JSON-RPC. The host owns the child. |
| `notifications/cancelled` | Notification. Host must not treat cancel as “wait until the tool returns.” |
| rmcp / mcp-proxy | Bridges or embeds MCP; HTTP/SSE variants exist, but the *local* shape remains A unless someone runs a proxy daemon. |
| Import | Cancel notification = keep. Stdio-child host = what Hub already is toward agents, not toward operators. |

### 3.7 `interprocess` (Windows named-pipe limbo) — Pattern D law

This is the library acp-hub-class code either uses or accidentally reimplements.

| Item | Finding |
|------|---------|
| Dirty send half | On Windows, dropping a send half that still has unflushed bytes enters **limbo**: `FlushFileBuffers` then close. |
| Hang condition | If the daemon is **not draining** the pipe, `FlushFileBuffers` blocks. `Drop` of the client stream then blocks the CLI (and a parent `WaitForExit`). |
| Intended API | After the reply is **fully read**, call `evade_limbo()` / `assume_flushed()` (names from the crate: skip or assert the flush because the peer already consumed the bytes). Then close. |
| `mem::forget` | A blunt `evade_limbo`: skip `Drop` so limbo never runs. Leaks the handle until `process::exit` / OS reclaim. |
| Import | rc.9 `mem::forget(client)` + Drop-abort is **E** standing in for this API. The industry-named fix is: **shutdown write + `assume_flushed` / `evade_limbo` after the RPC reply is read.** Then `process::exit` may remain as a belt, not as the only close. |

B1 (ConnectNamedPipe / last-handle / `ERROR_PIPE_BUSY`) is **not** closed by this paragraph. This is only the **client send-half Drop** rule that comparable Rust code already documents.

### 3.8 wezterm — Pattern D (AF_UNIX even on Windows)

| Item | Finding |
|------|---------|
| Transport | **AF_UNIX domain sockets on Windows**, not a long-lived named-pipe RPC session. |
| Spawn | `DETACHED_PROCESS` only. **No** `CREATE_BREAKAWAY_FROM_JOB`. |
| Import | If Hub stays on a byte-stream socket, wezterm’s lesson is “pick a transport whose close is EOF, and detach without the Job-breakaway cascade.” It does **not** justify acp-hub’s `start /B` fallback. Job/breakaway remains B2. |

### 3.9 nushell — Pattern D (half-close awareness)

| Item | Finding |
|------|---------|
| Role | Shell + plugin / IPC paths that use named pipe or AF_UNIX and must not block the interactive process on plugin teardown. |
| Import | Same D rule as interprocess: **explicit half-close**, do not Drop a dirty send half into limbo. Not a stdio-plugin-as-Hub lesson. |

### 3.10 sccache — Pattern C (closest analog)

sccache is the project to copy **spawn + RPC transport** from. It is a Rust CLI that auto-starts a local daemon, talks RPC, idle-exits, and treats timeouts as failures.

| Item | sccache | acp-hub HEAD |
|------|---------|--------------|
| RPC transport | **TCP `127.0.0.1`** | named pipe / UDS |
| Named pipe | **One-shot startup notify only** (client waits until the daemon signals “listening”) | **the whole RPC session** |
| Spawn API | `CreateProcessW` | `std::process::Command` (+ flag cascade) |
| `bInheritHandles` | **`FALSE`** | default **TRUE** (inherit) |
| Flags | `CREATE_UNICODE_ENVIRONMENT \| CREATE_NEW_PROCESS_GROUP \| CREATE_NO_WINDOW` | breakaway → `DETACHED_PROCESS` → `start /B` |
| Idle | daemon idle-exit | idle-exit (ActivityTracker; not re-measured in §3.A) |
| Timeout | **failure** | ordinary RPC 30s = `Err` (keep); timeout-as-success **removed** (keep absence) |
| Hang surface | TCP Drop does not `FlushFileBuffers` a named-pipe send half | pipe Drop / limbo / Job Wait |

Why this is the closest analog:

1. Same product shape: CLI is ephemeral; daemon is a singleton; next CLI reconnects.
2. Same failure ethic: a wall-clock is an error, not `registered`.
3. They **avoided** the Windows hang by not putting the RPC session on a named pipe, and by not inheriting handles.
4. The named pipe they *do* use is a **oneshot readiness** channel — closer to “handshake file appeared” than to `RpcClient`’s duplex stream.

Import for Hub (ordered in §5): upgrade **spawn** to this `CreateProcessW` contract first; keep or leave the pipe; **move RPC to TCP** only if D-limbo keeps biting after an explicit half-close.

### 3.11 agent-browser — same hang, same proposed fix

| Item | Finding |
|------|---------|
| Symptom | Parent Wait hangs after the child has conceptually finished. |
| Mechanism | `Command::spawn` with handle inherit **TRUE** (Rust default). Inherited pipe/socket handles keep the parent’s I/O or Job tree alive. |
| Proposed fix (in that project’s discussion) | **sccache-style `CreateProcessW`**: `bInheritHandles=FALSE`, no stray inherited stdio/pipe. |
| Import | acp-hub’s spawn cascade can still inherit. That is an independent hang from limbo-on-Drop. Fixing Drop without no-inherit spawn leaves the agent-browser class of Wait. |

---

## 4. Cancel: store-first + fire-and-forget is the industry contract

| Protocol | Cancel primitive | Client waits for peer to finish? | Timeout-as-success? |
|----------|------------------|----------------------------------|---------------------|
| ACP | `session/cancel` **notification** | No | No |
| LSP | `$/cancelRequest` **notification** | No | No |
| MCP | `notifications/cancelled` | No | No |
| acp-hub rc.8/rc.9 | mark in store, then fire-and-forget `session/cancel`; `CancelResult` = `requested` + `acp_notify_enqueued` | **No** (keep) | **No** (keep absence) |

**Keep.** Do not wait for the agent. Do not put a wall-clock on cancel and print success. Delivery may fail (`acp_notify_enqueued=false`); that is an honest field, not a hang and not a fake `cancelled`.

This matches §3.A “keep mark-first” and does **not** reopen the rc.7 CLI 12s cancel timeout.

---

## 5. Recommended stack for a local ACP hub

Not a written decision (plan §0 step 5 is still closed). This is the **import order** comparable projects support. Critique (§3.C) and synthesis (§5 of the plan) still have to accept or revise it.

| Step | Action | Why (from this survey) | Do not |
|------|--------|------------------------|--------|
| **1** | **Keep** store-first + fire-and-forget `session/cancel` | Identical to ACP / LSP `$/cancelRequest` / MCP `notifications/cancelled` | Wait for the agent; timeout-as-success |
| **2** | **Upgrade spawn** to sccache `CreateProcessW`: `bInheritHandles=FALSE`; `CREATE_UNICODE_ENVIRONMENT \| CREATE_NEW_PROCESS_GROUP \| CREATE_NO_WINDOW` | Dominant Rust CLI→daemon spawn that does not hang; agent-browser’s proposed fix for the inherit Wait | Restore `start /B` as the happy path; copy wezterm’s “DETACHED only” without no-inherit |
| **3** | After each one-shot RPC: **shutdown write** + **`assume_flushed` / `evade_limbo`**, then `process::exit` as a belt; **delete `mem::forget`** | interprocess intended API; `mem::forget` is blunt evade | Leave add-only `mem::forget` as the law |
| **4** | **Stay on named pipe / UDS** while D teardown is specified; **or move RPC to TCP `127.0.0.1`** (sccache / ra-multiplex) if limbo keeps biting. Named pipe as **oneshot startup notify** remains legal either way | C vs D are both valid Hub shapes; C removes the limbo surface | Copy Docker service+npipe (B); copy ACP stdio (A) |
| **5** | **Never restore timeout-as-success** | sccache and rc.9 honesty: timeout is `Err` | Disk presence of `agents.json` as `registered` |

Hard negatives:

- Official ACP SDK is **Pattern A**. Do not copy it as the Hub architecture.
- Docker/Podman **Pattern B** is a product change, not the default-practice patch.
- Pattern **E** may remain as `process::exit` after a specified close. It must not remain as `mem::forget` of a live `RpcClient`.

---

## 6. Mapping back to the plan (B6 only)

| Plan slot | This note |
|-----------|-----------|
| §3.B **B6** | Filled. Peer set + patterns A–E + recommended order. |
| §3.B B1–B5 | **Still open.** Limbo / `CreateProcessW` / AF_UNIX facts above are *peer* evidence, not a Windows API walk or a Job-object verdict. |
| §3.C / §5 | Not filled. Recommended order is input to critique and synthesis. |
| §9 B6 | Done — this file. |

---

## 7. Sources (public)

| Project | What was used |
|---------|----------------|
| Agent Client Protocol / official ACP SDK | stdio transport; `session/cancel` notification |
| Claude Code | ACP-agent-as-stdio-child role |
| GitHub Copilot LS | LSP stdio child; `$/cancelRequest` |
| rust-analyzer | editor-spawned stdio LSP |
| ra-multiplex | shared RA via long-lived multiplexer + TCP/socket |
| Docker Engine / Docker Desktop | HTTP over npipe; Windows Service |
| Podman | machine/service + API socket |
| MCP / rmcp / mcp-proxy | stdio default; `notifications/cancelled` |
| `interprocess` (kotauskas) | Windows send-half limbo; `FlushFileBuffers`; `evade_limbo` / `assume_flushed` |
| wezterm | AF_UNIX on Windows; `DETACHED_PROCESS` only; no `CREATE_BREAKAWAY_FROM_JOB` |
| nushell | named pipe / AF_UNIX half-close practice |
| sccache (mozilla) | TCP `127.0.0.1` RPC; named pipe oneshot notify; `CreateProcessW` `bInheritHandles=FALSE`; `CREATE_UNICODE_ENVIRONMENT \| CREATE_NEW_PROCESS_GROUP \| CREATE_NO_WINDOW`; idle-exit; timeout = failure |
| agent-browser | `Command::spawn` inherit=TRUE hang; proposed sccache `CreateProcessW` |

In-tree: `git:7dcffe0`; `doc:dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` §3.A; `doc:dev/work-report-error-hiding-audit-2026-07-26.md` §3 (`mem::forget` / WaitForExit).
