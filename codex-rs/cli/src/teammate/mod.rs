//! Hidden `codex teammate` subcommand: launches a FULL INTERACTIVE codex TUI in
//! "teammate mode" (Claude-Code Teams parity).
//!
//! The lead spawns one such process per teammate into a tmux / iTerm2 pane. The
//! teammate is the SAME interactive TUI as the lead — it boots its normal REPL
//! in the pane, but seeing the team-identity flags it enters teammate mode:
//! reads its on-disk inbox and injects the lead's messages as turns in its OWN
//! session (the first turn is delivered via the mailbox, NOT the command line).
//!
//! The inbox poll / idle / shutdown / mid-turn progress logic all lives in the
//! TUI (`tui/src/app/lead_inbox_poller.rs` + `tui/src/chatwidget/protocol.rs`);
//! an earlier headless run-loop was removed once the TUI became the live path.

use anyhow::Result;
use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_config::LoaderOverrides;
use codex_utils_cli::CliConfigOverrides;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

/// Hidden: run this `codex` process as a team member. The lead emits exactly
/// these flags when spawning a teammate process (the spawn contract).
#[derive(Debug, Parser)]
pub struct TeammateCommand {
    /// `"{name}@{team}"` — internal identity (`team_store::agent_id`).
    #[arg(long = "agent-id")]
    pub agent_id: String,

    /// Display name used for messaging/tasks and the inbox file.
    #[arg(long = "agent-name")]
    pub agent_name: String,

    /// Team this member belongs to.
    #[arg(long = "team-name")]
    pub team_name: String,

    /// Round-robin pane/pill color assigned by the lead.
    #[arg(long = "agent-color")]
    pub agent_color: Option<String>,

    /// Lead session UUID that spawned this teammate.
    #[arg(long = "parent-session-id")]
    pub parent_session_id: Option<String>,

    /// Optional agent role/type label.
    #[arg(long = "agent-type")]
    pub agent_type: Option<String>,

    /// Optional role customization layer (TOML) applied over this teammate's
    /// config at boot. The lead resolves the concrete path — `[teams.<name>]
    /// file` for startup members, or an agent role's `config_file` for
    /// tool-spawned teammates.
    #[arg(long = "agent-role-file")]
    pub agent_role_file: Option<std::path::PathBuf>,

    /// Start the teammate TUI in Plan mode and do not inherit bypass permissions.
    #[arg(long = "plan-mode-required", default_value_t = false)]
    pub plan_mode_required: bool,

    /// Run enabled hooks without requiring persisted hook trust for this teammate launch.
    #[arg(long = "dangerously-bypass-hook-trust", default_value_t = false)]
    pub bypass_hook_trust: bool,

    /// Teammate launch-mode hint (`auto` | `tmux` | `in-process`).
    #[arg(long = "teammate-mode")]
    pub teammate_mode: Option<String>,

    /// Initial prompt — reserved for parity; teammates receive their first turn
    /// via the mailbox, so this is intentionally NOT forwarded as `--prompt`.
    #[arg(long = "prompt")]
    pub prompt: Option<String>,

    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,
}

/// Launch the interactive codex TUI as this teammate.
pub async fn run_main(
    cmd: TeammateCommand,
    base_cli: codex_tui::Cli,
    arg0_paths: Arg0DispatchPaths,
    loader_overrides: LoaderOverrides,
) -> Result<()> {
    // Mark this process as a Codex Teams teammate FIRST so core team tools route
    // cross-process via the on-disk file mailbox (a teammate's in-memory team
    // registry is empty — it never ran `create_team`).
    codex_core::set_teammate_identity(cmd.team_name.clone(), cmd.agent_name.clone());

    let cli = build_teammate_tui_cli(&cmd, base_cli);

    codex_tui::run_main(cli, arg0_paths, loader_overrides, None).await?;
    Ok(())
}

fn build_teammate_tui_cli(cmd: &TeammateCommand, mut cli: codex_tui::Cli) -> codex_tui::Cli {
    // The first turn arrives via the mailbox (delivered by the lead after spawn),
    // never via the command line, so do not seed a prompt here.
    cli.prompt = None;
    cli.team_name = Some(cmd.team_name.clone());
    cli.agent_id = Some(cmd.agent_id.clone());
    cli.agent_name = Some(cmd.agent_name.clone());
    cli.agent_color = cmd.agent_color.clone();
    cli.parent_session_id = cmd.parent_session_id.clone();
    cli.agent_type = cmd.agent_type.clone();
    cli.plan_mode_required = cmd.plan_mode_required;
    cli.teammate_mode.clone_from(&cmd.teammate_mode);
    cli.bypass_hook_trust |= cmd.bypass_hook_trust;
    // Teams must be enabled for the tool set; approval/sandbox behavior comes
    // from the lead's forwarded `-c` overrides. Plan-mode teammates must not be
    // silently forced into bypass.
    cli.config_overrides
        .raw_overrides
        .push("features.teams=true".to_string());
    cli.config_overrides
        .raw_overrides
        .extend(cmd.config_overrides.raw_overrides.clone());
    // Role customization layer: flatten the role file into `-c` overrides at
    // session-flag precedence (mirroring how subagent role layers are applied).
    // Pushed LAST so keys the role file sets deliberately (e.g. `model`) win
    // over the lead's forwarded defaults. Best-effort: a broken file downgrades
    // this teammate to its default role instead of killing the pane.
    if let Some(role_file) = cmd.agent_role_file.as_ref() {
        match codex_core::load_teammate_role_overrides(role_file) {
            Ok(overrides) => cli.config_overrides.raw_overrides.extend(overrides),
            Err(err) => {
                #[allow(clippy::print_stderr)]
                {
                    eprintln!(
                        "warning: teammate role file {} not applied: {err}",
                        role_file.display()
                    );
                }
            }
        }
    }
    cli
}
