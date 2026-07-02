# OpenAB / ACP 讨论原文记录

> 说明：本文档为当前对话中关于 OpenAB、ACP 抽象层、对话管理能力的原文整理，便于下载后交给本地 agent 作为参考资料。未包含系统/开发者隐藏指令或模型内部推理。

---

## 1. 用户

查找几个名字类似agent chat room的github开原仓库, 他们在仓库中实现了discord bot adapter, 并且在底层完整实现了各种cli, codex, claude code, antigravity, opencode等的acp适配器, 在我看来已经超越这个仓库的范围, 可以拿来当作一个showcase了. 查找这个项目的具体信息.

## 2. 助手

我会直接按你给出的特征反向定位：项目名近似、Discord bot adapter、以及底层 ACP/CLI/Codex/Claude Code/Antigravity/OpenCode 适配器。定位后我会用仓库源码而不是项目简介来确认。

我定位到的核心项目是 **`openabdev/openab`，项目名 OpenAB — Open Agent Broker**。它最符合你描述：不是单纯“agent chat room”，而是一个把 **Discord / Slack / Gateway 聊天入口**桥接到 **ACP-compatible coding CLI** 的 Rust agent broker，README 明确列出 Kiro CLI、Claude Code、Codex、Gemini、OpenCode、MiMo-Code、Copilot CLI、Hermes、Grok、Antigravity、Pi 等后端，并通过 stdio JSON-RPC 走 Agent Client Protocol。([github.com](https://github.com/openabdev/openab))

容易混淆的另一个仓库是 **`bcurts/agentchattr`**。它名字和概念更像“agent chat room”：本地 chat server，让 Claude Code、Codex、Gemini CLI、Copilot CLI、Kimi、Qwen 等 agent 和人类在同一聊天室里交互，`@mention` 会把上下文注入对应 agent terminal。但它更像 local chat/MCP/terminal 协作工具，不是你描述的 Discord bot adapter + ACP adapter matrix。([github.com](https://github.com/bcurts/agentchattr)) 早先 Showcase 里的 **AI Agent Chat Room** 也只是“多个 AI agents 在聊天室里互相沟通”的展示线索，和你说的方向一致，但实现深度不如 OpenAB。([showcase.md](https://showcase.md/p/711641/ai-agent-chat-room/))

### OpenAB 的关键信息

**仓库状态**：`openabdev/openab` 是公开仓库，当前检索快照显示约 607 stars、156 forks、1,204 commits，语言几乎全是 Rust；最新 release 显示为 `openab-0.8.5-beta.13`，发布时间是 2026-06-19。这个数字会随 GitHub 变化。([github.com](https://github.com/openabdev/openab))

**总体架构**可以压缩成一句话：

> Discord / Slack / Gateway → OpenAB Rust broker → ACP stdio JSON-RPC → 各种 coding CLI agent。

README 里的架构图明确列出：`kiro-cli acp`、`claude-agent-acp`、`codex-acp`、`gemini --acp`、`copilot --acp`、`opencode acp`、`agy-acp`、`pi-acp` 等后端。([github.com](https://github.com/openabdev/openab))

**后端适配矩阵**很完整：Claude Code 走 `@agentclientprotocol/claude-agent-acp`，Codex 走 `@zed-industries/codex-acp`，Gemini/OpenCode/Kiro 等使用各自 native ACP 模式，Antigravity 通过仓库内的 `agy-acp` 适配器接入。([github.com](https://github.com/openabdev/openab))

**Discord adapter 是实打实做了的**：源码里 `src/discord.rs` 基于 `serenity` 实现 `DiscordAdapter`，支持发消息、reply、edit、delete、thread 创建、reaction、thread rename/archive；事件处理里做了 mention 检测、thread 检测、allowlist、bot-loop 防护、多 bot 检测、附件处理、音频 STT、图片/文件转 content block，然后交给 dispatcher。([github.com](https://github.com/openabdev/openab/raw/refs/heads/main/src/discord.rs))

**平台抽象也做得比较干净**：`src/adapter.rs` 定义了平台无关的 `ChatAdapter` trait、`ChannelRef`、`MessageRef`、`SenderContext`，`AdapterRouter` 负责把聊天平台消息路由到 ACP session，并处理 streaming、thinking、tool events、final output、`[[reply_to:id]]` 指令等。([github.com](https://github.com/openabdev/openab/raw/refs/heads/main/src/adapter.rs))

**ACP 底层不是伪适配**：`src/acp/connection.rs` 负责 spawn CLI 子进程，通过 stdin/stdout 读写 JSON-RPC line，发 `initialize`、`session/new`、`session/prompt`，处理 `session/request_permission`，支持 config option、session cancel/load、stderr reader、进程组 cleanup。也就是说它真的把聊天入口变成了 ACP client。([github.com](https://github.com/openabdev/openab/raw/refs/heads/main/src/acp/connection.rs))

**Antigravity 的适配尤其像 showcase**：仓库里的 `agy-acp` README 明说它是 Antigravity CLI 的 ACP adapter，会把 OpenAB 的 stdio JSON-RPC 转成 `agy -p "prompt"` 调用，用 SQLite 追踪 conversation id，并从 Antigravity protobuf 的 `step_payload` 字段提取响应，还带 E2E 测试。([github.com](https://github.com/openabdev/openab/tree/main/agy-acp))

**Gateway 也超出 Discord bot 范围**：OpenAB 的 Custom Gateway 是单独服务，用 WebSocket 把 Telegram、LINE、Feishu/Lark、Google Chat、WeCom 或自定义 webhook 事件规范化成 `openab.gateway.event.v1`，再转给 OpenAB；这样 OpenAB 本体保持 outbound-only、platform-agnostic。([github.com](https://github.com/openabdev/openab/tree/main/gateway))

### 我的判断

你说“已经超越这个仓库范围，可以拿来当 showcase”，这个判断是对的。**OpenAB 实际上不是 agent chat room demo，而是一个 chat-plane → ACP control-plane broker**：上层接 Discord/Slack/Gateway，下层接 Claude Code/Codex/Gemini/OpenCode/Antigravity/Pi 等 agent runtime，中间抽象出统一 session、streaming、tool/thinking events、权限请求、附件、reaction/status、多 agent 协作。

最值得研究的代码路径是：

```text
src/discord.rs
→ src/adapter.rs
→ src/dispatch.rs
→ src/acp/connection.rs
→ agy-acp/
```

一句话结论：**你要找的项目基本就是 `openabdev/openab`；`agentchattr` 是名字和概念相似的轻量 chat-room 参照物，但真正实现 Discord bot adapter + 多 CLI ACP 适配器矩阵的是 OpenAB。**

---

## 3. 用户

你着重说说openab对于acp方面的实现, 完整彻底调查源代码, 给出完整分析.
我在乎的部分不是这个项目和聊天bot接驳有多好, 而是他们的acp adapter, acp abstract framework, 都做了哪些工作, 为了对于各个acp进行适配, 都做了哪些工作, 其中最低熵, 最优价值和含金量的技术逻辑在于什么.
完整调查, 然后深入浅出地回答对于我的最小完整必要须知.

## 4. 助手

我会把重点限制在 ACP 层：协议连接、session 抽象、adapter wrapper、各 CLI 的命令/状态/事件差异处理。聊天平台层只作为入口背景，不展开评价。

结论先行：**OpenAB 的 ACP 含金量不在“为每个 CLI 写一套 adapter”，而在于把所有 agent runtime 压到一个极小边界：`stdio JSON-RPC ACP child process + durable session pool + normalized event stream`。**  
也就是说，它把 Claude Code / Codex / Gemini / OpenCode / Kiro / Antigravity 等差异，尽量收束成 `command + args + env + working_dir + auth/persistence + 少量协议兼容补丁`，而不是在主程序里堆大量 agent-specific 分支。OpenAB README 自己的架构图也就是：OpenAB Rust broker 通过 ACP stdio JSON-RPC 对接 `kiro-cli acp`、`claude-agent-acp`、`codex-acp`、`gemini --acp`、`opencode acp`、`agy-acp`、`pi-acp` 等后端。([github.com](https://github.com/openabdev/openab))

### 1. OpenAB 的 ACP 核心抽象是什么

最低熵模型如下：

```text
ContentBlock[]
  ↓
SessionPool(thread_key)
  ↓
AcpConnection(child process over stdio JSON-RPC)
  ↓
initialize / session/new / session/load / session/prompt / session/cancel
  ↓
AcpEvent: Text / Thinking / ToolStart / ToolDone / ConfigUpdate / Status
```

源码上，这套东西主要在：

```text
src/acp/protocol.rs
src/acp/connection.rs
src/acp/pool.rs
src/acp/agentcore.rs
src/adapter.rs
src/dispatch.rs
agy-acp/
```

`src/acp/protocol.rs` 做的是 **ACP JSON-RPC 形状和事件归一化**：定义 request/response/error/config option，并把 `session/update` notification 分类成 `Text`、`Thinking`、`ToolStart`、`ToolDone`、`ConfigUpdate`、`Status`。这里有几个非常有价值的兼容细节：它兼容 Codex ACP error 里 `error.data.message` 的形态；它兼容 Kiro CLI 不走标准 `configOptions`、而是返回 `models/modes` 的形态；它还专门处理 Claude adapter 的 `toolCallId` 问题，因为 Claude 可能先发一个还没有完整 input/title 的 tool event，后面再通过 `tool_call_update` 精炼。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/protocol.rs))

`src/acp/connection.rs` 是真正的 **ACP transport kernel**：spawn 一个 CLI 子进程，清空继承环境，重新注入必要的 HOME/PATH/USER 等基础变量，再按配置注入显式 env；stdin/stdout/stderr 全部管起来；stdout 按行读 JSON-RPC；用 pending map 匹配 request id；自动处理 `session/request_permission`；暴露 `initialize`、`session_new`、`session_load`、`session_prompt`、`session_cancel`、`session/set_config_option` 等方法。它还做了进程组管理，Unix 下 `setpgid`，Windows 下 new process group，退出时 SIGTERM 后 SIGKILL，避免 CLI 子进程泄漏。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/connection.rs))

`src/acp/pool.rs` 是 **ACP session 生命周期抽象**：一个 thread key 对应一个 ACP session/child process；它把活跃连接、可取消 handle、suspended session、persisted session、session workdir、creating gate 都分开管理。关键点是：**进程只是缓存，ACP session id 才是持久状态**。OpenAB 会把 thread→sessionId 持久化到 `~/.openab/thread_map.json` / `session_meta.json`，空闲时可以杀进程，之后如果 agent 支持 `loadSession`，就用 `session/load` 恢复同一个上下文；如果 load 只是暂时失败，它不会静默新建一个无上下文 session 来误处理当前消息，而是保留旧 session id 并报错。这个设计非常值钱。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/pool.rs))

### 2. 它不是“每个 CLI 一个框架”，而是“三层适配”

OpenAB 对不同 agent 的处理可以分成三类。

#### A. Native ACP CLI：OpenAB 几乎不关心 agent 是谁

Kiro、Gemini、OpenCode、MiMo、Copilot、Cursor、Hermes、Grok、Pi 这类后端，只要最终能以 stdio JSON-RPC 方式跑 ACP，OpenAB 就只是配置 `command/args/working_dir/env`。README 的 agent 表就是这个思路：Kiro 用 `kiro-cli acp`，Gemini 用 `gemini --acp`，OpenCode 用 `opencode acp`，Copilot 用 `copilot --acp --stdio`，Grok 用 `grok agent stdio`，Pi 用 `pi-acp`。([github.com](https://github.com/openabdev/openab))

这里的关键不是“支持了很多名字”，而是 **OpenAB 主体没有为每个名字写协议分支**。只要这个程序满足 ACP stdio contract，它就可以被当作同一种 `AcpConnection`。OpenCode 文档甚至明确说它原生支持 `acp` 子命令，不需要额外 adapter；OpenAB 只需要启动 `opencode acp`。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/docs/opencode.md))

#### B. External ACP adapter：Claude / Codex 不是 OpenAB 自己重写的

Claude Code 走 `@agentclientprotocol/claude-agent-acp`；OpenAB 的 Claude 镜像只是安装 `claude-agent-acp` 和 `@anthropic-ai/claude-code`，然后把 command 设成 `claude-agent-acp`。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/docs/claude-code.md))

Codex 走 `@zed-industries/codex-acp`；OpenAB 的 Codex 镜像安装 `codex-acp` 和 `@openai/codex`，推荐 command 是 `codex-acp`，并把 Codex 的 auth、sessions、generated images、skills 等持久化在 `/home/node/.codex/`。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/docs/codex.md))

所以对 Claude/Codex 来说，OpenAB 的价值不是“实现 Claude/Codex 协议”，而是 **把这些 adapter 作为普通 ACP child process 纳入统一 lifecycle、session pool、permission、streaming、timeout、cancel、env isolation**。Codex 文档里还暴露了很现实的无人值守问题：OpenAB deployment 没有人盯 terminal，所以 Codex 的 `approval_policy` / `approvals_reviewer` 要调成适合后台 agent 的形态，否则工具调用会卡到 hard timeout。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/docs/codex.md))

#### C. In-repo non-native adapter：Antigravity 的 `agy-acp` 是最像 showcase 的部分

`agy-acp` 是 OpenAB 仓库内真正实现的 adapter。文档说它是一个 thin Rust binary，把 ACP JSON-RPC 翻译成 `agy -p` 调用：第一轮 `agy --add-dir /home/agent -p "prompt"`，后续轮次 `agy --add-dir /home/agent --conversation <id> -p "follow-up"`，只把新增 delta 回传。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/docs/antigravity.md))

源码比文档更有意思。`agy-acp/src/adapter.rs` 会维护 `sessionId → conversationId / last_step_idx / model_id`，状态持久化到 `~/.openab/agy-acp/sessions.json`，还会扫描 Antigravity 的 SQLite conversation DB 目录来发现新 conversation；它还通过 `agy models` 获取模型列表，失败时用 cache，再失败才用 hardcoded fallback。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/agy-acp/src/adapter.rs))

`agy-acp/src/db.rs` 和 `protobuf.rs` 是最有技术含量的反向适配：它打开 Antigravity conversation SQLite DB，读 `steps` 表，筛 `step_type = 15` 的 response step，然后从 protobuf `step_payload` 里取 top-level field 20，再取 sub-field 1 作为 UTF-8 文本。它还识别一批 tool step type，并从 protobuf 里抽 tool name / input，用来生成 ACP `tool_call` / `tool_call_update`。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/agy-acp/src/db.rs))

`agy-acp/src/streaming.rs` 还做了轮询式 streaming：每 100ms poll SQLite DB，按 step idx 和已 emitted length 发增量 `agent_message_chunk`，并把工具 step 变成 ACP tool notifications。这里有个很重要的源码-文档差异：`docs/antigravity.md` 仍写着 “No streaming / cancel is a no-op”，但当前 `main` 源码里 `initialize` 已 advertises `streaming: true`，`execute_prompt` 也会在 cancel flag 触发时 kill child process。也就是说，文档在这一点上明显滞后于源码。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/docs/antigravity.md))

### 3. OpenAB 的 ACP abstract framework 真正做了哪些“难活”

第一，**把消息输入归一化成 ContentBlock，而不是字符串拼接**。`ContentBlock` 支持 text/image；进入 ACP turn 前，OpenAB 会把 sender metadata、用户文本、STT 文本、图片 block 等打包成一组 content blocks。这个边界很关键：上游平台怎么来不重要，下游 agent 只看到结构化 prompt blocks。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/connection.rs))

