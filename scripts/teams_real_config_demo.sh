#!/usr/bin/env bash
# REAL-CONFIG end-to-end Teams demo (NOT a mock). Uses your real ~/.codex config
# (custom provider on 127.0.0.1:8317, real gpt-5.5, real auth). Drives a real
# interactive lead TUI in tmux to: create a team, spawn a real teammate process,
# delegate a tiny concrete task, and let real a2a happen. Observes the real
# on-disk team store + mailboxes + capture-pane.
#
# WARNING: this consumes REAL model quota (both the lead and the teammate call
# gpt-5.5). It is intentionally SMALL (one trivial delegated task) to bound cost.
set -uo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
REAL_HOME="$HOME/.codex"
SESSION="codex-teams-realdemo-$(date +%H%M%S)"
EVID="/tmp/${SESSION}"
PANE=""
DEMO_TEAM="real-demo"
mkdir -p "$EVID"

log() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()  { printf '\033[1;32m OK \033[0m %s\n' "$*"; }
note(){ printf '\033[1;33m ·  \033[0m %s\n' "$*"; }
cleanup() {
  log "cleanup (leaving real ~/.codex team store intact for inspection)"
  [ -n "$PANE" ] && tmux kill-session -t "$SESSION" 2>/dev/null || true
  # kill the demo's own lead + any teammate it spawned (scoped to this session).
  pkill -f "team-name $DEMO_TEAM" 2>/dev/null || true
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  printf '\nEVIDENCE: %s\n' "$EVID"
}
trap cleanup EXIT
glob1() { local m=( $1 ); for x in "${m[@]}"; do [ -e "$x" ] && { printf "%s" "$x"; return 0; }; done; printf ""; }
capture() { tmux capture-pane -p -t "$PANE" 2>/dev/null; }
wait_screen() { local n="$1" s="${2:-20}" i; for ((i=0;i<s*10;i++)); do capture | grep -qiF "$n" && return 0; sleep 0.1; done; return 1; }

command -v tmux >/dev/null || { echo "tmux required"; exit 2; }
[ -x "$CODEX_BIN" ] || { echo "missing $CODEX_BIN"; exit 2; }

log "version + real provider preflight"
"$CODEX_BIN" --version
if lsof -iTCP:8317 -sTCP:LISTEN >/dev/null 2>&1; then ok "custom provider listening on 127.0.0.1:8317"; else
  echo "FAIL: real provider (8317) not listening — start it first"; exit 2; fi
[ -f "$REAL_HOME/auth.json" ] && ok "auth.json present" || { echo "FAIL: no auth.json"; exit 2; }

# Isolate this demo's team under a UNIQUE team name so we never collide with any
# real team you already have, and cleanup is scoped. We do NOT touch your
# config.toml — teammates inherit the real one via the shared CODEX_HOME.

log "launch REAL lead TUI (your ~/.codex config, gpt-5.5)"
PANE="$(tmux new-session -d -P -F '#{pane_id}' -x 200 -y 50 -c "$WORKSPACE" -s "$SESSION" -- \
  env CODEX_HOME="$REAL_HOME" CODEX_TEAMMATE_COMMAND="$CODEX_BIN" \
  "$CODEX_BIN" --enable teams --no-alt-screen --dangerously-bypass-hook-trust)"
echo "PANE=$PANE"

if wait_screen "gpt-5.5" 20 || wait_screen "Codex" 20; then ok "lead TUI booted on real config"; else
  echo "FAIL: lead did not boot"; capture | tail -15; exit 1; fi
# scan for the 401 you asked about — must be ABSENT on real config
sleep 3
if capture | grep -qiE '401|api.openai.com|Test API Key'; then
  note "WARNING: saw a 401/openai marker on REAL config — investigate"; capture | grep -iE '401|openai' | head
else ok "no 401 / api.openai.com / Test API Key on real config"; fi
# Let ALL startup banners settle (MCP failures, under-development warnings, hook
# trust) so the composer is genuinely focused before typing — otherwise Enter is
# swallowed by a transient view and the prompt never submits.
sleep 8

