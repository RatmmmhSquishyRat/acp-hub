# ACP Hub 操作者基线与版本反馈

**版本锚点：** `0.2.1-rc.5`  
**日期：** 2026-07-25  
**性质：** ① **不回归基线（MUST NOT regress）** · ② 产品原则 · ③ 本版意见  
**证据：** 全量真实动线 `tmp/acp-full-ux-20260725-232615/`（本机操作者复测，非脚本冒烟）  
**详述体验笔记：** [`ux-unified-feedback-2026-07-25-rc5.md`](./ux-unified-feedback-2026-07-25-rc5.md)

> **给实现者：** 后续重构、发布、改默认值时，**不得**使本文 §2 基线指标相对 rc.5 实测**变差**。  
> 若有意改默认，必须先改本文并说明取舍，而不是默默让典型动线变复杂。

---

## 1. 产品原则（固定，高于实现偏好）

### 1.1 默认 = 典型用户动线

**禁止：** 为了「还原典型用户动线」而要求操作者堆复杂配置、记一长串 flag、先读三页设计。

**要求：**

| 层次 | 放什么 |
|------|--------|
| **零 flag / 最少参数** | 每天 80% 的事：注册、建会话、发消息等到结束、回看对话、取消、关掉、删掉本地投影 |
| **短 flag（常见可选）** | 稍进阶但周频：`--json`、`--no-wait`、`wait`、`--probe`、`--local-only`（在能力缺失时） |
| **复杂参数 / 高级形态** | 低频、排障、编排、博物馆：`--raw`、`--from-seq/--to-seq`、`--kinds`、`--run` 复盘、`--all`、sandbox、多 root、proxy 链… |

**口诀：**

> 典型路径要短；不常用的才进复杂参数。  
> 不是「配齐了才能像用户」——是「默认就像用户」。

### 1.2 典型动线定义（默认必须覆盖）

操作者心智中的「正常用一次」：

```text
doctor
  → agent add <id> --command …          # 一次成功返回，无需二次抢救
  → agent inspect <id> --probe          # 可选但推荐；冷路径可引导
  → conv create <id> --cwd <abs>
  → send <conv> --text "…"              # 默认同屏等到结束 + 可读流式
  → conv show <conv>                    # 默认可读完整正文，无需 --json
  → （可选）cancel / send 再一轮
  → conv close <conv>
  → conv delete <conv>                  # 默认应成功或明确降级成功，不靠记 --local-only
```

编排旁观（次常见，允许短 flag，仍不算「复杂配置」）：

```text
send <conv> --text "…" --no-wait
wait <conv>
```

**不得**把下列事项做成「默认才能走通」的前置：

- 必须手写完整 JSON 注册文件  
- 必须 `--reveal-paths` 才能确认装对了  
- 必须 `conv show --json` 才能看见对话  
- 必须 `--local-only` 才能删掉 Cursor 会话（应默认成功或自动降级）  
- 必须 `--run` 才能完成「发完等结果」（默认 `send` 已含 wait）  
- 必须先读博物馆 `list --all` 才能找到自己的会话  

### 1.3 与 UX-CORE 四原语的关系

| 原语 | 默认角色 | 复杂形态 |
|------|----------|----------|
| **send** | 投递 + 等到 final（一体，给真人） | `--no-wait`、`--param`、`--mode`、`--json` |
| **wait** | 旁观/编排 attach | `--run` 复盘、`--since-seq`、`--timeout`、`--json` |
| **show** | 完整可读 transcript | `--tail`/`--head`、区间、`--kinds`、`--raw`、`--json` |
| **cancel** | 停当前 run | （保持简单） |

实现者可重构内部，**不可**把「默认 send 不再等到结束」或「默认 show 无正文」当作无文档的默认变更。

---

## 2. 舒适与快捷基线（不回归指标）

### 2.0 如何读本表

| 列 | 含义 |
|----|------|
| **指标** | 可观察、可复测 |
| **rc.5 基线** | 本轮实测或定性锚点 |
| **门槛** | 重构后不得差于 |
| **回归判定** | 失败即基线下滑 |

**环境锚点（复测时对齐）：** Windows x64 · Cursor adapter + cursor-agent · 隔离 `--home` · 模型 `grok-4.5[effort=high,fast=true]` 或同级 · 工作区本机 SSD。  
**说明：** 含 LLM 的墙钟随模型/网络波动；**结构与 UX 门槛**优先于绝对毫秒。LLM 步给出 **参考 P50（本轮）** 与 **软上限**（异常报警，非硬 CI）。

---

