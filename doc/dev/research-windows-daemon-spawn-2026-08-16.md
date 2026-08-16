# Research: Windows daemon spawn / Job Object

| Field | Value |
|-------|--------|
| **Nature** | Implementer research note (not operator SSOT; not a written decision; not a work-report close-out) |
| **Date** | 2026-08-16 |
| **Status** | Fills plan §3.B **B2** and §3.C **C5** only. Does **not** close B1 (named-pipe API), B3–B6, C1–C4/C6, or §5. |
| **Code** | **Do not implement** from this file. |
| **Frozen inputs** | `doc/dev/ux-*.md`; `doc/ssot/pillars/*` — unread for rewrite. |
| **Sibling plan** | `doc/dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` |
| **In-tree baseline** | Archaeology §3.A at `HEAD 7dcffe0` (`0.2.1-rc.9`); spawn path `crates/hub/src/daemon.rs` `spawn_daemon_windows` (`754-832`) |

This note imports **Windows process-create and Job-Object** constraints for `ensure_daemon` → `acp-hub serve`. It answers: why a parent `WaitForExit` / `WaitForSingleObject` can outlive a CLI that already printed success, which `CreateProcess` flags actually change that, and why the rc.9 cascade (`CREATE_BREAKAWAY_FROM_JOB` → `DETACHED_PROCESS` → `cmd start /B`) is the wrong default.

Named-pipe connect / `DisconnectNamedPipe` / Tokio Drop stay in `doc/dev/research-named-pipe-tokio-2026-08-16.md` (B1/B3/B4). Peer CLI→daemon topology stays in `doc/dev/research-comparable-ipc-daemons-2026-08-16.md` (B6). This file only covers the **spawn boundary**.

---

## 0. Verdict for a later synthesizer

Default practice is **(c)+(d)**: handle hygiene, then `CREATE_NO_WINDOW` **xor** `DETACHED_PROCESS`, plus `CREATE_NEW_PROCESS_GROUP`, plus the existing ready handshake, then the CLI process actually exits.

It is **not** breakaway-from-job. It is **not** a Windows Service for a per-user hub.

`WaitForSingleObject(hProcess)` does **not** hang after a true process exit. A report of “`WaitForExit` after `main` returned” is almost always one of: an inherited pipe write-end (Wait-for-EOF), a still-attached console, or a CLI thread still alive inside `RpcClient` Drop.

acp-hub already has the right **protocol**: spawn `serve`, child binds the pipe, parent polls `daemon.json`. The missing piece is spawn **without leaking handles or a console**.

---

## 1. What the tree does today

`ensure_daemon` (`daemon.rs:335-357`) discovers `daemon.json`, takes `daemon.lock` only long enough to spawn, drops the lock, then `poll_daemon` until metadata + connect succeed (`STARTUP_TIMEOUT` 15s). `serve` (`daemon.rs:265-331`) is the only path that opens `daemon.lock` for the daemon lifetime, binds `\\.\pipe\acp-hub-{id}`, and writes `daemon.json`.

Windows spawn (`daemon.rs:760-832`) is a three-step cascade:

1. `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW`
2. same without `CREATE_BREAKAWAY_FROM_JOB`
3. `cmd /c start /B … serve --home …` with `DETACHED_PROCESS | CREATE_NO_WINDOW`

Every step uses `std::process::Command` with `Stdio::null()` (unless `ACP_HUB_DAEMON_STDERR=inherit`). The comment at `daemon.rs:754-758` states the intended problem: Job-aware parents (`Start-Process -Wait`, `Start-Job`, some terminals/CI) wait the whole job tree.

That comment names a real supervisor behavior. The cascade does not fix the hang class that actually keeps `WaitForExit` alive after the CLI image has finished `main`.

---

## 2. Root-cause taxonomy

`WaitForSingleObject` on a process handle returns when that process object is signaled. MSDN: the process object is signaled when the process terminates. A hang after “the CLI returned” therefore means **the process the waiter holds is not the process that printed**, or **the waiter is not waiting on the process object**.

