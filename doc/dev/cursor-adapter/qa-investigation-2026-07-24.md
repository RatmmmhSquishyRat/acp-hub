# QA 反馈核实调查 — Cursor via ACP Hub（2026-07-24）

**输入：** [regression-feedback-2026-07-24.md](./regression-feedback-2026-07-24.md)  
**证据根：** `tmp/acp-regression-cursor-20260724-203801/`（summary.json + regression.log + 各用例 out）  
**Hub：** `0.2.1-rc.2` / main 同期  
**性质：** 对 QA 清单的 **真实性裁定 + 根因归属 + UX/QoL 缺口扫描**。  
**不是** 半成品补丁清单，也不是「回归绿 = 产品完成」。

---

## 0. 总判断（先说清楚）

1. **功能正确性（本轮 20/20）属实。** 写文件、续聊、ask、杀 daemon 恢复在产物上全部成功；status=`completed` 与 rc.2 状态镜像一致。  
2. **QA 列出的 QoL / 运维问题大部分是真实产品缺口，不是误报。**  
3. **这些缺口证明：过去的实现重心在「能通 + 不崩」，没有按「真实产品操作体验」做完整 UX 设计与验收。** 这与冻结 pillar「完整 client」及 agent-managed Product-UX「流畅手感 / 可平替内嵌 ACP」的目标 **不一致**。  
4. 后续必须用 **产品级 UX/QoL 设计 → 实现 → subagent review loop** 闭合，而不是对单点发文档补丁。

---

## 1. 证据基线（本轮回归）

| 用例 | ms | 关键观察 |
|------|-----|----------|
| A_register | 143 | 仅写 registry |
| A_inspect | **27** | `agentInfo/capabilities=null`, `cachePopulated=false`（见 `A_inspect.out.txt`） |
| B_create | **13123** | 冷路径（daemon + agent + session/new） |
| B_param | 4605 | 含 live agent |
| B_send | **16885** | 成功；thought **逐片**打印 |
| C_create | 4603 | 热路径仍秒级 |
| C_send | 13776 | |
| D_followup | 8623 | 同会话更快 |
| E_create | 1910 | 更热 |
| H_create_after_kill | **15080** | 再付冷路径 |
| H_send | 16952 | |
| G_create ×3 | ~7s+4.8s+… | 仅 create，未 close |

`B_show.out.txt`：一条写文件 turn 拆成 **11 行** message（thought 碎片 + tool_call + updates），且 **不显示 kind**，body 为 `content type text text …` 噪声。

---

## 2. QA 清单逐项裁定

图例：

| 裁定 | 含义 |
|------|------|
| **REAL** | 代码/产物可复现的真实问题 |
| **REAL-DESIGN** | 当前实现故意如此，但 **产品体验不合格** |
| **PARTIAL** | 机制属实，数字或归属需拆分 |
| **OPS** | 操作/分发/文档面，非协议 bug |
| **DESIGN-OK** | 产品边界，不应当 bug |
| **UNTESTED** | 风险真实但本轮无证据 |

### 2.1 P1

| ID | QA 标题 | 裁定 | 严重度 | 归属 | 核实摘要 |
|----|---------|------|--------|------|----------|
| **LATENCY** | 冷启 / create 偏慢 | **PARTIAL** | **P0 体感** | Hub + adapter + cursor-agent | 机制：create 串行 `ensure_daemon` → `agent_handle`（spawn stdio→adapter→cursor-agent→initialize≤30s）→ `session/new`。首次 create ~13s、杀 daemon 后 ~15s **与日志一致**。二次 create ~4.6s 说明热路径仍偏慢。send 14–17s **大量是模型/Cursor**，不能全算 Hub。缺 **分段计时** 是 Hub 产品债。 |
| **INSPECT** | inspect 无 agentInfo/capabilities | **REAL-DESIGN** | **P0 操作** | Hub core + CLI | `inspect_agent` **明确不连 agent**，只读 `agent_cache`；`agent add` **从不** 写 cache；cache 仅在 `agent_handle` 首次 initialize 后写入。回归在 create 前 inspect → 必然空洞（`registry.rs` docstring + `A_inspect.out.txt`）。另：cache 里 `agent_info` 恒写 `"{}"`，**auth_methods 不进 inspect DTO** → 即便热身后 inspect 仍残缺。 |
| **SESSION-ACCUM** | idle 会话/进程堆积 | **REFRAMED** | **非主问题** | 产品语义 | **堆积本身不是 bug。** 真问题是会话 **管理语义与可发现性**不合格（见 §2.5 / Product-UX §6）：使用者 agent 应一眼看到 running / 最近更新 / 内容摘要 / 读写能力，以便 **找到目标 session**。TTL/reaper 仅为可选运维，不是 UX 主轴。进程泄漏仍可另测，但不得用「清闲置」代替发现面设计。 |
| **SHIP** | 稳定版仍 0.2.0 | **OPS-REAL** | **P1 分发** | 发布 | crates.io / Latest 仍 0.2.0；rc 仅 GitHub。认知差会导致「装了 Hub 仍不能用 Cursor」误判。 |
| **MIGRATE** | 旧 agents.json 不迁移 | **REAL-DESIGN** | **P1 升级** | Hub + 文档 | 有意不改写磁盘 reject；升级 exe 不改配置。对 Cursor 写工具路径是 **真实陷阱**。 |

