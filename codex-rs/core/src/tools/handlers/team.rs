use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::agent::next_thread_spawn_depth;
use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
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
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::multi_agents_common::build_agent_spawn_config;
use crate::tools::handlers::multi_agents_common::function_arguments;
use crate::tools::handlers::multi_agents_common::thread_spawn_source;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::team_spec;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TeamTool {
    CreateTeam,
    ListTeams,
    TeamStatus,
    TeamSpawnMember,
    TeamSend,
    TeamMessageList,
    TeamTaskCreate,
    TeamTaskUpdate,
    TeamTaskClaim,
    TeamTaskList,
    TeamEventList,
    TeamMemberStop,
    TeamStop,
}

impl TeamTool {
    const ALL: [TeamTool; 13] = [
        TeamTool::CreateTeam,
        TeamTool::ListTeams,
        TeamTool::TeamStatus,
        TeamTool::TeamSpawnMember,
        TeamTool::TeamSend,
        TeamTool::TeamMessageList,
        TeamTool::TeamTaskCreate,
        TeamTool::TeamTaskUpdate,
        TeamTool::TeamTaskClaim,
        TeamTool::TeamTaskList,
        TeamTool::TeamEventList,
        TeamTool::TeamMemberStop,
        TeamTool::TeamStop,
    ];

    fn name(self) -> &'static str {
        match self {
            TeamTool::CreateTeam => "create_team",
            TeamTool::ListTeams => "list_teams",
            TeamTool::TeamStatus => "team_status",
            TeamTool::TeamSpawnMember => "team_spawn_member",
            TeamTool::TeamSend => "team_send",
            TeamTool::TeamMessageList => "team_message_list",
            TeamTool::TeamTaskCreate => "team_task_create",
            TeamTool::TeamTaskUpdate => "team_task_update",
            TeamTool::TeamTaskClaim => "team_task_claim",
            TeamTool::TeamTaskList => "team_task_list",
            TeamTool::TeamEventList => "team_event_list",
            TeamTool::TeamMemberStop => "team_member_stop",
            TeamTool::TeamStop => "team_stop",
        }
    }

    fn spec(self) -> ToolSpec {
        match self {
            TeamTool::CreateTeam => team_spec::create_team_create_tool(),
            TeamTool::ListTeams => team_spec::create_team_list_tool(),
            TeamTool::TeamStatus => team_spec::create_team_status_tool(),
            TeamTool::TeamSpawnMember => team_spec::create_team_spawn_member_tool(),
            TeamTool::TeamSend => team_spec::create_team_send_tool(),
            TeamTool::TeamMessageList => team_spec::create_team_message_list_tool(),
            TeamTool::TeamTaskCreate => team_spec::create_team_task_create_tool(),
            TeamTool::TeamTaskUpdate => team_spec::create_team_task_update_tool(),
            TeamTool::TeamTaskClaim => team_spec::create_team_task_claim_tool(),
            TeamTool::TeamTaskList => team_spec::create_team_task_list_tool(),
            TeamTool::TeamEventList => team_spec::create_team_event_list_tool(),
            TeamTool::TeamMemberStop => team_spec::create_team_member_stop_tool(),
            TeamTool::TeamStop => team_spec::create_team_stop_tool(),
        }
    }
}

pub(crate) struct TeamHandler {
    tool: TeamTool,
}

impl TeamHandler {
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        TeamTool::ALL.into_iter().map(|tool| Self { tool })
    }

    #[cfg(test)]
    fn for_tool(tool: TeamTool) -> Self {
        Self { tool }
    }
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for TeamHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(self.tool.name())
    }

    fn spec(&self) -> ToolSpec {
        self.tool.spec()
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        handle_team_tool(self.tool, invocation)
            .await
            .map(boxed_tool_output)
    }
}

impl CoreToolRuntime for TeamHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
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

