# ACP Hub CLI UX 完整体验与统一反馈（2026-07-24）

**版本：** `acp-hub 0.2.1-rc.2`  
**主机：** Windows  
**体验方式：** 独立 `--home` + 全命令走查 + Cursor agent 实写；产物 `tmp/acp-ux-final/`  
**关联：**

- [cursor-adapter/e2e-investigation-2026-07-24.md](./cursor-adapter/e2e-investigation-2026-07-24.md)
- [cursor-adapter/regression-feedback-2026-07-24.md](./cursor-adapter/regression-feedback-2026-07-24.md)

本文是**操作者视角**的 UX 手感与问题统一反馈，覆盖命令结构、输出形态、错误文案、状态一致性与 Cursor 路径上的摩擦，不替代 API/协议规格。

---

## 1. 总体感受（先说人话）

| 维度 | 感受 | 分数（主观 1–5） |
|------|------|------------------|
| 命令发现 / help | 清晰，clap 标准，子命令边界合理 | **4** |
| 成功路径的简短反馈 | `registered agent`、`set model=…`、`deleted conversation` 干净 | **4** |
| 表格 list 可读性 | 有对齐表，但 **过宽**、**TITLE 被截断**、多会话时噪音大 | **2.5** |
| JSON 模式 | 机器友好；人类偶发要用 | **4**（机器）/ **2.5**（人） |
| 错误文案 | 常见错误可用；部分 **过技术** / **能力缺失不友好** | **3** |
| `send` 流式输出 | 能干活，但像 **ACP 调试日志**，不像终端 agent | **2** |
| `conv show` 消息体 | 碎片化、`content type text text` 噪音 | **2** |
| 状态一致性 | 成功时 `completed` 好；失败后可能 **卡在 `running`** | **2.5** |
| 信任感 / 可预期性 | 主路径常可；`agent sessions`、部分 send 仍会 **daemon closed** | **3** |
| 端到端「想用 Cursor 干活」 | 能成，但要懂 home/权限/超时/脱敏 | **3.5** |

**一句话：**  
Hub 作为 **conductor/CLI 工具箱** 结构清楚、机器可脚本化；作为 **日常人类交互的 TUI 替代品** 还偏「工程师调试台」——能用，但不舒服，状态与会话列表有时会吓人。

---

## 2. 体验路径（做了什么）

独立 home：`tmp/acp-ux-final/hub-home`  
工作区：`tmp/acp-ux-final/work`  
Agent：发布包 Cursor adapter + 本机 cursor-agent  

| 步骤 | 命令族 | 结果摘要 |
|------|--------|----------|
| 版本/帮助 | `--version` / `--help` | 清晰 |
| 空列表 | `agent list` | `No agents registered.` 好 |
| 注册 | `agent add cursor … --allow-root <work>` | `registered agent cursor` 好 |
| 列表 | `agent list` / `--json` | 表格式可用；路径 **redacted** |
| 检查 | `agent inspect cursor` | config 可见；**agentInfo/capabilities 空** |
| 会话枚举 | `agent sessions cursor` | **~30s 后** `daemon closed the connection` |
| 缺失 agent | `inspect missing` | `Error: agent not found: missing` 好 |
| 空代理 | `proxy list` | `No proxies registered.` 好 |
| 建会话 | `conv create cursor --json` | JSON 干净，~6s |
| 参数/模式 | `param list/set`、`mode list/set` | set 确认语好；list 是 JSON 数组/对象 |
| 写文件 send | `send … Create file ux1.txt…` | **CLI 报 daemon closed**；磁盘 **已有 `UX1-OK`** |
| show | `conv show` | status **`running`**（错误后卡住）；消息体碎片 |
| list/search | `conv list` / `search` | 表宽、会话很多；search snippet 半 raw |
| cancel | `cancel <conv>` | 对仍 running 的 conv 可请求取消 |
| 错误 send | `send not-a-conv` | `conversation not found` 好 |
| close | `conv close` | **不支持**（缺 `session_capabilities.close`） |
| delete | `conv delete --local-only` | 成功 |
| ask send | mode=ask + 精确回复 | **成功**，流仍碎但终点清晰 |

脚本：`tmp/acp-ux-walk.ps1`；摘要：`tmp/acp-ux-final/summary.json`。

---

## 3. 分命令 UX 点评

### 3.1 顶层

**好：**

- 子命令划分清楚：`agent` / `proxy` / `conv` / `send` / `param` / `mode` / `cancel` / `search` / `mcp` / `serve`。
- 全局 `--home` 对多环境/测试友好。
- `--help` / `-V` 标准。

