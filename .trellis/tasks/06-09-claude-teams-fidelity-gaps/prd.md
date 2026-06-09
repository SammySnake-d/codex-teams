# Close Claude Code Teams fidelity gaps

## Goal

Make Codex Teams behave like Claude Code Teams for the user-visible and model-visible surfaces that are currently failing: teammate prompt delivery, Teams tool prompts/descriptions, teammate status/header/navigation, queued replies, and isolation from native Codex subagents.

## Problem

The previous `/teams` entrypoint work made Teams discoverable and fixed one executable tool path, but user testing showed that the real Teams experience still diverges from Claude Code:

- teammate panes can expose visible Codex-specific context instead of Claude-style task delivery;
- the teammate prompt and Teams tool prompts/descriptions are not source-faithful enough;
- TUI footer/statusbar/header/navigation do not match Claude Code;
- Down-arrow teammate selection/switching does not work as expected;
- reply delivery appears queued or detached from the expected SendMessage flow;
- with Teams enabled, wording such as `subagent` may still route into Teams/split panes instead of native Codex subagents.

## Non-Negotiables

- Compare against `https://github.com/ChinaSiro/claude-code-sourcemap` before each implementation slice.
- Preserve native Codex subagents. Generic `subagent`, `spawn_agent`, or ordinary parallel delegation must not create Teams or split panes.
- Do not fix routing with production user-language keyword classifiers in `codex-rs/core/src/tools/handlers/tool_search.rs`.
- Do not package/link the `codex` command until focused tests and real runtime E2E pass.
- Keep changes surgical. Prefer Teams-only state/adapters over broad Codex TUI rewrites.
- Use direct, per-file edits; no temporary rewrite scripts or bulk generated code edits.

## Source Anchors

- Generic Claude tool search: `/tmp/claude-code-sourcemap/restored-src/src/tools/ToolSearchTool/ToolSearchTool.ts`
- Claude normal subagent tool: `/tmp/claude-code-sourcemap/restored-src/src/tools/AgentTool/AgentTool.tsx`
- Claude team creation: `/tmp/claude-code-sourcemap/restored-src/src/tools/TeamCreateTool/TeamCreateTool.ts`
- Claude SendMessage: `/tmp/claude-code-sourcemap/restored-src/src/tools/SendMessageTool/SendMessageTool.ts`
- Claude teammate prompt addendum: `/tmp/claude-code-sourcemap/restored-src/src/utils/swarm/teammatePromptAddendum.ts`
- Claude process pane spawn: `/tmp/claude-code-sourcemap/restored-src/src/tools/shared/spawnMultiAgent.ts`
- Claude pane backend: `/tmp/claude-code-sourcemap/restored-src/src/utils/swarm/backends/PaneBackendExecutor.ts`
- Claude teammate view header: `/tmp/claude-code-sourcemap/restored-src/src/components/TeammateViewHeader.tsx`
- Claude team status: `/tmp/claude-code-sourcemap/restored-src/src/components/teams/TeamStatus.tsx`
- Claude background task navigation: `/tmp/claude-code-sourcemap/restored-src/src/hooks/useBackgroundTaskNavigation.ts`
- Claude teammate view helpers: `/tmp/claude-code-sourcemap/restored-src/src/state/teammateViewHelpers.ts`

## Vertical Slices

### Slice 1: Teammate Prompt Envelope

- Remove or hide visible Codex Teams context from teammate transcript.
- Deliver the first task in the Claude-style mailbox/task path.
- Move teammate communication contract into the correct system/tool prompt boundary.
- Add focused tests for no visible `Codex Teams context:` and plain first task delivery.

### Slice 2: Teams Tool Prompt And Search Semantics

- Re-read Claude tool prompts/descriptions/search hints.
- Port intent and schema semantics without copying terms that break Codex BM25 routing.
- Keep `tool_search.rs` generic.
- Add source-backed positive Teams tests and negative native subagent tests.

### Slice 3: TUI Statusbar, Header, And Navigation

- Add Claude-style teammate view status/header, including `Viewing @name · esc return` semantics where source-backed.
- Restore footer/statusbar teammate status under the main agent.
- Fix Down/Enter/Esc switching behavior to match the Claude source-backed model.
- Cover UI output with focused tests and snapshots.

### Slice 4: Reply Queue And Lead Poller

- Align `team_send` / SendMessage behavior with Claude source: plain assistant text is not the message path; messages route through the team mailbox.
- Prove queued delivery when a receiver is busy and prompt execution when idle.
- Add focused lead inbox poller and teammate reply tests.

