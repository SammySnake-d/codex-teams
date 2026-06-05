# Codex Teams → independent-process port (Claude Code mechanism, ported to Rust)

Truth source: ChinaSiro/claude-code-sourcemap. Teammates are INDEPENDENT `codex` PROCESSES,
each in its own tmux/iTerm pane, coordinating via on-disk shared team state + file mailbox.
Replaces codex's current in-process `registry.spawn_member` thread model.

## On-disk layout (adapt Claude's ~/.claude → $CODEX_HOME, default ~/.codex)
- Team config:   $CODEX_HOME/teams/{team}/config.json
- Inboxes:       $CODEX_HOME/teams/{team}/inboxes/{agent_name}.json
- Task list:     $CODEX_HOME/tasks/{team}/
Names sanitized (Claude getInboxPath sanitizes team+agent).

## Data types (port to Rust, serde)
TeamFile (config.json):
  name:String, description:Option<String>, created_at:i64 (createdAt),
  lead_agent_id:String (leadAgentId), lead_session_id:Option<String> (leadSessionId),
  hidden_pane_ids:Vec<String> (hiddenPaneIds), members: Vec<TeamFileMember>
TeamFileMember:
  agent_id:String (agentId="{name}@{team}"), name:String, agent_type:Option<String>,
  model:Option<String>, prompt:Option<String>, color:Option<String>,
  plan_mode_required:Option<bool>, joined_at:i64, tmux_pane_id:String (tmuxPaneId),
  cwd:String, worktree_path:Option<String>, session_id:Option<String>,
  subscriptions:Vec<String>, backend_type:Option<String> (tmux|iterm|in_process),
  is_active:Option<bool>, mode:Option<String>
TeammateMessage (inbox array element):
  from:String, text:String, timestamp:String(ISO8601), read:bool,
  color:Option<String>, summary:Option<String>
IdleNotification (createIdleNotification → JSON string put in `text`):
  kind:"idle", from, summary(last peer DM 5-10 words), color

## Mailbox ops (port; advisory file lock e.g. fs2 or a .lock sibling)
- inbox_path(team,agent) = teams/{team}/inboxes/{sanitize(agent)}.json
- write_to_mailbox(team, to_agent, msg): mkdir -p; lock; read array; push {..read:false}; write.
- read_mailbox(team,agent) -> Vec<TeammateMessage>
- read_unread(team,agent) -> filter read==false
- mark_message_read_by_index / mark_messages_read: lock; set read=true; write.

## Identity (CLI flags on the teammate `codex` process)
--agent-id "{name}@{team}"  --agent-name <display>  --team-name <team>
--agent-color <color>  --parent-session-id <lead session uuid>
[--agent-type <t>] [--plan-mode-required] [inherited: --model ...]
agent_name is used for messaging/tasks; agent_id for internal tracking.

## Phase plan (file-by-file; each phase compiles independently)

### P1. core: on-disk team store + mailbox  (NEW: codex-rs/core/src/team_store.rs)
- Port TeamFile/TeamFileMember/TeammateMessage/IdleNotification + path helpers + mailbox ops + lock.
- Pure module + unit tests (round-trip write/read/mark-read; concurrent lock).
- Do NOT wire yet. Keep existing in-memory registry compiling.

### P2. cli: teammate entrypoint  (codex-rs/cli/src/main.rs + a teammate runner)
- Add flags --agent-id/--agent-name/--team-name/--agent-color/--parent-session-id/--agent-type/--plan-mode-required.
- When --agent-name+--team-name present → "teammate mode": start a normal codex session whose
  turn source is the inbox. Run loop (port runInProcessTeammate/waitForNextPromptOrShutdown):
  poll read_unread; priority shutdown > TEAM_LEAD_NAME > others; mark read; inject as next user turn
  (peer msgs wrapped via formatAsTeammateMessage XML, user msgs plain); on session Stop → mark
  is_active=false in config + write IdleNotification to lead inbox.
- TEAM_LEAD_NAME constant ("team-lead" per Claude).

### P3. backends: tmux (NEW: codex-rs/core/src/team_backends/tmux.rs) + lead spawn-in-pane
- create_teammate_pane_with_leader: 1st = `tmux split-window -t <leaderPane> -h -l 70% -P -F '#{pane_id}'`;
  rest = split from existing teammate pane, alternate -v/-h by count parity.