**问题：**

| ID | 问题 | 严重度 |
|----|------|--------|
| UX-TOP-1 | 无「快速上手」示例（register → create → send 三行）印在 `--help` 底部 | 低 |
| UX-TOP-2 | 无 `acp-hub status` / `doctor`（daemon 是否活、home 路径、版本、adapter 健康） | 中 |
| UX-TOP-3 | 操作者看不到 daemon 已启动；失败时才出现 `daemon unavailable` | 中 |

### 3.2 `agent`

| 命令 | 感受 |
|------|------|
| `list` 空 | 文案好：`No agents registered.` |
| `add` 成功 | 一句 `registered agent cursor` 足够 |
| `list` 表 | `TARGET` 全是 `<redacted-command> <1 argument(s)>`，**排障几乎无用** |
| `list --json` | 结构好，但 command/args 同样 redacted |
| `inspect` | 人读是一整块 JSON；`agentInfo`/`capabilities` 恒 null 时像半残页面 |
| `sessions` | **差**：触发 Cursor 枚举后易 **~30s + daemon closed**；且 list 中突然出现大量 idle conv（像导入/投影副作用） |
| `inspect missing` | 错误清晰 |

**问题：**

| ID | 问题 | 严重度 |
|----|------|--------|
| UX-AGT-1 | 路径脱敏过度，local trusted 默认也应可显示完整 command（或 `--reveal`） | 中 |
| UX-AGT-2 | inspect 无「冷启动能力探测」；`cachePopulated: false` 无下一步指引 | 中 |
| UX-AGT-3 | `agent sessions` 不稳 + 可能污染/膨胀 Hub conv 列表 | **高** |
| UX-AGT-4 | `add` 的 `<ID>` 参数 help 无说明（命名约定、是否覆盖） | 低 |
| UX-AGT-5 | `--allow-read` 等必须写 `true`/`false` 若出现 flag；与「默认 true」组合易踩 clap | 低 |

### 3.3 `conv`

| 命令 | 感受 |
|------|------|
| `create --json` | 机器友好；人类缺「下一步：send …」提示 |
| `list` | 多列宽表，TITLE 截断；**STATUS** 在失败后可能长期 `running` |
| `show` | 头信息表好看；**SEQ 消息体像协议 dump** |
| `close` | Cursor 上直接失败，文案偏协议：`requires session_capabilities.close` |
| `delete` | 成功文案好；`--local-only` 语义需文档 |

**问题：**

| ID | 问题 | 严重度 |
|----|------|--------|
| UX-CNV-1 | show 消息：`content type text text`、按 token/片断分行，人类难读 | **高** |
| UX-CNV-2 | turn 异常结束后 status 可卡在 **`running`**（本轮 send 断连后实测） | **高** |
| UX-CNV-3 | close 对 Cursor 不可用时，应给「请用 delete --local-only」类可操作建议 | 中 |
| UX-CNV-4 | list 无默认分页/只显示最近 N 条；跨测试 home 或 sessions 导入后刷屏 | 中 |
| UX-CNV-5 | TITLE 自动生成有时可用，有时只是截断句，价值不稳定 | 低 |

### 3.4 `send` / `param` / `mode` / `cancel`

| 命令 | 感受 |
|------|------|
| `param list` / `mode list` | JSON 完整，选项描述好（agent/plan/ask） |
| `param set` / `mode set` | 确认句模板好：`set model=… for conv-…` |
| `send` 默认流 | 成功时像日志：`[assistant/thought]` 碎句 + tool_call dump |
| `send --json` | 文档有；默认人类路径未引导 | 
| `send` 失败 | `Error: daemon unavailable: daemon closed the connection` — 技术正确，**不说是否已改文件** |
| `cancel` | 对 running 有 `requested cancellation for conv… run…` 可接受 |

**问题：**

| ID | 问题 | 严重度 |
|----|------|--------|
| UX-SND-1 | 默认 send 输出不是「产品对话流」，是「投影调试流」 | **高** |
| UX-SND-2 | 失败时无「可能已写入工作区」提示；本轮 **CLI 失败但 `ux1.txt` 已是 `UX1-OK`** | **高** |
| UX-SND-3 | 断连后 status 不落到 `failed`/`cancelled`，停在 `running`，与 rc.2「镜像 turn 状态」目标不一致（至少 Cursor 路径） | **高** |
| UX-SND-4 | 无进度感（仅狂刷 thought）；长任务焦虑 | 中 |
| UX-SND-5 | `--json` 与默认流的使用场景未在 help 里对比说明 | 低 |

