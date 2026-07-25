# acp-hub CLI Cheatsheet（自测用）

版本参考：`0.2.1-rc.4`（以你本机 `acp-hub --version` 为准）  
环境：Windows PowerShell 7+ 为主；POSIX 在备注里。

---

## 0. 一次看懂（UX-CORE 四原语）

```
你 ──CLI──► Hub daemon（按 home 单例）──ACP──► 已注册 agent（如 cursor）
                 │
                 ▼
            hub home: agents.json / hub.db / daemon.*

send   —— 投递（默认阻塞到结束；--no-wait 立即返回 runId）
wait   —— 附着 in-flight / 已结束 run，增量守听
show   —— 任意时刻完整 / 尾 / 片段回看
cancel —— 打断 in-flight
```

| 概念 | 含义 |
|------|------|
| **home** | 状态目录。`--home <dir>` 或 `$env:ACP_HUB_HOME`，默认 `~\.acp-hub` |
| **agent id** | 你注册时起的名字，如 `cursor` |
| **conv id** | `conv create` 打印的 `conv-…` |
| **run id** | `send --no-wait` / `wait` 用的 `run-…` |
| **stdout** | 对话正文 / 最终结果 |
| **stderr** | `[acp-hub] stage=…` 进度与 timings |
| **sessions** | agent 侧历史「博物馆」（只读浏览） |
| **workbench** | `conv list` 默认：本 hub 可写会话 |

**自测务必用隔离 home**，别直接污染默认 `~\.acp-hub`（除非你就是要测默认）。

---

## 1. 本机准备（Windows）

```powershell
# 版本
acp-hub --version

# 建议：临时 home + 工作目录
$stamp   = Get-Date -Format 'yyyyMMdd-HHmmss'
$root    = "C:\Users\15480\Desktop\AIWorkshop\tmp\acp-selftest-$stamp"
$hubHome = Join-Path $root 'home'
$work    = Join-Path $root 'work'
New-Item -ItemType Directory -Force -Path $hubHome, $work | Out-Null

# 全局选项前缀（后面所有命令可复用）
$H = @('--home', $hubHome)

# Cursor adapter（二选一，路径按你机器改）
$adapter = 'C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\adapters\cursor\adapter.mjs'
# 或发布包解压目录：
# $adapter = '...\acp-hub-install-0.2.1-rc.4\extract\adapters\cursor\adapter.mjs'

# 依赖
node --version          # 建议 22.13+
Get-Command cursor-agent -ErrorAction SilentlyContinue
# 若只有 agent.cmd：
# Get-Command agent
```

安装最新 prerelease（可选）：

```powershell
cd $env:TEMP
gh release download v0.2.1-rc.4 -R RatmmmhSquishyRat/acp-hub `
  -p 'acp-hub-v0.2.1-rc.4-x86_64-pc-windows-msvc.zip*' -p SHA256SUMS
# 校验 SHA 后 Expand-Archive，把 acp-hub.exe 拷到 ~/.cargo/bin
```

---

## 2. 黄金路径（最短闭环）

```powershell
# 1) 健康检查 / 旅程
acp-hub @H doctor

# 2) 注册 cursor（auto-allow + 限制可写根到 $work）
acp-hub @H agent add cursor `
  --type stdio `
  --command node `
  --args $adapter `
  --allow-root $work

# 3) 列表（路径默认打码）
acp-hub @H agent list
acp-hub @H --reveal-paths agent list
acp-hub @H --reveal-paths agent inspect cursor

# 4) 拉 capability 缓存（doctor 说 empty 时必做）
acp-hub @H agent inspect cursor --probe

# 5) 再建一次 doctor，应看到 cache ready
acp-hub @H doctor

# 6) 开会话（cwd 必须绝对路径）
$conv = (acp-hub @H conv create cursor --cwd $work).Trim()
$conv   # 应类似 conv-xxxxxxxx

# 7) 发提示（PowerShell 多行用单行或 --stdin，见 §8）
acp-hub @H send $conv --text 'Reply with exactly one line: PING-OK'

# 8) 看投影 / 搜
acp-hub @H conv show $conv
acp-hub @H conv list
acp-hub @H search 'PING-OK' --agent cursor
```

POSIX 等价骨架：

```bash
export ACP_HUB_HOME=/tmp/acp-selftest-home
adapter=/abs/path/adapters/cursor/adapter.mjs
work=/tmp/acp-selftest-work
mkdir -p "$ACP_HUB_HOME" "$work"

