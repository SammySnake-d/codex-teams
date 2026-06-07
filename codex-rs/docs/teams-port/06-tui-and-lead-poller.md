# Phase 6 — TUI Teams dialog/overlay + colored footer pills + lead inbox poller

Faithful Rust port of Claude Code's `TeamsDialog.tsx` and the lead-side inbox
pollers (`useInboxPoller` / `runHeadlessStreaming`) into `codex-rs/tui`, wired to
the on-disk `team_store` (Phase 1), the process-backed `codex teammate` path, and
the in-process fallback in `team.rs` / team tools. Truth source:
`ChinaSiro/claude-code-sourcemap`.

This phase has two halves:

1. **View** — a Teams dialog/overlay + colored teammate "pills" in the footer
   (extends `tui/src/chatwidget/team_ui.rs`), porting `TeamsDialog.tsx`.
2. **Lead inbox poller** — a 1 s background poll of
   `$CODEX_HOME/teams/{team}/inboxes/{lead}.json` that injects teammate replies
   into the lead session as new user turns, porting `useInboxPoller`.

Read this together with `TEAMS_CLAUDE_PORT_SPEC.md` (phase map) and
`TEAMS_CLAUDE_PARITY.md` (what already exists).

---

## A. Claude truth source — exact identifiers, strings, JSON shapes

### A.1 Files (Claude `restored-src/src/…`)
- `components/teams/TeamsDialog.tsx` — `TeamsDialog`, `TeamDetailView`,
  `TeammateListItem`, `TeammateDetailView`, `viewTeammateOutput`,
  `toggleTeammateVisibility`, `hideTeammate`/`showTeammate` (empty stubs in the
  external build), `sendModeChangeToTeammate`, `killTeammate`.
- `hooks/useInboxPoller.ts` — `useInboxPoller`, `getAgentNameToPoll`,
  `INBOX_POLL_INTERVAL_MS = 1000`.
- `cli/print.ts` — `runHeadlessStreaming` (headless lead), `POLL_INTERVAL_MS = 500`.
- `utils/swarm/teammateLayoutManager.ts` — `assignTeammateColor`,
  `getTeammateColor`, `clearTeammateColors`, `teammateColorAssignments` map,
  `colorIndex`.
- `tools/AgentTool/agentColorManager.ts` — `AGENT_COLORS`,
  `AGENT_COLOR_TO_THEME_COLOR`, `AgentColorName`.
- `utils/permissions/PermissionMode.ts` — `getModeColor`, `permissionModeSymbol`,
  `permissionModeFromString`, `PERMISSION_MODE_CONFIG`.
- `utils/swarm/teamHelpers.ts` — `addHiddenPaneId`, `removeHiddenPaneId`,
  `setMemberMode`, `setMemberActive`, `removeTeammateFromTeamFile`, `TeamFile`.
- `utils/swarm/constants.ts` — `TEAM_LEAD_NAME='team-lead'`,
  `SWARM_SESSION_NAME='claude-swarm'`, `TMUX_COMMAND='tmux'`,
  `getSwarmSocketName()` → `claude-swarm-${pid}`.
- `constants/xml.ts` — `TEAMMATE_MESSAGE_TAG='teammate-message'`.
- `utils/swarm/backends/detection.ts` — `IT2_COMMAND`, `isInsideTmuxSync()`.

