//! Teams UI integration coverage: footer/status visibility and Teams-only roster
//! registration.
//!
//! These drive the real `ChatWidget` through the shared harness so the assertions
//! exercise the same code paths a live session does.

use super::*;
use codex_protocol::ThreadId;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;

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

fn feed_raw(chat: &mut ChatWidget, item: ResponseItem) {
    chat.handle_server_notification(
        ServerNotification::RawResponseItemCompleted(
            codex_app_server_protocol::RawResponseItemCompletedNotification {
                thread_id: String::new(),
                turn_id: "turn-1".to_string(),
                item,
            },
        ),
        /*replay_kind*/ None,
    );
}

fn configure(chat: &mut ChatWidget) -> NamedTempFile {
    let rollout_file = NamedTempFile::new().unwrap();
    let configured = crate::session_state::ThreadSessionState {
        thread_id: ThreadId::new(),
        forked_from_id: None,
        fork_parent_title: None,
        thread_name: None,
        model: "test-model".to_string(),
        model_provider_id: "test-provider".to_string(),
        service_tier: None,
        approval_policy: AskForApproval::Never,
        approvals_reviewer: ApprovalsReviewer::User,
        permission_profile: PermissionProfile::read_only(),
        active_permission_profile: None,
        cwd: test_path_buf("/home/user/project").abs(),
        runtime_workspace_roots: Vec::new(),
        instruction_source_paths: Vec::new(),
        reasoning_effort: Some(ReasoningEffortConfig::default()),
        collaboration_mode: None,
        personality: None,
        message_history: None,
        network_proxy: None,
        rollout_path: Some(rollout_file.path().to_path_buf()),
    };
    chat.handle_thread_session(configured);
    rollout_file
}

fn drain_all(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) {
    while rx.try_recv().is_ok() {}
}

fn alice_thread_id() -> ThreadId {
    ThreadId::from_string("00000000-0000-0000-0000-0000000000a1").expect("valid thread")
}

fn seed_team_with_pane_member(chat: &mut ChatWidget) {
    let alice_thread_id = alice_thread_id();
    feed_raw(chat, call("create_team", "c1", "{}"));
    feed_raw(
        chat,
        output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ),
    );
    feed_raw(
        chat,
        call("team_spawn_member", "c2", r#"{"team_id":"team-1"}"#),
    );
    feed_raw(
        chat,
        output(
            "c2",
            &format!(
                r#"{{"member":{{"id":"m1","name":"alice","agent_thread_id":"{alice_thread_id}","profile":"researcher","status":"active","agent_status":"running"}},"tmux_pane_id":"%9","backend_type":"tmux"}}"#
            ),
        ),
    );
}

#[tokio::test]
async fn team_tool_output_registers_process_teammate() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    seed_team_with_pane_member(&mut chat);

    assert_eq!(chat.team_ui.active_team_name().as_deref(), Some("Rocket"));

    let mut registered_thread = false;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AppEvent::RegisterTeammateThread {
                member_name,
                agent_thread_id,
                agent_role,
                tmux_pane_id,
                backend_type,
            } if member_name == "alice"
                && agent_thread_id == alice_thread_id()
                && agent_role.as_deref() == Some("researcher")
                && tmux_pane_id.as_deref() == Some("%9")
                && backend_type.as_deref() == Some("tmux") =>
            {
                registered_thread = true;
            }
            _ => {}
        }
    }
    assert!(
        registered_thread,
        "expected a RegisterTeammateThread event for alice"
    );
}

#[tokio::test]
async fn process_team_spawn_registers_existing_pane() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    feed_raw(&mut chat, call("create_team", "c1", "{}"));
    feed_raw(
        &mut chat,
        output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ),
    );
    drain_all(&mut rx);

    feed_raw(
        &mut chat,
        call("team_spawn_member", "c2", r#"{"team_id":"team-1"}"#),
    );
    feed_raw(
        &mut chat,
        output(
            "c2",
            &format!(
                r#"{{"member":{{"id":"m1","name":"alice","agent_thread_id":"{}","profile":"researcher","status":"active","agent_status":"running"}},"tmux_pane_id":"%9","backend_type":"tmux"}}"#,
                alice_thread_id()
            ),
        ),
    );

    let mut registered_existing_pane = false;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AppEvent::RegisterTeammateThread {
                member_name,
                agent_thread_id,
                tmux_pane_id,
                backend_type,
                ..
            } if member_name == "alice"
                && agent_thread_id == alice_thread_id()
                && tmux_pane_id.as_deref() == Some("%9")
                && backend_type.as_deref() == Some("tmux") =>
            {
                registered_existing_pane = true;
            }
            _ => {}
        }
    }

    assert!(
        registered_existing_pane,
        "expected pane metadata to reach Teams roster registration"
    );
}

