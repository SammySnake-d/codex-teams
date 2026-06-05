//! Lead-side inbox poller.
//!
//! The lead session reads its OWN on-disk inbox
//! (`$CODEX_HOME/teams/{team}/inboxes/{lead}.json`) every second and injects any
//! teammate replies / idle notifications into the chat as new user turns. Port
//! of Claude Code's `useInboxPoller` (`INBOX_POLL_INTERVAL_MS = 1000`).
//!
//! The teammate side that WRITES to this inbox is the teammate run-loop's idle
//! notification (Phase 2) and, eventually, member→lead `team_send`; this module
//! is purely the consumer.

use std::path::PathBuf;
use std::time::Duration;

use crate::legacy_core::team_coord::format_as_teammate_message;
use crate::legacy_core::team_store::{self, TeammateMessage};

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

/// Claude `INBOX_POLL_INTERVAL_MS`.
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

/// Start polling the lead's inbox every second. On unread messages it emits
/// [`AppEvent::InjectTeammateReplies`] and only THEN marks them read (so a crash
/// between send and mark re-reads them next tick, matching Claude). The
/// `team_store` calls take an advisory file lock with a blocking spin-sleep, so
/// they run on `spawn_blocking` to avoid stalling a runtime worker.
pub(crate) fn start_lead_inbox_poller(
    codex_home: PathBuf,
    team: String,
    lead_name: String,
    app_event_tx: AppEventSender,
) -> LeadInboxPoller {
    let handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(INBOX_POLL_INTERVAL_MS)).await;

            let unread = {
                let codex_home = codex_home.clone();
                let team = team.clone();
                let lead_name = lead_name.clone();
                match tokio::task::spawn_blocking(move || {
                    team_store::read_unread(&codex_home, &team, &lead_name)
                })
                .await
                {
                    Ok(Ok(unread)) => unread,
                    // Missing inbox file or transient lock error: retry next tick.
                    Ok(Err(_)) | Err(_) => continue,
                }
            };
            if unread.is_empty() {
                continue;
            }

            let text = format_teammate_messages(&unread);
            app_event_tx.send(AppEvent::InjectTeammateReplies { text });

            let codex_home = codex_home.clone();
            let team = team.clone();
            let lead_name = lead_name.clone();
            let _ = tokio::task::spawn_blocking(move || {
                team_store::mark_messages_read(&codex_home, &team, &lead_name)
            })
            .await;
        }
    });
    LeadInboxPoller { handle }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(from: &str, text: &str, color: Option<&str>, summary: Option<&str>) -> TeammateMessage {
        TeammateMessage {
            from: from.to_string(),
            text: text.to_string(),
            timestamp: "t".to_string(),
            read: false,
            color: color.map(str::to_string),
            summary: summary.map(str::to_string),
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
}
