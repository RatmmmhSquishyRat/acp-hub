# 工单报告：rc.5 操作者反馈实现落地

| 字段 | 值 |
|------|-----|
| **性质** | **实现工单 / 完成报告**（**不是**反馈 SSOT，不修改反馈原文） |
| **输入 SSOT（只读）** | [`ux-unified-feedback-2026-07-25-rc5.md`](./ux-unified-feedback-2026-07-25-rc5.md) · [`ux-operator-baseline-and-feedback-0.2.1-rc.5.md`](./ux-operator-baseline-and-feedback-0.2.1-rc.5.md) |
| **日期** | 2026-07-26 |
| **实现提交** | #61 `86f5983` |
| **发布** | **v0.2.1-rc.6** @ `2556c04` · [#62](https://github.com/RatmmmhSquishyRat/acp-hub/pull/62) |
| **Release** | https://github.com/RatmmmhSquishyRat/acp-hub/releases/tag/v0.2.1-rc.6 |

> **规则（实现者）：** 反馈书 / 操作者基线是冻结输入。落地后写**本文**对照关闭项；**禁止**回写「已修 / 已关闭」进反馈 SSOT。

---

## 1. 对照关闭表（相对 rc.5 统一反馈 §3 / 基线 §3.4）

| 反馈 ID | 要求（摘要，原文见 SSOT） | 实现 | 证据 |
|---------|---------------------------|------|------|
| **P0-1** / B-REG-01 / B-STB-01 | `agent add` 有限时间返回；不可静默假死 | CLI `register` **15s** `timeout`；超时若 `agents.json` 已含 id → 成功 + stderr 提示 `agent list` | `crates/cli/src/commands.rs` `AgentCommand::Add` |
| **P0-2** / B-DEL-01 | 默认 `conv delete` 无 remote 能力仍成功 | Hub：`DeleteMode::LocalFallback`；缓存/live 无 `session/delete` 时本地 soft-delete；CLI 一行说明 | `hub/lifecycle.rs` · `commands.rs` Delete |
| **P0-3** | 默认 show 有正文（回归锁） | 既有 #53；本工单未破坏 | 保留基线 B-SHO-01 |
| **P1-1** / B-SHO-05 | show 人读保换行/空格 | `sanitize_terminal_text` **保留 `\n`/`\t`** | `cli/src/output.rs` + `cli_tests` |
| **P1-2** / B-SEA-02 | search snippet 去 `type text text` | human SNIPPET 走 `clean_body` | `output.rs` `print_search_results` |
| **P1-4** | 空闲 wait 提示复盘 | `not_busy` 文案含 `wait --run` / `wait --last`；CLI/MCP `prefer_last` | `error.rs` · `WaitArgs` · store `resolve_wait_run_opts` |
| **P1-5** | param/mode 默认表 | 表格式；`--json` 机器 | `print_config_human` · args |
| **P1-6** / P2 | delete 错误双空格 | 空 endpoint → `"agent"` | `command_loop` close/delete labels |
| **P1-7** | soft-delete show 语义 | show 在 deleted 时打印 tombstone note | `commands.rs` Show |
| **P2-1** | doctor 编码破损 | 标题 ASCII `-` | `handle_doctor` |

**未纳入本工单（反馈 P2 沟通/环境项，非默认路径代码债）：**

- P2-2 crates.io Latest 滞后（发布沟通）
- P2-3 工具行路径摘要（进阶，仍靠 `--json`）
- P2-4 PowerShell 多行 `--text`（环境 + 文档）

---

## 2. 测试与发布

| 检查 | 结果 |
|------|------|
| `cargo test -p acp-hub-core --lib` | PASS |
| `cargo test -p acp-hub-core --test store` | PASS |
| `cargo test -p acp-hub-cli --tests` | PASS |
| `cargo clippy … -D warnings` | PASS |
| PR #61 CI | PASS → squash main |
| PR #62 version bump + tag `v0.2.1-rc.6` | PASS |
| release workflow #30165497966 | **success**（四平台资产 + SHA256SUMS；crates.io skip） |

---

## 3. 产物位置

| 产物 | 位置 |
|------|------|
| 代码 | main 含 #61 |
| 二进制 | GitHub Pre-release **v0.2.1-rc.6** |
| 变更说明 | `CHANGELOG.md` → `[0.2.1-rc.6]` |
| 产品表面 SSOT 状态注记 | `doc/ssot/agent-managed/UX-CORE.md`（实现 ship-state，**非**反馈原文） |
| **本工单** | 本文 |

---

## 4. 纪律备注

1. **反馈 SSOT 只读。** `ux-unified-feedback-*` / `ux-operator-baseline-*` 由操作者冻结；实现者不得改写其「未达 / 失败锚点」叙事。  
2. **关闭状态只写在工单报告**（本文）与可选 CHANGELOG / UX-CORE ship-state。  
3. 曾错误回写 baseline 的「已修」标注：**已还原**为冻结原文（见同 PR / 后续 fix commit）。

---

**文档结束。**
