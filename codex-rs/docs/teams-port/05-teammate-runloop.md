# 05 — Teammate Process Bootstrap + Run Loop (Phase 2)

Port target of Claude Code's **in-process teammate runner** to a Codex CLI teammate
entrypoint. Truth source: `ChinaSiro/claude-code-sourcemap`. This spec is "照搬"-grade:
exact Claude identifiers/paths, exact CLI flag/JSON shapes, the target Codex files +
module paths, Rust function signatures to add, and the wiring to `team_store.rs` /
`team.rs` / the team tools.

Scope of this component (per the orchestrator brief):
`main.tsx::run` → `extractTeammateOptions` → `getTeammateUtils().setDynamicTeamContext`,
`runInProcessTeammate`, `waitForNextPromptOrShutdown`, `initializeTeammateHooks` (Stop hook).

> Current Codex implementation note: the original headless embedded
> `CodexThread` runner plan in this file has been superseded by the interactive
> teammate process path. `codex-rs/cli/src/teammate/mod.rs` now launches the full
> Codex TUI in teammate mode; the TUI owns the teammate inbox poller and injects
> mailbox messages as turns in that teammate's own session. The old
> `cli/src/teammate/runner.rs` helpers are retained behind `#[allow(dead_code)]`
> for mailbox/run-loop parity reference, not as the current launch path. Keep
> using the Claude evidence below for mailbox priority, XML wrapping, idle, and
> shutdown semantics; do not use the headless file/module plan as current
> implementation guidance.

> Backends (tmux/iTerm), the lead's spawn-in-pane, and the lead inbox poller are **out of
> scope** here (Phases 3/4). This phase delivers the teammate-side binary that a spawned
> `codex` process runs.

---

## 1. Claude source of truth (real identifiers + paths)

| Claude symbol | File (in `ChinaSiro/claude-code-sourcemap`) | Role |
|---|---|---|
| `run(options)` | `restored-src/src/main.tsx` | entrypoint; detects teammate mode via `isAgentSwarmsEnabled()`, calls `extractTeammateOptions`, stores `storedTeammateOpts`, then `getTeammateUtils().setDynamicTeamContext(...)` |
| `extractTeammateOptions(options)` | `restored-src/src/main.tsx` | parses the teammate CLI flags, returns `TeammateOptions` |
| `setDynamicTeamContext(context)` | `restored-src/src/utils/teammate.ts` | sets module-global `dynamicTeamContext` (takes precedence over env for team join at runtime) |
| `runInProcessTeammate(config)` | `restored-src/src/utils/swarm/inProcessRunner.ts` | the run loop |
| `startInProcessTeammate(config)` | `restored-src/src/utils/swarm/inProcessRunner.ts` | fire-and-forget wrapper that calls `runInProcessTeammate` |
| `waitForNextPromptOrShutdown(...)` | `restored-src/src/utils/swarm/inProcessRunner.ts` | mailbox poll loop |
| `formatAsTeammateMessage(from, content, color?, summary?)` | `restored-src/src/utils/swarm/inProcessRunner.ts` | wraps a *peer* message as XML |
| `isShutdownRequest(text)` | swarm protocol module | parses JSON vs `ShutdownRequestMessageSchema` |
| `initializeTeammateHooks(...)` | teammate init module (registers `Stop` hook via `addFunctionHook`) | on Stop: `setMemberActive(false)` + `writeToMailbox(leadAgentName, idle notification)` |
| `createIdleNotification(from, {...})` | swarm protocol module | builds `IdleNotificationMessage` |
| `setMemberActive(teamName, agentName, isActive)` | team config module | read-modify-write `members[].isActive` in `config.json` |
| `getLastPeerDmSummary(messages)` | swarm util | extracts last peer DM summary string |
| `TEAM_LEAD_NAME` | `restored-src/src/utils/swarm/constants.ts` | **`'team-lead'`** |
| `TEAMMATE_MESSAGE_TAG` | `restored-src/src/constants/xml.js` | XML tag name used by `formatAsTeammateMessage` (e.g. `teammate-message`) |
| `POLL_INTERVAL_MS` | `inProcessRunner.ts` | **`500`** ms |

### 1.1 `extractTeammateOptions` — exact flags & returned shape