# Give the lead one concrete instruction. Phrased IMPERATIVELY so the model acts
# immediately via tool calls instead of exploring the codebase first (observed:
# a research-style prompt makes gpt spend the turn reading files).
log "instruct the lead to run a small real a2a collaboration"
PROMPT="Do this now with tool calls, do NOT explore the codebase, do NOT read any files. \
Step 1: call create_team with team_name=${DEMO_TEAM}. \
Step 2: call team_spawn_member to spawn a teammate named scout with a prompt telling scout to immediately use team_send to reply to team-lead with exactly REAL_A2A_OK. \
Step 3: wait for scout's reply, then tell me you received REAL_A2A_OK. \
Start with create_team right now."
tmux send-keys -t "$PANE" -l "$PROMPT"
sleep 1
tmux send-keys -t "$PANE" Enter
# Confirm the turn actually STARTED (the prompt left the composer '›' line and a
# working/tool indicator appeared). Retry Enter a few times if a transient view
# swallowed it.
submitted=0
for attempt in $(seq 1 5); do
  sleep 2
  if capture | grep -qiE 'working|esc to interrupt|create_team|team started|thinking|Team started'; then
    submitted=1; break
  fi
  # still sitting in the composer? nudge Enter again.
  tmux send-keys -t "$PANE" Enter
done
[ "$submitted" = 1 ] && ok "prompt submitted (turn started)" || note "prompt may not have submitted; continuing to watch disk"

log "watch the REAL team store appear under ~/.codex/teams/${DEMO_TEAM}"
TEAMDIR="$REAL_HOME/teams"
found_cfg=""
for i in $(seq 1 120); do
  cfg="$(glob1 "$TEAMDIR"/*${DEMO_TEAM}*/config.json)"
  [ -z "$cfg" ] && cfg="$(glob1 "$TEAMDIR"/*real*demo*/config.json)"
  if [ -n "$cfg" ] && grep -qiE 'scout' "$cfg" 2>/dev/null; then found_cfg="$cfg"; break; fi
  sleep 1
done
if [ -n "$found_cfg" ]; then
  ok "real team created + teammate 'scout' registered: $found_cfg"
  cp "$found_cfg" "$EVID/config.json" 2>/dev/null
  python3 -c "import json;d=json.load(open('$found_cfg'));print('   members:',[m['name'] for m in d['members']])" 2>/dev/null
else
  note "team/teammate not observed within 120s (the model may still be working)"
  capture > "$EVID/capture-lead.txt"; capture | tail -20
fi

# Look for the real a2a round-trip: scout -> lead inbox with REAL_A2A_OK.
if [ -n "$found_cfg" ]; then
  TEAMSUBDIR="$(dirname "$found_cfg")"
  LEAD_INBOX="$TEAMSUBDIR/inboxes/team-lead.json"
  log "watch for scout's real reply (REAL_A2A_OK) in the lead inbox"
  a2a=0
  for i in $(seq 1 120); do
    if [ -f "$LEAD_INBOX" ] && grep -qF "REAL_A2A_OK" "$LEAD_INBOX" 2>/dev/null; then a2a=1; break; fi
    sleep 1
  done
  if [ "$a2a" = 1 ]; then
    ok "REAL a2a round-trip: scout replied REAL_A2A_OK to the lead (real gpt-5.5, both processes)"
    cp "$LEAD_INBOX" "$EVID/lead-inbox.json" 2>/dev/null
    python3 -c "import json;d=json.load(open('$LEAD_INBOX'));[print('   ',m['from'],'->',m['text'][:60]) for m in d if 'idle' not in m.get('text','')]" 2>/dev/null
  else
    note "scout reply not seen in 120s; capturing state for inspection"
    capture > "$EVID/capture-lead.txt"
    [ -f "$LEAD_INBOX" ] && cp "$LEAD_INBOX" "$EVID/lead-inbox.json"
  fi
fi

log "final lead screen"
capture > "$EVID/capture-final.txt"
capture | tail -18
echo ""
note "team store preserved at ~/.codex/teams/ for your inspection; press-nothing, session will be cleaned."
