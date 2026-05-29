use crate::agent::AgentControl;
use crate::agent::AgentStatus;
use crate::config::Config;
use crate::error::CodexErr;
use crate::error::Result as CodexResult;
use codex_protocol::ThreadId;
use codex_protocol::protocol::SessionSource;
use codex_protocol::user_input::UserInput;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamStatus {
    Active,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamMemberStatus {
    Active,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamMessageDeliveryStatus {
    Submitted,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamMessageDeliveryMode {
    Queue,
    Interrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamTaskStatus {
    Open,
    Claimed,
    Completed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Team {
    pub(crate) id: ThreadId,
    pub(crate) name: String,
    pub(crate) lead_thread_id: ThreadId,
    pub(crate) status: TeamStatus,
    pub(crate) members: Vec<TeamMember>,
    pub(crate) tasks: Vec<TeamTask>,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
    pub(crate) live_session_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct TeamMember {
    pub(crate) id: ThreadId,
    pub(crate) name: String,
    pub(crate) agent_thread_id: ThreadId,
    pub(crate) profile: Option<String>,
    pub(crate) capabilities: Vec<String>,
    pub(crate) permissions: Vec<String>,
    pub(crate) status: TeamMemberStatus,
    pub(crate) agent_status: AgentStatus,
    pub(crate) created_at: i64,
    pub(crate) last_activity_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct TeamMessage {
    pub(crate) id: ThreadId,
    pub(crate) team_id: ThreadId,
    pub(crate) sender: TeamMessageEndpoint,
    pub(crate) target_member_id: ThreadId,
    pub(crate) content: String,
    pub(crate) submitted_id: Option<String>,
    pub(crate) delivery_mode: TeamMessageDeliveryMode,
    pub(crate) delivery_status: TeamMessageDeliveryStatus,
    pub(crate) created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamMessageEndpoint {
    Lead(ThreadId),
    Member(ThreadId),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct TeamTask {
    pub(crate) id: ThreadId,
    pub(crate) team_id: ThreadId,
    pub(crate) title: String,
    pub(crate) assignee_member_id: Option<ThreadId>,
    pub(crate) dependencies: Vec<ThreadId>,
    pub(crate) status: TeamTaskStatus,
    pub(crate) note: Option<String>,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TeamEvent {
    TeamCreated {
        team_id: ThreadId,
        name: String,
        at: i64,
    },
    MemberSpawned {
        team_id: ThreadId,
        member_id: ThreadId,
        agent_thread_id: ThreadId,
        name: String,
        at: i64,
    },
    MessageSubmitted {
        team_id: ThreadId,
        message_id: ThreadId,
        target_member_id: ThreadId,
        at: i64,
    },
    TaskCreated {
        team_id: ThreadId,
        task_id: ThreadId,
        at: i64,
    },
    TaskUpdated {
        team_id: ThreadId,
        task_id: ThreadId,
        at: i64,
    },
    TeamStopped {
        team_id: ThreadId,
        at: i64,
    },
    Failure {
        team_id: Option<ThreadId>,
        message: String,
        at: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct TeamSnapshot {
    pub(crate) team: Team,
    pub(crate) messages: Vec<TeamMessage>,
    pub(crate) events: Vec<TeamEvent>,
}

pub(crate) struct SpawnTeamMemberRequest {
    pub(crate) team_id: ThreadId,
    pub(crate) name: String,
    pub(crate) profile: Option<String>,
    pub(crate) capabilities: Vec<String>,
    pub(crate) permissions: Vec<String>,
    pub(crate) initial_items: Vec<UserInput>,
    pub(crate) config: Config,
    pub(crate) session_source: Option<SessionSource>,
}

pub(crate) struct SendTeamMessageRequest {
    pub(crate) team_id: ThreadId,
    pub(crate) sender_member_id: Option<ThreadId>,
    pub(crate) member_id: ThreadId,
    pub(crate) content: String,
    pub(crate) delivery_mode: TeamMessageDeliveryMode,
    pub(crate) items: Vec<UserInput>,
}

pub(crate) struct CreateTeamTaskRequest {
    pub(crate) team_id: ThreadId,
    pub(crate) title: String,
    pub(crate) assignee_member_id: Option<ThreadId>,
    pub(crate) dependencies: Vec<ThreadId>,
    pub(crate) note: Option<String>,
}

#[derive(Default)]
pub(crate) struct UpdateTeamTaskRequest {
    pub(crate) title: Option<String>,
    pub(crate) assignee_member_id: Option<ThreadId>,
    pub(crate) dependencies: Option<Vec<ThreadId>>,
    pub(crate) status: Option<TeamTaskStatus>,
    pub(crate) note: Option<String>,
}

#[derive(Default)]
pub(crate) struct TeamRegistry {
    state: RwLock<TeamRegistryState>,
}

#[derive(Default)]
struct TeamRegistryState {
    teams: HashMap<ThreadId, Team>,
    messages: Vec<TeamMessage>,
    events: Vec<TeamEvent>,
}

impl TeamRegistry {
    pub(crate) async fn create_team(&self, name: String, lead_thread_id: ThreadId) -> Team {
        let at = unix_timestamp();
        let team = Team {
            id: ThreadId::new(),
            name,
            lead_thread_id,
            status: TeamStatus::Active,
            members: Vec::new(),
            tasks: Vec::new(),
            created_at: at,
            updated_at: at,
            live_session_only: true,
        };

        let mut state = self.state.write().await;
        state.events.push(TeamEvent::TeamCreated {
            team_id: team.id,
            name: team.name.clone(),
            at,
        });
        state.teams.insert(team.id, team.clone());
        team
    }

    pub(crate) async fn list_teams(&self) -> Vec<Team> {
        let state = self.state.read().await;
        let mut teams = state.teams.values().cloned().collect::<Vec<_>>();
        teams.sort_by_key(|team| team.created_at);
        teams
    }

    pub(crate) async fn team_status(
        &self,
        team_id: ThreadId,
        agent_control: &AgentControl,
    ) -> CodexResult<TeamSnapshot> {
        self.refresh_agent_statuses(team_id, agent_control).await?;
        self.snapshot(team_id).await
    }

    pub(crate) async fn spawn_member(
        &self,
        request: SpawnTeamMemberRequest,
        agent_control: &AgentControl,
    ) -> CodexResult<TeamMember> {
        let SpawnTeamMemberRequest {
            team_id,
            name,
            profile,
            capabilities,
            permissions,
            mut initial_items,
            config,
            session_source,
        } = request;
        self.ensure_team_active(team_id).await?;
        let (team_name, lead_thread_id) = {
            let state = self.state.read().await;
            let team = state
                .teams
                .get(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            (team.name.clone(), team.lead_thread_id)
        };
        let member_id = ThreadId::new();
        let profile_label = profile.as_deref().unwrap_or("none");
        let capabilities_label = if capabilities.is_empty() {
            "none".to_string()
        } else {
            capabilities.join(", ")
        };
        let permissions_label = if permissions.is_empty() {
            "none".to_string()
        } else {
            permissions.join(", ")
        };
        let context = format!(
            "Codex Teams context:\n\
             - team_id: {team_id}\n\
             - team_name: {team_name}\n\
             - lead_thread_id: {lead_thread_id}\n\
             - member_id: {member_id}\n\
             - member_name: {name}\n\
             - profile: {profile_label}\n\
             - capabilities: {capabilities_label}\n\
             - permissions: {permissions_label}\n\
             - live_session_only: true\n\
             \n\
             You are an independent Codex Teams teammate. Do not assume you inherit the lead conversation history.\n\
             Treat the spawn prompt/items after this context as your assigned task boundary.\n\
             Use generic Teams tools when available: team_status, team_send, team_task_list, team_task_update, and team_event_list.\n\
             When sending as this teammate, set sender_member_id to your member_id."
        );
        let mut wrapped_items = Vec::with_capacity(initial_items.len() + 1);
        wrapped_items.push(UserInput::Text {
            text: context,
            text_elements: Vec::new(),
        });
        wrapped_items.append(&mut initial_items);
        let agent_thread_id = agent_control
            .spawn_agent(config, wrapped_items, session_source)
            .await?;
        let at = unix_timestamp();
        let agent_status = agent_control.get_status(agent_thread_id).await;
        let member = TeamMember {
            id: member_id,
            name,
            agent_thread_id,
            profile,
            capabilities,
            permissions,
            status: TeamMemberStatus::Active,
            agent_status,
            created_at: at,
            last_activity_at: at,
        };

        let mut state = self.state.write().await;
        let team = state
            .teams
            .get_mut(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        team.members.push(member.clone());
        team.updated_at = at;
        state.events.push(TeamEvent::MemberSpawned {
            team_id,
            member_id: member.id,
            agent_thread_id,
            name: member.name.clone(),
            at,
        });
        Ok(member)
    }

    pub(crate) async fn send_to_member(
        &self,
        request: SendTeamMessageRequest,
        agent_control: &AgentControl,
    ) -> CodexResult<TeamMessage> {
        let SendTeamMessageRequest {
            team_id,
            sender_member_id,
            member_id,
            content,
            delivery_mode,
            items,
        } = request;
        self.ensure_team_active(team_id).await?;
        let (sender, agent_thread_id) = {
            let state = self.state.read().await;
            let team = state
                .teams
                .get(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            let sender = if let Some(sender_member_id) = sender_member_id {
                if !team
                    .members
                    .iter()
                    .any(|member| member.id == sender_member_id)
                {
                    return Err(CodexErr::ThreadNotFound(sender_member_id));
                }
                TeamMessageEndpoint::Member(sender_member_id)
            } else {
                TeamMessageEndpoint::Lead(team.lead_thread_id)
            };
            let member = team
                .members
                .iter()
                .find(|member| member.id == member_id)
                .ok_or(CodexErr::ThreadNotFound(member_id))?;
            (sender, member.agent_thread_id)
        };
        if delivery_mode == TeamMessageDeliveryMode::Interrupt {
            agent_control.interrupt_agent(agent_thread_id).await?;
        }
        let submission_id = agent_control.send_input(agent_thread_id, items).await?;
        let at = unix_timestamp();
        let message = TeamMessage {
            id: ThreadId::new(),
            team_id,
            sender,
            target_member_id: member_id,
            content,
            submitted_id: Some(submission_id),
            delivery_mode,
            delivery_status: TeamMessageDeliveryStatus::Submitted,
            created_at: at,
        };

        let agent_status = agent_control.get_status(agent_thread_id).await;
        let mut state = self.state.write().await;
        state.messages.push(message.clone());
        if let Some(team) = state.teams.get_mut(&team_id) {
            if let Some(member) = team
                .members
                .iter_mut()
                .find(|member| member.id == member_id)
            {
                member.last_activity_at = at;
                member.agent_status = agent_status;
            }
            team.updated_at = at;
        }
        state.events.push(TeamEvent::MessageSubmitted {
            team_id,
            message_id: message.id,
            target_member_id: member_id,
            at,
        });
        Ok(message)
    }

    pub(crate) async fn record_failure(&self, team_id: Option<ThreadId>, message: String) {
        let at = unix_timestamp();
        let mut state = self.state.write().await;
        if let Some(team_id) = team_id
            && let Some(team) = state.teams.get_mut(&team_id)
        {
            team.updated_at = at;
        }
        state.events.push(TeamEvent::Failure {
            team_id,
            message,
            at,
        });
    }

    pub(crate) async fn list_tasks(&self, team_id: ThreadId) -> CodexResult<Vec<TeamTask>> {
        let state = self.state.read().await;
        let team = state
            .teams
            .get(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        Ok(team.tasks.clone())
    }

    pub(crate) async fn list_events(&self, team_id: ThreadId) -> CodexResult<Vec<TeamEvent>> {
        let state = self.state.read().await;
        if !state.teams.contains_key(&team_id) {
            return Err(CodexErr::ThreadNotFound(team_id));
        }
        Ok(state
            .events
            .iter()
            .filter(|event| event.team_id() == Some(team_id))
            .cloned()
            .collect())
    }

    pub(crate) async fn create_task(
        &self,
        request: CreateTeamTaskRequest,
    ) -> CodexResult<TeamTask> {
        let CreateTeamTaskRequest {
            team_id,
            title,
            assignee_member_id,
            dependencies,
            note,
        } = request;
        self.ensure_team_active(team_id).await?;
        let at = unix_timestamp();
        let task = TeamTask {
            id: ThreadId::new(),
            team_id,
            title,
            assignee_member_id,
            dependencies,
            status: TeamTaskStatus::Open,
            note,
            created_at: at,
            updated_at: at,
        };

        let mut state = self.state.write().await;
        let team = state
            .teams
            .get_mut(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        validate_member(team, task.assignee_member_id)?;
        validate_dependencies(team, &task.dependencies)?;
        team.tasks.push(task.clone());
        team.updated_at = at;
        state.events.push(TeamEvent::TaskCreated {
            team_id,
            task_id: task.id,
            at,
        });
        Ok(task)
    }

    pub(crate) async fn update_task(
        &self,
        team_id: ThreadId,
        task_id: ThreadId,
        request: UpdateTeamTaskRequest,
    ) -> CodexResult<TeamTask> {
        self.ensure_team_active(team_id).await?;
        let at = unix_timestamp();
        let mut state = self.state.write().await;
        let team = state
            .teams
            .get_mut(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        validate_member(team, request.assignee_member_id)?;
        if let Some(dependencies) = &request.dependencies {
            if dependencies.contains(&task_id) {
                return Err(CodexErr::UnsupportedOperation(format!(
                    "task {task_id} can't depend on itself"
                )));
            }
            validate_dependencies(team, dependencies)?;
        }
        let task = team
            .tasks
            .iter_mut()
            .find(|task| task.id == task_id)
            .ok_or(CodexErr::ThreadNotFound(task_id))?;
        if let Some(title) = request.title {
            task.title = title;
        }
        if let Some(assignee_member_id) = request.assignee_member_id {
            task.assignee_member_id = Some(assignee_member_id);
        }
        if let Some(dependencies) = request.dependencies {
            task.dependencies = dependencies;
        }
        if let Some(status) = request.status {
            task.status = status;
        }
        if let Some(note) = request.note {
            task.note = Some(note);
        }
        task.updated_at = at;
        let task = task.clone();
        team.updated_at = at;
        state.events.push(TeamEvent::TaskUpdated {
            team_id,
            task_id,
            at,
        });
        Ok(task)
    }

    pub(crate) async fn stop_team(
        &self,
        team_id: ThreadId,
        agent_control: &AgentControl,
    ) -> CodexResult<TeamSnapshot> {
        let agent_thread_ids = {
            let state = self.state.read().await;
            let team = state
                .teams
                .get(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            team.members
                .iter()
                .filter(|member| member.status == TeamMemberStatus::Active)
                .map(|member| member.agent_thread_id)
                .collect::<Vec<_>>()
        };

        for agent_thread_id in agent_thread_ids {
            let _ = agent_control.shutdown_agent(agent_thread_id).await;
        }

        let at = unix_timestamp();
        let statuses = {
            let state = self.state.read().await;
            let team = state
                .teams
                .get(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            team.members
                .iter()
                .map(|member| {
                    let member_id = member.id;
                    let agent_thread_id = member.agent_thread_id;
                    async move { (member_id, agent_control.get_status(agent_thread_id).await) }
                })
                .collect::<Vec<_>>()
        };
        let mut resolved_statuses = Vec::with_capacity(statuses.len());
        for status in statuses {
            resolved_statuses.push(status.await);
        }

        {
            let mut state = self.state.write().await;
            let team = state
                .teams
                .get_mut(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            team.status = TeamStatus::Stopped;
            team.updated_at = at;
            for member in &mut team.members {
                member.status = TeamMemberStatus::Stopped;
                if let Some((_, status)) = resolved_statuses
                    .iter()
                    .find(|(member_id, _)| *member_id == member.id)
                {
                    member.agent_status = status.clone();
                }
                member.last_activity_at = at;
            }
            state.events.push(TeamEvent::TeamStopped { team_id, at });
        }

        self.snapshot(team_id).await
    }

    async fn ensure_team_active(&self, team_id: ThreadId) -> CodexResult<()> {
        let state = self.state.read().await;
        let team = state
            .teams
            .get(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        if team.status == TeamStatus::Stopped {
            return Err(CodexErr::UnsupportedOperation(format!(
                "team {team_id} is stopped"
            )));
        }
        Ok(())
    }

    async fn refresh_agent_statuses(
        &self,
        team_id: ThreadId,
        agent_control: &AgentControl,
    ) -> CodexResult<()> {
        let members = {
            let state = self.state.read().await;
            let team = state
                .teams
                .get(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            team.members
                .iter()
                .map(|member| (member.id, member.agent_thread_id))
                .collect::<Vec<_>>()
        };

        let mut statuses = Vec::with_capacity(members.len());
        for (member_id, agent_thread_id) in members {
            statuses.push((member_id, agent_control.get_status(agent_thread_id).await));
        }

        let mut state = self.state.write().await;
        if let Some(team) = state.teams.get_mut(&team_id) {
            for (member_id, status) in statuses {
                if let Some(member) = team
                    .members
                    .iter_mut()
                    .find(|member| member.id == member_id)
                {
                    member.agent_status = status;
                }
            }
        }
        Ok(())
    }

    async fn snapshot(&self, team_id: ThreadId) -> CodexResult<TeamSnapshot> {
        let state = self.state.read().await;
        let team = state
            .teams
            .get(&team_id)
            .cloned()
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        let messages = state
            .messages
            .iter()
            .filter(|message| message.team_id == team_id)
            .cloned()
            .collect();
        let events = state
            .events
            .iter()
            .filter(|event| event.team_id() == Some(team_id))
            .cloned()
            .collect();
        Ok(TeamSnapshot {
            team,
            messages,
            events,
        })
    }
}

impl TeamEvent {
    fn team_id(&self) -> Option<ThreadId> {
        match self {
            TeamEvent::TeamCreated { team_id, .. }
            | TeamEvent::MemberSpawned { team_id, .. }
            | TeamEvent::MessageSubmitted { team_id, .. }
            | TeamEvent::TaskCreated { team_id, .. }
            | TeamEvent::TaskUpdated { team_id, .. }
            | TeamEvent::TeamStopped { team_id, .. } => Some(*team_id),
            TeamEvent::Failure { team_id, .. } => *team_id,
        }
    }
}

fn validate_member(team: &Team, member_id: Option<ThreadId>) -> CodexResult<()> {
    if let Some(member_id) = member_id
        && !team.members.iter().any(|member| member.id == member_id)
    {
        return Err(CodexErr::ThreadNotFound(member_id));
    }
    Ok(())
}

fn validate_dependencies(team: &Team, dependencies: &[ThreadId]) -> CodexResult<()> {
    for dependency in dependencies {
        if !team.tasks.iter().any(|task| task.id == *dependency) {
            return Err(CodexErr::ThreadNotFound(*dependency));
        }
    }
    Ok(())
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CodexAuth;
    use crate::ThreadManager;
    use crate::built_in_model_providers;
    use crate::codex::make_session_and_context;
    use crate::protocol::Op;
    use pretty_assertions::assert_eq;

    fn text_input(text: &str) -> Vec<UserInput> {
        vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }]
    }

    fn expected_spawn_items(team: &Team, member: &TeamMember, prompt: &str) -> Vec<UserInput> {
        let team_id = team.id;
        let team_name = &team.name;
        let lead_thread_id = team.lead_thread_id;
        let member_id = member.id;
        let member_name = &member.name;
        let profile_label = member.profile.as_deref().unwrap_or("none");
        let capabilities_label = if member.capabilities.is_empty() {
            "none".to_string()
        } else {
            member.capabilities.join(", ")
        };
        let permissions_label = if member.permissions.is_empty() {
            "none".to_string()
        } else {
            member.permissions.join(", ")
        };
        vec![
            UserInput::Text {
                text: format!(
                    "Codex Teams context:\n\
                     - team_id: {team_id}\n\
                     - team_name: {team_name}\n\
                     - lead_thread_id: {lead_thread_id}\n\
                     - member_id: {member_id}\n\
                     - member_name: {member_name}\n\
                     - profile: {profile_label}\n\
                     - capabilities: {capabilities_label}\n\
                     - permissions: {permissions_label}\n\
                     - live_session_only: true\n\
                     \n\
                     You are an independent Codex Teams teammate. Do not assume you inherit the lead conversation history.\n\
                     Treat the spawn prompt/items after this context as your assigned task boundary.\n\
                     Use generic Teams tools when available: team_status, team_send, team_task_list, team_task_update, and team_event_list.\n\
                     When sending as this teammate, set sender_member_id to your member_id."
                ),
                text_elements: Vec::new(),
            },
            UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            },
        ]
    }

    fn thread_manager() -> ThreadManager {
        ThreadManager::with_models_provider_for_tests(
            CodexAuth::from_api_key("dummy"),
            built_in_model_providers()["openai"].clone(),
        )
    }

    #[tokio::test]
    async fn create_team_lists_live_session_only_substrate() {
        let registry = TeamRegistry::default();
        let lead_thread_id = ThreadId::new();

        let team = registry
            .create_team("infra".to_string(), lead_thread_id)
            .await;

        assert_eq!(team.name, "infra");
        assert_eq!(team.lead_thread_id, lead_thread_id);
        assert_eq!(team.status, TeamStatus::Active);
        assert_eq!(team.members, Vec::new());
        assert_eq!(team.tasks, Vec::new());
        assert_eq!(team.live_session_only, true);
        assert_eq!(registry.list_teams().await, vec![team]);
    }

    #[tokio::test]
    async fn spawn_send_status_and_stop_use_agent_control() {
        let registry = TeamRegistry::default();
        let manager = thread_manager();
        let agent_control = manager.agent_control();
        let (_session, turn) = make_session_and_context().await;
        let team = registry
            .create_team("slice".to_string(), ThreadId::new())
            .await;

        let member = registry
            .spawn_member(
                SpawnTeamMemberRequest {
                    team_id: team.id,
                    name: "teammate".to_string(),
                    profile: Some("general".to_string()),
                    capabilities: Vec::new(),
                    permissions: Vec::new(),
                    initial_items: text_input("initial task"),
                    config: turn.config.as_ref().clone(),
                    session_source: None,
                },
                &agent_control,
            )
            .await
            .expect("spawn member");

        let expected_initial = (
            member.agent_thread_id,
            Op::UserInput {
                items: expected_spawn_items(&team, &member, "initial task"),
                final_output_json_schema: None,
            },
        );
        assert!(
            manager.captured_ops().contains(&expected_initial),
            "spawn should submit initial input"
        );

        let message = registry
            .send_to_member(
                SendTeamMessageRequest {
                    team_id: team.id,
                    sender_member_id: None,
                    member_id: member.id,
                    content: "follow up".to_string(),
                    delivery_mode: TeamMessageDeliveryMode::Queue,
                    items: text_input("follow up"),
                },
                &agent_control,
            )
            .await
            .expect("send message");
        assert_eq!(
            message.delivery_status,
            TeamMessageDeliveryStatus::Submitted
        );

        let expected_followup = (
            member.agent_thread_id,
            Op::UserInput {
                items: text_input("follow up"),
                final_output_json_schema: None,
            },
        );
        assert!(
            manager.captured_ops().contains(&expected_followup),
            "send should submit follow-up input"
        );

        let status = registry
            .team_status(team.id, &agent_control)
            .await
            .expect("team status");
        assert_eq!(status.team.members.len(), 1);
        assert_eq!(status.messages, vec![message]);

        let stopped = registry
            .stop_team(team.id, &agent_control)
            .await
            .expect("stop team");
        assert_eq!(stopped.team.status, TeamStatus::Stopped);
        assert_eq!(stopped.team.members[0].status, TeamMemberStatus::Stopped);
        assert_eq!(stopped.team.members[0].agent_status, AgentStatus::NotFound);
        assert!(
            manager
                .captured_ops()
                .iter()
                .any(|(id, op)| *id == member.agent_thread_id && matches!(op, Op::Shutdown)),
            "stop should submit shutdown"
        );
    }

    #[tokio::test]
    async fn task_board_create_update_list_and_events_are_generic() {
        let registry = TeamRegistry::default();
        let manager = thread_manager();
        let agent_control = manager.agent_control();
        let (_session, turn) = make_session_and_context().await;
        let team = registry
            .create_team("tasks".to_string(), ThreadId::new())
            .await;
        let member = registry
            .spawn_member(
                SpawnTeamMemberRequest {
                    team_id: team.id,
                    name: "teammate".to_string(),
                    profile: Some("general".to_string()),
                    capabilities: Vec::new(),
                    permissions: Vec::new(),
                    initial_items: text_input("initial task"),
                    config: turn.config.as_ref().clone(),
                    session_source: None,
                },
                &agent_control,
            )
            .await
            .expect("spawn member");
        let first = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "map the code".to_string(),
                assignee_member_id: None,
                dependencies: Vec::new(),
                note: Some("start here".to_string()),
            })
            .await
            .expect("create first task");
        let second = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "wire the tool".to_string(),
                assignee_member_id: None,
                dependencies: vec![first.id],
                note: None,
            })
            .await
            .expect("create second task");

        let updated = registry
            .update_task(
                team.id,
                second.id,
                UpdateTeamTaskRequest {
                    title: Some("wire and test the tool".to_string()),
                    assignee_member_id: Some(member.id),
                    dependencies: Some(vec![first.id]),
                    status: Some(TeamTaskStatus::Completed),
                    note: Some("ready".to_string()),
                },
            )
            .await
            .expect("update task");
        let expected_tasks = vec![first.clone(), updated.clone()];

        assert_eq!(
            registry.list_tasks(team.id).await.expect("list tasks"),
            expected_tasks
        );

        let snapshot = registry
            .team_status(team.id, &agent_control)
            .await
            .expect("team status");
        assert_eq!(snapshot.team.tasks, expected_tasks);

        let events = registry.list_events(team.id).await.expect("list events");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TeamEvent::TaskCreated { task_id, .. } if *task_id == first.id)),
            "task creation should be observable"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TeamEvent::TaskUpdated { task_id, .. } if *task_id == updated.id)),
            "task updates should be observable"
        );
    }
}