Flags parsed (Claude long-flag names):
`--agent-id`, `--agent-name`, `--team-name`, `--agent-color`, `--plan-mode-required`,
`--parent-session-id`, `--teammate-mode` (`'auto' | 'tmux' | 'in-process'`), `--agent-type`.

```ts
type TeammateOptions = {
  agentId?: string;
  agentName?: string;
  teamName?: string;
  agentColor?: string;
  planModeRequired?: boolean;
  parentSessionId?: string;
  teammateMode?: 'auto' | 'tmux' | 'in-process';
  agentType?: string;
};
```

`run` validates: if **any** teammate option is set, then `agentId`, `agentName`, `teamName`
are **all required**; only then does it call `setDynamicTeamContext`.

`setDynamicTeamContext` context object:
```ts
{ agentId: string; agentName: string; teamName: string;
  color?: string; planModeRequired: boolean; parentSessionId?: string }
```

### 1.2 `waitForNextPromptOrShutdown` — exact behavior

- Loop body sleeps `await sleep(POLL_INTERVAL_MS)` (**500 ms**) between scans.
- Reads the whole inbox via `readMailbox(agentName)`, filters unread (`read === false`).
- **Priority ordering across unread messages:**
  1. **Shutdown** — scan unread for `isShutdownRequest(text)`; if found, mark read and
     return `{ type: 'shutdown_request', ... }` **immediately** (highest priority).
  2. **`TEAM_LEAD_NAME`** — first unread whose `from === 'team-lead'`.
  3. **Other senders** — first remaining unread (FIFO by array order).
- On selecting a message: `markMessageAsReadByIndex(agentName, idx)`, then build the prompt:
  - **peer** message (`from !== 'team-lead'` and not a user message) → wrap via
    `formatAsTeammateMessage(from, content, color, summary)`.
  - **user/lead plain** message → used **verbatim** (no wrapper).
- Returns a `WaitResult`:
  ```ts
  | { type: 'shutdown_request', requestId, from, reason? }
  | { type: 'new_message', message: string /* the prompt */, from, color?, summary? }
  | { type: 'aborted' }
  ```

### 1.3 `formatAsTeammateMessage` — exact template

```ts
function formatAsTeammateMessage(from, content, color?, summary?): string {
  const colorAttr   = color   ? ` color="${color}"`     : ''
  const summaryAttr = summary ? ` summary="${summary}"` : ''
  return `<${TEAMMATE_MESSAGE_TAG} teammate_id="${from}"${colorAttr}${summaryAttr}>\n${content}\n</${TEAMMATE_MESSAGE_TAG}>`
}
```

### 1.4 `runInProcessTeammate` — exact control flow

- **First prompt source**: `currentPrompt` is initialized from `config.prompt` (the spawn
  `--prompt` / initial message), **wrapped via `formatAsTeammateMessage`** (treated as a
  message from the lead). It does **not** start by polling — it runs the first turn, then polls.
- `while (!abortController.signal.aborted && !shouldExit)`:
  - run a turn with `currentPrompt` (Claude: `runAgent(...)`, accumulating
    `iterationMessages` / `allMessages`; compaction when token count exceeds threshold).
  - on turn end → send **idle notification** to the lead.
  - `const waitResult = await waitForNextPromptOrShutdown(...)`:
    - `'shutdown_request'` → pass shutdown text to the model; set `currentPrompt` to the
      shutdown message (model decides; loop continues so the model can ack/finish).
    - `'new_message'` → `currentPrompt = waitResult.message`.
    - `'aborted'` → `shouldExit = true` (loop terminates).

### 1.5 Stop hook + idle notification — exact shapes

`initializeTeammateHooks` registers a `Stop` hook (`addFunctionHook(... 'Stop', '', async ...)`):
```ts
void setMemberActive(teamName, agentName, false)
const notification = createIdleNotification(agentName, {
  idleReason: 'available',
  summary: getLastPeerDmSummary(messages),
})
await writeToMailbox(leadAgentName, {
  from: agentName,
  text: jsonStringify(notification),
  timestamp: new Date().toISOString(),
  color: getTeammateColor(),
})
return true // don't block Stop
// hook timeout: 10000 ms
```