### 2.1 发现与引导（无 LLM）

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-DISC-01 | `acp-hub --help` 是否点明典型表面 | 写明 send / wait / show / cancel 与 quick start | **必须**仍用 ≤ 一屏说明典型四原语 + 最短 quick start |
| B-DISC-02 | `doctor` 冷启动下一步 | 无 agent 时提示 `agent add` | **必须**可执行下一步，禁止空泛「see docs」 |
| B-DISC-03 | `doctor` 有 agent 且 cache 空 | 提示 probe / create | **必须**区分 empty vs ready |
| B-DISC-04 | version 命令 | 即时一行版本号 | **必须** < 200ms 本地返回（本轮 ~35ms） |

---

### 2.2 注册与探针（无 / 轻网络）

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-REG-01 | `agent add` **客户端是否返回** | **失败锚点：** 配置已写入但 CLI 可挂死 | **必须**在有限时间内返回成功或明确错误（建议 ≤ 15s 无 agent 冷启动；超时须非静默） |
| B-REG-02 | add 成功后 `agent list` 可见 | 是 | **必须** |
| B-REG-03 | 默认 list 路径 redact | 是 | **可保持**；不得默认泄露敏感绝对路径 |
| B-REG-04 | `--reveal-paths` 时 list 展示真实 command+args | **通过**（node + adapter 全路径） | **必须**仍能展开，不得退回仅 `<1 argument(s)>` |
| B-REG-05 | `inspect` 无 cache 时引导 | 提示 `--probe` | **必须** |
| B-REG-06 | `inspect --probe` 成功时延（Cursor 冷） | **~2.4s**（本轮） | 软上限 **≤ 15s**；失败须明确错误非挂死 |
| B-REG-07 | probe 后 doctor cache | ready | **必须** |

---

### 2.3 会话创建（起 agent 进程）

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-CRT-01 | `conv create` 成功 stdout | 纯 `conv-…` id（或 `--json` 可解析） | **必须**可脚本捕获 id |
| B-CRT-02 | create 墙钟（Cursor 冷会话） | **~4.3s**（本轮） | 软上限 **≤ 30s**；stderr 须有 stage 进度 |
| B-CRT-03 | 默认 interaction | `writable` | 典型 create **必须**可 send |

---

### 2.4 Send 默认路径（典型 = 阻塞到 final）

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-SND-01 | 默认 `send` 语义 | 阻塞到 final response | **禁止**默认 fire-and-forget（除非显式 `--no-wait`） |
| B-SND-02 | final 可观察 | `Completed in Xs (end_turn\|…)` 或 json final | **必须**有明确终态 |
| B-SND-03 | 短答一轮墙钟（本环境） | **~9.9s** | 软：合理 LLM 延迟；结构失败（无 final / 挂死）= 回归 |
| B-SND-04 | 写文件 + 磁盘一致 | **~7.8s**，marker 内容与约定一致 | **必须**仍能完成工具写盘且内容正确 |
| B-SND-05 | 同 conv 多轮上下文 | 读回文件内容正确 **~5.6s** | **必须**同 session 多轮可用 |
| B-SND-06 | 人读流式噪音 | 无默认 `text ` 类型字面量、无默认刷完整 toolCallId | **不得**退回 rc.4 级协议碎屑为默认 |
| B-SND-07 | stderr 进度 | `stage=daemon_connect\|prompt\|end` + timings | **必须**保留人可跟的进度通道 |
| B-SND-08 | stdout/stderr 分工 | 对话/终态偏 stdout；进度偏 stderr | **必须**保持，便于管道 |

---

### 2.5 Wait（旁观 / 编排，短 flag）

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-WAI-01 | `send --no-wait` 返回 | ~25ms 级 `accepted` + `runId` + `busy:running` | **必须**立即返回 runId（无等 LLM） |
| B-WAI-02 | 随后 `wait` 收到 final | 本轮 ~3.6s 得 `stopReason` | **必须**能旁观到与 ACP final 一致的终态 |
| B-WAI-03 | 空闲 `wait` | `not_busy`，非挂死 | **必须** |
| B-WAI-04 | `wait --run <已结束>` | completed/cancelled 均可幂等回放 final | **必须**保留 |
| B-WAI-05 | wait 默认是否发送新 prompt | 否 | **禁止** wait 隐式 send |

---

### 2.6 Show（回看 = 默认舒适）

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-SHO-01 | **默认** `conv show` 有用户与助手正文 | **有**（rc.5 质变） | **禁止**再出现「仅 ROLE、BODY 空」为默认 |
| B-SHO-02 | 无需 `--json` 可读上一轮问答 | 是 | **必须** |
| B-SHO-03 | `--json` 含 `transcript.items[].bodyText` | 是 | **必须** |
| B-SHO-04 | `--tail` / `--no-tools` / `--kinds` | 可用 | **必须**保留（属短/中频过滤，非默认负担） |
| B-SHO-05 | 人读换行与空格 | **未达标锚点：** 存在吃换行、粘词 | 目标：正文换行/空格与 Store 一致；**至少不得比 rc.5 json 更差**；人读应向 json 看齐 |
| B-SHO-06 | show 延迟（本地库） | ~24ms | **必须** < 1s 本地（无网络） |

