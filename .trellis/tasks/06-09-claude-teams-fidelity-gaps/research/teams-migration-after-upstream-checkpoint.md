# Research: Teams migration after upstream checkpoint

- Query: After the latest recorded upstream sync and checkpoint commit, what remains for the Claude Code Teams migration, and is a smoke failure caused by a missing pane backend already recorded?
- Scope: mixed
- Date: 2026-07-10

## Findings

### Latest recorded checkpoint

- The latest task checkpoint is `2026-06-25 06:16 CST`: upstream/main `24423f5712` was already merged into `feat/codex-teams-infra` as merge commit `98d5643ea`; the final record reports no pending source diff from the reverted footer false-start. See `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:580` and `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:582`.
- That checkpoint is `passed_not_packaged`, not an incomplete code/test checkpoint. It records focused TUI (55), core (11 plus 4 spawn-route), CLI (3), proxy (13), snapshot, format, and diff checks; it rebuilt `codex-rs/target/debug/codex`, then passed the config-backed smoke with `CODEX_TEAMMATE_COMMAND` pinned to that binary. See `.trellis/tasks/06-09-claude-teams-fidelity-gaps/check.jsonl:41` and `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:584`.
- The recorded smoke proof is `TEAMS_SMOKE_PASS` under `/tmp/codex-teams-post-24423-smoke.mCyhM4`, with scoped negative markers absent. It is a debug-binary proof, not a PATH-install proof. See `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:597`.

### What remains

- The only explicit completion gates left in the latest checkpoint are user-owned final visual validation and an explicitly approved package/link decision. PATH still resolved to `/opt/homebrew/bin/codex` (`codex-cli 0.142.0`), while the verified Teams artifact was `codex-rs/target/debug/codex`. See `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:605` and `.trellis/tasks/06-09-claude-teams-fidelity-gaps/check.jsonl:41`.
- Do not claim unbounded or pixel-perfect Claude parity. The current architecture deliberately preserves one Codex BM25 search substrate, keeps generic subagent requests native, and routes only named/active Teams requests through the shared `spawn_agent` teammate branch. See `.trellis/spec/codex-rs/backend/teams-architecture.md:57` and `.trellis/spec/codex-rs/backend/teams-architecture.md:81`.
- A newly reported visual or behavior difference should be opened as a new bounded Teams fidelity slice rather than re-opening the completed upstream compatibility/test/smoke sequence. The latest record says no Rust/TUI source diff remained after restoring the Claude count-based footer behavior. See `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:583`.

### Pane backend and smoke-failure status

- The missing-pane-backend behavior is intentionally implemented and documented: `team_spawn_member` launches a real teammate process in tmux, otherwise iTerm2, then returns a model-visible error rather than falling back to a native in-process subagent. See `codex-rs/core/src/tools/handlers/team.rs:1021`, `codex-rs/core/src/tools/handlers/team.rs:1087`, and `codex-rs/core/src/tools/handlers/team.rs:1152`.
- This fail-closed boundary is covered by native multi-agent tests. See `codex-rs/core/src/tools/handlers/multi_agents_tests.rs:1142` and `codex-rs/core/src/tools/handlers/multi_agents_tests.rs:1335`.
- It is also recorded as the architecture contract and port documentation, not as a passing fallback path. See `.trellis/spec/codex-rs/backend/teams-architecture.md:134`, `.trellis/spec/codex-rs/backend/teams-architecture.md:200`, and `codex-rs/docs/teams-port/06-tui-and-lead-poller.md:483`.
- No linked smoke failure caused by a missing pane backend was found in the active task records or workspace journal. The documented smoke failures after upstream work were an absent `OPENAI_API_KEY` for the deterministic provider and a false-negative broad marker grep; the final tmux-backed smoke passed. See `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:507`, `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:538`, and `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:597`.

### Files found

- `.trellis/tasks/06-09-claude-teams-fidelity-gaps/task.json` - task scope, latest accepted architecture, and historical proof boundary.
- `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md` - acceptance criteria and chronological upstream/smoke work log.
- `.trellis/tasks/06-09-claude-teams-fidelity-gaps/check.jsonl` - authoritative focused-test and smoke checkpoint records; latest is `post-upstream-24423-debug-live-proof`.
- `.trellis/tasks/06-09-claude-teams-fidelity-gaps/implement.jsonl` - earlier runtime diagnosis record; it predates the upstream checkpoint.
- `.trellis/spec/codex-rs/backend/teams-architecture.md` - durable routing, isolation, pane-backend, and entrypoint contracts.
- `codex-rs/core/src/tools/handlers/team.rs` - shared `spawn_agent` teammate branch and tmux/iTerm fail-closed spawn implementation.
- `codex-rs/core/src/tools/handlers/multi_agents_tests.rs` - missing-pane-backend assertion coverage.
- `codex-rs/tui/src/app/lead_inbox_poller.rs` - lead mailbox reply injection implementation.
- `codex-rs/tui/src/chatwidget/team_ui.rs` - Teams tool output to roster/navigation state handling.
- `codex-rs/tui/src/chatwidget/snapshots/codex_tui__chatwidget__tests__teammate_view_header_with_transcript_layout.snap` - snapshot containing `Viewing @alice · esc to return`.
- `codex-rs/docs/teams-port/06-tui-and-lead-poller.md` - port design record describing the process-pane/no-backend relationship.

