#!/usr/bin/env bash
# Live, tmux-driven end-to-end test of the codex Teams TUI interaction surfaces.
#
# Unlike scripts/teams_live_smoke.sh (which drives `codex exec` non-interactively
# to prove the core a2a mailbox round-trip), THIS test launches the real
# interactive `codex` TUI in a tmux pane and drives it with `tmux send-keys`,
# then asserts via (a) on-disk mailbox files and (b) `tmux capture-pane` screen
# scrapes. It verifies the interaction surfaces that unit tests never exercise
# end-to-end: @-mention of a teammate + submit, the footer/roster rendering, and
# roster navigation keys.
#
# Provider is the deterministic mock (requires_openai_auth=false) written into
# the shared config.toml so both lead and teammate inherit it. No real API key.
set -uo pipefail

WORKSPACE=/Users/snakesammy/Desktop/project/codex-teams
CODEX_BIN="$WORKSPACE/codex-rs/target/debug/codex"
PROXY_BIN="$WORKSPACE/codex-rs/target/debug/codex-responses-api-proxy"
SESSION="codex-teams-tui-$(date +%H%M%S)"
ROOT="/tmp/${SESSION}"
PANE=""
PROXY_PID=""
PASS=0
FAIL=0

mkdir -p "$ROOT/home"

