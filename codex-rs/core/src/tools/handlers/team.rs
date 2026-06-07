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
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::multi_agents_common::function_arguments;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::team_spec;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ModeKind;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::user_input::UserInput;
use codex_tools::ToolName;
use codex_tools::ToolSearchInfo;
use codex_tools::ToolSearchSourceInfo;
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

    fn allowed_in_teammate_process(self) -> bool {
        matches!(self, TeamTool::TeamSend)
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

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for TeamHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(self.tool.name())
    }

    fn spec(&self) -> ToolSpec {
        self.tool.spec()
    }

    fn search_info(&self) -> Option<ToolSearchInfo> {
        ToolSearchInfo::from_spec(
            "codex teams team teammate roster mailbox task board event feed split pane swarm team_send team_spawn_member create_team"
                .to_string(),
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
    member_name: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    tmux_pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend_type: Option<String>,
}

struct ProcessSpawnedTeamMember {
    member: crate::team::TeamMember,
    tmux_pane_id: String,
    backend_type: String,
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
    if matches!(turn.session_source, SessionSource::SubAgent(_)) {
        return Err(FunctionCallError::RespondToModel(
            "Codex Teams tools are lead-only. Use native spawn_agent/subagent tools for ordinary delegation."
                .to_string(),
        ));
    }
    if crate::team::teammate_identity().is_some() && !tool.allowed_in_teammate_process() {
        return Err(FunctionCallError::RespondToModel(format!(
            "{} is lead-only in a Codex Teams teammate process. Teammates must not create teams or spawn roster members; use team_send to reply to the lead.",
            tool.name()
        )));
    }
    let arguments = function_arguments(payload)?;
    match tool {
        TeamTool::CreateTeam => create_team(session, arguments).await,
        TeamTool::ListTeams => list_teams(session, arguments).await,
        TeamTool::TeamStatus => team_status(session, arguments).await,
        TeamTool::TeamSpawnMember => team_spawn_member(session, turn, arguments).await,
        TeamTool::TeamSend => team_send(session, turn, arguments).await,
        TeamTool::TeamMessageList => team_message_list(session, turn, arguments).await,
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

    // Preferred path: launch a real `codex teammate` PROCESS in a tmux pane
    // (Claude `handleSpawnSplitPane`). If no process-pane backend is available,
    // fail closed instead of degrading into native `spawn_agent`: Teams teammates
    // need their own teammate process and mailbox, while ordinary delegation
    // belongs to the native subagent tool path.
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
        )
        .await?;
        return json_output(
            &TeamSpawnMemberResult {
                member: spawned.member,
                tmux_pane_id: Some(spawned.tmux_pane_id),
                backend_type: Some(spawned.backend_type),
            },
            Some(true),
            "team_spawn_member",
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
        )
        .await?;
        return json_output(
            &TeamSpawnMemberResult {
                member: spawned.member,
                tmux_pane_id: Some(spawned.tmux_pane_id),
                backend_type: Some(spawned.backend_type),
            },
            Some(true),
            "team_spawn_member",
        );
    }

    Err(FunctionCallError::RespondToModel(
        "team_spawn_member requires a tmux or iTerm2 pane backend so the teammate runs as a Codex Teams process. Use native spawn_agent/subagent tools for ordinary delegation."
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

fn append_teammate_config_string_override(flags: &mut Vec<String>, key: &str, value: &str) {
    flags.push("-c".to_string());
    let quoted_value = serde_json::Value::String(value.to_string()).to_string();
    flags.push(format!("{key}={quoted_value}"));
}

fn append_teammate_launch_mode_flags(
    flags: &mut Vec<String>,
    mode: ModeKind,
    approval_policy: AskForApproval,
    sandbox_policy: &SandboxPolicy,
) -> bool {
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
) -> Result<ProcessSpawnedTeamMember, FunctionCallError> {
    // Resolve everything that needs `.await` BEFORE constructing the tmux
    // backend: `TmuxBackend` holds a `RefCell`, so keeping it alive across an
    // await point would make this tool future `!Send`.
    let team_name = registry_team_name(session, team_id).await?;
    let backend = TmuxBackend::new();
    let teams_root = turn.config.codex_home.as_path();

    // Existing members → unique name (Claude `generateUniqueTeammateName`) +
    // round-robin color index.
    let existing_names: Vec<String> = team_store::read_config(teams_root, &team_name)
        .ok()
        .flatten()
        .map(|cfg| cfg.members.into_iter().map(|member| member.name).collect())
        .unwrap_or_default();
    let unique = spawn::unique_teammate_name(&name, &existing_names);
    let sanitized = spawn::sanitize_agent_name(&unique);
    let color = spawn::teammate_color(existing_names.len());

    let agent_id = team_store::agent_id(&sanitized, &team_name);
    let prompt_text = input_preview(items);
    let member_thread_id = ThreadId::new();
    // A split-pane teammate receives its first turn through the file mailbox, so
    // the `Codex Teams context:` markers (which the in-process spawn path injects
    // as a separate item before delivery) must be baked into the mailbox text
    // here — otherwise the teammate's model never sees the team context.
    let context = crate::team::team_context_envelope(
        team_id,
        &team_name,
        session.thread_id,
        member_thread_id,
        &sanitized,
        profile.as_deref(),
        &capabilities,
        &permissions,
    );
    let first_turn_text = if prompt_text.trim().is_empty() {
        context
    } else {
        format!("{context}\n\n{prompt_text}")
    };
    let parent_session_id = session.thread_id.to_string();
    let model = turn.model_info.slug.clone();
    let cwd = turn.config.cwd.as_path();
    let binary = spawn::teammate_binary().map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to resolve teammate binary: {err}"))
    })?;

    // Open + style the pane (create_teammate_pane sets the title + border color
    // internally, so they are not re-set here).
    let pane = backend
        .create_teammate_pane(&sanitized, color)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to create teammate pane: {err}"))
        })?;

    // NO `--prompt`: the first turn is delivered via the mailbox below
    // (mirroring Claude), so it is not run twice.
    let mut flags = vec![
        "teammate".to_string(),
        "--agent-id".to_string(),
        agent_id.clone(),
        "--agent-name".to_string(),
        sanitized.clone(),
        "--team-name".to_string(),
        team_name.clone(),
        "--agent-color".to_string(),
        color.as_name().to_string(),
        "--parent-session-id".to_string(),
        parent_session_id,
    ];
    if let Some(profile) = profile.as_deref() {
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
    let plan_mode_required = append_teammate_launch_mode_flags(
        &mut flags,
        turn.collaboration_mode.mode,
        turn.approval_policy.value(),
        &sandbox_policy,
    );

    let provider_env_keys = teammate_provider_env_keys(&turn.config.model_provider);
    let env = spawn::build_inherited_env_vars(teams_root, &provider_env_keys);
    backend
        .launch_teammate(
            &pane.pane_id,
            pane.used_external_session,
            cwd,
            &env,
            &binary,
            &flags,
        )
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to launch teammate process: {err}"))
        })?;

    // Register the member in the on-disk team config.
    let pane_id = pane.pane_id;
    let member_record = team_store::TeamFileMember {
        agent_id,
        member_id: Some(member_thread_id.to_string()),
        name: sanitized.clone(),
        agent_type: profile.clone(),
        model: Some(model),
        prompt: Some(prompt_text),
        color: Some(color.as_name().to_string()),
        joined_at: unix_millis(),
        tmux_pane_id: pane_id.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        backend_type: Some("tmux".to_string()),
        is_active: Some(true),
        plan_mode_required: plan_mode_required.then_some(true),
        mode: plan_mode_required.then_some("plan".to_string()),
        ..Default::default()
    };
    let team_for_cfg = team_name.clone();
    team_store::update_config(teams_root, &team_name, move |cfg| {
        cfg.name = team_for_cfg.clone();
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
    team_store::write_to_mailbox(
        teams_root,
        &team_name,
        &sanitized,
        team_store::TeammateMessage {
            from: team_store::TEAM_LEAD_NAME.to_string(),
            text: first_turn_text,
            timestamp: team_store::now_timestamp(),
            read: false,
            color: None,
            summary: None,
        },
    )
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to deliver teammate prompt: {err}"))
    })?;

    Ok(ProcessSpawnedTeamMember {
        member: crate::team::TeamMember::process_member_with_id(
            member_thread_id,
            sanitized,
            profile,
            capabilities,
            permissions,
        ),
        tmux_pane_id: pane_id,
        backend_type: "tmux".to_string(),
    })
}

