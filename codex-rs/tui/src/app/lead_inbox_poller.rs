//! Teams inbox pollers.
//!
//! The lead session reads its OWN on-disk inbox
//! (`$CODEX_HOME/teams/{team}/inboxes/{lead}.json`) every second and injects any
//! explicit teammate replies into the chat as new user turns. Idle lifecycle
//! notifications update Teams state but must not become model-visible lead
//! turns. Port of Claude Code's `useInboxPoller`
//! (`INBOX_POLL_INTERVAL_MS = 1000`).
//!
//! A spawned teammate TUI also reads its OWN on-disk inbox
//! (`$CODEX_HOME/teams/{team}/inboxes/{agent_name}.json`) and injects lead/peer
//! messages as turns in that teammate's own session.

use std::path::PathBuf;

use codex_file_watcher::inbox_watcher::InboxWatcher;

use crate::legacy_core::team_coord::IdleOptions;
use crate::legacy_core::team_coord::IdleReason;
use crate::legacy_core::team_coord::NextInbox;
use crate::legacy_core::team_coord::format_as_teammate_message;
use crate::legacy_core::team_coord::parse_idle_notification;
use crate::legacy_core::team_coord::select_next_inbox;
use crate::legacy_core::team_coord::send_idle_notification;
use crate::legacy_core::team_store::TEAM_LEAD_NAME;
use crate::legacy_core::team_store::TeammateMessage;
use crate::legacy_core::team_store::{self};

use crate::app_event::AppEvent;
use crate::app_event::TeammateInboxAck;
use crate::app_event_sender::AppEventSender;

/// Claude `INBOX_POLL_INTERVAL_MS`. Retained as the documented fallback cadence;
/// the live wake-up path now comes from [`InboxWatcher`], so this is referenced
/// only by tests that assert timing.
#[cfg_attr(not(test), allow(dead_code))]
const INBOX_POLL_INTERVAL_MS: u64 = 1000;

/// Handle to a running lead inbox poll task. Call [`LeadInboxPoller::stop`] (or
/// drop it) to cancel the task.
pub(crate) struct LeadInboxPoller {
    handle: tokio::task::JoinHandle<()>,
}

impl LeadInboxPoller {
    pub(crate) fn stop(self) {
        self.handle.abort();
    }
}

impl Drop for LeadInboxPoller {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Handle to a running teammate inbox poll task. Drop it to cancel the task and
/// publish the teammate's final idle lifecycle update.
pub(crate) struct TeammateInboxPoller {
    handle: tokio::task::JoinHandle<()>,
    lifecycle: TeammateLifecycle,
}

#[derive(Clone, Debug)]
pub(crate) struct TeammateLifecycle {
    codex_home: PathBuf,
    team: String,
    agent_name: String,
    color: Option<String>,
}

impl TeammateLifecycle {
    fn new(codex_home: PathBuf, team: String, agent_name: String) -> Self {
        let color = team_store::read_config(&codex_home, &team)
            .ok()
            .flatten()
            .and_then(|config| {
                config
                    .members
                    .into_iter()
                    .find(|member| member.name == agent_name)
                    .and_then(|member| member.color)
            });
        Self {
            codex_home,
            team,
            agent_name,
            color,
        }
    }

    pub(crate) fn notify_idle(&self, last_agent_message: Option<&str>) -> std::io::Result<()> {
        send_idle_notification(
            &self.codex_home,
            &self.team,
            &self.agent_name,
            self.color.clone(),
            IdleOptions {
                idle_reason: Some(IdleReason::Available),
                summary: last_agent_message.map(summarize),
                ..Default::default()
            },
        )
    }