第二，**把输出归一化成 AcpEvent，而不是直接渲染 stdout**。Text chunk、thinking chunk、tool call、tool update、config update 都被转成内部事件，再交给上层展示。这意味着 UI 可以换，Discord/Slack/Gateway 都不是本质；本质是 ACP event stream。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/protocol.rs))

第三，**严肃处理后台 agent 的失败语义**。它有 request timeout，`session/new` 默认 120 秒，其他请求 30 秒；prompt 有 hard timeout；连接死掉会 abandon request；收到 stale id-bearing message 会过滤；cancel 不走主连接锁，而是用独立 cancel handle 直接发 `session/cancel`。这都是长期运行 agent broker 必须做、demo 项目往往不做的东西。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/connection.rs))

第四，**持久 session 与进程生命周期解耦**。OpenAB 可以 idle eviction，把活跃进程移到 suspended/persisted session；重启或池满之后再尝试 `session/load`。这比“每个 thread 永远养一个 CLI 进程”成熟很多，也比“重启就丢上下文”更像真正 infra。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/pool.rs))

第五，**权限请求默认走无人值守策略**。`session/request_permission` 会自动选择最宽松可用项：优先 `allow_always`，其次 `allow_once`，再选非 reject，否则取消；没有 options 时 fallback `allow_always`。这很适合 trusted sandbox / isolated pod 的自动化 agent，但安全边界必须放在外层容器、VM、网络和 secret policy 上。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/connection.rs))