- set_pane_border_color (select-pane + set-option -p), set_pane_title (select-pane -T + pane-border-format).
- not in tmux → detached session "codex-swarm".
- send_command_to_pane(pane, cmd, press_enter) = `tmux send-keys -t <pane> <cmd> Enter`.
- spawn_command = `cd <cwd> && env <vars> <codex-bin> <teammate flags> <inherited>`.
- Replace team_spawn_member core path: register member in config.json + launch process in pane,
  store tmux_pane_id/backend_type. (Keep TeamSpawnMemberResult shape for the model + TUI observer.)

### P4. messaging cross-process
- team_send → write_to_mailbox(team, target_agent, msg) (lead→member or member→lead).
- Lead inbox poller (port useInboxPoller): the lead codex session polls its own inbox
  ($CODEX_HOME/teams/{team}/inboxes/{lead}.json) and injects teammate replies/idle as turns.
- team_message_list/team_event_list read from store.

### P5. TUI (codex-rs/tui)  — port TeamsDialog.tsx behavior
- Teams dialog: list teammates with color pills + is_active status; viewTeammateOutput
  (focus pane / tail), toggleTeammateVisibility (tmux hide/show pane, update hidden_pane_ids).
- Footer roster already implemented (team_ui.rs) — keep; add colors.
- ctrl+t switch stays for in-process fallback.

### P6. iTerm backend (NEW: codex-rs/core/src/team_backends/iterm.rs)
- `it2 session split` (1st vertical from leader, rest horizontal); setup prompt if `it2`/PythonAPI missing.

## Notes / risks
- This REPLACES in-process teammates with processes. Keep `Team.live_session_only` semantics: a process
  team is persistent (config on disk). Decide migration vs. dual-mode (teammateMode: process|in_process).
- agent_control.team_registry currently spawns threads; P3 changes spawn to processes. Audit callers.
- Auth/model: teammate process inherits provider/auth via env (buildInheritedEnvVars analog) + --model.
- Existing TUI team_ui observer (footer/@) keeps working because team tools still emit the same JSON.

## STATUS (updated)
DONE + green (cargo test -p codex-core --lib team_ => 27 passed; core compiles clean):
- P1  core/src/team_store.rs        — TeamFile/TeammateMessage, mailbox r/w/mark-read, config r/w/update, file lock, atomic write.
- P4* core/src/team_coord.rs        — IdleNotification + create/send, format_as_teammate_message (XML peer wrap),
                                       protocol detection (shutdown/plan-approval/idle), select_next_inbox priority,
                                       teammate_attachments, task-list CRUD under tasks/{team}/.
- P3a core/src/team_backends/tmux.rs— pick_split parity (vertical = count%2==1, idx=(count-1)/2), set_pane_title/border_color,
                                       send_command_to_pane, ensure swarm session, build_launch_line, AgentColor, BackendType.
- P6  core/src/team_backends/iterm.rs— it2 session split (1st vertical / rest horizontal), it2/python-API setup checks.
- core/src/team_backends/mod.rs + lib.rs mod decls wired.
Note: pick_split prose table in 02-tmux.md was self-contradictory; the literal extracted formula
(splitVertically = teammateCount % 2 == 1) is authoritative and the impl/tests follow it.

REMAINING (coupled; touch shared files cli/core/tui — do sequentially, needs codex session-lifecycle APIs):
- P2  cli teammate entrypoint + run loop (spec 05-teammate-runloop.md): new flags in codex-rs/cli/src/main.rs
      (--agent-id/--agent-name/--team-name/--agent-color/--parent-session-id/--agent-type/--plan-mode-required);
      boot a codex session whose turns come from the inbox; poll via select_next_inbox; inject peer msgs through
      team_coord::format_as_teammate_message; on Stop write team_coord idle notification to lead inbox.
- P3b rewire core team_spawn_member (spec 01-spawn.md): register member in team_store config + launch a separate
      codex process in a pane via team_backends::{tmux,iterm} (store tmuxPaneId/backendType) instead of in-process thread.
- P4b team_send -> team_store::write_to_mailbox cross-process; lead inbox poller (spec 06) injects teammate replies as turns.
- P5  TUI Teams dialog + colored pills + lead poll feeding turns (spec 06-tui-and-lead-poller.md).
