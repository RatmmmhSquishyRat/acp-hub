# Operator UX System Design — ACP Hub

**Status:** Agent-managed system design **v0.3**（政策层 + 动线 + 分期；**实现以 Phase 合同为准**）  
**Date:** 2026-07-24  
**Refine history:** v0.1 skeleton → v0.2 R1–R8 → v0.2.1 P0-1/A → **三方 review REJECT/NO-GO** → v0.3 闭合 wire/动线/门闩 + [PHASE1-CONTRACT](./OPERATOR-UX-PHASE1-CONTRACT.md)  
**Authority:** frozen pillars（只读）→ [Product-UX](./pillars/Product-UX.md) → [CHARTER](./OPERATOR-UX-CHARTER.md) → **本文** → **Phase N 合同**

| 层 | 文档 | 可否开工 |
|----|------|----------|
| 政策 / 对象 / F-* / 动线总册 | **本文 v0.3** | 理解产品 |
| Phase 1 可实现契约 | [OPERATOR-UX-PHASE1-CONTRACT.md](./OPERATOR-UX-PHASE1-CONTRACT.md) **v1.2 APPROVED** | **是 — Phase1 only** |
| Phase 2+ | 本文 §F/§G 规范 + 开写前补 Phase2-CONTRACT | 未单独冻结前勿实现展示层细节外的发明 |

**诚实声明：** v0.2.x **未**达到「完整设计=可实现」。v0.3 将 Phase1 降到可实现合同；全产品完成仍按 §K 分期，**禁止**宣称 M1–M6 已满足。

---

## A. 执行摘要

| 判断 | |
|------|--|
| Hub | 通用 ACP client/conductor（多 endpoint、双层历史、注册/对话/收发/搜索） |
| 主用户 | **操作者 agent**（CLI + MCP 同构 JSON） |
| As-Is | 底座可用；工作台/发现/可读性混乱 → **不可当完整产品** |
| 根因 | 无正向动线倒推的信息架构与 feature 边界 |
| 结束混乱 | Conversation 工作台 + Option A + 唯一 bind + F-* + **默认 workbench list** + Phase 合同 |
| Phase1 开工 | **仅**当实现严格遵循 PHASE1-CONTRACT（schema/meta/discover/bind/errors/list/SC） |

---

## B. As-Is（摘要）

Chaos：sessions 与 conv list 重叠且 discover 曾 **load 全量**；inspect 空；无进度；transcript 碎片；只读假一等；默认 list 易洪水；无场景验收。  
底座保留：Store-first、默认 auto-allow、lag 非致命、run 终态。

---

## C. 产品定义

**完成定义：** 操作者 agent 按 §G runbook 可完成 SC 主路径，输出字段与错误码符合合同，无需发明命令语义。

**非目标：** 内嵌 LLM/task 运行时、SaaS、fail-closed 挡主路径、TTL 当主 UX、静默 session/new、core 解析私有 DB。

---

## D. 概念模型与闭合决策

### D.1 对象

`AgentEndpoint` · `Conversation`（日常主键 `conv_id`/`id`）· `Run` · discover DTO（RemoteSessionRef 视图）

Conversation 字段：`origin` ∈ hub_created|bound|imported_list · `interaction` ∈ writable|read_only · `phase` · `busy` · `last_outcome` · title · summary_preview · …

### D.2 动词

| 动词 | 命令 | Store |
|------|------|-------|
| discover | `agent sessions` | 元数据 upsert，**禁止** session/load |
| create | `conv create` | hub_created + W |
| bind | `conv create --agent-session-id` | → bound（或保持 hub_created），重算 IX |
| list/show/send/… | 见门闩表 | PHASE1-CONTRACT §4.3 |

**无 `conv import`。**

### D.3 Option A + 不降级（强制）

1. `origin=imported_list` ⇒ **interaction 恒 read_only** 且 send/param-set/mode-set 拒。  
2. 仅 bind 或 hub_created 可打开写（且 IDE space 绑后仍 R）。  
3. discover **永不** 把 hub_created/bound **降级** 为 imported_list。  
4. 展示 `interaction` **必须等于门闩**（禁止显示 W 却 send 拒）。

### D.4 默认 list = workbench

- 默认 / MCP `workbench=true`：仅 hub_created|bound **或** 有 Layer2。  
- 博物馆全量：`conv list --all` / `include_imported=true`。  
- 理由：主用户是 agent，默认洪水 = 产品失败（P0-14 选定 workbench 默认）。

### D.5 并发

单 flight **每 conv_id**；跨 conv/agent **允许**并行 send。

### D.6 IDE 永远不可 send

space=ide → 永远 read_only。Next action **不是** bind 求 W，而是 **`conv create` 新建** 可写会话；show 只读历史。

