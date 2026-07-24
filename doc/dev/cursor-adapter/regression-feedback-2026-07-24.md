# Cursor via ACP Hub — 回归结论与剩余问题反馈（2026-07-24）

**版本：** `acp-hub 0.2.1-rc.2`（本机已安装的 GitHub prerelease）  
**主机：** Windows  
**适配器：** 发布包内 `adapters/cursor/adapter.mjs`（亦可对照仓库 `adapters/cursor/`）  
**Cursor agent：** 本机 `cursor-agent`（如 `versions/2026.07.16-899851b`）  
**模型：** Cursor first-party `grok-4.5[effort=high,fast=true]`  
**关联文档：**

- [e2e-investigation-2026-07-24.md](./e2e-investigation-2026-07-24.md) — 全日调查 + 0.2.0 失败史 + rc.2 复测
- [qa-investigation-2026-07-24.md](./qa-investigation-2026-07-24.md) — **对本反馈的代码级核实裁定 + UX/QoL 验收骨架**
- [../ux-walkthrough-feedback-2026-07-24.md](../ux-walkthrough-feedback-2026-07-24.md) — CLI 全命令 UX 体验与统一反馈
- [spec.md](./spec.md) — Cursor adapter 行为规格
- [adapters/cursor/README.md](../../../adapters/cursor/README.md) — 注册与会话空间说明

**性质：** 主机本地手工/脚本回归与产品反馈，**不是** CI 门禁，也不是 `adapter-test.mjs` 的替代品。

---

## 1. 一句话结论

- **功能回归：通过。** 写文件、同会话续聊、ask 模式、杀掉 daemon 后再 list/create/send 均可。  
- **剩余问题：性能冷启、inspect 不完整、闲置会话堆积、0.2.0→rc 升级/迁移，以及未覆盖的压力场景。**  
- **不是**「Cursor via Hub 又不能用了」；主路径在 **0.2.1-rc.2** 上已可继续作为本机自动化候选。

---

## 2. 回归范围与方法

### 2.1 脚本与产物

| 项 | 路径 |
|----|------|
| 脚本 | 工作区 `tmp/acp-regression-cursor.ps1` |
| 本轮产物根 | `tmp/acp-regression-cursor-20260724-203801/` |
| 汇总 | 同上目录 `summary.json`、`regression.log` |
| 工作目录文件 | `work/reg1.txt`, `reg1b.txt`, `reg2.txt`, `regH.txt` |

### 2.2 覆盖面

| 类别 | 内容 |
|------|------|
| A 注册 | `agent add` / `agent list` / `agent inspect` |
| B 写文件 | create → param(Grok 4.5) → mode agent → send 写 `reg1.txt` → show status |
| C 独立会话 | 再 create + 写 `reg2.txt` |
| D 同会话 follow-up | 在 B 的 conv 上再 send 写 `reg1b.txt` |
| E ask 模式 | mode=ask，精确回复标记串 |
| F 列表/搜索 | `conv list`、`search` |
| G 连续 create×3 | 无 send，仅压力 create |
| H 恢复 | 杀掉 `acp-hub` daemon 后 list → create → send 写 `regH.txt` |

隔离方式：每次使用独立 `--home <tmp>/hub-home`，避免污染默认 `~/.acp-hub`。

### 2.3 成功判据

| 判据 | 说明 |
|------|------|
| CLI 成功 | 进程退出且 stdout/stderr 无 `Error:` / clap `error:` |
| 文件成功 | 目标文件存在且内容**整行等于**约定 marker |
| 状态成功（写文件后） | `conv show` 中 status 为 **`completed`**（0.2.1-rc.2 起镜像 turn 结果） |
| 断连 | 若出现 `daemon closed` / `daemon unavailable` 记失败 |

---

## 3. 回归得分卡

| 指标 | 结果 |
|------|------|
| **PASS** | **20** |
| **FAIL** | **0** |
| **TOTAL** | **20** |
| 脚本退出码 | `0` |
| 版本 | `acp-hub 0.2.1-rc.2` |
| 断连 | **未出现** |

### 3.1 用例明细

