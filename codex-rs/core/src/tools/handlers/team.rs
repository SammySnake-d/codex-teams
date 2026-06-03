use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::agent::next_thread_spawn_depth;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::function_tool::FunctionCallError;
use crate::team::CreateTeamTaskRequest;
use crate::team::ListTeamMessagesRequest;
use crate::team::SendTeamMessageRequest;
use crate::team::SendTeamMessageTarget;
use crate::team::SpawnTeamMemberRequest;
use crate::team::TeamCaller;
use crate::team::TeamMessageDeliveryMode;
use crate::team::TeamMessageTargetFilter;
use crate::team::TeamTaskStatus;
use crate::team::UpdateTeamTaskRequest;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::collab;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use async_trait::async_trait;
use codex_protocol::ThreadId;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::user_input::UserInput;
use serde::Deserialize;
use serde::Serialize;
use std::sync::Arc;

pub struct TeamHandler;

#[async_trait]
impl ToolHandler for TeamHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            tool_name,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "team handler received unsupported payload".to_string(),
                ));
            }
        };

        match tool_name.as_str() {
            "create_team" => create_team(session, arguments).await,
            "list_teams" => list_teams(session, arguments).await,
            "team_status" => team_status(session, arguments).await,
            "team_spawn_member" => team_spawn_member(session, turn, arguments).await,
            "team_send" => team_send(session, arguments).await,
            "team_message_list" => team_message_list(session, arguments).await,
            "team_task_create" => team_task_create(session, arguments).await,
            "team_task_update" => team_task_update(session, arguments).await,
            "team_task_claim" => team_task_claim(session, arguments).await,
            "team_task_list" => team_task_list(session, arguments).await,
            "team_event_list" => team_event_list(session, arguments).await,
            "team_member_stop" => team_member_stop(session, arguments).await,
            "team_stop" => team_stop(session, arguments).await,
            other => Err(FunctionCallError::RespondToModel(format!(
                "unsupported team tool {other}"
            ))),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTeamArgs {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamIdArgs {
    team_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamSpawnMemberArgs {
    team_id: String,
    name: String,
    profile: Option<String>,
    capabilities: Option<Vec<String>>,
    permissions: Option<Vec<String>>,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamSendArgs {
    team_id: String,
    sender_member_id: Option<String>,
    target: Option<String>,
    member_id: Option<String>,
    delivery_mode: Option<String>,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamMessageListArgs {
    team_id: String,
    target: Option<String>,
    member_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamTaskCreateArgs {
    team_id: String,
    title: String,
    assignee_member_id: Option<String>,
    dependencies: Option<Vec<String>>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamTaskUpdateArgs {
    team_id: String,
    task_id: String,
    title: Option<String>,
    assignee_member_id: Option<String>,
    #[serde(default)]
    clear_assignee: bool,
    dependencies: Option<Vec<String>>,
    status: Option<String>,
    note: Option<String>,
    #[serde(default)]
    clear_note: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamTaskClaimArgs {
    team_id: String,
    task_id: String,
    member_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamMemberIdArgs {
    team_id: String,
    member_id: String,
}

#[derive(Debug, Serialize)]
struct CreateTeamResult {
    team: crate::team::Team,
}

#[derive(Debug, Serialize)]
struct ListTeamsResult {
    teams: Vec<crate::team::Team>,
}

#[derive(Debug, Serialize)]
struct TeamStatusResult {
    snapshot: crate::team::TeamSnapshot,
}

#[derive(Debug, Serialize)]
struct TeamSpawnMemberResult {
    member: crate::team::TeamMember,
}

#[derive(Debug, Serialize)]
struct TeamSendResult {
    message: crate::team::TeamMessage,
}

#[derive(Debug, Serialize)]
struct TeamMessageListResult {
    messages: Vec<crate::team::TeamMessage>,
}

#[derive(Debug, Serialize)]
struct TeamTaskCreateResult {
    task: crate::team::TeamTask,
}

#[derive(Debug, Serialize)]
struct TeamTaskUpdateResult {
    task: crate::team::TeamTask,
}

#[derive(Debug, Serialize)]
struct TeamTaskClaimResult {
    task: crate::team::TeamTask,
}

#[derive(Debug, Serialize)]
struct TeamTaskListResult {
    tasks: Vec<crate::team::TeamTask>,
}

#[derive(Debug, Serialize)]
struct TeamEventListResult {
    events: Vec<crate::team::TeamEvent>,
}

#[derive(Debug, Serialize)]
struct TeamMemberStopResult {
    snapshot: crate::team::TeamSnapshot,
}

#[derive(Debug, Serialize)]
struct TeamStopResult {
    snapshot: crate::team::TeamSnapshot,
}

async fn create_team(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: CreateTeamArgs = parse_arguments(&arguments)?;
    let name = non_empty(args.name, "team name")?;
    let team = session
        .services
        .team_registry
        .create_team(name, session.conversation_id)
        .await;
    json_output(&CreateTeamResult { team }, Some(true), "create_team")
}

async fn list_teams(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let _args: EmptyArgs = parse_arguments(&arguments)?;
    let teams = session
        .services
        .team_registry
        .list_teams()
        .await
        .into_iter()
        .filter(|team| {
            team.lead_thread_id == session.conversation_id
                || team
                    .members
                    .iter()
                    .any(|member| member.agent_thread_id == session.conversation_id)
        })
        .collect();
    json_output(&ListTeamsResult { teams }, Some(true), "list_teams")
}

async fn team_status(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_participant(session.as_ref(), team_id).await?;
    let snapshot = team_result(
        session.as_ref(),
        Some(team_id),
        session
            .services
            .team_registry
            .team_status(team_id, &session.services.agent_control)
            .await,
    )
    .await?;
    json_output(&TeamStatusResult { snapshot }, Some(true), "team_status")
}

async fn team_spawn_member(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamSpawnMemberArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_lead(session.as_ref(), team_id, "team_spawn_member").await?;
    let name = non_empty(args.name, "member name")?;
    let capabilities = non_empty_values(args.capabilities.unwrap_or_default(), "capability")?;
    let permissions = non_empty_values(args.permissions.unwrap_or_default(), "permission")?;
    let items = collab::parse_collab_input(args.message, args.items)?;
    let child_depth = next_thread_spawn_depth(&turn.session_source);
    if exceeds_thread_spawn_depth_limit(child_depth) {
        return Err(FunctionCallError::RespondToModel(
            "Agent depth limit reached. Solve the task yourself.".to_string(),
        ));
    }
    let config = collab::build_agent_spawn_config(
        &session.get_base_instructions().await,
        &turn,
        child_depth,
    )?;
    let session_source = collab::thread_spawn_source(session.conversation_id, child_depth);
    let member = team_result(
        session.as_ref(),
        Some(team_id),
        session
            .services
            .team_registry
            .spawn_member(
                SpawnTeamMemberRequest {
                    team_id,
                    name,
                    profile: args.profile,
                    capabilities,
                    permissions,
                    initial_items: items,
                    config,
                    session_source: Some(session_source),
                },
                &session.services.agent_control,
            )
            .await,
    )
    .await?;
    json_output(
        &TeamSpawnMemberResult { member },
        Some(true),
        "team_spawn_member",
    )
}

async fn team_send(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamSendArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    let requested_sender_member_id = optional_id_from_str("sender member", args.sender_member_id)?;
    let sender_member_id =
        authorize_team_sender(session.as_ref(), team_id, requested_sender_member_id).await?;
    let delivery_mode = match args.delivery_mode.as_deref() {
        Some("interrupt") => TeamMessageDeliveryMode::Interrupt,
        Some("queue") | None => TeamMessageDeliveryMode::Queue,
        Some(other) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "unsupported team message delivery mode {other}; use queue or interrupt"
            )));
        }
    };
    let target = match args.target.as_deref() {
        Some("lead") => {
            if args.member_id.is_some() {
                return Err(FunctionCallError::RespondToModel(
                    "member_id must be omitted when team_send target is lead".to_string(),
                ));
            }
            SendTeamMessageTarget::Lead
        }
        Some("member") | None => {
            let Some(member_id) = args.member_id else {
                return Err(FunctionCallError::RespondToModel(
                    "member_id is required when team_send target is member".to_string(),
                ));
            };
            SendTeamMessageTarget::Member(id_from_str("member", &member_id)?)
        }
        Some(other) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "unsupported team_send target {other}; use lead or member"
            )));
        }
    };
    let items = collab::parse_collab_input(args.message, args.items)?;
    let content = collab::input_preview(&items);
    let message = team_result(
        session.as_ref(),
        Some(team_id),
        session
            .services
            .team_registry
            .send_message(
                SendTeamMessageRequest {
                    team_id,
                    sender_member_id,
                    target,
                    content,
                    delivery_mode,
                    items,
                },
                &session.services.agent_control,
            )
            .await,
    )
    .await?;
    json_output(&TeamSendResult { message }, Some(true), "team_send")
}

async fn team_message_list(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamMessageListArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_participant(session.as_ref(), team_id).await?;
    let member_id = optional_id_from_str("member", args.member_id)?;
    let target = match args.target.as_deref() {
        Some("lead") => {
            if member_id.is_some() {
                return Err(FunctionCallError::RespondToModel(
                    "member_id must be omitted when team_message_list target is lead".to_string(),
                ));
            }
            Some(TeamMessageTargetFilter::Lead)
        }
        Some("member") => Some(TeamMessageTargetFilter::Member),
        Some("all") | None => None,
        Some(other) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "unsupported team_message_list target {other}; use all, lead, or member"
            )));
        }
    };
    let messages = team_result(
        session.as_ref(),
        Some(team_id),
        session
            .services
            .team_registry
            .list_messages(ListTeamMessagesRequest {
                team_id,
                target,
                member_id,
            })
            .await,
    )
    .await?;
    json_output(
        &TeamMessageListResult { messages },
        Some(true),
        "team_message_list",
    )
}