### A.2 Per-teammate color assignment (round-robin)
`AGENT_COLORS` (order is load-bearing — round-robin index):
```ts
['red','blue','green','yellow','purple','orange','pink','cyan']
```
```ts
// teammateLayoutManager.ts
const teammateColorAssignments = new Map<string, AgentColorName>()
let colorIndex = 0
export function assignTeammateColor(teammateId: string): AgentColorName {
  const existing = teammateColorAssignments.get(teammateId)
  if (existing) return existing
  const color = AGENT_COLORS[colorIndex % AGENT_COLORS.length]!
  teammateColorAssignments.set(teammateId, color)
  colorIndex++
  return color
}
export function clearTeammateColors(): void {
  teammateColorAssignments.clear(); colorIndex = 0
}
```
`AGENT_COLOR_TO_THEME_COLOR` (color name → theme key, `*_FOR_SUBAGENTS_ONLY`):
```ts
{ red:'red_FOR_SUBAGENTS_ONLY', blue:'blue_FOR_SUBAGENTS_ONLY',
  green:'green_FOR_SUBAGENTS_ONLY', yellow:'yellow_FOR_SUBAGENTS_ONLY',
  purple:'purple_FOR_SUBAGENTS_ONLY', orange:'orange_FOR_SUBAGENTS_ONLY',
  pink:'pink_FOR_SUBAGENTS_ONLY', cyan:'cyan_FOR_SUBAGENTS_ONLY' }
```
For Codex these map to ratatui `Color` (see B.2). The color is also persisted on
disk: `TeamFileMember.color` (`team_store::TeamFileMember.color: Option<String>`,
holding one of the 8 names) and echoed back to the model in the
`<teammate-message color="...">` wrapper.

### A.3 Permission-mode symbol + color (`PERMISSION_MODE_CONFIG`)
Used by `TeammateListItem` to draw the leading mode pill (`modeSymbol` in
`modeColor`):
```
default            symbol ''     color 'text'      (no pill)
plan               symbol ⏸      color 'planMode'  (PAUSE_ICON)
acceptEdits        symbol '⏵⏵'   color 'autoAccept'
bypassPermissions  symbol '⏵⏵'   color 'error'
dontAsk            symbol '⏵⏵'   color 'error'
```
```ts
export function getModeColor(mode){ return getModeConfig(mode).color }
export function permissionModeSymbol(mode){ return getModeConfig(mode).symbol }
```

### A.4 `TeammateListItem` render (the "pill" row)
Decompiled (react-compiler) shape — the row reads:
`{pointer/space}{[hidden]?}{[idle]?}{modeSymbol-in-modeColor}@{name}{model?}`,
dimmed when `isIdle && !isSelected`.
```tsx
const isIdle = teammate.status === 'idle'
const shouldDim = isIdle && !isSelected
const mode = teammate.mode ? permissionModeFromString(teammate.mode) : 'default'
const modeSymbol = permissionModeSymbol(mode)
const modeColor = getModeColor(mode)
// <Text color={isSelected?'suggestion':undefined} dimColor={shouldDim}>
//   {isSelected? figures.pointer+' ' : '  '}
//   {teammate.isHidden && '[hidden] '}
//   {isIdle && '[idle] '}
//   {modeSymbol && <Text color={modeColor}>{modeSymbol} </Text>}
//   @{teammate.name}
//   {teammate.model && <Text dimColor> ({teammate.model})</Text>}
// </Text>
```
`TeammateStatus.status ∈ {'running','idle','unknown'}`; `isHidden` derived from
`teamFile.hiddenPaneIds.includes(tmuxPaneId)`.

### A.5 `viewTeammateOutput` (focus a teammate's pane)
```ts
async function viewTeammateOutput(paneId, backendType) {
  if (backendType === 'iterm2') {
    await execFileNoThrow(IT2_COMMAND, ['session','focus','-s', paneId])
  } else {
    const args = isInsideTmuxSync()
      ? ['select-pane','-t', paneId]
      : ['-L', getSwarmSocketName(), 'select-pane','-t', paneId]
    await execFileNoThrow(TMUX_COMMAND, args)
  }
}
```

### A.6 `toggleTeammateVisibility` + hidden-pane persistence
```ts
async function toggleTeammateVisibility(teammate, teamName) {
  if (teammate.isHidden) await showTeammate(teammate, teamName)
  else                    await hideTeammate(teammate, teamName)
}
// hideTeammate/showTeammate are stubs in the external build; the on-disk
// truth is the hiddenPaneIds array, mutated by:
export function addHiddenPaneId(teamName, paneId): boolean   // push if absent → writeTeamFile
export function removeHiddenPaneId(teamName, paneId): boolean // splice if present → writeTeamFile
```
The real tmux hide/show is gated out of the external build; Codex ports the
**on-disk** semantics (mutate `hidden_pane_ids`) plus a best-effort tmux
break-pane/join-pane (see B.5), staying faithful to the persisted contract.

