# ACP Hub Feedback Book  
## Send / Wait / Show 分离 · 对话可观测性 · 全量复测结论

| 字段 | 值 |
|------|-----|
| **版本** | `acp-hub 0.2.1-rc.4`（本轮全量复测基线） |
| **日期** | 2026-07-25 |
| **主机** | Windows 11 / PowerShell 7 |
| **Agent** | Cursor adapter + `cursor-agent` |
| **证据根** | `tmp/acp-full-ux-20260725-154653/` |
| **性质** | 设计纠偏 + 操作者反馈书；**非**实现 PR、**非**已落地 API |

**关联文档：**

- 全量复测细节：[`ux-full-retest-feedback-2026-07-25-rc4.md`](./ux-full-retest-feedback-2026-07-25-rc4.md)
- 前序 walk（方法不足，仅作历史）：[`ux-walkthrough-feedback-2026-07-25-rc4.md`](./ux-walkthrough-feedback-2026-07-25-rc4.md)
- 产品 Spec 基线：[`spec.md`](./spec.md)（S3 已写「发送 + 等待回复 + 查看」，但未拆成正交操作）

---

## 0. 执行摘要（一页）

### 0.1 你的判断是否正确

| 判断 | 结论 |
|------|------|
| **发送**与**等待**本是两个不同操作 | **正确** |
| 现在「等最终 response」只能挂在 `send` 上 | **正确** |
| 先前实现者把二者做成了**集成功能** | **正确**（`send` = 发 prompt + 阻塞等到 `stopReason` + 顺带流式打印） |
| 若状态机正确，就「已经有 wait」 | **半对**：有的是 **send 内部的 wait**，没有 **独立 wait / 旁观者 wait** |
| 应能随时查看完整 / 最近 / 片段对话 | **目标正确**；现状默认 `conv show` **BODY 空**，只能靠 `--json` 或当次 `send` stdout |

### 0.2 一句话产品缺口

Hub 已经能当 **ACP Client**：发 `session/prompt`、收 `session/update`、等 final response、写 run 状态。  
Hub 还不能当 **Conductor 工作台**：把「投递」「守听」「回看」拆成可组合原语，让第二进程、脚本、主 agent 各取所需。

### 0.3 目标体验（目标态）

```text
send   —— 投递消息（可 fire-and-forget，也可顺带 wait）
wait   —— 守听任意 in-flight run / session，直到终态，期间持续收更新
show   —— 任意时刻回看完整 / 最近 / 片段对话（人读默认就要有正文）
cancel —— 打断 in-flight（已有，可与 wait 正交）
```

---

## 1. 问题诊断：Send 与 Wait 被粘在一起

### 1.1 ACP 协议本身就是两段

```text
Client ── session/prompt 请求 ──────────────────► Agent 进程
Client ◄── session/update（0..N 条通知）──────── Agent     ← 过程
Client ◄── prompt 的 RPC 响应 { stopReason } ── Agent     ← 结束
```

- **发送** = 发出那一次 `session/prompt`  
- **等待** = 等到**同一个 request** 的 final response  
- **过程接收** = 消费 `session/update`  

协议层三者正交；**合并只是客户端 API 造型选择**。

### 1.2 acp-hub 现状（代码语义）

`CoreHub::send_prompt`（`crates/hub/src/hub/prompt.rs`）单路径完成：

1. write gate / reserve operation  
2. `create_run` → `busy=running`  
3. 下发 `AgentCommand::SendPrompt` → 底层 `session/prompt`  
4. **阻塞**直到 `stopReason`  
5. `finalize_run_cas` → `busy=none`，`status/last_outcome` 终态  
6. CLI `send` 顺带把捕获消息打到 stdout  

`cancel` 已是独立命令；**wait 没有**。  
第二终端对同一 `conv` 再 `send` → `conversation_busy`（正确）；但第二终端也**无法** `wait` 同一轮。

### 1.3 为何说「当成了集成功能」

| 集成在 `send` 里的职责 | 是否应独立 |
|------------------------|------------|
| 构造并投递 prompt | 是（send） |
| 占用 run / busy 门闩 | 可与 send 绑定，也可拆「create_run + prompt」 |
| 阻塞等 final response | **应可独立 wait** |
| 流式展示 update | **应属 wait（或 follow）** |
| 回看历史 | **应属 show**（不该依赖 send 还活着） |

