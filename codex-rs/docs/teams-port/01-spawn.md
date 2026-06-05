# Teams port spec 01 — TEAMMATE SPAWN (Claude `spawnMultiAgent.ts` → Codex)

Truth source: `ChinaSiro/claude-code-sourcemap` (de-minified Claude Code `2.1.88`),
verified against mirror `leaf-kit/claude-analysis`. All Claude identifiers, command
strings and JSON shapes below are quoted from real code, not guessed.

Goal: change `team_spawn_member` from in-process thread spawn (`registry.spawn_member`)
to launching a **separate `codex` process in a tmux pane**, registering the member in
`team_store` `config.json` with `tmuxPaneId` + `backendType`, mirroring Claude's
`handleSpawnSplitPane`.

---

## 1. Claude source — exact identifiers & paths

| Concern | Claude file | Symbol |
|---|---|---|
| Spawn entry | `restored-src/src/tools/shared/spawnMultiAgent.ts` | `spawnTeammate()` → `handleSpawn()` |
| Split-pane spawn | same | `handleSpawnSplitPane()` |
| Separate-window spawn | same | `handleSpawnSeparateWindow()` |
| In-process spawn (fallback) | same | `handleSpawnInProcess()` |
| Binary resolver | same | `getTeammateCommand()` |
| Inherited CLI flags | same | `buildInheritedCliFlags()` |
| Inherited env vars | `restored-src/src/utils/swarm/spawnUtils.ts` | `buildInheritedEnvVars()`, `TEAMMATE_ENV_VARS` |
| Constants | `restored-src/src/utils/swarm/constants.ts` | `TEAM_LEAD_NAME='team-lead'`, `SWARM_SESSION_NAME='claude-swarm'`, `SWARM_VIEW_WINDOW_NAME='swarm-view'`, `TMUX_COMMAND='tmux'`, `HIDDEN_SESSION_NAME='claude-hidden'`, `TEAMMATE_COMMAND_ENV_VAR='CLAUDE_CODE_TEAMMATE_COMMAND'`, `TEAMMATE_COLOR_ENV_VAR='CLAUDE_CODE_AGENT_COLOR'`, `PLAN_MODE_REQUIRED_ENV_VAR='CLAUDE_CODE_PLAN_MODE_REQUIRED'`, `getSwarmSocketName()='claude-swarm-${pid}'` |
| Agent ID | `restored-src/src/utils/agentId.ts` | `formatAgentId(name, team)` → `` `${agentName}@${teamName}` ``, `parseAgentId()` |
| Color palette | `restored-src/src/tools/AgentTool/agentColorManager.ts` | `AGENT_COLORS = ['red','blue','green','yellow','purple','orange','pink','cyan']` |
| Color assignment | `restored-src/src/utils/swarm/teammateLayoutManager.ts` | `assignTeammateColor(teammateId)` (round-robin over `AGENT_COLORS`) |
| Name sanitize | `restored-src/src/utils/swarm/teamHelpers.ts` | `sanitizeAgentName(n)=n.replace(/@/g,'-')`, `sanitizeName(n)=n.replace(/[^a-zA-Z0-9]/g,'-').toLowerCase()` |
| tmux backend | `restored-src/src/utils/swarm/backends/TmuxBackend.ts` | `createTeammatePaneWithLeader`, `createTeammatePaneExternal`, `sendCommandToPane`, `killPane`, `setPaneTitle`, `setPaneBorderColor`, `enablePaneBorderStatus` |
| Pane orchestrator | `restored-src/src/utils/swarm/backends/PaneBackendExecutor.ts` | `spawn()` (delegates split → spawnCommand → send-keys) |
| shell quote | `restored-src/src/utils/bash/shellQuote.ts` | `quote([...])` |

---

## 2. The spawn command string (EXACT)

`handleSpawnSplitPane` and `handleSpawnSeparateWindow` build the identical template:

```ts
const spawnCommand = `cd ${quote([workingDir])} && env ${envStr} ${quote([binaryPath])} ${teammateArgs}${flagsStr}`
```

`binaryPath = getTeammateCommand()`:
```ts
function getTeammateCommand(): string {
  if (process.env[TEAMMATE_COMMAND_ENV_VAR]) return process.env[TEAMMATE_COMMAND_ENV_VAR]
  return isInBundledMode() ? process.execPath : process.argv[1]!
}
```

`teammateArgs` (joined with `' '`, empties filtered):
```ts
const teammateArgs = [
  `--agent-id ${quote([teammateId])}`,        // teammateId = formatAgentId(sanitizedName, teamName) = "name@team"
  `--agent-name ${quote([sanitizedName])}`,
  `--team-name ${quote([teamName])}`,
  `--agent-color ${quote([teammateColor])}`,
  `--parent-session-id ${quote([getSessionId()])}`,
  plan_mode_required ? '--plan-mode-required' : '',
  agent_type ? `--agent-type ${quote([agent_type])}` : '',
].filter(Boolean).join(' ')
```

`flagsStr = inheritedFlags ? ' '+inheritedFlags : ''`, from `buildInheritedCliFlags()`:
```ts
// permission mode (skipped when plan_mode_required):
//   bypassPermissions → '--dangerously-skip-permissions'
//   acceptEdits       → '--permission-mode acceptEdits'
//   auto              → '--permission-mode auto'
// '--model <quoted>'        (getMainLoopModelOverride)
// '--settings <quoted>'     (getFlagSettingsPath)
// '--plugin-dir <quoted>'   (per inline plugin)
// '--teammate-mode <mode>'  (always: tmux|in-process|auto)
// '--chrome' | '--no-chrome' (if explicitly set)
```
After building inheritedFlags, if a teammate-specific `model` exists Claude strips any
inherited `--model`/value pair then appends `--model ${quote([model])}`.

`envStr = buildInheritedEnvVars()`:
```ts
export function buildInheritedEnvVars(): string {
  const envVars = ['CLAUDECODE=1', 'CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1']
  for (const key of TEAMMATE_ENV_VARS) {
    const value = process.env[key]
    if (value !== undefined && value !== '') envVars.push(`${key}=${quote([value])}`)
  }
  return envVars.join(' ')
}
// TEAMMATE_ENV_VARS = ['CLAUDE_CODE_USE_BEDROCK','CLAUDE_CODE_USE_VERTEX',
//   'CLAUDE_CODE_USE_FOUNDRY','ANTHROPIC_BASE_URL','CLAUDE_CONFIG_DIR',
//   'CLAUDE_CODE_REMOTE','CLAUDE_CODE_REMOTE_MEMORY_DIR','HTTPS_PROXY','https_proxy',
//   'HTTP_PROXY','http_proxy','NO_PROXY','no_proxy','SSL_CERT_FILE',
//   'NODE_EXTRA_CA_CERTS','REQUESTS_CA_BUNDLE','CURL_CA_BUNDLE']
```

The command is run with **no prompt argument** — initial instructions are delivered via
the mailbox AFTER the pane is created (see §3 step 7).

---

## 3. `handleSpawnSplitPane` full ordering (the path Codex copies)

1. Resolve `model` (teammate `model` or leader default; `'inherit'`→leader model).
2. Require `name`+`prompt`; resolve `teamName` (input or `teamContext.teamName`).
3. `uniqueName = generateUniqueTeammateName(name, teamName)` — appends `-2`,`-3`… on
   collision against existing `teamFile.members[].name` (case-insensitive).
4. `sanitizedName = sanitizeAgentName(uniqueName)`; `teammateId = formatAgentId(sanitizedName, teamName)`; `workingDir = cwd || getCwd()`.
5. `teammateColor = assignTeammateColor(teammateId)`.
6. `{paneId, isFirstTeammate} = createTeammatePaneInSwarmView(sanitizedName, teammateColor)`
   (tmux split — see §4). If `isFirstTeammate && insideTmux` → `enablePaneBorderStatus()`.
