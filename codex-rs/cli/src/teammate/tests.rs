use super::*;
use clap::Parser;
use pretty_assertions::assert_eq;

#[test]
fn teammate_tui_cli_preserves_root_runtime_options() {
    let base_cli = codex_tui::Cli::parse_from([
        "codex",
        "-m",
        "gpt-5.5",
        "-p",
        "work",
        "-C",
        "/tmp/root",
        "--sandbox",
        "read-only",
        "--ask-for-approval",
        "never",
        "--search",
        "--no-alt-screen",
        "--dangerously-bypass-hook-trust",
        "stale prompt",
    ]);
    let cmd = TeammateCommand::parse_from([
        "teammate",
        "--agent-id",
        "alice@rocket",
        "--agent-name",
        "alice",
        "--team-name",
        "rocket",
        "--agent-color",
        "red",
        "--parent-session-id",
        "lead-session",
        "--agent-type",
        "reviewer",
        "--teammate-mode",
        "tmux",
        "--dangerously-bypass-hook-trust",
        "-c",
        r#"model_provider="custom""#,
    ]);

    let cli = build_teammate_tui_cli(&cmd, base_cli);

    assert_eq!(cli.prompt, None);
    assert_eq!(cli.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(cli.config_profile_v2.as_deref(), Some("work"));
    assert_eq!(cli.cwd.as_deref(), Some(std::path::Path::new("/tmp/root")));
    assert!(matches!(
        cli.sandbox_mode,
        Some(codex_utils_cli::SandboxModeCliArg::ReadOnly)
    ));
    assert!(matches!(
        cli.approval_policy,
        Some(codex_utils_cli::ApprovalModeCliArg::Never)
    ));
    assert!(cli.web_search);
    assert!(cli.no_alt_screen);
    assert!(cli.bypass_hook_trust);
    assert_eq!(cli.team_name.as_deref(), Some("rocket"));
    assert_eq!(cli.agent_id.as_deref(), Some("alice@rocket"));
    assert_eq!(cli.agent_name.as_deref(), Some("alice"));
    assert_eq!(cli.agent_color.as_deref(), Some("red"));
    assert_eq!(cli.parent_session_id.as_deref(), Some("lead-session"));
    assert_eq!(cli.agent_type.as_deref(), Some("reviewer"));
    assert_eq!(cli.teammate_mode.as_deref(), Some("tmux"));
    assert_eq!(
        cli.config_overrides.raw_overrides,
        vec![
            "features.teams=true".to_string(),
            r#"model_provider="custom""#.to_string(),
        ]
    );
}
