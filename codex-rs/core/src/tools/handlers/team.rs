use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::agent::next_thread_spawn_depth;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::function_tool::FunctionCallError;
use crate::team::CreateTeamTaskRequest;
use crate::team::SendTeamMessageRequest;
use crate::team::SpawnTeamMemberRequest;
use crate::team::TeamMessageDeliveryMode;
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
            "list_teams" => list_teams(session).await,
            "team_status" => team_status(session, arguments).await,
            "team_spawn_member" => team_spawn_member(session, turn, arguments).await,
            "team_send" => team_send(session, arguments).await,
            "team_task_create" => team_task_create(session, arguments).await,
            "team_task_update" => team_task_update(session, arguments).await,
            "team_task_list" => team_task_list(session, arguments).await,
            "team_event_list" => team_event_list(session, arguments).await,
            "team_stop" => team_stop(session, arguments).await,
            other => Err(FunctionCallError::RespondToModel(format!(
                "unsupported team tool {other}"
            ))),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateTeamArgs {
    name: String,
}

#[derive(Debug, Deserialize)]
struct TeamIdArgs {
    team_id: String,
}

#[derive(Debug, Deserialize)]
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
struct TeamSendArgs {
    team_id: String,
    sender_member_id: Option<String>,
    member_id: String,
    delivery_mode: Option<String>,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
}

#[derive(Debug, Deserialize)]
struct TeamTaskCreateArgs {
    team_id: String,
    title: String,
    assignee_member_id: Option<String>,
    dependencies: Option<Vec<String>>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TeamTaskUpdateArgs {
    team_id: String,
    task_id: String,
    title: Option<String>,
    assignee_member_id: Option<String>,
    dependencies: Option<Vec<String>>,
    status: Option<String>,
    note: Option<String>,
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
struct TeamTaskCreateResult {
    task: crate::team::TeamTask,
}

#[derive(Debug, Serialize)]
struct TeamTaskUpdateResult {
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

async fn list_teams(session: Arc<Session>) -> Result<ToolOutput, FunctionCallError> {
    let teams = session.services.team_registry.list_teams().await;
    json_output(&ListTeamsResult { teams }, Some(true), "list_teams")
}

async fn team_status(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
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
    let sender_member_id = optional_id_from_str("sender member", args.sender_member_id)?;
    let member_id = id_from_str("member", &args.member_id)?;
    let delivery_mode = match args.delivery_mode.as_deref() {
        Some("interrupt") => TeamMessageDeliveryMode::Interrupt,
        Some("queue") | None => TeamMessageDeliveryMode::Queue,
        Some(other) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "unsupported team message delivery mode {other}; use queue or interrupt"
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
            .send_to_member(
                SendTeamMessageRequest {
                    team_id,
                    sender_member_id,
                    member_id,
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

async fn team_task_create(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamTaskCreateArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
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
    let task_id = id_from_str("task", &args.task_id)?;
    let title = optional_non_empty(args.title, "task title")?;
    let assignee_member_id = optional_id_from_str("member", args.assignee_member_id)?;
    let dependencies = args
        .dependencies
        .map(|dependencies| ids_from_strs("task dependency", dependencies))
        .transpose()?;
    let status = args
        .status
        .map(|status| match status.as_str() {
            "open" => Ok(TeamTaskStatus::Open),
            "claimed" => Ok(TeamTaskStatus::Claimed),
            "completed" => Ok(TeamTaskStatus::Completed),
            "blocked" => Ok(TeamTaskStatus::Blocked),
            other => Err(FunctionCallError::RespondToModel(format!(
                "unsupported team task status {other}; use open, claimed, completed, or blocked"
            ))),
        })
        .transpose()?;
    let note = optional_non_empty(args.note, "task note")?;
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
                    dependencies,
                    status,
                    note,
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

async fn team_task_list(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
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

async fn team_stop(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
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
    struct TestTeamTaskCreateResult {
        task: crate::team::TeamTask,
    }

    #[derive(Debug, Deserialize)]
    struct TestTeamTaskUpdateResult {
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
    struct TestTeamStatusResult {
        snapshot: crate::team::TeamSnapshot,
    }

    #[tokio::test]
    async fn team_tool_chain_creates_spawns_sends_statuses_and_stops() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
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

        let member_sent = TeamHandler
            .handle(invocation(
                Arc::clone(&session),
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
        assert_eq!(member_sent.message.target_member_id, spawned_b.member.id);

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
        assert_eq!(listed_tasks.tasks, vec![updated_task.task.clone()]);

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
            vec![sent.message, member_sent.message]
        );
        assert_eq!(status.snapshot.team.tasks, vec![updated_task.task]);
        assert!(
            status
                .snapshot
                .events
                .iter()
                .any(|event| matches!(event, crate::team::TeamEvent::TaskCreated { .. })),
            "team_status should include task events"
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
                .any(|(id, op)| *id == spawned.member.agent_thread_id && matches!(op, Op::Shutdown)),
            "team stop should submit shutdown"
        );
    }
}
