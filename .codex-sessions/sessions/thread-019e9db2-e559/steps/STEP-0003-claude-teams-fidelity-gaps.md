# STEP-0003 Claude Teams Fidelity Gaps And Runtime E2E

status: installed-wrapper-runtime-e2e-passed
updated_at: 2026-06-10 01:50:00 CST

## Before

The `/teams` entrypoint and part of Teams/subagent routing isolation were implemented and focused-tested. User testing then showed that the runtime experience still does not match Claude Code Teams: teammate panes expose Codex-specific context text, TUI status/navigation differs, reply delivery is unclear, and native `subagent` wording may still trigger Teams when the feature is enabled.

## Issue

Do not treat the current Teams work as complete. `/teams` being visible is only an entrypoint proof. The actual target is Claude Code Teams parity, source-backed from `https://github.com/ChinaSiro/claude-code-sourcemap`, while preserving Codex native subagents.

## User-Visible Problems To Track

- Teammate transcript can still show Codex-specific visible context/envelope such as `Codex Teams context:` instead of Claude-style task delivery.
- Teammate prompt and Teams tool prompt/descriptions do not yet match Claude source structure closely enough.
- Main agent footer/statusbar lacks full Claude-style teammate status behavior.
- Teammate view lacks the expected Claude-style header/status such as `Viewing @agent · esc return` plus task prompt.
- Down-arrow selection/switching in the Teams UI is not behaving like Claude Code.
- Reply behavior appears queued or disconnected from Claude's SendMessage model and needs source-backed verification.
- Native `subagent` requests must remain native Codex subagents; enabling Teams must not make `subagent` open split-pane teammates.

## Decisions

- Start each implementation slice by reading the corresponding Claude source file and naming the source-backed behavior being ported.
- Do not solve Teams/subagent separation with production user-language keyword classifiers in `tool_search.rs`.
- Do not blindly copy Claude `searchHint` strings when Codex BM25 semantics would route generic `agent` or `subagent` prompts into Teams.
- Do not package/link `codex` again until focused tests and real runtime E2E prove the current slice. Current user-facing `codex` is a wrapper install because copying the Mach-O directly to `~/.cargo/bin/codex` was SIGKILLed.
- Keep changes surgical and Teams-only unless a Codex-native API needs a small adapter.
- Treat the current teammate header work as a half-finished Slice 3 patch. Do not declare it complete until it compiles and focused tests actually run.
- Treat this step as the active issue boundary for the next worker. Do not continue implementation from memory alone; re-open the source anchors and current diff before editing.

## Source Anchors

- Claude generic search: `/tmp/claude-code-sourcemap/restored-src/src/tools/ToolSearchTool/ToolSearchTool.ts`
- Claude subagent tool: `/tmp/claude-code-sourcemap/restored-src/src/tools/AgentTool/AgentTool.tsx`
- Claude team creation: `/tmp/claude-code-sourcemap/restored-src/src/tools/TeamCreateTool/TeamCreateTool.ts`
- Claude SendMessage: `/tmp/claude-code-sourcemap/restored-src/src/tools/SendMessageTool/SendMessageTool.ts`
- Claude teammate prompt addendum: `/tmp/claude-code-sourcemap/restored-src/src/utils/swarm/teammatePromptAddendum.ts`
- Claude process pane spawn: `/tmp/claude-code-sourcemap/restored-src/src/tools/shared/spawnMultiAgent.ts`
- Claude pane backend: `/tmp/claude-code-sourcemap/restored-src/src/utils/swarm/backends/PaneBackendExecutor.ts`
- Claude teammate view UI: `/tmp/claude-code-sourcemap/restored-src/src/components/TeammateViewHeader.tsx`
- Claude team status UI: `/tmp/claude-code-sourcemap/restored-src/src/components/teams/TeamStatus.tsx`
- Claude navigation helper: `/tmp/claude-code-sourcemap/restored-src/src/hooks/useBackgroundTaskNavigation.ts`
- Claude teammate view helper: `/tmp/claude-code-sourcemap/restored-src/src/state/teammateViewHelpers.ts`

## Acceptance Boundary

Teams is not complete until all of these are proven against the installed or staged binary:

- `/teams` command visible and feature-gated.
- Natural-language explicit Teams request can create a team without falling back to native subagents.
- `create_team` succeeds through the active tool registry.
- `team_spawn_member` creates a real teammate pane/session.
- First teammate task arrives through mailbox/plain task delivery with no visible Codex Teams context envelope.
- Teammate can reply with `team_send`.
- Lead poller receives and presents the reply correctly.
- Footer/statusbar/header/navigation match the source-backed Claude behavior for the implemented slice.
- Explicit native `subagent` requests still use native Codex subagents and do not create Teams/split panes.

## Validation Plan

- Prefer focused source-backed Rust tests before runtime.
- Use TUI snapshot coverage for visible status/header/navigation changes.
- Run runtime smoke before package/link.
- Clean `codex-rs/target` after validation if it grows large and no build/test is active.