第六，**配置选项不是硬编码 slash command，而是尽量走 ACP configOptions**。如果 agent 支持 `session/set_config_option`，OpenAB 直接发 configId/value；如果不支持，才 fallback 成 prompt 里的 slash-style command。Kiro 的 `models/modes` 非标准返回也被 parse 成标准 config options。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/connection.rs))

第七，**AgentCore bridge 说明它已经把 ACP 当成远程 runtime 协议了**。`src/acp/agentcore.rs` 把本地 ACP stdio 桥到 AWS Bedrock AgentCore Runtime 的 WebSocket shell，在远端 Firecracker microVM 里启动如 `kiro-cli acp --trust-all-tools`，并从带 PTY 噪声的输出中抽取 JSON object。这个不是聊天 bot 功能，而是把 ACP child process 从本地进程升级成远程隔离 runtime。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/agentcore.rs))

### 4. 最有价值的技术逻辑，压缩成几句话

**第一价值：不要适配 agent，适配 ACP process contract。**  
OpenAB 的核心选择是：agent backend 只要是一个能在 stdio 上讲 JSON-RPC ACP 的进程，就进入同一套生命周期。Claude/Codex 用外部 adapter，OpenCode/Gemini/Kiro 用 native ACP，Antigravity 用仓库内 `agy-acp` 把非 ACP CLI 包成 ACP。主程序不应该知道太多 agent 细节。([github.com](https://github.com/openabdev/openab))

**第二价值：session id 是真实状态，进程只是 transport cache。**  
这点对通用 agent harness 极其重要。你不能把“一个子进程还活着”当作上下文存在的唯一证明；应该持久化 session mapping，支持 load/resume，失败时宁可中止，也不要悄悄新建空上下文 session。OpenAB 在 `SessionPool` 里正是这么做的。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/pool.rs))

