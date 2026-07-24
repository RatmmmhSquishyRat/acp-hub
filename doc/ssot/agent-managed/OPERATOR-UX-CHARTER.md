# Operator UX Charter — 使用者正向动线与大 UX 问题登记

**Status:** Agent-managed product law / problem charter（**非**实现规格，**非**半成品功能清单）  
**Date:** 2026-07-24  
**Authority:** frozen `doc/ssot/pillars/*`（只读）+ [pillars/Product-UX.md](./pillars/Product-UX.md) + 本文  
**Audience:** 后续 design / review / 实现 agent  
**用户指示（摘要）：** 下列问题属于 **大型 UX 问题**；必须在 **完整设计使用者正向动线** 之后才能给出功能。当前功能不齐全、语义重叠 → **根本无法当作产品使用**。根因是此前 **没有** 对使用者动线与正向流程做完整设计。

---

## 0. 硬约束（如何做后续工作）

| # | 约束 |
|---|------|
| H1 | **先完整设计，后实现。** 禁止在动线未闭合前堆 CLI 子命令补丁、文档补丁、或「先做一个 gc / 先合并 thought」冒充产品完成。 |
| H2 | **设计对象是使用者正向流程体验**，不是孤立 API 表。必须回答：打开 CLI 后先做什么、后做什么、分支是什么、不同场景动线、如何读懂 CLI 输出并继续操作。 |
| H3 | **设计不得 trivial。** 必须尽可能覆盖真实使用场景（冷启、多 endpoint、只读历史、失败恢复、找会话、续聊、inspect 认知、长任务进度等）。 |
| H4 | **实现前必须经独立 review-rework loop**（对抗性 subagent / design-doc review），对照冻结 pillar + Product-UX + 本文；禁止自证正确。 |
| H5 | **禁止** 把「闲置堆积 / TTL / reaper」当成会话 UX 主问题；主问题是 **能力诚实 + 可发现 + 动线清晰**。 |
| H6 | 冻结 `doc/ssot/pillars/*` **不得** 为本文改写。 |

**当前状态判定（用户已确认的产品判断）：**  
在功能不齐全、语义仍有重叠的情况下，Hub **不能** 被当作完整可用的会话工作台 / 多 endpoint 客户端来宣传或验收。底座（Store-first、默认可用、状态镜像等）是必要条件，**不是** 动线产品完成。

---

## 1. 根因（必须写进设计前言）

### 1.1 根因陈述

**根本原因不是缺某几个 flag，而是：**

> 我们此前没有对 **使用者（操作者 agent + 人类操作者）** 的 **动线** 与 **正向使用流程** 做从头到尾的规范设计。

因此出现了：

- 有 CRUD 级命令，但 **没有「先做什么」的工作台语义**
- 有 `conv list` 与 `agent sessions` **两条发现路径**，语义重叠、对操作者不透明
- 有 capture 保真，但 **没有可读 transcript 的展示契约**
- 有 inspect，但 **默认给不出注册后立刻可用的认知信息**
- 有只读导入路径，但 **不显式只读**，假一等公民
- 有稳定回归绿，但 **没有按正向动线验收**

### 1.2 设计必须回答的动线问题（验收骨架）

完整设计文档 **必须** 非 trivial 地覆盖下列问题（可分场景分章，不可一笔带过）：

| ID | 问题 |
|----|------|
| J1 | 使用者 **打开 CLI / 接入 MCP 后第一步** 是什么？（注册？inspect？list？） |
| J2 | **注册 agent 之后** 立刻应看到什么、应能判断什么（能力 / 权限 / 是否可写工具）？ |
| J3 | **如何发现会话**：Hub 已有投影 vs 某 endpoint 远端 session/list — 何时用哪条？如何不混淆？ |
| J4 | **如何选中目标会话**（running / 最近更新 / 内容摘要 / 读写能力 / agent_id）？ |
| J5 | **如何读懂** `list` / `show` / `send` / 错误输出？字段含义、优先级、下一步动作？ |
| J6 | **主路径发任务**：create → param/mode → send → 看结果 → 再 send；每步失败分支？ |
| J7 | **冷启 / 长阻塞**：进度如何呈现？如何区分 Hub vs agent vs 模型时延？ |
| J8 | **只读会话**：如何识别、如何读历史、如何明确 **不能** send？ |
| J9 | **多 ACP endpoint**：如何分别/统一查看？跨 agent 找对话的动线？ |
| J10 | **失败与恢复**：daemon 挂、load 失败、run failed、cancel — 操作者下一步是什么？ |
| J11 | **关闭与删除**：何时 close、何时 delete、local_only 含义；与「找会话」的关系？ |
| J12 | **搜索**：何时用 search 代替 list；snippet 如何读？ |

**正向流程** = 成功路径 + 主要失败/分支的 **可执行下一步**，而不是仅列子命令名。

---

## 2. 大型 UX 问题登记（Problem Register）

下列问题 **均为大型 UX / 产品信息架构问题**。  
**裁定：REAL / 需完整设计后实现。** 不可在未闭合动线前当「小补丁」关掉。