| Class | What is still alive | Typical waiter | Does Job breakaway fix it? |
|-------|---------------------|----------------|----------------------------|
| **H1 — Wait-for-EOF (inherited write-end)** | Any descendant still holds the parent’s redirected stdout/stderr write-end | .NET `Process.WaitForExit()` with redirection; many CI wrappers; PowerShell when it redirected | **No.** Child stays in or leaves the job; the pipe write-end is still inherited. |
| **H2 — Console attachment** | Daemon (or grandchild) still attached to the parent console | `cmd.exe` / PowerShell window `exit`; some hosts wait the console, not the PID | **Partly.** `CREATE_NO_WINDOW` **or** `DETACHED_PROCESS` (not both — see §4). `FreeConsole` in the child is the git backup. |
| **H3 — CLI thread still in `RpcClient` Drop** | Tokio runtime / pipe flush / join still on the CLI stack after `main` “returned” from the command fn | Parent `WaitForExit` on the CLI PID — the CLI has **not** exited | **No.** This is client teardown (B1/B4 / C1–C2), not spawn. |
| **H4 — Job tree wait** | Daemon is a member of a job with `KILL_ON_JOB_CLOSE` and/or a supervisor that waits the job | `Start-Process -Wait` on a job; test harness `TerminateJobObject` | **Only if** breakaway succeeds. When it succeeds, the daemon **escapes a supervisor that wanted the tree dead** (bad for tests). |
| **H5 — Wrong PID waited** | `cmd /c start /B` returned; waiter holds `cmd.exe` **or** still holds inherited handles to the grandchild | .NET `WaitForExit` + redirection on the `cmd` process | **No.** `start /B` is the degraded form of H1+H4. |

H1 is the textbook hang. Rust `std::process::Command` always sets `bInheritHandles=TRUE` (`inherit_handles: true` at `Command::new`; rust-lang/rust `library/std/src/sys/process/windows.rs`). `Stdio::null()` still opens NUL, sets `STARTF_USESTDHANDLES`, and inherits **every** inheritable handle in the process — including a CI stdout write-end. The child holding that write-end prevents EOF. The parent looks like it is “waiting for the CLI to exit.”

H3 is real in this tree (`rpc.rs:374-392` abort-without-join; `commands.rs:71` `mem::forget` on add; `main.rs:24-43` `process::exit`). It is out of scope for B2 except as a differential diagnosis: if `hProcess` is the CLI and `WaitForSingleObject` has not returned, the CLI has not exited.

---

## 3. Ranked default practice

Letters are the candidate practices. Rank is product-fit for a **per-user, auto-spawned, idle-exiting** Hub — not “what Windows can do.”

| ID | Practice | Rank | Verdict |
|----|----------|------|---------|
| **(a)** | Windows Service (SCM) for the Hub | Reject as default | Correct for Docker Engine / an installed machine service (B6 pattern B). Wrong product shape for `ensure_daemon` from a per-user CLI. |
| **(b)** | Job breakaway (`CREATE_BREAKAWAY_FROM_JOB`) and/or `cmd start /B` as the default escape | Not default | Fails unless the job has `JOB_OBJECT_LIMIT_BREAKAWAY_OK`. `KILL_ON_JOB_CLOSE` jobs typically do **not** allow it. When it succeeds, the daemon escaped a supervisor that wanted the tree dead (tests, `Start-Job`, CI job objects). Treat as **explicit opt-in**, not the spawn default. Delete `start /B`. |
| **(c)** | Handle hygiene: `bInheritHandles=FALSE`, or `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` of exactly the intended NUL stdio handles | **Required** | Closes H1. Without this, every other flag is theater. |
| **(d)** | Console + group + ready: `CREATE_NO_WINDOW` **xor** `DETACHED_PROCESS`, plus `CREATE_NEW_PROCESS_GROUP`, plus `CREATE_UNICODE_ENVIRONMENT`, plus the existing `daemon.json` ready poll, then the CLI process actually exits | **Required** | Closes H2. Ready poll is already in-tree. “CLI actually exits” is H3 (Drop / `process::exit`) — spawn cannot substitute for it. |

**Default = (c)+(d).**

(a) remains a product decision, not a local spawn fix. (b) remains a documented escape hatch for a caller that *owns* the job and sets `BREAKAWAY_OK` on purpose.

---

## 4. `CreateProcess` flags + handle hygiene checklist

### 4.1 Recommended flags (later implementation; not a §5 verdict)

