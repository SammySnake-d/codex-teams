# Codex Teams Infrastructure

## Goal

Build a Codex Teams foundation for the Rust TUI that lets a user create and operate a group of independent Codex agent sessions from natural language and a manual `/teams` help surface.

The goal is infrastructure first: team lifecycle, member sessions, messaging, shared task state, observable events, role/capability boundaries, and TUI control. Reviewer/self-correction/Darwin workflows are future policies on top of this substrate, not the substrate itself.

## What I Already Know

- The working fork is `/Users/snakesammy/Desktop/project/codex-teams`.
- `origin` points to `https://github.com/SammySnake-d/codex.git`.
- `upstream` points to `https://github.com/openai/codex.git`.
- Codex already has multi-agent primitives: spawn, send message, followup task, wait, close, list, mailbox, and agent metadata.
- Codex TUI already has `/agent` and `/subagents` style switching, but not a Team registry or shared task board.
- Anthropic-style Teams are not just subagents: they include a lead, teammates as independent sessions, shared task list, mailbox messaging, and user/team display control.
- The user requirement includes natural-language startup: the user should be able to ask Codex to create a team, not memorize a slash command.

## Product-Body Skeleton

Teams substrate owns:

- team registry
- member registry
- independent teammate session lifecycle
- message bus
- shared task board
- event/worklog feed
- role/capability profile attachment
- status/watch surface
- stop/cleanup lifecycle

Policies attach later:

- reviewer PASS/BLOCKERS workflow
- read-only reviewer role
- Darwin incident loop
- planner/executor/reviewer recipes
- domain-specific team templates

## MVP Scope

The first vertical slice should support:

- create a team from model-callable core tools
- spawn one teammate from the team
- route lead-to-member, member-to-member, and member-to-lead messages
- list team/member status
- create, claim, update, and list generic shared task-board items
- list the append-only team event feed
- stop one teammate while the team remains active
- stop the team
- expose minimal `/teams` manual guidance; model-callable tools are the action path

## Natural-Language Acceptance

A user prompt like this must be enough to drive the model toward Teams tools once implemented:

```text
Create a Codex Teams workspace with 3 named Teams teammates to investigate this task in parallel.
```

Generic requests for subagents, agents in parallel, implementation/check lanes,
or read-only audit lanes should use native `spawn_agent`/subagent tools unless
the user explicitly says those agents should be Codex Teams teammates.

Slash command parity is intentionally narrow in this slice: `/teams` may guide the user, but it must not become the primary proof or embed workflow policy. Natural language requires model-exposed tools and usage hints.

## First-Slice Design Decisions

- Team state is live-session-only. Persistent resume is out of scope, and team snapshots must make the live-only boundary explicit.
- Live lead-side team state is scoped to the owning `ThreadManager`. Teams teammates join the team through the on-disk team store and mailbox as process-backed `codex teammate` sessions; ordinary in-process `spawn_agent` sessions remain outside the Teams teammate path.
- Stopped teams remain visible in `list_teams` and `team_status` so users can inspect final members, messages, tasks, and events after cleanup.
- `team_status` refreshes member agent statuses from `AgentControl` before returning the snapshot.
- `team_spawn_member` treats `message` or non-empty `items` as the teammate's first-turn prompt. It launches a real `codex teammate` process in a tmux/iTerm pane, registers it in the on-disk team store, and writes the generic Teams context envelope plus prompt into that teammate's mailbox. If no pane backend is available, the tool fails closed instead of falling back to native `spawn_agent`.
- Task state is now a generic substrate mutation boundary: `team_task_create`, `team_task_update`, and `team_task_list` can manage shared task-board items without encoding workflow policy.
- `team_task_claim` adds the smallest task-board state-machine transition: a known team member can claim an open task only when dependencies are completed and any existing assignee matches the claimant.
- `team_event_list` exposes lifecycle, message, task, and failure events as the standalone event-feed readback path.
- Member `capabilities` and `permissions` are generic labels attached to team members; Teams core stores and returns them but does not enforce policy from them.
- `team_send` records either the lead or a member as sender and supports member or lead targets. Process-backed teammate traffic is routed through the on-disk team mailbox; native `spawn_agent` sessions are not Teams members.
- `team_member_stop` stops one teammate agent, marks only that member stopped, records a `MemberStopped` event, keeps the team active/readable, rejects later sends from/to that stopped member, and rejects task claims by that stopped member.
- `team_stop` is a mutating lifecycle transition. It stops active teammate agents and marks the team stopped once; repeated `team_stop` calls against an already stopped team are rejected while readback paths remain available.
- Model-callable tools are `create_team`, `list_teams`, `team_status`, `team_spawn_member`, `team_send`, `team_task_create`, `team_task_update`, `team_task_claim`, `team_task_list`, `team_event_list`, `team_member_stop`, and `team_stop`.
- `/teams` is a manual TUI guidance surface only for this slice; it does not perform list/status/send/stop actions directly.