### 2.2 P2

| ID | 标题 | 裁定 | 严重度 | 归属 | 核实摘要 |
|----|------|------|--------|------|----------|
| **REDACT** | 路径脱敏妨碍排障 | **REAL-DESIGN** | P2 | Hub privacy | ordinary inspect 红acted command；无 trusted reveal 开关。安全默认 vs 排障冲突。 |
| **PROJECTION** | 投影过碎 / thinking 碎片 | **REAL** | **P0 阅读** | Capture 1:1 + **CLI 展示零合并** | 每 `AgentThoughtChunk`/`ToolCallUpdate` → 一行 Store（`capture.rs` `cap()`）。CLI `print_messages` **不显示 kind**、body 截 100 字、不合并连续 thought。`send` 输出同样碎片（`B_send.out.txt`）。`search_body` 递归抽字符串含 JSON key 噪声。**Store 不强制碎片**；展示层完全可合并而不改 durable 真相。 |
| **WIN-CLI** | Windows 引号 / allow 标志 | **OPS-REAL** | P2 | 文档 + 脚本 | clap 行为正确；操作者易踩。 |

### 2.3 P3 / 边界

| ID | 裁定 | 说明 |
|----|------|------|
| **MATRIX** 并发 send / mid-turn kill / 跨 OS | **UNTESTED** | 风险真实；稳定版前至少需 mid-turn 策略或显式「不支持」。 |
| **IDE-AS-NORMAL** | IDE/只读空间伪装普通 session | **REAL-DESIGN（我们的产品错）** | **不是**「Cursor 拒绝 resume 与我们无关」。是 **我们的 adapter** 把 IDE 等只读源并进 `session/list`，却在 prompt 才拒绝 → **假一等公民**。正确法：不能改 = **显式 read-only**（选型即可见），对齐 OMP「能力不同则类型/状态不同」。 |

### 2.4 调查新增（反馈未单列，但属同一 UX 债）

| ID | 问题 | 裁定 | 严重度 |
|----|------|------|--------|
| **PROGRESS** | `send`/`create` 长阻塞无进度、无心跳 | **REAL** | **P0** |
| **SHOW-KIND** | `conv show` 表无 `kind` 列 | **REAL** | P0 阅读 |
| **BODY-NOISE** | body_text 呈 `content type text text …` | **REAL** | P0 阅读 |
| **INSPECT-WARM-WEAK** | 热身后 agentInfo 仍空、无 auth methods | **REAL** | P1 |
| **NO-METRICS** | 无 create/send 分段计时对外暴露 | **REAL** | P1 诊断 |
| **SEARCH-KIND** | search KIND 只有 message/conversation | **REAL** | P2 |
| **SESSION-DISCOVER** | list 缺 running 优先、摘要、读写能力标签 | **REAL**（产品缺口） | **P0 操作者 agent** |

### 2.5 用户纠正（2026-07-24 后）：使用者 agent 的会话 UX

**完整理解（产品法，不是补丁口号）：**

1. **只读必须显式**  
   若会话不能修改（不能 prompt / 不能当工作会话续写），必须在 list/show 上 **明确只读**，不能装成普通可写 session。  
   参考 OMP：subagent **类型与能力绑定**（explore/plan 只读工具 vs 全功能；registry 状态清晰；不可消息对象不混充可派活实例）。  
   IDE 等空间：**我们展示、我们分类、我们负责标签**——Cursor 没有安全 resume ≠ 我们有权把它列成「普通 session」。

