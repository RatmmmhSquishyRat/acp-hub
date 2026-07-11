# Cursor ACP Adapter — Specification (v4, extension adapter)

> Grounded in: `doc/ssot/pillars/README.md` (Spec 1-5, design 1-5, FAQ lines 36-41)
> Parent spec: `doc/dev/spec.md`
> Supersedes:
> - v2(已删除代码):直接读写 IDE `state.vscdb` 的自制 adapter(克隆
>   bubble、伪造 composerData、session/prompt 只写库无 AI 回复)。方向性
>   错误——逆向内部 schema、写入非官方数据、无回复语义。
> - v3:直连官方 `cursor-agent acp`,无 adapter。协议正确,但只覆盖
>   Cursor 三个会话空间之一(ACP 空间),无法列出/查看/续接 CLI 交互
>   会话与 IDE 桌面端会话。

## 1. Purpose

Cursor 官方 CLI(`cursor-agent`)的 `acp` 子命令是一个完整 ACP agent,
但只管理自己的 ACP 会话空间。Cursor 实际有三个互相隔离的会话存储。
本 adapter(`adapters/cursor/adapter.mjs`,Node ≥ 22)**代理官方 agent
并扩展其会话空间覆盖**——对应 pillar Spec 1 的第二种形态:"也可以自己
直接上手, 为某个agent client开发一个ACP adapter程序并注册"。

## 2. Session Space Model(2026-07-05 实验实证)

| 空间 | 存储 | 读(list/load) | 写(prompt) |
|------|------|----------------|-------------|
| **acp** | `~/.cursor/acp-sessions/<id>/`(meta.json + store.db,与 cli 同构) | adapter 只读解析(本地枚举 + 回放;上游 `session/list` 不可靠,见 §8) | 上游透传(流式、modes、models、工具、权限;需 auth) |
| **cli** | `~/.cursor/chats/<md5(workspacePath)>/<chatId>/`(meta.json + store.db) | adapter 只读解析 | `cursor-agent --resume <id> --mode ask -p ... <prompt>`,**只读续接历史**(实证:能回答会话早前内容) |
| **ide** | `%APPDATA%/Cursor/User/globalStorage/state.vscdb`(composerData/bubbleId 键) | adapter 只读解析 | **拒绝**,返回 -32602 及原因说明 |

**实验事实(决定性约束)**:

1. `--resume <chatId>` 按 **md5(workspacePath) 桶**查找 chat。从非原
   workspace 的 cwd 执行时,cursor-agent **静默新建**一个同 id 空 chat
   到另一桶——假成功 + 存储污染。adapter 必须校验 spawn cwd 的 md5 等于
   chat 所在桶名,否则拒绝执行。