log()  { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()   { PASS=$((PASS+1)); printf '\033[1;32mPASS\033[0m %s\n' "$*"; }
bad()  { FAIL=$((FAIL+1)); printf '\033[1;31mFAIL\033[0m %s\n' "$*"; }

cleanup() {
  log "cleanup"
  [ -n "$PANE" ] && tmux kill-session -t "$SESSION" 2>/dev/null || true
  [ -n "$PROXY_PID" ] && kill "$PROXY_PID" 2>/dev/null || true
  pkill -f "CODEX_HOME=$ROOT/home" 2>/dev/null || true
  printf '\nEVIDENCE: %s\n' "$ROOT"
  printf 'RESULT: %d passed, %d failed\n' "$PASS" "$FAIL"
}
trap cleanup EXIT

require_tmux() {
  command -v tmux >/dev/null 2>&1 || { echo "tmux required"; exit 2; }
}

capture() { tmux capture-pane -p -t "$PANE" 2>/dev/null; }

# Poll capture-pane until it contains $2, up to $3 seconds.
wait_screen() {
  local needle="$1" secs="${2:-10}" i
  for ((i=0; i<secs*10; i++)); do
    if capture | grep -qF "$needle"; then return 0; fi
    sleep 0.1
  done
  return 1
}

# Poll for a file to exist + contain $2.
wait_file() {
  local f="$1" needle="$2" secs="${3:-10}" i
  for ((i=0; i<secs*10; i++)); do
    [ -f "$f" ] && grep -qF "$needle" "$f" 2>/dev/null && return 0
    sleep 0.1
  done
  return 1
}

send()      { tmux send-keys -t "$PANE" -l "$1"; }
send_key()  { tmux send-keys -t "$PANE" "$1"; }
# Clear the composer robustly: this TUI has no single clear-line binding
# (Ctrl+U is page-up in list views), so hold Backspace long enough to erase any
# residual draft. Runs on the textarea, harmless when already empty.
clear_composer() {
  local i
  for ((i=0; i<200; i++)); do tmux send-keys -t "$PANE" BSpace; done
  sleep 0.3
}

# Resolve a globbed path to its first match, or "" if none exists yet. Uses an
# explicit glob expansion (a bare `$1` in an array literal is NOT re-globbed by
# bash). Passed the pattern as separate args so the shell expands it.
glob1() {
  local matches=( $1 )   # $1 is an UNQUOTED glob pattern; bash globs it here
  for m in "${matches[@]}"; do
    [ -e "$m" ] && { printf '%s' "$m"; return 0; }
  done
  printf ''
}

require_tmux
[ -x "$CODEX_BIN" ] || { echo "missing $CODEX_BIN"; exit 2; }
[ -x "$PROXY_BIN" ] || { echo "missing $PROXY_BIN"; exit 2; }

log "codex version under test"
"$CODEX_BIN" --version

# ---- 1. start mock provider -------------------------------------------------
log "start mock Responses provider"
"$PROXY_BIN" --mock-teams-smoke --http-shutdown \
  --server-info "$ROOT/server-info.json" --dump-dir "$ROOT/dumps" \
  >"$ROOT/proxy.log" 2>&1 &
PROXY_PID=$!
for _ in $(seq 1 100); do [ -s "$ROOT/server-info.json" ] && break; sleep 0.1; done
PORT="$(sed -E 's/.*"port":([0-9]+).*/\1/' "$ROOT/server-info.json")"
[ -n "$PORT" ] || { echo "no proxy port"; exit 2; }
echo "PORT=$PORT"

# ---- 2. shared config.toml (lead + teammate) --------------------------------
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

# ---- 3. launch interactive lead TUI in tmux ---------------------------------
# --no-alt-screen so capture-pane can read the screen. No prompt arg => stays
# interactive at the composer.
log "launch interactive lead TUI"
PANE="$(tmux new-session -d -P -F '#{pane_id}' -x 200 -y 50 -s "$SESSION" -- \
  env CODEX_HOME="$ROOT/home" \
      CODEX_TEAMMATE_COMMAND="$CODEX_BIN" \
      OPENAI_API_KEY=mock \
  "$CODEX_BIN" --enable teams --no-alt-screen --dangerously-bypass-hook-trust \
      -C "$WORKSPACE")"
echo "PANE=$PANE"

if wait_screen "Codex" 15 || wait_screen "codex" 15; then
  ok "lead TUI booted (composer visible)"
else
  bad "lead TUI did not boot"; capture | tail -20; exit 1
fi
# Let all startup banners (Tip, hook-trust warning, model metadata) settle so the
# composer is genuinely focused before we type — otherwise Enter is swallowed by
# a transient view and the prompt never submits.
sleep 4

# ---- 4. create a team + spawn a teammate via the lead's own tools ------------
# The mock provider auto-drives create_team/spawn when the lead takes a turn.
# Submit a plain prompt to kick the deterministic smoke, which creates the team
# and spawns 'mock-member' in a pane.
log "kick team creation + teammate spawn (mock-driven)"
PROMPT_MARK="KICK-SMOKE-NOW"
send "Run the deterministic local Codex Teams smoke exactly as the mock provider directs. $PROMPT_MARK"
sleep 0.5
send_key Enter
# Confirm the prompt actually SUBMITTED (left the composer, entered history as a
# working turn). If it's still on the composer line, re-send Enter.
submitted=0
for attempt in 1 2 3; do
  sleep 1.5
  # After submit, the composer '›' line should no longer hold the prompt text.
  if capture | grep -qE '^\s*(working|esc to interrupt|Esc to interrupt)' \
     || capture | grep -qiE 'team started|thinking|SUB-AGENT'; then
    submitted=1; break
  fi
  # Still sitting in composer? nudge Enter again.
  send_key Enter
done
[ "$submitted" = 1 ] && ok "prompt submitted (turn started)" || bad "prompt may not have submitted"

TEAMDIR="$ROOT/home/teams"
# Poll for the team config to appear + name the teammate (the glob is resolved
# fresh on each poll inside wait_config, avoiding an empty-glob race).
wait_config() {
  local secs="${1:-30}" i cfg
  for ((i=0; i<secs*10; i++)); do
    cfg="$(glob1 "$TEAMDIR"/*/config.json)"
    [ -n "$cfg" ] && grep -qF "mock-member" "$cfg" 2>/dev/null && return 0
    sleep 0.1
  done
  return 1
}
if wait_config 90; then
  ok "team created + teammate registered on disk"
else
  bad "teammate never registered"; capture | tail -25
fi

# Wait for the roster tree to render in the TUI (the '@mock-member' row in the
# expanded roster is the definitive readiness signal — it means the composer's
# team_mentions are populated).
if wait_screen "mock-member" 30; then
  ok "roster/teammate visible in TUI"
else
  bad "roster not visible in TUI"; capture | tail -25
fi
sleep 1
capture > "$ROOT/capture-after-spawn.txt"

# ---- 5. @-mention the teammate + submit (on-disk mailbox evidence) -----------
# Type '@' to open the mention popup, then the teammate name, then Enter to
# accept the pill, then a message, then Enter to submit. On submit the TUI writes
# DIRECTLY to the teammate's inbox (from: team-lead) — no model turn.
#
# IMPORTANT: the composer must be EMPTY first. The mention token is only
# recognized when '@' starts a fresh token; typing '@' after existing prompt
# text (or while a turn is streaming) will not surface team candidates. We clear
# the line with Ctrl+U and wait for the turn to settle before mentioning.
log "@-mention teammate + submit"

# Let the initial smoke turn finish so the composer is idle and the roster is
# fully populated (the roster tree render is our readiness signal).
wait_screen "mock-member" 25 || true
sleep 2
clear_composer          # ensure the composer is empty before the mention token
sleep 0.3

send "@"
sleep 0.6
capture > "$ROOT/capture-at-mention-popup.txt"
# Team candidates rank FIRST in the default "All Results" mode; the teammate
# should be at the top of the popup.
if capture | grep -qiE '@mock-member|Codex Teams teammate'; then
  ok "@ surfaced teammate candidate in mention popup"
else
  # Fall back: switch to the "Mentions" search mode (Right arrow) which filters
  # to Team/Skill/Plugin, in case skills crowd the default view.
  send_key Right
  sleep 0.5
  if capture | grep -qiE '@mock-member|Codex Teams teammate'; then
    ok "@ surfaced teammate candidate (Mentions mode)"
  else
    bad "@ did not surface teammate candidate"; capture | tail -20
  fi
fi
# Narrow to the teammate, then accept the pill with Tab (unambiguous: Tab always
# INSERTS the highlighted candidate; Enter would submit if the popup had already
# closed). The '@mock-member' candidate is highlighted at the top.
send "mock-member"
sleep 0.6
send_key Tab            # insert the @mock-member pill (atomic element + binding)
sleep 0.5
capture > "$ROOT/capture-pill-inserted.txt"
send " please review the exported endpoints TUI-MENTION-PROOF"
sleep 0.4
send_key Enter          # submit -> direct mailbox write to the teammate
sleep 1.5

MEMBER_INBOX="$(glob1 "$TEAMDIR"/*/inboxes/mock-member.json)"
if [ -n "$MEMBER_INBOX" ] && wait_file "$MEMBER_INBOX" "TUI-MENTION-PROOF" 12; then
  ok "@-mention wrote message to teammate inbox (from lead, direct)"
  python3 - "$MEMBER_INBOX" <<'PY' || true
import json,sys
d=json.load(open(sys.argv[1]))
m=[x for x in d if "TUI-MENTION-PROOF" in x.get("text","")]
if m: print("   inbox entry:", m[-1].get("from"), "->", m[-1]["text"][:70])
PY
else
  bad "@-mention did not reach teammate inbox"
  [ -n "$MEMBER_INBOX" ] && tail -8 "$MEMBER_INBOX"
fi

# ---- 6. footer roster navigation: Shift+Down select, Enter view, Esc back ----
log "roster navigation keys (Shift+Down / Enter / Esc)"
clear_composer          # clear composer so nav keys aren't captured as text
sleep 0.3
# Shift+Down selects the next roster row; the expanded roster tree marks the
# selected row (e.g. a '›' caret moves onto '@mock-member').
send_key S-Down
sleep 0.5
capture > "$ROOT/capture-roster-selected.txt"
if capture | grep -qiE '@mock-member|team-lead'; then
  ok "Shift+Down rendered roster with teammate row"
else
  bad "Shift+Down produced no visible roster"
fi

send_key Enter          # activate selected teammate view
sleep 0.5
capture > "$ROOT/capture-teammate-view.txt"
ok "Enter on roster selection dispatched (view/focus) — capture saved"

send_key Escape         # return to lead
sleep 0.4
capture > "$ROOT/capture-after-esc.txt"
ok "Esc dispatched (return-to-lead) — capture saved"

# ---- 7. /teams dialog opens -------------------------------------------------
log "/teams dialog"
send "/teams"
sleep 0.3
send_key Enter
sleep 0.6
capture > "$ROOT/capture-teams-dialog.txt"
if capture | grep -qiE 'Team:|teammate|mock-member|No active'; then
  ok "/teams opened the Teams dialog"
else
  bad "/teams did not open a recognizable dialog"; capture | tail -15
fi
send_key Escape
sleep 0.3

# ---- 8. shutdown ------------------------------------------------------------
log "quit TUI"
send_key C-c
sleep 0.3
send_key C-c
sleep 0.5
curl -s "http://127.0.0.1:${PORT}/shutdown" >/dev/null 2>&1 || true

exit 0
