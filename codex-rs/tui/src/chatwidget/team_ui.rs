//! Lightweight Teams UI state derived from team tool calls.
//!
//! The core Teams runtime is model/tool driven. Until app-server exposes a dedicated
//! Teams notification stream, the TUI can still make Teams visible by observing the raw
//! team tool call/output items that already flow through the app-server event stream.

use std::collections::HashMap;

use codex_protocol::ThreadId;
use codex_protocol::models::ResponseItem;
use serde_json::Value;

use super::ChatWidget;
use crate::app_event::AppEvent;

#[derive(Default)]
pub(super) struct TeamUiState {
    pending_calls: HashMap<String, PendingTeamCall>,
    active_team: Option<TeamUiSummary>,
    routed_message_count: usize,
}

#[derive(Clone, Debug)]
struct PendingTeamCall {
    tool: TeamToolKind,
    team_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TeamToolKind {
    CreateTeam,
    ListTeams,
    TeamStatus,
    TeamSpawnMember,
    TeamSend,
    TeamMemberStop,
    TeamStop,
}

#[derive(Clone, Debug, Default)]
struct TeamUiSummary {
    id: String,
    name: String,
    status: String,
    members: Vec<TeamMemberUiSummary>,
}

#[derive(Clone, Debug)]
pub(super) struct TeamMemberUiSummary {
    id: String,
    name: String,
    agent_thread_id: String,
    /// Agent/profile label for thread navigation, for example `researcher`.
    agent_role: Option<String>,
}

#[derive(Debug)]
pub(super) enum TeamUiEvent {
    TeamCreated {
        name: String,
    },
    MemberSpawned {
        name: String,
        agent_thread_id: String,
        agent_role: Option<String>,
        tmux_pane_id: String,
        backend_type: Option<String>,
    },
    MessageSent,
    TeamStopped {
        name: String,
    },
    Updated,
}

impl TeamUiState {
    pub(super) fn observe_response_item(&mut self, item: &ResponseItem) -> Option<TeamUiEvent> {
        match item {
            ResponseItem::FunctionCall {
                name,
                namespace,
                arguments,
                call_id,
                ..
            } => {
                if namespace.is_some() {
                    return None;
                }
                let tool = TeamToolKind::from_name(name)?;
                let team_id = parse_argument_string(arguments, "team_id");
                self.pending_calls
                    .insert(call_id.clone(), PendingTeamCall { tool, team_id });
                None
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                let pending = self.pending_calls.remove(call_id)?;
                let text = output.text_content()?;
                let value = serde_json::from_str::<Value>(text).ok()?;
                self.apply_team_tool_output(pending, &value)
            }
            _ => None,
        }
    }

    fn apply_team_tool_output(
        &mut self,
        pending: PendingTeamCall,
        value: &Value,
    ) -> Option<TeamUiEvent> {
        match pending.tool {
            TeamToolKind::CreateTeam => {
                let team = parse_team(value.get("team")?)?;
                let name = team.name.clone();
                self.active_team = Some(team);
                Some(TeamUiEvent::TeamCreated { name })
            }
            TeamToolKind::ListTeams => {
                let teams = value.get("teams")?.as_array()?;
                let team = teams.iter().filter_map(parse_team).next_back()?;
                self.active_team = Some(team);
                Some(TeamUiEvent::Updated)
            }
            TeamToolKind::TeamStatus => {
                let team = parse_team(value.get("snapshot")?.get("team")?)?;
                self.active_team = Some(team);
                Some(TeamUiEvent::Updated)
            }
            TeamToolKind::TeamSpawnMember => {
                let tmux_pane_id = value
                    .get("tmux_pane_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|pane_id| !pane_id.is_empty())
                    .map(ToString::to_string)?;
                let backend_type = value
                    .get("backend_type")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|backend_type| !backend_type.is_empty())
                    .map(ToString::to_string);
                let member = parse_member(value.get("member")?)?;
                let name = member.name.clone();
                let agent_thread_id = member.agent_thread_id.clone();
                let agent_role = member.agent_role.clone();
                let team = self
                    .active_team
                    .as_mut()
                    .filter(|team| pending.team_id.as_deref() == Some(team.id.as_str()))?;
                upsert_member(&mut team.members, member);
                Some(TeamUiEvent::MemberSpawned {
                    name,
                    agent_thread_id,
                    agent_role,
                    tmux_pane_id,
                    backend_type,
                })
            }
            TeamToolKind::TeamSend => {
                self.routed_message_count = self.routed_message_count.saturating_add(1);
                Some(TeamUiEvent::MessageSent)
            }
            TeamToolKind::TeamMemberStop | TeamToolKind::TeamStop => {
                let team = parse_team(value.get("snapshot")?.get("team")?)?;
                let stopped =
                    matches!(pending.tool, TeamToolKind::TeamStop) || team.status == "stopped";
                let name = team.name.clone();
                self.active_team = Some(team);
                if stopped {
                    Some(TeamUiEvent::TeamStopped { name })
                } else {
                    Some(TeamUiEvent::Updated)
                }
            }
        }
    }