### Slice 5: Runtime E2E Before Package

- Run a staged binary smoke that proves create team, spawn teammate pane, first mailbox task, teammate reply, lead receipt, footer/header navigation, and no visible envelope.
- Run a separate native subagent smoke proving `subagent` still opens native subagents rather than Teams.
- Only after this proof, package/link the `codex` command.

## Acceptance Criteria

- [ ] `/teams` remains visible and feature-gated.
- [ ] Explicit Teams request can create a team without falling back to native subagents.
- [ ] `create_team` succeeds through the active registry.
- [ ] `team_spawn_member` creates a real teammate pane/session.
- [ ] First teammate task appears as plain task text without visible Codex Teams context.
- [ ] Teammate can reply with `team_send`.
- [ ] Lead receives the reply through the poller/inbox path.
- [ ] Footer/statusbar/header/navigation match the source-backed Claude behavior for implemented slices.
- [ ] Explicit native `subagent` requests still use native Codex subagents and do not create Teams/split panes.
- [ ] Focused tests and real runtime E2E pass before package/link.

## Validation Plan

- Start each slice with source inspection notes.
- Run focused tests first, not full workspace builds.
- Use snapshot coverage for visible TUI changes.
- Run staged runtime smoke before package/link.
- Clean `codex-rs/target` after validation if no build/test is active and disk use is high.

## Current Work Log

### 2026-06-09 06:34 CST

- Active slice: Slice 3, teammate view header/statusbar/navigation parity.
- Current patch is in progress, not validated.
- Partial implementation propagates teammate prompt through `TeamSpawnMemberResult`, `TeamRosterMember`, `RegisterTeammateThread`, and `TeamTeammateViewHeader` rendering paths.
- Partial test updates exist around `team_roster_navigation_tests`, app input/tests helpers, and `chatwidget/tests/team_ui.rs`.
- `just fmt` and `git diff --check` reportedly passed before latest compaction.
- Focused TUI test command `cd codex-rs && just test -p codex-tui team_roster_navigation team_ui` did not reach code validation because `webrtc-sys` failed to download the WebRTC macOS arm64 artifact with TLS EOF.
- Next action: inspect focused diff, fix compile misses around `prompt` call sites, unblock/resolve WebRTC dependency path, then re-run focused TUI tests before any runtime E2E or package/link.

### 2026-06-09 13:58 CST

- User requested a durable work log and issue record for the current state, not another development continuation.
- Current issue remains open: the visible teammate pane prompt/context, Teams tool prompts, TUI header/statusbar/navigation, Down-arrow switching, queued reply semantics, and native subagent isolation still require source-backed Claude Code comparison and runtime proof.
- No new package/link was performed in this checkpoint.
- Active Rust/Cargo processes were still present. Do not clean `codex-rs/target` while those are active.
- `codex-rs/target` was about `26G`; `/System/Volumes/Data` had about `91GiB` available.
- Resume rule: inspect source anchors and the current diff before editing; do not mark Slice 3 or Teams complete until focused tests and the staged runtime E2E pass.

### 2026-06-09 14:10 CST

- User again requested work-log and issue recording.
- Current issue remains open and unchanged: Claude Teams fidelity is not complete and `/teams` entrypoint proof is not enough.
- Latest process check showed `just fix -p codex-core` still active; no `just fix -p codex-tui` process was observed in that check.
- `codex-rs/target` was about `29G`; `/System/Volumes/Data` had about `88GiB` available.
- No source edit, validation run, package/link, process kill, or build-cache cleanup happened in this checkpoint.
- Resume rule remains: first inspect the current diff and Claude source anchors, then fix only the next bounded slice and validate before package/link.

### 2026-06-09 14:37 CST

- User requested work-log and issue recording again.
- Current issue remains open: the active task is still Claude Code Teams fidelity, not the already-completed `/teams` entrypoint task.
- Latest process check found no active Rust/Cargo/just build or test processes.
- `codex-rs/target` was about `33G`; `/System/Volumes/Data` had about `79GiB` available.
- No source edit, validation run, package/link, process kill, or build-cache cleanup happened in this checkpoint.
- Resume rule: if cleanup is the next action, recheck no Rust/Cargo process is active and then clean build artifacts; if development is the next action, inspect the current diff and Claude source anchors before editing.

### 2026-06-09 14:50 CST

