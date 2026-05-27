# Codex Rust Development Guidelines

> Applies to `codex-rs/`, especially `codex-rs/core`, `codex-rs/protocol`, and `codex-rs/tui`.

## Pre-Development Checklist

- Read the root `AGENTS.md` before changing Rust code.
- For TUI-visible behavior, read `codex-rs/tui/styles.md` and add or update snapshot coverage.
- For Teams work, separate the product-body skeleton from feature policy: team state, lifecycle, message routing, task board, and event feed belong in substrate; reviewer/blocker/Darwin policies do not.
- Natural-language invocation is required: expose Teams actions through model-callable core tools, not only through slash commands.
- Prefer narrow vertical slices that prove one substrate path before broad UX or policy expansion.

## Architecture Rules

- Keep stable coordination state in core-level substrate types, with TUI acting as a view/control surface.
- Reuse existing multi-agent primitives where possible: agent spawn, inter-agent communication, mailbox delivery, wait/list/close, and agent metadata.
- Do not hardcode role-specific policy into Teams core. Roles should be capability profiles or instructions attached to members.
- Treat display mode as a plugin boundary. In-process switching should work before tmux/iTerm split-pane support.
- Persist team state explicitly or fail closed when resume cannot prove that members are still live.

## TUI Rules

- Slash commands are manual control surfaces. They do not satisfy natural-language Teams onboarding by themselves.
- Add `/teams` as a user-facing control plane only after core actions have model-callable equivalents.
- Use existing TUI slash-command ordering and style conventions.
- Any new rendered UI state needs snapshot coverage in `codex-rs/tui`.

## Validation Rules

- For `codex-rs/tui` changes, run `just fmt` from `codex-rs`, then `cargo test -p codex-tui`.
- For `codex-rs/core` or protocol changes, run the focused crate tests first. Ask before running the full `cargo test --all-features`.
- If config schema changes, run `just write-config-schema`.
- Do not call mock/docs-only proof done for Teams substrate. At least one real local core/TUI path must prove create/list/status or create/spawn/message.

## Quality Check

- Does the change preserve a substrate/policy split?
- Can the same substrate host a different team policy without core rewrites?
- Can a user trigger the feature through natural language, not only command memorization?
- Is there a clear lifecycle for create, member spawn, message delivery, task state, status, and stop?
- Is the first slice independently testable without requiring the full Teams product?
