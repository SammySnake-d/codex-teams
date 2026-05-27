# Teams Architecture Notes

## Core Boundary

Codex Teams must be implemented as collaboration infrastructure, not as a hardcoded reviewer workflow.

The substrate owns:

- `Team`: id, name, lead thread, member registry, status, created/updated timestamps.
- `Member`: name, thread id, agent path, role/capability profile, status, permissions, last activity.
- `Message`: sender, target, delivery mode, content, timestamp, delivery status.
- `Task`: title, assignee, status, dependencies, result or blocker note.
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
3. Send a lead-to-member message.
4. List team status and member status.
5. Stop the team cleanly.

Do not start with tmux panes, reviewer policy, or Darwin feedback loops.

## Display Boundary

Initial display mode should be in-process TUI switching/watch output.

External display modes such as tmux or iTerm panes should be adapters over the same team state and event feed.

## Persistence Boundary

If persistent resume is not in the first slice, team state must clearly report that it is live-session-only. Stale team state must not be presented as resumable.
