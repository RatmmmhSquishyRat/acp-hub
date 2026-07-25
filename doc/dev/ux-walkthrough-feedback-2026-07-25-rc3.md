# ACP Hub CLI UX 完整体验与统一反馈（2026-07-25 · v0.2.1-rc.3）

**版本：** `acp-hub 0.2.1-rc.3`（本机新装 GitHub prerelease）  
**上一版体验：** `0.2.1-rc.2` → [ux-walkthrough-feedback-2026-07-24.md](./ux-walkthrough-feedback-2026-07-24.md)  
**主机：** Windows  
**Agent：** 发布包 Cursor adapter + 本机 `cursor-agent`；模型 `grok-4.5[effort=high,fast=true]`  
**产物：** `tmp/acp-ux-rc3-20260725-130445/`（`ux.log`、`summary.json`、`work/ux-rc3.txt`）  
**安装：** `gh release download v0.2.1-rc.3` → SHA256 校验通过 → 覆盖 `~/.cargo/bin/acp-hub.exe`

**性质：** 操作者视角手感与问题统一反馈；不是 CI 门禁。

---

## 1. 安装与版本

| 项 | 值 |
|----|-----|
| 装前 | `0.2.1-rc.2` |
| **最新发布** | **`v0.2.1-rc.3`**（2026-07-25，Pre-release） |
| 稳定 Latest | 仍为 `v0.2.0` |
| crates.io | 仍为 `0.2.0`（rc 仅 GitHub 包） |
| 校验 | `df0d6e865e776aa6d8528ac979c5ade29b86ae79d308b826208868d0c07a830c` OK |

rc.3 相对 rc.2 的 **Operator UX 大改**（CHANGELOG）：

- `doctor` 旅程 + 检查  
- `agent inspect --probe`  
- `agent sessions` 元数据发现（避免全量 load 洪水）  
- `conv list` workbench vs `--all` museum；origin / interaction  
- `conv show` 合并 transcript（`--raw` 可选）  
- create/send **progress + timings**  
- search 带 interaction/origin  

---

## 2. 总体感受（rc.3）

| 维度 | 感受 | 主观分 (1–5) | vs rc.2 |
|------|------|--------------|---------|
| 命令发现 / help | 更完整；`doctor` 进顶层 | **4.5** | ↑ |
| 上手路径 | doctor 直接给 G.0 七步旅程 | **4.5** | ↑↑ |
| 成功短反馈 | add/set/delete/close 仍干净 | **4** | → |
| 列表可读性 | workbench 默认 + IX/ORIGIN/STATUS，单行工作会话清晰 | **4** | ↑↑ |
| JSON 模式 | 更丰富（origin/interaction/timings/probe） | **4.5** | ↑ |
| 错误文案 | 出现 `error: agent_not_found` / `not_busy` 前缀 | **4** | ↑ |
| `send` 流式输出 | 仍有 tool_call 细节，但 thought 合并更好；stderr 有 stage | **3.5** | ↑ |
| `conv show` | origin/interaction/phase + 合并正文；不再纯 wire dump | **4** | ↑↑ |
| 状态一致性 | 写文件后 **`completed`**；cancel 对空闲给 `not_busy` | **4.5** | ↑↑（相对 rc.2 卡 running） |
| 信任感 | 本轮写盘 + CLI 均成功；sessions **未崩 daemon** | **4** | ↑↑ |
| 端到端 Cursor 干活 | 写文件 + ask 均通；close 本轮成功 | **4** | ↑ |

**一句话：**  
rc.3 把 Hub 从「工程师调试台」明显推进到「可引导的操作者 workbench」。主路径可信度大幅上升；剩余主要是 **输出仍偏技术、脱敏、Windows 乱码、分发仍是 prerelease**。

---

## 3. 体验路径与结果摘要

### 3.1 发现 / 健康

| 步骤 | 结果 | 手感 |
|------|------|------|
| `--help` | 含 `doctor` | 好 |
| `doctor`（空 home） | warn + 七步旅程 | **优秀**——终于有「从哪开始」 |
| `doctor --json` | checks + journey 数组 | 机器友好 |
| `agent list` 空 | `No agents registered.` | 好 |

### 3.2 Agent 管理

