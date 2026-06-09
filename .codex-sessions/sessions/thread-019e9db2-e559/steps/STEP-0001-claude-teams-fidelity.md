# STEP-0001 Claude Code Teams Fidelity Port

status: final-installed-smoke-passed-and-cleaned
updated_at: 2026-06-08 15:27:56 CST

## Before

The repository contains prior Codex Teams changes, but screenshots show teammate split panes and prompts still diverge from Claude Code. User specifically pointed to `https://github.com/ChinaSiro/claude-code-sourcemap` as the authority.

## Goal

Compare source-level Claude Code Teams behavior and port the smallest correct vertical slices into Codex Teams: command surface, prompt/context handling, Teams/subagent separation, TUI statusbar/navigation, and teammate reply routing.

## Actions

- Inspect git status and prior plan/worklog files.
- Inspect local/remote `claude-code-sourcemap` source for Teams and subagent behavior.
- Spawn bounded read-only subagents for parallel source mapping if continuing execution.
- Implement only after evidence is collected.
- Validate focused behavior before packaging/linking to `codex`.
- Compared Claude `AgentTool`, `TeamCreateTool`, `SendMessageTool`, `spawnMultiAgent`, `TeamStatus`, and `TeamsDialog` against current Codex source.
- Confirmed the current worktree already includes several fidelity fixes: `/teams`, plain mailbox first turn, `team_send.to`, footer aggregate teammate count, Teams dialog filtering of `team-lead`, and BM25 isolation tests for generic subagent wording.
- Full workspace validation completed in an earlier pass: `just test` passed with `10414 passed, 23 skipped`, then bench-smoke completed.
- Installed the verified final binary to `/Users/snakesammy/.cargo/bin/codex`.
- Final installed TUI smoke passed: `/teams` appeared as `open the Codex Teams dialog`, raw Enter dispatched it, and the installed TUI printed `No active Codex team to show. Start a team first; then teammates appear in the Teams dialog.`
- Final installed binary SHA256: `867e53742349115278c98a02ddc1be7a62c15cc42cc8ef99bc745b545200cc23`.
- Final package archive SHA256: `46ba05557045de9fef003e3e9a68f04a3d8525896501aa6b0e45c914a06e942a`.
- Cleaned build artifacts: `codex-rs/target` and `dist/local/teams-slice5/package` are gone; installed `codex` and the tar.gz package archive remain.

## Decisions

- Claude Code source evidence is required before further edits.
- Controller owns final edits and merge; subagents are evidence-only unless explicitly assigned bounded write slices.
- Do not copy Claude `TeamCreate.searchHint` literally into Codex. Codex BM25 must keep Teams search hints free of generic `agent` tokens to preserve native subagent routing.
- Copy Claude user/model-facing semantics where compatible, but preserve Codex-native split between `spawn_agent` and explicit Teams tools.
- Provider config for smoke must live in `CODEX_HOME/config.toml` for spawned teammates to see it. Lead-only `-c model_providers...` overrides are not automatically inherited by the teammate process.

## Evidence

- User screenshot shows teammate panes start with visible `Codex Teams context` YAML-like block and skill hook text.
- User states Claude Code prompt/output format and TUI do not match.
- User states lead agent lacks teammate statusbar.
- User states arrow-down selection/switching does not work and replies enter a queue unexpectedly.
- Claude source evidence: `AgentTool` uses normal subagent search hint `delegate work to a subagent`; teammate spawn only happens when `teamName && name`; `TeamCreateTool` search hint contains `multi-agent` but that belongs to Claude's routing architecture, not Codex BM25.
- Codex evidence: `spec_plan_tests.rs` includes negative searches for `subagent`, `spawn agent`, `parallel agents`, and `delegate task`, and positive search for `codex teams teammate`.
- Runtime evidence: installed TUI session `RUST_LOG=trace codex --enable teams --no-alt-screen -c 'log_dir="/tmp/codex-teams-installed-smoke"'` displayed `/teams  open the Codex Teams dialog`; Enter dispatched to `No active Codex team to show. Start a team first; then teammates appear in the Teams dialog.`
- Runtime evidence from an earlier linked-command smoke: `/tmp/codex-teams-linked-smoke-config/codex.out` contains `TEAMS_SMOKE_PASS team_id=019ea44c-61aa-77e1-b334-db4f8f0b166f member_id=019ea44c-6378-72f3-9dbd-12599c4506fd member_to_lead_completed=true`.
- Runtime caveat evidence: `/tmp/codex-teams-linked-smoke/codex.out` contains a fail when mock provider existed only as lead CLI `-c` overrides; `mock-member` inbox remained unread because the teammate lacked provider config.

## After

Validated, linked to the user-facing `codex` command, smoke-tested through the actual installed TUI, and cleaned build artifacts.

## Next

No remaining work for the `/teams` entrypoint/package/cleanup slice. If continuing, handle only newly reported runtime parity gaps.

## Search Keys

teams claude-code-sourcemap prompt statusbar slash-command subagent split teammate picker reply queue
