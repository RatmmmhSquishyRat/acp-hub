# QA 探针：冷 `agent add` 假死（实现者用）

**视角：** 纯 PRD / QA / 外部用户 — **不读源码、不 import 库、不依赖内部 API。**  
**只观测：** CLI 进程是否在限时内退出、退出码、列表是否可见、本地注册文件是否已写入。  
**问题定义（外行）：** 第一次登记 agent 时，配置可能已经写好，但命令行一直不结束，像死机。

---

## 实现者必须怎么用

1. 安装待测 `acp-hub` 到 `PATH`（或设 `ACP_HUB_BIN`）。
2. 在**干净机器或新临时目录**上跑（脚本自建隔离 home）。
3. **不要**先手动 `serve`；脚本只调公开 CLI。
4. 退出码 **0 = 探针通过**；非 0 = 未过门槛（见下方指标）。
5. 把脚本 stdout 的 JSON 摘要贴进 PR / CI 日志。

### Windows (PowerShell)

```powershell
cd path\to\acp-hub
pwsh -File doc/qa/cold-agent-add-hang/probe-cold-agent-add.ps1
```

### 可选环境变量

| 变量 | 默认 | 含义 |
|------|------|------|
| `ACP_HUB_BIN` | `acp-hub` | CLI 路径 |
| `PROBE_BUDGET_SEC` | `15` | **硬超时**：超时未退出 = 假死 FAIL |
| `PROBE_SOFT_SEC` | `3` | 软目标：超过记 `OK_SLOW`（仍 PASS） |
| `PROBE_AGENT_ID` | `qa-cold-probe` | 注册用 id |
| `PROBE_COMMAND` | `node` | stdio command（须本机存在） |
| `PROBE_ARGS` | 临时 fixture `.js` 路径 | 用 `\|%` 分隔多个 `--args` 值。默认惰性脚本路径（**勿**传以 `-` 开头的参数，CLI 会解析失败） |
| `PROBE_ROUNDS_WARM` | `1` | 冷成功后再跑几次「热/替换」对比 |

### 复现「像用户登记 Cursor」时

默认 fixture 只保证 **能登记**。若要贴近真实卡顿场景，传入与生产相同的 command/args，例如：

```powershell
$env:PROBE_COMMAND = 'node'
$env:PROBE_ARGS = 'C:\path\to\adapters\cursor\adapter.mjs'
pwsh -File doc/qa/cold-agent-add-hang/probe-cold-agent-add.ps1
```

仍只观测公开 CLI + `agents.json`，不读源码。

---

## 测什么（指标）

每次 `agent add` 记录：

| 指标 | 含义（用户能懂） |
|------|------------------|
| `wall_ms` | 从启动 CLI 到进程结束的墙钟毫秒 |
| `exited` | 是否在 `PROBE_BUDGET_SEC` 内正常结束 |
| `exit_code` | 进程退出码（超时杀进程记为 `null` + `timed_out=true`） |
| `listed_after` | 结束后（或超时杀后）`agent list` 是否出现该 id |
| `agents_json_has_id` | home 下 `agents.json` 文本是否包含该 agent id |
| `class` | 结果分类（下表） |

### 结果分类 `class`（给实现者修 bug 用）

| class | 用户体感 | 判定 |
|-------|----------|------|
| `OK` | 限时内结束且 list 能看见 | PASS |
| `OK_SLOW` | 限时内结束且可见，但 `wall_ms > soft` | PASS + WARN |
| `HANG` | 超时仍未退出 | **FAIL（假死）** |
| `HANG_BUT_WRITTEN` | 超时未退出，但 json/list 已有该 id | **FAIL（典型假死）** |
| `RETURNED_BUT_INVISIBLE` | 退出 0 但 list/json 都没有 | **FAIL（假成功）** |
| `FAILED_CLEAN` | 非 0 退出且未写入 | FAIL（登记失败，但是干净失败） |
| `FAILED_BUT_WRITTEN` | 非 0 退出但已写入 | **FAIL（状态与返回不一致）** |

### 门禁（脚本默认）

- **冷路径** `class` 必须是 `OK` 或 `OK_SLOW`。  
- **`HANG` / `HANG_BUT_WRITTEN`** → 总失败（这就是「冷假死」）。  
- 热路径仅作对比输出，不单独否决（除非你改脚本）。

---

## 测序（盲测逻辑）

1. 新建空 home（不复用用户 `~/.acp-hub`）。  
2. **冷：** 第一次 `agent add <id> …`，硬超时 `PROBE_BUDGET_SEC`。  
3. 无论成败：查 `agent list` + 读 `agents.json` 是否含 id。  
4. **热：** 再 `agent add` 同一 id 一次（替换），同样记指标。  
5. 打印人类摘要 + 一行 JSON。

**禁止：** 读 hub 源码、调内部 RPC、假设 daemon 文件名以外的实现细节。  
**允许：** 读 home 里用户可见的 `agents.json`（操作者排障也会打开）。

---

## 与产品问题的对应

| 用户说法 | 探针如何抓住 |
|----------|----------------|
| 一直转圈/不结束 | `timed_out` / `HANG*` |
| 其实已经加上了 | `HANG_BUT_WRITTEN` 或 `listed_after` |
| 第二次很快 | 冷 `wall_ms` ≫ 热 `wall_ms` |
| 说成功了但 list 没有 | `RETURNED_BUT_INVISIBLE` |

---

## 实现者验收建议

- 每个修 register/daemon 的 PR：跑本脚本，冷路径 `OK`/`OK_SLOW`。  
- CI 可选：`PROBE_BUDGET_SEC=15` 作为门禁。  
- 若故意异步登记：产品须改成「立刻返回 + 明确进行中」；在改 PRD 前，**仍按 HANG 失败**。

---

## 文件

| 文件 | 用途 |
|------|------|
| `probe-cold-agent-add.ps1` | Windows / pwsh 探针 |
| `probe-cold-agent-add.sh` | Linux/macOS bash 探针（同指标） |
| `README.md` | 本说明 |