---

### 2.7 Cancel / 状态机

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-CAN-01 | 空闲 cancel | `not_busy` | **必须** |
| B-CAN-02 | 运行中 cancel | 请求成功；终态 cancelled | **必须**可取消 in-flight |
| B-CAN-03 | busy 语义 | in-flight ⇒ `busy=running`；结束 ⇒ `busy=none` | **必须** |
| B-CAN-04 | last_outcome / status | completed / cancelled 等与 run 一致 | **必须** |

---

### 2.8 列表 / 搜索 / 博物馆

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-LST-01 | 默认 `conv list` | workbench 为主，干净 | **禁止**默认倾倒全量 IDE 博物馆 |
| B-LST-02 | `list --all` | 存在，噪声大 | 允许高级；**不得**变成默认 |
| B-LST-03 | `agent sessions` 默认切片 | `showing 20 of N` + 文案 | **必须**默认有界，博物馆进 `--all` |
| B-SEA-01 | `search` 能命中本轮关键词 | 是 | **必须** |
| B-SEA-02 | snippet 噪音 | 仍见 `type text text` | 目标降低；**不得**新增更重协议泄漏 |

---

### 2.9 生命周期删除（典型动线终点）

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-DEL-01 | 默认 `conv delete`（Cursor 无 remote delete） | **失败** | **不达标：** 典型终点失败。**目标门槛：** 默认成功删除 hub 投影，或自动 local 降级并打印一行说明；**禁止**要求用户先记住冷门 flag 才算「会删」 |
| B-DEL-02 | `--local-only` | 成功 | 保留为显式；但不应是唯一活路 |
| B-DEL-03 | close | 成功 | **必须** |

> **原则落地：** `delete` 无 remote 能力属于**能力差异**，应在**默认路径自动处理**；`--local-only` / 强 remote 才是复杂/显式形态——与 §1 一致。

---

### 2.10 稳定性与信任（舒适的底线）

| ID | 指标 | rc.5 基线 | 门槛（不回归） |
|----|------|-----------|----------------|
| B-STB-01 | 写配置类命令假死 | **add 可假死** | **必须**消除或超时可预期退出（见 B-REG-01） |
| B-STB-02 | 快乐路径 daemon | 本轮主路径可用 | 连续典型动线无 Access denied |
| B-STB-03 | 错误码可脚本化 | `error: <code>: …` | **必须**保持稳定前缀 |

---

### 2.11 「舒适手感」定性基线（评审用）

重构后人工走典型动线，应仍能给出 **≥ 下列主观判断**（相对 rc.5）：

| 感受 | rc.5 锚点 | 不回归 |
|------|-----------|--------|
| 像工作台而非协议调试器 | send/show 已偏工作台 | 不得退回「默认只配协议人员」 |
| 回看不靠 json | 默认 show 能读 | 不得倒退 |
| 发完有终局感 | `Completed in …` | 不得消失 |
| 进度不瞎等 | stderr stage | 不得消失 |
| 删会话不背锅 | **未达标** | 修到达标后锁住 |

---

### 2.12 基线复测最小剧本（实现者自证）

在隔离 `--home` 下执行（Cursor 或同等 ACP agent）：

1. `doctor` → `agent add` → **必须返回** → `inspect --probe`  
2. `conv create --cwd <work>`  
3. `send --text` 短答 → 见 final  
4. `conv show` **默认**见 user + assistant 正文  
5. `send` 写文件 → 读磁盘  
6. `send --no-wait` → `wait` 见 final  
7. 运行中 `cancel` 或空闲 `not_busy`  
8. `conv close` → `conv delete`（**默认应成功**；若仅 local，须自动降级）  

任一步需「查隐藏文档才知道的复杂配置」才能过 → **原则回归**。  
任一步 §2 门槛失败 → **指标回归**。

---

## 3. 本版（0.2.1-rc.5）意见与建议

### 3.1 总评

rc.5 是 **UX-CORE 第一次可摸到的实现**：四原语进 help/doctor，show 有正文，send/wait 可拆，主对话写盘多轮真实可用。  
综合操作者感受约 **B+**，比 rc.4 明显进步。

尚未达到「默认可推荐给所有人」：注册假死、默认 delete 失败、show 人读折行，仍像预览版。

