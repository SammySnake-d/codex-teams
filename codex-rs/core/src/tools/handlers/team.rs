use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::agent::next_thread_spawn_depth;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::function_tool::FunctionCallError;
use crate::team::SpawnTeamMemberRequest;
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
    message: Option<String>,
    items: Option<Vec<UserInput>>,
}

#[derive(Debug, Deserialize)]
struct TeamSendArgs {
    team_id: String,
    member_id: String,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
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
    let snapshot = session
        .services
        .team_registry
        .team_status(team_id, &session.services.agent_control)
        .await
        .map_err(team_error)?;
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
    let member = session
        .services
        .team_registry
        .spawn_member(
            SpawnTeamMemberRequest {
                team_id,
                name,
                profile: args.profile,
                initial_items: items,
                config,
                session_source: Some(session_source),
            },
            &session.services.agent_control,
        )
        .await
        .map_err(team_error)?;
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
    let member_id = id_from_str("member", &args.member_id)?;
    let items = collab::parse_collab_input(args.message, args.items)?;
    let content = collab::input_preview(&items);
    let message = session
        .services
        .team_registry
        .send_to_member(
            team_id,
            member_id,
            content,
            items,
            &session.services.agent_control,
        )
        .await
        .map_err(team_error)?;
    json_output(&TeamSendResult { message }, Some(true), "team_send")
}

async fn team_stop(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamIdArgs = parse_arguments(&arguments)?;
    let team_id = id_from_str("team", &args.team_id)?;
    let snapshot = session
        .services
        .team_registry
        .stop_team(team_id, &session.services.agent_control)
        .await
        .map_err(team_error)?;
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

    fn text_input(text: &str) -> Vec<UserInput> {
        vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }]
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
                    "message": "investigate one slice"
                }),
            ))
            .await
            .expect("spawn member");
        let spawned: TestTeamSpawnMemberResult =
            serde_json::from_str(&text_output(spawned)).expect("spawn result");
        assert_eq!(spawned.member.name, "teammate-a");

        assert!(
            manager.captured_ops().contains(&(
                spawned.member.agent_thread_id,
                Op::UserInput {
                    items: text_input("investigate one slice"),
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
                    "message": "send a progress update"
                }),
            ))
            .await
            .expect("send member message");
        let sent: TestTeamSendResult =
            serde_json::from_str(&text_output(sent)).expect("send result");
        assert_eq!(sent.message.content, "send a progress update");

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
        assert_eq!(status.snapshot.team.members.len(), 1);
        assert_eq!(status.snapshot.team.members[0].id, spawned.member.id);
        assert_eq!(status.snapshot.team.members[0].name, spawned.member.name);
        assert_eq!(status.snapshot.messages, vec![sent.message]);

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
