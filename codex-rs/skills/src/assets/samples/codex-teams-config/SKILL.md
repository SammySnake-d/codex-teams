---
name: codex-teams-config
description: Use when the user wants to configure Codex Agent Teams — adding or customizing persistent teammates ([teams.<name>] in config.toml), writing teammate role customization files, wiring startup teammates, or asking how teammates differ from subagents ([agents.<name>]). Covers the [teams] config schema, role-file format shared with agent roles, and how a role file can serve both a teammate and a subagent.
metadata:
  short-description: Configure Agent Teams teammates and their role files
---

# Codex Teams Configuration

Codex Agent Teams runs persistent teammate PROCESSES (tmux/iTerm2 panes with
their own mailbox) alongside the lead session. This skill covers configuring
them in `config.toml` and customizing them with role files.

## Teammates vs subagents

- `[teams.<name>]` — a TEAMMATE: a separate long-lived `codex teammate` process
  in a pane. Talks to the lead (and peers) through the on-disk mailbox. Started
  automatically when a lead session boots with the Teams feature enabled.
- `[agents.<name>]` — a SUBAGENT role: an in-process thread spawned per task via
  `spawn_agent`. Not a process, no pane, no mailbox.

The two share ONE role-file format, so a single customization file can back
both a teammate and a subagent role.

## Declaring startup teammates

Each `[teams.<name>]` table brings up one teammate when a fresh lead session
starts (requires `features.teams = true` and a tmux/iTerm2 pane backend; both
are on by default under the `codexteam` launcher inside tmux):

```toml
[features]
teams = true

[teams.reviewer]
prompt = "Watch the lead's work. Report drift and push corrections."
file   = "./agents/reviewer.toml"     # optional customization layer

[teams.scout]
prompt = "Survey the repo and report structure."   # no file -> default role
```

- Table key = teammate name (shows on the pane, in messages, and in the team
  roster).
- `prompt` — first message delivered via the teammate's mailbox. Omitted: the
  teammate gets a short standby instruction. For an uncustomized teammate, the
  prompt is the main steering tool — write the role's duties into it.
- `file` — optional path to a role customization TOML, resolved relative to the
  declaring `config.toml`. Omitted: the teammate runs the default role.
- Resume/fork sessions do NOT respawn the team; only brand-new sessions do.
- A missing `file` path fails that one member with a warning; the session and
  the other members still start.

## Role customization files

The `file` layer uses the SAME format as an agent role's `config_file`: a
config-shaped TOML applied over the teammate's config at boot. Common fields:

```toml
# ./agents/reviewer.toml
name = "reviewer"                 # metadata; REQUIRED if the file lives in an
                                  # agents/ dir (auto-discovery validates it)
developer_instructions = """
You are a meticulous code reviewer. Challenge risky changes, verify claims
against the diff, and push back with concrete evidence.
"""
model = "gpt-5.6-sol"            # omit to inherit the lead's model
model_reasoning_effort = "high"
```

Anything valid in `config.toml` is valid here; the layer overrides the base
config. `model_provider` and `service_tier` stay inherited unless the file sets
them explicitly.

To share the file with a subagent role, reference it from `[agents]` too:

```toml
[agents.reviewer]
description = "Meticulous code reviewer."
config_file = "./agents/reviewer.toml"    # same file as [teams.reviewer].file
```

Tool-spawned teammates (`team_spawn_member` / `spawn_agent` with a profile)
resolve the profile name through `[agents.<name>]` and load its `config_file`
the same way.

## Workflow for "add/customize a teammate"

1. Ask what the teammate should do; distill duties into a short `prompt`.
2. Only write a role `file` when the teammate needs persona/model/effort
   overrides beyond what the prompt expresses. Keep it minimal.
3. Add the `[teams.<name>]` table (and, if the role should also be spawnable as
   a subagent, an `[agents.<name>]` entry pointing at the same file).
4. Remind the user: teammates spawn on the NEXT fresh lead session inside
   tmux/iTerm2; each teammate opens its own model session (token cost scales
   with member count).