先前实现者选择了 **CLI 一体机路径**：`send` = 人坐在终端里聊一轮。  
这对「手动试一把」够用；对 **编排 / 旁观 / 主 agent 监听外置 CLI** 不够。

### 1.4 与 Spec 的张力

`spec.md` S3 原文：

> 指定 endpoint 的某个对话发送消息 · **等待回复** · **查看回复**

三件事被写成一条需求，落地时自然收成 **一个 `send`**。  
Feedback 立场：**需求应拆成正交原语**；`send` 可提供「便捷组合默认」，但不能消灭独立 wait/show。

### 1.5 状态机（现状可用，命名勿混 OMP）

| 字段 | 含义 |
|------|------|
| `busy=running` / `cancelling` | 有 in-flight run |
| `busy=none` | 无 in-flight；**可再 send** |
| `status` / `last_outcome` | 上一轮终态：`completed` / `cancelled` / `failed` … |

**「running → done-idle」在 hub 中的正确读法：**

```text
busy: running  →  none
status/last_outcome: completed | cancelled | failed
```

不是 OMP 的 `idle` peer session；不要用 OMP subagent 生命周期套 hub conv。

**全量复测：** 快乐路径下 busy/status 与 `stop_reason` **一致可用**；异常后 daemon 卡死时状态字失去意义（见 §6）。

---

## 2. 目标设计：三个正交原语

### 2.1 设计原则

1. **投递 ≠ 守听 ≠ 回看**  
2. **默认人机路径要简单**（`send` 默认可继续「发完等到结束」）  
3. **编排路径要可拆**（`--no-wait` + `wait`；或显式 `wait` 旁观）  
4. **身份主键优先 `conv_id`**（hub 投影）；可选 `--agent-session` 解析到绑定 conv  
5. **流式在 wait；终态以 final response / finalize_run 为准**（与 ACP 一致）  
6. **show 默认必须有可读正文**（全量复测 P0）

### 2.2 概念模型

```text
                    ┌─────────────┐
   operator/script  │  Hub CLI    │
                    └──────┬──────┘
           send│wait│show│cancel
                    │
                    ▼
              CoreHub / Store
           (conv, run, messages)
                    │
                    ▼
           ACP agent process
         (session/prompt+update)
```

| 原语 | 对 Store | 对 ACP | 阻塞？ |
|------|----------|--------|--------|
| **send** | 写 user 消息；通常 create_run | 发 `session/prompt` | 默认是；可关 |
| **wait** | 读 run / 跟 update 投影 | **不**新发 prompt；等已有 in-flight 的 final | 是 |
| **show** | 读 transcript | 可选 refresh/load | 否 |
| **cancel** | busy→cancelling→终态 | `session/cancel` | 否（请求后返回） |

---

## 3. 建议 CLI 签名（简洁好用）

> 以下为 **目标 API 草案**，非当前已实现命令。  
> 命名可微调，**语义不可再粘回单一 send**。

### 3.1 `send` — 只负责投递（默认可组合 wait）

```text
acp-hub send <conv_id>
  --text <string> | --stdin
  [--param KEY=VAL]...
  [--mode <mode_id>]
  [--no-wait]              # 投递后立即返回 { runId, promptSeq }；需另 wait
  [--wait]                 # 默认行为：投递后阻塞至终态（兼容现状）
  [--json]
```

**行为：**

| 模式 | 返回时机 | 返回要点 |
|------|----------|----------|
| 默认 / `--wait` | final response 后 | `stopReason`, `runId`, `promptSeq`；stdout 流式（或见 wait） |
| `--no-wait` | prompt **已交给** agent 且 run 已 `running` | `runId`；**不**打印整轮对话 |

**兼容：** 无 flag 时 ≡ 今日 `send`（降低迁移成本）。  
**清晰：** 文档写明「默认 = send+wait 便捷组合；编排请 `--no-wait` + `wait`」。

### 3.2 `wait` — 独立守听（本 feedback 核心新增）

```text
acp-hub wait <conv_id>
  [--run <run_id>]         # 默认：当前 in-flight run；无则 error: not_busy
  [--since-seq <n>]        # 只推送 seq > n 的新消息（接续）
  [--timeout <sec>]        # 超时退出码可区分；默认无上限或大默认值
  [--follow]               # 终态后若又有新 run 则继续（可选，v2）
  [--json]                 # NDJSON：message 增量 + 最终 final
```

**行为：**