- User requested another durable work-log and issue record.
- Current issue remains open: the runtime screenshots still show Claude Teams fidelity gaps, especially visible teammate prompt/context injection, missing or mismatched statusbar/header/navigation, queued reply semantics, and unresolved native subagent isolation proof.
- Latest process check found no active Rust/Cargo/just build or test processes.
- `codex-rs/target` was about `33G`; `/System/Volumes/Data` had about `81GiB` available.
- No Rust source edit, validation run, package/link, process kill, or build-cache cleanup happened in this checkpoint.
- Resume rule: do not restart broad implementation from memory alone. First re-open the Claude source anchors and current diff, then either clean build artifacts or continue one bounded source-backed slice.

### 2026-06-09 15:25 CST

- User requested another durable work-log and issue record.
- Latest Claude source mirror used by the previous worker: `/tmp/claude-code-sourcemap-codex-teams` at commit `a8a678cb6244e6770e1e421767ff0987a1d95549`.
- Source-backed behavior confirmed: Claude `TeamCreate` is a Teams tool; Claude teammate spawn is `AgentTool` only with `teamName && name`; plain subagents stay on the normal `AgentTool` path; the first teammate task is mailbox/plain prompt text; teammate communication belongs in the `SendMessage` addendum.
- Latest fixed/validated work from handoff: `SendMessage({"to":"alice", ...})` alias normalization was fixed; teammate spawn metadata now includes `color`, `mode`, and `is_active`; focused core and TUI tests passed before the current regression.
- Current failing state: `codex-rs/core/src/tools/handlers/team.rs` Teams search anchors include `working together`, and `teams_tool_search_requires_explicit_teams_terms_not_subagent` fails because query `delegate work to a subagent` loads Teams tools.
- Next minimum fix: remove `working together` and avoid `work` / generic `agent` anchors, then rerun `cd codex-rs && just fmt` and `cd codex-rs && just test -p codex-core team_tool_search_info_is_explicit_teams_only teams_tool_search_requires_explicit_teams_terms_not_subagent teams_feature_keeps_v1_subagent_search_separate`.
- Latest process check found no active Rust/Cargo/just build or test processes.
- `codex-rs/target` was about `34G`; `/System/Volumes/Data` had about `83GiB` available.
- No Rust source edit, validation run, package/link, process kill, or build-cache cleanup happened in this checkpoint.

### 2026-06-09 16:15 CST

- User corrected the architecture direction before more implementation: first land the latest architecture into Markdown/Trellis, then start parallel subagents.
- Source-backed trigger decision is now recorded in `.trellis/spec/codex-rs/backend/teams-architecture.md`: keep Codex's single global BM25 ToolSearch for now; do not add a Teams-only search layer; do not solve subagent isolation with production user-language classifiers; route teammate spawn through the shared `spawn_agent` surface using Claude-style structure (`active/explicit team + name`), while ordinary `subagent`/delegation stays native.
- `team_spawn_member` remains allowed as compatibility or exact Teams-control surface, but it must not be the main natural-language teammate spawn route.
- Teams search hints must avoid native-subagent collision terms such as `agent`, `subagent`, `work`, `parallel`, and broad delegation phrases.
- New runtime failure reported by the user: running the built test binary as `codex_core ... teammate --agent-id ...` fails with `error: Unrecognized option: 'agent-id'`. This points to a CLI teammate subcommand contract mismatch and must be diagnosed before package/link.
- No code fix or validation was performed in this checkpoint; next work should start with read-only subagent lanes and then a controller-owned merge.

### 2026-06-09 16:36 CST

- Controller launched three native Codex read-only subagents, not Teams teammates:
  - Lane A diagnosed the `--agent-id` runtime failure and confirmed high confidence that the flags are valid for the real `codex teammate` CLI, but the process was launching `target/debug/deps/codex_core-*`, where `teammate` is treated as a test filter and `--agent-id` is rejected by the test harness.
  - Lane B confirmed the recorded architecture: Codex still uses BM25, `spawn_agent` already has the Claude-style `name`/`team_name` teammate branch, native subagent isolation guards exist, and the remaining drift is that `team_spawn_member` is still a model-searchable route.
  - Lane C confirmed the main TUI parity gap: Codex currently focuses an external pane from footer selection, while Claude enters teammate view and renders `Viewing @name · esc return`; `f` view and `k` kill shortcuts are also not implemented in footer navigation.