### P-UX-01 — 缺少使用者正向动线（总根因）

| 字段 | 内容 |
|------|------|
| **现象** | 命令存在，但操作者不知道标准顺序；场景分支未规范；输出如何阅读未定义 |
| **为何无法使用** | 无「打开 → 工作 → 找到会话 → 行动 → 理解结果」闭环；只能靠试错 |
| **设计前置** | 完整 journey map（§1.2 J1–J12）+ 场景矩阵 + CLI/MCP 信息架构 |
| **禁止** | 只加命令不写动线；只写 README 列表冒充 journey |

### P-UX-02 — 会话管理不完整 + 语义重叠

| 字段 | 内容 |
|------|------|
| **现象** | `conv *` 管 Hub 投影；`agent sessions` 管远端 list+import；能力边界与先后顺序对使用者不清晰 |
| **现状能力** | create / list(--agent) / show / send / cancel / close / delete / search / param / mode — **CRUD 级**，非工作台 |
| **缺口** | 无完整「找会话」契约；无 rename/pin/归档；无 interaction 标签；无统一跨 endpoint 发现体验；title/摘要弱 |
| **为何无法使用** | 使用者 agent 无法稳定 **定位目标 session**；双路径语义重叠导致误用 |
| **设计前置** | Session 信息模型 + 发现动线 + conv vs agent-sessions 职责单一化 |
| **纠正** | **闲置堆积不是主问题**；主问题是 **管理语义清晰 + 可发现** |

### P-UX-03 — 多 ACP endpoint 对话查看动线未产品化

| 字段 | 内容 |
|------|------|
| **现象** | 技术上：`conv list [--agent]` 看 Hub 投影；`agent sessions <id>` 按 endpoint 拉远端；`conv show` 读 Store |
| **缺口** | 无统一「多 endpoint 工作台」叙事；操作者 agent 难一眼跨 agent 找会话；导入后与可写会话混列时无能力标签 |
| **为何无法使用** | 多 endpoint 是 hub 核心价值，但 **查看/选型动线未设计** |
| **设计前置** | 跨 endpoint 列表/过滤/排序/预览契约；与 import 的时机 |

### P-UX-04 — 不能修改却不显式只读（假一等公民）

| 字段 | 内容 |
|------|------|
| **现象** | 例如 Cursor adapter 将 IDE 等只读空间并入 `session/list`，prompt 才拒绝 |
| **归属** | **我们的产品设计**（adapter + hub 展示），不是「Cursor 官方拒绝所以无关」 |
| **为何无法使用** | list 像可写 → send 爆炸；破坏信任与自动化 |
| **设计前置** | interaction=`writable`\|`read_only` 一等字段；选型期可见；OMP 式能力绑定 |
| **参考** | Product-UX §6.1；OMP registry / subagent 类型与能力差异 |

### P-UX-05 — 注册后认知失败（inspect 空洞）

| 字段 | 内容 |
|------|------|
| **现象** | `agent inspect` cache-only；add 不写 cache；create 前 capabilities/agentInfo 空 |
| **为何无法使用** | 动线第一步就无法判断「这 agent 能不能干活」 |
| **设计前置** | 注册后 / inspect 默认如何获得协商结果（probe 时机、字段、失败文案） |

### P-UX-06 — 冷路径与长任务无进度、无分段可读时延

| 字段 | 内容 |
|------|------|
| **现象** | create/send 长阻塞静默；无阶段进度；难区分 Hub vs cursor-agent vs 模型 |
| **为何无法使用** | 操作者无法判断卡死 vs 正常；自动化难设超时与重试 |
| **设计前置** | 进度事件模型 + CLI 呈现 + 可选 metrics |

### P-UX-07 — Transcript / 投影展示不可读（含 thinking 碎片）

| 字段 | 内容 |
|------|------|
| **现象** | Store 1:1 capture 合理；CLI show/send 不合并 thought、不显示 kind、body 噪声 |
| **为何无法使用** | 做完任务也读不懂发生了什么；违背「看结果再续聊」 |
| **设计前置** | 展示层 view model（合并/折叠/kind）；与 Store-first 分离 |

### P-UX-08 — 错误与状态如何阅读未规范

| 字段 | 内容 |
|------|------|
| **现象** | 部分错误已分层；conversation status 已与 run 对齐（failed 等）；但 **操作者读完下一步做什么** 未成体系 |
| **设计前置** | 错误 → 动作 决策表；busy / failed / completed 在动线中的含义 |

### P-UX-09 — 分发与升级陷阱（ops，仍影响动线）

| 字段 | 内容 |
|------|------|
| **现象** | 稳定版滞后、旧 agents.json 不迁移、路径脱敏排障难 |
| **为何影响使用** | 动线在「装上就能用 / 升级后仍能用」处断裂 |
| **设计前置** | 版本与迁移在 onboarding 动线中的位置 |

### P-UX-10 — 并发 / 中途杀等场景未纳入动线

| 字段 | 内容 |
|------|------|
| **现象** | 矩阵未充分验收；产品是否支持、失败时如何呈现未写 |
| **设计前置** | 场景矩阵中的 supported / unsupported / degraded |