完整 schema/meta/算法/错误示例 → **PHASE1-CONTRACT**。

---

## E. Feature 目录 F-*

### Tier 0

| ID | 能力 | 落地 Phase |
|----|------|------------|
| F-REG | 注册 | 已有；文档 G 冷启 |
| F-COG | inspect+probe | Phase 3 |
| F-DISC | discover 元数据 | **Phase 1** |
| F-BIND | bind promote | **Phase 1** |
| F-NEW | create | **Phase 1** |
| F-FIND | list workbench/filters | **Phase 1** |
| F-READ | transcript view | Phase 2 |
| F-SEND | send + 门闩 | **Phase 1** |
| F-PROG | progress+timings | Phase 3 |
| F-CONT | 续聊 ensure_live | 已有+Phase1 错误 |
| F-FAIL | 错误码信封 | **Phase 1** 子集 / Phase3 全 |
| F-SRCH | search+IX | Phase 2 |
| F-RO | 横切门闩 | **Phase 1** |
| F-CLOSE/DEL/CXL | 生命周期 | **Phase 1** |
| F-MULTI | 跨 conv 并行规则 | **Phase 1**（规则） |
| F-DOC | doctor + help journey | Phase 4 |
| F-MIG/SHIP | 升级/发版 | Phase 4 |
| F-PARAM/AUTH/PROXY | 既有+门闩 | Phase1 门闩；AUTH 动线 Phase3–4 |

禁止未登记 F-* 命令。

### MCP 映射

见 PHASE1-CONTRACT §6.3 + 既有 tools；`list_conversations` 增加 workbench/include_imported/limit。

---

## F. 输出与 QoL 规范（跨 Phase）

### F.1 List（Phase1 列）

`CONV | AGENT | IX | ORIGIN | STATUS | TITLE | UPDATED`  
JSON：`id`/`conv_id`/`origin`/`interaction`/`status`/`phase`/`busy`/`last_outcome`/`summary_preview`(P1 null)

### F.2 Sessions

`SESSION | IX | SPACE | IN_HUB | CONV | TITLE`  
`in_hub_before` = upsert **前** 已存在。

### F.3 Transcript（Phase2 — 可测规则）

- 序：`(created_at, seq)` 升序。  
- 连续 `kind=thought` 合并，body `\n` 拼接。  
- 同 `toolCallId` 的 tool_call+updates → 一行，status/title 取最后。  
- 剥 `/^content type\s+/i` 与重复 `text text`。  
- 默认上限 256KiB 或 200 view 节点；`truncated` + `--full`。  
- `--raw` = 未合并 Store。  
- send 终态视图与 show **同一算法**。

### F.4 Progress（Phase3）

- 阻塞命令：**流式 progress + 最终 timings** 皆要。  
- progress：`{"type":"progress","stage":"daemon_connect|agent_spawn|initialize|session_op|prompt|end","at_ms":n}`  
- 人类 stderr：`[acp-hub] stage=<stage>`  
- 跳过阶段：**省略** timings key。  
- timings 键：`daemon_ms, agent_spawn_ms, initialize_ms, session_ms, prompt_ms, total_ms`

### F.5 错误（Phase1 起）

信封见 PHASE1-CONTRACT §5。  
`resume_load_failed` 时 **禁止** session/new。

### F.6 summary_preview（Phase2）

最近 `role=user` 可读 body，Unicode ≤80；否则 assistant 非 thought；否则 title；剥 F.3 噪声。

### F.7 Search hit（Phase2）

`{conv_id, agent_id, interaction, origin, snippet≤120, updated_at}` limit 默认 20。

---

## G. 操作者 Runbook（动线 — I/O 级）

### G.0 心智顺序

```
0. 安装 acp-hub 二进制；HOME=$ACP_HUB_HOME 或 ~/.acp-hub（daemon 由 CLI 自动 spawn，一般不必手动 serve）
1. agent add <id> --command …     # Cursor 例：adapters/cursor 文档 command 行
2. agent inspect <id> --probe     # 无 probe 且 cache 空 → probe_status=skipped，下一步 --probe
3. 新建干活: conv create <id> --cwd <abs>
4. send <conv_id> --text "…"
5. conv show <conv_id>
6. 找回: conv list          # 默认 workbench
        conv list --all     # 含 imported
        search "…"
7. 远端博物馆: agent sessions <id> → show 只读
   可写 ACP 历史: conv create <id> --agent-session-id <sid>
   IDE 历史: 只 show；要改代码 → 新 create，不要指望 bind 变 W
```

### G.1 SC-01 冷启注册 + 首次 send（PASS 标准）

