use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::agent::next_thread_spawn_depth;
use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::team::CreateTeamTaskRequest;
use crate::team::ListTeamMessagesRequest;
use crate::team::SendTeamMessageRequest;
use crate::team::SendTeamMessageTarget;
use crate::team::TeamCaller;
use crate::team::TeamMessageDeliveryMode;
use crate::team::TeamMessageTargetFilter;
use crate::team::TeamTaskStatus;
use crate::team::UpdateTeamTaskRequest;
use crate::team_backends::iterm::ITermBackend;
use crate::team_backends::iterm::{self};
use crate::team_backends::spawn;
use crate::team_backends::tmux::TmuxBackend;
use crate::team_coord;
use crate::team_store;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
#[cfg(test)]
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::multi_agents_common::function_arguments;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::team_spec;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStackOrdering;
use codex_features::Feature;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ModeKind;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::user_input::UserInput;
use codex_tools::ToolExecutorFuture;
use codex_tools::ToolName;
use codex_tools::ToolSearchInfo;
use codex_tools::ToolSearchSourceInfo;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn teams_root_for_turn(turn: &TurnContext) -> PathBuf {
    team_store::root_from_env_or(turn.config.codex_home.as_path())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TeamTool {
    CreateTeam,
    ClaudeTeamCreate,
    ListTeams,
    TeamStatus,
    TeamSpawnMember,
    TeamSend,
    ClaudeSendMessage,
    TeamMessageList,
    TeamTaskCreate,
    ClaudeTaskCreate,
    TeamTaskUpdate,
    ClaudeTaskUpdate,
    TeamTaskClaim,
    TeamTaskList,
    ClaudeTaskList,
    ClaudeTaskGet,
    TeamEventList,
    TeamMemberStop,
    TeamStop,
}

impl TeamTool {
    const ALL: [TeamTool; 19] = [
        TeamTool::CreateTeam,
        TeamTool::ClaudeTeamCreate,
        TeamTool::ListTeams,
        TeamTool::TeamStatus,
        TeamTool::TeamSpawnMember,
        TeamTool::TeamSend,
        TeamTool::ClaudeSendMessage,
        TeamTool::TeamMessageList,
        TeamTool::TeamTaskCreate,
        TeamTool::ClaudeTaskCreate,
        TeamTool::TeamTaskUpdate,
        TeamTool::ClaudeTaskUpdate,
        TeamTool::TeamTaskClaim,
        TeamTool::TeamTaskList,
        TeamTool::ClaudeTaskList,
        TeamTool::ClaudeTaskGet,
        TeamTool::TeamEventList,
        TeamTool::TeamMemberStop,
        TeamTool::TeamStop,
    ];

    fn name(self) -> &'static str {
        match self {
            TeamTool::CreateTeam => "create_team",
            TeamTool::ClaudeTeamCreate => "TeamCreate",
            TeamTool::ListTeams => "list_teams",
            TeamTool::TeamStatus => "team_status",
            TeamTool::TeamSpawnMember => "team_spawn_member",
            TeamTool::TeamSend => "team_send",
            TeamTool::ClaudeSendMessage => "SendMessage",
            TeamTool::TeamMessageList => "team_message_list",
            TeamTool::TeamTaskCreate => "team_task_create",
            TeamTool::ClaudeTaskCreate => "TaskCreate",
            TeamTool::TeamTaskUpdate => "team_task_update",
            TeamTool::ClaudeTaskUpdate => "TaskUpdate",
            TeamTool::TeamTaskClaim => "team_task_claim",
            TeamTool::TeamTaskList => "team_task_list",
            TeamTool::ClaudeTaskList => "TaskList",
            TeamTool::ClaudeTaskGet => "TaskGet",
            TeamTool::TeamEventList => "team_event_list",
            TeamTool::TeamMemberStop => "team_member_stop",
            TeamTool::TeamStop => "team_stop",
        }
    }

    fn allowed_in_teammate_process(self) -> bool {
        matches!(
            self,
            TeamTool::TeamSend
                | TeamTool::ClaudeSendMessage
                | TeamTool::TeamTaskCreate
                | TeamTool::ClaudeTaskCreate
                | TeamTool::TeamTaskUpdate
                | TeamTool::ClaudeTaskUpdate
                | TeamTool::TeamTaskClaim
                | TeamTool::TeamTaskList
                | TeamTool::ClaudeTaskList
                | TeamTool::ClaudeTaskGet
        )
    }

    fn spec(self) -> ToolSpec {
        match self {
            TeamTool::CreateTeam => team_spec::create_team_create_tool(),
            TeamTool::ClaudeTeamCreate => team_spec::create_claude_team_create_tool(),
            TeamTool::ListTeams => team_spec::create_team_list_tool(),
            TeamTool::TeamStatus => team_spec::create_team_status_tool(),
            TeamTool::TeamSpawnMember => team_spec::create_team_spawn_member_tool(),
            TeamTool::TeamSend => team_spec::create_team_send_tool(),
            TeamTool::ClaudeSendMessage => team_spec::create_claude_send_message_tool(),
            TeamTool::TeamMessageList => team_spec::create_team_message_list_tool(),
            TeamTool::TeamTaskCreate => team_spec::create_team_task_create_tool(),
            TeamTool::ClaudeTaskCreate => team_spec::create_claude_task_create_tool(),
            TeamTool::TeamTaskUpdate => team_spec::create_team_task_update_tool(),
            TeamTool::ClaudeTaskUpdate => team_spec::create_claude_task_update_tool(),
            TeamTool::TeamTaskClaim => team_spec::create_team_task_claim_tool(),
            TeamTool::TeamTaskList => team_spec::create_team_task_list_tool(),
            TeamTool::ClaudeTaskList => team_spec::create_claude_task_list_tool(),
            TeamTool::ClaudeTaskGet => team_spec::create_claude_task_get_tool(),
            TeamTool::TeamEventList => team_spec::create_team_event_list_tool(),
            TeamTool::TeamMemberStop => team_spec::create_team_member_stop_tool(),
            TeamTool::TeamStop => team_spec::create_team_stop_tool(),
        }
    }

    fn search_hint(self) -> &'static str {
        const TEAM_CREATE_SEARCH: &str =
            "codex teams team teammate teammates swarm 团队 队友 create_team TeamCreate";
        const TEAM_MESSAGE_SEARCH: &str = "codex teams team teammate teammates 队友 message mailbox inbox team_send SendMessage team_message_list";
        const TEAM_TASK_SEARCH: &str = "codex teams team task tasks taskboard team_task_create TaskCreate team_task_update TaskUpdate team_task_claim team_task_list TaskList TaskGet";
        const TEAM_STATUS_SEARCH: &str =
            "codex teams team status roster list_teams team_status team_event_list";
        const TEAM_STOP_SEARCH: &str = "codex teams team stop shutdown team_member_stop team_stop";
        match self {
            TeamTool::CreateTeam | TeamTool::ClaudeTeamCreate => TEAM_CREATE_SEARCH,
            TeamTool::TeamSpawnMember => "team_spawn_member",
            TeamTool::TeamSend | TeamTool::ClaudeSendMessage | TeamTool::TeamMessageList => {
                TEAM_MESSAGE_SEARCH
            }
            TeamTool::TeamTaskCreate
            | TeamTool::ClaudeTaskCreate
            | TeamTool::TeamTaskUpdate
            | TeamTool::ClaudeTaskUpdate
            | TeamTool::TeamTaskClaim
            | TeamTool::TeamTaskList
            | TeamTool::ClaudeTaskList
            | TeamTool::ClaudeTaskGet => TEAM_TASK_SEARCH,
            TeamTool::ListTeams | TeamTool::TeamStatus | TeamTool::TeamEventList => {
                TEAM_STATUS_SEARCH
            }
            TeamTool::TeamMemberStop | TeamTool::TeamStop => TEAM_STOP_SEARCH,
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

    pub(crate) fn for_teammate_process() -> impl Iterator<Item = Self> {
        TeamTool::ALL
            .into_iter()
            .filter(|tool| tool.allowed_in_teammate_process())
            .map(|tool| Self { tool })
    }

    #[cfg(test)]
    fn for_tool(tool: TeamTool) -> Self {
        Self { tool }
    }
}

impl ToolExecutor<ToolInvocation> for TeamHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(self.tool.name())
    }

    fn spec(&self) -> ToolSpec {
        self.tool.spec()
    }

    fn search_info(&self) -> Option<ToolSearchInfo> {
        ToolSearchInfo::from_spec(
            self.tool.search_hint().to_string(),
            self.spec(),
            Some(ToolSearchSourceInfo {
                name: "Codex Teams tools".to_string(),
                description: Some(
                    "Create and manage explicit Codex Teams workspaces, teammates, mailboxes, task boards, and split-pane sessions."
                        .to_string(),
                ),
            }),
        )
    }

    fn handle(&self, invocation: ToolInvocation) -> ToolExecutorFuture<'_> {
        Box::pin(async move {
            handle_team_tool(self.tool, invocation)
                .await
                .map(boxed_tool_output)
        })
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
    team_name: Option<String>,
    description: Option<String>,
    agent_type: Option<String>,
    name: Option<String>,
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

pub(crate) struct SpawnMemberFromAgentToolRequest {
    pub(crate) team_name: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) profile: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) mode: Option<ModeKind>,
    pub(crate) message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamSendArgs {
    team_id: Option<String>,
    to: Option<String>,
    summary: Option<String>,
    sender_member_id: Option<String>,
    target: Option<String>,
    member_id: Option<String>,
    member_name: Option<String>,
    delivery_mode: Option<String>,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
}

fn normalize_team_send_args(mut args: TeamSendArgs) -> Result<TeamSendArgs, FunctionCallError> {
    let Some(raw_to) = args.to.as_deref() else {
        return Ok(args);
    };
    let to = raw_to.trim();
    if to.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "to must not be empty".to_string(),
        ));
    }
    if to.contains('@') {
        return Err(FunctionCallError::RespondToModel(
            "to must be a bare teammate name or \"*\" — there is only one team per session"
                .to_string(),
        ));
    }
    if args.message.is_some()
        && args
            .summary
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    {
        return Err(FunctionCallError::RespondToModel(
            "summary is required when message is a string".to_string(),
        ));
    }
    if args.target.is_some() || args.member_id.is_some() || args.member_name.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "use either Claude-style team_send.to or legacy target/member_id/member_name fields, not both"
                .to_string(),
        ));
    }
    if to == "*" {
        args.target = Some("broadcast".to_string());
    } else if to.eq_ignore_ascii_case(team_store::TEAM_LEAD_NAME) || to.eq_ignore_ascii_case("lead")
    {
        args.target = Some("lead".to_string());
    } else {
        args.target = Some("member".to_string());
        args.member_name = Some(to.to_string());
    }
    args.to = None;
    Ok(args)
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
    team_id: Option<String>,
    title: String,
    assignee_member_id: Option<String>,
    dependencies: Option<Vec<String>>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamTaskUpdateArgs {
    team_id: Option<String>,
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
    team_id: Option<String>,
    task_id: String,
    member_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamTaskListArgs {
    team_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeSendMessageArgs {
    to: String,
    summary: Option<String>,
    message: ClaudeSendMessageContent,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ClaudeSendMessageContent {
    Text(String),
    Structured(ClaudeStructuredMessage),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeStructuredMessage {
    ShutdownRequest {
        reason: Option<String>,
    },
    ShutdownResponse {
        request_id: String,
        approve: bool,
        reason: Option<String>,
    },
    PlanApprovalResponse {
        request_id: String,
        approve: bool,
        feedback: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeTaskCreateArgs {
    subject: String,
    description: String,
    #[serde(rename = "activeForm")]
    active_form: Option<String>,
    metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeTaskUpdateArgs {
    #[serde(rename = "taskId")]
    task_id: String,
    subject: Option<String>,
    description: Option<String>,
    #[serde(rename = "activeForm")]
    active_form: Option<String>,
    status: Option<String>,
    #[serde(rename = "addBlocks")]
    add_blocks: Option<Vec<String>>,
    #[serde(rename = "addBlockedBy")]
    add_blocked_by: Option<Vec<String>>,
    owner: Option<String>,
    metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeTaskGetArgs {
    #[serde(rename = "taskId")]
    task_id: String,
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
    team_name: String,
    team_file_path: String,
    lead_agent_id: String,
}

#[derive(Debug, Serialize)]
struct ClaudeCreateTeamResult {
    team_name: String,
    team_file_path: String,
    lead_agent_id: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    tmux_pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
}

#[derive(Debug, Serialize)]
struct SpawnAgentTeammateResult {
    status: &'static str,
    prompt: String,
    teammate_id: String,
    agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_type: Option<String>,
    model: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    tmux_session_name: String,
    tmux_window_name: String,
    tmux_pane_id: String,
    team_name: String,
    is_splitpane: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    plan_mode_required: bool,
}

struct ProcessSpawnedTeamMember {
    member: crate::team::TeamMember,
    team_name: String,
    agent_id: String,
    tmux_pane_id: String,
    backend_type: String,
    color: Option<String>,
    model: String,
    mode: Option<String>,
    plan_mode_required: bool,
    is_active: bool,
}

#[derive(Debug, Serialize)]
struct ClaudeSendMessageResult {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    routing: Option<ClaudeMessageRouting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recipients: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeMessageRouting {
    sender: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sender_color: Option<String>,
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

fn claude_team_send_result(
    message: String,
    sender: String,
    target: String,
    summary: Option<String>,
    content: Option<String>,
) -> Result<FunctionToolOutput, FunctionCallError> {
    json_output(
        &ClaudeSendMessageResult {
            success: true,
            message,
            routing: Some(ClaudeMessageRouting {
                sender,
                sender_color: None,
                target,
                target_color: None,
                summary,
                content,
            }),
            recipients: None,
            request_id: None,
            target: None,
        },
        Some(true),
        "team_send",
    )
}

fn claude_team_send_broadcast_result(
    recipients: Vec<String>,
    sender: String,
    summary: Option<String>,
    content: Option<String>,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let routing = if recipients.is_empty() {
        None
    } else {
        Some(ClaudeMessageRouting {
            sender,
            sender_color: None,
            target: "@team".to_string(),
            target_color: None,
            summary,
            content,
        })
    };
    json_output(
        &ClaudeSendMessageResult {
            success: true,
            message: claude_broadcast_message(&recipients),
            routing,
            recipients: Some(recipients),
            request_id: None,
            target: None,
        },
        Some(true),
        "team_send",
    )
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

#[derive(Debug, Deserialize, Serialize)]
struct StoreTeamTaskCreateResult {
    task: team_coord::Task,
}

#[derive(Debug, Deserialize, Serialize)]
struct StoreTeamTaskUpdateResult {
    task: team_coord::Task,
}

#[derive(Debug, Deserialize, Serialize)]
struct StoreTeamTaskClaimResult {
    task: team_coord::Task,
}

#[derive(Debug, Deserialize, Serialize)]
struct StoreTeamTaskListResult {
    tasks: Vec<team_coord::Task>,
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
    if matches!(turn.session_source, SessionSource::SubAgent(_)) {
        return Err(FunctionCallError::RespondToModel(
            "Codex Teams tools are lead-only and are unavailable inside native spawned subagent sessions."
                .to_string(),
        ));
    }
    if crate::team::teammate_identity().is_some() && !tool.allowed_in_teammate_process() {
        return Err(FunctionCallError::RespondToModel(format!(
            "{} is lead-only in a Codex Teams teammate process. Teammates must not create teams or spawn roster members; use team_send plus team_task_* tools to coordinate.",
            tool.name()
        )));
    }
    let arguments = function_arguments(payload)?;
    match tool {
        TeamTool::CreateTeam => {
            create_team(session, turn, arguments, CreateTeamOutput::LegacyCodex).await
        }
        TeamTool::ClaudeTeamCreate => {
            create_team(session, turn, arguments, CreateTeamOutput::ClaudeAlias).await
        }
        TeamTool::ListTeams => list_teams(session, arguments).await,
        TeamTool::TeamStatus => team_status(session, arguments).await,
        TeamTool::TeamSpawnMember => team_spawn_member(session, turn, arguments).await,
        TeamTool::TeamSend => team_send(session, turn, arguments).await,
        TeamTool::ClaudeSendMessage => claude_send_message(session, turn, arguments).await,
        TeamTool::TeamMessageList => team_message_list(session, turn, arguments).await,
        TeamTool::TeamTaskCreate => team_task_create(session, turn, arguments).await,
        TeamTool::ClaudeTaskCreate => claude_task_create(session, turn, arguments).await,
        TeamTool::TeamTaskUpdate => team_task_update(session, turn, arguments).await,
        TeamTool::ClaudeTaskUpdate => claude_task_update(session, turn, arguments).await,
        TeamTool::TeamTaskClaim => team_task_claim(session, turn, arguments).await,
        TeamTool::TeamTaskList => team_task_list(session, turn, arguments).await,
        TeamTool::ClaudeTaskList => claude_task_list(session, turn, arguments).await,
        TeamTool::ClaudeTaskGet => claude_task_get(session, turn, arguments).await,
        TeamTool::TeamEventList => team_event_list(session, arguments).await,
        TeamTool::TeamMemberStop => team_member_stop(session, turn, arguments).await,
        TeamTool::TeamStop => team_stop(session, turn, arguments).await,
    }
}

#[derive(Debug, Clone, Copy)]
enum CreateTeamOutput {
    LegacyCodex,
    ClaudeAlias,
}

async fn create_team(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
    output: CreateTeamOutput,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: CreateTeamArgs = parse_arguments(&arguments)?;
    let requested_name = args.team_name.or(args.name).ok_or_else(|| {
        FunctionCallError::RespondToModel("team_name is required for TeamCreate".to_string())
    })?;
    let requested_name = non_empty(requested_name, "team name")?;
    let registry = session.services.agent_control.team_registry();
    if let Some(existing_team) = registry.list_teams().await.into_iter().find(|team| {
        team.lead_thread_id == session.thread_id
            && matches!(team.status, crate::team::TeamStatus::Active)
    }) {
        return Err(FunctionCallError::RespondToModel(format!(
            "Already leading team \"{}\". A leader can only manage one team at a time. Use TeamDelete to end the current team before creating a new one.",
            existing_team.name
        )));
    }
    let teams_root = teams_root_for_turn(turn.as_ref());
    let mut name = requested_name.clone();
    if team_store::read_config(&teams_root, &name)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to read team config: {err}"))
        })?
        .is_some()
    {
        for suffix in 2.. {
            let candidate = format!("{requested_name}-{suffix}");
            if team_store::read_config(&teams_root, &candidate)
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!("failed to read team config: {err}"))
                })?
                .is_none()
            {
                name = candidate;
                break;
            }
        }
    }
    let lead_agent_id = team_store::agent_id(team_store::TEAM_LEAD_NAME, &name);
    let lead_agent_type = args
        .agent_type
        .as_deref()
        .map(str::trim)
        .filter(|agent_type| !agent_type.is_empty())
        .unwrap_or(team_store::TEAM_LEAD_NAME)
        .to_string();
    let team_file_path = team_store::config_path(&teams_root, &name);
    let now = unix_millis();
    let team = registry.create_team(name.clone(), session.thread_id).await;
    let team_file = team_store::TeamFile {
        name: name.clone(),
        team_id: Some(team.id.to_string()),
        description: args.description,
        created_at: now,
        lead_agent_id: lead_agent_id.clone(),
        lead_session_id: Some(session.thread_id.to_string()),
        members: vec![team_store::TeamFileMember {
            agent_id: lead_agent_id.clone(),
            name: team_store::TEAM_LEAD_NAME.to_string(),
            agent_type: Some(lead_agent_type),
            model: Some(turn.model_info.slug.clone()),
            joined_at: now,
            cwd: turn.config.cwd.to_string_lossy().into_owned(),
            subscriptions: Vec::new(),
            ..Default::default()
        }],
        ..Default::default()
    };
    team_store::write_config(&teams_root, &name, &team_file).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to write team config: {err}"))
    })?;
    fs::create_dir_all(team_store::tasks_dir(&teams_root, &name)).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to create team task directory: {err}"))
    })?;
    let team_file_path = team_file_path.to_string_lossy().into_owned();
    match output {
        CreateTeamOutput::LegacyCodex => json_output(
            &CreateTeamResult {
                team,
                team_name: name,
                team_file_path,
                lead_agent_id,
            },
            Some(true),
            "create_team",
        ),
        CreateTeamOutput::ClaudeAlias => json_output(
            &ClaudeCreateTeamResult {
                team_name: name,
                team_file_path,
                lead_agent_id,
            },
            Some(true),
            "TeamCreate",
        ),
    }
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
    team_spawn_member_from_args(session, turn, args, "team_spawn_member", None, None).await
}

pub(crate) async fn maybe_spawn_member_from_agent_tool(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    request: SpawnMemberFromAgentToolRequest,
) -> Result<Option<FunctionToolOutput>, FunctionCallError> {
    if !turn.config.features.enabled(Feature::Teams) {
        return Ok(None);
    }
    let Some(name) = optional_non_empty(request.name, "teammate name")? else {
        return Ok(None);
    };
    let team_id = match optional_non_empty(request.team_name, "team name")? {
        Some(team_name) => Some(resolve_agent_tool_team_id(session.as_ref(), team_name).await?),
        None => infer_agent_tool_active_team_id(session.as_ref()).await,
    };
    let Some(team_id) = team_id else {
        return Ok(None);
    };
    let args = TeamSpawnMemberArgs {
        team_id: team_id.to_string(),
        name,
        profile: request.profile,
        capabilities: None,
        permissions: None,
        message: Some(request.message),
        items: None,
    };
    let requested_model = optional_non_empty(request.model, "teammate model")?;
    team_spawn_member_from_args(
        session,
        turn,
        args,
        "spawn_agent",
        requested_model,
        request.mode,
    )
    .await
    .map(Some)
}

async fn resolve_agent_tool_team_id(
    session: &Session,
    requested_team_name: String,
) -> Result<ThreadId, FunctionCallError> {
    let registry = session.services.agent_control.team_registry();
    let active_lead_teams = registry
        .list_teams()
        .await
        .into_iter()
        .filter(|team| {
            team.lead_thread_id == session.thread_id
                && matches!(team.status, crate::team::TeamStatus::Active)
        })
        .collect::<Vec<_>>();

    let requested_lower = requested_team_name.to_lowercase();
    active_lead_teams
        .into_iter()
        .find(|team| team.name.to_lowercase() == requested_lower)
        .map(|team| team.id)
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "No active Codex team named \"{requested_team_name}\". Use TeamCreate before spawning a teammate with spawn_agent name/team_name."
            ))
        })
}