| 步骤 | 结果 | 手感 |
|------|------|------|
| `agent add cursor` | `registered agent cursor` | 好 |
| `agent list` / `--json` | 表/JSON 可用；**路径仍 redact** | 中——排障仍难点 |
| `inspect`（无 probe） | `probeStatus=skipped` + **明确 next step** | 比 rc.2「干 null」好很多 |
| `inspect --probe` | ~2.6s；capabilities 填满 | **优秀** |
| `inspect --probe --json` | 同上结构化 | 好 |
| `agent sessions` | ~5s 成功，**无 daemon closed** | **相对 rc.2 质变** |
| `inspect nope` | `error: agent_not_found: …` | 好（结构化前缀） |

### 3.3 会话 / 发送

| 步骤 | 结果 | 手感 |
|------|------|------|
| `conv list` 空 | `No conversations.` | 好 |
| `conv list --all` | museum 列（本轮无导入行） | 好 |
| `conv create --json` | convId + **origin/interaction** + **timings**；stderr progress JSON | 好；progress 在 stderr |
| `param/mode list/set` | 与前相同清晰 | 好 |
| `send` 写文件 | **CLI ok**；文件 `UX-RC3-OK`；status 后 `completed`；stderr stages+timings | **主路径信任恢复** |
| `conv show` | origin/interaction/status/phase；合并 transcript | 明显可读 |
| `show --raw` / `--json` | 双通道成立 | 好 |
| `conv list` workbench | 一行：`W hub_created completed File Creator` | 清晰 |
| `search UX-RC3` | 带 IX/ORIGIN；snippet 仍略 raw | 中上 |
| `cancel`（已完成） | `error: not_busy: …` | 可预期 |
| `conv close` | **`closed conversation …` 成功** | 相对 rc.2 Cursor close 失败是改善（本轮） |
| ask `send` | `UX-RC3-ASK-OK` + final end_turn | 好 |
| 错误 conv | `error: conversation_not_found: …` | 好 |

### 3.4 时延（本轮）

| 操作 | ms |
|------|-----|
| create（冷） | ~5978 |
| create（热/二次） | ~1729 |
| inspect --probe | ~2608 |
| sessions | ~4970 |
| send 写文件 | ~15586 |
| send ask | ~10916 |

---

## 4. 相对 rc.2 UX 的「是否修好」对照

| rc.2 痛点 | rc.3 本轮 | 判定 |
|-----------|-----------|------|
| 无上手旅程 | `doctor` G.0 | **已显著改善** |
| inspect 空洞无指引 | `probeStatus` + message + `--probe` | **已改善** |
| `agent sessions` 崩 daemon | 约 5s 正常返回 | **已改善（本轮）** |
| send 失败但文件已写 + status running | 写文件 CLI **成功** + **completed** | **本轮主路径好**（不等于永不复现） |
| show 纯 wire dump | 合并 transcript + 元数据列 | **已改善** |
| list 刷屏无 workbench | 默认 workbench + `--all` | **已改善** |
| close 不支持无引导 | 本轮 close **成功** | **改善/环境相关** |
| 错误只有 `Error: …` | `error: <code>: message` | **已改善** |
| 路径脱敏 | 仍 redact | **未改** |
| 冷启慢 | 仍数秒～十余秒 | **未解决（预期内）** |
| 仅 prerelease | 仍是 | **未解决** |

---

## 5. 仍存问题（统一清单）

### P1 — 建议继续跟

| ID | 问题 | 证据 / 说明 |
|----|------|-------------|
| UX-RC3-1 | **send 默认输出仍偏「工具调试流」** | toolCallId 多行、diff 半截；比 rc.2 好，仍不像产品对话 |
| UX-RC3-2 | **progress 打在 stderr、结果在 stdout** | 重定向/日志采集时 stage 与正文分离；TTY 上夹杂 |
| UX-RC3-3 | **路径脱敏无 reveal** | list/inspect 仍 `<redacted-command>`，装错 adapter 难查 |
| UX-RC3-4 | **doctor 注册后仍提示 inspect --probe** | 即使用户刚 probe 过，doctor 不读 cache 是否已热 |
| UX-RC3-5 | **Windows 控制台乱码** | help/doctor 中 `…`/`→` 显示为 `�`（编码/代码页） |
| UX-RC3-6 | **分发滞后** | 稳定 Latest / crates 仍 0.2.0；默认安装用户享受不到 Operator UX |
| UX-RC3-7 | **冷启时延** | create/sessionMs 主导；需文档默认 timeout 与「慢≠坏」说明 |

