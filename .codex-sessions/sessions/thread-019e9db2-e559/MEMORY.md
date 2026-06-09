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

- Current final installed binary: `/Users/snakesammy/.cargo/bin/codex` SHA256 `867e53742349115278c98a02ddc1be7a62c15cc42cc8ef99bc745b545200cc23`.
- Current final package archive: `dist/local/teams-slice5/codex-package-aarch64-apple-darwin.tar.gz` SHA256 `46ba05557045de9fef003e3e9a68f04a3d8525896501aa6b0e45c914a06e942a`.
- Earlier memory entry with installed hash `e113c6f71bae0fcacdb5fa2bfc3b8a467a8c2e3eff3c9d17c5b47ac33a6da816` belongs to a prior linked build, not the final package installed at the end of this task.

## Corrections And Invalidated Beliefs

- Invalidated: preliminary Teams implementation is “good enough” after tests/build. Runtime screenshots show it is not functionally or visually faithful.
- Invalidated: displaying `Codex Teams context` and skill hook output as normal teammate transcript is acceptable. User says this does not match Claude Code.
- Invalidated: strict textual copying of every Claude search hint is safer. For Codex BM25, copying `multi-agent` regresses the user's reported bug where explicit native subagent requests open Teams/split panes.
- Invalidated: the latest header/statusbar patch is complete after formatting. It has not passed focused TUI tests; validation was blocked by a `webrtc-sys` artifact download failure before code compile.
- Invalidated: durable logging is optional. User explicitly asked to record current work log and issue state with `$session-memory`.
- Invalidated: writing visible teammate launch context as a normal user prompt is acceptable. User compared it to Claude Code and rejected the visible `Codex Teams context:` envelope.
- Invalidated: adding broad Teams search anchors such as `working together` is safe. Current handoff records a focused regression where query `delegate work to a subagent` loads Teams tools because the BM25 anchor contains `working`; remove `work`/`working`-like anchors before further validation.
- Invalidated: forwarding `CODEX_HOME` alone is enough for process-backed teammates. If the lead is using env-style auth, the teammate can appear unauthenticated and open the login screen unless Codex auth env entrypoints are forwarded and honored in teammate mode.

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
