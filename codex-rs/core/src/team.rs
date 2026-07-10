use crate::agent::AgentControl;
use crate::agent::AgentStatus;
#[cfg(test)]
use crate::agent::control::SpawnAgentOptions;
#[cfg(test)]
use crate::config::Config;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
#[cfg(test)]
use codex_protocol::protocol::SessionSource;
#[cfg(test)]
use codex_protocol::protocol::SubAgentSource;
#[cfg(test)]
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_protocol::user_input::UserInput;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::RwLock;

/// Identity of THIS process when it was launched as a spawned `codex teammate`
/// (set once at teammate startup via [`set_teammate_identity`]). It is `None` in
/// a normal lead / standalone session. Team tools running inside a teammate
/// process use it to route messages across the process boundary through the
/// on-disk file mailbox (Claude's model) instead of the in-memory registry,
/// which is empty in a teammate process (it never ran `create_team`).
#[derive(Debug, Clone)]
pub struct TeammateIdentity {
    pub(crate) team: String,
    pub(crate) agent_name: String,
}

static TEAMMATE_IDENTITY: OnceLock<TeammateIdentity> = OnceLock::new();

const TEAMMATE_SYSTEM_PROMPT_ADDENDUM: &str = r#"# Agent Teammate Communication

IMPORTANT: You are running as an agent in a team. To communicate with anyone on your team:
- Use the SendMessage tool with `to: "<name>"` to send messages to specific teammates
- Use the SendMessage tool with `to: "*"` sparingly for team-wide broadcasts

Just writing a response in text is not visible to others on your team - you MUST use the SendMessage tool.

The user interacts primarily with the team lead. Your work is coordinated through the task system and teammate messaging."#;

/// Record that this process is running as a teammate. Called once by the hidden
/// `codex teammate` entrypoint before its inbox loop starts; later calls are
/// ignored (the identity is fixed for the process lifetime).
pub fn set_teammate_identity(team: String, agent_name: String) {
    let _ = TEAMMATE_IDENTITY.set(TeammateIdentity { team, agent_name });
}

/// The teammate identity for this process, if it is a spawned teammate.
pub(crate) fn teammate_identity() -> Option<&'static TeammateIdentity> {
    TEAMMATE_IDENTITY.get()
}

pub(crate) fn teammate_system_prompt_addendum() -> Option<&'static str> {
    TEAMMATE_IDENTITY
        .get()
        .map(|_| TEAMMATE_SYSTEM_PROMPT_ADDENDUM)
}

