#!/usr/bin/env bash
# REAL-CONFIG live test: `[teams] startup_members` auto-spawn on gpt-5.6-luna.
#
# Unlike the mock smoke, this drives the REAL custom provider (127.0.0.1:8317,
# real auth) to prove startup auto-spawn works end-to-end on the daily model.
# The auto-spawn happens during SESSION CONSTRUCTION (before any model turn), so
# we only need to boot an interactive lead TUI in tmux — teammates are full TUI
# processes and split panes in the same session.
#
# Clean CODEX_HOME on purpose: only auth + the custom provider + teams +
# startup_members. NO persona/hooks/memory/mcp (those would steer the model, and
# the MCP stdio servers slow boot) — we still hit the real provider request path.
#
# GROUND TRUTH = disk + tmux, never model text:
#   - exactly ONE team dir; members == [team-lead, scout, watcher]
#   - 3 tmux panes (lead + 2 teammates)
#   - after a settle window, counts are STABLE (teammates read the same
#     config.toml with the same startup_members yet must NOT recurse —
#     teammate_identity() is the guard).
#
# WARNING: consumes REAL model quota (lead + 2 teammates each open a session on
# the real provider). Bounded: no task is delegated; members just stand by.
set -uo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
REAL_HOME="$HOME/.codex"
SESSION="codex-startup-real-$(date +%H%M%S)"
H="/tmp/${SESSION}"
PANE=""
mkdir -p "$H"

log() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()  { printf '\033[1;32m OK \033[0m %s\n' "$*"; }
bad() { printf '\033[1;31mFAIL\033[0m %s\n' "$*"; }
note(){ printf '\033[1;33m ·  \033[0m %s\n' "$*"; }
capture() { tmux capture-pane -p -t "$PANE" 2>/dev/null; }
cleanup() {
  [ -n "$PANE" ] && tmux kill-session -t "$SESSION" 2>/dev/null || true
  pkill -f "$H" 2>/dev/null || true
  printf '\nEVIDENCE: %s\n' "$H"
}
trap cleanup EXIT

command -v tmux >/dev/null || { echo "tmux required"; exit 2; }
[ -x "$CODEX_BIN" ] || { echo "missing $CODEX_BIN"; exit 2; }
lsof -iTCP:8317 -sTCP:LISTEN >/dev/null 2>&1 || { echo "real provider (8317) not listening"; exit 2; }
[ -f "$REAL_HOME/auth.json" ] || { echo "no auth.json"; exit 2; }

log "version"; "$CODEX_BIN" --version

# Clean CODEX_HOME: real auth + real provider + teams + startup_members only.
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

[teams.scout]
prompt = "STARTUP-PROBE: you were auto-started. Stand by."
file = "./agents/observer.toml"

[teams.watcher]

[model_providers.custom]
name = "custom"
requires_openai_auth = true
base_url = "http://127.0.0.1:8317/v1"
wire_api = "responses"

[projects."${WORKSPACE}"]
trust_level = "trusted"
EOF

# Role customization layer for scout. The marker in developer_instructions is
# the DISK ground truth that the teammate PROCESS actually applied the layer:
# it must appear in scout's session rollout and nowhere in the lead's.
ROLE_MARKER="ROLE_LAYER_APPLIED_$(date +%H%M%S)"
mkdir -p "$H/agents"
cat > "$H/agents/observer.toml" <<EOF
name = "observer"
developer_instructions = "${ROLE_MARKER}: you are a quiet observer. Stand by unless addressed."
model_reasoning_effort = "low"
EOF
note "clean CODEX_HOME=$H (auth + custom provider + [teams.<name>] tables + role file; no persona/hooks/memory/mcp)"

# Boot an interactive lead TUI on the real config. `codexteam` not needed here —
# teams is enabled via config; use plain codex so the binary under test is exact.
log "boot REAL lead TUI in tmux (auto-spawn fires during session init)"
PANE="$(tmux new-session -d -P -F '#{pane_id}' -x 220 -y 50 -c "$WORKSPACE" -s "$SESSION" -- \
  env CODEX_HOME="$H" CODEX_TEAMMATE_COMMAND="$CODEX_BIN" \
  "$CODEX_BIN" --no-alt-screen --dangerously-bypass-hook-trust)"
echo "PANE=$PANE"

# Wait for the team to appear on disk (session init spawns synchronously, but
# teammate binaries take a few seconds to open panes + register).
TEAM_DIR=""
for _ in $(seq 1 120); do
  TEAM_DIR="$(find "$H/teams" -maxdepth 1 -mindepth 1 -type d 2>/dev/null | head -1 || true)"
  [ -n "$TEAM_DIR" ] && [ -f "$TEAM_DIR/config.json" ] && break
  sleep 1
