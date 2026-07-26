# ACP Hub 统一 UX 反馈书（全量真实动线）

| 字段 | 值 |
|------|-----|
| **版本** | `acp-hub 0.2.1-rc.6`（GitHub Pre-release） |
| **发布说明** | *fix: close rc.5 operator feedback (add/delete/show/wait)* |
| **安装** | `gh release download v0.2.1-rc.6` · ZIP SHA256 `ce0fe4fa…8bf8` 校验通过 · 覆盖 `~\.cargo\bin\acp-hub.exe` · bin SHA256 `6370703B…C706` |
| **日期** | 2026-07-26（操作墙钟跨 2026-07-25 晚～26 凌晨） |
| **主机** | Windows 11 / PowerShell 7 |
| **Agent** | `repos/acp-hub/adapters/cursor/adapter.mjs` + Cursor CLI ACP |
| **模型** | `grok-4.5[effort=high,fast=true]`（param 表） |
| **隔离 home** | `tmp/acp-full-ux-20260726-004507/home` |
| **证据** | `tmp/acp-full-ux-20260726-004507/journal/` |
| **性质** | **操作者全流程完整体验**，非 probe/exit-code 冒烟 |
| **对照基线** | [`ux-operator-baseline-and-feedback-0.2.1-rc.5.md`](./ux-operator-baseline-and-feedback-0.2.1-rc.5.md) |

---

## 0. 方法与动线

### 0.1 安装

1. 确认最新 tag：`v0.2.1-rc.6`（晚于本机原 `rc.5`）  
2. 下载 Windows x64 zip + `SHA256SUMS`，哈希匹配  
3. 覆盖 PATH 二进制 → `acp-hub 0.2.1-rc.6`

### 0.2 真实动线

```text
version / help / doctor(冷)
  → agent add cursor（冷启动：曾挂死；agents.json 已写；替换路径 38ms 成功）
  → list / inspect / probe / doctor / reveal-paths
  → conv create --cwd work
  → param list（表）/ mode list（表）
  → send 短答 RC6-FULL-UX-ASK-OK          ~10.8s
  → conv show 默认 + --json + --tail
  → send 写 rc6-marker.txt                ~6.2s + 磁盘核验
  → send 多轮读回（先 daemon_unavailable，重试 ~11.7s 成功）
  → wait 空闲 / wait --last / cancel 空闲
  → send --no-wait → wait --json          ~4.5s
  → send --no-wait 长文 → cancel（曾挂死 >10min；杀进程后）
  → 再测 cancel：~33ms 成功，wait --last → cancelled
  → search / sessions / list / list --all
  → close → **默认 delete 成功（auto local）**
  → 第二 conv create；send 再遇 daemon drop / not_found
```

**原则自检：** 默认路径是否仍足够短？复杂能力是否只在 flag 后？——见 §4。

---

## 1. 总体感受

### 1.1 一句话

**rc.6 把 rc.5 反馈里最伤「默认动线」的两刀基本砍掉了：`delete` 默认可成功；`wait --last` / 更友好的 `not_busy` 文案到位；param/mode 变表；show 换行明显好于 rc.5。**  
主对话写盘/旁观 wait 仍可用。

**但仍不是「默认可托付」：** 冷 **`agent add` 仍可假死**、**`cancel` 在 busy 长任务上曾挂死超过十分钟**、多轮中途 **`daemon closed the connection`** 仍会打断典型聊天。稳定性拖累综合分。

### 1.2 主观评级

| 维度 | rc.5 | **rc.6** | 说明 |
|------|------|----------|------|
| 发现与引导 | A− | **A−** | doctor 补了 delete 默认 local、wait --last |
| 注册 | B | **B / B−** | 替换秒回；**冷 add 仍挂** |
| send 主对话 | A− | **A−** | 自然流式 + Completed；偶发 daemon drop |
| wait | A− | **A** | `--last` 短 flag 落地；错误提示可执行 |
| show | B+ | **A−** | 有正文且换行改善；工具路径偶截断乱码 |
| delete 默认 | B− | **A−** | **默认成功 + 说明 auto local** |
| cancel | A−（快乐路径） | **C+** | 成功时 33ms；**长任务 cancel 可挂死** |
| 稳定性 | B | **C+** | daemon_unavailable / cancel hang / 冷 add |
| **综合** | B+ | **B+** | 默认动线进步，稳定性抵消 |

相对 rc.5：**产品默认语义更好**；**运行时可靠度未同步跟上**。

---

## 2. 相对 rc.5 基线：兑现了什么

