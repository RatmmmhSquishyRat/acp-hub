# ACP Hub CLI 全量复测与统一反馈（2026-07-25 · v0.2.1-rc.4）

**版本：** `acp-hub 0.2.1-rc.4`  
**装法：** 本轮重新 `gh release download v0.2.1-rc.4`，ZIP SHA256  
`6cb3a33ca5d682193e9b0f537a47548d6c5610b621d544a0d25f095cf80dc2d0` 校验通过后覆盖  
`%USERPROFILE%\.cargo\bin\acp-hub.exe`（bin SHA256 `62FB6412…0295`）  
**主机：** Windows 11 / PowerShell 7.6  
**Agent：** 发布包 Cursor adapter + 本机 `cursor-agent`（`2026.07.23-e383d2b`）  
**默认模型（param 观测）：** `grok-4.5[effort=high,fast=true]`  
**产物根：** `tmp/acp-full-ux-20260725-154653/`  
（`install/`、`home-v2`/`home-v3`、`work/`、`journal/` 共 180+ 步日志）  

**前序（不再当作本轮结论）：**

- [ux-walkthrough-feedback-2026-07-25-rc4.md](./ux-walkthrough-feedback-2026-07-25-rc4.md) — 约 40s 脚本 walk，**方法不足**
- [ux-walkthrough-feedback-2026-07-25-rc3.md](./ux-walkthrough-feedback-2026-07-25-rc3.md)
- [ux-walkthrough-feedback-2026-07-24.md](./ux-walkthrough-feedback-2026-07-24.md)

**性质：** 操作者全量复测 + 证据化反馈；非 CI 门禁。

---

## 0. 方法自我纠正

用户指出得对：

> 测试太快、版本改动多却不做重新全量体验，不能叫做测试。

此前 rc.4 文档主要依赖 **脚本批量判 exit code**（约几十秒跑完 30+ 步）。那种做法只能证明「命令没立刻崩」，**不能**证明：

- 真实 agent 轮次是否完整结束  
- 写盘内容是否正确  
- 多轮上下文是否接得上  
- 中途 cancel 是否生效  
- daemon 在压力/异常下是否仍可服务  
- close / delete 语义是否与文案一致  

**本轮做法：**

| 项 | 本轮 |
|----|------|
| 安装 | 重新下载 prerelease + SHA 校验 + 覆盖 PATH 二进制 |
| Home | 隔离 `home-v2` / `home-v3`，不污染 `~/.acp-hub` |
| 等待 | `send` 单步允许 240–360s；按真实 agent 完成计时 |
| 写盘验证 | 读 `work/full-ux-marker.txt` 原文，不靠 CLI 自报 |
| 多轮 | 同一 conv 上 ask → write → follow-up read |
| 中途 cancel | 长任务启动后 4s 发 `cancel`，看 `stop_reason` |
| 生命周期 | close → delete（含失败路径）→ `--local-only` |
| 错误路径 | inspect 缺失 / send 坏 id / cancel 空闲 |
| 墙钟 | 安装约 15:46 → 主体路径结束约 16:12（**约 26 分钟**，含失败恢复） |
| 日志 | 每步独立 `*.stdout/stderr/meta`；**57 个 meta 计时点** |

本轮结论以 journal 为准；不把「脚本 29/32 pass」当作体验结论。

---

## 1. 总体判断（全量复测后）

| 维度 | 判断 | 说明 |
|------|------|------|
| 安装与发现 | **可用** | help / doctor 旅程完整；prerelease 提示清楚 |
| 注册与 probe | **可用** | 冷 inspect 引导 probe；probe 后 cache ready |
| 主对话（ask/write/多轮） | **可用且真实成功** | 见 §3 计时与 marker |
| mid-turn cancel | **可用（本轮实证）** | `stop_reason=cancelled`，show 为 cancelled |
| 列表 / search | **能用但吵** | workbench 清晰；sessions / list --all 博物馆噪音大 |
| show 可读性 | **不足** | 元数据好，**消息 BODY 列为空**，回看对话困难 |
| send 流式人读 | **半成品** | 角色标签有了，但仍有 `text ` 碎屑与 toolCallId |
| delete 默认路径 | **踩坑** | cursor 无 remote delete → 失败；需 `--local-only` |
| 稳定性（异常后） | **有风险** | home-v2 出现 daemon 高 CPU + 并发 CLI Access denied |
| 发布态 | **仍 prerelease** | crates.io Latest 仍 0.2.0 |