### A.7 `useInboxPoller` (REPL lead — inject teammate replies as turns)
Signature + behavior (1 s poll):
```ts
const INBOX_POLL_INTERVAL_MS = 1000
export function useInboxPoller({ enabled, isLoading, focusedInputDialog,
  onSubmitMessage }: Props): void
```
- `getAgentNameToPoll` returns the lead's display name (from
  `teamContext.teammates[leadAgentId].name`, fallback `'team-lead'`).
- `readUnreadMessages(agentName, teamName)` → unread inbox messages.
- Partitions control messages (permission/shutdown/mode/plan-approval) from
  `regularMessages`. **For the Codex port, only `regularMessages` matter** in
  this slice — the permission/plan-approval/sandbox branches are Claude-specific
  and remain out of scope even when the teammate is a separate `codex teammate`
  process.
- Formats each regular message and submits/queues:
```ts
const colorAttr = m.color ? ` color="${m.color}"` : ''
const summaryAttr = m.summary ? ` summary="${m.summary}"` : ''
`<${TEAMMATE_MESSAGE_TAG} teammate_id="${m.from}"${colorAttr}${summaryAttr}>\n${m.text}\n</${TEAMMATE_MESSAGE_TAG}>`
// joined by '\n\n'
```
- **Idle** (`!isLoading && !focusedInputDialog`): `onSubmitMessage(formatted)`
  immediately (a new user turn). If rejected (query running) → queue.
- **Busy**: queue into `AppState.inbox.messages` (status `'pending'`); a
  `useEffect` flushes pending messages when the session next goes idle.
- `markMessagesAsRead(agentName, teamName)` is called **only after** successful
  submit/queue (crash-safe: unread messages are re-read next poll).

### A.8 `runHeadlessStreaming` (headless lead — same logic, 500 ms)
```ts
const POLL_INTERVAL_MS = 500
while (true) {
  // stop when no active teammates
  const unread = await readUnreadMessages(agentName, teamName)
  if (unread.length > 0) {
    await markMessagesAsRead(agentName, teamName)          // mark first
    // (process shutdown_approved → removeTeammateFromTeamFile … omitted for codex)
    const formatted = unread.map(m =>
      `<${TEAMMATE_MESSAGE_TAG} teammate_id="${m.from}"${m.color?` color="${m.color}"`:''}>\n${m.text}\n</${TEAMMATE_MESSAGE_TAG}>`
    ).join('\n\n')
    enqueue({ mode:'prompt', value: formatted, uuid: randomUUID() })
    void run(); return
  }
  await sleep(POLL_INTERVAL_MS)
}
```

### A.9 Idle / `setMemberActive` (member → lead notification; Phase 2 owns the writer)
On a teammate's Stop hook (`initializeTeammateHooks`): `setMemberActive(team,
name, false)` then `writeToMailbox(lead, createIdleNotification())`. The idle
JSON is `{ kind:'idle', from, summary, color }` placed in `TeammateMessage.text`.
The lead poller (A.7) treats it as a regular message and surfaces it. **Writer is
Phase 2's teammate runner; Phase 6 only consumes the lead inbox + reads
`is_active` for the `[idle]` pill.**

---

## B. Codex target — files, structs, signatures, wiring

All new code lives under `codex-rs/tui`. No core changes are required for the
View; the poller reads `team_store` (Phase 1) and reuses existing TUI submission
paths. Color/mode helpers are small enough to live in the TUI.

### B.1 New module: `tui/src/chatwidget/team_colors.rs`
Port `assignTeammateColor` + the two lookup tables. Module path
`crate::chatwidget::team_colors`; declare `mod team_colors;` in
`tui/src/chatwidget.rs`.

```rust
//! Port of Claude's agentColorManager.ts + teammateLayoutManager.ts color logic.
use ratatui::style::Color;

/// Round-robin palette — order matches Claude `AGENT_COLORS`.
pub(crate) const AGENT_COLORS: [&str; 8] =
    ["red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan"];

