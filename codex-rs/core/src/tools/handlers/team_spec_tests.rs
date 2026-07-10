use super::*;

fn description(tool: ToolSpec) -> String {
    match tool {
        ToolSpec::Function(function) => function.description,
        ToolSpec::Namespace(_)
        | ToolSpec::ToolSearch { .. }
        | ToolSpec::WebSearch { .. }
        | ToolSpec::Freeform(_) => panic!("expected function tool"),
    }
}

#[test]
fn create_team_description_separates_team_context_from_subagents() {
    let ToolSpec::Function(function) = create_team_create_tool() else {
        panic!("expected function tool");
    };
    let properties = function
        .parameters
        .properties
        .expect("create_team should have parameters");

    assert_eq!(function.description, CODEX_TEAM_CREATE_DESCRIPTION);
    assert_eq!(
        function.parameters.required.as_ref(),
        Some(&vec!["team_name".to_string()])
    );
    assert!(properties.contains_key("team_name"));
    assert!(properties.contains_key("description"));
    assert!(properties.contains_key("agent_type"));
    assert!(!function.description.contains("spawn_agent"));
    assert!(!function.description.contains("subagent"));
}

#[test]
fn spawn_member_description_declares_named_teammate_split_pane_path() {
    let description = description(create_team_spawn_member_tool());

    assert_eq!(description, "Launch a new teammate in an existing team.");
    assert!(!description.contains("spawn_agent"));
    assert!(!description.contains("subagent"));
}

#[test]
fn claude_team_create_description_carries_team_workflow() {
    let ToolSpec::Function(function) = create_claude_team_create_tool() else {
        panic!("expected function tool");
    };
    let description = function.description;

    assert_eq!(description, CLAUDE_TEAM_CREATE_DESCRIPTION);
    assert!(description.contains("Task tools"));
    assert!(description.contains("spawn_agent"));
    assert!(description.contains("name"));
    assert!(description.contains("team_name"));
    assert!(description.contains("SendMessage"));
    assert!(description.contains("delivered automatically"));
    assert!(description.contains("queued"));
    assert!(!description.contains("subagent"));
}

#[test]
fn team_send_description_and_schema_follow_claude_send_message() {
    let ToolSpec::Function(function) = create_team_send_tool() else {
        panic!("expected function tool");
    };
    let properties = function
        .parameters
        .properties
        .expect("team_send should have parameters");
    let to_description = properties
        .get("to")
        .and_then(|schema| schema.description.as_deref())
        .expect("team_send.to should be described");
    let message_description = properties
        .get("message")
        .and_then(|schema| schema.description.as_deref())
        .expect("team_send.message should be described");

    assert_eq!(function.description, CODEX_TEAM_SEND_DESCRIPTION);
    assert!(to_description.contains("Recipient: teammate name"));
    assert!(to_description.contains("\"*\" for broadcast"));
    assert!(message_description.contains("plain text output is NOT visible"));
    assert!(message_description.contains("Refer to teammates by name, never by UUID"));
}

#[test]
fn claude_send_message_description_carries_visibility_and_delivery_rules() {
    let ToolSpec::Function(function) = create_claude_send_message_tool() else {
        panic!("expected function tool");
    };
    let properties = function
        .parameters
        .properties
        .expect("SendMessage should have parameters");
    let to_description = properties
        .get("to")
        .and_then(|schema| schema.description.as_deref())
        .expect("SendMessage.to should be described");
    let summary_description = properties
        .get("summary")
        .and_then(|schema| schema.description.as_deref())
        .expect("SendMessage.summary should be described");

    assert_eq!(function.description, CLAUDE_SEND_MESSAGE_DESCRIPTION);
    assert!(
        function
            .description
            .contains("Plain assistant text is not visible")
    );
    assert!(function.description.contains("bare teammate names"));
    assert!(function.description.contains("\"*\" for broadcast"));
    assert!(function.description.contains("delivered automatically"));
    assert!(function.description.contains("queued"));
    assert!(to_description.contains("Recipient: teammate name"));
    assert!(to_description.contains("team-lead"));
    assert!(to_description.contains("\"*\" for broadcast"));
    assert!(summary_description.contains("required when message is a string"));
}