async fn infer_agent_tool_active_team_id(session: &Session) -> Option<ThreadId> {
    let registry = session.services.agent_control.team_registry();
    let active_lead_teams = registry
        .list_teams()
        .await
        .into_iter()
        .filter(|team| {
            team.lead_thread_id == session.thread_id
                && matches!(team.status, crate::team::TeamStatus::Active)
        })
        .collect::<Vec<_>>();
    if active_lead_teams.len() == 1 {
        Some(active_lead_teams[0].id)
    } else {
        None
    }
}

async fn team_spawn_member_from_args(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    args: TeamSpawnMemberArgs,
    output_tool_name: &str,
    requested_model: Option<String>,
    requested_mode: Option<ModeKind>,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_lead(session.as_ref(), team_id, output_tool_name).await?;
    let name = non_empty(args.name, "member name")?;
    let capabilities = non_empty_values(args.capabilities.unwrap_or_default(), "capability")?;
    let permissions = non_empty_values(args.permissions.unwrap_or_default(), "permission")?;
    let items = parse_team_input(args.message, args.items)?;
    let child_depth = next_thread_spawn_depth(&turn.session_source);
    if exceeds_thread_spawn_depth_limit(child_depth, turn.config.agent_max_depth) {
        return Err(FunctionCallError::RespondToModel(
            "Teammate spawn depth limit reached. Solve the task yourself.".to_string(),
        ));
    }

    // Preferred path: launch a real `codex teammate` PROCESS in a tmux pane
    // (Claude `handleSpawnSplitPane`). If no process-pane backend is available,
    // fail closed: Teams teammates need their own teammate process and mailbox.
    if TmuxBackend::new().is_inside_tmux() {
        let spawned = spawn_member_in_pane(
            session.as_ref(),
            turn.as_ref(),
            team_id,
            name,
            args.profile,
            capabilities,
            permissions,
            &items,
            requested_model.as_deref(),
            requested_mode,
        )
        .await?;
        let ProcessSpawnedTeamMember {
            member,
            team_name,
            agent_id,
            tmux_pane_id,
            backend_type,
            color,
            model,
            mode,
            plan_mode_required,
            is_active,
        } = spawned;
        if output_tool_name == "spawn_agent" {
            return json_output(
                &SpawnAgentTeammateResult {
                    status: "teammate_spawned",
                    prompt: input_preview(&items),
                    teammate_id: agent_id.clone(),
                    agent_id,
                    agent_type: member.profile.clone(),
                    model,
                    name: member.name,
                    color,
                    tmux_session_name: "codex-swarm".to_string(),
                    tmux_window_name: "swarm-view".to_string(),
                    tmux_pane_id,
                    team_name,
                    is_splitpane: true,
                    plan_mode_required,
                },
                Some(true),
                output_tool_name,
            );
        }
        return json_output(
            &TeamSpawnMemberResult {
                member,
                tmux_pane_id: Some(tmux_pane_id),
                backend_type: Some(backend_type),
                color,
                mode,
                is_active: Some(is_active),
                prompt: Some(input_preview(&items)),
            },
            Some(true),
            output_tool_name,
        );
    }

    // Next-best path: an iTerm2 split pane driven by the `it2` Python-API CLI.
    // This still launches a real `codex teammate` process with mailbox routing.
    if iterm::is_in_iterm2() && ITermBackend::new().is_available().await {
        let spawned = spawn_member_in_iterm_pane(
            session.as_ref(),
            turn.as_ref(),
            team_id,
            name,
            args.profile,
            capabilities,
            permissions,
            &items,
            requested_model.as_deref(),
            requested_mode,
        )
        .await?;
        let ProcessSpawnedTeamMember {
            member,
            team_name,
            agent_id,
            tmux_pane_id,
            backend_type,
            color,
            model,
            mode,
            plan_mode_required,
            is_active,
        } = spawned;
        if output_tool_name == "spawn_agent" {
            return json_output(
                &SpawnAgentTeammateResult {
                    status: "teammate_spawned",
                    prompt: input_preview(&items),
                    teammate_id: agent_id.clone(),
                    agent_id,
                    agent_type: member.profile.clone(),
                    model,
                    name: member.name,
                    color,
                    tmux_session_name: "codex-swarm".to_string(),
                    tmux_window_name: "swarm-view".to_string(),
                    tmux_pane_id,
                    team_name,
                    is_splitpane: true,
                    plan_mode_required,
                },
                Some(true),
                output_tool_name,
            );
        }
        return json_output(
            &TeamSpawnMemberResult {
                member,
                tmux_pane_id: Some(tmux_pane_id),
                backend_type: Some(backend_type),
                color,
                mode,
                is_active: Some(is_active),
                prompt: Some(input_preview(&items)),
            },
            Some(true),
            output_tool_name,
        );
    }

    Err(FunctionCallError::RespondToModel(
        "team_spawn_member requires a tmux or iTerm2 pane backend so the teammate runs as a Codex Teams process. Start Codex inside a supported pane backend, then retry the Teams request."
            .to_string(),
    ))
}

/// Look up the live team's display name from the in-memory registry. The caller
/// has already passed `require_team_lead`, so the team exists.
async fn registry_team_name(
    session: &Session,
    team_id: ThreadId,
) -> Result<String, FunctionCallError> {
    session
        .services
        .agent_control
        .team_registry()
        .list_teams()
        .await
        .into_iter()
        .find(|team| team.id == team_id)
        .map(|team| team.name)
        .ok_or_else(|| FunctionCallError::RespondToModel(format!("team {team_id} not found")))
}

/// Milliseconds since the Unix epoch (Claude `joinedAt: Date.now()`).
fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn append_teammate_model_override(flags: &mut Vec<String>, model: &str) {
    append_teammate_config_string_override(flags, "model", model);
}

fn resolve_teammate_model(requested_model: Option<&str>, leader_model: &str) -> String {
    match requested_model {
        Some("inherit") | None => leader_model.to_string(),
        Some(model) => model.to_string(),
    }
}

fn append_teammate_config_string_override(flags: &mut Vec<String>, key: &str, value: &str) {
    flags.push("-c".to_string());
    let quoted_value = serde_json::Value::String(value.to_string()).to_string();
    flags.push(format!("{key}={quoted_value}"));
}

fn teammate_active_config_profile(config: &crate::config::Config) -> Option<&str> {
    config
        .config_layer_stack
        .get_layers(
            ConfigLayerStackOrdering::LowestPrecedenceFirst,
            /*include_disabled*/ false,
        )
        .iter()
        .find_map(|layer| match &layer.name {
            ConfigLayerSource::User {
                profile: Some(profile),
                ..
            } => Some(profile.as_str()),
            ConfigLayerSource::Mdm { .. }
            | ConfigLayerSource::System { .. }
            | ConfigLayerSource::EnterpriseManaged { .. }
            | ConfigLayerSource::User { profile: None, .. }
            | ConfigLayerSource::Project { .. }
            | ConfigLayerSource::SessionFlags
            | ConfigLayerSource::LegacyManagedConfigTomlFromFile { .. }
            | ConfigLayerSource::LegacyManagedConfigTomlFromMdm => None,
        })
}

fn append_teammate_launch_mode_flags(
    flags: &mut Vec<String>,
    mode: ModeKind,
    approval_policy: AskForApproval,
    sandbox_policy: &SandboxPolicy,
) -> bool {
    // A split-pane teammate is auto-launched by an already-running lead. It must
    // not stop at startup hook review before it can read its first mailbox turn.
    flags.push("--dangerously-bypass-hook-trust".to_string());

    let plan_mode_required = mode == ModeKind::Plan;
    if plan_mode_required {
        flags.push("--plan-mode-required".to_string());
        append_teammate_config_string_override(flags, "approval_policy", "on-request");
    } else {
        match approval_policy {
            AskForApproval::UnlessTrusted => {
                append_teammate_config_string_override(flags, "approval_policy", "untrusted");
            }
            AskForApproval::OnFailure => {
                append_teammate_config_string_override(flags, "approval_policy", "on-failure");
            }
            AskForApproval::OnRequest => {
                append_teammate_config_string_override(flags, "approval_policy", "on-request");
            }
            AskForApproval::Never => {
                append_teammate_config_string_override(flags, "approval_policy", "never");
            }
            AskForApproval::Granular(_) => {}
        }
    }

    match sandbox_policy {
        SandboxPolicy::DangerFullAccess if !plan_mode_required => {
            append_teammate_config_string_override(flags, "sandbox_mode", "danger-full-access");
        }
        SandboxPolicy::DangerFullAccess => {}
        SandboxPolicy::ReadOnly { .. } => {
            append_teammate_config_string_override(flags, "sandbox_mode", "read-only");
        }
        SandboxPolicy::WorkspaceWrite { .. } => {
            append_teammate_config_string_override(flags, "sandbox_mode", "workspace-write");
        }
        SandboxPolicy::ExternalSandbox { .. } => {}
    }

    plan_mode_required
}

fn teammate_provider_env_keys(provider: &ModelProviderInfo) -> Vec<&str> {
    let mut keys = Vec::new();
    if let Some(env_key) = provider.env_key.as_deref() {
        keys.push(env_key);
    }
    if let Some(headers) = provider.env_http_headers.as_ref() {
        let mut header_keys = headers.values().map(String::as_str).collect::<Vec<_>>();
        header_keys.sort_unstable();
        for key in header_keys {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

struct TeammateLaunchSpec {
    agent_id: String,
    agent_name: String,
    color: crate::team_backends::tmux::AgentColor,
    model: String,
    prompt_text: String,
    first_turn_text: String,
    cwd: std::path::PathBuf,
    binary: std::path::PathBuf,
    flags: Vec<String>,
    env: Vec<(String, String)>,
    plan_mode_required: bool,
}

fn build_teammate_launch_spec(
    turn: &TurnContext,
    team_name: &str,
    lead_thread_id: ThreadId,
    name: &str,
    profile: Option<&str>,
    existing_names: &[String],
    items: &[UserInput],
    requested_model: Option<&str>,
    requested_mode: Option<ModeKind>,
) -> Result<TeammateLaunchSpec, FunctionCallError> {
    let binary = spawn::teammate_binary(turn.config.codex_self_exe.as_deref()).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to resolve teammate binary: {err}"))
    })?;
    build_teammate_launch_spec_with_binary(
        turn,
        team_name,
        lead_thread_id,
        name,
        profile,
        existing_names,
        items,
        requested_model,
        requested_mode,
        binary,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_teammate_launch_spec_with_binary(
    turn: &TurnContext,
    team_name: &str,
    lead_thread_id: ThreadId,
    name: &str,
    profile: Option<&str>,
    existing_names: &[String],
    items: &[UserInput],
    requested_model: Option<&str>,
    requested_mode: Option<ModeKind>,
    binary: std::path::PathBuf,
) -> Result<TeammateLaunchSpec, FunctionCallError> {
    let unique = spawn::unique_teammate_name(name, existing_names);
    let sanitized = spawn::sanitize_agent_name(&unique);
    let color = spawn::teammate_color(existing_names.len());
    let agent_id = team_store::agent_id(&sanitized, team_name);
    let prompt_text = input_preview(items);
    let first_turn_text = prompt_text.clone();
    let model = resolve_teammate_model(requested_model, &turn.model_info.slug);
    let cwd = turn.config.cwd.as_path().to_path_buf();

    // NO `--prompt`: the first turn is delivered via the mailbox below
    // (mirroring Claude), so it is not run twice.
    let mut flags = Vec::new();
    if let Some(profile) = teammate_active_config_profile(&turn.config) {
        flags.push("--profile".to_string());
        flags.push(profile.to_string());
    }
    flags.extend([
        "teammate".to_string(),
        "--agent-id".to_string(),
        agent_id.clone(),
        "--agent-name".to_string(),
        sanitized.clone(),
        "--team-name".to_string(),
        team_name.to_string(),
        "--agent-color".to_string(),
        color.as_name().to_string(),
        "--parent-session-id".to_string(),
        lead_thread_id.to_string(),
    ]);
    if let Some(profile) = profile {
        flags.push("--agent-type".to_string());
        flags.push(profile.to_string());
    }
    // A `codex teammate` process is by definition a Teams session; enable the
    // (default-off) `teams` feature so the spawned process exposes the team
    // tools it needs (`team_send`, etc.). Without this the teammate boots with
    // teams OFF and cannot reply to the lead.
    flags.push("--enable".to_string());
    flags.push("teams".to_string());
    append_teammate_model_override(&mut flags, &model);
    let sandbox_policy = turn.sandbox_policy();
    let launch_mode = requested_mode.unwrap_or(turn.collaboration_mode.mode);
    let plan_mode_required = append_teammate_launch_mode_flags(
        &mut flags,
        launch_mode,
        turn.approval_policy.value(),
        &sandbox_policy,
    );

    let provider_env_keys = teammate_provider_env_keys(&turn.config.model_provider);
    let teams_root = teams_root_for_turn(turn);
    let env = spawn::build_inherited_env_vars_for_config_home(
        &teams_root,
        turn.config.codex_home.as_path(),
        &provider_env_keys,
    );

    Ok(TeammateLaunchSpec {
        agent_id,
        agent_name: sanitized,
        color,
        model,
        prompt_text,
        first_turn_text,
        cwd,
        binary,
        flags,
        env,
        plan_mode_required,
    })
}

async fn apply_lead_auth_to_teammate_env(
    turn: &TurnContext,
    env: &mut Vec<(String, String)>,
) -> Result<(), FunctionCallError> {
    if !turn.config.model_provider.requires_openai_auth {
        return Ok(());
    }

    let Some(auth_manager) = turn.auth_manager.as_ref() else {
        return Err(FunctionCallError::RespondToModel(
            "Cannot spawn a Codex Teams teammate because the selected model provider requires OpenAI/Codex auth, but this session has no auth manager. Run `codex login` or configure provider env auth, then retry.".to_string(),
        ));
    };

    let Some(auth) = auth_manager.auth().await else {
        return Err(FunctionCallError::RespondToModel(
            "Cannot spawn a Codex Teams teammate because the selected model provider requires OpenAI/Codex auth, but the lead session is not authenticated. Run `codex login` or configure provider env auth, then retry.".to_string(),
        ));
    };

    if let Some(api_key) = auth.api_key() {
        for key in teammate_api_key_env_keys(&turn.config.model_provider) {
            upsert_teammate_env(env, key, api_key.to_string());
        }
    } else {
        for key in teammate_api_key_env_keys(&turn.config.model_provider) {
            remove_teammate_env(env, &key);
        }
    }
    Ok(())
}

fn teammate_api_key_env_keys(provider: &ModelProviderInfo) -> Vec<String> {
    let mut keys = vec![
        codex_login::CODEX_API_KEY_ENV_VAR.to_string(),
        codex_login::OPENAI_API_KEY_ENV_VAR.to_string(),
    ];
    if let Some(env_key) = provider.env_key.as_ref().filter(|key| !key.is_empty())
        && !keys.contains(env_key)
    {
        keys.push(env_key.clone());
    }
    keys
}

fn upsert_teammate_env(env: &mut Vec<(String, String)>, key: String, value: String) {
    if let Some((_, existing_value)) = env
        .iter_mut()
        .find(|(existing_key, _)| existing_key == &key)
    {
        *existing_value = value;
    } else {
        env.push((key, value));
    }
}

fn remove_teammate_env(env: &mut Vec<(String, String)>, key: &str) {
    env.retain(|(existing_key, _)| existing_key != key);
}

/// Launch a teammate as a separate `codex teammate` process in a tmux pane and
/// register it on disk (Claude `handleSpawnSplitPane`). Returns a `TeamMember`
/// record for the tool result; the process coordinates via the on-disk team
/// store + file mailbox rather than the in-memory registry.
#[allow(clippy::too_many_arguments)]
async fn spawn_member_in_pane(
    session: &Session,
    turn: &TurnContext,
    team_id: ThreadId,
    name: String,
    profile: Option<String>,
    capabilities: Vec<String>,
    permissions: Vec<String>,
    items: &[UserInput],
    requested_model: Option<&str>,
    requested_mode: Option<ModeKind>,
) -> Result<ProcessSpawnedTeamMember, FunctionCallError> {
    // Resolve everything that needs `.await` BEFORE constructing the tmux
    // backend: `TmuxBackend` holds a `RefCell`, so keeping it alive across an
    // await point would make this tool future `!Send`.
    let team_name = registry_team_name(session, team_id).await?;
    let backend = TmuxBackend::new();
    let teams_root = teams_root_for_turn(turn);

    // Existing members → unique name (Claude `generateUniqueTeammateName`) +
    // round-robin color index.
    let existing_names: Vec<String> = team_store::read_config(&teams_root, &team_name)
        .ok()
        .flatten()
        .map(|cfg| cfg.members.into_iter().map(|member| member.name).collect())
        .unwrap_or_default();
    let mut spec = build_teammate_launch_spec(
        turn,
        &team_name,
        session.thread_id,
        &name,
        profile.as_deref(),
        &existing_names,
        items,
        requested_model,
        requested_mode,
    )?;
    let member_thread_id = ThreadId::new();
    apply_lead_auth_to_teammate_env(turn, &mut spec.env).await?;

    // Open + style the pane (create_teammate_pane sets the title + border color
    // internally, so they are not re-set here).
    let pane = backend
        .create_teammate_pane(&spec.agent_name, spec.color)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to create teammate pane: {err}"))
        })?;

    backend
        .launch_teammate(
            &pane.pane_id,
            pane.used_external_session,
            &spec.cwd,
            &spec.env,
            &spec.binary,
            &spec.flags,
        )
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to launch teammate process: {err}"))
        })?;

    // Register the member in the on-disk team config.
    let pane_id = pane.pane_id;
    let member_record = team_store::TeamFileMember {
        agent_id: spec.agent_id.clone(),
        member_id: Some(member_thread_id.to_string()),
        name: spec.agent_name.clone(),
        agent_type: profile.clone(),
        model: Some(spec.model.clone()),
        prompt: Some(spec.prompt_text.clone()),
        color: Some(spec.color.as_name().to_string()),
        joined_at: unix_millis(),
        tmux_pane_id: pane_id.clone(),
        cwd: spec.cwd.to_string_lossy().into_owned(),
        backend_type: Some("tmux".to_string()),
        is_active: Some(true),
        plan_mode_required: spec.plan_mode_required.then_some(true),
        mode: spec.plan_mode_required.then_some("plan".to_string()),
        ..Default::default()
    };
    let team_for_cfg = team_name.clone();
    team_store::update_config(&teams_root, &team_name, move |cfg| {
        cfg.name = team_for_cfg.clone();
        cfg.team_id = Some(team_id.to_string());
        if cfg.lead_agent_id.is_empty() {
            cfg.lead_agent_id = team_store::agent_id(team_store::TEAM_LEAD_NAME, &team_for_cfg);
        }
        cfg.members.push(member_record);
    })
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to register teammate in config: {err}"))
    })?;

    // Deliver the initial prompt via the mailbox (Claude step 9): the teammate's
    // inbox loop consumes it as its first turn.
    write_initial_teammate_prompt_to_mailbox(
        &teams_root,
        &team_name,
        &spec.agent_name,
        &spec.first_turn_text,
    )?;

    let member = crate::team::TeamMember::process_member_with_id(
        member_thread_id,
        spec.agent_name,
        profile,
        capabilities,
        permissions,
    );
    let member = team_result(
        session,
        Some(team_id),
        session
            .services
            .agent_control
            .team_registry()
            .register_process_member(team_id, member)
            .await,
    )
    .await?;

    Ok(ProcessSpawnedTeamMember {
        member,
        team_name,
        agent_id: spec.agent_id,
        tmux_pane_id: pane_id,
        backend_type: "tmux".to_string(),
        color: Some(spec.color.as_name().to_string()),
        model: spec.model,
        mode: spec.plan_mode_required.then_some("plan".to_string()),
        plan_mode_required: spec.plan_mode_required,
        is_active: true,
    })
}

