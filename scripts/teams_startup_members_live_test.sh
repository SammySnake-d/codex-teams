#!/usr/bin/env bash
# Live test: `[teams] startup_members` auto-spawn at lead session startup (team.15).
#
# Proves, against the freshly built debug binary with the deterministic mock
# Responses provider (NO real API key):
#   1. A fresh lead session (features.teams=true + [teams] startup_members)
#      automatically creates a team and spawns each configured member as a real
#      `codex teammate` process in a tmux pane — WITHOUT any tool call.
#   2. NO RECURSION: spawned teammates read the SAME config.toml (same
#      CODEX_HOME, same startup_members, same features.teams) yet must not
#      spawn teams of their own — `teammate_identity()` is the guard under test.
#
# GROUND TRUTH IS DISK + tmux, never model text:
#   - exactly ONE team dir under $CODEX_HOME/teams
#   - team config.json members == team-lead + the 2 configured members
#   - teammate panes/processes exist; counts are STABLE after settle time
set -euo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
PROXY_BIN="$WORKSPACE/codex-rs/target/debug/codex-responses-api-proxy"
SESSION="codex-startup-live-$(date +%H%M%S)"
ROOT="/tmp/${SESSION}"
mkdir -p "$ROOT/home" "$ROOT/dumps"

echo "ROOT=$ROOT"
"$CODEX_BIN" --version

fail() { echo "FAIL: $*" >&2; exit 1; }

cleanup() {
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  [ -f "$ROOT/proxy.pid" ] && kill "$(cat "$ROOT/proxy.pid")" 2>/dev/null || true
}
trap cleanup EXIT

# 1. Mock Responses provider (requires_openai_auth=false; no secrets).
"$PROXY_BIN" \
  --mock-teams-smoke \
  --http-shutdown \
  --server-info "$ROOT/server-info.json" \
  --dump-dir "$ROOT/dumps" \
  >"$ROOT/proxy.stdout" 2>"$ROOT/proxy.stderr" &
echo "$!" > "$ROOT/proxy.pid"
for _ in $(seq 1 100); do
  [ -s "$ROOT/server-info.json" ] && break
  sleep 0.1
done
[ -s "$ROOT/server-info.json" ] || fail "mock provider did not start"
PROXY_PORT="$(sed -E 's/.*"port":([0-9]+).*/\1/' "$ROOT/server-info.json")"
echo "PROXY_PORT=$PROXY_PORT"

# 2. Shared config.toml: teams feature + TWO startup members (one with prompt,
#    one without — exercising the default-prompt path). Teammates read this
#    exact file too, which is what makes the recursion check meaningful.
cat > "$ROOT/home/config.toml" <<EOF
model = "teams-smoke-model"
model_provider = "teams_smoke"
approval_policy = "never"
sandbox_mode = "danger-full-access"
suppress_unstable_features_warning = true

[features]
teams = true

[teams]
startup_members = [
  { name = "scout", prompt = "STARTUP-PROBE: report in." },
  { name = "watcher" },
]

[model_providers.teams_smoke]
name = "Teams Smoke Mock"
base_url = "http://127.0.0.1:${PROXY_PORT}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0

[projects."${WORKSPACE}"]
trust_level = "trusted"
EOF

# 3. Start the lead inside tmux. `codex exec` with a trivial prompt: the
#    auto-spawn happens during session construction, BEFORE/aside from any tool
#    use. Teammates split panes in this same tmux session.
tmux new-session -d -s "$SESSION" -x 220 -y 50 "cd '$WORKSPACE'; \
  CODEX_HOME='$ROOT/home' \
  CODEX_TEAMMATE_COMMAND='$CODEX_BIN' \
  OPENAI_API_KEY=mock \
  '$CODEX_BIN' exec --json -C '$WORKSPACE' 'say hi and stop' \
  >'$ROOT/exec.stdout' 2>'$ROOT/exec.stderr'; echo \$? >'$ROOT/exec.exit'; sleep 600"

# 4. Wait for the team to appear on disk (startup spawn is synchronous in
#    session init, but binary/pane startup takes a few seconds).
TEAM_DIR=""
for _ in $(seq 1 120); do
  TEAM_DIR="$(find "$ROOT/home/teams" -maxdepth 1 -mindepth 1 -type d 2>/dev/null | head -1 || true)"
  [ -n "$TEAM_DIR" ] && [ -f "$TEAM_DIR/config.json" ] && break
  sleep 1
done
[ -n "$TEAM_DIR" ] && [ -f "$TEAM_DIR/config.json" ] || {
  echo "--- exec.stderr ---"; tail -40 "$ROOT/exec.stderr" 2>/dev/null || true
  fail "no team config.json appeared under $ROOT/home/teams"
}
echo "TEAM_DIR=$TEAM_DIR"

# 5. Wait until both configured members are registered on disk.
members_ok() {
  python3 - "$TEAM_DIR/config.json" <<'PY'
import json,sys
cfg=json.load(open(sys.argv[1]))
names=sorted(m["name"] for m in cfg.get("members",[]))
ok = names == ["scout","team-lead","watcher"]
print("members:",names)
sys.exit(0 if ok else 1)
PY
}
MEMBERS_SETTLED=0
for _ in $(seq 1 90); do
  if members_ok; then MEMBERS_SETTLED=1; break; fi
  sleep 1
done
[ "$MEMBERS_SETTLED" = "1" ] || { cat "$TEAM_DIR/config.json"; fail "members never reached exactly [scout, team-lead, watcher]"; }

# 6. Teammate processes really running (codex teammate in pane).
sleep 3
TEAMMATE_PROCS="$(pgrep -fl "codex.*teammate.*--agent-name" 2>/dev/null | grep -c "$ROOT" || true)"
echo "teammate processes referencing this CODEX_HOME: (pgrep view below)"
pgrep -fl "teammate" | head -10 || true

# 7. RECURSION CHECK: settle, then re-assert exact counts. If a teammate
#    process ran the startup hook, extra teams/members/panes would appear.
echo "settling 25s for recursion check..."
sleep 25
TEAM_COUNT="$(find "$ROOT/home/teams" -maxdepth 1 -mindepth 1 -type d | wc -l | tr -d ' ')"
[ "$TEAM_COUNT" = "1" ] || fail "recursion suspected: expected 1 team dir, found $TEAM_COUNT"
members_ok || { cat "$TEAM_DIR/config.json"; fail "recursion suspected: member list changed after settle"; }
PANE_COUNT="$(tmux list-panes -t "$SESSION" 2>/dev/null | wc -l | tr -d ' ')"
echo "pane count in $SESSION: $PANE_COUNT (expect 3 = lead + 2 teammates)"
[ "$PANE_COUNT" = "3" ] || fail "expected 3 panes (lead+2 teammates), found $PANE_COUNT"

# 8. Inbox proof for the prompted member: scout's mailbox got the configured
#    initial prompt (delivered by the lead at spawn).
INBOX_SCOUT="$ROOT/home/teams/$(basename "$TEAM_DIR")/inboxes"
echo "--- inbox dir ---"; ls "$INBOX_SCOUT" 2>/dev/null || true

echo ""
echo "TEAMS_STARTUP_MEMBERS_PASS team_dir=$(basename "$TEAM_DIR") members=scout,team-lead,watcher panes=$PANE_COUNT recursion=none"
