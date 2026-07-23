#!/usr/bin/env bash
# LIVE proof of the iTerm2 spawn backend (NOT tmux), using the deterministic
# mock Responses provider so it needs NO model quota. Runs a lead via
# `codex exec` inside iTerm2; the mock drives create_team -> team_spawn_member,
# and because we are in iTerm2 (not tmux) the iTerm2 backend (it2 Python API)
# fires. Asserts the teammate registered on disk with backend_type="iterm2".
#
# Must run from an iTerm2 session with the it2 Python API enabled
# (`it2 session list` exits 0), NOT inside tmux. It WILL open a real split pane
# in the current iTerm2 window; the teammate process is cleaned up at the end.
set -uo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
PROXY_BIN="$WORKSPACE/codex-rs/target/debug/codex-responses-api-proxy"
H="/tmp/codex-iterm-$(date +%H%M%S)"
mkdir -p "$H/dumps"

log() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()  { printf '\033[1;32m OK \033[0m %s\n' "$*"; }
bad() { printf '\033[1;31mFAIL\033[0m %s\n' "$*"; }

PROXY_PID=""
cleanup() {
  pkill -f "team-name viterm" 2>/dev/null || true
  pkill -f "$H" 2>/dev/null || true
  [ -n "$PROXY_PID" ] && kill "$PROXY_PID" 2>/dev/null || true
  printf '\nEVIDENCE: %s\n' "$H"
}
trap cleanup EXIT

# Preconditions: iTerm2 + it2 API, NOT tmux (so the iterm branch is taken).
[ -x "$CODEX_BIN" ] || { echo "missing $CODEX_BIN"; exit 2; }
[ -x "$PROXY_BIN" ] || { echo "missing $PROXY_BIN"; exit 2; }
[ -z "${TMUX:-}" ] || { echo "running inside tmux — this test must run in a bare iTerm2 session"; exit 2; }
[ "${TERM_PROGRAM:-}" = "iTerm.app" ] || { echo "not iTerm2 (TERM_PROGRAM=$TERM_PROGRAM)"; exit 2; }
it2 session list >/dev/null 2>&1 || { echo "it2 Python API not available"; exit 2; }
ok "iTerm2 + it2 API present, not in tmux"

"$CODEX_BIN" --version

# Deterministic mock Responses provider (no key, no quota).
"$PROXY_BIN" --mock-teams-smoke --http-shutdown \
  --server-info "$H/si.json" --dump-dir "$H/dumps" >"$H/proxy.log" 2>&1 &
PROXY_PID=$!
for _ in $(seq 1 100); do [ -s "$H/si.json" ] && break; sleep 0.1; done
[ -s "$H/si.json" ] || { bad "mock provider did not start"; cat "$H/proxy.log"; exit 1; }
PORT="$(sed -E 's/.*"port":([0-9]+).*/\1/' "$H/si.json")"
ok "mock provider on port $PORT (no quota needed)"

cat > "$H/config.toml" <<EOF
model = "teams-smoke-model"
model_provider = "teams_smoke"
approval_policy = "never"
sandbox_mode = "danger-full-access"
suppress_unstable_features_warning = true

[features]
teams = true

[model_providers.teams_smoke]
name = "Teams Smoke Mock"
base_url = "http://127.0.0.1:${PORT}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0

[projects."${WORKSPACE}"]
trust_level = "trusted"
EOF

log "drive the mock lead via codex exec (in iTerm2, not tmux) — expect iTerm2 split"
env CODEX_HOME="$H" CODEX_TEAMMATE_COMMAND="$CODEX_BIN" OPENAI_API_KEY=mock \
  "$CODEX_BIN" exec --enable teams --skip-git-repo-check -C "$WORKSPACE" \
  "spawn a teammate" </dev/null >"$H/exec.log" 2>&1 &
EXECPID=$!
( sleep 120; kill -TERM $EXECPID 2>/dev/null ) & GUARD=$!

RESULT=""
for _ in $(seq 1 120); do
  TEAM_DIR="$(find "$H/teams" -maxdepth 1 -mindepth 1 -type d 2>/dev/null | head -1 || true)"
  if [ -n "$TEAM_DIR" ] && [ -f "$TEAM_DIR/config.json" ]; then
    BT="$(python3 -c "
import json
c=json.load(open('$TEAM_DIR/config.json'))
m=[x for x in c.get('members',[]) if x['name']!='team-lead']
print(m[0].get('backendType','') if m else '')
" 2>/dev/null)"
    if [ -n "$BT" ]; then RESULT="$BT"; break; fi
  fi
  sleep 1
done
kill $GUARD 2>/dev/null || true
wait $EXECPID 2>/dev/null || true

log "verdict"
echo "--- exec.log tail ---"; tail -6 "$H/exec.log" 2>/dev/null || true
if [ "$RESULT" = "iterm2" ]; then
  echo "teammate backend_type on disk: $RESULT"
  python3 -c "import json; print('members:', sorted(m['name'] for m in json.load(open('$TEAM_DIR/config.json'))['members']))"
  ok "ITERM2_SPAWN_PASS — teammate spawned via the iTerm2 backend (it2), not tmux"
elif [ -n "$RESULT" ]; then
  bad "teammate spawned but via backend_type=$RESULT (expected iterm2)"; exit 1
else
  bad "no teammate registered"; tail -20 "$H/exec.log"; exit 1
fi

