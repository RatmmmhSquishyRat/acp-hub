# ACP Hub 统一 UX 反馈书（全量真实动线）

| 字段 | 值 |
|------|-----|
| **版本** | `acp-hub 0.2.1-rc.5`（GitHub Pre-release） |
| **安装** | `gh release download v0.2.1-rc.5` · ZIP SHA256 `f082a325…d4bc` 校验通过 · 覆盖 `%USERPROFILE%\.cargo\bin\acp-hub.exe` |
| **日期** | 2026-07-25 |
| **主机** | Windows 11 / PowerShell 7 |
| **Agent** | 本机 `repos/acp-hub/adapters/cursor/adapter.mjs` + Cursor CLI ACP |
| **模型（param 观测）** | `grok-4.5[effort=high,fast=true]` |
| **隔离 home** | `tmp/acp-full-ux-20260725-232615/home` |
| **证据根** | `tmp/acp-full-ux-20260725-232615/journal/`（每步 stdout/stderr/meta） |
| **性质** | **操作者全流程完整体验**，非 probe/脚本冒烟 |

**相对上一代：** 本轮 **不**沿用 rc.4 的 40s 脚本结论；动线含真实 agent 轮次、写盘核验、send/wait 分离、mid-cancel、close/delete、博物馆列表。

**不回归基线（实现者必读）：** [`ux-operator-baseline-and-feedback-0.2.1-rc.5.md`](./ux-operator-baseline-and-feedback-0.2.1-rc.5.md) — 原则「默认=典型动线」+ 舒适指标冻结 + 本版意见。

---

## 0. 方法与动线（做了什么）

### 0.1 安装

1. 确认最新 tag：`v0.2.1-rc.5`（晚于 rc.4）  
2. 下载 Windows x64 zip + `SHA256SUMS`，哈希匹配  
3. 覆盖 PATH 上的 `acp-hub.exe` → `acp-hub 0.2.1-rc.5`

### 0.2 真实动线（按操作者顺序）

```text
version / help / doctor(冷)
  → agent add cursor（--allow-root = work）
  → agent list / inspect / inspect --probe / doctor
  → --reveal-paths list + inspect
  → conv create --cwd work
  → param list / mode list
  → send 短答（FULL-UX-RC5-ASK-OK）     ~9.9s
  → conv show 默认 / --json / --tail / --no-tools / --kinds
  → send 写 rc5-marker.txt              ~7.8s + 磁盘核验
  → send 多轮读回文件内容               ~5.6s
  → wait --help；wait 空闲；cancel 空闲
  → send --no-wait --json → wait --json  ~3.6s 旁观成功
  → send --no-wait 长任务 → cancel → show 终态 cancelled
  → wait --run <已结束 runId>（completed / cancelled 均可回放 final）
  → search / agent sessions / conv list / list --all
  → close → delete（默认失败）→ delete --local-only
  → show tombstone
  → 第二会话 create → send → delete --local-only
```

**墙钟：** 安装约 23:26 → 主路径结束约 23:36+（含 agent-add 挂起恢复、多轮真实 LLM）。

### 0.3 非本轮范围

- 未测 HTTP/WS agent、proxy 链、MCP 宿主挂载  
- 未测双终端「同时 wait 同一 run」的并发压力  
- 未测 daemon 杀进程后恢复（上轮 rc.4 有 Access denied 案例，本轮未复现）

---

## 1. 总体感受（人话）

### 1.1 一句话

**rc.5 是第一次让人觉得「可以当工作台坐下来用」的版本**：  
help/doctor 把 **send / wait / show / cancel** 说清楚了；**show 终于能读对话**；**send --no-wait + wait** 旁观路径真实可用；主对话写盘/多轮可靠。

仍是 **认真 prerelease**：注册客户端可挂死、默认 delete 对 Cursor 仍踩坑、人读排版有断词/丢换行、博物馆列表仍吵。

### 1.2 主观评级