`createIdleNotification` output (`IdleNotificationMessage`):
```ts
type IdleNotificationMessage = {
  type: 'idle_notification'
  from: string
  timestamp: string
  idleReason?: 'available' | 'interrupted' | 'failed'
  summary?: string
  completedTaskId?: string
  completedStatus?: 'resolved' | 'blocked' | 'failed'
  failureReason?: string
}
```

`isShutdownRequest` validates against `ShutdownRequestMessageSchema`:
```ts
z.object({
  type: z.literal('shutdown_request'),
  requestId: z.string(),
  from: z.string(),
  reason: z.string().optional(),
  timestamp: z.string(),
})
```

> NOTE — divergence from `team_store.rs` as shipped: Claude's idle notification has
> `type: "idle_notification"`. The Phase-1 spec text in `TEAMS_CLAUDE_PORT_SPEC.md` wrote
> `kind:"idle"`. **Use `type:"idle_notification"`** to stay faithful (see §4.4).

---

## 2. Codex target files & module paths

All new code lives in `codex-rs/cli`. Read-only elsewhere.

| New / edited file | Purpose |
|---|---|
| `codex-rs/cli/src/main.rs` *(edit)* | add a hidden `Teammate(TeammateCommand)` subcommand + dispatch arm |
| `codex-rs/cli/src/teammate_cmd.rs` *(NEW)* | `TeammateCommand` clap struct (the flags) + `run_main(...)` async entry |
| `codex-rs/cli/src/teammate/runner.rs` *(NEW, `mod` under `teammate_cmd`)* | the Rust run loop (port of `runInProcessTeammate` + `waitForNextPromptOrShutdown`) |
| `codex-rs/cli/src/teammate/message_fmt.rs` *(NEW)* | `format_as_teammate_message`, `is_shutdown_request`, idle/shutdown serde types |
| `codex-rs/cli/src/lib.rs` *(edit)* | `pub mod teammate_cmd;` (or `mod` if not re-exported) |

Historical rationale for `cli` (not `tui`/`exec`): the first plan modeled a
teammate as a **headless embedded session** driven by the inbox, not an
interactive TUI and not the one-shot `exec` flow. Current code kept the hidden
`codex teammate` CLI entrypoint but changed its body: it constructs a normal
interactive TUI `Cli` with teammate identity fields and lets the TUI-side
poller consume the mailbox.

Module wiring in `teammate_cmd.rs`:
```rust
mod runner;       // pub(crate) async fn run_teammate_loop(...)
mod message_fmt;  // format_as_teammate_message, is_shutdown_request, IdleNotification, ShutdownRequest
```

---

## 3. Codex embedded-session APIs (exact — to start a headless teammate session)

Confirmed from `codex-rs/core/src/thread_manager.rs` and `codex-rs/core/src/codex_thread.rs`:

- **Manager**: `codex_core::ThreadManager` (alias `ConversationManager`; see
  `core/src/lib.rs:124 pub type ConversationManager = ThreadManager;`).
  - `ThreadManager::new(config: &Config, auth_manager: Arc<AuthManager>, session_source: SessionSource, environment_manager: Arc<EnvironmentManager>, extensions: Arc<ExtensionRegistry<Config>>, analytics_events_client: Option<AnalyticsEventsClient>, thread_store: Arc<dyn ThreadStore>, state_db: Option<StateDbHandle>, installation_id: String, attestation_provider: Option<Arc<dyn AttestationProvider>>) -> Self`
  - helper `thread_store_from_config(config: &Config, state_db: Option<StateDbHandle>) -> Arc<dyn ThreadStore>`
  - `pub async fn start_thread(&self, config: Config) -> CodexResult<NewThread>`
  - or `start_thread_with_options(StartThreadOptions { config, initial_history: InitialHistory::New, session_source: Some(SessionSource::...), thread_source: None, dynamic_tools: vec![], metrics_service_name: None, parent_trace: None, environments })`
