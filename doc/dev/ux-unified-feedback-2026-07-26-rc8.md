# ACP Hub 统一 UX 反馈书（全量真实动线）

| 字段 | 值 |
|------|-----|
| **版本** | `acp-hub 0.2.1-rc.8` |
| **发布说明** | *fix: root-cause P0-1 register / P0-2 cancel hang (rc.8)* |
| **安装** | GitHub `v0.2.1-rc.8` Windows zip · SHA 校验通过 · 覆盖 PATH |
| **日期** | 2026-07-26 |
| **主机** | Windows 11 / PowerShell 7 |
| **Agent** | `adapters/cursor/adapter.mjs` + Cursor CLI ACP |
| **模型** | `grok-4.5[effort=high,fast=true]` |
| **隔离 home** | `tmp/acp-full-ux-20260726-102153/home` |
| **证据** | `tmp/acp-full-ux-20260726-102153/journal/` |
| **性质** | 操作者**全流程真实动线**（非 probe 冒烟）；**未**使用 `serve` |

---

## 0. 动线与方法

```text
version / help / serve --help / doctor
  → agent add（冷：挂死；agents.json 已写；replace 40ms 成功）
  → list / inspect / probe / doctor / reveal-paths
  → conv create
  → param / mode（表）
  → send 短答 ~11.6s → show 有正文
  → send 写 rc8-marker.txt ~7.3s → 磁盘核验 OK
  → send 多轮读回：高 CPU 挂死 ~15min（强杀）；重试再挂
  → show / wait --last / send --no-wait → wait OK
  → long --no-wait → cancel ~38ms → wait --last cancelled
  → search / sessions / list
  → close → 默认 delete auto-local 成功
```

测试路径：`acp-hub --home <隔离>` 各子命令；**不**跑 `serve`。

---

## 1. 总体感受

### 1.1 一句话

**快乐短路径仍像可用工作台**（send 终局、show 正文、写盘、旁观 wait、**cancel 有界返回**、默认 delete）。  
**连续多轮与冷注册仍不可靠**：冷 `agent add` 仍假死；第三轮起 `send` 可 **高 CPU 挂死** 需强杀——比 rc.6 的「daemon closed」更糟（进程占满 CPU 不退出）。

综合：**B / B+**（默认语义保持；稳定性在多轮上退步或未根治）。

### 1.2 相对 rc.6 / 发布声称

| 声称 / 项 | 本轮 |
|-----------|------|
| P0-1 冷 register 不挂 | **未兑现**：冷 add 仍挂；仅 replace 秒回 |
| P0-2 cancel 不挂 | **本轮长任务 cancel 38ms 成功**（改善，仍需防回归） |
| 默认 delete / wait --last / 表 param | **保持** |
| 多轮连续聊天 | **恶化**：读回轮 **高 CPU 挂死**，非清晰错误 |

---

## 2. 分路径结论

### 2.1 引导与文案

- 四原语 + doctor 清楚；默认 delete / wait --last 有说明。  
- **`serve` 仍只一句 *for a home directory***，未写「日常不必」——文案债仍在（见既有 serve/home 反馈）。  
- 主 help 仍突出 `serve` 与 `--home`。

### 2.2 注册

| 观察 | 证据 |
|------|------|
| 冷 `agent add` 长时间不返回，但 `agents.json` 已有 cursor | `20-agent-add-cold.meta` |
| 再次 add（replace） | **40ms** · `registered agent cursor` |
| probe | ~2.8s ok |

### 2.3 主对话（真实）

| 轮次 | 结果 | 耗时 |
|------|------|------|
| 短答 `RC8-FULL-UX-ASK-OK` | 成功 | **11.6s** |
| 写 `rc8-marker.txt` | 磁盘两行正确 | **7.3s** |
| 多轮读回 | **挂死**（~15min 高 CPU，强杀） | — |
| 重试读回 | 再挂，放弃 | — |

show 在成功轮次后有正文；`wait --last` 曾回放到读回 prompt + TLS/aborted 错误串（网络/abort 痕迹进 transcript）。

### 2.4 wait / cancel

| 场景 | 结果 |
|------|------|
| wait 空闲 | `not_busy`（快） |
| wait --last | 快回放 |
| no-wait + wait | final `end_turn` ~11.8s |
| long no-wait + cancel | **cancel 38ms**；wait --last → `cancelled` |

### 2.5 列表 / 删除

- search 能命中 RC8 关键词。  
- sessions 默认有界。  
- **默认 delete**：`deleted … locally (agent has no session delete)` **成功**。

---

## 3. 统一问题清单

### P0

| ID | 问题 |
|----|------|
| **P0-1** | **冷 `agent add` 仍可假死**（配置已写、CLI 不回）——rc.8 声称未在本环境根治 |
| **P0-2** | **多轮 `send` 可高 CPU 挂死**（本轮第三轮读回）；操作者只能强杀；无清晰超时错误 |
| **P0-3**（降级观察） | 异常后 transcript 可出现 TLS/aborted 类噪音；状态与「用户以为还在聊」可能脱节 |

### P1

| ID | 问题 |
|----|------|
| **P1-1** | `serve` / `--home` 主文案仍不自解释（日常不必 serve；home 应 advanced） |
| **P1-2** | search snippet 仍偏协议味（可接受次要） |
| **P1-3** | sessions 博物馆仍大 |

### 保持良好（勿回归）

- 默认 send 等到 final + `Completed in …`  
- 默认 show 有正文  
- no-wait + wait / wait --last  
- **cancel 有界**（本轮长任务路径）  
- 默认 delete auto-local  
- param/mode 表  
- reveal-paths  

---

## 4. 建议优先序

1. **根治冷 `agent add` 返回**（或 ≤15s 明确超时 + 文案）。  
2. **send 全路径硬超时 + 避免 busy-loop 高 CPU**；多轮不得静默卡死。  
3. cancel 保持有界（加回归测试，防 rc.6 挂死回潮）。  
4. CLI 文案：serve 可选、home 沉 advanced（专文反馈已单开）。  

---

## 5. 总结

| 问题 | 结论 |
|------|------|
| 短路径写盘/回看/旁观/删？ | **可用** |
| cancel 是否还挂？ | **本轮长任务路径不挂（38ms）** |
| 冷 add 是否修好？ | **否** |
| 连续多轮？ | **否（高 CPU 挂死）** |
| 综合 | **默认形状对；稳定性仍是发布门闩** |

**一句话：** rc.8 在 cancel 有界上像修了；**冷注册与多轮 send 仍会把操作者逼到强杀进程**——不能叫完整体验过关。

---

## 6. 证据

| 项 | 路径 |
|----|------|
| journal | `tmp/acp-full-ux-20260726-102153/journal/` |
| marker | `…/work/rc8-marker.txt` |
| conv | `conv-b9e284be300642e58c3c03a028c30712` |

**修订：** 2026-07-26 初版。
