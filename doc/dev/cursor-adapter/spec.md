# Cursor ACP Endpoint — Specification (v3, official-endpoint rework)

> Grounded in: `doc/ssot/pillars/README.md` (Spec 1-5, design 1-5, FAQ lines 36-41)
> Parent spec: `doc/dev/spec.md`
> Supersedes: v2 (2026-06) — v2 设计了一个直接读写 Cursor IDE `state.vscdb`
> 的自制 adapter(克隆 bubble、伪造 composerData、不产生 AI 回复)。该方向
> 已整体废弃并删除代码:Cursor 官方 CLI 原生实现了完整 ACP agent,自制
> DB 桥接既冗余又危险(逆向内部 schema、写入非官方数据、无 AI 回复语义)。

## 1. Purpose

Cursor 官方 CLI(`cursor-agent`,别名 `agent`)通过 `cursor-agent acp`
子命令以 stdio newline-delimited JSON-RPC 运行一个**完整的 ACP Agent**
(真实 LLM 回复、工具调用、权限请求、会话管理)。

本集成**不需要任何 adapter 程序**。对应 pillar Spec 1 的第一种形态:
"像注册MCP一样, 配置注册自己喜欢的ACP Agent Endpoint" —— 直接把官方
endpoint 注册进 Hub,由 Hub 的 capability negotiation(pillar design 2)
决定可执行的操作面。

## 2. Registration

```json
{
  "acpAgents": {
    "cursor": {
      "transport": {
        "type": "stdio",
        "command": "cmd",
        "args": ["/c", "cursor-agent", "acp"],
        "env": {}
      },
      "permission_policy": "reject",
      "client_capabilities": {
        "fs": { "read_text_file": false, "write_text_file": false, "allowed_roots": [] },
        "terminal": false
      }
    }
  }
}
```

- Windows:`cursor-agent` 是 `.cmd` shim,stdio spawn 必须经 `cmd /c`
  (与 codex endpoint 相同模式)。POSIX 直接 `"command": "cursor-agent",
  "args": ["acp"]`。
- 认证前置:`cursor-agent login`(或 env `CURSOR_API_KEY`)。agent 在
  initialize 中广告 authMethod `cursor_login`;已登录时无需 `agent auth`。
- 模板文件:`adapters/cursor/agents.json`。

## 3. Negotiated Capability Surface(实测 CLI 2026.07.01)

initialize 响应(protocolVersion 1):

```json
{
  "agentCapabilities": {
    "loadSession": true,
    "mcpCapabilities": { "http": true, "sse": true },
    "promptCapabilities": { "audio": false, "embeddedContext": false, "image": true },
    "sessionCapabilities": { "list": {} }
  },
  "authMethods": [{ "id": "cursor_login", "name": "Cursor Login" }]
}
```

| Hub 操作 | ACP method | 结果 |
|---|---|---|
| conv create | session/new | ✅ 返回 modes(agent/plan/ask)、models、configOptions(mode/model) |
| send | session/prompt | ✅ 流式 agent_message_chunk / tool_call / plan / thought |
| conv show(load) | session/load | ✅ `loadSession: true`,历史回放为 Layer 1 |
| agent sessions | session/list | ✅ SessionInfo: sessionId/cwd/title/updatedAt |
| mode set | session/set_mode | ✅ agent / plan / ask |
| param set | session/set_config_option | ✅ `mode`、`model` |
| cancel | session/cancel | ✅ |
| conv close / delete | session/close, delete | ❌ 未声明 → Hub 依能力协商拒绝(`--local-only` 删除投影仍可用) |
| resume | session/resume | ❌ 未声明(用 load 代替) |

Spec 3(发送消息, 等待回复, 并查看回复)由官方 endpoint 完整满足 ——
v2 adapter 的 "delivery-only、无 AI 回复" 能力残缺不复存在。

## 4. Cursor Extension Methods

Cursor 会向 client 发送 `cursor/*` 扩展方法:

- Blocking requests:`cursor/create_plan`、`cursor/ask_question`
- Notifications:`cursor/update_todos`、`cursor/task`、`cursor/generate_image`

Hub 的 SDK client 对未注册方法返回 JSON-RPC -32601(method not found)。
实测 cursor-agent 对 -32601 优雅降级:plan 经由标准 `session/update`
(`sessionUpdate: "plan"`)仍被 Hub 捕获,prompt 正常以 `end_turn` 完成。
未来若要富化(如回答 ask_question),可通过 ACP Proxy 或 Hub 扩展处理,
非本 spec 范围。

## 5. Permissions

工具审批走标准 `session/request_permission`,由 Hub `permission_policy`
应答:

- `reject`(默认注册值):只读/纯对话可用;写文件与命令执行被拒。
- `auto-allow`:完整 agent 能力,风险自担。
- Cursor 的选项 kind 为 `allow_once` / `allow_always` / `reject_once`。

## 6. Two-Layer Data Model(FAQ lines 36-41)

- Layer 1(agent original):`session/list` + `session/load` 回放,Hub 存为
  `source='load_replay'`。Cursor CLI 的会话存储由 Cursor 自己维护,
  Hub 不触碰。
- Layer 2(hub capture):prompt 期间的 `session/update` 流,存为
  `source='local_turn'`。

两层平行展示,互不覆盖 —— 与 pillar FAQ 语义一致,无需任何特判。

## 7. Scope Notes

- **会话可见范围(2026-07-05 实测)**:Cursor 有三个互相隔离的会话存储,
  `cursor-agent acp` 的 session/list・load 只暴露第一个:
  1. **ACP 会话** — `~/.cursor/acp-sessions/<sessionId>/`。✅ 全部可见,
     可 list / load / prompt。
  2. **CLI 交互会话** — `~/.cursor/chats/<workspace-hash>/<chatId>/`
     (终端 UI、`create-chat`、`-p` headless)。❌ 不出现在 ACP
     session/list(实测:`create-chat` 建 chat 并写入消息后仍不可见)。
  3. **IDE 桌面端聊天** — `state.vscdb`。❌ 不可见。
  这是 endpoint 能力边界(pillar design 2),不是 Hub 缺陷;2/3 类会话
  如需接入属未来独立课题,且不应通过逆向存储写入实现。
- "搜索全部历史对话" 的语义因此限定为:全部 **ACP 空间** 会话。流程:
  `acp-hub agent sessions cursor --import`(Layer 1 全量导入)→
  `acp-hub search <关键字>` 全文命中 user/assistant 消息;经 Hub 发送的
  新消息由 Layer 2 实时捕获,无需再导入。
- MCP servers:endpoint 声明 `mcpCapabilities: {http, sse}`;此外
  cursor-agent 自身会读取项目/用户级 `.cursor/mcp.json`。

## 8. Verification

- `adapters/cursor/smoke-test.mjs`:直连 `cursor-agent acp` 冒烟
  (initialize → new → prompt → list → load)。
- `adapters/cursor/probe.mjs`:wire 形态诊断(capabilities、modes、
  configOptions、`cursor/*` 扩展方法行为、permission options)。
- Hub 端到端(2026-07-04 实测通过):
  `agent add` → `conv create` → `send`(流式回复 + end_turn)→
  `agent sessions --import`(Layer 1 回放)→ `conv show`(两层数据)→
  `mode list` / `param list`(modes 与 configOptions 投影)。
