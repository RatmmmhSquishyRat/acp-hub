# 工单报告：rc.6 P0-1 / P0-2 **根因**修复（非超时糊弄）

| 字段 | 值 |
|------|-----|
| **性质** | 实现工单（**不改**反馈 SSOT） |
| **输入** | `ux-unified-feedback-2026-07-26-rc6.md` P0-1 / P0-2 |
| **否定** | rc.7 的「CLI 超时 + 本地写 agents.json 兜底」不是合格实现 |
| **发布** | 本报告合入后打 **0.2.1-rc.8** |

---

## 1. P0-1 冷 `agent add` 假死

### 根因

`CoreHub::mutate_registry` **在写 `agents.json` 之前** 对每个 affected agent 执行：

```text
init_lock.lock().await
ctx.agent_generation_writer(agent_id).await   // 无界等待 commands/callbacks write
handles.lock()
```

`agent_generation_writer` 要拿 generation **写锁**。任意该 agent 上正在跑的 command loop / load / prompt 占着对应锁时，**register RPC 永不返回**。  
磁盘可能在别的时序下已可见，操作者表现为「配置已写、CLI 死机」。

冷路径第一次 add **本不该**等任何 live connection；旧代码对**不存在 handle 的新 agent** 仍走 generation 路径的 init/epoch 装配，且 **replace/remove 在 busy 时无界等待**。

### 正确修复

1. **有 in-flight conversation ops → 立即 `Conflict`**（不 spin 无界）。
2. **handle init** 只等 init mutex，**有 10s 上限**。
3. **仅当 `handles` 里已有该 agent** 时，才 `try_agent_generation_writer` 轮询等连接静默，**15s 上限**；**新 agent id 完全跳过**（冷 add 主路径）。
4. **先 commit 磁盘 + 发布 memory + epoch++**，再 revoke handles；不再在持有 handles 时做无界 generation 等待。

CLI 去掉「本地乱写 agents.json 当主路径」；仅保留 daemon 回复卡住时「若磁盘已有 id 则提示」的窄安全网。

---

## 2. P0-2 长任务 `cancel` 挂死

### 根因

Cancel RPC 在 mark 之后 **await** `agent_handle` + **`ConnectionTo::send_notification`**。  
`session/cancel` 设计上是绕过 cmd 队列的 out-of-band 通知，但 **写共享 ACP 连接在 agent 满速吐 generation 时会阻塞写端** → cancel RPC 与整代 LLM 墙钟绑定（实测 >10min）。

CLI 再套 12s timeout **不能**代替「RPC 路径不得 join agent I/O」。

### 正确修复

1. Hub **先** `request_run_cancel_cas` + runtime Cancelling + `cancel_requested=true`，**再返回**。
2. 只从 **`handles` 已有连接** 取 handle（**禁止** cancel 路径 `agent_handle` 冷启）。
3. `send_notification` 放到 **`spawn_blocking` fire-and-forget**，**不 join**。
4. 操作者立即得到 `requested=true`；agent 侧最终 `StopReason::Cancelled` 仍由既有 finalize 路径收敛。

---

## 3. 测试

| 套件 | 结果 |
|------|------|
| `hub::tests::registry::*`（含 remove 等 load、replace 等 init） | PASS |
| `public_run_rpc_requires_owner_and_blocks_registry_mutation` | PASS（Conflict） |
| `hub::tests::cancel::*` | PASS |
| full core lib + cli tests | 见 CI |

---

## 4. 纪律

- 反馈原文只读。
- 本文件记录根因与实现；CHANGELOG 写用户可见行为。

---

**文档结束。**
