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

fn assert_no_user_turn(op_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Op>) {
    while let Ok(op) = op_rx.try_recv() {
        assert!(
            !matches!(op, Op::UserTurn { .. }),
            "expected direct Teams mailbox delivery, got model turn: {op:?}"
        );
    }
}

fn expect_user_turn(op_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Op>) -> Vec<UserInput> {
    while let Ok(op) = op_rx.try_recv() {
        if let Op::UserTurn { items, .. } = op {
            return items;
        }
    }
    panic!("expected submitted model turn");
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
                r#"{{"member":{{"id":"m1","name":"alice","agent_thread_id":"{alice_thread_id}","profile":"researcher","status":"active","agent_status":"running"}},"tmux_pane_id":"%9","backend_type":"tmux","color":"red","mode":"plan","is_active":true,"prompt":"Inspect issue #15."}}"#
            ),
        ),
    );
}

fn seed_team_store_with_alice(chat: &ChatWidget) {
    let team_store = crate::legacy_core::team_store::TeamFile {
        name: "Rocket".to_string(),
        team_id: Some("team-1".to_string()),
        created_at: 0,
        lead_agent_id: crate::legacy_core::team_store::agent_id(
            crate::legacy_core::team_store::TEAM_LEAD_NAME,
            "Rocket",
        ),
        members: vec![crate::legacy_core::team_store::TeamFileMember {
            agent_id: crate::legacy_core::team_store::agent_id("alice", "Rocket"),
            name: "alice".to_string(),
            member_id: Some("m1".to_string()),
            agent_type: Some("researcher".to_string()),
            joined_at: 0,
            tmux_pane_id: "%9".to_string(),
            cwd: "/home/user/project".to_string(),
            backend_type: Some("tmux".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    crate::legacy_core::team_store::write_config(&chat.config.codex_home, "Rocket", &team_store)
        .expect("write team config");
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
                color,
                mode,
                is_active,
                prompt,
            } if member_name == "alice"
                && agent_thread_id == alice_thread_id()
                && agent_role.as_deref() == Some("researcher")
                && tmux_pane_id.as_deref() == Some("%9")
                && backend_type.as_deref() == Some("tmux")
                && color.as_deref() == Some("red")
                && mode.as_deref() == Some("plan")
                && is_active == Some(true)
                && prompt.as_deref() == Some("Inspect issue #15.") =>
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
async fn team_mention_binding_delivers_directly_to_teammate_mailbox() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);
    seed_team_with_pane_member(&mut chat);
    seed_team_store_with_alice(&chat);
    drain_all(&mut rx);

    let accepted = chat.submit_user_message_with_history_record(
        UserMessage {
            text: "@alice please inspect the failing test".to_string(),
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: vec![MentionBinding {
                sigil: '@',
                mention: "alice".to_string(),
                path: "team://alice".to_string(),
            }],
        },
        UserMessageHistoryRecord::UserMessageText,
    );

    assert!(accepted);
    assert_no_user_turn(&mut op_rx);
    let messages =
        crate::legacy_core::team_store::read_mailbox(&chat.config.codex_home, "Rocket", "alice")
            .expect("read alice mailbox");
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].from,
        crate::legacy_core::team_store::TEAM_LEAD_NAME
    );
    assert_eq!(messages[0].text, "@alice please inspect the failing test");
    assert!(!messages[0].read);

    let mut saw_confirmation = false;
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::InsertHistoryCell(cell) = ev {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            if rendered.contains("Teams message sent to @alice.") {
                saw_confirmation = true;
            }
        }
    }
    assert!(saw_confirmation, "expected direct-send confirmation");
}

#[tokio::test]
async fn raw_team_address_delivers_directly_to_teammate_mailbox() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);
    seed_team_with_pane_member(&mut chat);
    seed_team_store_with_alice(&chat);
    drain_all(&mut rx);

    let accepted = chat.submit_user_message_with_history_record(
        UserMessage {
            text: "@alice please inspect the failing test".to_string(),
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: Vec::new(),
        },
        UserMessageHistoryRecord::UserMessageText,
    );

    assert!(accepted);
    assert_no_user_turn(&mut op_rx);
    let messages =
        crate::legacy_core::team_store::read_mailbox(&chat.config.codex_home, "Rocket", "alice")
            .expect("read alice mailbox");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].from, "user");
    assert_eq!(messages[0].text, "please inspect the failing test");
    assert!(!messages[0].read);

    let mut saw_confirmation = false;
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::InsertHistoryCell(cell) = ev {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            if rendered.contains("Teams message sent to @alice.") {
                saw_confirmation = true;
            }
        }
    }
    assert!(saw_confirmation, "expected direct-send confirmation");
}

