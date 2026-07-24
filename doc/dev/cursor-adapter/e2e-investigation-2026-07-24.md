# Cursor agent via ACP Hub — E2E investigation (2026-07-24)

> **Supersession (product defaults, same day):** Host-local findings below were
> taken against **reject-by-default samples** and **lag connection-fatal** hub
> behavior. After the UX rebalance, new registrations default to usable
> auto-allow + fs/terminal, and lag no longer kills the client (see
> `doc/ssot/agent-managed/`). Windows create→send reliability may still be
> residual (RESIDUALS.md). Treat permission/lag rows here as **historical
> failure modes**, not current product law.

**Status:** investigation complete (manual / host-local)  
**Host:** Windows 10/11, `acp-hub 0.2.0`, Cursor adapter `adapters/cursor/adapter.mjs`  
**Cursor agent:** `cursor-agent` / `versions/2026.07.16-899851b`  
**Model observed in session params:** `grok-4.5[effort=high,fast=true]` (Cursor first-party)  
**Related:** [cursor-adapter spec](./spec.md), hub RPC `daemon closed the connection` (`crates/hub/src/rpc.rs`)

This document records a host-local end-to-end probe: whether ACP Hub can drive the Cursor adapter so that `cursor-agent` performs real workspace work (file create/edit). It is **not** a CI gate and is not a substitute for `adapter-test.mjs`.

---

## 1. Executive summary

| Question | Answer |
|----------|--------|
| Can ACP Hub call Cursor and make it do work? | **Yes — proven once** (file written with expected marker). |
| Is the path stable enough for production automation? | **No.** |
| Primary failure surface | Hub **daemon / session lifecycle** on Windows (disconnect, hang, resume/load failure), not “Cursor cannot write files”. |
| Overall verdict | **Functionally possible, operationally unreliable.** |

One-line result:

> **Capability: pass (at least once). Reliability: fail.**

---

## 2. Environment and registration

### 2.1 Binary / paths

| Component | Path / version |
|-----------|----------------|
| `acp-hub` | `C:\Users\15480\.cargo\bin\acp-hub.exe` → `0.2.0` |
| Cursor adapter | `repos/acp-hub/adapters/cursor/adapter.mjs` |
| Cursor agent | `%LOCALAPPDATA%\cursor-agent\` (launcher + versioned `node.exe` + `index.js`) |
| Default Hub home | `%USERPROFILE%\.acp-hub` |

### 2.2 Working registration shape (required for tools)

Default registry sample often uses `permission_policy: reject` and disabled FS/terminal capabilities. For agent **write** work, registration must advertise callbacks and auto-allow permissions:

```text
acp-hub agent add cursor --type stdio --command node --args <adapter.mjs> ^
  --permission-policy auto-allow ^
  --allow-read --allow-write --allow-terminal ^
  --allow-root <work-dir>
```

Observed effective `agents.json` fragment after successful re-register:

```json
{
  "permission_policy": "auto-allow",
  "client_capabilities": {
    "fs": {
      "read_text_file": true,
      "write_text_file": true,
      "allowed_roots": ["...\\tmp\\acp-cursor-smoke"]
    },
    "terminal": true
  }
}
```

Without `auto-allow` + FS/terminal, tool calls may be rejected by the Hub permission policy even if Cursor wants to edit.

### 2.3 Process tree when healthy

```text
acp-hub.exe serve --home <hub-home>
  └─ node adapters/cursor/adapter.mjs
       └─ <cursor-agent>/node.exe index.js acp
            └─ index.js worker-server