2. **会话管理服务「找对 session」**  
   - 谁在 **running**  
   - 谁 **最近更新**  
   - **内容大致是什么**（title / preview / 意图摘要）  
   - 能否 **写**（writable vs read_only）  
   最终服务于 **使用者 agent 定位自己需要的那条会话**。  
   **闲置堆积不是问题**；语义糊、列表不可扫、能力不可见才是问题。

3. **与冷启/inspect/thinking 碎片同属一层**  
   都是 **操作者 agent 的 UX/QoL**，不是运维边角。

权威落点：

- `doc/ssot/agent-managed/pillars/Product-UX.md` §1.1 / §6  
- **`doc/ssot/agent-managed/OPERATOR-UX-CHARTER.md`** — 大型 UX 问题全表、正向动线强制前置、场景矩阵、禁止补丁冒充完成  

**用户进一步裁定（已登记 charter）：**

- 会话不完整管理、多 endpoint 查看糊、只读不显式、inspect/进度/transcript 等 **都属于大型 UX 问题**  
- **必须完整设计之后** 才能给功能；现状功能不齐 + 语义重叠 → **根本无法使用**  
- **根本原因：** 此前没有对使用者动线与正向流程（打开后先/后做什么、分支、如何读 CLI、如何继续）做完整、清晰、非 trivial、覆盖实际场景的设计  

---

## 3. 根因结构（为何会成这样）

```
ACP stream (fine-grained thought/tool updates)
        │
        ▼  Hub capture: 1 update → 1 Store row (fidelity)     ← 合理
   SQLite dual-layer truth
        │
        ├── hub/conv/update live fan-out (1:1)               ← CLI 未订阅
        │
        └── CLI 操作面
              agent inspect → cache only, no probe           ← REAL-DESIGN 空洞
              conv create   → serial cold stack, no spans    ← 体感冷
              send          → block until end, dump chunks   ← 无进度 + 碎片
              conv show     → no kind, no merge, 100c clip   ← 不可读
```

**结论：** Store-first / 稳定性工作是必要底座；**操作者产品面（inspect / progress / readable transcript / lifecycle / metrics）几乎未按产品验收。** 回归 20/20 只证明「任务能做完」，不证明「用着像产品」。

---

## 4. 产品 UX/QoL 目标（验收级，非补丁列表）

以下为后续设计必须满足的 **操作者体验契约**（对齐 Product-UX P0 + 冻结 pillar「完整 client」）。实现前需完整 design，并经 **独立 review subagent** 对抗通过后再合入。

### 4.1 注册与认知（Registration）

| 验收 | 说明 |
|------|------|
| R1 | `agent add` 后，`agent inspect` **无需先 create** 即可看到协商结果（capabilities + auth methods），或显式 `inspect --probe` 且默认文档/技能引导 probe。 |
| R2 | 热身后 inspect 不得只剩空 `agentInfo: {}`。 |
| R3 | 本机 trusted 模式可 reveal transport 路径（排障）；默认仍 redacted。 |
| R4 | 升级路径：文档 + 可选 migrate；reject 旧配置不可静默「像坏了」。 |

### 4.2 连接与冷路径（Cold path）

| 验收 | 说明 |
|------|------|
| C1 | create/send 对操作者暴露 **阶段进度**（daemon / agent spawn / initialize / session/new / prompt）。 |
| C2 | 日志或 `--verbose` 输出各阶段 ms，可区分 Hub vs Cursor/模型。 |
| C3 | 文档与 skill 给出推荐 timeout；错误文案可行动（勿只挂死）。 |
| C4 | 可选：register 或 idle 时 background warm（产品决策，非必须首版）。 |

### 4.3 回合与进度（Turn）

| 验收 | 说明 |
|------|------|
| T1 | `send` 进行中有可读进度（订阅 `hub/conv/update` 或轮询），不得长时间静默。 |
| T2 | 展示层合并连续 thought；tool_call 与 updates 可折叠为一条工具时间线。 |
| T3 | `conv show` 显示 kind；正文为人类可读文本，非 raw JSON leaf 拼接。 |
| T4 | 终态 status 与 run 一致（rc.2 已部分完成）。 |

### 4.4 会话发现与能力（Session discoverability — 主轴）

