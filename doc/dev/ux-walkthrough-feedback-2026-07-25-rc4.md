# ACP Hub CLI UX 完整体验与统一反馈（2026-07-25 · v0.2.1-rc.4）

**版本：** `acp-hub 0.2.1-rc.4`（本机新装 GitHub prerelease）  
**装前：** `0.2.1-rc.3`  
**主机：** Windows  
**Agent：** 发布包 Cursor adapter + 本机 `cursor-agent`；模型 `grok-4.5[effort=high,fast=true]`  
**产物：** `tmp/acp-ux-rc4-20260725-154147/`（`ux.log`、`summary.json`、`work/ux-rc4.txt`）  
**安装：** `gh release download v0.2.1-rc.4`，SHA256 `6cb3a33ca5d682193e9b0f537a47548d6c5610b621d544a0d25f095cf80dc2d0` 校验通过  

**前序：**

- [ux-walkthrough-feedback-2026-07-24.md](./ux-walkthrough-feedback-2026-07-24.md)（0.2.1-rc.2）
- [ux-walkthrough-feedback-2026-07-25-rc3.md](./ux-walkthrough-feedback-2026-07-25-rc3.md)（0.2.1-rc.3）

**性质：** 操作者视角手感与问题统一反馈；非 CI 门禁。

---

## 1. 安装与版本

| 项 | 值 |
|----|-----|
| 最新发布 | **v0.2.1-rc.4**（2026-07-25 Pre-release） |
| 稳定 Latest | 仍为 **v0.2.0** |
| crates.io | 仍为 **0.2.0**（rc 仅 GitHub 包） |
| 本机 PATH | `acp-hub 0.2.1-rc.4` |

rc.4 CHANGELOG 相对 rc.3 的 UX refine（与此前反馈对齐）：

- send/show 更紧凑（`[tool]` / `[thinking]`，去 toolCallId 噪音，短答粘合）
- doctor 感知 cache；空 cache 才推 probe；lifecycle + progress 通道提示；ASCII 旅程
- **`--reveal-paths`**
- 顶层 help：quick start、stderr/stdout 约定、reveal、prerelease 锚定
- sessions 空博物馆文案；search 去 content-type 噪音；标题截断

---

## 2. 总体感受（rc.4）

| 维度 | 感受 | 分 (1–5) | vs rc.3 |
|------|------|----------|---------|
| 命令发现 / help | quick start + 通道说明写进 long_about | **5** | ↑ |
| 上手路径 | doctor 旅程 + 生命周期/通道提示 | **5** | ↑ |
| 成功短反馈 | 仍干净 | **4** | → |
| 列表 | workbench 清晰；reveal 可用 | **4.5** | ↑ |
| inspect | 冷/热分层清楚；reveal 见真路径 | **4.5** | ↑ |
| doctor 状态感知 | probe 后显示 cache ready，next 变为 create | **5** | ↑↑ |
| send 人类可读 | `[thinking]`/`[assistant]`/`[tool]` 压缩；无 content-type 串 | **4** | ↑ |
| show | 元数据完整 + 角色合并行 | **4.5** | ↑ |
| 状态一致性 | write 后 `completed`；空闲 cancel `not_busy` | **5** | →/↑ |
| sessions | 不崩；但全局历史会话列表很吵 | **3.5** | →（稳）/ 观感仍吵 |
| 信任感（写盘 vs CLI） | 本轮写文件 CLI 成功 + 文件正确 | **5** | → |
| Windows 文案 | doctor ASCII，无明显乱码 | **4.5** | ↑ |
| 端到端 Cursor | 写 + ask + close 通 | **4.5** | → |

**一句话：**  
rc.4 把 rc.3 的 Operator UX **磨到可日常当本机 workbench 用**。主路径可读、可引导、可排障（reveal/probe）。剩余主要是 **send 仍带 tool 内部 id、sessions 博物馆列表噪音、稳定版未出、冷启时延**。

**主观综合：约 A−（操作者 CLI）。**

---

## 3. 体验路径与结果

### 3.1 得分概览

| 指标 | 结果 |
|------|------|
| 步骤 ok（脚本判据） | **29 pass / 3 fail / 32 total** |
| 预期失败 | `inspect nope`、`cancel` 空闲 `not_busy`、`send not-a-conv`（错误路径，文案正确） |
| 主路径失败 | **无** |
| 写文件 | `ux-rc4.txt` = `UX-RC4-OK` |
| ask | `UX-RC4-ASK-OK` 单行成功 |

