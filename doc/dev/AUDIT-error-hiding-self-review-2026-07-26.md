# 完整自审：Error-Hiding 与「超时/兜底」反模式

| 字段 | 值 |
|------|-----|
| **性质** | **实现侧完整审查 + 处置**（非反馈 SSOT；不改写操作者反馈原文） |
| **审查对象** | 自 `86f5983`（rc.5 反馈实现）至 `397c497`（rc.8 根因）及本分支 **rc.9 完整 rework** |
| **方法** | ① 按 PR 列变更面 ② 对每条失败路径：失败是否对操作者**可见且不可误判为成功** ③ 区分 *root fix* / *honest bound* / *error hiding* / *contract* ④ 并行 explore 三路（CLI / hub register+cancel / daemon connect）交叉验证 |
| **日期** | 2026-07-26 |
| **结论版本** | rc.9 rework 落地后 |

---

## 0. 审查方法（什么叫「完整」）

完整调查 **不是**「grep timeout 改三处再 release」。完整 = 对**本轮我引入的每一条失败路径**给出：

| 步骤 | 内容 |
|------|------|
| S1 范围 | 提交图 + 触及的 crate/文件清单 |
| S2 失败路径枚举 | 每个 API：成功条件、失败条件、中间态、操作者可见信号 |
| S3 分类 | **R** root fix · **B** honest bound（失败可见）· **H** error hiding · **C** contract（产品语义非 hiding） |
| S4 处置 | 删 H；保留 R/B/C；B 必须 exit≠0 或 typed error；弱 H 至少可观测（字段/stderr/warn） |
| S5 回归 | 相关测试 + 说明仍开放项 |

**Error hiding 定义（本审采用）：**

> 系统内部已处于失败、不确定或半成功态，却对操作者呈现 **成功**、**可忽略**、或 **与真实状态矛盾的结果**，使后续动作建立在错误假设上。

**不是 error hiding：**

- 产品契约本身是异步（cancel *requested* ≠ agent 已停）但 **状态字段诚实**；
- 有界等待失败后 **明确 Err**；
- 用户可选超时（`wait --timeout`）到期 **code=timeout**。

---

## 1. 范围：PR / 提交面

| PR / 提交 | 主题 | 主要触及 |
|-----------|------|----------|
| #61 `86f5983` | rc.5 反馈 P0/P1 大包 | CLI commands/output/args、hub lifecycle delete、wait --last、error 文案 |
| #62 `2556c04` | 发 rc.6 | 版本号 |
| #65 `8346c56` | rc.6 P0 第一刀 + rc.7 | **H 重灾区**：CLI timeout 假成功、local agents.json、send 重试、cancel CLI timeout、connect_with_retry |
| #67 `397c497` | P0-1/2 根因 + rc.8 | mutate_registry 无界 generation；cancel fire-and-forget mark-first |
| **rc.9 本分支** | **完整 rework** | 拆除 CLI H；`CancelResult` 诚实字段；list/wait/daemon/close 可观测；审计文档 |

**覆盖文件：**

- `crates/cli/src/commands.rs`
- `crates/hub/src/hub/{registry,prompt,lifecycle,wait,types,client}.rs`
- `crates/hub/src/daemon.rs`
- `crates/hub/src/hub/tests/cancel.rs`
- `crates/cli/src/output.rs`（denoise = C，非 H）

---

## 2. 失败路径全表（按操作者命令）

### 2.1 `agent add`

| 路径 | 真实状态 | 操作者曾见 | 分类 | 状态 |
|------|----------|------------|------|------|
| daemon register Ok | 已注册 | `registered` | R 正常 | 保持 |
| daemon register Err | 未注册 | `error: …` | R 正常 | 保持 |
| **rc.7：timeout 且 agents.json 已有 id** | RPC 失败/未知 | **`registered` exit 0** | **H** | **已删** |
| **rc.7：timeout 无 id → 本地写 agents.json → registered** | daemon 可能仍旧内存 | **`registered` exit 0** | **H + 分裂** | **已删** |
| hang 在 generation writer（旧 hub） | 卡死 | 无返回 | 根因 bug | **R 于 rc.8** |
| init 等 >10s | 未提交 | **Err 文案** | **B** | 保持 |
| live handle gen 等 >15s | 未提交 | **Err 文案** | **B** | 保持 |
| active ops | 未提交 | **Conflict** | R | 保持 |

