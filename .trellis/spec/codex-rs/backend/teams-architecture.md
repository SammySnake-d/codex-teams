# Teams Architecture Notes

## Core Boundary

Codex Teams must be implemented as collaboration infrastructure, not as a hardcoded reviewer workflow.

The substrate owns:

- `Team`: id, name, lead thread, member registry, status, created/updated timestamps.
- `Member`: name, thread id, agent path, role/capability profile, status, permissions, last activity.
- `Message`: sender, target, delivery mode, content, timestamp, delivery status.
- `Task`: title, assignee, status, dependencies, and a generic note.
- `TeamEvent`: append-only observable events for lifecycle, messages, task updates, and failures.

Policy layers own:

- reviewer read-only behavior
- PASS/BLOCKERS contracts
- Darwin incident handling
- planner/executor/reviewer templates
- domain-specific team recipes

## Natural-Language Requirement

Users must be able to say a request such as:

```text
Create a Codex Teams workspace with 3 named Teams teammates to investigate this task in parallel.
```

Generic requests for subagents, agents in parallel, implementation/check lanes,
or read-only audit lanes should use native `spawn_agent`/subagent tools unless
the user explicitly says those agents should be Codex Teams teammates.

That means Teams actions need model-callable tools in core. `/teams` can exist, but only as a manual control surface over the same substrate.

## Claude-Style Trigger Routing Decision

The current Codex Teams port must follow Claude Code's trigger architecture
where it matters, but without replacing Codex's whole tool-search substrate in
this slice.

Source-backed Claude behavior:

- Claude `AgentTool.searchHint` is `delegate work to a subagent`.
- Claude teammate spawn is not a separately searched spawn tool. It is the
  normal `AgentTool` branch when `resolveTeamName(...)` returns a team name and
  `name` is present.
- Claude `TeamCreate` remains a dedicated Teams tool with a Teams/swarm search
  hint.
- Claude `SendMessage` remains a dedicated Teams communication tool.
- Claude `ToolSearchTool` uses its own keyword scoring and exact selection
  design; Codex currently uses a global BM25 `SearchEngine(Language::English)`.

Minimal-change Codex decision:

- Keep the single global Codex BM25 `ToolSearch` substrate for now.
- Do not add a second Teams-only search path. A duplicate search layer would
  create two competing routing systems and would still need integration with
  Codex's deferred tool loading.
- Do not solve native subagent isolation with production user-language
  classifiers in `tool_search.rs`.
- Make `spawn_agent` the shared spawn surface: without explicit Teams structure
  it stays the native Codex subagent tool; only explicit `team_name` plus
  `name` enters the Teams teammate branch.
- Treat `team_spawn_member` as compatibility or exact Teams-control surface,
  not as the primary natural-language route for teammate creation.
- `team_spawn_member` search exposure must remain exact-name oriented. Teams
  natural-language terms such as teammate/swarm/团队/队友 should load team
  creation/status/message surfaces and let the shared `spawn_agent` tool take
  the teammate branch only when the model provides both `team_name` and `name`.
- Claude-compatible alias tools such as `TeamCreate`, `SendMessage`,
  `TaskCreate`, `TaskUpdate`, `TaskList`, and `TaskGet` may exist as Teams API
  compatibility surfaces, but they must not become a second spawn route or
  blur native Codex subagents with Teams teammates.
- Keep Teams search hints narrow. They may use Teams-specific terms such as
  `team`, `teams`, `teammate`, `swarm`, `roster`, `mailbox`, `task board`,
  `团队`, and `队友`, but must avoid generic native-subagent collision terms
  such as `agent`, `subagent`, `work`, `parallel`, and broad delegation phrases.

Required invariant:

- A user request that says `subagent`, `spawn_agent`, or ordinary delegation
  must load and execute the native Codex subagent path unless the model provides
  the Teams teammate structure (`team_name` plus `name`). Enabling
  `features.teams` must not turn native subagents into split-pane teammates.
- In the Codex `spawn_agent` schema, `task_name` is the native Codex subagent
  identity. If `task_name` is present, the call must remain on the native
  subagent path even when a single active Teams workspace exists and `name` is
  also present. This preserves ordinary Codex agent naming from being captured
  by Teams active-state inference.