/// Deterministic per-team color assignment. Mirrors `assignTeammateColor`:
/// stable per `teammate_id`, round-robin by first-seen order.
#[derive(Default)]
pub(crate) struct TeammateColors {
    assignments: std::collections::HashMap<String, &'static str>,
    next_index: usize,
}
impl TeammateColors {
    pub(crate) fn assign(&mut self, teammate_id: &str) -> &'static str { /* … */ }
    pub(crate) fn get(&self, teammate_id: &str) -> Option<&'static str> { /* … */ }
    pub(crate) fn clear(&mut self) { self.assignments.clear(); self.next_index = 0; }
}

/// `AGENT_COLOR_TO_THEME_COLOR` → ratatui. Unknown names fall back to `Color::White`.
pub(crate) fn agent_color_to_tui(name: &str) -> Color {
    match name {
        "red" => Color::Red, "blue" => Color::Blue, "green" => Color::Green,
        "yellow" => Color::Yellow, "purple" => Color::Magenta, "orange" => Color::Rgb(255,165,0),
        "pink" => Color::Rgb(255,105,180), "cyan" => Color::Cyan, _ => Color::White,
    }
}

/// Port of `PERMISSION_MODE_CONFIG`: (symbol, ratatui color). `default` → ("", _).
pub(crate) fn mode_symbol_and_color(mode: &str) -> (&'static str, Color) {
    match mode {
        "plan"               => ("\u{23F8}", Color::Cyan),     // PAUSE_ICON
        "acceptEdits"        => ("\u{23F5}\u{23F5}", Color::Green),
        "bypassPermissions" | "dontAsk" => ("\u{23F5}\u{23F5}", Color::Red),
        _ /* default */      => ("", Color::Reset),
    }
}
```
Persistence note: `TeammateColors::assign` should also write the chosen name into
`team_store::TeamFileMember.color` via `team_store::update_config(...)` when the
lead first sees a teammate, so the color survives restarts and is echoed in the
`color="…"` attribute. This matches Claude persisting `color` on the member.

### B.2 Extend `tui/src/chatwidget/team_ui.rs` — colored footer pills + roster fields
`TeamUiState`/`TeamMemberUiSummary` already track the roster from team-tool
output. Add three things, all additive:

1. Color + mode fields on `TeamMemberUiSummary`:
```rust
pub(super) struct TeamMemberUiSummary {
    // existing: id, name, agent_thread_id, status, agent_status
    color: Option<&'static str>, // from TeammateColors::assign(member.id)
    mode: Option<String>,        // permission mode if surfaced; default None
    is_hidden: bool,             // mirrors hidden_pane_ids
}
```
2. A `TeammateColors` owner on `TeamUiState` (assign on `MemberSpawned`):
```rust
#[derive(Default)]
pub(super) struct TeamUiState {
    // … existing fields …
    colors: crate::chatwidget::team_colors::TeammateColors,
}
```
   In `apply_team_tool_output` → `TeamSpawnMember` and `parse_team`, call
   `self.colors.assign(&member.id)` to populate `color`. Clear on `TeamStop`.
3. Pills in `footer_label`. Today it emits
   `@{name} {status_label}`. Make it carry color by switching the footer to a
   styled-spans builder. Add:
```rust
/// Styled footer roster (replaces the plain `footer_label` String when the
/// active-agent label can render spans). Each pill = mode symbol (in mode
/// color) + "@name" (in teammate color) + status suffix.
pub(super) fn footer_spans(&self) -> Option<Vec<ratatui::text::Span<'static>>>;
```
   Keep `footer_label() -> Option<String>` for the plain fallback. If
   `set_active_agent_label` only accepts `String` today (it does — see
   `bottom_pane/mod.rs:1720`), the minimal faithful port keeps the String API but
   appends a color marker per pill is NOT possible; instead add a sibling setter
   (see B.6) `set_active_team_pills(Vec<Span>)` so pills render in color while the
   existing `active_agent_label` String path is untouched.

`status_label()` already maps `agent_status` → starting/running/idle/etc.; the
`[idle]` pill = `is_active == Some(false)` (from `team_store`) OR
`status_label()=="idle"`. `[hidden]` = `is_hidden`.

### B.3 New module: `tui/src/chatwidget/teams_dialog.rs` (the overlay) — ports `TeamsDialog.tsx`
Module path `crate::chatwidget::teams_dialog`. A bottom-pane overlay/view with two
levels mirroring Claude's `dialogLevel`:

```rust
pub(crate) enum TeamsDialogLevel {
    TeammateList { team: String },
    TeammateDetail { team: String, member_name: String },
}