### 3.2 分命令手感

| 区域 | 结果 | 手感要点 |
|------|------|----------|
| `--help` | 优 | Quick start、channels、reveal、prerelease 说明——终于像产品 |
| `doctor` | 优 | 空 home warn + 旅程；probe 后 **cache ready**、next=create |
| `agent add/list` | 优 | 默认 redact；`--reveal-paths` 表显示 `node` |
| `inspect` / `--probe` / `--reveal-paths` | 优 | 冷跳过+指引；probe ~1.9s 填 capabilities；reveal 出 adapter 全路径与 roots |
| `agent sessions` | 中上 | **不崩**（~1s）；列出大量历史 session（SPACE unknown / IN_HUB false），表仍宽 |
| `conv create --json` | 优 | origin/interaction + timings；stderr progress |
| `param`/`mode` | 优 | 确认句清晰 |
| `send` 写文件 | 优− | CLI ok + 文件 ok + final end_turn；`[thinking]`/`[tool]` 压缩；**tool 行仍含 raw call id** |
| `send` ask | 优 | 答案粘合为单行 marker |
| `conv show` | 优 | origin/interaction/status/phase/busy/last_outcome；body 角色合并，无 content-type 噪音 |
| `conv list` workbench | 优 | `W hub_created completed` 一行可读 |
| `search` | 优− | 有 IX/ORIGIN；snippet 更干净 |
| `cancel` 空闲 | 优 | `error: not_busy: …` 可预期 |
| `close` | 优 | 本轮成功 |
| 错误路径 | 优 | 结构化前缀 |

### 3.3 时延（本轮）

| 操作 | ms |
|------|-----|
| create | ~4901 |
| inspect --probe | ~1899 |
| sessions | ~1040 |
| send 写文件 | ~14923 |
| send ask | ~11148 |
| param set | ~1763 |

### 3.4 代表性输出（摘录）

**send（stdout，人类通道）：**

```text
[thinking] text Creating ux-rc4.txt text with the single line text UX-RC4-OK. Then stopping.
[assistant] text Creating `ux-rc4.txt` with the requested line.
[tool] fc_… title Edit File kind edit rawInput | … status in_progress
[assistant] text Done.
final: conv=… stop_reason=end_turn
```

**send（stderr，进度通道）：**

```text
[acp-hub] stage=daemon_connect
[acp-hub] stage=prompt
[acp-hub] stage=end
[acp-hub] timings total_ms=14889 prompt_ms=Some(14886)
```

**show 头信息：** `origin=hub_created` `interaction=writable` `status=completed` `busy=none` `last_outcome=completed`；正文为 user/thinking/assistant/tool 行。

**doctor after probe：**  
`capability cache: 1 ready, 0 empty`；`cursor: capability cache present; probe optional`；`next: conv create …`。

---

## 4. 相对 rc.3 反馈的「是否落地」

| rc.3 反馈项 | rc.4 本轮 | 判定 |
|-------------|-----------|------|
| send 偏调试流 | 有 `[thinking]`/`[tool]` 压缩；无 content-type 串 | **大部分改善**；tool 仍带 call id |
| progress 在 stderr 无说明 | 顶层 help + doctor 明确 channels | **已改善** |
| 路径脱敏无 reveal | `--reveal-paths` 全局/list/inspect/doctor | **已改善** |
| doctor 永远推 probe | cache ready 后 next=create | **已改善** |
| Windows 乱码 | doctor ASCII 旅程，本轮 capture 无糊 | **已改善** |
| sessions 危险 | 稳定返回 | **保持稳定**；列表噪音仍在 |
| 稳定版滞后 | 仍 prerelease | **未解决** |
| 冷启慢 | 仍数秒～十余秒 | **未解决（预期）** |

**结论：** rc.4 是对 **rc.3 UX 走查反馈的有效回应**；主路径体验从「可用 workbench」到「可推荐日常本机使用（rc 渠道）」。

---

## 5. 仍存问题（统一清单）

### P1