Preferred (no stdio inheritance at all):

| Item | Value |
|------|--------|
| `bInheritHandles` | `FALSE` |
| `dwCreationFlags` | `CREATE_NO_WINDOW \| CREATE_NEW_PROCESS_GROUP \| CREATE_UNICODE_ENVIRONMENT` |
| `STARTF_USESTDHANDLES` | **off** |
| `CREATE_BREAKAWAY_FROM_JOB` | **off** (opt-in only) |
| `DETACHED_PROCESS` | **off** on this path (`CREATE_NO_WINDOW` already set) |

If NUL stdio is required (child code assumes valid stdin/stdout/stderr):

| Item | Value |
|------|--------|
| `bInheritHandles` | `TRUE` (HANDLE_LIST requires it) |
| Attribute | `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` of **exactly** those NUL handles |
| `STARTF_USESTDHANDLES` | on, pointing at those NULs only |
| Same creation flags | `CREATE_NO_WINDOW \| CREATE_NEW_PROCESS_GROUP \| CREATE_UNICODE_ENVIRONMENT` |

Do not pass the named-pipe server handle. Do not pass `daemon.lock`. Do not pass the parent’s redirected stdout/stderr.

### 4.2 Flag hygiene (MSDN)

| Flag / field | Rule | In-tree today |
|--------------|------|---------------|
| `CREATE_NO_WINDOW` + `DETACHED_PROCESS` | MSDN: `CREATE_NO_WINDOW` is **ignored** if used with `DETACHED_PROCESS` (or `CREATE_NEW_CONSOLE`). | ORed on **every** path (`daemon.rs:786-787`, `799`, `822`). The `CREATE_NO_WINDOW` bit is a no-op. |
| `DETACHED_PROCESS` | Child does not inherit the parent console. Cannot combine with `CREATE_NEW_CONSOLE`. | Used as the “silent” fallback. Redundant with `CREATE_NO_WINDOW` if the latter is allowed to work. |
| `CREATE_NEW_PROCESS_GROUP` | Child is root of a new Ctrl+Break group; Ctrl+C disabled in that group. Ignored if combined with `CREATE_NEW_CONSOLE`. | Already set on paths 1–2. Keep. |
| `CREATE_BREAKAWAY_FROM_JOB` | No effect if the caller is not in a job. If the caller **is** in a job, the job **must** set `JOB_OBJECT_LIMIT_BREAKAWAY_OK` or `CreateProcess` fails. | Path 1. `KILL_ON_JOB_CLOSE` jobs typically omit `BREAKAWAY_OK`, so this fails closed and falls through. |
| `CREATE_UNICODE_ENVIRONMENT` | Required when `lpEnvironment` is Unicode. Rust std sets this when it builds a Unicode env block. | Not set explicitly in `spawn_daemon_windows`; std may add it. Set it explicitly on a raw `CreateProcess` wrapper. |
| `bInheritHandles` | `FALSE` = inherit nothing (unless HANDLE_LIST, which needs `TRUE` and an explicit list). | Rust std **always** `TRUE`. Not configurable on stable (`inherit_handles` is unstable — rust#146407). |
| `STARTF_USESTDHANDLES` | Only if the three `hStd*` handles are the ones you intend to inherit. | `Stdio::null()` makes rust std set this **and** still inherit every other inheritable handle. |
| `start /B` | `cmd` returns immediately; grandchild still shares the job and inheritable handles. | Path 3. Delete. |

### 4.3 Handle checklist (spawn parent)

Before `CreateProcess`:

1. **Do not inherit `daemon.lock`.** `ensure_daemon` holds the write lock across `spawn_daemon` (`daemon.rs:342-351`). An inheritable lock handle in the child keeps the singleton lock alive after the parent drops `guard`.
2. **Do not create or pass the named-pipe server handle in the CLI.** `bind_listener` belongs in `serve` only (`daemon.rs:296-299`, `564-608`).
3. **Do not inherit CI / harness stdout or stderr write-ends.** `Stdio::null()` does not prevent this under rust std.
4. **`ACP_HUB_DAEMON_STDERR=inherit` reopens H1.** It puts the parent stderr (often a redirected pipe) into the child. Keep it as an explicit debug hatch; do not let it be the default, and do not combine it with a HANDLE_LIST that includes that pipe if the parent Wait is pipe-EOF.
5. Close the parent’s copies of any NUL handles after spawn. Close `pi.hThread`. Do not wait `pi.hProcess` for daemon lifetime; the ready signal is `daemon.json` + connect.

### 4.4 Rust std constraint

| Fact | Source |
|------|--------|
| `Command` defaults `inherit_handles: true` | rust-lang/rust `library/std/src/sys/process/windows.rs` (`Command::new`) |
| `CreateProcessW(..., inherit_handles, ...)` | same file, `CreateProcessW` call |
| `Stdio::null()` / any set stdio → `STARTF_USESTDHANDLES` | same file, `is_set` guard |
| `CommandExt::inherit_handles` still **unstable** | rust-lang/rust#146407 (`windows_process_extensions_inherit_handles`) |
| HANDLE_LIST is **not** the std default | rust-lang/rust#73281 (closed as duplicate of #38227); std still inherits all inheritable handles when `bInheritHandles` is true and no attribute list is supplied |

A later implementation cannot express (c) with stable `std::process::Command` alone. It needs a thin `CreateProcessW` wrapper (or an equivalent `windows-sys` helper). That is an implementation note, not a reason to keep breakaway.

---

## 5. Named-pipe interaction (spawn boundary only)

This section does **not** replace B1 (`ConnectNamedPipe` / last-handle-close / `ERROR_PIPE_BUSY`). It only states what spawn must not disturb.

| Rule | Why |
|------|-----|
| Named-pipe **server** lives only in `serve` | `bind_listener` after `daemon.lock` (`daemon.rs:276-299`). CLI is a client of `\\.\pipe\acp-hub-{id}` via `daemon.json`. |
| CLI must not `CreateNamedPipe` or pass a server handle into the child | A leaked server handle in the CLI (or an inherited one in a grandchild) keeps the pipe object alive after `serve` exits; next boot sees a busy / access-denied name. That is the leftover-pipe hazard in A2, from the spawn side. |
| Ready signal is **bind + `daemon.json`**, not “child process created” | `write_metadata` happens after bind (`daemon.rs:319-325`). `poll_daemon` already waits for that. Do not treat `Command::spawn` Ok as ready. |
| Do not inherit `daemon.lock` | See §4.3. The parent must drop `guard` after spawn (`daemon.rs:350`) **and** the child must not hold a duplicate handle. |
| Client connect is a **new** `CreateFile` / `LocalSocketStream::connect` | Handshake (`hub/daemon/handshake`, protocol v2) stays a byte-stream RPC. Spawn must not pre-create the client pipe. |

The protocol is already correct. Handle leakage is what couples the pipe object and the parent Wait to the wrong lifetime.

---

## 6. Critique of the current cascade (C5)

Path: `CREATE_BREAKAWAY_FROM_JOB` → `DETACHED` → `start /B` (`daemon.rs:754-832`). Archaeology classed this **still-workaround** (plan §3.A.3 / A8). Industry class:

| Step | What it was meant to do | What it actually does |
|------|-------------------------|------------------------|
| 1. `CREATE_BREAKAWAY_FROM_JOB` + `DETACHED_PROCESS` + `CREATE_NO_WINDOW` | Leave the parent Job so `Start-Process -Wait` does not wait the daemon | `CreateProcess` **fails** unless the job has `BREAKAWAY_OK`. `KILL_ON_JOB_CLOSE` test/CI jobs usually do not. On success, the daemon outlives `TerminateJobObject` — the opposite of what a harness that used a job wanted. `CREATE_NO_WINDOW` is ignored because `DETACHED_PROCESS` is also set. H1 is untouched: rust std still inherits every inheritable handle. |
| 2. `DETACHED_PROCESS` + `CREATE_NO_WINDOW` (no breakaway) | “Silent breakaway” if the job has `SILENT_BREAKAWAY_OK` | Same `CREATE_NO_WINDOW` no-op. Still in the job if the job did not allow silent breakaway. Still H1. |
| 3. `cmd /c start /B` | Return immediately so the CLI is not the daemon’s parent | `start /B` does **not** create a new job and does **not** strip inheritable handles. The grandchild is still in the parent job. .NET `WaitForExit` **with redirection** still waits the grandchild (dotnet/runtime#103384 — Wait-for-EOF on inherited stdio, closed as duplicate of #51277). The CLI’s own `Command::status()` on `cmd` may return, but a QA parent that redirected the CLI still hangs. |

§3.A.6’s archaeology-derived line — “Daemon must escape the parent Job when the OS permits; fallback (`start /B`) must be observable” — is incident law from rc.9, not an industry default. B2 revises it: **do not escape the job by default; do not keep `start /B`.**

`ACP_HUB_DAEMON_STDERR=inherit` (`daemon.rs:714`, `776-780`) is a debug hatch that reopens H1. `cli_contract.rs` already sets it in some tests. A later spawn fix that only flips flags and leaves this inherit path on will recreate the hang under those tests and under any operator who exported the variable.

Breakaway as **explicit opt-in** (env or API) is compatible with a supervisor that set `BREAKAWAY_OK` on purpose. It is not the default for `ensure_daemon`.

---

## 7. Industry citations

### 7.1 MSDN / Microsoft

| Claim | Citation |
|-------|----------|
| `CREATE_NO_WINDOW` is ignored with `DETACHED_PROCESS` or `CREATE_NEW_CONSOLE` | [Process Creation Flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags) — `CREATE_NO_WINDOW` (0x08000000) |
| `CREATE_BREAKAWAY_FROM_JOB` requires `JOB_OBJECT_LIMIT_BREAKAWAY_OK` when the caller is in a job | same page — `CREATE_BREAKAWAY_FROM_JOB` (0x01000000) |
| `DETACHED_PROCESS` = do not inherit the parent console; incompatible with `CREATE_NEW_CONSOLE` | same page |
| `CREATE_NEW_PROCESS_GROUP` = new Ctrl+Break group | same page |
| `bInheritHandles` / `CreateProcessW` | [CreateProcessW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw) |
| `WaitForSingleObject` on a process handle returns when the process terminates | [WaitForSingleObject](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitforsingleobject) |
| `FreeConsole` detaches the calling process from its console | [FreeConsole](https://learn.microsoft.com/en-us/windows/console/freeconsole) |
| HANDLE_LIST lets the parent name exactly which handles are inherited | Raymond Chen, [“Controlling which handles are inherited…”](https://devblogs.microsoft.com/oldnewthing/20111216-00/?p=8873) (2011-12-16) |

### 7.2 nginx

`ngx_execute` in `src/os/win32/ngx_process.c`:

```c
CreateProcess(ctx->path, ctx->args,
              NULL, NULL, 0, CREATE_NO_WINDOW, NULL, NULL, &si, &pi)
```

`bInheritHandles` is `0` (`FALSE`). The only creation flag is `CREATE_NO_WINDOW`. No `DETACHED_PROCESS`, no `CREATE_BREAKAWAY_FROM_JOB`, no `STARTF_USESTDHANDLES`. Ready is a named event + `WaitForMultipleObjects` on `{master_event, child}` with a 5s bound — not “child handle stays open forever.”

Source: [nginx/nginx `src/os/win32/ngx_process.c`](https://github.com/nginx/nginx/blob/master/src/os/win32/ngx_process.c) (`ngx_execute`).

### 7.3 git

Two layers, both closer to (c)+(d) than to (b):

1. **HANDLE_LIST of stdin/stdout/stderr only** — `compat/mingw.c` `mingw_spawnve_fd` (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, `CREATE_UNICODE_ENVIRONMENT`). Commit [9a780a3](https://github.com/git/git/commit/9a780a384de21a35866a380247b34442b5ca3bb8) (“spawned processes need to inherit only standard handles”). Motive: a child that inherits a random file handle holds that file open on Windows.
2. **Ready callback + `FreeConsole`** — `git fsmonitor--daemon start` uses `start_bg_command` + a ready callback; `run` optionally calls `FreeConsole()` so `cmd` / PowerShell `exit` does not wait the inherited console. Commit [c284e27](https://github.com/git/git/commit/c284e27ba77ee385d322bb90aeb2284bf52c014b).

git does **not** default to `CREATE_BREAKAWAY_FROM_JOB` for the daemon.

### 7.4 postgres

`pg_ctl start` waits on **`postmaster.pid` contents** (`wait_for_postmaster_start` in `src/bin/pg_ctl/pg_ctl.c`), not on “`CreateProcess` returned.” The Windows **service** path is a different product: SCM checkpoints (`do_checkpoint` / `SetServiceStatus`) so the service control manager does not kill a slow start. That split is the same as (d) vs (a): pid-file / metadata handshake for a user-started daemon; SCM only if you *are* a service.

Windows historically launched via `cmd.exe`, which made the waited PID a shell ancestor and weakened the pid-file check — the same class of mistake as `start /B`. Hackers thread: [pg_ctl start may return 0 even if the postmaster has already started on Windows](https://www.postgresql.org/message-id/flat/20240606.172146.1104070889615683903.horikyota.ntt%40gmail.com).

### 7.5 rust std

| Item | Citation |
|------|----------|
| `bInheritHandles` always true unless unstable `inherit_handles(false)` | rust-lang/rust#146407; `library/std/src/sys/process/windows.rs` `inherit_handles: true` |
| HANDLE_LIST not the default | rust-lang/rust#73281 (dup of #38227). PR #75551 added a way to *limit* handles; it is not what `Command::spawn` does for ordinary `Stdio::null()`. |

### 7.6 .NET

dotnet/runtime#103384: `cmd.exe` + `start` + **redirected** stdout/stderr → `WaitForExit()` waits the grandchild. Triage (adamsitnik): child starts with redirected pipes; grandchild inherits those pipe handles; EOF is not signaled until **all** write-ends close. Closed as duplicate of #51277. This is H1 with `start /B` as the extra hop — the exact fallback acp-hub uses.

---

## 8. What acp-hub already has vs what spawn still leaks

Already present (keep; do not redesign in a spawn patch):

| Piece | Where |
|-------|--------|
| Singleton `daemon.lock` | `serve` holds it; `ensure_daemon` holds it only to spawn |
| Discover via `daemon.json` `{ pid, endpoint, daemon_id, started_at }` | `write_metadata` / `poll_daemon` |
| Child binds `\\.\pipe\acp-hub-{id}` (SDDL owner/system/admin) | `daemon_endpoint` + `bind_listener` |
| Ready = metadata + connect, 15s poll | `STARTUP_TIMEOUT` |
| Handshake before business RPC | `hub/daemon/handshake`, `DAEMON_RPC_PROTOCOL_VERSION = 2` |
| Idle exit | `ActivityTracker` + `ACP_HUB_IDLE_TIMEOUT` (default 1800s) |

Missing at the spawn boundary:

| Hole | Effect |
|------|--------|
| rust std `bInheritHandles=TRUE` + `Stdio::null()` | H1: CI / QA `WaitForExit` on redirected CLI |
| `CREATE_NO_WINDOW \| DETACHED_PROCESS` on every path | `CREATE_NO_WINDOW` ignored (H2 half-done) |
| Default `CREATE_BREAKAWAY_FROM_JOB` | Fails on `KILL_ON_JOB_CLOSE` jobs; succeeds by escaping the test supervisor (H4 misfire) |
| `start /B` fallback | Still shares job + inheritable handles; .NET redirected Wait still joins grandchild |
| `ACP_HUB_DAEMON_STDERR=inherit` | Reopens H1 on demand |

---

## 9. Mapping back to the living plan

| Plan slot | This note |
|-----------|-----------|
| B2 | §2–§4, §7 |
| C5 | §6 |
| A8 / §3.A.6 “escape the Job / start /B” | Archaeology left as-is. Industry revision: default is (c)+(d), not (b). |
| B1 named-pipe API | Not filled. Spawn-boundary rules only: §5. |
| §5 Windows-only notes | Not filled. Synthesizer may copy §0 + §3 after a written decision. |
| Open question 5 (breakaway vs unspecified Drop) | Both: breakaway/DETACHED were compensating for H1+H2+H3, not a substitute for specified Drop. Drop remains C1/C2. Spawn default does not need breakaway. |

---

## 10. Discipline

- No source changes from this note.
- No edits to `doc/dev/ux-*.md` or `doc/ssot/pillars/*`.
- No `docs/dev/` tree.
- Not a written decision and not a §5 default practice.

---

**End of note.**
