//! Team coordination layer (Phase 4 of the Claude Code teams port).
//!
//! Ports the coordination *gaps* that sit on top of the Phase-1 file mailbox
//! ([`crate::team_store`]). The Claude sources mirrored here are:
//! - `restored-src/src/utils/teammateMailbox.ts` — `IdleNotificationMessage` /
//!   `createIdleNotification` / `sendIdleNotification`, `formatAsTeammateMessage`,
//!   `isStructuredProtocolMessage` + the typed protocol detectors
//!   (`isShutdownRequest`, `isIdleNotification`, plan-approval shapes),
//!   `getTeammateMailboxAttachments`, and `waitForNextPromptOrShutdown` selection.
//! - `restored-src/src/utils/tasks.ts` — the shared task list (`createTask`,
//!   `getTask`, `listTasks`, `updateTask`, delete, `resetTaskList`,
//!   high-water-mark ID allocation).
//! - `restored-src/src/constants/xml.ts` — `TEAMMATE_MESSAGE_TAG`.
//! - `restored-src/src/utils/swarm/constants.ts` — `TEAM_LEAD_NAME` (reused from
//!   [`crate::team_store::TEAM_LEAD_NAME`]).
//!
//! This module is std + serde/serde_json only and builds strictly on the *public*
//! surface of [`crate::team_store`]. It re-implements the small file-locking and
//! atomic-write primitives locally (the Phase-1 equivalents are module-private)
//! so the task-list ops keep the same serialization guarantees without touching
//! `team_store.rs`.
//!
//! It stays unwired until the team lead integrates it.
#![allow(dead_code)]

use std::fs;
use std::io;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde::Serialize;

use crate::team_store::MessageKind;
use crate::team_store::SourceRole;
use crate::team_store::TEAM_LEAD_NAME;
use crate::team_store::TeammateMessage;
use crate::team_store::read_unread;
use crate::team_store::sanitize;
use crate::team_store::tasks_dir;
use crate::team_store::write_to_mailbox;

// ---------------------------------------------------------------------------
// GAP 5 — RFC-3339 timestamps (std-only).
// ---------------------------------------------------------------------------

/// Current time as an RFC-3339 / ISO-8601 UTC string with millisecond precision
/// and a trailing `Z` (mirrors Claude's `new Date().toISOString()`), e.g.
/// `"2026-06-05T12:34:56.789Z"`.
///
/// Implemented std-only (no `chrono` import in this module) so the coordination
/// layer stays dependency-free. The integrator may swap this for
/// `chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)` —
/// `chrono` is already a `codex-core` dependency — if a single source of truth
/// is preferred.
pub fn now_rfc3339() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_rfc3339_utc(dur.as_secs() as i64, dur.subsec_millis())
}

/// Format `secs` (Unix epoch seconds, UTC) + `millis` as `YYYY-MM-DDTHH:MM:SS.sssZ`.
fn format_rfc3339_utc(secs: i64, millis: u32) -> String {
    // Days since epoch and seconds within the day.
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Convert a count of days since 1970-01-01 to a `(year, month, day)` triple.
///
/// Uses Howard Hinnant's well-known `civil_from_days` algorithm (public domain),
/// valid for the full proleptic Gregorian range.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

// ---------------------------------------------------------------------------
// GAP 1 — Idle notification.
// ---------------------------------------------------------------------------

/// Why a teammate went idle (mirrors Claude `IdleNotificationMessage.idleReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleReason {
    Available,
    Interrupted,
    Failed,
}

/// Completion status reported alongside an idle notification
/// (mirrors Claude `IdleNotificationMessage.completedStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletedStatus {
    Resolved,
    Blocked,
    Failed,
}

/// Mirrors Claude `IdleNotificationMessage` — JSON-encoded into a
/// [`TeammateMessage::text`] and delivered to the team lead's inbox when a
/// teammate session goes idle.
///
/// The `type` key serializes to the literal `"idle_notification"`; all other
/// keys are camelCased to match Claude's wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdleNotification {
    /// Always `"idle_notification"`.
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_reason: Option<IdleReason>,
    /// Brief summary of the last peer DM this turn (`[to {name}] {summary}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_status: Option<CompletedStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

/// Optional fields for [`create_idle_notification`] (mirrors Claude's
/// `createIdleNotification(agentId, options?)` second argument).
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
        timestamp: now_rfc3339(),
        idle_reason: opts.idle_reason,
        summary: opts.summary,
        completed_task_id: opts.completed_task_id,
        completed_status: opts.completed_status,
        failure_reason: opts.failure_reason,
    }
}