pub(crate) struct TeamsDialog {
    level: TeamsDialogLevel,
    selected_index: usize,
    teammates: Vec<TeammateRow>,     // refreshed from team_store every ~1s (useInterval)
    last_refresh: std::time::Instant,
}

/// Row backed by team_store::TeamFileMember + assigned color.
pub(crate) struct TeammateRow {
    pub name: String,
    pub agent_id: String,
    pub tmux_pane_id: String,
    pub backend_type: Option<String>,  // "tmux" | "iterm" | "in_process"
    pub model: Option<String>,
    pub mode: Option<String>,
    pub color: Option<String>,
    pub is_active: Option<bool>,        // → [idle] when Some(false)
    pub is_hidden: bool,                // pane_id ∈ hidden_pane_ids
}
```

Methods (port the dialog's `useInput` handlers + helpers):
```rust
impl TeamsDialog {
    pub(crate) fn open(team: String) -> Self;                 // start at TeammateList
    pub(crate) fn refresh(&mut self, codex_home: &Path);      // re-read config.json (useInterval, 1s)
    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme); // TeamDetailView / TeammateDetailView
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<TeamsDialogAction>;
}

/// Side-effects requested by the dialog, executed by App (needs tmux/exec).
pub(crate) enum TeamsDialogAction {
    ViewTeammateOutput { pane_id: String, backend_type: Option<String> }, // ↵ on detail
    ToggleVisibility   { team: String, pane_id: String, hide: bool },     // 'h'
    Close,
}
```
Key map (from `TeamsDialog.tsx` `useInput`): `j/k`+arrows move `selected_index`;
`↵` on a list row → drill into `TeammateDetail`; `↵` on detail →
`ViewTeammateOutput`; `h` → `ToggleVisibility`; `esc`/`q` → `Close`. (Claude also
has kill/shutdown on `x`/`d` and mode-cycle on tab — out of scope unless Phase 3
process teammates land; gate them behind a follow-up.)

`render` reproduces `TeammateListItem` exactly (B.1 helpers):
`{pointer}{[hidden] }{[idle] }{modeSymbol in modeColor}@{name}{ (model)}`, dimmed
when idle && not selected, selected row in the theme "suggestion" color.

### B.4 New module: `tui/src/app/lead_inbox_poller.rs` — ports `useInboxPoller`
Module path `crate::app::lead_inbox_poller`; declare `mod lead_inbox_poller;` in
`tui/src/app.rs`. This is the lead session reading **its own** inbox.

Wiring shape: a background tokio task spawned when the lead session has an active
team, polling every 1 s (`INBOX_POLL_INTERVAL_MS`). It does not touch the UI
directly; it emits an `AppEvent` (B.6) that the app loop turns into a user turn.

```rust
pub(crate) struct LeadInboxPoller {
    handle: tokio::task::JoinHandle<()>,
    stop: tokio_util::sync::CancellationToken,
}

/// Start polling `$CODEX_HOME/teams/{team}/inboxes/{lead}.json` every 1s.
/// `lead_name` defaults to team_store::TEAM_LEAD_NAME ("team-lead").
pub(crate) fn start_lead_inbox_poller(
    codex_home: PathBuf,
    team: String,
    lead_name: String,
    app_event_tx: AppEventSender,
) -> LeadInboxPoller;