**第三价值：事件流必须规范化。**  
不同 agent 会发不同粒度的 tool/thinking/text/config update；OpenAB 把它们压成 `AcpEvent`，上层只消费统一事件。这个设计比直接把 stdout 丢给 UI 高级得多，也更容易迁移到你的场景。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/protocol.rs))

**第四价值：无人值守 agent 的工程语义比协议本身更难。**  
真正难的是 timeout、cancel、stale response、process group kill、env isolation、auth persistence、approval policy、session eviction、load failure handling。OpenAB 在这些地方投入的代码，比“能发一条 prompt 拿一条 response”的 demo 项目成熟很多。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/src/acp/connection.rs))

**第五价值：`agy-acp` 展示了如何把非 ACP 工具 ACP 化。**  
它不是简单 wrapper，而是做了 session→conversation 映射、SQLite conversation 追踪、protobuf payload 解析、tool step 抽取、模型 config option 暴露、轮询式 streaming/cancel。这个是最值得你拿来当 adapter showcase 的部分。([raw.githubusercontent.com](https://raw.githubusercontent.com/openabdev/openab/main/agy-acp/src/adapter.rs))

### 5. 对你的最小完整必要须知

你如果要借鉴 OpenAB 做更通用的 agent runtime，真正该抄的不是 Discord adapter，而是这个 ACP 骨架：

```text
AcpConnection:
  spawn configured command
  env isolation
  JSON-RPC pending map
  initialize/session/new/load/prompt/cancel
  permission auto-response
  stderr/process-group cleanup

SessionPool:
  thread/session key → AcpConnection
  persisted sessionId
  loadSession-aware resume
  idle eviction
  lock-free cancel
  workspace/session metadata

Protocol normalization:
  ContentBlock input
  AcpEvent output
  configOptions normalization
  tool/thinking/text/status event classifier

Adapter strategy:
  native ACP CLI: just command/args
  external ACP adapter: install binary, treat as child process
  non-ACP CLI: write a tiny ACP server wrapper like agy-acp
```

最终判断：**OpenAB 的 ACP 层已经不是“bot 项目顺便调用 CLI”，而是一个可复用的 ACP harness。它最强的地方是低熵：主框架只认 ACP process/session/event，具体 agent 的混乱性被限制在 adapter binary、镜像配置、auth/persistence 和少量兼容 shim 里。**

---

## 5. 用户

你给出OpenAB在acp抽象层给出的所有重要model, 以及interface. 让我能够slicing了解全貌, 你现在讲的太冗杂, 很难读懂.

## 6. 助手

我会改成“代码切片图谱”：只列 ACP 层的核心 model/interface、它们在哪个文件、职责、上下游关系。不会再按项目故事展开。

下面是 **OpenAB ACP 抽象层的 model / interface 切片图谱**。先给最低熵全貌：

```text
AgentConfig
  ↓
SessionPool(thread_id → session)
  ↓
AcpConnection(child process over stdio)
  ↓
JsonRpcRequest / JsonRpcMessage
  ↓
ACP methods:
  initialize
  session/new
  session/load
  session/prompt
  session/cancel
  session/set_config_option
  ↓
AcpEvent:
  Text / Thinking / ToolStart / ToolDone / ConfigUpdate / Status
```

OpenAB 的 ACP 层**不是 trait-heavy 的插件框架**；它没有定义一个通用 `trait AcpAdapter`。它的真实 interface 是：**一个可启动的子进程必须通过 stdio 讲 ACP JSON-RPC**；OpenAB 只负责 process lifecycle、session pool、JSON-RPC pending map、event normalization、config option normalization。这个判断来自 `src/acp/protocol.rs`、`connection.rs`、`pool.rs` 的结构。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/protocol.rs))

---

### 0. 模块切片

| 文件 | 层级 | 作用 |
|---|---:|---|
| `src/acp/protocol.rs` | 协议 model 层 | JSON-RPC request/response/message/error；ACP config option；ACP notification → `AcpEvent` |
| `src/acp/connection.rs` | transport/runtime 层 | spawn agent CLI；stdio JSON-RPC；pending request；reader loop；permission auto-reply；session methods |
| `src/acp/pool.rs` | session manager 层 | thread/session 映射；active/suspended/persisted session；resume/load；eviction；cancel/reset |
| `src/config.rs` | backend declaration 层 | agent command/args/env/working_dir/inherit_env |
| `agy-acp/src/*` | non-native adapter 层 | 把 Antigravity CLI 包装成 ACP server |

---

### 1. `protocol.rs`: ACP 协议模型

#### 1.1 JSON-RPC 基础模型

```rust
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    pub params: Option<Value>,
}

pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub result: Value,
}

pub struct JsonRpcMessage {
    pub id: Option<u64>,
    pub method: Option<String>,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
    pub params: Option<Value>,
}
```

这是 OpenAB ACP 层的最底层数据形状：**OpenAB 不关心 agent 类型，只关心 JSON-RPC method + params + result/error**。`JsonRpcRequest` 用于发往 agent，`JsonRpcMessage` 用于接收 response 或 notification。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/protocol.rs))

#### 1.2 Error 兼容模型

```rust
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn data_message(&self) -> Option<&str>
}
```

