# ACP Hub — Specification (Spec)

> Source of truth: `doc/ssot/pillars/README.md` (Spec 1-5, design 1-5, FAQ)
> TechSel: `doc/ssot/pillars/TechSel.md`

## 1. Purpose

ACP Hub 是一个独立的、通用的、不假设 client/agent 用途的 ACP 管理和调用 core。使用者可以注册任意 ACP Agent Endpoint，管理对话，收发消息，并自动捕获维护可获取的对话和消息历史记录。

## 2. Functional Requirements (Spec)

### S1 — 注册 ACP Agent Endpoints
- 像 MCP 一样通过配置注册 ACP Agent Endpoints
- 支持 stdio JSON-RPC / HTTP / WebSocket 传输
- 用户可以自己开发 ACP adapter 程序并注册
- 注册数据存储在 `agents.json`（JSON 对象映射，镜像 MCP `mcpServers` 约定）

### S2 — 全局搜索 + 对话 CRUD
- 全局关键字搜索对话和消息记录
- 指定 endpoint 增/删对话
- 查看对话中的消息

### S3 — 发送消息
- 指定 endpoint 的某个对话发送消息
- 等待回复
- 查看回复

### S4 — 参数设置
- 设置发送消息的具体参数：模型/思考强度/模式等
- 覆盖外部文本 slash command 覆盖不到的全部消息模式和状态

### S5 — ACP Proxies
- 通过 ACP proxies 预处理发送消息（添加工具信息、polish 文本等）
- 对收到的消息进行后处理（格式化等）

## 3. Two-Layer Data Model (FAQ lines 33-40)

### 3.1 定义

ACP Hub 的对话数据是**两层完全平行的数据**，不是同层级互斥数据：

| 层级 | 名称 | 来源 | source 列值 | 语义 |
|------|------|------|-------------|------|
| Layer 1 | Agent Original | ACP `session/list` + `session/load` 返回的原始数据 | `load_replay` | Agent 自己维护的真实对话历史 |
| Layer 2 | Hub Capture | Hub 通过 `send/prompt` 捕获的 `session/update` | `local_turn` | Hub 作为 client 视角的增量记录 |

### 3.2 显示规则

- **conv list**: 显示所有对话——包括 agent 发现的（Layer 1）和 Hub 创建的（Layer 2）
- **conv show**: 显示两层消息，各自标注来源
- **search**: 搜索两层数据
- **Fallback**: 仅当 agent endpoint 完全不支持 `session/list` / `session/load` 时，才仅 fallback 到 Hub 捕获记录（Layer 2）

### 3.3 数据独立性

- 捕获的数据和原始对话记录的语义不相同，且各自有用
- 不能只显示 Hub 自己创建的对话的捕获数据
- 两层数据都是独立信息，都应当被显示

### 3.4 静态资源 Snapshot (FAQ lines 33-34)

- 没办法保证所有 agent 完整支持 conversation 的各种 API
- 一旦功能调用成功，Hub 就会全量记录相关的静态资源 snapshot
- 每次调用更新，尽可能完整体现 ACP agent endpoints 的静态资源情况

## 4. Design Requirements (design 1-5)

### D1 — Endpoint 注册
- 注册 stdio JSON-RPC / HTTP / WebSocket ACP Agent Endpoints（同 MCP）

### D2 — 统一抽象层 + Capability Negotiation
- 不关注各 agent 具体实现，内部提供统一 `AgentEndpoint` 抽象层
- 运行时 capability negotiation，检查最小必要能力和可选能力覆盖
- 具体能执行的 ACP 操作取决于 endpoint 能力支持程度

### D3 — 统一传输交互
- 通过统一抽象层，使用 stdio/HTTP/WS 与各 ACP Agent 进程管理和交互

### D4 — Client + Conductor 角色
- Hub 是 ACP 协议中的 Client 和 Conductor
- `Client(Hub) - ACP Conductor(Hub) & Proxies(if any) - ACP Agents`

