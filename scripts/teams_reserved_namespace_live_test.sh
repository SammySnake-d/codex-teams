#!/usr/bin/env bash
# REAL-CONFIG regression guard for the reserved-namespace fix (see
# codex-rs/docs/FORK_PATCHES.md #1).
#
# Some frontier models (e.g. gpt-5.6-sol) NATIVELY reserve the `collaboration`
# function namespace with a fixed schema. Upstream codex sends its multi_agent_v2
# tools under exactly that namespace, so on such a provider the whole tools array
# is rejected (`invalid_request: Function 'collaboration.spawn_agent' is
# reserved ...`) — which breaks ALL tool use. The fork moves the default namespace
# off the reserved name (DEFAULT_MULTI_AGENT_V2_TOOL_NAMESPACE -> "codex_agents"),
# UNCONDITIONALLY, so plain `codex` on that model works again — not just Teams.
#
# This proves it on the DAILY path: a real gpt-5.6-sol run with NO `--enable teams`
# and NO config override must (a) not hit the reserved-name rejection, and (b)
# actually execute a tool (ground truth: a unique marker printed by a shell call).
#
# WARNING: consumes REAL model quota (one bounded exec run).
set -uo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
REAL_HOME="$HOME/.codex"
MARKER="NSFIX_OK_$(date +%H%M%S)"
H="/tmp/codex-teams-nsguard-$(date +%H%M%S)"
DEADLINE=${DEADLINE:-180}
mkdir -p "$H"

log() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()  { printf '\033[1;32m OK \033[0m %s\n' "$*"; }
bad() { printf '\033[1;31mFAIL\033[0m %s\n' "$*"; }
trap 'printf "\nEVIDENCE: %s\n" "$H"' EXIT

run_bounded() { local s=$1; shift; "$@" & local p=$!; ( sleep "$s"; kill -TERM "$p" 2>/dev/null ) & local g=$!; wait "$p"; local r=$?; kill "$g" 2>/dev/null; return $r; }

[ -x "$CODEX_BIN" ] || { echo "missing $CODEX_BIN"; exit 2; }
lsof -iTCP:8317 -sTCP:LISTEN >/dev/null 2>&1 || { echo "cpa (8317) not listening"; exit 2; }
[ -f "$REAL_HOME/auth.json" ] || { echo "no auth.json"; exit 2; }

# Clean CODEX_HOME mirroring ONLY the real provider (real auth + custom provider),
# NO features.multi_agent_v2.tool_namespace override and NO --enable teams — so a
# pass proves the FORK's default-namespace change (not a config knob, not the
# Teams gate) fixes the daily path. No user hooks/memory/persona/mcp, which would
# otherwise steer the model away from calling tools.
cp "$REAL_HOME/auth.json" "$H/auth.json"
cat > "$H/config.toml" <<'CFG'
model_provider = "custom"
model = "gpt-5.6-sol"
model_reasoning_effort = "medium"
approval_policy = "never"

[model_providers.custom]
name = "custom"
requires_openai_auth = true
base_url = "http://127.0.0.1:8317/v1"
wire_api = "responses"
CFG

log "version"; "$CODEX_BIN" --version
log "drive a plain tool call on REAL gpt-5.6-sol (NO --enable teams, NO namespace override)"
run_bounded "$DEADLINE" env CODEX_HOME="$H" "$CODEX_BIN" exec \
  --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox \
  "Run exactly this shell command and nothing else: echo ${MARKER}" \
  < /dev/null > "$H/out.txt" 2>&1
rc=$?; echo "exec rc=$rc"

log "assert 1/2: the reserved-namespace collision is GONE"
if grep -qE 'reserved for use by this model' "$H/out.txt"; then
  bad "collaboration.spawn_agent reservation still rejected the request"; grep -E 'reserved for use' "$H/out.txt" | head -1; exit 1
fi
ok "no 'collaboration.spawn_agent is reserved' rejection"

log "assert 2/2: a tool actually executed (ground truth: marker printed by shell)"
R="$(ls -t "$H"/sessions/*/*/*/rollout-*.jsonl 2>/dev/null | head -1)"
if grep -qF "$MARKER" "$H/out.txt" "$R" 2>/dev/null; then
  ok "marker '$MARKER' produced by a real tool call — request accepted, tools dispatch"
  echo; ok "reserved-namespace fix verified live on gpt-5.6-sol (daily path, no Teams)"
  exit 0
else
  bad "marker not found — model may not have executed the tool"; tail -8 "$H/out.txt"; exit 1
fi
