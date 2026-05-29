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
Create an agent team with 3 teammates to investigate this task in parallel.
```

That means Teams actions need model-callable tools in core. `/teams` can exist, but only as a manual control surface over the same substrate.

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

Do not start with tmux panes, reviewer policy, or Darwin feedback loops.

## Scenario: Model-Callable Teams Substrate

### 1. Scope / Trigger

- Trigger: Teams adds model-callable core tools that create and mutate collaboration state across agent lifecycle, message routing, task-board state, and event readback.
- Scope: `codex-core` owns the live-session-only registry and function tools. The TUI `/teams` surface remains manual guidance over the same substrate.
- Registry lifetime: the live `TeamRegistry` is scoped to the owning `ThreadManager` and carried by `AgentControl` into spawned Codex sessions, so lead and teammate tools read and mutate the same live team state.

### 2. Signatures

- `create_team(name)` -> creates a live-session-only `Team`.
- `list_teams()` -> returns visible teams, including stopped teams.
- `team_status(team_id)` -> returns `TeamSnapshot { team, messages, events }` after refreshing member agent statuses.
- `team_spawn_member(team_id, name, profile?, capabilities?, permissions?, message?/items?)` -> spawns one independent Codex agent session through `AgentControl` after prepending a generic Teams context envelope to the teammate's spawn prompt/items.
- `team_send(team_id, target?, member_id?, sender_member_id?, delivery_mode?, message?/items?)` -> submits input to an existing member agent when `target` is `member`, or records a message to the lead mailbox when `target` is `lead`.
- `team_task_create(team_id, title, assignee_member_id?, dependencies?, note?)` -> creates one generic shared task-board item.
- `team_task_update(team_id, task_id, title?, assignee_member_id?, dependencies?, status?, note?)` -> updates one generic shared task-board item.
- `team_task_claim(team_id, task_id, member_id)` -> claims one open shared task-board item for a team member after dependency and assignee checks.
- `team_task_list(team_id)` -> lists a team's shared task-board items.
- `team_event_list(team_id)` -> lists a team's append-only lifecycle, message, task, and failure events.
- `team_member_stop(team_id, member_id)` -> stops one teammate agent and marks only that member stopped while keeping the team active.
- `team_stop(team_id)` -> shuts down active teammate agents and marks the team stopped.

### 3. Contracts

- `Team.live_session_only` must be `true` until persistent resume is explicitly implemented.
- `TeamRegistry` must be shared across lead and spawned teammate sessions within the same `ThreadManager`; per-session registries would make teammate-originated team tools unable to see the lead-created team.
- Stopped teams remain readable through list/status/task/event readback, but mutating paths must reject them.
- Member `capabilities` and `permissions` are generic labels. Teams core stores and returns them but does not enforce policy from them.
- `team_spawn_member.message` or non-empty `items` is required because teammates do not inherit the lead conversation history. Teams core must prepend only generic identity and coordination context; it must not add reviewer, PASS/BLOCKERS, Darwin, tmux, or role-specific workflow policy.
- `team_send.target` defaults to `member`; member targets require `member_id`, while lead targets must omit `member_id` and are recorded in the shared mailbox/event feed without submitting input to an agent thread.
- `team_send.delivery_mode` defaults to `queue`; `interrupt` must call `AgentControl::interrupt_agent` before submitting input to member targets and must be rejected for lead targets.
- `team_send.sender_member_id` is optional. Omit it for a lead-originated message; provide a member id only when that member belongs to the same team.
- `team_member_stop` requires an active team and a known member. It must be idempotent for an already stopped member, must not stop the team, and must not shut down other active members.
- Stopped members remain visible in snapshots and event readback, but member-targeted sends from/to stopped members and task claims by stopped members must be rejected.
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
- Bad: encode reviewer, PASS/BLOCKERS, Darwin, split-pane, or role-marketplace behavior in core team types or tools.

### 6. Tests Required

- Core registry tests must cover create/list, spawn/send/status/stop, task create/claim/update/list, and task/event visibility.
- Tool-handler tests must cover the natural-language tool path: spec args parse into registry operations and return JSON outputs.
- Tool-spec tests must prove the collab feature exposes the exact Teams tool set and handler registrations.
- TUI tests must keep `/teams` snapshot coverage limited to manual guidance until a later UI action surface is intentionally added.

### 7. Wrong vs Correct

#### Wrong

Add a `reviewer` team mode that creates hardcoded blocker statuses and display panes in Teams core.

#### Correct

Keep Teams core as live collaboration state plus generic tools. Attach reviewer workflows, external panes, and domain templates later as adapters or policy layers over `Team`, `Member`, `Message`, `Task`, and `TeamEvent`.

## Display Boundary

Initial display mode should be in-process TUI switching/watch output.

External display modes such as tmux or iTerm panes should be adapters over the same team state and event feed.

## Persistence Boundary

If persistent resume is not in the first slice, team state must clearly report that it is live-session-only. Stale team state must not be presented as resumable.