7. Build `spawnCommand` (§2) → `sendCommandToPane(paneId, spawnCommand, !insideTmux)`.
8. Register in TeamFile (`readTeamFileAsync`→ push member → `writeTeamFileAsync`):
   ```ts
   teamFile.members.push({
     agentId: teammateId, name: sanitizedName, agentType: agent_type, model, prompt,
     color: teammateColor, planModeRequired: plan_mode_required, joinedAt: Date.now(),
     tmuxPaneId: paneId, cwd: workingDir, subscriptions: [],
     backendType: detectionResult.backend.type,  // 'tmux' | 'iterm' (split-pane); 'tmux' (separate-window)
   })
   ```
9. Deliver the initial prompt via mailbox (NOT as a CLI arg):
   ```ts
   await writeToMailbox(sanitizedName,
     { from: TEAM_LEAD_NAME, text: prompt, timestamp: new Date().toISOString() }, teamName)
   ```
10. Return `SpawnOutput { teammate_id, agent_id, agent_type, model, name, color,
    tmux_session_name, tmux_window_name, tmux_pane_id, team_name, is_splitpane:true, plan_mode_required }`.

`handleSpawnSeparateWindow` differs only in step 6/7: it `ensureSession(SWARM_SESSION_NAME)`,
then `tmux new-window -t claude-swarm -n teammate-<sanitizeName(name)> -P -F '#{pane_id}'`,
then `tmux send-keys -t claude-swarm:<window> <spawnCommand> Enter`; `backendType:'tmux'`,
`is_splitpane:false`.

`handleSpawn` routing: `isInProcessEnabled()` → in-process; else try `detectAndGetBackend()`
(on failure in `auto` mode → `markInProcessFallback()` → in-process; in explicit `tmux` mode
→ propagate); else `use_splitpane !== false` → split-pane, else separate-window.

---

## 4. tmux backend commands (EXACT arrays passed to `execFileNoThrow('tmux', …)`)

```ts
// createTeammatePaneWithLeader (FIRST teammate, inside tmux): split from leader pane
['split-window','-t', currentPaneId, '-h','-l','70%','-P','-F','#{pane_id}']

// 2nd+ teammate (both withLeader and external): alternate split, target an existing teammate pane
//   splitVertically = (teammateCount % 2 === 1)   // odd→'-v', even→'-h'
['split-window','-t', targetPane, splitVertically ? '-v' : '-h', '-P','-F','#{pane_id}']

// sendCommandToPane
['send-keys','-t', paneId, command, 'Enter']

// killPane
['kill-pane','-t', paneId]

// setPaneTitle
['select-pane','-t', paneId, '-T', name]
['set-option','-p','-t', paneId, 'pane-border-format', `#[fg=${tmuxColor},bold] #{pane_title} #[default]`]