async fn handle_team_tool(
    tool: TeamTool,
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let ToolInvocation {
        session,
        turn,
        payload,
        ..
    } = invocation;
    let arguments = function_arguments(payload)?;
    match tool {
        TeamTool::CreateTeam => create_team(session, arguments).await,
        TeamTool::ListTeams => list_teams(session, arguments).await,
        TeamTool::TeamStatus => team_status(session, arguments).await,
        TeamTool::TeamSpawnMember => team_spawn_member(session, turn, arguments).await,
        TeamTool::TeamSend => team_send(session, arguments).await,
        TeamTool::TeamMessageList => team_message_list(session, arguments).await,
        TeamTool::TeamTaskCreate => team_task_create(session, arguments).await,
        TeamTool::TeamTaskUpdate => team_task_update(session, arguments).await,
        TeamTool::TeamTaskClaim => team_task_claim(session, arguments).await,
        TeamTool::TeamTaskList => team_task_list(session, arguments).await,
        TeamTool::TeamEventList => team_event_list(session, arguments).await,
        TeamTool::TeamMemberStop => team_member_stop(session, arguments).await,
        TeamTool::TeamStop => team_stop(session, arguments).await,
    }
}

async fn create_team(
    session: Arc<Session>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: CreateTeamArgs = parse_arguments(&arguments)?;
    let name = non_empty(args.name, "team name")?;
    let registry = session.services.agent_control.team_registry();
    let team = registry.create_team(name, session.thread_id).await;
    json_output(&CreateTeamResult { team }, Some(true), "create_team")
}

async fn list_teams(
    session: Arc<Session>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let _args: EmptyArgs = parse_arguments(&arguments)?;
    let registry = session.services.agent_control.team_registry();
    let teams = registry
        .list_teams()
        .await
        .into_iter()
        .filter(|team| {
            team.lead_thread_id == session.thread_id
                || team
                    .members
                    .iter()
                    .any(|member| member.agent_thread_id == session.thread_id)
        })
        .collect();
    json_output(&ListTeamsResult { teams }, Some(true), "list_teams")
}

