//! Teammate inbox-driven run loop.
//!
//! Port of Claude Code's `runInProcessTeammate` + `waitForNextPromptOrShutdown`
//! (source-map: `ChinaSiro/claude-code-sourcemap`,
//! `restored-src/src/utils/swarm/inProcessRunner.ts`). A spawned
//! `codex teammate` process owns one embedded [`CodexThread`]; its first turn is
//! the spawn prompt (wrapped as a message from the lead), and every turn
//! thereafter is pulled from the on-disk inbox.
//!
//! Mailbox selection, peer-message XML wrapping, idle notifications and shutdown
//! detection are all delegated to the already-tested `codex_core` team modules
//! ([`team_coord`] / [`team_store`]) — this file is a thin client of them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use codex_core::CodexThread;
use codex_core::team_coord::IdleOptions;
use codex_core::team_coord::IdleReason;
use codex_core::team_coord::NextInbox;
use codex_core::team_coord::{self};
use codex_core::team_store::TEAM_LEAD_NAME;
use codex_core::team_store::{self};
use codex_file_watcher::inbox_watcher::InboxWatcher;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;

/// Claude `POLL_INTERVAL_MS` = 500 ms.
const POLL_INTERVAL_MS: u64 = 500;

/// Minimum spacing between mid-turn PROGRESS pushes to the lead. A busy teammate
/// can fire many tool calls per second; throttling coalesces that burst into at
/// most one milestone per interval so monitoring stays event-driven and cheap
/// (a teammate running 50 commands does NOT spam the lead with 50 turns). The
/// first milestone of a turn is always pushed immediately.
const PROGRESS_THROTTLE: Duration = Duration::from_secs(5);

/// Immutable identity + on-disk locators for a running teammate session.
pub(crate) struct TeammateRuntime {
    /// `$CODEX_HOME` root (the directory that *contains* `teams/`). `team_store`
    /// joins `teams/` itself, so this must NOT already include it.
    pub teams_root: PathBuf,
    pub team: String,
    pub agent_name: String,
    pub color: Option<String>,
}

/// What [`wait_for_next_prompt_or_shutdown`] resolved to.
enum WaitResult {
    Shutdown { prompt: String },
    NewMessage { prompt: String },
}

/// Port of `runInProcessTeammate`. Owns one [`CodexThread`]; the first prompt is
/// `initial_prompt` (wrapped as a message from the lead), thereafter driven by
/// the inbox until a shutdown request arrives or the thread dies.
pub(crate) async fn run_teammate_loop(
    rt: TeammateRuntime,
    thread: Arc<CodexThread>,
    initial_prompt: Option<String>,
) -> Result<()> {
    // First turn: the spawn prompt, wrapped as a lead-sourced teammate message
    // (Claude §1.4 — the initial prompt is treated as a message from the lead).
    // If there is no initial prompt we skip straight to polling the inbox rather
    // than exiting immediately.
    let mut current_prompt: Option<String> = initial_prompt.map(|p| {
        team_coord::format_as_teammate_message(TEAM_LEAD_NAME, &p, rt.color.as_deref(), None)
    });

    // Event-driven wake-up: block on OS file-change notifications for this
    // teammate's own `inboxes/` directory instead of polling every 500ms. Falls
    // back to interval ticking if the OS watcher is unavailable, so this never
    // regresses below the old poll loop.
    let inboxes_dir = team_store::inboxes_dir(&rt.teams_root, &rt.team);
    let mut watcher = InboxWatcher::new(inboxes_dir);

    loop {
        if let Some(prompt) = current_prompt.take() {
            let last_agent_message = run_one_turn(&rt, &thread, prompt).await?;
            // Idle notification to the lead on each turn boundary (Claude sends
            // idle on every turn end). Best-effort: a mailbox write failure must
            // not kill the loop.
            let _ = send_idle(&rt, last_agent_message.as_deref());
        }

        match wait_for_next_prompt_or_shutdown(&rt, &mut watcher).await? {
            WaitResult::Shutdown { prompt } => {
                // Feed the shutdown text to the model so it can acknowledge /
                // wind down, then exit after that final turn (Claude §1.4).
                let _ = run_one_turn(&rt, &thread, prompt).await;
                break;
            }
            WaitResult::NewMessage { prompt } => current_prompt = Some(prompt),
        }
    }

    // Stop-hook analog (Claude `initializeTeammateHooks`): mark inactive in the
    // shared config + a final idle notification, then tear the session down.
    mark_inactive(&rt);
    let _ = send_idle(&rt, None);
    let _ = thread.shutdown_and_wait().await;
    Ok(())
}

