# Codex Teams ← Claude Code parity (design basis)

Truth source: `ChinaSiro/claude-code-sourcemap` (de-minified Claude Code).

## Claude Code "Agent Teams" mechanism (verified from source)
- Spawn: `spawnTeammate` (`restored-src/src/tools/shared/spawnMultiAgent.ts`) →
  `handleSpawnSplitPane` / `handleSpawnSeparateWindow`.
- tmux: `TmuxBackend.createTeammatePaneWithLeader`
  - 1st teammate: `tmux split-window -t <leaderPane> -h -l 70% -P -F '#{pane_id}'`
  - rest: split from an existing teammate pane, alternating `-v`/`-h` (tiled by odd/even count)
  - `setPaneBorderColor` (`select-pane` + `set-option -p`), `setPaneTitle` (`select-pane -T` + `set-option -p pane-border-format`)
  - not in tmux → detached session `claude-swarm`
- Launch command (run inside the new pane via `tmux send-keys -t <pane> <cmd> Enter`):
  `cd <dir> && env <vars> <claude-binary> --agent-id X --agent-name Y --team-name Z --agent-color C --parent-session-id S [--plan-mode-required] [--agent-type T] [--model ...]`
- iTerm2: `ITermBackend` via `it2 session split` (1st vertical from leader, rest horizontal).
- Teammate bootstrap (`main.tsx::run` → `extractTeammateOptions`): reads `--agent-id/--agent-name/--team-name`,
  sets `dynamicTeamContext`. Shared state on disk:
  - team config: `~/.claude/teams/{team}/config.json`
  - task list: `~/.claude/tasks/{team}/`
  - mailbox: file-based; `readUnreadMessages(agentName, teamName)`, `writeToMailbox(...)`
- Messaging/@: write to a teammate's mailbox; teammate replies via `writeToMailbox`; an idle `Stop` hook
  notifies the lead. Messages auto-deliver as new conversation turns.
- TUI: `restored-src/src/components/teams/TeamsDialog.tsx` (`viewTeammateOutput`, `toggleTeammateVisibility`),
  per-teammate colors, status "pills".
- Two display modes (`settings.json: teammateMode` / `--teammate-mode`):
  - `in-process`: all teammates in the one terminal; Shift+↑/↓ select; type to message. ANY terminal.
  - split-pane: each teammate own pane (tmux/iTerm). Requires separate teammate PROCESSES.

## Codex teams reality (this repo)
- `core/src/tools/handlers/team.rs::team_spawn_member` → `session.services.agent_control.team_registry()`
  spawns an IN-PROCESS agent thread (`TeamMember.agent_thread_id` lives in the lead's app-server).
- Messaging is in-memory registry + `Op::InterAgentCommunication`; no on-disk mailbox/process.
- `Team.live_session_only: bool` exists; teammates are threads, not processes.
- => Codex's runtime == Claude's IN-PROCESS mode.

## What is already implemented (codex TUI, this branch) = Claude in-process-mode parity
- Footer/toolbar roster: `tui/src/chatwidget/team_ui.rs` (`footer_label`) — `Teams: <name> · @member running … · ctrl+t teammates`.
- `@teammate` candidates + routing: `mentions_v2` + `input_submission.rs` (team:// → model-mediated `team_send`).
- ctrl+t teammate switch (native AgentNavigation) == Claude Shift+↑/↓.
- Best-effort tmux side pane that TAILS the teammate rollout (read-only) — `app/teammate_panes.rs`.
- All 4 TUI tests pass; standalone tmux split smoke passes.

## The gap (split-pane mode)
Real Claude split panes = separate teammate processes. Codex teammates are in-process threads, so a pane
can only OBSERVE (tail) them, not host an interactive teammate. To match Claude's split-pane mode faithfully
requires a codex teammate-PROCESS mode.

## Paths
- A (faithful, large, core change): add a split-pane teammate mode to codex:
  `team_spawn_member` (or a TUI reaction) launches a separate `codex` teammate process in a tmux pane
  (split-window -h 70% + alternating, colored borders/titles), the teammate process joins the team via
  shared on-disk team state + a file mailbox, and lead⇄member messaging goes through that mailbox.
  Touches core (team registry persistence, a `codex --agent-id/--team` teammate entrypoint, mailbox IPC).
- B (pragmatic, smaller): keep in-process runtime; upgrade the TUI to mirror Claude's UX precisely —
  Claude's pane LAYOUT (split-window -h 70% + alternating, colored borders + titles) showing each teammate's
  live view, colored teammate "pills" in the footer, and the in-terminal select/@ flow. No core change.

## Recommendation
Ship B now (matches Claude in-process mode + Claude-style pane layout for visibility), and only do A if
truly-interactive separate teammate panes are required (it duplicates the runtime as multi-process).