**一句话：**  
在干净 home、正常顺序操作下，**rc.4 主路径（register → probe → create → send 多轮写读 → cancel → close → local delete）可以当真用**；但「show 看不清正文」「send 仍有协议碎屑」「默认 delete 对 Cursor 失败」「异常后 daemon 可能拒绝访问」让它仍停留在 **认真 prerelease / 高级用户 workbench**，还不是无脑推荐给所有人的稳定 CLI。

**主观综合（全量后）：B+ / A− 之间偏 B+**（功能通，可信度被稳定性与 show/send 细节拖住）。  
相对「40s 脚本的 A−」下调一档是合理的。

---

## 2. 安装与版本

| 项 | 值 |
|----|-----|
| 最新 GitHub tag | **v0.2.1-rc.4**（Pre-release） |
| 稳定 Latest | 仍 **v0.2.0** |
| crates.io | 仍 **0.2.0** |
| 本机 `acp-hub --version` | `acp-hub 0.2.1-rc.4` |
| 顶层 help | Quick start / stderr·stdout 通道 / `--reveal-paths` / prerelease 说明齐全 |

`agent add` 传 `--args --agent-bin ...` 会被 clap 当成未知 flag（需 `-- --agent-bin` 或改注册方式）。官方 Cursor adapter README 的默认注册（仅 adapter 路径）本轮可用，cursor-agent 由 adapter 自行发现。

---

## 3. 主路径实测（证据）

### 3.1 关键步骤墙钟（home-v3）

| 步骤 | 结果 | 耗时 |
|------|------|------|
| `agent list`（已注册） | ok | 240 ms |
| `agent inspect cursor --probe` | ok，cache populated | **1.7 s** |
| `doctor`（probe 后） | cache ready，next=create | 64 ms |
| `conv create cursor --cwd work` | `conv-9cd3e130…` | **4.9 s** |
| `send` 短答 `FULL-UX-ASK-OK` | 匹配成功 | **14.7 s** |
| `send` 写 `full-ux-marker.txt` | 写盘成功 | **12.6 s** |
| `send` 多轮读回文件内容 | 内容正确 | **7.3 s** |
| 长任务 + `cancel`（约 4s 后） | `stop_reason=cancelled` | send 总 **4.1 s** |
| `conv close` | closed；workbench list 空 | 26 ms |
| `conv delete`（默认） | **失败** unsupported_capability | 29 ms |
| `conv delete --local-only` | deleted | 35 ms |

磁盘验证（**不以 CLI 文案代替**）：

```
work/full-ux-marker.txt
FULL-UX-20260725
PASS
```

### 3.2 send 人类可读样本（仍有碎屑）

短答：

```text
[thinking] text The response will be text exactly one line: FULL-UX-ASK-OK.
[assistant] text FULL-UX-ASK-OK
final: conv=… stop_reason=end_turn
```

写盘：

```text
[tool] fc_otWRnoM-3LYxF7-52314f8e-aws_ue1_0 title Edit File kind edit rawInput | …
[assistant] text WRITE-DONE
```

**问题：**

1. `[thinking]/[assistant]` 后仍夹 **`text `** 字面碎屑（像 content-type 泄漏）。  
2. tool 行仍带 **完整 toolCallId**（`fc_…`），rc.4 声称「去 toolCallId 噪音」**未做干净**。  
3. 偶发粘连：`onlytext  \`WRITE-DONE\``。

stderr 进度通道本身清晰：`stage=daemon_connect|prompt|end` + `timings total_ms=…`。

### 3.3 show：元数据对、正文不可用

`conv show` 字段（id/agent/session/status/cwd/last_outcome）正确；  
但消息表 **BODY 列整列空白**（仅 ROLE 如 thinking/assistant/tool），操作者无法从 show 回看对话内容。  
对「会话博物馆 / 排障」这是 **硬伤**，比 send 碎屑更影响日常使用。

### 3.4 search

`search FULL-UX` / `search WRITE-DONE --conv …` 能命中，snippet 有高亮。  
仍会出现 `toolCallId … rawOutput` 类噪音行；Windows 控制台对中文/路径偶发乱码（编码问题，需区分产品 vs 终端）。

### 3.5 param / mode

