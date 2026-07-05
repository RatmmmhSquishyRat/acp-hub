# Cursor ACP Endpoint

Cursor 官方 CLI (`cursor-agent`) 原生支持 ACP:`cursor-agent acp` 以 stdio
JSON-RPC (newline-delimited) 运行一个完整的 ACP Agent。本目录不包含任何
adapter 程序 —— 直接注册官方 endpoint 即可。

> 历史注记:本目录曾有一个直接读写 Cursor IDE `state.vscdb` 数据库的
> "adapter"(伪造 bubble、不产生 AI 回复)。该实现已整体删除,被官方
> `cursor-agent acp` 取代。见 `doc/dev/cursor-adapter/spec.md` (v3)。

## 前置条件

1. 安装 Cursor CLI(随 Cursor 桌面版分发,Windows 上通常在
   `%LOCALAPPDATA%\cursor-agent\`,`cursor-agent` / `agent` 已加入 PATH)。
2. 登录:`cursor-agent login`(或设置 `CURSOR_API_KEY` 环境变量)。
   验证:`cursor-agent status`。

## 注册

```sh
acp-hub agent add cursor --type stdio --command cmd --args /c cursor-agent acp
```

或使用本目录的 `agents.json` 作为模板合并到 `~/.acp-hub/agents.json`。

Windows 上 `cursor-agent` 是 `.cmd` 批处理,stdio spawn 必须经由
`cmd /c`(与 codex adapter 相同模式)。Linux/macOS 直接
`--command cursor-agent --args acp`。

## 能力(实测 2026-07,CLI 2026.07.01)

| 能力 | 支持 | 说明 |
|------|------|------|
| initialize | ✅ | protocolVersion 1, authMethods: `cursor_login` |
| session/new | ✅ | 返回 modes (agent/plan/ask) + configOptions (mode, model) |
| session/prompt | ✅ | 流式 `agent_message_chunk` / `tool_call` / `plan` 等 |
| session/load | ✅ | `loadSession: true`,回放历史消息 |
| session/list | ✅ | `sessionCapabilities.list`,返回 SessionInfo(含 cwd/title/updatedAt) |
| session/set_mode | ✅ | agent / plan / ask |
| session/set_config_option | ✅ | `mode`、`model`(全模型列表) |
| session/cancel | ✅ | 标准 ACP |
| session/resume, delete, close | ❌ | 未在 capabilities 中声明(Hub 按能力协商自动拒绝) |
| prompt image | ✅ | `promptCapabilities.image: true` |
| MCP servers | ✅ http/sse | `mcpCapabilities: {http, sse}` |

Cursor 还会发送 `cursor/*` 扩展方法(`cursor/create_plan`、
`cursor/ask_question` 等)。Hub 的 SDK 对未注册方法回复
method-not-found (-32601),实测 cursor-agent 会优雅降级并继续,
不影响 prompt 完成。

工具权限经由标准 `session/request_permission`,由 Hub 的
`permission_policy`(reject / auto-cancel / auto-allow)应答。默认
`reject`:只读对话可用,写文件/执行命令的工具会被拒绝。需要完整
agent 能力时改用 `auto-allow` 并自担风险。

## 会话可见范围(实测)

`session/list` 只暴露 **ACP 会话空间**(`~/.cursor/acp-sessions/`)。
CLI 交互会话(`~/.cursor/chats/`,含 `create-chat` / `-p` headless)与
Cursor IDE 桌面端聊天(`state.vscdb`)均不可见。搜索全部 ACP 历史:
`acp-hub agent sessions cursor --import` 后 `acp-hub search <关键字>`。

## 验证脚本

- `smoke-test.mjs` — 直连 `cursor-agent acp` 全流程冒烟:
  `node smoke-test.mjs`
- `probe.mjs` — wire 形态诊断(capabilities/modes/configOptions/扩展方法):
  `node probe.mjs`
- `list-check.mjs` — 验证某个 chat id 是否在 ACP session/list 可见:
  `node list-check.mjs <chatId>`
