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

## Claude Code branch boundary: Teams vs subagents
- `restored-src/src/tools/AgentTool/AgentTool.tsx` resolves `teamName` from
  explicit `team_name` or the active `appState.teamContext`, but only when Agent
  Teams is enabled.
- The decisive Teams branch is `if (teamName && name) { spawnTeammate(...);
  return ... }`. It happens before the generic subagent path.
- Teammates cannot spawn nested teammates with `name`; Claude raises an error and
  tells the teammate to omit `name` to spawn an ordinary subagent instead.
- If that Teams branch is not taken, execution continues into the normal
  `runAgent(...)` path. So Claude's split-pane behavior is a teammate branch, not
  a property of every subagent.
- TUI switching is likewise separated: Claude uses Shift+↑/↓ + Enter for
  teammate selection, `@name` mailbox routing when team context exists, and normal
  background/subagent mechanics otherwise.

## Codex teams reality (this repo)
- `core/src/tools/handlers/team.rs::team_spawn_member` now prefers a real
  `codex teammate` process in tmux/iTerm2 when a pane backend is available.
- Process-backed teammates receive their first turn through the on-disk team
  mailbox and run the full interactive Codex TUI in teammate mode.
- If no tmux/iTerm2 pane backend is available, `team_spawn_member` fails closed
  instead of degrading into native `spawn_agent`; ordinary delegation remains on
  the native subagent path.
- Teams tools are default-off behind `features.teams` and are exposed only to
  lead/top-level sessions; native subagents keep the normal `spawn_agent` path.
- Codex intentionally does not overload generic `spawn_agent` with Claude's
  `teamContext + name` teammate shortcut. The equivalent Teams product path is
  explicit: `create_team` then `team_spawn_member`.

## What is already implemented (codex TUI, this branch)
- Footer/toolbar roster: Teams label plus colored `@main` / teammate pills, with Shift+↑/↓ selection, Enter switch, and Esc return-to-lead behavior.
- `@teammate` candidates + routing: `mentions_v2` + `input_submission.rs` (team:// → model-mediated `team_send`).
- Generic `/agent` navigation remains shared with ordinary subagents; the Teams footer roster is populated only by Teams teammate spawn events.
- Process-backed teammates own their tmux/iTerm pane; the TUI focuses that pane
  from the Teams roster instead of treating the returned Teams member id as a
  generic `/agent` thread.
- Legacy side-pane tailing remains only a display helper for already-known
  thread transcripts; it is not a Teams teammate spawn fallback.
- Validation must be reported from current command output; do not reuse the old
  "All 4 TUI tests pass" claim after this process-pane rewrite.

## Remaining validation gap (cross-process split-pane E2E)
Codex has the process-pane substrate, but do not call this slice complete unless
current validation proves:
`team_spawn_member -> pane process -> teammate inbox poller -> first turn -> team_send -> lead poller`.
Do not call this slice done from static review or docs-only proof.

## Historical paths
- A (faithful, large, core change; current direction): add a split-pane teammate mode to codex:
  `team_spawn_member` (or a TUI reaction) launches a separate `codex` teammate process in a tmux pane
  (split-window -h 70% + alternating, colored borders/titles), the teammate process joins the team via
  shared on-disk team state + a file mailbox, and lead⇄member messaging goes through that mailbox.
  Touches core (team registry persistence, a `codex --agent-id/--team` teammate entrypoint, mailbox IPC).
- B (superseded fallback idea): keep in-process runtime; upgrade the TUI to mirror Claude's UX precisely —
  Claude's pane LAYOUT (split-window -h 70% + alternating, colored borders + titles) showing each teammate's
  live view, colored teammate "pills" in the footer, and the in-terminal select/@ flow. No core change.

## Recommendation
Continue the process-pane path. Keep native `spawn_agent` and generic `/agent`
navigation as ordinary subagent surfaces; copy Claude Teams behavior only into
the explicit Teams product path (`create_team` / `team_spawn_member` / Teams
roster / mailbox pollers).
