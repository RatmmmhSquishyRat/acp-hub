# Product UX Pillar — ACP Hub CLI

**Status:** Agent-managed pillar (2026-07-24) — **not** a frozen user SSOT file  
**Tree:** `doc/ssot/agent-managed/` (see [../README.md](../README.md))  
**Frozen baseline (do not edit):** `doc/ssot/pillars/README.md`, `TechSel.md`  
**Intent:** 在不改写冻结 pillar 的前提下，记录用户于 2026-07-24 纠正的产品主次：可用性优先于 fail-closed / down-privilege 默认；锁定 hub CLI 的操作验收标准。

---

## 1. 产品一句话

**acp-hub 是可日常使用的 ACP 客户端设施**：用同一套 CLI/MCP/库入口注册任意 agent、管理对话、发任务、设参、搜索历史 — 体验上应能 **替代客户端内嵌 ACP**，并在操作模型上 **可平替** OMP 式 subagent/task 工作流（注册 → 开对话 → 设参 → 发任务 → 看结果 → 续聊），而不是一个默认拒权、动辄断连的 fail-closed 调试器。

### 1.1 完成定义（用户 2026-07-24 纠正）

**「能跑通命令」≠ 产品可用。**  
若 **使用者正向动线** 未完整设计、会话语义重叠/发现面不合格、只读不显式、输出不可读——则在真实场景下 **根本无法当作完整产品使用**。

- **大型 UX 问题登记：** 见 [../OPERATOR-UX-CHARTER.md](../OPERATOR-UX-CHARTER.md)  
- **从零系统评估 / 功能规范 F-* / 动线 / 分期：** 见 [../OPERATOR-UX-SYSTEM.md](../OPERATOR-UX-SYSTEM.md)（结束功能混乱的 SSOT）  
- **根因：** 此前未对使用者动线与 UX 功能体系做完整设计  
- **法：** 先闭合 SYSTEM 设计与 review，再按 Phase 实现；禁止补丁冒充完成  

---

## 2. 主次顺序（硬约束）

| 优先级 | 目标 | 说明 |
|--------|------|------|
| P0 | **完整可用主路径** | `agent add` → `conv create` → `param`/`mode`（可选）→ `send` → 看结果 → 再 `send` 连续成功 |
| P0 | **流畅手感** | 不无故 hang；错误可理解可行动；高 churn agent 不把成功 turn 报成失败 |
| P0 | **使用者正向动线** | 打开后步骤、场景分支、输出如何读、多 endpoint/只读/找会话 — 见 OPERATOR-UX-CHARTER；**未设计不得宣称可用** |
| P1 | **可平替 subagent 操作感** | 多 agent 注册、并行/串行任务、结果可检索、会话可续，对标 OMP task 给操作者的清晰感 |
| P2 | **安全与资源边界** | 路径 roots、字节上限、隐私 redaction、可选收紧策略 — **不得挡 P0** |
| P3 | **审查/形式正确性** | 投影完整性、typed errors — 用缓冲/聚合/明确降级实现，**禁止**用「杀连接 / 默认 reject」当首选 |

**冲突裁决：** P0 > P2 > 任何审查 finding。若 R-\*/F-\* 与本 pillar 冲突，**改实现与文档以符合本 pillar**，而不是保留审查时的防御默认。

---

## 3. 默认配置哲学

### 3.1 本地信任默认（Default = usable）

Hub 默认运行模型是 **本机操作者信任自己注册的 agent**（与「本机装 omp/cursor 并让它改仓库」一致）。

因此默认应：

| 项 | 默认 | 收紧方式（显式） |
|----|------|------------------|
| `permission_policy` | **auto-allow** | `--permission-policy reject` / `auto-cancel` |
| `fs.read_text_file` / `write_text_file` | **true** | 关掉对应 allow 旗标 |
| `terminal` | **true** | `--no-terminal` 或等价 |
| `allowed_roots` | **会话 cwd**（及 CLI 可扩展） | `--allow-root` 收窄/扩展 |

「更安全」是 **高级模式**，不是开箱默认。

### 3.2 安全仍做什么

- 绝对路径校验、roots 外拒绝读写（当 roots 配置时）
- RPC/资源上限、隐私 redaction（ordinary inspect 不泄密钥路径）
- 能力不足 → typed error（不静默假成功）
- 不可恢复协议错误 → 明确失败

