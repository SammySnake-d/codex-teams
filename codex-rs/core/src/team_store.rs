//! On-disk team store + file mailbox (Phase 1 of the Claude Code teams port).
//!
//! Teammates are independent `codex` processes that coordinate via shared on-disk
//! state, mirroring Claude Code's mechanism (see `TEAMS_CLAUDE_PORT_SPEC.md`). This
//! module owns the persistent layout under a teams root (`$CODEX_HOME`):
//! ```text
//!   teams/{team}/config.json
//!   teams/{team}/inboxes/{agent}.json
//!   tasks/{team}/
//! ```
//! It is intentionally self-contained (std + serde only) and not yet wired into the
//! in-process team runtime; later phases replace `spawn_member` with a process spawn
//! and route `team_send` through these mailboxes.
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

/// Reserved agent name for the team lead (mirrors Claude's `TEAM_LEAD_NAME`).
pub const TEAM_LEAD_NAME: &str = "team-lead";

/// `teams/{team}/config.json` — mirrors Claude's `TeamFile` (camelCase on disk).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamFile {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: i64,
    pub lead_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_id: Option<String>,
    #[serde(default)]
    pub hidden_pane_ids: Vec<String>,
    #[serde(default)]
    pub members: Vec<TeamFileMember>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamFileMember {
    pub agent_id: String,
    pub name: String,
    /// Registry-style `ThreadId` (string) returned to the model when this member
    /// was spawned. Split-pane PROCESS members live only on disk (never in the
    /// in-memory registry), so the lead resolves a `team_send` `member_id` back
    /// to a mailbox recipient through this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode_required: Option<bool>,
    pub joined_at: i64,
    #[serde(default)]
    pub tmux_pane_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub subscriptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// One entry in `inboxes/{agent}.json` — mirrors Claude's `TeammateMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeammateMessage {
    pub from: String,
    pub text: String,
    pub timestamp: String,
    pub read: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Sanitize a team/agent name for use as a path segment (mirrors Claude `getInboxPath`).
pub fn sanitize(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

pub fn team_dir(teams_root: &Path, team: &str) -> PathBuf {
    teams_root.join("teams").join(sanitize(team))
}
pub fn config_path(teams_root: &Path, team: &str) -> PathBuf {
    team_dir(teams_root, team).join("config.json")
}
pub fn inboxes_dir(teams_root: &Path, team: &str) -> PathBuf {
    team_dir(teams_root, team).join("inboxes")
}
pub fn inbox_path(teams_root: &Path, team: &str, agent: &str) -> PathBuf {
    inboxes_dir(teams_root, team).join(format!("{}.json", sanitize(agent)))
}
pub fn tasks_dir(teams_root: &Path, team: &str) -> PathBuf {
    teams_root.join("tasks").join(sanitize(team))
}

/// `agentId` is `"{name}@{team}"` (mirrors Claude's identity scheme).
pub fn agent_id(name: &str, team: &str) -> String {
    format!("{name}@{team}")
}

/// Best-effort timestamp (seconds since epoch, fractional). Claude uses ISO-8601;
/// this stays dep-free until a later phase can adopt RFC-3339.
pub fn now_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    format!("{secs:.3}")
}

/// Dep-free advisory lock via atomic create of a sibling `<stem>.lock` file.
struct FileLock {
    path: PathBuf,
}
impl FileLock {
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
        // Stale lock fallback: take it over so a crashed holder cannot wedge the team.
        let _ = fs::remove_file(&path);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self { path })
    }
}
impl Drop for FileLock {
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

fn read_messages(path: &Path) -> io::Result<Vec<TeammateMessage>> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Append a message to `to_agent`'s inbox with `read: false` (mirrors `writeToMailbox`).
pub fn write_to_mailbox(
    teams_root: &Path,
    team: &str,
    to_agent: &str,
    msg: TeammateMessage,
) -> io::Result<()> {
    let path = inbox_path(teams_root, team, to_agent);
    let _lock = FileLock::acquire(&path)?;
    let mut msgs = read_messages(&path)?;
    msgs.push(msg);
    let bytes = serde_json::to_vec_pretty(&msgs).map_err(io::Error::other)?;
    write_atomic(&path, &bytes)
}

pub fn read_mailbox(
    teams_root: &Path,
    team: &str,
    agent: &str,
) -> io::Result<Vec<TeammateMessage>> {
    read_messages(&inbox_path(teams_root, team, agent))
}

/// Mirrors `readUnreadMessages` — messages with `read == false`.
pub fn read_unread(teams_root: &Path, team: &str, agent: &str) -> io::Result<Vec<TeammateMessage>> {
    Ok(read_mailbox(teams_root, team, agent)?
        .into_iter()
        .filter(|m| !m.read)
        .collect())
}

pub fn mark_message_read_by_index(
    teams_root: &Path,
    team: &str,
    agent: &str,
    index: usize,
) -> io::Result<()> {
    let path = inbox_path(teams_root, team, agent);
    let _lock = FileLock::acquire(&path)?;
    let mut msgs = read_messages(&path)?;
    if let Some(message) = msgs.get_mut(index) {
        message.read = true;
    }
    let bytes = serde_json::to_vec_pretty(&msgs).map_err(io::Error::other)?;
    write_atomic(&path, &bytes)
}

pub fn mark_messages_read(teams_root: &Path, team: &str, agent: &str) -> io::Result<()> {
    let path = inbox_path(teams_root, team, agent);
    let _lock = FileLock::acquire(&path)?;
    let mut msgs = read_messages(&path)?;
    for message in &mut msgs {
        message.read = true;
    }
    let bytes = serde_json::to_vec_pretty(&msgs).map_err(io::Error::other)?;
    write_atomic(&path, &bytes)
}

pub fn read_config(teams_root: &Path, team: &str) -> io::Result<Option<TeamFile>> {
    match fs::read(config_path(teams_root, team)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).map_err(io::Error::other)?,
        )),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn write_config(teams_root: &Path, team: &str, config: &TeamFile) -> io::Result<()> {
    let path = config_path(teams_root, team);
    let _lock = FileLock::acquire(&path)?;
    let bytes = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
    write_atomic(&path, &bytes)
}