// setPaneBorderColor
['select-pane','-t', paneId, '-P', `bg=default,fg=${tmuxColor}`]   // tmux 3.2+
['set-option','-p','-t', paneId, 'pane-border-style', `fg=${tmuxColor}`]
['set-option','-p','-t', paneId, 'pane-active-border-style', `fg=${tmuxColor}`]
```
Outside tmux: a detached session named `claude-swarm` (`ensureSession`:
`tmux has-session -t <s>` then `tmux new-session -d -s <s>`); external commands run on the
swarm socket (`getSwarmSocketName()='claude-swarm-<pid>'`, passed as the `useSwarmSocket`
boolean = `!insideTmux`). `PaneBackendExecutor.spawn()` is the orchestration wrapper:
`backend.createTeammatePaneInSwarmView` → build spawnCommand → `backend.sendCommandToPane`;
TeamFile registration happens in `handleSpawnSplitPane`, not in the executor.

---

## 5. Codex target — files & module paths

- **NEW** `codex-rs/core/src/team_backends/mod.rs` — backend trait + types.
- **NEW** `codex-rs/core/src/team_backends/tmux.rs` — tmux command wrappers (§4).
- **NEW** `codex-rs/core/src/team_backends/spawn.rs` — spawn-command builder (§2) + env/flags.
- **EDIT (later phase, not this spec's job)** `codex-rs/core/src/lib.rs` — `mod team_backends;`.
- **EDIT** `codex-rs/core/src/tools/handlers/team.rs::team_spawn_member` — switch from
  `registry.spawn_member(...)` to process-in-pane spawn + `team_store` registration.

Existing reusable Phase-1 module: `codex-rs/core/src/team_store.rs` provides
`TeamFile`, `TeamFileMember` (camelCase serde incl. `tmuxPaneId`, `backendType`,
`isActive`), `agent_id(name,team)` (= Claude `formatAgentId`), `sanitize` (path segment),
`update_config`, `write_config`, `read_config`, `write_to_mailbox`, `now_timestamp`,
`TEAM_LEAD_NAME`. Reuse these; do NOT duplicate.

NOTE: `team_store::sanitize` is the PATH-segment sanitizer (Claude `getInboxPath`), NOT
Claude's `sanitizeAgentName`/`sanitizeName`. The new `spawn.rs` needs the agent-name
sanitizer (`@`→`-`) separately — see §6 `sanitize_agent_name`.

---

## 6. Rust signatures to add

### `core/src/team_backends/mod.rs`
```rust
//! Out-of-process teammate backends (Claude `PaneBackendExecutor` + `TmuxBackend`).
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendType { Tmux, Iterm, InProcess }

impl BackendType {
    pub fn as_str(self) -> &'static str {
        match self { Self::Tmux => "tmux", Self::Iterm => "iterm", Self::InProcess => "in-process" }
    }
}

/// Round-robin palette (Claude `AGENT_COLORS`).
pub const AGENT_COLORS: [&str; 8] =
    ["red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan"];

/// Result of opening a pane for a teammate.
pub struct PaneHandle { pub pane_id: String, pub is_first_teammate: bool }

/// Common backend surface (start with Tmux only; Iterm in Phase 6).
pub trait PaneBackend: Send + Sync {
    fn backend_type(&self) -> BackendType;
    fn inside_tmux(&self) -> bool;
    /// Claude `createTeammatePaneInSwarmView`.
    fn create_teammate_pane(&self, name: &str, color: &str) -> std::io::Result<PaneHandle>;
    /// Claude `sendCommandToPane`.
    fn send_command_to_pane(&self, pane_id: &str, command: &str) -> std::io::Result<()>;
    /// Claude `killPane`.
    fn kill_pane(&self, pane_id: &str) -> std::io::Result<()>;
    /// Claude `setPaneTitle` + `setPaneBorderColor`.
    fn set_pane_title(&self, pane_id: &str, name: &str, color: &str) -> std::io::Result<()>;
    fn enable_pane_border_status(&self) -> std::io::Result<()>;
}
```

### `core/src/team_backends/tmux.rs`
```rust
pub const TMUX_COMMAND: &str = "tmux";
pub const SWARM_SESSION_NAME: &str = "claude-swarm"; // keep Claude's name for parity
pub const SWARM_VIEW_WINDOW_NAME: &str = "swarm-view";

pub struct TmuxBackend { inside_tmux: bool, leader_pane_id: Option<String>, teammate_count: usize, socket: Option<String> }