- Implemented the runtime blocker fix:
  - `codex-rs/core/src/team_backends/spawn.rs` now resolves the teammate binary from `CODEX_TEAMMATE_COMMAND`, then configured `codex_self_exe`, and escapes `target/debug/deps/*` test binaries to the sibling real `target/debug/codex` when available.
  - `codex-rs/core/src/tools/handlers/team.rs` now passes `turn.config.codex_self_exe.as_deref()` to the teammate binary resolver in both tmux and iTerm spawn paths.
  - Fixed a small existing TUI compile seam in `codex-rs/tui/src/chatwidget/team_ui.rs` by storing the `spawn_agent` teammate call's inferred team id as `Some(...)`.
- Validation:
  - `cd codex-rs && just fmt` passed twice; only existing Ruff `exclude-newer = "7 days"` warnings appeared.
  - `cd codex-rs && just test -p codex-core teammate_binary_escapes_cargo_deps_test_binary` passed.
  - `cd codex-rs && just test -p codex-cli teammate_parses_bypass_hook_trust_flag` passed.
  - `cd codex-rs && just test -p codex-tui team_ui` passed: 19 tests passed, nextest reported 1 leaky test but no failure.
  - `git diff --check` passed.
- Still not complete: no staged runtime E2E has proven create team, spawn teammate pane, mailbox first task, teammate reply, lead receipt, footer/header navigation, and native subagent isolation through the packaged command.

### 2026-06-09 17:04 CST

- User required the latest architecture to be landed into Markdown/Trellis before any more parallel subagents.
- Session active task pointer was corrected from `.trellis/tasks/06-07-fix-teams-user-entrypoint` to `.trellis/tasks/06-09-claude-teams-fidelity-gaps` so future Trellis injection reads the Claude fidelity task, not the completed entrypoint task.
- Architecture spec now records the latest implementation contract:
  - keep one global Codex BM25 `ToolSearch`;
  - do not add a Teams-only search layer;
  - do not route native subagent isolation through broad production user-language classifiers;
  - use shared `spawn_agent` as the primary spawn surface: no `name` means native Codex subagent; active/explicit team plus `name` means Teams teammate;
  - keep `team_spawn_member` as exact-name compatibility/control surface, not broad natural-language teammate spawn;
  - keep Teams hints away from `agent`, `subagent`, `work`, `parallel`, and broad delegation phrases;
  - runtime teammate spawn must launch the real `codex` CLI and escape `target/debug/deps/*` test harness binaries.
- Current verified state from handoff remains:
  - `just fmt` passed;
  - `codex-core teammate_binary_escapes_cargo_deps_test_binary` passed;
  - `codex-cli teammate_parses_bypass_hook_trust_flag` passed;
  - `codex-tui team_ui` passed with 19 passed and 1 leaky;
  - three core Teams/subagent isolation tests and `multi_agent_v2_spawn_name_uses_active_team_teammate_branch` reportedly passed;
  - `git diff --check` passed.
- Remaining proof before package/link:
  - exact/internal `team_spawn_member` remains callable;
  - core lead-only/defer isolation tests pass;
  - TUI footer/header/navigation parity is implemented and snapshot-covered;
  - runtime E2E proves create/spawn/mailbox first task/reply/lead receipt/navigation/native subagent isolation.

### 2026-06-09 18:14 CST

- User reported a new runtime readiness failure: spawned teammate panes can enter the Codex login screen instead of directly entering teammate mode.
- Diagnosis: process-backed teammate spawn inherited `CODEX_HOME` but did not forward Codex auth env entrypoints; TUI embedded app-server also always started with `enable_codex_api_key_env=false`, so env-auth lead sessions could work while teammate sessions appeared unauthenticated.
- Focused implementation:
  - `codex-rs/core/src/team_backends/spawn.rs` now forwards `CODEX_API_KEY`, `CODEX_ACCESS_TOKEN`, and `OPENAI_API_KEY` as inherited teammate env vars.
  - `codex-rs/tui/src/lib.rs` now passes `enable_codex_api_key_env=true` to the embedded app-server only when the TUI was launched as a teammate process with `team_name` and `agent_name`; ordinary TUI launches keep the previous default.
- Validation:
  - `cd codex-rs && just fmt` passed with only existing Ruff `exclude-newer = "7 days"` warnings.
  - `cd codex-rs && just test -p codex-core codex_auth_env_vars_are_forwarded_by_default teammate_binary_escapes_cargo_deps_test_binary` passed.
  - `cd codex-rs && just test -p codex-tui embedded_app_server_forwards_codex_api_key_env_toggle embedded_app_server_start_failure_is_returned` passed.