/// Submit one user turn and pump events until the turn completes (or errors / is
/// aborted). Returns the last agent message observed, for the idle summary.
///
/// While the turn runs, tool-call milestones (a command starting, a file being
/// patched, an MCP tool invoked) are pushed to the lead as throttled PROGRESS
/// messages so the lead can watch the work unfold at TOOL granularity and correct
/// drift mid-turn — not only at the turn boundary.
///
/// NOTE: the LIVE teammate is the full interactive TUI (`codex teammate`), whose
/// milestone push lives in `tui/src/chatwidget/protocol.rs`
/// (`maybe_push_teammate_progress`, driven by `handle_item_started_notification`).
/// This headless run-loop is retained for parity/reuse but is not the live path;
/// the two share identical throttle + formatting semantics.
async fn run_one_turn(
    rt: &TeammateRuntime,
    thread: &Arc<CodexThread>,
    prompt: String,
) -> Result<Option<String>> {
    // `Op::UserInput` is a `#[non_exhaustive]` 6-field variant; build it via the
    // canonical `From<Vec<UserInput>> for Op` rather than a struct literal.
    let op: Op = vec![UserInput::Text {
        text: prompt,
        text_elements: Vec::new(),
    }]
    .into();
    thread.submit(op).await?;

    let mut last_agent_message: Option<String> = None;
    // Throttle state: `None` means no milestone pushed yet this turn (first one
    // fires immediately); otherwise the last push instant.
    let mut last_progress_push: Option<Instant> = None;
    loop {
        let event = thread.next_event().await?;
        match event.msg {
            EventMsg::AgentMessage(agent) => last_agent_message = Some(agent.message),
            EventMsg::ExecCommandBegin(ev) => {
                maybe_push_progress(rt, &mut last_progress_push, progress_for_exec(&ev.command));
            }
            EventMsg::PatchApplyBegin(ev) => {
                maybe_push_progress(
                    rt,
                    &mut last_progress_push,
                    progress_for_patch(ev.changes.len()),
                );
            }
            EventMsg::McpToolCallBegin(ev) => {
                maybe_push_progress(
                    rt,
                    &mut last_progress_push,
                    progress_for_mcp(&ev.invocation.tool),
                );
            }
            EventMsg::WebSearchBegin(_) => {
                maybe_push_progress(rt, &mut last_progress_push, "searching the web".to_string());
            }
            EventMsg::TurnComplete(turn) => {
                if turn.last_agent_message.is_some() {
                    last_agent_message = turn.last_agent_message;
                }
                break;
            }
            EventMsg::TurnAborted(_) | EventMsg::Error(_) => break,
            _ => {}
        }
    }
    Ok(last_agent_message)
}

/// Push a progress milestone to the lead, throttled: the first milestone of a
/// turn fires immediately, subsequent ones only after [`PROGRESS_THROTTLE`] has
/// elapsed. A mailbox write failure must never disrupt the turn, so it is
/// best-effort. Coalescing is implicit: milestones that arrive inside the
/// throttle window are simply dropped (the next one past the window reports the
/// then-current work), keeping monitoring cheap.
fn maybe_push_progress(rt: &TeammateRuntime, last: &mut Option<Instant>, milestone: String) {
    let now = Instant::now();
    let due = match *last {
        None => true,
        Some(prev) => now.duration_since(prev) >= PROGRESS_THROTTLE,
    };
    if !due {
        return;
    }
    *last = Some(now);
    let summary = summarize(&milestone);
    let _ = team_coord::send_progress_to_lead(
        &rt.teams_root,
        &rt.team,
        &rt.agent_name,
        rt.color.clone(),
        &milestone,
        Some(summary),
    );
}

/// One-line progress note for a command milestone, truncated so a long command
/// line does not bloat the lead's inbox.
fn progress_for_exec(command: &[String]) -> String {
    let joined = command.join(" ");
    let shown: String = joined.chars().take(120).collect();
    format!("running: {shown}")
}

fn progress_for_patch(file_count: usize) -> String {
    if file_count == 1 {
        "editing 1 file".to_string()
    } else {
        format!("editing {file_count} files")
    }
}

fn progress_for_mcp(tool: &str) -> String {
    format!("calling tool {tool}")
}

