# Product UX Pillar — ACP Hub CLI

**Status:** Agent-managed pillar (2026-07-24) — **not** a frozen user SSOT file  
**Tree:** `doc/ssot/agent-managed/` (see [../README.md](../README.md))  
**Frozen baseline (do not edit):** `doc/ssot/pillars/README.md`, `TechSel.md`  
**Intent:** 在不改写冻结 pillar 的前提下，记录用户于 2026-07-24 纠正的产品主次：可用性优先于 fail-closed / down-privilege 默认；锁定 hub CLI 的操作验收标准。

---

## 1. 产品一句话

**acp-hub 是可日常使用的 ACP 客户端设施**：用同一套 CLI/MCP/库入口注册任意 agent、管理对话、发任务、设参、搜索历史 — 体验上应能 **替代客户端内嵌 ACP**，并在操作模型上 **可平替** OMP 式 subagent/task 工作流（注册 → 开对话 → 设参 → 发任务 → 看结果 → 续聊），而不是一个默认拒权、动辄断连的 fail-closed 调试器。

---

## 2. 主次顺序（硬约束）

| 优先级 | 目标 | 说明 |
|--------|------|------|
| P0 | **完整可用主路径** | `agent add` → `conv create` → `param`/`mode`（可选）→ `send` → 看结果 → 再 `send` 连续成功 |
| P0 | **流畅手感** | 不无故 hang；错误可理解可行动；高 churn agent 不把成功 turn 报成失败 |
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

## 5. 连接与投影（相对 R-DAEMON-004 的纠正）

- **投影完整性有价值**，但优先级低于 **turn 完成与 CLI 可用性**。
- 客户端落后于 notification 时：应 **扩容、聚合、丢弃非关键更新并标记 stale、或请求 resync**，而不是默认 **connection-fatal 中断 in-flight RPC**。
- 若必须断连：仅在确认无 in-flight 业务 RPC，或提供可配置策略；默认策略服务 P0。

---

## 6. 错误与可观测性

- `ResumeLoadFailed` 的 source 在 CLI 展示层必须保留可区分原因（agent ACP 错误、unsupported、timeout），**禁止** 统一显示为 `daemon unavailable: resume/load operation failed` 误导操作者。
- Hang 必须有超时或进度；`agent add` 不得在 agents.json 已写后无限无响应。

---

## 7. 非目标（仍成立）

- Hub **不是** 内嵌 LLM 产品；task/subagent 图仍由 endpoint（如 OMP）实现。
- Hub **不** 解析厂商私有 DB（adapters 边界仍成立）。
- Hub **不** 做多租户远程 SaaS。

「可平替 subagents」指 **操作模型与顺滑度**，不是把 OMP `task` 工具搬进 CoreHub。

---

## 8. 对下游文档的要求

本 **agent-managed** pillar 生效后，agent 应审查并更新（**不得**为此改写冻结 `doc/ssot/pillars/*`）：

- `doc/dev/*` 中与 least-privilege 默认、lag-fatal 相关的条款（agent/dev 文档）
- `adapters/*/agents.json` 与 README 示例
- `crates/cli` 默认 flag
- `SECURITY.md` / README 中「samples reject by default」表述
- 历史 research 中「fail-closed conductor」表述标注为 **已被本 overlay 纠正**

**实现裁决：** 用户已明确要求 UX 优先时，实现遵循本文件；冻结 pillar 仍描述「能做什么」，本文件描述「默认怎么好用 / 主次如何排」。  
旧 review book 的「deny-by-default samples」**不再作为 agent 实现的默认目标**。