## Out Of Scope For First Slice

- nested teams
- teammate spawning teammates
- persistent resume across process restarts
- policy-specific task workflows
- reviewer-specific blockers
- Darwin incident processing
- automatic code-review policy
- broad role marketplace

## Acceptance Criteria

- [x] Core has explicit Team, Member, Message, Task, and TeamEvent substrate types or equivalent clearly named boundaries.
- [x] Natural-language model path can create and inspect a team through tool exposure, not only slash commands.
- [x] Natural-language model path can create/claim/update/list generic team tasks and read the event feed through model-callable tools.
- [x] TUI has a documented first subset: `/teams` renders manual guidance only.
- [x] First slice uses the on-disk mailbox for process-backed teammates and keeps native `spawn_agent` delegation outside the Teams teammate path.
- [x] Team core does not hardcode reviewer, PASS/BLOCKERS, or Darwin policy.
- [x] Tests cover the first substrate path.
- [x] TUI-visible changes include snapshot coverage when rendering changes.

## Technical Notes

- Core multi-agent exposure is in `codex-rs/core/src/tools/spec.rs`.
- V2 message delivery distinguishes queue-only versus triggered turns.
- Session mailbox state exists in `codex-rs/core/src/session/input_queue.rs`.
- Agent lifecycle primitives exist in `codex-rs/core/src/agent/control.rs`.
- TUI slash command definitions live in `codex-rs/tui/src/slash_command.rs`.
- Trellis context for this task is in `implement.jsonl` and `check.jsonl`.

## Validation Evidence

Current validation and packaging proof from 2026-06-07:

- `cd codex-rs && just fmt` passed; only the existing ruff `exclude-newer = "7 days"` warning was emitted.
- `git diff --check` passed.
- Focused CodeMode regression proof passed:
  `just test -p codex-core code_mode_only_guides_all_tools_search_and_calls_deferred_app_tools`.
- Focused tool-routing proof passed:
  `just test -p codex-core code_mode_only_can_expose_namespaced_multi_agent_v2_as_normal_tools hosted_tools_follow_provider_auth_model_and_config_gates code_mode_only_restricts_prompt_tools code_mode_only_guides_all_tools_search_and_calls_deferred_app_tools shell_tools_run_in_parallel mixed_parallel_tools_run_in_parallel`.
- Full `cd codex-rs && just test` completed with `10381 tests run: 10379 passed, 2 failed, 23 skipped`; the two failures were timing flakes in `shell_tools_run_in_parallel` and `mixed_parallel_tools_run_in_parallel`.
- Focused retry for the full-suite failures passed:
  `just test -p codex-core shell_tools_run_in_parallel mixed_parallel_tools_run_in_parallel`.
- The additional flaky `snapshot_rollback_past_compaction_replays_append_only_history` also passed on focused retry.
- Installed the current dirty worktree into the user command:
  `cargo install --path cli --bin codex --locked --force --root /Users/snakesammy/.cargo`.
