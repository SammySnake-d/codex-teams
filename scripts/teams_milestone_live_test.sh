#!/usr/bin/env bash
# Live proof of SEMANTIC MILESTONE PUSH (gap #2 depth): a real teammate process
# that runs a tool mid-turn must push a `kind:progress` milestone to the lead's
# inbox at the TOOL-CALL boundary — not only at the turn boundary.
#
# Flow: preload the teammate 'worker' inbox with a task carrying MILESTONE-PROBE.
# The mock provider answers that turn with a shell_command call (echo ...), which
# triggers ExecCommandBegin inside the teammate → maybe_push_progress → a
# progress message ("running: echo ...") lands in team-lead's inbox with
# kind=progress. Evidence is on-disk in the lead inbox. No real API key.
set -uo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
PROXY_BIN="$WORKSPACE/codex-rs/target/debug/codex-responses-api-proxy"
SESSION="codex-teams-ms-$(date +%H%M%S)"
ROOT="/tmp/${SESSION}"
PROXY_PID=""; PANE=""; PASS=0; FAIL=0
mkdir -p "$ROOT/home"

log() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()  { PASS=$((PASS+1)); printf '\033[1;32mPASS\033[0m %s\n' "$*"; }
bad() { FAIL=$((FAIL+1)); printf '\033[1;31mFAIL\033[0m %s\n' "$*"; }
cleanup() {
  [ -n "$PANE" ] && tmux kill-session -t "$SESSION" 2>/dev/null || true
  [ -n "$PROXY_PID" ] && kill "$PROXY_PID" 2>/dev/null || true
  pkill -f "CODEX_HOME=$ROOT/home" 2>/dev/null || true
  printf '\nEVIDENCE: %s\nRESULT: %d passed, %d failed\n' "$ROOT" "$PASS" "$FAIL"
}
trap cleanup EXIT
glob1() { local m=( $1 ); for x in "${m[@]}"; do [ -e "$x" ] && { printf "%s" "$x"; return 0; }; done; printf ""; }

command -v tmux >/dev/null || { echo "tmux required"; exit 2; }
[ -x "$CODEX_BIN" ] && [ -x "$PROXY_BIN" ] || { echo "missing binaries"; exit 2; }

log "version under test"; "$CODEX_BIN" --version

# 1. mock provider
"$PROXY_BIN" --mock-teams-smoke --http-shutdown \
  --server-info "$ROOT/server-info.json" --dump-dir "$ROOT/dumps" >"$ROOT/proxy.log" 2>&1 &
PROXY_PID=$!
for _ in $(seq 1 100); do [ -s "$ROOT/server-info.json" ] && break; sleep 0.1; done
PORT="$(sed -E 's/.*"port":([0-9]+).*/\1/' "$ROOT/server-info.json")"
[ -n "$PORT" ] || { echo "no port"; exit 2; }

# 2. shared config
cat > "$ROOT/home/config.toml" <<EOF
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

# 3. Pre-load worker inbox with a task that triggers the shell milestone.
TEAM="ms team"; SAN="ms_team"
INBOX="$ROOT/home/teams/$SAN/inboxes"
mkdir -p "$INBOX"
cat > "$INBOX/worker.json" <<'EOF'
[
  { "from": "team-lead", "text": "Please run the probe. MILESTONE-PROBE", "timestamp": "2026-07-11T00:00:01.000Z", "read": false }
]
EOF
cat > "$ROOT/home/teams/$SAN/config.json" <<EOF
{ "name": "$TEAM", "createdAt": 1783000000000, "leadAgentId": "team-lead@$TEAM",
  "members": [
    { "agentId": "team-lead@$TEAM", "name": "team-lead", "joinedAt": 1783000000000 },
    { "agentId": "worker@$TEAM", "name": "worker", "joinedAt": 1783000000000, "isActive": true }
  ] }
EOF

log "launch real teammate 'worker' (its turn will run a shell_command)"
PANE="$(tmux new-session -d -P -F '#{pane_id}' -x 200 -y 50 -c "$WORKSPACE" -s "$SESSION" -- \
  env CODEX_HOME="$ROOT/home" OPENAI_API_KEY=mock CODEX_TEAM_STORE_ROOT="$ROOT/home" \
  "$CODEX_BIN" teammate --agent-id "worker@$TEAM" --agent-name worker --team-name "$TEAM" \
    --dangerously-bypass-hook-trust)"
echo "PANE=$PANE"

LEAD_INBOX="$ROOT/home/teams/$SAN/inboxes/team-lead.json"
log "wait for a kind:progress milestone to appear in the LEAD inbox"
found=0
for i in $(seq 1 60); do
  if [ -f "$LEAD_INBOX" ] && python3 - "$LEAD_INBOX" <<'PY'
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: sys.exit(1)
# success: a message tagged kind=progress from the worker whose text is a
# tool milestone (running: echo ...)
sys.exit(0 if any(m.get("kind")=="progress" and "running:" in m.get("text","") for m in d) else 1)
PY
  then found=1; echo "milestone observed after ${i}s"; break; fi
  sleep 1
done

log "lead inbox contents"
[ -f "$LEAD_INBOX" ] && python3 - "$LEAD_INBOX" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
for m in d:
    print(f"  kind={str(m.get('kind','-')):9} role={str(m.get('source_role','-')):8} from={m['from']:10} {m['text'][:55]}")
PY

if [ "$found" = 1 ]; then
  ok "teammate pushed a kind:progress tool milestone to the lead mid-turn"
else
  bad "no progress milestone reached the lead inbox"
  echo "--- teammate pane tail ---"; tmux capture-pane -p -t "$PANE" 2>/dev/null | grep -vE '^\s*$' | tail -8
fi

curl -s "http://127.0.0.1:${PORT}/shutdown" >/dev/null 2>&1 || true
exit 0