### D5 — On-Demand Singleton Daemon
- 本体是 on-demand singleton daemon
- CLI / MCP / 内嵌库入口都通过文件发现并锁定服务器
- 使用 rust interprocess + JSON-RPC 连接 core daemon
- 无人使用一段时间后自动退出

## 5. Technology (TechSel)

- Rust 技术栈 + crate 生态最佳实践
- 原则上使用最新稳定版本包。直接依赖的 ACP Rust SDK 与 `rmcp` 不能长期停留
  在已经被稳定大版本替代的旧 major；升级前需要核对官方迁移和 wire
  compatibility，升级后必须通过真实 ACP 与 MCP process smoke。
- 项目本体很小，不做 MVP，直接给出完整实现
- 与 openab 等参考库没有直接关系

## 6. Scope Boundary

- ACP v1 only（协商其他大版本则断开）
- ACP 协议层/传输层/conductor/MCP 集成/测试 fixtures 中不重复造轮子——使用官方 `agent-client-protocol` rust-sdk
- Core Hub 只通过 ACP endpoint 交互，不直接解析任何 agent 私有存储。
- 当厂商 ACP endpoint 无法列出或加载已有会话时，可以提供独立的、显式注册的 vendor adapter 作为兼容层。该 adapter 必须：
  - 在自己的 spec 中列出读取的路径、schema 假设、支持版本和失败行为；
  - 对未公开存储只做最小必要读取，数据库连接必须是只读；
  - 禁止伪造、更新或删除逆向得到的内部记录；
  - 将调用官方 resume/delete CLI 产生的写入或删除与直接只读解析分开说明；
  - schema 不兼容时返回明确错误，不能把解析失败伪装成空历史；
  - 通过可复现的 compatibility matrix 验证，不把单机路径、会话 id、计数、临时分支或版本猜测写入长期规格。
- `adapters/cursor` 和 `adapters/grok` 是上述兼容层，不是 Core Hub 可以任意读取 agent 私有数据的授权。

## 7. Maintainability and Module Boundaries

- `crate::hub` 的公共路径保持稳定：`CoreHub`、`HubClient` 和已有公共 DTO
  继续从该路径导出。
- `crates/hub/src/hub.rs` 只负责私有子模块声明和公共 re-export，不承载具体
  业务实现。
- 模块边界要求适用于整个项目，不只适用于 `hub.rs`。ACP driver、callbacks、
  transport、daemon、RPC、store、CLI 和 MCP 都按领域职责拆分；测试按被验证
  的行为分组，不把大量不相关场景继续堆在一个文件中。
- 生产或测试 Rust 文件不能达到 1,000 行；以约 900 行作为主动拆分边界。
  接近边界时，实现者需要先同步 spec/design/BDD/TDD/impl_plan，完成第三方
  review-rework loop，再执行拆分，不能等待用户提醒。
- facade 可以保留稳定的模块路径和 re-export，但不能借 facade 名义继续放置
  大量实现或测试。
- 拆分属于内部维护边界调整，不能改变 ACP、JSON-RPC、CLI、MCP、存储语义，
  也不能改变现有公共 Rust API、命令名、MCP tool schema 或持久化 schema。

## 8. SDK Currency and Compatibility

- ACP protocol、conductor 和 test harness 必须来自同一官方 rust-sdk release
  line，不能混用跨 major 的核心类型。HTTP/WebSocket 使用项目的
  pre-deserialization bounded transport，并消费该 release line 的 core SDK
  types；不声明未使用的 `agent-client-protocol-http` 依赖。
- MCP server 使用官方 `rmcp` 当前稳定 major。升级不能通过删除 tool、
  放宽 schema、跳过 process smoke 或保留两套不一致 API 来完成。
- ACP wire protocol 仍以 initialize 协商到的 protocol v1 为准；crate major
  升级不允许静默改变 Hub 对外声明的 wire version。
- 依赖升级必须保留本项目额外施加的 frame、queue、privacy、capability、
  cancellation 和两层历史约束。官方 SDK 默认行为不能替代这些项目级边界。