| 基线 / 反馈项 | rc.6 实测 | 判定 |
|---------------|-----------|------|
| B-SHO-01 默认 show 有正文 | 有 You/assistant 正文 | **保持** |
| show 人读换行 | 文件两行显示正确 `RC6-…` / `PASS` | **改善**（rc.5 曾粘成一行） |
| B-SND-01 默认 send 等到 final | 是 | **保持** |
| B-WAI 旁观 no-wait+wait | 通过 | **保持** |
| wait 空闲提示 | 明确写 `--run` / **`--last`** | **改善** |
| **B-DEL-01 默认 delete** | `deleted … locally (agent has no session delete)` exit 0 | **达标（质变）** |
| doctor 写清 delete 默认 | 第 5 步 + lifecycle 文案 | **达标** |
| param/mode 人读 | **表格式**（非整坨 JSON） | **改善** |
| B-REG-01 agent add 必返回 | 冷路径仍可挂死；replace ~38ms | **未完全达标** |
| cancel 可靠 | 一次 >10min 挂死；另一次 33ms | **退步/不稳定** |
| 连续多轮无 daemon 崩 | 写后读、第二 conv send 曾 `daemon closed` | **未达标** |

---

## 3. 分命令体验笔记

### 3.1 help / doctor

- 四原语 + quick start 清晰。  
- doctor **ASCII 横线**（无 rc.5 编码破损 `?`）。  
- 新增：**delete 默认 local ok**；**wait --last** 指引。  

**感受：** 冷启动像「教你怎么用」，不是协议手册。

### 3.2 agent add / list / probe

| 观察 | 证据 |
|------|------|
| 冷 `agent add` 墙钟 >45s 仍不返回，但 `agents.json` 已写好 | 需强杀；与 rc.5 同类 |
| 再次 `agent add`（replace） | **38ms**，stdout `registered agent cursor` |
| probe | ~2.4s ok |
| reveal-paths list | 完整 node + adapter 路径 |

**感受：** 修好了「返回文案」，**没彻底修冷启动假死**。典型用户第一次 add 仍可能以为死机。

### 3.3 create / param / mode

- create ~4.3s，stage 清晰。  
- **param/mode 默认是表**，current 与 choices 一眼可见——符合「默认简单」。

### 3.4 send

| 轮次 | 结果 | 耗时 |
|------|------|------|
| 短答 `RC6-FULL-UX-ASK-OK` | 成功 | **10.8s** |
| 写 `rc6-marker.txt` | 磁盘 `RC6-FULL-UX-20260726` + `PASS` | **6.2s** |
| 多轮读回 | 先 `daemon_unavailable`；重试成功 | fail 1.0s / ok **11.7s** |
| no-wait 短答 + wait | final end_turn | wait **4.5s** |

人读输出干净：`Edit File` / `Read File` / `Completed in Xs (end_turn)`。  
show 中工具行有时带路径且截断乱码：`Edit …\rc6?`。

### 3.5 show

- 默认可读；tail 工作；换行对多行文件内容已基本正确。  
- soft-delete 后有 note：tombstone 保留 transcript（审计）；无 `--purge`——**高级能力未塞进默认，合格**。  
- 折行仍偶发（thought 断行），但不再 rc.5 级「粘词毁掉 marker」。

### 3.6 wait

| 场景 | 结果 |
|------|------|
| 空闲 wait | `not_busy` + **可执行建议** `--run` / `--last` |
| `wait --last` | 立即回放上一轮（含 tool/正文） |
| no-wait 后 wait --json | message 流 + final |
| cancel 后 wait --last | `status=cancelled` / `stopReason=cancelled` |
| kill daemon 后 wait --last | `failed` / `daemon_restarted`（可观测） |

**感受：** 旁观与复盘终于「短 flag 就够」，符合原则。

### 3.7 cancel（本版最大退步点）

| 场景 | 结果 |
|------|------|
| 空闲 cancel | `not_busy` + 同样友好提示 |
| 长 essay `--no-wait` 后 cancel | **CLI 挂死 >10 分钟**，强杀；run 事后 `failed`/`daemon_restarted` |
| 杀进程后新长任务 cancel | **33ms** 成功，`wait --last` → cancelled |

**感受：** 功能「有时很快」，「有时像死机」——比「稳定慢」更伤信任。P0。

### 3.8 search / sessions / list

- search 能命中；snippet 仍偶见 `toolCallId`/`rawOutput`。  
- sessions：`20 of 432`，IN_HUB 标记正确。  
- list 默认干净；`--all` 仍博物馆 + 中文截断——**正确放在复杂/全量形态**。

### 3.9 close / delete

```text
closed conversation …
deleted conversation … locally (agent has no session delete)   # 默认！
```

**这是 rc.6 对「默认=典型动线」最对齐的修复。** 不再要求用户先会 `--local-only`。

### 3.10 第二会话

- create2 成功 ~1.6s。  
- send2：`daemon closed the connection`。  
- 重试：`conversation_not_found`（daemon/投影状态在崩溃后不一致）。  
- delete2 仍报 local deleted（对缺失投影是否幂等需实现侧澄清）。

---

## 4. 原则对照（默认 vs 复杂）

| 原则 | rc.6 评价 |
|------|-----------|
| 典型动线零/少 flag | **进步**：delete 默认成功；show 默认可读；send 默认等到 final |
| 不常用进复杂参数 | **大体遵守**：museum `--all`、show 过滤、wait `--run` 合理；`--last` 做成短 flag 正确 |
| 禁止「配齐才像用户」 | delete 已对齐；**add/cancel/daemon 稳定**仍会逼用户「强杀进程」——那是更糟的复杂操作 |

