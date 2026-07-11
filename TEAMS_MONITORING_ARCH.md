# Codex Teams — 实时监控 & a2a 架构决策（第一性原理）

Truth source: 本仓库现有 crate（`codex-file-watcher`）+ Claude Code source-map 的 mailbox 语义。

## 目标（用户原话拆解）

1. 一个 agent 干活时，另一个能**实时监控**它、发现**路径漂移**时**调方向 / peer 新 prompt** —— 把"人看 agent"变成 a2a。
2. **不能**让 teammate **循环查看** agent 历史输出——太占资源。
3. 要**最优雅、最省资源**的方式。cron 不一定对，要用第一性原理找底层不变量。

## 第一性原理：监控的本质

> 监控 = "状态变化时被通知"，**不是** "不断主动查询状态"。

这是 **push（事件驱动）** vs **pull（轮询）** 的根本对立：

| | 成本模型 | 空转 | 延迟 |
|---|---|---|---|
| 轮询 pull | 频率 × 每次开销，**大部分查询是空的** | 有 | 最坏 = 一个周期 |
| 定时 cron | 同轮询，且**错过周期内变化** | 有 | 固定周期 |
| 事件驱动 push | = **实际变化次数**，趋近理论下限 | 无 | 趋近 0 |

**结论：cron 是错的**（high confidence）。它是定时轮询的特例，违背不变量：没变化时也唤醒、错过 tick 间变化、固定开销。source-map 里的 cron 工具是给**用户定时任务**用的（"每天巡检"），不是 agent 间实时监控。迁移它是**另一个独立功能**，与本问题无关。

## 底层不变量 → 最优方案

**不变量 1**：inbox 是磁盘文件 `teams/{team}/inboxes/{agent}.json`，路径**确定**。
→ 可以用 **OS 内核级文件监听**（macOS FSEvents / Linux inotify）直接 push。

**关键发现**：本仓库**已有** `codex-file-watcher` crate（`notify 8.2.0`），提供：
- `FileWatcher::new()`（失败降级 `noop()`）
- `add_subscriber()` → `Receiver.recv().await`（async push）
- `FileWatcherEvent { paths }`（去重、排序、**coalesced**）
- `DebouncedWatchReceiver` / `ThrottledWatchReceiver`（内建去抖/节流，防事件风暴）
- 已在 `app-server/src/skills_watcher.rs` 用于 skills/fs 监听 —— **有成熟范式**。

→ **最优方案 = 用 file-watcher 把 inbox 轮询换成事件驱动 push**。lead/teammate 不再 `sleep(1s/500ms)` 空转，而是 `rx.recv().await` 阻塞等 OS 通知，来了才读 inbox。零空轮询，延迟趋近 0，且 debounce 兜底防高频写。

**不变量 2（修正）**：agent 的对话上下文是**单一序列化资源**，但 inbox 的 FIFO 已经**物理串行化**了并发写入——不会撕裂单个 turn。真正的问题**不是"禁止多写者"**（那会阉割 reviewer→主agent 直接纠偏这个核心能力），而是**多个纠偏源并发时的语义协调**。

→ **正确模型 = 多源仲裁（multi-source arbitration），不是 single-writer 互斥**：
- teammate **可以**当审查员：观察主 agent → 汇报 → **直接 peer 纠偏**（保留，这是核心能力）。
- teammate 间 **可以** a2a 讨论/脑暴（逆向/渗透的启发性思考）。
- 冲突协调靠**优先级仲裁**，不靠禁止写入。地基已存在：`select_next_inbox` 已实现 `shutdown > lead > FIFO` 三级优先级。
- **人类 peer + teammate peer 的协调**：人类（经 lead）的纠偏优先级最高，teammate 纠偏次之，FIFO 兜底。多个 teammate 同时纠偏 → 按到达顺序 FIFO 串行注入（inbox 已保证），worker 逐条处理，不撕裂上下文。
- 增强方向：给 mailbox 消息加 `kind`（progress/report/correction/discussion）+ `source_role`（human/lead/reviewer/peer），让 `select_next_inbox` 能按语义优先级仲裁，而非纯 FIFO。

**不变量 3**：teammate 只在 **turn 边界** push idle（10 词摘要）——turn 中途漂移看不到，信息太薄。
→ 增强为**语义里程碑 push**：teammate 在工具调用边界主动写结构化进度到 lead inbox。**事件驱动**（真的产生进度才写），非定时。

## 落地设计

### A. inbox 轮询 → file-watcher push（缺口 #2 的正解）
- `start_lead_inbox_poller` / `start_teammate_inbox_poller`：保留函数名与投递-先于-已读语义，把内层 `sleep` 循环换成 `DebouncedWatchReceiver.recv().await`。
- 降级：`FileWatcher::new()` 失败 → 回退到当前 sleep 轮询（保证不 regress）。
- watch 路径：lead watch 自己的 `inboxes/{lead}.json`；teammate watch `inboxes/{self}.json`。
- **首次立即读一次**（catch 已存在的未读），之后纯事件驱动。

### B. 多源仲裁（缺口 #3，修正后）
**不是** single-writer 互斥。teammate 保留观察 + 汇报 + 直接 peer 纠偏的完整能力（reviewer 场景的核心）。协调靠优先级仲裁：
- mailbox 消息扩展 `kind`（progress/report/correction/discussion）+ `source_role`（human/lead/reviewer/peer）。
- `select_next_inbox` 从 `shutdown > lead > FIFO` 扩展为 `shutdown > human-correction > lead > reviewer-correction > peer-discussion > FIFO`。
- 多个 teammate 同时纠偏：inbox FIFO 已物理串行化，worker 逐条处理不撕裂。仲裁决定"先处理谁"，不丢弃任何一方。
- 人类 peer（经 lead）与 teammate peer 并存：人类优先级最高，但 teammate 纠偏不被禁止，只是排在人类之后。

### C. 语义里程碑 push（缺口 #2 增强）
- teammate runner 在 `run_one_turn` 的工具边界，可选 push 结构化进度事件到 lead inbox（复用 idle 通道，加 `kind: progress`）。
- lead poller 收到后由 **lead 模型**判断是否漂移 → 漂移则 `team_send` 纠偏。"像人一样看着调方向"。

### 资源账
- 稳态开销：**0 空轮询**。只有真的有 inbox 写入时，OS 才唤醒 recv。
- 对比：cron = 每 tick 唤醒 + 读盘；循环读历史 = 每次读全量 transcript。file-watcher 省几个数量级。

## 验证（必须 live，不走 mock 桩）
- 每个缺口改完 bump `TEAMS_VERSION`，重编译，`codex --version` 确认新版本。
- file-watcher：live smoke 证明 lead 在 teammate 写 inbox 后 **无 1s 轮询延迟**收到（观测注入延迟 < 200ms）。
- single-writer：live 构造两 teammate，非 lead 发纠偏被拒。
- TUI：tmux send-keys 注入 @提及/切换/send，真实交互验证。
