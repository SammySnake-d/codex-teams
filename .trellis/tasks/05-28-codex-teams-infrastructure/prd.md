# Codex Teams Infrastructure

## Goal

Build a Codex Teams foundation for the Rust TUI that lets a user create and operate a group of independent Codex agent sessions from natural language and from a `/teams` control surface.

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
- route a lead-to-member message
- list team/member status
- stop the team
- expose a minimal `/teams` manual control surface over the same core substrate

## Natural-Language Acceptance

A user prompt like this must be enough to drive the model toward Teams tools once implemented:

```text
Create an agent team with 3 teammates to investigate this task in parallel.
```

Slash command parity is also needed, but `/teams` is not the primary proof. Natural language requires model-exposed tools and usage hints.

## Out Of Scope For First Slice

- tmux/iTerm split-pane display mode
- nested teams
- teammate spawning teammates
- persistent resume across process restarts
- reviewer-specific blockers
- Darwin incident processing
- automatic code-review policy
- broad role marketplace

## Acceptance Criteria

- [ ] Core has explicit Team, Member, Message, Task, and TeamEvent substrate types or equivalent clearly named boundaries.
- [ ] Natural-language model path can create and inspect a team through tool exposure, not only slash commands.
- [ ] TUI has a minimal `/teams` control surface for list/status/send/stop or a documented first subset.
- [ ] First slice reuses existing AgentControl and mailbox primitives instead of duplicating agent lifecycle.
- [ ] Team core does not hardcode reviewer, PASS/BLOCKERS, or Darwin policy.
- [ ] Tests cover the first substrate path.
- [ ] TUI-visible changes include snapshot coverage when rendering changes.

## Technical Notes

- Core multi-agent exposure is in `codex-rs/core/src/tools/spec_plan.rs`.
- V2 message delivery distinguishes queue-only versus triggered turns.
- Session mailbox state exists in `codex-rs/core/src/session/input_queue.rs`.
- Agent lifecycle primitives exist in `codex-rs/core/src/agent/control.rs`.
- TUI slash command definitions live in `codex-rs/tui/src/slash_command.rs`.
- Trellis context for this task is in `implement.jsonl` and `check.jsonl`.

## Definition Of Done For This Planning Slice

- Trellis is initialized in the forked worktree.
- This PRD records Teams as infrastructure, not a policy workflow.
- The implementation/check manifests point to Teams-specific spec and research files.
- A git commit captures the fork/Trellis/task baseline before code implementation starts.