## Next

Continue Slice 3: teammate view header/statusbar parity.

Current partial implementation notes:

- `TeamSpawnMemberResult` now carries optional `prompt` in the current patch.
- `TeamRosterMember` now carries optional `prompt` in the current patch.
- `RegisterTeammateThread` now carries optional `prompt` in the current patch.
- `TeamTeammateViewHeader` and rendering plumbing were added to show a Claude-style teammate view header.
- Tests were partially updated in `team_roster_navigation_tests`, app input/tests helpers, and `chatwidget/tests/team_ui.rs`.

Known validation state:

- `just fmt` passed for the patch before latest compaction.
- `git diff --check` passed before latest compaction.
- Focused TUI tests did not run to completion: `webrtc-sys` failed while downloading the WebRTC macOS arm64 artifact with TLS EOF.
- At 2026-06-09 13:58 CST, old Rust/Cargo process trees still existed. Do not run cleanup while those Rust/Cargo processes are active.
- At 2026-06-09 14:10 CST, the latest process check still showed `just fix -p codex-core`; no `just fix -p codex-tui` process was observed.
- At 2026-06-09 14:10 CST, `codex-rs/target` was about `29G`; disk availability was about `88GiB` on `/System/Volumes/Data`.
- At 2026-06-09 14:37 CST, no active Rust/Cargo/just build or test processes were observed.
- At 2026-06-09 14:37 CST, `codex-rs/target` was about `33G`; disk availability was about `79GiB` on `/System/Volumes/Data`.
- At 2026-06-09 14:50 CST, no active Rust/Cargo/just build or test processes were observed.
- At 2026-06-09 14:50 CST, `codex-rs/target` was about `33G`; disk availability was about `81GiB` on `/System/Volumes/Data`.
- User requested durable work log and issue recording in this turn; no new code validation, package/link, source edit, or cleanup happened in this memory update.
- Latest source mirror for Claude comparison is `/tmp/claude-code-sourcemap-codex-teams`, commit `a8a678cb6244e6770e1e421767ff0987a1d95549`.
- Latest handoff confirmed the Claude split: `TeamCreate` is a Teams tool; teammate spawn happens through `AgentTool` only when `teamName && name`; plain subagents remain the normal `AgentTool` path.
- Latest handoff confirmed first teammate work should be mailbox/plain task text, not visible `Codex Teams context:` transcript text, and teammate communication belongs in the addendum rule requiring `SendMessage`.
- Latest handoff records a fixed `SendMessage({"to":"alice", ...})` alias bug: shared `TeamSendArgs` normalization now maps `to` before falling through to legacy `member_id` validation.
- Latest handoff records improved teammate spawn result metadata: `TeamSpawnMemberResult` returns `color`, `mode`, and `is_active`, and TUI parser/tests verify propagation into `RegisterTeammateThread`.
- Latest validation passed before the current regression: 9 focused `codex-core` Teams/subagent/prompt tests; 3 focused core mailbox/SendMessage tests; `codex-tui team_ui team_roster_navigation` with 24 tests; `just fmt` twice.
- Current failing state: `codex-rs/core/src/tools/handlers/team.rs` search anchors include `working together`, and `teams_tool_search_requires_explicit_teams_terms_not_subagent` fails because query `delegate work to a subagent` now loads Teams tools.
- At 2026-06-09 15:25 CST, no active Rust/Cargo/just build or test processes were observed.
- At 2026-06-09 15:25 CST, `codex-rs/target` was about `34G`; disk availability was about `83GiB` on `/System/Volumes/Data`.
- User requested durable work log and issue recording in this turn; no Rust source edit, validation run, package/link, process kill, or build-cache cleanup happened in this checkpoint.
- At 2026-06-09 18:14 CST, user reported a new runtime readiness failure: spawned teammate panes can enter the Codex login screen instead of directly entering teammate mode.
- Current diagnosis: teammate spawn inherited `CODEX_HOME` but did not forward Codex auth env entrypoints; the TUI embedded app-server also always started with `enable_codex_api_key_env=false`, so env-auth leads could work while spawned teammates appeared unauthenticated.
- Focused implementation: `codex-rs/core/src/team_backends/spawn.rs` now includes `CODEX_API_KEY`, `CODEX_ACCESS_TOKEN`, and `OPENAI_API_KEY` in the inherited env whitelist; `codex-rs/tui/src/lib.rs` now enables `CODEX_API_KEY` env auth for embedded app-server only when launched as a teammate process with `team_name` and `agent_name`.
- Focused validation passed: `cd codex-rs && just fmt`; `just test -p codex-core codex_auth_env_vars_are_forwarded_by_default teammate_binary_escapes_cargo_deps_test_binary`; `just test -p codex-tui embedded_app_server_forwards_codex_api_key_env_toggle embedded_app_server_start_failure_is_returned`.
- This fixes the auth-env login-screen cause only. It is not full Teams completion and did not include package/link or staged runtime E2E.
- At 2026-06-09 18:48 CST, user asked why teammate enters the login screen; read-only native subagents confirmed the source-backed diagnosis: a split-pane teammate is a new TUI process, so it must inherit usable auth/config/provider state. A temporary `CODEX_HOME` without credentials or a forced `model_provider="openai"` can show login even if the real `~/.codex` custom-provider setup works.
- Source comparison confirmed Claude `AgentTool` passes `model ?? agentDef?.model` into teammate spawn and Claude `resolveTeammateModel` maps `"inherit"` to the leader model.
- Focused implementation extended the `spawn_agent` teammate branch: `codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs` now forwards `SpawnAgentArgs.model` into `SpawnMemberFromAgentToolRequest`; `codex-rs/core/src/tools/handlers/team.rs` resolves requested teammate model so `None` and `"inherit"` use the lead model while explicit model values are preserved.
- Focused validation passed: `just fmt`; `just test -p codex-core teammate_model_resolves_inherit_to_leader_model multi_agent_v2_spawn_name_uses_active_team_teammate_branch codex_auth_env_vars_are_forwarded_by_default teammate_binary_escapes_cargo_deps_test_binary`; `just test -p codex-core team_tool_search_info_is_explicit_teams_only teams_tool_search_requires_explicit_teams_terms_not_subagent teams_feature_keeps_v1_subagent_search_separate`; `just test -p codex-tui embedded_app_server_forwards_codex_api_key_env_toggle embedded_app_server_start_failure_is_returned`; `git diff --check`.
- Latest environment check after validation: no active Rust/Cargo/just processes observed; `codex-rs/target` about `38G`; `/System/Volumes/Data` about `59GiB` available.
- No package/link was performed. Full runtime E2E remains required before the user-facing `codex` command should be relinked.

