# Research: named-pipe / Tokio client lifecycle

| Field | Value |
|-------|--------|
| **Nature** | Implementer research note (not operator SSOT; not a written decision; not a work-report close-out) |
| **Date** | 2026-08-16 |
| **Status** | Findings recorded. **Do not implement** from this file. Synthesis and §5 of the living plan remain open. |
| **Scope** | Windows named-pipe + Tokio / async-Rust client Drop, IO-task teardown, and “CLI printed success but parent `WaitForExit` still waits” |
| **In-tree HEAD** | `7dcffe0` (`0.2.1-rc.9`) |
| **Living plan** | `doc/dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` §3.B (B1–B4) |
| **Frozen** | Do not edit `doc/dev/ux-*.md` or `doc/ssot/pillars/*` |

This note imports OS and peer-project constraints. It does not rewrite archaeology (§3.A) and does not decide the product default practice.

---

## 0. Verdict for a later synthesizer

Industry default is **structured shutdown + non-blocking Drop**, not `mem::forget`.

1. **Drop = signal only.** Abort the reader/writer (or send a oneshot). Never join, never `FlushFileBuffers`, never `block_on` on the caller thread.
2. **`async fn shutdown(self)` = structured close** after the last RPC, with a short join timeout if you keep `JoinHandle`s at all.
3. **CLI process lifetime = keep `process::exit`** (or `Runtime::shutdown_timeout` if you cannot exit). A leftover reader must not own `WaitForExit`.
4. **Delete `mem::forget(client)`.** It is redundant with CLI `process::exit`, skips abort, and leaves a live reader if anyone later removes `process::exit`.

Current `RpcClient` Drop (abort, no join) is already the mature half. `mem::forget` after successful `agent add` is the immature half.

Parent `WaitForExit` is often **inherited stdio / Job membership / Wait-for-EOF**, not pipe `Drop`. acp-hub already nulls stdin/stdout and uses `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW`. `ACP_HUB_DAEMON_STDERR=inherit` reintroduces a Wait-for-EOF hang under a redirecting parent.

---

## 1. Industry lifecycle (connect → use → shutdown → drop)

There is no special “named-pipe RPC” protocol. Windows named pipes are a byte/message stream. Mature projects treat them like TCP / Unix domain sockets and put lifecycle in **the RPC client and the process**, not in `CloseHandle` folklore.

### 1.1 Connect

Microsoft’s client sequence: `CreateFile(OPEN_EXISTING)` → retry `ERROR_PIPE_BUSY` with `WaitNamedPipe` → `WriteFile` / `ReadFile` → `CloseHandle`.

Tokio `ClientOptions::open` is that `CreateFile`. Documented retry:

| Win32 / Tokio result | Meaning |
|----------------------|---------|
| `NotFound` | Server not up |
| `ERROR_PIPE_BUSY` | Instance exists but nobody is listening; sleep and retry |

Server side: keep **one idle instance always created** before handing the connected one to a task, then `connect().await`. After a client goes away, `disconnect()` (`DisconnectNamedPipe`) before reuse. Tokio tests that **dropping the client is enough** to break the server instance (`ERROR_NO_DATA`).

acp-hub already does the right connect path: `interprocess` `LocalSocketStream::connect` → split → spawn reader/writer (`crates/hub/src/rpc.rs`).

### 1.2 Use

One background **read loop** + one **write loop**, multiplexed by request id. That is jsonrpsee / tarpc / lsp-server, not a named-pipe invention. The frontend holds a channel plus a “I was dropped” signal; IO lives in spawned tasks.

### 1.3 Shutdown (the part everyone gets right)

Order that shows up again and again:

1. **Stop sending** (close the outbound channel, or send a close message).
2. **Cancel the reader** (`AbortHandle` or `CancellationToken`). A daemon will **not** close the pipe just because your last RPC finished.
3. **Do not join on the caller thread.** Await join only inside `async fn shutdown(self)` with a **timeout**.
4. **Then drop the client.** Drop only signals; it does not wait.
5. **Then let the process exit** (CLI: `process::exit` or `Runtime::shutdown_timeout`).

What those projects do **not** do: `mem::forget` the client after success.

`cargo`, `ripgrep`, `deno`, `tokio-console`, `tonic` mostly do not own a Windows named-pipe RPC client (stdio, TCP, or HTTP/2). Their reusable lesson is the same: **protocol/task shutdown, then process exit** — not leaking the transport.

### 1.4 Peer Drop / shutdown contracts