async fn team_task_create(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamTaskCreateArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_active_team_participant(session.as_ref(), team_id).await?;
    let title = non_empty(args.title, "task title")?;
    let assignee_member_id = optional_id_from_str("member", args.assignee_member_id)?;
    let dependencies = ids_from_strs("task dependency", args.dependencies.unwrap_or_default())?;
    let note = optional_non_empty(args.note, "task note")?;
    let task = team_result(
        session.as_ref(),
        Some(team_id),
        session
            .services
            .team_registry
            .create_task(CreateTeamTaskRequest {
                team_id,
                title,
                assignee_member_id,
                dependencies,
                note,
            })
            .await,
    )
    .await?;
    json_output(
        &TeamTaskCreateResult { task },
        Some(true),
        "team_task_create",
    )
}

async fn team_task_update(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamTaskUpdateArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_active_team_participant(session.as_ref(), team_id).await?;
    let task_id = id_from_str("task", &args.task_id)?;
    let title = optional_non_empty(args.title, "task title")?;
    if args.clear_assignee && args.assignee_member_id.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "clear_assignee cannot be combined with assignee_member_id".to_string(),
        ));
    }
    let assignee_member_id = optional_id_from_str("member", args.assignee_member_id)?;
    let dependencies = args
        .dependencies
        .map(|dependencies| ids_from_strs("task dependency", dependencies))
        .transpose()?;
    let status = args
        .status
        .map(|status| match status.as_str() {
            "open" => Ok(TeamTaskStatus::Open),
            "claimed" => Err(FunctionCallError::RespondToModel(
                "use team_task_claim to claim tasks".to_string(),
            )),
            "completed" => Ok(TeamTaskStatus::Completed),
            "blocked" => Ok(TeamTaskStatus::Blocked),
            other => Err(FunctionCallError::RespondToModel(format!(
                "unsupported team task status {other}; use open, completed, or blocked; use team_task_claim for claimed"
            ))),
        })
        .transpose()?;
    let note = optional_non_empty(args.note, "task note")?;
    if args.clear_note && note.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "clear_note cannot be combined with note".to_string(),
        ));
    }
    let task = team_result(
        session.as_ref(),
        Some(team_id),
        session
            .services
            .team_registry
            .update_task(
                team_id,
                task_id,
                UpdateTeamTaskRequest {
                    title,
                    assignee_member_id,
                    clear_assignee: args.clear_assignee,
                    dependencies,
                    status,
                    note,
                    clear_note: args.clear_note,
                },
            )
            .await,
    )
    .await?;
    json_output(
        &TeamTaskUpdateResult { task },
        Some(true),
        "team_task_update",
    )
}

async fn team_task_claim(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamTaskClaimArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    let task_id = id_from_str("task", &args.task_id)?;
    let member_id = id_from_str("member", &args.member_id)?;
    authorize_team_member_argument(session.as_ref(), team_id, member_id, "team_task_claim").await?;
    let task = team_result(
        session.as_ref(),
        Some(team_id),
        session
            .services
            .team_registry
            .claim_task(team_id, task_id, member_id)
            .await,
    )
    .await?;
    json_output(&TeamTaskClaimResult { task }, Some(true), "team_task_claim")
}

async fn team_task_list(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_participant(session.as_ref(), team_id).await?;
    let tasks = team_result(
        session.as_ref(),
        Some(team_id),
        session.services.team_registry.list_tasks(team_id).await,
    )
    .await?;
    json_output(&TeamTaskListResult { tasks }, Some(true), "team_task_list")
}

async fn team_event_list(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_participant(session.as_ref(), team_id).await?;
    let events = team_result(
        session.as_ref(),
        Some(team_id),
        session.services.team_registry.list_events(team_id).await,
    )
    .await?;
    json_output(
        &TeamEventListResult { events },
        Some(true),
        "team_event_list",
    )
}

