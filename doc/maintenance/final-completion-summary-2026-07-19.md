# ACP Hub 最终维护总结

**日期**：2026-07-19
**范围**：接管既有未提交工作，核对项目 pillar、原始会话待办、维护 Task Plan、Review Book 和中断 handoff，完成修复、复核、本地验证与提交准备。

## 1. 起点与判断依据

本次工作开始时，仓库并不是一个可以直接发布的干净版本：

- 当前分支为 `codex/resolve-review-feedback`；
- 前序 agent 已留下大量未提交代码、文档、adapter 和 workflow 修改；
- `doc/review/complete-review-book-2026-07-18.md` 记录了原「已完成」判断中的 32 个缺陷；
- `doc/maintenance/continuation-handoff-2026-07-18.md` section 16 仍把 operation finalizer、RPC 修复和完整验证列为未完成；
- 项目 pillar 明确要求完整 ACP client/conductor、两层历史、真实 session CRUD、能力门控、官方 SDK 复用和可发布交付，不接受以 MVP、stub 或局部测试代替完整实现。

最终判断按以下证据顺序形成：

1. `doc/ssot/pillars/README.md` 与 `TechSel.md`；
2. `doc/ssot/dev-principles/实现规划原则.md`；
3. 当前代码、测试、adapter、workflow 和发布内容；
4. `complete-task-plan-2026-07-18.md` 与 Review Book；
5. 本次重新执行的本地命令和安装后用户流程。

## 2. 与会话最开始待办的对照

会话启动时维护了 14 项工作清单。后续因为发现 `hub.rs` 超过主动拆分边界，又增加了 Hub 生产模块、测试模块和边界复核任务。最终状态如下。

| 初始阶段 | 最初待办 | 最终结果 |
|---|---|---|
| Discovery | 映射仓库与当前工作树 | 完成。较早控制点记录 52 个 tracked 修改和 7 个 untracked 路径/目录；后续修复、拆分与记录继续增加候选差异。全程没有 reset、stash 或丢弃前序工作。 |
| Discovery | 恢复前序 agent 计划与 review 状态 | 完成。读取 Task Plan、Review Book、continuation handoff、开发五文档与 role 文档；section 16 的旧 restart order 已被 sections 17–19 取代。 |
| Discovery | 确认 pillar 与本地假设 | 完成。以 SSOT 为最高层级，移除私有路径、机器状态、虚构命令和超出实现的 capability/只读声明。 |
| Discovery | 复现构建和测试基线 | 完成。先复现局部失败，再按 root cause 修复 operation、RPC、adapter、release 和测试问题；最终执行完整本地矩阵。 |
| Design | 从证据定义维护范围 | 完成。范围覆盖协议/runtime、store/registry、CLI/MCP、adapters、文档/skill、安装和 release。 |
| Design | 比较安全的清理方法 | 完成。保留用户与前序 agent 的语义修改，只删除错误假设、泄漏信息、无界接口和失效说明；没有用 reset 或重写历史规避问题。 |
| Design | 确定接管设计 | 完成。以 Review Book finding、Task Plan ID 和项目 pillar 对应实现与验证，不用「编译成功」替代验收。 |
| Design | 记录维护设计与计划 | 完成。Task Plan、Review Book、continuation handoff、开发五文档和 implementer role 均已更新。 |
| Execution | 执行仓库维护任务 | 完成。Task Plan 的 T-000 至 T-602 共 26 项均为 `verified`。 |
| Execution | 对每项修改做规格复核 | 完成。协议/runtime、store、CLI/MCP、adapter、release/documentation 分 lane 复核；最终没有可确认的 Critical/High/Medium 缺陷。 |
| Execution | 复核集成代码质量 | 完成。Clippy `-D warnings`、完整测试和独立 review 通过；误报经源码或上游契约核验后未写入生产代码。 |
| Verification | 安装后端到端 smoke | 完成。隔离 `cargo install` 后执行 daemon-backed agent add/list/inspect/remove。 |
| Verification | 完整 build/lint/test gates | 完成。本地格式、Clippy、185 个 Rust tests、adapter fixtures、dependency policy、package dry-run 和 diff integrity 均通过。 |
| Verification | 最终文档与状态记录 | 完成。本文件、Task Plan appendix、Review Book final reconciliation 和 continuation handoff 共同记录最终状态。 |

追加的 Hub 拆分任务也已完成：

- 原 `crates/hub/src/hub.rs` 从约 3,900 行改为 21 行 facade；
- 生产代码拆分为 `hub/{types,state,registry,conversation,prompt,lifecycle,dispatch,client}.rs`；
- 测试拆分为 `hub/tests/{support,registry,client,operation,replay}.rs`；
- 最大文件为 `hub/tests/operation.rs`，726 行；所有 Hub 生产和测试文件均低于 900 行主动拆分边界；
- 公共 API 保持由 `crate::hub::{CoreHub, HubClient, ...}` 暴露。