2. 对 IDE composer id 执行 `--resume` 同样触发陷阱 1,且**不会**载入
   IDE 对话历史(实证:20 条消息的 IDE 对话,resume 后模型自述"无法访问
   此前的对话历史")。因此 IDE 会话发消息不可行,必须拒绝。
   **加重实证(2026-07-05 二次实验)**:即使从 IDE 会话所属 workspace 的
   正确 cwd 执行 `--resume <ide-composer-id>`,仍然 (a) 回答与原对话完全
   无关的内容(未载入任何历史);(b) 在 `~/.cursor/chats/` fork 同 id 空
   chat;(c) **破坏性覆盖**共享的 per-project transcript 镜像
   `~/.cursor/projects/<project>/agent-transcripts/<id>/<id>.jsonl`
   (367KB 完整对话镜像被整体覆盖为仅含 resume 那一轮的 503B 文件;
   state.vscdb 主存储无损,IDE UI 历史不受影响)。IDE prompt 拒绝
   不仅防污染,还防真实数据破坏。
3. CLI chat 的 `store.db`(SQLite)`blobs` 表含明文 JSON 消息记录
   (`{"role":"user"|"assistant","content":...}`),按 rowid 有序;
   `meta` 表 hex 编码 JSON 含 name/mode。只读解析可完整回放历史。
4. IDE `state.vscdb` 的 `composerData:<id>` 含
   `fullConversationHeadersOnly` 有序 bubble 引用;`bubbleId:<id>:<bid>`
   含 text/richText,type 1=user、2=assistant。只读解析可完整回放。
5. ACP 会话目录(`~/.cursor/acp-sessions/<id>/`)与 cli chat **同构**
   (meta.json + store.db,blob 为 `{"role","content"}` 明文 JSON,user
   消息同样以 `<user_info>` 注入 + `<user_query>` 包裹)。故 acp 空间的
   list/load 可复用 cli 的只读解析路径。**上游 `cursor-agent acp
   session/list` 对磁盘 ACP 会话不可靠**(2026-07-09 实测:磁盘 5 个,
   上游返回 0;与论坛 #158388 / Zed #56246 一致),且上游 `session/load`
   要求 auth(即便 `agentCapabilities` 未广告 `authentication` 字段)。
   因此 acp list/load 改为**本地只读**(与 cli/ide 一致),查看 ACP 历史
   不依赖 auth、不受上游 flakiness 影响;仅 acp `session/prompt`(实时
   续接)透传上游并需 auth。

**所有 acp/cli/ide 存储访问严格只读**(SQLite readOnly 连接)。v2 的写库
路线仍然禁止。

## 3. Architecture

```
Hub ──stdio JSON-RPC──> adapter.mjs ──stdio JSON-RPC──> cursor-agent acp (upstream)
                          │  ├─ session/list: 上游结果 + acp/cli/ide 会话合并(末页合并,按 id 去重)
                          │  ├─ session/load: acp(磁盘)/cli/ide→本地只读回放; acp(仅上游 live)/未知→透传
                          │  ├─ session/prompt: acp→透传; cli→headless resume 子进程; ide→拒绝
                          │  ├─ session/set_mode|set_config_option: acp→透传; cli/ide→拒绝
                          │  ├─ session/cancel: cli prompt 运行中→kill 子进程; 否则透传
                          │  └─ 其余(initialize/new/authenticate/…)→透传
                          ├─(只读)~/.cursor/chats/**/store.db
                          └─(只读)%APPDATA%/Cursor/.../state.vscdb
```

- 双向透传:client→上游请求按原 id 转发;上游→client 的请求
  (`session/request_permission`、`cursor/*` 扩展)与通知原样转发,
  client 应答原样回传。adapter 自身从不向上游发起请求(除转发)。
- 路由判定 `classify(sessionId)`:acp-sessions 目录 > chats 桶 >
  composerData 存在性,优先级依次降低;未知 id 交上游,由上游给出
  权威错误。
- CLI prompt 流式:`stream-json` 中带 `timestamp_ms` 的 assistant 增量
  → `agent_message_chunk`;无增量时(格式漂移防御)回退到 result 全文。
  prompt 按 Cursor CLI 文档作为 positional argv 传入。Windows 直接启动
  bundle 内的 `node.exe index.js`,不经过 `cmd /c` / shim 的二次展开。
- 会话标题前缀 `[cli]` / `[ide]`,`_meta["cursor-adapter"].space` 标注
  空间,供 Hub 侧区分。

## 4. Pillar Alignment

- **Spec 2(全局搜索/列举/查看)**:三空间 list/load 全覆盖 →
  `agent sessions cursor --import` 后 Hub 全文搜索命中全部历史。✅
- **Spec 3(发送消息, 等待回复, 并查看回复)**:acp 与 cli 空间完整
  支持;ide 空间受官方能力限制仅只读——这是 endpoint 能力边界
  (pillar design 2),adapter 以明确错误暴露而非伪装成功。⚠️ 如实声明
- **Spec 2 的"增/删对话"**:上游未声明 session/delete/close,cli/ide
  空间同样不提供删除(避免写 Cursor 内部存储)。Hub `--local-only`
  删除投影可用。⚠️ 能力边界
- **FAQ 两层数据**:cli/ide 的 load 回放 = Layer 1(`load_replay`);
  经 Hub 发送时的流式捕获 = Layer 2(`local_turn`)。两层平行。✅

## 5. Registration

```json
{
  "transport": {
    "type": "stdio",
    "command": "node",
    "args": ["<abs>/adapters/cursor/adapter.mjs"],
    "env": {}
  },
  "permission_policy": "reject"
}
```

环境变量:`CURSOR_AGENT_CMD`、`CURSOR_DB_PATH`、`CURSOR_HOME`。
前置:`cursor-agent login`(上游广告 authMethod `cursor_login`)。

注意:headless resume(cli prompt)不能把工具审批回传到 ACP/Hub,因此
adapter 强制 `--mode ask` 只读运行,不传 `--trust`/`--force`。需要完整
工具与 Hub 权限流时,必须使用 live ACP session。

## 6. Error Handling

| 情形 | 响应 |
|------|------|
| cli/ide 会话不存在 | -32002 Session not found |
| ide 会话 prompt | -32602 + 拒绝原因(resume 陷阱说明) |
| cli prompt 但 cwd 无法匹配 chat 桶 | -32603 + 拒绝原因(防污染) |
| cli/ide 会话 set_mode / set_config_option | -32602 not supported |
| headless 子进程失败 | -32603 + exit code + result 错误文本 |
| 上游 session/list 失败 | 仍返回本地 cli/ide 会话(降级) |
| 上游进程退出 | adapter 跟随退出 |

## 7. Verification(2026-07-05 全部实测通过)

- `adapter-test.mjs` 直连 adapter:initialize 透传 / 三空间 list 合并
  (5 acp + 12 cli + 236 ide = 253)/ cli load 回放 / cli prompt 带历史
  上下文回复(`end_turn`)/ ide load 回放(32 条)/ ide prompt 明确拒绝。
- Hub 端到端:`agent sessions cursor`(253 会话)→
  `conv create --agent-session-id <cliChatId>`(Layer 1 回放)→
  `send`(流式真实回复,验证历史续接)→
  `conv create --agent-session-id <ideComposerId>`(IDE 历史回放)→
  `search`(命中 IDE 会话中文内容)。
- 2026-07-09 干净分支(`feat/cursor-acp-adapter-fix`,基于最新 main)
  只读冒烟复测通过:三空间 list 合并(5 acp + 12 cli + 303 ide = 320)/
  cli load 回放 10 chunks(含 `CLI-CHAT-OK` 种子)/ ide load 回放 3 chunks /
  ide prompt 明确拒绝。adapter 代码与 c5338c0 完全一致(cherry-pick 透传)。
- 2026-07-09 后续(`feat/cursor-acp-local-list`,基于已合并 main):ACP
  list+load 改本地只读后,冒烟复测 acp=5(稳定,不再依赖上游 flakiness)、
  acp load 回放 2 chunks(首条 `Reply with exactly: HUB-E2E-OK`,**免 auth**)、
  cli/ide load 与 ide 拒绝依旧通过。`adapter-test.mjs` 只读断言全过;live
  cli prompt 在本机因 cursor-agent 未登录返回 "Authentication required"
  (环境前置,非代码回归)。

## 8. Research & References(2026-07-09,实现后回溯校验设计取向)

本节为用户要求的"实现前完整研究 Cursor 文档/讨论/现有 adapter"的
证据留痕,用于校验本 adapter 的设计取向是否与官方语义及社区共识一致。

**官方文档**
- [Cursor ACP 官方文档](https://cursor.com/docs/cli/acp):`agent acp` 为
  stdio JSON-RPC 2.0 / NDJSON agent;auth 方法 `cursor_login`,可经
  `agent login` / `CURSOR_API_KEY` / `CURSOR_AUTH_TOKEN` 预登录;modes
  `agent`/`plan`/`ask`;权限 `session/request_permission` →
  `allow-once`/`allow-always`/`reject-once`;扩展方法
  `cursor/ask_question`、`cursor/create_plan`(阻塞)、`cursor/update_todos`、
  `cursor/task`、`cursor/generate_image`(通知)。本 adapter 对这些一律
  透传(default 分支 + `method===undefined` 回传),与官方语义一致。
- [ACP 协议规范](https://agentclientprotocol.com/protocol/v1/overview):
  `session/load` 必须**经 `session/update` 的 `user_message_chunk` /
  `agent_message_chunk` 回放历史**——这是下述 gap 判定的规范依据。

**`session/load` 历史回放 gap(已被官方修复)**
- [Cursor 论坛 #158388](https://forum.cursor.com/t/acp-no-conversation-history-is-restored-when-loading-an-existing-session/158388)
  (2026-04-18 报告):`session/load` 不回放消息 chunk,违反 ACP 规范;
  仅返回 modes/models/configOptions + `available_commands_update`。
  **2026-06-07 确认 `2026.06.04-5fd875e` 已修复**(上游 ACP session/load
  现会回放历史)。
- [Zed issue #56246](https://github.com/zed-industries/zed/issues/56246)
  (2026-05-09):同源问题;指出 `~/.cursor/acp-sessions/` 当时不存在、
  原生 transcript 在 `~/.cursor/projects/.../agent-transcripts/*.jsonl`;
  社区建议"客户端侧缓存回放"作为 workaround。
- **对本 adapter 的影响**:上游 `session/list` 对磁盘 ACP 会话不可靠
  (2026-07-09 实测磁盘 5 个、上游返回 0),且上游 `session/load` 要求 auth
  (`agentCapabilities` 未广告 `authentication` 字段却返回 -32000
  "Authentication required")。故 acp 空间的 list 与 load 均**改为本地只读**
  (复用 cli 解析路径,与 cli/ide 一致):查看 ACP 历史不依赖 auth、不受上游
  flakiness 影响;仅 acp `session/prompt`(实时续接)透传上游并需 auth。
  §2 实证 2 的 IDE `--resume` 破坏性覆盖 `agent-transcripts/*.jsonl`
  与本 issue 指出的 transcript 路径相互印证。

**社区 adapter 参照(均为简单代理,不覆盖 CLI/IDE 空间)**
- [blowmage/cursor-agent-acp-npm](https://github.com/blowmage/cursor-agent-acp-npm)
  (TS,127★):桥接 `cursor-agent` 到 ACP,自带 `SessionManager` 会话存储,
  声明 session/new/load/list/delete/prompt——**自维护会话存储**而非读取
  Cursor 原生三空间。
- [chrisnharvey/cursor-acp](https://github.com/chrisnharvey/cursor-acp)
  (Node):modes、cancellation、auth 提示;不强调历史持久化。
- [oxiglade/cursor-acp](https://github.com/oxiglade/cursor-acp)(Rust,
  已 ARCHIVED):声明 session history persistence,同样自维护存储。
- **取向差异**:社区 adapter 在上游 session/load 缺失期(gap 修复前)以
  **自维护会话存储**绕开;本 adapter 反向选择**只读读取 Cursor 原生三空间
  存储**(ACP 目录 / CLI store.db / IDE state.vscdb),不引入第二份数据源、
  不与 Cursor 内部 schema 写入冲突,且是唯一覆盖 CLI 与 IDE 空间的实现。
  gap 修复后,自维护存储型 adapter 的持久化价值下降,而原生读取型
  (本 adapter)对 CLI/IDE 的覆盖价值不变。
