# Phase 4 spec — MAILBOX + COORDINATION semantics (Claude → codex Rust port)

Truth source: `ChinaSiro/claude-code-sourcemap`. The relevant Claude module is
`restored-src/src/utils/teammateMailbox.ts` (mailbox + protocol-message types/detectors),
`restored-src/src/utils/tasks.ts` (shared task list), `restored-src/src/constants/xml.ts`
(`TEAMMATE_MESSAGE_TAG`), `restored-src/src/utils/swarm/constants.ts` (`TEAM_LEAD_NAME`),
and the consumers `useInboxPoller` / `inProcessRunner` (`runInProcessTeammate`,
`waitForNextPromptOrShutdown`) + `getTeammateMailboxAttachments` (MailboxBridge).

Codex target: `codex-rs/core/src/team_store.rs` ALREADY ships the file primitives
(`write_to_mailbox`, `read_mailbox`, `read_unread`, `mark_message_read_by_index`,
`mark_messages_read`, `read/write/update_config`, `inbox_path`, `tasks_dir`, `sanitize`,
`agent_id`, `TEAM_LEAD_NAME`, dep-free `FileLock`, `write_atomic`). This phase fills the
COORDINATION GAPS in that file and wires `team_send` (`core/src/tools/handlers/team.rs`)
through it cross-process. **Do NOT re-port the primitives that already exist; only add the
gap items below.**

This doc is read-only spec. No source edits, no cargo.

---

## 0. Already-present in `team_store.rs` (DO NOT duplicate) — fidelity audit vs Claude

| Claude (`teammateMailbox.ts`) | codex `team_store.rs` | Verdict |
|---|---|---|
| `getInboxPath(agentName, teamName?)` → `teams/{safeTeam}/inboxes/{safeAgent}.json` | `inbox_path(root, team, agent)` | OK (identical layout) |
| `writeToMailbox` (lock, re-read, push `{...read:false}`, pretty-2 write) | `write_to_mailbox` | OK |
| `readMailbox` (ENOENT → `[]`) | `read_mailbox` | OK |
| `readUnreadMessages` (`filter(m => !m.read)`) | `read_unread` | OK |
| `markMessageAsReadByIndex` | `mark_message_read_by_index` | OK |
| `markMessagesAsRead` | `mark_messages_read` | OK |
| `sanitizePathComponent`: `input.replace(/[^a-zA-Z0-9_-]/g, '-')` | `sanitize` (maps bad chars to `'_'`) | **DIVERGENT replacement char** — Claude uses `-`, codex uses `_`. Pre-existing; do NOT change it here (would break round-trip tests + Phase 1 layout). Note it as a known intentional divergence. |
| `TEAM_LEAD_NAME = 'team-lead'` | `pub const TEAM_LEAD_NAME = "team-lead"` | OK |
| `LOCK_OPTIONS = { retries: { retries:10, minTimeout:5, maxTimeout:100 } }` (proper-lockfile, exp backoff 5→100ms, 10 retries) | `FileLock::acquire`: 200 spins × 10ms = ~2s, then steals stale lock | Functionally adequate; do NOT rewrite. |

Claude inbox JSON element (matches codex `TeammateMessage` 1:1):
```ts
export type TeammateMessage = {
  from: string; text: string; timestamp: string; read: boolean
  color?: string   // sender color e.g. 'red','blue','green'
  summary?: string // 5-10 word UI preview
}
```
New inbox file initialized as `'[]'` (codex `read_messages` returns `Vec::new()` on ENOENT — equivalent).

---

## GAP 1 — Idle notification (`IdleNotification` struct + helper + detector)