Concrete resume checklist:

- Check no stale background `find`/Cargo process is still running before starting validation.
- If the next action is cleanup, `cargo clean` or targeted removal of `codex-rs/target` is allowed only after one fresh check confirms no Rust/Cargo process is active.
- Installed-command runtime E2E is now proven through the user-facing `codex` wrapper: create team, spawn teammate pane, first mailbox task without visible `Codex Teams context:` envelope, teammate `team_send`, lead `team_message_list` receipt, `team_event_list`, and `team_status`.
- Installed runtime sentinel: `TEAMS_SMOKE_PASS team_id=019ead80-7dbd-7971-a8bb-5e2d1c1dc1ae member_id=019ead80-7ef7-7433-90d8-1aecd1dbb362 member_to_lead_completed=true status_output_bytes=964`.
- Installed runtime evidence directory: `/tmp/codex-teams-installed-smoke.3VieM9`.
- Current install form: `/Users/snakesammy/.cargo/bin/codex` is a shell wrapper that execs `/Users/snakesammy/Desktop/project/codex-teams/codex-rs/target/debug/codex`.
- Do not delete `/Users/snakesammy/Desktop/project/codex-teams/codex-rs/target/debug/codex`; the wrapper depends on it.
- Direct copied-binary install is a tracked issue: copying the Mach-O into `~/.cargo/bin/codex` caused exit code `137` / SIGKILL. Backups are `/Users/snakesammy/.cargo/bin/codex.backup-20260610-014152` and `/Users/snakesammy/.cargo/bin/codex.failed-copy-20260610-014405`.
- Build cache cleanup removed `target/debug/deps`, `target/debug/incremental`, and `target/debug/build`, leaving `codex-rs/target` about `1.4G` and disk availability about `76GiB`.
- Continue the broader Claude Code Teams fidelity audit before marking the goal complete: source-backed footer/header/navigation behavior, native subagent isolation under real user prompts, and remaining Claude prompt/tool parity gaps still need completion audit.
- Include a teammate model/provider check in the staged E2E: spawn through `spawn_agent` with `name/team_name` and either omitted model or `model="inherit"`, then confirm the child command/config uses the lead model/provider and does not fall back to a login-triggering provider.
- Consider extracting a pure teammate launch spec builder before more runtime work, so tests can assert binary, env, and flags before tmux/iTerm side effects.
- If runtime E2E exposes another source drift, inspect current focused diff around that slice before editing.
- Search for missing `prompt` call sites if resuming header work: `TeamRosterMember::new(`, `RegisterTeammateThread {`, and `TeamUiEvent::MemberSpawned {`.
- Re-run `cd codex-rs && just test -p codex-tui team_roster_navigation team_ui` once the WebRTC dependency path is unblocked.
- Keep prompt/envelope removal and reply queue semantics as separate slices unless compile paths force a shared adapter.
- If the user asks "is Teams done?", answer no unless the runtime E2E proves create team, spawn teammate pane, first mailbox task without visible `Codex Teams context:`, teammate `team_send`, lead receipt, footer/header navigation, and native subagent isolation.

## Search Keys

claude-code-sourcemap teams prompt statusbar down navigation team_send queue subagent isolation teammate header runtime e2e