- `command -v codex` resolved to `/Users/snakesammy/.cargo/bin/codex`.
- `/Users/snakesammy/.cargo/bin/codex --version` returned `codex-cli 0.0.0`.
- `codex --help` rendered the expected CLI command surface.
- `codex doctor --summary --ascii` returned `15 ok | 1 idle | 4 notes | 2 warn | 0 fail degraded`.
- Backed up the previous installed command at `/Users/snakesammy/.cargo/bin/codex.backup.20260607-204823`.
- Ran `cargo clean` in `codex-rs`, removing `295398 files, 89.6GiB total`.
- Verified `codex-rs/target` was removed and `/System/Volumes/Data` free space recovered to about `90Gi`.
- Verified `/Users/snakesammy/.cargo/bin/codex` still existed and remained executable after cleanup.

Final local proof from 2026-05-29:

- `cd codex-rs && just fmt` passed.
- `cargo test -p codex-core team --locked` passed: 6 passed.
- `cargo test -p codex-core test_build_specs_collab_tools_enabled --locked` passed: 1 passed.
- `cargo test -p codex-tui teams --locked` passed: 2 passed.
- `just fix -p codex-core` passed after refactoring `TeamRegistry::send_to_member` to `SendTeamMessageRequest`.
- `python3 ./.trellis/scripts/task.py validate 05-28-codex-teams-infrastructure` passed.
- `git diff --check` passed.
- `rg -n "PASS|BLOCKERS|Darwin|tmux|reviewer" codex-rs/core/src/team.rs codex-rs/core/src/tools/handlers/team.rs codex-rs/core/src/tools/spec.rs codex-rs/tui/src/chatwidget.rs codex-rs/tui/src/slash_command.rs` returned no matches.
- `find codex-rs -name '*.snap.new' -o -name '*.pending-snap'` returned no pending snapshot files.
- `cargo install cargo-insta` installed `cargo-insta 1.47.2`; `cargo insta pending-snapshots -p codex-tui` is unsupported in this installed CLI version (`unexpected argument '-p'`), and the supported `--manifest-path tui/Cargo.toml` form was terminated after hanging in `cargo metadata`.

Spawn context envelope proof from 2026-05-29:

- `cd codex-rs && just fmt` passed.
- `cargo test -p codex-core spawn_send_status_and_stop_use_agent_control --locked` passed: 1 passed.
- `cargo test -p codex-core team_tool_chain_creates_spawns_sends_statuses_and_stops --locked` passed: 1 passed.
- `cargo test -p codex-core team --locked` passed: 6 passed.
- `cargo test -p codex-core test_build_specs_collab_tools_enabled --locked` passed: 1 passed.
- `just fix -p codex-core` passed.
- `python3 ./.trellis/scripts/task.py validate 05-28-codex-teams-infrastructure` passed.
- `git diff --check` passed.
- `rg -n "PASS|BLOCKERS|Darwin|tmux|reviewer" codex-rs/core/src/team.rs codex-rs/core/src/tools/handlers/team.rs codex-rs/core/src/tools/spec.rs` returned no matches.

Shared team registry proof from 2026-05-29:

- `cd codex-rs && just fmt` passed.
- `cargo test -p codex-core agent_control_uses_thread_manager_team_registry --locked` passed: 1 passed.
- `cargo test -p codex-core team_tool_chain_creates_spawns_sends_statuses_and_stops --locked` passed: 1 passed.
- `cargo test -p codex-core team --locked` passed: 8 passed.
- `cargo test -p codex-core thread_manager --locked` passed: 3 passed.
- `cargo test -p codex-core test_build_specs_collab_tools_enabled --locked` passed: 1 passed.
- `just fix -p codex-core` passed.
- `python3 ./.trellis/scripts/task.py validate 05-28-codex-teams-infrastructure` passed.
- `git diff --check` passed.
- `rg -n "PASS|BLOCKERS|Darwin|tmux|reviewer" codex-rs/core/src/team.rs codex-rs/core/src/tools/handlers/team.rs codex-rs/core/src/tools/spec.rs codex-rs/core/src/agent/control.rs codex-rs/core/src/thread_manager.rs codex-rs/core/src/codex.rs` returned no matches.