acp-hub doctor
acp-hub agent add cursor --type stdio --command node --args "$adapter" --allow-root "$work"
acp-hub agent inspect cursor --probe
conv=$(acp-hub conv create cursor --cwd "$work")
acp-hub send "$conv" --text 'Reply with exactly one line: PING-OK'
acp-hub conv show "$conv"
```

---

## 3. 命令地图

| 命令 | 作用 |
|------|------|
| `doctor` | 健康 + 下一步建议 |
| `agent list/add/remove/inspect/auth/logout/sessions` | 注册与 agent 侧会话 |
| `conv create/list/show/close/delete` | Hub 会话投影 |
| `send <conv> --text \| --stdin` | 发一轮 prompt |
| `param list/set` | 会话配置（mode/model…） |
| `mode list/set` | 会话模式 agent/plan/ask |
| `cancel <conv>` | 取消进行中的 run |
| `search <query>` | 全文搜 Hub 投影 |
| `mcp` | MCP stdio（给 IDE 用，自测少用） |
| `serve` | 前台跑 daemon（一般不用） |

全局：

```text
acp-hub [--home DIR] [--reveal-paths] <command> ...
```

---

## 4. Agent

```powershell
acp-hub @H agent list
acp-hub @H agent list --json
acp-hub @H --reveal-paths agent list

acp-hub @H agent add cursor --type stdio --command node --args $adapter --allow-root $work
# 沙箱（拒权限、关 fs/terminal）：
# acp-hub @H agent add cursor --type stdio --command node --args $adapter --sandbox

acp-hub @H agent inspect cursor                 # 冷：引导 --probe
acp-hub @H agent inspect cursor --probe         # 热：填 capabilities
acp-hub @H --reveal-paths agent inspect cursor

acp-hub @H agent sessions cursor                # 博物馆列表（可能很长）
acp-hub @H agent remove cursor                  # 慎用

# 错误路径自测
acp-hub @H agent inspect nope                   # 期望 agent_not_found
```

`agent add` 常用开关：

| 开关 | 说明 |
|------|------|
| `--permission-policy auto-allow\|reject\|…` | 默认 local 常用 auto-allow |
| `--allow-read true\|false` | 默认 true |
| `--allow-write true\|false` | 默认 true |
| `--allow-terminal true\|false` | 默认 true |
| `--allow-root <path>` | 可重复；限制 callback 根目录 |
| `--sandbox` | 一键收紧 |

---

## 5. Conversation

```powershell
$conv = (acp-hub @H conv create cursor --cwd $work).Trim()
# 绑定已有 agent session（可选）：
# $conv = (acp-hub @H conv create cursor --cwd $work --agent-session-id '<sid>').Trim()

acp-hub @H conv list              # 默认 workbench（可写）
acp-hub @H conv list --all        # 含 imported 博物馆（很吵）
acp-hub @H conv list --agent cursor
acp-hub @H conv list --json

acp-hub @H conv show $conv
acp-hub @H conv show $conv --json

acp-hub @H conv close $conv       # 关远端 session，本地投影仍在
acp-hub @H conv delete $conv --local-only   # Cursor 推荐：只删本地投影
# acp-hub @H conv delete $conv              # 若 agent 无 delete capability 会失败
```

生命周期心智：

```text
cancel  = 停当前 run（会话还在）
close   = 结束远端 session，本地还能 show
delete  = 删投影；Cursor 上优先 --local-only
```

---

## 6. Send / Cancel

```powershell
# 短答
acp-hub @H send $conv --text 'Reply with exactly one line: PING-OK'

# 写文件（cwd 已是 $work 时）
acp-hub @H send $conv --text 'Create file marker.txt with exactly: HELLO. Then reply only WRITE-DONE'

# 多轮（同一 $conv）
acp-hub @H send $conv --text 'What is in marker.txt? Reply with raw contents only.'

# 带 mode / param（id 以 param list 为准）
acp-hub @H param list $conv
acp-hub @H mode list $conv
acp-hub @H mode set $conv ask
acp-hub @H send $conv --text 'Explain what cwd is in one sentence.' --mode ask
# acp-hub @H send $conv --text '...' --param 'model=grok-4.5[effort=high,fast=true]'

# 机器可读
acp-hub @H send $conv --text 'hi' --json

# stdin（适合长文，PowerShell 更稳）
@'
Create file long.txt with three lines of lorem.
Reply DONE when finished.
'@ | acp-hub @H send $conv --stdin

