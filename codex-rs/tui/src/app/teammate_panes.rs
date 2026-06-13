//! Best-effort external teammate panes.
//!
//! Split-pane process teammates are launched by the core Teams backend. This TUI
//! module owns only best-effort pane side effects for those existing panes:
//! focusing and hiding/showing by pane id.
//!
//! Everything here is best-effort: a missing multiplexer or any tmux/iTerm quirk is
//! logged and ignored so it can never disrupt the lead session.

use std::process::Command;
use std::process::Stdio;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_terminal_detection::Multiplexer;
use codex_terminal_detection::terminal_info;

use super::App;

impl App {
    /// Focus a teammate's pane (port of Claude's `viewTeammateOutput`, §B.5).
    ///
    /// The iTerm2 backend focuses via `it2 session focus -s <pane>` like Claude.
    /// Every other backend uses tmux:
    /// `select-pane` on the ambient server when the TUI is inside tmux,
    /// otherwise on the detached swarm socket `codex-swarm-<pid>`. Best-effort:
    /// any failure is logged and swallowed so it can never disrupt the lead
    /// session.
    pub(super) fn focus_teammate_pane(&self, pane_id: &str, backend_type: Option<&str>) {
        let pane_id = pane_id.trim();
        if pane_id.is_empty() {
            return;
        }

        if matches!(backend_type, Some("iterm") | Some("iterm2")) {
            let mut command = Command::new("it2");
            command.stdin(Stdio::null());
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
            command.args(["session", "focus", "-s", pane_id]);
            if let Err(err) = command.status() {
                tracing::warn!(error = %err, pane_id, "failed to focus teammate iTerm pane");
            }
            return;
        }

        let mut command = Command::new("tmux");
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        if inside_tmux() {
            command.args(["select-pane", "-t", pane_id]);
        } else {
            command.args(["-L", &swarm_socket_name(), "select-pane", "-t", pane_id]);
        }
        if let Err(err) = command.status() {
            tracing::warn!(error = %err, pane_id, "failed to focus teammate tmux pane");
        }
    }

    /// Toggle a teammate pane's persisted visibility (port of Claude's
    /// `toggleTeammateVisibility` on-disk half, §B.5).
    ///
    /// The on-disk truth is the `hidden_pane_ids` array (Claude's
    /// `addHiddenPaneId` / `removeHiddenPaneId`); we mutate it under the team
    /// store lock, then issue a best-effort tmux move that never errors the
    /// session. The real tmux break/join is gated out of Claude's external
    /// build, so a `select-pane` (focus on show) is the faithful minimum here.
    pub(super) fn set_teammate_pane_hidden(&self, team: &str, pane_id: &str, hide: bool) {
        let pane_id = pane_id.trim();
        if pane_id.is_empty() {
            return;
        }
        let teams_root =
            crate::legacy_core::team_store::root_from_env_or(self.config.codex_home.as_path());
        let id = pane_id.to_string();
        if let Err(err) = crate::legacy_core::team_store::update_config(&teams_root, team, |c| {
            if hide {
                if !c.hidden_pane_ids.contains(&id) {
                    c.hidden_pane_ids.push(id.clone());
                }
            } else {
                c.hidden_pane_ids.retain(|p| p != &id);
            }
        }) {
            tracing::warn!(error = %err, team, pane_id, "failed to persist teammate pane visibility");
        }

        // Best-effort tmux side effect: bringing a shown pane back into focus.
        // Hiding is purely on-disk in the external build, so there is no tmux
        // command to faithfully mirror; never propagate a tmux error.
        if !hide {
            self.focus_teammate_pane(pane_id, /*backend_type*/ None);
        }
    }

    pub(super) fn set_all_teammate_panes_hidden(&self, team: &str, hide: bool) {
        let teams_root =
            crate::legacy_core::team_store::root_from_env_or(self.config.codex_home.as_path());
        if let Err(err) = crate::legacy_core::team_store::update_config(&teams_root, team, |c| {
            if hide {
                c.hidden_pane_ids = c
                    .members
                    .iter()
                    .map(|member| member.tmux_pane_id.trim())
                    .filter(|pane_id| !pane_id.is_empty())
                    .map(ToString::to_string)
                    .collect();
            } else {
                c.hidden_pane_ids.clear();
            }
        }) {
            tracing::warn!(error = %err, team, "failed to persist all teammate pane visibility");
        }
    }

    pub(super) fn send_teammate_shutdown_request(&self, team: &str, teammate_name: &str) {
        let request_id = format!("shutdown-{teammate_name}-{}", unix_millis());
        let teams_root =
            crate::legacy_core::team_store::root_from_env_or(self.config.codex_home.as_path());
        if let Err(err) = crate::legacy_core::team_coord::send_shutdown_request(
            &teams_root,
            team,
            teammate_name,
            crate::legacy_core::team_store::TEAM_LEAD_NAME,
            &request_id,
            Some("Graceful shutdown requested by team lead".to_string()),
            /*color*/ None,
        ) {
            tracing::warn!(error = %err, team, teammate_name, "failed to request teammate shutdown");
        }
    }

    pub(super) fn kill_teammate_pane_and_remove_member(
        &self,
        team: &str,
        pane_id: &str,
        backend_type: Option<&str>,
        agent_id: &str,
    ) {
        self.kill_teammate_pane(pane_id, backend_type);
        let pane_id = pane_id.trim().to_string();
        let agent_id = agent_id.to_string();
        let teams_root =
            crate::legacy_core::team_store::root_from_env_or(self.config.codex_home.as_path());
        if let Err(err) = crate::legacy_core::team_store::update_config(&teams_root, team, |c| {
            c.members.retain(|member| {
                member.tmux_pane_id.trim() != pane_id && member.agent_id != agent_id
            });
            c.hidden_pane_ids.retain(|hidden| hidden != &pane_id);
        }) {
            tracing::warn!(error = %err, team, pane_id, "failed to remove killed teammate from config");
        }
    }

    fn kill_teammate_pane(&self, pane_id: &str, backend_type: Option<&str>) {
        let pane_id = pane_id.trim();
        if pane_id.is_empty() {
            return;
        }
        if matches!(backend_type, Some("iterm") | Some("iterm2")) {
            let mut command = Command::new("it2");
            command.stdin(Stdio::null());
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
            command.args(["session", "close", "-f", "-s", pane_id]);
            if let Err(err) = command.status() {
                tracing::warn!(error = %err, pane_id, "failed to kill teammate iTerm pane");
            }
            return;
        }

        let mut command = Command::new("tmux");
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        if inside_tmux() {
            command.args(["kill-pane", "-t", pane_id]);
        } else {
            command.args(["-L", &swarm_socket_name(), "kill-pane", "-t", pane_id]);
        }
        if let Err(err) = command.status() {
            tracing::warn!(error = %err, pane_id, "failed to kill teammate tmux pane");
        }
    }
}

/// tmux `-L` socket name for the detached swarm server, analog of Claude's
/// `getSwarmSocketName()` (`claude-swarm-<pid>`). Codex uses `codex-swarm`.
fn swarm_socket_name() -> String {
    format!("codex-swarm-{}", std::process::id())
}

fn inside_tmux() -> bool {
    matches!(terminal_info().multiplexer, Some(Multiplexer::Tmux { .. }))
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}