## 3. 与维护 Task Plan 的对照

`doc/maintenance/complete-task-plan-2026-07-18.md` 共 26 项任务。当前没有 pending、blocked 或 partial 项。

| Phase | Task IDs | 数量 | 最终状态 | 主要结果 |
|---|---|---:|---|---|
| Baseline/control | T-000–T-001 | 2 | verified | 保存 live worktree；建立 Review Book、Task Plan 与证据边界。 |
| Protocol/callbacks/daemon | T-100–T-104 | 5 | verified | endpoint-scoped session、capability enforcement、ACP v1 验证、terminal I/O、同连接 cancellation、capture failure。 |
| Store/conversation/registry/search | T-200–T-204 | 5 | verified | transactional replay、两层历史、active-state protection、caller cwd、serialized registry mutation、统一分页。 |
| CLI/MCP | T-300–T-303 | 4 | verified | 安全公共 DTO、完整管理 surface、cwd/NDJSON/pagination 契约、真实进程 smoke。 |
| Adapters/docs/skill | T-400–T-404 | 5 | verified | 厂商写入边界、隔离 fixture、真实 CLI、五文档一致、可执行且最小权限的 skill。 |
| Installation/release | T-500–T-501 | 2 | verified | registry samples、安装说明、release allowlist、archive 内引用完整。 |
| Final verification | T-600–T-602 | 3 | verified | 本地矩阵、独立 review、完成/验证/发布/工作树状态分开报告。 |

Task Plan 早期 execution record 中的 `81 passed`、adapter `10/16` 和 core package `23 files` 是当时检查点，不再是最终计数。最终计数见本文件 section 7 和 Task Plan 的 final reconciliation appendix。

## 4. 与 Review Book 未完成部分的对照

Review Book 最初记录 32 个 finding：6 个 Critical、19 个 High、7 个 Medium。当前 resolution ledger 中 F-001 至 F-032 全部为 `resolved`。

| Finding | Severity | 最终处理 |
|---|---|---|
| F-001 | Critical | session/callback/terminal identity 改为 endpoint-scoped `SessionKey`，增加同 session id 跨 endpoint 隔离测试。 |
| F-002 | Critical | existing-session load 先建立 provisional parent projection；失败时回滚，不再让 replay 先于 conversation。 |
| F-003 | Critical | 初始化按配置发布 fs/terminal capability；callback 逐 endpoint/session/capability 校验。 |
| F-004 | High | 拒绝非 ACP v1 的 initialize response。 |
| F-005 | High | daemon 持续读请求、受限并发执行、单 writer 返回响应；同一连接可提交 cancel。 |
| F-006 | High | conversation create 要求 caller 提供绝对 cwd，不再继承 singleton daemon 启动目录。 |
| F-007 | Critical | Layer 1 replay 使用 begin/commit/rollback；refresh 保留 Layer 2 本地捕获并支持崩溃恢复。 |
| F-008 | High | active/cancelling conversation 删除返回 conflict。 |
| F-009 | High | run finalization 在同一事务中验证 run 的 conversation owner。 |
| F-010 | High | registry mutation 统一走 validate/save/swap；保护被引用 proxy 和 active agent；替换后清理旧 handle。 |
| F-011 | Medium | `agents.json` 明确为启动输入；运行时修改只走验证过的 Hub RPC，不再暗示外部文件热更新。 |
| F-012 | High | search 在统一结果集上应用 limit/offset，snippet 和 message page 同时受行数/字节预算限制。 |
| F-013 | High | callback persistence failure 进入 active load/run 结果，不能返回无条件成功。 |
| F-014 | High | stdout/stderr 并发 drain，不在 terminal registry lock 下做阻塞 I/O；增加 ownership/quota/drop cleanup。 |
| F-015 | Medium | Store 打开时终止孤立 running/cancelling run 并修复 conversation 状态。 |
| F-016 | Critical | CLI/MCP 共用 redacted public endpoint DTO；覆盖 env/header/URL/argument secret。 |
| F-017 | High | Hub home、registry、DB、daemon metadata、Unix socket 和 Windows named pipe 权限加固。 |
| F-018 | High | MCP 覆盖完整 endpoint config、session listing、cancel、分页和准确 tool annotation。 |
| F-019 | Medium | send 只返回 current-run output；历史读取必须经过 server-side row/byte-bounded page。 |
| F-020 | High | adapter 文档把只读 discovery/replay 与 vendor resume/delete 写入分开说明。 |
| F-021 | Critical | 默认 adapter probe 使用 synthetic home；live mutation 需要两个显式 opt-in；错误输出做隐私断言。 |
| F-022 | High | Grok initialize 的上游 JSON-RPC error 保持 error channel，不再包装成 result。 |
| F-023 | High | 文档/skill 使用真实 top-level `send`、`search` 和自动 import；CLI contract test 拒绝虚构命令。 |
| F-024 | High | Grok delete 使用真实 vendor CLI；成功/失败输出清理；不支持的 mutation 明确拒绝。 |
| F-025 | High | Hub core 保持 ACP-only；私有 store 读取只存在于 fail-closed vendor compatibility adapter。 |
| F-026 | High | 测试清单使用真实文件并动态计数；补充真实 CLI/MCP process smoke。 |
| F-027 | High | proxy、cancel、close、replay、pagination、callback 测试改为验证命名行为，移除 false-green assertion。 |
| F-028 | Medium | 持久文档移除本机版本、session 数、branch/commit/marker；sample 改为 placeholder 和最小权限。 |
| F-029 | Medium | PowerShell/POSIX 命令和 adapter 路径占位符分开，修复跨平台示例。 |
| F-030 | High | release archive 包含 adapters、skill、root operator docs 和 BUILD_INFO，并验证解压内容及引用。 |
| F-031 | Medium | 正常 adapter diagnostics 不输出本地路径；probe 捕获 vendor stderr 并验证 id/path 不泄漏。 |
| F-032 | Medium | 修复 copyable syntax、Markdown fence、本地链接和不存在的 script 引用。 |

