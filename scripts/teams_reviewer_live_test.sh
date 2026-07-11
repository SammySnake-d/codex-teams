#!/usr/bin/env bash
# Live proof of the REVIEWER OBSERVATION CHANNEL (gap #1) with real teammate
# processes. Two things proven end-to-end, no mock stubs of the mechanism:
#
#   Phase 1 — team_watch writes a subscription: a real reviewer teammate calls
#     team_watch(worker) (mock-driven via WATCH-PROBE); the shared config.json
#     gains reviewer.subscriptions=[worker].
#   Phase 2 — progress fans out to the subscriber: a real worker teammate runs a
#     shell tool (mock-driven via MILESTONE-PROBE); its kind:progress milestone
#     is delivered to BOTH the lead AND the subscribed reviewer's inbox.
#
# Together this is the reviewer scenario: reviewer subscribes to the worker, then
# sees the worker's tool milestones as they happen (and could team_send a
# correction — that correction path is already proven by the arbitration test).
# Uses the deterministic mock provider (requires_openai_auth=false). No real key.
set -uo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
PROXY_BIN="$WORKSPACE/codex-rs/target/debug/codex-responses-api-proxy"
SESSION="codex-teams-rev-$(date +%H%M%S)"
ROOT="/tmp/${SESSION}"
PROXY_PID=""; PANE_R=""; PANE_W=""; PASS=0; FAIL=0
mkdir -p "$ROOT/home"

log() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()  { PASS=$((PASS+1)); printf '\033[1;32mPASS\033[0m %s\n' "$*"; }
bad() { FAIL=$((FAIL+1)); printf '\033[1;31mFAIL\033[0m %s\n' "$*"; }
cleanup() {
  [ -n "$PANE_R" ] && tmux kill-session -t "${SESSION}-r" 2>/dev/null || true
  [ -n "$PANE_W" ] && tmux kill-session -t "${SESSION}-w" 2>/dev/null || true
  [ -n "$PROXY_PID" ] && kill "$PROXY_PID" 2>/dev/null || true
  pkill -f "CODEX_HOME=$ROOT/home" 2>/dev/null || true
  printf '\nEVIDENCE: %s\nRESULT: %d passed, %d failed\n' "$ROOT" "$PASS" "$FAIL"
}
trap cleanup EXIT

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

TEAM="rev team"; SAN="rev_team"
INBOX="$ROOT/home/teams/$SAN/inboxes"
CFG="$ROOT/home/teams/$SAN/config.json"
mkdir -p "$INBOX"
cat > "$CFG" <<EOF
{ "name": "$TEAM", "createdAt": 1783000000000, "leadAgentId": "team-lead@$TEAM",
  "members": [
    { "agentId": "team-lead@$TEAM", "name": "team-lead", "joinedAt": 1783000000000 },
    { "agentId": "worker@$TEAM",   "name": "worker",   "joinedAt": 1783000000000, "isActive": true },
    { "agentId": "reviewer@$TEAM", "name": "reviewer", "joinedAt": 1783000000000, "isActive": true }
  ] }
EOF

# ---- Phase 1: reviewer calls team_watch(worker) -----------------------------
log "Phase 1: launch real reviewer teammate; it calls team_watch(worker)"
cat > "$INBOX/reviewer.json" <<'EOF'
[ { "from": "team-lead", "text": "You review the worker. WATCH-PROBE worker", "timestamp": "2026-07-12T00:00:01.000Z", "read": false } ]
EOF
PANE_R="$(tmux new-session -d -P -F '#{pane_id}' -x 200 -y 50 -c "$WORKSPACE" -s "${SESSION}-r" -- \
  env CODEX_HOME="$ROOT/home" OPENAI_API_KEY=mock CODEX_TEAM_STORE_ROOT="$ROOT/home" \
  "$CODEX_BIN" teammate --agent-id "reviewer@$TEAM" --agent-name reviewer --team-name "$TEAM" \
    --dangerously-bypass-hook-trust)"

watched=0
for i in $(seq 1 60); do
  if python3 - "$CFG" <<'PY'
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: sys.exit(1)
r=[m for m in d.get("members",[]) if m.get("name")=="reviewer"]
sys.exit(0 if r and "worker" in (r[0].get("subscriptions") or []) else 1)
PY
  then watched=1; echo "subscription written after ${i}s"; break; fi
  sleep 1
done
if [ "$watched" = 1 ]; then
  ok "team_watch wrote reviewer.subscriptions=[worker] to shared config"
  python3 -c "import json;d=json.load(open('$CFG'));print('   reviewer.subscriptions:',[m for m in d['members'] if m['name']=='reviewer'][0].get('subscriptions'))"
else
  bad "team_watch did not update subscriptions"; tmux capture-pane -p -t "$PANE_R" 2>/dev/null | tail -8
fi
tmux kill-session -t "${SESSION}-r" 2>/dev/null || true; PANE_R=""

# ---- Phase 2: worker runs a tool; progress reaches the reviewer -------------
log "Phase 2: launch real worker teammate; it runs a tool -> progress fans out"
cat > "$INBOX/worker.json" <<'EOF'
[ { "from": "team-lead", "text": "Do the probe now. MILESTONE-PROBE", "timestamp": "2026-07-12T00:00:02.000Z", "read": false } ]
EOF
# clear the reviewer + lead inboxes so Phase-2 progress is unambiguous
printf '[]' > "$INBOX/reviewer.json"
printf '[]' > "$INBOX/team-lead.json"
PANE_W="$(tmux new-session -d -P -F '#{pane_id}' -x 200 -y 50 -c "$WORKSPACE" -s "${SESSION}-w" -- \
  env CODEX_HOME="$ROOT/home" OPENAI_API_KEY=mock CODEX_TEAM_STORE_ROOT="$ROOT/home" \
  "$CODEX_BIN" teammate --agent-id "worker@$TEAM" --agent-name worker --team-name "$TEAM" \
    --dangerously-bypass-hook-trust)"

progress_in() { # $1 = inbox file
  python3 - "$1" <<'PY'
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: sys.exit(1)
sys.exit(0 if any(m.get("kind")=="progress" and "running:" in m.get("text","") for m in d) else 1)
PY
}
rev_got=0; lead_got=0
for i in $(seq 1 60); do
  progress_in "$INBOX/reviewer.json" && rev_got=1
  progress_in "$INBOX/team-lead.json" && lead_got=1
  [ "$rev_got" = 1 ] && [ "$lead_got" = 1 ] && { echo "both inboxes got progress after ${i}s"; break; }
  sleep 1
done

log "reviewer inbox"
[ -f "$INBOX/reviewer.json" ] && python3 - "$INBOX/reviewer.json" <<'PY'
import json,sys
for m in json.load(open(sys.argv[1])):
    print(f"  kind={str(m.get('kind','-')):9} from={m['from']:10} {m['text'][:50]}")
PY

if [ "$rev_got" = 1 ]; then
  ok "worker's tool milestone was delivered to the SUBSCRIBED reviewer's inbox (peer observation)"
else
  bad "reviewer did not receive worker progress"; tmux capture-pane -p -t "$PANE_W" 2>/dev/null | tail -8
fi
if [ "$lead_got" = 1 ]; then
  ok "lead still receives the milestone too (central monitoring preserved)"
else
  bad "lead did not receive worker progress"
fi

curl -s "http://127.0.0.1:${PORT}/shutdown" >/dev/null 2>&1 || true
exit 0