/// Serialize + deliver an idle notification to the team lead's inbox
/// (mirror of Claude `sendIdleNotification`). The notification JSON rides in the
/// wrapping [`TeammateMessage::text`] field.
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
            timestamp: now_rfc3339(),
            read: false,
            color,
            summary: None,
            ..Default::default()
        },
    )
}

// ---------------------------------------------------------------------------
// GAP 2 — Peer-message XML wrapper.
// ---------------------------------------------------------------------------

/// Mirrors Claude `TEAMMATE_MESSAGE_TAG`.
pub const TEAMMATE_MESSAGE_TAG: &str = "teammate-message";

/// Mirror of Claude `formatAsTeammateMessage` — wraps a peer/lead message as
/// `<teammate-message teammate_id="…" [color="…"] [summary="…"]>\n{content}\n</teammate-message>`.
///
/// User-originated turns are injected PLAIN (not wrapped); only teammate-sourced
/// turns use this. Attributes are emitted raw (no XML escaping), matching Claude
/// byte-for-byte.
pub fn format_as_teammate_message(
    from: &str,
    content: &str,
    color: Option<&str>,
    summary: Option<&str>,
) -> String {
    let color_attr = color.map(|c| format!(" color=\"{c}\"")).unwrap_or_default();
    let summary_attr = summary
        .map(|s| format!(" summary=\"{s}\""))
        .unwrap_or_default();
    format!(
        "<{TEAMMATE_MESSAGE_TAG} teammate_id=\"{from}\"{color_attr}{summary_attr}>\n{content}\n</{TEAMMATE_MESSAGE_TAG}>",
    )
}

// ---------------------------------------------------------------------------
// GAP 3 — Structured protocol message detection.
// ---------------------------------------------------------------------------

/// Discriminator-only enum for routing structured protocol messages
/// (mirror of the `type` literals recognized by Claude
/// `isStructuredProtocolMessage`). `idle_notification` is intentionally NOT in
/// this set — it is detected separately by [`parse_idle_notification`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolKind {
    PermissionRequest,
    PermissionResponse,
    SandboxPermissionRequest,
    SandboxPermissionResponse,
    ShutdownRequest,
    ShutdownApproved,
    TeamPermissionUpdate,
    ModeSetRequest,
    PlanApprovalRequest,
    PlanApprovalResponse,
}

/// Mirror of Claude `isStructuredProtocolMessage`. Returns `Some(kind)` iff
/// `text` parses to a JSON object whose `type` field is one of the protocol
/// literals (`idle_notification` excluded).
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

/// `true` iff `text` is one of the routed structured protocol messages.
pub fn is_structured_protocol_message(text: &str) -> bool {
    protocol_kind(text).is_some()
}

/// Mirror of Claude `shutdown_request` (zod) shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownRequestMessage {
    /// Always `"shutdown_request"`.
    #[serde(rename = "type")]
    pub kind: String,
    pub request_id: String,
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub timestamp: String,
}

/// Mirror of Claude `plan_approval_request` shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalRequestMessage {
    /// Always `"plan_approval_request"`.
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
    pub timestamp: String,
    pub plan_file_path: String,
    pub plan_content: String,
    pub request_id: String,
}

/// Mirror of Claude `plan_approval_response` shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalResponseMessage {
    /// Always `"plan_approval_response"`.
    #[serde(rename = "type")]
    pub kind: String,
    pub request_id: String,
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
}

/// Mirror of Claude `isShutdownRequest` — strict: `type` literal must match and
/// required fields must be present.
pub fn parse_shutdown_request(text: &str) -> Option<ShutdownRequestMessage> {
    let m: ShutdownRequestMessage = serde_json::from_str(text).ok()?;
    (m.kind == "shutdown_request").then_some(m)
}

/// Mirror of Claude plan-approval-request detector.
pub fn parse_plan_approval_request(text: &str) -> Option<PlanApprovalRequestMessage> {
    let m: PlanApprovalRequestMessage = serde_json::from_str(text).ok()?;
    (m.kind == "plan_approval_request").then_some(m)
}

/// Mirror of Claude plan-approval-response detector.
pub fn parse_plan_approval_response(text: &str) -> Option<PlanApprovalResponseMessage> {
    let m: PlanApprovalResponseMessage = serde_json::from_str(text).ok()?;
    (m.kind == "plan_approval_response").then_some(m)
}

/// Mirror of Claude `isIdleNotification`.
pub fn parse_idle_notification(text: &str) -> Option<IdleNotification> {
    let n: IdleNotification = serde_json::from_str(text).ok()?;
    (n.kind == "idle_notification").then_some(n)
}

