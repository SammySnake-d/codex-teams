# Fix Codex Teams user entrypoint

## Goal

Close the user-facing Codex Teams entrypoint gap so a user with `features.teams = true` can see and open the Teams control surface with `/teams`, and so model-callable Teams tools execute through the registered Teams handlers instead of surfacing `unsupported call` or falling back to native subagents.

## What I already know

- User enabled `[features] teams = true`.
- Natural-language `开启两个teammate` exposed or attempted Teams tool names, but `list_teams` and `create_team` returned `unsupported call`.
- The runtime then fell back to native `multi_agent_v1.spawn_agent`, which creates ordinary Codex subagents, not Codex Teams teammates.
- `codex-rs/tui/src/app_event.rs` already has `OpenTeamsDialog`.
- `codex-rs/tui/src/slash_command.rs` currently has no `/teams` command.
- The prior Teams infrastructure slice was validated for backend/TUI internals, but not for this visible entrypoint/runtime error path.

## Requirements

- Add a visible `/teams` slash command when the Teams feature is enabled.
- Dispatch `/teams` to the existing Teams dialog/open event path.
- Preserve ordinary `/agent` and native subagent behavior; do not turn generic subagent requests into Teams.
- Fix Teams tool execution so `features.teams = true` exposes executable `create_team`, `list_teams`, and related Teams tools through the active registry.
- If Teams cannot execute, fail closed with a Teams-specific error instead of silently creating native subagents.
- Keep changes surgical and aligned with Claude Code parity: Teams and subagents are separate user/product surfaces.

## Acceptance Criteria

- [x] `/teams` appears in slash command discovery when `features.teams = true`.
- [x] `/teams` opens the Teams dialog/control surface.
- [x] Teams slash command is hidden or unavailable when `features.teams = false`.
- [x] With `features.teams = true`, `create_team` and `list_teams` are executable, not just searchable/deferred names.
- [x] Native `spawn_agent` remains separate and does not populate the Teams roster.
- [x] Focused codex-tui and codex-core tests pass.

## Out of Scope

- New reviewer/blocker/Darwin team policies.
- Reworking ordinary native subagent navigation.
- Full persistent Teams resume.
- Broad UI redesign beyond the missing Teams command/control entrypoint.

## Technical Notes

- Relevant specs: `.trellis/spec/codex-rs/backend/index.md`, `.trellis/spec/codex-rs/backend/teams-architecture.md`.
- Root constraint: update files directly and reviewably; no temporary rewrite scripts.
- Use `just fmt` after Rust edits and focused `just test` targets before final.
## Vertical Slices

- Slice 1: TUI `/teams` control surface. Write set: TUI slash-command enum/filter/dispatch and focused TUI tests only. Proof: `/teams` resolves when `features.teams=true`, is gated when false, and dispatches `AppEvent::OpenTeamsDialog`.
- Slice 2: Core Teams executor path. Write set: core tool planning/registry/tests only. Proof: with `features.teams=true`, `create_team` and `list_teams` dispatch successfully through `ToolRegistry`; they remain direct tools even when `tool_search` is available for unrelated deferred tools.
- Slice 3: No native-subagent fallback. Write set: core tool descriptions/tests only if needed. Proof: explicit Teams teammate language routes to Teams tools or fails Teams-specific; native `spawn_agent` remains separate.
- Slice 4: Runtime smoke before package. Write set: test/smoke notes only unless a bug is found. Proof: run a local command/session path or focused executable test that proves `/teams` and `create_team/list_teams` work before installing/linking `codex`.
- Slice 5: Packaging after proof. Write set: build/install/link only after Slice 1-4 pass. Proof: installed `codex` binary shows `/teams` and no `unsupported call` for Teams tools.

## Updated Execution Contract

- Do not run full cargo builds until code-level and minimal runtime proofs pass.
- Prefer focused Rust tests and direct binary smoke over workspace-wide tests while target is cold.
- Use subagents for bounded investigation/patch proposals; main session owns final edits, verification, packaging, and merge.

## Validation Results

- `just fmt` passed in `codex-rs`; only existing ruff `exclude-newer = "7 days"` warnings were printed.
- `git diff --check` passed.
- `just test -p codex-core teams_tools_are_lead_only_and_do_not_replace_spawn_agent teams_tools_stay_direct_when_tool_search_available teams_create_and_list_dispatch_through_registry_when_tool_search_available team_create_and_list_use_agent_control_registry` passed: 4 tests run, 4 passed.
- `just test -p codex-tui teams_command slash_teams` passed: 7 tests run, 7 passed.
- `just fix -p codex-core` passed.
- `just fix -p codex-tui` passed.
- `cargo build -p codex-cli --bin codex` passed and produced `codex-rs/target/debug/codex`.
- Staged binary smoke passed: `--enable teams features list` reported `teams ... true`; `--disable teams features list` reported `teams ... false`.
- Staged TUI smoke passed: `/tea` displayed `/teams  open the Codex Teams dialog`; `/teams` dispatched to the Teams path and showed `No active Codex team to show.`
- Package archive produced: `dist/local/teams-slice5/codex-package-aarch64-apple-darwin.tar.gz`.
- Installed command smoke passed after linking `/Users/snakesammy/.cargo/bin/codex`: `codex --enable teams features list` reported `teams ... true`.
- Installed TUI smoke passed in the real installed command: `codex --enable teams --no-alt-screen -c 'log_dir="/tmp/codex-teams-installed-smoke"'` showed `/teams  open the Codex Teams dialog`; raw Enter dispatched it and printed `No active Codex team to show. Start a team first; then teammates appear in the Teams dialog.`
- Final installed binary hash: `/Users/snakesammy/.cargo/bin/codex` SHA256 `867e53742349115278c98a02ddc1be7a62c15cc42cc8ef99bc745b545200cc23`.
- Final package archive: `dist/local/teams-slice5/codex-package-aarch64-apple-darwin.tar.gz` SHA256 `46ba05557045de9fef003e3e9a68f04a3d8525896501aa6b0e45c914a06e942a`.
- Cleanup completed and rechecked: `codex-rs/target` and `dist/local/teams-slice5/package` were deleted; the tar.gz package archive and installed `codex` command were retained.
- Post-clean installed command proof passed: `codex --enable teams features list` still reported `teams ... true`.

## Follow-up Issue

This task only proves the user-visible `/teams` entrypoint and initial executable tool path. It does not prove full Claude Code Teams parity.

The active follow-up issue is `.trellis/tasks/06-09-claude-teams-fidelity-gaps/`. That issue owns the current user-reported gaps:

- teammate prompt/envelope still differs from Claude Code and may show Codex-specific Teams context;
- Teams tool prompts/descriptions must be rechecked against `claude-code-sourcemap`;
- TUI footer/statusbar/header/navigation still differs, including Down-arrow switching and teammate view status;
- reply queue behavior must be source-backed and runtime-proven;
- native `subagent` requests must remain isolated from Teams when `features.teams = true`;
- full create/spawn/message/reply runtime E2E must pass before another package/link step.