| 维度 | 分数感 | 说明 |
|------|--------|------|
| 发现与引导（help/doctor） | **A−** | UX-CORE 四原语写进主帮助 |
| 注册 / probe | **B** | 功能对；**agent add CLI 可挂** |
| 主对话 send | **A−** | 流式自然许多；终态 `Completed in Xs (end_turn)` 清楚 |
| wait 分离 | **A−** | 设计落地；旁观与 `--run` 回放可用 |
| show 回看 | **B+** | **有正文**（相对 rc.4 质变）；排版仍糙 |
| search / sessions / list | **B−** | 能用；噪音与编码问题仍在 |
| 生命周期 close/delete | **B−** | close 好；默认 delete 仍坑 |
| 稳定性手感 | **B** | 快乐路径稳；add 挂起伤信任 |
| **综合** | **B+ / A− 偏 B+** | 比 rc.4 全量的「偏 B+ 且 show 崩」**明显上一个台阶** |

### 1.3 相对 rc.4 全量反馈的变化

| 项 | rc.4 全量 | **rc.5 本轮** |
|----|-----------|----------------|
| 默认 show BODY | **空**（P0） | **有可读正文** |
| send/wait | 粘在 send | **已拆**：`--no-wait` + `wait` |
| 流式 `text ` / toolCallId | 明显 | **人读大幅改善**（工具变 `Edit File` / `Read File`） |
| reveal-paths @ list | 仍 `<1 argument(s)>` | **list 可展开真实路径** |
| doctor 叙事 | 旅程式 | **四原语 surface** |
| agent add 挂起 | 有 | **仍有**（配置已写、CLI 不回） |
| Cursor 默认 delete | 失败 | **仍失败**（双空格文案仍在） |

---

## 2. 分阶段体验笔记

### 2.1 冷启动：version / help / doctor

**感受：清楚、有产品感。**

- 版本一行干净。  
- 顶层 help 明确：

  > Product surface (UX-CORE): **send / wait / show / cancel**  
  > `send … --no-wait` then `wait`  

- doctor 冷启动：`no agents registered` + 下一步；不乱改配置。  

**小问题：** doctor 标题里出现 `acp-hub doctor ? UX-CORE` 一类 **编码破损字符**（Windows 控制台/日志捕获下可见）。

### 2.2 注册 agent

**结果：agents.json 正确写入；CLI 进程长时间不退出，需强杀。**

- 写入内容正确：stdio `node` + adapter 路径、`allowed_roots`、`auto-allow`。  
- 强杀后 `agent list` 立刻可见 cursor。  

**操作者感受：** 「已经成功了却像死机」——比功能失败更伤信任。  
**建议：** 写配置类 RPC **硬超时** + 超时文案：`registry updated; verify with agent list`。

### 2.3 inspect / probe / reveal

| 命令 | 结果 | 感受 |
|------|------|------|
| inspect（无 probe） | `cachePopulated=false`，引导 probe | 好 |
| inspect --probe | ~2.4s，`probeStatus=ok`，能力齐全 | 好 |
| doctor 再跑 | cache ready | 好 |
| list 默认 | 路径 redact | 合理 |
| list + `--reveal-paths` | **完整 node + adapter 路径** | **rc.5 修好** |
| inspect + reveal | `pathsRevealed: true` + allowed_roots | 好 |

### 2.4 conv create

- ~4.3s 出 `conv-44bf6606…`  
- stderr stage 清晰：`daemon_connect` → `session_op` → `end` + timings  

**感受：** 冷启动 agent 进程的等待可接受，进度通道有用。

### 2.5 param / mode

- 选项丰富（mode agent/plan/ask；大量 model 变体）。  
- **人读是整坨 JSON**，不是表——能用，但不「友好」。  
- 默认 mode=`agent`，model=`grok-4.5[…]`。

### 2.6 send 主对话（真实）