- The runtime teammate binary must be the real `codex` CLI, not a Cargo
  `target/debug/deps/*` test harness. Teammate spawn resolves
  `CODEX_TEAMMATE_COMMAND`, then configured `codex_self_exe`, then `current_exe`,
  and must escape a deps test binary to the sibling real `target/debug/codex`
  when available.
- Process-backed teammates must use the lead's resolved `CODEX_HOME` so
  `config.toml` remains the provider source of truth. Runtime smoke tests that
  need a mock provider should write that provider into a temporary
  `CODEX_HOME/config.toml`; they must not rely on session-only `-c
  model_provider=...` overrides being forwarded into the teammate process.

## Minimal Vertical Slice

First implementation should prove:

1. Create a team.
2. Spawn one teammate as an independent agent session.
3. Send lead-to-member, member-to-member, and member-to-lead messages.
4. List team status and member status.
5. Create, claim, update, and list generic task-board items.
6. List the team event feed.
7. Stop one teammate while the team remains active.
8. Stop the team cleanly.

Do not start with reviewer policy or Darwin feedback loops. Process-backed
tmux/iTerm teammate panes are now part of the explicit Teams display path. Native
in-process `spawn_agent` sessions stay on the subagent path and must not be
registered as Teams teammates.

## Scenario: Model-Callable Teams Substrate

### 1. Scope / Trigger

- Trigger: Teams adds model-callable core tools that create and mutate collaboration state across agent lifecycle, message routing, task-board state, and event readback.
- Scope: `codex-core` owns the live-session-only registry and function tools. The TUI `/teams` surface remains manual guidance over the same substrate.
- Registry lifetime: the lead-side live `TeamRegistry` is scoped to the owning `ThreadManager`. Process-backed teammates join the same team through the on-disk team store and mailbox; native `spawn_agent` sessions do not become Teams teammates.

### 2. Signatures

- `create_team(name)` -> creates a live-session-only `Team`.
- `list_teams()` -> returns visible teams, including stopped teams.
- `team_status(team_id)` -> returns `TeamSnapshot { team, messages, events }` after refreshing member agent statuses.
- `team_spawn_member(team_id, name, profile?, capabilities?, permissions?, message?/items?)` -> spawns one named Teams teammate as a process-backed `codex teammate` in a tmux/iTerm pane, with the generic Teams context envelope plus first prompt delivered through the on-disk mailbox. If no pane backend is available, fail closed instead of falling back to native `spawn_agent`.
- `team_send(team_id, target?, member_id?, sender_member_id?, delivery_mode?, message?/items?)` -> routes process-backed teammate traffic through the on-disk mailbox.
- `team_task_create(team_id, title, assignee_member_id?, dependencies?, note?)` -> creates one generic shared task-board item.
- `team_task_update(team_id, task_id, title?, assignee_member_id?, dependencies?, status?, note?)` -> updates one generic shared task-board item.
- `team_task_claim(team_id, task_id, member_id)` -> claims one open shared task-board item for a team member after dependency and assignee checks.
- `team_task_list(team_id)` -> lists a team's shared task-board items.
- `team_event_list(team_id)` -> lists a team's append-only lifecycle, message, task, and failure events.
- `team_member_stop(team_id, member_id)` -> stops one teammate agent and marks only that member stopped while keeping the team active.
- `team_stop(team_id)` -> shuts down active teammate agents and marks the team stopped.

### 3. Contracts