```

---

## 3. Test protocol

### 3.1 Task

In an isolated work directory, ask the agent to create a marker file, e.g.:

- File: `hello-from-acp-hub.txt` / `trial1.txt` / `smoke3.txt`
- Content: single-line marker such as `ACP-HUB-CURSOR-OK <timestamp>` or `TRIAL1-OK`

### 3.2 Command sequence (canonical)

```text
acp-hub agent list
acp-hub conv create cursor --cwd <work> --json
# optional:
acp-hub param set <conv> model "grok-4.5[effort=high,fast=true]"
acp-hub mode set <conv> agent
acp-hub send <conv> --text "<create-file prompt>"
acp-hub conv show <conv>
# verify file on disk
```

### 3.3 Artifact roots (host-local, outside repo)

| Root | Role |
|------|------|
| `tmp/acp-cursor-smoke/` | First successful file write |
| `tmp/acp-investigation-*` | Later repro attempts, logs, create/send captures |
| `tmp/acp-investigation-py/`, `tmp/acp_investigate*.py` | Scripted repro attempts |

These paths live under the AIWorkshop workspace, not under the published crate tree.

---

## 4. Trial log (chronological)

### 4.1 Trial A — first smoke (primary positive evidence)

| Step | Result |
|------|--------|
| Re-register cursor with auto-allow | OK |
| `conv create cursor` | OK → `conv-ee4939a69963408084243109885aa9e5`, status idle |
| `param list` / `mode list` | OK; model already `grok-4.5[effort=high,fast=true]`, mode `agent` |
| `send` (create `hello-from-acp-hub.txt`) | **Partial** |
| File on disk | **OK** |
| Session status after send | **`failed`** |

**File content (persisted):**

```text
ACP-HUB-CURSOR-OK 2026-07-24T12:36:55.7514337+08:00
```

Path:

```text
tmp/acp-cursor-smoke/hello-from-acp-hub.txt
```

**Hub projection (condensed):**

1. User prompt captured  
2. Assistant thought stream (many small text chunks)  
3. Tool: **Edit File** → `in_progress` → **`completed`** with diff to `hello-from-acp-hub.txt`  
4. Further tool/shell activity started  
5. CLI error: `Error: daemon unavailable: daemon closed the connection`  
6. Conversation status: **failed**

**Interpretation:** Cursor agent **did real work**. Hub CLI / daemon connection **did not complete the turn cleanly**.

---

### 4.2 Trial B — second smoke (`smoke2`)

| Step | Result |
|------|--------|
| Fresh `conv create` | OK → `conv-c4f288c3b3544d3f9f61b54773acc361` |
| param/mode set | OK |
| `send` (create `smoke2.txt`) | Fail |
| File | **Missing** |
| Error | `daemon closed the connection` / status **failed** |
| Projection | Often only user message (or early fragment) |

---

### 4.3 Trial C — background restart smoke (`smoke3`)

| Step | Result |
|------|--------|
| Kill/restart daemon, `agent list` | OK |
| `conv create` | OK → `conv-c44c083689654304b507cdfdd38ffe35` |
| `param set` / `send` | Fail |
| File `smoke3.txt` | **Missing** |
| Error | `could not session/load conversation on endpoint cursor` caused by `daemon unavailable: resume/load operation failed` |

**Interpretation:** Create can succeed; subsequent operations that need a live/resumable endpoint session fail.

---

### 4.4 Trial D — clean create (isolated captures)

| Observation | Result |
|-------------|--------|
| `conv create cursor` (clean-ish process tree) | **OK** — e.g. `conv-aa7bc168efd94a4ba663d192ca4ccb5f`, `conv-20d773ecf26742f6896f5bd5c4b1b450` |
| `conv create grok` (control) | **OK** — e.g. `conv-f3ae808e5b2f469480e9f112091be8fd` |
| Create after aggressive process kill | Sometimes **`io error: 拒绝访问 (os error 5)` / Access denied** |

Cursor create is **not always broken**; failures concentrate on **post-create** operations and **degraded daemon state**.

---

### 4.5 Trial E — hangs (reliability collapse)

| Operation | Symptom |
|-----------|---------|
| `agent add cursor ...` | Often **hangs after writing `agents.json`**; CLI never returns |
| `param set <conv> model ...` | **Hang**; Hub process CPU observed **300–400+**; adapter/agent idle |
| `agent list` under bad state | Can hang **far beyond 30–60s**; Python `subprocess` timeout did not cleanly terminate the tree |
| Hub under hang | `acp-hub.exe serve` high CPU; child adapter low CPU |

Process tree when stuck on param/create:

```text
acp-hub.exe serve   (high CPU)
  └─ node adapter.mjs
       └─ cursor-agent ... index.js acp
            └─ worker-server