/// iTerm2 twin of [`spawn_member_in_pane`]: launch a real `codex teammate`
/// process in an iTerm2 split pane via the `it2` Python-API CLI (Claude's
/// `ITermBackend`). The common contract (mailbox-delivered first turn, on-disk
/// member record + `member_id`, file mailbox delivery, returned process member)
/// is identical to the tmux path; only pane creation + command delivery differ.
/// Unlike `TmuxBackend` (which holds a `RefCell`), `ITermBackend` is async/`Send`,
/// so it may be held across the `.await`s here.
#[allow(clippy::too_many_arguments)]
async fn spawn_member_in_iterm_pane(
    session: &Session,
    turn: &TurnContext,
    team_id: ThreadId,
    name: String,
    profile: Option<String>,
    capabilities: Vec<String>,
    permissions: Vec<String>,
    items: &[UserInput],
    requested_model: Option<&str>,
    requested_mode: Option<ModeKind>,
) -> Result<ProcessSpawnedTeamMember, FunctionCallError> {
    let team_name = registry_team_name(session, team_id).await?;
    let backend = ITermBackend::new();
    let teams_root = teams_root_for_turn(turn);

    let existing_names: Vec<String> = team_store::read_config(&teams_root, &team_name)
        .ok()
        .flatten()
        .map(|cfg| cfg.members.into_iter().map(|member| member.name).collect())
        .unwrap_or_default();
    let mut spec = build_teammate_launch_spec(
        turn,
        &team_name,
        session.thread_id,
        &name,
        profile.as_deref(),
        &existing_names,
        items,
        requested_model,
        requested_mode,
    )?;
    let member_thread_id = ThreadId::new();
    apply_lead_auth_to_teammate_env(turn, &mut spec.env).await?;

    // Create the iTerm2 split pane (first teammate: vertical split off the lead;
    // later teammates stack downward — handled inside the backend).
    let pane = backend
        .create_teammate_pane_in_swarm_view(&spec.agent_name, spec.color.as_name())
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to create iTerm2 pane: {err}"))
        })?;

    // Reuse the tmux launch-line builder so the same shell-quoting applies (flag
    // values such as a team name with spaces must be quoted).
    let launch_line = crate::team_backends::tmux::build_launch_line(
        &spec.cwd,
        &spec.env,
        &spec.binary,
        &spec.flags,
    )
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to build teammate launch line: {err}"))
    })?;
    backend
        .send_command_to_pane(
            &pane.pane_id,
            &launch_line,
            /*use_external_session*/ false,
        )
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to launch teammate process: {err}"))
        })?;

    let member_record = team_store::TeamFileMember {
        agent_id: spec.agent_id.clone(),
        member_id: Some(member_thread_id.to_string()),
        name: spec.agent_name.clone(),
        agent_type: profile.clone(),
        model: Some(spec.model.clone()),
        prompt: Some(spec.prompt_text.clone()),
        color: Some(spec.color.as_name().to_string()),
        joined_at: unix_millis(),
        tmux_pane_id: pane.pane_id.clone(),
        cwd: spec.cwd.to_string_lossy().into_owned(),
        backend_type: Some("iterm2".to_string()),
        is_active: Some(true),
        plan_mode_required: spec.plan_mode_required.then_some(true),
        mode: spec.plan_mode_required.then_some("plan".to_string()),
        ..Default::default()
    };
    let team_for_cfg = team_name.clone();
    team_store::update_config(&teams_root, &team_name, move |cfg| {
        cfg.name = team_for_cfg.clone();
        cfg.team_id = Some(team_id.to_string());
        if cfg.lead_agent_id.is_empty() {
            cfg.lead_agent_id = team_store::agent_id(team_store::TEAM_LEAD_NAME, &team_for_cfg);
        }
        cfg.members.push(member_record);
    })
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to register teammate in config: {err}"))
    })?;

    write_initial_teammate_prompt_to_mailbox(
        &teams_root,
        &team_name,
        &spec.agent_name,
        &spec.first_turn_text,
    )?;

    let member = crate::team::TeamMember::process_member_with_id(
        member_thread_id,
        spec.agent_name,
        profile,
        capabilities,
        permissions,
    );
    let member = team_result(
        session,
        Some(team_id),
        session
            .services
            .agent_control
            .team_registry()
            .register_process_member(team_id, member)
            .await,
    )
    .await?;

    Ok(ProcessSpawnedTeamMember {
        member,
        team_name,
        agent_id: spec.agent_id,
        tmux_pane_id: pane.pane_id,
        backend_type: "iterm2".to_string(),
        color: Some(spec.color.as_name().to_string()),
        model: spec.model,
        mode: spec.plan_mode_required.then_some("plan".to_string()),
        plan_mode_required: spec.plan_mode_required,
        is_active: true,
    })
}

async fn team_send(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamSendArgs = normalize_team_send_args(parse_arguments(&arguments)?)?;
    team_send_args(session, turn, args, "team_send").await
}

async fn team_send_args(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    args: TeamSendArgs,
    output_tool_name: &'static str,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args = normalize_team_send_args(args)?;
    // Teammate-process path: a spawned `codex teammate` has an EMPTY in-memory
    // registry (it never ran create_team), so the registry-backed delivery below
    // cannot resolve the team and `authorize_team_sender` would reject it. Route
    // its sends across the process boundary via the on-disk file mailbox instead
    // (Claude's cross-process model: reply lands in the lead's inbox).
    if let Some(identity) = crate::team::teammate_identity() {
        let requested_team_id = args
            .team_id
            .as_deref()
            .map(|team_id| id_from_str("team", team_id))
            .transpose()?;
        return teammate_team_send(
            turn.as_ref(),
            requested_team_id,
            identity,
            args,
            output_tool_name,
        )
        .await;
    }

    let team_id_arg = args.team_id.as_deref().ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "team_id is required when team_send is used by the team lead".to_string(),
        )
    })?;
    let team_id = id_from_str("team", team_id_arg)?;

    let requested_sender_member_id =
        optional_id_from_str("sender member", args.sender_member_id.clone())?;
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

    if matches!(args.target.as_deref(), Some("broadcast")) {
        return team_send_broadcast_to_pane_members(session.as_ref(), turn.as_ref(), team_id, args)
            .await;
    }

    // Split-pane PROCESS teammates are mirrored into the live registry for status
    // and task lifecycle, but delivery still goes through the mailbox their
    // process polls. They can be addressed by NAME or by the spawn-returned
    // member_id persisted on disk.
    if matches!(args.target.as_deref(), Some("member") | None)
        && let Some((pane_name, _pane_member_label)) = resolve_pane_member(
            session.as_ref(),
            turn.as_ref(),
            team_id,
            args.member_name.as_deref(),
            args.member_id.as_deref(),
        )
        .await?
    {
        return team_send_to_pane_member(
            session.as_ref(),
            turn.as_ref(),
            PaneMemberSendRequest {
                team_id,
                member_name: pane_name,
                delivery_mode: args.delivery_mode,
                summary: args.summary,
                message: args.message,
                items: args.items,
            },
        )
        .await;
    }

    let target = match args.target.as_deref() {
        Some("lead") => {
            if args.member_id.is_some() {
                return Err(FunctionCallError::RespondToModel(
                    "member_id must be omitted when team_send target is lead".to_string(),
                ));
            }
            SendTeamMessageTarget::Lead
        }
        Some("broadcast") => unreachable!("broadcast handled before registry delivery"),
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
    let summary = args.summary.clone();
    let target_label = match &target {
        SendTeamMessageTarget::Lead => format!("@{}", team_store::TEAM_LEAD_NAME),
        SendTeamMessageTarget::Member(member_id) => format!("@{member_id}"),
    };
    let registry = session.services.agent_control.team_registry();
    let _message = team_result(
        session.as_ref(),
        Some(team_id),
        registry
            .send_message(
                SendTeamMessageRequest {
                    team_id,
                    sender_member_id,
                    target,
                    content: content.clone(),
                    delivery_mode,
                    items,
                },
                &session.services.agent_control,
            )
            .await,
    )
    .await?;
    let sender = sender_member_id
        .map(|member_id| format!("@{member_id}"))
        .unwrap_or_else(|| team_store::TEAM_LEAD_NAME.to_string());
    claude_team_send_result(
        format!("Message sent to {target_label}'s inbox"),
        sender,
        target_label,
        summary,
        Some(content),
    )
}

struct PaneMemberSendRequest {
    team_id: ThreadId,
    member_name: String,
    delivery_mode: Option<String>,
    summary: Option<String>,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
}

/// Deliver a lead-originated message to a split-pane PROCESS teammate via its
/// on-disk inbox (the teammate's run-loop polls it). Process members are not in
/// the in-memory registry, so they are addressed by name, not ThreadId.
async fn team_send_to_pane_member(
    session: &Session,
    turn: &TurnContext,
    request: PaneMemberSendRequest,
) -> Result<FunctionToolOutput, FunctionCallError> {
    if matches!(request.delivery_mode.as_deref(), Some("interrupt")) {
        return Err(FunctionCallError::RespondToModel(
            "interrupt delivery is not supported for split-pane teammate mailboxes; use queue"
                .to_string(),
        ));
    }
    let team_name = registry_team_name(session, request.team_id).await?;
    let teams_root = teams_root_for_turn(turn);
    let sanitized = crate::team_backends::spawn::sanitize_agent_name(&request.member_name);

    let known = team_store::read_config(&teams_root, &team_name)
        .ok()
        .flatten()
        .is_some_and(|cfg| {
            cfg.members
                .iter()
                .any(|member| member.name == sanitized && store_member_is_active(member))
        });
    if !known {
        return Err(FunctionCallError::RespondToModel(format!(
            "no teammate named '{}' in team '{team_name}'",
            request.member_name
        )));
    }

    let items = parse_team_input(request.message, request.items)?;
    let content = input_preview(&items);
    write_plain_mailbox_message(
        &teams_root,
        &team_name,
        &sanitized,
        team_store::TEAM_LEAD_NAME,
        &content,
        request.summary.clone(),
    )?;

    claude_team_send_result(
        format!("Message sent to {sanitized}'s inbox"),
        team_store::TEAM_LEAD_NAME.to_string(),
        format!("@{sanitized}"),
        request.summary,
        Some(content),
    )
}

async fn team_send_broadcast_to_pane_members(
    session: &Session,
    turn: &TurnContext,
    team_id: ThreadId,
    args: TeamSendArgs,
) -> Result<FunctionToolOutput, FunctionCallError> {
    if matches!(args.delivery_mode.as_deref(), Some("interrupt")) {
        return Err(FunctionCallError::RespondToModel(
            "interrupt delivery is not supported for split-pane teammate mailboxes; use queue"
                .to_string(),
        ));
    }
    let team_name = registry_team_name(session, team_id).await?;
    let teams_root = teams_root_for_turn(turn);
    let members = active_store_members(&teams_root, &team_name)?
        .into_iter()
        .filter(|member| member.name != team_store::TEAM_LEAD_NAME)
        .collect::<Vec<_>>();
    let items = parse_team_input(args.message, args.items)?;
    let content = input_preview(&items);
    let mut targets = Vec::with_capacity(members.len());
    for member in members {
        write_plain_mailbox_message(
            &teams_root,
            &team_name,
            &member.name,
            team_store::TEAM_LEAD_NAME,
            &content,
            args.summary.clone(),
        )?;
        targets.push(member.name);
    }

    claude_team_send_broadcast_result(
        targets,
        team_store::TEAM_LEAD_NAME.to_string(),
        args.summary,
        Some(content),
    )
}

/// Resolve a lead's `team_send` member target to an on-disk (split-pane) teammate
/// by NAME or by the spawn-returned `member_id` (persisted in the team config).
/// Returns `(sanitized_name, label_for_envelope)` when a process member matches,
/// or `None` so the caller falls back to the in-memory registry (in-process
/// members). Errors only if the live team itself cannot be resolved.
async fn resolve_pane_member(
    session: &Session,
    turn: &TurnContext,
    team_id: ThreadId,
    member_name: Option<&str>,
    member_id: Option<&str>,
) -> Result<Option<(String, String)>, FunctionCallError> {
    let team_name = registry_team_name(session, team_id).await?;
    let teams_root = teams_root_for_turn(turn);
    let Some(cfg) = team_store::read_config(&teams_root, &team_name)
        .ok()
        .flatten()
    else {
        return Ok(None);
    };
    if let Some(name) = member_name {
        let sanitized = crate::team_backends::spawn::sanitize_agent_name(name);
        if let Some(member) = cfg.members.iter().find(|member| member.name == sanitized) {
            if !store_member_is_active(member) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "team member '{}' is stopped",
                    member.name
                )));
            }
            let label = member
                .member_id
                .clone()
                .unwrap_or_else(|| member.name.clone());
            return Ok(Some((member.name.clone(), label)));
        }
    }
    if let Some(id) = member_id
        && let Some(member) = cfg
            .members
            .iter()
            .find(|member| member.member_id.as_deref() == Some(id))
    {
        if !store_member_is_active(member) {
            return Err(FunctionCallError::RespondToModel(format!(
                "team member {id} is stopped"
            )));
        }
        return Ok(Some((member.name.clone(), id.to_string())));
    }
    Ok(None)
}

fn store_member_is_active(member: &team_store::TeamFileMember) -> bool {
    member.is_active.unwrap_or(true)
}

fn write_plain_mailbox_message(
    teams_root: &std::path::Path,
    team_name: &str,
    recipient: &str,
    sender: &str,
    text: &str,
    summary: Option<String>,
) -> Result<(), FunctionCallError> {
    team_store::write_to_mailbox(
        teams_root,
        team_name,
        recipient,
        team_store::TeammateMessage {
            from: sender.to_string(),
            text: text.to_string(),
            timestamp: team_store::now_timestamp(),
            read: false,
            color: None,
            summary,
        },
    )
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to deliver to {recipient} mailbox: {err}"
        ))
    })
}

fn write_initial_teammate_prompt_to_mailbox(
    teams_root: &std::path::Path,
    team_name: &str,
    recipient: &str,
    prompt_text: &str,
) -> Result<(), FunctionCallError> {
    team_store::write_to_mailbox(
        teams_root,
        team_name,
        recipient,
        team_store::TeammateMessage {
            from: team_store::TEAM_LEAD_NAME.to_string(),
            text: prompt_text.to_string(),
            timestamp: team_store::now_timestamp(),
            read: false,
            color: None,
            summary: None,
        },
    )
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to deliver teammate prompt: {err}"))
    })
}

fn active_store_member_by_id(
    teams_root: &std::path::Path,
    team_name: &str,
    member_id: &str,
) -> Result<Option<team_store::TeamFileMember>, FunctionCallError> {
    let config = team_store::read_config(teams_root, team_name).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to read team config: {err}"))
    })?;
    Ok(config.and_then(|config| {
        config.members.into_iter().find(|member| {
            member.member_id.as_deref() == Some(member_id) && store_member_is_active(member)
        })
    }))
}

fn active_store_members(
    teams_root: &std::path::Path,
    team_name: &str,
) -> Result<Vec<team_store::TeamFileMember>, FunctionCallError> {
    let config = team_store::read_config(teams_root, team_name).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to read team config: {err}"))
    })?;
    Ok(config
        .map(|config| {
            config
                .members
                .into_iter()
                .filter(|member| {
                    member.name != team_store::TEAM_LEAD_NAME
                        && !member.tmux_pane_id.trim().is_empty()
                        && store_member_is_active(member)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default())
}

fn mark_store_members_inactive(
    teams_root: &std::path::Path,
    team_name: &str,
    member_ids: &[String],
) -> Result<(), FunctionCallError> {
    team_store::update_config(teams_root, team_name, |config| {
        for member in &mut config.members {
            if member
                .member_id
                .as_ref()
                .is_some_and(|member_id| member_ids.contains(member_id))
            {
                member.is_active = Some(false);
            }
        }
    })
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to mark teammate inactive: {err}"))
    })
}

fn send_process_shutdown_request(
    teams_root: &std::path::Path,
    team_name: &str,
    member: &team_store::TeamFileMember,
) -> Result<(), FunctionCallError> {
    team_coord::send_shutdown_request(
        teams_root,
        team_name,
        &member.name,
        team_store::TEAM_LEAD_NAME,
        &format!("shutdown-{}-{}", member.name, unix_millis()),
        Some("team stop requested".to_string()),
        member.color.clone(),
    )
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to request teammate shutdown: {err}"))
    })
}

struct TeammateProcessContext {
    team_id: ThreadId,
    team_name: String,
    member_agent_id: String,
}

struct ClaudeTeamContext {
    team_id: ThreadId,
    team_name: String,
    sender_name: String,
    sender_color: Option<String>,
    is_team_lead: bool,
}

