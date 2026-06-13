# Memory

## Stable Preferences

- User requires Claude Code Teams fidelity for this task and repeatedly rejected generic Codex-native approximations.
- User requires checking `https://github.com/ChinaSiro/claude-code-sourcemap` before implementing Teams/subagent behavior.
- User wants Teams and subagent responsibilities strictly separated: generic subagents must not trigger Teams split panes; Teams UI/tools should only activate through explicit Teams surface.
- User wants `/teams` command implemented so Teams feature presence is visible in the TUI.
- User prefers many read-only subagents for parallel evidence gathering, but controller must own merge/edit decisions.
- User requires validation before packaging/linking to `codex`, because Cargo builds are expensive.
- User wants work logs and issue records kept current when Teams work stalls or pivots, rather than continuing unbounded implementation while calling the feature complete.

## Working Model

- Treat current Codex Teams work as a Claude Code fidelity port, not a fresh product design.
- Use Karpathy/minimal-change constraint: copy the proven source behavior where possible, but keep impact on existing Codex features small.
- Prompt/context injected into teammates should not visibly pollute the teammate prompt transcript unless Claude Code does so.
- TUI work must include lead-side teammate statusbar, picker navigation, pane switching, and reply routing parity, not only backend tools.

## Decisions This Session Believes

- Before further edits, compare against `claude-code-sourcemap` for Teams prompt, tools, slash commands, TUI roster/statusbar, and subagent separation.
- Do not expand into a generic agent navigation layer unless Claude Code evidence demands it.
- Claude Code's `TeamCreate.searchHint = "create a multi-agent swarm team"` must not be copied literally into Codex BM25 search text, because Codex performs tool-search routing before the Claude-style `Agent(team_name && name)` branch exists. Codex Teams search hints must avoid generic `agent` tokens and reserve Teams for explicit `team`/`teams`/`teammate`/`swarm` terms.
- Source-fidelity means semantic parity plus architecture adapters. Claude uses one `Agent` tool for both subagents and teammates, split by `team_name && name`; Codex keeps native `spawn_agent` separate and exposes explicit Teams tools such as `create_team`, `team_spawn_member`, and `team_send`.
- For future Teams packages, do not link the user-facing `codex` command until the newly built binary has passed focused tests plus runtime smoke on the actual installed command. This session's final installed proof used `/teams` in the real TUI, not only unit tests or completion popup checks.
- When user says `subagent`, the expected behavior is native Codex subagent delegation unless the prompt explicitly asks for Teams/team/teammate/split-pane product surface. Enabling `features.teams` must not make generic subagent requests disappear into Teams.
- Teammate process launch must inherit Codex auth env entrypoints (`CODEX_API_KEY`, `CODEX_ACCESS_TOKEN`, and compatible provider env) from the lead. The TUI embedded app-server may honor `CODEX_API_KEY` for teammate-mode startup, but ordinary TUI startup should keep its existing default unless separately justified.

## Final Artifact Evidence

- Current final user-facing command: `/Users/snakesammy/.cargo/bin/codex` is a wrapper that execs `/Users/snakesammy/Desktop/project/codex-teams/codex-rs/target/debug/codex`; wrapper SHA256 `54f22aa5653b085c41c85d686337ede33ecf84a7dc8b386d3c6018b6a23ff0fb`.
- Current final debug binary: `codex-rs/target/debug/codex` SHA256 `573ea1c034ebcd7da536080c85e327ec819a998428a31ab3c2e76f6cb8dbb510`.
- Current final package archive: `dist/local/teams-final/codex-package-aarch64-apple-darwin.tar.gz` SHA256 `44ff75303baf9ba6c9801dad8b8428e584b3b8ce12afe8ec1339b59b4ba4b70e`.
- Packaged binary smoke passed after extracting the archive: `bin/codex --version` and `bin/codex teammate --help` exposed the hidden teammate flags.
- Latest final interactive visual audit evidence: `/tmp/codex-teams-visual-audit2.4cS0hD`; lead showed `TEAMS_SMOKE_PASS ... member_to_lead_completed=true`, teammate showed `TEAMS_SMOKE_MEMBER_DONE member_to_lead_sent=true`, and bad-marker grep found no visible legacy Teams context, Test API Key, official OpenAI URL, target/debug/deps, unrecognized `--agent-id`, or smoke failure marker.
- Earlier memory entries with installed hash `867e53742349115278c98a02ddc1be7a62c15cc42cc8ef99bc745b545200cc23` or archive `dist/local/teams-slice5/...` belong to prior slices, not the final `teams-final` package checkpoint.