    fn mark_inactive(&self) -> std::io::Result<()> {
        team_store::update_config(&self.codex_home, &self.team, |config| {
            if let Some(member) = config
                .members
                .iter_mut()
                .find(|member| member.name == self.agent_name)
            {
                member.is_active = Some(false);
            }
        })
    }
}

impl Drop for TeammateInboxPoller {
    fn drop(&mut self) {
        self.handle.abort();
        let _ = self.lifecycle.mark_inactive();
        let _ = self.lifecycle.notify_idle(/*last_agent_message*/ None);
    }
}

impl TeammateInboxPoller {
    pub(crate) fn notify_idle(&self, last_agent_message: Option<&str>) -> std::io::Result<()> {
        self.lifecycle.notify_idle(last_agent_message)
    }
}

/// Wrap each teammate message as `<teammate-message …>` and join with a blank
/// line (port of the formatting in `useInboxPoller` / `runHeadlessStreaming`).
/// Reuses the core wrapper so the tag + attributes match the rest of the port.
pub(crate) fn format_teammate_messages(messages: &[TeammateMessage]) -> String {
    messages
        .iter()
        .map(|message| {
            format_as_teammate_message(
                &message.from,
                &message.text,
                message.color.as_deref(),
                message.summary.as_deref(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(crate) fn format_lead_teammate_messages(messages: &[TeammateMessage]) -> String {
    let regular_messages: Vec<TeammateMessage> = messages
        .iter()
        .filter(|message| parse_idle_notification(&message.text).is_none())
        .cloned()
        .collect();
    format_teammate_messages(&regular_messages)
}

pub(crate) fn format_teammate_inbox_message(message: TeammateMessage) -> String {
    let text = if message.from == TEAM_LEAD_NAME {
        strip_legacy_visible_team_context(&message.text)
    } else {
        message.text.as_str()
    };
    format_as_teammate_message(
        &message.from,
        text,
        message.color.as_deref(),
        message.summary.as_deref(),
    )
}

pub(crate) fn format_shutdown_inbox_message(from: String, raw: String) -> String {
    format_as_teammate_message(&from, &raw, None, None)
}

fn strip_legacy_visible_team_context(text: &str) -> &str {
    const PREFIX: &str = "Codex Teams context:";
    const END_MARKER: &str = "Do not create teams or spawn teammates from this teammate process.";
    const CLAUDE_CONTEXT_PREFIX: &str = "# Agent Teammate Communication";
    const CLAUDE_CONTEXT_MARKER: &str = "The user interacts primarily with the team lead. Your work is coordinated through the task system and teammate messaging.";
    const CODEX_CONTEXT_PREFIX: &str = "# Codex Teams Teammate Communication";
    const CODEX_CONTEXT_MARKER: &str = "The user interacts primarily with the team lead. Your work is coordinated through Teams tasks and teammate messaging.";
    let trimmed_addendum = text.trim_start();
    for (prefix, marker) in [
        (CLAUDE_CONTEXT_PREFIX, CLAUDE_CONTEXT_MARKER),
        (CODEX_CONTEXT_PREFIX, CODEX_CONTEXT_MARKER),
    ] {
        if !trimmed_addendum.starts_with(prefix) {
            continue;
        }
        let Some((_, task)) = trimmed_addendum.split_once(marker) else {
            continue;
        };
        let task = task.trim_start();
        if !task.is_empty() {
            return task;
        }
    }
    let trimmed = text.trim_start();
    if !trimmed.starts_with(PREFIX) {
        return text;
    }
    let Some((_, task)) = trimmed.split_once(END_MARKER) else {
        return text;
    };
    let task = task.trim_start();
    if task.is_empty() { text } else { task }
}

fn summarize(message: &str) -> String {
    message
        .split_whitespace()
        .take(10)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Start polling the lead's inbox every second. On unread explicit teammate
/// messages it emits [`AppEvent::InjectTeammateReplies`] with an ack payload.
/// The app marks the snapshot read only after the chat widget accepts or queues
/// the injected turn, matching Claude's delivery-before-read semantics. Idle
/// lifecycle notifications are consumed without injecting a lead user turn. The
/// `team_store` calls take an advisory file lock with a blocking spin-sleep, so
/// they run on `spawn_blocking` to avoid stalling a runtime worker.
pub(crate) fn start_lead_inbox_poller(
    codex_home: PathBuf,
    team: String,
    lead_name: String,
    app_event_tx: AppEventSender,
) -> LeadInboxPoller {
    let handle = tokio::spawn(async move {
        // Event-driven wake-up: block on OS file-change notifications for the
        // lead's `inboxes/` directory instead of polling on a fixed interval.
        // Falls back to interval ticking if the OS watcher is unavailable, so
        // this never regresses below the old poll loop. A wake-up means "read
        // now"; we still read + diff + ack, preserving delivery-before-read.
        let inboxes_dir = team_store::inboxes_dir(&codex_home, &team);
        let mut watcher = InboxWatcher::new(inboxes_dir);
        // Read once immediately to catch messages already unread at startup,
        // then wait for the next change before each subsequent read.
        let mut wait_first = false;

        loop {
            if wait_first {
                if !watcher.changed().await {
                    break;
                }
            } else {
                wait_first = true;
            }

            let unread = {
                let codex_home = codex_home.clone();
                let team = team.clone();
                let lead_name = lead_name.clone();
                match tokio::task::spawn_blocking(move || {
                    team_store::read_unread_with_indices(&codex_home, &team, &lead_name)
                })
                .await
                {
                    Ok(Ok(unread)) => unread,
                    // Missing inbox file or transient lock error: wait for the
                    // next change and retry.
                    Ok(Err(_)) | Err(_) => continue,
                }
            };
            if unread.is_empty() {
                continue;
            }

            let unread_messages = unread
                .iter()
                .map(|entry| entry.message.clone())
                .collect::<Vec<_>>();
            let indices = unread.iter().map(|entry| entry.index).collect::<Vec<_>>();
            let text = format_lead_teammate_messages(&unread_messages);
            if text.is_empty() {
                let codex_home = codex_home.clone();
                let team = team.clone();
                let lead_name = lead_name.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    team_store::mark_messages_read_by_indices(
                        &codex_home,
                        &team,
                        &lead_name,
                        &indices,
                    )
                })
                .await;
                continue;
            }
            let ack = TeammateInboxAck {
                codex_home: codex_home.clone(),
                team: team.clone(),
                agent_name: lead_name.clone(),
                indices,
            };
            if !app_event_tx.send_checked(AppEvent::InjectTeammateReplies { text, ack }) {
                continue;
            }
        }
    });
    LeadInboxPoller { handle }
}

/// Start polling this teammate's own inbox. Lead-origin task text stays the
/// message body of a `team-lead` teammate message; stale visible Teams metadata
/// envelopes are stripped before wrapping. Peer messages and shutdown requests
/// are wrapped as teammate messages.
pub(crate) fn start_teammate_inbox_poller(
    codex_home: PathBuf,
    team: String,
    agent_name: String,
    app_event_tx: AppEventSender,
) -> TeammateInboxPoller {
    let lifecycle = TeammateLifecycle::new(codex_home.clone(), team.clone(), agent_name.clone());
    let handle = tokio::spawn(async move {
        // Event-driven wake-up on this teammate's own `inboxes/` directory (see
        // the lead poller for rationale). First pass reads immediately; each
        // later pass waits for a change signal.
        let inboxes_dir = team_store::inboxes_dir(&codex_home, &team);
        let mut watcher = InboxWatcher::new(inboxes_dir);
        let mut poll_immediately = true;
        loop {
            if poll_immediately {
                poll_immediately = false;
            } else if !watcher.changed().await {
                break;
            }

            let selected = {
                let codex_home = codex_home.clone();
                let team = team.clone();
                let agent_name = agent_name.clone();
                match tokio::task::spawn_blocking(move || {
                    let messages = team_store::read_mailbox(&codex_home, &team, &agent_name)?;
                    Ok::<NextInbox, std::io::Error>(select_next_inbox(&messages))
                })
                .await
                {
                    Ok(Ok(selected)) => selected,
                    // Missing inbox file or transient lock error: wait for the
                    // next change and retry.
                    Ok(Err(_)) | Err(_) => continue,
                }
            };

            let (index, text) = match selected {
                NextInbox::Shutdown {
                    index,
                    request,
                    raw,
                } => (index, format_shutdown_inbox_message(request.from, raw)),
                NextInbox::Message { index, message } => {
                    (index, format_teammate_inbox_message(message))
                }
                NextInbox::Empty => continue,
            };
            let ack = TeammateInboxAck {
                codex_home: codex_home.clone(),
                team: team.clone(),
                agent_name: agent_name.clone(),
                indices: vec![index],
            };
            app_event_tx.send(AppEvent::InjectTeammateInboxMessage { text, ack });
        }
    });
    TeammateInboxPoller { handle, lifecycle }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_core::team_coord::parse_idle_notification;
    use pretty_assertions::assert_eq;
    use std::time::Duration;

    fn msg(from: &str, text: &str, color: Option<&str>, summary: Option<&str>) -> TeammateMessage {
        TeammateMessage {
            from: from.to_string(),
            text: text.to_string(),
            timestamp: "t".to_string(),
            read: false,
            color: color.map(str::to_string),
            summary: summary.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn format_wraps_each_and_joins_with_blank_line() {
        let out = format_teammate_messages(&[
            msg("alice", "hello", Some("red"), None),
            msg("bob", "hi", None, Some("greet")),
        ]);
        assert!(out.contains(
            "<teammate-message teammate_id=\"alice\" color=\"red\">\nhello\n</teammate-message>"
        ));
        assert!(out.contains(
            "<teammate-message teammate_id=\"bob\" summary=\"greet\">\nhi\n</teammate-message>"
        ));
        assert!(out.contains("</teammate-message>\n\n<teammate-message"));
    }

    #[test]
    fn format_empty_is_empty() {
        assert_eq!(format_teammate_messages(&[]), "");
    }

    #[test]
    fn lead_format_filters_idle_notifications() {
        let idle = serde_json::json!({
            "type": "idle_notification",
            "from": "alice",
            "timestamp": "2026-06-05T00:00:00.000Z",
            "idleReason": "available",
        })
        .to_string();
        let out = format_lead_teammate_messages(&[
            msg("alice", &idle, Some("green"), None),
            msg("alice", "explicit update", Some("green"), Some("done")),
        ]);

        assert_eq!(
            out,
            "<teammate-message teammate_id=\"alice\" color=\"green\" summary=\"done\">\nexplicit update\n</teammate-message>"
        );
    }

    #[test]
    fn lead_format_returns_empty_for_idle_only() {
        let idle = serde_json::json!({
            "type": "idle_notification",
            "from": "alice",
            "timestamp": "2026-06-05T00:00:00.000Z",
        })
        .to_string();

        assert_eq!(
            format_lead_teammate_messages(&[msg("alice", &idle, None, None)]),
            ""
        );
    }

    #[tokio::test]
    async fn lead_poller_emits_wrapped_replies_and_marks_them_read() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let team = "Rocket".to_string();
        team_store::write_to_mailbox(
            codex_home.path(),
            &team,
            TEAM_LEAD_NAME,
            msg("alice", "first update", Some("red"), None),
        )
        .expect("write first lead inbox message");
        team_store::write_to_mailbox(
            codex_home.path(),
            &team,
            TEAM_LEAD_NAME,
            msg("bob", "second update", Some("blue"), Some("done")),
        )
        .expect("write second lead inbox message");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let poller = start_lead_inbox_poller(
            codex_home.path().to_path_buf(),
            team.clone(),
            TEAM_LEAD_NAME.to_string(),
            AppEventSender::new(tx),
        );

        let event = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("poller event")
            .expect("event channel");
        match event {
            AppEvent::InjectTeammateReplies { text, ack } => {
                assert_eq!(
                    text,
                    "<teammate-message teammate_id=\"alice\" color=\"red\">\nfirst update\n</teammate-message>\n\n<teammate-message teammate_id=\"bob\" color=\"blue\" summary=\"done\">\nsecond update\n</teammate-message>"
                );
                team_store::mark_messages_read_by_indices(
                    &ack.codex_home,
                    &ack.team,
                    &ack.agent_name,
                    &ack.indices,
                )
                .expect("ack lead inbox messages");
            }
            other => panic!("expected lead inbox injection event, got {other:?}"),
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if team_store::read_unread(codex_home.path(), &team, TEAM_LEAD_NAME)
                    .expect("read lead inbox")
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("lead inbox messages marked read");

        drop(poller);
    }

    #[tokio::test]
    async fn lead_poller_keeps_replies_unread_when_injection_fails() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let team = "Rocket".to_string();
        team_store::write_to_mailbox(
            codex_home.path(),
            &team,
            TEAM_LEAD_NAME,
            msg("alice", "first update", Some("red"), None),
        )
        .expect("write lead inbox message");
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);

        let poller = start_lead_inbox_poller(
            codex_home.path().to_path_buf(),
            team.clone(),
            TEAM_LEAD_NAME.to_string(),
            AppEventSender::new(tx),
        );

        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::time::sleep(Duration::from_millis(INBOX_POLL_INTERVAL_MS + 100)).await;
            let messages = team_store::read_mailbox(codex_home.path(), &team, TEAM_LEAD_NAME)
                .expect("read lead inbox");
            assert_eq!(messages.len(), 1);
            assert!(!messages[0].read);
        })
        .await
        .expect("lead inbox stayed unread after failed injection");

        drop(poller);
    }

    #[test]
    fn teammate_inbox_wraps_lead_message() {
        let out = format_teammate_inbox_message(msg(
            TEAM_LEAD_NAME,
            "Please inspect task 1.",
            None,
            None,
        ));

        assert_eq!(
            out,
            format!(
                "<teammate-message teammate_id=\"{TEAM_LEAD_NAME}\">\nPlease inspect task 1.\n</teammate-message>"
            )
        );
    }

    #[test]
    fn teammate_inbox_strips_legacy_visible_team_context_from_lead_message() {
        let legacy = "\
Codex Teams context:
  - team_id: team-1
  - team_name: Rocket
  - lead_thread_id: lead-1
  - member_id: member-1
  - member_name: alice
  - profile: Read-only locator
  - capabilities: code-search
  - permissions: read-only
  - live_session_only: true

You are an independent Codex Teams teammate. Do not assume you inherit the lead conversation history.
Treat the spawn prompt/items after this context as your assigned task boundary.
Use team_send to reply to the lead or another named teammate.
Do not create teams or spawn teammates from this teammate process.

Please inspect task 1.";
        let out = format_teammate_inbox_message(msg(TEAM_LEAD_NAME, legacy, None, None));

        assert_eq!(
            out,
            format!(
                "<teammate-message teammate_id=\"{TEAM_LEAD_NAME}\">\nPlease inspect task 1.\n</teammate-message>"
            )
        );
    }

    #[test]
    fn teammate_inbox_strips_legacy_codex_addendum_context_from_lead_message() {
        let legacy = "\
# Codex Teams Teammate Communication

Plain assistant text is not visible to other teammates or the lead.

The user interacts primarily with the team lead. Your work is coordinated through Teams tasks and teammate messaging.

Please inspect task 1.";
        let out = format_teammate_inbox_message(msg(TEAM_LEAD_NAME, legacy, None, None));

        assert_eq!(
            out,
            format!(
                "<teammate-message teammate_id=\"{TEAM_LEAD_NAME}\">\nPlease inspect task 1.\n</teammate-message>"
            )
        );
    }

    #[test]
    fn teammate_inbox_strips_current_claude_addendum_context_from_lead_message() {
        let legacy = "\
# Agent Teammate Communication

IMPORTANT: You are running as an agent in a team. To communicate with anyone on your team:
- Use the SendMessage tool with `to: \"<name>\"` to send messages to specific teammates
- Use the SendMessage tool with `to: \"*\"` sparingly for team-wide broadcasts

Just writing a response in text is not visible to others on your team - you MUST use the SendMessage tool.

The user interacts primarily with the team lead. Your work is coordinated through the task system and teammate messaging.

Please inspect task 1.";
        let out = format_teammate_inbox_message(msg(TEAM_LEAD_NAME, legacy, None, None));

        assert_eq!(
            out,
            format!(
                "<teammate-message teammate_id=\"{TEAM_LEAD_NAME}\">\nPlease inspect task 1.\n</teammate-message>"
            )
        );
    }

    #[test]
    fn teammate_inbox_preserves_quoted_claude_addendum_marker() {
        let text = "\
Please verify this exact sentence remains visible:
The user interacts primarily with the team lead. Your work is coordinated through the task system and teammate messaging.
Then continue.";
        let out = format_teammate_inbox_message(msg(TEAM_LEAD_NAME, text, None, None));

        assert_eq!(
            out,
            format!(
                "<teammate-message teammate_id=\"{TEAM_LEAD_NAME}\">\n{text}\n</teammate-message>"
            )
        );
    }

    #[test]
    fn teammate_inbox_wraps_peer_message() {
        let out =
            format_teammate_inbox_message(msg("alice", "peer update", Some("blue"), Some("done")));

        assert_eq!(
            out,
            "<teammate-message teammate_id=\"alice\" color=\"blue\" summary=\"done\">\npeer update\n</teammate-message>"
        );
    }

    #[test]
    fn teammate_inbox_wraps_shutdown_request_raw_text() {
        let raw =
            r#"{"type":"shutdown_request","from":"team-lead","requestId":"s1","timestamp":"t"}"#;
        let out = format_shutdown_inbox_message(TEAM_LEAD_NAME.to_string(), raw.to_string());

        assert_eq!(
            out,
            format!(
                "<teammate-message teammate_id=\"{TEAM_LEAD_NAME}\">\n{raw}\n</teammate-message>"
            )
        );
    }

    #[tokio::test]
    async fn teammate_poller_emits_message_and_marks_it_read() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let team = "Rocket".to_string();
        let agent_name = "alice".to_string();
        team_store::write_to_mailbox(
            codex_home.path(),
            &team,
            &agent_name,
            msg(TEAM_LEAD_NAME, "Please inspect task 1.", None, None),
        )
        .expect("write teammate inbox");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let poller = start_teammate_inbox_poller(
            codex_home.path().to_path_buf(),
            team.clone(),
            agent_name.clone(),
            AppEventSender::new(tx),
        );

        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("poller event")
            .expect("event channel");
        match event {
            AppEvent::InjectTeammateInboxMessage { text, ack } => {
                assert_eq!(
                    text,
                    format!(
                        "<teammate-message teammate_id=\"{TEAM_LEAD_NAME}\">\nPlease inspect task 1.\n</teammate-message>"
                    )
                );
                team_store::mark_messages_read_by_indices(
                    &ack.codex_home,
                    &ack.team,
                    &ack.agent_name,
                    &ack.indices,
                )
                .expect("ack teammate inbox message");
            }
            other => panic!("expected teammate inbox injection event, got {other:?}"),
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let messages = team_store::read_mailbox(codex_home.path(), &team, &agent_name)
                    .expect("read teammate inbox");
                assert_eq!(messages.len(), 1);
                if messages[0].read {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("teammate inbox message marked read after injection");

        drop(poller);
    }

    #[tokio::test]
    async fn teammate_poller_keeps_message_unread_when_injection_fails() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let team = "Rocket".to_string();
        let agent_name = "alice".to_string();
        team_store::write_to_mailbox(
            codex_home.path(),
            &team,
            &agent_name,
            msg(TEAM_LEAD_NAME, "Please inspect task 1.", None, None),
        )
        .expect("write teammate inbox");
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);

        let poller = start_teammate_inbox_poller(
            codex_home.path().to_path_buf(),
            team.clone(),
            agent_name.clone(),
            AppEventSender::new(tx),
        );

        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::time::sleep(Duration::from_millis(INBOX_POLL_INTERVAL_MS + 100)).await;
            let messages = team_store::read_mailbox(codex_home.path(), &team, &agent_name)
                .expect("read teammate inbox");
            assert_eq!(messages.len(), 1);
            assert!(!messages[0].read);
        })
        .await
        .expect("teammate inbox stayed unread after failed injection");

        drop(poller);
    }