| ID | 问题 | 说明 |
|----|------|------|
| UX-RC4-1 | **send 的 `[tool]` 行仍含原始 toolCallId** | 人类不需要；建议默认只 `Edit File path` 一行 |
| UX-RC4-2 | **thinking 正文仍残留 `text ` 碎片词** | 如 `text Creating… text with…`；合并未剥净 vendor 前缀 |
| UX-RC4-3 | **sessions 博物馆列表噪音** | 多历史 session、SPACE=unknown、IN_HUB=false；缺「仅本地 / 最近 N / 过滤」 |
| UX-RC4-4 | **list --reveal-paths 表 TARGET 仍简写** | 仅 `node <1 argument(s)>`，全路径只在 inspect reveal 见 |
| UX-RC4-5 | **稳定分发滞后** | crates/Latest 0.2.0 用户仍无本套 UX |
| UX-RC4-6 | **冷启/模型时延** | 需文档默认 timeout；可选更细 stage（agent spawn vs model TTFT） |

### P2

| ID | 问题 |
|----|------|
| UX-RC4-7 | `prompt_ms=Some(14886)` Debug 风格，人类通道可写 `prompt_ms=14886` |
| UX-RC4-8 | show 表 ROLE 为 `thinking`/`tool activity` 好，但 SEQ 跳号（1,2,5,6,9…）可能困惑 |
| UX-RC4-9 | search snippet 高亮括号略怪（`[ux-rc4]`） |
| UX-RC4-10 | 成功路径无「变更文件摘要」一行（仅靠 tool 行推断） |
| UX-RC4-11 | 未压测 mid-turn kill / 并发 send（稳定性尾项） |

### 已不再是主矛盾

- doctor 缺失 / 无旅程  
- inspect 干 null 无指引  
- sessions 必崩 daemon  
- 写文件 CLI 失败 + status 假 running（本轮未现）  
- show 完全 wire dump  
- 无 reveal  
- help 无 quick start / 通道说明  

---

## 6. 统一反馈意见

1. **rc.4 值得作为本机 Cursor-via-Hub 的默认二进制**（优于 rc.3，远优于 0.2.0）。  

2. **产品形态已经「像 workbench」**：help、doctor、probe、reveal、merge、progress 通道形成闭环；再大改结构收益小于打磨细节。  

3. **下一刀优先砍 send 噪音**：去掉 toolCallId、剥 thinking 的 `text ` 前缀、可选「变更文件」收尾行——即可再抬半档可读性。  

4. **sessions 要产品化过滤**，否则博物馆会重新变成惊吓源（虽不再崩）。  

5. **分发仍是最大「非代码」风险**：体验绑 rc.4；请尽快稳定 **0.2.1** 并写清安装渠道。  

6. **信任主路径本轮健康**：文件写出与 CLI 成功一致；勿因历史 rc.2 失败叙事低估当前状态。  

7. **综合评分：操作者 CLI ≈ A−**；剩余是抛光与发布，不是方向错误。  

---

## 7. 建议落地顺序

| 序 | 项 |
|----|-----|
| 1 | 发稳定 0.2.1 + README 版本锚定 |
| 2 | send/show 再压 toolCallId 与 `text ` 前缀 |
| 3 | sessions：默认最近 N / 可写过滤 / SPACE 标注 |
| 4 | list --reveal-paths 显示完整 args 或第二列 path |
| 5 | timings 人类格式（去掉 `Some(...)`） |
| 6 | 成功 send 可选 files-changed 摘要 |
| 7 | mid-turn kill / 并发 回归用例 |

---

## 8. 证据索引

| 证据 | 路径 |
|------|------|
| 安装目录 | `tmp/acp-hub-install-0.2.1-rc.4/` |
| UX 日志 | `tmp/acp-ux-rc4-20260725-154147/ux.log` |
| 摘要 | `tmp/acp-ux-rc4-20260725-154147/summary.json` |
| 写文件 | `tmp/acp-ux-rc4-20260725-154147/work/ux-rc4.txt` |
| CHANGELOG | 仓库 `[0.2.1-rc.4]` |
| 前序 UX | rc.2 / rc.3 同目录文档 |

---

## 9. 变更记录

| 日期 | 说明 |
|------|------|
| 2026-07-25 | 安装 **v0.2.1-rc.4**，完整 CLI UX 走查，落盘本统一反馈。 |
| 2026-07-25 | **设计程序纠正：** 禁止未闭合设计就写代码。半截 rc.5 实现已从工作区回退。人类扫读全量设计包见 `doc/ssot/agent-managed/HUMAN-READING*.md`（LAW + DESIGN + CONTRACT + REVIEW APPROVED）。**实现须严格按 CONTRACT，另开 PR。** |