---

## 5. 统一问题清单

### P0 — 信任 / 默认动线阻断

| ID | 问题 | 证据 |
|----|------|------|
| **P0-1** | **冷 `agent add` 可长时间不返回**（配置已写） | journal `20-agent-add`；replace 却 38ms |
| **P0-2** | **`cancel` 在 in-flight 长任务上可挂死** | `81-cancel` >10min；对比 `86-cancel` 33ms |
| **P0-3** | **多轮中 `daemon closed the connection`** | `62-send-read`、`111-send2`；打断「连续聊天」典型动线 |

### P1 — 日常手感

| ID | 问题 |
|----|------|
| **P1-1** | show 工具行路径截断/乱码（`rc6?`） |
| **P1-2** | search snippet 仍有 toolCallId/raw 噪音 |
| **P1-3** | daemon 重启后 conv 状态可能与客户端认知不一致（create 后 not_found） |
| **P1-4** | list --all / sessions 博物馆与中文乱码（可接受为高级，但编码应修） |
| **P1-5** | soft-delete 后 show 仍很长（有 note；可考虑默认摘要） |

### P2 — 抛光

| ID | 问题 |
|----|------|
| **P2-1** | 稳定版仍未上 crates.io Latest |
| **P2-2** | cancel 成功与挂死的根因需日志可诊断（操作者目前只能强杀） |

### 已关闭（相对 rc.5 反馈）

| 项 | rc.6 |
|----|------|
| 默认 delete 必须记 `--local-only` | **已关**（auto local + 文案） |
| wait 空闲无提示 | **已关**（指向 --last/--run） |
| param/mode 整坨 JSON | **已关**（表） |
| show BODY 空 | **仍关**（rc.5 已修，rc.6 保持） |
| show 吃换行毁掉两行文件 | **明显改善** |

---

## 6. 明确通过（有证据）

1. 安装 SHA + 版本 rc.6  
2. 四原语 + doctor 默认 delete / wait --last 叙事  
3. probe / reveal-paths  
4. 真实短答 + 写盘磁盘核验  
5. 多轮读回（重试后）  
6. 默认 show 可读 + 换行基本正确  
7. send --no-wait + wait 旁观  
8. wait --last 复盘  
9. cancel 成功路径（二次实测）  
10. **默认 conv delete 成功（Cursor 无 remote delete）**  
11. param/mode 表格式  
12. list 默认非博物馆  

---

## 7. 建议实现优先序

1. **根治冷 agent add 返回**（B-REG-01）——超时也行，禁止静默挂死  
2. **cancel 必须有界**：对 agent cancel RPC 硬超时；超时后本地 finalize cancelled/failed 并返回 CLI  
3. **daemon closed 后的自愈**：CLI 自动重连一次；conv 绑定不丢；错误文案区分「可重试」  
4. show 工具行路径截断编码  
5. search 去噪  

**不要**再堆更多高级 flag；默认动线已经接近正确形状——**把挂死砍掉**。

---

## 8. 与基线文档的关系

- 操作者原则与不回归表：[`ux-operator-baseline-and-feedback-0.2.1-rc.5.md`](./ux-operator-baseline-and-feedback-0.2.1-rc.5.md)  
- **建议：** 将 B-DEL-01 标为 **rc.6 已满足**；新增 **B-CAN-hang** 与 **B-DAEMON-reconnect** 门槛；冷 add 继续列为 P0。  
- 本文件为 **rc.6 全量体验 SSOT 反馈**，实现 PR 可对照关闭项。

---

## 9. 总结表

| 问题 | 结论 |
|------|------|
| 能不能当真多轮写盘？ | **能**（有磁盘证据） |
| 默认能不能删 Cursor 会话？ | **能**（auto local）——rc.5 不能 |
| 对话能不能默认回看？ | **能**，换行更好 |
| send/wait 分离？ | **能**，且有 `--last` |
| 最大信任杀手？ | **冷 add 假死**、**cancel 挂死**、**daemon 中途断连** |
| 综合 | **B+**：默认 UX 更像产品；稳定性仍是预览版 |

**一句话：**  
rc.6 正确地把「不常用的失败路径」收回默认（delete/wait 复盘），方向对；下一步不是加配置，而是 **保证 add/cancel/daemon 在有限时间内总有答案**。

---

## 10. 证据索引

| 项 | 路径 |
|----|------|
| 环境 | `tmp/acp-full-ux-20260726-004507/env.txt` |
| journal | `…/journal/*.meta.txt` 等 |
| 磁盘 marker | `…/work/rc6-marker.txt` |
| 主 conv | `conv-b38a62c1ae1749468921edf1b0bf759f` |

**修订：** 2026-07-26 初版 — rc.6 重装 + 真实全动线统一反馈。