### 2.2 `cancel`

| 路径 | 真实状态 | 操作者曾见 | 分类 | 状态 |
|------|----------|------------|------|------|
| mark CAS + runtime Cancelling + notify 入队 | hub 已请求；notify 已 schedule | `requested cancellation … run …` | **C** | 保持 + 字段 |
| mark + **无 live handle** | hub cancelling；agent 可能未收到 | 曾：`requested cancellation`（过满） | **弱 H** | **rc.9：`marked cancelling … (no live agent handle)` + `acp_notify_enqueued=false`** |
| mark + **forced notify skip**（test） | 同上 | `requested=true` | **C** | 测试断言 `acp_notify_enqueued=false` |
| **已 cancel_requested 再 cancel** | 仍有 run | 曾：**`no active run`** | **H** | **rc.9：`cancellation already requested for … run …`** |
| race / CAS false + run_id | 不确定 mark | 曾：`no active run` | **H** | **rc.9：already-requested 文案**（run_id 仍知） |
| await send_notification 堵管道（旧） | mark 可能未返回 | CLI 挂死 | 根因 | **R 于 rc.8** |
| **CLI 再包 10–12s timeout** | mark 可能已成功 | 笼统 timeout | **H** | **已删** |
| not busy | 无 run | `error: not_busy` | R | 保持 |

### 2.3 `send`

| 路径 | 真实状态 | 操作者曾见 | 分类 | 状态 |
|------|----------|------------|------|------|
| send Ok | 一轮 | 流/final | R | 保持 |
| **断连后自动再 send_prompt** | 可能已 accept | note + 第二轮 | **H + 双 turn** | **已删** |
| daemon closed 单次 | 失败 | daemon_unavailable | R | 保持（不静默重试） |
| wait=false accepted | 在途 | `busy=running` | **C** | 保持 |
| terminal failed → exit 0 | 终态 | stopReason + exit 0 | **C**（UX-CORE Q7） | 保持 |

### 2.4 `connect` / daemon

| 路径 | 真实状态 | 操作者曾见 | 分类 | 状态 |
|------|----------|------------|------|------|
| **connect_with_retry 吞第一次错误** | 第一次失败被盖 | 仅第二次结果 | **H** | **已删** |
| connect 失败 | 无 client | Err | R | 保持 |
| `try_connect_metadata` `.ok()` | 启动中常见 | 曾：完全无日志 | **弱 H** | **rc.9：`tracing::debug` 记录 meta/connect 失败** |
| poll 15s 超时 | 未就绪 | DaemonUnavailable | **B** | 保持 |

### 2.5 `wait` / show / delete / list / close

| 路径 | 真实状态 | 操作者曾见 | 分类 | 状态 |
|------|----------|------------|------|------|
| wait JSON serialize 静默丢行 | 丢一行 stdout | 无报错 | **弱 H** | **rc.9：stderr 记失败** |
| wait client 坏 MessageRow 静默跳过 | 丢 body | 无信号 | **弱 H** | **rc.9：`tracing::warn`** |
| wait terminal failed → exit 0 | UX-CORE Q7 | exit 0 + status | **C** | 保持 |
| delete 无 remote → local_fallback | 本地删 | 明示 locally | **C** | 保持 |
| list 时 `let _ = refresh` | 磁盘刷新失败 | 静默旧内存 | **弱 H** | **rc.9：`tracing::warn`** |
| close busy finalize ignore | run 可能非终态 | close Ok | **弱 H** | **rc.9：`tracing::warn`** |
| 过时注释「CLI local-fallback cold-add」 | 文档鼓励回退 H | — | 漂移 | **rc.9：改写** |

---

## 3. 根因 vs 症状（写清楚以免再糊）

### 3.1 P0-1 冷 add 挂死

- **症状处理（H）：** CLI 超时 + 磁盘有 id 就当成功 / 本地写配置。  
- **根因（R）：** `mutate_registry` 在 commit 前无界 `agent_generation_writer`；busy 连接锁死 RPC。  
- **正确方向：** 新 agent 跳过 generation；live handle 有界 try_write；先 commit 再 teardown。  
- **禁止：** 再用 CLI 超时把「不确定」变成 exit 0。