Lead mailbox target proof from 2026-05-29:

- `cd codex-rs && just fmt` passed.
- `cargo test -p codex-core spawn_send_status_and_stop_use_agent_control --locked` passed: 1 passed.
- `cargo test -p codex-core team_tool_chain_creates_spawns_sends_statuses_and_stops --locked` passed: 1 passed.
- `cargo test -p codex-core team --locked` passed: 8 passed.
- `cargo test -p codex-core test_build_specs_collab_tools_enabled --locked` passed: 1 passed.
- `just fix -p codex-core` passed.
- `python3 ./.trellis/scripts/task.py validate 05-28-codex-teams-infrastructure` passed.
- `git diff --check` passed.
- `rg -n "PASS|BLOCKERS|Darwin|tmux|reviewer" codex-rs/core/src/team.rs codex-rs/core/src/tools/handlers/team.rs codex-rs/core/src/tools/spec.rs` returned no matches.

Member stop lifecycle proof from 2026-05-29:

- `cd codex-rs && just fmt` passed.
- `cargo test -p codex-core stop_member_keeps_team_active_and_blocks_member_mutations --locked` passed: 1 passed.
- `cargo test -p codex-core team_tool_chain_creates_spawns_sends_statuses_and_stops --locked` passed: 1 passed.
- `cargo test -p codex-core team --locked` passed: 9 passed.
- `cargo test -p codex-core test_build_specs_collab_tools_enabled --locked` passed: 1 passed.
- `cargo test -p codex-core team_tools_have_exact_specs_and_handlers_when_collab_is_enabled --locked` passed: 1 passed.
- `just fix -p codex-core` passed.
- `python3 ./.trellis/scripts/task.py validate 05-28-codex-teams-infrastructure` passed.
- `git diff --check` passed.
- `rg -n "PASS|BLOCKERS|Darwin|tmux|reviewer" codex-rs/core/src/team.rs codex-rs/core/src/tools/handlers/team.rs codex-rs/core/src/tools/spec.rs` returned no matches.

Stopped team mutation guard proof from 2026-05-29:

- `cd codex-rs && just fmt` passed.
- `cargo test -p codex-core spawn_send_status_and_stop_use_agent_control --locked` passed: 1 passed.
- `cargo test -p codex-core team_tool_chain_creates_spawns_sends_statuses_and_stops --locked` passed: 1 passed.
- `cargo test -p codex-core team --locked` passed: 9 passed.
- `just fix -p codex-core` passed.
- `python3 ./.trellis/scripts/task.py validate 05-28-codex-teams-infrastructure` passed.
- `git diff --check` passed.
- `rg -n "PASS|BLOCKERS|Darwin|tmux|reviewer" codex-rs/core/src/team.rs codex-rs/core/src/tools/handlers/team.rs codex-rs/core/src/tools/spec.rs` returned no matches.

Task claim state-machine proof from 2026-05-29:

- `cd codex-rs && just fmt` passed.
- `cargo test -p codex-core task_claim_respects_dependency_and_assignment_boundaries --locked` passed: 1 passed.
- `cargo test -p codex-core team_tool_chain_creates_spawns_sends_statuses_and_stops --locked` passed: 1 passed.
- `cargo test -p codex-core team --locked` passed: 7 passed.
- `cargo test -p codex-core test_build_specs_collab_tools_enabled --locked` passed: 1 passed.
- `just fix -p codex-core` passed.
- `python3 ./.trellis/scripts/task.py validate 05-28-codex-teams-infrastructure` passed.
- `git diff --check` passed.
- `rg -n "PASS|BLOCKERS|Darwin|tmux|reviewer" codex-rs/core/src/team.rs codex-rs/core/src/tools/handlers/team.rs codex-rs/core/src/tools/spec.rs` returned no matches.

## Definition Of Done For This Planning Slice

- Trellis is initialized in the forked worktree.
- This PRD records Teams as infrastructure, not a policy workflow.
- The implementation/check manifests point to Teams-specific spec and research files.
- A git commit captures the fork/Trellis/task baseline before code implementation starts.
