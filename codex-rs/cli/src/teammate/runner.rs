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

use anyhow::Result;
use codex_core::CodexThread;
use codex_core::team_coord::IdleOptions;
use codex_core::team_coord::IdleReason;
use codex_core::team_coord::NextInbox;
use codex_core::team_coord::{self};
use codex_core::team_store::TEAM_LEAD_NAME;
use codex_core::team_store::{self};
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;

/// Claude `POLL_INTERVAL_MS` = 500 ms.
const POLL_INTERVAL_MS: u64 = 500;

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

    loop {
        if let Some(prompt) = current_prompt.take() {
            let last_agent_message = run_one_turn(&thread, prompt).await?;
            // Idle notification to the lead on each turn boundary (Claude sends
            // idle on every turn end). Best-effort: a mailbox write failure must
            // not kill the loop.
            let _ = send_idle(&rt, last_agent_message.as_deref());
        }

        match wait_for_next_prompt_or_shutdown(&rt).await? {
            WaitResult::Shutdown { prompt } => {
                // Feed the shutdown text to the model so it can acknowledge /
                // wind down, then exit after that final turn (Claude §1.4).
                let _ = run_one_turn(&thread, prompt).await;
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
async fn run_one_turn(thread: &Arc<CodexThread>, prompt: String) -> Result<Option<String>> {
    // `Op::UserInput` is a `#[non_exhaustive]` 6-field variant; build it via the
    // canonical `From<Vec<UserInput>> for Op` rather than a struct literal.
    let op: Op = vec![UserInput::Text {
        text: prompt,
        text_elements: Vec::new(),
    }]
    .into();
    thread.submit(op).await?;

    let mut last_agent_message: Option<String> = None;
    loop {
        let event = thread.next_event().await?;
        match event.msg {
            EventMsg::AgentMessage(agent) => last_agent_message = Some(agent.message),
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

/// Port of `waitForNextPromptOrShutdown`: poll this teammate's inbox every
/// `POLL_INTERVAL_MS`; resolve as soon as [`team_coord::select_next_inbox`]
/// yields a shutdown request or a regular message. Selection priority
/// (shutdown > lead > FIFO) lives in `team_coord`; `index` is aligned with
/// `team_store::mark_message_read_by_index`.
async fn wait_for_next_prompt_or_shutdown(rt: &TeammateRuntime) -> Result<WaitResult> {
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
                tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
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
