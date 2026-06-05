//! Teams UI integration coverage: footer/status visibility and the external-pane
//! trigger.
//!
//! These drive the real `ChatWidget` through the shared harness so the assertions
//! exercise the same code paths a live session does.

use super::*;
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

fn seed_team_with_member(chat: &mut ChatWidget) {
    feed_raw(chat, call("create_team", "c1", "{}"));
    feed_raw(
        chat,
        output(
            "c1",
            r#"{"team":{"id":"team-1","name":"Rocket","status":"active","members":[]}}"#,
        ),
    );
    feed_raw(chat, call("team_spawn_member", "c2", r#"{"team_id":"team-1"}"#));
    feed_raw(
        chat,
        output(
            "c2",
            r#"{"member":{"id":"m1","name":"alice","agent_thread_id":"thr-1","status":"active","agent_status":"running"}}"#,
        ),
    );
}

#[tokio::test]
async fn team_tool_output_updates_footer_and_opens_pane() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    seed_team_with_member(&mut chat);

    // Status bar: the team and teammate are visible in the footer label.
    let footer = chat.team_ui.footer_label().expect("team footer label");
    assert!(footer.contains("Teams: Rocket"), "footer: {footer}");
    assert!(footer.contains("@alice"), "footer: {footer}");
    assert!(footer.contains("ctrl+t teammates"), "footer: {footer}");

    // Split: a live MemberSpawned requests an external teammate pane.
    let mut opened_pane = false;
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::OpenTeammatePane {
            member_name,
            agent_thread_id,
        } = ev
            && member_name == "alice"
            && agent_thread_id == "thr-1"
        {
            opened_pane = true;
            break;
        }
    }
    assert!(opened_pane, "expected an OpenTeammatePane event for alice");
}