async fn team_status(
    session: Arc<Session>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_participant(session.as_ref(), team_id).await?;
    let registry = session.services.agent_control.team_registry();
    let snapshot = team_result(
        session.as_ref(),
        Some(team_id),
        registry
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
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamSpawnMemberArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_lead(session.as_ref(), team_id, "team_spawn_member").await?;
    let name = non_empty(args.name, "member name")?;
    let capabilities = non_empty_values(args.capabilities.unwrap_or_default(), "capability")?;
    let permissions = non_empty_values(args.permissions.unwrap_or_default(), "permission")?;
    let items = parse_team_input(args.message, args.items)?;
    let child_depth = next_thread_spawn_depth(&turn.session_source);
    if exceeds_thread_spawn_depth_limit(child_depth, turn.config.agent_max_depth) {
        return Err(FunctionCallError::RespondToModel(
            "Agent depth limit reached. Solve the task yourself.".to_string(),
        ));
    }
    let config = build_agent_spawn_config(&session.get_base_instructions().await, turn.as_ref())?;
    let session_source = thread_spawn_source(
        session.thread_id,
        &turn.session_source,
        child_depth,
        /*agent_role*/ None,
        /*task_name*/ None,
    )?;
    let registry = session.services.agent_control.team_registry();
    let member = team_result(
        session.as_ref(),
        Some(team_id),
        registry
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
                    environments: Some(turn.environments.to_selections()),
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
) -> Result<FunctionToolOutput, FunctionCallError> {
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
    let items = parse_team_input(args.message, args.items)?;
    let content = input_preview(&items);
    let registry = session.services.agent_control.team_registry();
    let message = team_result(
        session.as_ref(),
        Some(team_id),
        registry
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
) -> Result<FunctionToolOutput, FunctionCallError> {
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
    let registry = session.services.agent_control.team_registry();
    let messages = team_result(
        session.as_ref(),
        Some(team_id),
        registry
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
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamTaskCreateArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_active_team_participant(session.as_ref(), team_id).await?;
    let title = non_empty(args.title, "task title")?;
    let assignee_member_id = optional_id_from_str("member", args.assignee_member_id)?;
    let dependencies = ids_from_strs("task dependency", args.dependencies.unwrap_or_default())?;
    let note = optional_non_empty(args.note, "task note")?;
    let registry = session.services.agent_control.team_registry();
    let task = team_result(
        session.as_ref(),
        Some(team_id),
        registry
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
) -> Result<FunctionToolOutput, FunctionCallError> {
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
    let registry = session.services.agent_control.team_registry();
    let task = team_result(
        session.as_ref(),
        Some(team_id),
        registry
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
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamTaskClaimArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    let task_id = id_from_str("task", &args.task_id)?;
    let member_id = id_from_str("member", &args.member_id)?;
    authorize_team_member_argument(session.as_ref(), team_id, member_id, "team_task_claim").await?;
    let registry = session.services.agent_control.team_registry();
    let task = team_result(
        session.as_ref(),
        Some(team_id),
        registry.claim_task(team_id, task_id, member_id).await,
    )
    .await?;
    json_output(&TeamTaskClaimResult { task }, Some(true), "team_task_claim")
}

async fn team_task_list(
    session: Arc<Session>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_participant(session.as_ref(), team_id).await?;
    let registry = session.services.agent_control.team_registry();
    let tasks = team_result(
        session.as_ref(),
        Some(team_id),
        registry.list_tasks(team_id).await,
    )
    .await?;
    json_output(&TeamTaskListResult { tasks }, Some(true), "team_task_list")
}

async fn team_event_list(
    session: Arc<Session>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_participant(session.as_ref(), team_id).await?;
    let registry = session.services.agent_control.team_registry();
    let events = team_result(
        session.as_ref(),
        Some(team_id),
        registry.list_events(team_id).await,
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
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamMemberIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_lead(session.as_ref(), team_id, "team_member_stop").await?;
    let member_id = id_from_str("member", &args.member_id)?;
    let registry = session.services.agent_control.team_registry();
    let snapshot = team_result(
        session.as_ref(),
        Some(team_id),
        registry
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
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_lead(session.as_ref(), team_id, "team_stop").await?;
    let registry = session.services.agent_control.team_registry();
    let snapshot = team_result(
        session.as_ref(),
        Some(team_id),
        registry
            .stop_team(team_id, &session.services.agent_control)
            .await,
    )
    .await?;
    json_output(&TeamStopResult { snapshot }, Some(true), "team_stop")
}

fn parse_team_input(
    message: Option<String>,
    items: Option<Vec<UserInput>>,
) -> Result<Vec<UserInput>, FunctionCallError> {
    match (message, items) {
        (Some(_), Some(_)) => Err(FunctionCallError::RespondToModel(
            "Provide either message or items, but not both".to_string(),
        )),
        (None, None) => Err(FunctionCallError::RespondToModel(
            "Provide one of: message or items".to_string(),
        )),
        (Some(message), None) => {
            if message.trim().is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Empty message can't be sent to an agent".to_string(),
                ));
            }
            Ok(vec![UserInput::Text {
                text: message,
                text_elements: Vec::new(),
            }])
        }
        (None, Some(items)) => {
            if items.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Items can't be empty".to_string(),
                ));
            }
            Ok(items)
        }
    }
}

fn input_preview(items: &[UserInput]) -> String {
    let op: Op = items.to_vec().into();
    crate::agent::control::render_input_preview(&op)
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
    let registry = session.services.agent_control.team_registry();
    let caller = team_result(
        session,
        Some(team_id),
        registry.caller_for_thread(team_id, session.thread_id).await,
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
    let registry = session.services.agent_control.team_registry();
    let caller = team_result(
        session,
        Some(team_id),
        registry.caller_for_thread(team_id, session.thread_id).await,
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
    let registry = session.services.agent_control.team_registry();
    let caller = team_result(
        session,
        Some(team_id),
        registry
            .active_caller_for_thread(team_id, session.thread_id)
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
    let registry = session.services.agent_control.team_registry();
    let caller = team_result(
        session,
        Some(team_id),
        registry.caller_for_thread(team_id, session.thread_id).await,
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
    let registry = session.services.agent_control.team_registry();
    let caller = team_result(
        session,
        Some(team_id),
        registry.caller_for_thread(team_id, session.thread_id).await,
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
    result: CodexResult<T>,
) -> Result<T, FunctionCallError> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => {
            let message = err.to_string();
            session
                .services
                .agent_control
                .team_registry()
                .record_failure(team_id, message)
                .await;
            Err(team_error(err))
        }
    }
}

fn team_error(err: CodexErr) -> FunctionCallError {
    match err {
        CodexErr::ThreadNotFound(id) => {
            FunctionCallError::RespondToModel(format!("team resource with id {id} not found"))
        }
        CodexErr::UnsupportedOperation(message) => FunctionCallError::RespondToModel(message),
        err => FunctionCallError::RespondToModel(format!("team tool failed: {err}")),
    }
}

fn json_output<T: Serialize>(
    value: &T,
    success: Option<bool>,
    tool_name: &str,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let content = serde_json::to_string(value).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize {tool_name} result: {err}"))
    })?;
    Ok(FunctionToolOutput::from_text(content, success))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::make_session_and_context;
    use crate::tools::context::SharedTurnDiffTracker;
    use crate::tools::context::ToolCallSource;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::models::ResponseInputItem;
    use pretty_assertions::assert_eq;
    use serde::Deserialize;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn invocation(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        tool_name: &str,
        args: serde_json::Value,
    ) -> ToolInvocation {
        ToolInvocation {
            session,
            turn,
            tracker: SharedTurnDiffTracker::default(),
            cancellation_token: CancellationToken::new(),
            call_id: "call-1".to_string(),
            tool_name: ToolName::plain(tool_name),
            source: ToolCallSource::Direct,
            payload: ToolPayload::Function {
                arguments: args.to_string(),
            },
        }
    }

    fn text_output(output: Box<dyn ToolOutput>) -> String {
        let payload = ToolPayload::Function {
            arguments: "{}".to_string(),
        };
        let ResponseInputItem::FunctionCallOutput { output, .. } =
            output.to_response_item("call-1", &payload)
        else {
            panic!("expected function output");
        };
        let FunctionCallOutputBody::Text(text) = output.body else {
            panic!("expected text function output");
        };
        text
    }

    #[derive(Debug, Deserialize)]
    struct TestCreateTeamResult {
        team: crate::team::Team,
    }

    #[derive(Debug, Deserialize)]
    struct TestListTeamsResult {
        teams: Vec<crate::team::Team>,
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
    }

    #[test]
    fn team_input_requires_exactly_one_input_source() {
        assert!(parse_team_input(Some("hi".to_string()), None).is_ok());
        assert!(
            parse_team_input(
                None,
                Some(vec![UserInput::Text {
                    text: "hi".to_string(),
                    text_elements: Vec::new(),
                }]),
            )
            .is_ok()
        );
        assert!(parse_team_input(None, None).is_err());
        assert!(parse_team_input(Some("hi".to_string()), Some(Vec::new())).is_err());
    }

    #[tokio::test]
    async fn team_create_and_list_use_agent_control_registry() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let created = TeamHandler::for_tool(TeamTool::CreateTeam)
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

        let listed = TeamHandler::for_tool(TeamTool::ListTeams)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "list_teams",
                json!({}),
            ))
            .await
            .expect("list teams");
        let listed: TestListTeamsResult =
            serde_json::from_str(&text_output(listed)).expect("list result");

        assert_eq!(listed.teams, vec![created.team]);
    }
}