| 轮次 | 耗时 | 结果 |
|------|------|------|
| 短答 `FULL-UX-RC5-ASK-OK` | **9.9s** | 匹配成功 |
| 写 `rc5-marker.txt` | **7.8s** | 磁盘内容 `RC5-FULL-UX-20260725` / `PASS` **核验通过** |
| 多轮读回 | **5.6s** | 内容正确 |

**人读 send 输出样本（写盘）：**

```text
  Creating rc5-marker.txt with the two specified lines. ...
  Edit File
  Created rc5-marker.txt ...
WRITE-DONE
Completed in 7.8s (end_turn)
```

**感受：**

- 比 rc.4 的 `[thinking] text …` / `fc_… toolCallId` **自然得多**。  
- 终局一行 `Completed in … (end_turn)` 很像成熟 CLI。  
- thinking 缩进为正文，assistant 最终句清晰。  
- 仍偶发 thinking 与正文层次靠缩进区分，工具行只有标题 `Edit File`/`Read File`（信息够日常，排障时要 `--json`/raw）。

### 2.7 show（质变 + 仍糙）

**质变：** 默认 show **有完整对话正文**（You: / 缩进 thought / 最终句 / 工具标题）。  
`--tail` / `--no-tools` / `--kinds` / `--from-seq` / `--run` / `--max-chars` **均在 help 且实测 tail/no-tools/kinds 可用**。  
`--json` 的 `transcript.items[].bodyText` 结构清晰。

**仍糙（日常会烦）：**

1. **词间丢空格 / 错误折行**  
   - `asingle-line response`  
   - `rc5-marker.txtwith the two`  
   - 多行文件内容显示成 `RC5-FULL-UX-20260725PASS`（**换行被吃**）  
2. 用户多行 prompt 在表头路径下被挤成一行（创建文件指令里两行内容粘在一起）。  
3. soft-delete 后 **show 仍吐全文**（tombstone + 完整 transcript）——语义要文档写死。

**JSON 层 bodyText 换行是对的** → 问题主要在 **人读渲染**，不是 Store 丢数据。

### 2.8 wait（四原语核心，实测通过）

| 场景 | 结果 |
|------|------|
| `wait` 空闲 | `not_busy`（正确，exit 1） |
| `send --no-wait --json` | 立即 `accepted` + `runId` + `busy:running`（~25ms） |
| 随后 `wait --json` | 流式 message NDJSON + `final{stopReason:end_turn}` ~3.6s |
| 长任务 `--no-wait` → 3s 后 `cancel` | cancel 成功；show `status=cancelled` |
| cancel 后立刻裸 `wait` | `not_busy`（run 已终态，默认只认 in-flight） |
| `wait --run <cancelledRunId> --json` | **立即**回放 messages + `final status=cancelled` |
| `wait --run <completedRunId>` | 回放 + `end_turn` |

**感受：**

- **发送与等待分离已从设计变成可摸到的产品。**  
- 旁观路径（A 投递、B wait）成立。  
- 已结束 run 必须带 `--run`，默认 wait 不「复盘上一轮」——合理，但 help/doctor 应一句点破，避免 cancel 后裸 wait 误以为失败。  
- 本轮未并行起「wait 挂着再 cancel」（cancel 太快已终态）；`--run` 回放弥补了终态查询。

### 2.9 search

- 能命中 `FULL-UX-RC5` / `WRITE-DONE` / `NOWAIT`。  
- snippet 仍偶见协议味：`type text text Reply with…`  
- 功能合格，观感一般。

### 2.10 sessions / list

| 命令 | 结果 | 感受 |
|------|------|------|
| `agent sessions cursor` | ~5.5s；`showing 20 of 430`；当前 IN_HUB=true | workbench 默认切片 OK；博物馆巨大 |
| `conv list` | 默认 workbench 干净（本 hub 会话） | 好 |
| `conv list --all` | 大量 `imported_list` IDE 会话；中文 title 乱码/截断 | **仍吵**；表极宽 |