- **`NewThread`** (`thread_manager.rs:111`): `{ thread_id: ThreadId, thread: Arc<CodexThread>, session_configured: SessionConfiguredEvent }`.
- **Drive the session** (`codex_thread.rs`):
  - submit a turn: `CodexThread::submit(&self, op: Op) -> CodexResult<String>` (returns submission id).
  - read events: `CodexThread::next_event(&self) -> CodexResult<Event>`.
  - interrupt: `Op::Interrupt`; teardown: `CodexThread::shutdown_and_wait(&self) -> CodexResult<()>`.
- **Turn input** (`codex_protocol::protocol::Op`, `protocol.rs:523`):
  ```rust
  Op::UserInput { items: Vec<UserInput> }
  // From<Vec<UserInput>> for Op exists (protocol.rs:673) → vec![..].into()
  ```
  `UserInput::Text { text: String, text_elements: Vec<TextElement> }` (`user_input.rs:15`).
- **Turn completion signal** (`codex_protocol::protocol::EventMsg`, `protocol.rs:1160`):
  - `EventMsg::TurnComplete(TurnCompleteEvent { turn_id, last_agent_message: Option<String>, .. })`
    (`protocol.rs:1207` / `1857`) — **this is the per-turn boundary** the run loop waits on.
  - `EventMsg::AgentMessage(AgentMessageEvent { message, .. })` (`protocol.rs:1214`/`2170`) —
    accumulate to capture the teammate's reply text (feeds idle-notification `summary` and any
    cross-process reply written back to the lead).
  - `EventMsg::Error(..)` / `EventMsg::TurnAborted(..)` — treat as turn end (abort path).
  - `EventMsg::SessionConfigured(..)` is the first event after `start_thread`.

> `Config` is loaded via `codex_core::config` (`ConfigOverrides`, `Config` builder; see
> `core/src/config/mod.rs`). The teammate inherits provider/auth/model the same way the
> interactive path does in `cli/src/main.rs` (`AuthManager`, `EnvironmentManager`,
> `ExtensionRegistry`, model from `--model` / config). Reuse the same construction the
> `None` (interactive) arm performs before handing to the TUI, but keep the
> `ThreadManager`/`CodexThread` instead of launching the TUI.

---

## 4. Rust to add

### 4.0 `teammate_cmd.rs` — clap struct + entry

```rust
use clap::Parser;
use codex_utils_cli::CliConfigOverrides;

/// Hidden: run this codex process as a team member driven by its on-disk inbox.
#[derive(Debug, Parser)]
pub struct TeammateCommand {
    /// "{name}@{team}" — internal identity (team_store::agent_id).
    #[arg(long = "agent-id")]
    pub agent_id: String,
    /// Display name used for messaging/tasks/inbox file.
    #[arg(long = "agent-name")]
    pub agent_name: String,
    #[arg(long = "team-name")]
    pub team_name: String,
    #[arg(long = "agent-color")]
    pub agent_color: Option<String>,
    #[arg(long = "parent-session-id")]
    pub parent_session_id: Option<String>,
    #[arg(long = "agent-type")]
    pub agent_type: Option<String>,
    #[arg(long = "plan-mode-required", default_value_t = false)]
    pub plan_mode_required: bool,
    /// Initial prompt (Claude `config.prompt`); first turn input.
    #[arg(long = "prompt")]
    pub prompt: Option<String>,
    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,
}

pub async fn run_main(
    cmd: TeammateCommand,
    arg0_paths: codex_arg0::Arg0DispatchPaths,
) -> anyhow::Result<()> { /* build Config + ThreadManager, then runner::run_teammate_loop */ }
```

`main.rs` wiring (mirrors existing arms, e.g. `Some(Subcommand::Exec(..))`):
- Add to `enum Subcommand`: `#[clap(hide = true)] Teammate(TeammateCommand),`
- Add dispatch arm in the `match subcommand { ... }` (near line 922):
  ```rust
  Some(Subcommand::Teammate(mut teammate_cli)) => {
      prepend_config_flags(&mut teammate_cli.config_overrides, root_config_overrides.clone());
      crate::teammate_cmd::run_main(teammate_cli, arg0_paths.clone()).await?;
  }
  ```

> The "teammate mode = a flag set on the normal entry" form from `TEAMS_CLAUDE_PORT_SPEC.md`
> P2 is equivalent; a hidden subcommand is the cleaner clap shape and keeps the interactive
> `None` arm untouched. Either works — pick subcommand.