```

---

### 4.6 Scripted / multi-home attempts

Attempts with isolated `--home`, Python drivers, and hard timeouts reproduced the same classes of failure:

- Register or list hang  
- Create OK, send/param hang or daemon disconnect  
- Forced kill → Access denied / rotten locks  

No scripted run achieved **two clean create+send+file+idle** cycles in a row.

---

## 5. Error taxonomy

| Error / symptom | When | Likely meaning |
|-----------------|------|----------------|
| `daemon unavailable: daemon closed the connection` | Mid-`send` after tools started | Client RPC reader hit EOF on daemon pipe (`rpc.rs` reader_loop `Ok(None)`). Turn aborted; work may already have landed on disk. |
| `daemon unavailable: resume/load operation failed` + `could not session/load conversation on endpoint cursor` | After create, on param/send | Endpoint session cannot be reloaded/resumed; Hub treats conversation as unusable. |
| `conversation <id> is busy with an in-flight turn` | Concurrent `agent add` / second client | In-flight turn lock; expected under concurrency. |
| `io error: Access denied (os error 5)` | Immediately after force-killing Hub/adapters | Named pipe / DB / lock left in a bad Windows state. |
| Silent hang + Hub high CPU | `agent add`, `param set`, sometimes `list`/`create` | Hub busy-wait / blocked RPC while agent or daemon path does not complete. |
| Empty CLI capture files | Hang before process exit | Caller timed out or killed mid-command. |

Hub source reference for disconnect message:

```text
crates/hub/src/rpc.rs  — reader_loop: Ok(None) => "daemon closed the connection"
```

Design note: notification receiver lag is treated as **connection-fatal** in maintained Hub behavior (see review book R-DAEMON-004). High-churn Cursor streaming (many small thought/tool updates) is a plausible stressor for that path.

---

## 6. Root-cause ranking

| Priority | Cause | Evidence |
|----------|-------|----------|
| **P0** | Hub daemon ↔ Cursor agent long-lived / streaming session instability on Windows | Write succeeded then daemon closed; hangs with Hub spinning |
| **P0** | Process-tree kill leaves pipe/DB/lock unusable | Access denied; cascading hangs after cleanup |
| **P1** | Stream notification volume / lag → connection drop | Many micro-updates in projection; Hub design connection-fatal on lag |
| **P1** | Default `permission_policy: reject` blocks tools if not overridden | Must re-register with auto-allow for edits |
| **P2** | Investigation concurrency (multiple shells, mutual kill) | Amplifies flakiness; not the only failure mode |

**Ruled out as sole cause:** “Cursor Grok cannot write files” — contradicted by Trial A file + Edit File completed.

---

## 7. What works vs what does not

### Works (intermittent but observed)

- Install / `acp-hub --version`
- Register cursor agent (when it returns)
- `agent list`
- `conv create cursor` and `conv create grok` (often OK when process tree is clean)
- `param list` / `mode list` on a live conversation
- Cursor agent **Edit File** through Hub (at least once)
- Hub projection capture of thoughts + tool updates

### Does not work reliably

- End-to-end `send` completing with status **idle** and CLI exit 0
- Back-to-back create+send cycles
- `agent add` always returning
- `param set` always returning
- Resume/load after create under stress
- Recovery after forced `taskkill` of Hub mid-turn

---

## 8. Operator recommendations (until fixed)

1. **Single-flight only:** one create/send at a time; no parallel Hub CLIs on the same home.  
2. **Register with auto-allow + roots** before expecting writes.  
3. **Prefer clean process tree:** stop `acp-hub` before long retests; avoid killing mid-turn if possible.  
4. If state is rotten: stop Hub, inspect `%USERPROFILE%\.acp-hub` (`daemon.*`, `hub.db-wal`); consider a **fresh `--home`** for isolation rather than only force-killing.  
5. If `param set` hangs: try **send with default model** (often already Grok 4.5 on this host).  
6. Treat **file existence** and **Hub status** as separate success criteria; a `failed` conversation may still have applied edits.  
7. Do not use this path as the only production automation until hangs/disconnects are fixed or mitigated.

---

## 9. Suggested follow-ups (product / engineering)

1. Reproduce under a single clean Windows session with RUST logging / Hub daemon debug for one `send` that disconnects mid-tool.  
2. Compare notification rate (Cursor vs Grok) against lag thresholds that trigger connection-fatal behavior.  
3. Ensure `agent add` cannot hang after durable `agents.json` write (timeout + clear error).  
4. Harden Windows shutdown of endpoint process trees so Access denied does not poison the next start.  
5. Document operator checklist in `adapters/cursor/README.md` (auto-allow, single-flight, clean shutdown).  
6. Optional: integration test that only asserts create+list on CI; host-local write smoke remains manual until stable.

---

## 10. Verdict table (for status boards)

| Criterion | Pass/Fail | Notes |
|-----------|-----------|-------|
| Adapter launches under Hub | Pass (intermittent hang on add) | |
| Conversation create | Pass (usually) | |
| Cursor tool write via Hub | **Pass (once)** | Strong positive evidence |
| Send completes cleanly | **Fail** | daemon closed / load failed / hang |
| Repeatability (N≥2 clean cycles) | **Fail** | |
| Safe recovery after kill | **Fail** | Access denied / hang |

**Final:** ACP Hub **can** drive Cursor agent for real work; **stability is insufficient**. Track as a **Windows host-local reliability defect** around daemon/session lifecycle and Cursor streaming, not as “Cursor provider impossible.”

---

## 11. Evidence index

| Evidence | Location / value |
|----------|------------------|
| Successful marker file | Workspace `tmp/acp-cursor-smoke/hello-from-acp-hub.txt` |
| Failed conv (write succeeded) | `conv-ee4939a69963408084243109885aa9e5` status failed |
| Failed conv (no write) | `conv-c4f288c3b3544d3f9f61b54773acc361` status failed |
| Create OK + load fail | `conv-c44c083689654304b507cdfdd38ffe35` + resume/load error |
| Create OK capture | e.g. `tmp/acp-investigation-py/create-out.txt`, `tmp/acp-investigation-final/cursor-create-out.txt` |
| Grok control create | `tmp/acp-investigation-final/control-grok-create.txt` |
| Param hang artifact | `tmp/acp-investigation-py/param1.txt` length 0 |
| List hang log | `tmp/acp-investigation-v2/run.log` stuck on `agent list timeout=30` |
| Hub disconnect source | `crates/hub/src/rpc.rs` |
| Adapter behavior | `adapters/cursor/adapter.mjs`, `adapters/cursor/README.md` |

---

## 12. Change log

| Date | Note |
|------|------|
| 2026-07-24 | Initial host-local investigation written from live smoke + multi-attempt repro on Windows. |