### Code patterns

- Native versus teammate routing begins at `maybe_spawn_member_from_agent_tool`; no name returns to the native path, while a resolved Teams context invokes the shared Teams spawn code. See `codex-rs/core/src/tools/handlers/team.rs:915` and `codex-rs/core/src/tools/handlers/team.rs:923`.
- Process-backed spawn selects tmux first and iTerm2 second, returning pane metadata only for successful process launches. See `codex-rs/core/src/tools/handlers/team.rs:1024` and `codex-rs/core/src/tools/handlers/team.rs:1089`.
- The lead inbox poller is a cancellable background task, so member mailbox replies can become lead-side events. See `codex-rs/tui/src/app/lead_inbox_poller.rs:35` and `codex-rs/tui/src/app/lead_inbox_poller.rs:228`.
- The visible teammate-header parity contract is snapshot-covered. See `codex-rs/tui/src/chatwidget/snapshots/codex_tui__chatwidget__tests__teammate_view_header_with_transcript_layout.snap:5`.

### External references

- Claude source reference: `https://github.com/ChinaSiro/claude-code-sourcemap`; task source anchors include `PaneBackendExecutor.ts`, `TeammateViewHeader.tsx`, `TeamStatus.tsx`, and `useBackgroundTaskNavigation.ts`. See `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:27`.
- Upstream boundary: upstream/main `24423f5712`, merged locally as `98d5643ea`. See `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:582`.
- Validation dependency note: the upstream 1.42-era TUI test route used `v8 v149.2.0` release-artifact overrides; this is a validation-environment issue, not a Teams regression. See `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md:527` and compound card `1067`.

### Related specs

- `.trellis/spec/codex-rs/backend/index.md`
- `.trellis/spec/codex-rs/backend/teams-architecture.md`

### Recommended record locations

- Append any new live smoke result, including a real no-pane-backend failure, to `.trellis/tasks/06-09-claude-teams-fidelity-gaps/check.jsonl` with runtime evidence root, backend detection state, exact command/artifact, and outcome.
- Add a concise milestone or user-decision entry to the `## Current Work Log` in `.trellis/tasks/06-09-claude-teams-fidelity-gaps/prd.md` when manual validation or package/link is accepted.
- Update `.trellis/spec/codex-rs/backend/teams-architecture.md` only if the stable routing, pane-backend, or installed-entrypoint contract changes.

## Caveats / Not Found

- This is a record-based audit. No current upstream fetch, git inspection, build, or live smoke was run; upstream and PATH state may have drifted after the 2026-06-25 checkpoint.
- The local Claude source-mirror directories referenced by the PRD were not present during this audit, so source-fidelity claims rely on the task's existing source-anchor records rather than a fresh upstream source comparison.
- No task/workspace event was found that attributes a smoke failure to the absence of tmux/iTerm2. The condition exists as code/spec/test behavior; record a separate runtime event if that failure occurs.

## Follow-up live proof 2026-07-10 01:54 CST

After the record-only audit, the controller reproduced both sides of the pane-backend boundary:

- Non-pane linked smoke evidence root `/tmp/codex-teams-linked-24423-smoke.YaXCiK`: `create_team` succeeded, then `team_spawn_member` failed closed because the controller shell had no `TMUX` / `ITERM_SESSION_ID`.
- Tmux-backed linked smoke evidence root `/tmp/codex-teams-linked-tmux-24423-smoke.EEfqMN`: running the same current PATH `codex` shim from a temporary tmux session passed with `TEAMS_SMOKE_PASS team_id=019f4802-f2f2-7022-94b3-c29556a4cc2c member_id=019f4802-f321-7e53-a0e1-9d15ecd277f1 member_to_lead_completed=true status_output_bytes=964`.
- Scoped bad-marker check passed for the tmux smoke, and the team store showed `mock-member` as `backend=tmux` with `tmuxPaneId=%1` and plain first-task prompt text.

This confirms the earlier missing-pane result was environment readiness, not a Teams routing/auth/config regression.
