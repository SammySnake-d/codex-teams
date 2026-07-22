#!/usr/bin/env bash
# LIVE proof that Teams tools are model-visible on a SEARCH-CAPABLE model.
#
# Before the fix, a lead on a model with `supports_search_tool=true` (real
# gpt-5.6-luna) had Teams tools DEFERRED behind tool_search — the model had to
# search "teammate/团队" to surface create_team/team_spawn_member, and often
# didn't, falling back to a plain sub-agent. This test drives such a model to
# spawn a teammate via the Teams tools and asserts a real teammate registers on
# disk. Clean CODEX_HOME (no persona/hooks/memory/mcp) so the model isn't
# steered into introspecting/hallucinating about its tools.
#
# GROUND TRUTH = disk: a teammate named `probe` appears in the team config.json.
# WARNING: consumes REAL model quota (lead + teammate).
set -uo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
REAL_HOME="$HOME/.codex"
SESSION="codex-teamvis-$(date +%H%M%S)"
H="/tmp/${SESSION}"
PANE=""
mkdir -p "$H"

log() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()  { printf '\033[1;32m OK \033[0m %s\n' "$*"; }
bad() { printf '\033[1;31mFAIL\033[0m %s\n' "$*"; }
capture() { tmux capture-pane -p -t "$PANE" 2>/dev/null; }
cleanup() { [ -n "$PANE" ] && tmux kill-session -t "$SESSION" 2>/dev/null || true; pkill -f "$H" 2>/dev/null || true; printf '\nEVIDENCE: %s\n' "$H"; }
trap cleanup EXIT

command -v tmux >/dev/null || { echo "tmux required"; exit 2; }
lsof -iTCP:8317 -sTCP:LISTEN >/dev/null 2>&1 || { echo "provider 8317 down"; exit 2; }
[ -f "$REAL_HOME/auth.json" ] || { echo "no auth.json"; exit 2; }

"$CODEX_BIN" --version
cp "$REAL_HOME/auth.json" "$H/auth.json"
cat > "$H/config.toml" <<EOF
model_provider = "custom"
model = "gpt-5.6-luna"
model_reasoning_effort = "low"
approval_policy = "never"
sandbox_mode = "danger-full-access"
suppress_unstable_features_warning = true

[features]
teams = true

[model_providers.custom]
name = "custom"
requires_openai_auth = true
base_url = "http://127.0.0.1:8317/v1"
wire_api = "responses"

[projects."${WORKSPACE}"]
trust_level = "trusted"
EOF

log "boot lead TUI (gpt-5.6-luna, supports_search_tool=true, teams on)"
PANE="$(tmux new-session -d -P -F '#{pane_id}' -x 220 -y 50 -c "$WORKSPACE" -s "$SESSION" -- \
  env CODEX_HOME="$H" CODEX_TEAMMATE_COMMAND="$CODEX_BIN" \
  "$CODEX_BIN" --enable teams --no-alt-screen --dangerously-bypass-hook-trust)"
echo "PANE=$PANE"

# wait for boot
for _ in $(seq 1 40); do capture | grep -qiE "gpt-|Codex|YOLO" && break; sleep 0.5; done
sleep 6

log "instruct the lead to spawn a teammate via the Teams tools (must be visible)"
PROMPT="Use your Teams tools right now, no exploration. Call create_team with team_name=vis, then call team_spawn_member to spawn a teammate named probe with message 'stand by'. "
tmux send-keys -t "$PANE" -l "$PROMPT"; sleep 1; tmux send-keys -t "$PANE" Enter

# wait for either the teammate on disk, or an explicit missing signal
TEAM_DIR=""; RESULT=""
for _ in $(seq 1 150); do
  TEAM_DIR="$(find "$H/teams" -maxdepth 1 -mindepth 1 -type d 2>/dev/null | head -1 || true)"
  if [ -n "$TEAM_DIR" ] && [ -f "$TEAM_DIR/config.json" ]; then
    if python3 -c "import json,sys; c=json.load(open('$TEAM_DIR/config.json')); sys.exit(0 if any(m['name']=='probe' for m in c.get('members',[])) else 1)" 2>/dev/null; then
      RESULT="spawned"; break
    fi
  fi
  if capture | grep -q "TEAMS_TOOLS_MISSING"; then RESULT="missing"; break; fi
  sleep 1
done

log "verdict"
if [ "$RESULT" = "spawned" ]; then
  echo "members on disk:"; python3 -c "import json; print(sorted(m['name'] for m in json.load(open('$TEAM_DIR/config.json'))['members']))"
  ok "TEAMS_TOOLS_VISIBLE_PASS — search-capable model saw + called team_spawn_member; teammate 'probe' registered"
elif [ "$RESULT" = "missing" ]; then
  bad "model reported TEAMS_TOOLS_MISSING — team tools not visible to it"
  capture | tail -20; exit 1
else
  bad "timed out — no teammate on disk and no explicit missing signal (model may have stalled/hallucinated)"
  capture | tail -25; exit 1
fi
