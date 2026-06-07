use super::*;

fn description(tool: ToolSpec) -> String {
    match tool {
        ToolSpec::Function(function) => function.description,
        ToolSpec::Namespace(_)
        | ToolSpec::ToolSearch { .. }
        | ToolSpec::ImageGeneration { .. }
        | ToolSpec::WebSearch { .. }
        | ToolSpec::Freeform(_) => panic!("expected function tool"),
    }
}

#[test]
fn create_team_description_separates_team_context_from_subagents() {
    let description = description(create_team_create_tool());

    assert!(description.contains("Codex Teams workspace and team context"));
    assert!(description.contains("does not spawn teammates or open panes"));
    assert!(description.contains("explicit Codex Teams product path"));
    assert!(!description.contains("spawn_agent"));
    assert!(!description.contains("subagent"));
}

#[test]
fn spawn_member_description_declares_named_teammate_split_pane_path() {
    let description = description(create_team_spawn_member_tool());

    assert!(description.contains("named Teams teammate"));
    assert!(description.contains("requires a team_id from create_team"));
    assert!(description.contains("requires a tmux/iTerm split-pane backend"));
    assert!(!description.contains("spawn_agent"));
    assert!(!description.contains("subagent"));
}
