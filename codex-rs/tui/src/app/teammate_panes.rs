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
        let codex_home = self.config.codex_home.to_path_buf();
        let id = pane_id.to_string();
        if let Err(err) = crate::legacy_core::team_store::update_config(&codex_home, team, |c| {
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
}

/// tmux `-L` socket name for the detached swarm server, analog of Claude's
/// `getSwarmSocketName()` (`claude-swarm-<pid>`). Codex uses `codex-swarm`.
fn swarm_socket_name() -> String {
    format!("codex-swarm-{}", std::process::id())
}

fn inside_tmux() -> bool {
    matches!(terminal_info().multiplexer, Some(Multiplexer::Tmux { .. }))
}