### 4.1 Run loop (`teammate/runner.rs`) — port of `runInProcessTeammate`

```rust
use std::sync::Arc;
use std::path::Path;
use std::time::Duration;
use codex_core::{ThreadManager, codex_thread::CodexThread};
use codex_core::team_store::{self, TeammateMessage, TEAM_LEAD_NAME};
use codex_protocol::protocol::{Op, EventMsg};
use codex_protocol::user_input::UserInput;

pub(crate) struct TeammateRuntime {
    pub teams_root: std::path::PathBuf, // $CODEX_HOME
    pub team: String,
    pub agent_name: String,
    pub color: Option<String>,
    pub lead_name: String,              // = TEAM_LEAD_NAME
}

/// Port of runInProcessTeammate. Owns one CodexThread; first prompt = `initial_prompt`
/// (wrapped as a lead message), thereafter driven by the inbox.
pub(crate) async fn run_teammate_loop(
    rt: TeammateRuntime,
    thread: Arc<CodexThread>,
    initial_prompt: Option<String>,
) -> anyhow::Result<()> {
    // First currentPrompt: wrap initial_prompt as a teammate(lead) message (§1.4).
    let mut current_prompt: Option<String> = initial_prompt
        .map(|p| crate::teammate_cmd::message_fmt::format_as_teammate_message(
            &rt.lead_name, &p, rt.color.as_deref(), None));

    loop {
        let Some(prompt) = current_prompt.take() else { break };

        // Run one turn and capture last agent message.
        let last_agent_message = run_one_turn(&thread, prompt).await?;

        // Idle notification to the lead on each turn boundary (§4.4).
        let _ = write_idle_notification(&rt, last_agent_message.as_deref());

        // Wait for next prompt or shutdown (§4.2).
        match wait_for_next_prompt_or_shutdown(&rt).await? {
            WaitResult::Shutdown { reason } => {
                // Feed shutdown text to the model so it can ack; then exit after that turn.
                current_prompt = Some(shutdown_prompt_text(reason));
                run_one_turn(&thread, current_prompt.take().unwrap()).await.ok();
                break;
            }
            WaitResult::NewMessage { prompt } => current_prompt = Some(prompt),
            WaitResult::Aborted => break,
        }
    }

    // Stop-hook analog (§4.4): mark inactive + final idle notification, then teardown.
    mark_inactive(&rt);
    let _ = write_idle_notification(&rt, None);
    thread.shutdown_and_wait().await.ok();
    Ok(())
}

/// Submit Op::UserInput and pump events until EventMsg::TurnComplete (or Error/TurnAborted).
/// Returns the last EventMsg::AgentMessage text seen (for idle summary / lead reply).
async fn run_one_turn(thread: &Arc<CodexThread>, prompt: String) -> anyhow::Result<Option<String>> {
    let items = vec![UserInput::Text { text: prompt, text_elements: Vec::new() }];
    thread.submit(Op::UserInput { items }).await?;
    let mut last_agent_message = None;
    loop {
        let ev = thread.next_event().await?;
        match ev.msg {
            EventMsg::AgentMessage(a) => last_agent_message = Some(a.message),
            EventMsg::TurnComplete(t) => {
                if t.last_agent_message.is_some() { last_agent_message = t.last_agent_message; }
                break;
            }
            EventMsg::TurnAborted(_) | EventMsg::Error(_) => break,
            _ => {}
        }
    }
    Ok(last_agent_message)
}
```

### 4.2 Mailbox poll (`runner.rs`) — port of `waitForNextPromptOrShutdown`