## Corrections And Invalidated Beliefs

- Invalidated: preliminary Teams implementation is “good enough” after tests/build. Runtime screenshots show it is not functionally or visually faithful.
- Invalidated: displaying `Codex Teams context` and skill hook output as normal teammate transcript is acceptable. User says this does not match Claude Code.
- Invalidated: strict textual copying of every Claude search hint is safer. For Codex BM25, copying `multi-agent` regresses the user's reported bug where explicit native subagent requests open Teams/split panes.
- Invalidated: the latest header/statusbar patch is complete after formatting. It has not passed focused TUI tests; validation was blocked by a `webrtc-sys` artifact download failure before code compile.
- Invalidated: durable logging is optional. User explicitly asked to record current work log and issue state with `$session-memory`.
- Invalidated: writing visible teammate launch context as a normal user prompt is acceptable. User compared it to Claude Code and rejected the visible `Codex Teams context:` envelope.
- Invalidated: adding broad Teams search anchors such as `working together` is safe. Current handoff records a focused regression where query `delegate work to a subagent` loads Teams tools because the BM25 anchor contains `working`; remove `work`/`working`-like anchors before further validation.
- Invalidated: forwarding `CODEX_HOME` alone is enough for process-backed teammates. If the lead is using env-style auth, the teammate can appear unauthenticated and open the login screen unless Codex auth env entrypoints are forwarded and honored in teammate mode.
- Pitfall: do not describe every bad teammate provider/base-url screenshot as "teammate ignored config.toml." If the pane warning points at `/private/var/folders/.../.tmp*/config.toml` or `/private/tmp/.../home/config.toml`, the process is reading a temporary `CODEX_HOME`; the child is likely inheriting the wrong lead runtime/home, not ignoring `/Users/snakesammy/.codex/config.toml`.
- Pitfall: exiting/restarting only fixes teammate config drift when the fresh lead is started from the verified wrapper/current binary with real home, for example `CODEX_HOME=/Users/snakesammy/.codex /Users/snakesammy/.cargo/bin/codex --enable teams`. Restarting into another temp-home harness preserves the bug.
- Pitfall: a running lead process does not hot-reload a rebuilt `target/debug/codex`, `CODEX_HOME`, or provider/auth state into already-open Teams panes. If teammate screenshots still show official OpenAI URL, `Test API Key`, or a temp `config.toml`, first restart the lead from the current wrapper with real home and then reproduce; do not patch provider/auth code from a stale-pane screenshot alone.
- Pitfall evidence 2026-06-12 20:29 CST: active lead `PID 85348` started at `17:46:57`, before current `target/debug/codex` mtime `20:12:17`. A fresh current-wrapper smoke with provider only in `CODEX_HOME/config.toml` passed `TEAMS_SMOKE_PASS` and all requests hit the configured loopback provider, so this failure mode is stale running lead, not current source ignoring config.

## Latest Source-Backed Findings

- Local Claude source mirror used for the latest comparison: `/tmp/claude-code-sourcemap-codex-teams`, commit `a8a678cb6244e6770e1e421767ff0987a1d95549`.
- Claude Code keeps `TeamCreate` as its own Teams tool.
- Claude Code spawns a teammate through `AgentTool` only when `teamName && name` are present; plain subagents stay on the normal `AgentTool` path.
- Claude Code delivers the first teammate task as mailbox/plain prompt text, not as visible `Codex Teams context:` transcript content.
- Claude Code places the teammate communication rule in a teammate system/tool prompt addendum: use `SendMessage` for teammate communication.

## Hypotheses / Verify Before Use

- Hypothesis: teammate context should be passed as hidden/system-like initialization rather than user-visible prompt text.
- Hypothesis: `/teams` should be a first-class slash command distinct from native subagent controls.
- Hypothesis: lead footer/statusbar needs Teams-only state and should not modify ordinary `/agent` picker behavior.
- Hypothesis: the in-progress `TeamTeammateViewHeader` plumbing is the right minimal Slice 3 seam, but all `prompt` propagation call sites must be rechecked before validation.