安全 **不做什么**：

- 默认拒一切 permission
- 默认不声明 fs/terminal 导致 agent 无法工具工作
- 为「可能漏通知」而中断已在进行的成功 turn

---

## 4. 对标 OMP 的 UX 验收（操作者视角）

下列是 **体验验收**，不是要求 hub 内嵌 OMP task 运行时：

| OMP 手感 | Hub 对应验收 |
|----------|--------------|
| 装好就能干活 | 示例注册 + 无 flag 或最少 flag 即可写文件 send |
| 显式 yolo / 配置层清晰 | 默认已可用；收紧有文档与 flag |
| 参数/模式可查可设 | `param list/set`、`mode list/set` 不 hang、错误真实 |
| 任务能完成并回报 | `send` 退出 0 当 agent 正常结束；文件/投影可见 |
| 可续聊 / resume | create 后连续 send；daemon 重启后可恢复或明确可重试错误 |
| 失败可懂 | 文案区分 daemon / agent / permission / load |
| 子 agent 不挡主路径 | 多 endpoint 互不污染；单 conversation single-flight 有清晰 busy 错误 |

---

## 5. 对话所有权：Store-first（相对 R-DAEMON-004 / 「模糊 turn」的纠正）

### 5.1 产品法（硬）

| 层 | 角色 | 失败语义 |
|----|------|----------|
| **Store（SQLite 投影）** | Hub **自有的** 持久对话真相：Layer1 `load_replay` + Layer2 `local_turn`、run 状态、快照 | capture 写库失败 → 该更新失败；**不**用 live 流代替真相 |
| **Live fan-out（`hub/conv/update`）** | 尽力而为的实时展示（CLI 流） | lag / 丢包 → **只**丢实时画面；**不**表示 Store 不完整，**不**强迫 operator「resync」，**更不**强迫外部 agent 刷新重放 |
| **Agent 原会话** | Layer1 的远端来源；经 Hub 发起的 `session/load` / resume 导入 | 失败由 Hub 记录与回滚；操作者用 `conv show` 读 **Hub Store**，不是去「刷新 agent 看一遍」 |

**Hub 完整管理 agent 的持久化对话投影**——正常完成、错误、中途截断（agent 断流 / capture 预算 / 进程退出）一律由 Hub 在 Store 中落成可查询状态。Turn 管理不得模糊成「live 漏了几条通知 = 对话丢了」。

### 5.2 与 R-DAEMON-004 的关系

- 历史审查曾把 **receiver lag** 当作 **不可检测的投影残缺**，并默认 **connection-fatal**。那是把 **live 信道**误当成 **durable 真相**。
- 当前法：lag **不**杀连接、**不**中断 in-flight RPC；日志只描述 **live stream gap**。
- Durable 完整性靠 **capture → Store 先写 → 再 notify**；CLI 需要完整历史时读 Store（`conv show` / search），这是 **Hub 自有读路径**，不是「operator resync 修复不完整投影」叙事。

### 5.3 禁止的产品语言

- ❌「projection may be incomplete until resync」指代 **lag 后的 Store**
- ❌「force agent refresh / 让 agent 再刷一遍」作为 live lag 的修复手段
- ❌ 用 live 漏包衡量 turn 是否成功

### 5.4 仍允许的 Hub 自管操作

- Hub 发起的 `session/load` / resume 刷新 Layer1（带 rollback）
- run 终态 CAS、cancel、crash recovery
- capture 失败记录、预算上限、明确错误

---

## 6. 使用者 agent 的会话 UX（硬：能力诚实 + 可发现）

**主用户不是人类 CLI 考古，而是操作者 agent**（OMP/Grok/Cursor 主会话通过 hub CLI/MCP 找对话、派任务、读结果）。  
对标 OMP：registry 状态机（`running` / `idle` / `parked` / `aborted`）、子 agent 类型与能力差异（`explore`/`plan` 只读工具 vs `general-purpose`）、advisor 等 **不可消息** 类型不伪装成可派活实例——**能力与状态必须一眼可读**。

### 6.1 能力诚实：不能改 = 显式只读