Review Book ledger 之后，额外 review 又发现并修复了以下跨 finding 问题：

- stdio、HTTP/SSE、WebSocket 在 JSON 反序列化前执行 32 MiB frame ceiling；
- 每条 leg 限制 4096 frames / 32 MiB outstanding input；callback、SSE stream 和 partial event 有独立或共享预算；
- 删除无界 `hub/conv/messages` RPC，兼容客户端通过 `messages_page` 遍历；
- endpoint replacement/delete 在 active operation 存在时被拒绝；
- callback rollback 不再逆转 session/pending lock order；
- workflow action 使用 full SHA，默认 `contents: read`，只有 release upload job 提升为 `contents: write`；
- operation admission 在 endpoint config/handle lookup 之前保留 token；cancel、refresh publication 和 replay prune guard 使用 generation/owned admission 防止旧 operation 清理新状态；
- RPC valid-method parameter error 固定为无数据 `INVALID_PARAMS`，oversized response 保留 request id 并允许连接继续使用，clean EOF drain 保留首个失败。

最终六个独立 review lane 中，三个直接返回 `APPROVED`；另外三个 candidate finding 经源码或上游契约核验为误报：terminal root child 实际会 `wait()`、CLI 不存在所称 `--prompt-file`、release action 默认更新已有 release 并覆盖同名资产。没有为误报修改生产代码。

## 5. 中断 handoff 中原未完成事项

`continuation-handoff-2026-07-18.md` section 16 曾留下四类未完成工作：

1. **operation finalizer/cancellation 回归未完成**：已完成。prompt、refresh、set-mode、set-config、delete、close、new-session、supplied-session load 的 abort/drop 路径进入完整测试矩阵。
2. **三个 RPC finding 未完成**：已完成。`INVALID_PARAMS`、oversized normal-ID correlation/connection reuse、clean-EOF first failure 均已实现并测试。
3. **完整本地 gate 未执行**：已完成。格式、Clippy、全部 Rust tests、adapter fixtures、dependency policy、package/install smoke 和 diff integrity 已重新执行。
4. **hosted CI、push、tag、crate/GitHub Release 未执行**：仍属于外部发布状态。本次按用户要求提交本地 commit；push、tag、hosted jobs 和 publication 不包含在本次提交动作中。

因此 section 16 的 restart order 已失效；sections 17–19 与本文件是当前状态入口。

## 6. 实际修改范围

### 6.1 ACP runtime、daemon 与 transport

- endpoint-scoped session、connection generation、binding 与 terminal ownership；
- capability negotiation、ACP version validation、capture failure propagation；
- daemon concurrent request dispatch、same-connection cancel、EOF/error preservation；
- bounded stdio/HTTP/SSE/WebSocket framing、pending ledger、callback/SSE budgets；
- terminal process-tree kill、reader drain、reap、quota、output truncation 和 cleanup。

主要文件：

- `crates/hub/src/acp.rs`
- `crates/hub/src/callbacks.rs`
- `crates/hub/src/daemon.rs`
- `crates/hub/src/rpc.rs`
- `crates/hub/src/transport.rs`
- `crates/hub/src/bounded_transport.rs`

### 6.2 Conversation、run、store、registry 与 search