| 用例 | 结果 | 备注 |
|------|------|------|
| A_register | Pass | ~143 ms |
| A_list | Pass | 可见 cursor |
| A_inspect | Pass | 见 §5.2：字段不完整 |
| B_create | Pass | ~13.1 s（冷启偏慢） |
| B_param | Pass | ~4.6 s |
| B_mode | Pass | |
| B_send | Pass | ~16.9 s；无 daemon 错 |
| B_file | Pass | 内容 `REG1-OK` |
| B_status_completed | Pass | status=`completed` |
| C_create | Pass | ~4.6 s |
| C_send_file | Pass | `REG2-OK` |
| C_status_completed | Pass | |
| D_followup_same_conv | Pass | `REG1B-OK`，~8.6 s |
| E_ask_mode | Pass | 回复含 `ASK-REG-OK`，~11.5 s |
| F_conv_list | Pass | |
| F_search_cmd | Pass | 命令成功；投影偏碎 |
| G_rapid_create_3 | Pass | 3/3 |
| H_list_after_daemon_kill | Pass | |
| H_create_after_daemon_kill | Pass | ~15.1 s |
| H_send_after_daemon_kill | Pass | `REGH-OK`，~17.0 s |

### 3.2 磁盘产物（内容）

```text
reg1.txt  → REG1-OK
reg1b.txt → REG1B-OK
reg2.txt  → REG2-OK
regH.txt  → REGH-OK
```

### 3.3 与同日更早结论的对比

| 维度 | 0.2.0 调查（同日早） | 0.2.1-rc.2 复测 | **本轮扩展回归** |
|------|---------------------|-----------------|------------------|
| 写文件 | 曾成功 1 次，会话常 `failed` | 2/2 写 + ask 过 | **写×3 会话 + 恢复写，全过** |
| 同会话 follow-up | 未稳 | 未专项 | **Pass** |
| 杀 daemon 后恢复 | 易 Access denied / 挂死 | 未专项 | **Pass** |
| 连续 create | — | — | **3/3 Pass** |
| 断连 hang | 常见 | 干净复测未见 | **未见** |

---

## 4. 总体观点（产品判断）

1. **0.2.1-rc.2 在「Cursor 作为 ACP 后端干活」上，已经跨过「能不能用」的门槛。**  
   同日 0.2.0 的主要故障（中途 `daemon closed`、param 挂死、假 `idle`/`failed`）在本轮回归中**没有复现**。

2. **当前主要矛盾从「正确性/稳定性崩溃」转为「体验、运维与升级路径」。**  
   对内可继续用 rc 做主机自动化；对外仍应标明 prerelease，并推动稳定版 0.2.1 与迁移说明。

3. **回归绿不代表「无问题」。**  
   冷启耗时、inspect 空洞、闲置会话堆积、旧配置不迁移，都会在真实团队使用中变成摩擦；未覆盖的压力场景仍可能暴露新缺陷。

4. **问题分层要分清：**  
   - Hub 核心（daemon、Store、状态镜像）— rc.2 已明显好转  
   - Cursor 冷启与模型时延 — 大量 send 时间归 Cursor/模型  
   - 操作者/Windows 脚本 — 引号、`--allow-* true` 等  
   - Adapter 产品边界 — IDE resume 拒绝等是设计，不是回归失败  

---

## 5. 剩余问题清单（完整）

以下问题**未导致本轮用例失败**，但建议作为后续跟踪项。

### 5.1 中等 — 建议跟进

#### (1) 冷启动与时延偏高

| 观察 | 量级（本轮） |
|------|----------------|
| 首次 `conv create` | ~13 s |
| 杀 daemon 后再 create | ~15 s |
| 写文件类 send | ~14–17 s |
| param set | 可达 ~4–5 s |

**观点：** 写路径 send 时间含 Cursor/模型，不完全是 Hub 责任；但 create 冷路径仍偏长，影响交互体感与编排超时设置。  
**建议：** 区分「Hub/adapter 启动」与「模型 TTFT」的计时埋点；文档给出推荐 timeout（create ≥60–90s，send ≥120–180s）。

#### (2) `agent inspect` 在无会话时信息空洞

本轮 inspect 典型输出：