    #[test]
    fn teammate_lifecycle_sends_idle_with_summary_and_color() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let team = "Rocket".to_string();
        let agent_name = "alice".to_string();
        team_store::update_config(codex_home.path(), &team, |config| {
            config.name = team.clone();
            config.lead_agent_id = team_store::agent_id(TEAM_LEAD_NAME, &team);
            config.members.push(team_store::TeamFileMember {
                agent_id: team_store::agent_id(&agent_name, &team),
                name: agent_name.clone(),
                color: Some("green".to_string()),
                is_active: Some(true),
                ..Default::default()
            });
        })
        .expect("write team config");

        let lifecycle = TeammateLifecycle::new(
            codex_home.path().to_path_buf(),
            team.clone(),
            agent_name.clone(),
        );
        lifecycle
            .notify_idle(Some(
                "one two three four five six seven eight nine ten eleven twelve",
            ))
            .expect("send idle");

        let messages =
            team_store::read_mailbox(codex_home.path(), &team, TEAM_LEAD_NAME).expect("read lead");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from, agent_name);
        assert_eq!(messages[0].color.as_deref(), Some("green"));
        let idle = parse_idle_notification(&messages[0].text).expect("idle notification");
        assert_eq!(idle.idle_reason, Some(IdleReason::Available));
        assert_eq!(
            idle.summary.as_deref(),
            Some("one two three four five six seven eight nine ten")
        );
    }

    #[tokio::test]
    async fn teammate_poller_drop_marks_inactive_and_sends_final_idle() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let team = "Rocket".to_string();
        let agent_name = "alice".to_string();
        team_store::update_config(codex_home.path(), &team, |config| {
            config.name = team.clone();
            config.lead_agent_id = team_store::agent_id(TEAM_LEAD_NAME, &team);
            config.members.push(team_store::TeamFileMember {
                agent_id: team_store::agent_id(&agent_name, &team),
                name: agent_name.clone(),
                color: Some("yellow".to_string()),
                is_active: Some(true),
                ..Default::default()
            });
        })
        .expect("write team config");
        let lifecycle = TeammateLifecycle::new(
            codex_home.path().to_path_buf(),
            team.clone(),
            agent_name.clone(),
        );
        let poller = TeammateInboxPoller {
            handle: tokio::spawn(async {
                std::future::pending::<()>().await;
            }),
            lifecycle,
        };

        drop(poller);

        let config = team_store::read_config(codex_home.path(), &team)
            .expect("read config")
            .expect("config");
        let member = config
            .members
            .into_iter()
            .find(|member| member.name == agent_name)
            .expect("member");
        assert_eq!(member.is_active, Some(false));

        let messages =
            team_store::read_mailbox(codex_home.path(), &team, TEAM_LEAD_NAME).expect("read lead");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].color.as_deref(), Some("yellow"));
        let idle = parse_idle_notification(&messages[0].text).expect("idle notification");
        assert_eq!(idle.idle_reason, Some(IdleReason::Available));
        assert_eq!(idle.summary, None);
    }
}