| 验收 | 说明 |
|------|------|
| S1 | **不能修改 ⇒ 显式 read-only**（list/show/选型）；禁止 list 像可写、send 才拒。 |
| S2 | list 默认服务「找会话」：running 优先或可滤、按 `updated_at`、title + **内容预览/摘要**。 |
| S3 | 每条会话可见 **interaction**（writable / read_only）与来源（hub-owned / agent-original / vendor-space）。 |
| S4 | close/delete 可用且文档清楚；**TTL/gc 可选**，**不得**用「消堆积」替代 S1–S3。 |

### 4.5 检索（Search）

| 验收 | 说明 |
|------|------|
| Q1 | snippet 优先正文；弱化 JSON key 噪声。 |
| Q2 | 可选按 run 聚合 hit，避免 thought 碎片刷屏。 |

### 4.6 分发（Ship）

| 验收 | 说明 |
|------|------|
| D1 | 稳定 0.2.1 对齐 crates.io 与 GitHub Latest 的决策与时间表。 |
| D2 | Cursor README 置顶最低版本与 auto-allow。 |

---

## 5. 建议工程阶段（闭合方式）

**禁止：** 只改一句文档、只合并 thought 的 20 行 CLI 补丁、无 review 直接宣称完成。

| 阶段 | 内容 | 退出标准 |
|------|------|----------|
| **0 调查** | 本文档 | 每条 QA 有裁定 + 证据 |
| **1 UX 设计** | 独立 design 文档：操作模型、默认、probe、进度、transcript view、lifecycle | design-doc reviewer 共识；对照 §4 验收表 |
| **2 实现** | 按设计切 PR（inspect/probe · progress · transcript · lifecycle · metrics） | 每 PR 有单测 + CLI 契约 |
| **3 Review loop** | 每 PR 独立 subagent review → rework → 再测 | 无「自证正确」 |
| **4 回归** | 扩展 Cursor 脚本：inspect 热/冷、create 分段时延、show 可读性、create×N 后 close 进程数 | 新 summary 通过 |
| **5 发布** | 稳定 0.2.1 或后续 rc 含 UX 契约 | CHANGELOG 可操作者理解 |

---

## 6. 优先级建议（工程排序）

| 序 | 主题 | 理由 |
|----|------|------|
| 1 | **Transcript 可读性**（show/send merge + kind + clean body） | 证据最硬、用户每轮都看到；不碰 Store 语义即可做展示层 |
| 2 | **Progress / 非静默长操作** | Product-UX 明文 hang 不可接受；与冷启体感同一类 |
| 3 | **Inspect probe + 完整 cache** | 注册后「看不见能力」= 假客户端 |
| 4 | **分段 metrics** | 拆清 Cursor vs Hub，避免错误背锅与错误超时 |
| 5 | **Session lifecycle 策略** | 编排器真实风险 |
| 6 | **Migrate + 稳定版 ship** | 认知与升级 |

---

## 7. 明确不做的误判

| 误判 | 正确说法 |
|------|----------|
| 把 14–17s send 全怪 Hub | 含模型/Cursor；需 span |
| 把 thought 碎片当 Store 损坏 | Store-first 忠实捕获；**展示**不合格 |
| 把 inspect 空当 daemon bug | 设计为 cache-only；**产品**不合格 |
| 把 IDE resume 拒绝当回归失败 | 适配器边界 |
| 回归 20/20 = UX 完成 | 只证明功能路径 |

---

## 8. 证据索引

| 证据 | 路径 |
|------|------|
| QA 原文 | `doc/dev/cursor-adapter/regression-feedback-2026-07-24.md` |
| summary | `tmp/acp-regression-cursor-20260724-203801/summary.json` |
| inspect 输出 | 同上 `A_inspect.out.txt` |
| 碎片 thought | 同上 `B_send.out.txt` / `B_show.out.txt` |
| inspect 实现 | `crates/hub/src/hub/registry.rs` `inspect_agent` / `agent_handle` |
| capture 1:1 | `crates/hub/src/callbacks/capture.rs` |
| show 展示 | `crates/cli/src/output.rs` `print_messages` |
| create 串行 | `crates/hub/src/hub/conversation.rs` + `registry.rs` agent_handle |

---

## 9. 变更记录

| 日期 | 说明 |
|------|------|
| 2026-07-24 | 全量核实 QA 反馈 + 代码/回归产物；落盘裁定与 UX 验收骨架。下一阶段：正式 UX design（非实现冲刺）。 |