#[tokio::test]
async fn raw_unknown_team_address_falls_through_to_model_turn() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);
    seed_team_with_pane_member(&mut chat);
    seed_team_store_with_alice(&chat);
    drain_all(&mut rx);

    let accepted = chat.submit_user_message_with_history_record(
        UserMessage {
            text: "@utils explain this code".to_string(),
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: Vec::new(),
        },
        UserMessageHistoryRecord::UserMessageText,
    );

    assert!(accepted);
    assert_eq!(
        expect_user_turn(&mut op_rx),
        vec![UserInput::Text {
            text: "@utils explain this code".to_string(),
            text_elements: Vec::new(),
        }]
    );
    let messages =
        crate::legacy_core::team_store::read_mailbox(&chat.config.codex_home, "Rocket", "alice")
            .expect("read alice mailbox");
    assert!(messages.is_empty());
}

#[tokio::test]
async fn raw_team_address_without_active_team_falls_through_to_model_turn() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    let accepted = chat.submit_user_message_with_history_record(
        UserMessage {
            text: "@alice please inspect the failing test".to_string(),
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: Vec::new(),
        },
        UserMessageHistoryRecord::UserMessageText,
    );

    assert!(accepted);
    assert_eq!(
        expect_user_turn(&mut op_rx),
        vec![UserInput::Text {
            text: "@alice please inspect the failing test".to_string(),
            text_elements: Vec::new(),
        }]
    );
}

#[tokio::test]
async fn duplicate_team_mention_binding_writes_one_mailbox_message() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);
    seed_team_with_pane_member(&mut chat);
    seed_team_store_with_alice(&chat);
    drain_all(&mut rx);

    let accepted = chat.submit_user_message_with_history_record(
        UserMessage {
            text: "@alice @alice inspect this once".to_string(),
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: vec![
                MentionBinding {
                    sigil: '@',
                    mention: "alice".to_string(),
                    path: "team://alice".to_string(),
                },
                MentionBinding {
                    sigil: '@',
                    mention: "alice".to_string(),
                    path: "team://alice".to_string(),
                },
            ],
        },
        UserMessageHistoryRecord::UserMessageText,
    );

    assert!(accepted);
    assert_no_user_turn(&mut op_rx);
    let messages =
        crate::legacy_core::team_store::read_mailbox(&chat.config.codex_home, "Rocket", "alice")
            .expect("read alice mailbox");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "@alice @alice inspect this once");
}

#[tokio::test]
async fn team_mention_without_active_team_restores_draft_and_does_not_submit() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let _rollout = configure(&mut chat);
    drain_all(&mut rx);

    let accepted = chat.submit_user_message_with_history_record(
        UserMessage {
            text: "@alice please inspect the failing test".to_string(),
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: vec![MentionBinding {
                sigil: '@',
                mention: "alice".to_string(),
                path: "team://alice".to_string(),
            }],
        },
        UserMessageHistoryRecord::UserMessageText,
    );

    assert!(!accepted);
    assert_no_user_turn(&mut op_rx);
    let mut saw_error = false;
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::InsertHistoryCell(cell) = ev {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            if rendered.contains("No active Codex team is available") {
                saw_error = true;
            }
        }
    }
    assert!(saw_error, "expected no-active-team error");
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
async fn spawn_agent_with_teammate_name_updates_teams_roster() {
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
        call(
            "spawn_agent",
            "spawn-1",
            r#"{"name":"alice","team_name":"Rocket","message":"Inspect issue #15."}"#,
        ),
    );
    feed_raw(
        &mut chat,
        output(
            "spawn-1",
            &format!(
                r#"{{"member":{{"id":"m1","name":"alice","agent_thread_id":"{}","profile":"researcher"}},"tmux_pane_id":"%9","backend_type":"tmux","color":"red","prompt":"Inspect issue #15."}}"#,
                alice_thread_id()
            ),
        ),
    );

    let mut registered = false;
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::RegisterTeammateThread {
            member_name,
            prompt,
            ..
        } = ev
        {
            assert_eq!(member_name, "alice");
            assert_eq!(prompt.as_deref(), Some("Inspect issue #15."));
            registered = true;
        }
    }
    assert!(
        registered,
        "spawn_agent teammate branch should register the teammate thread"
    );
}

#[tokio::test]
async fn spawn_agent_with_wrong_team_name_does_not_update_teams_roster() {
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
        call(
            "spawn_agent",
            "spawn-1",
            r#"{"name":"alice","team_name":"Other","message":"Inspect issue #15."}"#,
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

    assert_eq!(chat.team_ui.active_team_name().as_deref(), Some("Rocket"));
    while let Ok(ev) = rx.try_recv() {
        assert!(
            !matches!(ev, AppEvent::RegisterTeammateThread { .. }),
            "wrong-team spawn_agent output must not affect Teams navigation: {ev:?}"
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