    /// Name of the active (non-stopped) team, for opening the Teams dialog.
    pub(super) fn active_team_name(&self) -> Option<String> {
        self.active_team
            .as_ref()
            .filter(|team| team.status != "stopped")
            .map(|team| team.name.clone())
    }
}

impl ChatWidget {
    /// Name of the active (non-stopped) Codex team, if any. Used by `App` to
    /// open the Teams dialog (Phase 6 §B.6).
    pub(crate) fn active_team_name(&self) -> Option<String> {
        self.team_ui.active_team_name()
    }

    pub(super) fn handle_raw_response_item_for_team_ui(
        &mut self,
        item: ResponseItem,
        from_replay: bool,
    ) {
        // Gate: the observer is a strict no-op for any item that is not a team
        // tool call/output (see `observe_response_item`, which returns `None` for
        // every non-team `FunctionCall`/`FunctionCallOutput`). A session that is
        // not running a team — including one running native subagents — therefore
        // renders and behaves exactly like upstream, because it never receives
        // team-shaped tool output to begin with.
        let Some(event) = self.team_ui.observe_response_item(&item) else {
            return;
        };
        // The observer updates active team metadata and emits live side effects.
        // Footer roster pills are owned by App-level Teams navigation state so
        // generic subagents and raw tool summaries cannot become roster items.
        // Transcript notices and external panes are live-only side effects: replaying
        // historical tool output must not re-announce teams or re-open teammate panes.
        if from_replay {
            return;
        }
        match event {
            TeamUiEvent::TeamCreated { name } => {
                self.add_info_message(
                    format!("Codex team started: {name}"),
                    Some(
                        "Spawn teammates to make them appear in the footer and teammate picker."
                            .to_string(),
                    ),
                );
                self.app_event_tx
                    .send(AppEvent::TeamBecameActive { team: name });
            }
            TeamUiEvent::MemberSpawned {
                name,
                agent_thread_id,
                agent_role,
                tmux_pane_id,
                backend_type,
            } => {
                self.add_info_message(
                    format!("Teammate @{name} started"),
                    Some(format!(
                        "Agent thread {agent_thread_id}; use Shift+Up/Down then Enter to switch, Esc to return to lead, or @{name} to message them."
                    )),
                );
                match ThreadId::from_string(&agent_thread_id) {
                    Ok(agent_thread_id) => {
                        self.app_event_tx.send(AppEvent::RegisterTeammateThread {
                            member_name: name,
                            agent_thread_id,
                            agent_role,
                            tmux_pane_id: Some(tmux_pane_id),
                            backend_type,
                        });
                    }
                    Err(err) => {
                        tracing::warn!(
                            agent_thread_id,
                            error = %err,
                            "ignoring teammate with invalid thread id during agent navigation registration"
                        );
                    }
                }
            }
            TeamUiEvent::MessageSent => {
                self.add_info_message(
                    "Teams message routed".to_string(),
                    Some("The Teams roster is updated from teammate spawn events.".to_string()),
                );
            }
            TeamUiEvent::TeamStopped { name } => {
                self.add_info_message(format!("Codex team stopped: {name}"), None);
                self.app_event_tx.send(AppEvent::TeamBecameInactive);
            }
            TeamUiEvent::Updated => {}
        }
    }

    pub(crate) fn set_team_footer_context(
        &mut self,
        team_label: Option<String>,
        team_spans: Option<Vec<ratatui::text::Span<'static>>>,
    ) {
        self.team_footer_label = team_label;
        self.team_footer_spans = team_spans;
        self.sync_footer_context_label();
    }

    pub(super) fn sync_footer_context_label(&mut self) {
        let mut labels = Vec::new();
        if let Some(active_agent_label) = self.active_agent_label.as_ref() {
            labels.push(active_agent_label.clone());
        }
        if let Some(team_label) = self.team_footer_label.as_ref() {
            labels.push(team_label.clone());
        }
        let combined = (!labels.is_empty()).then(|| labels.join(" · "));
        self.bottom_pane.set_active_agent_label(combined);
        // Colored teammate pills render alongside the plain label (Phase 6 §B.6);
        // the plain `footer_label` String path above is left untouched so the
        // textual footer keeps working when spans are unavailable.
        self.bottom_pane
            .set_active_team_pills(self.team_footer_spans.clone());
    }
}

impl TeamToolKind {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "create_team" => Some(Self::CreateTeam),
            "list_teams" => Some(Self::ListTeams),
            "team_status" => Some(Self::TeamStatus),
            "team_spawn_member" => Some(Self::TeamSpawnMember),
            "team_send" => Some(Self::TeamSend),
            "team_member_stop" => Some(Self::TeamMemberStop),
            "team_stop" => Some(Self::TeamStop),
            _ => None,
        }
    }
}

