# 工单报告：完整 error-hiding 审查与 rework（rc.9）

| 字段 | 值 |
|------|-----|
| **性质** | 实现自审 / 完整 rework（**不改**反馈 SSOT） |
| **触发** | 操作者指出：超时假成功、吞错、本地兜底 = error hiding；要求 **full review 不是 patch** |
| **日期** | 2026-07-26 |
| **版本** | 0.2.1-rc.9 |
| **审计 SSOT** | `doc/dev/AUDIT-error-hiding-self-review-2026-07-26.md` |

---

## 1. 审查结论（我留下的屎 + 处置）

| 模式 | 位置 | 为何是 error hiding | 处置 |
|------|------|---------------------|------|
| 超时后若磁盘有 agent 仍 `println!(registered)` + exit 0 | CLI `agent add` | **把不确定伪装成成功** | **删除**；只认 daemon `register_agent` Ok |
| `send_prompt` 断连后自动再 send 一次 | CLI `handle_send` | 接受可能已成功却重投 → **静默双 turn** | **删除**；send 失败即失败 |
| CLI `timeout` 包 cancel | CLI `handle_cancel` | 用墙钟把「daemon 慢/挂」糊成笼统超时文案 | **删除**；hub cancel 已不 join agent |
| `connect_with_retry` 静默重连 | CLI | 第一次失败被吞 | **删除**；connect 失败原样上抛 |
| 本地写 `agents.json` 当成功（rc.7） | CLI | 磁盘与 daemon 内存分裂 | 保持不回退 |
| cancel `requested=false` 打印 `no active run` | CLI | hub 仍有 `run_id`（已 requested） | **rc.9：already requested 文案** |
| cancel 无 handle 仍「requested cancellation」过满 | CLI/hub | 暗示 agent 已收到 | **rc.9：`acp_notify_enqueued` + mark-only 文案** |
| list/mutate `let _ = refresh` | hub registry | 刷新失败静默旧内存 | **rc.9：`tracing::warn`** |
| wait JSON serialize 静默丢行 | CLI wait | 流式丢消息无信号 | **rc.9：stderr** |
| wait client bad MessageRow | hub wait | 静默跳过 | **rc.9：warn** |
| daemon probe `.ok()` | daemon | 启动失败无诊断 | **rc.9：debug 日志** |
| close `finalize_run_cas` ignore | lifecycle | close Ok 但 run 可能非终态 | **rc.9：warn** |

**根因修复（rc.8）保留：** `mutate_registry` 不再无界等 generation；cancel 不 join `send_notification`。  
**本工单是完整 rework：** 拆 CLI/语义层 H + 补诚实契约字段与可观测性，不是再贴 timeout。

---

## 2. 仍诚实失败的路径

- `agent add` → 仅 `connect` + `register_agent`；Err 即 exit 1  
- `send` → 一次 `send_prompt`；daemon closed → `daemon_unavailable`，**不重投**  
- `cancel` → 一次 RPC；`requested` = hub mark；`acp_notify_enqueued` = 是否 schedule 了 notify  
- `wait --timeout` → `error: timeout` exit 1（非假成功）

P0-3（daemon 中途掉）**不**用静默重试伪装。

---

## 3. 冷 `agent add` 再开刀（rc.8 QA 仍挂 — 已自测）

| 证据 | 内容 |
|------|------|
| QA journal | `tmp/acp-full-ux-20260726-102153/journal/20-agent-add-cold.meta.txt`：hung；`agents.json` 已写；stdout 有 `registered` |
| 本地复现 | CLI 已打印 `registered`，父进程 Wait 仍可挂很久（Windows pipe Drop / Job 树） |
| 修复 | `RpcClient` Drop 不阻塞；`agent add` `mem::forget(client)`；Windows spawn breakaway→DETACHED→`start /B`；普通 RPC 30s 诚实超时 |
| **自测** | `WaitForExit(15s)` 冷 add ×5：~140ms–1s，`exit=0`，`registered`，`agents.json` 存在（debug `0.2.1-rc.9` 二进制） |

**说明：** rc.8 的 `mutate_registry` 有界是必要条件，但**不是** QA 症状的全部；后半段是 Windows 客户端/进程树问题。

---

## 4. 测试

- hub：`cancel_marks_requested_even_when_agent_notify_fails` 断言 `acp_notify_enqueued=false`  
- hub：`cancel_is_idempotent_after_successful_request` 断言 first enqueue + second already-requested 仍有 run_id  
- **冷 add WaitForExit 自测**（上表）  
- CI：`cargo test` core + cli；clippy `-D warnings`

---

## 4. 反馈 SSOT

**未修改**任何 `ux-*-feedback-*.md` / operator baseline 冻结文。  
仅实现侧审计 + 本工单 + CHANGELOG。

---

**文档结束。**