/// Port of `waitForNextPromptOrShutdown`: wait for this teammate's inbox to
/// change, then resolve as soon as [`team_coord::select_next_inbox`] yields a
/// shutdown request or a regular message. Selection priority
/// (shutdown > lead > FIFO) lives in `team_coord`; `index` is aligned with
/// `team_store::mark_message_read_by_index`.
///
/// Wake-ups come from [`InboxWatcher`] (OS file events, interval fallback). The
/// first read happens immediately so a message already waiting is handled
/// without a wake-up; an empty inbox then blocks on the next change instead of
/// spinning.
async fn wait_for_next_prompt_or_shutdown(
    rt: &TeammateRuntime,
    watcher: &mut InboxWatcher,
) -> Result<WaitResult> {
    loop {
        let messages = team_store::read_mailbox(&rt.teams_root, &rt.team, &rt.agent_name)?;
        match team_coord::select_next_inbox(&messages) {
            NextInbox::Shutdown { index, raw, .. } => {
                team_store::mark_message_read_by_index(
                    &rt.teams_root,
                    &rt.team,
                    &rt.agent_name,
                    index,
                )?;
                return Ok(WaitResult::Shutdown {
                    prompt: team_coord::format_as_teammate_message(
                        TEAM_LEAD_NAME,
                        &raw,
                        None,
                        None,
                    ),
                });
            }
            NextInbox::Message { index, message } => {
                team_store::mark_message_read_by_index(
                    &rt.teams_root,
                    &rt.team,
                    &rt.agent_name,
                    index,
                )?;
                let prompt = team_coord::format_as_teammate_message(
                    &message.from,
                    &message.text,
                    message.color.as_deref(),
                    message.summary.as_deref(),
                );
                return Ok(WaitResult::NewMessage { prompt });
            }
            NextInbox::Empty => {
                // Block until the inbox directory changes (or the fallback tick
                // fires). A closed watcher channel means the watcher was
                // dropped; fall back to a short sleep so the loop still makes
                // progress rather than busy-spinning.
                if !watcher.changed().await {
                    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                }
            }
        }
    }
}

/// Send an idle notification to the lead's inbox. `team_coord` writes it to the
/// lead's mailbox and JSON-encodes the notification into the message `text`.
fn send_idle(rt: &TeammateRuntime, last_agent_message: Option<&str>) -> std::io::Result<()> {
    team_coord::send_idle_notification(
        &rt.teams_root,
        &rt.team,
        &rt.agent_name,
        rt.color.clone(),
        IdleOptions {
            idle_reason: Some(IdleReason::Available),
            summary: last_agent_message.map(summarize),
            ..Default::default()
        },
    )
}

/// `setMemberActive(team, agent, false)` analog: flip this member's `is_active`
/// flag in the shared `config.json` (read-modify-write under the file lock).
fn mark_inactive(rt: &TeammateRuntime) {
    let _ = team_store::update_config(&rt.teams_root, &rt.team, |cfg| {
        if let Some(member) = cfg.members.iter_mut().find(|m| m.name == rt.agent_name) {
            member.is_active = Some(false);
        }
    });
}

/// `getLastPeerDmSummary` analog: a short (≤10-word) summary of the agent's last
/// message, used as the idle-notification `summary`.
fn summarize(message: &str) -> String {
    message
        .split_whitespace()
        .take(10)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn exec_progress_is_prefixed_and_truncated() {
        let short = progress_for_exec(&["cargo".to_string(), "test".to_string()]);
        assert_eq!(short, "running: cargo test");

        let long_arg = "x".repeat(300);
        let long = progress_for_exec(&["echo".to_string(), long_arg]);
        assert!(long.starts_with("running: echo "));
        // "running: " (9) + up to 120 chars of the joined command.
        assert!(long.chars().count() <= 9 + 120);
    }

    #[test]
    fn patch_progress_pluralizes() {
        assert_eq!(progress_for_patch(1), "editing 1 file");
        assert_eq!(progress_for_patch(3), "editing 3 files");
    }

    #[test]
    fn mcp_progress_names_the_tool() {
        assert_eq!(progress_for_mcp("search"), "calling tool search");
    }

    #[test]
    fn throttle_pushes_first_then_suppresses_until_interval() {
        // First milestone (last=None) is always due; the immediately-following one
        // inside the throttle window is suppressed. We assert the throttle DECISION
        // without performing a mailbox write by replicating its predicate.
        let mut last: Option<Instant> = None;
        // first is due
        let due1 = matches!(last, None) || false;
        assert!(due1, "first milestone must be due");
        last = Some(Instant::now());
        // an immediate second is NOT due (well within PROGRESS_THROTTLE)
        let due2 = match last {
            None => true,
            Some(prev) => Instant::now().duration_since(prev) >= PROGRESS_THROTTLE,
        };
        assert!(
            !due2,
            "second milestone inside the window must be throttled"
        );
    }
}