1. 解析目标 run（指定或「当前 busy run」）  
2. 若已终态：立即返回该 run 的 `stopReason` + 自 `--since-seq` 起的消息（幂等重入）  
3. 若 in-flight：  
   - 持续把**新投影消息**打到 stdout（与今日 send 流式同形，去碎屑）  
   - **直到** `finalize_run`（即 ACP prompt response 已到并落库）  
4. 退出码：`0` 正常终态；`not_busy` / timeout / failed 可区分  

**关键能力：**

- **旁观者**：终端 A `send --no-wait`，终端 B `wait`  
- **编排器 / 主 agent**：spawn 后只 `wait`，不重新投递  
- **与 cancel 正交**：终端 C `cancel`，B 的 `wait` 收到 `stopReason=cancelled`

**不做：**

- `wait` **不**发送新 prompt  
- `wait` **不**在无 run 时空等到有人 send（除非未来显式 `--follow`）

### 3.3 `show` / 消息查看 — 完整 · 最近 · 片段

今日：`conv show [--raw] [--json]`；默认表 **BODY 空**（全量复测 P0）。

**目标签名（建议收敛到 `conv show`，避免命令爆炸）：**

```text
acp-hub conv show <conv_id>
  [--json | --raw]
  [--full]                 # 默认：完整合并 transcript（人读有正文）
  [--tail <n>]             # 最近 n 条 item（按 seq）
  [--head <n>]             # 最早 n 条
  [--from-seq <a> --to-seq <b>]   # 闭或半开区间片段
  [--run <run_id>]         # 仅该 run
  [--kinds user,assistant,thinking,tool]  # 过滤 kind
  [--no-tools]             # 快捷：隐藏 tool 行
  [--max-chars <n>]        # 单条 body 截断（人读防爆）
```

**人读默认必须：**

- ROLE + **非空 BODY**（或明确 `(empty)`）  
- 无 `text ` 类型字面量、无默认刷 toolCallId（详情进 `--raw` / `--json`）

**机器默认：**

```text
acp-hub conv show <id> --json
# transcript.items[].{seq,role,kind,bodyText,runId,...}
```

**片段 / 搜索衔接：**

```text
acp-hub search <query> [--conv <id>] [--agent <id>]   # 已有
acp-hub conv show <id> --from-seq <a> --to-seq <b>  # 精读命中邻域
```

### 3.4 可选：`follow`（若不想 overload wait）

若希望 **空闲时挂着等下一轮**，不要塞进基础 `wait`：

```text
acp-hub follow <conv_id> [--json]
# 阻塞：每当出现新 run → 流式 → 终态 → 再等下一轮；Ctrl+C 退出
```

v1 可只做 `wait`；`follow` 标 v2。

### 3.5 主键：`conv_id` vs `agent_session_id`

| 参数 | 用途 |
|------|------|
| `<conv_id>` | **默认主键**（hub 投影、busy、run、本地 transcript） |
| `--agent-session <sid>` | 解析「该 agent 上绑定此 sid 的 conv」；多条则报错要求歧义消解 |

**不建议**把「裸 ACP session 且无 hub conv」当成 wait 主路径——无投影就无可靠 show。  
应：`conv create … --agent-session-id` 先建立投影，再 send/wait/show。

### 3.6 组合示例（目标体验）

```powershell
# 1) 兼容今日：发完等到结束
acp-hub send $conv --text "PING-OK"

# 2) 编排：投递与守听分离
acp-hub send $conv --text "长任务…" --no-wait --json   # → runId
acp-hub wait  $conv --run $runId --json                 # 旁路持续收
# 另一终端
acp-hub cancel $conv

# 3) 任意时刻回看
acp-hub conv show $conv                 # 全文，有 BODY
acp-hub conv show $conv --tail 20
acp-hub conv show $conv --run $runId
acp-hub conv show $conv --from-seq 10 --to-seq 25 --no-tools
acp-hub conv show $conv --json          # 机器全量
```

### 3.7 MCP / RPC 映射（实现时）

| CLI | Hub RPC（建议） |
|-----|-----------------|
| `send` | 现有 `hub/conv/send`；增 `wait: bool`（默认 true） |
| `wait` | **新** `hub/conv/wait` `{ convId, runId?, sinceSeq?, timeoutMs? }` → stream + final |
| `show` | 现有 show/messages；增 filter 参数 |
| `cancel` | 现有 cancel |