这个 model 的价值在于兼容 **Codex ACP** 一类 adapter：错误细节可能藏在 `error.data.message`，而不是标准 `error.message` 里。OpenAB 把这个兼容逻辑收在 `JsonRpcError::data_message()`，避免上层到处写 Codex 特判。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/protocol.rs))

#### 1.3 ConfigOption 模型

```rust
pub struct ConfigOptionValue {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub option_type: String,
    pub current_value: String,
    pub options: Vec<ConfigOptionValue>,
}
```

这是 OpenAB 对 **model / mode / agent profile** 的统一抽象。它不直接把“切模型”做成 slash command，而是优先归一化成 `ConfigOption`。标准 ACP 返回 `configOptions` 时直接解析；Kiro CLI 返回 `models/modes` 时，OpenAB fallback 解析成 `model` / `agent` 两个 config option。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/protocol.rs))

#### 1.4 AcpEvent：输出事件归一化模型

```rust
pub enum AcpEvent {
    Text(String),
    Thinking,
    ToolStart { id: String, title: String },
    ToolDone { id: String, title: String, status: String },
    ConfigUpdate { options: Vec<ConfigOption> },
    Status,
}
```

这是最重要的输出切片。不同 agent 的 notification 原始形态不同，但 OpenAB 上层只消费这几个事件：文本、思考、工具开始、工具结束、配置更新、状态更新。`classify_notification()` 会把 ACP `session/update` 中的 `agent_message_chunk`、`agent_thought_chunk`、`tool_call`、`tool_call_update`、`plan`、`config_option_update` 分类成这些事件。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/protocol.rs))

最低熵理解：

```text
agent stdout JSON-RPC notification
  → JsonRpcMessage
  → classify_notification()
  → AcpEvent
  → 上层 UI / dispatcher
```

---

### 2. `connection.rs`: ACP 子进程连接模型

#### 2.1 ContentBlock：输入模型

```rust
pub enum ContentBlock {
    Text { text: String },
    Image { media_type: String, data: String },
}

impl ContentBlock {
    pub fn to_json(&self) -> Value
}
```

这是发给 ACP `session/prompt` 的输入 block。OpenAB 不只发纯字符串，而是把 prompt 变成 `text` / `image` content block。这里是它未来支持多模态 agent 的最小抽象点。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

#### 2.2 AcpConnection：单个 agent runtime

```rust
pub struct AcpConnection {
    _proc: Child,
    child_pgid: Option<i32>,
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    notify_tx: Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>>,

    pub acp_session_id: Option<String>,
    pub supports_load_session: bool,
    pub config_options: Vec<ConfigOption>,
    pub last_active: Instant,
    pub session_reset: bool,

    _reader_handle: JoinHandle<()>,
    _stderr_handle: Option<JoinHandle<()>>,
}
```

这是 OpenAB ACP 层的核心对象：**一个 `AcpConnection` = 一个正在运行的 agent CLI 子进程 + 一个 ACP session 状态 + 一个 JSON-RPC pending map**。它保存 child process、stdin、request id、pending response、notification channel、当前 ACP session id、是否支持 `session/load`、当前 config options、最后活跃时间。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

#### 2.3 `spawn(...)`: 进程启动 interface

```rust
pub async fn spawn(
    command: &str,
    args: &[String],
    working_dir: &str,
    env: &HashMap<String, String>,
    inherit_env: &[String],
) -> Result<Self>
```

语义：

```text
AgentConfig.command + args
  → spawn child process
  → pipe stdin/stdout/stderr
  → clear env
  → re-inject HOME/PATH/USER/basic vars
  → selectively inject env/inherit_env
  → start reader loop
```

重要点：OpenAB 会 `env_clear()`，避免把 Discord token 等上层 secret 默认泄露给 agent；只显式传 `[agent].env` 和必要基础环境变量，并对 `inherit_env` 做显式白名单。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

#### 2.4 `run_reader_loop`: transport event loop

```rust
pub(crate) async fn run_reader_loop<R, W>(
    reader: R,
    writer: Arc<Mutex<W>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    notify_tx: Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>>,
)
```

职责：

```text
read stdout line
  → parse JsonRpcMessage
  → if session/request_permission: auto reply
  → if id matches pending: resolve response
  → else forward notification to subscriber
  → if connection closed: fail all pending
```

这个函数是 ACP runtime 的“泵”。它同时处理 response、notification、permission request、stale id-bearing message、connection closed。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

#### 2.5 AcpConnection 的 public protocol methods

##### `initialize`

```rust
pub async fn initialize(&mut self) -> Result<()>
```

发送：

```json
{
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientCapabilities": {},
    "clientInfo": {"name": "openab", "version": "0.1.0"}
  }
}
```

读取 agent 返回的 `agentCapabilities.loadSession`，写入 `supports_load_session`。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

##### `session_new`

```rust
pub async fn session_new(&mut self, cwd: &str) -> Result<String>
```

发送：

```json
{
  "method": "session/new",
  "params": {"cwd": "...", "mcpServers": []}
}
```

结果是拿到 `sessionId`，并解析返回里的 `configOptions`。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

##### `session_load`

```rust
pub async fn session_load(&mut self, session_id: &str, cwd: &str) -> Result<()>
```

发送：

```json
{
  "method": "session/load",
  "params": {"sessionId": "...", "cwd": "...", "mcpServers": []}
}
```

这是 OpenAB 让 agent session 脱离进程生命周期的关键接口：进程可以死，session id 可以持久化，之后用 `session/load` 恢复。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

##### `session_prompt`

```rust
pub async fn session_prompt(
    &mut self,
    blocks: Vec<ContentBlock>
) -> Result<(mpsc::UnboundedReceiver<JsonRpcMessage>, u64)>
```

语义：

```text
ContentBlock[]
  → JSON prompt array
  → send session/prompt
  → return notification receiver + request_id
```

它不会阻塞等完整 answer，而是返回一个 receiver，让上层持续消费 streaming notification。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

##### `set_config_option`

