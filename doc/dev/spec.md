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
- 原则上使用最新稳定版本包
- 项目本体很小，不做 MVP，直接给出完整实现
- 与 openab 等参考库没有直接关系

## 6. Scope Boundary

- ACP v1 only（协商其他大版本则断开）
- ACP 协议层/传输层/conductor/MCP 集成/测试 fixtures 中不重复造轮子——使用官方 `agent-client-protocol` rust-sdk
- 不涉及 agent 内部存储的直接读取（如 Cursor 的 state.vscdb）——仅通过 ACP 协议交互