Claude `IdleNotificationMessage` (literal, `teammateMailbox.ts`):
```ts
export type IdleNotificationMessage = {
  type: 'idle_notification'
  from: string
  timestamp: string
  idleReason?: 'available' | 'interrupted' | 'failed'
  summary?: string                 // brief summary of last DM this turn
  completedTaskId?: string
  completedStatus?: 'resolved' | 'blocked' | 'failed'
  failureReason?: string
}
```
`createIdleNotification(agentId, options?)` returns the above with `timestamp:new Date().toISOString()`.
The notification is NOT a `TeammateMessage` itself: it is JSON-stringified and placed in the
`text` field of a normal `TeammateMessage` written to the **team-lead's** inbox (see how
`sendShutdownRequestToMailbox` wraps a struct into `text: jsonStringify(...)`). `summary` comes
from `getLastPeerDmSummary(messages)` at the Stop hook (the `[to {name}] {summary}` string from
the last assistant turn's `SendMessage` tool_use that targeted a peer, not the lead).

### Rust to ADD to `team_store.rs`

```rust
/// Mirrors Claude `IdleNotificationMessage` — JSON-encoded into a `TeammateMessage.text`
/// and delivered to the team lead's inbox when a teammate session goes idle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdleNotification {
    /// Always "idle_notification" (serde tag is the literal string, NOT camelCased).
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_reason: Option<IdleReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_status: Option<CompletedStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleReason { Available, Interrupted, Failed }

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletedStatus { Resolved, Blocked, Failed }

#[derive(Debug, Clone, Default)]
pub struct IdleOptions {
    pub idle_reason: Option<IdleReason>,
    pub summary: Option<String>,
    pub completed_task_id: Option<String>,
    pub completed_status: Option<CompletedStatus>,
    pub failure_reason: Option<String>,
}

/// Mirror of Claude `createIdleNotification(agentId, options?)`.
pub fn create_idle_notification(agent_id: &str, opts: IdleOptions) -> IdleNotification {
    IdleNotification {
        kind: "idle_notification".to_string(),
        from: agent_id.to_string(),
        timestamp: now_timestamp(), // see GAP 5: switch to RFC-3339 to match new Date().toISOString()
        idle_reason: opts.idle_reason,
        summary: opts.summary,
        completed_task_id: opts.completed_task_id,
        completed_status: opts.completed_status,
        failure_reason: opts.failure_reason,
    }
}

/// Serialize + deliver an idle notification to the team lead's inbox (mirror of `sendIdleNotification`).
pub fn send_idle_notification(
    teams_root: &Path,
    team: &str,
    from_agent: &str,
    color: Option<String>,
    opts: IdleOptions,
) -> io::Result<()> {
    let note = create_idle_notification(from_agent, opts);
    let text = serde_json::to_string(&note).map_err(io::Error::other)?;
    write_to_mailbox(
        teams_root,
        team,
        TEAM_LEAD_NAME,
        TeammateMessage {
            from: from_agent.to_string(),
            text,
            timestamp: now_timestamp(),
            read: false,
            color,
            summary: None,
        },
    )
}
```
**Note on camelCase**: Claude emits the idle JSON keys verbatim
(`type`, `from`, `timestamp`, `idleReason`, `summary`, `completedTaskId`, `completedStatus`,
`failureReason`). With `#[serde(rename_all = "camelCase")]` + the explicit `#[serde(rename = "type")]`
on `kind`, the output matches exactly. Detector below must reject by checking `type == "idle_notification"`.

---

## GAP 2 — `format_as_teammate_message` (peer-message XML wrapper)

`TEAMMATE_MESSAGE_TAG = 'teammate-message'` (`constants/xml.ts`).

Claude `formatAsTeammateMessage` (used by `runInProcessTeammate`/`useInboxPoller` to inject a
peer/lead message into the model turn):
```ts
function formatAsTeammateMessage(from, content, color?, summary?): string {
  const colorAttr = color ? ` color="${color}"` : ''
  const summaryAttr = summary ? ` summary="${summary}"` : ''
  return `<${TEAMMATE_MESSAGE_TAG} teammate_id="${from}"${colorAttr}${summaryAttr}>\n${content}\n</${TEAMMATE_MESSAGE_TAG}>`
}
```
(`formatTeammateMessages` is the batch variant: maps each msg through the SAME opening tag but
its closing token is `\n ` (newline+space) instead of the full close tag, and joins with `\n\n`.
The single-message `formatAsTeammateMessage` is the canonical one to port for turn injection.)

### Rust to ADD to `team_store.rs`

```rust
/// Mirrors Claude `TEAMMATE_MESSAGE_TAG`.
pub const TEAMMATE_MESSAGE_TAG: &str = "teammate-message";

/// Mirror of Claude `formatAsTeammateMessage` — wraps a peer/lead message as
/// `<teammate-message teammate_id="…" [color="…"] [summary="…"]>\n{content}\n</teammate-message>`.
/// User-originated turns are injected PLAIN (not wrapped); only teammate-sourced turns use this.
pub fn format_as_teammate_message(
    from: &str,
    content: &str,
    color: Option<&str>,
    summary: Option<&str>,
) -> String {
    let color_attr = color.map(|c| format!(" color=\"{c}\"")).unwrap_or_default();
    let summary_attr = summary.map(|s| format!(" summary=\"{s}\"")).unwrap_or_default();
    format!(
        "<{tag} teammate_id=\"{from}\"{color_attr}{summary_attr}>\n{content}\n</{tag}>",
        tag = TEAMMATE_MESSAGE_TAG,
    )
}
```
Wiring: in the Phase-2 teammate run loop, when a selected inbox message is consumed, inject its
`text` as the next user turn — wrapped via `format_as_teammate_message(&msg.from, &msg.text,
msg.color.as_deref(), msg.summary.as_deref())`. (Attributes are emitted raw, same as Claude; no
XML-escaping is performed by Claude — preserve that to stay byte-identical.)

---

## GAP 3 — Structured-protocol-message detection (filter + typed detectors)

Claude discriminator field: `type`. `isStructuredProtocolMessage(text)` returns true for any of:
`permission_request`, `permission_response`, `sandbox_permission_request`,
`sandbox_permission_response`, `shutdown_request`, `shutdown_approved`,
`team_permission_update`, `mode_set_request`, `plan_approval_request`, `plan_approval_response`.
(`idle_notification` is NOT in that set — idle is handled separately by `isIdleNotification`.)

`getTeammateMailboxAttachments` (MailboxBridge): reads `readUnreadMessages`, then
`.filter(m => !isStructuredProtocolMessage(m.text))` so protocol messages are NOT bundled as raw
LLM context — they are routed by `useInboxPoller` to their handlers. (In the provided source it
only FILTERS; marking-read of non-protocol attachments happens in the poller, not here.)

Codex Phase-4 scope needs at minimum: `shutdown_request`, `plan_approval_request` /
`plan_approval_response`, and `idle_notification`. Port the full set of literal tags for the
filter (so unimplemented types are still kept out of raw context).

Exact Claude shapes to mirror (the three codex cares about now):
```ts
// shutdown_request (zod): type:'shutdown_request', requestId, from, reason?, timestamp
// plan_approval_request:  type:'plan_approval_request', from, timestamp, planFilePath, planContent, requestId
// plan_approval_response: type:'plan_approval_response', requestId, approved:bool, feedback?, timestamp, permissionMode?
// shutdown_approved:      type:'shutdown_approved', requestId, from, timestamp, paneId?, backendType?
```
Note key casing: these protocol messages use **`requestId`** (camelCase), `from`, `timestamp` —
serde `rename_all = "camelCase"` reproduces them.

### Rust to ADD to `team_store.rs`

```rust
/// Discriminator-only enum for routing. `text` is the raw `TeammateMessage.text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolKind {
    PermissionRequest, PermissionResponse,
    SandboxPermissionRequest, SandboxPermissionResponse,
    ShutdownRequest, ShutdownApproved,
    TeamPermissionUpdate, ModeSetRequest,
    PlanApprovalRequest, PlanApprovalResponse,
}

/// Mirror of `isStructuredProtocolMessage`. Returns Some(kind) iff `text` parses to a JSON
/// object whose `type` is one of the protocol literals (idle_notification is excluded).
pub fn protocol_kind(text: &str) -> Option<ProtocolKind> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    match v.get("type")?.as_str()? {
        "permission_request" => Some(ProtocolKind::PermissionRequest),
        "permission_response" => Some(ProtocolKind::PermissionResponse),
        "sandbox_permission_request" => Some(ProtocolKind::SandboxPermissionRequest),
        "sandbox_permission_response" => Some(ProtocolKind::SandboxPermissionResponse),
        "shutdown_request" => Some(ProtocolKind::ShutdownRequest),
        "shutdown_approved" => Some(ProtocolKind::ShutdownApproved),
        "team_permission_update" => Some(ProtocolKind::TeamPermissionUpdate),
        "mode_set_request" => Some(ProtocolKind::ModeSetRequest),
        "plan_approval_request" => Some(ProtocolKind::PlanApprovalRequest),
        "plan_approval_response" => Some(ProtocolKind::PlanApprovalResponse),
        _ => None,
    }
}