| Project | Contract |
|---------|----------|
| **jsonrpsee `Client`** | `Drop` sends a `oneshot`; **never joins**. `on_disconnect()` is async. |
| **jsonrpsee `Subscription`** | `Drop` is `try_send` only (“may fail if buffer full”). |
| **rust-analyzer / `lsp-server`** | Protocol `shutdown` + `exit`, **then** `IoThreads::join()`. Join is after the protocol close, not inside `Drop`. |
| **tarpc** | Dropping a request future sends `ClientMessage::Cancel`. Server work is `Abortable`, not Drop-join. |
| **Tokio `TcpStream`** | Graceful close is **async** `shutdown()` (write half). Drop closes both. `SO_LINGER` is deprecated because it **blocks the thread on drop**. |
| **Tokio `Runtime`** | `Drop` waits **forever** for work that does not yield. Escape hatches: `shutdown_timeout` / `shutdown_background`. |
| **tokio-util** | `CancellationToken::cancel()` is the structured “please stop”. |
| **Microsoft pipe server** | Optional `FlushFileBuffers` **then** `DisconnectNamedPipe`. Flush **blocks until the client has read everything**. |

---

## 2. Why naive Drop hangs (three mechanisms)

The pipe handle is usually only the last one. Mechanism A is the acp-hub shape and is **not Windows-specific**.

### A. Leftover reader parked on a live daemon pipe → runtime Drop waits forever

`RpcClient` spawns `reader_loop` on a long-lived daemon pipe (`crates/hub/src/rpc.rs`). After `register_agent` returns, the daemon **keeps the instance open**. The reader stays parked in `read_bounded_line`.

Tokio’s contract (`Runtime` shutdown docs):

> Tasks spawned through `Runtime::spawn` keep running until they yield. Then they are dropped.
> The thread initiating the shutdown blocks until all spawned work has been stopped. This can take an indefinite amount of time. The `Drop` implementation waits forever for this.

Sequence:

```text
last RPC Ok → main returns → #[tokio::main] drops Runtime
  → runtime waits for reader
  → reader is in overlapped ReadFile / UDS read
  → process never exits
  → parent WaitForExit / Start-Process -Wait hangs
```

Same hang on **Unix domain sockets** if the daemon stays up. This is not “named pipes are cursed.” It is “a spawned task that does not yield until the peer closes.”

`AbortHandle::abort()` makes the task yield at the next poll point; then the runtime can finish. jsonrpsee’s oneshot does the same job.

### B. `CancelIoEx` is async; waiting for it in Drop is what blocks

Win32: `CancelIoEx` only **requests** cancel. You must not free `OVERLAPPED` / buffers until the operation **completes**. Completion is another IOCP event (or `GetOverlappedResult(..., TRUE)`, which **blocks**).

mio’s `NamedPipe::drop` is explicit: cancel pending reads/connects, do **not** cancel writes (so data can flush), call `CancelIoEx`, and **do not wait**.

The known mio bug is the **opposite** of a hang: if IOCP is not reaped, `Arc<Inner>` never hits 0 and **`CloseHandle` never runs** (handle leak). Waiting robustly would make `Poll::drop` block (`tokio-rs/mio#1944`).

Tokio’s `NamedPipeClient::poll_shutdown` is a no-op flush (`Poll::Ready(Ok(()))`). There is no graceful FIN. Closing **is** `CloseHandle` when the last owner drops.

**Tokio/mio named-pipe Drop is designed not to block.** A block almost always means joining a task, `block_on`ing shutdown, or calling `FlushFileBuffers` — not a Tokio pipe destructor that waits.

`CancelIoEx` can also hang if the same thread mixes **synchronous** pipe I/O with overlapped I/O (kernel APC cannot be delivered). Do not put `std::fs::File` and Tokio on the same pipe.

### C. `FlushFileBuffers` on the server end blocks until the client reads

MSDN (`FlushFileBuffers`): if `hFile` is a handle to the **server end** of a named pipe, the function does not return until the client has read all buffered data.

`DisconnectNamedPipe` docs tell servers to flush first if they do not want to discard unread data — and that flush waits on the client.

This is the named-pipe analog of Tokio deprecating `SO_LINGER` (blocks the thread on drop). Do **not** call `FlushFileBuffers` on the CLI teardown path.

### What the rc.9 “fix” actually did

| Piece | Location | Assessment |
|-------|----------|------------|
| `RpcClient` Drop: mark closed, abort reader/writer, **no join** | `crates/hub/src/rpc.rs` | **Already mature** (jsonrpsee-shaped) |
| `std::mem::forget(client)` after successful `agent add` | `crates/cli/src/commands.rs` | **Redundant and harmful** |
| `std::process::exit` after `run()` | `crates/cli/src/main.rs` | **Keep** (CLI policy) |