async fn team_member_stop(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamMemberIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_lead(session.as_ref(), team_id, "team_member_stop").await?;
    let member_id = id_from_str("member", &args.member_id)?;
    let snapshot = team_result(
        session.as_ref(),
        Some(team_id),
        session
            .services
            .team_registry
            .stop_member(team_id, member_id, &session.services.agent_control)
            .await,
    )
    .await?;
    json_output(
        &TeamMemberStopResult { snapshot },
        Some(true),
        "team_member_stop",
    )
}

async fn team_stop(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_lead(session.as_ref(), team_id, "team_stop").await?;
    let snapshot = team_result(
        session.as_ref(),
        Some(team_id),
        session
            .services
            .team_registry
            .stop_team(team_id, &session.services.agent_control)
            .await,
    )
    .await?;
    json_output(&TeamStopResult { snapshot }, Some(true), "team_stop")
}

fn id_from_str(label: &str, id: &str) -> Result<ThreadId, FunctionCallError> {
    ThreadId::from_string(id)
        .map_err(|err| FunctionCallError::RespondToModel(format!("invalid {label} id {id}: {err}")))
}

fn non_empty(value: String, label: &str) -> Result<String, FunctionCallError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "{label} can't be empty"
        )));
    }
    Ok(trimmed.to_string())
}

fn optional_non_empty(
    value: Option<String>,
    label: &str,
) -> Result<Option<String>, FunctionCallError> {
    value.map(|value| non_empty(value, label)).transpose()
}

fn non_empty_values(values: Vec<String>, label: &str) -> Result<Vec<String>, FunctionCallError> {
    values
        .into_iter()
        .map(|value| non_empty(value, label))
        .collect()
}

fn optional_id_from_str(
    label: &str,
    id: Option<String>,
) -> Result<Option<ThreadId>, FunctionCallError> {
    id.map(|id| id_from_str(label, &id)).transpose()
}

fn ids_from_strs(label: &str, ids: Vec<String>) -> Result<Vec<ThreadId>, FunctionCallError> {
    ids.iter().map(|id| id_from_str(label, id)).collect()
}

async fn require_team_lead(
    session: &Session,
    team_id: ThreadId,
    tool_name: &str,
) -> Result<(), FunctionCallError> {
    let caller = team_result(
        session,
        Some(team_id),
        session
            .services
            .team_registry
            .caller_for_thread(team_id, session.conversation_id)
            .await,
    )
    .await?;
    if caller == TeamCaller::Lead {
        return Ok(());
    }
    Err(FunctionCallError::RespondToModel(format!(
        "{tool_name} can only be called by the team lead"
    )))
}

async fn require_team_participant(
    session: &Session,
    team_id: ThreadId,
) -> Result<(), FunctionCallError> {
    let caller = team_result(
        session,
        Some(team_id),
        session
            .services
            .team_registry
            .caller_for_thread(team_id, session.conversation_id)
            .await,
    )
    .await?;
    match caller {
        TeamCaller::Lead | TeamCaller::Member(_) => Ok(()),
        TeamCaller::Unknown => Err(FunctionCallError::RespondToModel(
            "caller is not part of this team".to_string(),
        )),
    }
}

async fn require_active_team_participant(
    session: &Session,
    team_id: ThreadId,
) -> Result<(), FunctionCallError> {
    let caller = team_result(
        session,
        Some(team_id),
        session
            .services
            .team_registry
            .active_caller_for_thread(team_id, session.conversation_id)
            .await,
    )
    .await?;
    match caller {
        TeamCaller::Lead | TeamCaller::Member(_) => Ok(()),
        TeamCaller::Unknown => Err(FunctionCallError::RespondToModel(
            "caller is not part of this team".to_string(),
        )),
    }
}

async fn authorize_team_sender(
    session: &Session,
    team_id: ThreadId,
    requested_sender_member_id: Option<ThreadId>,
) -> Result<Option<ThreadId>, FunctionCallError> {
    let caller = team_result(
        session,
        Some(team_id),
        session
            .services
            .team_registry
            .caller_for_thread(team_id, session.conversation_id)
            .await,
    )
    .await?;
    match caller {
        TeamCaller::Lead => {
            if requested_sender_member_id.is_some() {
                return Err(FunctionCallError::RespondToModel(
                    "team lead must omit sender_member_id".to_string(),
                ));
            }
            Ok(None)
        }
        TeamCaller::Member(member_id) => {
            if requested_sender_member_id != Some(member_id) {
                return Err(FunctionCallError::RespondToModel(
                    "team member callers must set sender_member_id to their own member_id"
                        .to_string(),
                ));
            }
            Ok(Some(member_id))
        }
        TeamCaller::Unknown => Err(FunctionCallError::RespondToModel(
            "caller is not part of this team".to_string(),
        )),
    }
}

async fn authorize_team_member_argument(
    session: &Session,
    team_id: ThreadId,
    member_id: ThreadId,
    tool_name: &str,
) -> Result<(), FunctionCallError> {
    let caller = team_result(
        session,
        Some(team_id),
        session
            .services
            .team_registry
            .caller_for_thread(team_id, session.conversation_id)
            .await,
    )
    .await?;
    match caller {
        TeamCaller::Lead => Ok(()),
        TeamCaller::Member(caller_member_id) if caller_member_id == member_id => Ok(()),
        TeamCaller::Member(_) => Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} member_id must match the calling team member"
        ))),
        TeamCaller::Unknown => Err(FunctionCallError::RespondToModel(
            "caller is not part of this team".to_string(),
        )),
    }
}

async fn team_result<T>(
    session: &Session,
    team_id: Option<ThreadId>,
    result: crate::error::Result<T>,
) -> Result<T, FunctionCallError> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => {
            let message = err.to_string();
            session
                .services
                .team_registry
                .record_failure(team_id, message)
                .await;
            Err(team_error(err))
        }
    }
}

fn team_error(err: crate::error::CodexErr) -> FunctionCallError {
    match err {
        crate::error::CodexErr::ThreadNotFound(id) => {
            FunctionCallError::RespondToModel(format!("team resource with id {id} not found"))
        }
        crate::error::CodexErr::UnsupportedOperation(message) => {
            FunctionCallError::RespondToModel(message)
        }
        err => FunctionCallError::RespondToModel(format!("team tool failed: {err}")),
    }
}

