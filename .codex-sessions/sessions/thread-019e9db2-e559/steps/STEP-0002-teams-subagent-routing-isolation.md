# STEP-0002 Teams/Subagent Routing Isolation

status: focused-validation-passed-and-cache-cleaned
updated_at: 2026-06-09 06:08:00 CST

## Before

User reported that explicitly asking for `subagent` still opened Codex Teams/split panes when the Teams feature was enabled. A proposed patch attempted to solve this by adding language-specific keyword logic and examples such as `开启subagent`, `子代理`, `代理`, `智能体`, and `多智能体` into global tool search behavior/tests.

## Issue

That direction is wrong. Claude Code does not put Teams/subagent separation into a global `ToolSearchTool` classifier. Claude source keeps tool search generic and relies on tool-owned `searchHint`, schema shape, and runtime branching/gates:

- `ToolSearchTool` performs generic keyword scoring over tool names/descriptions/search hints.
- `AgentTool` advertises normal subagents with `searchHint: delegate work to a subagent`.
- Teams tools advertise Teams/swarm semantics separately.
- Teammate spawn is a gated branch, not a global keyword rewrite.

Codex must preserve the same separation semantically while adapting to Codex architecture: native subagents are `spawn_agent` / multi-agent tools, while Codex Teams are explicit Teams tools and feature/session-gated surfaces.

## Actions

- Rechecked local Claude source under `/tmp/claude-code-sourcemap/restored-src` for `ToolSearchTool`, `AgentTool`, `TeamCreateTool`, and `SendMessageTool`.
- Confirmed `codex-rs/core/src/tools/handlers/tool_search.rs` is currently generic BM25/coalescing infrastructure and should remain generic.
- Removed hardcoded Chinese routing phrases from the current Teams isolation tests.
- Confirmed with `rg` that the hardcoded examples and classifier markers are absent from `codex-rs/core/src` and `codex-rs/tui/src`.
- Ran `cd codex-rs && just fmt`; it passed with only existing ruff `exclude-newer = "7 days"` warnings.
- Started focused validation: `cd codex-rs && just test -p codex-core team_tool_search_info_is_explicit_teams_only teams_tool_search_requires_explicit_teams_terms_not_subagent`.
- Focused validation passed: 2 tests passed, 2716 skipped.
- After validation exited, ran `cargo clean` and removed `194644` files / `55.8GiB`.
- Disk availability after cleanup: `123GiB`.

## Decisions

- Do not edit `codex-rs/core/src/tools/handlers/tool_search.rs` for Teams/subagent split.
- Do not add production keyword classifiers for user-language phrases like `开启subagent`, `子代理`, `代理`, `智能体`, or `多智能体`.
- Keep Teams discovery on Teams-owned `search_info` and schemas.
- Keep native subagent discovery on native multi-agent tool search info.
- Keep runtime protection in `spec_plan.rs`: Teams requires `Feature::Teams` and excludes `SessionSource::SubAgent(_)`; teammate process exceptions are explicit.

## Evidence

- Claude source evidence: `/tmp/claude-code-sourcemap/restored-src/src/tools/ToolSearchTool/ToolSearchTool.ts` is generic keyword scoring.
- Claude source evidence: `/tmp/claude-code-sourcemap/restored-src/src/tools/AgentTool/AgentTool.tsx` uses normal subagent `searchHint: delegate work to a subagent`.
- Claude source evidence: `/tmp/claude-code-sourcemap/restored-src/src/tools/TeamCreateTool/TeamCreateTool.ts` uses Teams/swarm search hint in Claude's different routing architecture.
- Codex source evidence: `codex-rs/core/src/tools/spec_plan.rs` gates Teams behind `Feature::Teams` and excludes `SessionSource::SubAgent(_)`.
- Codex source evidence: `codex-rs/core/src/tools/handlers/tool_search.rs` remains generic and should not receive Teams-specific conditionals.
- Disk evidence: `codex-rs/target` was `43G`; filesystem had about `82GiB` available during this checkpoint.

## Current Validation

Completed:

```text
cd codex-rs && just test -p codex-core team_tool_search_info_is_explicit_teams_only teams_tool_search_requires_explicit_teams_terms_not_subagent
```

Result: 2 tests passed, 2716 skipped.

## Next

Continue next Teams parity slices only after rechecking Claude source and keeping native subagent routing separate. Do not package/link until focused proof plus runtime smoke pass again.

## Search Keys

teams subagent routing tool_search BM25 search_info claude-code-sourcemap split panes keyword classifier