### 2.11 close / delete

| 操作 | 结果 |
|------|------|
| close | ok，`closed conversation …` |
| delete 默认 | **fail** `unsupported_capability: endpoint  does not support delete`（endpoint 后 **双空格**） |
| delete `--local-only` | ok |
| show after delete | status/phase=deleted，**消息仍完整展示** |
| 第二 conv + local delete | 路径顺利 |

**感受：** Cursor 用户若只记 `delete` 必踩坑；doctor 生命周期一句不够，**默认策略或错误提示应直指 `--local-only`**。

---

## 3. 统一问题清单（按优先级）

### P0 — 信任 / 主路径坑

| ID | 问题 | 证据 | 建议 |
|----|------|------|------|
| **P0-1** | **`agent add` 配置已写入，CLI 可长时间不返回** | 本轮强杀后 agents.json 完整 | RPC/客户端硬超时 + 「可能已成功」文案；修挂起根因 |
| **P0-2** | **默认 `conv delete` 对 Cursor 失败** | `unsupported_capability`；须 `--local-only` | 无 delete 能力时自动 local-only 或交互确认；错误文案给可复制命令 |
| **P0-3** | （已缓解）show 无正文 | rc.4 P0；**rc.5 已修** | 回归测试锁住「默认 show 含 user/assistant 文本」 |

### P1 — 日常手感

| ID | 问题 | 证据 | 建议 |
|----|------|------|------|
| **P1-1** | show 人读 **丢空格 / 吃换行 / 怪折行** | `asingle-line`；`…20260725PASS` | 按 raw body 保换行；禁错误 width wrap |
| **P1-2** | search snippet 仍有 `type text text` 噪音 | search FULL-UX | 与 send 人读同一去噪层 |
| **P1-3** | `conv list --all` / sessions 博物馆噪音 + 中文乱码 | 100+ 行 imported IDE | 默认过滤、encoding、更强 workbench 文案 |
| **P1-4** | 默认 `wait` 不复盘已结束 run | cancel 后裸 wait → not_busy | help：`已结束请 wait --run <id>`；或 `wait --last` |
| **P1-5** | param/mode 人读纯 JSON | param list | 表格式 + `--json` 机器用 |
| **P1-6** | delete 错误 `endpoint  ` 双空格 | stderr | 文案修复 |
| **P1-7** | soft-delete 后 show 全文 | show after delete | help 写清 tombstone；可选 `--purge` |

### P2 — 抛光

| ID | 问题 |
|----|------|
| **P2-1** | doctor 标题编码破损字符 |
| **P2-2** | 稳定版仍未上 crates.io Latest（沟通问题） |
| **P2-3** | send 工具行仅标题，无路径摘要（进阶排障靠 json） |
| **P2-4** | 多行 `--text` 在 PowerShell 仍易拆参（环境+文档） |

---

## 4. 明确通过（有证据，可写进 release 信任）

1. **安装校验**：rc.5 zip SHA 匹配，版本号正确。  
2. **四原语叙事**：help/doctor 与命令表一致（send/wait/show/cancel）。  
3. **probe 后能力缓存**：doctor 从 empty → ready。  
4. **reveal-paths**：list + inspect 可展开真实路径。  
5. **真实多轮对话**：ask → 写盘 → 读回；磁盘与回复一致。  
6. **show 默认可读对话**（相对 rc.4 关键里程碑）。  
7. **show 过滤参数**：tail / no-tools / kinds 工作。  
8. **send --no-wait + wait**：旁观路径完整；final stopReason 正确。  
9. **wait --run**：对 completed / cancelled 终态幂等回放。  
10. **mid-turn cancel**：status/last_outcome=cancelled。  
11. **空闲 cancel/wait**：稳定 `not_busy`。  
12. **close + local-only delete**：可用。  
13. **stderr 进度通道**：stage + timings 清晰。  

---

## 5. 设计对照：send / wait / show（用户意见落地情况）