/// iTerm2 twin of [`spawn_member_in_pane`]: launch a real `codex teammate`
/// process in an iTerm2 split pane via the `it2` Python-API CLI (Claude's
/// `ITermBackend`). The common contract (envelope-wrapped first turn, on-disk
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
) -> Result<ProcessSpawnedTeamMember, FunctionCallError> {
    let team_name = registry_team_name(session, team_id).await?;
    let backend = ITermBackend::new();
    let teams_root = turn.config.codex_home.as_path();

    let existing_names: Vec<String> = team_store::read_config(teams_root, &team_name)
        .ok()
        .flatten()
        .map(|cfg| cfg.members.into_iter().map(|member| member.name).collect())
        .unwrap_or_default();
    let unique = spawn::unique_teammate_name(&name, &existing_names);
    let sanitized = spawn::sanitize_agent_name(&unique);
    let color = spawn::teammate_color(existing_names.len());

    let agent_id = team_store::agent_id(&sanitized, &team_name);
    let prompt_text = input_preview(items);
    let member_thread_id = ThreadId::new();
    let context = crate::team::team_context_envelope(
        team_id,
        &team_name,
        session.thread_id,
        member_thread_id,
        &sanitized,
        profile.as_deref(),
        &capabilities,
        &permissions,
    );
    let first_turn_text = if prompt_text.trim().is_empty() {
        context
    } else {
        format!("{context}\n\n{prompt_text}")
    };
    let parent_session_id = session.thread_id.to_string();
    let model = turn.model_info.slug.clone();
    let cwd = turn.config.cwd.as_path();
    let binary = spawn::teammate_binary().map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to resolve teammate binary: {err}"))
    })?;

    // Create the iTerm2 split pane (first teammate: vertical split off the lead;
    // later teammates stack downward — handled inside the backend).
    let pane = backend
        .create_teammate_pane_in_swarm_view(&sanitized, color.as_name())
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to create iTerm2 pane: {err}"))
        })?;

    // NO `--prompt`: the first turn is delivered via the mailbox below
    // (mirroring Claude), so it is not run twice.
    let mut flags = vec![
        "teammate".to_string(),
        "--agent-id".to_string(),
        agent_id.clone(),
        "--agent-name".to_string(),
        sanitized.clone(),
        "--team-name".to_string(),
        team_name.clone(),
        "--agent-color".to_string(),
        color.as_name().to_string(),
        "--parent-session-id".to_string(),
        parent_session_id,
    ];
    if let Some(profile) = profile.as_deref() {
        flags.push("--agent-type".to_string());
        flags.push(profile.to_string());
    }
    // Teammates are by definition Teams sessions (see the tmux twin for why).
    flags.push("--enable".to_string());
    flags.push("teams".to_string());
    append_teammate_model_override(&mut flags, &model);
    let sandbox_policy = turn.sandbox_policy();
    let plan_mode_required = append_teammate_launch_mode_flags(
        &mut flags,
        turn.collaboration_mode.mode,
        turn.approval_policy.value(),
        &sandbox_policy,
    );

    let provider_env_keys = teammate_provider_env_keys(&turn.config.model_provider);
    let env = spawn::build_inherited_env_vars(teams_root, &provider_env_keys);
    // Reuse the tmux launch-line builder so the same shell-quoting applies (flag
    // values such as a team name with spaces must be quoted).
    let launch_line = crate::team_backends::tmux::build_launch_line(cwd, &env, &binary, &flags)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to build teammate launch line: {err}"
            ))
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
        agent_id,
        member_id: Some(member_thread_id.to_string()),
        name: sanitized.clone(),
        agent_type: profile.clone(),
        model: Some(model),
        prompt: Some(prompt_text.clone()),
        color: Some(color.as_name().to_string()),
        joined_at: unix_millis(),
        tmux_pane_id: pane.pane_id.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        backend_type: Some("iterm2".to_string()),
        is_active: Some(true),
        plan_mode_required: plan_mode_required.then_some(true),
        mode: plan_mode_required.then_some("plan".to_string()),
        ..Default::default()
    };
    let team_for_cfg = team_name.clone();
    team_store::update_config(teams_root, &team_name, move |cfg| {
        cfg.name = team_for_cfg.clone();
        if cfg.lead_agent_id.is_empty() {
            cfg.lead_agent_id = team_store::agent_id(team_store::TEAM_LEAD_NAME, &team_for_cfg);
        }
        cfg.members.push(member_record);
    })
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to register teammate in config: {err}"))
    })?;

    team_store::write_to_mailbox(
        teams_root,
        &team_name,
        &sanitized,
        team_store::TeammateMessage {
            from: team_store::TEAM_LEAD_NAME.to_string(),
            text: first_turn_text,
            timestamp: team_store::now_timestamp(),
            read: false,
            color: None,
            summary: None,
        },
    )
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to deliver teammate prompt: {err}"))
    })?;

    Ok(ProcessSpawnedTeamMember {
        member: crate::team::TeamMember::process_member_with_id(
            member_thread_id,
            sanitized,
            profile,
            capabilities,
            permissions,
        ),
        tmux_pane_id: pane.pane_id,
        backend_type: "iterm2".to_string(),
    })
}