#[tokio::test]
async fn team_spawn_member_without_pane_metadata_does_not_register_teammate() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    feed_raw(&mut chat, call("create_team", "c1", "{}"));
    feed_raw(
        &mut chat,
        output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ),
    );
    drain_all(&mut rx);

    feed_raw(
        &mut chat,
        call("team_spawn_member", "c2", r#"{"team_id":"team-1"}"#),
    );
    feed_raw(
        &mut chat,
        output(
            "c2",
            &format!(
                r#"{{"member":{{"id":"m1","name":"alice","agent_thread_id":"{}","profile":"researcher","status":"active","agent_status":"running"}}}}"#,
                alice_thread_id()
            ),
        ),
    );

    assert_eq!(chat.team_ui.active_team_name().as_deref(), Some("Rocket"));
    while let Ok(ev) = rx.try_recv() {
        assert!(
            !matches!(ev, AppEvent::RegisterTeammateThread { .. }),
            "team_spawn_member without pane metadata must not affect Teams navigation: {ev:?}"
        );
    }
}

#[tokio::test]
async fn spawn_agent_output_does_not_update_teams_roster() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    feed_raw(
        &mut chat,
        call(
            "spawn_agent",
            "spawn-1",
            r#"{"task_name":"audit","message":"Inspect the code"}"#,
        ),
    );
    feed_raw(
        &mut chat,
        output(
            "spawn-1",
            r#"{"agent_id":"agent-1","status":"running","task_name":"audit"}"#,
        ),
    );

    assert_eq!(chat.team_ui.active_team_name(), None);
    while let Ok(ev) = rx.try_recv() {
        assert!(
            !matches!(ev, AppEvent::RegisterTeammateThread { .. }),
            "spawn_agent must not affect Teams UI or teammate navigation: {ev:?}"
        );
    }
}

#[tokio::test]
async fn namespaced_team_spawn_member_output_does_not_update_teams_roster() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    feed_raw(&mut chat, call("create_team", "c1", "{}"));
    feed_raw(
        &mut chat,
        output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ),
    );
    drain_all(&mut rx);

    feed_raw(
        &mut chat,
        ResponseItem::FunctionCall {
            id: None,
            name: "team_spawn_member".to_string(),
            namespace: Some("mcp".to_string()),
            arguments: r#"{"team_id":"team-1"}"#.to_string(),
            call_id: "mcp-spawn-1".to_string(),
        },
    );
    feed_raw(
        &mut chat,
        output(
            "mcp-spawn-1",
            &format!(
                r#"{{"member":{{"id":"m1","name":"alice","agent_thread_id":"{}","profile":"researcher"}},"tmux_pane_id":"%9","backend_type":"tmux"}}"#,
                alice_thread_id()
            ),
        ),
    );

    assert_eq!(chat.team_ui.active_team_name().as_deref(), Some("Rocket"));
    while let Ok(ev) = rx.try_recv() {
        assert!(
            !matches!(ev, AppEvent::RegisterTeammateThread { .. }),
            "namespaced team-shaped output must not affect Teams navigation: {ev:?}"
        );
    }
}

#[tokio::test]
async fn spawn_agent_team_shaped_output_does_not_update_teams_roster() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    feed_raw(
        &mut chat,
        call(
            "spawn_agent",
            "spawn-1",
            r#"{"task_name":"audit","message":"Inspect the code"}"#,
        ),
    );
    feed_raw(
        &mut chat,
        output(
            "spawn-1",
            &format!(
                r#"{{"member":{{"id":"m1","name":"alice","agent_thread_id":"{}","profile":"researcher"}},"tmux_pane_id":"%9","backend_type":"tmux"}}"#,
                alice_thread_id()
            ),
        ),
    );

    assert_eq!(chat.team_ui.active_team_name(), None);
    while let Ok(ev) = rx.try_recv() {
        assert!(
            !matches!(ev, AppEvent::RegisterTeammateThread { .. }),
            "team-shaped spawn_agent output must not affect Teams navigation: {ev:?}"
        );
    }
}

#[tokio::test]
async fn team_spawn_member_output_with_wrong_team_id_does_not_register_teammate() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    feed_raw(&mut chat, call("create_team", "c1", "{}"));
    feed_raw(
        &mut chat,
        output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ),
    );
    drain_all(&mut rx);

    feed_raw(
        &mut chat,
        call("team_spawn_member", "c2", r#"{"team_id":"other-team"}"#),
    );
    feed_raw(
        &mut chat,
        output(
            "c2",
            &format!(
                r#"{{"member":{{"id":"m1","name":"alice","agent_thread_id":"{}","profile":"researcher"}},"tmux_pane_id":"%9","backend_type":"tmux"}}"#,
                alice_thread_id()
            ),
        ),
    );

    assert_eq!(chat.team_ui.active_team_name().as_deref(), Some("Rocket"));
    while let Ok(ev) = rx.try_recv() {
        assert!(
            !matches!(ev, AppEvent::RegisterTeammateThread { .. }),
            "mismatched team_spawn_member output must not affect Teams navigation: {ev:?}"
        );
    }
}