- `Team.live_session_only` must be `true` until persistent resume is explicitly implemented.
- `TeamRegistry` must remain the lead-side live state authority, while process-backed teammates use the on-disk team store and mailbox as their cross-process authority. Per-session registries alone would make teammate-originated team tools unable to see the lead-created team.
- Stopped teams remain readable through list/status/task/event readback, but mutating paths must reject them.
- Member `capabilities` and `permissions` are generic labels. Teams core stores and returns them but does not enforce policy from them.
- `team_spawn_member.message` or non-empty `items` is required because teammates do not inherit the lead conversation history. Teams core must prepend only generic identity and coordination context; it must not add reviewer, PASS/BLOCKERS, Darwin, or role-specific workflow policy. Display mechanics such as tmux/iTerm panes belong to the Teams display adapter, not to policy-specific prompt context.
- Process-backed teammate launch must explicitly inherit `CODEX_HOME`, `CODEX_TEAMMATE`, proxy/cert env vars, the current model provider `env_key`, and env vars referenced by `env_http_headers`. Do not rely on the tmux/iTerm child shell having the same provider auth environment as the lead process.
- `team_send.target` defaults to `member`; member targets require `member_id`, while lead targets must omit `member_id`. Process-backed sends are recorded in the shared mailbox/event feed.
- `team_send.delivery_mode` defaults to `queue`; `interrupt` must be rejected for lead targets and process-mailbox targets.
- `team_send.sender_member_id` is optional. Omit it for a lead-originated message; provide a member id only when that member belongs to the same team.
- `team_member_stop` requires an active team and a known member. It must be idempotent for an already stopped member, must not stop the team, and must not shut down other active members.
- Stopped members remain visible in snapshots and event readback, but member-targeted sends from/to stopped members and task claims by stopped members must be rejected.
- `team_stop` requires an active team. Once stopped, the team remains visible through readback paths, but repeated `team_stop` calls must be rejected as stopped-team mutations.
- Task dependencies are task ids from the same team. A task must not depend on itself.
- `team_task_claim` requires an active team, known member, open task status, completed dependencies, and either no assignee or the same assignee as the claiming member.
- Task `note` is generic metadata for the shared task board. It must not become a policy-specific result, blocker, review verdict, or workflow template field in Teams core.
- `TeamEvent::Failure` records team-tool operation errors so failed tool calls remain observable in the event feed.

### 4. Validation & Error Matrix

- Invalid team id -> `ThreadNotFound` mapped to a model-readable team-resource-not-found error.
- Mutating a stopped team -> `UnsupportedOperation`.
- Empty team/member/task/message labels -> model-readable validation error before registry mutation.
- Unknown member assignee or sender -> `ThreadNotFound`.
- Send to/from stopped member or claim by stopped member -> `UnsupportedOperation`.
- Unknown task dependency -> `ThreadNotFound`.
- Self-dependency on task update -> `UnsupportedOperation`.
- Unsupported task status or delivery mode -> model-readable validation error listing supported values.
- Spawn depth overflow -> model-readable depth-limit error; do not bypass existing multi-agent depth controls.

### 5. Good/Base/Bad Cases

- Good: create a team, spawn members with generic labels, send messages, create/claim/update/list tasks, list events, inspect status, and stop the team.
- Base: create a team with no members or tasks; status still returns an explicit live-session-only snapshot.
- Bad: encode reviewer, PASS/BLOCKERS, Darwin, or role-marketplace behavior in core team types or tools. Split-pane process launch is allowed only as a Teams display adapter over the generic substrate, not as a policy workflow.

### 6. Tests Required

- Core registry tests must cover create/list, spawn/send/status/stop, task create/claim/update/list, and task/event visibility.
- Tool-handler tests must cover the natural-language tool path: spec args parse into registry operations and return JSON outputs.
- Tool-spec tests must prove the collab feature exposes the exact Teams tool set and handler registrations.
- TUI tests must keep `/teams` snapshot coverage limited to manual guidance until a later UI action surface is intentionally added.

### 7. Wrong vs Correct

#### Wrong

Add a `reviewer` team mode that creates hardcoded blocker statuses and display panes in Teams core.

#### Correct

Keep Teams core as live collaboration state plus generic tools. Attach reviewer workflows and domain templates later as policy layers over `Team`, `Member`, `Message`, `Task`, and `TeamEvent`; attach tmux/iTerm panes as display adapters over that same substrate.

## Display Boundary

Process-backed tmux/iTerm teammate panes are the preferred Teams display path
when a pane backend is available. If no pane backend is available,
`team_spawn_member` fails closed instead of creating a native in-process
subagent, so Teams teammate state stays separate from native subagent
navigation.

All display modes must remain adapters over the same team state and event feed.

## Persistence Boundary

If persistent resume is not in the first slice, team state must clearly report that it is live-session-only. Stale team state must not be presented as resumable.