```rust
pub async fn set_config_option(
    &mut self,
    config_id: &str,
    value: &str,
) -> Result<Vec<ConfigOption>>
```

发送：

```json
{
  "method": "session/set_config_option",
  "params": {
    "sessionId": "...",
    "configId": "...",
    "value": "..."
  }
}
```

这是模型切换、模式切换的标准化接口。OpenAB 把不同 agent 的 model/mode 差异尽量压到 `ConfigOption`。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

##### `abandon_request` / `cancel_handle`

```rust
pub async fn abandon_request(&self, request_id: u64)
pub fn cancel_handle(&self) -> Arc<Mutex<ChildStdin>>
```

`abandon_request` 会删除 pending request，并 best-effort 发 `session/cancel` notification；`cancel_handle` 则把 stdin clone 出来，让 pool 可以在不锁住整个 connection 的情况下 cancel。这个是为了避免“streaming 时 connection mutex 被持有，cancel 反而卡住”的死锁式问题。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs))

---

### 3. `pool.rs`: ACP session pool 模型

#### 3.1 PoolState：真实 session 状态表

```rust
struct PoolState {
    active: HashMap<String, Arc<Mutex<AcpConnection>>>,
    cancel_handles: HashMap<String, (Arc<Mutex<ChildStdin>>, String)>,
    suspended: HashMap<String, String>,
    persisted: HashMap<String, String>,
    creating: HashMap<String, Arc<Mutex<()>>>,
    session_workdirs: HashMap<String, String>,
}
```

这是 OpenAB ACP 层最值得抄的 model。字段含义：

| 字段 | key | value | 作用 |
|---|---|---|---|
| `active` | thread_id | live `AcpConnection` | 当前活着的 agent 进程 |
| `cancel_handles` | thread_id | stdin + session_id | lock-free cancel |
| `suspended` | thread_id | session_id | 没有活进程，但可 `session/load` |
| `persisted` | thread_id | session_id | 写盘，进程重启后恢复 |
| `creating` | thread_id | mutex gate | 防止同一 thread 并发创建重复 session |
| `session_workdirs` | thread_id | canonical workdir | 每个 session 固定 workspace |

源码注释明确说：`PoolState` 用一个总锁保护，且不要在持有 pool state 时 await connection mutex，避免死锁。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs))

#### 3.2 SessionPool：对外 session manager

```rust
pub struct SessionPool {
    state: RwLock<PoolState>,
    config: AgentConfig,
    max_sessions: usize,
    mapping_path: PathBuf,
    meta_path: PathBuf,
}
```

`SessionPool` 负责把上游 thread 映射到下游 ACP session。它还维护两个持久化文件：`~/.openab/thread_map.json` 和 `~/.openab/session_meta.json`，分别保存 thread→sessionId 和 thread→workdir。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs))

#### 3.3 SessionPool 的核心 interface

##### `get_or_create`

```rust
pub async fn get_or_create(
    &self,
    thread_id: &str,
    working_dir_override: Option<&str>,
) -> Result<bool>
```

决策树：

```text
if active alive:
    reuse
else if suspended/persisted session_id exists and agent supports loadSession:
    spawn new AcpConnection
    initialize
    session/load
else:
    spawn new AcpConnection
    initialize
    session/new
```

最关键的安全语义：如果 `session/load` 是 transient failure，比如 timeout 或 channel closed，OpenAB 会保留原 session id，返回错误，**不会静默新建一个空上下文 session 来处理当前消息**。这是避免上下文错乱的关键设计。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs))

##### `with_connection`

```rust
pub async fn with_connection<F, R>(
    &self,
    thread_id: &str,
    f: F,
) -> Result<R>
```

这个 interface 是上层实际使用 connection 的入口。它只短暂持有 pool `RwLock` 查找 `Arc<Mutex<AcpConnection>>`，然后释放 pool lock，再锁单个 connection。这个 lock ordering 是 OpenAB 避免全局 session 池互相阻塞的关键。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs))

##### `get_config_options` / `set_config_option`

```rust
pub async fn get_config_options(&self, thread_id: &str) -> Vec<ConfigOption>

pub async fn set_config_option(
    &self,
    thread_id: &str,
    config_id: &str,
    value: &str,
) -> Result<Vec<ConfigOption>>
```

这是 pool 级别的 model/mode 操作入口，本质是转发到该 thread 对应的 `AcpConnection::set_config_option`。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs))

##### `cancel_session`

```rust
pub async fn cancel_session(&self, thread_id: &str) -> Result<()>
```

使用 `cancel_handles` 里的 stdin + session id，直接写：

```json
{
  "jsonrpc": "2.0",
  "method": "session/cancel",
  "params": {"sessionId": "..."}
}
```

这个设计绕过了 `AcpConnection` 的主 mutex，所以 streaming 正在进行时也能 cancel。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs))

##### `reset_session`

```rust
pub async fn reset_session(&self, thread_id: &str) -> Result<()>
```

语义：

```text
send session/cancel
remove active
remove cancel_handle
remove suspended
remove persisted
remove creating gate
remove session_workdir
save mapping/meta
```

它不是“清空 prompt”，而是彻底丢弃该 thread 的 ACP session 状态。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs))

##### `cleanup_idle`

```rust
pub async fn cleanup_idle(&self, ttl_secs: u64)
```

如果 active connection 超过 TTL 或不 alive，就从 `active` 移到 `suspended/persisted`，保留 session id 供之后 `session/load`。这体现了 OpenAB 的核心设计：**process 是 cache，session id 才是状态**。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs))

---

### 4. `config.rs`: agent backend declaration model

#### `AgentConfig`

```rust
pub struct AgentConfig {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: String,
    pub env: HashMap<String, String>,
    pub inherit_env: Vec<String>,
    pub command_explicit: bool,
}
```

这就是 OpenAB 支持 Claude Code / Codex / Gemini / OpenCode / Kiro / Antigravity 的低熵来源。它没有为每个 agent 写一个 Rust class；大多数 agent 只是不同的：

```toml
[agent]
command = "opencode"
args = ["acp"]
working_dir = "/workspace"
env = {...}
inherit_env = [...]
```