### 3.5 `search` / `proxy` / `mcp`

| 点 | 感受 |
|----|------|
| search | 能命中；SNIPPET 仍是 raw `type text text …` |
| proxy list 空 | 好 |
| mcp | 本轮未深测交互；作为门面存在合理 |

**问题：** search snippet 需清洗；proxy/mcp 缺「何时需要」的一句话指引。

---

## 4. 本轮体验中的「硬故障」快照

不仅是「不好看」，下列是体验中踩到的**真实故障态**：

| # | 现象 | 用户感受 |
|---|------|----------|
| 1 | `agent sessions cursor` → ~30s → `daemon closed the connection` | 「列一下会话把整个 Hub 弄挂了」 |
| 2 | `send` 写文件 → CLI **Error daemon closed**；磁盘文件 **已正确写出** | 「到底成功没有？」极度不信任 |
| 3 | 随后 `conv show` **status=running**，list 同 | 「任务还在跑」其实已死/半死 |
| 4 | `conv close` → 不支持 close capability | 「关不掉会话」 |
| 5 | `conv list` 冒出大量 idle（含他次测试标题） | 「home 不干净 / sessions 有副作用」的惊吓 |

对照：同版本下 **ask 模式 send 完整成功**（`UX-ASK-OK` + `final: … stop_reason=end_turn`），说明 UX 在「短问答」尚可，在「工具写文件 + sessions」仍脆。

---

## 5. 与功能回归结论的关系

| 来源 | 结论 |
|------|------|
| 扩展功能回归 20/20（同日） | 干净脚本路径上 **能力与可靠性可通过** |
| 本 UX 走查 | **同一版本仍可在真实交互中打出 daemon closed + 状态卡 running** |

**观点：**  
功能回归「能绿」与 UX 走查「仍痛」可并存：  
- 回归用例避开了 `agent sessions`、对 send 使用了更稳的时序/引号；  
- UX 走查按「人会点的顺序」多摸了 sessions / 失败后 show / close。  

统一评价必须两面都写：**主路径可用；边角与失败呈现仍伤信任。**

---

## 6. 统一问题清单（功能 + UX + 产品）

### P0 — 信任 / 正确性呈现

| ID | 标题 | 说明 |
|----|------|------|
| P0-1 | 失败 CLI vs 成功副作用 | send 失败时不提示工作区可能已改；应有「check workspace / last tool result」 |
| P0-2 | 异常结束后 status 卡 `running` | 必须落到 `failed`/`cancelled`，与 Store-first / status 镜像目标一致 |
| P0-3 | `agent sessions` 搞挂 daemon | 命令要么稳，要么快速失败并说明不支持/降级 |

### P1 — 日常可用性

| ID | 标题 | 说明 |
|----|------|------|
| P1-1 | send/show 人类可读模式 | 默认合并 thought、工具一行摘要；`--verbose`/`--wire` 再出 raw |
| P1-2 | inspect 冷数据 | 填充或明确「需 create 后 inspect」 |
| P1-3 | 路径脱敏可开关 | local trusted 默认 reveal 或 `--reveal-paths` |
| P1-4 | close 不可用时的引导 | 指向 delete / 说明 Cursor 无 close capability |
| P1-5 | 冷启时延 | create 数秒～十余秒；文档与默认超时建议 |
| P1-6 | 升级与配置迁移 | 0.2.0 reject 配置、crates 仍 0.2.0 的认知差 |
| P1-7 | idle 会话堆积 | list 膨胀；缺 reaping/close 习惯引导 |

### P2 — 打磨

| ID | 标题 |
|----|------|
| P2-1 | help 附最小教程三行 |
| P2-2 | `doctor`/`status` 子命令 |
| P2-3 | conv list 默认最近 N 条 + `--all` |
| P2-4 | search snippet 清洗 |
| P2-5 | Windows 脚本引号 / allow 标志文档 |
| P2-6 | cancel/close/delete 决策树一张图 |

### P3 — 边界 / 未测

| ID | 标题 |
|----|------|
| P3-1 | 并发 send、mid-turn kill |
| P3-2 | 跨 OS |
| P3-3 | IDE/CLI/ACP 历史统一（设计边界，勿当静默 bug） |

---

## 7. 输出设计原则建议（产品）

若只改一层「人话输出」，建议：