/// Builder + delivery mirror of Claude `createShutdownRequestMessage` /
/// `sendShutdownRequestToMailbox`.
///
/// `request_id` mirrors Claude `generateRequestId('shutdown', target)`; the
/// caller chooses the scheme (e.g. a UUID or `"shutdown-{target}-{nanos}"`).
/// `from` defaults to [`TEAM_LEAD_NAME`] semantics at the call site.
pub fn send_shutdown_request(
    teams_root: &Path,
    team: &str,
    target: &str,
    from: &str,
    request_id: &str,
    reason: Option<String>,
    color: Option<String>,
) -> io::Result<()> {
    let msg = ShutdownRequestMessage {
        kind: "shutdown_request".to_string(),
        request_id: request_id.to_string(),
        from: from.to_string(),
        reason,
        timestamp: now_rfc3339(),
    };
    let text = serde_json::to_string(&msg).map_err(io::Error::other)?;
    write_to_mailbox(
        teams_root,
        team,
        target,
        TeammateMessage {
            from: from.to_string(),
            text,
            timestamp: now_rfc3339(),
            read: false,
            color,
            summary: None,
            ..Default::default()
        },
    )
}

// ---------------------------------------------------------------------------
// GAP 4 — Inbox selection priority + attachment filtering.
// ---------------------------------------------------------------------------

/// What the teammate run loop should do next
/// (mirror of Claude `waitForNextPromptOrShutdown`'s result).
#[derive(Debug, Clone)]
pub enum NextInbox {
    /// A shutdown request was found; `index` is its position in the FULL mailbox
    /// vec so the caller can `mark_message_read_by_index` it.
    Shutdown {
        index: usize,
        request: ShutdownRequestMessage,
        raw: String,
    },
    /// A regular message was selected (team-lead priority after shutdown, then FIFO).
    Message {
        index: usize,
        message: TeammateMessage,
    },
    /// No unread messages remain.
    Empty,
}

/// Pure selection over the FULL mailbox vec (index-aligned with
/// `mark_message_read_by_index`). Priority mirrors Claude
/// `waitForNextPromptOrShutdown`:
/// 1. first unread `shutdown_request` (prevents starvation under peer flood),
/// 2. else first unread `team-lead` message,
/// 3. else FIFO first-unread peer message.
/// Arbitration rank for one unread message. HIGHER wins. This is the multi-source
/// arbitration ladder: several teammates (plus the human via the lead) may all
/// write corrections to one working agent; the agent must process the most
/// authoritative one FIRST without any of them being dropped. Concurrency is
/// already serialized by the append-only mailbox (single-writer-at-a-time via the
/// file lock), so arbitration only decides ORDER, never discards.
///
/// Ladder (high → low):
///   human correction > lead > reviewer correction > peer correction >
///   report/progress > discussion > unclassified FIFO chatter
///
/// `kind`/`source_role` are optional: a message with neither ranks as ordinary
/// FIFO chatter, so pre-existing mailboxes and the plain lead→member task path
/// behave exactly as before this ladder existed. (Shutdown is handled separately
/// and outranks everything.)
fn arbitration_rank(message: &TeammateMessage) -> u8 {
    let is_correction = matches!(message.kind, Some(MessageKind::Correction));
    // The lead speaking (by name) is user intent and keeps its historical rank
    // just below a human-tagged correction, matching the previous lead>FIFO rule.
    let from_lead = message.from == TEAM_LEAD_NAME;
    match (message.source_role, is_correction, from_lead) {
        // A correction explicitly relayed for the human operator wins (short of
        // shutdown): it is the person steering the work.
        (Some(SourceRole::Human), true, _) => 100,
        // Any human-tagged message, even non-correction, still speaks for the
        // operator and outranks agent chatter.
        (Some(SourceRole::Human), false, _) => 90,
        // The lead agent (explicit role or by name) — preserves lead>peer.
        (Some(SourceRole::Lead), _, _) => 80,
        (_, _, true) => 80,
        // A reviewer teammate's correction: it is monitoring for drift and its
        // redirect should preempt ordinary peer discussion.
        (Some(SourceRole::Reviewer), true, _) => 70,
        (Some(SourceRole::Reviewer), false, _) => 40,
        // An ordinary peer's correction still outranks non-correction chatter.
        (Some(SourceRole::Peer), true, _) => 60,
        // Non-correction, role-tagged informational messages.
        (_, _, _)
            if matches!(
                message.kind,
                Some(MessageKind::Report | MessageKind::Progress)
            ) =>
        {
            40
        }
        // Peer discussion / brainstorming: additive, never preempts a correction.
        (_, _, _) if matches!(message.kind, Some(MessageKind::Discussion)) => 20,
        // Unclassified: ordinary FIFO chatter (legacy behavior).
        _ => 10,
    }
}

