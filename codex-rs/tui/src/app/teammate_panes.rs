//! Best-effort external teammate panes.
//!
//! Codex teammates run as in-process agent threads, so they cannot be re-attached
//! with `codex resume` (that errors on an already-running thread). When the TUI is
//! itself running inside tmux we instead open a detached split that tails the
//! teammate's rollout transcript, giving a live, read-only window beside the lead
//! session — the closest honest analogue to Claude Code's per-teammate panes.
//!
//! Everything here is best-effort: a missing multiplexer, a not-yet-written rollout,
//! or any tmux quirk is logged and ignored so it can never disrupt the lead session.
//! Set `CODEX_TEAMS_NO_TMUX_PANES=1` to opt out entirely.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::SystemTime;

use codex_terminal_detection::Multiplexer;
use codex_terminal_detection::terminal_info;

use super::App;

impl App {
    /// Open a detached tmux split tailing `agent_thread_id`'s rollout transcript.
    ///
    /// No-op (with a trace log) when not inside tmux, when opted out, when the
    /// teammate already has a pane, or when the rollout file cannot be located yet.
    pub(super) fn open_teammate_tmux_pane(&self, member_name: &str, agent_thread_id: &str) {
        if std::env::var_os("CODEX_TEAMS_NO_TMUX_PANES").is_some() {
            return;
        }
        let agent_thread_id = agent_thread_id.trim();
        if agent_thread_id.is_empty() || !inside_tmux() {
            return;
        }

        let marker = format!("codex-teammate:{agent_thread_id}");
        if teammate_pane_exists(&marker) {
            return;
        }

        let sessions_dir = self
            .config
            .codex_home
            .to_path_buf()
            .join(codex_rollout::SESSIONS_SUBDIR);
        let Some(rollout_path) = find_rollout_for_thread(&sessions_dir, agent_thread_id) else {
            tracing::info!(
                agent_thread_id,
                "teammate rollout not found yet; skipping tmux pane"
            );
            return;
        };

        if let Err(err) = spawn_tail_pane(member_name, &marker, &rollout_path) {
            tracing::warn!(error = %err, "failed to open teammate tmux pane");
        }
    }

    /// Focus a teammate's pane (port of Claude's `viewTeammateOutput`, §B.5).
    ///
    /// iTerm2 panes route through the iTerm backend (not yet wired — see the
    /// Phase 6-iTerm follow-up); every other backend uses tmux: `select-pane`
    /// on the ambient server when the TUI is inside tmux, otherwise on the
    /// detached swarm socket `codex-swarm-<pid>`. Best-effort: any failure is
    /// logged and swallowed so it can never disrupt the lead session.
    pub(super) fn focus_teammate_pane(&self, pane_id: &str, backend_type: Option<&str>) {
        let pane_id = pane_id.trim();
        if pane_id.is_empty() {
            return;
        }

        if matches!(backend_type, Some("iterm") | Some("iterm2")) {
            // The iTerm backend (`team_backends/iterm.rs`) is not yet written;
            // until it lands there is no command to focus an iTerm session, so
            // this is a logged no-op rather than a tmux call against a pane id
            // that is not a tmux pane.
            tracing::info!(
                pane_id,
                "iTerm teammate pane focus is not yet supported; skipping"
            );
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

/// Returns true when any tmux pane is already titled with `marker`, so a teammate
/// is never given a second pane (e.g. if the spawn output is observed twice).
fn teammate_pane_exists(marker: &str) -> bool {
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{pane_title}"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|title| title == marker),
        _ => false,
    }
}

/// Find the newest `rollout-<ts>-<thread_id>.jsonl` under `sessions_dir`.
///
/// Rollouts are stored in a shallow date-partitioned tree (`sessions/YYYY/MM/DD`);
/// the walk is depth-bounded so a surprising layout cannot turn into a deep scan.
fn find_rollout_for_thread(sessions_dir: &Path, thread_id: &str) -> Option<PathBuf> {
    let needle = format!("-{thread_id}.jsonl");
    let mut best: Option<(SystemTime, PathBuf)> = None;
    let mut stack = vec![(sessions_dir.to_path_buf(), 0u32)];

    while let Some((dir, depth)) = stack.pop() {
        if depth > 6 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                stack.push((path, depth + 1));
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with("rollout-") || !name.ends_with(&needle) {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if best.as_ref().is_none_or(|(best, _)| modified >= *best) {
                best = Some((modified, path));
            }
        }
    }

    best.map(|(_, path)| path)
}

/// Spawn the detached tmux split. `-d` keeps focus on the lead pane; `-h` places
/// the teammate window to the side. The pane is titled with `marker` for dedup.
fn spawn_tail_pane(member_name: &str, marker: &str, rollout_path: &Path) -> std::io::Result<()> {
    let path_display = rollout_path.to_string_lossy();
    let banner = format!("=== Codex teammate: {member_name} — live rollout (read-only) ===");
    // Print a banner, then follow the rollout. Raw rollout JSONL: a teammate is an
    // in-process thread, so this is an observation pane, not an interactive attach.
    let inner = format!(
        "printf '%s\\n' {banner}; exec tail -n 50 -F {path}",
        banner = shell_single_quote(&banner),
        path = shell_single_quote(&path_display),
    );

    let output = Command::new("tmux")
        .args([
            "split-window",
            "-d",
            "-h",
            "-P",
            "-F",
            "#{pane_id}",
            "sh",
            "-c",
            &inner,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;

    if output.status.success() {
        let pane_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !pane_id.is_empty() {
            let _ = Command::new("tmux")
                .args(["select-pane", "-t", &pane_id, "-T", marker])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    Ok(())
}

/// Wrap `value` in single quotes for safe inclusion in a `sh -c` string, escaping
/// embedded single quotes (teammate names are model-controlled, so never trusted).
fn shell_single_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}