async fn claude_team_context(
    session: &Session,
    turn: &TurnContext,
) -> Result<ClaudeTeamContext, FunctionCallError> {
    if let Some(identity) = crate::team::teammate_identity() {
        let context = validate_teammate_process_identity(
            turn, /*requested_team_id*/ None, identity, None,
        )?;
        let teams_root = teams_root_for_turn(turn);
        let sender_color = team_store::read_config(&teams_root, &context.team_name)
            .ok()
            .flatten()
            .and_then(|config| {
                config
                    .members
                    .into_iter()
                    .find(|member| member.name == identity.agent_name)
                    .and_then(|member| member.color)
            });
        return Ok(ClaudeTeamContext {
            team_id: context.team_id,
            team_name: context.team_name,
            sender_name: identity.agent_name.clone(),
            sender_color,
            is_team_lead: false,
        });
    }

    let registry = session.services.agent_control.team_registry();
    let teams = registry
        .list_teams()
        .await
        .into_iter()
        .filter(|team| {
            team.lead_thread_id == session.thread_id
                && matches!(team.status, crate::team::TeamStatus::Active)
        })
        .collect::<Vec<_>>();
    let team = match teams.as_slice() {
        [team] => team,
        [] => {
            return Err(FunctionCallError::RespondToModel(
                "No active Codex team. Use TeamCreate before Teams task or message tools."
                    .to_string(),
            ));
        }
        _ => {
            return Err(FunctionCallError::RespondToModel(
                "Multiple active Codex teams found; stop stale teams before using Claude-style Teams tools."
                    .to_string(),
            ));
        }
    };

    Ok(ClaudeTeamContext {
        team_id: team.id,
        team_name: team.name.clone(),
        sender_name: team_store::TEAM_LEAD_NAME.to_string(),
        sender_color: None,
        is_team_lead: true,
    })
}

fn validate_teammate_process_identity(
    turn: &TurnContext,
    requested_team_id: Option<ThreadId>,
    identity: &crate::team::TeammateIdentity,
    sender_member_id: Option<&str>,
) -> Result<TeammateProcessContext, FunctionCallError> {
    let teams_root = teams_root_for_turn(turn);
    let config = team_store::read_config(&teams_root, &identity.team).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to read team config: {err}"))
    })?;
    let Some(config) = config else {
        return Err(FunctionCallError::RespondToModel(format!(
            "team '{}' is not registered for this teammate process",
            identity.team
        )));
    };
    let team_id = match (requested_team_id, config.team_id.as_deref()) {
        (Some(requested), Some(config_team_id)) if config_team_id != requested.to_string() => {
            return Err(FunctionCallError::RespondToModel(format!(
                "team_id must match this teammate process team ({config_team_id})"
            )));
        }
        (Some(requested), _) => requested,
        (None, Some(config_team_id)) => id_from_str("team", config_team_id)?,
        (None, None) => {
            return Err(FunctionCallError::RespondToModel(
                "could not resolve this teammate process team_id from team config".to_string(),
            ));
        }
    };
    let member = config
        .members
        .iter()
        .find(|member| member.name == identity.agent_name)
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "teammate '{}' is not registered in team '{}'",
                identity.agent_name, identity.team
            ))
        })?;
    if !store_member_is_active(member) {
        return Err(FunctionCallError::RespondToModel(format!(
            "team member '{}' is stopped",
            identity.agent_name
        )));
    }
    if let Some(sender_member_id) = sender_member_id
        && member.member_id.as_deref() != Some(sender_member_id)
    {
        return Err(FunctionCallError::RespondToModel(
            "sender_member_id must match this teammate process member_id".to_string(),
        ));
    }
    Ok(TeammateProcessContext {
        team_id,
        team_name: identity.team.clone(),
        member_agent_id: member.agent_id.clone(),
    })
}

/// Deliver a teammate-originated `team_send` across the process boundary via the
/// on-disk file mailbox. A spawned `codex teammate` has no access to the lead's
/// live `TeamRegistry`, so the reply is written to the lead's inbox (`target:
/// "lead"`, the default) or a named peer's inbox (`target: "member"` +
/// `member_name`). The lead surfaces it via its inbox poller (TUI) and
/// `team_message_list` (model).
async fn teammate_team_send(
    turn: &TurnContext,
    requested_team_id: Option<ThreadId>,
    identity: &crate::team::TeammateIdentity,
    args: TeamSendArgs,
    output_tool_name: &'static str,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args = normalize_team_send_args(args)?;
    let teams_root = teams_root_for_turn(turn);
    let context = validate_teammate_process_identity(
        turn,
        requested_team_id,
        identity,
        args.sender_member_id.as_deref(),
    )?;
    let team_id = context.team_id;
    let delivery_mode = match args.delivery_mode.as_deref() {
        Some("interrupt") => TeamMessageDeliveryMode::Interrupt,
        Some("queue") | None => TeamMessageDeliveryMode::Queue,
        Some(other) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "unsupported team message delivery mode {other}; use queue or interrupt"
            )));
        }
    };
    if matches!(args.target.as_deref(), Some("broadcast")) {
        return teammate_team_send_broadcast(&teams_root, team_id, identity, args).await;
    }
    let (recipient, target) = match args.target.as_deref() {
        Some("lead") | None => (
            team_store::TEAM_LEAD_NAME.to_string(),
            crate::team::TeamMessageEndpoint::Lead(ThreadId::new()),
        ),
        Some("member") => {
            let name = args.member_name.clone().ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "member_name is required when a teammate sends to a member".to_string(),
                )
            })?;
            let sanitized = crate::team_backends::spawn::sanitize_agent_name(&name);
            let config = team_store::read_config(&teams_root, &identity.team).map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read team config: {err}"))
            })?;
            let Some(member) = config.as_ref().and_then(|config| {
                config
                    .members
                    .iter()
                    .find(|member| member.name == sanitized)
            }) else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "no teammate named '{}' in team '{}'",
                    name, identity.team
                )));
            };
            if member.name == team_store::TEAM_LEAD_NAME || member.tmux_pane_id.trim().is_empty() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "no split-pane teammate named '{}' in team '{}'",
                    name, identity.team
                )));
            }
            if !store_member_is_active(member) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "team member '{}' is stopped",
                    member.name
                )));
            }
            (
                sanitized,
                crate::team::TeamMessageEndpoint::Member(ThreadId::new()),
            )
        }
        Some("broadcast") => unreachable!("broadcast handled before peer delivery"),
        Some(other) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "unsupported team_send target {other}; use lead or member"
            )));
        }
    };
    let items = parse_team_input(args.message, args.items)?;
    let content = input_preview(&items);
    write_plain_mailbox_message(
        &teams_root,
        &identity.team,
        &recipient,
        &identity.agent_name,
        &content,
        args.summary.clone(),
    )?;
    let _ = (team_id, target, items, delivery_mode, output_tool_name);

    claude_team_send_result(
        format!("Message sent to {recipient}'s inbox"),
        identity.agent_name.clone(),
        format!("@{recipient}"),
        args.summary,
        Some(content),
    )
}

async fn teammate_team_send_broadcast(
    teams_root: &std::path::Path,
    team_id: ThreadId,
    identity: &crate::team::TeammateIdentity,
    args: TeamSendArgs,
) -> Result<FunctionToolOutput, FunctionCallError> {
    if matches!(args.delivery_mode.as_deref(), Some("interrupt")) {
        return Err(FunctionCallError::RespondToModel(
            "interrupt delivery is not supported for split-pane teammate mailboxes; use queue"
                .to_string(),
        ));
    }
    let members = active_store_members(teams_root, &identity.team)?;
    let recipients = members
        .into_iter()
        .filter(|member| member.name != identity.agent_name)
        .map(|member| member.name)
        .collect::<Vec<_>>();
    let items = parse_team_input(args.message, args.items)?;
    let content = input_preview(&items);
    for recipient in &recipients {
        write_plain_mailbox_message(
            teams_root,
            &identity.team,
            recipient,
            &identity.agent_name,
            &content,
            args.summary.clone(),
        )?;
    }

    let _ = (team_id, items);
    claude_team_send_broadcast_result(
        recipients,
        identity.agent_name.clone(),
        args.summary,
        Some(content),
    )
}

async fn teammate_team_task_create(
    turn: &TurnContext,
    identity: &crate::team::TeammateIdentity,
    args: TeamTaskCreateArgs,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let requested_team_id = optional_id_from_str("team", args.team_id)?;
    let context = validate_teammate_process_identity(
        turn,
        requested_team_id,
        identity,
        /*sender_member_id*/ None,
    )?;
    let teams_root = teams_root_for_turn(turn);
    let mut task = team_coord::Task::new(non_empty(args.title, "task title")?, "");
    task.description = optional_non_empty(args.note, "task note")?.unwrap_or_default();
    task.owner = args
        .assignee_member_id
        .map(|member_id| resolve_store_member_agent_id(&teams_root, &context.team_name, &member_id))
        .transpose()?;
    task.blocked_by = args.dependencies.unwrap_or_default();

    let id = team_coord::create_task(&teams_root, &context.team_name, task).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to create team task: {err}"))
    })?;
    let task = team_coord::get_task(&teams_root, &context.team_name, &id)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to read team task: {err}"))
        })?
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!("team task {id} not found after create"))
        })?;

    json_output(
        &StoreTeamTaskCreateResult { task },
        Some(true),
        "team_task_create",
    )
}

async fn teammate_team_task_update(
    turn: &TurnContext,
    identity: &crate::team::TeammateIdentity,
    args: TeamTaskUpdateArgs,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let TeamTaskUpdateArgs {
        team_id,
        task_id,
        title,
        assignee_member_id,
        clear_assignee,
        dependencies,
        status,
        note,
        clear_note,
    } = args;
    let requested_team_id = optional_id_from_str("team", team_id)?;
    let context = validate_teammate_process_identity(
        turn,
        requested_team_id,
        identity,
        /*sender_member_id*/ None,
    )?;
    let teams_root = teams_root_for_turn(turn);
    let status = status
        .map(|status| store_task_status(&status))
        .transpose()?;
    if clear_assignee && assignee_member_id.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "clear_assignee cannot be combined with assignee_member_id".to_string(),
        ));
    }
    let assignee = assignee_member_id
        .map(|member_id| resolve_store_member_agent_id(&teams_root, &context.team_name, &member_id))
        .transpose()?;
    let note = optional_non_empty(note, "task note")?;
    if clear_note && note.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "clear_note cannot be combined with note".to_string(),
        ));
    }
    let task = team_coord::update_task(&teams_root, &context.team_name, &task_id, |task| {
        if let Some(title) = title {
            task.subject = title;
        }
        if clear_assignee {
            task.owner = None;
        } else if let Some(assignee) = assignee {
            task.owner = Some(assignee);
        }
        if let Some(dependencies) = dependencies {
            task.blocked_by = dependencies;
        }
        if let Some(status) = status {
            task.status = status;
            if status == team_coord::TaskStatus::InProgress && task.owner.is_none() {
                task.owner = Some(context.member_agent_id.clone());
            }
        }
        if clear_note {
            task.description.clear();
        } else if let Some(note) = note {
            task.description = note;
        }
    })
    .map_err(|err| FunctionCallError::RespondToModel(format!("failed to update team task: {err}")))?
    .ok_or_else(|| FunctionCallError::RespondToModel(format!("team task {task_id} not found")))?;

    json_output(
        &StoreTeamTaskUpdateResult { task },
        Some(true),
        "team_task_update",
    )
}

async fn teammate_team_task_claim(
    turn: &TurnContext,
    identity: &crate::team::TeammateIdentity,
    args: TeamTaskClaimArgs,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let requested_team_id = optional_id_from_str("team", args.team_id)?;
    let context = validate_teammate_process_identity(
        turn,
        requested_team_id,
        identity,
        /*sender_member_id*/ None,
    )?;
    let teams_root = teams_root_for_turn(turn);
    let member_agent_id = match args.member_id {
        Some(member_id) => {
            let resolved =
                resolve_store_member_agent_id(&teams_root, &context.team_name, &member_id)?;
            if resolved != context.member_agent_id {
                return Err(FunctionCallError::RespondToModel(
                    "team_task_claim member_id must identify this teammate process".to_string(),
                ));
            }
            resolved
        }
        None => context.member_agent_id.clone(),
    };
    validate_store_task_claim(
        &teams_root,
        &context.team_name,
        &args.task_id,
        &member_agent_id,
    )?;
    let task = team_coord::update_task(&teams_root, &context.team_name, &args.task_id, |task| {
        task.owner = Some(member_agent_id.clone());
        task.status = team_coord::TaskStatus::InProgress;
    })
    .map_err(|err| FunctionCallError::RespondToModel(format!("failed to claim team task: {err}")))?
    .ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("team task {} not found", args.task_id))
    })?;

    json_output(
        &StoreTeamTaskClaimResult { task },
        Some(true),
        "team_task_claim",
    )
}

async fn teammate_team_task_list(
    turn: &TurnContext,
    identity: &crate::team::TeammateIdentity,
    args: TeamTaskListArgs,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let requested_team_id = optional_id_from_str("team", args.team_id)?;
    let context = validate_teammate_process_identity(
        turn,
        requested_team_id,
        identity,
        /*sender_member_id*/ None,
    )?;
    let teams_root = teams_root_for_turn(turn);
    let tasks = team_coord::list_tasks(&teams_root, &context.team_name).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to list team tasks: {err}"))
    })?;

    json_output(
        &StoreTeamTaskListResult { tasks },
        Some(true),
        "team_task_list",
    )
}

fn store_task_status(status: &str) -> Result<team_coord::TaskStatus, FunctionCallError> {
    match status {
        "pending" | "open" => Ok(team_coord::TaskStatus::Pending),
        "in_progress" | "claimed" => Ok(team_coord::TaskStatus::InProgress),
        "completed" => Ok(team_coord::TaskStatus::Completed),
        "blocked" => Err(FunctionCallError::RespondToModel(
            "Claude-style teammate task lists do not store blocked status; create or update a blocking task instead"
                .to_string(),
        )),
        other => Err(FunctionCallError::RespondToModel(format!(
            "unsupported teammate task status {other}; use pending, in_progress, or completed"
        ))),
    }
}

fn validate_store_task_claim(
    teams_root: &std::path::Path,
    team_name: &str,
    task_id: &str,
    member_agent_id: &str,
) -> Result<(), FunctionCallError> {
    let task = team_coord::get_task(teams_root, team_name, task_id)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to read team task: {err}"))
        })?
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!("team task {task_id} not found"))
        })?;
    if task.status == team_coord::TaskStatus::Completed {
        return Err(FunctionCallError::RespondToModel(format!(
            "team task {task_id} is already completed"
        )));
    }
    if let Some(owner) = task.owner.as_deref()
        && owner != member_agent_id
    {
        return Err(FunctionCallError::RespondToModel(format!(
            "team task {task_id} is owned by {owner}"
        )));
    }
    for dependency in &task.blocked_by {
        let dependency_task = team_coord::get_task(teams_root, team_name, dependency)
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read dependency task: {err}"))
            })?
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "team task {task_id} depends on missing task {dependency}"
                ))
            })?;
        if dependency_task.status != team_coord::TaskStatus::Completed {
            return Err(FunctionCallError::RespondToModel(format!(
                "team task {task_id} depends on incomplete task {dependency}"
            )));
        }
    }
    Ok(())
}

fn resolve_store_member_agent_id(
    teams_root: &std::path::Path,
    team_name: &str,
    member_id_or_name: &str,
) -> Result<String, FunctionCallError> {
    let config = team_store::read_config(teams_root, team_name).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to read team config: {err}"))
    })?;
    let Some(config) = config else {
        return Err(FunctionCallError::RespondToModel(format!(
            "team '{team_name}' is not registered"
        )));
    };
    let sanitized = crate::team_backends::spawn::sanitize_agent_name(member_id_or_name);
    config
        .members
        .into_iter()
        .find(|member| {
            member.member_id.as_deref() == Some(member_id_or_name)
                || member.agent_id == member_id_or_name
                || member.name == sanitized
        })
        .map(|member| member.agent_id)
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "team member {member_id_or_name} not found in team '{team_name}'"
            ))
        })
}