async fn team_send(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: TeamSendArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;

    // Teammate-process path: a spawned `codex teammate` has an EMPTY in-memory
    // registry (it never ran create_team), so the registry-backed delivery below
    // cannot resolve the team and `authorize_team_sender` would reject it. Route
    // its sends across the process boundary via the on-disk file mailbox instead
    // (Claude's cross-process model: reply lands in the lead's inbox).
    if let Some(identity) = crate::team::teammate_identity() {
        return teammate_team_send(turn.as_ref(), team_id, identity, args).await;
    }

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

    // Split-pane PROCESS teammates (P3b) live only in the on-disk team store, not
    // in the in-memory registry, so they are delivered to via the file mailbox
    // their process polls (lead -> process member). They can be addressed by NAME
    // or by the spawn-returned member_id (persisted on disk). The registry path
    // further down is retained for legacy in-memory members, but
    // `team_spawn_member` no longer creates those members as a fallback.
    if matches!(args.target.as_deref(), Some("member") | None)
        && let Some((pane_name, pane_member_label)) = resolve_pane_member(
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
                member_label: pane_member_label,
                delivery_mode: args.delivery_mode,
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

#[derive(Serialize)]
struct MailboxSendResult {
    delivered: bool,
    target: String,
    via: &'static str,
}

struct PaneMemberSendRequest {
    team_id: ThreadId,
    member_name: String,
    member_label: String,
    delivery_mode: Option<String>,
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
    let team_name = registry_team_name(session, request.team_id).await?;
    let teams_root = turn.config.codex_home.as_path();
    let sanitized = crate::team_backends::spawn::sanitize_agent_name(&request.member_name);

    let known = team_store::read_config(teams_root, &team_name)
        .ok()
        .flatten()
        .is_some_and(|cfg| cfg.members.iter().any(|member| member.name == sanitized));
    if !known {
        return Err(FunctionCallError::RespondToModel(format!(
            "no teammate named '{}' in team '{team_name}'",
            request.member_name
        )));
    }

    let items = parse_team_input(request.message, request.items)?;
    let content = input_preview(&items);
    // Wrap with the `Codex Teams message:` envelope so the teammate's model sees
    // the same routing markers the in-process delivery path adds; the teammate
    // runner injects lead messages verbatim, so the markers must be in the text.
    let delivery_mode_label = match request.delivery_mode.as_deref() {
        Some("interrupt") => "interrupt",
        _ => "queue",
    };
    let enveloped = crate::team::team_message_envelope(
        request.team_id,
        ThreadId::new(),
        &format!("lead:{}", session.thread_id),
        &request.member_label,
        delivery_mode_label,
        &content,
    );
    team_store::write_to_mailbox(
        teams_root,
        &team_name,
        &sanitized,
        team_store::TeammateMessage {
            from: team_store::TEAM_LEAD_NAME.to_string(),
            text: enveloped,
            timestamp: team_store::now_timestamp(),
            read: false,
            color: None,
            summary: None,
        },
    )
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to deliver to teammate mailbox: {err}"))
    })?;

    json_output(
        &MailboxSendResult {
            delivered: true,
            target: sanitized,
            via: "mailbox",
        },
        Some(true),
        "team_send",
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
    let teams_root = turn.config.codex_home.as_path();
    let Some(cfg) = team_store::read_config(teams_root, &team_name)
        .ok()
        .flatten()
    else {
        return Ok(None);
    };
    if let Some(name) = member_name {
        let sanitized = crate::team_backends::spawn::sanitize_agent_name(name);
        if let Some(member) = cfg.members.iter().find(|member| member.name == sanitized) {
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
        return Ok(Some((member.name.clone(), id.to_string())));
    }
    Ok(None)
}

/// Deliver a teammate-originated `team_send` across the process boundary via the
/// on-disk file mailbox. A spawned `codex teammate` has an empty in-memory team
/// registry, so the normal registry path cannot run here; the reply is written
/// to the lead's inbox (`target: "lead"`, the default) or a named peer's inbox
/// (`target: "member"` + `member_name`). The lead surfaces it via its inbox
/// poller (TUI) and `team_message_list` (model).
async fn teammate_team_send(
    turn: &TurnContext,
    team_id: ThreadId,
    identity: &crate::team::TeammateIdentity,
    args: TeamSendArgs,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let teams_root = turn.config.codex_home.as_path();
    let delivery_mode = match args.delivery_mode.as_deref() {
        Some("interrupt") => TeamMessageDeliveryMode::Interrupt,
        Some("queue") | None => TeamMessageDeliveryMode::Queue,
        Some(other) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "unsupported team message delivery mode {other}; use queue or interrupt"
            )));
        }
    };
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
            (
                crate::team_backends::spawn::sanitize_agent_name(&name),
                crate::team::TeamMessageEndpoint::Member(ThreadId::new()),
            )
        }
        Some(other) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "unsupported team_send target {other}; use lead or member"
            )));
        }
    };
    let items = parse_team_input(args.message, args.items)?;
    let content = input_preview(&items);
    team_store::write_to_mailbox(
        teams_root,
        &identity.team,
        &recipient,
        team_store::TeammateMessage {
            from: identity.agent_name.clone(),
            text: content.clone(),
            timestamp: team_store::now_timestamp(),
            read: false,
            color: None,
            summary: None,
        },
    )
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to deliver to {recipient} mailbox: {err}"
        ))
    })?;

    // Return a registry-shaped message so the teammate's model sees a normal
    // team_send result (sender id is synthetic — process members are not in any
    // registry; the lead authoritatively re-stamps ids when it reads its inbox).
    let sender = match optional_id_from_str("sender member", args.sender_member_id)? {
        Some(id) => crate::team::TeamMessageEndpoint::Member(id),
        None => crate::team::TeamMessageEndpoint::Member(ThreadId::new()),
    };
    let message = crate::team::TeamMessage {
        id: ThreadId::new(),
        team_id,
        sender,
        target,
        target_member_id: None,
        content,
        items,
        submitted_id: None,
        delivery_mode,
        delivery_status: crate::team::TeamMessageDeliveryStatus::Submitted,
        created_at: unix_millis(),
    };
    json_output(&TeamSendResult { message }, Some(true), "team_send")
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
        let teams_root = turn.config.codex_home.as_path();
        let inbox = team_store::read_mailbox(teams_root, &team_name, team_store::TEAM_LEAD_NAME)
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
        for tool in [TeamTool::CreateTeam, TeamTool::TeamSpawnMember] {
            let info = TeamHandler::for_tool(tool)
                .search_info()
                .expect("team tools should expose deferred search info");
            let source_description = info
                .source_info
                .as_ref()
                .and_then(|source| source.description.as_deref())
                .expect("team search info should describe its source");
            let output_description = match &info.entry.output {
                codex_tools::LoadableToolSpec::Function(function) => function.description.as_str(),
                _ => panic!("expected Teams tool search output to be a function"),
            };

            assert!(info.entry.search_text.contains("codex teams"));
            assert!(source_description.contains("explicit Codex Teams workspaces"));
            assert!(output_description.contains("Teams"));
            for forbidden_trigger in [
                "spawn_agent",
                "subagent",
                "sub-agent",
                "ordinary delegation",
                "parallel delegation",
                "parallel agent work",
            ] {
                assert!(
                    !info.entry.search_text.contains(forbidden_trigger),
                    "Teams search text must not match ordinary agent trigger {forbidden_trigger:?}"
                );
                assert!(
                    !source_description.contains(forbidden_trigger),
                    "Teams source description must not match ordinary agent trigger {forbidden_trigger:?}"
                );
                assert!(
                    !output_description.contains(forbidden_trigger),
                    "Teams tool description must not match ordinary agent trigger {forbidden_trigger:?}"
                );
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
        assert!(message.contains("spawn_agent/subagent"));

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