此前反馈书主张：

> 发送与等待本应分离；show 要完整/最近/片段。

| 主张 | rc.5 状态 |
|------|-----------|
| send 默认可仍阻塞 | **是**（兼容） |
| send --no-wait | **是**，返回 accepted+runId |
| 独立 wait + 流式 | **是** |
| wait --run / --since-seq / --timeout / --json | **help 具备**；run/json 实测 |
| show 默认有正文 | **是** |
| show --tail / range / run / kinds | **help 具备**；tail/kinds/no-tools 实测 |
| 旁观者 wait | **实测通过** |

**结论：** UX-CORE 四原语在 rc.5 **已经从设计进入可体验实现**；剩余主要是 **渲染质量、注册挂起、delete 默认策略、博物馆噪音**。

---

## 6. 操作者推荐动线（当前版本可照做）

```powershell
$hubHome = '...\isolated-home'
$work    = '...\work'
$adapter = '...\adapters\cursor\adapter.mjs'
$H = @('--home', $hubHome)

acp-hub @H doctor
acp-hub @H agent add cursor --type stdio --command node --args $adapter --allow-root $work
# 若 add 卡住：另开终端 agent list；确认有 cursor 后可杀卡住的客户端

acp-hub @H agent inspect cursor --probe
$conv = (acp-hub @H conv create cursor --cwd $work).Trim()

acp-hub @H send $conv --text '...'                 # 默认同屏等到结束
# 或编排：
acp-hub @H send $conv --text '...' --no-wait --json
acp-hub @H wait $conv --json

acp-hub @H conv show $conv
acp-hub @H conv show $conv --tail 20 --no-tools
acp-hub @H search '关键词' --conv $conv

acp-hub @H cancel $conv                            # 仅 busy 时
acp-hub @H conv close $conv
acp-hub @H conv delete $conv --local-only          # Cursor 请带 --local-only
```

---

## 7. 给维护者的优先序（建议下一刀）

1. **修 agent add 挂起**（或保证超时可预期退出）——信任 P0  
2. **show 人读保换行/空格**——每天都碰  
3. **delete 无 remote 能力时的默认/提示**——Cursor 主路径  
4. **search 去噪 + list --all 编码/过滤**  
5. **wait 体验文案**（`--last` 或「已结束用 --run」）  

不必再争论「要不要拆 send/wait」——**rc.5 已拆对**；把边角做稳即可逼近稳定 0.2.1。

---

## 8. 总结表

| 问题 | 本轮结论 |
|------|----------|
| 能不能当真用 Cursor 多轮写盘？ | **能**（有磁盘证据） |
| 对话能不能回看？ | **能**（show 有正文；排版仍糙） |
| send/wait 是否已分离？ | **是**，旁观实测通过 |
| 最大信任杀手？ | **agent add 假死**、**默认 delete 失败** |
| 相对 rc.4？ | **明显进步**（show + UX-CORE 落地） |
| 是否可推全员默认工具？ | **尚未**；高级用户 / 隔离 home 推荐 |

**综合：B+（偏 A− 的进步版 prerelease）。**  
主路径已像产品；注册挂起与 delete/排版/博物馆仍像工程预览。

---

## 9. 证据索引

| 内容 | 路径 |
|------|------|
| 环境 | `tmp/acp-full-ux-20260725-232615/env.txt` |
| 逐步日志 | `…/journal/*.meta.txt` / `*.stdout.txt` / `*.stderr.txt` |
| 磁盘 marker | `…/work/rc5-marker.txt` |
| 主 conv | `conv-44bf6606aaed445997ab7dc37f97d9f9` |
| 设计对照（前序） | `doc/dev/feedback-book-send-wait-show-2026-07-25.md` |
| rc.4 全量（历史） | `doc/dev/ux-full-retest-feedback-2026-07-25-rc4.md` |

---

**修订：** 2026-07-25 初版 — rc.5 重装 + 真实全动线统一反馈。