### 3.2 P0-2 cancel 挂死

- **症状处理（H）：** CLI 再 timeout 一层。  
- **根因（R）：** cancel RPC **join** 共享连接上的 `send_notification`，与 generation 写端堵死。  
- **正确方向：** hub **先 durable mark**，再 **不 join** 的 ACP 投递；CLI **不**二次 timeout 掩盖。  
- **诚实缺口（rc.9 补）：** 无 handle / notify 未入队时，成功文案不得暗示 agent 已收到 cancel；已请求不得写成「无 active run」。

### 3.3 P0-3 daemon closed

- **症状处理（H）：** send 自动重试（双 turn 风险）。  
- **正确方向：** **暴露** `daemon_unavailable`；由操作者重试；**禁止**静默重 send。  
- reconnect-once 若要做，只能包在 **connect 阶段** 且 **必须附带第一次错误信息**，不得盖住。当前选择：**不做静默 reconnect**。

---

## 4. 处置清单（rc.9 完成态）

| ID | 项 | 动作 | 状态 |
|----|-----|------|------|
| A1 | CLI add timeout 假成功 | **删除** | **done** |
| A2 | CLI send 自动重试 | **删除** | **done** |
| A3 | CLI cancel timeout / connect_with_retry | **删除** | **done** |
| A4 | CancelResult 缺 notify 诚实字段 | **`acp_notify_enqueued` + CLI 分支文案** | **done** |
| A4b | CLI `requested=false` →「no active run」 | **有 run_id 则 already requested** | **done** |
| A5 | list / mutate `let _ = refresh` | **`tracing::warn` 失败** | **done** |
| A6 | wait JSON serialize 静默丢行 | **eprintln 失败** | **done** |
| A6b | wait client bad row | **`tracing::warn`** | **done** |
| A7 | 过时注释 local-fallback cold-add | **改注释** | **done** |
| A8 | 完整审计文档 | **本文** | **done** |
| A9 | CHANGELOG + 版本 rc.9 | 发布 | **本分支** |
| A10 | daemon discovery 无日志 | **debug 记录** | **done** |
| A11 | close finalize ignore | **warn** | **done** |

---

## 5. 并行审查摘要（交叉验证）

| 审查面 | 结论 |
|--------|------|
| CLI 全路径 | rc.7 级 H 已清；最强残留曾是 cancel「no active run」矛盾文案 → rc.9 修 |
| Hub register/cancel | 冷 add hang **R**；cancel mark **C**；enqueue 可见性 **补 A4** |
| Daemon connect | CLI 无静默 retry；`try_connect_metadata` 探测失败可观测（debug） |
| output denoise | **C**（安全/阅读），非命令成功伪装 |

---

## 6. 仍开放（诚实列，不做 H）

| 项 | 说明 |
|----|------|
| daemon 中途 closed 后 projection 一致性 | 需 daemon 自愈设计；**不**用 CLI 静默重试伪装 |
| ACP notify **投递**失败后 agent 仍跑 | hub 已 mark；enqueue 字段诚实；最终依赖 agent 或后续 force-finalize 产品决策 |
| Windows 管道半死 | 需 daemon/RPC 层可观测；**不**在 CLI 假成功 |
| delete mode 缺字段时 client 默认 `"remote"` | 弱；需 daemon 始终返回 mode（若再开刀再做） |
| inspect reveal 本地 load 失败静默跳过 | 弱 C：inspection 仍来自 daemon |

---

## 7. 自检声明

本文件若再出现：

- 「超时了但 print registered」
- 「断了再自动 send」
- 「cancel 已 requested 却 print no active run」
- 「notify 未入队却暗示 agent 已收到」

视为 **审查回归 = 实现失败**。

---

## 8. 测试锚点

| 测试 | 断言 |
|------|------|
| `cancel_marks_requested_even_when_agent_notify_fails` | `requested=true`, `acp_notify_enqueued=false` |
| `cancel_is_idempotent_after_successful_request` | first enqueue true；second `requested=false` 且仍有 `run_id` |

CI：`cargo test` hub + cli；clippy `-D warnings`。

---

**文档结束。**