fn parse_team(value: &Value) -> Option<TeamUiSummary> {
    let members = value
        .get("members")
        .and_then(Value::as_array)
        .map(|members| members.iter().filter_map(parse_member).collect())
        .unwrap_or_default();

    Some(TeamUiSummary {
        id: value.get("id")?.as_str()?.to_string(),
        name: value.get("name")?.as_str()?.to_string(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("active")
            .to_string(),
        members,
    })
}

fn parse_member(value: &Value) -> Option<TeamMemberUiSummary> {
    Some(TeamMemberUiSummary {
        id: value.get("id")?.as_str()?.to_string(),
        name: value.get("name")?.as_str()?.to_string(),
        agent_thread_id: value.get("agent_thread_id")?.as_str()?.to_string(),
        agent_role: value
            .get("profile")
            .or_else(|| value.get("agent_type"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|role| !role.is_empty())
            .map(ToString::to_string),
    })
}

fn parse_argument_string(arguments: &str, key: &str) -> Option<String> {
    serde_json::from_str::<Value>(arguments)
        .ok()?
        .get(key)?
        .as_str()
        .map(ToString::to_string)
}

fn upsert_member(members: &mut Vec<TeamMemberUiSummary>, member: TeamMemberUiSummary) {
    if let Some(existing) = members.iter_mut().find(|existing| existing.id == member.id) {
        *existing = member;
    } else {
        members.push(member);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::FunctionCallOutputPayload;

    fn call(name: &str, call_id: &str, arguments: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.to_string(),
            namespace: None,
            arguments: arguments.to_string(),
            call_id: call_id.to_string(),
        }
    }

    fn output(call_id: &str, json: &str) -> ResponseItem {
        ResponseItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: FunctionCallOutputPayload::from_text(json.to_string()),
        }
    }

    #[test]
    fn active_team_metadata_tracks_team_tool_output() {
        let mut state = TeamUiState::default();
        assert_eq!(state.active_team_name(), None);

        state.observe_response_item(&call("create_team", "c1", "{}"));
        let created = state.observe_response_item(&output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ));
        assert!(matches!(created, Some(TeamUiEvent::TeamCreated { .. })));
        assert_eq!(state.active_team_name(), Some("Rocket".to_string()));

        state.observe_response_item(&call("team_spawn_member", "c2", r#"{"team_id":"team-1"}"#));
        let spawned = state.observe_response_item(&output(
            "c2",
            r#"{"member":{"id":"m1","name":"alice","agent_thread_id":"thr-1","profile":"researcher","status":"active","agent_status":"running"},"tmux_pane_id":"%9","backend_type":"tmux"}"#,
        ));
        assert!(matches!(
            spawned,
            Some(TeamUiEvent::MemberSpawned {
                name,
                agent_thread_id,
                agent_role,
                tmux_pane_id,
                backend_type,
            }) if name == "alice"
                && agent_thread_id == "thr-1"
                && agent_role.as_deref() == Some("researcher")
                && tmux_pane_id == "%9"
                && backend_type.as_deref() == Some("tmux")
        ));
        assert_eq!(
            state.active_team.as_ref().map(|team| team.members.len()),
            Some(1)
        );
        assert_eq!(state.active_team_name(), Some("Rocket".to_string()));

        state.observe_response_item(&call("team_stop", "c3", r#"{"team_id":"team-1"}"#));
        state.observe_response_item(&output(
            "c3",
            r#"{"snapshot":{"team":{"id":"team-1","name":"Rocket","status":"stopped","members":[]}}}"#,
        ));
        assert_eq!(state.active_team_name(), None);
    }

    #[test]
    fn namespaced_team_shaped_tool_output_is_ignored() {
        let mut state = TeamUiState::default();

        state.observe_response_item(&ResponseItem::FunctionCall {
            id: None,
            name: "create_team".to_string(),
            namespace: Some("mcp".to_string()),
            arguments: "{}".to_string(),
            call_id: "c1".to_string(),
        });
        let created = state.observe_response_item(&output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ));

        assert!(created.is_none());
        assert_eq!(state.active_team_name(), None);
    }

    #[test]
    fn team_spawn_member_without_pane_metadata_is_ignored() {
        let mut state = TeamUiState::default();

        state.observe_response_item(&call("create_team", "c1", "{}"));
        let _ = state.observe_response_item(&output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ));
        state.observe_response_item(&call("team_spawn_member", "c2", r#"{"team_id":"team-1"}"#));
        let spawned = state.observe_response_item(&output(
            "c2",
            r#"{"member":{"id":"m1","name":"alice","agent_thread_id":"thr-1","profile":"researcher","status":"active","agent_status":"running"}}"#,
        ));

        assert!(spawned.is_none());
        assert_eq!(
            state.active_team.as_ref().map(|team| team.members.len()),
            Some(0)
        );
        assert_eq!(state.active_team_name(), Some("Rocket".to_string()));
    }
}
