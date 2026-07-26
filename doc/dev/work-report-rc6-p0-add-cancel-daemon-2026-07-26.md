# 工单报告：rc.6 顽固 P0（add / cancel / daemon）

| 字段 | 值 |
|------|-----|
| **性质** | **实现工单 / 完成报告**（**不修改**反馈 SSOT） |
| **输入 SSOT（只读）** | [`ux-unified-feedback-2026-07-26-rc6.md`](./ux-unified-feedback-2026-07-26-rc6.md) |
| **日期** | 2026-07-26 |
| **发布目标** | `0.2.1-rc.7`（本报告合入后打 tag） |

---

## 1. 反馈对照（§5 P0 — 关闭于本工单）

| ID | 反馈问题 | 实现 |
|----|----------|------|
| **P0-1** | 冷 `agent add` 可长时间不返回 | **connect+register 整体 15s 超时**；超时/失败时 **本地写 agents.json** 并返回成功；daemon `list`/`mutate` **磁盘指纹刷新**吸收 local write |
| **P0-2** | 长任务 `cancel` 可挂死 >10min | Hub：**先 mark cancelling**，再 **≤8s 有界** ACP `session/cancel`；失败/超时仍返回 `requested=true`。CLI：**12s 硬超时** + 可执行提示 |
| **P0-3** | 多轮 `daemon closed` | CLI **connect 失败自动重连一次**；**send 遇 daemon 断连重试一次** |

### P1 顺带

| ID | 实现 |
|----|------|
| **P1-2** search toolCallId/raw 噪音 | snippet 额外过滤 `toolCallId` / `fc_` / `rawOutput` |

未做（反馈 P1-1/3/4/5 或 P2）：工具路径 `?` 截断（控制台编码）、daemon 全量自愈投影重建、museum 中文、soft-delete 默认摘要。

---

## 2. 关键代码路径

| 区域 | 文件 |
|------|------|
| cancel 有界 | `crates/hub/src/hub/prompt.rs` |
| cancel 测试语义 | `crates/hub/src/hub/tests/cancel.rs` |
| agent add 超时 + local write | `crates/cli/src/commands.rs` |
| daemon reconnect / send retry | `crates/cli/src/commands.rs` |
| registry disk refresh | `crates/hub/src/hub/registry.rs` |
| search denoise | `crates/cli/src/output.rs` |

---

## 3. 测试

| 检查 | 结果 |
|------|------|
| `cargo test -p acp-hub-core --lib cancel` | 见 CI / 本地 |
| 全量 core lib + cli tests | 见 PR CI |

---

## 4. 纪律

- **未改写** `ux-unified-feedback-2026-07-26-rc6.md` 与 operator baseline。
- 关闭状态只写在本文 + CHANGELOG。

---

**文档结束。**