/// Port of the formatting in useInboxPoller / runHeadlessStreaming.
/// Wraps each TeammateMessage in <teammate-message …>; joins with "\n\n".
pub(crate) fn format_teammate_messages(msgs: &[team_store::TeammateMessage]) -> String;
```
Poll body (port of A.7/A.8, regular-messages-only):
1. `let unread = team_store::read_unread(&codex_home, &team, &lead_name)?;`
   (skip control-message partitioning — codex has no cross-process perm bridge).
2. If empty → return.
3. `let formatted = format_teammate_messages(&unread);`
4. `app_event_tx.send(AppEvent::InjectTeammateReplies { text: formatted });`
5. `team_store::mark_messages_read(&codex_home, &team, &lead_name)?;` **after** the
   send (crash-safe re-read, matching Claude's "mark only after delivery").

`format_teammate_messages` (exact):
```rust
const TEAMMATE_MESSAGE_TAG: &str = "teammate-message"; // Claude constants/xml.ts
// per message:
//   let color = m.color.map(|c| format!(" color=\"{c}\"")).unwrap_or_default();
//   let summary = m.summary.map(|s| format!(" summary=\"{s}\"")).unwrap_or_default();
//   format!("<{TAG} teammate_id=\"{}\"{color}{summary}>\n{}\n</{TAG}>", m.from, m.text)
// join("\n\n")
```

### B.5 `viewTeammateOutput` + visibility in `App` (tmux side-effects)
Put the exec in `tui/src/app/teammate_panes.rs` (already the tmux home). Add:
```rust
impl App {
    /// Port of viewTeammateOutput: focus a teammate's pane.
    pub(super) fn focus_teammate_pane(&self, pane_id: &str, backend_type: Option<&str>);
    /// Port of toggleTeammateVisibility's on-disk half + best-effort tmux move.
    pub(super) fn set_teammate_pane_hidden(&self, team: &str, pane_id: &str, hide: bool);
}
```
`focus_teammate_pane` mirrors A.5:
- `Some("iterm")` → `Command::new(IT2)…["session","focus","-s",pane_id]` (IT2 cmd
  string TBD — see Phase 6 iTerm in `05-…`/Phase 6 of the master spec).
- else tmux: inside-tmux → `tmux select-pane -t <pane_id>`; outside →
  `tmux -L codex-swarm-<pid> select-pane -t <pane_id>` (socket name analog of
  `getSwarmSocketName()`; codex uses `codex-swarm` per `TEAMS_CLAUDE_PORT_SPEC.md`
  §P3, so `codex-swarm-<pid>`).
`set_teammate_pane_hidden` mirrors A.6: mutate the on-disk array via
`team_store::update_config(&codex_home, team, |c| { if hide { if !c.hidden_pane_ids.contains(id) { c.hidden_pane_ids.push(id) } } else { c.hidden_pane_ids.retain(|p| p != id) } })`
(this is the faithful port — `addHiddenPaneId`/`removeHiddenPaneId`), then a
best-effort `tmux break-pane -d`/`join-pane` (or `select-pane`) that never errors
the session. Reuse the existing `shell_single_quote` and `inside_tmux()` helpers
already in `teammate_panes.rs`.

### B.6 New `AppEvent` variants (`tui/src/app_event.rs`)
Add to `enum AppEvent` (sits beside the existing `OpenTeammatePane`):
```rust
/// Lead inbox poll found teammate replies; inject them as a new user turn on
/// the active (lead) thread. `text` is the joined <teammate-message …> payload.
InjectTeammateReplies { text: String },

/// Open the Teams dialog overlay (Shift+↑/↓ already moves through the Teams
/// footer roster; bind a distinct key, e.g. the existing teams entry point, to
/// this).
OpenTeamsDialog,