async fn team_message_list(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
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
    let mut messages = team_result(
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

    // Cross-process replies from split-pane teammates arrive in the lead's
    // on-disk inbox (their `team_send` cannot reach this process's in-memory
    // registry), so surface them here too when the lead asks for lead/all
    // messages. The TUI separately injects them via its inbox poller.
    if matches!(args.target.as_deref(), Some("all") | Some("lead") | None)
        && let Ok(team_name) = registry_team_name(session.as_ref(), team_id).await
    {
        let teams_root = teams_root_for_turn(turn.as_ref());
        let inbox = team_store::read_mailbox(&teams_root, &team_name, team_store::TEAM_LEAD_NAME)
            .unwrap_or_default();
        for entry in inbox {
            if !is_model_visible_lead_mailbox_entry(&entry) {
                continue;
            }
            messages.push(crate::team::TeamMessage {
                id: ThreadId::new(),
                team_id,
                sender: crate::team::TeamMessageEndpoint::Member(ThreadId::new()),
                target: crate::team::TeamMessageEndpoint::Lead(session.thread_id),
                target_member_id: None,
                content: entry.text,
                items: Vec::new(),
                submitted_id: None,
                delivery_mode: TeamMessageDeliveryMode::Queue,
                delivery_status: crate::team::TeamMessageDeliveryStatus::Submitted,
                created_at: unix_millis(),
            });
        }
    }

    json_output(
        &TeamMessageListResult { messages },
        Some(true),
        "team_message_list",
    )
}

fn is_model_visible_lead_mailbox_entry(entry: &team_store::TeammateMessage) -> bool {
    team_coord::parse_idle_notification(&entry.text).is_none()
}

fn claude_recipient_display_name(to: &str) -> String {
    if to.eq_ignore_ascii_case("lead") {
        team_store::TEAM_LEAD_NAME.to_string()
    } else {
        to.to_string()
    }
}

fn team_member_color(
    teams_root: &std::path::Path,
    team_name: &str,
    member_name: &str,
) -> Option<String> {
    team_store::read_config(teams_root, team_name)
        .ok()
        .flatten()
        .and_then(|config| {
            config
                .members
                .into_iter()
                .find(|member| member.name.eq_ignore_ascii_case(member_name))
                .and_then(|member| member.color)
        })
}

fn mailbox_broadcast_recipients(output_text: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(output_text)
        .ok()
        .and_then(|value| {
            value
                .get("targets")
                .or_else(|| value.get("recipients"))
                .and_then(serde_json::Value::as_array)
                .map(|targets| {
                    targets
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                })
        })
        .unwrap_or_default()
}

fn claude_broadcast_message(recipients: &[String]) -> String {
    if recipients.is_empty() {
        "No teammates to broadcast to (you are the only team member)".to_string()
    } else {
        format!(
            "Message broadcast to {} teammate(s): {}",
            recipients.len(),
            recipients.join(", ")
        )
    }
}

fn claude_send_message_result(
    result: ClaudeSendMessageResult,
) -> Result<FunctionToolOutput, FunctionCallError> {
    json_output(&result, Some(true), "SendMessage")
}

fn claude_plan_approval_permission_mode(turn: &TurnContext) -> String {
    match turn.collaboration_mode.mode {
        ModeKind::Plan => "default",
        ModeKind::Default | ModeKind::PairProgramming | ModeKind::Execute => {
            match turn.approval_policy.value() {
                AskForApproval::Never => "bypassPermissions",
                AskForApproval::UnlessTrusted
                | AskForApproval::OnFailure
                | AskForApproval::OnRequest
                | AskForApproval::Granular(_) => "default",
            }
        }
    }
    .to_string()
}

async fn claude_send_message(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: ClaudeSendMessageArgs = parse_arguments(&arguments)?;
    let to = validate_claude_recipient(args.to)?;

    match args.message {
        ClaudeSendMessageContent::Text(message) => {
            let context = claude_team_context(session.as_ref(), turn.as_ref()).await?;
            let team_id = context.is_team_lead.then(|| context.team_id.to_string());
            let summary = args.summary;
            let output = team_send_args(
                Arc::clone(&session),
                Arc::clone(&turn),
                TeamSendArgs {
                    team_id,
                    to: Some(to.clone()),
                    summary: summary.clone(),
                    sender_member_id: None,
                    target: None,
                    member_id: None,
                    member_name: None,
                    delivery_mode: None,
                    message: Some(message.clone()),
                    items: None,
                },
                "SendMessage",
            )
            .await?;
            let teams_root = teams_root_for_turn(turn.as_ref());
            let (message_text, recipients, routing_target, target_color) = if to == "*" {
                let recipients = mailbox_broadcast_recipients(&output.into_text());
                (
                    claude_broadcast_message(&recipients),
                    Some(recipients),
                    "@team".to_string(),
                    None,
                )
            } else {
                let recipient = claude_recipient_display_name(&to);
                let target_color = team_member_color(&teams_root, &context.team_name, &recipient);
                (
                    format!("Message sent to {recipient}'s inbox"),
                    None,
                    format!("@{recipient}"),
                    target_color,
                )
            };
            let routing = if recipients.as_ref().is_some_and(Vec::is_empty) {
                None
            } else {
                Some(ClaudeMessageRouting {
                    sender: context.sender_name,
                    sender_color: context.sender_color,
                    target: routing_target,
                    target_color,
                    summary,
                    content: Some(message),
                })
            };
            claude_send_message_result(ClaudeSendMessageResult {
                success: true,
                message: message_text,
                routing,
                recipients,
                request_id: None,
                target: None,
            })
        }
        ClaudeSendMessageContent::Structured(message) => {
            if to == "*" {
                return Err(FunctionCallError::RespondToModel(
                    "structured messages cannot be broadcast (to: \"*\")".to_string(),
                ));
            }
            let context = claude_team_context(session.as_ref(), turn.as_ref()).await?;
            let teams_root = teams_root_for_turn(turn.as_ref());
            match message {
                ClaudeStructuredMessage::ShutdownRequest { reason } => {
                    let request_id = format!("shutdown-{}-{}", to, unix_millis());
                    team_coord::send_shutdown_request(
                        &teams_root,
                        &context.team_name,
                        &to,
                        &context.sender_name,
                        &request_id,
                        reason,
                        context.sender_color,
                    )
                    .map_err(|err| {
                        FunctionCallError::RespondToModel(format!(
                            "failed to send shutdown request: {err}"
                        ))
                    })?;
                    claude_send_message_result(ClaudeSendMessageResult {
                        success: true,
                        message: format!("Shutdown request sent to {to}. Request ID: {request_id}"),
                        routing: None,
                        recipients: None,
                        request_id: Some(request_id),
                        target: Some(to),
                    })
                }
                ClaudeStructuredMessage::ShutdownResponse {
                    request_id,
                    approve,
                    reason,
                } => {
                    if to != team_store::TEAM_LEAD_NAME {
                        return Err(FunctionCallError::RespondToModel(format!(
                            "shutdown_response must be sent to \"{}\"",
                            team_store::TEAM_LEAD_NAME
                        )));
                    }
                    if !approve && reason.as_deref().map(str::trim).is_none_or(str::is_empty) {
                        return Err(FunctionCallError::RespondToModel(
                            "reason is required when rejecting a shutdown request".to_string(),
                        ));
                    }
                    if approve {
                        write_json_mailbox_message(
                            &teams_root,
                            &context.team_name,
                            team_store::TEAM_LEAD_NAME,
                            &context.sender_name,
                            &ClaudeShutdownApprovedMessage {
                                kind: "shutdown_approved",
                                request_id: request_id.clone(),
                                from: context.sender_name.clone(),
                                timestamp: team_coord::now_rfc3339(),
                                pane_id: None,
                                backend_type: None,
                            },
                            context.sender_color,
                        )?;
                        claude_send_message_result(ClaudeSendMessageResult {
                            success: true,
                            message: format!(
                                "Shutdown approved. Sent confirmation to team-lead. Agent {} is now exiting.",
                                context.sender_name
                            ),
                            routing: None,
                            recipients: None,
                            request_id: Some(request_id),
                            target: None,
                        })
                    } else {
                        let reason = reason.unwrap_or_default();
                        write_json_mailbox_message(
                            &teams_root,
                            &context.team_name,
                            team_store::TEAM_LEAD_NAME,
                            &context.sender_name,
                            &ClaudeShutdownRejectedMessage {
                                kind: "shutdown_rejected",
                                request_id: request_id.clone(),
                                from: context.sender_name.clone(),
                                reason: reason.clone(),
                                timestamp: team_coord::now_rfc3339(),
                            },
                            context.sender_color,
                        )?;
                        claude_send_message_result(ClaudeSendMessageResult {
                            success: true,
                            message: format!(
                                "Shutdown rejected. Reason: \"{reason}\". Continuing to work."
                            ),
                            routing: None,
                            recipients: None,
                            request_id: Some(request_id),
                            target: None,
                        })
                    }
                }
                ClaudeStructuredMessage::PlanApprovalResponse {
                    request_id,
                    approve,
                    feedback,
                } => {
                    if !context.is_team_lead {
                        return Err(FunctionCallError::RespondToModel(
                            "Only the team lead can approve plans. Teammates cannot approve their own or other plans."
                                .to_string(),
                        ));
                    }
                    let feedback = if approve {
                        feedback
                    } else {
                        Some(feedback.unwrap_or_else(|| "Plan needs revision".to_string()))
                    };
                    write_json_mailbox_message(
                        &teams_root,
                        &context.team_name,
                        &to,
                        team_store::TEAM_LEAD_NAME,
                        &ClaudePlanApprovalResponseMessage {
                            kind: "plan_approval_response",
                            request_id: request_id.clone(),
                            approved: approve,
                            feedback: feedback.clone(),
                            timestamp: team_coord::now_rfc3339(),
                            permission_mode: approve
                                .then(|| claude_plan_approval_permission_mode(turn.as_ref())),
                        },
                        None,
                    )?;
                    let content = if approve {
                        format!(
                            "Plan approved for {to}. They will receive the approval and can proceed with implementation."
                        )
                    } else {
                        format!(
                            "Plan rejected for {to} with feedback: \"{}\"",
                            feedback.unwrap_or_default()
                        )
                    };
                    claude_send_message_result(ClaudeSendMessageResult {
                        success: true,
                        message: content,
                        routing: None,
                        recipients: None,
                        request_id: Some(request_id),
                        target: None,
                    })
                }
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeShutdownApprovedMessage {
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: String,
    from: String,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend_type: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeShutdownRejectedMessage {
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: String,
    from: String,
    reason: String,
    timestamp: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudePlanApprovalResponseMessage {
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: String,
    approved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback: Option<String>,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_mode: Option<String>,
}

fn validate_claude_recipient(to: String) -> Result<String, FunctionCallError> {
    let to = to.trim();
    if to.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "to must not be empty".to_string(),
        ));
    }
    if to.contains('@') {
        return Err(FunctionCallError::RespondToModel(
            "to must be a bare teammate name or \"*\" — there is only one team per session"
                .to_string(),
        ));
    }
    Ok(to.to_string())
}

fn write_json_mailbox_message<T: Serialize>(
    teams_root: &std::path::Path,
    team_name: &str,
    recipient: &str,
    sender: &str,
    message: &T,
    color: Option<String>,
) -> Result<(), FunctionCallError> {
    let text = serde_json::to_string(message).map_err(|err| {
        FunctionCallError::Fatal(format!(
            "failed to serialize structured Teams message: {err}"
        ))
    })?;
    team_store::write_to_mailbox(
        teams_root,
        team_name,
        recipient,
        team_store::TeammateMessage {
            from: sender.to_string(),
            text,
            timestamp: team_store::now_timestamp(),
            read: false,
            color,
            summary: None,
        },
    )
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to deliver to {recipient} mailbox: {err}"
        ))
    })
}

async fn claude_task_create(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: ClaudeTaskCreateArgs = parse_arguments(&arguments)?;
    let context = claude_team_context(session.as_ref(), turn.as_ref()).await?;
    let subject = non_empty(args.subject, "task subject")?;
    let description = non_empty(args.description, "task description")?;
    let mut task = team_coord::Task::new(subject.clone(), description);
    task.active_form = optional_non_empty(args.active_form, "active form")?;
    task.metadata = args.metadata;
    let teams_root = teams_root_for_turn(turn.as_ref());
    let id = team_coord::create_task(&teams_root, &context.team_name, task).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to create team task: {err}"))
    })?;

    Ok(FunctionToolOutput::from_text(
        format!("Task #{id} created successfully: {subject}"),
        Some(true),
    ))
}

async fn claude_task_update(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: ClaudeTaskUpdateArgs = parse_arguments(&arguments)?;
    let ClaudeTaskUpdateArgs {
        task_id,
        subject,
        description,
        active_form,
        status,
        add_blocks,
        add_blocked_by,
        owner,
        metadata,
    } = args;
    let context = claude_team_context(session.as_ref(), turn.as_ref()).await?;
    let teams_root = teams_root_for_turn(turn.as_ref());
    let task_id = non_empty(task_id, "task id")?;
    let Some(existing_task) = team_coord::get_task(&teams_root, &context.team_name, &task_id)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to read team task: {err}"))
        })?
    else {
        return Ok(FunctionToolOutput::from_text(
            format!("Task #{task_id} not found"),
            Some(true),
        ));
    };

    let status = status.as_deref().map(claude_task_status).transpose()?;
    if matches!(status, Some(ClaudeTaskStatusUpdate::Deleted)) {
        team_coord::delete_task(&teams_root, &context.team_name, &task_id).map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to delete team task: {err}"))
        })?;
        remove_deleted_task_references(&teams_root, &context.team_name, &task_id)?;
        return Ok(FunctionToolOutput::from_text(
            format!("Updated task #{task_id} deleted"),
            Some(true),
        ));
    }

    let subject = optional_non_empty(subject, "task subject")?;
    let description = optional_non_empty(description, "task description")?;
    let active_form = optional_non_empty(active_form, "active form")?;
    let owner = optional_non_empty(owner, "task owner")?;
    let mut updated_fields = Vec::new();
    if subject
        .as_ref()
        .is_some_and(|subject| subject != &existing_task.subject)
    {
        updated_fields.push("subject");
    }
    if description
        .as_ref()
        .is_some_and(|description| description != &existing_task.description)
    {
        updated_fields.push("description");
    }
    if active_form.as_ref() != existing_task.active_form.as_ref() && active_form.is_some() {
        updated_fields.push("activeForm");
    }
    if owner.as_ref() != existing_task.owner.as_ref() && owner.is_some() {
        updated_fields.push("owner");
    }
    if metadata.is_some() {
        updated_fields.push("metadata");
    }
    if let Some(ClaudeTaskStatusUpdate::Stored(status)) = status
        && status != existing_task.status
    {
        updated_fields.push("status");
    }

    let updated_task = team_coord::update_task(&teams_root, &context.team_name, &task_id, |task| {
        if let Some(subject) = subject {
            task.subject = subject;
        }
        if let Some(description) = description {
            task.description = description;
        }
        if let Some(active_form) = active_form {
            task.active_form = Some(active_form);
        }
        if let Some(owner) = owner {
            task.owner = Some(owner);
        }
        if let Some(metadata) = metadata {
            let mut merged = task.metadata.take().unwrap_or_default();
            for (key, value) in metadata {
                if value.is_null() {
                    merged.remove(&key);
                } else {
                    merged.insert(key, value);
                }
            }
            task.metadata = Some(merged);
        }
        if let Some(ClaudeTaskStatusUpdate::Stored(status)) = status {
            task.status = status;
            if status == team_coord::TaskStatus::InProgress
                && task.owner.is_none()
                && !context.is_team_lead
            {
                task.owner = Some(context.sender_name.clone());
            }
        }
    })
    .map_err(|err| FunctionCallError::RespondToModel(format!("failed to update team task: {err}")))?
    .ok_or_else(|| FunctionCallError::RespondToModel(format!("team task {task_id} not found")))?;

    if let Some(blocks) = add_blocks {
        let mut changed = false;
        for blocked_id in blocks {
            changed |= block_store_task(&teams_root, &context.team_name, &task_id, &blocked_id)?;
        }
        if changed {
            updated_fields.push("blocks");
        }
    }
    if let Some(blocked_by) = add_blocked_by {
        let mut changed = false;
        for blocker_id in blocked_by {
            changed |= block_store_task(&teams_root, &context.team_name, &blocker_id, &task_id)?;
        }
        if changed {
            updated_fields.push("blockedBy");
        }
    }

    let mut content = format!("Updated task #{} {}", task_id, updated_fields.join(", "));
    if updated_task.status == team_coord::TaskStatus::Completed && !context.is_team_lead {
        content.push_str(
            "\n\nTask completed. Call TaskList now to find your next available task or see if your work unblocked others.",
        );
    }
    Ok(FunctionToolOutput::from_text(content, Some(true)))
}

async fn claude_task_list(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let _args: EmptyArgs = parse_arguments(&arguments)?;
    let context = claude_team_context(session.as_ref(), turn.as_ref()).await?;
    let teams_root = teams_root_for_turn(turn.as_ref());
    let tasks = sorted_visible_store_tasks(&teams_root, &context.team_name)?;
    Ok(FunctionToolOutput::from_text(
        format_claude_task_list(&tasks),
        Some(true),
    ))
}

async fn claude_task_get(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: ClaudeTaskGetArgs = parse_arguments(&arguments)?;
    let context = claude_team_context(session.as_ref(), turn.as_ref()).await?;
    let task_id = non_empty(args.task_id, "task id")?;
    let teams_root = teams_root_for_turn(turn.as_ref());
    let task = team_coord::get_task(&teams_root, &context.team_name, &task_id).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to read team task: {err}"))
    })?;
    Ok(FunctionToolOutput::from_text(
        format_claude_task_get(task.as_ref()),
        Some(true),
    ))
}

#[derive(Clone, Copy)]
enum ClaudeTaskStatusUpdate {
    Stored(team_coord::TaskStatus),
    Deleted,
}

fn claude_task_status(status: &str) -> Result<ClaudeTaskStatusUpdate, FunctionCallError> {
    match status {
        "pending" => Ok(ClaudeTaskStatusUpdate::Stored(
            team_coord::TaskStatus::Pending,
        )),
        "in_progress" => Ok(ClaudeTaskStatusUpdate::Stored(
            team_coord::TaskStatus::InProgress,
        )),
        "completed" => Ok(ClaudeTaskStatusUpdate::Stored(
            team_coord::TaskStatus::Completed,
        )),
        "deleted" => Ok(ClaudeTaskStatusUpdate::Deleted),
        other => Err(FunctionCallError::RespondToModel(format!(
            "unsupported task status {other}; use pending, in_progress, completed, or deleted"
        ))),
    }
}

fn sorted_visible_store_tasks(
    teams_root: &std::path::Path,
    team_name: &str,
) -> Result<Vec<team_coord::Task>, FunctionCallError> {
    let mut tasks = team_coord::list_tasks(teams_root, team_name)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to list team tasks: {err}"))
        })?
        .into_iter()
        .filter(|task| !task_is_internal(task))
        .collect::<Vec<_>>();
    tasks.sort_by_key(|task| task.id.parse::<u64>().unwrap_or(u64::MAX));
    Ok(tasks)
}