/// Select the next inbox message to process, arbitrating across sources.
///
/// Priority: a shutdown request always wins (safety), so the agent can wind down
/// even if higher-signal work is queued. Otherwise the highest [`arbitration_rank`]
/// wins; ties break FIFO (earliest unread index) so same-rank messages keep
/// arrival order. `index` is the position in the FULL mailbox vec so the caller
/// can `mark_message_read_by_index` it.
pub fn select_next_inbox(messages: &[TeammateMessage]) -> NextInbox {
    for (i, m) in messages.iter().enumerate() {
        if !m.read
            && let Some(req) = parse_shutdown_request(&m.text)
        {
            return NextInbox::Shutdown {
                index: i,
                request: req,
                raw: m.text.clone(),
            };
        }
    }
    // Highest arbitration rank wins; ties break to the earliest index (FIFO).
    // `max_by_key` returns the LAST max on ties, so we fold manually to keep the
    // earliest.
    let mut best: Option<(usize, u8)> = None;
    for (i, m) in messages.iter().enumerate() {
        if m.read {
            continue;
        }
        let rank = arbitration_rank(m);
        match best {
            Some((_, best_rank)) if rank <= best_rank => {}
            _ => best = Some((i, rank)),
        }
    }
    match best {
        Some((i, _)) => NextInbox::Message {
            index: i,
            message: messages[i].clone(),
        },
        None => NextInbox::Empty,
    }
}

/// Mirror of Claude `getTeammateMailboxAttachments`: unread messages, minus
/// structured protocol messages, each wrapped via [`format_as_teammate_message`].
///
/// Marking-read is the caller's responsibility (matching Claude, where the
/// poller — not the bridge — marks attachments read).
pub fn teammate_attachments(teams_root: &Path, team: &str, agent: &str) -> io::Result<Vec<String>> {
    Ok(read_unread(teams_root, team, agent)?
        .into_iter()
        .filter(|m| !is_structured_protocol_message(&m.text))
        .map(|m| {
            format_as_teammate_message(&m.from, &m.text, m.color.as_deref(), m.summary.as_deref())
        })
        .collect())
}

// ---------------------------------------------------------------------------
// GAP 6 — Shared task list.
// ---------------------------------------------------------------------------

/// Task status (mirrors Claude `TASK_STATUSES`). `deleted` is a transient action
/// (it removes the file) rather than a stored status — see [`delete_task`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

/// One task in the shared task list (mirrors Claude `TaskSchema`). Disk keys are
/// camelCase (`activeForm`, `blockedBy`); the rest already match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// Owner agent ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub status: TaskStatus,
    /// Task IDs this task blocks.
    #[serde(default)]
    pub blocks: Vec<String>,
    /// Task IDs that must complete before this one.
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Task {
    /// Construct a minimal pending task with empty dependency arrays and a
    /// placeholder `id` (replaced by [`create_task`]).
    pub fn new(subject: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            subject: subject.into(),
            description: description.into(),
            active_form: None,
            owner: None,
            status: TaskStatus::Pending,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: None,
        }
    }
}

/// Per-task file: `tasks/{team}/{sanitize(task_id)}.json`.
pub fn task_path(teams_root: &Path, team: &str, task_id: &str) -> PathBuf {
    tasks_dir(teams_root, team).join(format!("{}.json", sanitize(task_id)))
}

fn highwater_path(teams_root: &Path, team: &str) -> PathBuf {
    tasks_dir(teams_root, team).join(".highwatermark")
}

fn read_highwater(teams_root: &Path, team: &str) -> i64 {
    fs::read_to_string(highwater_path(teams_root, team))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Highest numeric task ID across the `*.json` task files (0 if none).
fn highest_task_id_from_files(teams_root: &Path, team: &str) -> i64 {
    let dir = tasks_dir(teams_root, team);
    let mut max = 0;
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".json")
                && let Ok(n) = stem.parse::<i64>()
            {
                max = max.max(n);
            }
        }
    }
    max
}