fn json_output<T: Serialize>(
    value: &T,
    success: Option<bool>,
    tool_name: &str,
) -> Result<ToolOutput, FunctionCallError> {
    let content = serde_json::to_string(value).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize {tool_name} result: {err}"))
    })?;
    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CodexAuth;
    use crate::ThreadManager;
    use crate::built_in_model_providers;
    use crate::codex::make_session_and_context;
    use crate::features::Feature;
    use crate::protocol::Op;
    use crate::turn_diff_tracker::TurnDiffTracker;
    use pretty_assertions::assert_eq;
    use serde::Deserialize;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn invocation(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        tool_name: &str,
        args: serde_json::Value,
    ) -> ToolInvocation {
        ToolInvocation {
            session,
            turn,
            tracker: Arc::new(Mutex::new(TurnDiffTracker::default())),
            call_id: "call-1".to_string(),
            tool_name: tool_name.to_string(),
            payload: ToolPayload::Function {
                arguments: args.to_string(),
            },
        }
    }

    fn thread_manager() -> ThreadManager {
        ThreadManager::with_models_provider_for_tests(
            CodexAuth::from_api_key("dummy"),
            built_in_model_providers()["openai"].clone(),
        )
    }

    fn expected_spawn_items(
        team: &crate::team::Team,
        member: &crate::team::TeamMember,
        prompt: &str,
    ) -> Vec<UserInput> {
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
                     Use generic Teams tools when available: team_status, team_send, team_task_list, team_task_update, team_task_claim, and team_event_list.\n\
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

    fn expected_message_items(message: &crate::team::TeamMessage, prompt: &str) -> Vec<UserInput> {
        expected_message_items_with_items(
            message,
            prompt,
            vec![UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }],
        )
    }

    fn expected_message_items_with_items(
        message: &crate::team::TeamMessage,
        prompt: &str,
        mut items: Vec<UserInput>,
    ) -> Vec<UserInput> {
        let team_id = message.team_id;
        let message_id = message.id;
        let sender_label = match &message.sender {
            crate::team::TeamMessageEndpoint::Lead(lead_thread_id) => {
                format!("lead:{lead_thread_id}")
            }
            crate::team::TeamMessageEndpoint::Member(member_id) => format!("member:{member_id}"),
        };
        let target_member_label = message
            .target_member_id
            .map(|member_id| member_id.to_string())
            .unwrap_or_else(|| "none".to_string());
        let delivery_mode_label = match &message.delivery_mode {
            crate::team::TeamMessageDeliveryMode::Queue => "queue",
            crate::team::TeamMessageDeliveryMode::Interrupt => "interrupt",
        };
        let mut wrapped = vec![UserInput::Text {
            text: format!(
                "Codex Teams message:\n\
                 - team_id: {team_id}\n\
                 - message_id: {message_id}\n\
                 - sender: {sender_label}\n\
                 - target_member_id: {target_member_label}\n\
                 - delivery_mode: {delivery_mode_label}\n\
                 \n\
                 Treat the following item(s) as a routed Teams message, not inherited conversation history.\n\
                 To reply, use team_send with this team_id and set sender_member_id to your member_id.\n\
                 Message preview:\n{prompt}"
            ),
            text_elements: Vec::new(),
        }];
        wrapped.append(&mut items);
        wrapped
    }

    fn text_output(output: ToolOutput) -> String {
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            ..
        } = output
        else {
            panic!("expected text function output");
        };
        content
    }

    fn respond_to_model_message(err: FunctionCallError) -> String {
        let FunctionCallError::RespondToModel(message) = err else {
            panic!("expected respond-to-model error");
        };
        message
    }

    fn expect_respond_to_model_error(
        result: Result<ToolOutput, FunctionCallError>,
        context: &str,
    ) -> String {
        match result {
            Err(err) => respond_to_model_message(err),
            Ok(_) => panic!("{context}"),
        }
    }

    fn sorted_team_ids(teams: &[crate::team::Team]) -> Vec<String> {
        let mut ids = teams
            .iter()
            .map(|team| team.id.to_string())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    #[derive(Debug, Deserialize)]
    struct TestListTeamsResult {
        teams: Vec<crate::team::Team>,
    }

    #[derive(Debug, Deserialize)]
    struct TestCreateTeamResult {
        team: crate::team::Team,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamSpawnMemberResult {
        member: crate::team::TeamMember,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamSendResult {
        message: crate::team::TeamMessage,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamMessageListResult {
        messages: Vec<crate::team::TeamMessage>,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamTaskCreateResult {
        task: crate::team::TeamTask,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamTaskUpdateResult {
        task: crate::team::TeamTask,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamTaskClaimResult {
        task: crate::team::TeamTask,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamTaskListResult {
        tasks: Vec<crate::team::TeamTask>,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamEventListResult {
        events: Vec<crate::team::TeamEvent>,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamMemberStopResult {
        snapshot: crate::team::TeamSnapshot,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamStatusResult {
        snapshot: crate::team::TeamSnapshot,
    }

    #[test]
    fn team_tool_argument_structs_reject_unknown_fields() {
        assert!(parse_arguments::<EmptyArgs>(r#"{"unexpected":true}"#).is_err());
        assert!(parse_arguments::<CreateTeamArgs>(r#"{"name":"x","unexpected":true}"#).is_err());
        assert!(parse_arguments::<TeamIdArgs>(r#"{"team_id":"team","unexpected":true}"#).is_err());
        assert!(
            parse_arguments::<TeamSpawnMemberArgs>(
                r#"{"team_id":"team","name":"member","message":"start","unexpected":true}"#
            )
            .is_err()
        );
        assert!(
            parse_arguments::<TeamSendArgs>(
                r#"{"team_id":"team","member_id":"member","message":"hi","unexpected":true}"#
            )
            .is_err()
        );
        assert!(
            parse_arguments::<TeamMessageListArgs>(
                r#"{"team_id":"team","target":"member","member_id":"member","unexpected":true}"#
            )
            .is_err()
        );
        assert!(
            parse_arguments::<TeamTaskCreateArgs>(
                r#"{"team_id":"team","title":"task","unexpected":true}"#
            )
            .is_err()
        );
        assert!(
            parse_arguments::<TeamTaskUpdateArgs>(
                r#"{"team_id":"team","task_id":"task","status":"open","unexpected":true}"#
            )
            .is_err()
        );
        assert!(
            parse_arguments::<TeamTaskClaimArgs>(
                r#"{"team_id":"team","task_id":"task","member_id":"member","unexpected":true}"#
            )
            .is_err()
        );
        assert!(
            parse_arguments::<TeamMemberIdArgs>(
                r#"{"team_id":"team","member_id":"member","unexpected":true}"#
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn list_teams_parses_empty_arguments_and_rejects_extras() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "list_teams",
                json!({}),
            ))
            .await
            .expect("list teams with empty args");

        let extra_arg = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "list_teams",
                json!({"unexpected": true}),
            ))
            .await;
        assert!(
            extra_arg.is_err(),
            "list_teams should reject arguments outside its empty schema"
        );
    }

    #[tokio::test]
    async fn team_tool_chain_creates_spawns_sends_statuses_and_stops() {
        let (mut session, mut turn) = make_session_and_context().await;
        let mut turn_config = turn.config.as_ref().clone();
        turn_config.features.enable(Feature::Collab);
        turn.config = Arc::new(turn_config);
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        session.services.team_registry = manager.team_registry();
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let created = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "create_team",
                json!({"name": "parallel investigation"}),
            ))
            .await
            .expect("create team");
        let created: TestCreateTeamResult =
            serde_json::from_str(&text_output(created)).expect("create result");

        let spawned = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_spawn_member",
                json!({
                    "team_id": created.team.id.to_string(),
                    "name": "teammate-a",
                    "profile": "general collaborator",
                    "capabilities": ["code search"],
                    "permissions": ["workspace read"],
                    "message": "investigate one slice"
                }),
            ))
            .await
            .expect("spawn member");
        let spawned: TestTeamSpawnMemberResult =
            serde_json::from_str(&text_output(spawned)).expect("spawn result");
        assert_eq!(spawned.member.name, "teammate-a");
        assert_eq!(spawned.member.capabilities, vec!["code search"]);
        assert_eq!(spawned.member.permissions, vec!["workspace read"]);
        let spawned_thread = manager
            .get_thread(spawned.member.agent_thread_id)
            .await
            .expect("spawned member thread");
        assert!(
            spawned_thread.enabled(Feature::Collab),
            "spawned team members must retain Teams tools"
        );

        let spawned_b = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_spawn_member",
                json!({
                    "team_id": created.team.id.to_string(),
                    "name": "teammate-b",
                    "message": "take another slice"
                }),
            ))
            .await
            .expect("spawn second member");
        let spawned_b: TestTeamSpawnMemberResult =
            serde_json::from_str(&text_output(spawned_b)).expect("second spawn result");

        let (mut member_session, _) = make_session_and_context().await;
        member_session.conversation_id = spawned.member.agent_thread_id;
        member_session.services.agent_control = manager.agent_control();
        member_session.services.team_registry = manager.team_registry();
        let member_session = Arc::new(member_session);

        let (mut unknown_session, _) = make_session_and_context().await;
        unknown_session.conversation_id = ThreadId::new();
        unknown_session.services.agent_control = manager.agent_control();
        unknown_session.services.team_registry = manager.team_registry();
        let unknown_session = Arc::new(unknown_session);

        let other_team = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "create_team",
                json!({"name": "separate team"}),
            ))
            .await
            .expect("create separate team");
        let other_team: TestCreateTeamResult =
            serde_json::from_str(&text_output(other_team)).expect("other team result");
        let other_spawned = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_spawn_member",
                json!({
                    "team_id": other_team.team.id.to_string(),
                    "name": "other-team-member",
                    "message": "stay isolated"
                }),
            ))
            .await
            .expect("spawn other team member");
        let other_spawned: TestTeamSpawnMemberResult =
            serde_json::from_str(&text_output(other_spawned)).expect("other spawn result");
        let (mut other_member_session, _) = make_session_and_context().await;
        other_member_session.conversation_id = other_spawned.member.agent_thread_id;
        other_member_session.services.agent_control = manager.agent_control();
        other_member_session.services.team_registry = manager.team_registry();
        let other_member_session = Arc::new(other_member_session);

        let lead_teams = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "list_teams",
                json!({}),
            ))
            .await
            .expect("list teams for lead");
        let lead_teams: TestListTeamsResult =
            serde_json::from_str(&text_output(lead_teams)).expect("lead list teams result");
        let mut expected_lead_team_ids =
            vec![created.team.id.to_string(), other_team.team.id.to_string()];
        expected_lead_team_ids.sort();
        assert_eq!(sorted_team_ids(&lead_teams.teams), expected_lead_team_ids);

        let member_teams = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "list_teams",
                json!({}),
            ))
            .await
            .expect("list teams for member");
        let member_teams: TestListTeamsResult =
            serde_json::from_str(&text_output(member_teams)).expect("member list teams result");
        assert_eq!(
            sorted_team_ids(&member_teams.teams),
            vec![created.team.id.to_string()]
        );

        let other_member_teams = TeamHandler
            .handle(invocation(
                Arc::clone(&other_member_session),
                Arc::clone(&turn),
                "list_teams",
                json!({}),
            ))
            .await
            .expect("list teams for other member");
        let other_member_teams: TestListTeamsResult =
            serde_json::from_str(&text_output(other_member_teams))
                .expect("other member list teams result");
        assert_eq!(
            sorted_team_ids(&other_member_teams.teams),
            vec![other_team.team.id.to_string()]
        );

        let unknown_teams = TeamHandler
            .handle(invocation(
                Arc::clone(&unknown_session),
                Arc::clone(&turn),
                "list_teams",
                json!({}),
            ))
            .await
            .expect("list teams for outsider");
        let unknown_teams: TestListTeamsResult =
            serde_json::from_str(&text_output(unknown_teams)).expect("outsider list teams result");
        assert!(
            unknown_teams.teams.is_empty(),
            "outsider should not see teams they do not participate in"
        );

        let outsider_status = expect_respond_to_model_error(
            TeamHandler
                .handle(invocation(
                    Arc::clone(&unknown_session),
                    Arc::clone(&turn),
                    "team_status",
                    json!({"team_id": created.team.id.to_string()}),
                ))
                .await,
            "outsider team_status should fail",
        );
        assert_eq!(outsider_status, "caller is not part of this team");

        let wrong_team_message_list = expect_respond_to_model_error(
            TeamHandler
                .handle(invocation(
                    Arc::clone(&other_member_session),
                    Arc::clone(&turn),
                    "team_message_list",
                    json!({"team_id": created.team.id.to_string()}),
                ))
                .await,
            "other-team member message list should fail",
        );
        assert_eq!(wrong_team_message_list, "caller is not part of this team");

        let outsider_task_list = expect_respond_to_model_error(
            TeamHandler
                .handle(invocation(
                    Arc::clone(&unknown_session),
                    Arc::clone(&turn),
                    "team_task_list",
                    json!({"team_id": created.team.id.to_string()}),
                ))
                .await,
            "outsider team_task_list should fail",
        );
        assert_eq!(outsider_task_list, "caller is not part of this team");

        let wrong_team_event_list = expect_respond_to_model_error(
            TeamHandler
                .handle(invocation(
                    Arc::clone(&other_member_session),
                    Arc::clone(&turn),
                    "team_event_list",
                    json!({"team_id": created.team.id.to_string()}),
                ))
                .await,
            "other-team member event list should fail",
        );
        assert_eq!(wrong_team_event_list, "caller is not part of this team");

        let member_spawn_member = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_spawn_member",
                json!({
                    "team_id": created.team.id.to_string(),
                    "name": "unauthorized teammate",
                    "message": "should not spawn"
                }),
            ))
            .await;
        assert!(
            member_spawn_member.is_err(),
            "member callers should not spawn teammates"
        );

        let member_stop_other = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_member_stop",
                json!({
                    "team_id": created.team.id.to_string(),
                    "member_id": spawned_b.member.id.to_string()
                }),
            ))
            .await;
        assert!(
            member_stop_other.is_err(),
            "member callers should not stop teammates"
        );

        let member_stop_team = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_stop",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await;
        assert!(
            member_stop_team.is_err(),
            "member callers should not stop teams"
        );

        let unknown_spawn_member = TeamHandler
            .handle(invocation(
                Arc::clone(&unknown_session),
                Arc::clone(&turn),
                "team_spawn_member",
                json!({
                    "team_id": created.team.id.to_string(),
                    "name": "unknown teammate",
                    "message": "should not spawn"
                }),
            ))
            .await;
        assert!(
            unknown_spawn_member.is_err(),
            "unknown callers should not spawn teammates"
        );

        let unknown_stop_member = TeamHandler
            .handle(invocation(
                Arc::clone(&unknown_session),
                Arc::clone(&turn),
                "team_member_stop",
                json!({
                    "team_id": created.team.id.to_string(),
                    "member_id": spawned_b.member.id.to_string()
                }),
            ))
            .await;
        assert!(
            unknown_stop_member.is_err(),
            "unknown callers should not stop teammates"
        );

        let unknown_stop_team = TeamHandler
            .handle(invocation(
                Arc::clone(&unknown_session),
                Arc::clone(&turn),
                "team_stop",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await;
        assert!(
            unknown_stop_team.is_err(),
            "unknown callers should not stop teams"
        );

        assert!(
            manager.captured_ops().contains(&(
                spawned.member.agent_thread_id,
                Op::UserInput {
                    items: expected_spawn_items(
                        &created.team,
                        &spawned.member,
                        "investigate one slice"
                    ),
                    final_output_json_schema: None,
                },
            )),
            "team spawn should submit initial member input"
        );

        let sent = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "member_id": spawned.member.id.to_string(),
                    "delivery_mode": "interrupt",
                    "message": "send a progress update"
                }),
            ))
            .await
            .expect("send member message");
        let sent: TestTeamSendResult =
            serde_json::from_str(&text_output(sent)).expect("send result");
        assert_eq!(sent.message.content, "send a progress update");
        assert_eq!(
            sent.message.delivery_mode,
            crate::team::TeamMessageDeliveryMode::Interrupt
        );
        let captured_ops = manager.captured_ops();
        let ops_for_member: Vec<&Op> = captured_ops
            .iter()
            .filter_map(|(id, op)| (*id == spawned.member.agent_thread_id).then_some(op))
            .collect();
        assert!(
            ops_for_member.iter().any(|op| matches!(op, Op::Interrupt)),
            "interrupt delivery mode should interrupt before message submission"
        );
        assert!(
            manager.captured_ops().contains(&(
                spawned.member.agent_thread_id,
                Op::UserInput {
                    items: expected_message_items(&sent.message, "send a progress update"),
                    final_output_json_schema: None,
                },
            )),
            "team_send should submit member messages with a Teams envelope"
        );

        let member_sent = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "sender_member_id": spawned.member.id.to_string(),
                    "member_id": spawned_b.member.id.to_string(),
                    "message": "handoff from member a"
                }),
            ))
            .await
            .expect("send member-originated message");
        let member_sent: TestTeamSendResult =
            serde_json::from_str(&text_output(member_sent)).expect("member send result");
        assert_eq!(
            member_sent.message.sender,
            crate::team::TeamMessageEndpoint::Member(spawned.member.id)
        );
        assert_eq!(
            member_sent.message.target,
            crate::team::TeamMessageEndpoint::Member(spawned_b.member.id)
        );
        assert_eq!(
            member_sent.message.target_member_id,
            Some(spawned_b.member.id)
        );

        let member_omitted_sender = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "member_id": spawned_b.member.id.to_string(),
                    "message": "pretend to be lead"
                }),
            ))
            .await;
        assert!(
            member_omitted_sender.is_err(),
            "member callers should not omit sender_member_id"
        );

        let member_spoofed_sender = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "sender_member_id": spawned_b.member.id.to_string(),
                    "member_id": spawned_b.member.id.to_string(),
                    "message": "pretend to be member b"
                }),
            ))
            .await;
        assert!(
            member_spoofed_sender.is_err(),
            "member callers should not use another member's sender_member_id"
        );

        let lead_spoofed_sender = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "sender_member_id": spawned.member.id.to_string(),
                    "member_id": spawned_b.member.id.to_string(),
                    "message": "lead should not proxy member sender"
                }),
            ))
            .await;
        assert!(
            lead_spoofed_sender.is_err(),
            "lead callers should omit sender_member_id"
        );

        let structured_items = vec![
            UserInput::Text {
                text: "structured follow up".to_string(),
                text_elements: Vec::new(),
            },
            UserInput::Image {
                image_url: "data:image/png;base64,BBBB".to_string(),
            },
        ];
        let structured_sent = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "member_id": spawned_b.member.id.to_string(),
                    "items": [
                        {"type": "text", "text": "structured follow up"},
                        {"type": "image", "image_url": "data:image/png;base64,BBBB"}
                    ]
                }),
            ))
            .await
            .expect("send structured member message");
        let structured_sent: TestTeamSendResult =
            serde_json::from_str(&text_output(structured_sent)).expect("structured send result");
        assert_eq!(
            structured_sent.message.content,
            "structured follow up\n[image]"
        );
        assert_eq!(structured_sent.message.items, structured_items);
        assert!(
            manager.captured_ops().contains(&(
                spawned_b.member.agent_thread_id,
                Op::UserInput {
                    items: expected_message_items_with_items(
                        &structured_sent.message,
                        "structured follow up\n[image]",
                        structured_sent.message.items.clone(),
                    ),
                    final_output_json_schema: None,
                },
            )),
            "team_send should preserve structured items after the Teams envelope"
        );

        let lead_sent = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "sender_member_id": spawned.member.id.to_string(),
                    "target": "lead",
                    "message": "report to lead"
                }),
            ))
            .await
            .expect("send member-originated lead message");
        let lead_sent: TestTeamSendResult =
            serde_json::from_str(&text_output(lead_sent)).expect("lead send result");
        assert_eq!(
            lead_sent.message.target,
            crate::team::TeamMessageEndpoint::Lead(created.team.lead_thread_id)
        );
        assert_eq!(lead_sent.message.target_member_id, None);
        assert_eq!(
            lead_sent.message.items,
            vec![UserInput::Text {
                text: "report to lead".to_string(),
                text_elements: Vec::new(),
            }]
        );
        assert_eq!(lead_sent.message.submitted_id, None);

        let lead_messages = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_message_list",
                json!({
                    "team_id": created.team.id.to_string(),
                    "target": "lead"
                }),
            ))
            .await
            .expect("list lead messages");
        let lead_messages: TestTeamMessageListResult =
            serde_json::from_str(&text_output(lead_messages)).expect("lead messages result");
        assert_eq!(lead_messages.messages, vec![lead_sent.message.clone()]);

        let member_messages = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_message_list",
                json!({
                    "team_id": created.team.id.to_string(),
                    "target": "member",
                    "member_id": spawned_b.member.id.to_string()
                }),
            ))
            .await
            .expect("list member messages");
        let member_messages: TestTeamMessageListResult =
            serde_json::from_str(&text_output(member_messages)).expect("member messages result");
        assert_eq!(
            member_messages.messages,
            vec![member_sent.message.clone(), structured_sent.message.clone()]
        );

        let bad_message_target = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_message_list",
                json!({
                    "team_id": created.team.id.to_string(),
                    "target": "urgent"
                }),
            ))
            .await;
        assert!(
            bad_message_target.is_err(),
            "unknown message target should fail"
        );

        let bad_lead_filter = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_message_list",
                json!({
                    "team_id": created.team.id.to_string(),
                    "target": "lead",
                    "member_id": spawned.member.id.to_string()
                }),
            ))
            .await;
        assert!(
            bad_lead_filter.is_err(),
            "lead message target should not take member_id"
        );

        let bad_lead_interrupt = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "target": "lead",
                    "delivery_mode": "interrupt",
                    "message": "urgent lead report"
                }),
            ))
            .await;
        assert!(
            bad_lead_interrupt.is_err(),
            "lead mailbox target should reject interrupt delivery"
        );

        let bad_delivery_mode = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "member_id": spawned.member.id.to_string(),
                    "delivery_mode": "now",
                    "message": "invalid delivery mode"
                }),
            ))
            .await;
        assert!(
            bad_delivery_mode.is_err(),
            "unsupported delivery mode should fail"
        );

        let bad_sender = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "sender_member_id": ThreadId::new().to_string(),
                    "member_id": spawned_b.member.id.to_string(),
                    "message": "invalid sender"
                }),
            ))
            .await;
        assert!(bad_sender.is_err(), "unknown sender member should fail");

        let unknown_task_create = TeamHandler
            .handle(invocation(
                Arc::clone(&unknown_session),
                Arc::clone(&turn),
                "team_task_create",
                json!({
                    "team_id": created.team.id.to_string(),
                    "title": "unauthorized task"
                }),
            ))
            .await;
        assert!(
            unknown_task_create.is_err(),
            "unknown callers should not create team tasks"
        );

        let member_created_task = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_task_create",
                json!({
                    "team_id": created.team.id.to_string(),
                    "title": "member-created task"
                }),
            ))
            .await
            .expect("member creates task");
        let member_created_task: TestTeamTaskCreateResult =
            serde_json::from_str(&text_output(member_created_task)).expect("member task result");
        assert_eq!(member_created_task.task.title, "member-created task");

        let member_updated_task = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": member_created_task.task.id.to_string(),
                    "title": "member-updated task",
                    "note": "member edit"
                }),
            ))
            .await
            .expect("member updates task");
        let member_updated_task: TestTeamTaskUpdateResult =
            serde_json::from_str(&text_output(member_updated_task)).expect("member update result");
        assert_eq!(member_updated_task.task.title, "member-updated task");
        assert_eq!(
            member_updated_task.task.note.as_deref(),
            Some("member edit")
        );

        let created_task = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_create",
                json!({
                    "team_id": created.team.id.to_string(),
                    "title": "investigate one slice",
                    "assignee_member_id": spawned.member.id.to_string(),
                    "note": "shared task board entry"
                }),
            ))
            .await
            .expect("create task");
        let created_task: TestTeamTaskCreateResult =
            serde_json::from_str(&text_output(created_task)).expect("create task result");
        assert_eq!(
            created_task.task.assignee_member_id,
            Some(spawned.member.id)
        );

        let unknown_task_claim = TeamHandler
            .handle(invocation(
                Arc::clone(&unknown_session),
                Arc::clone(&turn),
                "team_task_claim",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "member_id": spawned.member.id.to_string()
                }),
            ))
            .await;
        assert!(
            unknown_task_claim.is_err(),
            "unknown callers should not claim team tasks"
        );

        let member_claim_for_other = TeamHandler
            .handle(invocation(
                Arc::clone(&member_session),
                Arc::clone(&turn),
                "team_task_claim",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "member_id": spawned_b.member.id.to_string()
                }),
            ))
            .await;
        assert!(
            member_claim_for_other.is_err(),
            "member callers should not claim tasks for another member"
        );

        let claimed_task = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_claim",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "member_id": spawned.member.id.to_string()
                }),
            ))
            .await
            .expect("claim task");
        let claimed_task: TestTeamTaskClaimResult =
            serde_json::from_str(&text_output(claimed_task)).expect("claim task result");
        assert_eq!(
            claimed_task.task.assignee_member_id,
            Some(spawned.member.id)
        );
        assert_eq!(
            claimed_task.task.status,
            crate::team::TeamTaskStatus::Claimed
        );

        let unknown_task_update = TeamHandler
            .handle(invocation(
                Arc::clone(&unknown_session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "status": "completed"
                }),
            ))
            .await;
        assert!(
            unknown_task_update.is_err(),
            "unknown callers should not update team tasks"
        );

        let other_team_task_update = TeamHandler
            .handle(invocation(
                Arc::clone(&other_member_session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "status": "completed"
                }),
            ))
            .await;
        assert!(
            other_team_task_update.is_err(),
            "members of another team should not update this team's tasks"
        );

        let bad_dependency = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_create",
                json!({
                    "team_id": created.team.id.to_string(),
                    "title": "bad dependency",
                    "dependencies": [ThreadId::new().to_string()]
                }),
            ))
            .await;
        assert!(bad_dependency.is_err(), "unknown dependency should fail");

        let updated_task = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "status": "completed",
                    "note": "ready"
                }),
            ))
            .await
            .expect("update task");
        let updated_task: TestTeamTaskUpdateResult =
            serde_json::from_str(&text_output(updated_task)).expect("update task result");
        assert_eq!(
            updated_task.task.status,
            crate::team::TeamTaskStatus::Completed
        );

        let cleared_task = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "clear_assignee": true,
                    "clear_note": true
                }),
            ))
            .await
            .expect("clear optional task fields");
        let cleared_task: TestTeamTaskUpdateResult =
            serde_json::from_str(&text_output(cleared_task)).expect("clear task result");
        assert_eq!(cleared_task.task.assignee_member_id, None);
        assert_eq!(cleared_task.task.note, None);

        let bad_clear_assignee = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "assignee_member_id": spawned.member.id.to_string(),
                    "clear_assignee": true
                }),
            ))
            .await;
        assert!(
            bad_clear_assignee.is_err(),
            "clear_assignee should not combine with assignee_member_id"
        );

        let bad_clear_note = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "note": "conflict",
                    "clear_note": true
                }),
            ))
            .await;
        assert!(
            bad_clear_note.is_err(),
            "clear_note should not combine with note"
        );

        let claimed_status = expect_respond_to_model_error(
            TeamHandler
                .handle(invocation(
                    Arc::clone(&session),
                    Arc::clone(&turn),
                    "team_task_update",
                    json!({
                        "team_id": created.team.id.to_string(),
                        "task_id": created_task.task.id.to_string(),
                        "status": "claimed"
                    }),
                ))
                .await,
            "claimed task update should fail",
        );
        assert_eq!(claimed_status, "use team_task_claim to claim tasks");

        let bad_status = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "status": "reviewed"
                }),
            ))
            .await;
        assert!(bad_status.is_err(), "unsupported task status should fail");

        let self_dependency = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": created_task.task.id.to_string(),
                    "dependencies": [created_task.task.id.to_string()]
                }),
            ))
            .await;
        assert!(self_dependency.is_err(), "self dependency should fail");

        let listed_tasks = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_list",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await
            .expect("list tasks");
        let listed_tasks: TestTeamTaskListResult =
            serde_json::from_str(&text_output(listed_tasks)).expect("list tasks result");
        assert_eq!(
            listed_tasks.tasks,
            vec![member_updated_task.task.clone(), cleared_task.task.clone()]
        );

        let listed_events = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_event_list",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await
            .expect("list events");
        let listed_events: TestTeamEventListResult =
            serde_json::from_str(&text_output(listed_events)).expect("list events result");
        assert!(
            listed_events.events.iter().any(
                |event| matches!(event, crate::team::TeamEvent::TaskUpdated { task_id, .. } if *task_id == updated_task.task.id)
            ),
            "team_event_list should expose task updates"
        );

        let status = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_status",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await
            .expect("team status");
        let status: TestTeamStatusResult =
            serde_json::from_str(&text_output(status)).expect("status result");
        assert_eq!(status.snapshot.team.members.len(), 2);
        assert_eq!(status.snapshot.team.members[0].id, spawned.member.id);
        assert_eq!(status.snapshot.team.members[0].name, spawned.member.name);
        assert_eq!(status.snapshot.team.members[1].id, spawned_b.member.id);
        assert_eq!(status.snapshot.team.members[1].name, spawned_b.member.name);
        assert_eq!(
            status.snapshot.messages,
            vec![
                sent.message,
                member_sent.message,
                structured_sent.message,
                lead_sent.message,
            ]
        );
        assert_eq!(
            status.snapshot.team.tasks,
            vec![member_updated_task.task, cleared_task.task]
        );
        assert!(
            status
                .snapshot
                .events
                .iter()
                .any(|event| matches!(event, crate::team::TeamEvent::TaskCreated { .. })),
            "team_status should include task events"
        );

        let member_stopped = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_member_stop",
                json!({
                    "team_id": created.team.id.to_string(),
                    "member_id": spawned.member.id.to_string()
                }),
            ))
            .await
            .expect("stop one member");
        let member_stopped: TestTeamMemberStopResult =
            serde_json::from_str(&text_output(member_stopped)).expect("member stop result");
        assert_eq!(
            member_stopped.snapshot.team.status,
            crate::team::TeamStatus::Active
        );
        assert_eq!(
            member_stopped.snapshot.team.members[0].status,
            crate::team::TeamMemberStatus::Stopped
        );
        assert_eq!(
            member_stopped.snapshot.team.members[1].status,
            crate::team::TeamMemberStatus::Active
        );
        assert!(
            member_stopped.snapshot.events.iter().any(|event| matches!(
                event,
                crate::team::TeamEvent::MemberStopped { member_id, .. }
                    if *member_id == spawned.member.id
            )),
            "team_member_stop should be observable"
        );
        assert!(
            manager
                .captured_ops()
                .iter()
                .any(|(id, op)| *id == spawned.member.agent_thread_id && matches!(op, Op::Shutdown)),
            "team_member_stop should submit shutdown"
        );

        let send_to_stopped_member = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "member_id": spawned.member.id.to_string(),
                    "message": "after stop"
                }),
            ))
            .await;
        assert!(
            send_to_stopped_member.is_err(),
            "team_send to stopped member should fail"
        );

        let stopped_member_task_create = expect_respond_to_model_error(
            TeamHandler
                .handle(invocation(
                    Arc::clone(&member_session),
                    Arc::clone(&turn),
                    "team_task_create",
                    json!({
                        "team_id": created.team.id.to_string(),
                        "title": "stopped member task"
                    }),
                ))
                .await,
            "stopped member task create should fail",
        );
        assert_eq!(
            stopped_member_task_create,
            format!("team member {} is stopped", spawned.member.id)
        );

        let stopped_member_task_update = expect_respond_to_model_error(
            TeamHandler
                .handle(invocation(
                    Arc::clone(&member_session),
                    Arc::clone(&turn),
                    "team_task_update",
                    json!({
                        "team_id": created.team.id.to_string(),
                        "task_id": member_created_task.task.id.to_string(),
                        "status": "open"
                    }),
                ))
                .await,
            "stopped member task update should fail",
        );
        assert_eq!(
            stopped_member_task_update,
            format!("team member {} is stopped", spawned.member.id)
        );

        let send_to_active_member = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "member_id": spawned_b.member.id.to_string(),
                    "message": "still active"
                }),
            ))
            .await
            .expect("send to active member after stopping one member");
        let send_to_active_member: TestTeamSendResult =
            serde_json::from_str(&text_output(send_to_active_member)).expect("active send result");
        assert_eq!(
            send_to_active_member.message.target,
            crate::team::TeamMessageEndpoint::Member(spawned_b.member.id)
        );

        let failed_update = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_task_update",
                json!({
                    "team_id": created.team.id.to_string(),
                    "task_id": spawned.member.id.to_string(),
                    "status": "open"
                }),
            ))
            .await;
        assert!(failed_update.is_err(), "missing task update should fail");
        let listed_events = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_event_list",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await
            .expect("list events after failure");
        let listed_events: TestTeamEventListResult =
            serde_json::from_str(&text_output(listed_events)).expect("events after failure");
        assert!(
            listed_events
                .events
                .iter()
                .any(|event| matches!(event, crate::team::TeamEvent::Failure { .. })),
            "team_event_list should expose recorded failures"
        );
        let failure_count_before_stop = listed_events
            .events
            .iter()
            .filter(|event| matches!(event, crate::team::TeamEvent::Failure { .. }))
            .count();

        TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_stop",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await
            .expect("stop team");
        assert!(
            manager
                .captured_ops()
                .iter()
                .any(|(id, op)| *id == spawned_b.member.agent_thread_id
                    && matches!(op, Op::Shutdown)),
            "team stop should submit shutdown for remaining active members"
        );

        let repeated_stop = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_stop",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await;
        assert!(
            repeated_stop.is_err(),
            "team_stop should reject already stopped teams"
        );

        let stopped_status = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_status",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await
            .expect("stopped team status remains readable");
        let stopped_status: TestTeamStatusResult =
            serde_json::from_str(&text_output(stopped_status)).expect("stopped status result");
        assert_eq!(
            stopped_status.snapshot.team.status,
            crate::team::TeamStatus::Stopped
        );

        let stopped_events = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_event_list",
                json!({"team_id": created.team.id.to_string()}),
            ))
            .await
            .expect("stopped team events remain readable");
        let stopped_events: TestTeamEventListResult =
            serde_json::from_str(&text_output(stopped_events)).expect("stopped events result");
        assert!(
            stopped_events
                .events
                .iter()
                .filter(|event| matches!(event, crate::team::TeamEvent::Failure { .. }))
                .count()
                > failure_count_before_stop,
            "rejected repeated team_stop should be recorded as a failure event"
        );
    }
}