pub fn is_structured_protocol_message(text: &str) -> bool { protocol_kind(text).is_some() }

// --- Typed payloads codex needs now ---
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownRequestMessage {
    #[serde(rename = "type")] pub kind: String, // "shutdown_request"
    pub request_id: String,
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub reason: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalRequestMessage {
    #[serde(rename = "type")] pub kind: String, // "plan_approval_request"
    pub from: String,
    pub timestamp: String,
    pub plan_file_path: String,
    pub plan_content: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalResponseMessage {
    #[serde(rename = "type")] pub kind: String, // "plan_approval_response"
    pub request_id: String,
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub feedback: Option<String>,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub permission_mode: Option<String>,
}

/// Mirror of `isShutdownRequest` (strict: type literal must match + required fields present).
pub fn parse_shutdown_request(text: &str) -> Option<ShutdownRequestMessage> {
    let m: ShutdownRequestMessage = serde_json::from_str(text).ok()?;
    (m.kind == "shutdown_request").then_some(m)
}
pub fn parse_plan_approval_request(text: &str) -> Option<PlanApprovalRequestMessage> {
    let m: PlanApprovalRequestMessage = serde_json::from_str(text).ok()?;
    (m.kind == "plan_approval_request").then_some(m)
}
pub fn parse_plan_approval_response(text: &str) -> Option<PlanApprovalResponseMessage> {
    let m: PlanApprovalResponseMessage = serde_json::from_str(text).ok()?;
    (m.kind == "plan_approval_response").then_some(m)
}

/// Mirror of `isIdleNotification`.
pub fn parse_idle_notification(text: &str) -> Option<IdleNotification> {
    let n: IdleNotification = serde_json::from_str(text).ok()?;
    (n.kind == "idle_notification").then_some(n)
}

/// Builder mirror of `createShutdownRequestMessage` / `sendShutdownRequestToMailbox`.
/// requestId in Claude = generateRequestId('shutdown', target); codex may use a uuid or
/// "shutdown-{target}-{nanos}". sender defaults to TEAM_LEAD_NAME when unset.
pub fn send_shutdown_request(
    teams_root: &Path, team: &str, target: &str, from: &str,
    request_id: &str, reason: Option<String>, color: Option<String>,
) -> io::Result<()> {
    let msg = ShutdownRequestMessage {
        kind: "shutdown_request".to_string(),
        request_id: request_id.to_string(),
        from: from.to_string(),
        reason,
        timestamp: now_timestamp(),
    };
    let text = serde_json::to_string(&msg).map_err(io::Error::other)?;
    write_to_mailbox(teams_root, team, target, TeammateMessage {
        from: from.to_string(), text, timestamp: now_timestamp(), read: false, color, summary: None,
    })
}
```

---

## GAP 4 — Inbox selection priority + attachment filtering (poller helpers)

Claude `waitForNextPromptOrShutdown` (in `inProcessRunner`/`useInboxPoller`) selection order over
`readMailbox(...)` (ALL messages, not just unread — it indexes into the full array so
`mark_message_read_by_index` aligns):

1. **Shutdown first** — scan unread for the first `isShutdownRequest(text)`; if found, mark THAT
   index read and return it (prevents starvation under peer flood).
2. **Team-lead next** — else first unread where `from === TEAM_LEAD_NAME`.
3. **FIFO fallback** — else `findIndex(m => !m.read)` (first unread, any sender).

### Rust to ADD to `team_store.rs`

```rust
/// What the teammate run loop should do next (mirror of waitForNextPromptOrShutdown result).
pub enum NextInbox {
    Shutdown { index: usize, request: ShutdownRequestMessage, raw: String },
    Message  { index: usize, message: TeammateMessage },
    Empty,
}

/// Pure selection over the FULL mailbox vec (index-aligned for mark_message_read_by_index).
/// Priority: shutdown_request > TEAM_LEAD_NAME > FIFO first-unread.
pub fn select_next_inbox(messages: &[TeammateMessage]) -> NextInbox {
    for (i, m) in messages.iter().enumerate() {
        if !m.read {
            if let Some(req) = parse_shutdown_request(&m.text) {
                return NextInbox::Shutdown { index: i, request: req, raw: m.text.clone() };
            }
        }
    }
    if let Some(i) = messages.iter().position(|m| !m.read && m.from == TEAM_LEAD_NAME) {
        return NextInbox::Message { index: i, message: messages[i].clone() };
    }
    match messages.iter().position(|m| !m.read) {
        Some(i) => NextInbox::Message { index: i, message: messages[i].clone() },
        None => NextInbox::Empty,
    }
}

/// Mirror of getTeammateMailboxAttachments: unread, minus structured protocol messages,
/// each wrapped via format_as_teammate_message. (Marking-read is the caller's responsibility,
/// matching Claude where the poller—not the bridge—marks read.)
pub fn teammate_attachments(
    teams_root: &Path, team: &str, agent: &str,
) -> io::Result<Vec<String>> {
    Ok(read_unread(teams_root, team, agent)?
        .into_iter()
        .filter(|m| !is_structured_protocol_message(&m.text))
        .map(|m| format_as_teammate_message(&m.from, &m.text, m.color.as_deref(), m.summary.as_deref()))
        .collect())
}
```

---

## GAP 5 — Timestamps (RFC-3339)

Claude uses `new Date().toISOString()` for `timestamp` and all protocol `timestamp`/idle fields.
Current `now_timestamp()` emits fractional epoch seconds (`"{secs:.3}"`). For wire parity, add an
RFC-3339 helper and use it in the NEW builders above (do NOT change existing primitive callers /
tests unless a follow-up phase migrates them). `chrono` is already a workspace dep
(used elsewhere in core); if avoiding a new import in this module, format manually, but prefer:
```rust
pub fn now_rfc3339() -> String { chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true) }
```
Blocking-unknown: confirm `chrono` is allowed in `core/src/team_store.rs` (Phase 1 was
intentionally std+serde only). If not, keep `now_timestamp()` and accept timestamp-format drift.

---

## GAP 6 — Shared task list ops (`~/.claude/tasks/{team}/` → `$CODEX_HOME/tasks/{team}/`)

Claude `tasks.ts` layout: **one JSON file per task** at
`getTasksDir(taskListId)/{sanitize(taskId)}.json`; `getTasksDir = join(home,'tasks',sanitize(id))`
(codex `tasks_dir(root, team)` already = `tasks/{sanitize(team)}`). Task IDs are **sequential
integers as strings** (`"1"`, `"2"`, …). High-water-mark file `.highwatermark` in the task dir
guards against ID reuse after deletes. `taskListId` == team name (via `getTaskListId()` chain;
for codex, pass the team name).

Exact Claude schema/constants:
```ts
export const TASK_STATUSES = ['pending', 'in_progress', 'completed'] as const
// status may also be set to 'deleted' (deletes the file). Schema:
TaskSchema = z.object({
  id: string, subject: string, description: string,
  activeForm?: string, owner?: string,            // owner = agent ID
  status: 'pending'|'in_progress'|'completed',
  blocks: string[], blockedBy: string[],          // task-id deps (REQUIRED arrays, default [])
  metadata?: Record<string, unknown>,
})
sanitizePathComponent(s) = s.replace(/[^a-zA-Z0-9_-]/g, '-')
createTask: lock(.lock); id = String(findHighestTaskId+1); write {id,...data}; notifyTasksUpdated
findHighestTaskId = max(highest .json file id, readHighWaterMark)
resetTaskList: lock; persist current highest → .highwatermark; unlink all *.json (skip dotfiles)
listTasks: readdir; filter *.json; getTask each; drop nulls
getTask: read {id}.json; zod-validate; ENOENT→null
```

### Rust to ADD to `team_store.rs`

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus { Pending, InProgress, Completed }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub active_form: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub owner: Option<String>, // agent id
    pub status: TaskStatus,
    #[serde(default)] pub blocks: Vec<String>,
    #[serde(default)] pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
}
```
NOTE: Claude task JSON keys are camelCase for `activeForm`/`blockedBy`. Add
`#[serde(rename_all = "camelCase")]` on `Task` (then `active_form`→`activeForm`,
`blocked_by`→`blockedBy`; `id/subject/description/owner/status/blocks/metadata` already match).

Path + IO helpers (reuse existing `tasks_dir`, `sanitize`, `FileLock`, `write_atomic`):
```rust
pub fn task_path(teams_root: &Path, team: &str, task_id: &str) -> PathBuf {
    tasks_dir(teams_root, team).join(format!("{}.json", sanitize(task_id)))
}
fn highwater_path(teams_root: &Path, team: &str) -> PathBuf {
    tasks_dir(teams_root, team).join(".highwatermark")
}
fn read_highwater(teams_root: &Path, team: &str) -> i64 {
    fs::read_to_string(highwater_path(teams_root, team))
        .ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}
fn highest_task_id_from_files(teams_root: &Path, team: &str) -> i64 {
    let dir = tasks_dir(teams_root, team);
    let mut max = 0;
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".json") {
                if let Ok(n) = stem.parse::<i64>() { max = max.max(n); }
            }
        }
    }
    max
}

/// Mirror of createTask — lock task list, id = max(files, highwater)+1, write {id,...}.
pub fn create_task(teams_root: &Path, team: &str, task: Task /* id ignored */) -> io::Result<String> {
    let lock_target = tasks_dir(teams_root, team).join(".tasklist");
    let _lock = FileLock::acquire(&lock_target)?;
    let id = (highest_task_id_from_files(teams_root, team).max(read_highwater(teams_root, team)) + 1).to_string();
    let mut task = task; task.id = id.clone();
    let bytes = serde_json::to_vec_pretty(&task).map_err(io::Error::other)?;
    write_atomic(&task_path(teams_root, team, &id), &bytes)?;
    Ok(id)
}

pub fn get_task(teams_root: &Path, team: &str, task_id: &str) -> io::Result<Option<Task>> {
    match fs::read(task_path(teams_root, team, task_id)) {
        Ok(b) => Ok(serde_json::from_slice(&b).ok()), // zod-safeParse → None on invalid
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn list_tasks(teams_root: &Path, team: &str) -> io::Result<Vec<Task>> {
    let dir = tasks_dir(teams_root, team);
    let mut out = Vec::new();
    let rd = match fs::read_dir(&dir) { Ok(rd) => rd, Err(_) => return Ok(out) };
    for e in rd.flatten() {
        let name = e.file_name(); let name = name.to_string_lossy();
        if let Some(stem) = name.strip_suffix(".json") {
            if stem.starts_with('.') { continue; }
            if let Some(t) = get_task(teams_root, team, stem)? { out.push(t); }
        }
    }
    Ok(out)
}

/// Read-modify-write a task under the task-list lock (mirror of updateTask).
pub fn update_task<F: FnOnce(&mut Task)>(teams_root: &Path, team: &str, task_id: &str, f: F) -> io::Result<Option<Task>> {
    let lock_target = tasks_dir(teams_root, team).join(".tasklist");
    let _lock = FileLock::acquire(&lock_target)?;
    let Some(mut task) = get_task(teams_root, team, task_id)? else { return Ok(None) };
    f(&mut task);
    let bytes = serde_json::to_vec_pretty(&task).map_err(io::Error::other)?;
    write_atomic(&task_path(teams_root, team, task_id), &bytes)?;
    Ok(Some(task))
}

/// status:'deleted' path → remove the file (mirror of TaskUpdateTool delete).
pub fn delete_task(teams_root: &Path, team: &str, task_id: &str) -> io::Result<()> {
    match fs::remove_file(task_path(teams_root, team, task_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Mirror of resetTaskList — persist highest id to .highwatermark, delete all task *.json.
pub fn reset_task_list(teams_root: &Path, team: &str) -> io::Result<()> {
    let lock_target = tasks_dir(teams_root, team).join(".tasklist");
    let _lock = FileLock::acquire(&lock_target)?;
    let highest = highest_task_id_from_files(teams_root, team);
    if highest > read_highwater(teams_root, team) {
        write_atomic(&highwater_path(teams_root, team), highest.to_string().as_bytes())?;
    }
    if let Ok(rd) = fs::read_dir(tasks_dir(teams_root, team)) {
        for e in rd.flatten() {
            let name = e.file_name(); let name = name.to_string_lossy();
            if name.ends_with(".json") && !name.starts_with('.') {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}
```
**Locking caveat**: `FileLock::acquire` here locks a sibling `.tasklist.lock` (its
`with_extension("lock")` of `.../.tasklist`). Claude locks one `.lock` for the whole task-list
dir; the codex pattern of locking a `.tasklist` sentinel achieves the same serialization. Verify
`FileLock` is exported/visible (currently it is a private `struct` in the module — these new fns
live in the SAME module so visibility is fine; do NOT make it `pub` unless the task fns move out).

---

## WIRING — `team_send` → `write_to_mailbox` cross-process

Target: `core/src/tools/handlers/team.rs::team_send` (currently calls the in-process
`registry.send_message(SendTeamMessageRequest{…})`). Phase-4 cross-process route (keep the
in-process path as fallback, gate on whether the team has an on-disk `config.json` /
`backend_type != in_process`):

1. Resolve teams_root = `$CODEX_HOME` (the existing `team_store` path root used by Phase 1).
   Resolve `team` (string name) and `target_agent` name:
   - `target == "lead"` → `TEAM_LEAD_NAME`.
   - `target == "member"` → the member's `name` (NOT the ThreadId). `team.rs` currently keys
     members by `ThreadId`; for the file store you must map `member_id → TeamFileMember.name`
     via `read_config(teams_root, team)`.
2. Build the `TeammateMessage`:
   ```rust
   write_to_mailbox(teams_root, team, target_agent, TeammateMessage {
       from: sender_name,                 // caller's agent name; lead → TEAM_LEAD_NAME
       text: input_preview(&items),       // existing helper renders the message text
       timestamp: now_rfc3339(),
       read: false,
       color: sender_color,               // from TeamFileMember.color (or None for lead)
       summary: None,                     // or a 5-10 word summary if the tool grows a field
   })?;
   ```
3. The recipient process (Phase-2 teammate loop or the lead inbox poller) picks it up via
   `select_next_inbox` / `teammate_attachments`. `delivery_mode` (`queue` vs `interrupt`) maps to
   whether the recipient injects on next idle (queue) or interrupts current turn (interrupt) —
   the file write is identical; only consumer behavior differs.
4. `team_message_list` / `team_event_list` (read paths) should, in process-team mode, read from
   `read_mailbox(teams_root, team, agent)` instead of the in-memory `registry`. Keep the same
   `TeamMessage` JSON result shape so the model + TUI observer are unchanged.

Sender identity must stay consistent with `authorize_team_sender` (already enforces
lead-omits-sender / member-sets-own-id). Translate the authorized `member_id: ThreadId` → name
via config before the file write.

---

## Module wiring summary
- All GAP 1–6 items are **added to the existing `codex-rs/core/src/team_store.rs`** (same module,
  reuse `FileLock`, `write_atomic`, `read_messages`, `sanitize`, `tasks_dir`, `inbox_path`,
  `TeammateMessage`, `TEAM_LEAD_NAME`, `now_timestamp`/new `now_rfc3339`). No new file required.
- Public surface to add: `IdleNotification`+enums+`IdleOptions`, `create_idle_notification`,
  `send_idle_notification`, `TEAMMATE_MESSAGE_TAG`, `format_as_teammate_message`, `ProtocolKind`,
  `protocol_kind`, `is_structured_protocol_message`, the three protocol structs +
  `parse_*` detectors, `parse_idle_notification`, `send_shutdown_request`, `NextInbox`,
  `select_next_inbox`, `teammate_attachments`, `Task`/`TaskStatus`, `task_path`, `create_task`,
  `get_task`, `list_tasks`, `update_task`, `delete_task`, `reset_task_list`, and `now_rfc3339`.
- Consumer wiring in `core/src/tools/handlers/team.rs::team_send` (+ `team_message_list`) as above.

## Blocking unknowns
1. **chrono in team_store**: Phase 1 mandated std+serde-only. RFC-3339 timestamp parity needs
   `chrono` (or manual formatting). Confirm allowed, else accept epoch-fraction timestamp drift.
2. **member_id (ThreadId) ↔ TeamFileMember.name mapping**: `team.rs` keys members by `ThreadId`;
   the file mailbox keys by sanitized agent NAME. Phase 3/4 must record the name↔ThreadId mapping
   in `config.json` so `team_send` can resolve the recipient inbox. Needs the Phase-3 spawn change
   to populate it; until then cross-process `team_send` to a specific member can't resolve a name.
3. **sanitize divergence** (`-` vs `_`): intentionally left as-is; only flag if cross-tool
   interop with real Claude `~/.claude` dirs is ever required (it is not — codex uses `$CODEX_HOME`).