done
[ -n "$TEAM_DIR" ] && [ -f "$TEAM_DIR/config.json" ] || { bad "no team config.json under $H/teams"; capture | tail -20; exit 1; }
ok "team created on disk: $(basename "$TEAM_DIR")"

members_ok() {
  python3 - "$TEAM_DIR/config.json" <<'PY'
import json,sys
cfg=json.load(open(sys.argv[1]))
names=sorted(m["name"] for m in cfg.get("members",[]))
print("members:",names)
sys.exit(0 if names==["scout","team-lead","watcher"] else 1)
PY
}

log "wait for both configured members to register"
SETTLED=0
for _ in $(seq 1 120); do members_ok >/dev/null 2>&1 && { SETTLED=1; break; }; sleep 1; done
members_ok || true
[ "$SETTLED" = 1 ] || { bad "members never reached [scout, team-lead, watcher]"; cat "$TEAM_DIR/config.json"; exit 1; }
ok "both startup members registered"

log "no 401 / openai leakage on real provider"
sleep 2
if capture | grep -qiE '401|api\.openai\.com|Test API Key'; then note "saw a 401/openai marker:"; capture | grep -iE '401|openai' | head; else ok "clean"; fi

log "RECURSION CHECK: settle 30s, re-assert exact counts"
sleep 30
TEAM_COUNT="$(find "$H/teams" -maxdepth 1 -mindepth 1 -type d | wc -l | tr -d ' ')"
[ "$TEAM_COUNT" = 1 ] || { bad "recursion: expected 1 team dir, found $TEAM_COUNT"; ls "$H/teams"; exit 1; }
members_ok || { bad "recursion: member list changed after settle"; cat "$TEAM_DIR/config.json"; exit 1; }
ok "still exactly 1 team / 3 members after settle"

PANE_COUNT="$(tmux list-panes -t "$SESSION" 2>/dev/null | wc -l | tr -d ' ')"
echo "panes=$PANE_COUNT (expect 3 = lead + 2 teammates)"
[ "$PANE_COUNT" = 3 ] || { bad "expected 3 panes, found $PANE_COUNT"; exit 1; }
ok "3 tmux panes (lead + 2 teammate processes)"

log "teammate processes really live"
pgrep -fl "teammate" | grep -F "$H" | head -6 || pgrep -fl "teammate" | head -6 || true

log "ROLE FILE proof 1/2: lead passed --agent-role-file to scout's process"
if ps ax -o command | grep -F -- "--agent-name scout" | grep -v grep | grep -qF -- "--agent-role-file"; then
  ok "scout process carries --agent-role-file"
else
  bad "scout process is missing --agent-role-file"
  ps ax -o command | grep -F -- "--agent-name scout" | grep -v grep | head -2
  exit 1
fi
if ps ax -o command | grep -F -- "--agent-name watcher" | grep -v grep | grep -qF -- "--agent-role-file"; then
  bad "watcher (no file configured) unexpectedly got --agent-role-file"
  exit 1
fi
ok "watcher (file-less) correctly launched without a role file"

log "ROLE FILE proof 2/2: layer actually APPLIED (marker in scout's session, absent from lead's)"
MARKER_SETTLED=0
for _ in $(seq 1 60); do
  if grep -rlF "$ROLE_MARKER" "$H"/sessions/ >/dev/null 2>&1; then MARKER_SETTLED=1; break; fi
  sleep 1
done
if [ "$MARKER_SETTLED" = 1 ]; then
  MARKED_FILES="$(grep -rlF "$ROLE_MARKER" "$H"/sessions/ 2>/dev/null | wc -l | tr -d ' ')"
  ok "role-layer marker found in $MARKED_FILES session rollout(s)"
else
  bad "role-layer marker never appeared in any session rollout — layer not applied"
  exit 1
fi
# The lead + watcher have no role file: the marker must appear in exactly ONE
# session's rollouts (scout's).
SESSIONS_WITH_MARKER="$(grep -rlF "$ROLE_MARKER" "$H"/sessions/ 2>/dev/null | xargs -n1 dirname | sort -u | wc -l | tr -d ' ')"
if [ "$SESSIONS_WITH_MARKER" = "1" ]; then
  ok "marker confined to exactly 1 session (scout) — lead/watcher unaffected"
else
  bad "marker leaked into $SESSIONS_WITH_MARKER session dirs (expected 1)"
  grep -rlF "$ROLE_MARKER" "$H"/sessions/ | head -5
  exit 1
fi

echo ""
ok "TEAMS_STARTUP_MEMBERS_REAL_PASS model=gpt-5.6-luna team=$(basename "$TEAM_DIR") members=scout,team-lead,watcher panes=$PANE_COUNT recursion=none role_file=applied"