| 步 | CLI | 成功可见 | 失败 → 下一步 |
|----|-----|----------|----------------|
| 0 | （已安装） | `acp-hub --version` | 安装/PATH |
| 1 | `agent add cursor --command <见 adapter README>` | list 含 cursor | invalid_argument / spawn 路径 |
| 2 | `agent inspect cursor --probe` | probe_status=ok；caps；permission_policy | probe failed → 查 command；auth_required → agent auth |
| 3 | `conv create cursor --cwd <abs repo>` | 打印 conv_id；JSON origin=hub_created interaction=writable | daemon_unavailable 重试；agent_spawn_failed 查 inspect |
| 4 | `send <cid> --text "create marker file …"` | exit 0；last_outcome=completed 或诚实 failed | busy/权限/agent_acp 按 F.5 |
| 5 | `conv show <cid>` | 消息可读（P1 可粗糙；P2 merge） | |

**MCP 等价：** register 若暴露 / list_agents → create_conversation → send_prompt → get_messages。

**非目标本 SC：** 自动 migrate 旧 reject（见 SC-12）。

### G.2 SC-02 inspect 认知

| | |
|--|--|
| cache 空无 probe | JSON `probe_status=skipped` + message 建议 `--probe`；**不得**像完整成功 |
| 有 probe | capabilities 含 fs/session 子集；auth_methods；permission_policy |
| 旧 reject | 警告文案「policy=reject 可能拒写；改 registry 或 re-add」≠ agent_spawn_failed |

### G.3 SC-05 / FLOOD 找回

| 意图 | 命令 | 期望 |
|------|------|------|
| 刚才工作 | `conv list` | 仅 workbench；顶行最近 |
| 博物馆某条 | `agent sessions` 记 sid/title → `conv list --all` 或 sessions 的 CONV | origin=imported_list IX=R |
| 内容关键词 | `search`（P2 起对 Layer2/已 load；纯 imported 无 body 时 hit 可能仅 title/meta——P2 写清） | |
| 过多 | limit=100 truncated | 加 --offset 或收窄 --agent |

### G.4 SC-06/07 IDE 只读 + 误 send

见 PHASE1-CONTRACT §9 逐步 oracle。  
**禁止下一步：**「bind 后即可 send」。  
**正确下一步：** `conv create <agent> --cwd …` 新 W 会话。

### G.5 SC-MULTI 两 agent 并行

| 步 | |
|----|--|
| 1–2 | 两 agent 各 `conv create` → cidA cidB |
| 3 | 并行 `send` A 与 B（两进程/两 MCP） |
| 成功 | 均可 completed；互不 conversation_busy |
| list | 两行不同 agent_id |

### G.6 SC-MK / 中途死

| 事件 | 终态 | code | 下一步 |
|------|------|------|--------|
| 同 conv 双 send | 第二拒 | conversation_busy | wait/cancel |
| cancel 后 30s agent 无视 | last_outcome=failed busy=none | run_failed data.reason=cancel_ignored | show；决定重发 |
| **CLI 进程** Ctrl+C mid-send | run failed/cancelled；busy 清 | run_failed 或 cancelled | show；可重发（非幂等保证，人/agent 判断） |
| **daemon** 死 mid-send | 见 PHASE1-CONTRACT §5.2b：**客户端主码 `daemon_unavailable`**；恢复后 busy=none last_outcome=failed（不得永久 running） | daemon_unavailable | show；**一次** 重 send |
| ensure_live 失败 | 不 session/new | resume_load_failed | show；新 create 或修 agent |

### G.7 SC-NO-LIST

| agent caps | sessions | 工作 |
|------------|----------|------|
| 无 list | unsupported_capability | 仅 hub_created；勿 bind 未知 sid |
| 有 list 无 load | sessions ok；show 可仅 Layer2 | send 靠 live；load 失败分类 |
| 空 list | 成功空数组 | create 新会话 |

### G.8 SC-12 旧 reject 配置

| | |
|--|--|
| 检测 | inspect：`permission_policy=reject` |
| 输出 | **固定子串** `permission_policy=reject; re-add agent with defaults or edit agents.json`（exit 0 + warning） |
| send/create 被拒 | code **`permission_policy_reject`**，message 含同上子串；**禁止**仅 `agent_spawn_failed` / 挂死 |
| 动作 | remove+add 或手改 agents.json；**Phase4** doctor 扫描 |
| 禁止 | 表现为神秘 agent 坏 |

### G.9 SC-13 thought 碎片（Phase2 验收）

Fixture 输出 ≥10 thought chunk → show 默认 **1** 个 thought 视图节点（或远少于 10）；`--raw` 还原多行。

### G.10 SC-14 MCP

同一 G.0；tool 描述字符串 = PHASE1-CONTRACT 固定句。

### G.11 SC-AUTH（Phase3+）

send/create → auth_required → `agent auth` → 重试。

### G.12 close vs delete