fn task_is_internal(task: &team_coord::Task) -> bool {
    task.metadata
        .as_ref()
        .and_then(|metadata| metadata.get("_internal"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn format_claude_task_list(tasks: &[team_coord::Task]) -> String {
    if tasks.is_empty() {
        return "No tasks found".to_string();
    }
    let resolved = tasks
        .iter()
        .filter(|task| task.status == team_coord::TaskStatus::Completed)
        .map(|task| task.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    tasks
        .iter()
        .map(|task| {
            let owner = task
                .owner
                .as_ref()
                .map(|owner| format!(" ({owner})"))
                .unwrap_or_default();
            let blocked_by = task
                .blocked_by
                .iter()
                .filter(|id| !resolved.contains(id.as_str()))
                .map(|id| format!("#{id}"))
                .collect::<Vec<_>>();
            let blocked = if blocked_by.is_empty() {
                String::new()
            } else {
                format!(" [blocked by {}]", blocked_by.join(", "))
            };
            format!(
                "#{} [{}] {}{}{}",
                task.id,
                store_task_status_name(task.status),
                task.subject,
                owner,
                blocked
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_claude_task_get(task: Option<&team_coord::Task>) -> String {
    let Some(task) = task else {
        return "Task not found".to_string();
    };
    let mut lines = vec![
        format!("Task #{}: {}", task.id, task.subject),
        format!("Status: {}", store_task_status_name(task.status)),
        format!("Description: {}", task.description),
    ];
    if !task.blocked_by.is_empty() {
        lines.push(format!(
            "Blocked by: {}",
            task.blocked_by
                .iter()
                .map(|id| format!("#{id}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !task.blocks.is_empty() {
        lines.push(format!(
            "Blocks: {}",
            task.blocks
                .iter()
                .map(|id| format!("#{id}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.join("\n")
}

fn store_task_status_name(status: team_coord::TaskStatus) -> &'static str {
    match status {
        team_coord::TaskStatus::Pending => "pending",
        team_coord::TaskStatus::InProgress => "in_progress",
        team_coord::TaskStatus::Completed => "completed",
    }
}

fn block_store_task(
    teams_root: &std::path::Path,
    team_name: &str,
    from_task_id: &str,
    to_task_id: &str,
) -> Result<bool, FunctionCallError> {
    let from_task = team_coord::get_task(teams_root, team_name, from_task_id).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to read blocking task: {err}"))
    })?;
    let to_task = team_coord::get_task(teams_root, team_name, to_task_id).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to read blocked task: {err}"))
    })?;
    let (Some(from_task), Some(to_task)) = (from_task, to_task) else {
        return Ok(false);
    };
    let mut changed = false;
    if !from_task.blocks.iter().any(|id| id == to_task_id) {
        team_coord::update_task(teams_root, team_name, from_task_id, |task| {
            task.blocks.push(to_task_id.to_string());
        })
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to update blocking task: {err}"))
        })?;
        changed = true;
    }
    if !to_task.blocked_by.iter().any(|id| id == from_task_id) {
        team_coord::update_task(teams_root, team_name, to_task_id, |task| {
            task.blocked_by.push(from_task_id.to_string());
        })
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to update blocked task: {err}"))
        })?;
        changed = true;
    }
    Ok(changed)
}

fn remove_deleted_task_references(
    teams_root: &std::path::Path,
    team_name: &str,
    task_id: &str,
) -> Result<(), FunctionCallError> {
    let tasks = team_coord::list_tasks(teams_root, team_name).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to list team tasks: {err}"))
    })?;
    for task in tasks {
        if task.blocks.iter().any(|id| id == task_id)
            || task.blocked_by.iter().any(|id| id == task_id)
        {
            team_coord::update_task(teams_root, team_name, &task.id, |task| {
                task.blocks.retain(|id| id != task_id);
                task.blocked_by.retain(|id| id != task_id);
            })
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to remove deleted task references: {err}"
                ))
            })?;
        }
    }
    Ok(())
}

async fn team_task_create(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamTaskCreateArgs = parse_arguments(&arguments)?;
    if let Some(identity) = crate::team::teammate_identity() {
        return teammate_team_task_create(turn.as_ref(), identity, args).await;
    }
    let team_id = required_team_id(args.team_id)?;
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
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamTaskUpdateArgs = parse_arguments(&arguments)?;
    if let Some(identity) = crate::team::teammate_identity() {
        return teammate_team_task_update(turn.as_ref(), identity, args).await;
    }
    let team_id = required_team_id(args.team_id)?;
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
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamTaskClaimArgs = parse_arguments(&arguments)?;
    if let Some(identity) = crate::team::teammate_identity() {
        return teammate_team_task_claim(turn.as_ref(), identity, args).await;
    }
    let team_id = required_team_id(args.team_id)?;
    let task_id = id_from_str("task", &args.task_id)?;
    let member_id = args.member_id.ok_or_else(|| {
        FunctionCallError::RespondToModel("member_id is required for team_task_claim".to_string())
    })?;
    let member_id = id_from_str("member", &member_id)?;
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
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamTaskListArgs = parse_arguments(&arguments)?;
    if let Some(identity) = crate::team::teammate_identity() {
        return teammate_team_task_list(turn.as_ref(), identity, args).await;
    }
    let team_id = required_team_id(args.team_id)?;
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
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamMemberIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_lead(session.as_ref(), team_id, "team_member_stop").await?;
    let member_id = id_from_str("member", &args.member_id)?;
    let team_name = registry_team_name(session.as_ref(), team_id).await?;
    let teams_root = teams_root_for_turn(turn.as_ref());
    let process_member =
        active_store_member_by_id(&teams_root, &team_name, &member_id.to_string())?;
    let registry = session.services.agent_control.team_registry();
    let snapshot = team_result(
        session.as_ref(),
        Some(team_id),
        registry
            .stop_member(team_id, member_id, &session.services.agent_control)
            .await,
    )
    .await?;
    if let Some(member) = process_member {
        send_process_shutdown_request(&teams_root, &team_name, &member)?;
        mark_store_members_inactive(&teams_root, &team_name, &[member_id.to_string()])?;
    }
    json_output(
        &TeamMemberStopResult { snapshot },
        Some(true),
        "team_member_stop",
    )
}

async fn team_stop(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    require_team_lead(session.as_ref(), team_id, "team_stop").await?;
    let team_name = registry_team_name(session.as_ref(), team_id).await?;
    let teams_root = teams_root_for_turn(turn.as_ref());
    let process_members = active_store_members(&teams_root, &team_name)?;
    let registry = session.services.agent_control.team_registry();
    let snapshot = team_result(
        session.as_ref(),
        Some(team_id),
        registry
            .stop_team(team_id, &session.services.agent_control)
            .await,
    )
    .await?;
    for member in &process_members {
        send_process_shutdown_request(&teams_root, &team_name, member)?;
    }
    let member_ids = process_members
        .iter()
        .filter_map(|member| member.member_id.clone())
        .collect::<Vec<_>>();
    if !member_ids.is_empty() {
        mark_store_members_inactive(&teams_root, &team_name, &member_ids)?;
    }
    json_output(&TeamStopResult { snapshot }, Some(true), "team_stop")
}

fn parse_team_input(
    message: Option<String>,
    items: Option<Vec<UserInput>>,
) -> Result<Vec<UserInput>, FunctionCallError> {
    let items = if message.is_some() && matches!(items.as_ref(), Some(items) if items.is_empty()) {
        None
    } else {
        items
    };
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
                    "Empty message can't be sent to a teammate".to_string(),
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
    items
        .iter()
        .filter_map(|item| match item {
            UserInput::Text { text, .. } => Some(text.clone()),
            UserInput::Image { .. } => Some("[image]".to_string()),
            UserInput::LocalImage { path, .. } => Some(format!("[local_image:{}]", path.display())),
            // Skill and mention items are injected into model context elsewhere in
            // normal turns; they are not user-visible Teams mailbox text.
            UserInput::Skill { .. } | UserInput::Mention { .. } => None,
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
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

fn required_team_id(id: Option<String>) -> Result<ThreadId, FunctionCallError> {
    let id =
        id.ok_or_else(|| FunctionCallError::RespondToModel("team_id is required".to_string()))?;
    id_from_str("team", &id)
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
    use codex_protocol::protocol::SubAgentSource;
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

    fn contains_flag_pair(flags: &[String], key_value: &str) -> bool {
        flags
            .windows(2)
            .any(|pair| pair[0] == "-c" && pair[1] == key_value)
    }

    fn contains_arg_pair(args: &[String], key: &str, value: &str) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == key && pair[1] == value)
    }

    #[test]
    fn initial_teammate_prompt_mailbox_message_is_plain_task_text() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let prompt = "Inspect issue #15 and report the minimal proof path.";

        write_initial_teammate_prompt_to_mailbox(codex_home.path(), "Rocket", "alice", prompt)
            .expect("write initial teammate prompt");

        let messages = team_store::read_mailbox(codex_home.path(), "Rocket", "alice")
            .expect("read teammate inbox");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from, team_store::TEAM_LEAD_NAME);
        assert_eq!(messages[0].text, prompt);
        assert!(!messages[0].read);
        assert_eq!(messages[0].summary, None);
        for forbidden in [
            "Codex Teams context:",
            "team_id:",
            "member_id:",
            "You are an independent Codex Teams teammate",
            "Do not create teams or spawn teammates",
        ] {
            assert!(
                !messages[0].text.contains(forbidden),
                "initial teammate prompt must not expose legacy Teams envelope field {forbidden:?}"
            );
        }
    }

    #[test]
    fn teammate_process_model_override_uses_config_override() {
        let mut flags = vec![
            "teammate".to_string(),
            "--enable".to_string(),
            "teams".to_string(),
        ];

        append_teammate_model_override(&mut flags, r#"gpt 5 "preview""#);

        assert_eq!(
            flags,
            vec![
                "teammate".to_string(),
                "--enable".to_string(),
                "teams".to_string(),
                "-c".to_string(),
                r#"model="gpt 5 \"preview\"""#.to_string(),
            ]
        );

        let override_arg = flags.last().expect("model override");
        let Some((key, value)) = override_arg.split_once('=') else {
            panic!("expected key=value override");
        };
        assert_eq!(key, "model");
        let parsed: toml::Table =
            toml::from_str(&format!("_x_ = {value}")).expect("valid TOML string value");
        assert_eq!(
            parsed.get("_x_").and_then(toml::Value::as_str),
            Some(r#"gpt 5 "preview""#)
        );
    }

    #[test]
    fn teammate_model_resolves_inherit_to_leader_model() {
        assert_eq!(
            resolve_teammate_model(Some("inherit"), "gpt-5-lead"),
            "gpt-5-lead"
        );
        assert_eq!(
            resolve_teammate_model(Some("gpt-5-worker"), "gpt-5-lead"),
            "gpt-5-worker"
        );
        assert_eq!(resolve_teammate_model(None, "gpt-5-lead"), "gpt-5-lead");
    }

    #[tokio::test]
    async fn teammate_launch_spec_inherits_codex_home_without_provider_cli_overrides() {
        let (_session, mut turn) = make_session_and_context().await;
        let codex_home = tempfile::tempdir().expect("codex home");
        let cwd = tempfile::tempdir().expect("cwd");
        let binary = cwd.path().join("bin").join("codex");
        let config = Arc::make_mut(&mut turn.config);
        config.codex_home =
            codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(codex_home.path())
                .expect("absolute codex home");
        config.cwd = codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(cwd.path())
            .expect("absolute cwd");
        config.model_provider_id = "custom".to_string();
        config.model_provider = ModelProviderInfo {
            name: "custom".to_string(),
            base_url: Some("https://gw2.oops.asia/v1".to_string()),
            env_key: Some("OPENAI_API_KEY".to_string()),
            wire_api: codex_model_provider_info::WireApi::Responses,
            requires_openai_auth: true,
            supports_websockets: true,
            ..Default::default()
        };
        let user_config_path = codex_home.path().join(codex_config::CONFIG_TOML_FILE);
        let user_config_toml = r#"
model_provider = "custom"

[model_providers.custom]
name = "custom"
base_url = "https://gw2.oops.asia/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = true
"#;
        std::fs::write(&user_config_path, user_config_toml).expect("write user config");
        let user_config = toml::from_str(user_config_toml).expect("user config toml");
        config.config_layer_stack = codex_config::ConfigLayerStack::new(
            vec![codex_config::ConfigLayerEntry::new(
                ConfigLayerSource::User {
                    file: codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(
                        &user_config_path,
                    )
                    .expect("absolute user config path"),
                    profile: None,
                },
                user_config,
            )],
            codex_config::ConfigRequirements::default(),
            codex_config::ConfigRequirementsToml::default(),
        )
        .expect("config layer stack");
        let inherited_model_override = format!(r#"model="{}""#, turn.model_info.slug);

        let spec = build_teammate_launch_spec_with_binary(
            &turn,
            "rocket",
            ThreadId::new(),
            "alice",
            Some("reviewer"),
            &[],
            &[UserInput::Text {
                text: "inspect this repo".to_string(),
                text_elements: Vec::new(),
            }],
            Some("inherit"),
            None,
            binary.clone(),
        )
        .expect("launch spec");

        assert_eq!(spec.binary, binary);
        assert_eq!(spec.cwd, cwd.path());
        assert_eq!(spec.agent_id, "alice@rocket");
        assert_eq!(spec.agent_name, "alice");
        assert_eq!(spec.prompt_text, "inspect this repo");
        assert_eq!(spec.first_turn_text, "inspect this repo");
        assert!(!spec.flags.contains(&"--prompt".to_string()));
        assert!(contains_arg_pair(&spec.flags, "--agent-id", "alice@rocket"));
        assert!(contains_arg_pair(&spec.flags, "--agent-name", "alice"));
        assert!(contains_arg_pair(&spec.flags, "--team-name", "rocket"));
        assert!(contains_arg_pair(&spec.flags, "--agent-type", "reviewer"));
        assert!(contains_arg_pair(&spec.flags, "--enable", "teams"));
        assert!(contains_flag_pair(&spec.flags, &inherited_model_override));
        assert!(
            !spec
                .flags
                .iter()
                .any(|flag| flag.starts_with("model_provider=")
                    || flag.starts_with("openai_base_url=")
                    || flag.starts_with("model_providers."))
        );
        assert!(!spec.flags.iter().any(|flag| flag.contains("gw2.oops.asia")));
        assert!(
            spec.env
                .contains(&("CODEX_TEAMMATE".to_string(), "1".to_string()))
        );
        assert!(spec.env.contains(&(
            "CODEX_HOME".to_string(),
            codex_home.path().to_string_lossy().into_owned()
        )));
        assert!(spec.env.contains(&(
            team_store::TEAM_STORE_ROOT_ENV_VAR.to_string(),
            codex_home.path().to_string_lossy().into_owned()
        )));
    }

    #[tokio::test]
    async fn teammate_launch_spec_forwards_active_config_profile() {
        let (_session, mut turn) = make_session_and_context().await;
        let codex_home = tempfile::tempdir().expect("codex home");
        let cwd = tempfile::tempdir().expect("cwd");
        let binary = cwd.path().join("bin").join("codex");
        let config = Arc::make_mut(&mut turn.config);
        config.codex_home =
            codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(codex_home.path())
                .expect("absolute codex home");
        config.cwd = codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(cwd.path())
            .expect("absolute cwd");
        config.model_provider_id = "custom".to_string();
        config.model_provider = ModelProviderInfo {
            name: "custom".to_string(),
            base_url: Some("https://gw2.oops.asia/v1".to_string()),
            env_key: Some("OPENAI_API_KEY".to_string()),
            wire_api: codex_model_provider_info::WireApi::Responses,
            requires_openai_auth: true,
            supports_websockets: true,
            ..Default::default()
        };
        let user_config_path = codex_home.path().join("work.config.toml");
        let user_config = toml::from_str(
            r#"
model_provider = "custom"

[model_providers.custom]
name = "custom"
base_url = "https://gw2.oops.asia/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = true
"#,
        )
        .expect("profile config toml");
        config.config_layer_stack = codex_config::ConfigLayerStack::new(
            vec![codex_config::ConfigLayerEntry::new(
                ConfigLayerSource::User {
                    file: codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(
                        &user_config_path,
                    )
                    .expect("absolute profile config path"),
                    profile: Some("work".to_string()),
                },
                user_config,
            )],
            codex_config::ConfigRequirements::default(),
            codex_config::ConfigRequirementsToml::default(),
        )
        .expect("config layer stack");

        let spec = build_teammate_launch_spec_with_binary(
            &turn,
            "rocket",
            ThreadId::new(),
            "alice",
            /*profile*/ None,
            &[],
            &[UserInput::Text {
                text: "inspect this repo".to_string(),
                text_elements: Vec::new(),
            }],
            Some("inherit"),
            None,
            binary,
        )
        .expect("launch spec");

        assert!(contains_arg_pair(&spec.flags, "--profile", "work"));
        assert!(
            spec.flags
                .iter()
                .position(|arg| arg == "--profile")
                .expect("profile flag")
                < spec
                    .flags
                    .iter()
                    .position(|arg| arg == "teammate")
                    .expect("teammate subcommand")
        );
        assert!(
            !spec
                .flags
                .iter()
                .any(|flag| flag.starts_with("model_provider=")
                    || flag.starts_with("openai_base_url=")
                    || flag.starts_with("model_providers."))
        );
    }

    #[tokio::test]
    async fn teammate_launch_spec_does_not_override_provider_from_resolved_lead_config() {
        let (_session, mut turn) = make_session_and_context().await;
        let codex_home = tempfile::tempdir().expect("codex home");
        let cwd = tempfile::tempdir().expect("cwd");
        let binary = cwd.path().join("bin").join("codex");
        let config = Arc::make_mut(&mut turn.config);
        config.codex_home =
            codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(codex_home.path())
                .expect("absolute codex home");
        config.cwd = codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(cwd.path())
            .expect("absolute cwd");
        config.model_provider_id = "mock".to_string();
        config.model_provider = ModelProviderInfo {
            name: "mock".to_string(),
            base_url: Some("http://127.0.0.1:12345/v1".to_string()),
            env_key: Some("PATH".to_string()),
            wire_api: codex_model_provider_info::WireApi::Responses,
            requires_openai_auth: false,
            supports_websockets: false,
            ..Default::default()
        };

        let spec = build_teammate_launch_spec_with_binary(
            &turn,
            "rocket",
            ThreadId::new(),
            "alice",
            /*profile*/ None,
            &[],
            &[UserInput::Text {
                text: "inspect this repo".to_string(),
                text_elements: Vec::new(),
            }],
            Some("inherit"),
            None,
            binary,
        )
        .expect("launch spec");

        assert!(
            !spec
                .flags
                .iter()
                .any(|flag| flag.starts_with("model_provider=")
                    || flag.starts_with("openai_base_url=")
                    || flag.starts_with("model_providers."))
        );
        assert!(
            !spec
                .flags
                .iter()
                .any(|flag| flag.contains("127.0.0.1:12345"))
        );
        assert!(
            spec.env
                .iter()
                .any(|(key, value)| key == "PATH" && !value.is_empty())
        );
    }

    #[test]
    fn teammate_plan_launch_flags_do_not_inherit_bypass() {
        let mut flags = vec!["teammate".to_string()];

        let plan_mode_required = append_teammate_launch_mode_flags(
            &mut flags,
            ModeKind::Plan,
            AskForApproval::Never,
            &SandboxPolicy::DangerFullAccess,
        );

        assert!(plan_mode_required);
        assert!(flags.contains(&"--plan-mode-required".to_string()));
        assert!(flags.contains(&"--dangerously-bypass-hook-trust".to_string()));
        assert!(contains_flag_pair(
            &flags,
            r#"approval_policy="on-request""#
        ));
        assert!(!contains_flag_pair(&flags, r#"approval_policy="never""#));
        assert!(!contains_flag_pair(
            &flags,
            r#"sandbox_mode="danger-full-access""#
        ));
    }

    #[test]
    fn teammate_plan_launch_flags_keep_restrictive_sandbox() {
        let mut flags = vec!["teammate".to_string()];

        let plan_mode_required = append_teammate_launch_mode_flags(
            &mut flags,
            ModeKind::Plan,
            AskForApproval::Never,
            &SandboxPolicy::ReadOnly {
                network_access: false,
            },
        );

        assert!(plan_mode_required);
        assert!(flags.contains(&"--plan-mode-required".to_string()));
        assert!(flags.contains(&"--dangerously-bypass-hook-trust".to_string()));
        assert!(contains_flag_pair(
            &flags,
            r#"approval_policy="on-request""#
        ));
        assert!(!contains_flag_pair(&flags, r#"approval_policy="never""#));
        assert!(contains_flag_pair(&flags, r#"sandbox_mode="read-only""#));
    }

    #[test]
    fn teammate_plan_launch_flags_keep_workspace_write_sandbox_mode() {
        let mut flags = vec!["teammate".to_string()];

        let plan_mode_required = append_teammate_launch_mode_flags(
            &mut flags,
            ModeKind::Plan,
            AskForApproval::Never,
            &SandboxPolicy::WorkspaceWrite {
                writable_roots: Vec::new(),
                network_access: false,
                exclude_tmpdir_env_var: true,
                exclude_slash_tmp: true,
            },
        );

        assert!(plan_mode_required);
        assert!(flags.contains(&"--plan-mode-required".to_string()));
        assert!(flags.contains(&"--dangerously-bypass-hook-trust".to_string()));
        assert!(contains_flag_pair(
            &flags,
            r#"approval_policy="on-request""#
        ));
        assert!(!contains_flag_pair(&flags, r#"approval_policy="never""#));
        assert!(contains_flag_pair(
            &flags,
            r#"sandbox_mode="workspace-write""#
        ));
        assert!(!contains_flag_pair(
            &flags,
            r#"sandbox_mode="danger-full-access""#
        ));
    }

    #[test]
    fn teammate_default_launch_flags_inherit_never_and_danger_full_access() {
        let mut flags = vec!["teammate".to_string()];

        let plan_mode_required = append_teammate_launch_mode_flags(
            &mut flags,
            ModeKind::Default,
            AskForApproval::Never,
            &SandboxPolicy::DangerFullAccess,
        );

        assert!(!plan_mode_required);
        assert!(!flags.contains(&"--plan-mode-required".to_string()));
        assert!(flags.contains(&"--dangerously-bypass-hook-trust".to_string()));
        assert!(contains_flag_pair(&flags, r#"approval_policy="never""#));
        assert!(contains_flag_pair(
            &flags,
            r#"sandbox_mode="danger-full-access""#
        ));
    }

    #[test]
    fn teammate_provider_env_keys_include_provider_auth_and_header_envs() {
        let provider = ModelProviderInfo {
            env_key: Some("OPENAI_API_KEY".to_string()),
            env_http_headers: Some(std::collections::HashMap::from([
                ("x-api-key".to_string(), "OPENAI_API_KEY".to_string()),
                ("x-org".to_string(), "OPENAI_ORG_ID".to_string()),
            ])),
            ..Default::default()
        };

        assert_eq!(
            teammate_provider_env_keys(&provider),
            vec!["OPENAI_API_KEY", "OPENAI_ORG_ID"]
        );
    }

    #[tokio::test]
    async fn teammate_spawn_requires_lead_auth_when_provider_requires_openai_auth() {
        let (_session, mut turn) = make_session_and_context().await;
        Arc::make_mut(&mut turn.config)
            .model_provider
            .requires_openai_auth = true;
        turn.auth_manager = None;

        let mut env = Vec::new();
        let err = apply_lead_auth_to_teammate_env(&turn, &mut env)
            .await
            .expect_err("missing auth must reject before pane creation");

        match err {
            FunctionCallError::RespondToModel(message) => {
                assert!(message.contains("requires OpenAI/Codex auth"));
            }
            other => panic!("expected model-readable auth readiness error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn teammate_spawn_accepts_lead_auth_when_provider_requires_openai_auth() {
        let (_session, mut turn) = make_session_and_context().await;
        Arc::make_mut(&mut turn.config)
            .model_provider
            .requires_openai_auth = true;

        let mut env = Vec::new();
        apply_lead_auth_to_teammate_env(&turn, &mut env)
            .await
            .expect("lead auth should satisfy teammate spawn readiness");
    }

    #[tokio::test]
    async fn teammate_auth_env_overwrites_inherited_api_keys_with_lead_api_key() {
        let (_session, mut turn) = make_session_and_context().await;
        let config = Arc::make_mut(&mut turn.config);
        config.model_provider.requires_openai_auth = true;
        config.model_provider.env_key = Some("CUSTOM_PROVIDER_API_KEY".to_string());
        turn.auth_manager = Some(codex_login::AuthManager::from_auth_for_testing(
            codex_login::CodexAuth::from_api_key("lead-auth-json-key"),
        ));
        let mut env = vec![
            (
                codex_login::CODEX_API_KEY_ENV_VAR.to_string(),
                "stale-codex-api-key".to_string(),
            ),
            (
                codex_login::OPENAI_API_KEY_ENV_VAR.to_string(),
                "stale-openai-api-key".to_string(),
            ),
            (
                "CUSTOM_PROVIDER_API_KEY".to_string(),
                "stale-provider-api-key".to_string(),
            ),
        ];

        apply_lead_auth_to_teammate_env(&turn, &mut env)
            .await
            .expect("lead api-key auth should be usable by spawned teammate");

        assert_eq!(
            env,
            vec![
                (
                    codex_login::CODEX_API_KEY_ENV_VAR.to_string(),
                    "lead-auth-json-key".to_string(),
                ),
                (
                    codex_login::OPENAI_API_KEY_ENV_VAR.to_string(),
                    "lead-auth-json-key".to_string(),
                ),
                (
                    "CUSTOM_PROVIDER_API_KEY".to_string(),
                    "lead-auth-json-key".to_string(),
                ),
            ]
        );
    }

    #[tokio::test]
    async fn teammate_auth_env_removes_inherited_api_keys_for_token_auth() {
        let (_session, mut turn) = make_session_and_context().await;
        let config = Arc::make_mut(&mut turn.config);
        config.model_provider.requires_openai_auth = true;
        config.model_provider.env_key = Some("CUSTOM_PROVIDER_API_KEY".to_string());
        turn.auth_manager = Some(codex_login::AuthManager::from_auth_for_testing(
            codex_login::CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        ));
        let mut env = vec![
            (
                codex_login::CODEX_API_KEY_ENV_VAR.to_string(),
                "stale-codex-api-key".to_string(),
            ),
            (
                codex_login::OPENAI_API_KEY_ENV_VAR.to_string(),
                "stale-openai-api-key".to_string(),
            ),
            (
                "CUSTOM_PROVIDER_API_KEY".to_string(),
                "stale-provider-api-key".to_string(),
            ),
            ("PATH".to_string(), "/bin".to_string()),
        ];

        apply_lead_auth_to_teammate_env(&turn, &mut env)
            .await
            .expect("lead token auth should satisfy teammate spawn readiness");

        assert_eq!(env, vec![("PATH".to_string(), "/bin".to_string())]);
    }

    #[derive(Debug, Deserialize)]
    struct TestCreateTeamResult {
        team: crate::team::Team,
        team_name: String,
        team_file_path: String,
        lead_agent_id: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TestClaudeCreateTeamResult {
        team_name: String,
        team_file_path: String,
        lead_agent_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct TestListTeamsResult {
        teams: Vec<crate::team::Team>,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamMemberStopResult {
        snapshot: crate::team::TeamSnapshot,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamStopResult {
        snapshot: crate::team::TeamSnapshot,
    }

    fn process_member_record(
        team: &crate::team::Team,
        member: &crate::team::TeamMember,
        name: &str,
        active: bool,
    ) -> team_store::TeamFileMember {
        team_store::TeamFileMember {
            agent_id: team_store::agent_id(name, &team.name),
            member_id: Some(member.id.to_string()),
            name: name.to_string(),
            color: Some("green".to_string()),
            joined_at: unix_millis(),
            tmux_pane_id: "%10".to_string(),
            backend_type: Some("tmux".to_string()),
            is_active: Some(active),
            ..Default::default()
        }
    }

    fn write_process_member_config(
        codex_home: &std::path::Path,
        team: &crate::team::Team,
        member_record: team_store::TeamFileMember,
    ) {
        let team_name = team.name.clone();
        team_store::update_config(codex_home, &team.name, move |config| {
            config.name = team_name.clone();
            config.team_id = Some(team.id.to_string());
            config.lead_agent_id = team_store::agent_id(team_store::TEAM_LEAD_NAME, &team_name);
            config.members.push(member_record);
        })
        .expect("write process member config");
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
    fn team_input_requires_one_non_empty_input_source() {
        assert!(parse_team_input(Some("hi".to_string()), None).is_ok());
        assert!(parse_team_input(Some("hi".to_string()), Some(Vec::new())).is_ok());
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
        assert!(parse_team_input(None, Some(Vec::new())).is_err());
        assert!(
            parse_team_input(
                Some("hi".to_string()),
                Some(vec![UserInput::Text {
                    text: "hi".to_string(),
                    text_elements: Vec::new(),
                }]),
            )
            .is_err()
        );
    }

    #[test]
    fn team_input_preview_excludes_skill_and_mention_markers() {
        let preview = input_preview(&[
            UserInput::Text {
                text: "do the task".to_string(),
                text_elements: Vec::new(),
            },
            UserInput::Skill {
                name: "truth-first-workflow".to_string(),
                path: std::path::PathBuf::from("/tmp/SKILL.md"),
            },
            UserInput::Mention {
                name: "connector".to_string(),
                path: "app://connector".to_string(),
            },
            UserInput::Image {
                image_url: "data:image/png;base64,abc".to_string(),
                detail: None,
            },
        ]);

        assert_eq!(preview, "do the task\n[image]");
    }

    #[test]
    fn lead_mailbox_filter_excludes_idle_notifications_only() {
        let idle = team_store::TeammateMessage {
            from: "alice".to_string(),
            text: serde_json::json!({
                "type": "idle_notification",
                "from": "alice",
                "timestamp": "2026-06-05T00:00:00.000Z",
                "idleReason": "available",
            })
            .to_string(),
            timestamp: "t".to_string(),
            read: false,
            color: None,
            summary: None,
        };
        let explicit = team_store::TeammateMessage {
            from: "alice".to_string(),
            text: "explicit update".to_string(),
            timestamp: "t".to_string(),
            read: false,
            color: None,
            summary: Some("done".to_string()),
        };

        assert!(!is_model_visible_lead_mailbox_entry(&idle));
        assert!(is_model_visible_lead_mailbox_entry(&explicit));
    }

    #[test]
    fn team_tool_search_info_is_explicit_teams_only() {
        for tool in TeamTool::ALL {
            let info = TeamHandler::for_tool(tool)
                .search_info()
                .expect("team tools should expose deferred search info");
            let source_description = info
                .source_info
                .as_ref()
                .and_then(|source| source.description.as_deref())
                .expect("team search info should describe its source");
            let output = match &info.entry.output {
                codex_tools::LoadableToolSpec::Function(function) => function,
                _ => panic!("expected Teams tool search output to be a function"),
            };
            let mut visible_descriptions = vec![output.description.as_str()];
            collect_schema_descriptions(&output.parameters, &mut visible_descriptions);

            assert!(info.entry.search_text.contains("team"));
            if matches!(tool, TeamTool::CreateTeam | TeamTool::ClaudeTeamCreate) {
                assert!(info.entry.search_text.contains("swarm"));
                assert!(info.entry.search_text.contains("团队"));
                assert!(info.entry.search_text.contains("队友"));
            } else if matches!(tool, TeamTool::TeamSpawnMember) {
                assert_eq!(info.entry.search_text, "team_spawn_member");
            }
            assert!(source_description.contains("explicit Codex Teams workspaces"));
            assert!(!output.description.is_empty());
            for forbidden_search_trigger in [
                "agent",
                "work",
                "working",
                "collaboration",
                "collaborate",
                "coordinate",
                "group",
            ] {
                assert!(
                    !info.entry.search_text.contains(forbidden_search_trigger),
                    "Teams search text must not match ordinary agent trigger {forbidden_search_trigger:?}"
                );
            }
            for forbidden_trigger in [
                "spawn_agent",
                "subagent",
                "sub-agent",
                "ordinary delegation",
                "parallel delegation",
                "parallel agent work",
                "multiple agents",
                "another agent",
                "other agents",
                "inter-agent",
                "spawn prompt",
                "teammate agent",
                "teammate agents",
            ] {
                assert!(
                    !info.entry.search_text.contains(forbidden_trigger),
                    "Teams search text must not match ordinary agent trigger {forbidden_trigger:?}"
                );
                assert!(
                    !source_description.contains(forbidden_trigger),
                    "Teams source description must not match ordinary agent trigger {forbidden_trigger:?}"
                );
                for visible_description in &visible_descriptions {
                    if matches!(tool, TeamTool::ClaudeTeamCreate)
                        && forbidden_trigger == "spawn_agent"
                    {
                        continue;
                    }
                    assert!(
                        !visible_description.contains(forbidden_trigger),
                        "Teams tool descriptions must not match ordinary agent trigger {forbidden_trigger:?}: {visible_description:?}"
                    );
                }
            }
        }
    }

    fn collect_schema_descriptions<'a>(
        schema: &'a codex_tools::JsonSchema,
        descriptions: &mut Vec<&'a str>,
    ) {
        if let Some(description) = schema.description.as_deref() {
            descriptions.push(description);
        }
        if let Some(items) = schema.items.as_deref() {
            collect_schema_descriptions(items, descriptions);
        }
        if let Some(properties) = schema.properties.as_ref() {
            for property in properties.values() {
                collect_schema_descriptions(property, descriptions);
            }
        }
        if let Some(any_of) = schema.any_of.as_ref() {
            for variant in any_of {
                collect_schema_descriptions(variant, descriptions);
            }
        }
        if let Some(defs) = schema.defs.as_ref() {
            for definition in defs.values() {
                collect_schema_descriptions(definition, descriptions);
            }
        }
        if let Some(definitions) = schema.definitions.as_ref() {
            for definition in definitions.values() {
                collect_schema_descriptions(definition, descriptions);
            }
        }
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
                json!({
                    "team_name": "parallel investigation",
                    "description": "Working on source parity",
                    "agent_type": "researcher"
                }),
            ))
            .await
            .expect("create team");
        let created: TestCreateTeamResult =
            serde_json::from_str(&text_output(created)).expect("create result");
        assert_eq!(created.team_name, "parallel investigation");
        assert_eq!(created.lead_agent_id, "team-lead@parallel investigation");
        assert!(
            created
                .team_file_path
                .ends_with("teams/parallel_investigation/config.json")
        );
        let config = team_store::read_config(turn.config.codex_home.as_path(), &created.team_name)
            .expect("read team config")
            .expect("team config should be written at create time");
        assert_eq!(config.name, created.team_name);
        assert_eq!(config.team_id, Some(created.team.id.to_string()));
        assert_eq!(
            config.description.as_deref(),
            Some("Working on source parity")
        );
        assert_eq!(config.lead_agent_id, created.lead_agent_id);
        assert_eq!(config.members.len(), 1);
        assert_eq!(config.members[0].name, team_store::TEAM_LEAD_NAME);
        assert_eq!(config.members[0].agent_type.as_deref(), Some("researcher"));
        assert!(
            team_store::tasks_dir(turn.config.codex_home.as_path(), &created.team_name).exists()
        );

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

    #[tokio::test]
    async fn claude_team_create_returns_exact_claude_result_shape() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let created = TeamHandler::for_tool(TeamTool::ClaudeTeamCreate)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "TeamCreate",
                json!({
                    "team_name": "rocket",
                    "description": "Claude-compatible envelope",
                    "agent_type": "researcher"
                }),
            ))
            .await
            .expect("TeamCreate");
        let created: TestClaudeCreateTeamResult =
            serde_json::from_str(&text_output(created)).expect("Claude TeamCreate result");

        assert_eq!(created.team_name, "rocket");
        assert_eq!(created.lead_agent_id, "team-lead@rocket");
        assert!(created.team_file_path.ends_with("teams/rocket/config.json"));
    }

    #[tokio::test]
    async fn claude_task_aliases_use_shared_task_files_and_text_results() {
        let (session, turn) = make_session_and_context().await;
        let registry = session.services.agent_control.team_registry();
        registry
            .create_team("Rocket".to_string(), session.thread_id)
            .await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let created = TeamHandler::for_tool(TeamTool::ClaudeTaskCreate)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "TaskCreate",
                json!({
                    "subject": "inspect parser",
                    "description": "find owner",
                    "activeForm": "Inspecting parser"
                }),
            ))
            .await
            .expect("TaskCreate");
        assert_eq!(
            text_output(created),
            "Task #1 created successfully: inspect parser"
        );

        TeamHandler::for_tool(TeamTool::ClaudeTaskCreate)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "TaskCreate",
                json!({
                    "subject": "implement parser",
                    "description": "patch owner"
                }),
            ))
            .await
            .expect("second TaskCreate");

        let updated = TeamHandler::for_tool(TeamTool::ClaudeTaskUpdate)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "TaskUpdate",
                json!({
                    "taskId": "2",
                    "status": "in_progress",
                    "owner": "alice",
                    "addBlockedBy": ["1"]
                }),
            ))
            .await
            .expect("TaskUpdate");
        assert_eq!(
            text_output(updated),
            "Updated task #2 owner, status, blockedBy"
        );

        let listed = TeamHandler::for_tool(TeamTool::ClaudeTaskList)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "TaskList",
                json!({}),
            ))
            .await
            .expect("TaskList");
        assert_eq!(
            text_output(listed),
            "#1 [pending] inspect parser\n#2 [in_progress] implement parser (alice) [blocked by #1]"
        );

        let task = TeamHandler::for_tool(TeamTool::ClaudeTaskGet)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "TaskGet",
                json!({"taskId": "1"}),
            ))
            .await
            .expect("TaskGet");
        assert_eq!(
            text_output(task),
            "Task #1: inspect parser\nStatus: pending\nDescription: find owner\nBlocks: #2"
        );
    }

    #[tokio::test]
    async fn claude_send_message_alias_routes_plain_and_structured_mailbox_messages() {
        let (session, turn) = make_session_and_context().await;
        let registry = session.services.agent_control.team_registry();
        let team = registry
            .create_team("Rocket".to_string(), session.thread_id)
            .await;
        let member = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &member, "alice", true),
        );
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let sent = TeamHandler::for_tool(TeamTool::ClaudeSendMessage)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "SendMessage",
                json!({
                    "to": "alice",
                    "summary": "assign parser",
                    "message": "start task #1"
                }),
            ))
            .await
            .expect("SendMessage plain text");
        let sent: serde_json::Value =
            serde_json::from_str(&text_output(sent)).expect("SendMessage JSON result");
        assert_eq!(sent["success"], true);
        assert_eq!(sent["message"], "Message sent to alice's inbox");
        assert_eq!(sent["routing"]["sender"], team_store::TEAM_LEAD_NAME);
        assert_eq!(sent["routing"]["target"], "@alice");
        assert_eq!(sent["routing"]["targetColor"], "green");
        assert_eq!(sent["routing"]["summary"], "assign parser");
        assert_eq!(sent["routing"]["content"], "start task #1");

        let broadcast = TeamHandler::for_tool(TeamTool::ClaudeSendMessage)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "SendMessage",
                json!({
                    "to": "*",
                    "summary": "announce",
                    "message": "stand by"
                }),
            ))
            .await
            .expect("SendMessage broadcast");
        let broadcast: serde_json::Value =
            serde_json::from_str(&text_output(broadcast)).expect("broadcast JSON result");
        assert_eq!(broadcast["success"], true);
        assert_eq!(
            broadcast["message"],
            "Message broadcast to 1 teammate(s): alice"
        );
        assert_eq!(broadcast["recipients"], json!(["alice"]));
        assert_eq!(broadcast["routing"]["target"], "@team");
        assert_eq!(broadcast["routing"]["summary"], "announce");
        assert_eq!(broadcast["routing"]["content"], "stand by");

        let shutdown = TeamHandler::for_tool(TeamTool::ClaudeSendMessage)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "SendMessage",
                json!({
                    "to": "alice",
                    "message": {
                        "type": "shutdown_request",
                        "reason": "wrap up"
                    }
                }),
            ))
            .await
            .expect("SendMessage shutdown request");
        let shutdown: serde_json::Value =
            serde_json::from_str(&text_output(shutdown)).expect("shutdown JSON result");
        assert!(
            shutdown["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("Shutdown request sent to alice.")),
            "shutdown output should match Claude SendMessage text"
        );
        assert_eq!(shutdown["success"], true);
        assert_eq!(shutdown["target"], "alice");
        assert!(
            shutdown["request_id"]
                .as_str()
                .is_some_and(|request_id| request_id.starts_with("shutdown-alice-"))
        );

        let inbox = team_store::read_mailbox(turn.config.codex_home.as_path(), &team.name, "alice")
            .expect("read alice inbox");
        assert_eq!(inbox.len(), 3);
        assert_eq!(inbox[0].text, "start task #1");
        assert_eq!(inbox[0].summary.as_deref(), Some("assign parser"));
        assert_eq!(inbox[1].text, "stand by");
        assert_eq!(inbox[1].summary.as_deref(), Some("announce"));
        let shutdown = team_coord::parse_shutdown_request(&inbox[2].text)
            .expect("structured shutdown request");
        assert_eq!(shutdown.from, team_store::TEAM_LEAD_NAME);
        assert_eq!(shutdown.reason.as_deref(), Some("wrap up"));
    }

    #[tokio::test]
    async fn claude_send_message_empty_broadcast_omits_routing() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        TeamHandler::for_tool(TeamTool::ClaudeTeamCreate)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "TeamCreate",
                json!({"team_name": "Rocket"}),
            ))
            .await
            .expect("TeamCreate");

        let broadcast = TeamHandler::for_tool(TeamTool::ClaudeSendMessage)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "SendMessage",
                json!({
                    "to": "*",
                    "summary": "announce",
                    "message": "stand by"
                }),
            ))
            .await
            .expect("SendMessage empty broadcast");
        let broadcast: serde_json::Value =
            serde_json::from_str(&text_output(broadcast)).expect("broadcast JSON result");

        assert_eq!(broadcast["success"], true);
        assert_eq!(
            broadcast["message"],
            "No teammates to broadcast to (you are the only team member)"
        );
        assert_eq!(broadcast["recipients"], json!([]));
        assert!(broadcast.get("routing").is_none());
    }

    async fn send_plan_approval_response(
        mode: ModeKind,
        approval_policy: AskForApproval,
        approve: bool,
    ) -> team_coord::PlanApprovalResponseMessage {
        let (session, mut turn) = make_session_and_context().await;
        turn.collaboration_mode.mode = mode;
        turn.approval_policy = codex_config::Constrained::allow_any(approval_policy);
        let registry = session.services.agent_control.team_registry();
        let team = registry
            .create_team("Rocket".to_string(), session.thread_id)
            .await;
        let member = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &member, "alice", true),
        );
        let payload = if approve {
            json!({
                "to": "alice",
                "message": {
                    "type": "plan_approval_response",
                    "request_id": "plan-1",
                    "approve": true
                }
            })
        } else {
            json!({
                "to": "alice",
                "message": {
                    "type": "plan_approval_response",
                    "request_id": "plan-1",
                    "approve": false,
                    "feedback": "revise the plan"
                }
            })
        };
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        TeamHandler::for_tool(TeamTool::ClaudeSendMessage)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "SendMessage",
                payload,
            ))
            .await
            .expect("SendMessage plan approval response");

        let inbox = team_store::read_mailbox(turn.config.codex_home.as_path(), &team.name, "alice")
            .expect("read alice inbox");
        assert_eq!(inbox.len(), 1);
        team_coord::parse_plan_approval_response(&inbox[0].text).expect("plan approval response")
    }

    #[tokio::test]
    async fn claude_send_message_plan_approval_inherits_lead_permission_mode() {
        let plan_lead = send_plan_approval_response(
            ModeKind::Plan,
            AskForApproval::Never,
            /*approve*/ true,
        )
        .await;
        assert_eq!(plan_lead.permission_mode.as_deref(), Some("default"));

        let bypass_lead = send_plan_approval_response(
            ModeKind::Default,
            AskForApproval::Never,
            /*approve*/ true,
        )
        .await;
        assert_eq!(
            bypass_lead.permission_mode.as_deref(),
            Some("bypassPermissions")
        );

        let rejected = send_plan_approval_response(
            ModeKind::Default,
            AskForApproval::Never,
            /*approve*/ false,
        )
        .await;
        assert!(!rejected.approved);
        assert_eq!(rejected.feedback.as_deref(), Some("revise the plan"));
        assert_eq!(rejected.permission_mode, None);
    }

    #[tokio::test]
    async fn create_team_rejects_second_active_team_for_same_lead() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        TeamHandler::for_tool(TeamTool::CreateTeam)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "create_team",
                json!({"team_name": "Rocket"}),
            ))
            .await
            .expect("first team");

        let result = TeamHandler::for_tool(TeamTool::CreateTeam)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "create_team",
                json!({"team_name": "Second"}),
            ))
            .await;
        let message = match result {
            Err(FunctionCallError::RespondToModel(message)) => message,
            Err(_) => panic!("expected model-visible duplicate-team error"),
            Ok(_) => panic!("second active team should be rejected"),
        };

        assert!(message.contains("Already leading team \"Rocket\""));
        assert!(message.contains("only manage one team at a time"));
    }

    #[tokio::test]
    async fn team_member_stop_sends_shutdown_to_process_member_and_marks_inactive() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let registry = session.services.agent_control.team_registry();
        let team = registry
            .create_team("Rocket".to_string(), session.thread_id)
            .await;
        let member = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        registry
            .register_process_member(team.id, member.clone())
            .await
            .expect("register process member");
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &member, "alice", true),
        );

        let stopped = TeamHandler::for_tool(TeamTool::TeamMemberStop)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_member_stop",
                json!({"team_id": team.id.to_string(), "member_id": member.id.to_string()}),
            ))
            .await
            .expect("stop process member");
        let stopped: TestTeamMemberStopResult =
            serde_json::from_str(&text_output(stopped)).expect("stop result");

        assert_eq!(
            stopped.snapshot.team.members[0].status,
            crate::team::TeamMemberStatus::Stopped
        );
        let config = team_store::read_config(turn.config.codex_home.as_path(), &team.name)
            .expect("read config")
            .expect("team config");
        assert_eq!(config.members[0].is_active, Some(false));
        let messages =
            team_store::read_mailbox(turn.config.codex_home.as_path(), &team.name, "alice")
                .expect("read teammate inbox");
        assert_eq!(messages.len(), 1);
        assert!(team_coord::parse_shutdown_request(&messages[0].text).is_some());
    }

    #[tokio::test]
    async fn team_stop_sends_shutdown_to_process_members_and_marks_inactive() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let registry = session.services.agent_control.team_registry();
        let team = registry
            .create_team("Rocket".to_string(), session.thread_id)
            .await;
        let member = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        registry
            .register_process_member(team.id, member.clone())
            .await
            .expect("register process member");
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &member, "alice", true),
        );

        let stopped = TeamHandler::for_tool(TeamTool::TeamStop)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_stop",
                json!({"team_id": team.id.to_string()}),
            ))
            .await
            .expect("stop team");
        let stopped: TestTeamStopResult =
            serde_json::from_str(&text_output(stopped)).expect("stop result");

        assert_eq!(
            stopped.snapshot.team.status,
            crate::team::TeamStatus::Stopped
        );
        let config = team_store::read_config(turn.config.codex_home.as_path(), &team.name)
            .expect("read config")
            .expect("team config");
        assert_eq!(config.members[0].is_active, Some(false));
        let messages =
            team_store::read_mailbox(turn.config.codex_home.as_path(), &team.name, "alice")
                .expect("read teammate inbox");
        assert_eq!(messages.len(), 1);
        assert!(team_coord::parse_shutdown_request(&messages[0].text).is_some());
    }

    #[tokio::test]
    async fn team_send_rejects_inactive_process_member_by_name() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let registry = session.services.agent_control.team_registry();
        let team = registry
            .create_team("Rocket".to_string(), session.thread_id)
            .await;
        let member = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &member, "alice", false),
        );

        let result = TeamHandler::for_tool(TeamTool::TeamSend)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({"team_id": team.id.to_string(), "member_name": "alice", "message": "hi"}),
            ))
            .await;
        let message = match result {
            Err(FunctionCallError::RespondToModel(message)) => message,
            Err(_) => panic!("expected model-visible stopped-member error"),
            Ok(_) => panic!("inactive process member should reject sends"),
        };

        assert!(message.contains("stopped"));
    }

    #[tokio::test]
    async fn lead_team_send_requires_team_id() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let result = TeamHandler::for_tool(TeamTool::TeamSend)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({"target": "lead", "message": "status"}),
            ))
            .await;
        let message = match result {
            Err(FunctionCallError::RespondToModel(message)) => message,
            Err(_) => panic!("expected model-visible missing-team error"),
            Ok(_) => panic!("lead team_send without team_id must fail"),
        };

        assert!(message.contains("team_id is required"));
    }

    #[tokio::test]
    async fn teammate_process_team_send_can_omit_team_id() {
        let (session, turn) = make_session_and_context().await;
        let registry = session.services.agent_control.team_registry();
        let team = registry
            .create_team("Rocket".to_string(), ThreadId::new())
            .await;
        let member = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &member, "alice", true),
        );
        let identity = crate::team::TeammateIdentity {
            team: team.name.clone(),
            agent_name: "alice".to_string(),
        };

        let output = teammate_team_send(
            &turn,
            None,
            &identity,
            TeamSendArgs {
                team_id: None,
                to: Some(team_store::TEAM_LEAD_NAME.to_string()),
                summary: Some("work complete".to_string()),
                sender_member_id: None,
                target: None,
                member_id: None,
                member_name: None,
                delivery_mode: None,
                message: Some("done".to_string()),
                items: None,
            },
            "team_send",
        )
        .await
        .expect("teammate send");
        let sent: serde_json::Value =
            serde_json::from_str(&output.into_text()).expect("team_send result");

        assert_eq!(sent["success"], true);
        assert_eq!(sent["message"], "Message sent to team-lead's inbox");
        assert_eq!(sent["routing"]["sender"], "alice");
        assert_eq!(sent["routing"]["target"], "@team-lead");
        assert_eq!(sent["routing"]["summary"], "work complete");
        assert_eq!(sent["routing"]["content"], "done");
        let inbox = team_store::read_mailbox(
            turn.config.codex_home.as_path(),
            &team.name,
            team_store::TEAM_LEAD_NAME,
        )
        .expect("read lead inbox");
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].from, "alice");
        assert_eq!(inbox[0].text, "done");
        assert_eq!(inbox[0].summary.as_deref(), Some("work complete"));
    }

    #[tokio::test]
    async fn teammate_process_task_tools_use_shared_claude_task_list() {
        let (session, turn) = make_session_and_context().await;
        let registry = session.services.agent_control.team_registry();
        let team = registry
            .create_team("Rocket".to_string(), ThreadId::new())
            .await;
        let member = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &member, "alice", true),
        );
        let identity = crate::team::TeammateIdentity {
            team: team.name.clone(),
            agent_name: "alice".to_string(),
        };

        let created = teammate_team_task_create(
            &turn,
            &identity,
            TeamTaskCreateArgs {
                team_id: None,
                title: "inspect parser".to_string(),
                assignee_member_id: None,
                dependencies: None,
                note: Some("find owner".to_string()),
            },
        )
        .await
        .expect("create shared task");
        let created: StoreTeamTaskCreateResult =
            serde_json::from_str(&created.into_text()).expect("task create result");
        assert_eq!(created.task.id, "1");
        assert_eq!(created.task.subject, "inspect parser");
        assert_eq!(created.task.description, "find owner");
        assert_eq!(created.task.status, team_coord::TaskStatus::Pending);

        let listed = teammate_team_task_list(&turn, &identity, TeamTaskListArgs { team_id: None })
            .await
            .expect("list shared tasks");
        let listed: StoreTeamTaskListResult =
            serde_json::from_str(&listed.into_text()).expect("task list result");
        assert_eq!(listed.tasks, vec![created.task.clone()]);

        let claimed = teammate_team_task_claim(
            &turn,
            &identity,
            TeamTaskClaimArgs {
                team_id: None,
                task_id: "1".to_string(),
                member_id: None,
            },
        )
        .await
        .expect("claim task as current teammate");
        let claimed: StoreTeamTaskClaimResult =
            serde_json::from_str(&claimed.into_text()).expect("task claim result");
        assert_eq!(claimed.task.status, team_coord::TaskStatus::InProgress);
        assert_eq!(claimed.task.owner.as_deref(), Some("alice@Rocket"));

        let completed = teammate_team_task_update(
            &turn,
            &identity,
            TeamTaskUpdateArgs {
                team_id: None,
                task_id: "1".to_string(),
                title: None,
                assignee_member_id: None,
                clear_assignee: false,
                dependencies: None,
                status: Some("completed".to_string()),
                note: Some("owner found".to_string()),
                clear_note: false,
            },
        )
        .await
        .expect("complete task");
        let completed: StoreTeamTaskUpdateResult =
            serde_json::from_str(&completed.into_text()).expect("task update result");
        assert_eq!(completed.task.status, team_coord::TaskStatus::Completed);
        assert_eq!(completed.task.description, "owner found");

        let disk_task = team_coord::get_task(turn.config.codex_home.as_path(), &team.name, "1")
            .expect("read disk task")
            .expect("disk task should exist");
        assert_eq!(disk_task, completed.task);
    }

    #[tokio::test]
    async fn teammate_team_send_rejects_unknown_or_inactive_peer() {
        let (session, turn) = make_session_and_context().await;
        let registry = session.services.agent_control.team_registry();
        let team = registry
            .create_team("Rocket".to_string(), ThreadId::new())
            .await;
        let alice = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &alice, "alice", true),
        );
        let identity = crate::team::TeammateIdentity {
            team: team.name.clone(),
            agent_name: "alice".to_string(),
        };

        let unknown = teammate_team_send(
            &turn,
            None,
            &identity,
            TeamSendArgs {
                team_id: None,
                to: Some("bob".to_string()),
                summary: Some("peer".to_string()),
                sender_member_id: None,
                target: None,
                member_id: None,
                member_name: None,
                delivery_mode: None,
                message: Some("hello".to_string()),
                items: None,
            },
            "team_send",
        )
        .await;
        let message = match unknown {
            Err(FunctionCallError::RespondToModel(message)) => message,
            Err(_) => panic!("expected model-visible unknown-peer error"),
            Ok(_) => panic!("unknown peer should reject sends"),
        };
        assert!(message.contains("no teammate named"));
        assert!(
            team_store::read_mailbox(turn.config.codex_home.as_path(), &team.name, "bob")
                .expect("read bob inbox")
                .is_empty()
        );

        let bob = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "bob".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &bob, "bob", false),
        );
        let inactive = teammate_team_send(
            &turn,
            None,
            &identity,
            TeamSendArgs {
                team_id: None,
                to: Some("bob".to_string()),
                summary: Some("peer".to_string()),
                sender_member_id: None,
                target: None,
                member_id: None,
                member_name: None,
                delivery_mode: None,
                message: Some("hello".to_string()),
                items: None,
            },
            "team_send",
        )
        .await;
        let message = match inactive {
            Err(FunctionCallError::RespondToModel(message)) => message,
            Err(_) => panic!("expected model-visible stopped-peer error"),
            Ok(_) => panic!("inactive peer should reject sends"),
        };
        assert!(message.contains("stopped"));
        assert!(
            team_store::read_mailbox(turn.config.codex_home.as_path(), &team.name, "bob")
                .expect("read bob inbox")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn lead_to_process_member_mailbox_message_is_plain_text() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let registry = session.services.agent_control.team_registry();
        let team = registry
            .create_team("Rocket".to_string(), session.thread_id)
            .await;
        let member = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        registry
            .register_process_member(team.id, member.clone())
            .await
            .expect("register process member");
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &team,
            process_member_record(&team, &member, "alice", true),
        );

        TeamHandler::for_tool(TeamTool::TeamSend)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": team.id.to_string(),
                    "to": "alice",
                    "summary": "continue",
                    "message": "please continue"
                }),
            ))
            .await
            .expect("lead send to process member");

        let inbox = team_store::read_mailbox(turn.config.codex_home.as_path(), &team.name, "alice")
            .expect("read teammate inbox");
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].from, team_store::TEAM_LEAD_NAME);
        assert_eq!(inbox[0].text, "please continue");
        assert_eq!(inbox[0].summary.as_deref(), Some("continue"));
        assert!(!inbox[0].text.contains("Codex Teams message:"));
    }

    #[tokio::test]
    async fn lead_broadcast_excludes_team_lead_and_empty_targets_are_ok() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let created = TeamHandler::for_tool(TeamTool::CreateTeam)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "create_team",
                json!({"team_name": "Rocket"}),
            ))
            .await
            .expect("create team");
        let created: TestCreateTeamResult =
            serde_json::from_str(&text_output(created)).expect("create_team result");

        let empty_broadcast = TeamHandler::for_tool(TeamTool::TeamSend)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "to": "*",
                    "summary": "announce",
                    "message": "hello everyone"
                }),
            ))
            .await
            .expect("empty broadcast succeeds");
        let empty_broadcast: serde_json::Value =
            serde_json::from_str(&text_output(empty_broadcast)).expect("broadcast result");
        assert_eq!(empty_broadcast["success"], true);
        assert_eq!(
            empty_broadcast["message"],
            "No teammates to broadcast to (you are the only team member)"
        );
        assert_eq!(empty_broadcast["recipients"], json!([]));
        assert!(empty_broadcast.get("routing").is_none());
        assert!(
            team_store::read_mailbox(
                turn.config.codex_home.as_path(),
                &created.team.name,
                team_store::TEAM_LEAD_NAME
            )
            .expect("read lead inbox")
            .is_empty()
        );

        let member = crate::team::TeamMember::process_member_with_id(
            ThreadId::new(),
            "alice".to_string(),
            None,
            Vec::new(),
            Vec::new(),
        );
        session
            .services
            .agent_control
            .team_registry()
            .register_process_member(created.team.id, member.clone())
            .await
            .expect("register process member");
        write_process_member_config(
            turn.config.codex_home.as_path(),
            &created.team,
            process_member_record(&created.team, &member, "alice", true),
        );

        let broadcast = TeamHandler::for_tool(TeamTool::TeamSend)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_send",
                json!({
                    "team_id": created.team.id.to_string(),
                    "to": "*",
                    "summary": "announce",
                    "message": "hello everyone"
                }),
            ))
            .await
            .expect("broadcast succeeds");
        let broadcast: serde_json::Value =
            serde_json::from_str(&text_output(broadcast)).expect("broadcast result");

        assert_eq!(broadcast["success"], true);
        assert_eq!(
            broadcast["message"],
            "Message broadcast to 1 teammate(s): alice"
        );
        assert_eq!(broadcast["recipients"], json!(["alice"]));
        assert_eq!(broadcast["routing"]["target"], "@team");
        assert!(
            team_store::read_mailbox(
                turn.config.codex_home.as_path(),
                &created.team.name,
                team_store::TEAM_LEAD_NAME
            )
            .expect("read lead inbox")
            .is_empty()
        );
        let alice_inbox = team_store::read_mailbox(
            turn.config.codex_home.as_path(),
            &created.team.name,
            "alice",
        )
        .expect("read alice inbox");
        assert_eq!(alice_inbox.len(), 1);
        assert_eq!(alice_inbox[0].from, team_store::TEAM_LEAD_NAME);
        assert_eq!(alice_inbox[0].text, "hello everyone");
    }

    #[tokio::test]
    async fn team_send_to_field_validates_claude_constraints() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let team_id = ThreadId::new().to_string();

        for (payload, expected) in [
            (
                json!({"team_id": team_id.clone(), "to": "", "summary": "x", "message": "hi"}),
                "to must not be empty",
            ),
            (
                json!({"team_id": team_id.clone(), "to": "alice@team", "summary": "x", "message": "hi"}),
                "to must be a bare teammate name",
            ),
            (
                json!({"team_id": team_id, "to": "alice", "message": "hi"}),
                "summary is required when message is a string",
            ),
        ] {
            let result = TeamHandler::for_tool(TeamTool::TeamSend)
                .handle(invocation(
                    Arc::clone(&session),
                    Arc::clone(&turn),
                    "team_send",
                    payload,
                ))
                .await;
            let message = match result {
                Err(FunctionCallError::RespondToModel(message)) => message,
                Err(_) => panic!("expected model-visible validation error"),
                Ok(_) => panic!("invalid Claude-style team_send should fail"),
            };
            assert!(
                message.contains(expected),
                "expected {expected:?} in {message:?}"
            );
        }
    }

    #[tokio::test]
    async fn claude_send_message_alias_validates_claude_constraints() {
        let (session, turn) = make_session_and_context().await;
        let registry = session.services.agent_control.team_registry();
        registry
            .create_team("Rocket".to_string(), session.thread_id)
            .await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        for (payload, expected) in [
            (
                json!({"to": "", "summary": "x", "message": "hi"}),
                "to must not be empty",
            ),
            (
                json!({"to": "alice@team", "summary": "x", "message": "hi"}),
                "to must be a bare teammate name",
            ),
            (
                json!({"to": "alice", "message": "hi"}),
                "summary is required when message is a string",
            ),
        ] {
            let result = TeamHandler::for_tool(TeamTool::ClaudeSendMessage)
                .handle(invocation(
                    Arc::clone(&session),
                    Arc::clone(&turn),
                    "SendMessage",
                    payload,
                ))
                .await;
            let message = match result {
                Err(FunctionCallError::RespondToModel(message)) => message,
                Err(_) => panic!("expected model-visible validation error"),
                Ok(_) => panic!("invalid SendMessage call should fail"),
            };
            assert!(
                message.contains(expected),
                "expected {expected:?} in {message:?}"
            );
        }
    }

    #[tokio::test]
    async fn team_tools_reject_subagent_execution_before_mutation() {
        let (session, mut turn) = make_session_and_context().await;
        turn.session_source =
            SessionSource::SubAgent(SubAgentSource::Other("implementation_lane".to_string()));
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let result = TeamHandler::for_tool(TeamTool::CreateTeam)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "create_team",
                json!({"name": "should not exist"}),
            ))
            .await;
        let message = match result {
            Err(FunctionCallError::RespondToModel(message)) => message,
            Err(_) => panic!("expected model-visible lead-only error"),
            Ok(_) => panic!("subagents must not execute Teams tools"),
        };
        assert!(message.contains("lead-only"));
        assert!(message.contains("native spawned subagent"));

        let result = TeamHandler::for_tool(TeamTool::TeamSpawnMember)
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                "team_spawn_member",
                json!({"team_id": ThreadId::new().to_string(), "name": "alice"}),
            ))
            .await;
        let message = match result {
            Err(FunctionCallError::RespondToModel(message)) => message,
            Err(_) => panic!("expected model-visible lead-only error"),
            Ok(_) => panic!("subagents must not spawn Teams teammates"),
        };
        assert!(message.contains("lead-only"));
        assert!(message.contains("native spawned subagent"));

        let (_unused_session, lead_turn) = make_session_and_context().await;
        let listed = TeamHandler::for_tool(TeamTool::ListTeams)
            .handle(invocation(
                Arc::clone(&session),
                Arc::new(lead_turn),
                "list_teams",
                json!({}),
            ))
            .await
            .expect("list teams");
        let listed: TestListTeamsResult =
            serde_json::from_str(&text_output(listed)).expect("list result");
        assert_eq!(listed.teams, Vec::new());
    }
}