| 规则 | 说明 |
|------|------|
| **不能 prompt / 不能修改** 的会话 | 必须在 list / show / 选择路径上 **显式标为 read-only**（或等价：`interaction=read_only` / `can_prompt=false`），**禁止** 伪装成与可写 ACP 会话同一档「普通 session」 |
| **拒绝动作的时机** | 在 **选型/列表** 阶段就应可见，而不是 list 像普通货、`send` 才炸 |
| **归属** | 这是 **Hub + adapter 的产品设计**（例如 Cursor adapter 把 IDE 空间塞进 `session/list` 却在 prompt 拒绝）——**不是**「Cursor 官方拒绝所以与我们无关」 |
| **OMP 类比** | 只读/观测型 worker **类型不同、能力不同、界面不同**；不会把不可续聊对象默认当成可 `task` 的实例 |

允许：只读 **发现 / load 回放 / 搜索**。  
禁止：用「普通 session」信息架构承载只读历史。

### 6.2 会话管理的真正目标：找到对的 session

**闲置会话堆积本身不是问题。**  
有很多 conversation/session 是正常的（create 留档、多任务并行、历史导入）。

**问题是语义不清晰、发现路径不合格**——使用者 agent 必须 **一眼** 能回答：

| 问题 | 列表/详情应给出的信号 |
|------|------------------------|
| **谁在跑？** | `running` / 有 active run 的会话优先、可过滤 |
| **最近动过谁？** | `updated_at` 排序默认合理；最近更新置顶或可排序 |
| **这是什么？** | 可读 **title / 摘要 / 最近用户意图或 last turn 预览**（内容大致是什么） |
| **能干什么？** | `writable` vs `read_only`（及 agent/space 来源）一眼可见 |
| **我要哪条？** | 上述字段 + search 共同服务 **定位目标 session**，不是逼操作者扫一堆 UUID |

**生命周期策略（TTL/gc）是可选运维**，不是会话 UX 的主目标。  
主目标永远是：**服务于使用者 agent 找到并正确使用自己需要的 session**。

### 6.3 当前缺口（对照，非实现）

| 面 | 现状粗判 | 与本条关系 |
|----|----------|------------|
| `conv list` | id / agent / status / title / updated | 有 status/updated/title 骨架；缺 **interaction 能力**、缺 **摘要/预览**、缺 **running 优先语义** 的产品契约 |
| Cursor IDE 等只读源 | list 可进、prompt 才拒 | **违反 6.1** |
| 堆积/reaper | 曾误当 P1 问题 | **纠正**：非主问题；主问题是 **可发现性与能力标签** |

---

## 7. 错误与可观测性

- `ResumeLoadFailed` 的 source 在 CLI 展示层必须保留可区分原因（agent ACP 错误、unsupported、timeout），**禁止** 统一显示为 `daemon unavailable: resume/load operation failed` 误导操作者。
- Hang 必须有超时或进度；`agent add` 不得在 agents.json 已写后无限无响应。

---

## 8. 非目标（仍成立）

- Hub **不是** 内嵌 LLM 产品；task/subagent 图仍由 endpoint（如 OMP）实现。
- Hub **不** 解析厂商私有 DB（adapters 边界仍成立）。
- Hub **不** 做多租户远程 SaaS。

「可平替 subagents」指 **操作模型与顺滑度**，不是把 OMP `task` 工具搬进 CoreHub。

---

## 9. 对下游文档的要求

本 **agent-managed** pillar 生效后，agent 应审查并更新（**不得**为此改写冻结 `doc/ssot/pillars/*`）：

- `doc/dev/*` 中与 least-privilege 默认、lag-fatal 相关的条款（agent/dev 文档）
- 会话 list/show/import 的 **能力标签与发现字段**（对齐 §6）
- `adapters/*/agents.json` 与 README 示例（含只读空间不得伪装可写）
- `crates/cli` 默认 flag
- `SECURITY.md` / README 中「samples reject by default」表述
- 历史 research 中「fail-closed conductor」表述标注为 **已被本 overlay 纠正**

**实现裁决：** 用户已明确要求 UX 优先时，实现遵循本文件；冻结 pillar 仍描述「能做什么」，本文件描述「默认怎么好用 / 主次如何排」。  
旧 review book 的「deny-by-default samples」**不再作为 agent 实现的默认目标**。