impl TmuxBackend {
    pub fn detect() -> Option<Self>;                  // $TMUX present → inside; else detached-session mode
    fn run(&self, args: &[&str]) -> std::io::Result<std::process::Output>; // execFileNoThrow analog
    fn split_first(&self, leader_pane: &str) -> std::io::Result<String>;   // §4 ['split-window','-t',_,'-h','-l','70%','-P','-F','#{pane_id}']
    fn split_next(&self, target_pane: &str, vertical: bool) -> std::io::Result<String>; // §4 alternate -v/-h
    fn ensure_session(&self, session: &str) -> std::io::Result<()>;        // has-session / new-session -d
}
// impl PaneBackend for TmuxBackend { ... } — maps directly to the §4 arrays.
```

### `core/src/team_backends/spawn.rs`
```rust
use crate::config::Config;

/// Claude `sanitizeAgentName`: `@` → `-`.
pub fn sanitize_agent_name(name: &str) -> String { name.replace('@', "-") }

/// Claude `assignTeammateColor`: round-robin AGENT_COLORS by spawn index.
pub fn assign_teammate_color(index: usize) -> &'static str {
    crate::team_backends::AGENT_COLORS[index % crate::team_backends::AGENT_COLORS.len()]
}

/// POSIX single-quote (Claude `quote([...])`).
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() { return "''".into(); }
    if s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./:=@%+".contains(&b)) { return s.into(); }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Resolve the codex binary to launch (Claude `getTeammateCommand`).
/// Honors `CODEX_TEAMMATE_COMMAND` override, else `std::env::current_exe()`.
pub fn teammate_command() -> std::io::Result<String> {
    if let Ok(v) = std::env::var("CODEX_TEAMMATE_COMMAND") { if !v.is_empty() { return Ok(v); } }
    Ok(std::env::current_exe()?.to_string_lossy().into_owned())
}

/// Claude `buildInheritedEnvVars` analog (codex-flavored).
/// Always sets `CODEX_TEAMMATE=1`; forwards proxy/cert vars + `CODEX_HOME`.
pub fn build_inherited_env_vars() -> String;

/// Claude `buildInheritedCliFlags` analog. Emits `--model <m>` etc. for codex flags.
pub fn build_inherited_cli_flags(model: Option<&str>, plan_mode_required: bool) -> String;

pub struct TeammateSpawnParams<'a> {
    pub agent_id: &'a str,       // "name@team"  (team_store::agent_id)
    pub agent_name: &'a str,     // sanitized
    pub team_name: &'a str,
    pub agent_color: &'a str,
    pub parent_session_id: &'a str,
    pub agent_type: Option<&'a str>,
    pub plan_mode_required: bool,
    pub model: Option<&'a str>,
    pub working_dir: &'a Path,
    pub binary_path: &'a str,
}

