# UX 文案反馈：`serve` 与 `--home`（2026-07-26）

**范围：** CLI 主说明 / `serve` / 全局 `--home` 的**产品叙事**，非协议实现。  
**版本参考：** 操作时本机 `0.2.1-rc.6`（文案问题与具体 rc 弱绑定）。

---

## 需求（操作者）

1. **主路径不出现「home」概念**  
   - `acp-hub --help` quick start、doctor、主命令表说明：只谈典型动线（add / create / send / wait / show / cancel / delete）。  
   - **不要**用「for a home directory」等表述逼用户先理解 home。

2. **`--home` 沉到高级层**  
   - 能力可保留（隔离、测试、多实例）。  
   - 归入 advanced / 调试；**单独**完整机制说明（默认路径、单例 daemon、隔离语义、与各命令关系）。  
   - 其他子命令与 `serve` **共用**这一套说明，不要每个入口各自讲 home。

3. **`serve` 文案先讲用途，再讲可选细节**  
   - 必须让只读 CLI 的人明白：  
     - 日常 **不必** 先 `serve`（其它命令会 on-demand 拉起 daemon）；  
     - `serve` = **前台**跑单例 daemon，主要用于调试 / 少数长驻场景。  
   - 不得让人以为「必须先起服务」或「必须先准备一个单独 home」。

4. **可从 CLI 自解释**  
   - 仅 `acp-hub --help` / `serve --help` 应足够建立上述心智；不应依赖 README 才知道「usually unnecessary」。

---

## 现状问题（简）

| 点 | 问题 |
|----|------|
| `serve` 一行说明 | 只写 *Run the singleton Hub daemon for a home directory* → 半懂 |
| 主 help | `serve` 靠前、无「通常不需要」 |
| doctor / quick start | 不提自动 daemon，也不澄清 serve |
| home | 实现正确（默认 `~/.acp-hub`），主叙事过重、缺专节 |

---

## 期望改动方向（给实现，不展开设计）

- 主 help / doctor：不教 home；`serve` 标成可选或附一句 usually unnecessary。  
- `serve --help`：前台 daemon + 日常可省略 + 链到 home 机制（若保留）。  
- `--home`：advanced + 独立机制说明一文/一 help 主题。

---

## 原则对齐

与操作者原则一致：**默认 = 典型动线；复杂概念与参数藏深处。**  
home / 前台 serve 属于机制与专家能力，不是主产品故事。