### P2 — 打磨

| ID | 问题 |
|----|------|
| UX-RC3-8 | sessions 表头极宽，会话少时「空表」压迫感 |
| UX-RC3-9 | search snippet 仍夹路径/半 raw 文本 |
| UX-RC3-10 | ask 回复偶发跨行拼接（`UX-RC3-` / `ASK-OK`）——合并未完全 |
| UX-RC3-11 | `cancel`/`close`/`delete` 决策仍缺一张「何时用哪个」短说明（doctor 有部分） |
| UX-RC3-12 | help 顶层仍无三行最小示例（doctor 补了，但 `--help` 本身没有） |

### P3 — 边界 / 未测

| ID | 问题 |
|----|------|
| UX-RC3-13 | 并发 send、mid-turn kill、长重构未再压测 |
| UX-RC3-14 | museum 导入/IDE RO 行展示本轮数据少，未深测 |
| UX-RC3-15 | 多 agent 同 home 的 doctor/list 体验 |

### 已明显不再是主矛盾（本轮）

- sessions 必崩 daemon  
- 写文件必「CLI 失败 + running」  
- 无 doctor / 无 probe 指引  
- show 完全不可读  

---

## 6. 输出与交互设计评价

### 6.1 做得对的

1. **Journey 前置（doctor）** —— 解决「装完不会用」。  
2. **Workbench 默认** —— 把「当前可写工作」与 museum 拆开。  
3. **诚实元数据** —— origin / interaction / phase / last_outcome。  
4. **Probe 显式化** —— 冷 inspect 不装懂，并给 next step。  
5. **Progress + timings** —— create/send 慢时至少知道卡在 session 还是 prompt。  
6. **结构化错误码前缀** —— 便于脚本与人类扫读。  
7. **Sessions 元数据发现** —— 不再用「全量 load」冒充列表。  

### 6.2 仍别扭的

1. **双通道（stdout 对话 / stderr stage）** 未在 help 说明；日志工具默认只抓一侧会丢 progress。  
2. **默认 send 仍显示过多 tool 内部 id**；「合并」更像半成品，不是聊天 UI。  
3. **脱敏默认偏 SaaS**；本机 trusted CLI 应默认可 reveal 或 doctor 提示如何 reveal。  
4. **编码**：Windows 终端非 UTF-8 时，产品文案中的 Unicode 符号会糊。  

---

## 7. 体验评分卡（rc.3）

| 命令/场景 | 可用性 | 可读性 | 可预期性 | 备注 |
|-----------|--------|--------|----------|------|
| help/version | A | A | A | |
| doctor | A | A− | A | 乱码扣分 |
| agent add/list | A | C（脱敏） | A | |
| inspect | A− | B | A | 有 probe 指引 |
| inspect --probe | A | B | A | |
| agent sessions | A− | B− | B+ | 不再崩；表宽 |
| conv create | A | A (json) | A− | 慢但有 timings |
| param/mode | A | B | A | |
| send 写文件 | A | B− | A | 本轮可信 |
| send ask | A | B | A | |
| conv show | A | B+ | A | 合并有效 |
| conv list workbench | A | A− | A | |
| search | A | B | A | |
| cancel 空闲 | A | A | A | not_busy |
| close (本轮) | A | A | B | 成功；跨版本曾失败 |
| 错误路径 | A | A− | A | 前缀码 |

**综合：操作者 CLI 体验约 B+～A−（相对 rc.2 的 C+ 明显上台阶）。**

---

## 8. 统一反馈意见（结论）

1. **rc.3 的 Operator UX 交付是「对的产品方向」**：doctor、probe、workbench、merge、progress、错误码，正好打在此前反馈的痛点上。  

2. **主路径信任在本轮体验中成立**：写文件 CLI 成功 + 文件正确 + status completed + close 可用；这与 rc.2 走查中的「假失败」形成对比。  