- Still not complete: no package/link or staged runtime E2E has proven create team, spawn teammate pane, no login screen under env auth, mailbox first task, teammate reply, lead receipt, footer/header navigation, and native subagent isolation.

## Active Issue Notes

- Do not mark `/teams` or Slice 3 as complete from static inspection.
- Do not package/link until the staged binary proves create team, spawn teammate pane, mailbox first task, teammate reply, lead receipt, footer/header navigation, and native subagent isolation.
- Do not solve subagent isolation by adding user-language keyword classifiers to `tool_search.rs`.
- Do not create a separate Teams-only tool-search layer in the current slice. Keep one Codex ToolSearch substrate and move teammate-spawn disambiguation to `spawn_agent` structured arguments.

## Out Of Scope

- Broad Codex TUI redesign unrelated to Teams.
- Replacing native Codex subagents with Teams.
- New unrelated reviewer/blocker/Darwin policies.
- Full persistent Teams resume unless required by the current Claude-fidelity slice.

### 2026-06-09 19:33 CST

- New current-state evidence refined the teammate login-screen diagnosis.
- `codex-rs/target/debug/codex teammate --help` is Teams-capable and exposes `--agent-id`, `--agent-name`, and `--team-name`.
- `/Applications/Codex.app/Contents/Resources/codex teammate --help` is still the old top-level Codex help and does not expose the hidden `teammate` subcommand, so that binary can treat `teammate` as a normal prompt/TUI path and fall into ordinary login/onboarding behavior.
- This means the latest login-screen issue is not explained by a bad `auth.json` alone; the stronger root cause is binary selection plus teammate-mode readiness. Spawn must fail closed unless the selected binary proves `codex teammate` support.
- Current disk state before further validation: no active Rust/Cargo/just processes; `codex-rs/target` is about `46G`, mostly `debug/deps` and `debug/incremental`; `/System/Volumes/Data` is about `91%` used with about `41GiB` available.
- Do not package/link yet. Use `codex-rs/target/debug/codex` or `CODEX_TEAMMATE_COMMAND=/Users/snakesammy/Desktop/project/codex-teams/codex-rs/target/debug/codex` for staged E2E.

### 2026-06-10 00:19 CST

- Staged no-package runtime E2E passed using `codex-rs/target/debug/codex`, `CODEX_TEAMMATE_COMMAND` pointing to that debug binary, tmux, and the deterministic no-secret `codex responses-api-proxy --mock-teams-smoke` provider.
- Runtime sentinel: `TEAMS_SMOKE_PASS team_id=019ead17-2df4-7643-99ea-46b1a5d75067 member_id=019ead17-2e0a-7233-98f3-3250efc4dd75 member_to_lead_completed=true status_output_bytes=964`.
- Runtime evidence directory: `/tmp/codex-teams-smoke.POHSgc`.
- Evidence proved: `create_team`, `team_spawn_member`, tmux split-pane teammate process, mailbox first task, lead-to-member mailbox send, teammate-originated `team_send` to lead, lead `team_message_list` receipt, `team_event_list`, and `team_status`.
- Teammate pane launched `/Users/snakesammy/Desktop/project/codex-teams/codex-rs/target/debug/codex teammate ... --enable teams` and did not use `/Applications/Codex.app/Contents/Resources/codex`.
- No visible `Codex Teams context:` / `You are an independent Codex Teams teammate` envelope appeared in the teammate pane capture or team mailbox files. Member inbox contained only the assigned task text and lead follow-up.
- Focused validation after smoke passed:
  - `cd codex-rs && just fmt` passed with only existing Ruff `exclude-newer = "7 days"` warnings.
  - `just test -p codex-core teammate_binary_rejects_non_teammate_cli teammate_binary_escapes_cargo_deps_test_binary codex_auth_env_vars_are_forwarded_by_default teammate_model_resolves_inherit_to_leader_model multi_agent_v2_spawn_name_uses_active_team_teammate_branch team_tool_search_info_is_explicit_teams_only teams_tool_search_requires_explicit_teams_terms_not_subagent teams_feature_keeps_v1_subagent_search_separate teammate_spawn_requires_lead_auth_when_provider_requires_openai_auth teammate_spawn_accepts_lead_auth_when_provider_requires_openai_auth team_send_to_field_validates_claude_constraints teammate_process_team_send_can_omit_team_id` passed: 12 tests passed; one test was reported leaky by nextest but not failed.
  - `just test -p codex-tui slash_teams team_roster_navigation team_ui footer_snapshots mentions_v2 teammate_startup_skips_onboarding_even_when_login_or_trust_would_show embedded_app_server_forwards_codex_api_key_env_toggle embedded_app_server_start_failure_is_returned` passed: 38 tests passed.
  - `just test -p codex-core tool_search_with_teams_feature_keeps_subagent_query_native` passed: 1 test passed, proving Teams feature enabled does not route `subagent` search to Teams tools.
  - `just test -p codex-cli teammate_parses_bypass_hook_trust_flag` passed: 1 test passed.
  - `just test -p codex-responses-api-proxy` passed: 10 tests passed.
  - `cargo insta pending-snapshots --manifest-path tui/Cargo.toml --as-json` produced no pending snapshot output; `find codex-rs -name '*.snap.new' -o -name '*.snap.pending'` found none.
  - `git diff --check` passed.