- 当前公共 Rust API 直接包含官方 ACP 类型，因此 ACP crate major 升级属于公开
  类型 identity 变更。执行该迁移时 `acp-hub-core` 与 `acp-hub-cli` 至少升级
  到 `0.2.0`，发布说明必须明确，并由 workspace 外 consumer fixture 编译验证
  实际新 API；不能在 `0.1.x` 内宣称 semver-compatible 的纯内部迁移。

## 9. Registry and Store Commit Invariants

- `session/list` 以单 session 为原子导入单位：首次导入失败不得留下 ghost
  row；既有 conversation 的 title、cwd、directories、FTS 与静态 snapshot
  必须恢复到导入前状态。同一 batch 中失败前已完成的 session 保持提交，失败
  session 回滚，后续 session 不处理，RPC 返回 typed partial-import error 和
  已完成数量。重复 `(agent_id, session_id)` 在 dedupe 前计入预算，第一次出现
  的 metadata 胜出且只 replay 一次。
- registry mutation 只能返回两种真实结果：未提交且内存/磁盘不变，或已提交
  且磁盘、内存、fingerprint、capability cache 与 live handle 属于同一
  generation。rename 后失败不能伪装成“未写入”。
- daemon 运行期间不支持其他进程直接编辑 `agents.json`；所有受支持的运行时
  mutation 必须走 Hub API。fingerprint 在取得内部 admission 后再次检测常见
  drift 并返回 conflict，但普通文件系统没有跨平台 rename CAS，因此该检测是
  best-effort，不承诺保护不遵守 Hub lock/version protocol 的并发 writer。
  daemon 停止后允许人工编辑。旧 endpoint initializer 不得在
  replacement/remove 后发布 handle。
- schema migration 与 migration marker 同事务提交；conversation metadata、
  FTS、cwd 和 additional directories 同事务提交。损坏 JSON/enum 不能静默
  映射成正常业务值。
- 公开 run lifecycle 与 prompt 使用同一 operation owner；任何 finalization
  CAS 失败必须向调用者返回冲突，不能继续报告 prompt 成功。
- cancel 必须在发送 `session/cancel` 前，以 exact run/conversation CAS 将
  持久化状态从 `running` 取得为 `cancelling`，并与 prompt worker 的 terminal
  finalization 串行化。已经 terminal 或已经 cancelling 的 run 不得再次发送
  通知；通知发送失败必须把 operation flag、runtime state、run 和 conversation
  状态一起回滚到可重试的 running/live 状态，回滚失去 ownership 时显式失败。
- 动态 current projection 不得使用会在 replay commit 间漏行的裸 offset。
  message traversal 使用 generation-aware keyset/snapshot cursor。
- Store 为每个 conversation 持久化单调递增的 projection generation，并在
  replay membership commit 的同一事务内递增。opaque cursor 必须绑定
  conversation、generation、last key、include-audit、run/filter identity；
  restart 后仍可校验，查询身份变化或 projection replacement 返回 stale
  cursor conflict。普通 tail append 不改变已有 keyset traversal。
- load/new 的 config options 与 modes 独立保存 presence；modes-only response
  不能丢失。一次成功 refresh 是 static snapshot 的完整 replacement：
  本次未出现的旧 plan/commands/usage/config/modes 不再是 current。
- `session/new` 在请求发出前按 agent 建立有界通知隔离，并持有当前 connection
  generation 直到本地 publication 或 rollback 完成。同一 agent 不能并发执行
  两个 `session/new`。返回 session id 后，只能发布匹配 id 的通知；既有 bound
  session 的非匹配通知按原 owner 重放。远端未返回 id 或本地 publication
  失败时，归属未知的通知必须丢弃，不能污染后续 retry。conversation row、
  static snapshots、binding、runtime state 与匹配通知必须作为一个 owner-aware
  的本地 publication 单元；失败只能清理本操作实际取得 ownership 的状态。
