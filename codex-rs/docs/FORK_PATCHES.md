# Fork patches (codex-teams)

Deliberate divergences from upstream `openai/codex` that are NOT part of the
Teams feature itself but are required for it to run. Each entry says how to
detect the upstream fix and how to revert. Keep this list short; prefer additive
Teams-scoped code over editing upstream logic.

## 1. multi_agent_v2 default tool namespace: `collaboration` → `codex_agents`

**Since:** TEAMS_VERSION team.13

**Why:** Some frontier models (observed on `gpt-5.6-sol` via a custom/`responses`
provider) NATIVELY reserve the `collaboration` function namespace with a fixed
schema. Upstream sends its `multi_agent_v2` sub-agent tools under exactly that
namespace, so the provider rejects the **entire tools array**:

```
invalid_request: Function 'collaboration.spawn_agent' is reserved for use by this
model and must match the configured schema. (param: tools)
```

One poisoned tool rejects the whole request, so **all** tool use breaks — not just
Teams, since Teams' own tools ride in the same request. This is provider-wide
(reproduces with a plain `codex exec`, no `--enable teams`), which is why the fix
is unconditional rather than Teams-gated.

**Change:** `codex-rs/core/src/config/mod.rs`
- `DEFAULT_MULTI_AGENT_V2_TOOL_NAMESPACE`: `"collaboration"` → `"codex_agents"`
- `DEFAULT_MULTI_AGENT_V2_SHARED_USAGE_HINT_TEXT`: the `to=functions.collaboration.spawn_agent`
  example → `to=functions.codex_agents.spawn_agent` (keeps the model-facing hint in
  sync with the actual namespace).

**Ripple:** the local `const MULTI_AGENT_V2_NAMESPACE = "collaboration"` in these
tests was updated to `"codex_agents"` (they assert the planned tool namespace and
otherwise fail):
`core/tests/suite/{model_runtime_selectors,rollout_budget,subagent_notifications,agent_execution,pending_input}.rs`,
`core/src/tools/spec_plan_tests.rs`, `app-server/tests/suite/v2/turn_start.rs`.

**Detect upstream fix / revert when:** upstream models stop reserving
`collaboration`, or upstream changes its own default namespace, or a per-provider
capability lands that suppresses reserved-name schemas. To revert, restore
`"collaboration"` in `config/mod.rs` (constant + hint) and the seven test
constants. `grep -rn 'codex_agents' codex-rs/` finds every touch point.

**Verify:** `scripts/teams_reserved_namespace_live_test.sh` (real gpt-5.6-sol, no
config override) asserts the reserved-name rejection is gone and a tool actually
dispatches.

## 2. Reasoning field omission for non-reasoning providers — RETIRED
**Status:** Retired at the `rust-v0.145.0-alpha.24` sync (was team.8, an inline
port of upstream `rust-v0.144.1`; never had its own entry here).
**What it was:** `client.rs::build_reasoning` returned `Option<Reasoning>` and
omitted the whole `reasoning` field (and `reasoning.encrypted_content` include)
when `ModelInfo.supports_reasoning_summaries` was false, so OpenAI-compatible
proxies serving non-reasoning models would not 400 on an unexpected `reasoning`
field.
**Why retired:** upstream evolved this exact path with a more precise gate,
`ModelInfo.supports_reasoning_summary_parameter`, which controls only the
`summary` sub-field while always sending `reasoning`. Our fork's field
`supports_reasoning_summaries` is `#[serde(default = false)]` and is set `true`
nowhere in the merged tree (the upstream model catalog only populates
`supports_reasoning_summary_parameter`), so keeping our outer gate would have
returned `None` for EVERY model and disabled reasoning globally. We therefore
took upstream's `build_reasoning` verbatim. The now-orphaned
`supports_reasoning_summaries` field is left in place (harmless, unread) to
avoid churning ~20 constructor sites; a later cleanup can remove it.
**Re-add if:** a live run on the user's provider (cliproxy / gpt-5.6-luna)
starts returning `400 invalid_request` on `tools` because the model does not
accept `reasoning`. Then reintroduce a narrow gate keyed on a field the model
catalog actually populates.
**Verify:** `scripts/teams_startup_members_real_live_test.sh` and any real
`codexteam exec` on the user's provider must dispatch tools without a reasoning
400.