- `param list`：mode=agent / model=grok-4.5[…]，选项丰富，JSON 可读。  
- `mode list` / `mode set agent`：成功。  
本轮未再强制切 ask 模式做对照（agent 模式已覆盖写盘与读盘）。

### 3.6 sessions

`agent sessions cursor` 约 1.1s，**不崩**。  
当前会话 `IN_HUB=true` 标记正确；其余大量历史/IDE 会话以 RO 列出，标题截断，**博物馆噪音依旧**。  
`conv list` 默认 workbench 干净；`conv list --all` 被 imported IDE 会话淹没，中文标题在捕获日志中乱码严重。

### 3.7 cancel

| 场景 | 结果 |
|------|------|
| 空闲 cancel | `error: not_busy: …`（正确） |
| 运行中 cancel（4s 后） | `requested cancellation …`；send `stop_reason=cancelled`；show `status=cancelled` |

**本轮首次认真验证 mid-turn cancel：通过。**

### 3.8 close / delete

| 操作 | 结果 | 备注 |
|------|------|------|
| `conv close` | ok | workbench list 变空；`show` 仍可见，`phase=closed` |
| `conv delete` | **fail** | `unsupported_capability: endpoint  does not support delete (requires session_capabilities.delete)`；错误文案里 `endpoint` 后 **双空格**（小瑕疵） |
| `conv delete --local-only` | ok | `status/phase=deleted`；但 **show 仍返回 tombstone + 消息表**（软删语义，需在 help 写清） |

Cursor 路径下，文档/doctor 若只写 `delete` 不写 `--local-only`，操作者会踩坑。

### 3.9 错误路径

| 命令 | 退出 | 文案 |
|------|------|------|
| `agent inspect nope` | 1 | `agent_not_found: agent not found: nope` |
| `send not-a-conv --text hi` | 1 | `conversation_not_found: …` |
| `conv show conv-does-not-exist` | 1 | `conversation_not_found: …` |

错误前缀稳定、可脚本化；合格。

---

## 4. 稳定性与恢复（本轮新发现，脚本 walk 测不到）

### 4.1 home-v2：daemon Access denied

一次长 `send`（stdin 路径 / 客户端挂起）后观察到：

- `acp-hub serve` **CPU 持续数百**（观测到 ~700+）  
- 并发 `conv show` / `conv list` → **`error: io error: 拒绝访问 (os error 5)`**  
- adapter + `cursor-agent … acp` 仍挂在 daemon 下  
- 必须 **强制杀进程树** 才能恢复  
- 该次 **marker 未写出**

日志：`journal/FINDING-daemon-access-denied.txt`。

这不是「help 文案」问题，而是 **操作者信任底线** 问题。rc.4 在快乐路径上很稳，但异常后的自愈/超时/锁语义仍不够。

### 4.2 客户端挂起 vs 服务端已完成

曾出现：`agents.json` 已写好（agent add 实质完成），但 CLI 客户端长时间不退出；  
`WaitForExit`/Kill 在个别捕获方式下也不干净。  
**建议：** 所有写配置类命令对 daemon RPC 设明确超时；客户端超时后打印「配置可能已写入，请 agent list 核对」。

### 4.3 PowerShell 使用陷阱（环境，但影响 Windows 体验）

- `Start-Process` 拆多行 `--text` 会把参数拆碎 → `unexpected argument 'a'`  
- 应用 `ProcessStartInfo.ArgumentList` 或 `--stdin`  
- 变量名 `$home` 在 PowerShell 中只读，脚本易踩坑（应用 `$hubHome`）

产品侧可在 Windows 文档给一条 **推荐调用示例**。

---

## 5. 与「rc.4 声称改进」对照

| rc.4 方向 | 全量复测结论 |
|-----------|----------------|
| doctor 感知 cache | **成立**（empty → ready 文案切换正确） |
| send/show 压缩、去 toolCallId | **部分成立**：标签有了；**toolCallId 与 `text ` 仍在** |
| `--reveal-paths` | **部分成立**：inspect 可揭路径；**list 仍只显示 `node <1 argument(s)>`** |
| sessions 博物馆文案 | 不崩；**列表仍然很长很吵** |
| search 去 content-type 噪音 | **改善有限**（仍见 toolCallId/raw 类 snippet） |
| 状态一致性 | **快乐路径好**；异常路径见 §4 |

---

## 6. 问题清单（按优先级）

### P0 — 影响信任 / 可用性