流式：CLI 可继续用「daemon 内投影 + client 拉 pages / 或 daemon 推 notification」。  
**终态信号不得靠轮询猜**；以 finalize_run / stopReason 为 SSOT（与今日 send worker 一致）。

---

## 4. 全量复测结论如何支撑本设计

### 4.1 证明「final response 检测」底层是对的

| 证据 | 含义 |
|------|------|
| 短答 / 写盘 / 多轮 `send` 均返回 `stop_reason=end_turn` | prompt response 等待正确 |
| mid-turn `cancel` → `cancelled` | cancel 与 finalize 路径正确 |
| `busy` / `last_outcome` 与 show 头一致（快乐路径） | run 状态机可用 |

→ **wait 独立化是 API 拆分，不是重做协议。**  
实现上把 `send_prompt` 里「等 response + 流式」抽成可被 `wait` 复用的 run 订阅即可。

### 4.2 证明「回看」今天不合格（show 必须修）

| 证据 | 含义 |
|------|------|
| 默认 `conv show` **BODY 列空** | 人读会话失败 |
| `conv show --json` 有 `bodyText` | 数据在，展示层坏了 |
| 多轮「测过」若只盯 send stdout | **不是**会话可回看 |

→ 无可靠 show，Conductor 故事不成立；**P0 与 wait 拆分同级或更高。**

### 4.3 证明「仅 send 集成」的操作痛点

| 场景 | 今日 | 目标 |
|------|------|------|
| 后台长任务 | 必须占着一个 `send` 进程 | `send --no-wait` + `wait` |
| 第二眼看进度 | 不能 attach | `wait --since-seq` |
| 主 agent 监听外置 CLI | 无原语 | wait + show |
| 回看 | show 空 BODY | show 有正文 / tail / 片段 |

### 4.4 其它仍成立的 P0/P1（并入本书）

摘自全量复测，**不因 API 拆分而消失**：

| ID | 项 | 优先级 |
|----|-----|--------|
| F-1 | 异常后 daemon 高 CPU + CLI `Access denied`，需强杀 | **P0** |
| F-2 | `conv show` 默认 BODY 空 | **P0** |
| F-3 | Cursor 默认 `delete` 失败，需 `--local-only` 或自动降级 | **P0** |
| F-4 | send 流式仍有 `text ` 碎屑、toolCallId | **P1** |
| F-5 | `--reveal-paths` 在 agent list 未展开 | **P1** |
| F-6 | sessions / list --all 博物馆噪音 | **P1** |
| F-7 | 配置类 CLI 客户端挂起而 daemon 已成功 | **P1** |
| F-8 | **send/wait 未分离**（本书主题） | **P0 设计 / P1 实现可分期** |

---

## 5. 推荐实现分期

### Phase A — 可信回看（先还债）

1. 修复默认 `conv show` 人读 BODY  
2. send 流式去 `text ` / 默认隐藏 toolCallId  
3. Cursor delete 降级或 doctor 强提示  

**验收：** 关掉 send 终端后，仅靠 `conv show` 能读懂上一轮对话。

### Phase B — 语义拆分（兼容默认）

1. `send` 增加 `--no-wait`（默认仍 wait）  
2. 新增 `wait`：attach 当前 run，流式 + final  
3. RPC `hub/conv/wait`  
4. 文档 / doctor / cheatsheet 更新「三原语」  

**验收：**

- A：`send --no-wait` 返回 runId  
- B：`wait` 收到同一 `stopReason`  
- C：B 等待中 C `cancel`，B 得 `cancelled`  
- D：已结束 run 上 `wait --run` 幂等返回终态  

### Phase C — 回看参数完备

1. `show --tail / --from-seq --to-seq / --run / --kinds`  
2. search 命中 → show 片段工作流写进 help  

### Phase D — 稳定性

1. daemon 异常自愈 / 超时 / 拒绝访问根因  
2. 所有写配置 RPC 客户端超时与「可能已成功」提示  

---

## 6. 验收矩阵（设计落地后）

| # | 场景 | 期望 |
|---|------|------|
| V1 | `send --text` 无 flag | 行为 ≡ 今日（兼容） |
| V2 | `send --no-wait` | 立即返回；`busy=running` |
| V3 | 另进程 `wait` | 流式增量；结束得 `stopReason`；`busy=none` |
| V4 | `wait` 无 in-flight | `not_busy`，非挂死 |
| V5 | `wait` 已完成 run | 立即返回历史终态 + 可选消息 |
| V6 | wait 中 `cancel` | wait 结束为 cancelled |
| V7 | `show` 默认 | 完整可读正文 |
| V8 | `show --tail 5` | 仅最近 5 item |
| V9 | `show --from-seq --to-seq` | 片段正确 |
| V10 | 双 send 同 conv | 第二仍 `conversation_busy` |
| V11 | 写盘任务 | 磁盘内容正确（不靠 CLI 自报） |
| V12 | daemon 杀后恢复 | doctor / 重连策略可描述 |