```json
{
  "agentId": "cursor",
  "config": { "...": "redacted transport + auto-allow" },
  "agentInfo": null,
  "capabilities": null,
  "cachePopulated": false
}
```

**观点：** 注册成功后仍看不到 auth methods / prompt capabilities，排障与 UI 选型不便。  
**建议：** 可选「轻量 probe 会话」填充 cache，或文档写明「需 create 后才有完整 capabilities」。

#### (3) 闲置 Cursor 会话 / 进程堆积

`G_rapid_create_3` 连续创建 3 个 idle 会话均成功，但**未验证**后续 reaping：

- Hub 侧会话条目增长  
- 本机 `cursor-agent` / adapter 进程是否长期驻留  

**观点：** 编排器若频繁 create 而不 close/delete，可能吃内存与句柄。  
**建议：** 文档推荐 `conv close`/`delete` 生命周期；可选 idle TTL 或 max concurrent endpoint sessions。

#### (4) 稳定版滞后与分发分裂

| 渠道 | 状态（写文档时） |
|------|------------------|
| 本机 PATH 二进制 | **0.2.1-rc.2** |
| GitHub Latest（稳定） | **v0.2.0** |
| crates.io | **0.2.0**（rc 不发布 crate） |

**观点：** 默认 `cargo install acp-hub-cli` 用户仍拿不到 lag 不断连、status 镜像、本地默认 auto-allow 等修复。  
**建议：** 尽快发稳定 **0.2.1**；README 显著提示 Windows Cursor 用户使用 ≥ rc.2 或 0.2.1+。

#### (5) 旧 `agents.json` 不自动迁移

0.2.0 时代常见配置：`permission_policy: reject`，FS/terminal 回调关闭。  
升级二进制**不会**改写已有条目。

**观点：** 「只换 exe」后仍可能出现「模型在想、工具全被拒」。  
**建议：** 升级说明中强制一步 `agent add ...`（或提供 `agent migrate-defaults`）；CHANGELOG 已提示，但需在 Cursor adapter README 再强调。

---

### 5.2 较低 — 体验 / 运维

#### (6) 路径脱敏 vs 排障

list/inspect 中 command 为 `<redacted-command>`。  
**优点：** 少泄本地路径。  
**缺点：** 装错 adapter 时更难一眼确认。  
**建议：** debug 模式或 `--reveal-paths` 仅限本机 trusted 模式。

#### (7) 投影碎片与 search 可读性

Hub 捕获大量细粒度 thought / tool_update 小片；`search` 能命中，但人类阅读成本高。  
**建议：** 展示层合并连续 thought；search snippet 做折叠。

#### (8) Windows 操作者脚本陷阱

| 点 | 说明 |
|----|------|
| `--allow-read` 等 | 若写出 flag，必须带 `true`/`false`（默认已是 true 时可省略 flag） |
| `Start-Process -ArgumentList` | 多词 `--text` 必须整段引号，否则 clap 报 `unexpected argument 'file'` |
| 强制杀进程 | 0.2.0 下曾导致 Access denied；rc.2 本轮 idle kill 恢复 OK，**中途杀 running send 仍未测** |

这些主要是**操作面**，不是 ACP 协议语义错误，但真实用户会踩。

---

### 5.3 产品边界（设计如此，勿当 bug 误报）

| 边界 | 说明 |
|------|------|
| IDE 会话 resume | Adapter **拒绝**用 CLI resume 接 IDE 会话（防静默开新聊） |
| CLI 空间 prompt | 偏 `ask`/只读续聊语义；与 ACP 全工具会话不同 |
| 三套 Cursor 历史 | ACP / CLI / IDE 不由 Hub 统一成单一时间线 |

详见 adapter README / spec。

---

### 5.4 本轮未覆盖（风险清单）

| 未测场景 | 风险 |
|----------|------|
| 同一 conv **并发** 双 send | busy 锁、交错投影 |
| **send 进行中** taskkill 后再 resume | 比 idle kill 严酷得多 |
| 大仓、多文件、长时 tool 环 | 超时、投影体积、lag 边界 |
| Linux / macOS | 仅 Windows 证据 |
| 默认 `~/.acp-hub` 脏库长期运行 | 隔离 home 掩盖了部分状态腐烂 |
| 多 agent（cursor+grok）同 home 编排 | 资源与权限交叉 |