```rust
const POLL_INTERVAL_MS: u64 = 500;

pub(crate) enum WaitResult {
    Shutdown { reason: Option<String> },
    NewMessage { prompt: String },
    Aborted, // reserved for an abort signal (e.g. ctrl-c / parent kill)
}

async fn wait_for_next_prompt_or_shutdown(rt: &TeammateRuntime) -> anyhow::Result<WaitResult> {
    loop {
        // read_mailbox returns Vec<TeammateMessage> in array order (team_store).
        let msgs = team_store::read_mailbox(&rt.teams_root, &rt.team, &rt.agent_name)?;
        let unread: Vec<(usize, &TeammateMessage)> =
            msgs.iter().enumerate().filter(|(_, m)| !m.read).collect();

        // (1) shutdown — highest priority.
        if let Some((idx, m)) = unread.iter()
            .find(|(_, m)| message_fmt::is_shutdown_request(&m.text))
        {
            team_store::mark_message_read_by_index(&rt.teams_root, &rt.team, &rt.agent_name, *idx)?;
            let reason = message_fmt::parse_shutdown(&m.text).and_then(|s| s.reason);
            return Ok(WaitResult::Shutdown { reason });
        }
        // (2) TEAM_LEAD_NAME, then (3) any other — FIFO.
        let pick = unread.iter().find(|(_, m)| m.from == TEAM_LEAD_NAME)
            .or_else(|| unread.first());
        if let Some((idx, m)) = pick {
            team_store::mark_message_read_by_index(&rt.teams_root, &rt.team, &rt.agent_name, *idx)?;
            // peer (not lead, not idle/shutdown json) → wrap; lead/user → verbatim.
            let prompt = if m.from == TEAM_LEAD_NAME {
                m.text.clone()
            } else {
                message_fmt::format_as_teammate_message(
                    &m.from, &m.text, m.color.as_deref(), m.summary.as_deref())
            };
            return Ok(WaitResult::NewMessage { prompt });
        }
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}
```

> Claude treats lead/user messages as *plain* and peer messages as *wrapped*. Codex's mailbox
> distinguishes sender only by `from`; treat `from == TEAM_LEAD_NAME` (the lead, which relays
> the human) as plain, everything else as a peer to wrap. This matches §1.2.

### 4.3 `message_fmt.rs` — formatting + shutdown detection

```rust
pub(crate) const TEAMMATE_MESSAGE_TAG: &str = "teammate-message"; // = Claude TEAMMATE_MESSAGE_TAG

pub(crate) fn format_as_teammate_message(
    from: &str, content: &str, color: Option<&str>, summary: Option<&str>,
) -> String {
    let color_attr = color.map(|c| format!(" color=\"{c}\"")).unwrap_or_default();
    let summary_attr = summary.map(|s| format!(" summary=\"{s}\"")).unwrap_or_default();
    format!("<{tag} teammate_id=\"{from}\"{color_attr}{summary_attr}>\n{content}\n</{tag}>",
        tag = TEAMMATE_MESSAGE_TAG)
}

#[derive(serde::Deserialize)]
pub(crate) struct ShutdownRequest {
    #[serde(rename = "type")] pub kind: String, // must == "shutdown_request"
    pub request_id: String,                      // serde rename "requestId"
    pub from: String,
    pub reason: Option<String>,
    pub timestamp: String,
}

pub(crate) fn parse_shutdown(text: &str) -> Option<ShutdownRequest> {
    serde_json::from_str::<ShutdownRequest>(text).ok()
        .filter(|s| s.kind == "shutdown_request")
}
pub(crate) fn is_shutdown_request(text: &str) -> bool { parse_shutdown(text).is_some() }

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdleNotification {
    #[serde(rename = "type")] pub kind: &'static str, // "idle_notification"
    pub from: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub idle_reason: Option<String>, // "available"
    #[serde(skip_serializing_if = "Option::is_none")] pub summary: Option<String>,
}
```

(Use `#[serde(rename_all = "camelCase")]` on `ShutdownRequest` too so `requestId` matches.)

### 4.4 Stop / idle wiring — port of `initializeTeammateHooks`

Codex has no per-session `Stop` hook equivalent for an embedded thread; instead emit the
idle notification at **two points** that together reproduce Claude's behavior:
1. **after every turn** (`run_one_turn` returns) — Claude sends idle on each turn boundary;
2. **on loop exit** (shutdown/abort) — final notification + mark inactive.