---

## 7. 明确**不要**做的设计

1. **不要**把 wait 做成「轮询 show 直到 status 变」的脚本糖而不接 finalize/stopReason（会与 ACP 真结束条件漂移）。  
2. **不要**让 show 依赖「必须先 wait 过」——历史必须在 Store 可读。  
3. **不要**用 OMP 进程内 `task`/`yield`/`history://` 类比当 hub 实现——hub 的边界是 **外置 ACP 进程 + 本地投影**。  
4. **不要**在 v1 用裸 `agent_session_id` 绕过 conv 投影做 wait。  
5. **不要**再把「脚本 40s exit0」写成完整体验结论。

---

## 8. 对「先前实现者」的公允评价

| 做对的 | 代价 |
|--------|------|
| 用 ACP final response 驱动 finalize_run | API 只有 send 能用到这条正确路径 |
| busy 门闩防双飞 | 没有旁路 wait，门闩只服务「第二个 send」 |
| send 流式改善操作手感 | 流式与投递耦合；show 未同等用力 |
| cancel 已拆出 | 证明团队知道「控制面可正交」——**wait/show 应同样拆** |

结论：**协议理解大体正确；操作原语切分不足。**  
本书要求的是 **切分补齐**，不是推翻 ACP 等待语义。

---

## 9. 总评与优先级叙事

### 9.1 今日产品定位（复测后）

**认真 prerelease / 高级用户 workbench。**  
快乐路径：register → probe → create → send（多轮）→ cancel → close → local delete **可用**。  
缺口：回看、旁路守听、异常自愈、Cursor delete 默认路径。

### 9.2 正确设计一句话

> **Send 投递，Wait 守听，Show 回看；默认 send=send+wait 仅为便捷，不得取消正交性。**

### 9.3 建议对外叙述（对维护者）

1. 承认 send/wait 粘连是 **API 造型债**，不是用户理解错。  
2. 全量复测证明 **等 final response 的底层对**，拆 wait 可行。  
3. **show BODY** 与 **daemon 自愈** 与 API 拆分并列 P0，否则 wait 也只是「多一个挂着的空终端」。  
4. 落地顺序：A 回看 → B wait 拆分 → C 片段参数 → D 稳定性。

---

## 10. 附录

### A. 现状命令 vs 目标

| 现状 | 目标 |
|------|------|
| `send`（唯一阻塞守听） | `send` [--no-wait] + `wait` |
| `cancel` | 保持 |
| `conv show` [--json][--raw] | + tail/range/run/kinds；默认有 BODY |
| `search` | 保持；与 show 片段衔接 |
| （无） | `wait` / 可选 `follow` |

### B. 状态字段速查

```text
in-flight:  busy ∈ {running, cancelling}
done:       busy = none
outcome:    last_outcome / status ∈ {completed, cancelled, failed, …}
```

### C. 证据索引

| 项 | 位置 |
|----|------|
| 全量 journal | `tmp/acp-full-ux-20260725-154653/` |
| Access denied | `…/journal/FINDING-daemon-access-denied.txt` |
| 写盘 marker | `…/work/full-ux-marker.txt` |
| send 实现 | `crates/hub/src/hub/prompt.rs` |
| ACP prompt | `crates/hub/src/acp.rs` `send_prompt` |
| CLI send | `crates/cli/src/commands.rs` `handle_send` |
| Spec S3 | `doc/dev/spec.md` |

### D. 修订记录

| 日期 | 说明 |
|------|------|
| 2026-07-25 | 初版：基于 rc.4 全量复测 + 用户对 send/wait/show 分离的设计意见 |

---

**本书结论再压一行：**

发送与等待本应是两个操作；今天的 hub 把正确的 ACP「等 final response」锁在了 `send` 集成路径里。  
补上 **独立 wait（持续接收）** 与 **可靠 show（完整/最近/片段）**，才是 Conductor 该有的形状——底层状态机已经大半就绪，欠的是原语切开与回看还债。
)