- Cleanup performed before tests: removed `codex-rs/target/debug/incremental`, reducing `codex-rs/target` from about `46G` to about `25G`; after tests it is about `28G` and `/System/Volumes/Data` has about `59GiB` available.
- Remaining before user-facing retest: package/link the staged binary into the `codex` command, verify the installed command is the Teams-capable binary, and then clean build artifacts without deleting the installed command/package.

### 2026-06-10 01:50 CST

- User-facing `codex` command is now routed to the verified development binary through a wrapper at `/Users/snakesammy/.cargo/bin/codex`:
  - wrapper target: `/Users/snakesammy/Desktop/project/codex-teams/codex-rs/target/debug/codex`
  - target binary SHA256: `bdb0a59cd5ada3f45937575d83f1e11bd410c1d70f118ad610acb9f4bc542908`
  - previous installed binary backup: `/Users/snakesammy/.cargo/bin/codex.backup-20260610-014152`
  - failed copied Mach-O backup: `/Users/snakesammy/.cargo/bin/codex.failed-copy-20260610-014405`
- Important installation finding: copying the Mach-O directly to `/Users/snakesammy/.cargo/bin/codex` produced exit code `137` / SIGKILL for `codex --version`, `codex teammate --help`, `codex login status`, and `codex --enable teams features list`. A wrapper that `exec`s the original `target/debug/codex` works.
- Installed-command smoke passed after wrapper install:
  - `codex --version` -> `codex-cli 0.0.0`
  - `codex teammate --help` exposes `Usage: codex teammate` plus `--agent-id`, `--agent-name`, and `--team-name`
  - `CODEX_HOME=/Users/snakesammy/.codex codex --enable teams features list` reports `teams ... true`
  - `CODEX_HOME=/Users/snakesammy/.codex codex login status` reports API-key login
- Installed-command runtime E2E passed using the user-facing `codex` wrapper, tmux, and `codex responses-api-proxy --mock-teams-smoke`.
- Runtime sentinel: `TEAMS_SMOKE_PASS team_id=019ead80-7dbd-7971-a8bb-5e2d1c1dc1ae member_id=019ead80-7ef7-7433-90d8-1aecd1dbb362 member_to_lead_completed=true status_output_bytes=964`.
- Runtime evidence directory: `/tmp/codex-teams-installed-smoke.3VieM9`.
- Evidence proved through the installed command: `create_team`, `team_spawn_member`, split-pane tmux teammate process, mailbox first task, lead-to-member mailbox send, teammate-originated `team_send` to lead, lead `team_message_list` receipt, `team_event_list`, and `team_status`.
- Pane capture showed the teammate command used `/Users/snakesammy/.cargo/bin/codex teammate ...`, which now resolves to the development binary through the wrapper. No visible `Codex Teams context:` / `You are an independent Codex Teams teammate` envelope was found in the installed smoke evidence.
- Build artifact cleanup completed after confirming no Rust/Cargo/just process was active and `otool -L target/debug/codex` only referenced system libraries:
  - removed `codex-rs/target/debug/deps`
  - removed `codex-rs/target/debug/incremental`
  - removed `codex-rs/target/debug/build`
  - kept `codex-rs/target/debug/codex` because the installed wrapper depends on it
  - `codex-rs/target` is about `1.4G`; `/System/Volumes/Data` has about `76GiB` available
- Remaining caveat: this is a working wrapper install, not a standalone copied binary install. Do not delete `codex-rs/target/debug/codex` unless `/Users/snakesammy/.cargo/bin/codex` is replaced with another verified working executable.