/// Mirror of Claude `createTask` — lock the task list, allocate
/// `id = max(highest file id, high-water mark) + 1`, then persist `{id, ...}`.
/// The incoming `task.id` is ignored. Returns the assigned ID.
pub fn create_task(teams_root: &Path, team: &str, task: Task) -> io::Result<String> {
    let lock_target = tasks_dir(teams_root, team).join(".tasklist");
    let _lock = LocalFileLock::acquire(&lock_target)?;
    let id = (highest_task_id_from_files(teams_root, team).max(read_highwater(teams_root, team))
        + 1)
    .to_string();
    let mut task = task;
    task.id = id.clone();
    let bytes = serde_json::to_vec_pretty(&task).map_err(io::Error::other)?;
    write_atomic(&task_path(teams_root, team, &id), &bytes)?;
    Ok(id)
}

/// Mirror of Claude `getTask` — read + validate `{id}.json`; ENOENT or invalid
/// JSON → `None` (matching `safeParse`).
pub fn get_task(teams_root: &Path, team: &str, task_id: &str) -> io::Result<Option<Task>> {
    match fs::read(task_path(teams_root, team, task_id)) {
        Ok(b) => Ok(serde_json::from_slice(&b).ok()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Mirror of Claude `listTasks` — read the dir, keep `*.json` (skip dotfiles),
/// load+validate each, drop invalid entries.
pub fn list_tasks(teams_root: &Path, team: &str) -> io::Result<Vec<Task>> {
    let dir = tasks_dir(teams_root, team);
    let mut out = Vec::new();
    let rd = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Ok(out),
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if let Some(stem) = name.strip_suffix(".json") {
            if stem.starts_with('.') {
                continue;
            }
            if let Some(t) = get_task(teams_root, team, stem)? {
                out.push(t);
            }
        }
    }
    Ok(out)
}

/// Read-modify-write a task under the task-list lock (mirror of Claude
/// `updateTask`). Returns `None` if the task does not exist.
pub fn update_task<F: FnOnce(&mut Task)>(
    teams_root: &Path,
    team: &str,
    task_id: &str,
    f: F,
) -> io::Result<Option<Task>> {
    let lock_target = tasks_dir(teams_root, team).join(".tasklist");
    let _lock = LocalFileLock::acquire(&lock_target)?;
    let Some(mut task) = get_task(teams_root, team, task_id)? else {
        return Ok(None);
    };
    f(&mut task);
    let bytes = serde_json::to_vec_pretty(&task).map_err(io::Error::other)?;
    write_atomic(&task_path(teams_root, team, task_id), &bytes)?;
    Ok(Some(task))
}

/// `status: 'deleted'` path → remove the file (mirror of the Claude
/// `TaskUpdateTool` delete branch). Missing file is a no-op.
pub fn delete_task(teams_root: &Path, team: &str, task_id: &str) -> io::Result<()> {
    match fs::remove_file(task_path(teams_root, team, task_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Mirror of Claude `resetTaskList` — persist the current highest ID to
/// `.highwatermark` (guarding against ID reuse), then delete all task `*.json`
/// files (dotfiles skipped).
pub fn reset_task_list(teams_root: &Path, team: &str) -> io::Result<()> {
    let lock_target = tasks_dir(teams_root, team).join(".tasklist");
    let _lock = LocalFileLock::acquire(&lock_target)?;
    let highest = highest_task_id_from_files(teams_root, team);
    if highest > read_highwater(teams_root, team) {
        write_atomic(
            &highwater_path(teams_root, team),
            highest.to_string().as_bytes(),
        )?;
    }
    if let Ok(rd) = fs::read_dir(tasks_dir(teams_root, team)) {
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".json") && !name.starts_with('.') {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Local file-locking + atomic-write primitives.
//
// `team_store`'s `FileLock`/`write_atomic` are module-private, so the task-list
// ops re-implement std-only equivalents here with the same serialization
// semantics (advisory `<stem>.lock` create + tmp-then-rename). Mailbox ops do
// NOT need these — they go through `team_store::write_to_mailbox`, which already
// locks internally.
// ---------------------------------------------------------------------------

/// Dep-free advisory lock via atomic create of a sibling `<stem>.lock` file
/// (behaviorally identical to `team_store`'s private `FileLock`).
struct LocalFileLock {
    path: PathBuf,
}

impl LocalFileLock {
    fn acquire(target: &Path) -> io::Result<Self> {
        let path = target.with_extension("lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        for _ in 0..200 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self { path }),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => sleep(Duration::from_millis(10)),
                Err(e) => return Err(e),
            }
        }
        // Stale-lock fallback: steal it so a crashed holder cannot wedge the team.
        let _ = fs::remove_file(&path);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self { path })
    }
}

impl Drop for LocalFileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn unique_root() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        temp_dir().join(format!("codex-team-coord-{nanos}"))
    }

    #[test]
    fn rfc3339_known_epoch() {
        // 2026-06-05T00:00:00.000Z == 1_780_617_600 seconds.
        assert_eq!(
            format_rfc3339_utc(1_780_617_600, 0),
            "2026-06-05T00:00:00.000Z"
        );
        // Unix epoch.
        assert_eq!(format_rfc3339_utc(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_rfc3339_utc(86_399, 500), "1970-01-01T23:59:59.500Z");
    }

    #[test]
    fn idle_notification_camel_case_and_round_trip() {
        let note = create_idle_notification(
            "alice@t1",
            IdleOptions {
                idle_reason: Some(IdleReason::Available),
                summary: Some("[to bob] looked into the parser".to_string()),
                completed_task_id: Some("3".to_string()),
                completed_status: Some(CompletedStatus::Resolved),
                failure_reason: None,
            },
        );
        let json = serde_json::to_string(&note).unwrap();
        assert!(json.contains("\"type\":\"idle_notification\""), "{json}");
        assert!(json.contains("\"idleReason\":\"available\""), "{json}");
        assert!(json.contains("\"completedTaskId\":\"3\""), "{json}");
        assert!(json.contains("\"completedStatus\":\"resolved\""), "{json}");
        assert!(!json.contains("failureReason"), "{json}");

        let parsed = parse_idle_notification(&json).unwrap();
        assert_eq!(parsed, note);
        assert!(parse_shutdown_request(&json).is_none());
        assert!(!is_structured_protocol_message(&json));
    }

    #[test]
    fn format_wrapper_matches_claude() {
        assert_eq!(
            format_as_teammate_message("bob", "hello", None, None),
            "<teammate-message teammate_id=\"bob\">\nhello\n</teammate-message>"
        );
        assert_eq!(
            format_as_teammate_message("bob", "hi", Some("red"), Some("a greeting")),
            "<teammate-message teammate_id=\"bob\" color=\"red\" summary=\"a greeting\">\nhi\n</teammate-message>"
        );
    }

    #[test]
    fn protocol_detection_and_strictness() {
        let shutdown = serde_json::json!({
            "type": "shutdown_request",
            "requestId": "shutdown-bob-1",
            "from": "team-lead",
            "timestamp": "2026-06-05T00:00:00.000Z",
        })
        .to_string();
        assert_eq!(
            protocol_kind(&shutdown),
            Some(ProtocolKind::ShutdownRequest)
        );
        assert!(is_structured_protocol_message(&shutdown));
        let req = parse_shutdown_request(&shutdown).unwrap();
        assert_eq!(req.request_id, "shutdown-bob-1");
        assert!(parse_plan_approval_request(&shutdown).is_none());

        let plan = serde_json::json!({
            "type": "plan_approval_response",
            "requestId": "plan-1",
            "approved": true,
            "timestamp": "2026-06-05T00:00:00.000Z",
        })
        .to_string();
        let resp = parse_plan_approval_response(&plan).unwrap();
        assert!(resp.approved);
        assert_eq!(
            protocol_kind(&plan),
            Some(ProtocolKind::PlanApprovalResponse)
        );

        assert!(protocol_kind("not json").is_none());
        assert!(protocol_kind("{\"type\":\"chatter\"}").is_none());
    }

    #[test]
    fn select_next_inbox_priority() {
        let lead = TeammateMessage {
            from: TEAM_LEAD_NAME.to_string(),
            text: "do the thing".to_string(),
            timestamp: now_rfc3339(),
            read: false,
            color: None,
            summary: None,
            ..Default::default()
        };
        let peer = TeammateMessage {
            from: "bob".to_string(),
            text: "fyi".to_string(),
            timestamp: now_rfc3339(),
            read: false,
            color: None,
            summary: None,
            ..Default::default()
        };
        let shutdown_text = serde_json::json!({
            "type": "shutdown_request",
            "requestId": "s1",
            "from": "team-lead",
            "timestamp": "2026-06-05T00:00:00.000Z",
        })
        .to_string();
        let shutdown = TeammateMessage {
            from: TEAM_LEAD_NAME.to_string(),
            text: shutdown_text,
            timestamp: now_rfc3339(),
            read: false,
            color: None,
            summary: None,
            ..Default::default()
        };

        // Peer first, then lead, then shutdown — shutdown must still win.
        let msgs = vec![peer.clone(), lead.clone(), shutdown];
        match select_next_inbox(&msgs) {
            NextInbox::Shutdown { index, .. } => assert_eq!(index, 2),
            other => panic!("expected shutdown, got {other:?}"),
        }

        // No shutdown → team-lead messages represent user intent and should
        // jump ahead of older peer chatter, matching Claude.
        let msgs = vec![peer.clone(), lead];
        match select_next_inbox(&msgs) {
            NextInbox::Message { index, message } => {
                assert_eq!(index, 1);
                assert_eq!(message.from, TEAM_LEAD_NAME);
            }
            other => panic!("expected fifo message, got {other:?}"),
        }

        // FIFO fallback still applies to peer messages.
        let msgs = vec![peer.clone()];
        match select_next_inbox(&msgs) {
            NextInbox::Message { index, .. } => assert_eq!(index, 0),
            other => panic!("expected fifo message, got {other:?}"),
        }

        // Empty when all read.
        let mut read_peer = peer;
        read_peer.read = true;
        assert!(matches!(select_next_inbox(&[read_peer]), NextInbox::Empty));
    }

    /// Build an unread message with an explicit arbitration classification.
    fn classified(
        from: &str,
        text: &str,
        kind: Option<MessageKind>,
        source_role: Option<SourceRole>,
    ) -> TeammateMessage {
        TeammateMessage {
            from: from.to_string(),
            text: text.to_string(),
            timestamp: now_rfc3339(),
            read: false,
            color: None,
            summary: None,
            kind,
            source_role,
        }
    }

    #[test]
    fn arbitration_reviewer_correction_outranks_peer_discussion() {
        // A worker's inbox holds an older peer brainstorm message and a newer
        // reviewer correction (the reviewer noticed drift). The correction must
        // be processed FIRST even though it arrived later — but the discussion
        // is NOT dropped (it stays unread for the next round).
        let discussion = classified(
            "bob",
            "what if we also tried X?",
            Some(MessageKind::Discussion),
            Some(SourceRole::Peer),
        );
        let reviewer_fix = classified(
            "reviewer",
            "STOP: you are editing the wrong module, target auth.rs",
            Some(MessageKind::Correction),
            Some(SourceRole::Reviewer),
        );
        let msgs = vec![discussion, reviewer_fix];
        match select_next_inbox(&msgs) {
            NextInbox::Message { index, message } => {
                assert_eq!(index, 1, "reviewer correction should win over discussion");
                assert_eq!(message.from, "reviewer");
            }
            other => panic!("expected reviewer correction, got {other:?}"),
        }
    }

    #[test]
    fn arbitration_human_correction_outranks_reviewer_correction() {
        // Both a teammate reviewer and the human operator (relayed via the lead)
        // send a correction to the same worker. The human's intent wins, but the
        // reviewer's correction is preserved for the following turn.
        let reviewer_fix = classified(
            "reviewer",
            "use a HashMap here",
            Some(MessageKind::Correction),
            Some(SourceRole::Reviewer),
        );
        let human_fix = classified(
            TEAM_LEAD_NAME,
            "operator says: revert that, keep the Vec",
            Some(MessageKind::Correction),
            Some(SourceRole::Human),
        );
        // Human message arrives LAST; must still be selected first.
        let msgs = vec![reviewer_fix, human_fix];
        match select_next_inbox(&msgs) {
            NextInbox::Message { index, message } => {
                assert_eq!(index, 1, "human correction should outrank reviewer");
                assert_eq!(message.source_role, Some(SourceRole::Human));
            }
            other => panic!("expected human correction, got {other:?}"),
        }
    }

    #[test]
    fn arbitration_shutdown_still_beats_every_correction() {
        // Safety invariant: a shutdown request outranks even a human correction
        // so an agent can always wind down.
        let human_fix = classified(
            TEAM_LEAD_NAME,
            "operator: change direction",
            Some(MessageKind::Correction),
            Some(SourceRole::Human),
        );
        let shutdown_text = serde_json::json!({
            "type": "shutdown_request",
            "requestId": "s1",
            "from": "team-lead",
            "timestamp": "2026-06-05T00:00:00.000Z",
        })
        .to_string();
        let shutdown = classified(TEAM_LEAD_NAME, &shutdown_text, None, None);
        let msgs = vec![human_fix, shutdown];
        match select_next_inbox(&msgs) {
            NextInbox::Shutdown { index, .. } => assert_eq!(index, 1),
            other => panic!("expected shutdown to win, got {other:?}"),
        }
    }

    #[test]
    fn arbitration_same_rank_breaks_fifo() {
        // Two reviewer corrections at the same rank: the EARLIER one is processed
        // first (FIFO tie-break), so same-authority sources keep arrival order.
        let first = classified(
            "reviewer-a",
            "fix 1",
            Some(MessageKind::Correction),
            Some(SourceRole::Reviewer),
        );
        let second = classified(
            "reviewer-b",
            "fix 2",
            Some(MessageKind::Correction),
            Some(SourceRole::Reviewer),
        );
        let msgs = vec![first, second];
        match select_next_inbox(&msgs) {
            NextInbox::Message { index, message } => {
                assert_eq!(index, 0, "same-rank corrections keep FIFO order");
                assert_eq!(message.from, "reviewer-a");
            }
            other => panic!("expected first reviewer correction, got {other:?}"),
        }
    }

    #[test]
    fn arbitration_untagged_messages_preserve_legacy_fifo() {
        // Messages with no kind/source_role must behave exactly as before the
        // ladder existed: plain FIFO, with a team-lead message jumping ahead.
        let peer_old = classified("bob", "old chatter", None, None);
        let peer_new = classified("carol", "new chatter", None, None);
        // Two untagged peers → earliest wins (FIFO).
        match select_next_inbox(&[peer_old.clone(), peer_new.clone()]) {
            NextInbox::Message { index, .. } => assert_eq!(index, 0),
            other => panic!("expected FIFO peer, got {other:?}"),
        }
        // Untagged lead message still outranks untagged peer chatter.
        let lead = classified(TEAM_LEAD_NAME, "do the thing", None, None);
        match select_next_inbox(&[peer_old, lead]) {
            NextInbox::Message { index, message } => {
                assert_eq!(index, 1);
                assert_eq!(message.from, TEAM_LEAD_NAME);
            }
            other => panic!("expected lead over peer, got {other:?}"),
        }
    }

    #[test]
    fn attachments_filter_protocol_and_wrap() {
        let root = unique_root();
        let team = "t1";
        let me = "alice";

        write_to_mailbox(
            &root,
            team,
            me,
            TeammateMessage {
                from: "bob".to_string(),
                text: "plain message".to_string(),
                timestamp: now_rfc3339(),
                read: false,
                color: Some("green".to_string()),
                summary: None,
                ..Default::default()
            },
        )
        .unwrap();
        send_shutdown_request(&root, team, me, TEAM_LEAD_NAME, "s1", None, None).unwrap();

        let attachments = teammate_attachments(&root, team, me).unwrap();
        assert_eq!(attachments.len(), 1, "{attachments:?}");
        assert!(attachments[0].contains("teammate_id=\"bob\""));
        assert!(attachments[0].contains("color=\"green\""));
        assert!(attachments[0].contains("plain message"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn task_list_crud_and_id_allocation() {
        let root = unique_root();
        let team = "t1";

        let id1 = create_task(&root, team, Task::new("first", "do first")).unwrap();
        let id2 = create_task(&root, team, Task::new("second", "do second")).unwrap();
        assert_eq!(id1, "1");
        assert_eq!(id2, "2");

        let t1 = get_task(&root, team, &id1).unwrap().unwrap();
        assert_eq!(t1.subject, "first");
        assert_eq!(t1.status, TaskStatus::Pending);
        assert!(t1.blocks.is_empty() && t1.blocked_by.is_empty());

        // camelCase on disk.
        let json = fs::read_to_string(task_path(&root, team, &id1)).unwrap();
        assert!(json.contains("\"blockedBy\""), "{json}");
        assert!(json.contains("\"id\": \"1\""), "{json}");

        update_task(&root, team, &id2, |t| {
            t.status = TaskStatus::InProgress;
            t.owner = Some("alice@t1".to_string());
            t.blocked_by.push("1".to_string());
        })
        .unwrap()
        .unwrap();
        let t2 = get_task(&root, team, &id2).unwrap().unwrap();
        assert_eq!(t2.status, TaskStatus::InProgress);
        assert_eq!(t2.blocked_by, vec!["1".to_string()]);

        assert_eq!(list_tasks(&root, team).unwrap().len(), 2);

        delete_task(&root, team, &id1).unwrap();
        assert!(get_task(&root, team, &id1).unwrap().is_none());
        assert_eq!(list_tasks(&root, team).unwrap().len(), 1);

        // High-water mark prevents ID reuse after reset.
        reset_task_list(&root, team).unwrap();
        assert!(list_tasks(&root, team).unwrap().is_empty());
        let id3 = create_task(&root, team, Task::new("third", "do third")).unwrap();
        assert_eq!(id3, "3", "high-water mark must advance past deleted ids");

        let _ = fs::remove_dir_all(&root);
    }
}