/// Build the EXACT Claude command:
/// `cd <cwd> && env <vars> <binary> <teammateArgs><flagsStr>`
pub fn build_spawn_command(p: &TeammateSpawnParams<'_>) -> String;
```

`build_spawn_command` body (the literal port of §2; teammate CLI flag names kept identical
to Claude so a future codex teammate entrypoint — Phase 2 — accepts them):
```rust
let mut args = vec![
    format!("--agent-id {}", shell_quote(p.agent_id)),
    format!("--agent-name {}", shell_quote(p.agent_name)),
    format!("--team-name {}", shell_quote(p.team_name)),
    format!("--agent-color {}", shell_quote(p.agent_color)),
    format!("--parent-session-id {}", shell_quote(p.parent_session_id)),
];
if p.plan_mode_required { args.push("--plan-mode-required".into()); }
if let Some(t) = p.agent_type { args.push(format!("--agent-type {}", shell_quote(t))); }
let teammate_args = args.join(" ");
let mut flags = build_inherited_cli_flags(p.model, p.plan_mode_required);
let flags_str = if flags.is_empty() { String::new() } else { format!(" {flags}") };
let env_str = build_inherited_env_vars();
format!(
    "cd {} && env {} {} {}{}",
    shell_quote(&p.working_dir.to_string_lossy()),
    env_str,
    shell_quote(p.binary_path),
    teammate_args,
    flags_str,
)
```

---

## 7. How `team_spawn_member` rewires (the actual change in `team.rs` handler)

Current handler (lines 384–437 of `core/src/tools/handlers/team.rs`) calls
`registry.spawn_member(SpawnTeamMemberRequest{…})` which spawns an in-process agent thread.

New flow (process-in-pane), keeping the same `TeamSpawnMemberResult { member: TeamMember }`
JSON shape so the model + TUI observer are unchanged:

1. Keep auth (`require_team_lead`) + depth checks + `non_empty` name as-is.
2. Resolve team name + lead session id from the in-mem registry team (`team.name`,
   `team.lead_thread_id`) — these still exist for the live lead session.
3. `teams_root = turn.config.codex_home.as_path()` (field at `config/mod.rs:840`,
   `Config.codex_home: AbsolutePathBuf`; reachable via `turn.config` = `Arc<Config>`).
4. Compute identity:
   ```rust
   let sanitized = team_backends::spawn::sanitize_agent_name(&name);
   let team = &team_name;                       // raw team display name
   let agent_id = team_store::agent_id(&sanitized, team);   // "name@team"
   let color = team_backends::spawn::assign_teammate_color(member_index);
   let model = turn.config.model.clone();       // Config.model: Option<String> (config/mod.rs:587)
   let workingdir = turn.config.cwd.clone();     // teammate cwd
   let binary = team_backends::spawn::teammate_command()?;
   ```
5. Open pane:
   ```rust
   let backend = TmuxBackend::detect()
       .ok_or_else(|| FunctionCallError::RespondToModel("tmux not available for teammate pane".into()))?;
   let PaneHandle { pane_id, is_first_teammate } = backend.create_teammate_pane(&sanitized, color)?;
   if is_first_teammate && backend.inside_tmux() { backend.enable_pane_border_status()?; }
   backend.set_pane_title(&pane_id, &sanitized, color)?;
   ```
6. Build + send command:
   ```rust
   let cmd = team_backends::spawn::build_spawn_command(&TeammateSpawnParams {
       agent_id: &agent_id, agent_name: &sanitized, team_name: team, agent_color: color,
       parent_session_id: &session.thread_id.to_string(),
       agent_type: args.profile.as_deref(),     // codex `profile` ≈ Claude `agent_type`
       plan_mode_required: false, model: model.as_deref(),
       working_dir: workingdir.as_path(), binary_path: &binary,
   });
   backend.send_command_to_pane(&pane_id, &cmd)?;
   ```
7. Register in on-disk `config.json` via `team_store::update_config`:
   ```rust
   team_store::update_config(teams_root, team, |cfg| {
       cfg.name = team.to_string();
       if cfg.lead_agent_id.is_empty() {
           cfg.lead_agent_id = team_store::agent_id(team_store::TEAM_LEAD_NAME, team);
       }
       cfg.members.push(team_store::TeamFileMember {
           agent_id: agent_id.clone(), name: sanitized.clone(),
           agent_type: args.profile.clone(), model: model.clone(),
           prompt: Some(prompt_text.clone()), color: Some(color.to_string()),
           plan_mode_required: Some(false), joined_at: now_unix_millis(),
           tmux_pane_id: pane_id.clone(), cwd: workingdir.to_string_lossy().into_owned(),
           backend_type: Some(backend.backend_type().as_str().to_string()),
           is_active: Some(true), subscriptions: Vec::new(),
           ..Default::default()
       });
   })?;
   ```
8. Deliver initial prompt via mailbox (Claude step 9):
   ```rust
   team_store::write_to_mailbox(teams_root, team, &sanitized, team_store::TeammateMessage {
       from: team_store::TEAM_LEAD_NAME.to_string(),
       text: prompt_text, timestamp: team_store::now_timestamp(),
       read: false, color: None, summary: None,
   })?;
   ```
9. Synthesize the existing `TeamMember` shape for the return value (so
   `TeamSpawnMemberResult` JSON is unchanged). The struct (`team.rs:70`) needs an
   `agent_thread_id: ThreadId` — for a process teammate there is no in-process agent
   thread, so generate a placeholder `ThreadId::new()` for the member id and reuse it (or
   add a follow-up: thread the real session id from the teammate once it registers).
   Set `status: Active`, `agent_status: <Idle/Spawning>`, capabilities/permissions as
   parsed, `profile`, timestamps from `unix_timestamp()`.

### Argument-source notes (mapping codex → Claude)
- codex `TeamSpawnMemberArgs.profile` ↔ Claude `agent_type`.
- codex `TeamSpawnMemberArgs.message`/`items` → flatten to a single prompt string for the
  mailbox text (reuse `parse_team_input` then `input_preview`/text extraction).
- codex has no `plan_mode_required` arg yet → pass `false` (add flag in a later phase).
- `parent_session_id` = the lead session's `session.thread_id` rendered as string.

### Registry coexistence
Keep `core/src/team.rs` registry for the in-mem `Team`/`TeamMember`/`TeamSnapshot` types
the other tools (`team_status`, `team_send`, `team_message_list`, `team_event_list`,
`team_member_stop`, `team_stop`) still read. This spec changes ONLY the spawn path to
launch a process + write on-disk state; later phases (P4) migrate messaging to the mailbox
and (P3) wire `team_member_stop`→`backend.kill_pane(pane_id)` + `is_active=false`.

---

## 8. Codex env-var naming decisions (rename Claude → codex)

| Claude | Codex equivalent | Notes |
|---|---|---|
| `CLAUDECODE=1` | `CODEX_TEAMMATE=1` | teammate-mode marker |
| `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` | (drop or `CODEX_TEAMS=1`) | feature flag |
| `CLAUDE_CODE_TEAMMATE_COMMAND` | `CODEX_TEAMMATE_COMMAND` | binary override |
| `CLAUDE_CODE_AGENT_COLOR` | `CODEX_AGENT_COLOR` | optional; color now also a flag |
| `CLAUDE_CONFIG_DIR` | `CODEX_HOME` | forward so teammate uses same teams root |
| proxy/cert vars (`HTTPS_PROXY`,`HTTP_PROXY`,`NO_PROXY`,`SSL_CERT_FILE`,`NODE_EXTRA_CA_CERTS`,`REQUESTS_CA_BUNDLE`,`CURL_CA_BUNDLE`, lowercase variants) | forward verbatim | network parity |
| Bedrock/Vertex/Foundry/`ANTHROPIC_BASE_URL` | codex provider env (e.g. `OPENAI_*`, `CODEX_*` provider keys) | substitute codex's auth/provider env; the teammate process inherits provider via env + `--model` |

Keep the CLI flag names IDENTICAL to Claude (`--agent-id`, `--agent-name`, `--team-name`,
`--agent-color`, `--parent-session-id`, `--plan-mode-required`, `--agent-type`, `--model`)
so the Phase-2 teammate entrypoint port is a 1:1 flag parser.

---

## 9. Blocking unknowns (to resolve before/while implementing)

1. **Teammate entrypoint does not exist yet** (Phase 2). This spec's `build_spawn_command`
   emits flags the codex binary cannot yet parse. Spawned panes will launch codex without
   teammate-mode behavior until Phase 2 adds the flag parser + inbox run-loop. Sequence
   P2 before relying on P3 end-to-end.
2. **`TeamMember.agent_thread_id` is required** but a process teammate has no in-process
   agent thread. Decision needed: placeholder `ThreadId` now, or extend `TeamMember`/store
   to carry an optional `session_id` populated when the teammate self-registers.
3. **codex provider/auth env forwarding**: confirm which env vars actually carry
   provider+auth for a fresh codex process (the Claude Bedrock/Vertex list does not map
   1:1). Must be verified against `core/src/config` provider resolution before finalizing
   `build_inherited_env_vars`.