1. **默认人类通道（send/show/search）**  
   - 合并连续 thought 为一块可折叠摘要  
   - 工具：`✓ Edit File path (+N/-M)` 一行  
   - 结束：`done status=completed | failed | cancelled` + 若有文件变更则列表  

2. **调试通道**  
   - `--json` / `--wire` 保持现有投影与 ACP 细节  

3. **错误通道**  
   - 分级：`UserError`（not found）/ `AgentError` / `DaemonError`  
   - DaemonError 附：home 路径、是否建议 `taskkill` 后重试、**不要假设工作区未改**  

4. **状态通道**  
   - 任何 RPC 失败结束 turn 时强制 CAS status 离开 `running`  

---

## 8. 体验评分卡（可贴评审）

| 命令/场景 | 可用性 | 可读性 | 可预期性 | 备注 |
|-----------|--------|--------|----------|------|
| help/version | A | A | A | |
| agent add/list | A | C（脱敏） | B | |
| agent inspect | B | C | C | 空字段 |
| agent sessions | D | — | D | 断连 |
| conv create | A | A(json) | B | 慢 |
| param/mode | A | B | A | |
| send (ask) | A | C | A | 碎但成功 |
| send (write) | B | C | D | 失败却写盘；status running |
| conv show | B | D | C | 碎片 + 状态 |
| conv list | B | C | B | 宽表、多行 |
| conv close (cursor) | D | C | C | 能力缺失 |
| conv delete | A | A | A | |
| search | B | C | B | |
| cancel | B | B | B | |

**综合：工程师可脚本化 B+；人类日常 CLI 体验 C+。**

---

## 9. 统一反馈意见（结论性）

1. **不要再用「能不能驱动 Cursor」当唯一 KPI。** 0.2.1-rc.2 上主路径与回归已证明能；下一阶段 KPI 应是 **失败可理解、状态可信、输出可读**。  

2. **最大信任杀手**不是慢，而是：**命令报错了，文件其实写了，列表还显示 running**。这比 hang 更伤。  

3. **`agent sessions` 在修好前应视为危险命令**（或文档标 experimental + 不保证 daemon 存活）。  

4. **默认输出层需要「产品模式」**，raw 投影留给 `--json`。否则永远像半套 ACP 抓包工具。  

5. **Cursor 能力缺口要翻译成人话**（close 不支持 → 怎么收尾），不要只抛 capability 名。  

6. **脱敏、空 inspect、宽表、会话膨胀** 是同一类「本地 trusted CLI 却按多租户 SaaS 输出」的问题——默认 audience 应是本机开发者。  

7. **功能回归绿 + UX 走查仍见 daemon closed** → 稳定性尚未「关单」；需把 sessions、写工具长 turn、失败状态机纳入必测，而非只测 happy path。  

8. **分发：** 体验结论建立在 rc.2 上；若用户仍在 0.2.0，反馈会完全不同——文档与安装指引必须版本锚定。  

---

## 10. 建议落地顺序（工程）

| 序 | 项 | 预期收益 |
|----|----|----------|
| 1 | turn 失败强制离开 `running` | 恢复 list/show 信任 |
| 2 | send 失败摘要 +「可能已改文件」 | 消除最大认知失调 |
| 3 | send/show 人类可读默认渲染 | 日常可用性跃迁 |
| 4 | 修复或隔离 `agent sessions` | 去掉地雷命令 |
| 5 | close 引导 / delete 文案 | 会话生命周期 |
| 6 | inspect reveal + 冷探测 | 排障 |
| 7 | doctor/status + help 三行教程 | 上手 |
| 8 | 稳定 0.2.1 发布与迁移说明 | 用户对齐版本 |

---

## 11. 证据索引

| 证据 | 路径 |
|------|------|
| UX 走查摘要 | `tmp/acp-ux-final/summary.json` |
| UX 日志 | `tmp/acp-ux-final/ux.log` |
| 写成功但 send 失败 | `tmp/acp-ux-final/work/ux1.txt` = `UX1-OK`；`16_send.err.txt` daemon closed |
| 卡 running | `17_show.out.txt` status=running |
| sessions 断连 | `07_sessions.err.txt` |
| close 不支持 | `23_close.err.txt` |
| ask 成功 | `28_send_ask.out.txt` |
| 功能回归 20/20 | `tmp/acp-regression-cursor-20260724-203801/summary.json` |

---

## 12. 变更记录

| 日期 | 说明 |
|------|------|
| 2026-07-24 | 基于本机对 `0.2.1-rc.2` 的全命令 UX 走查，落盘统一反馈。 |