---

## 3. 已具备的底座（设计时不要推倒重来，但不要误当完成）

下列是 **必要条件**，**不等于** Operator UX 完成：

| 底座 | 说明 |
|------|------|
| 多 endpoint 注册 | agent add / list / remove |
| Store-first 双层投影 | Layer1 load_replay + Layer2 local_turn |
| 基本 conv CRUD + send/search | 见 P-UX-02 表 |
| 默认可用（auto-allow 等） | 0.2.1-rc 线 |
| lag 不杀连接 | live ≠ durable |
| run 与 conv 终态镜像 | failed/cancelled/completed |

设计应 **建立在这些底座上补齐动线与信息架构**，而不是再发明第二套存储。

---

## 4. 场景矩阵（设计必须覆盖；可增不可空）

设计文档需为每场景写：**触发 → 步骤 → 可见输出 → 成功标准 → 失败分支 → 下一步**。

| 场景 ID | 场景 | 最低覆盖点 |
|---------|------|------------|
| SC-01 | 全新安装，第一次注册 Cursor（或任意 stdio agent） | J1 J2 J6 J9 |
| SC-02 | 注册后立刻 inspect / 判断能否写仓库 | J2 P-UX-05 |
| SC-03 | 冷 create + 首次 send（长等待） | J6 J7 P-UX-06 |
| SC-04 | 同会话续聊 | J6 J5 |
| SC-05 | 多 agent 已注册，找「刚才那个」会话 | J3 J4 P-UX-02/03 |
| SC-06 | 从某 endpoint 发现历史并只读查看 | J3 J8 P-UX-04 |
| SC-07 | 误对只读会话 send | J8 明确拒绝与文案 |
| SC-08 | daemon 被杀后恢复再 send | J10 |
| SC-09 | run 失败后读状态再决策 | J5 J10 |
| SC-10 | search 定位旧任务 | J12 |
| SC-11 | close vs delete 选择 | J11 |
| SC-12 | 升级后旧 reject 配置 | P-UX-09 |
| SC-13 | 高 churn thought 的可读结果 | P-UX-07 |
| SC-14 | MCP 使用者 agent 等价动线 | 与 CLI 同构的 tool 语义 |

---

## 5. 与 OMP 的对照原则（设计输入，非抄运行时）

| OMP 手感 | 对 Hub 设计的含义 |
|----------|-------------------|
| Registry 状态清晰（running/idle/parked…） | list/status 服务「谁在干活」 |
| 类型绑定能力（explore 只读 vs 全功能） | 只读必须显式；不可伪装 |
| Task 完成与可续 | send 终态诚实；续聊路径单一 |
| 进度可见 | 冷路径与 turn 不可静默 |
| Load ≠ resume 语义分清 | 发现/导入/续活动词不混用 |
| 结果可寻址 | conv_id / 摘要 / show 可读 |

**不** 把 OMP task 运行时搬进 CoreHub；**要** 借操作模型把动线做清楚。

---

## 6. 交付顺序（强制）

```
1) 完整 UX 系统评估/功能规范/动线设计（见 OPERATOR-UX-SYSTEM.md）— 不仅 journey 提纲
2) 独立 design review-rework loop 至共识
3) 按 SYSTEM 分期切 PR 实现（语义 → 发现/只读 → 展示 → 进度 → …）
4) 每 PR 独立 review + 测试
5) 按场景矩阵做 Cursor/多 agent 回归（正向流程验收，不单测命令存在）
6) 发版说明面向操作者动线，不面向内部重构叙事
```

**禁止跳步。** 未完成 (1)(2) 不得宣称 P-UX-* 已解决。

系统级设计正文：**[OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md)**（As-Is 评估、概念模型、F-* 功能目录、IA、动线、分期、门禁）。

---

## 7. 关联文档

| 文档 | 关系 |
|------|------|
| [pillars/Product-UX.md](./pillars/Product-UX.md) | UX 主次、Store-first、§6 会话能力诚实与可发现 |
| **[OPERATOR-UX-SYSTEM.md](./OPERATOR-UX-SYSTEM.md)** | **从零系统评估 + 功能规范化 + 动线 + 分期（结束混乱）** |
| [COMPLIANCE.md](./COMPLIANCE.md) | 底座合规（≠ 动线完成） |
| [PLAN.md](./PLAN.md) | 旧 overlay 清单已闭合；**动线大 UX 是新工作流** |
| `doc/dev/cursor-adapter/qa-investigation-2026-07-24.md` | QA 核实与 §2.5 纠正 |
| `doc/dev/cursor-adapter/regression-feedback-2026-07-24.md` | 原始 QA 输入 |
| `doc/research/omp-vs-acp-hub-2026-07-24/` | OMP 参考（历史包需注意 supersession） |

---

## 8. 变更记录

| Date | Note |
|------|------|
| 2026-07-24 | 初版：用户要求妥善记录大型 UX 问题、根因（无正向动线设计）、须完整设计后方可实现、场景与禁止项。 |