```rust
fn write_idle_notification(rt: &TeammateRuntime, last_agent_message: Option<&str>) -> std::io::Result<()> {
    let note = message_fmt::IdleNotification {
        kind: "idle_notification",
        from: rt.agent_name.clone(),
        timestamp: team_store::now_timestamp(),
        idle_reason: Some("available".to_string()),
        summary: last_agent_message.map(summarize_5_to_10_words), // getLastPeerDmSummary analog
    };
    let text = serde_json::to_string(&note).map_err(std::io::Error::other)?;
    team_store::write_to_mailbox(&rt.teams_root, &rt.team, &rt.lead_name, TeammateMessage {
        from: rt.agent_name.clone(),
        text,
        timestamp: team_store::now_timestamp(),
        read: false,
        color: rt.color.clone(),
        summary: None,
    })
}

fn mark_inactive(rt: &TeammateRuntime) {
    // setMemberActive(team, agent, false): read-modify-write members[].is_active.
    let _ = team_store::update_config(&rt.teams_root, &rt.team, |cfg| {
        if let Some(m) = cfg.members.iter_mut().find(|m| m.name == rt.agent_name) {
            m.is_active = Some(false);
        }
    });
}
```

- `now_timestamp()` already exists in `team_store.rs` (NOTE: it currently emits seconds-since-
  epoch, not ISO-8601; faithful Claude is ISO-8601 — acceptable for Phase 2, RFC-3339 deferred
  per the module's own comment).
- `write_to_mailbox`, `update_config`, `read_mailbox`, `mark_message_read_by_index`,
  `TEAM_LEAD_NAME`, `TeammateMessage`, `agent_id`, `inbox_path` are all already public in
  `core/src/team_store.rs`.

---

## 5. Wiring summary (team_store / team.rs / team tools)

- **team_store.rs (consumed as-is):** the teammate process is a pure *client* of the on-disk
  store. It only calls `read_mailbox` / `mark_message_read_by_index` (its own inbox) and
  `write_to_mailbox(lead)` + `update_config` (idle + inactive). No edits to `team_store.rs`
  required for this phase **except** optionally aligning the idle `type` value (§1 NOTE).
- **team.rs / `team_spawn_member` (Phase 3, not here):** today `team.rs` spawns an in-process
  thread via `registry.spawn_member`. In Phase 3 the lead instead (a) registers the member in
  `config.json` and (b) launches a `codex teammate --agent-id ... --agent-name ... --team-name
  ... [--agent-color] [--parent-session-id] [--agent-type] [--plan-mode-required] [--prompt ...]`
  process (in a tmux/iTerm pane). The flags in §4.0 are exactly the spawn contract Phase 3 must
  emit, and exactly what Claude's `spawnMultiAgent.ts` / `PaneBackendExecutor.ts` construct.
- **team_send (Phase 4):** `team_send` → `write_to_mailbox(team, target_agent, msg)`. The
  teammate's `wait_for_next_prompt_or_shutdown` is the *receiver* of those writes; the lead's
  inbox poller (Phase 4) is the receiver of this loop's idle/reply writes.
- **Identity:** `--agent-id` = `team_store::agent_id(name, team)` = `"{name}@{team}"`;
  `--agent-name` is the messaging/inbox key (`inbox_path(.., agent_name)`); `lead_name =
  TEAM_LEAD_NAME`.

---

## 6. Open / blocking unknowns

1. **`TEAMMATE_MESSAGE_TAG` exact string** — deep-wiki confirmed it lives in
   `restored-src/src/constants/xml.js` and is used as `<TAG teammate_id="..">…</TAG>`, but the
   literal value was not quoted. Spec assumes `teammate-message`; **verify the raw constant**
   before locking (low risk; cosmetic XML tag only).
2. **Compaction**: Claude compacts `allMessages` past a token threshold inside the loop. Codex
   manages context inside `CodexThread`/the turn engine, so the per-message accumulation is not
   needed — the embedded session already handles history/compaction. (Confidence: high that we
   can omit Claude's manual compaction.)
3. **Abort/shutdown precision**: Claude's `'aborted'` comes from `abortController`. In Codex the
   equivalent triggers are parent process kill / ctrl-c / `Op::Interrupt`; `WaitResult::Aborted`
   is wired to a `CancellationToken`/signal handler at the `run_main` level (not detailed here).
4. **First-turn semantics on shutdown ack**: Claude "passes the shutdown request to the model";
   §4.1 runs one more turn with the shutdown text then breaks. If a no-ack fast-exit is desired
   instead, drop that extra `run_one_turn`. (Confidence: moderate; choose during Phase 4 review.)