/// Read-modify-write the team config under a lock (create default if missing).
pub fn update_config<F: FnOnce(&mut TeamFile)>(
    teams_root: &Path,
    team: &str,
    update: F,
) -> io::Result<()> {
    let path = config_path(teams_root, team);
    let _lock = FileLock::acquire(&path)?;
    let mut config = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
        Err(e) if e.kind() == ErrorKind::NotFound => TeamFile::default(),
        Err(e) => return Err(e),
    };
    update(&mut config);
    let bytes = serde_json::to_vec_pretty(&config).map_err(io::Error::other)?;
    write_atomic(&path, &bytes)
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
        temp_dir().join(format!("codex-team-store-{nanos}"))
    }

    #[test]
    fn mailbox_round_trip_and_mark_read() {
        let root = unique_root();
        let team = "Rocket Team";
        let agent = "alice";
        assert!(read_unread(&root, team, agent).unwrap().is_empty());

        write_to_mailbox(
            &root,
            team,
            agent,
            TeammateMessage {
                from: TEAM_LEAD_NAME.to_string(),
                text: "ship it".to_string(),
                timestamp: now_timestamp(),
                read: false,
                color: None,
                summary: None,
            },
        )
        .unwrap();
        write_to_mailbox(
            &root,
            team,
            agent,
            TeammateMessage {
                from: "bob".to_string(),
                text: "hi".to_string(),
                timestamp: now_timestamp(),
                read: false,
                color: Some("green".to_string()),
                summary: None,
            },
        )
        .unwrap();

        assert_eq!(read_unread(&root, team, agent).unwrap().len(), 2);
        mark_message_read_by_index(&root, team, agent, 0).unwrap();
        let unread = read_unread(&root, team, agent).unwrap();
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].from, "bob");
        assert!(
            inbox_path(&root, team, agent)
                .to_string_lossy()
                .contains("Rocket_Team")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn config_round_trip_is_claude_camel_case() {
        let root = unique_root();
        let team = "t1";
        update_config(&root, team, |config| {
            config.name = "t1".to_string();
            config.lead_agent_id = agent_id(TEAM_LEAD_NAME, team);
            config.members.push(TeamFileMember {
                agent_id: agent_id("alice", team),
                name: "alice".to_string(),
                tmux_pane_id: "%1".to_string(),
                is_active: Some(true),
                ..Default::default()
            });
        })
        .unwrap();

        let config = read_config(&root, team).unwrap().unwrap();
        assert_eq!(config.lead_agent_id, "team-lead@t1");
        assert_eq!(config.members.len(), 1);

        let json = String::from_utf8(fs::read(config_path(&root, team)).unwrap()).unwrap();
        assert!(json.contains("leadAgentId"), "json: {json}");
        assert!(json.contains("tmuxPaneId"), "json: {json}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sanitize_strips_unsafe_chars() {
        assert_eq!(sanitize("a/b c.d"), "a_b_c_d");
        assert_eq!(sanitize(""), "_");
        assert_eq!(sanitize("team-lead_1"), "team-lead_1");
    }
}
