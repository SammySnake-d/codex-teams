#!/usr/bin/env bash
# Live proof of MULTI-SOURCE ARBITRATION (gap #6) with a real teammate process.
#
# The unit tests prove select_next_inbox's ordering; this proves it end-to-end:
# a real `codex teammate` process, whose inbox is pre-loaded with a lower-priority
# peer DISCUSSION followed by a higher-priority reviewer CORRECTION, must consume
# the CORRECTION first (arbitration) — not FIFO order — and neither message is
# dropped. Evidence is on-disk: the correction is marked read before the
# discussion.
#
# We drive the teammate directly (no lead needed) against the mock provider so
# the turn completes deterministically. No real API key.
set -uo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
PROXY_BIN="$WORKSPACE/codex-rs/target/debug/codex-responses-api-proxy"
SESSION="codex-teams-arb-$(date +%H%M%S)"
ROOT="/tmp/${SESSION}"
PROXY_PID=""
PANE=""
PASS=0; FAIL=0
mkdir -p "$ROOT/home"

log()  { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()   { PASS=$((PASS+1)); printf '\033[1;32mPASS\033[0m %s\n' "$*"; }
bad()  { FAIL=$((FAIL+1)); printf '\033[1;31mFAIL\033[0m %s\n' "$*"; }
cleanup() {
  [ -n "$PANE" ] && tmux kill-session -t "$SESSION" 2>/dev/null || true
  [ -n "$PROXY_PID" ] && kill "$PROXY_PID" 2>/dev/null || true
  pkill -f "CODEX_HOME=$ROOT/home" 2>/dev/null || true
  printf '\nEVIDENCE: %s\nRESULT: %d passed, %d failed\n' "$ROOT" "$PASS" "$FAIL"
}
trap cleanup EXIT

glob1() { local m=( $1 ); for x in "${m[@]}"; do [ -e "$x" ] && { printf "%s" "$x"; return 0; }; done; printf ""; }

command -v tmux >/dev/null || { echo "tmux required"; exit 2; }
[ -x "$CODEX_BIN" ] || { echo "missing $CODEX_BIN"; exit 2; }
[ -x "$PROXY_BIN" ] || { echo "missing $PROXY_BIN"; exit 2; }

log "version under test"; "$CODEX_BIN" --version

# 1. mock provider
"$PROXY_BIN" --mock-teams-smoke --http-shutdown \
  --server-info "$ROOT/server-info.json" --dump-dir "$ROOT/dumps" \
  >"$ROOT/proxy.log" 2>&1 &
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

# 3. Pre-load the teammate 'worker' inbox with a DISCUSSION (earlier) then a
#    CORRECTION (later). FIFO would pick the discussion; arbitration must pick
#    the correction. We write the raw mailbox JSON the way the store serializes
#    it (camelCase not needed — TeammateMessage is snake plain serde).
TEAM="arb team"
TEAMDIR_SAN="arb_team"          # team_store::sanitize replaces space with _
INBOX_DIR="$ROOT/home/teams/$TEAMDIR_SAN/inboxes"
mkdir -p "$INBOX_DIR"
cat > "$INBOX_DIR/worker.json" <<'EOF'
[
  {
    "from": "peer-bob",
    "text": "brainstorm: maybe we should also refactor the parser?",
    "timestamp": "2026-07-11T00:00:01.000Z",
    "read": false,
    "kind": "discussion",
    "source_role": "peer"
  },
  {
    "from": "reviewer-carol",
    "text": "CORRECTION: you are editing the wrong file. Stop and switch to auth.rs. ARB-CORRECTION-FIRST",
    "timestamp": "2026-07-11T00:00:02.000Z",
    "read": false,
    "kind": "correction",
    "source_role": "reviewer"
  }
]
EOF
# minimal team config so the teammate process resolves its team
cat > "$ROOT/home/teams/$TEAMDIR_SAN/config.json" <<EOF
{
  "name": "$TEAM",
  "createdAt": 1783000000000,
  "leadAgentId": "team-lead@$TEAM",
  "members": [
    { "agentId": "team-lead@$TEAM", "name": "team-lead", "joinedAt": 1783000000000 },
    { "agentId": "worker@$TEAM", "name": "worker", "joinedAt": 1783000000000, "isActive": true }
  ]
}
EOF

log "inbox pre-loaded: [discussion(peer) @t1, correction(reviewer) @t2]"

# 4. Launch the real teammate process. Its run-loop calls select_next_inbox and
#    processes ONE message per turn. After the first turn, the CORRECTION must be
#    marked read (arbitration) while the discussion stays unread.
log "launch real teammate process (worker)"
PANE="$(tmux new-session -d -P -F '#{pane_id}' -x 200 -y 50 -c "$WORKSPACE" -s "$SESSION" -- \
  env CODEX_HOME="$ROOT/home" OPENAI_API_KEY=mock CODEX_TEAM_STORE_ROOT="$ROOT/home" \
  "$CODEX_BIN" teammate \
    --agent-id "worker@$TEAM" --agent-name worker --team-name "$TEAM" \
    --dangerously-bypass-hook-trust)"
echo "PANE=$PANE"
sleep 3
echo "--- teammate pane boot state ---"
tmux capture-pane -p -t "$PANE" 2>/dev/null | grep -vE '^\s*$' | tail -6 || true

# 5. Proof of ORDER: the teammate processes one message per turn and marks it
#    read. The definitive evidence of arbitration is WHICH message reached the
#    model FIRST — captured in the mock proxy's request dumps. FIFO would send
#    the discussion first; arbitration sends the correction first. (Checking the
#    final `read` booleans is too coarse: both get consumed across turns.)
log "wait for the teammate to take its first turn(s)"
for i in $(seq 1 40); do
  [ -n "$(glob1 "$ROOT"/dumps/*request.json 2>/dev/null)" ] && { sleep 2; break; }
  sleep 1
done

log "which teammate message reached the model FIRST? (proxy dump order)"
verdict="$(python3 - "$ROOT/dumps" <<'PY'
import json, glob, os, sys
reqs = sorted(glob.glob(os.path.join(sys.argv[1], "*request.json")))
for f in reqs:
    b = json.dumps(json.load(open(f)))
    has_corr = "ARB-CORRECTION-FIRST" in b or "wrong file" in b
    has_disc = "refactor the parser" in b
    if has_corr and not has_disc:
        print("CORRECTION_FIRST"); break
    if has_disc and not has_corr:
        print("DISCUSSION_FIRST"); break
    if has_corr and has_disc:
        # both in same request: earliest text position wins
        print("CORRECTION_FIRST" if b.find("wrong file") < b.find("refactor the parser") else "DISCUSSION_FIRST"); break
else:
    print("NONE")
PY
)"
echo "verdict: $verdict"

log "final worker inbox state"
python3 - "$INBOX_DIR/worker.json" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
for m in d:
    print(f"  read={str(m['read']):5} kind={m.get('kind','-'):11} from={m['from']:16} {m['text'][:45]}")
PY

if [ "$verdict" = "CORRECTION_FIRST" ]; then
  ok "reviewer CORRECTION reached the model BEFORE peer DISCUSSION (arbitration, not FIFO)"
else
  bad "arbitration not observed (verdict=$verdict)"
fi

curl -s "http://127.0.0.1:${PORT}/shutdown" >/dev/null 2>&1 || true
exit 0