`AgentConfigRaw` 用 `Option` 判断 command/args 是否显式设置：如果显式设置了 command 但没有设置 args，就不会套用默认 args，避免把环境变量里的默认 args 泄漏到自定义 command。([github.com](https://github.com/openabdev/openab/blob/main/src/config.rs))

---

### 5. `agy-acp`: 非 ACP CLI → ACP adapter 的 model/interface

这部分是 OpenAB 最像 showcase 的 adapter，因为 Antigravity 本身不是标准 ACP 后端，所以它自己实现了一个小 ACP server。

#### 5.1 `agy-acp/src/types.rs`: adapter 内部模型

```rust
pub struct JsonRpcRequest {
    pub id: Option<Value>,
    pub method: Option<String>,
    pub params: Option<Value>,
}

pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

pub struct JsonRpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: Value,
}
```

注意这里的 `id` 是 `Value`，不是 `u64`。这说明 `agy-acp` 比主 OpenAB client 更宽松：它接受 string id 或 number id，适合作为通用 JSON-RPC server。([github.com](https://github.com/openabdev/openab/blob/main/agy-acp/src/types.rs))

#### 5.2 Antigravity session mapping

```rust
pub struct SessionStore {
    pub sessions: HashMap<String, StoredSession>,
}

pub struct StoredSession {
    pub conversation_id: Option<String>,
    pub last_step_idx: i64,
    pub model_id: Option<String>,
}

pub struct Session {
    pub conversation_id: Option<String>,
    pub last_step_idx: i64,
    pub model_id: Option<String>,
}
```

这是 `agy-acp` 的关键 model：**ACP session id ≠ Antigravity conversation id**。adapter 自己维护：

```text
ACP session_id
  → Antigravity conversation_id
  → last_step_idx
  → selected model_id
```

并持久化到 `~/.openab/agy-acp/sessions.json`。([github.com](https://github.com/openabdev/openab/blob/main/agy-acp/src/types.rs))

#### 5.3 StreamingState / PromptOutput

```rust
pub struct StreamingState {
    pub conversation_id: Option<String>,
    pub base_step_idx: i64,
    pub last_step_idx: i64,
    pub emitted_len: HashMap<i64, usize>,
    pub emitted_tool_steps: HashSet<i64>,
    pub had_updates: bool,
}

pub struct PromptOutput {
    pub response_lines: Vec<String>,
    pub session_update: Option<(Option<String>, i64)>,
}

pub struct StopGuard(pub Arc<AtomicBool>);
```

`StreamingState` 是轮询 Antigravity SQLite conversation DB 时的去重/增量状态：哪个 step 已经发过、文本发到哪个长度、哪个 tool step 已经发过。`StopGuard` 用 drop guard 在 task abort 时设置 stop flag，避免后台 polling 残留。([github.com](https://github.com/openabdev/openab/blob/main/agy-acp/src/types.rs))

#### 5.4 `Adapter`: Antigravity ACP server 主对象

```rust
pub struct Adapter {
    pub sessions: HashMap<String, Session>,
    pub working_dir: String,
    pub conversations_dir: PathBuf,
    pub state_file: PathBuf,
    pub available_models: Option<Vec<String>>,
}
```

它保存内存 session、工作目录、Antigravity conversation DB 目录、持久化 state file、模型缓存。模型列表通过 `agy models` 获取，失败则用 cache，再失败用 hardcoded fallback；然后暴露成 ACP `configOptions`。([github.com](https://github.com/openabdev/openab/blob/main/agy-acp/src/adapter.rs))

#### 5.5 `agy-acp` 对外 ACP method handlers

```rust
pub fn handle_initialize(&self, id: Value) -> JsonRpcResponse
pub fn handle_session_new(&mut self, id: Value) -> JsonRpcResponse
pub fn handle_session_load(&mut self, id: Value, params: &Value) -> JsonRpcResponse
pub fn handle_session_set_config_option(&mut self, id: Value, params: &Value) -> JsonRpcResponse
pub fn prepare_prompt_state(...) -> (...)
```

语义：

| handler | ACP method | 作用 |
|---|---|---|
| `handle_initialize` | `initialize` | 返回 protocolVersion、agentInfo、`streaming: true`、`loadSession: true` |
| `handle_session_new` | `session/new` | 生成 UUID session id，创建空 session，返回 configOptions |
| `handle_session_load` | `session/load` | 从 `sessions.json` 恢复 session→conversation 映射 |
| `handle_session_set_config_option` | `session/set_config_option` | 目前主要支持 `model` |
| `prepare_prompt_state` | `session/prompt` 前置准备 | 从 ACP prompt blocks 提取 text，构造 `agy --add-dir ... -p ...` 参数 |

这些 handler 说明 `agy-acp` 的本质是：**实现一层 ACP server facade，把 ACP session 操作翻译成 Antigravity CLI conversation 操作**。([github.com](https://github.com/openabdev/openab/blob/main/agy-acp/src/adapter.rs))

#### 5.6 `agy-acp/main.rs`: JSON-RPC dispatch interface

`agy-acp` 的 main loop 接收 stdin JSON-RPC，根据 method 分派：

```text
initialize
session/new
session/load
session/prompt
session/setConfigOption | session/set_config_option
session/cancel
```

`session/cancel` 既支持 notification form，也支持 request form；内部用 `active_cancellations: session_id → AtomicBool` 来通知正在运行的 prompt 取消。([github.com](https://github.com/openabdev/openab/blob/main/agy-acp/src/main.rs))

---

### 6. 最小完整 mental model

你可以把 OpenAB ACP 层切成 5 个面：

```text
[Backend Declaration]
AgentConfig {
  command, args, working_dir, env, inherit_env
}

[Runtime Transport]
AcpConnection {
  child process,
  stdin/stdout JSON-RPC,
  pending request map,
  notification receiver,
  acp_session_id
}

[Protocol Shape]
JsonRpcRequest / JsonRpcMessage / JsonRpcError
ContentBlock
ConfigOption
AcpEvent

[Session Lifecycle]
SessionPool / PoolState {
  active,
  suspended,
  persisted,
  cancel_handles,
  creating,
  session_workdirs
}

[Non-native Adapter Pattern]
agy-acp Adapter {
  ACP session_id
    → native CLI conversation_id
    → last_step_idx
    → model_id
}
```

一句话概括：**OpenAB 的 ACP 抽象不是“多 adapter OO 插件系统”，而是“进程协议边界 + session pool + event normalization”。所有 agent 的复杂性被压到 `command/args/env`、`ConfigOption`、`AcpEvent`、以及像 `agy-acp` 这种必要时才写的外部 ACP server wrapper。**

---

## 7. 用户

所以我理解, 他对于我所需要的所有对话层面的抽象都有缺失对吗?
对于对话的搜索, 对话的创建删除, 对话的当前运行状态显示, 对话的消息查看,
发送消息, 修改发送消息的参数, 等待消息完成回复,
这些主要功能的统一抽象层, openab是否具备?

## 8. 助手

对，你这个理解基本正确，但要加一个限定：

**OpenAB 有“ACP session runtime 抽象”，没有你需要的“Conversation CRUD / Message Store / Conversation Query API 抽象”。**

它能把一个外部 chat thread 映射成一个 ACP session，然后发 prompt、收 streaming、取消、重置、切 config。但它没有把“对话”建模成一个可搜索、可列举、可读消息、可独立管理的一级资源。

### 功能矩阵

| 你关心的能力 | OpenAB 是否具备 | 实际情况 |
|---|---:|---|
| 对话搜索 / 对话列表 | **不具备** | 没有统一 `ConversationRepository` / `listConversations` / `searchConversations`。源码里 `connection.rs` / `pool.rs` / `adapter.rs` 都没有 `session/list` 调用；ACP 官方最新 schema/RFD 已有可选 `session/list`，但 OpenAB 当前 ACP 层没有接。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs)) |
| 对话创建 | **半具备，内部隐式** | `SessionPool::get_or_create(thread_id)` 会按外部 thread id 自动创建或恢复 ACP session；底层调用 `session/new`。但这不是一个独立的“创建对话 API”，没有 title、metadata、createdAt、conversation object。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs)) |
| 对话删除 | **半具备，语义不完整** | `reset_session(thread_id)` 会 cancel、移除 active/suspended/persisted mapping，下次消息新建 session；但它没有调用 ACP `session/delete`，源码里也没有 `session/delete`。所以它只是“OpenAB 侧忘记/重置”，不是 backend conversation deletion。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs)) |
| 对话当前状态显示 | **半具备，非统一模型** | `PoolState` 有 `active / cancel_handles / suspended / persisted / session_workdirs`，也有 `has_active_session` 判断；adapter 层会根据 streaming/tool/thinking 更新状态。但没有统一 `ConversationStatus { idle, running, cancelled, failed, completed }` 查询模型。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs)) |
| 查看对话消息 | **不具备** | OpenAB 不保存 transcript/message history。它只把当前 turn 的 streaming text/tool events 拼成最终回复发回平台；历史消息要么在 Discord/Slack 等上游平台，要么在 Claude/Codex/OpenCode 等下游 agent 自己的 session 存储里。OpenAB 中间层没有统一 `Message` model。([github.com](https://github.com/openabdev/openab/blob/main/src/adapter.rs)) |
| 给对话发送消息 | **具备，内部能力** | `AcpConnection::session_prompt(ContentBlock[])` 发送 `session/prompt`，返回 streaming receiver 和 request id；adapter 层消费 receiver，直到收到最终 id-bearing response。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs)) |
| 修改发送消息参数 | **部分具备** | 主要是 `session/set_config_option(configId, value)`，用于模型/模式等 session-level config。它不是完整 per-message 参数抽象；没有统一的 temperature、approval policy、tool policy、system prompt、slash command 参数层。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/connection.rs)) |
| 等待消息完成回复 | **具备，但不是独立 API** | adapter 里拿到 `session_prompt` 返回的 `rx/request_id` 后循环消费 notification，分类成 `Text / Thinking / ToolStart / ToolDone / ConfigUpdate`，最终收到 prompt response 后 finalize。也就是“内部等待完成”，不是面向外部调用者的 `awaitMessage(conversationId, messageId)`。([github.com](https://github.com/openabdev/openab/blob/main/src/adapter.rs)) |
| 启动 / 停止对话 | **半具备** | 启动是隐式 `get_or_create → spawn → initialize → session/new/load`；停止当前运行是 `cancel_session` 发 `session/cancel`；重置是 `reset_session`。但没有 formal `startConversation / stopConversation / closeConversation` 抽象。([github.com](https://github.com/openabdev/openab/blob/main/src/acp/pool.rs)) |

### 最低熵结论

OpenAB 的真实抽象层是这个：

```text
thread_key
  → ACP sessionId
  → live/suspended child process
  → session/prompt streaming receiver
  → AcpEvent
```

你需要的是这个：

```text
Conversation {
  id
  title
  createdAt
  updatedAt
  status
  backend
  workspace
  params
  messages[]
}

ConversationService {
  list/search/create/delete/load
  getStatus
  listMessages
  sendMessage
  updateSendParams
  waitMessage
  cancelMessage
}
```

**OpenAB 没有后者。**

它只有一个很实用的 runtime broker：

```text
SessionPool
AcpConnection
ContentBlock
AcpEvent
ConfigOption
```

所以如果你的目标是做通用 agent harness / conscious emerge agent 管理层，OpenAB 可以借鉴的不是“对话管理抽象”，而是：

```text
1. ACP process lifecycle
2. sessionId 持久化
3. session/load 恢复
4. prompt streaming receiver
5. event normalization
6. lock-free cancel
7. config option 归一化
```

但你必须自己补一层：

```text
ConversationRegistry
MessageStore
Run/Turn model
ConversationStatus model
SearchIndex
Param/Profile abstraction
Backend capability matrix
```

一句话：**OpenAB 是 ACP runtime adapter，不是 conversation management framework。它能跑会话，但不管理“对话资产”。**