/// Public `(team, agent_name)` for the current teammate process, if it was
/// launched as one. The TUI uses this at startup to enter teammate mode (poll
/// its own inbox and inject the lead's messages as turns in its own session).
pub fn teammate_identity_parts() -> Option<(String, String)> {
    TEAMMATE_IDENTITY
        .get()
        .map(|id| (id.team.clone(), id.agent_name.clone()))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamStatus {
    Active,
    Stopped,
}

#[cfg(test)]
mod teammate_prompt_tests {
    use super::TEAMMATE_SYSTEM_PROMPT_ADDENDUM;

    #[test]
    fn teammate_prompt_addendum_matches_claude_visibility_contract() {
        assert_eq!(
            TEAMMATE_SYSTEM_PROMPT_ADDENDUM,
            r#"# Agent Teammate Communication

IMPORTANT: You are running as an agent in a team. To communicate with anyone on your team:
- Use the SendMessage tool with `to: "<name>"` to send messages to specific teammates
- Use the SendMessage tool with `to: "*"` sparingly for team-wide broadcasts

Just writing a response in text is not visible to others on your team - you MUST use the SendMessage tool.

The user interacts primarily with the team lead. Your work is coordinated through the task system and teammate messaging."#
        );
        assert!(!TEAMMATE_SYSTEM_PROMPT_ADDENDUM.contains("Codex"));
        assert!(!TEAMMATE_SYSTEM_PROMPT_ADDENDUM.contains("team_send"));
    }
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

impl TeamMember {
    /// Build a record for an out-of-process (split-pane) teammate using a
    /// caller-provided `id`, so the spawn handler can reference the same id in
    /// the on-disk member record before returning it to the model. The live
    /// registry mirrors this member for status/task/stop semantics, while
    /// cross-process IO stays on disk. `agent_thread_id` mirrors `id` as a
    /// sentinel because there is no
    /// in-process agent thread behind a pane-backed teammate.
    pub(crate) fn process_member_with_id(
        id: ThreadId,
        name: String,
        profile: Option<String>,
        capabilities: Vec<String>,
        permissions: Vec<String>,
    ) -> Self {
        let at = unix_timestamp();
        Self {
            id,
            name,
            agent_thread_id: id,
            profile,
            capabilities,
            permissions,
            status: TeamMemberStatus::Active,
            agent_status: AgentStatus::Running,
            created_at: at,
            last_activity_at: at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct TeamMessage {
    pub(crate) id: ThreadId,
    pub(crate) team_id: ThreadId,
    pub(crate) sender: TeamMessageEndpoint,
    pub(crate) target: TeamMessageEndpoint,
    pub(crate) target_member_id: Option<ThreadId>,
    pub(crate) content: String,
    #[serde(default)]
    pub(crate) items: Vec<UserInput>,
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
    MemberStopped {
        team_id: ThreadId,
        member_id: ThreadId,
        agent_thread_id: ThreadId,
        at: i64,
    },
    MessageSubmitted {
        team_id: ThreadId,
        message_id: ThreadId,
        target: TeamMessageEndpoint,
        target_member_id: Option<ThreadId>,
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

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct TeamSnapshot {
    pub(crate) team: Team,
    pub(crate) messages: Vec<TeamMessage>,
    pub(crate) events: Vec<TeamEvent>,
}

#[cfg(test)]
pub(crate) struct SpawnTeamMemberRequest {
    pub(crate) team_id: ThreadId,
    pub(crate) name: String,
    pub(crate) profile: Option<String>,
    pub(crate) capabilities: Vec<String>,
    pub(crate) permissions: Vec<String>,
    pub(crate) initial_items: Vec<UserInput>,
    pub(crate) config: Config,
    pub(crate) session_source: Option<SessionSource>,
    pub(crate) environments: Option<Vec<TurnEnvironmentSelection>>,
}

pub(crate) struct SendTeamMessageRequest {
    pub(crate) team_id: ThreadId,
    pub(crate) sender_member_id: Option<ThreadId>,
    pub(crate) target: SendTeamMessageTarget,
    pub(crate) content: String,
    pub(crate) delivery_mode: TeamMessageDeliveryMode,
    pub(crate) items: Vec<UserInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SendTeamMessageTarget {
    Lead,
    Member(ThreadId),
}

pub(crate) struct ListTeamMessagesRequest {
    pub(crate) team_id: ThreadId,
    pub(crate) target: Option<TeamMessageTargetFilter>,
    pub(crate) member_id: Option<ThreadId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TeamMessageTargetFilter {
    Lead,
    Member,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TeamCaller {
    Lead,
    Member(ThreadId),
    Unknown,
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
    pub(crate) clear_assignee: bool,
    pub(crate) dependencies: Option<Vec<ThreadId>>,
    pub(crate) status: Option<TeamTaskStatus>,
    pub(crate) note: Option<String>,
    pub(crate) clear_note: bool,
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

    pub(crate) async fn register_process_member(
        &self,
        team_id: ThreadId,
        member: TeamMember,
    ) -> CodexResult<TeamMember> {
        let at = unix_timestamp();
        let mut state = self.state.write().await;
        let team = state
            .teams
            .get_mut(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        validate_team_active(team_id, team)?;
        if team.members.iter().any(|existing| existing.id == member.id) {
            return Ok(member);
        }
        team.members.push(member.clone());
        team.updated_at = at;
        state.events.push(TeamEvent::MemberSpawned {
            team_id,
            member_id: member.id,
            agent_thread_id: member.agent_thread_id,
            name: member.name.clone(),
            at,
        });
        Ok(member)
    }

    pub(crate) async fn team_status(
        &self,
        team_id: ThreadId,
        agent_control: &AgentControl,
    ) -> CodexResult<TeamSnapshot> {
        self.refresh_agent_statuses(team_id, agent_control).await?;
        self.snapshot(team_id).await
    }

    pub(crate) async fn caller_for_thread(
        &self,
        team_id: ThreadId,
        thread_id: ThreadId,
    ) -> CodexResult<TeamCaller> {
        let state = self.state.read().await;
        let team = state
            .teams
            .get(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        if team.lead_thread_id == thread_id {
            return Ok(TeamCaller::Lead);
        }
        if let Some(member) = team
            .members
            .iter()
            .find(|member| member.agent_thread_id == thread_id)
        {
            return Ok(TeamCaller::Member(member.id));
        }
        Ok(TeamCaller::Unknown)
    }

    pub(crate) async fn active_caller_for_thread(
        &self,
        team_id: ThreadId,
        thread_id: ThreadId,
    ) -> CodexResult<TeamCaller> {
        let state = self.state.read().await;
        let team = state
            .teams
            .get(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        if team.lead_thread_id == thread_id {
            return Ok(TeamCaller::Lead);
        }
        if let Some(member) = team
            .members
            .iter()
            .find(|member| member.agent_thread_id == thread_id)
        {
            if member.status == TeamMemberStatus::Stopped {
                return Err(CodexErr::UnsupportedOperation(format!(
                    "team member {} is stopped",
                    member.id
                )));
            }
            return Ok(TeamCaller::Member(member.id));
        }
        Ok(TeamCaller::Unknown)
    }

    #[cfg(test)]
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
            initial_items,
            config,
            session_source,
            environments,
        } = request;
        {
            let state = self.state.read().await;
            let team = state
                .teams
                .get(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            validate_team_active(team_id, team)?;
        }
        let member_id = ThreadId::new();
        let parent_thread_id = match session_source.as_ref() {
            Some(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
                parent_thread_id, ..
            })) => Some(*parent_thread_id),
            _ => None,
        };
        let spawned_agent = agent_control
            .spawn_agent_with_metadata(
                config,
                initial_items,
                session_source,
                SpawnAgentOptions {
                    parent_thread_id,
                    environments,
                    ..Default::default()
                },
            )
            .await?;
        let agent_thread_id = spawned_agent.thread_id;
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

        let stale_team = {
            let mut state = self.state.write().await;
            let team = state
                .teams
                .get_mut(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            if team.status == TeamStatus::Stopped {
                true
            } else {
                team.members.push(member.clone());
                team.updated_at = at;
                state.events.push(TeamEvent::MemberSpawned {
                    team_id,
                    member_id: member.id,
                    agent_thread_id,
                    name: member.name.clone(),
                    at,
                });
                false
            }
        };
        if stale_team {
            let _ = Box::pin(agent_control.close_agent(agent_thread_id)).await;
            return Err(stopped_team_error(team_id));
        }
        Ok(member)
    }

    pub(crate) async fn send_message(
        &self,
        request: SendTeamMessageRequest,
        agent_control: &AgentControl,
    ) -> CodexResult<TeamMessage> {
        let SendTeamMessageRequest {
            team_id,
            sender_member_id,
            target,
            content,
            delivery_mode,
            items,
        } = request;
        let message_id = ThreadId::new();
        let at = unix_timestamp();
        let (mut message, target_member_id, agent_thread_id) = {
            let mut state = self.state.write().await;
            let team = state
                .teams
                .get_mut(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            validate_team_active(team_id, team)?;
            let sender = if let Some(sender_member_id) = sender_member_id {
                validate_active_member(team, Some(sender_member_id))?;
                TeamMessageEndpoint::Member(sender_member_id)
            } else {
                TeamMessageEndpoint::Lead(team.lead_thread_id)
            };
            let (target, target_member_id, agent_thread_id) = match target {
                SendTeamMessageTarget::Lead => {
                    if delivery_mode == TeamMessageDeliveryMode::Interrupt {
                        return Err(CodexErr::UnsupportedOperation(
                            "interrupt delivery is only supported for member targets".to_string(),
                        ));
                    }
                    (TeamMessageEndpoint::Lead(team.lead_thread_id), None, None)
                }
                SendTeamMessageTarget::Member(member_id) => {
                    validate_active_member(team, Some(member_id))?;
                    let member = team
                        .members
                        .iter()
                        .find(|member| member.id == member_id)
                        .ok_or(CodexErr::ThreadNotFound(member_id))?;
                    (
                        TeamMessageEndpoint::Member(member_id),
                        Some(member_id),
                        Some(member.agent_thread_id),
                    )
                }
            };
            let message = TeamMessage {
                id: message_id,
                team_id,
                sender,
                target,
                target_member_id,
                content,
                items: items.clone(),
                submitted_id: None,
                delivery_mode,
                delivery_status: TeamMessageDeliveryStatus::Submitted,
                created_at: at,
            };
            if let Some(target_member_id) = target_member_id
                && let Some(member) = team
                    .members
                    .iter_mut()
                    .find(|member| member.id == target_member_id)
            {
                member.last_activity_at = at;
            }
            team.updated_at = at;
            state.messages.push(message.clone());
            state.events.push(TeamEvent::MessageSubmitted {
                team_id,
                message_id: message.id,
                target: message.target.clone(),
                target_member_id,
                at,
            });
            (message, target_member_id, agent_thread_id)
        };

        let Some(agent_thread_id) = agent_thread_id else {
            return Ok(message);
        };

        if message.delivery_mode == TeamMessageDeliveryMode::Interrupt
            && let Err(err) = agent_control.interrupt_agent(agent_thread_id).await
        {
            let agent_status = agent_control.get_status(agent_thread_id).await;
            message.delivery_status = TeamMessageDeliveryStatus::Failed;
            self.update_message_delivery(
                team_id,
                message_id,
                target_member_id,
                None,
                TeamMessageDeliveryStatus::Failed,
                Some(agent_status),
            )
            .await;
            return Err(err);
        }

        match agent_control.send_input(agent_thread_id, items).await {
            Ok(submission_id) => {
                let agent_status = agent_control.get_status(agent_thread_id).await;
                message.submitted_id = Some(submission_id.clone());
                self.update_message_delivery(
                    team_id,
                    message_id,
                    target_member_id,
                    Some(submission_id),
                    TeamMessageDeliveryStatus::Submitted,
                    Some(agent_status),
                )
                .await;
                Ok(message)
            }
            Err(err) => {
                let agent_status = agent_control.get_status(agent_thread_id).await;
                message.delivery_status = TeamMessageDeliveryStatus::Failed;
                self.update_message_delivery(
                    team_id,
                    message_id,
                    target_member_id,
                    None,
                    TeamMessageDeliveryStatus::Failed,
                    Some(agent_status),
                )
                .await;
                Err(err)
            }
        }
    }

    async fn update_message_delivery(
        &self,
        team_id: ThreadId,
        message_id: ThreadId,
        target_member_id: Option<ThreadId>,
        submitted_id: Option<String>,
        delivery_status: TeamMessageDeliveryStatus,
        agent_status: Option<AgentStatus>,
    ) {
        let at = unix_timestamp();
        let mut state = self.state.write().await;
        if let Some(message) = state
            .messages
            .iter_mut()
            .find(|message| message.team_id == team_id && message.id == message_id)
        {
            message.submitted_id = submitted_id;
            message.delivery_status = delivery_status;
        }
        if let Some(team) = state.teams.get_mut(&team_id) {
            if let Some(target_member_id) = target_member_id
                && let Some(member) = team
                    .members
                    .iter_mut()
                    .find(|member| member.id == target_member_id)
            {
                if let Some(agent_status) = agent_status {
                    member.agent_status = agent_status;
                }
                member.last_activity_at = at;
            }
            team.updated_at = at;
        }
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

    pub(crate) async fn list_messages(
        &self,
        request: ListTeamMessagesRequest,
    ) -> CodexResult<Vec<TeamMessage>> {
        let ListTeamMessagesRequest {
            team_id,
            target,
            member_id,
        } = request;
        let state = self.state.read().await;
        let team = state
            .teams
            .get(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        validate_member_exists(team, member_id)?;
        Ok(state
            .messages
            .iter()
            .filter(|message| message.team_id == team_id)
            .filter(|message| match target {
                Some(TeamMessageTargetFilter::Lead) => {
                    matches!(message.target, TeamMessageEndpoint::Lead(_))
                }
                Some(TeamMessageTargetFilter::Member) => {
                    matches!(message.target, TeamMessageEndpoint::Member(_))
                }
                None => true,
            })
            .filter(|message| match member_id {
                Some(member_id) => message.target_member_id == Some(member_id),
                None => true,
            })
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
        validate_team_active(team_id, team)?;
        validate_active_member(team, task.assignee_member_id)?;
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
        validate_team_active(team_id, team)?;
        if request.clear_assignee && request.assignee_member_id.is_some() {
            return Err(CodexErr::UnsupportedOperation(
                "clear_assignee can't be combined with assignee_member_id".to_string(),
            ));
        }
        if request.clear_note && request.note.is_some() {
            return Err(CodexErr::UnsupportedOperation(
                "clear_note can't be combined with note".to_string(),
            ));
        }
        validate_active_member(team, request.assignee_member_id)?;
        if let Some(dependencies) = &request.dependencies {
            if dependencies.contains(&task_id) {
                return Err(CodexErr::UnsupportedOperation(format!(
                    "task {task_id} can't depend on itself"
                )));
            }
            validate_dependencies(team, dependencies)?;
        }
        let task_index = team
            .tasks
            .iter()
            .position(|task| task.id == task_id)
            .ok_or(CodexErr::ThreadNotFound(task_id))?;
        let existing_task = &team.tasks[task_index];
        if request.status.as_ref() == Some(&TeamTaskStatus::Claimed)
            && existing_task.status != TeamTaskStatus::Claimed
        {
            return Err(CodexErr::UnsupportedOperation(
                "use team_task_claim to claim tasks".to_string(),
            ));
        }
        let next_assignee_member_id = if request.clear_assignee {
            None
        } else {
            request
                .assignee_member_id
                .or(existing_task.assignee_member_id)
        };
        let next_dependencies = request
            .dependencies
            .as_deref()
            .unwrap_or(&existing_task.dependencies);
        let next_status = request
            .status
            .clone()
            .unwrap_or_else(|| existing_task.status.clone());
        if next_status == TeamTaskStatus::Claimed {
            let Some(assignee_member_id) = next_assignee_member_id else {
                return Err(CodexErr::UnsupportedOperation(format!(
                    "task {task_id} needs an assignee before it can be claimed"
                )));
            };
            validate_active_member(team, Some(assignee_member_id))?;
            if existing_task.status == TeamTaskStatus::Claimed {
                if next_assignee_member_id != existing_task.assignee_member_id {
                    return Err(CodexErr::UnsupportedOperation(format!(
                        "claimed task {task_id} can't change assignee while claimed"
                    )));
                }
                if next_dependencies != existing_task.dependencies.as_slice() {
                    return Err(CodexErr::UnsupportedOperation(format!(
                        "claimed task {task_id} can't change dependencies while claimed"
                    )));
                }
            }
        }
        validate_dependency_graph_acyclic(team, task_id, next_dependencies)?;

        let task = &mut team.tasks[task_index];
        if let Some(title) = request.title {
            task.title = title;
        }
        if request.clear_assignee {
            task.assignee_member_id = None;
        } else if let Some(assignee_member_id) = request.assignee_member_id {
            task.assignee_member_id = Some(assignee_member_id);
        }
        if let Some(dependencies) = request.dependencies {
            task.dependencies = dependencies;
        }
        if let Some(status) = request.status {
            task.status = status;
        }
        if request.clear_note {
            task.note = None;
        } else if let Some(note) = request.note {
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

    pub(crate) async fn claim_task(
        &self,
        team_id: ThreadId,
        task_id: ThreadId,
        member_id: ThreadId,
    ) -> CodexResult<TeamTask> {
        self.ensure_team_active(team_id).await?;
        let at = unix_timestamp();
        let mut state = self.state.write().await;
        let team = state
            .teams
            .get_mut(&team_id)
            .ok_or(CodexErr::ThreadNotFound(team_id))?;
        validate_team_active(team_id, team)?;
        validate_active_member(team, Some(member_id))?;
        let task_index = team
            .tasks
            .iter()
            .position(|task| task.id == task_id)
            .ok_or(CodexErr::ThreadNotFound(task_id))?;
        let task = &team.tasks[task_index];
        if task.status == TeamTaskStatus::Claimed && task.assignee_member_id == Some(member_id) {
            return Ok(task.clone());
        }
        if task.status != TeamTaskStatus::Open {
            return Err(CodexErr::UnsupportedOperation(format!(
                "task {task_id} must be open before it can be claimed"
            )));
        }
        if let Some(assignee_member_id) = task.assignee_member_id
            && assignee_member_id != member_id
        {
            return Err(CodexErr::UnsupportedOperation(format!(
                "task {task_id} is assigned to member {assignee_member_id}"
            )));
        }
        for dependency in &task.dependencies {
            let dependency_task = team
                .tasks
                .iter()
                .find(|candidate| candidate.id == *dependency)
                .ok_or(CodexErr::ThreadNotFound(*dependency))?;
            if dependency_task.status != TeamTaskStatus::Completed {
                return Err(CodexErr::UnsupportedOperation(format!(
                    "task {task_id} depends on incomplete task {dependency}"
                )));
            }
        }

        let task = &mut team.tasks[task_index];
        task.assignee_member_id = Some(member_id);
        task.status = TeamTaskStatus::Claimed;
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

    pub(crate) async fn stop_member(
        &self,
        team_id: ThreadId,
        member_id: ThreadId,
        agent_control: &AgentControl,
    ) -> CodexResult<TeamSnapshot> {
        let (agent_thread_id, already_stopped) = {
            let at = unix_timestamp();
            let mut state = self.state.write().await;
            let team = state
                .teams
                .get_mut(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            validate_team_active(team_id, team)?;
            let member = team
                .members
                .iter_mut()
                .find(|member| member.id == member_id)
                .ok_or(CodexErr::ThreadNotFound(member_id))?;
            if member.status == TeamMemberStatus::Stopped {
                (member.agent_thread_id, true)
            } else if is_process_member(member) {
                let agent_thread_id = member.agent_thread_id;
                member.status = TeamMemberStatus::Stopped;
                member.agent_status = AgentStatus::Shutdown;
                member.last_activity_at = at;
                team.updated_at = at;
                state.events.push(TeamEvent::MemberStopped {
                    team_id,
                    member_id,
                    agent_thread_id,
                    at,
                });
                (agent_thread_id, true)
            } else {
                let agent_thread_id = member.agent_thread_id;
                member.status = TeamMemberStatus::Stopped;
                member.last_activity_at = at;
                team.updated_at = at;
                state.events.push(TeamEvent::MemberStopped {
                    team_id,
                    member_id,
                    agent_thread_id,
                    at,
                });
                (agent_thread_id, false)
            }
        };

        if already_stopped {
            return self.snapshot(team_id).await;
        }

        let _ = Box::pin(agent_control.close_agent(agent_thread_id)).await;
        let agent_status = agent_control.get_status(agent_thread_id).await;
        {
            let mut state = self.state.write().await;
            if let Some(team) = state.teams.get_mut(&team_id)
                && let Some(member) = team
                    .members
                    .iter_mut()
                    .find(|member| member.id == member_id)
            {
                member.agent_status = agent_status;
            }
        }

        self.snapshot(team_id).await
    }

    pub(crate) async fn stop_team(
        &self,
        team_id: ThreadId,
        agent_control: &AgentControl,
    ) -> CodexResult<TeamSnapshot> {
        let agent_thread_ids = {
            let at = unix_timestamp();
            let mut state = self.state.write().await;
            let team = state
                .teams
                .get_mut(&team_id)
                .ok_or(CodexErr::ThreadNotFound(team_id))?;
            validate_team_active(team_id, team)?;
            team.status = TeamStatus::Stopped;
            team.updated_at = at;
            let agent_thread_ids = team
                .members
                .iter_mut()
                .filter_map(|member| {
                    if member.status == TeamMemberStatus::Active {
                        member.status = TeamMemberStatus::Stopped;
                        member.last_activity_at = at;
                        if is_process_member(member) {
                            member.agent_status = AgentStatus::Shutdown;
                            None
                        } else {
                            Some(member.agent_thread_id)
                        }
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            state.events.push(TeamEvent::TeamStopped { team_id, at });
            agent_thread_ids
        };

        for agent_thread_id in &agent_thread_ids {
            let _ = Box::pin(agent_control.close_agent(*agent_thread_id)).await;
        }

        let mut resolved_statuses = Vec::with_capacity(agent_thread_ids.len());
        for agent_thread_id in agent_thread_ids {
            resolved_statuses.push((
                agent_thread_id,
                agent_control.get_status(agent_thread_id).await,
            ));
        }

        {
            let mut state = self.state.write().await;
            if let Some(team) = state.teams.get_mut(&team_id) {
                for member in &mut team.members {
                    if let Some((_, status)) = resolved_statuses
                        .iter()
                        .find(|(agent_thread_id, _)| *agent_thread_id == member.agent_thread_id)
                    {
                        member.agent_status = status.clone();
                    }
                }
            }
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
                .filter(|member| !is_process_member(member))
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
            | TeamEvent::MemberStopped { team_id, .. }
            | TeamEvent::MessageSubmitted { team_id, .. }
            | TeamEvent::TaskCreated { team_id, .. }
            | TeamEvent::TaskUpdated { team_id, .. }
            | TeamEvent::TeamStopped { team_id, .. } => Some(*team_id),
            TeamEvent::Failure { team_id, .. } => *team_id,
        }
    }
}

fn validate_team_active(team_id: ThreadId, team: &Team) -> CodexResult<()> {
    if team.status == TeamStatus::Stopped {
        return Err(stopped_team_error(team_id));
    }
    Ok(())
}

fn stopped_team_error(team_id: ThreadId) -> CodexErr {
    CodexErr::UnsupportedOperation(format!("team {team_id} is stopped"))
}

fn validate_active_member(team: &Team, member_id: Option<ThreadId>) -> CodexResult<()> {
    if let Some(member_id) = member_id {
        let member = team
            .members
            .iter()
            .find(|member| member.id == member_id)
            .ok_or(CodexErr::ThreadNotFound(member_id))?;
        if member.status == TeamMemberStatus::Stopped {
            return Err(CodexErr::UnsupportedOperation(format!(
                "team member {member_id} is stopped"
            )));
        }
    }
    Ok(())
}

fn validate_member_exists(team: &Team, member_id: Option<ThreadId>) -> CodexResult<()> {
    if let Some(member_id) = member_id
        && !team.members.iter().any(|member| member.id == member_id)
    {
        return Err(CodexErr::ThreadNotFound(member_id));
    }
    Ok(())
}

fn is_process_member(member: &TeamMember) -> bool {
    member.agent_thread_id == member.id
}

fn validate_dependency_graph_acyclic(
    team: &Team,
    task_id: ThreadId,
    dependencies: &[ThreadId],
) -> CodexResult<()> {
    for dependency in dependencies {
        let mut visited = Vec::new();
        if dependency_reaches_task(
            team,
            *dependency,
            task_id,
            task_id,
            dependencies,
            &mut visited,
        )? {
            return Err(CodexErr::UnsupportedOperation(format!(
                "task {task_id} dependency graph contains a cycle"
            )));
        }
    }
    Ok(())
}

fn dependency_reaches_task(
    team: &Team,
    from_task_id: ThreadId,
    target_task_id: ThreadId,
    replacement_task_id: ThreadId,
    replacement_dependencies: &[ThreadId],
    visited: &mut Vec<ThreadId>,
) -> CodexResult<bool> {
    if from_task_id == target_task_id {
        return Ok(true);
    }
    if visited.contains(&from_task_id) {
        return Ok(false);
    }
    visited.push(from_task_id);

    let dependencies = if from_task_id == replacement_task_id {
        replacement_dependencies
    } else {
        team.tasks
            .iter()
            .find(|task| task.id == from_task_id)
            .ok_or(CodexErr::ThreadNotFound(from_task_id))?
            .dependencies
            .as_slice()
    };
    for dependency in dependencies {
        if dependency_reaches_task(
            team,
            *dependency,
            target_task_id,
            replacement_task_id,
            replacement_dependencies,
            visited,
        )? {
            return Ok(true);
        }
    }
    Ok(false)
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
    use crate::ThreadManager;
    use crate::session::tests::make_session_and_context;
    use codex_login::CodexAuth;
    use codex_model_provider_info::built_in_model_providers;
    use codex_protocol::protocol::Op;
    use pretty_assertions::assert_eq;

    fn text_input(text: &str) -> Vec<UserInput> {
        vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }]
    }

    fn expected_spawn_items(team: &Team, member: &TeamMember, prompt: &str) -> Vec<UserInput> {
        let _ = (team, member);
        vec![UserInput::Text {
            text: prompt.to_string(),
            text_elements: Vec::new(),
        }]
    }

    fn expected_message_items(message: &TeamMessage, prompt: &str) -> Vec<UserInput> {
        let _ = message;
        text_input(prompt)
    }

    fn expected_message_items_with_items(
        message: &TeamMessage,
        prompt: &str,
        items: Vec<UserInput>,
    ) -> Vec<UserInput> {
        let _ = (message, prompt);
        items
    }

    fn thread_manager() -> ThreadManager {
        ThreadManager::with_models_provider_for_tests(
            CodexAuth::from_api_key("dummy"),
            built_in_model_providers(/*openai_base_url*/ None)["openai"].clone(),
        )
    }

    fn run_team_test<F, Fut>(test: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = std::thread::Builder::new()
            .name("team-test".to_string())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("create team test runtime");
                runtime.block_on(test());
            })
            .expect("spawn team test thread");
        handle.join().expect("team test thread should not panic");
    }

    async fn remove_backing_agent(manager: &ThreadManager, thread_id: ThreadId) {
        let stale_thread = manager
            .remove_thread(&thread_id)
            .await
            .expect("member thread should be loaded before removal");
        Box::pin(stale_thread.submit(Op::Shutdown {}))
            .await
            .expect("removed member thread should accept shutdown");
        stale_thread.wait_until_terminated().await;
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

    #[test]
    fn spawn_send_status_and_stop_use_agent_control() {
        run_team_test(spawn_send_status_and_stop_use_agent_control_body);
    }

    async fn spawn_send_status_and_stop_use_agent_control_body() {
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
                    environments: None,
                },
                &agent_control,
            )
            .await
            .expect("spawn member");

        let expected_initial: (ThreadId, Op) = (
            member.agent_thread_id,
            expected_spawn_items(&team, &member, "initial task").into(),
        );
        assert!(
            manager.captured_ops().contains(&expected_initial),
            "spawn should submit initial input"
        );

        let message = registry
            .send_message(
                SendTeamMessageRequest {
                    team_id: team.id,
                    sender_member_id: None,
                    target: SendTeamMessageTarget::Member(member.id),
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

        let expected_followup: (ThreadId, Op) = (
            member.agent_thread_id,
            expected_message_items(&message, "follow up").into(),
        );
        assert!(
            manager.captured_ops().contains(&expected_followup),
            "send should submit follow-up input without a visible Teams envelope"
        );

        let member_items = vec![
            UserInput::Text {
                text: "structured follow up".to_string(),
                text_elements: Vec::new(),
            },
            UserInput::Image {
                image_url: "data:image/png;base64,BBBB".to_string(),
                detail: None,
            },
        ];
        let structured_member_message = registry
            .send_message(
                SendTeamMessageRequest {
                    team_id: team.id,
                    sender_member_id: None,
                    target: SendTeamMessageTarget::Member(member.id),
                    content: "structured follow up\n[image]".to_string(),
                    delivery_mode: TeamMessageDeliveryMode::Queue,
                    items: member_items.clone(),
                },
                &agent_control,
            )
            .await
            .expect("send structured member message");
        assert_eq!(structured_member_message.items, member_items);
        assert!(
            manager.captured_ops().contains(&(
                member.agent_thread_id,
                expected_message_items_with_items(
                    &structured_member_message,
                    "structured follow up\n[image]",
                    member_items.clone(),
                )
                .into(),
            )),
            "structured member messages should preserve original items without a visible Teams envelope"
        );

        let lead_items = vec![
            UserInput::Text {
                text: "lead update".to_string(),
                text_elements: Vec::new(),
            },
            UserInput::Image {
                image_url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        ];
        let lead_message = registry
            .send_message(
                SendTeamMessageRequest {
                    team_id: team.id,
                    sender_member_id: Some(member.id),
                    target: SendTeamMessageTarget::Lead,
                    content: "lead update\n[image]".to_string(),
                    delivery_mode: TeamMessageDeliveryMode::Queue,
                    items: lead_items.clone(),
                },
                &agent_control,
            )
            .await
            .expect("send lead mailbox message");
        assert_eq!(
            lead_message.target,
            TeamMessageEndpoint::Lead(team.lead_thread_id)
        );
        assert_eq!(lead_message.items, lead_items);
        assert_eq!(lead_message.target_member_id, None);
        assert_eq!(lead_message.submitted_id, None);
        assert_eq!(
            registry
                .list_messages(ListTeamMessagesRequest {
                    team_id: team.id,
                    target: Some(TeamMessageTargetFilter::Lead),
                    member_id: None,
                })
                .await
                .expect("list lead messages"),
            vec![lead_message.clone()]
        );
        assert_eq!(
            registry
                .list_messages(ListTeamMessagesRequest {
                    team_id: team.id,
                    target: Some(TeamMessageTargetFilter::Member),
                    member_id: Some(member.id),
                })
                .await
                .expect("list member messages"),
            vec![message.clone(), structured_member_message.clone()]
        );
        assert!(
            registry
                .list_messages(ListTeamMessagesRequest {
                    team_id: team.id,
                    target: Some(TeamMessageTargetFilter::Lead),
                    member_id: Some(member.id),
                })
                .await
                .expect("lead messages ignore member filter")
                .is_empty()
        );

        let task_before_stop = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "before stop".to_string(),
                assignee_member_id: Some(member.id),
                dependencies: Vec::new(),
                note: None,
            })
            .await
            .expect("create pre-stop task");

        let status = registry
            .team_status(team.id, &agent_control)
            .await
            .expect("team status");
        assert_eq!(status.team.members.len(), 1);
        assert_eq!(
            status.messages,
            vec![message, structured_member_message, lead_message]
        );

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
        assert!(
            registry.stop_team(team.id, &agent_control).await.is_err(),
            "stopping an already stopped team should fail"
        );
        assert_eq!(
            registry
                .team_status(team.id, &agent_control)
                .await
                .expect("stopped team remains readable")
                .team
                .status,
            TeamStatus::Stopped
        );
        assert_eq!(
            registry.list_teams().await[0].status,
            TeamStatus::Stopped,
            "stopped team remains visible in list_teams"
        );
        assert_eq!(
            registry
                .list_tasks(team.id)
                .await
                .expect("list stopped team tasks"),
            vec![task_before_stop.clone()]
        );
        assert!(
            registry
                .spawn_member(
                    SpawnTeamMemberRequest {
                        team_id: team.id,
                        name: "too late".to_string(),
                        profile: None,
                        capabilities: Vec::new(),
                        permissions: Vec::new(),
                        initial_items: text_input("after stop"),
                        config: turn.config.as_ref().clone(),
                        session_source: None,
                        environments: None,
                    },
                    &agent_control,
                )
                .await
                .is_err(),
            "stopped team should reject member spawn"
        );
        assert!(
            registry
                .send_message(
                    SendTeamMessageRequest {
                        team_id: team.id,
                        sender_member_id: None,
                        target: SendTeamMessageTarget::Member(member.id),
                        content: "after stop".to_string(),
                        delivery_mode: TeamMessageDeliveryMode::Queue,
                        items: text_input("after stop"),
                    },
                    &agent_control,
                )
                .await
                .is_err(),
            "stopped team should reject sends"
        );
        assert!(
            registry
                .create_task(CreateTeamTaskRequest {
                    team_id: team.id,
                    title: "after stop".to_string(),
                    assignee_member_id: None,
                    dependencies: Vec::new(),
                    note: None,
                })
                .await
                .is_err(),
            "stopped team should reject task creation"
        );
        assert!(
            registry
                .update_task(
                    team.id,
                    task_before_stop.id,
                    UpdateTeamTaskRequest {
                        status: Some(TeamTaskStatus::Completed),
                        ..Default::default()
                    },
                )
                .await
                .is_err(),
            "stopped team should reject task updates"
        );
        assert!(
            registry
                .claim_task(team.id, task_before_stop.id, member.id)
                .await
                .is_err(),
            "stopped team should reject task claims"
        );
        assert!(
            registry
                .list_events(team.id)
                .await
                .expect("list stopped team events")
                .iter()
                .any(|event| matches!(event, TeamEvent::TeamStopped { .. })),
            "stopped team event feed remains readable"
        );
    }

    #[tokio::test]
    async fn process_member_mirror_supports_status_tasks_and_stop() {
        let registry = TeamRegistry::default();
        let manager = thread_manager();
        let agent_control = manager.agent_control();
        let team = registry
            .create_team("process members".to_string(), ThreadId::new())
            .await;
        let member = TeamMember::process_member_with_id(
            ThreadId::new(),
            "pane-a".to_string(),
            Some("general".to_string()),
            vec!["audit".to_string()],
            Vec::new(),
        );

        let registered = registry
            .register_process_member(team.id, member.clone())
            .await
            .expect("register process member");

        assert_eq!(registered, member);
        let status = registry
            .team_status(team.id, &agent_control)
            .await
            .expect("process member status");
        assert_eq!(status.team.members, vec![member.clone()]);

        let task = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "owned by pane".to_string(),
                assignee_member_id: Some(member.id),
                dependencies: Vec::new(),
                note: None,
            })
            .await
            .expect("assign task to process member");
        assert_eq!(task.assignee_member_id, Some(member.id));

        let stopped = registry
            .stop_member(team.id, member.id, &agent_control)
            .await
            .expect("stop process member");
        assert_eq!(stopped.team.status, TeamStatus::Active);
        assert_eq!(stopped.team.members[0].status, TeamMemberStatus::Stopped);
        assert_eq!(stopped.team.members[0].agent_status, AgentStatus::Shutdown);
        assert!(
            manager.captured_ops().is_empty(),
            "process member stop must not use native AgentControl shutdown"
        );
        assert!(
            registry
                .send_message(
                    SendTeamMessageRequest {
                        team_id: team.id,
                        sender_member_id: None,
                        target: SendTeamMessageTarget::Member(member.id),
                        content: "after stop".to_string(),
                        delivery_mode: TeamMessageDeliveryMode::Queue,
                        items: text_input("after stop"),
                    },
                    &agent_control,
                )
                .await
                .is_err(),
            "stopped process member should reject sends"
        );
    }

    #[test]
    fn failed_member_delivery_keeps_failed_message_record() {
        run_team_test(failed_member_delivery_keeps_failed_message_record_body);
    }

    async fn failed_member_delivery_keeps_failed_message_record_body() {
        let registry = TeamRegistry::default();
        let manager = thread_manager();
        let agent_control = manager.agent_control();
        let (_session, turn) = make_session_and_context().await;
        let team = registry
            .create_team("failed delivery".to_string(), ThreadId::new())
            .await;
        let member = registry
            .spawn_member(
                SpawnTeamMemberRequest {
                    team_id: team.id,
                    name: "member".to_string(),
                    profile: None,
                    capabilities: Vec::new(),
                    permissions: Vec::new(),
                    initial_items: text_input("initial task"),
                    config: turn.config.as_ref().clone(),
                    session_source: None,
                    environments: None,
                },
                &agent_control,
            )
            .await
            .expect("spawn member");

        remove_backing_agent(&manager, member.agent_thread_id).await;
        let send_result = registry
            .send_message(
                SendTeamMessageRequest {
                    team_id: team.id,
                    sender_member_id: None,
                    target: SendTeamMessageTarget::Member(member.id),
                    content: "lost message".to_string(),
                    delivery_mode: TeamMessageDeliveryMode::Queue,
                    items: text_input("lost message"),
                },
                &agent_control,
            )
            .await;
        assert!(
            send_result.is_err(),
            "delivery without backing agent should fail"
        );

        let messages = registry
            .list_messages(ListTeamMessagesRequest {
                team_id: team.id,
                target: Some(TeamMessageTargetFilter::Member),
                member_id: Some(member.id),
            })
            .await
            .expect("list failed member messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "lost message");
        assert_eq!(messages[0].submitted_id, None);
        assert_eq!(
            messages[0].delivery_status,
            TeamMessageDeliveryStatus::Failed
        );
    }

    #[test]
    fn stop_member_keeps_team_active_and_blocks_member_mutations() {
        run_team_test(stop_member_keeps_team_active_and_blocks_member_mutations_body);
    }

    async fn stop_member_keeps_team_active_and_blocks_member_mutations_body() {
        let registry = TeamRegistry::default();
        let manager = thread_manager();
        let agent_control = manager.agent_control();
        let (_session, turn) = make_session_and_context().await;
        let team = registry
            .create_team("member lifecycle".to_string(), ThreadId::new())
            .await;
        let member_a = registry
            .spawn_member(
                SpawnTeamMemberRequest {
                    team_id: team.id,
                    name: "member-a".to_string(),
                    profile: None,
                    capabilities: Vec::new(),
                    permissions: Vec::new(),
                    initial_items: text_input("initial task"),
                    config: turn.config.as_ref().clone(),
                    session_source: None,
                    environments: None,
                },
                &agent_control,
            )
            .await
            .expect("spawn member a");
        let member_b = registry
            .spawn_member(
                SpawnTeamMemberRequest {
                    team_id: team.id,
                    name: "member-b".to_string(),
                    profile: None,
                    capabilities: Vec::new(),
                    permissions: Vec::new(),
                    initial_items: text_input("initial task"),
                    config: turn.config.as_ref().clone(),
                    session_source: None,
                    environments: None,
                },
                &agent_control,
            )
            .await
            .expect("spawn member b");

        let stopped = Box::pin(registry.stop_member(team.id, member_a.id, &agent_control))
            .await
            .expect("stop member");
        assert_eq!(stopped.team.status, TeamStatus::Active);
        assert_eq!(stopped.team.members[0].status, TeamMemberStatus::Stopped);
        assert_eq!(stopped.team.members[0].agent_status, AgentStatus::NotFound);
        assert_eq!(stopped.team.members[1].status, TeamMemberStatus::Active);
        assert!(
            manager
                .captured_ops()
                .iter()
                .any(|(id, op)| *id == member_a.agent_thread_id && matches!(op, Op::Shutdown)),
            "single-member stop should submit shutdown to the stopped member"
        );
        assert!(
            !manager
                .captured_ops()
                .iter()
                .any(|(id, op)| *id == member_b.agent_thread_id && matches!(op, Op::Shutdown)),
            "single-member stop should not shut down other members"
        );

        let send_to_stopped = registry
            .send_message(
                SendTeamMessageRequest {
                    team_id: team.id,
                    sender_member_id: None,
                    target: SendTeamMessageTarget::Member(member_a.id),
                    content: "stopped member".to_string(),
                    delivery_mode: TeamMessageDeliveryMode::Queue,
                    items: text_input("stopped member"),
                },
                &agent_control,
            )
            .await;
        assert!(
            send_to_stopped.is_err(),
            "send to stopped member should fail"
        );

        let send_from_stopped = registry
            .send_message(
                SendTeamMessageRequest {
                    team_id: team.id,
                    sender_member_id: Some(member_a.id),
                    target: SendTeamMessageTarget::Lead,
                    content: "from stopped member".to_string(),
                    delivery_mode: TeamMessageDeliveryMode::Queue,
                    items: text_input("from stopped member"),
                },
                &agent_control,
            )
            .await;
        assert!(
            send_from_stopped.is_err(),
            "send from stopped member should fail"
        );

        let message = registry
            .send_message(
                SendTeamMessageRequest {
                    team_id: team.id,
                    sender_member_id: None,
                    target: SendTeamMessageTarget::Member(member_b.id),
                    content: "active member".to_string(),
                    delivery_mode: TeamMessageDeliveryMode::Queue,
                    items: text_input("active member"),
                },
                &agent_control,
            )
            .await
            .expect("send to active member");
        assert_eq!(message.target, TeamMessageEndpoint::Member(member_b.id));

        let task = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "claimable".to_string(),
                assignee_member_id: None,
                dependencies: Vec::new(),
                note: None,
            })
            .await
            .expect("create task");
        let stopped_claim = registry.claim_task(team.id, task.id, member_a.id).await;
        assert!(
            stopped_claim.is_err(),
            "stopped member should not claim tasks"
        );

        let status = registry
            .team_status(team.id, &agent_control)
            .await
            .expect("team status after member stop");
        assert_eq!(status.team.status, TeamStatus::Active);
        assert_eq!(status.messages, vec![message]);
        assert!(
            status.events.iter().any(
                |event| matches!(event, TeamEvent::MemberStopped { member_id, .. } if *member_id == member_a.id)
            ),
            "member stop should be observable"
        );
        assert_eq!(
            registry
                .stop_member(team.id, member_a.id, &agent_control)
                .await
                .expect("stop member is idempotent"),
            status
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
                    environments: None,
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
                    ..Default::default()
                },
            )
            .await
            .expect("update task");
        let conflicting_assignee_clear = registry
            .update_task(
                team.id,
                second.id,
                UpdateTeamTaskRequest {
                    assignee_member_id: Some(member.id),
                    clear_assignee: true,
                    ..Default::default()
                },
            )
            .await;
        assert!(
            conflicting_assignee_clear.is_err(),
            "clear_assignee should not combine with an assignee"
        );
        let conflicting_note_clear = registry
            .update_task(
                team.id,
                second.id,
                UpdateTeamTaskRequest {
                    note: Some("conflict".to_string()),
                    clear_note: true,
                    ..Default::default()
                },
            )
            .await;
        assert!(
            conflicting_note_clear.is_err(),
            "clear_note should not combine with a note"
        );
        let cleared = registry
            .update_task(
                team.id,
                second.id,
                UpdateTeamTaskRequest {
                    clear_assignee: true,
                    clear_note: true,
                    ..Default::default()
                },
            )
            .await
            .expect("clear optional task fields");
        assert_eq!(cleared.assignee_member_id, None);
        assert_eq!(cleared.note, None);
        let expected_tasks = vec![first.clone(), cleared.clone()];

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

    #[test]
    fn task_claim_respects_dependency_and_assignment_boundaries() {
        run_team_test(task_claim_respects_dependency_and_assignment_boundaries_body);
    }

    async fn task_claim_respects_dependency_and_assignment_boundaries_body() {
        let registry = TeamRegistry::default();
        let manager = thread_manager();
        let agent_control = manager.agent_control();
        let (_session, turn) = make_session_and_context().await;
        let team = registry
            .create_team("claims".to_string(), ThreadId::new())
            .await;
        let member_a = registry
            .spawn_member(
                SpawnTeamMemberRequest {
                    team_id: team.id,
                    name: "member-a".to_string(),
                    profile: None,
                    capabilities: Vec::new(),
                    permissions: Vec::new(),
                    initial_items: text_input("initial task"),
                    config: turn.config.as_ref().clone(),
                    session_source: None,
                    environments: None,
                },
                &agent_control,
            )
            .await
            .expect("spawn member a");
        let member_b = registry
            .spawn_member(
                SpawnTeamMemberRequest {
                    team_id: team.id,
                    name: "member-b".to_string(),
                    profile: None,
                    capabilities: Vec::new(),
                    permissions: Vec::new(),
                    initial_items: text_input("initial task"),
                    config: turn.config.as_ref().clone(),
                    session_source: None,
                    environments: None,
                },
                &agent_control,
            )
            .await
            .expect("spawn member b");
        let first = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "first".to_string(),
                assignee_member_id: None,
                dependencies: Vec::new(),
                note: None,
            })
            .await
            .expect("create first task");
        let second = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "second".to_string(),
                assignee_member_id: None,
                dependencies: vec![first.id],
                note: None,
            })
            .await
            .expect("create second task");
        let direct_claim = registry
            .update_task(
                team.id,
                first.id,
                UpdateTeamTaskRequest {
                    assignee_member_id: Some(member_a.id),
                    status: Some(TeamTaskStatus::Claimed),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            direct_claim.is_err(),
            "update_task should reject transitions into claimed; use team_task_claim"
        );

        let cycle_a = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "cycle-a".to_string(),
                assignee_member_id: None,
                dependencies: Vec::new(),
                note: None,
            })
            .await
            .expect("create cycle-a task");
        let cycle_b = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "cycle-b".to_string(),
                assignee_member_id: None,
                dependencies: vec![cycle_a.id],
                note: None,
            })
            .await
            .expect("create cycle-b task");
        let cyclic_update = registry
            .update_task(
                team.id,
                cycle_a.id,
                UpdateTeamTaskRequest {
                    dependencies: Some(vec![cycle_b.id]),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            cyclic_update.is_err(),
            "task dependency updates should reject cycles"
        );

        let blocked = registry.claim_task(team.id, second.id, member_a.id).await;
        assert!(
            blocked.is_err(),
            "dependency should block claiming dependent task"
        );

        let claimed_first = registry
            .claim_task(team.id, first.id, member_a.id)
            .await
            .expect("claim first task");
        let mut expected_first = first.clone();
        expected_first.assignee_member_id = Some(member_a.id);
        expected_first.status = TeamTaskStatus::Claimed;
        expected_first.updated_at = claimed_first.updated_at;
        assert_eq!(claimed_first, expected_first);
        assert_eq!(
            registry
                .claim_task(team.id, first.id, member_a.id)
                .await
                .expect("same member reclaim is idempotent"),
            claimed_first
        );

        registry
            .update_task(
                team.id,
                first.id,
                UpdateTeamTaskRequest {
                    status: Some(TeamTaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .expect("complete first task");
        let claimed_second = registry
            .claim_task(team.id, second.id, member_a.id)
            .await
            .expect("claim unblocked second task");
        let mut expected_second = second.clone();
        expected_second.assignee_member_id = Some(member_a.id);
        expected_second.status = TeamTaskStatus::Claimed;
        expected_second.updated_at = claimed_second.updated_at;
        assert_eq!(claimed_second, expected_second);
        let clear_claimed_assignee = registry
            .update_task(
                team.id,
                second.id,
                UpdateTeamTaskRequest {
                    clear_assignee: true,
                    ..Default::default()
                },
            )
            .await;
        assert!(
            clear_claimed_assignee.is_err(),
            "claimed tasks should not be clearable to an unassigned state"
        );

        let claimed_metadata_update = registry
            .update_task(
                team.id,
                second.id,
                UpdateTeamTaskRequest {
                    title: Some("second updated".to_string()),
                    assignee_member_id: Some(member_a.id),
                    dependencies: Some(vec![first.id]),
                    status: Some(TeamTaskStatus::Claimed),
                    note: Some("still claimed".to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect("claimed task metadata update with unchanged claim fields");
        assert_eq!(claimed_metadata_update.status, TeamTaskStatus::Claimed);
        assert_eq!(
            claimed_metadata_update.assignee_member_id,
            Some(member_a.id)
        );
        assert_eq!(claimed_metadata_update.dependencies, vec![first.id]);
        assert_eq!(claimed_metadata_update.title, "second updated");
        assert_eq!(
            claimed_metadata_update.note.as_deref(),
            Some("still claimed")
        );

        let reassign_claimed = registry
            .update_task(
                team.id,
                second.id,
                UpdateTeamTaskRequest {
                    assignee_member_id: Some(member_b.id),
                    status: Some(TeamTaskStatus::Claimed),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            reassign_claimed.is_err(),
            "claimed tasks should not be reassigned while remaining claimed"
        );
        assert_eq!(
            registry
                .list_tasks(team.id)
                .await
                .expect("list tasks after rejected reassignment")
                .into_iter()
                .find(|task| task.id == second.id)
                .expect("second task remains listed"),
            claimed_metadata_update
        );

        let change_claimed_dependencies = registry
            .update_task(
                team.id,
                second.id,
                UpdateTeamTaskRequest {
                    dependencies: Some(vec![cycle_a.id]),
                    status: Some(TeamTaskStatus::Claimed),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            change_claimed_dependencies.is_err(),
            "claimed tasks should not change dependencies while remaining claimed"
        );
        assert_eq!(
            registry
                .list_tasks(team.id)
                .await
                .expect("list tasks after rejected dependency change")
                .into_iter()
                .find(|task| task.id == second.id)
                .expect("second task remains listed"),
            claimed_metadata_update
        );

        let completed_with_replanned_fields = registry
            .update_task(
                team.id,
                second.id,
                UpdateTeamTaskRequest {
                    clear_assignee: true,
                    dependencies: Some(Vec::new()),
                    status: Some(TeamTaskStatus::Completed),
                    note: Some("completed after replanning".to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect("claimed task can transition out while changing claim fields");
        assert_eq!(
            completed_with_replanned_fields.status,
            TeamTaskStatus::Completed
        );
        assert_eq!(completed_with_replanned_fields.assignee_member_id, None);
        assert_eq!(
            completed_with_replanned_fields.dependencies,
            Vec::<ThreadId>::new()
        );
        assert_eq!(
            completed_with_replanned_fields.note.as_deref(),
            Some("completed after replanning")
        );

        let assigned = registry
            .create_task(CreateTeamTaskRequest {
                team_id: team.id,
                title: "assigned".to_string(),
                assignee_member_id: Some(member_a.id),
                dependencies: Vec::new(),
                note: None,
            })
            .await
            .expect("create assigned task");
        let wrong_member = registry.claim_task(team.id, assigned.id, member_b.id).await;
        assert!(
            wrong_member.is_err(),
            "task assigned to one member should reject another claimant"
        );
    }
}