/// Side-effects requested by the Teams dialog (focus / hide-show a pane).
TeamsDialogAction(crate::chatwidget::teams_dialog::TeamsDialogAction),
```
Dispatch in `tui/src/app/event_dispatch.rs` (next to the `OpenTeammatePane` arm
at line ~47):
- `InjectTeammateReplies { text }` → inject as a user turn on the active lead
  thread. Reuse the existing submission path: route through the chat widget's
  `submit_user_message` (`tui/src/chatwidget/input_submission.rs:65`) when idle,
  matching Claude's `onSubmitMessage`. If a turn is running, hold in a small
  `VecDeque<String>` on `App` (analog of `AppState.inbox.messages` pending queue)
  and flush on the next idle transition. (Do **not** reuse
  `ClearUiAndSubmitUserMessage` — that resets the session; teammate replies must
  land in the *current* lead turn stream.)
- `OpenTeamsDialog` → construct `TeamsDialog::open(team)` and install it as the
  active overlay (same plumbing as other bottom-pane overlays).
- `TeamsDialogAction(a)` → call `App::focus_teammate_pane` /
  `App::set_teammate_pane_hidden` (B.5).

Add to `BottomPane` (`tui/src/bottom_pane/mod.rs`, beside
`set_active_agent_label` at 1720) a colored sibling so pills render in color:
```rust
pub(crate) fn set_active_team_pills(&mut self, pills: Option<Vec<Span<'static>>>);
```
and have `team_ui::sync_footer_context_label` also push
`self.team_ui.footer_spans()` into it.

### B.7 Lifecycle wiring (where the poller starts/stops)
- The lead's team identity is available once `create_team` runs. The TUI learns
  the team name from the team-tool output already observed in `team_ui.rs`
  (`apply_team_tool_output → CreateTeam`). When `active_team` becomes `Some`,
  fire a one-shot to start the poller; when it becomes stopped/None, cancel.
- Concretely: in `team_ui::handle_raw_response_item_for_team_ui`, on
  `TeamUiEvent::TeamCreated`/first `MemberSpawned`, send a new internal event to
  `App` to `start_lead_inbox_poller(self.config.codex_home.clone(), team_name,
  TEAM_LEAD_NAME, app_event_tx)`. Store the `LeadInboxPoller` on `App` (new field
  `lead_inbox_poller: Option<LeadInboxPoller>`); drop/cancel it on `TeamStopped`
  and on app exit.
- `codex_home` is `App.config.codex_home` (used already in
  `teammate_panes.rs`). The teams root passed to `team_store` IS `codex_home`
  (Phase 1 layout: `teams/{team}/…` under `$CODEX_HOME`).

### B.8 Relationship to teammate runtime modes (important)
Codex now has a process-backed teammate path: `team_spawn_member` can launch a
full interactive `codex teammate` TUI in tmux/iTerm2, and that teammate receives
its first turn through the on-disk team mailbox. If no pane backend is available,
`team_spawn_member` fails closed instead of creating a native in-process
subagent.

That means the lead inbox poller is no longer speculative for process-backed
teammates; it is the mechanism that turns teammate mailbox replies into lead
turns:
- Process-backed path: require the teammate TUI poller to read its inbox,
  execute the first/follow-up turns, and write replies/idle notifications to the
  lead inbox.

### B.9 Tests (TUI-local, no cargo run here)
- `team_colors`: round-robin determinism, stable per id, `clear` resets index;
  `mode_symbol_and_color` table matches A.3.
- `format_teammate_messages`: exact `<teammate-message teammate_id="…" color="…"
  summary="…">\n…\n</teammate-message>` joined by `\n\n`; omit absent attrs.
- `lead_inbox_poller` poll step against a temp `$CODEX_HOME`: write two unread via
  `team_store::write_to_mailbox`, assert one `InjectTeammateReplies` with both
  wrapped, and that `read_unread` is empty afterward (mark-read ordering).
- `teams_dialog`: key handling (`j/k` selection, `↵` drill-in, `h` →
  `ToggleVisibility{hide}` matching `is_hidden`), `[idle]`/`[hidden]` pills.
- Extend `tui/src/chatwidget/tests/team_ui.rs` footer assertion to check the
  colored-pill span path (color present after `MemberSpawned`).

---

## C. Open / blocking unknowns
1. `IT2_COMMAND` literal for the codex iTerm backend is owned by Phase 6-iTerm
   (`team_backends/iterm.rs`, not yet written). `focus_teammate_pane`'s iTerm arm
   should call into that backend once it exists; until then it tmux-only.
2. Whether `BottomPane`'s active-agent label can render styled spans, or needs the
   new `set_active_team_pills` sibling (B.6). The sibling is the safe path.
3. Whether to add a future verified in-process teammate mode with a distinct
   session source. Do not reuse native `spawn_agent` as a Teams fallback.