`forget` **skips Drop**, so abort never runs. Exit still works only because `process::exit` skips the Tokio runtime destructor entirely. The working combo is **`process::exit`**, not `forget`. If someone later removes `process::exit`, the forgotten reader lives until the OS reaps the process — and a library/`HubClient` caller has no such exit.

---

## 3. Ranked alternatives to `mem::forget`

| Rank | Practice | What it does | Tradeoff |
|------|----------|--------------|----------|
| **1 (default)** | **`async fn shutdown(self)` + abort-only `Drop`** | Close outbound → abort reader → optional `timeout(join)` → drop | Matches jsonrpsee / lsp-server / tarpc. Drop never blocks. CLI can still `process::exit`. |
| 2 | **CLI `process::exit` after `run()`** (already present) | Skip runtime Drop; OS reaps handles | Correct for a short-lived CLI. Not a substitute for a correct `RpcClient` used inside the daemon, MCP, tests, or lib callers. |
| 3 | **`Runtime::shutdown_timeout(Duration)`** | Unblock even if a task is stuck; leak the stuck work | Documented Tokio escape hatch. Use if you cannot `process::exit`. |
| 4 | **One-shot RPC, no background reader** | Write, read response, `CloseHandle`, exit | Best for a single request. Does not fit notification subscriptions. |
| 5 | **`AbortHandle` / `CancellationToken` only** (current Drop, no `forget`) | Tasks yield; runtime can shut down | Enough if you never join in Drop. Still add `shutdown()` so callers are explicit. |
| 6 | Server `disconnect()` after client gone | Forces client reads to fail | Daemon-side reuse. Do **not** `FlushFileBuffers` on the CLI path. |
| **Avoid** | **`mem::forget(client)`** | Leak; skip abort | Hides the leftover reader. Wrong if `process::exit` is removed. |
| **Avoid** | Join IO tasks in `Drop` / `block_on` in `Drop` | “Looks thorough” | Deadlocks with IOCP / runtime (Tokio #6463, #2119). |
| **Avoid** | `FlushFileBuffers` in client teardown | Wait for peer to read | Can block forever if the daemon is not reading. |

**Recommend as default:** rank 1, with rank 2 kept for the CLI binary only.

`forget` is what you do when a destructor is unsound or you are transferring ownership to the OS (rare). It is not a client lifecycle.

---

## 4. Recommended default (not yet a written decision)

For later §5 / §0 step 5. This note does not accept or reject the living plan.

| Surface | Rule |
|---------|------|
| `RpcClient` Drop | Abort-only. Never join. Never flush. Never `forget`. |
| `RpcClient` close | Add `async fn shutdown(self)`: mark closed, abort, optional short `timeout` join. |
| CLI `main` | **Keep** `process::exit` after `run()`. rustc-style: leftover tasks must not own process lifetime. |
| CLI `agent add` | **Delete** `mem::forget(client)`. Call `shutdown().await` on the success path (and on other one-shot commands for symmetry). |
| Lib / MCP / tests | Same Drop + `shutdown`. They do **not** share CLI `process::exit`. |
| Daemon spawn | Keep null stdin/stdout + `DETACHED \| BREAKAWAY \| NO_WINDOW`. Treat `ACP_HUB_DAEMON_STDERR=inherit` as a hang-reintroducing debug switch. |

Windows and Unix share this contract. Only the transport primitive differs. Mechanism A is identical on UDS.

---

## 5. Concrete Rust shape (recommendation only)

No repo edits from this file. Shape for a later implementer after a written decision.

```rust
pub struct RpcClient {
    inner: Arc<RpcClientInner>,
    reader: AbortHandle,          // from JoinHandle::abort_handle()
    writer: AbortHandle,
    // Keep JoinHandles only if you want timeout-join in shutdown().
    reader_join: Option<JoinHandle<()>>,
    writer_join: Option<JoinHandle<()>>,
}

impl RpcClient {
    pub async fn shutdown(mut self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        // Closing the channel lets writer_loop finish after the last flush.
        self.reader.abort();
        self.writer.abort();

        let joins = async {
            if let Some(h) = self.reader_join.take() { let _ = h.await; }
            if let Some(h) = self.writer_join.take() { let _ = h.await; }
        };
        let _ = tokio::time::timeout(Duration::from_millis(200), joins).await;
        // Drop runs next: abort is idempotent.
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.reader.abort();
        self.writer.abort();
        // NEVER: handle.join(), rt.block_on(...), FlushFileBuffers, mem::forget
    }
}
```

CLI command path (success):

```rust
let client = connect(home).await?;
client.register_agent(id.clone(), config).await?;
println!("registered agent {id}");
client.shutdown().await;   // not mem::forget
Ok(())
```

Keep `std::process::exit(code)` in `main`.

APIs to use, not invent:

- `tokio::task::AbortHandle` / `JoinHandle::abort`
- `tokio_util::sync::CancellationToken` if many tasks must stop together
- `tokio::time::timeout` around any join
- `Runtime::shutdown_timeout` if you ever drop a runtime without `process::exit`
- Server: `NamedPipeServer::disconnect()` after the handler ends; **never** `FlushFileBuffers` unless you have a proven “client still reading” invariant
- `tokio::process::Child::wait` **closes stdin first** to avoid the classic deadlock

Do **not** wrap the named pipe in `std::fs::File` / blocking `Read`/`Write`. Overlapped handles + std I/O abort the process (`I/O error: operation failed to complete synchronously`). Tokio moved child stdio off async named pipes onto the blocking pool for this reason.

`ManuallyDrop` + leak of the `Arc` is unnecessary once Drop only aborts. jsonrpsee holds a normal `oneshot::Sender` and sends on Drop.

---

## 6. Pitfalls that look like a pipe hang

These are more common on Windows than a blocking `CloseHandle`.

### 6.1 Parent `WaitForExit` is waiting for stdout EOF, not the PID

.NET `Process.WaitForExit()` with redirected stdout waits until the pipe hits EOF. A **grandchild** that inherited the write handle keeps EOF from arriving after the child has already exited.

PowerShell `Start-Process -Wait` is a process wait; .NET-style host wrappers often are not. Diagnose with: did the CLI PID actually die (`Get-Process`), or is the parent blocked on a pipe read?

Raymond Chen: accidental inheritance when two `CreateProcess` calls race with `bInheritHandles=TRUE` and no `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`.

### 6.2 Daemon inherited the CLI’s stdout/stderr

If `spawn_daemon` does not force `Stdio::null()` (or an explicit handle list), the daemon holds the parent’s redirected pipes. acp-hub already nulls stdin/stdout and uses `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW` (`crates/hub/src/daemon.rs`). That is the right family of flags.

`ACP_HUB_DAEMON_STDERR=inherit` will reintroduce a Wait-for-EOF hang under a redirecting parent.

### 6.3 Job object

Parent waits on a job; daemon was created inside that job. `CREATE_BREAKAWAY_FROM_JOB` is the standard escape. POSIX process groups are not Windows jobs (`command-group` / `process-wrap` exist because of that mismatch). Archaeology already maps the in-tree cascade (`daemon.rs` breakaway → DETACHED → `cmd start /B`).

### 6.4 Classic stdio deadlock (not the Hub pipe)

Parent reads stdout to completion, then waits; child is blocked writing stderr because the buffer is full. Or parent waits while child waits for stdin EOF. Tokio documents closing stdin before `wait()`. Drain stdout and stderr concurrently.

### 6.5 Runtime Drop + `block_in_place` / `block_on`

Workers parked, nobody left to drive IOCP (Tokio #6463).

### 6.6 Unidirectional pipe + `current_thread` spin

Tokio #5170: `access_inbound(false)` made `connect` busy-loop at 100% CPU. Looks like a hang.

### 6.7 Mixing overlapped and synchronous I/O on the same handle/thread

`CancelIoEx` hang (documented in Go). Don’t put `std::fs::File` and Tokio on the same pipe.

### 6.8 Server write after client already closed

`ERROR_NO_DATA` (“The pipe is being closed”). Tokio’s own drop test. Not a hang.

### 6.9 `#[tokio::main]` waiting for detached `JoinHandle`s

Dropping a `JoinHandle` **detaches** the task (it keeps running). Only `abort()` or peer EOF stops a reader. `forget(client)` detaches **and** skips abort.

---

## 7. In-tree mapping (evidence only; not a decision)

| Fact | Path |
|------|------|
| Drop aborts reader/writer, does not join | `crates/hub/src/rpc.rs` (`impl Drop for RpcClient`) |
| CLI `process::exit` after `run()` | `crates/cli/src/main.rs` |
| `mem::forget(client)` after successful add only | `crates/cli/src/commands.rs` (`AgentCommand::Add`) |
| Remove/list Drop normally (asymmetric vs add) | same file, `Remove` / list paths |
| Daemon stdin/stdout nulled; stderr inherit is opt-in | `crates/hub/src/daemon.rs` `ACP_HUB_DAEMON_STDERR` |
| Windows spawn: `DETACHED \| BREAKAWAY \| NO_WINDOW` cascade | `crates/hub/src/daemon.rs` `spawn_daemon_windows` |
| Archaeology wait graph | living plan §3.A (do not overwrite) |

`mem::forget` is CLI-add-only. Lib / MCP share abort-on-Drop but not `process::exit` / `forget`. Synthesis must say whether they need the same `shutdown()`.

---

## 8. Citations

### Windows / MSDN

- Named pipe client: https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-client
- Named pipe operations (flush then disconnect): https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-operations
- `FlushFileBuffers` (server end waits for client read): https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers
- `DisconnectNamedPipe`: https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-disconnectnamedpipe
- `CancelIoEx`: https://learn.microsoft.com/en-us/windows/win32/fileio/cancelioex-func
- Canceling pending I/O: https://learn.microsoft.com/en-us/windows/win32/fileio/canceling-pending-i-o-operations

### Tokio / mio

- `Runtime` shutdown; Drop waits forever: https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html
- `Runtime` source: https://github.com/tokio-rs/tokio/blob/master/tokio/src/runtime/runtime.rs
- `TcpStream` async `shutdown()`: https://docs.rs/tokio/latest/tokio/net/struct.TcpStream.html
- `AsyncWrite::poll_shutdown`: https://docs.rs/tokio/latest/tokio/io/trait.AsyncWrite.html
- `ClientOptions` / named pipe: https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/struct.ClientOptions.html
- Named-pipe source: https://github.com/tokio-rs/tokio/blob/master/tokio/src/net/windows/named_pipe.rs
- Client-drop test (`ERROR_NO_DATA`): https://github.com/tokio-rs/tokio/blob/master/tokio/tests/net_named_pipe.rs
- `Child::wait` closes stdin first: https://docs.rs/tokio/latest/tokio/process/struct.Child.html
- `CancellationToken`: https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html
- mio `NamedPipe::drop` cancels, does not wait: https://github.com/tokio-rs/mio/blob/master/src/sys/windows/named_pipe.rs
- mio handle leak vs blocking `Poll::drop`: https://github.com/tokio-rs/mio/issues/1944
- Runtime Drop / `block_on` deadlock: https://github.com/tokio-rs/tokio/issues/6463
- Unidirectional named-pipe spin: https://github.com/tokio-rs/tokio/issues/5170
- Overlapped handle + std I/O: https://github.com/tokio-rs/tokio/issues/4802 , https://github.com/tokio-rs/tokio/pull/4824 , https://github.com/rust-lang/rust/pull/95469

### Peer RPC / LSP

- jsonrpsee `Client` Drop sends oneshot: https://docs.rs/jsonrpsee-core/latest/src/jsonrpsee_core/client/async_client/mod.rs.html (`impl Drop for Client`)
- jsonrpsee `Subscription` Drop `try_send` only: https://docs.rs/jsonrpsee-core/latest/src/jsonrpsee_core/client/mod.rs.html
- rust-analyzer `IoThreads::join` after protocol close: https://github.com/rust-lang/rust-analyzer/blob/master/lib/lsp-server/src/stdio.rs
- rust-analyzer must drop `Connection` before join: https://github.com/rust-lang/rust-analyzer/issues/22521
- tarpc `ClientMessage::Cancel` on request-future Drop: https://docs.rs/tarpc/latest/tarpc/enum.ClientMessage.html
- tarpc cascading cancel: https://docs.rs/tarpc/latest/tarpc/

### Process / inheritance (looks like pipe hang)

- .NET `WaitForExit` waits redirected stdout EOF: https://github.com/dotnet/runtime/issues/29232
- Raymond Chen, handle inheritance race: https://devblogs.microsoft.com/oldnewthing/20131018-00/?p=2893
- duct inheritance gotcha: https://github.com/oconnor663/duct.py/blob/master/gotchas.md
- CPython overlapped cancel: https://github.com/python/cpython/issues/56537
- futures-rs waiting for IOCP in Drop: https://github.com/rust-lang-nursery/futures-rs/issues/1278
- `CancelIoEx` hang when mixing sync + overlapped: https://www.ntkernel.com/a-rare-cancelioex-hang-in-go-on-windows/
- POSIX groups ≠ Windows jobs: https://github.com/watchexec/command-group

### In-tree prior notes (read-only)

- `doc/dev/work-report-error-hiding-audit-2026-07-26.md` §3 (cold add printed `registered`, parent Wait still hung)
- `doc/dev/plan-ipc-lifecycle-default-practice-2026-08-16.md` §3.A (archaeology; do not overwrite)

---

**Document end.** No source changes. No `ux-*.md` edits. No commit implied.
