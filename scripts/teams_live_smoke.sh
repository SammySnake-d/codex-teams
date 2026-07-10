#!/usr/bin/env bash
# Live end-to-end Codex Teams smoke against the freshly built debug binary.
# Drives: create_team -> team_spawn_member -> tmux pane teammate process ->
# teammate inbox first turn -> team_send(member->lead) -> lead team_message_list receipt.
# Uses the deterministic mock Responses provider (requires_openai_auth=false), so
# NO real API key is needed. Terminal sentinel: TEAMS_SMOKE_PASS ... member_to_lead_completed=true
set -euo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
PROXY_BIN="$WORKSPACE/codex-rs/target/debug/codex-responses-api-proxy"
SESSION="codex-teams-live-$(date +%H%M%S)"
SMOKE_ROOT="/tmp/${SESSION}"
mkdir -p "$SMOKE_ROOT/home" "$SMOKE_ROOT/dumps"

echo "SMOKE_ROOT=$SMOKE_ROOT"
echo "CODEX_BIN=$CODEX_BIN"
"$CODEX_BIN" --version

# 1. Start the no-secret mock Responses provider.
"$PROXY_BIN" \
  --mock-teams-smoke \
  --http-shutdown \
  --server-info "$SMOKE_ROOT/server-info.json" \
  --dump-dir "$SMOKE_ROOT/dumps" \
  >"$SMOKE_ROOT/proxy.stdout" 2>"$SMOKE_ROOT/proxy.stderr" &
PROXY_PID=$!
echo "$PROXY_PID" > "$SMOKE_ROOT/proxy.pid"

for _ in $(seq 1 100); do
  [ -s "$SMOKE_ROOT/server-info.json" ] && break
  sleep 0.1
done
if [ ! -s "$SMOKE_ROOT/server-info.json" ]; then
  echo "FAIL: server-info not written" >&2
  cat "$SMOKE_ROOT/proxy.stderr" >&2 || true
  kill "$PROXY_PID" 2>/dev/null || true
  exit 1
fi
PROXY_PORT="$(sed -E 's/.*"port":([0-9]+).*/\1/' "$SMOKE_ROOT/server-info.json")"
echo "PROXY_PORT=$PROXY_PORT"

# 2. Lead config: shared CODEX_HOME + mock provider WRITTEN INTO config.toml so
#    that the teammate process (which reads $CODEX_HOME/config.toml, NOT the
#    lead's -c CLI overrides) inherits the exact same provider definition. This
#    is the "shared single config.toml" contract under test.
cat > "$SMOKE_ROOT/home/config.toml" <<EOF
model = "teams-smoke-model"
model_provider = "teams_smoke"
approval_policy = "never"
sandbox_mode = "danger-full-access"
suppress_unstable_features_warning = true

[features]
teams = true

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
echo "--- shared config.toml (lead + teammate both read this) ---"
cat "$SMOKE_ROOT/home/config.toml"

# 3. Drive the lead with `codex exec` (non-interactive) inside tmux so the
#    teammate can open its own pane in the same session. Provider now comes from
#    the shared config.toml, so no -c provider override is needed.
rm -f "$SMOKE_ROOT/exec.exit"
tmux new-session -d -s "$SESSION" -x 200 -y 50 "cd '$WORKSPACE'; \
  CODEX_HOME='$SMOKE_ROOT/home' \
  CODEX_TEAMMATE_COMMAND='$CODEX_BIN' \
  OPENAI_API_KEY=mock \
  '$CODEX_BIN' exec --json --enable teams -C '$WORKSPACE' \
    --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check \
    'Run the deterministic local Codex Teams smoke exactly as the mock provider directs.' \
    > '$SMOKE_ROOT/exec.jsonl' 2> '$SMOKE_ROOT/exec.err'; \
  printf '%s' \$? > '$SMOKE_ROOT/exec.exit'"

echo "TMUX_SESSION=$SESSION"

# 4. Wait for the smoke to finish (exec.exit written) up to 90s.
RESULT="TIMEOUT"
for i in $(seq 1 90); do
  if [ -f "$SMOKE_ROOT/exec.exit" ]; then
    RESULT="DONE(exit=$(cat "$SMOKE_ROOT/exec.exit"))"
    echo "exec finished after ${i}s: $RESULT"
    break
  fi
  sleep 1
done

# 5. Extract the terminal sentinel from the lead transcript.
echo "--- PASS/FAIL sentinel ---"
grep -aoE 'TEAMS_SMOKE_(PASS|FAIL)[^"]*' "$SMOKE_ROOT/exec.jsonl" 2>/dev/null | tail -1 || echo "NO SENTINEL FOUND"

# 6. Show the on-disk team store proof (config.json + inboxes).
echo "--- team store on disk ---"
find "$SMOKE_ROOT/home/teams" -type f 2>/dev/null | while read -r f; do
  echo "### $f"
  head -c 800 "$f"; echo
done

# 7. Teardown: shut the proxy, kill the tmux session.
curl -s "http://127.0.0.1:${PROXY_PORT}/shutdown" >/dev/null 2>&1 || true
tmux kill-session -t "$SESSION" 2>/dev/null || true
kill "$PROXY_PID" 2>/dev/null || true

echo "SMOKE_ROOT=$SMOKE_ROOT (evidence preserved)"