# 中途取消：另开一个终端
acp-hub @H cancel $conv
# 空闲时会：error: not_busy: ...
```

自测写盘时 **一定要自己读文件**：

```powershell
Get-Content (Join-Path $work 'marker.txt') -Raw
```

---

## 7. Search

```powershell
acp-hub @H search 'PING-OK'
acp-hub @H search 'PING-OK' --agent cursor
acp-hub @H search 'WRITE-DONE' --conv $conv
acp-hub @H search 'marker' --limit 20 --json
```

---

## 8. PowerShell 坑（自测高频）

1. **不要用 `$home` 当变量名**（PowerShell 只读）。用 `$hubHome`。  
2. **多行 `--text` 被拆参** → `unexpected argument 'a'`。改用：
   - 单行 `--text '...'`
   - 或 `--stdin` + 管道  
3. **`--args --agent-bin ...`** 会被 clap 当成 flag。官方 adapter 一般只需 `--args $adapter`。若必须传以 `-` 开头的 arg：查 `agent add --help`，用 `--` 分隔。  
4. **进度在 stderr**：重定向时别丢 `2>`。  
5. **看路径**：全局加 `--reveal-paths`（在子命令前）：  
   `acp-hub --home $hubHome --reveal-paths agent inspect cursor`

---

## 9. 推荐自测清单（15–30 分钟）

按顺序勾，每步看 **exit code + 人话输出 + 磁盘**（写盘时）。

| # | 动作 | 期望 |
|---|------|------|
| 1 | `doctor` 空 home | warn: 先 agent add |
| 2 | `agent add` + `list` | 有 cursor；默认路径 redacted |
| 3 | `--reveal-paths agent inspect` | 能看到 adapter 路径 |
| 4 | `inspect --probe` | `probeStatus=ok` / cache true |
| 5 | `doctor` | cache ready，next create |
| 6 | `conv create --cwd $work` | 打印 `conv-…`（可等数秒） |
| 7 | `param list` / `mode list` | JSON 有 mode/model |
| 8 | `send` 短答 token | stdout 含约定字符串 |
| 9 | `send` 写文件 + **读磁盘** | 文件内容正确 |
| 10 | 同 conv 再 `send` 追问 | 能引用上文/读文件 |
| 11 | `conv show` / `conv list` | 状态 completed；show 元数据完整（BODY 列目前可能空） |
| 12 | `search` | 能命中关键词 |
| 13 | 另开终端 `cancel` 长任务 | `stop_reason=cancelled` 或 run 被取消 |
| 14 | 空闲 `cancel` | `not_busy` |
| 15 | `agent sessions cursor` | 列表出来、不崩 |
| 16 | `conv close` | list workbench 变空；show 仍可能可见 closed |
| 17 | `conv delete` 无 flag | Cursor 可能 `unsupported_capability` |
| 18 | `conv delete --local-only` | 成功 |
| 19 | 错误 id | `*_not_found` 类清晰错误 |

**可选压力：** 长任务中再开一个 `conv list`；若出现 `Access denied`，记下并重启该 home 的 daemon（结束 `acp-hub` 进程或换新 `--home`）。

---

## 10. 排障速查

| 现象 | 处理 |
|------|------|
| `acp-hub` 找不到 | 装 CLI；检查 PATH / `~\.cargo\bin` |
| create 失败 agent missing | `agent add` / `agent list` |
| send 一直挂 | 另终端 `cancel $conv`；看 cursor-agent 是否在跑 |
| doctor 一直叫 probe | `agent inspect <id> --probe` |
| delete 失败 | 加 `--local-only`（Cursor 常见） |
| Access denied / daemon 卡死 | 结束该 home 的 `acp-hub` 进程；或换新 `--home` |
| 路径被打码 | 加 `--reveal-paths` |
| 与别的项目串状态 | 每个项目独立 `--home` |

查 daemon 相关进程（Windows）：

```powershell
Get-Process acp-hub -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process |
  Where-Object { $_.CommandLine -match 'acp-hub' } |
  Select-Object ProcessId, CommandLine
```

---

## 11. 一键复制：最小自测脚本骨架

```powershell
$ErrorActionPreference = 'Continue'
$stamp   = Get-Date -Format 'yyyyMMdd-HHmmss'
$root    = "C:\Users\15480\Desktop\AIWorkshop\tmp\acp-selftest-$stamp"
$hubHome = Join-Path $root 'home'
$work    = Join-Path $root 'work'
$adapter = 'C:\Users\15480\Desktop\AIWorkshop\repos\acp-hub\adapters\cursor\adapter.mjs'
New-Item -ItemType Directory -Force -Path $hubHome, $work | Out-Null
$H = @('--home', $hubHome)

acp-hub --version
acp-hub @H doctor
acp-hub @H agent add cursor --type stdio --command node --args $adapter --allow-root $work
acp-hub @H agent inspect cursor --probe
$conv = (acp-hub @H conv create cursor --cwd $work).Trim()
Write-Host "CONV=$conv"
acp-hub @H send $conv --text 'Reply with exactly one line: SELFTEST-OK'
acp-hub @H send $conv --text 'Create file selftest.txt with exactly: OK. Reply WRITE-DONE'
Get-Content (Join-Path $work 'selftest.txt') -ErrorAction SilentlyContinue
acp-hub @H conv show $conv
acp-hub @H search 'SELFTEST' --agent cursor
acp-hub @H conv close $conv
acp-hub @H conv delete $conv --local-only
Write-Host "DONE root=$root"
```

把 `$adapter` 改成你机器上的真实路径即可。

---

## 12. 帮助永远以本机为准

```powershell
acp-hub --help
acp-hub doctor --help
acp-hub agent --help
acp-hub agent add --help
acp-hub conv --help
acp-hub send --help
acp-hub cancel --help
acp-hub search --help
acp-hub param --help
acp-hub mode --help
```

不要发明 `conv send` / `conv search` 这类命令——顶层是 `send` / `search`。