| | close | delete |
|--|-------|--------|
| 远端 | 尽力；失败仍本地 closed | 可选删远端 |
| 本地 | phase=closed；show 仍可读 | phase=deleted；默认不 list |
| list | 默认隐藏 closed | 隐藏 |

---

## H. QoL

Q1 默认可用 · Q2 阻塞有阶段(P3) · Q3 list 默认可扫(workbench) · Q4 view≠Store · Q5 错误信封 · Q6 JSON 稳定 · Q7 CLI/MCP 同构 · Q8 能力诚实 · Q9 help 含 G.0 · Q10 SC 回归门槛

---

## I. Pillar 映射

注册/搜索/CRUD/收发/参数/proxy/双层/发现已有会话/协商/daemon — 全部挂 F-*；双层 load 策略：discover 不 load；show Phase2 默认可 load 一次（`--local-only` 关）。

**Phase2 show Layer1 默认：ON**（空 Layer1 且 agent 支持 load 时）；JSON `layer1_refreshed:boolean`。

---

## J. Charter 映射

| P-UX / J / SC | 闭合位置 |
|---------------|----------|
| P-UX-01 动线 | §G |
| P-UX-02/03 重叠与多 endpoint | D + workbench + SC-MULTI |
| P-UX-04 只读 | Option A + IDE 永不 W |
| P-UX-05 inspect | G.2 / Phase3 |
| P-UX-06 进度 | F.4 Phase3 |
| P-UX-07 transcript | F.3 Phase2 |
| P-UX-08 错误 | F.5 + CONTRACT |
| P-UX-09 升级 | G.8 / F-MIG Phase4 |
| P-UX-10 并发中杀 | G.5 G.6 |
| 全 SC | §G + CONTRACT §9 |

---

## K. 分期（可执行）

| Phase | 交付 | 合同 | 退出 |
|-------|------|------|------|
| **1** | origin/interaction/phase 状态；discover 无 load；Option A；bind；list workbench；close；错误信封；并发规则；Cursor meta 解析 | **PHASE1-CONTRACT** | 该文 §10 checklist |
| **2** | summary_preview；transcript F.3；search hit；show Layer1 默认 ON | 开工前写 PHASE2-CONTRACT（≤2 页，复制 F.3/F.6/F.7） | SC-05/13 契约 |
| **3** | inspect probe 字段；progress+timings；错误码全表；cancel 30s | PHASE3-CONTRACT | SC-01/02/03 冷路径 |
| **4** | help journey；doctor；MIG 提示；场景回归脚本；发版说明 | — | M1–M8 评估 |
| **5** | F-PIN/F-RUNS/F-REVEAL… | — | 可选 |

**Phase 边界铁律：** 未写该 Phase 合同前，实现者 **不得** 发明 JSON 字段名或默认值。

---

## L. 门禁（产品完成 — 非现在）

| ID | 标准 |
|----|------|
| M1 | G.1 冷启 runbook 不猜命令（含 probe） |
| M2 | list 行可知 W/R |
| M3 | SC-13 merge |
| M4 | create/send 有 stage+timings |
| M5 | workbench+search 可找回 |
| M6 | 只读/IDE 误 send 稳定码+正确 next |
| M7 | CLI/MCP 同构 |
| M8 | SC 回归 |
| M9 | 无野生命令 |

**当前（v0.3）：** 仅宣称 **Phase1 可按合同实现**；M1–M6 **未**满足。

---

## M. 文档树

| 文档 | 角色 |
|------|------|
| CHARTER | 问题与禁止 |
| Product-UX | 默认与 §6 |
| **SYSTEM v0.3** | 政策+动线+分期 |
| **PHASE1-CONTRACT** | 可实现 SSOT Phase1 |
| 后续 PHASE2/3-CONTRACT | 实现前补 |
| COMPLIANCE | 底座 |

---

## N. Refine log

| Ver | 结果 |
|-----|------|
| 0.1 | 骨架 |
| 0.2–0.2.1 | 政策闭合；**假称可实现** |
| Review×3 | REJECT / NO-GO / 7 journey FAIL |
| **0.3** | 默认 workbench；IDE 永不 W；状态机；错误信封；G runbook；**PHASE1-CONTRACT 可实现**；诚实未完成 M* |

---

## O. Phase0 退出（设计侧）

- [x] 政策决策无「实现再定」的 Phase1 范围  
- [x] Phase1 独立合同含 schema/meta/discover/bind/list/error/SC  
- [x] 动线含冷启/只读/并行/daemon/no-list/reject  
- [x] 假闭合声明删除  
- [x] PHASE1-CONTRACT v1.1 闭合 re-review 阻断项（list envelope、delete、daemon、reject、cli W…）  
- [ ] 可选：再确认一轮 APPROVE（nits only）  
- [ ] Phase2/3 合同在开工对应 Phase 前补齐  