- 两层历史：vendor original replay 与 Hub capture 独立保存、共同展示；
- refresh/load/replay 的 provisional parent、transactional publication、rollback 和 reopen recovery；
- operation token/generation admission、active deletion protection、run owner finalization；
- registry mutation serialization、proxy reference validation、active agent protection、handle invalidation；
- combined search pagination、message row/byte budgets、bounded snippets；
- Windows 路径 canonicalization 与 state-file/IPC 权限处理。

主要文件：

- `crates/hub/src/hub/`
- `crates/hub/src/store.rs`
- `crates/hub/src/runtime.rs`
- `crates/hub/src/endpoint.rs`
- `crates/hub/tests/`

### 6.3 CLI 与 MCP

- 安全的 public endpoint representation 和统一 redaction；
- 完整 endpoint/proxy/session/conversation management；
- caller-resolved cwd、search offset、message cursor、NDJSON streaming；
- current-run output，移除 server-side unbounded history materialization；
- process-level CLI contract 和 MCP stdio initialize/list/call smoke。

主要文件：

- `crates/cli/src/main.rs`
- `crates/cli/src/mcp.rs`
- `crates/cli/tests/`

### 6.4 Vendor adapters

- Cursor：CLI/IDE session discovery/load、fail-closed storage parsing、隐私错误、真实 ACP upstream forwarding；
- Grok：session list/load/resume/delete、prompt file 不进入 argv、process-tree shutdown、sanitized vendor failure；
- Codex/OMP：修复注册配置、真实命令、能力与最小权限说明；
- 默认测试只使用 synthetic fixtures，live mutation 保持显式 operator opt-in。

主要文件：`adapters/{cursor,grok,codex,omp}/`。

### 6.5 文档、skill、安装与 release

- README、RELEASING、CHANGELOG、CONTRIBUTING 与 adapter README 对齐真实 CLI；
- `.grok/skills/acp-hub` 使用可复制命令、最小权限 sample 和一致的 session/import 说明；
- 五份开发文档和 implementer role 与 SSOT、当前模块边界和测试证据同步；
- CI/release action full-SHA pin、权限最小化、Rust 1.91 MSRV、Node 22.13 adapter matrix；
- release archive 使用 operator allowlist，包含 adapters、skill、root docs、BUILD_INFO 和 checksum 验证。

## 7. 最终本地验证

| Gate | 最终结果 |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | pass |
| `cargo test --workspace --all-targets --all-features --locked -- --test-threads=1` | 185 passed，5 ignored，17 suites |
| Cursor adapter fixture | 20 passed，1 deliberate live-write skip |
| Grok adapter fixture | 23 passed，1 deliberate live-write skip |
| adapter JavaScript `node --check` | pass |
| `cargo deny check` | advisories、bans、licenses、sources pass；duplicate/unmatched-allowance warnings 为已记录非 fatal warning |
| public/authority Markdown fences 与本地链接 | pass |
| `cargo publish --dry-run --allow-dirty --locked --package acp-hub-core` | pass；37 files，763.6 KiB（145.9 KiB compressed） |
| CLI package list | pass；11 packaged paths |
| `cargo install --path crates/cli --locked` | 安装 `acp-hub 0.1.3` 成功 |
| 安装后 daemon-backed add/list/inspect/remove | pass |
| `git diff --check` | pass；只有本地 AutoCRLF notice |

安装后验证得到：

```json
{"version":"0.1.3","registered":"smoke","transport":"stdio","removed":true}
```

## 8. 最终未执行项与残余边界

以下项目没有被包装成「已完成」：

- Linux/macOS、Ubuntu Rust 1.91 和四目标 release matrix 需要 commit 后由 hosted CI 执行；
- Windows 本地 Rust 1.91 单独 target check 曾三次被操作系统在链接 `rustls` build script 时以 `Access is denied (os error 5)` 阻止；声明的 1.91 floor 由 lock graph 和 pinned Ubuntu job负责验证；
- live Cursor/Grok resume、prompt 和 destructive delete 没有针对用户真实 session 执行；fixture 已覆盖 parser/router/error/privacy/process cleanup；
- operator 注册的 endpoint/proxy 是同用户权限下的可执行代码；frame/resource limit 不等于不可信代码 sandbox；
- Cursor/Grok 私有 store schema 可能随厂商版本变化，因此 adapter 保持 fail-closed；
- 本次执行本地 commit，但不执行 push、tag、crates.io publish 或 GitHub Release。

这些是明确的运行或发布边界，不是未解决的 Review Book Critical/High/Medium finding。

## 9. 提交边界

本文件与完整维护 candidate 一起提交。提交前重新执行最终差异、格式、测试与文档完整性检查。commit hash 记录在最终用户汇报中，避免在同一个 commit 内写入自引用 hash。