1. **异常后 daemon 可进入 Access denied + 高 CPU**，需强杀（§4.1）  
2. **`conv show` BODY 为空**，无法回看对话（§3.3）  
3. **默认 `conv delete` 在 Cursor 上失败**，应默认降级 `--local-only` 或 doctor/help 强提示（§3.8）

### P1 — 日常手感

4. send 仍泄漏 **`text ` 碎屑** 与 **toolCallId**（§3.2）  
5. `--reveal-paths` 在 **agent list** 未真正展开 args 路径（§2 / §3）  
6. `sessions` / `conv list --all` 博物馆噪音；缺 filter（space / in_hub / limit）（§3.6）  
7. 客户端在 daemon 已成功时仍可能挂死（§4.2）

### P2 — 抛光

8. delete 错误文案 `endpoint  does not support` 双空格  
9. soft-delete 后 show 仍展示全文：语义要在 help 写清（tombstone vs purge）  
10. Windows 文档：ArgumentList / stdin 推荐写法  
11. 稳定版仍未出；crates.io 滞后（发布沟通问题，非功能 bug）

---

## 7. 明确通过的能力（本轮有证据）

- 重装 + SHA 校验安装路径  
- doctor 空 home / 有 agent / cache ready 三段引导  
- agent add / list（redact）/ inspect / probe  
- conv create（~5s 含 agent session）  
- param / mode list、mode set  
- send 短答、写文件、多轮读文件（磁盘一致）  
- search 按 agent / 按 conv  
- mid-turn cancel → cancelled  
- close 后 workbench 清空  
- delete --local-only  
- 标准错误路径文案  

---

## 8. 对「能不能当真用」的直接回答

| 场景 | 建议 |
|------|------|
| 本机个人 workbench、接受 prerelease、会 `--home` 隔离 | **可以真用**（本轮主路径实证） |
| 需要可靠回看历史对话 | **先等 show BODY 修复**，否则用 search / 外置日志 |
| 需要 Cursor 上「删会话」心智 | **务必 `--local-only`**，或等 hub 对无 delete capability 的降级 |
| 无人值守 / 长任务 / 强依赖 cancel 后继续 concurrent CLI | **谨慎**：先观察 §4.1 是否复现并修 |
| 给不熟 CLI 的用户当默认入口 | **还早**；help 很好，稳定性与 show 还不够 |

---

## 9. 建议的下一轮验证（避免再「测太快」）

最小全量清单（每项都要有墙钟 + 产物，而不是只看 exit code）：

1. 干净 home 安装校验  
2. doctor → add → probe → doctor  
3. create → param/mode  
4. send 短答（匹配 token）  
5. send 写文件 + **读磁盘**  
6. 同 conv 多轮  
7. show（**检查 BODY 非空**）+ search  
8. 长任务 + mid-turn cancel  
9. close + delete 默认 + delete --local-only  
10. 故意 kill daemon 后的恢复（新 home 或文档步骤）  
11. （可选）并发双 conv  

单轮预算：**15–40 分钟** 才像一次真实体验；40 秒脚本只能叫 smoke。

---

## 10. 产物索引

| 路径 | 内容 |
|------|------|
| `tmp/acp-full-ux-20260725-154653/install/` | 重装 zip + extract |
| `…/home-v3/` | 成功全量路径 home |
| `…/home-v2/` | Access denied 故障现场（已强杀） |
| `…/work/full-ux-marker.txt` | 写盘金标 |
| `…/journal/*.meta.txt` | 逐步 exit + ms |
| `…/journal/FINDING-daemon-access-denied.txt` | 稳定性发现 |
| `…/journal/v3b-10-send-ask.stdout.txt` 等 | send/show/cancel 原文 |

---

## 11. 结语

承认：此前把「脚本快速绿」写成「完整体验」，**不成立**。

本轮在 **重新安装 rc.4** 后，用 **约 26 分钟墙钟、真实 cursor-agent 多轮、写盘校验、中途 cancel、close/delete** 重新走过主链路。结论是：

- **主路径功能真实可用**（这点比 40s 脚本更站得住）  
- **show 正文、send 噪音、默认 delete、异常 daemon** 仍是诚实的产品债  
- 在这些问题收掉之前，综合评级不宜再报乐观的 A−  

若只改文档不改代码：至少应把「Cursor 删除请 `--local-only`」「show 目前不便读正文」「异常后 Access denied 需重启 daemon」写进 README / doctor note。