- 标准 `SessionInfo.updated_at` 与 opaque vendor `_meta` 与 session id、title、
  cwd、additional directories 一起进入 Agent Original 静态 snapshot。
  ordinary public read 对 `_meta` 使用 size/type 校验和 privacy projection，
  不能因不展示原文而丢弃原始层数据。

## 10. Protocol Budgets, Paths, Capabilities, and Public Privacy

- daemon admission 同时限制 in-flight 数量和全局跨客户端 128 MiB retained
  RPC budget，并固定分为 request 87 MiB、ordinary response 40 MiB、terminal/
  fallback 1 MiB。request 按实际读取字节渐进 admission，覆盖解析与 dispatch；
  response reservation 保留到慢 writer flush 完成。单帧仍为 32 MiB，读取阶段
  frame slot 不能替代总预算；独立 fallback 分区保证 response 饱和时仍能返回
  有界 terminal error。
- ACP `session/list` 限制最多 256 pages、20,000 received sessions、单 cursor
  8 KiB、累计 canonical serialized input 64 MiB。所有收到的 item/cursor 在
  dedupe 前计费，再按 `(agent_id, session_id)` 去重；超限返回 typed
  `ResourceLimit`。
- agent 返回的 cwd 与 additional directories 在协商/`session/list` 后、
  任何持久化和 `session/load` 前必须是绝对路径。
- prompt 中的 image、audio、embedded resource 分别要求 agent 宣告相应
  capability；initialize 可以先发生，但拒绝必须早于 ensure-live-session、
  config/mode、`session/prompt`、创建 run 和写入用户消息。
- endpoint registry 的普通 CLI/MCP/daemon inspection DTO 不返回 stdio command
  原文、argument/env/header 值或 URL path/query/userinfo。stdio allowlist 为
  transport type、`<redacted-command>`、redacted argument placeholders/count、
  env key names、proxy chain、permission/client capability flags；filesystem
  `allowed_roots` 是本地授权边界，字段与绝对路径均不得进入 ordinary read。
  HTTP/WS allowlist
  为 scheme/host/port/redacted path 与 header names。agent id 和脱敏 cached
  capabilities 可返回。conversation/session 的 canonical cwd 不属于该
  registry-inspection 禁止范围。写入接口仍接收完整配置。
- proxy flow 的正确性必须由真实 bounded physical legs 验证，不能由绕过
  `FlowBudget` 的 in-process fixture 代替。每个物理 leg 使用 monotonic token、
  canonical semantic identity 和 retained bytes 记账；notification 绑定 method+
  params，response 绑定 canonical result/error。identity 不匹配必须显式失败；
  同 identity 的歧义只释放最小 reservation，确保只能保守高估、不能低估。
- bounded stdio 在完整 newline 出现前也必须按实际 consumed wire bytes 渐进
  占用 aggregate partial budget；完整 frame 在同一 flow-budget 临界区将
  partial 原子转换为正式 physical reservation，parse/EOF/error/drop 均释放。
- daemon client 的 broadcast receiver 报告 lag 时，**默认继续连接与 in-flight
  RPC**（Product-UX Store-first / agent-managed 2026-07-24）：仅记录 live
  stream gap 警告。Store 仍是 durable 真相（capture 先写库再 fan-out）；
  操作者用 `conv show` / search 读 Hub Store，**不得**把 lag 叙述为
  「投影不完整需 resync」或强迫外部 agent 刷新。禁止把「杀连接」作为默认
  策略；历史 R-DAEMON-004 的 connection-fatal 行为已为 Store-first UX 让位。
- session unbind、delete、endpoint replacement 或 revoke 必须先从 active
  terminal table 摘除属于失效 owner 的 handle，在 terminal lock 外做
  process-tree cleanup。cleanup 失败不能重新占用 terminal quota 或 daemon
  activity；仍绑定、可访问 terminal 的显式 kill/release 仍保留可重试语义。
- CLI、MCP 与嵌入式 `HubClient` 在返回可用 client 前必须执行无副作用的
  daemon RPC contract handshake。旧 daemon 缺方法、malformed response 或
  exact version 不匹配时，任何 business RPC 都不得发送。
