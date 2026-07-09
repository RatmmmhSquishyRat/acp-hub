# Cursor ACP Adapter

`adapter.mjs` 代理 Cursor 官方 ACP agent(`cursor-agent acp`),并在其上
扩展会话空间覆盖。官方 agent 只暴露自己的 ACP 会话;Cursor 实际有三个
互相隔离的会话存储,adapter 把它们统一接入 Hub:

| 空间 | 存储 | list/load | prompt |
|------|------|-----------|--------|
| acp | `~/.cursor/acp-sessions/<id>/`(meta.json + store.db,与 cli 同构) | adapter 只读(本地枚举 + 回放;上游 `session/list` 不可靠,见 spec §8) | 官方透传(完整 ACP:流式/modes/models/工具;需 `cursor-agent login`) |
| cli | `~/.cursor/chats/<ws-hash>/<chatId>/` | adapter 只读 | `cursor-agent --resume <id> -p` headless,真实续接历史(需 `cursor-agent login`) |
| ide | `%APPDATA%/Cursor/User/globalStorage/state.vscdb` | adapter 只读 | **拒绝**(见下) |

acp/cli/ide 存储严格只读,adapter 不写任何 Cursor 内部数据。acp 空间的
`session/load` 同样走本地只读回放(与 cli/ide 一致),因此**查看 ACP 历史
不需要 auth**;只有给 ACP 会话发消息(`session/prompt` 透传上游)才需要
`cursor-agent login`。

## 关键实验事实(2026-07-05)

- CLI chat 经 `--resume <id> -p` 收发消息**真实续接历史**(实证:能答出
  会话早前内容),`stream-json` 增量翻译为 `agent_message_chunk`。
- `--resume` 按 **md5(workspacePath)** 桶查找 chat。从其他 cwd resume
  会**静默新建同 id 空 chat**。adapter 因此强制校验 cwd 与 chat 桶
  一致,不一致直接报错而非污染。
- IDE composer id 用 `--resume` 同样触发上述陷阱且**不会**载入 IDE 对话
  历史,因此 IDE 会话 prompt 被明确拒绝,只提供 list/load(只读回放)。
- 二次实验(正确 cwd 下 resume IDE id)加重了结论:除 fork 污染外,还会
  **整体覆盖** `~/.cursor/projects/<project>/agent-transcripts/<id>/` 下的
  共享 transcript 镜像文件(数据破坏;state.vscdb 主存储无损)。

## 前置条件

1. Cursor CLI 已安装(Windows 通常在 `%LOCALAPPDATA%\cursor-agent\`)。
2. 已登录:`cursor-agent login`,验证 `cursor-agent status`。
3. Node.js ≥ 22(`node:sqlite`)。

## 注册

```sh
acp-hub agent add cursor --type stdio --command node --args "<此目录绝对路径>\adapter.mjs"
```

环境变量(可选):`CURSOR_AGENT_CMD`(launcher 路径)、`CURSOR_DB_PATH`
(state.vscdb 路径)、`CURSOR_HOME`(~/.cursor)。

## Hub 侧用法

```sh
acp-hub agent sessions cursor              # 三空间全量列表([cli]/[ide] 标题前缀)
acp-hub agent sessions cursor --import     # 全量导入 Layer 1,之后可全文搜索
acp-hub conv create cursor --agent-session-id <id> --cwd <dir>   # 绑定任意空间会话
acp-hub send <conv> --text "..."           # acp/cli 会话真实收发;ide 会话报错
acp-hub search <关键字>                     # 跨空间全文搜索(需先 import 或有捕获)
```

## 验证脚本

- `adapter-test.mjs` — adapter 全功能测试(三空间 list 合并/CLI 回放收发/
  IDE 回放与拒绝):`node adapter-test.mjs <cliChatId> <ideComposerId>`
- `smoke-test.mjs` — 直连官方 `cursor-agent acp` 冒烟(不经 adapter)
- `probe.mjs` — 官方 agent wire 形态诊断

## 历史注记

- v1/v2(已删除):直接读写 IDE `state.vscdb`(克隆 bubble、伪造
  composerData、无 AI 回复)。方向性错误。
- v3:直连官方 `cursor-agent acp`,无 adapter。正确但只覆盖 ACP 空间。
- v4(当前):官方 agent 之上的扩展 adapter,三空间统一接入。
