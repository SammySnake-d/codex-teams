# Anthropic Agent Teams Research Summary

## Source

- Claude Code Agent Teams documentation: `https://code.claude.com/docs/en/agent-teams`
- Claude Code sub-agents documentation: `https://docs.anthropic.com/en/docs/claude-code/sub-agents`

## Relevant Capability Definition

Anthropic-style Agent Teams are a collaboration mode, not just a subagent return mechanism.

Key concepts:

- Team lead creates and coordinates the team.
- Teammates are independent Claude Code sessions with their own context windows.
- Shared task list lets agents claim, complete, and unblock work.
- Mailbox supports agent-to-agent messages.
- Display mode may be in-process switching or terminal multiplexer panes.
- Local state stores team configuration and task state.
- Subagent definitions can be reused as teammate role profiles.

## Implications For Codex

- Codex Teams needs a team substrate, not only a `/teams` slash command.
- Natural language team creation requires model-callable tools and usage hints.
- Existing Codex multi-agent primitives should be reused as lower-level session/message building blocks.
- Team state, task board, and event feed are missing product-body skeleton pieces.
- Reviewer/self-correction workflows must remain policy plugins on top of the generic substrate.

## MVP Design Constraint

Match the capability shape before matching every display mode. The first Codex implementation should prove independent sessions, member registry, messaging, status, and stop/cleanup before adding tmux-style panes or specialized reviewer recipes.
