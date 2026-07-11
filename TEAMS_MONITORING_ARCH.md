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

**不变量 2**：agent 的对话上下文是**单一序列化资源**（single-writer）。
→ 多个 teammate 并发对同一 worker 注入 = 上下文竞争撕裂。**必须 single-writer 纠偏**：只有 lead 能下发纠偏，其他 teammate 观察 + 建议，lead 单点汇总。

**不变量 3**：teammate 只在 **turn 边界** push idle（10 词摘要）——turn 中途漂移看不到，信息太薄。
→ 增强为**语义里程碑 push**：teammate 在工具调用边界主动写结构化进度到 lead inbox。**事件驱动**（真的产生进度才写），非定时。

## 落地设计

### A. inbox 轮询 → file-watcher push（缺口 #2 的正解）
- `start_lead_inbox_poller` / `start_teammate_inbox_poller`：保留函数名与投递-先于-已读语义，把内层 `sleep` 循环换成 `DebouncedWatchReceiver.recv().await`。
- 降级：`FileWatcher::new()` 失败 → 回退到当前 sleep 轮询（保证不 regress）。
- watch 路径：lead watch 自己的 `inboxes/{lead}.json`；teammate watch `inboxes/{self}.json`。
- **首次立即读一次**（catch 已存在的未读），之后纯事件驱动。

### B. single-writer 纠偏（缺口 #3）
- `team_send` target=member 时，校验 sender：lead → 允许；非 lead teammate → 拒绝并提示"向 lead 建议，由 lead 下发"。
- 观察是只读的（file-watcher 订阅），不产生写冲突。

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