---

## 6. 问题优先级总表

| 优先级 | ID | 标题 | 是否阻断本机 Cursor 主路径 |
|--------|-----|------|---------------------------|
| P1 | LATENCY | 冷启 / create 偏慢 | 否（需调超时） |
| P1 | INSPECT | inspect 无 agentInfo/capabilities | 否 |
| P1 | SESSION-ACCUM | idle 会话/进程堆积 | 潜在（高 create 频率） |
| P1 | SHIP | 稳定版仍 0.2.0；rc 仅 GitHub | 对「默认安装用户」是 |
| P1 | MIGRATE | 旧 agents.json 不迁移 | 对升级用户可能是 |
| P2 | REDACT | 路径脱敏妨碍排障 | 否 |
| P2 | PROJECTION | 投影过碎 | 否 |
| P2 | WIN-CLI | Windows 引号 / allow 标志 | 否 |
| P3 | MATRIX | 未测并发/中途杀/跨 OS | 未知 |
| — | DESIGN | IDE resume 等边界 | 设计约束 |

---

## 7. 建议行动（按角色）

### 7.1 本机操作者（现在）

1. 继续使用 **`acp-hub 0.2.1-rc.2`（或更新的 0.2.1+）**，不要回退 0.2.0 做 Cursor 写工具。  
2. 新 home 或升级后执行一次显式 `agent add`（auto-allow + allow-root）。  
3. 编排超时：create ≥ 90s，send ≥ 150s。  
4. 用完会话尽量 `conv close` / `delete`，避免无界 create。  
5. 自动化脚本对 `--text` 整段加引号。

### 7.2 ACP Hub 工程

1. 推进 **稳定 0.2.1** 与 crates.io 对齐。  
2. 升级文档 + 可选 registry migrate。  
3. inspect 能力缓存或 probe。  
4. idle 会话 reaping / 并发上限策略。  
5. 指标：create 冷时间、send 端到端、endpoint 进程数。  
6. 补测试：并发 send、mid-turn kill、跨平台 smoke。

### 7.3 Cursor adapter

1. README 置顶：最低 Hub 版本、auto-allow、会话三空间限制。  
2. 与 Hub 协作：创建失败时的超时错误文案更可操作。  

---

## 8. 反馈意见（直接表述）

1. **rc.2 值得在本机当作 Cursor-via-Hub 的默认二进制**；相对 0.2.0 是质变（断连/挂死主路径消失）。  
2. **不要把 20/20 绿读成「无技术债」**；当前债在体验、运维、分发与未测矩阵。  
3. **最大产品风险**是：社区仍装 0.2.0 或沿用 reject 配置，会误判「Hub 不能驱动 Cursor」。应用文档和版本号解决认知差。  
4. **最大工程风险**是：未测的并发与 mid-turn 杀进程；建议在发稳定 0.2.1 前至少补一项 mid-turn 恢复或明确「不支持」。  
5. **性能**上 create 冷路径与模型时延应拆分沟通，避免全算在 Hub 头上，也避免编排器用 30s 超时误杀。  

---

## 9. 证据索引

| 证据 | 位置 |
|------|------|
| 回归 summary | `tmp/acp-regression-cursor-20260724-203801/summary.json` |
| 回归日志 | `tmp/acp-regression-cursor-20260724-203801/regression.log` |
| 写出文件 | 同目录 `work/reg*.txt` |
| 全日调查（含 0.2.0 失败） | [e2e-investigation-2026-07-24.md](./e2e-investigation-2026-07-24.md) |
| Hub 断连源码（历史） | `crates/hub/src/rpc.rs`（`daemon closed the connection`） |
| 发布说明 | GitHub `v0.2.1-rc.2` CHANGELOG：lag 不断连、status 镜像、默认 auto-allow |

---

## 10. 变更记录

| 日期 | 说明 |
|------|------|
| 2026-07-24 | 根据本轮扩展回归（20/20）与产品判断，落盘本反馈文档。 |