3. **下一阶段不要再堆功能名词，要磨默认通道：**  
   - 更干净的 send 人类模式  
   - progress 与结果通道策略写进 help  
   - 可选路径 reveal  
   - Windows UTF-8 / 纯 ASCII fallback  

4. **稳定性未完全关单，但地雷已从「主路径」缩到「边角+未压测」。** sessions 本轮稳住；仍建议保留回归与 mid-turn 杀进程用例。  

5. **分发是最大产品外风险：** 体验结论绑在 **rc.3**；装 crates/Latest 0.2.0 的用户仍活在旧世界——发布说明与 README 必须版本锚定。  

6. **doctor 应更「状态感知」**：已 probe 的 agent 不要永远只说「next: probe」。  

7. **总体评价：** 从「能调 Cursor 的调试器」进化到「带旅程的 workbench CLI」；值得作为 Cursor-via-Hub 的默认本机版本，并尽快固化为稳定 **0.2.1**。  

---

## 9. 建议落地顺序

| 序 | 项 | 收益 |
|----|----|------|
| 1 | 稳定版 0.2.1 发布 + 安装指引锚定 | 用户对齐体验 |
| 2 | send 默认更短摘要 / tool 一行 | 日常可读 |
| 3 | help 说明 stderr progress 约定 | 日志不丢 stage |
| 4 | `--reveal-paths` 或 doctor 提示 | 排障 |
| 5 | doctor 读 cache/probe 热状态 | 减少无效 next |
| 6 | Windows 文案 ASCII 安全集 | 乱码 |
| 7 | sessions/museum 空表与宽表收紧 | 观感 |
| 8 | 压测：mid-turn kill、并发 send | 关稳定性尾项 |

---

## 10. 证据索引

| 证据 | 位置 |
|------|------|
| 安装包 | `tmp/acp-hub-install-0.2.1-rc.3/` |
| UX 日志 | `tmp/acp-ux-rc3-20260725-130445/ux.log` |
| 摘要 | `tmp/acp-ux-rc3-20260725-130445/summary.json` |
| 写文件 | `tmp/acp-ux-rc3-20260725-130445/work/ux-rc3.txt` → `UX-RC3-OK` |
| 前序 rc.2 UX | [ux-walkthrough-feedback-2026-07-24.md](./ux-walkthrough-feedback-2026-07-24.md) |
| 功能回归 | [cursor-adapter/regression-feedback-2026-07-24.md](./cursor-adapter/regression-feedback-2026-07-24.md) |
| CHANGELOG | 仓库 `CHANGELOG.md` → `[0.2.1-rc.3]` |

---

## 11. 变更记录

| 日期 | 说明 |
|------|------|
| 2026-07-25 | 安装 **v0.2.1-rc.3**，完整体验 CLI UX，落盘本统一反馈。 |
| 2026-07-25 | **工程 refine（→ rc.4）** 对照 §5/§9：send 人类压缩输出、doctor 热缓存感知、`--reveal-paths`、help 通道说明、ASCII 旅程文案、短 chunk 合并、search snippet 清理。未做：稳定 0.2.1 正式发版（产品决策）、mid-turn 压测。 |

---

## 12. Refine 对照（rc.3 反馈 → 代码）

| QA ID | 落地 |
|-------|------|
| UX-RC3-1 | `compact_human_body` + send/show `[tool]`/`[thinking]` 一行 |
| UX-RC3-2 | help long_about + doctor `progress_channels` 检查项 |
| UX-RC3-3 | 全局 `--reveal-paths` |
| UX-RC3-4 | `agent_cache_ready` / 仅 empty 时 probe next |
| UX-RC3-5 | doctor journey ASCII（`...` 不用 `…`/`→`） |
| UX-RC3-8 | sessions 空表文案 + title/sid 截断 |
| UX-RC3-9 | search snippet 去 content-type 噪声 |
| UX-RC3-10 | 短 assistant chunk 拼接（≤512 字） |
| UX-RC3-11 | doctor lifecycle_hint |
| UX-RC3-12 | help Quick start 块 |
| UX-RC3-6/7 | help 版本锚定说明；时延仍靠 timings（文档预期） |