### 3.2 做得好的（应写入基线、保护）

1. **默认 send = 等到 final**，并给出 `Completed in …`  
2. **默认 show 有正文**（相对 rc.4 质变）  
3. **`--no-wait` + `wait` 旁观**真实可用  
4. **stderr 进度 / stdout 正文** 通道清楚  
5. **reveal-paths 真能展开 list 路径**  
6. **doctor 四原语引导**  
7. **sessions / list 默认有界**，博物馆进 `--all`  

### 3.3 意见：默认仍不够「典型」

| 意见 | 说明 |
|------|------|
| **删除不应惩罚小白** | Cursor 无 session delete 时，默认 `delete` 应删本地投影并说明，而不是报 unsupported 让人去搜 `--local-only` |
| **注册返回是信任底线** | 配置已写但 CLI 挂死 = 比失败更糟；必须修 |
| **show 人读要保真** | JSON 有换行、终端粘成 `…PASS` / `asingle-line` → 人读层 bug，默认舒适被扣分 |
| **复杂能力别挤默认** | `--raw`、seq 区间、kinds、museum `--all` 保持高级；不要反过来要求默认用户用它们才能「看清」 |
| **wait 空闲报错 OK** | 但 cancel 后用户若裸 wait，应在错误中提示 `wait --run <id>` 或提供 `wait --last`（短 flag，非复杂配置） |

### 3.4 建议改动（按优先级）

#### P0 — 对齐 §1 原则 + 信任

1. **`agent add` 保证返回**（超时 + 根因）；满足 B-REG-01  
2. **`conv delete` 默认成功路径**：无 remote delete 时自动 local + 一行 `deleted locally (agent has no session delete)`；`--remote` / 强制 remote 才走复杂失败  
3. **锁住 B-SHO-01**（CI 或 release 手测剧本 §2.12）

#### P1 — 日常舒适

4. show 人读：**保留换行与空格**（向 bodyText 看齐）  
5. search snippet 去 `type text text`  
6. `wait` 在 `not_busy` 时提示如何复盘上一 run  
7. param/mode 默认表格式，`--json` 给机器  

#### P2 — 抛光

8. delete 错误文案双空格  
9. doctor 编码字符  
10. soft-delete 后 show 是否全文：文档写清或默认摘要 + `--full`  

### 3.5 明确反对的方向

- 为了架构纯度，把默认 `send` 改成必须 `send + wait` 两步才能「正常聊天」  
- 为了省实现，默认 show 再变空 BODY、逼用户 `--json`  
- 把「安全 redact」做成 reveal 后仍看不清 command  
- 用「用户应读 design doc」替代默认可用  

### 3.6 对文档/发布

- 本文为 **操作者基线 SSOT 补充**；实现 PR 若动默认语义，须勾选「未破坏 §2」或更新基线版本号。  
- 稳定 `0.2.1` 建议准入：§2.12 剧本全绿 + P0-1/P0-2 关闭。  

---

## 4. 基线版本与变更规则

| 字段 | 值 |
|------|-----|
| **Baseline ID** | `ux-operator-baseline/0.2.1-rc.5` |
| **冻结日** | 2026-07-25 |
| **允许** | 指标变好；高级 flag 增加且不影响默认 |
| **禁止** | 默认路径变长、默认能力变弱、无说明放宽 §2 门槛 |
| **修订** | 改门槛须 bump Baseline ID 后缀或新文件，并写 changelog 一句 |

---

## 5. 证据与关联

| 项 | 位置 |
|----|------|
| 本轮 journal | `tmp/acp-full-ux-20260725-232615/journal/`（操作者机；可复测） |
| 统一体验反馈 | [`ux-unified-feedback-2026-07-25-rc5.md`](./ux-unified-feedback-2026-07-25-rc5.md) |
| send/wait/show 设计意见 | [`feedback-book-send-wait-show-2026-07-25.md`](./feedback-book-send-wait-show-2026-07-25.md) |
| rc.4 全量（历史对照） | [`ux-full-retest-feedback-2026-07-25-rc4.md`](./ux-full-retest-feedback-2026-07-25-rc4.md) |

---

## 6. 一页纸摘要

**原则：** 默认走典型动线；复杂配置只服务不常用操作。  

**已达舒适基线（须锁）：** 默认 send 等到 final；默认 show 有正文；no-wait+wait 旁观；进度通道；list 默认非博物馆；reveal-paths 可用。  

**未达（修完后锁）：** agent add 必返回；默认 delete 在无 remote 能力时仍成功。  

**本版意见：** rc.5 方向对、进步大；下一步不是加更多专家 flag，而是 **把删除与注册做成默认可靠**，并把 show 人读保真。

---

**文档结束。**
